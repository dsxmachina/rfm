use std::{
    io::{stdout, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

use crossterm::{
    cursor,
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};

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

        let mime_type = mime_guess::from_path(&absolute).first_or_text_plain();
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
