use std::{
    fs::File,
    io::{self, BufRead, Stdout},
    ops::Range,
    path::{Path, PathBuf},
    process::Stdio,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::util::truncate_with_color_codes;

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
                        // Increase column
                        cy += 1;
                    }
                    queue!(stdout, ResetColor)?;
                    // Reset everything else
                    for y in cy..y_range.end {
                        for x in 0..width {
                            let cx = x_range.start.saturating_add(x).saturating_add(1);
                            queue!(stdout, cursor::MoveTo(cx, y), Print(" "),)?;
                        }
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
            }
            Preview::Text { lines } => {
                // Print preview
                let mut idx = 0;
                // Clear entire panel
                for x in x_range.start + 1..x_range.end {
                    for y in y_range.clone() {
                        queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
                    }
                }
                for line in lines.iter().take(height as usize) {
                    let cy = idx + y_range.start;
                    let line = truncate_with_color_codes(line, width.saturating_sub(1) as usize);
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start + 1, cy),
                        Print(" "),
                        cursor::MoveTo(x_range.start + 2, cy),
                        Print(line)
                    )?;
                    idx += 1;
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

        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or_else(SystemTime::now);

        let mime = mime_guess::from_ext(extension).first_or_text_plain();

        let preview = match (mime.type_().as_str(), mime.subtype().as_str()) {
            ("image", _) => {
                if let Ok(img_bytes) = image::io::Reader::open(&path) {
                    let img = img_bytes.decode().ok();
                    Preview::Image { img }
                } else {
                    Preview::Image { img: None }
                }
            }
            ("audio", _) | ("video", _) => cmd_to_preview(
                "mediainfo",
                std::process::Command::new("mediainfo")
                    .arg(&path)
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            ),
            ("application", "gzip") => cmd_to_preview("tar", tar_list(&path)),
            ("application", "x-tar") => cmd_to_preview("tar", tar_list(&path)),
            ("application", "zip") => cmd_to_preview(
                "unzip",
                std::process::Command::new("unzip")
                    .arg("-l")
                    .arg(&path)
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            ),
            // Text based application/* types
            ("application", "x-sh")
            | ("application", "json")
            | ("application", "javascript")
            | ("application", "javascript; charset=utf-8")
            | ("application", "rtf")
            | ("application", "xml")
            | ("application", "xhtml+xml") => bat_preview(&path, false),
            // Binary based application/* types
            ("application", "octet-stream") | ("application", "msgpack") => {
                bat_preview(&path, true)
            }
            // Use mediainfo for everything else
            ("application", _) => cmd_to_preview(
                "mediainfo",
                std::process::Command::new("mediainfo")
                    .arg(&path)
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            ),
            ("text", _) => bat_preview(&path, false),
            // Default to bat with binary mode enabled
            _ext => bat_preview(&path, true),
        };

        FilePreview {
            path,
            modified,
            preview,
        }
    }
}

fn bat_preview<P: AsRef<Path>>(path: P, binary: bool) -> Preview {
    // Use bat for preview generation (if present)
    let mut cmd = std::process::Command::new("bat");
    cmd.arg("--color=always")
        .arg("--style=plain")
        .arg("--line-range=0:128");

    // If binary, use --show-all
    if binary {
        cmd.arg("--show-all");
    }

    let lines = match cmd.arg(path.as_ref()).output() {
        Ok(output) => output
            .stdout
            .lines()
            .take(128)
            .flatten()
            .map(|l| l.replace(['\r', '\n'], ""))
            .collect(),
        Err(_e) => {
            // Otherwise default to just reading the file
            match File::open(&path) {
                Ok(file) => io::BufReader::new(file)
                    .lines()
                    .take(128)
                    .flatten()
                    .collect(),
                Err(e) => vec![
                    format!("Failed to open '{}'", path.as_ref().display()),
                    "".to_string(),
                    format!("{}", e),
                ],
            }
        }
    };
    Preview::Text { lines }
}

fn cmd_to_preview(cmd_name: &'static str, result: std::io::Result<Vec<String>>) -> Preview {
    let lines = match result {
        Ok(l) => l,
        Err(e) => vec![
            format!("Error: Could not run {cmd_name}"),
            e.to_string(),
            "".to_string(),
            format!("You must have {cmd_name} installed to get a preview for this file-type."),
        ],
    };
    Preview::Text { lines }
}

// Helper function to generate a preview from tar output
fn tar_list(path: &Path) -> std::io::Result<Vec<String>> {
    let tar = std::process::Command::new("tar")
        .arg("--list")
        .arg("-f")
        .arg(path)
        .stdout(Stdio::piped())
        .spawn()?;
    match tar.stdout {
        Some(tar_stdout) => {
            let output = std::process::Command::new("head")
                .arg("-64")
                .stdin(Stdio::from(tar_stdout))
                .output()?;
            Ok(output.stdout.lines().take(64).flatten().collect())
        }
        None => Ok(vec![format!("Failed to fetch stdout from 'tar --list'")]),
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

    fn update_content(&mut self, mut content: Self) {
        match self {
            PreviewPanel::Dir(panel) => {
                // If the content is for the same path, also select the correct item
                if panel.path() == content.path() {
                    if let Some(path) = panel.selected_path() {
                        content.select_path(path);
                    }
                }
            }
            _ => (),
        }
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

    pub fn select_path(&mut self, selection: &Path) {
        if let PreviewPanel::Dir(panel) = self {
            log::debug!("preview-panel: selecting {}", selection.display());
            panel.select_path(selection);
        }
    }
}
