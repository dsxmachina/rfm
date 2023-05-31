use std::{
    io::{stdout, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crossterm::{
    cursor,
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use mime::Mime;

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

#[derive(Default)]
pub struct OpenEngine {}

impl OpenEngine {
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
            "image" => {
                Command::new("sxiv")
                    .stderr(Stdio::null())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .arg(absolute.clone())
                    .spawn()?;
            }
            "audio" | "video" => {
                Command::new("mpv").arg(absolute).spawn()?.wait()?;
            }
            "application" => match mime_type.subtype().as_str() {
                "pdf" => {
                    Command::new("zathura").arg(absolute).spawn()?;
                }
                _ => {
                    Command::new("nvim").arg(absolute).spawn()?.wait()?;
                }
            },
            _ => {
                // Everything else with vim
                Command::new("nvim").arg(absolute).spawn()?.wait()?;
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
