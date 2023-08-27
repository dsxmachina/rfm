use std::{
    fs::File,
    io::{self, BufRead, Stdout},
    ops::Range,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::util::ExactWidth;

use super::{BasePanel, DirPanel, Draw, PanelContent};
use crossterm::{
    cursor, queue,
    style::{self, Colors, Print, PrintStyledContent, ResetColor, SetColors, Stylize},
    Result,
};
use image::{DynamicImage, GenericImageView};

#[derive(Debug, Clone)]
pub enum Preview {
    Image { img: Option<DynamicImage> },
    Text { lines: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct FilePreview {
    path: PathBuf,
    modified: SystemTime,
    preview: Preview,
}

impl Draw for FilePreview {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start.saturating_add(1));
        let height = y_range.end.saturating_sub(y_range.start);

        // Plot left border
        for y in y_range.start + 1..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("│".dark_green().bold()),
            )?;
        }

        match &self.preview {
            Preview::Image { img } => {
                // load image
                if let Some(img) = img {
                    // crop height
                    // let img_height = ((height as f32) - (height as f32) / 3.6).round();
                    let aspect_ratio = (img.height() as f32) / (img.width() as f32);
                    let img_height = ((width as f32) * aspect_ratio).round();
                    let img = img
                        .thumbnail_exact(width as u32, img_height as u32)
                        .into_rgb8();
                    let mut cy = y_range.start;
                    for y in (0..img_height as usize).step_by(2) {
                        for x in 0..width {
                            // cursor x
                            let cx = x_range.start.saturating_add(x).saturating_add(1);
                            queue!(stdout, cursor::MoveTo(cx, cy))?;
                            let px_hi = unsafe { img.unsafe_get_pixel(x as u32, y as u32) };
                            if let Some(px_lo) = img.get_pixel_checked(x as u32, (y + 1) as u32) {
                                let color = Colors::new(
                                    style::Color::Rgb {
                                        r: px_lo.0[0],
                                        g: px_lo.0[1],
                                        b: px_lo.0[2],
                                    },
                                    style::Color::Rgb {
                                        r: px_hi.0[0],
                                        g: px_hi.0[1],
                                        b: px_hi.0[2],
                                    },
                                );
                                queue!(stdout, SetColors(color), Print("▄"),)?;
                            } else {
                                queue!(stdout, cursor::MoveTo(cx, cy), ResetColor, Print(" "),)?;
                            }
                        }
                        queue!(stdout, cursor::MoveTo(0, 0), ResetColor,)?;
                        // Increase column
                        cy += 1;
                    }
                } else {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start + 1, y_range.start + 1),
                        Print(format!("Failed to load image '{}'", self.path().display())),
                    )?;
                    for y in y_range.start + 1..y_range.end {
                        for x in x_range.start + 1..x_range.end {
                            queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
                        }
                    }
                }

                // let density =
                //     "$@B%8&WM#*oahkbdpqwmZO0QLCJUYXzcvunxrjft/\\|()1{}[]?-_+~<>i!lI;:,\"^`\'. ";
            }
            Preview::Text { lines } => {
                // Print preview
                let mut idx = 0;
                for line in lines.iter().take(height as usize) {
                    let cy = idx + y_range.start;
                    let line = line
                        // .replace('\r', "")
                        .exact_width(width.saturating_sub(1) as usize);
                    queue!(stdout, cursor::MoveTo(x_range.start + 1, cy), Print(" "),)?;
                    for (i, c) in line.escape_default().enumerate() {
                        queue!(
                            stdout,
                            cursor::MoveTo(x_range.start + 2 + i as u16, cy),
                            // Print(" "),
                            Print(c),
                        )?;
                    }
                    idx += 1;
                }
                for cy in idx + 1..y_range.end {
                    for cx in x_range.start + 1..x_range.end {
                        queue!(stdout, cursor::MoveTo(cx, cy), Print(" "),)?;
                    }
                }
            }
        }
        Ok(())
    }
}

impl FilePreview {
    pub fn new(path: PathBuf) -> Self {
        let extension = path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();

        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or_else(SystemTime::now);

        let preview = match extension.as_str() {
            "png" | "bmp" | "jpg" | "jpeg" => {
                if let Ok(img_bytes) = image::io::Reader::open(&path) {
                    let img = img_bytes.decode().ok();
                    Preview::Image { img }
                } else {
                    Preview::Image { img: None }
                }
            }
            "wav" | "aiff" | "au" | "flac" | "m4a" | "mp3" | "opus" | "mov" | "pdf" | "doc"
            | "docx" | "ppt" | "pptx" | "xls" | "xlsx" | "zip" => {
                let lines = match std::process::Command::new("mediainfo").arg(&path).output() {
                    Ok(output) => output.stdout.lines().take(128).flatten().collect(),
                    Err(e) => {
                        vec![
                            "Error: Could not run mediainfo".to_string(),
                            e.to_string(),
                            "".to_string(),
                            "You must have mediainfo installed to get a preview for this file-type.".to_string(),
                        ]
                    }
                };
                Preview::Text { lines }
            }
            // "tar" | "tar.gz" | ".gz" => {
            //     let output = std::process::Command::new("tar")
            //         .arg("--list")
            //         .arg("-f")
            //         .arg(&path)
            //         .output()
            //         .expect("failed to run tar");
            //     let lines: Vec<String> = output.stdout.lines().take(128).flatten().collect();
            //     Preview::Text { lines }
            // }
            _ => {
                // Simple method
                if let Ok(file) = File::open(&path) {
                    let lines = io::BufReader::new(file)
                        .lines()
                        .take(128)
                        .flatten()
                        .collect();
                    Preview::Text { lines }
                } else {
                    Preview::Text { lines: Vec::new() }
                }
            }
        };

        FilePreview {
            path,
            modified,
            preview,
        }
    }
}

impl PanelContent for FilePreview {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn modified(&self) -> SystemTime {
        self.modified
    }

    fn update_content(&mut self, content: Self) {
        *self = content
    }
}
#[derive(Debug, Clone)]
pub enum PreviewPanel {
    /// Directory preview
    Dir(DirPanel),
    /// File preview
    File(FilePreview),
    /// Empty panel
    Empty,
}

impl Draw for PreviewPanel {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        match self {
            PreviewPanel::Dir(panel) => panel.draw(stdout, x_range, y_range),
            PreviewPanel::File(preview) => preview.draw(stdout, x_range, y_range),
            PreviewPanel::Empty => {
                // Draw empty panel
                for y in y_range {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start, y),
                        PrintStyledContent("│".dark_green().bold()),
                    )?;
                    for x in x_range.start + 1..x_range.end {
                        queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl PanelContent for PreviewPanel {
    fn path(&self) -> &Path {
        match self {
            PreviewPanel::Dir(panel) => panel.path(),
            PreviewPanel::File(preview) => preview.path(),
            PreviewPanel::Empty => Path::new("path-of-empty-panel"),
        }
    }

    fn modified(&self) -> SystemTime {
        match self {
            PreviewPanel::Dir(p) => p.modified(),
            PreviewPanel::File(p) => p.modified(),
            PreviewPanel::Empty => UNIX_EPOCH,
        }
    }

    fn update_content(&mut self, content: Self) {
        *self = content;
    }
}

impl BasePanel for PreviewPanel {
    fn empty() -> Self {
        PreviewPanel::Empty
    }

    fn loading(path: PathBuf) -> Self {
        PreviewPanel::Dir(DirPanel::loading(path))
    }

    fn from_path(path: PathBuf) -> Self {
        if path.is_dir() {
            PreviewPanel::Dir(DirPanel::from_path(path))
        } else if path.is_file() {
            PreviewPanel::File(FilePreview::new(path))
        } else {
            PreviewPanel::Empty
        }
    }
}

impl PreviewPanel {
    pub fn maybe_path(&self) -> Option<PathBuf> {
        match self {
            PreviewPanel::Dir(panel) => Some(panel.path().to_path_buf()),
            PreviewPanel::File(panel) => Some(panel.path().to_path_buf()),
            PreviewPanel::Empty => None,
        }
    }
}
