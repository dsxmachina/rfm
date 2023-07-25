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
use log::{debug, error, info};
use mime::Mime;
use serde::{Deserialize, Serialize};

/// Uses mime_guess to extract the mime-type.
///
/// However: There are a few exceptions,
/// where mime_guess is wrong, which is why we wrap the functionality here.
pub fn get_mime_type<P: AsRef<Path>>(path: P) -> Mime {
    let ext = path.as_ref().extension().and_then(|e| e.to_str());
    // Check the special extensions here
    match ext {
        Some("ts") => return mime::TEXT_PLAIN,
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
        let mut handle = Command::new(&self.name)
            .args(&self.args)
            .arg(path.as_ref())
            .spawn()?;
        if self.terminal {
            handle.wait()?;
        }
        Ok(())
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
                    error!("Unset config value for mime-type 'text'");
                }
            }
            "image" => {
                debug!("MIME-Type: Image");
                if let Some(engine) = &self.config.image {
                    engine.open(absolute)?;
                } else {
                    error!("Unset config value for mime-type 'image'");
                }
            }
            "audio" => {
                debug!("MIME-Type: Audio");
                if let Some(engine) = &self.config.audio {
                    engine.open(absolute)?;
                } else {
                    error!("Unset config value for mime-type 'audio'");
                }
            }
            "video" => {
                debug!("MIME-Type: Video");
                if let Some(engine) = &self.config.video {
                    engine.open(absolute)?;
                } else {
                    error!("Unset config value for mime-type 'video'");
                }
            }
            "application" => {
                debug!("MIME-Type: Application");
                if let Some(app) = &self.config.application {
                    app.open(absolute)?
                } else {
                    error!("Unset config value for mime-type 'application'");
                }
            }
            _ => {
                // Otherwise print error
                error!("Cannot open '{}' - unknown mime-type", absolute.display());
            }
        }

        // if let Some(ext) = absolute.extension().and_then(|ext| ext.to_str()) {
        //     match ext {
        //         "png" | "bmp" | "jpg" | "jpeg" | "svg" => {
        //             Command::new("sxiv")
        //                 .stderr(Stdio::null())
        //                 .stdin(Stdio::null())
        //                 .stdout(Stdio::null())
        //                 .arg(absolute.clone())
        //                 .spawn()?;
        //         }
        //         "wav" | "aiff" | "au" | "flac" | "m4a" | "mp3" | "opus" => {
        //             Command::new("mpv").arg(absolute).spawn()?.wait()?;
        //         }
        //         "pdf" => {
        //             Command::new("zathura").arg(absolute).spawn()?;
        //         }
        //         _ => {
        //             // Everything else with vim
        //             Command::new("nvim").arg(absolute).spawn()?.wait()?;
        //         }
        //     }
        // } else {
        //     // Try to open things without extensions with vim
        //     Command::new("nvim").arg(absolute).spawn()?.wait()?;
        // }
        terminal::enable_raw_mode()?;
        Ok(())
    }
}
