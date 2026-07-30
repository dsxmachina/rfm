use std::{
    env::temp_dir,
    fs::File,
    io::{self, BufRead, Read, Stdout},
    ops::Range,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    config::color::print_vertical_bar,
    engine::opener::get_mime_type,
    util::{truncate_with_color_codes, ExactWidth},
};

use super::{BasePanel, DirPanel, Draw, PanelContent};
use crossterm::{
    cursor, queue,
    style::{self, Colors, Print, ResetColor, SetColors},
    Result,
};
use image::DynamicImage;
use once_cell::sync::OnceCell;

#[derive(Debug, Clone)]
pub enum Preview {
    Image {
        img: Option<DynamicImage>,
        info: Vec<String>,
    },
    Text {
        lines: Vec<String>,
    },
}

#[derive(Debug, Clone)]
struct ResizeCache {
    w: u32,
    h: u32,
    rgb: image::RgbImage,
    count: u32,
}

/// Resize `src` to `w`x`h`, reusing `cache` when the requested dimensions are
/// unchanged. Operates on the fields directly so callers can pass disjoint
/// borrows of `FilePreview` (the draw path holds `&self.preview` while
/// mutating `&mut self.resize_cache`).
fn resized_rgb<'a>(
    cache: &'a mut Option<ResizeCache>,
    src: &DynamicImage,
    w: u32,
    h: u32,
) -> &'a image::RgbImage {
    let need = cache.as_ref().map_or(true, |c| c.w != w || c.h != h);
    if need {
        let prev = cache.as_ref().map_or(0, |c| c.count);
        log::debug!("converting img: {}x{}", w, h);
        let rgb = src.thumbnail(w, h).into_rgb8();
        *cache = Some(ResizeCache {
            w,
            h,
            rgb,
            count: prev + 1,
        });
    }
    &cache.as_ref().unwrap().rgb
}

#[derive(Debug, Clone)]
pub struct FilePreview {
    path: PathBuf,
    modified: SystemTime,
    preview: Preview,
    /// Cache of the last resized RGB image, keyed on the requested cell
    /// dimensions. Recomputed only when those change or a new preview
    /// arrives (a new preview replaces the whole `FilePreview`, so its
    /// cache starts empty).
    resize_cache: Option<ResizeCache>,
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
                print_vertical_bar(),
            )?;
        }

        // Destructure disjoint fields so the cache can be mutated while the
        // preview is read.
        let Self {
            preview,
            resize_cache,
            path,
            ..
        } = self;
        match preview {
            Preview::Image { img, info } => {
                // load image
                if img.is_some() {
                    // Generate thumbnail
                    let thumbnail_height = if info.is_empty() {
                        2 * height
                    } else {
                        4 * height / 3
                    };
                    let src = img.as_ref().unwrap();
                    // Rebuild the resized RGB image only when the requested
                    // cell dimensions change (or on first draw).
                    let img = resized_rgb(resize_cache, src, width as u32, thumbnail_height as u32);
                    log::debug!(
                        "img: {}x{}, wxh: {}x{}",
                        img.width(),
                        img.height(),
                        width,
                        height,
                    );
                    let mut cy = y_range.start;
                    for y in (0..img.height() as usize).step_by(2) {
                        for x in 0..width {
                            // cursor x
                            let cx = x_range.start.saturating_add(x).saturating_add(1);
                            queue!(stdout, cursor::MoveTo(cx, cy))?;
                            let px_hi = img.get_pixel_checked(x as u32, y as u32);
                            let px_lo = img.get_pixel_checked(x as u32, (y + 1) as u32);
                            if let (Some(px_hi), Some(px_lo)) = (px_hi, px_lo) {
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
                                queue!(stdout, ResetColor, Print(" "),)?;
                            }
                        }
                        // Increase column
                        cy += 1;
                    }
                    queue!(stdout, ResetColor)?;
                    // Reset everything else
                    let mut idx = 0;
                    for y in cy..y_range.end {
                        if let Some(line) = info.get(idx) {
                            let line = line.exact_width(width as usize);
                            let cx = x_range.start.saturating_add(1);
                            queue!(stdout, cursor::MoveTo(cx, y), Print(" "), Print(line))?;
                        } else {
                            for x in 0..width {
                                let cx = x_range.start.saturating_add(x).saturating_add(1);
                                queue!(stdout, cursor::MoveTo(cx, y), Print(" "),)?;
                            }
                        }
                        idx += 1;
                    }
                } else {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start + 1, y_range.start + 1),
                        Print(format!("Failed to load image '{}'", path.display())),
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
        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or_else(SystemTime::now);

        let mime = get_mime_type(&path);

        let preview = match (mime.type_().as_str(), mime.subtype().as_str()) {
            ("image", _) => image_preview(&path, mediainfo(&path).unwrap_or_default()),
            ("audio", _) => cmd_to_preview("mediainfo", mediainfo(&path)),
            ("video", _) => video_preview(&path, modified),
            ("application", "x-x509-ca-cert") => cert_preview(&path),
            ("application", "gzip") => gz_preview(&path),
            ("application", "x-tar") => tar_preview(&path),
            ("application", "zip") => zip_preview(&path),
            // Text based application/* types
            ("application", "x-sh")
            | ("application", "json")
            | ("application", "javascript")
            | ("application", "javascript; charset=utf-8")
            | ("application", "rtf")
            | ("application", "xml")
            | ("application", "x-sql")
            | ("application", "xhtml+xml") => bat_preview(&path, false),
            // Binary based application/* types
            ("application", "octet-stream") | ("application", "msgpack") => {
                bat_preview(&path, true)
            }
            // Use mediainfo for everything else
            ("application", _) => cmd_to_preview("mediainfo", mediainfo(&path)),
            ("text", _) => bat_preview(&path, false),
            // Default to bat with binary mode enabled
            _ext => bat_preview(&path, true),
        };

        FilePreview {
            path,
            modified,
            preview,
            resize_cache: None,
        }
    }

    /// Return the source image resized to `w`x`h` cells, caching the result.
    /// Only recomputes when the requested dimensions change (or on first
    /// call). Must only be called when `self.preview` is a
    /// `Preview::Image` whose `img` is `Some`.
    #[cfg(test)]
    fn resized_rgb(&mut self, w: u32, h: u32) -> &image::RgbImage {
        let Self {
            preview,
            resize_cache,
            ..
        } = self;
        let src = match preview {
            Preview::Image { img: Some(img), .. } => img,
            _ => unreachable!("resized_rgb called without a Some image"),
        };
        resized_rgb(resize_cache, src, w, h)
    }

    #[cfg(test)]
    fn resize_count(&self) -> u32 {
        self.resize_cache.as_ref().map_or(0, |c| c.count)
    }

    #[cfg(test)]
    fn from_image_for_test(img: DynamicImage) -> FilePreview {
        FilePreview {
            path: PathBuf::new(),
            modified: SystemTime::now(),
            preview: Preview::Image {
                img: Some(img),
                info: Vec::new(),
            },
            resize_cache: None,
        }
    }
}

fn image_preview(path: impl AsRef<Path>, info: Vec<String>) -> Preview {
    log::debug!("--- creating image-preview for {}", path.as_ref().display());
    if let Ok(img_bytes) = image::io::Reader::open(&path) {
        let img = img_bytes.decode().ok().map(|img| img.thumbnail(960, 540));
        log::debug!("--- created image-preview for {}", path.as_ref().display());
        Preview::Image { img, info }
    } else {
        log::debug!(
            "--- created empty image-preview for {}",
            path.as_ref().display()
        );
        Preview::Image { img: None, info }
    }
}

fn video_preview(path: impl AsRef<Path>, modified: SystemTime) -> Preview {
    // Check, if ffmpeg exists
    static FFMPEG_INSTALLED: OnceCell<bool> = OnceCell::new();
    FFMPEG_INSTALLED.get_or_init(|| {
        let success = std::process::Command::new("ffmpeg")
            .arg("-h")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .and_then(|mut c| c.wait())
            .map(|e| e.success())
            .unwrap_or_default();
        success
    });
    if !FFMPEG_INSTALLED.get().unwrap() {
        return cmd_to_preview(
            "mediainfo",
            std::process::Command::new("mediainfo")
                .arg(path.as_ref())
                .output()
                .and_then(|o| o.stdout.lines().take(128).collect()),
        );
    }
    let modified = modified
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .unwrap_or_default();

    // Use ffmpeg
    match ffmpeg_thumbnail(&path, modified) {
        Ok(preview) => preview,
        Err(e) => {
            // Expected e.g. for videos shorter than the 10s thumbnail
            // seek - not worth an on-screen error on every visit.
            log::debug!("no ffmpeg thumbnail, falling back to mediainfo: {e}");
            cmd_to_preview(
                "mediainfo",
                std::process::Command::new("mediainfo")
                    .arg(path.as_ref())
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            )
        }
    }
}

fn ffmpeg_thumbnail(path: impl AsRef<Path>, modified: u64) -> anyhow::Result<Preview> {
    static THUMBNAIL_DIR: OnceCell<PathBuf> = OnceCell::new();
    let full_path = path.as_ref().as_os_str();
    let path_hash = seahash::hash(full_path.as_encoded_bytes());
    let identifier = format!("{path_hash}{modified}.jpg");
    let thumbnail = THUMBNAIL_DIR
        .get_or_init(|| {
            let dir = temp_dir().join("rfm-thumbnails");
            let _ = std::fs::create_dir_all(&dir);
            // One-time housekeeping: a thumbnail is never referenced
            // again once its video changed, so drop everything older
            // than a week instead of littering the temp-dir forever.
            prune_older_than(&dir, Duration::from_secs(7 * 24 * 60 * 60));
            dir
        })
        .join(identifier);
    if thumbnail.exists() {
        log::debug!("using existing thumbnail {}", thumbnail.display());
        Ok(image_preview(
            thumbnail,
            mediainfo(path).unwrap_or_default(),
        ))
    } else {
        log::debug!("generating thumbnail {}", thumbnail.display());
        let mut cmd = std::process::Command::new("ffmpeg");
        cmd.arg("-ss")
            .arg("00:00:10")
            .arg("-y")
            .arg("-i")
            .arg(path.as_ref())
            .arg("-vframes")
            .arg("1")
            .arg("-q:v")
            .arg("2")
            .arg("-vf")
            .arg("scale=120:-1")
            .arg(&thumbnail);
        cmd.stdin(Stdio::null());
        let out = cmd.output()?;
        // ffmpeg fails without writing the thumbnail e.g. for videos
        // shorter than the 10s seek; report that instead of building a
        // preview from a file that was never written (the caller falls
        // back to mediainfo on Err).
        if !out.status.success() || !thumbnail.exists() {
            // A failed run may still have created a partial file; drop
            // it, or the exists()-fast-path above would serve the
            // broken thumbnail on every later visit.
            let _ = std::fs::remove_file(&thumbnail);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
            anyhow::bail!(
                "ffmpeg did not produce a thumbnail ({}): {}",
                out.status,
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            );
        }
        Ok(image_preview(
            thumbnail,
            mediainfo(path).unwrap_or_default(),
        ))
    }
}

/// Best-effort cleanup for the thumbnail-dir: removes regular files in
/// `dir` that were modified more than `max_age` ago. Every error is
/// ignored - a stale thumbnail is never worth failing a preview over.
fn prune_older_than(dir: &Path, max_age: Duration) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age > max_age)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Parse a PEM or DER certificate and print the fields people actually
/// inspect. Errors route the caller to the openssl/bat fallback.
fn native_cert_lines(data: &[u8]) -> anyhow::Result<Vec<String>> {
    use x509_parser::prelude::*;
    let der: Vec<u8>;
    let cert_der: &[u8] = if data.starts_with(b"-----BEGIN") {
        let (_, pem) = parse_x509_pem(data)?;
        der = pem.contents;
        &der
    } else {
        data
    };
    let (_, cert) = X509Certificate::from_der(cert_der)?;
    let mut lines = vec![
        format!("Subject:    {}", cert.subject()),
        format!("Issuer:     {}", cert.issuer()),
        format!("Not before: {}", cert.validity().not_before),
        format!("Not after:  {}", cert.validity().not_after),
        format!("Serial:     {}", cert.raw_serial_as_string()),
        format!("Sig. alg.:  {}", cert.signature_algorithm.algorithm),
    ];
    if let Ok(Some(san)) = cert.subject_alternative_name() {
        for name in &san.value.general_names {
            lines.push(format!("SAN:        {name}"));
        }
    }
    Ok(lines)
}

/// Native-first certificate arm; the openssl shell-out (with its bat
/// fallback) only runs when the native parse fails.
fn cert_preview(path: impl AsRef<Path>) -> Preview {
    let native = std::fs::read(path.as_ref())
        .map_err(anyhow::Error::from)
        .and_then(|data| native_cert_lines(&data));
    match native {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native cert parse failed, trying openssl: {e}");
            cert_preview_external(path)
        }
    }
}

fn cert_preview_external(path: impl AsRef<Path>) -> Preview {
    // Check, if openssl exists
    static OPENSSL_INSTALLED: OnceCell<bool> = OnceCell::new();
    OPENSSL_INSTALLED.get_or_init(|| {
        let success = std::process::Command::new("openssl")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .and_then(|mut c| c.wait())
            .map(|e| e.success())
            .unwrap_or_default();
        success
    });
    if *OPENSSL_INSTALLED.get().unwrap() {
        let res = cmd_to_preview(
            "openssl",
            std::process::Command::new("openssl")
                .arg("x509")
                .arg("-in")
                .arg(path.as_ref())
                .arg("-text")
                .arg("-noout")
                .output()
                .and_then(|o| o.stdout.lines().take(128).collect()),
        );
        return res;
    }
    bat_preview(path, false)
}

/// List a zip archive natively: `size  name` per entry, capped at 128.
/// Reads only the central directory - `by_index_raw` never inflates.
fn native_zip_list(path: &Path) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut lines = Vec::new();
    for i in 0..archive.len().min(128) {
        let entry = archive.by_index_raw(i)?;
        lines.push(format!(
            "{:>8}  {}",
            crate::util::file_size_str(entry.size()),
            entry.name()
        ));
    }
    Ok(lines)
}

/// List a tar stream natively: `mode size name` per entry, capped at
/// 128. Generic over `Read` so the gzip arm can feed it a decoding
/// stream; streaming, so only the first 128 headers are read even from
/// a huge archive (which is what makes the kill-before-reap dance of
/// `tar_list` unnecessary on the primary path).
fn native_tar_list<R: io::Read>(reader: R) -> anyhow::Result<Vec<String>> {
    let mut archive = tar::Archive::new(reader);
    let mut lines = Vec::new();
    for entry in archive.entries()?.take(128) {
        let entry = entry?;
        let header = entry.header();
        // Tar headers carry the permission bits only; the file type
        // lives in the entry-type flag. OR it back in so unix_mode
        // prints "-rw-r--r--" instead of "?rw-r--r--".
        let type_bits = match header.entry_type() {
            tar::EntryType::Directory => 0o040000,
            tar::EntryType::Symlink => 0o120000,
            tar::EntryType::Char => 0o020000,
            tar::EntryType::Block => 0o060000,
            tar::EntryType::Fifo => 0o010000,
            _ => 0o100000,
        };
        lines.push(format!(
            "{} {:>8}  {}",
            unix_mode::to_string(type_bits | (header.mode().unwrap_or(0) & 0o7777)),
            crate::util::file_size_str(header.size().unwrap_or(0)),
            entry.path()?.display()
        ));
    }
    Ok(lines)
}

/// gzip arm: tar.gz gets a member listing, a gzipped non-tar file gets
/// its decompressed head as text. Falls back to the tar binary only
/// when the gzip stream itself is unreadable.
fn gz_preview(path: &Path) -> Preview {
    match native_gz_preview(path) {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!("native gzip preview failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

/// Decompress the head, sniff the tar magic (`ustar` at offset 257 -
/// covers both POSIX `ustar\0` and GNU `ustar  `), then either chain
/// head+rest into `native_tar_list` or show the decompressed head as
/// text (bounded 64 KiB read, lossy UTF-8, 128 lines, `\r` scrubbed
/// like `bat_preview`).
fn native_gz_preview(path: &Path) -> anyhow::Result<Preview> {
    let mut decoder = flate2::read::GzDecoder::new(File::open(path)?);
    // Read up to one tar block; a short read just means a small file.
    let mut head = [0u8; 512];
    let mut filled = 0;
    while filled < head.len() {
        let n = decoder.read(&mut head[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    let rest = io::Cursor::new(head[..filled].to_vec()).chain(decoder);
    if filled >= 262 && &head[257..262] == b"ustar" {
        return Ok(Preview::Text {
            lines: native_tar_list(rest)?,
        });
    }
    // Not a tar: show the decompressed head as text (bounded).
    let mut buf = Vec::with_capacity(64 * 1024);
    rest.take(64 * 1024).read_to_end(&mut buf)?;
    let lines = String::from_utf8_lossy(&buf)
        .lines()
        .take(128)
        .map(|l| l.replace('\r', ""))
        .collect();
    Ok(Preview::Text { lines })
}

/// Native-first x-tar arm; the tar binary stays as the fallback (the
/// zombie-reaping tar_list() is now only reachable through it).
fn tar_preview(path: &Path) -> Preview {
    let native = File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(native_tar_list);
    match native {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native tar list failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

/// Native-first zip arm; unzip stays as the shell-out fallback.
fn zip_preview(path: &Path) -> Preview {
    match native_zip_list(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native zip list failed, trying unzip: {e}");
            cmd_to_preview(
                "unzip",
                std::process::Command::new("unzip")
                    .arg("-l")
                    .arg(path)
                    .output()
                    .and_then(|o| o.stdout.lines().take(128).collect()),
            )
        }
    }
}

fn mediainfo(path: impl AsRef<Path>) -> io::Result<Vec<String>> {
    std::process::Command::new("mediainfo")
        .arg(path.as_ref())
        .output()
        .and_then(|o| o.stdout.lines().take(128).collect())
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
    let mut tar = std::process::Command::new("tar")
        .arg("--list")
        .arg("-f")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let lines: Vec<String> = match tar.stdout.take() {
        Some(tar_stdout) => io::BufReader::new(tar_stdout)
            .lines()
            .take(64)
            .flatten()
            .collect(),
        None => Vec::new(),
    };
    // Kill before reaping, so wait() cannot block on a tar that is
    // still streaming a huge archive; both are best-effort (kill fails
    // harmlessly if tar already exited), but without the wait() every
    // archive preview would leak a zombie process.
    let _ = tar.kill();
    let status = tar.wait();
    if lines.is_empty() && !status.map(|s| s.success()).unwrap_or(false) {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "tar --list failed and produced no output",
        ));
    }
    Ok(lines)
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
        // KNOWN DEBT: mirror of `DirPanel::draw` — the `active` flag lives on
        // the inherent `draw_active`, not the shared `Draw` trait. This trait
        // `draw` is the `active=false` default (the preview column is never the
        // focused cursor in single view); split/single rendering calls
        // `draw_active` explicitly to pass the real flag.
        self.draw_active(stdout, x_range, y_range, false)
    }
}

impl PreviewPanel {
    /// Like [`Draw::draw`], but forwards `active` to a directory preview so it
    /// can render a bright/dimmed cursor (used by split view). File and empty
    /// previews have no cursor and ignore it.
    pub fn draw_active(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
        active: bool,
    ) -> Result<()> {
        match self {
            PreviewPanel::Dir(panel) => panel.draw_active(stdout, x_range, y_range, active),
            PreviewPanel::File(preview) => preview.draw(stdout, x_range, y_range),
            PreviewPanel::Empty => {
                // Draw empty panel
                for y in y_range {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start, y),
                        print_vertical_bar(),
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
        if let PreviewPanel::Dir(panel) = self {
            // If the content is for the same path, also select the correct item
            if panel.path() == content.path() {
                if let Some(path) = panel.selected_path() {
                    content.select_path(path);
                }
            }
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
            panel.select_path(selection, None);
        }
    }
}

#[cfg(test)]
mod native_backend_tests {
    use super::*;

    /// Builds `dir/archive.zip` containing `files` via the zip crate.
    fn make_zip(dir: &Path, files: &[String]) -> PathBuf {
        use std::io::Write;
        let archive = dir.join("archive.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for name in files {
            writer.start_file(name.as_str(), options).unwrap();
            writer.write_all(b"content").unwrap();
        }
        writer.finish().unwrap();
        archive
    }

    #[test]
    fn native_zip_list_returns_all_names_of_a_small_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = ["a.txt", "b.txt", "dir/c.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let archive = make_zip(tmp.path(), &files);
        let lines = native_zip_list(&archive).unwrap();
        assert_eq!(lines.len(), 3);
        for (line, name) in lines.iter().zip(&files) {
            assert!(line.contains(name.as_str()), "{line}");
        }
    }

    #[test]
    fn native_zip_list_caps_the_listing_at_128_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = (0..130).map(|i| format!("file-{i:03}.txt")).collect();
        let archive = make_zip(tmp.path(), &files);
        assert_eq!(native_zip_list(&archive).unwrap().len(), 128);
    }

    #[test]
    fn native_zip_list_on_garbage_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("not-a.zip");
        std::fs::write(&bogus, b"definitely not a zip").unwrap();
        // Err is what triggers the unzip fallback in zip_preview().
        assert!(native_zip_list(&bogus).is_err());
    }

    /// Builds `dir/archive.tar` containing `files` via the tar crate.
    fn make_native_tar(dir: &Path, files: &[String]) -> PathBuf {
        let archive = dir.join("archive.tar");
        let mut builder = tar::Builder::new(std::fs::File::create(&archive).unwrap());
        for name in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name.as_str(), &b"content"[..])
                .unwrap();
        }
        builder.finish().unwrap();
        archive
    }

    #[test]
    fn native_tar_list_returns_all_names_of_a_small_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = ["a.txt", "b.txt"].iter().map(|s| s.to_string()).collect();
        let archive = make_native_tar(tmp.path(), &files);
        let lines = native_tar_list(File::open(archive).unwrap()).unwrap();
        assert_eq!(lines.len(), 2);
        // GNU tar headers carry no file-type bits in the mode; the type
        // char must come from the entry type, not render as '?'.
        assert!(
            lines[0].contains("a.txt") && lines[0].contains("-rw-r--r--"),
            "{lines:?}"
        );
    }

    #[test]
    fn native_tar_list_caps_the_listing_at_128_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = (0..130).map(|i| format!("file-{i:03}.txt")).collect();
        let archive = make_native_tar(tmp.path(), &files);
        assert_eq!(
            native_tar_list(File::open(archive).unwrap()).unwrap().len(),
            128
        );
    }

    #[test]
    fn native_tar_list_on_garbage_is_an_error() {
        let garbage = std::io::Cursor::new(vec![0xffu8; 1024]);
        assert!(native_tar_list(garbage).is_err());
    }

    #[test]
    fn tar_preview_of_garbage_degrades_to_a_text_preview() {
        // Corrupt archive: the native path errors, the tar-binary
        // fallback runs (and errors too) - the result must still be
        // text lines, never a panic or a blank preview.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("corrupt.tar");
        std::fs::write(&bogus, vec![0xffu8; 1024]).unwrap();
        match tar_preview(&bogus) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn zip_preview_of_garbage_degrades_to_a_text_preview() {
        // Corrupt archive: the native path errors, the unzip fallback
        // runs (and errors too) - the result must still be text lines,
        // never a panic or a blank preview.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("corrupt.zip");
        std::fs::write(&bogus, b"definitely not a zip").unwrap();
        match zip_preview(&bogus) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
    }

    /// Builds `dir/archive.tar.gz` via tar::Builder into a GzEncoder.
    fn make_native_tar_gz(dir: &Path, files: &[String]) -> PathBuf {
        let archive = dir.join("archive.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            std::fs::File::create(&archive).unwrap(),
            flate2::Compression::default(),
        );
        let mut builder = tar::Builder::new(encoder);
        for name in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, name.as_str(), &b"content"[..])
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        archive
    }

    #[test]
    fn native_gz_preview_lists_the_members_of_a_tar_gz() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = ["a.txt", "b.txt"].iter().map(|s| s.to_string()).collect();
        let archive = make_native_tar_gz(tmp.path(), &files);
        match gz_preview(&archive) {
            Preview::Text { lines } => {
                assert!(lines.iter().any(|l| l.contains("a.txt")), "{lines:?}");
                assert!(lines.iter().any(|l| l.contains("b.txt")), "{lines:?}");
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn gz_of_a_non_tar_file_previews_the_decompressed_text() {
        // The old dispatch sent every application/gzip to `tar --list`,
        // so a plain foo.txt.gz produced an error instead of its content.
        let tmp = tempfile::tempdir().unwrap();
        let gz = tmp.path().join("foo.txt.gz");
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(&gz).unwrap(),
            flate2::Compression::default(),
        );
        enc.write_all(b"hello from a gzipped text file\r\nsecond line\n")
            .unwrap();
        enc.finish().unwrap();
        match gz_preview(&gz) {
            Preview::Text { lines } => {
                assert!(
                    lines[0].contains("hello from a gzipped text file"),
                    "{lines:?}"
                );
                assert!(
                    !lines[0].contains('\r'),
                    "carriage returns must be scrubbed: {lines:?}"
                );
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn gz_preview_of_garbage_degrades_to_a_text_preview() {
        // Not gzip at all: the native path errors, the tar-binary
        // fallback runs (and errors too) - the result must still be
        // text lines, never a panic or a blank preview.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("corrupt.tar.gz");
        std::fs::write(&bogus, vec![0xffu8; 64]).unwrap();
        match gz_preview(&bogus) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
    }

    /// Deterministic fixture, generated once via:
    /// `openssl req -x509 -newkey rsa:2048 -keyout /dev/null -out /dev/stdout
    ///  -days 3650 -nodes -subj "/CN=rfm-test/O=Example Org"
    ///  -addext "subjectAltName=DNS:example.test"`
    const TEST_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDTDCCAjSgAwIBAgIUD861X4nFcMAGMDWLj/hTRRyWB8EwDQYJKoZIhvcNAQEL
BQAwKTERMA8GA1UEAwwIcmZtLXRlc3QxFDASBgNVBAoMC0V4YW1wbGUgT3JnMB4X
DTI2MDczMDE5MTk1NVoXDTM2MDcyNzE5MTk1NVowKTERMA8GA1UEAwwIcmZtLXRl
c3QxFDASBgNVBAoMC0V4YW1wbGUgT3JnMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A
MIIBCgKCAQEAmoeJKTcZztfVWsNO6H7wx8HeNOIBWBxrya9SF08nBzIHQigM/2My
rO8doTZesvwpplUG2ZyngXL5yhssOjADclFvZfkYBgTGWdXN0v7FMcDsiGWJowz9
Ac0/9J47MPkaxA/HxwrX6VNSVVueXeA2Uj7lS2wdViYnb7CP5aYnm+49GucVfb26
XYAUC4C8P90BIgUk3Xm46DM6+bkcbX9GEWqLvD+v1tkH4aeJjKQ0Dk0Sd80xddwG
yRcU61j2SOxByzEop/r2loaBYFYW4C6Px8QiZeca5k/Yj/mr4UN7n1cwpQtW6TYQ
ltwbfe6s0axt54VPHhrRelGPJroMmnRBFwIDAQABo2wwajAdBgNVHQ4EFgQUTKq5
p33mslVLdH+w05SrQvEDzccwHwYDVR0jBBgwFoAUTKq5p33mslVLdH+w05SrQvED
zccwDwYDVR0TAQH/BAUwAwEB/zAXBgNVHREEEDAOggxleGFtcGxlLnRlc3QwDQYJ
KoZIhvcNAQELBQADggEBADNEfWo/XLi5e+O2uU4IxU6hgeWERiKZTTkbLQLEmQIW
L7qqKLXJNntmD0kfnLUcxjxcxYnT6PoJInc7VkeEnZa1aPoCo1SL3a8vHw2EBT4E
ciIr3itD7fBxu8oZWn2L/rhVoyOooGcJY7sJLOMAUkPr5LFzr16aJ9BgbqJqiqAK
kz4d/UmvTrE/YN0qxBr50MHesafMq4pdinNaFkn8+PbCpX1LrNZmdPcgBRY1gIt/
+j4USRPsMNfpiz/0HuwSEWt7gK+xOl+UOGcrlt8BC3BUhkGKjaSijrslLS8+0LeW
Q7ZNh7owTFb+WgkD0bBFJFVxePwzS/hyUAb6w+9Vufg=
-----END CERTIFICATE-----
";

    #[test]
    fn native_cert_lines_include_subject_validity_and_san() {
        let lines = native_cert_lines(TEST_PEM.as_bytes()).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("rfm-test"), "{joined}");
        assert!(joined.contains("Not before"), "{joined}");
        assert!(joined.contains("Not after"), "{joined}");
        assert!(joined.contains("example.test"), "{joined}");
    }

    #[test]
    fn native_cert_lines_parse_der_as_well_as_pem() {
        // The same certificate, DER-decoded from the PEM body.
        let (_, pem) = x509_parser::pem::parse_x509_pem(TEST_PEM.as_bytes()).unwrap();
        let lines = native_cert_lines(&pem.contents).unwrap();
        assert!(lines.join("\n").contains("rfm-test"), "{lines:?}");
    }

    #[test]
    fn native_cert_lines_on_garbage_are_an_error() {
        // Err is what routes cert_preview to the openssl/bat fallback.
        assert!(native_cert_lines(b"not a certificate").is_err());
    }

    #[test]
    fn cert_preview_of_a_pem_file_is_native_text() {
        let tmp = tempfile::tempdir().unwrap();
        let pem = tmp.path().join("server cert.pem");
        std::fs::write(&pem, TEST_PEM).unwrap();
        match cert_preview(&pem) {
            Preview::Text { lines } => {
                let joined = lines.join("\n");
                assert!(joined.contains("Subject:"), "{joined}");
                assert!(joined.contains("rfm-test"), "{joined}");
            }
            _ => panic!("expected a text preview"),
        }
    }
}

#[cfg(test)]
mod external_cmd_tests {
    use super::*;

    /// Creates `files` in `dir` and packs them into `dir/archive.tar`
    /// via the real tar binary (the same one `tar_list` shells out to).
    fn make_tar(dir: &Path, files: &[String]) -> PathBuf {
        for name in files {
            std::fs::write(dir.join(name), b"content").unwrap();
        }
        let archive = dir.join("archive.tar");
        let status = std::process::Command::new("tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(dir)
            .args(files)
            .status()
            .unwrap();
        assert!(status.success(), "building the fixture archive failed");
        archive
    }

    #[test]
    fn tar_list_returns_all_names_of_a_small_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = ["a.txt", "b.txt", "c.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let archive = make_tar(tmp.path(), &files);
        let lines = tar_list(&archive).unwrap();
        assert_eq!(lines, files);
    }

    #[test]
    fn tar_list_caps_the_listing_at_64_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<String> = (0..70).map(|i| format!("file-{i:03}.txt")).collect();
        let archive = make_tar(tmp.path(), &files);
        let lines = tar_list(&archive).unwrap();
        assert_eq!(lines.len(), 64);
    }

    #[test]
    fn tar_list_on_a_missing_archive_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let result = tar_list(&tmp.path().join("missing.tar"));
        // tar exits non-zero and prints nothing to stdout; that must
        // surface as Err so cmd_to_preview() shows its error text
        // instead of a blank preview.
        assert!(result.is_err());
    }

    /// Generates a tiny test video of `seconds` length with ffmpeg.
    fn make_video(dir: &Path, name: &str, seconds: u32) -> PathBuf {
        let video = dir.join(name);
        let status = std::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-f")
            .arg("lavfi")
            .arg("-i")
            .arg(format!("testsrc=duration={seconds}:size=64x64:rate=10"))
            .arg(&video)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "building the fixture video failed");
        video
    }

    #[test]
    fn ffmpeg_thumbnail_of_a_too_short_video_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "short.mp4", 1);
        // The hardcoded 10s seek is past the end of this clip, so
        // ffmpeg fails and writes no thumbnail - that must surface as
        // Err (the caller then falls back to mediainfo) instead of a
        // phantom image preview.
        assert!(ffmpeg_thumbnail(&video, 0).is_err());
    }

    #[test]
    fn ffmpeg_thumbnail_of_a_long_video_lands_in_the_thumbnail_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "long.mp4", 15);
        ffmpeg_thumbnail(&video, 1).unwrap();
        let path_hash = seahash::hash(video.as_os_str().as_encoded_bytes());
        let thumbnail = temp_dir()
            .join("rfm-thumbnails")
            .join(format!("{path_hash}1.jpg"));
        assert!(thumbnail.is_file());
        let _ = std::fs::remove_file(thumbnail);
    }

    #[test]
    fn prune_older_than_removes_only_stale_files() {
        let tmp = tempfile::tempdir().unwrap();
        let old = tmp.path().join("old.jpg");
        let fresh = tmp.path().join("fresh.jpg");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&fresh, b"fresh").unwrap();
        let status = std::process::Command::new("touch")
            .arg("-d")
            .arg("10 days ago")
            .arg(&old)
            .status()
            .unwrap();
        assert!(status.success(), "backdating the fixture file failed");
        prune_older_than(tmp.path(), Duration::from_secs(7 * 24 * 60 * 60));
        assert!(!old.exists(), "the stale file must be pruned");
        assert!(fresh.exists(), "the fresh file must survive");
    }
}

#[cfg(test)]
mod render_cache_tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn tiny_preview() -> FilePreview {
        let img = DynamicImage::ImageRgb8(RgbImage::new(64, 64));
        FilePreview::from_image_for_test(img)
    }

    #[test]
    fn resize_is_cached_across_equal_dimensions() {
        let mut p = tiny_preview();
        let a = p.resized_rgb(20, 30).clone();
        let b = p.resized_rgb(20, 30).clone();
        assert_eq!(p.resize_count(), 1, "second call must hit the cache");
        assert_eq!(a.dimensions(), b.dimensions());
    }

    #[test]
    fn resize_recomputes_when_dimensions_change() {
        let mut p = tiny_preview();
        p.resized_rgb(20, 30);
        p.resized_rgb(21, 30);
        assert_eq!(p.resize_count(), 2);
    }
}
