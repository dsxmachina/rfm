use std::{
    fs::File,
    io::{self, BufRead, Stdout},
    ops::Range,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use crossterm::{
    cursor, queue,
    style::{self, Colors, Print, PrintStyledContent, ResetColor, SetColors, Stylize},
    Result,
};
use image::DynamicImage;
use pad::PadStr;

use super::{BasePanel, DirPanel, Draw};

#[derive(Debug, Clone)]
pub enum Preview {
    Image { img: Option<DynamicImage> },
    Text { lines: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct FilePreview {
    path: PathBuf,
    hash: u64,
    preview: Preview,
}

impl Draw for FilePreview {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
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
                    let img_height = ((height as f32) - (height as f32) / 3.6).round();
                    let img = img
                        .thumbnail_exact(width as u32, img_height as u32)
                        .into_rgb8();
                    for y in 0..height {
                        // cursor y
                        let cy = y_range.start.saturating_add(y);
                        for x in 0..width {
                            // cursor x
                            let cx = x_range.start.saturating_add(x).saturating_add(1);
                            queue!(stdout, cursor::MoveTo(cx, cy))?;
                            if let Some(px) = img.get_pixel_checked(x as u32, y as u32) {
                                let color = Colors::new(
                                    style::Color::Rgb {
                                        r: px.0[0],
                                        g: px.0[1],
                                        b: px.0[2],
                                    },
                                    style::Color::Rgb {
                                        r: px.0[0],
                                        g: px.0[1],
                                        b: px.0[2],
                                    },
                                );
                                queue!(stdout, SetColors(color), Print(" "),)?;
                            } else {
                                queue!(stdout, cursor::MoveTo(cx, cy), ResetColor, Print(" "),)?;
                            }
                        }
                        queue!(stdout, cursor::MoveTo(0, 0), ResetColor,)?;
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
                for line in lines.into_iter().take(height as usize) {
                    let cy = idx as u16 + y_range.start;
                    let line = line.replace("\r", "").with_exact_width(width as usize);
                    queue!(stdout, cursor::MoveTo(x_range.start + 1, cy), Print(line),)?;
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
            .unwrap_or_default();

        let hash = path
            .metadata()
            .ok()
            .and_then(|m| m.accessed().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .unwrap_or_default()
            .as_secs();

        let preview = match extension {
            "png" | "bmp" | "jpg" | "jpeg" => {
                if let Ok(img_bytes) = image::io::Reader::open(&path) {
                    let img = img_bytes.decode().ok();
                    Preview::Image { img }
                } else {
                    Preview::Image { img: None }
                }
            }
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
                // BAT
                // let output = std::process::Command::new("bat")
                //     .arg("--color always")
                //     .arg(&path)
                //     .spawn()
                //     .expect("failed to run neovim")
                //     .wait_with_output()
                //     .unwrap();
                // let lines = output.stdout.lines().take(128).flatten().collect();
                // Preview::Text { lines }
            }
        };

        FilePreview {
            path,
            hash,
            preview,
        }
    }
}

impl BasePanel for FilePreview {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn content_hash(&self) -> u64 {
        self.hash
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
}

impl Draw for PreviewPanel {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        match self {
            PreviewPanel::Dir(panel) => panel.draw(stdout, x_range, y_range),
            PreviewPanel::File(preview) => preview.draw(stdout, x_range, y_range),
        }
    }
}

impl BasePanel for PreviewPanel {
    fn path(&self) -> &Path {
        match self {
            PreviewPanel::Dir(panel) => panel.path(),
            PreviewPanel::File(preview) => preview.path(),
        }
    }

    fn content_hash(&self) -> u64 {
        match self {
            PreviewPanel::Dir(p) => p.content_hash(),
            PreviewPanel::File(p) => p.content_hash(),
        }
    }

    fn update_content(&mut self, content: Self) {
        *self = content;
    }
}

impl PreviewPanel {
    // pub fn from_path<P: AsRef<Path>>(maybe_path: Option<P>) -> Result<PreviewPanel> {
    //     if let Some(path) = maybe_path {
    //         if path.as_ref().is_dir() {
    //             Ok(PreviewPanel::Dir(DirPanel::empty()))
    //         } else {
    //             Ok(PreviewPanel::File(FilePreview::new(path.as_ref().into())))
    //         }
    //     } else {
    //         Ok(PreviewPanel::Dir(DirPanel::empty()))
    //     }
    // }

    pub fn empty() -> PreviewPanel {
        PreviewPanel::Dir(DirPanel::empty())
    }

    pub fn loading(path: PathBuf) -> PreviewPanel {
        PreviewPanel::Dir(DirPanel::loading(path))
    }

    pub fn path(&self) -> Option<PathBuf> {
        match self {
            PreviewPanel::Dir(panel) => Some(panel.path().to_path_buf()),
            PreviewPanel::File(panel) => Some(panel.path().to_path_buf()),
        }
    }
}
