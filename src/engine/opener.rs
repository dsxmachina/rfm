use std::{
    io::{stdout, Write},
    path::{Path, PathBuf},
    process::Command,
};

use crossterm::{
    cursor,
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use log::{debug, info, warn};
use mime::Mime;
use serde::{Deserialize, Serialize};

use crate::util::check_filename;

/// Uses mime_guess to extract the mime-type.
///
/// However: There are a few exceptions,
/// where mime_guess is wrong, which is why we wrap the functionality here.
pub fn get_mime_type<P: AsRef<Path>>(path: P) -> Mime {
    let ext = path.as_ref().extension().and_then(|e| e.to_str());
    // Check the special extensions here (for types that mime_guess doesn't handle correctly)
    match ext {
        Some("ts") => return mime::TEXT_JAVASCRIPT,
        Some("nix") => return "text/x-nix".parse().unwrap(),
        Some("vue") => return "text/x-vue".parse().unwrap(),
        Some("svelte") => return "text/x-svelte".parse().unwrap(),
        Some("graphql" | "gql") => return "text/x-graphql".parse().unwrap(),
        Some("proto") => return "text/x-protobuf".parse().unwrap(),
        Some("sol") => return "text/x-solidity".parse().unwrap(),
        Some("astro") => return "text/x-astro".parse().unwrap(),
        Some("gradle") => return "text/x-gradle".parse().unwrap(),
        Some("groovy") => return "text/x-groovy".parse().unwrap(),
        None => return mime::TEXT_PLAIN,
        _ => (),
    }
    // Otherwise just use mime_guess
    mime_guess::from_path(path).first_or_text_plain()
}

/// Leaves raw mode for the lifetime of the guard and restores it on drop —
/// so every early `?` return of the open functions (e.g. a configured opener
/// binary that is not installed) puts the terminal back instead of leaving
/// the whole TUI without raw mode.
struct RawModeGuard;

impl RawModeGuard {
    fn suspend() -> Result<RawModeGuard> {
        terminal::disable_raw_mode()?;
        Ok(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        // Best effort: there is nothing left to do about a failure here.
        let _ = terminal::enable_raw_mode();
        let _ = crossterm::execute!(stdout(), terminal::DisableLineWrap);
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Application {
    name: String,
    terminal: bool,
    args: Vec<String>,
}

impl Application {
    pub fn open<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        info!("Opening '{}' with '{}'", path.as_ref().display(), self.name);
        if self.terminal {
            stdout().queue(terminal::EnableLineWrap)?.flush()?;
        }
        let mut handle = Command::new(&self.name)
            .args(&self.args)
            .arg(path.as_ref())
            .spawn()?;
        if self.terminal {
            handle.wait()?;
            stdout().queue(terminal::DisableLineWrap)?.flush()?;
        } else {
            // GUI apps run detached: reap the child in the background so it
            // doesn't linger as a zombie once it exits.
            std::thread::spawn(move || {
                let _ = handle.wait();
            });
        }
        Ok(())
    }

    /// Opens a file and always waits for the process to complete.
    /// Used for operations that need to wait for the editor result (like bulkrename).
    pub fn open_blocking<P: AsRef<Path>>(&self, path: P) -> Result<std::process::ExitStatus> {
        info!(
            "Opening '{}' with '{}' (blocking)",
            path.as_ref().display(),
            self.name
        );
        stdout().queue(terminal::EnableLineWrap)?.flush()?;
        let mut handle = Command::new(&self.name)
            .args(&self.args)
            .arg(path.as_ref())
            .spawn()?;
        let status = handle.wait()?;
        stdout().queue(terminal::DisableLineWrap)?.flush()?;
        Ok(status)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenOptions {
    default: Application,
    extensions: Option<Vec<(String, Application)>>,
}

impl OpenOptions {
    pub fn open(&self, absolute: PathBuf) -> Result<()> {
        if let Some(ext_list) = &self.extensions {
            info!("checking extensions: {:?}", ext_list);
            let path_extension = absolute.extension().and_then(|s| s.to_str());
            for (ext, application) in ext_list.iter() {
                if Some(ext.as_str()) == path_extension {
                    return application.open(&absolute);
                }
            }
        }
        self.default.open(absolute)
    }

    pub fn open_blocking(&self, absolute: PathBuf) -> Result<std::process::ExitStatus> {
        if let Some(ext_list) = &self.extensions {
            info!("checking extensions (blocking): {:?}", ext_list);
            let path_extension = absolute.extension().and_then(|s| s.to_str());
            for (ext, application) in ext_list.iter() {
                if Some(ext.as_str()) == path_extension {
                    return application.open_blocking(&absolute);
                }
            }
        }
        self.default.open_blocking(absolute)
    }
}

// #[derive(Debug, Default, Clone, Serialize, Deserialize)]
// pub struct Applications(HashMap<String, Application>);

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct OpenerConfig {
    application: Option<OpenOptions>,
    audio: Option<OpenOptions>,
    video: Option<OpenOptions>,
    image: Option<OpenOptions>,
    text: Option<OpenOptions>,
}

#[derive(Default)]
pub struct OpenEngine {
    config: OpenerConfig,
}

impl OpenEngine {
    pub fn with_config(config: OpenerConfig) -> Self {
        OpenEngine { config }
    }

    pub fn open(&self, path: PathBuf) -> Result<()> {
        let absolute = if path.is_absolute() {
            path
        } else {
            path.canonicalize().unwrap_or_default()
        };
        let _raw_mode = RawModeGuard::suspend()?;
        let mut stdout = stdout();
        stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?;
        stdout.flush()?;

        // Check mime-type
        let mime_type = get_mime_type(&absolute);
        match mime_type.type_().as_str() {
            "text" => {
                debug!("MIME-Type: Text");
                if let Some(engine) = &self.config.text {
                    engine.open(absolute)?;
                } else {
                    info!("Unset config value for mime-type 'text', using default opener");
                    if let Err(e) = opener::open(&absolute) {
                        warn!("Error while opening {}: {e}", absolute.display());
                    }
                }
            }
            "image" => {
                debug!("MIME-Type: Image");
                if let Some(engine) = &self.config.image {
                    engine.open(absolute)?;
                } else {
                    info!("Unset config value for mime-type 'image', using default opener");
                    if let Err(e) = opener::open(&absolute) {
                        warn!("Error while opening {}: {e}", absolute.display());
                    }
                }
            }
            "audio" => {
                debug!("MIME-Type: Audio");
                if let Some(engine) = &self.config.audio {
                    engine.open(absolute)?;
                } else {
                    info!("Unset config value for mime-type 'audio', using default opener");
                    if let Err(e) = opener::open(&absolute) {
                        warn!("Error while opening {}: {e}", absolute.display());
                    }
                }
            }
            "video" => {
                debug!("MIME-Type: Video");
                if let Some(engine) = &self.config.video {
                    engine.open(absolute)?;
                } else {
                    info!("Unset config value for mime-type 'video', using default opener");
                    if let Err(e) = opener::open(&absolute) {
                        warn!("Error while opening {}: {e}", absolute.display());
                    }
                }
            }
            "application" => {
                debug!("MIME-Type: Application");
                if let Some(app) = &self.config.application {
                    app.open(absolute)?
                } else {
                    info!("Unset config value for mime-type 'application', using default opener");
                    if let Err(e) = opener::open(&absolute) {
                        warn!("Error while opening {}: {e}", absolute.display());
                    }
                }
            }
            _ => {
                // Otherwise print error
                info!(
                    "unknown mime-type for {}, trying to use default opener",
                    absolute.display()
                );
                if let Err(e) = opener::open(&absolute) {
                    warn!("Error while opening {}: {e}", absolute.display());
                }
            }
        }
        Ok(())
    }

    /// Opens a text file and blocks until the editor is closed.
    /// Used for operations that need to wait for the editor result (like bulkrename).
    /// Falls back to $EDITOR or vi if no text opener is configured.
    pub fn open_text_blocking(&self, path: PathBuf) -> Result<std::process::ExitStatus> {
        let absolute = if path.is_absolute() {
            path
        } else {
            path.canonicalize().unwrap_or_default()
        };
        let _raw_mode = RawModeGuard::suspend()?;
        let mut stdout = stdout();
        stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?;
        stdout.flush()?;

        let status = if let Some(engine) = &self.config.text {
            engine.open_blocking(absolute)?
        } else {
            // Fallback to $EDITOR or vi
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            info!("No text opener configured, using {}", editor);
            stdout.queue(terminal::EnableLineWrap)?.flush()?;
            let mut handle = Command::new(&editor).arg(&absolute).spawn()?;
            let status = handle.wait()?;
            stdout.queue(terminal::DisableLineWrap)?.flush()?;
            status
        };

        Ok(status)
    }

    /// Creates a zip archive in `dir` and returns the path it was written to.
    pub fn zip(&self, items: Vec<PathBuf>, dir: &Path) -> Result<PathBuf> {
        info!("Creating zip archive from {} files", items.len());
        require_binary("zip", "zip is not installed - install it to create zip archives")?;
        let mut process = std::process::Command::new("zip");
        let archive_path = check_filename("output", dir, "zip")?;
        process.current_dir(dir);
        process.arg(archive_path.as_os_str());
        process.arg("--");
        for path in items.iter().flat_map(|p| p.file_name()) {
            process.arg(path);
        }
        run_archive_tool(&mut process, "zip", Some(&archive_path))?;
        Ok(archive_path)
    }

    /// Creates a tar.gz archive in `dir` and returns the path it was written to.
    pub fn tar(&self, items: Vec<PathBuf>, dir: &Path) -> Result<PathBuf> {
        info!("Creating tar.gz archive from {} files", items.len());
        require_binary("tar", "tar is not installed - install it to create tar archives")?;
        let mut process = std::process::Command::new("tar");
        process.current_dir(dir);
        process.arg("-czf");
        let archive_path = check_filename("output", dir, "tar.gz")?;
        process.arg(archive_path.as_os_str());
        process.arg("--");
        for path in items.iter().flat_map(|p| p.file_name()) {
            process.arg(path);
        }
        run_archive_tool(&mut process, "tar", Some(&archive_path))?;
        Ok(archive_path)
    }

    /// Extracts `archive` into `dir`.
    pub fn extract(&self, archive: PathBuf, dir: &Path) -> Result<()> {
        info!("Extracting archive '{}'", archive.display());
        let extension = archive
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();

        let mime = mime_guess::from_ext(extension).first_or_text_plain();

        match (mime.type_().as_str(), mime.subtype().as_str()) {
            ("application", "gzip") => {
                require_binary("tar", "tar is not installed - install it to extract tar archives")?;
                let mut process = std::process::Command::new("tar");
                process
                    .current_dir(dir)
                    .arg("-xzf")
                    .arg(archive.as_os_str());
                run_archive_tool(&mut process, "tar", None)?;
            }
            ("application", "zip") => {
                require_binary(
                    "unzip",
                    "unzip is not installed - install it to extract zip archives",
                )?;
                let mut process = std::process::Command::new("unzip");
                process.current_dir(dir).arg(archive.as_os_str());
                run_archive_tool(&mut process, "unzip", None)?;
            }
            _ => {
                log::warn!("{} is not an archive", archive.display());
            }
        }
        Ok(())
    }
}

/// Errors with `msg` (NotFound) when `name` is not an executable on PATH,
/// so a missing archiver surfaces as a readable message instead of a raw
/// spawn error.
fn require_binary(name: &str, msg: &str) -> Result<()> {
    if crate::util::binary_on_path(name) {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, msg))
    }
}

/// Runs a prepared archiver invocation and checks its exit status. On
/// failure the partially written `archive_path` (if any) is removed and the
/// exit code plus the first lines of stderr are surfaced in the error.
fn run_archive_tool(
    process: &mut Command,
    tool: &str,
    archive_path: Option<&Path>,
) -> Result<()> {
    let output = process.stdin(std::process::Stdio::null()).output()?;
    if output.status.success() {
        return Ok(());
    }
    // tar & friends may leave a partial archive behind on failure; the path
    // came fresh from check_filename, so removing it can't hit user data.
    if let Some(path) = archive_path {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.lines().take(3).collect::<Vec<_>>().join(" | ");
    Err(std::io::Error::new(
        std::io::ErrorKind::Other,
        format!("{tool} failed ({}): {stderr}", output.status),
    ))
}

#[cfg(test)]
mod archive_tests {
    use super::*;

    fn fixture(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let tmp = tempfile::tempdir().unwrap();
        let items = names
            .iter()
            .map(|name| {
                let path = tmp.path().join(name);
                std::fs::write(&path, format!("content of {name}")).unwrap();
                path
            })
            .collect();
        (tmp, items)
    }

    #[test]
    fn tar_creates_an_archive_in_the_target_dir() {
        let (tmp, items) = fixture(&["a.txt", "b.txt"]);
        let archive = OpenEngine::default().tar(items, tmp.path()).unwrap();
        assert!(archive.is_file());
        assert_eq!(archive.parent(), Some(tmp.path()));
    }

    #[test]
    fn tar_failure_reports_err_and_leaves_no_archive_behind() {
        let (tmp, mut items) = fixture(&["a.txt"]);
        items.push(tmp.path().join("missing.txt"));
        // tar exits non-zero on the missing member but still writes the
        // archive — the partial file must be cleaned up.
        let result = OpenEngine::default().tar(items, tmp.path());
        assert!(result.is_err());
        assert!(!tmp.path().join("output.tar.gz").exists());
    }

    #[test]
    fn zip_creates_an_archive_in_the_target_dir() {
        let (tmp, items) = fixture(&["a.txt", "b.txt"]);
        let archive = OpenEngine::default().zip(items, tmp.path()).unwrap();
        assert!(archive.is_file());
        assert_eq!(archive.parent(), Some(tmp.path()));
    }

    #[test]
    fn zip_failure_reports_err_and_leaves_no_archive_behind() {
        // zip exits 0 when at least one item matches, so only an
        // all-missing item list produces a failure ("nothing to do").
        let (tmp, _) = fixture(&[]);
        let items = vec![tmp.path().join("missing.txt")];
        let result = OpenEngine::default().zip(items, tmp.path());
        assert!(result.is_err());
        assert!(!tmp.path().join("output.zip").exists());
    }

    #[test]
    fn extract_roundtrips_a_tar_archive() {
        let (tmp, items) = fixture(&["a.txt", "b.txt"]);
        let engine = OpenEngine::default();
        let archive = engine.tar(items, tmp.path()).unwrap();

        let out = tempfile::tempdir().unwrap();
        engine.extract(archive, out.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.path().join("a.txt")).unwrap(),
            "content of a.txt"
        );
        assert_eq!(
            std::fs::read_to_string(out.path().join("b.txt")).unwrap(),
            "content of b.txt"
        );
    }
}
