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
        terminal::disable_raw_mode()?;
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
        terminal::enable_raw_mode()?;
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
        terminal::disable_raw_mode()?;
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

        terminal::enable_raw_mode()?;
        Ok(status)
    }

    /// Creates a zip archive and returns the path it was written to.
    pub fn zip(&self, items: Vec<PathBuf>) -> Result<PathBuf> {
        info!("Creating zip archive from {} files", items.len());
        let mut process = std::process::Command::new("zip");
        let archive_path = check_filename("output", ".", "zip")?;
        process.arg(archive_path.as_os_str());
        process.arg("--");
        for path in items.iter().flat_map(|p| p.file_name()) {
            process.arg(path);
        }
        process
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        let mut handle = process.spawn()?;
        handle.wait()?;
        Ok(archive_path)
    }

    /// Creates a tar.gz archive and returns the path it was written to.
    pub fn tar(&self, items: Vec<PathBuf>) -> Result<PathBuf> {
        info!("Creating tar.gz archive from {} files", items.len());
        let mut process = std::process::Command::new("tar");
        process.arg("-czf");
        let archive_path = check_filename("output", ".", "tar.gz")?;
        process.arg(archive_path.as_os_str());
        process.arg("--");
        for path in items.iter().flat_map(|p| p.file_name()) {
            process.arg(path);
        }
        process
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null());
        let mut handle = process.spawn()?;
        handle.wait()?;
        Ok(archive_path)
    }

    pub fn extract(&self, archive: PathBuf) -> Result<()> {
        info!("Extracting archive '{}'", archive.display());
        let extension = archive
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();

        let mime = mime_guess::from_ext(extension).first_or_text_plain();

        match (mime.type_().as_str(), mime.subtype().as_str()) {
            ("application", "gzip") => {
                std::process::Command::new("tar")
                    .arg("-xzf")
                    .arg(archive.as_os_str())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .stdin(std::process::Stdio::null())
                    .spawn()?
                    .wait()?;
            }
            ("application", "zip") => {
                std::process::Command::new("unzip")
                    .arg(archive.as_os_str())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .stdin(std::process::Stdio::null())
                    .spawn()?
                    .wait()?;
            }
            _ => {
                log::warn!("{} is not an archive", archive.display());
            }
        }
        Ok(())
    }
}
