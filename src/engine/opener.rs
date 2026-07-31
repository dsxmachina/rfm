use std::{
    collections::HashMap,
    io::{stdout, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::SystemTime,
};

use crossterm::{
    cursor,
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use log::{debug, info, warn};
use mime::Mime;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::util::check_filename;

/// Uses mime_guess to extract the mime-type.
///
/// However: There are a few exceptions,
/// where mime_guess is wrong, which is why we wrap the functionality here.
/// When the extension has no real answer (none at all, or mime_guess
/// falls back to octet-stream), the content is sniffed instead.
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
        // mime_guess has no mapping for zstd and the compound tar
        // extensions (.xz/.bz2/.7z it does know); resolving them here
        // saves the content sniff.
        Some("zst" | "tzst") => return "application/zstd".parse().unwrap(),
        Some("txz") => return "application/x-xz".parse().unwrap(),
        Some("tbz2") => return "application/x-bzip2".parse().unwrap(),
        // No extension: the guess has nothing to work with - sniff.
        None => return sniffed(path.as_ref()).unwrap_or(mime::TEXT_PLAIN),
        _ => (),
    }
    // Otherwise just use mime_guess; sniff only when it has no real answer.
    match mime_guess::from_path(&path).first() {
        Some(mime) if mime != mime::APPLICATION_OCTET_STREAM => mime,
        _ => sniffed(path.as_ref()).unwrap_or(mime::TEXT_PLAIN),
    }
}

/// One bounded 512-byte read (enough to cover the `ustar` magic at
/// offset 257); any error skips the sniff and never fails the caller.
/// Only reached on the extension-fallback path.
///
/// Two hard requirements from the draw path (per-entry styling and the
/// footer sniff on every repaint):
/// - Never open a non-regular file: open() on a FIFO blocks until a
///   writer appears (one named pipe in a browsed directory would wedge
///   the whole UI), and opening device nodes can block or have side
///   effects. `metadata()` follows symlinks, so a symlink to a regular
///   file still gets sniffed.
/// - Cache the result keyed on mtime, so a directory full of
///   extensionless files costs one read per file, not one per repaint.
fn sniffed(path: &Path) -> Option<Mime> {
    use std::io::Read;
    /// path -> (mtime at sniff time, sniff result - `None` is cached too).
    type SniffCache = HashMap<PathBuf, (Option<SystemTime>, Option<Mime>)>;
    let meta = path.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok();
    static CACHE: OnceCell<Mutex<SniffCache>> = OnceCell::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((cached_mtime, mime)) = cache.lock().unwrap().get(path) {
        if *cached_mtime == mtime {
            return mime.clone();
        }
    }
    let mut head = [0u8; 512];
    let mut file = std::fs::File::open(path).ok()?;
    let n = file.read(&mut head).ok()?;
    let mime = sniff_mime(&head[..n]);
    let mut cache = cache.lock().unwrap();
    // Crude but sufficient bound: a full clear every ~4k distinct paths
    // beats an unbounded map, and re-sniffing is cheap.
    if cache.len() >= 4096 {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (mtime, mime.clone()));
    mime
}

/// Content sniff over the first bytes: shebang, then magic numbers
/// (infer: pdf/zip/gzip/sqlite/elf/png/jpeg/gif/tar/...), then a
/// mostly-printable-UTF-8 heuristic. `None` means "no idea" - the
/// caller keeps today's text/plain fallback.
fn sniff_mime(head: &[u8]) -> Option<Mime> {
    if head.is_empty() {
        return None;
    }
    if head.starts_with(b"#!") {
        return Some(mime::TEXT_PLAIN);
    }
    if let Some(kind) = infer::get(head) {
        return kind.mime_type().parse().ok();
    }
    // A bounded window may cut a multi-byte char: only judge the valid
    // prefix. Printable = no control bytes besides \n, \r, \t.
    let valid = match std::str::from_utf8(head) {
        Ok(s) => s,
        Err(e) if e.valid_up_to() > 0 => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap(),
        Err(_) => return None,
    };
    let printable = valid
        .chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'));
    printable.then_some(mime::TEXT_PLAIN)
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
        require_binary(
            "zip",
            "zip is not installed - install it to create zip archives",
        )?;
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
        require_binary(
            "tar",
            "tar is not installed - install it to create tar archives",
        )?;
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
                require_binary(
                    "tar",
                    "tar is not installed - install it to extract tar archives",
                )?;
                let mut process = std::process::Command::new("tar");
                process
                    .current_dir(dir)
                    .arg("-xzf")
                    .arg(archive.as_os_str());
                run_archive_tool(&mut process, "tar", None)?;
            }
            ("application", "zip") => {
                // NOTE: the zip crate (already a dependency for previews)
                // could take over extraction here.
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
fn run_archive_tool(process: &mut Command, tool: &str, archive_path: Option<&Path>) -> Result<()> {
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
mod mime_tests {
    use super::*;

    #[test]
    fn sniff_mime_detects_a_shebang_as_text() {
        assert_eq!(sniff_mime(b"#!/bin/sh\necho hi\n"), Some(mime::TEXT_PLAIN));
    }

    #[test]
    fn sniff_mime_detects_pdf_zip_and_gzip_magic() {
        assert_eq!(
            sniff_mime(b"%PDF-1.4 rest").unwrap().to_string(),
            "application/pdf"
        );
        assert_eq!(
            sniff_mime(b"PK\x03\x04rest").unwrap().to_string(),
            "application/zip"
        );
        assert_eq!(
            sniff_mime(b"\x1f\x8b\x08rest").unwrap().to_string(),
            "application/gzip"
        );
    }

    #[test]
    fn sniff_mime_treats_mostly_printable_utf8_as_text() {
        assert_eq!(
            sniff_mime("kein shebang, nur Text — ümlaute ok\n".as_bytes()),
            Some(mime::TEXT_PLAIN)
        );
    }

    #[test]
    fn sniff_mime_judges_only_the_valid_prefix_of_a_cut_utf8_char() {
        // A sniff window may cut a multi-byte char in half; the valid
        // prefix alone must still count as text.
        let mut head = "nur Text bis zum Schnitt: ü".as_bytes().to_vec();
        head.pop(); // cut the two-byte ü in half
        assert_eq!(sniff_mime(&head), Some(mime::TEXT_PLAIN));
    }

    #[test]
    fn sniff_mime_returns_none_for_random_binary() {
        assert_eq!(sniff_mime(&[0x00, 0x01, 0x02, 0xfe, 0xff, 0x00]), None);
    }

    #[test]
    fn sniff_mime_returns_none_for_an_empty_head() {
        assert_eq!(sniff_mime(b""), None);
    }

    #[test]
    fn extensionless_shebang_script_gets_text_plain() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("deploy script"); // space, no extension
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        assert_eq!(get_mime_type(&script), mime::TEXT_PLAIN);
    }

    #[test]
    fn extensionless_pdf_bytes_get_application_pdf() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("report");
        std::fs::write(&pdf, b"%PDF-1.4\n%fake body").unwrap();
        assert_eq!(get_mime_type(&pdf).to_string(), "application/pdf");
    }

    #[test]
    fn an_unknown_binary_extension_gets_sniffed_too() {
        // mime_guess has no answer for ".blob0815" - the octet-stream
        // fallback path must sniff the content as well.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("report.blob0815");
        std::fs::write(&pdf, b"%PDF-1.4\n%fake body").unwrap();
        assert_eq!(get_mime_type(&pdf).to_string(), "application/pdf");
    }

    #[test]
    fn a_txt_file_with_zip_magic_keeps_its_extension_mime() {
        // The sniff must only run on the fallback path - a known
        // extension wins even when the content lies.
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notes.txt");
        std::fs::write(&file, b"PK\x03\x04 pretending to be a zip").unwrap();
        assert_eq!(get_mime_type(&file), mime::TEXT_PLAIN);
    }

    #[test]
    fn an_unreadable_extensionless_path_falls_back_to_text_plain() {
        // Sniff read errors must never fail the caller.
        assert_eq!(get_mime_type(Path::new("/no/such/file")), mime::TEXT_PLAIN);
    }

    #[test]
    fn jxl_extension_resolves_to_image_jxl() {
        // Pins the .jxl -> image-arm routing. No special-case exists
        // (or is needed): mime_guess 2.0.5 already maps jxl to
        // image/jxl in its registry — this test guards against a
        // future mime_guess bump losing the mapping. Nonexistent path
        // proves no content was read.
        assert_eq!(
            get_mime_type(Path::new("/no/such/photo.jxl")).to_string(),
            "image/jxl"
        );
    }

    #[test]
    fn zst_extensions_resolve_without_sniffing() {
        // mime_guess has no mapping for zstd and the compound tar
        // extensions; without the special-cases these fall into the
        // sniff path. Nonexistent paths prove no content was read (the
        // sniff fallback would yield text/plain).
        assert_eq!(
            get_mime_type(Path::new("/no/such/foo.tar.zst")).to_string(),
            "application/zstd"
        );
        assert_eq!(
            get_mime_type(Path::new("/no/such/foo.tzst")).to_string(),
            "application/zstd"
        );
        assert_eq!(
            get_mime_type(Path::new("/no/such/foo.txz")).to_string(),
            "application/x-xz"
        );
        assert_eq!(
            get_mime_type(Path::new("/no/such/foo.tbz2")).to_string(),
            "application/x-bzip2"
        );
    }

    #[test]
    fn a_fifo_is_never_opened_by_the_sniff() {
        // The sniff runs on the draw path (per-entry styling + footer);
        // opening a FIFO for reading blocks until a writer appears, so a
        // single named pipe in a browsed directory would wedge the whole
        // UI. Non-regular files must be skipped without any open().
        let tmp = tempfile::tempdir().unwrap();
        let fifo = tmp.path().join("a named pipe"); // extensionless -> sniff path
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "creating the fixture fifo failed");
        // Would block forever without the regular-file guard.
        assert_eq!(get_mime_type(&fifo), mime::TEXT_PLAIN);
    }
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
