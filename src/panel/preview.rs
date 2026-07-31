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

use super::{
    graphics::{self, GraphicsProtocol},
    raster_cache, BasePanel, DirPanel, Draw, PanelContent,
};
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

/// Cell rows the image may occupy in a graphics-protocol draw: the full
/// height when there are no info lines, else two thirds — mirroring the
/// half-block branch's `2*height` / `4*height/3` pixel budget (2 px per
/// cell row). Info lines stay ordinary cell text below the image.
fn image_rows(height: u16, has_info: bool) -> u16 {
    if has_info {
        2 * height / 3
    } else {
        height
    }
}

/// Cell size assumed when the terminal never reported pixel geometry (D2).
/// Kitty scales the raster into the `c=`/`r=` cell rectangle, so placement
/// stays exact even if the guess is off — only the pre-scale sharpness
/// varies.
const ASSUMED_CELL: graphics::CellGeometry = graphics::CellGeometry {
    cell_w: 8,
    cell_h: 16,
};

/// The cell geometry a protocol can draw with, or `None` when it cannot
/// draw at all: kitty tolerates missing geometry (assumed cell, D2), a
/// sixel raster is placed verbatim and demands the real cell size, and
/// half-block is not a graphics emitter.
fn geometry_for(proto: GraphicsProtocol) -> Option<graphics::CellGeometry> {
    match proto {
        GraphicsProtocol::Kitty => Some(graphics::cell_geometry().unwrap_or(ASSUMED_CELL)),
        GraphicsProtocol::Sixel => graphics::cell_geometry(),
        GraphicsProtocol::HalfBlock => None,
    }
}

/// Draw the image via a graphics protocol (kitty or sixel). Returns the
/// cell rows used, so the caller's info-line/blanking tail runs unchanged
/// below the image. Any error falls back to the half-block loop for this
/// frame. Generic over the writer so the byte stream is unit-testable
/// against a `Vec<u8>` sink.
#[allow(clippy::too_many_arguments)]
fn draw_graphics(
    proto: GraphicsProtocol,
    stdout: &mut impl io::Write,
    resize_cache: &mut Option<ResizeCache>,
    src: &DynamicImage,
    has_info: bool,
    x_range: &Range<u16>,
    y_range: &Range<u16>,
    path: &Path,
    modified: SystemTime,
) -> Result<u16> {
    let width = x_range.end.saturating_sub(x_range.start.saturating_add(1));
    let height = y_range.end.saturating_sub(y_range.start);
    let rows_budget = image_rows(height, has_info);
    if width == 0 || rows_budget == 0 {
        return Ok(0);
    }
    let geo = geometry_for(proto).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "protocol needs pixel cell geometry, none available",
        )
    })?;
    // The raster is fitted into the pane's pixel box; the resize cache is
    // keyed on these requested pixel dims (D8).
    let (px_w, px_h) = graphics::pixel_box(width, rows_budget, geo);
    let rgb = resized_rgb(resize_cache, src, px_w, px_h);
    // Placement in cells from the *actual* raster size (thumbnail keeps the
    // aspect ratio), clamped to the pane span for exact clipping.
    let (cols, rows) =
        graphics::placement_cells(rgb.width(), rgb.height(), geo, width, rows_budget);
    if cols == 0 || rows == 0 {
        return Ok(0);
    }
    let origin = (x_range.start.saturating_add(1), y_range.start);
    let key = graphics::EmitKey {
        path: path.to_path_buf(),
        mtime_secs: mtime_secs(modified),
        px_w,
        px_h,
        origin_cell: origin,
    };
    match proto {
        GraphicsProtocol::Kitty => graphics::emit_kitty(stdout, key, rgb, cols, rows)?,
        GraphicsProtocol::Sixel => graphics::emit_sixel(stdout, key, rgb, cols, rows)?,
        // Unreachable via the dispatch guard; kept as a graceful fallback
        // instead of a panic in the draw path.
        GraphicsProtocol::HalfBlock => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "half-block is not a graphics emitter",
            ))
        }
    };
    // The raster covers only `cols` of the pane's `width` columns. The
    // strip beside a narrower-than-pane image is ordinary cell content this
    // draw owns (full-repaint invariant): paint it every frame — the emit
    // above may be gated and write zero raster bytes, but without this the
    // previous preview's cells would persist there indefinitely.
    graphics::blank_cells(
        stdout,
        origin.0.saturating_add(cols)..x_range.end,
        y_range.start..y_range.start.saturating_add(rows),
    )?;
    Ok(rows)
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
            modified,
            ..
        } = self;
        match preview {
            Preview::Image { img, info } => {
                // load image
                if img.is_some() {
                    let src = img.as_ref().unwrap();
                    // Graphics-protocol tier: real pixels via kitty/sixel
                    // when the frame allows a placement (single view, no
                    // overlay). Any emit failure falls back to half-blocks
                    // for this frame — a preview always renders *something*.
                    let mut graphics_cy = None;
                    let proto = graphics::protocol();
                    if proto != GraphicsProtocol::HalfBlock && graphics::frame_allows_image() {
                        match draw_graphics(
                            proto,
                            stdout,
                            resize_cache,
                            src,
                            !info.is_empty(),
                            &x_range,
                            &y_range,
                            path,
                            *modified,
                        ) {
                            Ok(rows) => graphics_cy = Some(y_range.start.saturating_add(rows)),
                            Err(e) => {
                                log::debug!("graphics: emit failed, half-block fallback: {e}")
                            }
                        }
                    }
                    let cy = if let Some(cy) = graphics_cy {
                        cy
                    } else {
                        // Half-block fallback: two vertical pixels per cell
                        // via the `▄` glyph. Generate thumbnail
                        let thumbnail_height = if info.is_empty() {
                            2 * height
                        } else {
                            4 * height / 3
                        };
                        // Rebuild the resized RGB image only when the requested
                        // cell dimensions change (or on first draw).
                        let img =
                            resized_rgb(resize_cache, src, width as u32, thumbnail_height as u32);
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
                        cy
                    };
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
            // Before the raster arm: image/svg+xml used to mis-land on
            // the bitmap decode (which cannot read SVG). Covers .svgz
            // too — the arm inflates the gzip layer itself, bounded
            // (usvg's auto-decompression has no output cap). NOTE: mime 0.3
            // splits "svg+xml" into subtype "svg" + suffix "xml", so
            // the subtype to match is "svg".
            ("image", "svg") => svg_preview(&path, modified),
            ("image", _) => cached_image_preview(&path, modified, &mime),
            // ttf/otf (and any sfnt the sniff finds) get the rendered
            // sample; woff/woff2 currently degrade to the stat fallback
            // inside the arm (no MSRV-1.83 pure-Rust woff decoder —
            // ttf-parser cannot read WOFF containers).
            ("font", _) => font_preview(&path, modified, &mime),
            // .otf via mime_guess; extensionless ttf/otf and woff via
            // the infer sniff — the legacy application/font-* aliases.
            ("application", "font-sfnt") | ("application", "font-woff") => {
                font_preview(&path, modified, &mime)
            }
            ("audio", _) => audio_preview(&path),
            ("video", _) => video_preview(&path, modified),
            ("application", "x-x509-ca-cert") => cert_preview(&path),
            ("application", "gzip") => gz_preview(&path),
            // zstd/xz/bzip2 ride the same decompress -> tar-sniff ->
            // list-or-text logic as gzip, via pure-Rust decoders.
            // Extensions route through the get_mime_type special-cases
            // (.zst/.tzst/.txz/.tbz2) or mime_guess (.xz/.bz2/.7z);
            // extensionless files through the infer magics.
            ("application", "zstd") => zst_preview(&path),
            ("application", "x-xz") => xz_preview(&path),
            ("application", "x-bzip2") => bz2_preview(&path),
            ("application", "x-7z-compressed") => sevenz_preview(&path),
            ("application", "x-tar") => tar_preview(&path),
            ("application", "zip") => zip_preview(&path),
            // Zip-container documents: extensions resolve via
            // mime_guess, extensionless files via infer's zip-content
            // discrimination — both land here. A container infer can
            // only see as generic zip takes the plain zip arm above
            // (an acceptable listing, never empty). NOTE: mime 0.3
            // splits "epub+zip" into subtype "epub" + suffix "zip"
            // (the svg+xml trap), while the vnd.* subtypes carry no
            // suffix and match whole.
            ("application", "vnd.openxmlformats-officedocument.wordprocessingml.document") => {
                doc_preview(&path, DocKind::Docx)
            }
            ("application", "vnd.openxmlformats-officedocument.spreadsheetml.sheet") => {
                doc_preview(&path, DocKind::Xlsx)
            }
            ("application", "vnd.openxmlformats-officedocument.presentationml.presentation") => {
                doc_preview(&path, DocKind::Pptx)
            }
            ("application", "vnd.oasis.opendocument.text")
            | ("application", "vnd.oasis.opendocument.spreadsheet")
            | ("application", "vnd.oasis.opendocument.presentation") => {
                doc_preview(&path, DocKind::Odt)
            }
            ("application", "epub") => doc_preview(&path, DocKind::Epub),
            // Sniff-routed: .db/.sqlite/.sqlite3 get no mime_guess
            // answer, so infer resolves the `SQLite format 3\0` magic —
            // a .db that is not SQLite routes elsewhere (correct).
            ("application", "vnd.sqlite3") => sqlite_preview(&path, &mime),
            // Tiered pdf preview (image -> text -> stat). The `%PDF`
            // magic also routes extensionless files here via the
            // sniff; mime 0.3 keeps subtype "pdf" whole — no suffix
            // trap like svg+xml/epub+zip.
            ("application", "pdf") => pdf_preview(&path, modified, &mime),
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
            // Native stat block for everything else (mediainfo is only
            // the fallback for the rare unreadable-metadata case)
            ("application", _) => stat_preview(&path, &mime),
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

/// Seconds since the epoch for a file mtime — the cache-key clock.
fn mtime_secs(modified: SystemTime) -> u64 {
    modified
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .unwrap_or_default()
}

/// Info footer for an image preview, built from the decode and file
/// metadata instead of a mediainfo shell-out.
fn image_info_lines(
    width: u32,
    height: u32,
    color: image::ColorType,
    byte_size: u64,
    modified: SystemTime,
    subtype: &str,
) -> Vec<String> {
    use time::OffsetDateTime;
    let t = OffsetDateTime::from(modified);
    vec![
        format!("{width} × {height}  {color:?}"),
        format!("{subtype} · {}", crate::util::file_size_str(byte_size)),
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            t.year(),
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute(),
            t.second()
        ),
    ]
}

/// Decode an image upright: the decoder's EXIF orientation (parsed
/// natively by the JPEG/TIFF/WebP decoders in image 0.25; every other
/// format reports `NoTransforms` for free) is applied to the raster
/// before it is thumbnailed or cached, so cached rasters are upright by
/// construction and the hit path never re-applies rotation. Unreadable
/// EXIF degrades to a no-op, never a decode failure.
fn decode_upright(path: &Path) -> Option<DynamicImage> {
    use image::{metadata::Orientation, ImageDecoder};
    let mut decoder = image::ImageReader::open(path).ok()?.into_decoder().ok()?;
    // Must be read BEFORE from_decoder consumes the decoder.
    let orientation = decoder
        .orientation()
        .unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    Some(img)
}

/// Decode dispatch for the image arm: JPEG XL goes through jxl-oxide's
/// `ImageDecoder` integration (the image crate has no JXL support;
/// jxl-oxide applies the codestream's own orientation during decode —
/// do NOT also apply EXIF), everything else through [`decode_upright`].
/// `None` routes the caller to the mediainfo fallback either way.
fn decode_raster(path: &Path, mime: &mime::Mime) -> Option<DynamicImage> {
    if mime.subtype().as_str() == "jxl" {
        let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        let decoder = jxl_oxide::integration::JxlDecoder::new(reader).ok()?;
        return DynamicImage::from_decoder(decoder).ok();
    }
    decode_upright(path)
}

/// Upright source dimensions without a full decode, for the cache-hit
/// info line: a header-only read, w/h swapped for the transposing EXIF
/// orientations (Rotate90/270 ± flip). JPEG XL dispatches to jxl-oxide
/// (whose reported dims already have the codestream orientation
/// applied); `None` lets the caller fall back to the cached raster.
fn upright_source_dims(path: &Path, subtype: &str) -> Option<(u32, u32)> {
    use image::{metadata::Orientation, ImageDecoder};
    if subtype == "jxl" {
        let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
        let decoder = jxl_oxide::integration::JxlDecoder::new(reader).ok()?;
        return Some(decoder.dimensions());
    }
    let mut decoder = image::ImageReader::open(path).ok()?.into_decoder().ok()?;
    let (w, h) = decoder.dimensions();
    Some(
        match decoder.orientation().unwrap_or(Orientation::NoTransforms) {
            Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH => (h, w),
            _ => (w, h),
        },
    )
}

/// Image arm: decode once, derive the info lines from the decode itself
/// (dimensions are read before the thumbnail shrink).
fn native_image_preview(path: &Path, mime: &mime::Mime) -> Preview {
    let meta = path.metadata().ok();
    let byte_size = meta.as_ref().map(|m| m.len()).unwrap_or_default();
    let modified = meta
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    match decode_raster(path, mime) {
        Some(img) => {
            let info = image_info_lines(
                img.width(),
                img.height(),
                img.color(),
                byte_size,
                modified,
                mime.subtype().as_str(),
            );
            // thumbnail() UPscales smaller-than-bound sources; keep
            // originals that already fit — no reason to hold (or
            // persist) an inflated raster, the draw path rescales to
            // cell dimensions anyway.
            let img = if img.width() <= 960 && img.height() <= 540 {
                img
            } else {
                img.thumbnail(960, 540)
            };
            Preview::Image {
                img: Some(img),
                info,
            }
        }
        // Formats the image crate cannot decode (HEIC/AVIF/most RAW):
        // keep the mediainfo shell-out as the last-resort fallback so
        // exotic inputs still show an info block instead of nothing.
        None => {
            log::debug!(
                "native image decode failed, trying mediainfo: {}",
                path.display()
            );
            cmd_to_preview("mediainfo", mediainfo(path))
        }
    }
}

/// Image arm via the persistent raster cache: a hit decodes only the
/// small cached JPEG; a miss decodes the original via
/// `native_image_preview` and stores the thumbnail for next time.
fn cached_image_preview(path: &Path, modified: SystemTime, mime: &mime::Mime) -> Preview {
    cached_image_preview_in(raster_cache::dir(), path, modified, mime)
}

/// Dir-parameterized core of [`cached_image_preview`]; `None` (cache
/// disabled) degrades to exactly `native_image_preview`.
fn cached_image_preview_in(
    cache_dir: Option<&Path>,
    path: &Path,
    modified: SystemTime,
    mime: &mime::Mime,
) -> Preview {
    let mtime = mtime_secs(modified);
    if let Some(dir) = cache_dir {
        if let Some(img) = raster_cache::lookup_in(dir, path, mtime, raster_cache::KIND_IMAGE) {
            log::debug!("raster cache hit for {}", path.display());
            let info = cached_image_info(path, &img, mime.subtype().as_str());
            return Preview::Image {
                img: Some(img),
                info,
            };
        }
    }
    let preview = native_image_preview(path, mime);
    if let Some(dir) = cache_dir {
        if let Preview::Image { img: Some(img), .. } = &preview {
            if let Err(e) = raster_cache::store_in(dir, path, mtime, raster_cache::KIND_IMAGE, img)
            {
                log::debug!("raster cache store failed for {}: {e}", path.display());
            }
        }
    }
    preview
}

/// Info lines for a cache hit, without the full decode: original
/// dimensions via a header-only read (falling back to the cached
/// thumbnail's), color from the cached thumbnail (always Rgb8 after the
/// JPEG round-trip — accepted display drift), size/mtime from metadata.
/// The header dims are raw sensor dims — swapped for a transposing EXIF
/// orientation so the hit path agrees with the (upright) miss path.
fn cached_image_info(path: &Path, cached: &DynamicImage, subtype: &str) -> Vec<String> {
    let (width, height) =
        upright_source_dims(path, subtype).unwrap_or_else(|| (cached.width(), cached.height()));
    let meta = path.metadata().ok();
    let byte_size = meta.as_ref().map(|m| m.len()).unwrap_or_default();
    let modified = meta
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    image_info_lines(width, height, cached.color(), byte_size, modified, subtype)
}

/// Render an SVG to a white-backed RGBA raster, aspect-fit to the same
/// 960×540 bound as image previews. Unlike bitmap thumbnails, vectors
/// are *scaled up* to the bound — there is no source resolution to
/// preserve. `None` routes the caller to the text fallback.
fn native_svg_render(data: &[u8]) -> Option<DynamicImage> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree
        .size()
        .to_int_size()
        .scale_to(resvg::tiny_skia::IntSize::from_wh(960, 540)?); // aspect-fit
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())?;
    // White background: the JPEG cache stores RGB (no alpha), and a
    // white fill also makes premultiplied == straight alpha, so the
    // raw buffer converts directly to an RgbaImage.
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    let sx = size.width() as f32 / tree.size().width();
    let sy = size.height() as f32 / tree.size().height();
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(sx, sy),
        &mut pixmap.as_mut(),
    );
    let img = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.take())?;
    Some(image::DynamicImage::ImageRgba8(img))
}

/// Whole-file read, bounded at `max` bytes. Callers pick a cap any
/// sane instance of their format stays under (4 MiB SVG, 64 MiB font)
/// so a mislabeled multi-GB file cannot exhaust memory; a truncated
/// read simply fails the downstream parse and takes that arm's
/// fallback.
fn read_bounded(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(max).read_to_end(&mut data)?;
    Ok(data)
}

/// 4 MiB covers any sane SVG source (compressed or not).
const SVG_SOURCE_MAX: u64 = 4 * 1024 * 1024;
/// Fonts are small; even the big CJK faces stay well under 64 MiB.
const FONT_SOURCE_MAX: u64 = 64 * 1024 * 1024;
/// A .svgz may legitimately inflate ~10-50x; 32 MiB of XML is far past
/// any real SVG.
const SVGZ_INFLATED_MAX: u64 = 32 * 1024 * 1024;
/// PDFs above this take the stat block (or the image tier): the text
/// tier's value is capped anyway and load_filtered slurps the file.
const PDF_SOURCE_MAX: u64 = 32 * 1024 * 1024;
/// Total decompressed bytes lopdf may materialize per document.
const PDF_DECOMP_BUDGET: u64 = 64 * 1024 * 1024;
/// Longest retained preview line (chars) — a PDF can emit one
/// multi-megabyte text run with no newlines.
const PDF_LINE_MAX: usize = 1024;

/// usvg auto-detects the gzip magic (.svgz and .svg alike) and
/// inflates it with NO output bound (`decompress_svgz` is a plain
/// `read_to_end`), so the source-read cap alone would not stop a
/// crafted ~4 MiB svgz from inflating to gigabytes inside the
/// renderer — and an allocation failure aborts the process, skipping
/// the designed text fallback. Inflate the gzip layer ourselves,
/// bounded: over-budget or broken gzip is `None` (→ text fallback);
/// plain XML passes through untouched.
fn inflate_svgz_bounded(data: Vec<u8>) -> Option<Vec<u8>> {
    if !data.starts_with(&[0x1f, 0x8b]) {
        return Some(data);
    }
    let mut inflated = Vec::new();
    flate2::read::GzDecoder::new(&data[..])
        .take(SVGZ_INFLATED_MAX + 1)
        .read_to_end(&mut inflated)
        .ok()?;
    (inflated.len() as u64 <= SVGZ_INFLATED_MAX).then_some(inflated)
}

/// Info footer for a rendered SVG: the render's dimensions (the source
/// has no pixel dims), source size and mtime.
fn svg_info_lines(path: &Path, rendered: &DynamicImage) -> Vec<String> {
    let meta = path.metadata().ok();
    let byte_size = meta.as_ref().map(|m| m.len()).unwrap_or_default();
    let modified = meta
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    image_info_lines(
        rendered.width(),
        rendered.height(),
        rendered.color(),
        byte_size,
        modified,
        "svg",
    )
}

/// SVG arm via the persistent raster cache (kind `svg960`).
fn svg_preview(path: &Path, modified: SystemTime) -> Preview {
    svg_preview_in(raster_cache::dir(), path, modified)
}

/// Dir-parameterized core of [`svg_preview`]; `None` (cache disabled)
/// renders in-memory. Render/parse failure falls back to the bat/text
/// path (the raw XML) — never a broken `Preview::Image { img: None }`.
fn svg_preview_in(cache_dir: Option<&Path>, path: &Path, modified: SystemTime) -> Preview {
    let mtime = mtime_secs(modified);
    if let Some(dir) = cache_dir {
        if let Some(img) = raster_cache::lookup_in(dir, path, mtime, raster_cache::KIND_SVG) {
            log::debug!("raster cache hit for {}", path.display());
            let info = svg_info_lines(path, &img);
            return Preview::Image {
                img: Some(img),
                info,
            };
        }
    }
    let rendered = read_bounded(path, SVG_SOURCE_MAX)
        .ok()
        .and_then(inflate_svgz_bounded)
        .and_then(|data| native_svg_render(&data));
    match rendered {
        Some(img) => {
            if let Some(dir) = cache_dir {
                if let Err(e) =
                    raster_cache::store_in(dir, path, mtime, raster_cache::KIND_SVG, &img)
                {
                    log::debug!("raster cache store failed for {}: {e}", path.display());
                }
            }
            let info = svg_info_lines(path, &img);
            Preview::Image {
                img: Some(img),
                info,
            }
        }
        None => {
            log::debug!("svg render failed, falling back to bat: {}", path.display());
            bat_preview(path, false)
        }
    }
}

/// The rendered sample text — `raster_cache::KIND_FONT` encodes its
/// version (`s1`) and the px size (`24`): changing either MUST bump
/// that constant, or stale cache entries keep serving the old sample.
const FONT_SAMPLE_LINES: [&str; 2] = [
    "The quick brown fox jumps over the lazy dog",
    "0123456789 ?!&@%(){}[]",
];
const FONT_SAMPLE_PX: f32 = 24.0;

/// Family/style from the name table (IDs 1/2, unicode entries only),
/// scrubbed — name tables are attacker-controlled. Empty when the face
/// does not parse or carries no unicode names.
fn font_name_lines(data: &[u8]) -> Vec<String> {
    let face = match ttf_parser::Face::parse(data, 0) {
        Ok(face) => face,
        Err(_) => return Vec::new(),
    };
    let mut family = None;
    let mut style = None;
    for name in face.names() {
        if !name.is_unicode() {
            continue;
        }
        match name.name_id {
            ttf_parser::name_id::FAMILY => family = family.or_else(|| name.to_string()),
            ttf_parser::name_id::SUBFAMILY => style = style.or_else(|| name.to_string()),
            _ => {}
        }
    }
    [family, style]
        .into_iter()
        .flatten()
        .map(scrub_line)
        .collect()
}

/// Rasterise the sample rows (pangram + digits/symbols) at 24 px onto a
/// white grayscale canvas — dark glyphs on white so the JPEG round-trip
/// of the raster cache stays clean. Returns the name lines alongside.
/// `Err` (not an sfnt — includes woff/woff2, see the dispatch comment)
/// routes the caller to the stat fallback.
fn native_font_sample(data: &[u8]) -> anyhow::Result<(Vec<String>, DynamicImage)> {
    use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
    let names = font_name_lines(data);
    let font = FontRef::try_from_slice(data)?;
    let scaled = font.as_scaled(PxScale::from(FONT_SAMPLE_PX));
    let line_height = scaled.height() + scaled.line_gap();
    let margin = 8.0f32;
    let width: u32 = 960;
    // The metrics come straight from the (attacker-controlled) face: a
    // crafted ascent over a tiny units_per_em would demand an
    // arbitrarily tall canvas. Clamp — 512 px is far above any sane
    // 2-row sample at 24 px.
    let height_px = 2.0 * line_height + 2.0 * margin;
    anyhow::ensure!(height_px.is_finite(), "non-finite font metrics");
    let height = height_px.ceil().clamp(16.0, 512.0) as u32;
    let mut img = image::GrayImage::from_pixel(width, height, image::Luma([255u8]));
    for (row, text) in FONT_SAMPLE_LINES.iter().enumerate() {
        let baseline = margin + scaled.ascent() + row as f32 * line_height;
        let mut x = margin;
        for c in text.chars() {
            // Position is set on the glyph BEFORE outlining (there is
            // no into_glyph_at); advance comes from the scaled font.
            let mut glyph = scaled.scaled_glyph(c);
            glyph.position = ab_glyph::point(x, baseline);
            x += scaled.h_advance(glyph.id);
            if x > width as f32 {
                break;
            }
            if let Some(og) = scaled.outline_glyph(glyph) {
                let bounds = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if (0..width as i32).contains(&px) && (0..height as i32).contains(&py) {
                        let p = img.get_pixel_mut(px as u32, py as u32);
                        p.0[0] = p.0[0].min(255u8.saturating_sub((cov * 255.0) as u8));
                    }
                });
            }
        }
    }
    Ok((names, DynamicImage::ImageLuma8(img)))
}

/// Info footer for a font preview: name lines (when readable) plus the
/// size/mtime block from metadata.
fn font_info_lines(path: &Path, mut names: Vec<String>) -> Vec<String> {
    use time::OffsetDateTime;
    if !names.is_empty() {
        names.push(String::new());
    }
    if let Ok(meta) = path.metadata() {
        names.push(format!(
            "Size:     {}",
            crate::util::file_size_str(meta.len())
        ));
        if let Ok(modified) = meta.modified() {
            let t = OffsetDateTime::from(modified);
            names.push(format!(
                "Modified: {}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ));
        }
    }
    names
}

/// Font arm via the persistent raster cache (kind `font-s1-24`).
fn font_preview(path: &Path, modified: SystemTime, mime: &mime::Mime) -> Preview {
    font_preview_in(raster_cache::dir(), path, modified, mime)
}

/// Dir-parameterized core of [`font_preview`]; `None` (cache disabled)
/// rasterises in-memory. A cache hit re-reads the source only for the
/// name lines (best-effort — unreadable source keeps the metadata
/// lines). Parse failure falls back to the stat block. Both reads are
/// bounded ([`FONT_SOURCE_MAX`]): the sniff routes arbitrary binaries
/// here (the ttf magic is ambiguous), and a mislabeled multi-GB file
/// must not be slurped whole — same rationale as the SVG arm.
fn font_preview_in(
    cache_dir: Option<&Path>,
    path: &Path,
    modified: SystemTime,
    mime: &mime::Mime,
) -> Preview {
    let mtime = mtime_secs(modified);
    if let Some(dir) = cache_dir {
        if let Some(img) = raster_cache::lookup_in(dir, path, mtime, raster_cache::KIND_FONT) {
            log::debug!("raster cache hit for {}", path.display());
            let names = read_bounded(path, FONT_SOURCE_MAX)
                .map(|data| font_name_lines(&data))
                .unwrap_or_default();
            return Preview::Image {
                img: Some(img),
                info: font_info_lines(path, names),
            };
        }
    }
    let sample = read_bounded(path, FONT_SOURCE_MAX)
        .map_err(anyhow::Error::from)
        .and_then(|data| native_font_sample(&data));
    match sample {
        Ok((names, img)) => {
            if let Some(dir) = cache_dir {
                if let Err(e) =
                    raster_cache::store_in(dir, path, mtime, raster_cache::KIND_FONT, &img)
                {
                    log::debug!("raster cache store failed for {}: {e}", path.display());
                }
            }
            Preview::Image {
                img: Some(img),
                info: font_info_lines(path, names),
            }
        }
        Err(e) => {
            // Also hit by binary junk the sniff mistakes for a font —
            // the ttf magic (00 01 00 00) is notoriously ambiguous.
            log::debug!(
                "font sample failed, falling back to stat: {}: {e}",
                path.display()
            );
            stat_preview(path, mime)
        }
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
        return cmd_to_preview("mediainfo", mediainfo(path.as_ref()));
    }
    let modified = mtime_secs(modified);

    // Use ffmpeg — unless the user opted out of on-disk thumbnails
    // entirely (`preview_cache = false` promises "nothing about your
    // files is written to disk", and an ffmpeg thumbnail IS a write).
    let preview = video_thumbnail_dir()
        .ok_or_else(|| anyhow::anyhow!("on-disk thumbnails disabled (preview_cache = false)"))
        .and_then(|dir| ffmpeg_thumbnail(dir, &path, modified));
    match preview {
        Ok(preview) => preview,
        Err(e) => {
            // Expected e.g. for videos shorter than the 10s thumbnail
            // seek - not worth an on-screen error on every visit.
            log::debug!("no ffmpeg thumbnail, falling back to mediainfo: {e}");
            cmd_to_preview("mediainfo", mediainfo(path.as_ref()))
        }
    }
}

/// Where video thumbnails may be written: the persistent raster cache
/// when available, the temp-dir fallback when the cache is enabled but
/// could not be set up, and `None` when the user opted out via
/// `preview_cache = false` — then there is no thumbnail location at all
/// and video previews degrade to mediainfo text.
fn video_thumbnail_dir() -> Option<&'static Path> {
    video_thumbnail_dir_from(
        raster_cache::dir(),
        raster_cache::persistence_enabled(),
        fallback_thumbnail_dir,
    )
}

/// Pure core of [`video_thumbnail_dir`], parameterized for tests.
fn video_thumbnail_dir_from(
    cache_dir: Option<&'static Path>,
    enabled: bool,
    fallback: fn() -> &'static Path,
) -> Option<&'static Path> {
    match cache_dir {
        Some(dir) => Some(dir),
        None if enabled => Some(fallback()),
        None => None,
    }
}

/// Thumbnail dir when the persistent raster cache is enabled but
/// unavailable (could not be resolved/created): today's exact pre-cache
/// behavior — `temp_dir()/rfm-thumbnails` with one-time 7-day
/// housekeeping (a thumbnail is never referenced again once its video
/// changed, so don't litter the temp-dir forever).
fn fallback_thumbnail_dir() -> &'static Path {
    static THUMBNAIL_DIR: OnceCell<PathBuf> = OnceCell::new();
    THUMBNAIL_DIR.get_or_init(|| {
        let dir = temp_dir().join("rfm-thumbnails");
        let _ = raster_cache::create_dir_all_private(&dir);
        prune_older_than(&dir, Duration::from_secs(7 * 24 * 60 * 60));
        dir
    })
}

/// Deadline for every external raster producer (ffmpeg, pdftoppm/mutool).
const EXTERNAL_RENDER_DEADLINE: Duration = Duration::from_secs(10);

/// Run `cmd` to completion — but never past `deadline`. A hung or
/// pathological producer (corrupt file, stalled network mount) must
/// cost one preview slot for `deadline` at most, not forever.
///
/// stdout/stderr are piped and drained on threads (`read_to_end`), so
/// a chatty child can never deadlock against a full 64 KiB pipe while
/// the main thread only polls `try_wait` (~25ms). On deadline: kill,
/// then wait — the kill-then-reap discipline from `tar_list` (the kill
/// fails harmlessly if the child just exited; without the wait every
/// timeout would leak a zombie) — then join the drain threads (EOF
/// arrives when the killed child's pipes close) and return
/// `Err(TimedOut)`. Partial-output cleanup is the *caller's* job: only
/// it knows which part file its command was writing.
fn run_bounded(
    cmd: &mut std::process::Command,
    deadline: Duration,
) -> io::Result<std::process::Output> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let end = std::time::Instant::now() + deadline;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) | Err(_) if std::time::Instant::now() >= end => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("external command exceeded the {deadline:?} deadline"),
                ));
            }
            Ok(None) | Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    Ok(std::process::Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Build (or fetch) the `vid120` thumbnail of `path` in `dir` and wrap
/// it in an image preview. Lookups go through the cache's corrupt-entry
/// rule: a decode failure deletes the entry and regenerates — never a
/// blank preview served until the 30-day prune.
fn ffmpeg_thumbnail(dir: &Path, path: impl AsRef<Path>, modified: u64) -> anyhow::Result<Preview> {
    if let Some(img) =
        raster_cache::lookup_in(dir, path.as_ref(), modified, raster_cache::KIND_VIDEO)
    {
        log::debug!("raster cache hit for {}", path.as_ref().display());
        return Ok(Preview::Image {
            img: Some(img),
            info: mediainfo(path).unwrap_or_default(),
        });
    }
    let name = raster_cache::entry_name(path.as_ref(), modified, raster_cache::KIND_VIDEO);
    let thumbnail = dir.join(&name);
    log::debug!("generating thumbnail {}", thumbnail.display());
    // A mid-session `rm -rf` of the cache dir must not cost the session
    // its video previews — recreate the dir before ffmpeg writes to it.
    let _ = raster_cache::create_dir_all_private(dir);
    // ffmpeg writes to a same-dir temp name and the result is renamed
    // into place, so the lookup fast path above can never see a partial
    // file. The extension stays LAST so ffmpeg's container inference
    // still works.
    let part = dir.join(format!("{name}.{}.part.jpg", raster_cache::part_token()));
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
        .arg(&part);
    cmd.stdin(Stdio::null());
    let out = match run_bounded(&mut cmd, EXTERNAL_RENDER_DEADLINE) {
        Ok(out) => out,
        Err(e) => {
            // Spawn failure or deadline kill: a killed ffmpeg may have
            // left a half-written part file — drop it before
            // propagating (the caller falls back to mediainfo).
            let _ = std::fs::remove_file(&part);
            return Err(e.into());
        }
    };
    // ffmpeg fails without writing the thumbnail e.g. for corrupt
    // input; report that instead of building a preview from a file
    // that was never written (the caller falls back to mediainfo on
    // Err).
    if !out.status.success() || !part.exists() {
        // A failed run may still have created a partial file; drop it.
        let _ = std::fs::remove_file(&part);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        anyhow::bail!(
            "ffmpeg did not produce a thumbnail ({}): {}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        );
    }
    std::fs::rename(&part, &thumbnail)?;
    raster_cache::cleanup_stale(dir, path.as_ref(), modified);
    // Decode via the same corrupt-entry rule: if even the fresh entry
    // does not decode, it is deleted and the caller falls back to
    // mediainfo instead of showing a blank image preview.
    let img = raster_cache::lookup_in(dir, path.as_ref(), modified, raster_cache::KIND_VIDEO)
        .ok_or_else(|| anyhow::anyhow!("freshly generated thumbnail failed to decode"))?;
    Ok(Preview::Image {
        img: Some(img),
        info: mediainfo(path).unwrap_or_default(),
    })
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

/// `bat_preview`'s scrubbing convention, applied to every line-based
/// native preview: attacker-controlled strings (archive member names,
/// audio tags, certificate fields) may contain `\r`/`\n`, which must
/// not break the one-entry-one-line invariant of the preview pane.
fn scrub_line(line: String) -> String {
    if line.contains(['\r', '\n']) {
        line.replace(['\r', '\n'], "")
    } else {
        line
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
    // Same 128-line cap as every other line-based preview: a hostile
    // certificate with thousands of SAN entries stays bounded.
    lines.truncate(128);
    Ok(lines.into_iter().map(scrub_line).collect())
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
        lines.push(scrub_line(format!(
            "{:>8}  {}",
            crate::util::file_size_str(entry.size()),
            entry.name()
        )));
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
        let entry = match entry {
            Ok(entry) => entry,
            // A stream truncated mid-scan (the compressed arms cap
            // their decompression budget at TAR_SCAN_MAX) ends the
            // listing; only a stream yielding nothing is an error.
            Err(_) if !lines.is_empty() => break,
            Err(e) => return Err(e.into()),
        };
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
        lines.push(scrub_line(format!(
            "{} {:>8}  {}",
            unix_mode::to_string(type_bits | (header.mode().unwrap_or(0) & 0o7777)),
            crate::util::file_size_str(header.size().unwrap_or(0)),
            entry.path()?.display()
        )));
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

fn native_gz_preview(path: &Path) -> anyhow::Result<Preview> {
    native_compressed_preview(flate2::read::GzDecoder::new(File::open(path)?))
}

/// Shared tail of every compressed-single-stream arm (gzip, zstd, xz,
/// bzip2): decompress the head, sniff the tar magic (`ustar` at offset
/// 257 - covers both POSIX `ustar\0` and GNU `ustar  `), then either
/// chain head+rest into `native_tar_list` or show the decompressed
/// head as text (bounded 64 KiB read, lossy UTF-8, 128 lines, `\r`
/// scrubbed like `bat_preview`) - a compressed non-tar file shows its
/// text head, never a tar error.
fn native_compressed_preview(mut decoder: impl Read) -> anyhow::Result<Preview> {
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
        // tar over a non-Seek stream reaches the next header by
        // read-and-discarding the content in between: a crafted first
        // entry declaring a huge size would otherwise force the codec
        // to decompress that entire span (a CPU/time DoS on the
        // preview task — memory stays bounded) just to reach header
        // #2. The budget truncates the listing instead.
        const TAR_SCAN_MAX: u64 = 64 * 1024 * 1024;
        return Ok(Preview::Text {
            lines: native_tar_list(rest.take(TAR_SCAN_MAX))?,
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

/// zstd arm (`.zst`/`.tzst`/`.tar.zst`): pure-Rust ruzstd decode into
/// the shared tar-or-text logic; a system tar built with zstd stays as
/// the fallback (exactly the old fragile path, now demoted).
fn zst_preview(path: &Path) -> Preview {
    let native = File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(|f| {
            Ok(ruzstd::decoding::StreamingDecoder::new(
                io::BufReader::new(f),
            )?)
        })
        .and_then(native_compressed_preview);
    match native {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!("native zstd preview failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

/// xz arm (`.xz`/`.txz`/`.tar.xz`): pure-Rust lzma-rust2 decode into
/// the shared tar-or-text logic; the tar binary stays as the fallback.
fn xz_preview(path: &Path) -> Preview {
    let native = File::open(path).map_err(anyhow::Error::from).and_then(|f| {
        native_compressed_preview(lzma_rust2::XzReader::new(io::BufReader::new(f), true))
    });
    match native {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!("native xz preview failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

/// bzip2 arm (`.bz2`/`.tbz2`/`.tar.bz2`): pure-Rust libbz2-rs decode
/// into the shared tar-or-text logic; the tar binary stays as the
/// fallback.
fn bz2_preview(path: &Path) -> Preview {
    let native = File::open(path)
        .map_err(anyhow::Error::from)
        .and_then(|f| native_compressed_preview(bzip2::read::BzDecoder::new(f)));
    match native {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!("native bzip2 preview failed, trying tar: {e}");
            cmd_to_preview("tar", tar_list(path))
        }
    }
}

/// sevenz-rust trusts the 32-byte start header completely: it
/// allocates `vec![0; next_header_size]` BEFORE reading a single
/// header byte, so a crafted tiny .7z declaring a multi-TB header
/// aborts the whole process — an allocation failure is not an `Err`
/// the `7z l` fallback could catch. Validate the start header against
/// the real file length first; implausible files become a normal
/// `Err` (→ fallback). The 1 MiB cap is far past the metadata of any
/// archive whose first 128 entries we would list — genuinely bigger
/// headers just take the `7z l` fallback. (Varint counts INSIDE a
/// CRC-valid header, e.g. num_files, are still trusted by the crate;
/// bounding those would mean reimplementing its header parser.)
fn plausible_sevenz_start_header(path: &Path) -> anyhow::Result<()> {
    const SEVENZ_MAGIC: [u8; 6] = [b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c];
    const SEVENZ_HEADER_MAX: u64 = 1024 * 1024;
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut start = [0u8; 32];
    file.read_exact(&mut start)?;
    anyhow::ensure!(start[..6] == SEVENZ_MAGIC, "not a 7z archive");
    let offset = u64::from_le_bytes(start[12..20].try_into().expect("8 bytes"));
    let size = u64::from_le_bytes(start[20..28].try_into().expect("8 bytes"));
    anyhow::ensure!(
        size <= SEVENZ_HEADER_MAX,
        "implausible next-header size {size}"
    );
    let end = offset.checked_add(size).and_then(|e| e.checked_add(32));
    anyhow::ensure!(
        end.is_some_and(|e| e <= len),
        "the next header lies outside the file"
    );
    Ok(())
}

/// List a 7z archive natively: `size  name` per entry, capped at 128.
/// `Archive::open` reads only the archive metadata - nothing is
/// extracted or decompressed.
fn native_sevenz_list(path: &Path) -> anyhow::Result<Vec<String>> {
    plausible_sevenz_start_header(path)?;
    let archive = sevenz_rust::Archive::open(path)?;
    Ok(archive
        .files
        .iter()
        .take(128)
        .map(|entry| {
            scrub_line(format!(
                "{:>8}  {}",
                crate::util::file_size_str(entry.size()),
                entry.name()
            ))
        })
        .collect())
}

/// Native-first 7z arm. There was no 7z shell-out before this arm; the
/// `7z l` fallback is added fallback-only, so encrypted-header archives
/// still get a listing when the binary exists (else error text - the
/// preview is never empty).
fn sevenz_preview(path: &Path) -> Preview {
    match native_sevenz_list(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native 7z list failed, trying 7z: {e}");
            cmd_to_preview(
                "7z",
                std::process::Command::new("7z")
                    .arg("l")
                    .arg(path)
                    .output()
                    // `lines()` trims the \r\n pair but not embedded
                    // \r; member names are attacker-controlled, so the
                    // scrub convention applies here too.
                    .and_then(|o| {
                        o.stdout
                            .lines()
                            .take(128)
                            .map(|l| l.map(scrub_line))
                            .collect()
                    }),
            )
        }
    }
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

/// Which primary part(s) of a zip-container document carry the text.
#[derive(Debug, Clone, Copy)]
enum DocKind {
    Docx,
    /// Also covers ods/odp — all OpenDocument flavors keep their body
    /// in `content.xml` with `text:p` paragraphs.
    Odt,
    Xlsx,
    Pptx,
    Epub,
}

/// Streaming tag-strip of one XML document: text nodes accumulate and
/// flush as one line per closing paragraph tag (`p` covers `w:p`,
/// `text:p`, DrawingML `a:p` and XHTML `p` via the local name; `t`
/// gives xlsx one shared string per line). Text inside
/// `style`/`script`/`title` is skipped (epub XHTML heads — a leaked
/// `<title>` would glue itself onto the first paragraph). Appends to
/// `lines`, stopping at the
/// 128-line cap; every line goes through `scrub_line` — document text
/// is attacker-controlled.
fn xml_text_lines<R: BufRead>(
    reader: R,
    paragraph_tags: &[&[u8]],
    lines: &mut Vec<String>,
) -> anyhow::Result<()> {
    use quick_xml::events::Event;
    let mut xml = quick_xml::Reader::from_reader(reader);
    let mut buf = Vec::new();
    let mut current = String::new();
    let mut skip_depth = 0usize;
    while lines.len() < 128 {
        match xml.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) => {
                if matches!(e.local_name().as_ref(), b"style" | b"script" | b"title") {
                    skip_depth += 1;
                }
            }
            Event::Text(t) if skip_depth == 0 => current.push_str(&t.decode()?),
            // quick-xml 0.38 reports `&…;` separately: resolve char
            // refs and the predefined entities, drop unknown ones.
            Event::GeneralRef(r) if skip_depth == 0 => {
                if let Ok(Some(c)) = r.resolve_char_ref() {
                    current.push(c);
                } else {
                    match r.decode()?.as_ref() {
                        "amp" => current.push('&'),
                        "lt" => current.push('<'),
                        "gt" => current.push('>'),
                        "quot" => current.push('"'),
                        "apos" => current.push('\''),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                let local = e.local_name();
                if matches!(local.as_ref(), b"style" | b"script" | b"title") {
                    skip_depth = skip_depth.saturating_sub(1);
                } else if paragraph_tags.contains(&local.as_ref()) {
                    let line = scrub_line(std::mem::take(&mut current));
                    if !line.trim().is_empty() {
                        lines.push(line);
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(())
}

/// Tag-strip one archive member into `lines`. The decompressed member
/// read is bounded (512 KiB) so a zip bomb cannot exhaust memory.
fn member_text_lines(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
    paragraph_tags: &[&[u8]],
    lines: &mut Vec<String>,
) -> anyhow::Result<()> {
    let member = archive.by_name(name)?;
    xml_text_lines(
        io::BufReader::new(member.take(512 * 1024)),
        paragraph_tags,
        lines,
    )
}

/// First attribute value of `attr` on any `tag` element in the stream —
/// enough XML to follow epub's container.xml (`rootfile`/`full-path`).
fn xml_first_attr<R: BufRead>(reader: R, tag: &[u8], attr: &[u8]) -> anyhow::Result<String> {
    use quick_xml::events::Event;
    let mut xml = quick_xml::Reader::from_reader(reader);
    let mut buf = Vec::new();
    loop {
        match xml.read_event_into(&mut buf)? {
            Event::Eof => anyhow::bail!(
                "no {} attribute on any {} element",
                String::from_utf8_lossy(attr),
                String::from_utf8_lossy(tag)
            ),
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == tag => {
                for a in e.attributes() {
                    let a = a?;
                    if a.key.local_name().as_ref() == attr {
                        return Ok(a.unescape_value()?.into_owned());
                    }
                }
            }
            _ => {}
        }
        buf.clear();
    }
}

/// The spine reading order of an epub: parse the OPF's manifest
/// (id → href) and spine (idref order), resolve hrefs against the OPF's
/// directory. Any missing piece is an `Err` (→ zip-listing fallback).
fn epub_spine_docs(
    archive: &mut zip::ZipArchive<File>,
    opf_path: &str,
) -> anyhow::Result<Vec<String>> {
    use quick_xml::events::Event;
    let opf_dir = match opf_path.rfind('/') {
        Some(idx) => &opf_path[..=idx],
        None => "",
    };
    let opf = archive.by_name(opf_path)?;
    let mut xml = quick_xml::Reader::from_reader(io::BufReader::new(opf.take(512 * 1024)));
    let mut buf = Vec::new();
    let mut manifest: Vec<(String, String)> = Vec::new(); // id → href
    let mut spine: Vec<String> = Vec::new(); // idrefs in order
    loop {
        match xml.read_event_into(&mut buf)? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"item" => {
                    let mut id = None;
                    let mut href = None;
                    for a in e.attributes() {
                        let a = a?;
                        match a.key.local_name().as_ref() {
                            b"id" => id = Some(a.unescape_value()?.into_owned()),
                            b"href" => href = Some(a.unescape_value()?.into_owned()),
                            _ => {}
                        }
                    }
                    if let (Some(id), Some(href)) = (id, href) {
                        manifest.push((id, href));
                    }
                }
                b"itemref" => {
                    for a in e.attributes() {
                        let a = a?;
                        if a.key.local_name().as_ref() == b"idref" {
                            spine.push(a.unescape_value()?.into_owned());
                        }
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }
    let docs: Vec<String> = spine
        .iter()
        .filter_map(|idref| manifest.iter().find(|(id, _)| id == idref))
        .map(|(_, href)| format!("{opf_dir}{href}"))
        .collect();
    anyhow::ensure!(!docs.is_empty(), "empty spine in {opf_path}");
    Ok(docs)
}

/// Extract the text lines of a zip-container document (the plan's C2
/// group). Any missing/corrupt piece is an `Err` — the caller then
/// treats the file as a plain zip so the preview is never empty.
fn native_doc_lines(path: &Path, kind: DocKind) -> anyhow::Result<Vec<String>> {
    let mut archive = zip::ZipArchive::new(File::open(path)?)?;
    let mut lines = Vec::new();
    match kind {
        DocKind::Docx => member_text_lines(&mut archive, "word/document.xml", &[b"p"], &mut lines)?,
        DocKind::Odt => member_text_lines(&mut archive, "content.xml", &[b"p"], &mut lines)?,
        DocKind::Xlsx => {
            member_text_lines(&mut archive, "xl/sharedStrings.xml", &[b"t"], &mut lines)?
        }
        DocKind::Pptx => {
            let mut slides: Vec<String> = archive
                .file_names()
                .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
                .map(str::to_owned)
                .collect();
            // Ascending slide number: (len, lexicographic) sorts
            // slide2.xml before slide10.xml without parsing the N.
            slides.sort_by_key(|n| (n.len(), n.clone()));
            anyhow::ensure!(!slides.is_empty(), "no slides in the container");
            for name in slides {
                if lines.len() >= 128 {
                    break;
                }
                member_text_lines(&mut archive, &name, &[b"p"], &mut lines)?;
            }
        }
        DocKind::Epub => {
            let container = archive.by_name("META-INF/container.xml")?;
            let opf_path = xml_first_attr(
                io::BufReader::new(container.take(512 * 1024)),
                b"rootfile",
                b"full-path",
            )?;
            for doc in epub_spine_docs(&mut archive, &opf_path)? {
                if lines.len() >= 128 {
                    break;
                }
                member_text_lines(&mut archive, &doc, &[b"p"], &mut lines)?;
            }
        }
    }
    Ok(lines)
}

/// Office/OpenDocument/epub arm: extracted text, else the file is
/// treated as the plain zip it physically is (whose arm already chains
/// into the unzip/error-text fallback) — the preview is never empty.
fn doc_preview(path: &Path, kind: DocKind) -> Preview {
    match native_doc_lines(path, kind) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!(
                "{kind:?} text extraction failed, falling back to zip listing: {}: {e}",
                path.display()
            );
            zip_preview(path)
        }
    }
}

/// Table listing of a SQLite database: read-only open, zero busy
/// timeout (fail fast, never block the panel task), one `count  name`
/// line per table from sqlite_master. A count failure for one table
/// (corrupt page) prints `?` for that row; only an open/master-query
/// failure is `Err` (→ stat fallback). Names are attacker-controlled:
/// quoted for the COUNT (embedded quotes doubled) and scrubbed for
/// display. 2 header lines + 126 tables = the shared 128-line cap.
fn native_sqlite_lines(path: &Path) -> anyhow::Result<Vec<String>> {
    // COUNT(*) is a full table/index scan: on a multi-GB database (a
    // browser places.sqlite) up to 126 of them would churn disk/CPU
    // in the preview task for a long time. Past this source size the
    // counts print `?` (the same placeholder as a corrupt table) —
    // the table NAMES still list instantly from sqlite_master.
    const SQLITE_COUNT_MAX_BYTES: u64 = 64 * 1024 * 1024;
    native_sqlite_lines_bounded(path, SQLITE_COUNT_MAX_BYTES)
}

/// Budget-parameterized core of [`native_sqlite_lines`].
fn native_sqlite_lines_bounded(path: &Path, count_budget: u64) -> anyhow::Result<Vec<String>> {
    use rusqlite::OpenFlags;
    let count_rows = path.metadata()?.len() <= count_budget;
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::ZERO)?;
    let mut stmt =
        conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    let mut lines = vec![
        format!(
            "SQLite database · {} table{}",
            names.len(),
            if names.len() == 1 { "" } else { "s" }
        ),
        String::new(),
    ];
    for name in names.iter().take(126) {
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let count = if count_rows {
            conn.query_row(&format!("SELECT COUNT(*) FROM {quoted}"), [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|n| n.to_string())
            .unwrap_or_else(|_| String::from("?"))
        } else {
            String::from("?")
        };
        lines.push(scrub_line(format!("{count:>8}  {name}")));
    }
    Ok(lines)
}

/// SQLite arm (sniff-routed via the `SQLite format 3\0` magic): native
/// listing, stat block as the fallback for locked/garbage databases.
fn sqlite_preview(path: &Path, mime: &mime::Mime) -> Preview {
    match native_sqlite_lines(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!(
                "sqlite listing failed, falling back to stat: {}: {e}",
                path.display()
            );
            stat_preview(path, mime)
        }
    }
}

/// The `pdf_render` config switch, set once at startup (like
/// `raster_cache::init`). Unset — e.g. in unit tests — counts as
/// opted out, keeping every test hermetic.
static PDF_RENDER: OnceCell<bool> = OnceCell::new();

/// Wire the `pdf_render` config switch (called once from main).
pub fn set_pdf_render(enabled: bool) {
    let _ = PDF_RENDER.set(enabled);
}

fn pdf_render_enabled() -> bool {
    PDF_RENDER.get().copied().unwrap_or(false)
}

/// The external PDF renderer for the optional image tier.
#[derive(Clone, Copy, Debug)]
enum PdfRenderer {
    Pdftoppm,
    Mutool,
}

/// OnceCell probe, ffmpeg-style but two candidates: `pdftoppm -v`,
/// else `mutool -v`. Present = spawned AND (exit success OR output
/// contains "version") — the -v exit codes are not uniform across
/// packagings. Only consulted when pdf_render_enabled(), so the base
/// install never spawns probes.
fn pdf_renderer() -> Option<PdfRenderer> {
    static RENDERER: OnceCell<Option<PdfRenderer>> = OnceCell::new();
    *RENDERER.get_or_init(|| {
        let present = |bin: &str| {
            std::process::Command::new(bin)
                .arg("-v")
                .stdin(Stdio::null())
                .output()
                .map(|out| {
                    out.status.success()
                        || String::from_utf8_lossy(&out.stdout).contains("version")
                        || String::from_utf8_lossy(&out.stderr).contains("version")
                })
                .unwrap_or(false)
        };
        if present("pdftoppm") {
            Some(PdfRenderer::Pdftoppm)
        } else if present("mutool") {
            Some(PdfRenderer::Mutool)
        } else {
            None
        }
    })
}

thread_local! {
    /// Remaining decompression budget for the load_filtered call on
    /// this thread (the FilterFunc is a plain fn pointer, so the
    /// budget travels beside it; rayon is disabled in our lopdf
    /// features, the filter runs on the loading thread).
    static PDF_BUDGET: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Max single-stage deflate/LZW expansion ratio (1032:1).
const MAX_INFLATE_RATIO: u64 = 1032;

/// True for a filter name that inflates its input (deflate / LZW).
fn is_inflating_filter(name: &[u8]) -> bool {
    name == b"FlateDecode" || name == b"LZWDecode"
}

/// Counting-decompress `content` as zlib through a `Take`'d sink,
/// never reading past `budget + 1` output bytes. A mid-stream zlib
/// error bounds lopdf's own partial `read_to_end` at the same offset,
/// so the returned count matches what lopdf would materialize.
fn flate_output_len(content: &[u8], budget: u64) -> u64 {
    let mut n: u64 = 0;
    let mut decoder = flate2::read::ZlibDecoder::new(content).take(budget.saturating_add(1));
    let mut buf = [0u8; 16 * 1024];
    loop {
        match decoder.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => n += k as u64,
            Err(_) => break,
        }
    }
    n
}

/// How many bytes lopdf would materialize when decompressing `content`
/// through `filters`, bounded probes only — never more than
/// `budget + 1` counting bytes. `None` means over budget: drop it.
///
/// - FlateDecode (sole filter): counting-decompress exactly.
/// - Any other chain containing an inflating filter (LZWDecode, or a
///   multi-stage chain like [FlateDecode FlateDecode] / [ASCII85Decode
///   FlateDecode] whose raw bytes cannot be counting-decoded): charge
///   pessimistically at `MAX_INFLATE_RATIO ^ k`, where `k` is the
///   number of inflating stages. A k-deep flate chain inflates up to
///   1032^k (each stage decompresses the previous stage's output), so
///   a single 1032:1 multiply undercounts — this raises it to the true
///   upper bound (non-inflating stages like ASCII85 only shrink).
/// - DCT/ASCII85/plain pass at zero: lopdf never inflates DCT,
///   ASCII85 shrinks, and plain content is bounded by PDF_SOURCE_MAX.
fn inflation_charge(content: &[u8], filters: &[&[u8]], budget: u64) -> Option<u64> {
    if filters.len() == 1 && filters[0] == b"FlateDecode" {
        let n = flate_output_len(content, budget);
        return (n <= budget).then_some(n);
    }
    let stages = filters.iter().filter(|f| is_inflating_filter(f)).count() as u32;
    if stages > 0 {
        let ratio = MAX_INFLATE_RATIO.checked_pow(stages).unwrap_or(u64::MAX);
        let charge = (content.len() as u64).saturating_mul(ratio);
        return (charge <= budget).then_some(charge);
    }
    Some(0)
}

/// How many bytes lopdf would materialize when decompressing `stream`.
/// `None` means over budget: drop the stream. See [`inflation_charge`].
fn pdf_stream_charge(stream: &lopdf::Stream, budget: u64) -> Option<u64> {
    match stream.filters() {
        Ok(filters) => inflation_charge(&stream.content, &filters, budget),
        // No/unreadable Filter entry: plain content, nothing inflates.
        Err(_) => Some(0),
    }
}

/// lopdf's decompress_zlib/_lzw are unbounded read_to_ends — verify
/// every stream BEFORE lopdf inflates it (`load_filtered` calls this
/// for each parsed object; returning `None` drops it). Over budget =>
/// drop the object AND zero the budget: one bomb costs at most
/// budget+1 counting bytes, every later stream then drops after <=1
/// byte — total work across a hostile file stays O(2 * budget). A
/// dropped stream leaves a hole; lopdf then errs or extracts nothing
/// and the caller degrades — exactly right for a hostile file.
fn pdf_guard_filter(
    id: (u32, u16),
    object: &mut lopdf::Object,
) -> Option<((u32, u16), lopdf::Object)> {
    if let lopdf::Object::Stream(stream) = &*object {
        let budget = PDF_BUDGET.with(|b| b.get());
        match pdf_stream_charge(stream, budget) {
            Some(charge) => PDF_BUDGET.with(|b| b.set(budget - charge)),
            None => {
                PDF_BUDGET.with(|b| b.set(0));
                log::debug!("pdf stream {id:?} exceeds the decompression budget, dropping it");
                return None;
            }
        }
    }
    // The clone is the FilterFunc API's shape; it is bounded by
    // PDF_SOURCE_MAX (the raw, still-compressed bytes).
    Some((id, object.clone()))
}

/// The `pdf_guard_filter` above bounds every *object-loop* stream, but
/// lopdf inflates PDF-1.5+ cross-reference STREAMS (`/Type /XRef` with
/// `/Filter /FlateDecode`) far earlier — inside `xref_and_trailer ->
/// decode_xref_stream -> Stream::decompress` (an unbounded
/// `read_to_end`) — before any `FilterFunc` exists in the flow. That
/// path (and the `/Prev` / `/XRefStm` chain of linked xref streams) is
/// therefore exempt from `PDF_BUDGET`; a small file whose xref stream
/// inflates to GiBs (compression-ratio bomb + huge `/Size`) would OOM
/// the process on plain cursor navigation. Since `load_filtered`'s
/// guard cannot reach it, we pre-scan the raw bytes and reject the file
/// before handing it to lopdf.
///
/// Conservative by construction: it returns `true` (let lopdf proceed)
/// for every ambiguous or classic-`xref`-table case, and only `false`
/// when a compressed xref stream is positively measured over budget.
/// It scans every stream object carrying the `/XRef` type marker —
/// which covers whichever ones the `startxref`/`/Prev`/`/XRefStm` chain
/// actually reaches — and charges each with the same
/// counting-decompress / pessimistic-ratio logic as the object guard.
fn pdf_xref_streams_within_budget(bytes: &[u8], budget: u64) -> bool {
    let mut cursor = 0usize;
    while let Some(rel) = find_bytes(&bytes[cursor..], b"stream") {
        let kw = cursor + rel;
        cursor = kw + b"stream".len();
        // Skip the "stream" inside "endstream".
        if kw >= 3 && &bytes[kw - 3..kw] == b"end" {
            continue;
        }
        // The stream's dictionary lies between its `N G obj` marker and
        // the `stream` keyword. The nearest preceding `obj` is this
        // object's own declaration.
        let dict_start = rfind_bytes(&bytes[..kw], b"obj")
            .map(|p| p + 3)
            .unwrap_or(0);
        let dict = &bytes[dict_start..kw];
        if find_bytes(dict, b"/XRef").is_none() {
            continue; // not a cross-reference stream (ObjStm, content, …)
        }
        // Stream data: after the keyword's EOL, up to `endstream`.
        let mut data_start = kw + b"stream".len();
        if bytes.get(data_start) == Some(&b'\r') {
            data_start += 1;
        }
        if bytes.get(data_start) == Some(&b'\n') {
            data_start += 1;
        }
        let data_end = find_bytes(&bytes[data_start..], b"endstream")
            .map(|r| data_start + r)
            .unwrap_or(bytes.len());
        let content = &bytes[data_start..data_end.max(data_start)];
        let filters = xref_stream_filters(dict);
        if inflation_charge(content, &filters, budget).is_none() {
            log::debug!("pdf cross-reference stream exceeds the decompression budget, rejecting");
            return false;
        }
    }
    true
}

/// The `/Filter` names of an xref-stream dictionary (raw dict bytes).
/// Empty when there is no `/Filter` (an uncompressed xref stream —
/// bounded by the source itself). Handles both `/Filter /FlateDecode`
/// and `/Filter [/FlateDecode …]`.
fn xref_stream_filters(dict: &[u8]) -> Vec<&[u8]> {
    let Some(rel) = find_bytes(dict, b"/Filter") else {
        return Vec::new();
    };
    let mut rest = &dict[rel + b"/Filter".len()..];
    // Skip whitespace.
    while rest.first().is_some_and(|b| b.is_ascii_whitespace()) {
        rest = &rest[1..];
    }
    let mut names = Vec::new();
    if rest.first() == Some(&b'[') {
        rest = &rest[1..];
        while let Some(pos) = rest.iter().position(|&b| b == b'/' || b == b']') {
            if rest[pos] == b']' {
                break;
            }
            rest = &rest[pos + 1..];
            let end = rest
                .iter()
                .position(|&b| b.is_ascii_whitespace() || b == b'/' || b == b']' || b == b'[')
                .unwrap_or(rest.len());
            names.push(&rest[..end]);
            rest = &rest[end..];
        }
    } else if rest.first() == Some(&b'/') {
        rest = &rest[1..];
        let end = rest
            .iter()
            .position(|&b| b.is_ascii_whitespace() || b == b'/' || b == b'[' || b == b'>')
            .unwrap_or(rest.len());
        names.push(&rest[..end]);
    }
    names
}

/// First index of `needle` in `haystack`.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Last index of `needle` in `haystack`.
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

/// Bounded lopdf load: size pre-check, budget arm, guarded parse.
fn load_pdf_guarded(path: &Path) -> anyhow::Result<lopdf::Document> {
    load_pdf_guarded_bounded(path, PDF_SOURCE_MAX, PDF_DECOMP_BUDGET)
}

/// Parameterized core of [`load_pdf_guarded`]. The size pre-check
/// replaces a literal bounded read (`load_filtered` takes a path and
/// slurps it): benign TOCTOU — a file growing between check and read
/// only wastes one preview's work.
fn load_pdf_guarded_bounded(
    path: &Path,
    source_max: u64,
    budget: u64,
) -> anyhow::Result<lopdf::Document> {
    let len = path.metadata()?.len();
    anyhow::ensure!(
        len <= source_max,
        "pdf too large for the text tier ({len} bytes)"
    );
    // Cross-reference streams are inflated by lopdf BEFORE the object
    // guard runs; bound them here or a 48 KiB file can OOM the process.
    let bytes = read_bounded(path, source_max)?;
    anyhow::ensure!(
        pdf_xref_streams_within_budget(&bytes, budget),
        "pdf cross-reference stream exceeds the decompression budget"
    );
    PDF_BUDGET.with(|b| b.set(budget));
    Ok(lopdf::Document::load_filtered(path, pdf_guard_filter)?)
}

/// Scrub + length-cap one PDF-sourced line (/Info strings and
/// extracted text are attacker-controlled).
fn pdf_line(line: &str) -> String {
    let scrubbed = scrub_line(line.to_string());
    if scrubbed.len() > PDF_LINE_MAX {
        // chars().take is char-boundary safe by construction; when the
        // byte length exceeds the cap but the char count does not, the
        // collect returns the string unchanged.
        scrubbed.chars().take(PDF_LINE_MAX).collect()
    } else {
        scrubbed
    }
}

/// `Size:`/`Modified:` block from file metadata (best-effort).
fn size_modified_lines(path: &Path) -> Vec<String> {
    use time::OffsetDateTime;
    let mut lines = Vec::new();
    if let Ok(meta) = path.metadata() {
        lines.push(format!(
            "Size:     {}",
            crate::util::file_size_str(meta.len())
        ));
        if let Ok(modified) = meta.modified() {
            let t = OffsetDateTime::from(modified);
            lines.push(format!(
                "Modified: {}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            ));
        }
    }
    lines
}

/// Header + body lines of the text tier. Encrypted docs get
/// "encrypted PDF (N pages)" + Size/Modified only — NO /Info strings
/// (they are encrypted garbage in a real encrypted file) and no
/// extract_text. Otherwise: "PDF · N pages", Title/Author/Producer
/// from /Info when present, blank, then page-1 text — every line
/// scrubbed + length-capped, 128 lines total. Body-extraction failure
/// (dropped/looping content) is "no body", never a tier failure — the
/// metadata header still shows.
fn native_pdf_lines(doc: &lopdf::Document, path: &Path) -> Vec<String> {
    let n_pages = doc.get_pages().len();
    let pages_word = if n_pages == 1 { "page" } else { "pages" };
    if doc.is_encrypted() {
        let mut lines = vec![
            format!("encrypted PDF ({n_pages} {pages_word})"),
            String::new(),
        ];
        lines.extend(size_modified_lines(path));
        return lines;
    }
    let mut lines = vec![format!("PDF · {n_pages} {pages_word}")];
    if let Ok(info) = doc
        .trailer
        .get(b"Info")
        .and_then(|o| doc.dereference(o))
        .and_then(|(_, o)| o.as_dict())
    {
        for (key, label) in [
            (b"Title".as_slice(), "Title:    "),
            (b"Author".as_slice(), "Author:   "),
            (b"Producer".as_slice(), "Producer: "),
        ] {
            let value = info
                .get(key)
                .and_then(|o| doc.dereference(o))
                .ok()
                .and_then(|(_, o)| o.as_str().ok())
                .map(String::from_utf8_lossy);
            if let Some(value) = value {
                if !value.trim().is_empty() {
                    lines.push(pdf_line(&format!("{label}{value}")));
                }
            }
        }
    }
    lines.push(String::new());
    if let Ok(text) = doc.extract_text(&[1]) {
        for line in text.lines() {
            if lines.len() >= 128 {
                break;
            }
            lines.push(pdf_line(line));
        }
    }
    lines.truncate(128);
    lines
}

/// PDF text tier: guarded lopdf load + metadata/page-1 lines.
fn pdf_text_tier(path: &Path) -> anyhow::Result<Preview> {
    let doc = load_pdf_guarded(path)?;
    Ok(Preview::Text {
        lines: native_pdf_lines(&doc, path),
    })
}

/// Info footer for the pdf image tier: metadata via the guarded lopdf
/// load (best-effort — an unreadable source keeps just Size/Modified;
/// the renderer may well parse files lopdf cannot).
fn pdf_image_info(path: &Path) -> Vec<String> {
    let mut lines = match load_pdf_guarded(path) {
        Ok(doc) => {
            let mut lines = native_pdf_lines(&doc, path);
            lines.truncate(2); // "PDF · N pages" + title (or blank)
            lines.push(String::new());
            lines
        }
        Err(_) => Vec::new(),
    };
    lines.extend(size_modified_lines(path));
    lines
}

/// Render page 1 of `path` into `dir` under KIND_PDF. Discipline as
/// `ffmpeg_thumbnail`: `lookup_in` first (corrupt-entry rule),
/// re-create the dir (mid-session `rm -rf` safety), render to a
/// same-dir `<entry>.<part_token()>.part.<ext>` temp name (extension
/// LAST so the tools' format inference works: pdftoppm gets the
/// prefix and appends ".jpg" itself, mutool writes ".png"), then
/// decode the part (`with_guessed_format` — the output format differs
/// per tool), delete it, and finalize via `store_in` — whose own
/// part+rename gives the atomic final write and the stale-sibling
/// sweep. This decode→store_in normalization is a deliberate
/// deviation from ffmpeg_thumbnail's direct rename: one uniform path
/// for both tools, uniform JPEG quality. Non-zero exit, missing or
/// undecodable output => remove the part, Err (the caller drops to
/// the text tier).
fn pdf_render_first_page_in(
    dir: &Path,
    renderer: PdfRenderer,
    path: &Path,
    mtime: u64,
) -> anyhow::Result<Preview> {
    if let Some(img) = raster_cache::lookup_in(dir, path, mtime, raster_cache::KIND_PDF) {
        log::debug!("raster cache hit for {}", path.display());
        return Ok(Preview::Image {
            img: Some(img),
            info: pdf_image_info(path),
        });
    }
    let name = raster_cache::entry_name(path, mtime, raster_cache::KIND_PDF);
    log::debug!("rendering pdf page 1 of {}", path.display());
    let _ = raster_cache::create_dir_all_private(dir);
    let prefix = dir.join(format!("{name}.{}.part", raster_cache::part_token()));
    let mut cmd;
    let part;
    match renderer {
        PdfRenderer::Pdftoppm => {
            // pdftoppm appends ".jpg" to the given prefix itself.
            part = PathBuf::from(format!("{}.jpg", prefix.display()));
            cmd = std::process::Command::new("pdftoppm");
            cmd.arg("-jpeg")
                .arg("-f")
                .arg("1")
                .arg("-l")
                .arg("1")
                .arg("-scale-to")
                .arg("960")
                .arg("-singlefile")
                .arg(path)
                .arg(&prefix);
        }
        PdfRenderer::Mutool => {
            part = PathBuf::from(format!("{}.png", prefix.display()));
            cmd = std::process::Command::new("mutool");
            cmd.arg("draw")
                .arg("-o")
                .arg(&part)
                .arg("-w")
                .arg("960")
                .arg("-h")
                .arg("540")
                .arg(path)
                .arg("1");
        }
    }
    cmd.stdin(Stdio::null());
    let out = match run_bounded(&mut cmd, EXTERNAL_RENDER_DEADLINE) {
        Ok(out) => out,
        Err(e) => {
            // Spawn failure or deadline kill: a killed renderer may
            // have left a half-written part file — drop it before
            // propagating (the caller drops to the text tier).
            let _ = std::fs::remove_file(&part);
            return Err(e.into());
        }
    };
    if !out.status.success() || !part.exists() {
        // A failed run may still have created a partial file; drop it.
        let _ = std::fs::remove_file(&part);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(3).collect();
        anyhow::bail!(
            "{renderer:?} did not render a page ({}): {}",
            out.status,
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        );
    }
    let decoded = image::ImageReader::open(&part)
        .map_err(anyhow::Error::from)
        .and_then(|r| Ok(r.with_guessed_format()?.decode()?));
    let _ = std::fs::remove_file(&part);
    let img = decoded?;
    // Defensive bound: the cache must never hold an unbounded raster,
    // whatever the tool produced.
    let img = if img.width() <= 960 && img.height() <= 540 {
        img
    } else {
        img.thumbnail(960, 540)
    };
    raster_cache::store_in(dir, path, mtime, raster_cache::KIND_PDF, &img)?;
    Ok(Preview::Image {
        img: Some(img),
        info: pdf_image_info(path),
    })
}

/// Tiered PDF arm: optional external image tier (pdf_render on AND a
/// renderer installed AND somewhere to write — `video_thumbnail_dir`'s
/// policy: cache dir / temp fallback / `None` when `preview_cache =
/// false`, a privacy promise the external render must honor, since a
/// render IS a write) → native text tier → stat block. A PDF never
/// shows a bare error panel.
fn pdf_preview(path: &Path, modified: SystemTime, mime: &mime::Mime) -> Preview {
    let renderer = if pdf_render_enabled() {
        pdf_renderer()
    } else {
        None
    };
    pdf_preview_in(renderer, video_thumbnail_dir(), path, modified, mime)
}

/// Testably parameterized core of [`pdf_preview`]; every downgrade
/// logs a debug-level "falling back" line (socket `log` history).
fn pdf_preview_in(
    renderer: Option<PdfRenderer>,
    thumb_dir: Option<&Path>,
    path: &Path,
    modified: SystemTime,
    mime: &mime::Mime,
) -> Preview {
    if let (Some(renderer), Some(dir)) = (renderer, thumb_dir) {
        match pdf_render_first_page_in(dir, renderer, path, mtime_secs(modified)) {
            Ok(preview) => return preview,
            Err(e) => log::debug!("pdf render failed, falling back to text tier: {e}"),
        }
    }
    match pdf_text_tier(path) {
        Ok(preview) => preview,
        Err(e) => {
            log::debug!(
                "pdf text tier failed, falling back to stat: {}: {e}",
                path.display()
            );
            stat_preview(path, mime)
        }
    }
}

/// Dependency-free stat block for generic application/* files: path,
/// size, mtime, MIME type, permissions. Replaces the mediainfo
/// boilerplate; an Err routes to the mediainfo fallback.
fn stat_block_lines(path: &Path, mime: &mime::Mime) -> io::Result<Vec<String>> {
    use std::os::unix::fs::PermissionsExt;
    use time::OffsetDateTime;
    let meta = path.metadata()?;
    let modified = meta
        .modified()
        .map(OffsetDateTime::from)
        .map(|t| {
            format!(
                "{}-{:02}-{:02} {:02}:{:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        })
        .unwrap_or_else(|_| String::from("cannot read timestamp"));
    Ok(vec![
        format!("{}", path.display()),
        String::new(),
        format!("Size:        {}", crate::util::file_size_str(meta.len())),
        format!("Modified:    {modified}"),
        format!("MIME type:   {mime}"),
        format!(
            "Permissions: {}",
            unix_mode::to_string(meta.permissions().mode())
        ),
    ])
}

/// Generic application/* arm: native stat block, mediainfo only as the
/// fallback for the rare unreadable-metadata case.
fn stat_preview(path: &Path, mime: &mime::Mime) -> Preview {
    match stat_block_lines(path, mime) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("stat block failed, trying mediainfo: {e}");
            cmd_to_preview("mediainfo", mediainfo(path))
        }
    }
}

/// Curated audio block via lofty: format/duration/bitrate + common
/// tags. Note the `primary_tag().or_else(first_tag)` - WAV's *primary*
/// tag type is ID3v2, so `primary_tag()` alone misses RIFF-INFO tags.
fn native_audio_lines(path: &Path) -> anyhow::Result<Vec<String>> {
    use lofty::prelude::*;
    let tagged = lofty::read_from_path(path)?;
    let props = tagged.properties();
    let secs = props.duration().as_secs();
    let mut lines = vec![
        format!("Format:      {:?}", tagged.file_type()),
        format!("Duration:    {}:{:02}", secs / 60, secs % 60),
    ];
    if let Some(bitrate) = props.audio_bitrate() {
        lines.push(format!("Bitrate:     {bitrate} kbps"));
    }
    if let Some(rate) = props.sample_rate() {
        lines.push(format!("Sample rate: {rate} Hz"));
    }
    if let Some(channels) = props.channels() {
        lines.push(format!("Channels:    {channels}"));
    }
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        lines.push(String::new());
        if let Some(title) = tag.title() {
            lines.push(format!("Title:       {title}"));
        }
        if let Some(artist) = tag.artist() {
            lines.push(format!("Artist:      {artist}"));
        }
        if let Some(album) = tag.album() {
            lines.push(format!("Album:       {album}"));
        }
        if let Some(year) = tag.year() {
            lines.push(format!("Year:        {year}"));
        }
    }
    Ok(lines.into_iter().map(scrub_line).collect())
}

/// Native-first audio arm; mediainfo stays as the shell-out fallback.
fn audio_preview(path: &Path) -> Preview {
    match native_audio_lines(path) {
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native audio metadata failed, trying mediainfo: {e}");
            cmd_to_preview("mediainfo", mediainfo(path))
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

    #[test]
    fn image_info_lines_contain_dimensions_format_and_size() {
        let lines = image_info_lines(
            64,
            48,
            image::ColorType::Rgb8,
            1234,
            SystemTime::UNIX_EPOCH,
            "png",
        );
        let joined = lines.join("\n");
        assert!(joined.contains("64 × 48"), "{joined}");
        assert!(joined.contains("Rgb8"), "{joined}");
        assert!(joined.contains("png"), "{joined}");
        assert!(joined.contains("1.2 K"), "{joined}");
    }

    #[test]
    fn native_image_preview_populates_info_without_mediainfo() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tiny.png");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, info } => {
                assert!(img.is_some());
                assert!(info.iter().any(|l| l.contains("8 × 8")), "{info:?}");
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn native_image_preview_of_an_undecodable_image_falls_back_to_text() {
        // Formats the image crate cannot decode (HEIC/AVIF/most RAW)
        // must keep the mediainfo fallback: a text preview (mediainfo
        // output, or its error block), never an empty image preview.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("photo.heic");
        std::fs::write(&path, b"definitely not a decodable image").unwrap();
        let mime: mime::Mime = "image/heic".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Text { lines } => assert!(!lines.is_empty(), "{lines:?}"),
            _ => panic!("expected the mediainfo text fallback"),
        }
    }

    #[test]
    fn cached_image_preview_stores_on_miss_and_hits_without_full_decode() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tiny.png");
        image::RgbImage::new(8, 8).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();

        // miss: decode + store
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, .. } => assert!(img.is_some()),
            _ => panic!("expected an image preview"),
        }
        let entry = cache.path().join(raster_cache::entry_name(
            &path,
            mtime_secs(modified),
            raster_cache::KIND_IMAGE,
        ));
        assert!(entry.is_file(), "miss must store the thumbnail");

        // Delete the SOURCE: a second call with the same mtime must still
        // yield pixels — proof they came from the cache, not a re-decode.
        std::fs::remove_file(&path).unwrap();
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, .. } => assert!(img.is_some(), "hit must serve cached pixels"),
            _ => panic!("expected an image preview from the cache"),
        }
    }

    #[test]
    fn cached_image_preview_hit_reports_original_dimensions() {
        // Larger than the 960×540 thumbnail bound: the hit's info lines
        // must show the ORIGINAL dimensions (header read), not the
        // thumbnail's.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.png");
        image::RgbImage::new(1200, 800).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();

        cached_image_preview_in(Some(cache.path()), &path, modified, &mime);
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, info } => {
                let img = img.unwrap();
                assert!(img.width() < 1200, "cache holds the thumbnail");
                assert!(
                    info.iter().any(|l| l.contains("1200 × 800")),
                    "hit must report original dimensions: {info:?}"
                );
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn small_images_are_previewed_and_cached_at_original_size() {
        // image's thumbnail() UPscales sources smaller than the 960×540
        // bound; the producer must not persist an inflated raster (a
        // 100×80 png would balloon to 540-fit) — the draw path rescales
        // to cell dimensions anyway.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("small.png");
        image::RgbImage::new(100, 80).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img: Some(img), .. } => {
                assert_eq!((img.width(), img.height()), (100, 80), "no upscale");
            }
            _ => panic!("expected an image preview"),
        }
        let cached = raster_cache::lookup_in(
            cache.path(),
            &path,
            mtime_secs(modified),
            raster_cache::KIND_IMAGE,
        )
        .unwrap();
        assert_eq!((cached.width(), cached.height()), (100, 80));
    }

    #[test]
    fn cached_image_preview_of_an_undecodable_image_falls_back_to_text() {
        // The mediainfo fallback survives the caching front-end, and a
        // failed decode never writes a cache entry.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("photo.heic");
        std::fs::write(&path, b"not an image").unwrap();
        let mime: mime::Mime = "image/heic".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Text { lines } => assert!(!lines.is_empty(), "{lines:?}"),
            _ => panic!("expected the mediainfo text fallback"),
        }
        assert_eq!(
            std::fs::read_dir(cache.path()).unwrap().count(),
            0,
            "no cache entry for an undecodable image"
        );
    }

    /// Writes `dir/name`: `img` JPEG-encoded with a minimal EXIF APP1
    /// segment (little-endian TIFF header + a one-entry IFD0 carrying
    /// tag 0x0112 = `orientation`) spliced in right after SOI — the
    /// hermetic stand-in for a phone photo.
    fn jpeg_with_orientation(
        dir: &Path,
        name: &str,
        img: image::RgbImage,
        orientation: u16,
    ) -> PathBuf {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&img)
            .unwrap();
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "encoder must emit SOI first");
        // APP1 payload: "Exif\0\0" (6) + TIFF header (8) + entry count
        // (2) + one 12-byte IFD entry + next-IFD offset (4) = 32 bytes;
        // the length field counts itself, so 34.
        let mut app1: Vec<u8> = vec![0xFF, 0xE1, 0x00, 0x22];
        app1.extend_from_slice(b"Exif\0\0");
        app1.extend_from_slice(&[0x49, 0x49, 0x2A, 0x00]); // "II", 42 LE
        app1.extend_from_slice(&8u32.to_le_bytes()); // IFD0 at offset 8
        app1.extend_from_slice(&1u16.to_le_bytes()); // one entry
        app1.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        app1.extend_from_slice(&3u16.to_le_bytes()); // type SHORT
        app1.extend_from_slice(&1u32.to_le_bytes()); // count
        app1.extend_from_slice(&orientation.to_le_bytes());
        app1.extend_from_slice(&[0, 0]); // value padding
        app1.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        let mut spliced = Vec::with_capacity(jpeg.len() + app1.len());
        spliced.extend_from_slice(&jpeg[..2]);
        spliced.extend_from_slice(&app1);
        spliced.extend_from_slice(&jpeg[2..]);
        let path = dir.join(name);
        std::fs::write(&path, spliced).unwrap();
        path
    }

    #[test]
    fn native_image_preview_applies_exif_orientation() {
        // orientation=6 (Rotate90): a 20×10 source must preview upright
        // as 10×20, and the info line must report the UPRIGHT dims.
        let tmp = tempfile::tempdir().unwrap();
        let path = jpeg_with_orientation(
            tmp.path(),
            "rotated.jpg",
            image::RgbImage::new(20, 10),
            6,
        );
        let mime: mime::Mime = "image/jpeg".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, info } => {
                let img = img.unwrap();
                assert_eq!((img.width(), img.height()), (10, 20), "raster upright");
                assert!(info.iter().any(|l| l.contains("10 × 20")), "{info:?}");
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn native_image_preview_orientation_1_is_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = jpeg_with_orientation(
            tmp.path(),
            "upright.jpg",
            image::RgbImage::new(20, 10),
            1,
        );
        let mime: mime::Mime = "image/jpeg".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, .. } => {
                let img = img.unwrap();
                assert_eq!((img.width(), img.height()), (20, 10));
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn native_image_preview_flips_mirrored_orientations() {
        // orientation=2 (FlipHorizontal): a white|black 2×1 must come
        // out black|white. Luminance survives JPEG; exact values don't.
        let mut src = image::RgbImage::new(2, 1);
        src.put_pixel(0, 0, image::Rgb([255, 255, 255]));
        src.put_pixel(1, 0, image::Rgb([0, 0, 0]));
        let tmp = tempfile::tempdir().unwrap();
        let path = jpeg_with_orientation(tmp.path(), "mirrored.jpg", src, 2);
        let mime: mime::Mime = "image/jpeg".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, .. } => {
                let rgb = img.unwrap().to_rgb8();
                let left = rgb.get_pixel(0, 0).0[0];
                let right = rgb.get_pixel(1, 0).0[0];
                assert!(
                    left < right,
                    "flip must swap pixel order: left={left} right={right}"
                );
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn a_png_without_exif_is_untouched() {
        // Guards the cheap skip: decoders without EXIF report
        // NoTransforms and the raster keeps its raw dimensions.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plain.png");
        image::RgbImage::new(20, 10).save(&path).unwrap();
        let mime: mime::Mime = "image/png".parse().unwrap();
        match native_image_preview(&path, &mime) {
            Preview::Image { img, .. } => {
                let img = img.unwrap();
                assert_eq!((img.width(), img.height()), (20, 10));
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn cached_image_preview_stores_the_upright_raster() {
        // The cached raster itself must be upright (correct by
        // construction — the hit path must never re-apply rotation),
        // and the hit's info line must report upright dims too.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = jpeg_with_orientation(
            tmp.path(),
            "rotated.jpg",
            image::RgbImage::new(20, 10),
            6,
        );
        let mime: mime::Mime = "image/jpeg".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();

        // miss: decode + store
        cached_image_preview_in(Some(cache.path()), &path, modified, &mime);
        let cached = raster_cache::lookup_in(
            cache.path(),
            &path,
            mtime_secs(modified),
            raster_cache::KIND_IMAGE,
        )
        .expect("miss must store the thumbnail");
        assert_eq!((cached.width(), cached.height()), (10, 20), "cache upright");

        // hit: still upright, info reports upright dims
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, info } => {
                let img = img.unwrap();
                assert_eq!((img.width(), img.height()), (10, 20));
                assert!(info.iter().any(|l| l.contains("10 × 20")), "{info:?}");
            }
            _ => panic!("expected an image preview"),
        }
    }

    /// A bare 8×8 solid-color JPEG XL codestream (66 bytes), produced
    /// once with libjxl 0.11.2 (`magick -size 8x8 xc:'#3060c0'
    /// -define jxl:effort=1 tiny-8x8.jxl`) — jxl-oxide is decode-only,
    /// so the fixture is embedded, like testdata/subset-noto-sans.ttf.
    const TEST_JXL: &[u8] = include_bytes!("testdata/tiny-8x8.jxl");

    #[test]
    fn a_jxl_file_previews_as_a_native_image() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tiny.jxl");
        std::fs::write(&path, TEST_JXL).unwrap();
        match FilePreview::new(path).preview {
            Preview::Image { img, info } => {
                let img = img.expect("jxl must decode natively");
                assert_eq!((img.width(), img.height()), (8, 8));
                assert!(info.iter().any(|l| l.contains("8 × 8")), "{info:?}");
            }
            other => panic!("expected an image preview, got {other:?}"),
        }
    }

    #[test]
    fn a_jxl_preview_round_trips_through_the_raster_cache() {
        // JXL rides the existing ("image", _) arm: miss stores under
        // KIND_IMAGE, hit serves the cached raster without the source.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tiny.jxl");
        std::fs::write(&path, TEST_JXL).unwrap();
        let mime: mime::Mime = "image/jxl".parse().unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();

        cached_image_preview_in(Some(cache.path()), &path, modified, &mime);
        assert!(
            raster_cache::lookup_in(
                cache.path(),
                &path,
                mtime_secs(modified),
                raster_cache::KIND_IMAGE,
            )
            .is_some(),
            "miss must store the thumbnail"
        );
        std::fs::remove_file(&path).unwrap();
        match cached_image_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, .. } => {
                assert_eq!(img.unwrap().to_rgb8().dimensions(), (8, 8));
            }
            other => panic!("expected a cached image preview, got {other:?}"),
        }
    }

    #[test]
    fn a_broken_jxl_falls_back_to_text() {
        // Garbage under the .jxl extension must land on the existing
        // mediainfo/info fallback — no panic, no bare error panel.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.jxl");
        std::fs::write(&path, b"not a jxl codestream at all").unwrap();
        match FilePreview::new(path).preview {
            Preview::Text { lines } => assert!(!lines.is_empty(), "{lines:?}"),
            other => panic!("expected the text fallback, got {other:?}"),
        }
    }

    /// Builds `dir/archive.zip` containing `files` via the zip crate.
    fn make_zip(dir: &Path, files: &[String]) -> PathBuf {
        use std::io::Write;
        let archive = dir.join("archive.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
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
    fn native_zip_list_scrubs_newlines_from_member_names() {
        // A crafted member name with \r/\n must not break the
        // one-entry-one-line invariant (bat_preview's scrub convention).
        let tmp = tempfile::tempdir().unwrap();
        let archive = make_zip(tmp.path(), &["evil\r\nname.txt".to_string()]);
        let lines = native_zip_list(&archive).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains(['\r', '\n']),
            "newlines must be scrubbed: {lines:?}"
        );
        assert!(lines[0].contains("evilname.txt"), "{lines:?}");
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
    fn native_tar_list_scrubs_newlines_from_member_names() {
        // Same scrub convention as the zip lister: hostile member names
        // must stay on one line.
        let tmp = tempfile::tempdir().unwrap();
        let archive = make_native_tar(tmp.path(), &["evil\r\nname.txt".to_string()]);
        let lines = native_tar_list(File::open(archive).unwrap()).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(
            !lines[0].contains(['\r', '\n']),
            "newlines must be scrubbed: {lines:?}"
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

    /// Minimal PCM WAV (1 s of silence, 8 kHz mono 8-bit), tagged via
    /// lofty itself - no committed binary, no external tool.
    fn make_tagged_wav(dir: &Path) -> PathBuf {
        make_tagged_wav_titled(dir, "Test Title")
    }

    fn make_tagged_wav_titled(dir: &Path, title: &str) -> PathBuf {
        use lofty::prelude::*;
        use lofty::tag::{Tag, TagType};
        let path = dir.join("tone.wav");
        let data_len: u32 = 8000;
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&8000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&1u16.to_le_bytes()); // block align
        wav.extend_from_slice(&8u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&vec![128u8; data_len as usize]);
        std::fs::write(&path, wav).unwrap();
        let mut tag = Tag::new(TagType::RiffInfo);
        tag.set_title(title.to_string());
        tag.set_artist("Test Artist".to_string());
        tag.save_to_path(&path, lofty::config::WriteOptions::default())
            .unwrap();
        path
    }

    #[test]
    fn native_audio_lines_include_title_and_duration() {
        let tmp = tempfile::tempdir().unwrap();
        let wav = make_tagged_wav(tmp.path());
        let lines = native_audio_lines(&wav).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("Test Title"), "{joined}");
        assert!(joined.contains("Test Artist"), "{joined}");
        assert!(joined.contains("Duration"), "{joined}");
        assert!(joined.contains("8000"), "sample rate expected: {joined}");
    }

    #[test]
    fn native_audio_lines_scrub_newlines_from_tag_values() {
        // ID3/RIFF tag values are attacker-controlled; embedded \r/\n
        // must not smear the preview across lines.
        let tmp = tempfile::tempdir().unwrap();
        let wav = make_tagged_wav_titled(tmp.path(), "Evil\r\nTitle");
        let lines = native_audio_lines(&wav).unwrap();
        assert!(
            lines.iter().all(|l| !l.contains(['\r', '\n'])),
            "newlines must be scrubbed: {lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("EvilTitle")), "{lines:?}");
    }

    #[test]
    fn native_audio_lines_on_garbage_are_an_error() {
        // Err is what routes audio_preview to the mediainfo fallback.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("noise.mp3");
        std::fs::write(&bogus, b"not audio at all").unwrap();
        assert!(native_audio_lines(&bogus).is_err());
    }

    #[test]
    fn audio_preview_of_garbage_degrades_to_a_text_preview() {
        // Native path errors, the mediainfo fallback runs (and may
        // error too) - the result must still be text lines.
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("noise.flac");
        std::fs::write(&bogus, vec![0u8; 32]).unwrap();
        match audio_preview(&bogus) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn stat_block_lines_contain_size_permissions_and_mime() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("blob.bin");
        std::fs::write(&path, vec![0u8; 2048]).unwrap();
        let mime: mime::Mime = "application/octet-stream".parse().unwrap();
        let lines = stat_block_lines(&path, &mime).unwrap();
        let joined = lines.join("\n");
        assert!(joined.contains("blob.bin"), "{joined}");
        assert!(joined.contains("2.0 K"), "{joined}");
        assert!(joined.contains("application/octet-stream"), "{joined}");
        assert!(
            joined.contains("rw-"),
            "permission string expected: {joined}"
        );
    }

    #[test]
    fn stat_preview_of_a_missing_file_degrades_to_a_text_preview() {
        // Metadata unreadable: the mediainfo fallback runs (and may
        // error too) - the result must still be text lines.
        let mime: mime::Mime = "application/x-frobnicate".parse().unwrap();
        match stat_preview(Path::new("/no/such/file"), &mime) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
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

    /// Every file in the thumbnail dir `dir` belonging to `video`
    /// (matched on the 16-hex hash prefix of its cache key).
    fn thumbnail_dir_entries_of(dir: &Path, video: &Path) -> Vec<String> {
        let prefix = raster_cache::entry_name(video, 0, raster_cache::KIND_VIDEO)[..17].to_string();
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with(&prefix))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn ffmpeg_thumbnail_of_a_too_short_video_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "short.mp4", 1);
        // The hardcoded 10s seek is past the end of this clip, so
        // ffmpeg fails and writes no thumbnail - that must surface as
        // Err (the caller then falls back to mediainfo) instead of a
        // phantom image preview.
        assert!(ffmpeg_thumbnail(cache.path(), &video, 0).is_err());
        // the failed run leaves no .part file behind
        assert!(
            thumbnail_dir_entries_of(cache.path(), &video).is_empty(),
            "a failed run must leave nothing behind"
        );
    }

    #[test]
    fn ffmpeg_thumbnail_of_a_long_video_lands_in_the_thumbnail_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "long.mp4", 15);
        ffmpeg_thumbnail(cache.path(), &video, 1).unwrap();
        let name = raster_cache::entry_name(&video, 1, raster_cache::KIND_VIDEO);
        assert!(cache.path().join(&name).is_file());
        // atomicity: no .part siblings left behind
        assert_eq!(thumbnail_dir_entries_of(cache.path(), &video), vec![name]);
    }

    #[test]
    fn ffmpeg_thumbnail_corrupt_cache_entry_is_deleted_and_regenerated() {
        // Design §error handling: a lookup decode failure deletes the
        // entry and regenerates. An exists()-only fast path would serve
        // this corrupt entry as a BLANK preview until the 30-day prune.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "long.mp4", 15);
        let entry = cache.path().join(raster_cache::entry_name(
            &video,
            1,
            raster_cache::KIND_VIDEO,
        ));
        std::fs::write(&entry, b"not a jpeg").unwrap();
        match ffmpeg_thumbnail(cache.path(), &video, 1).unwrap() {
            Preview::Image { img, .. } => assert!(img.is_some(), "regenerated, never blank"),
            _ => panic!("expected an image preview"),
        }
        // the regenerated entry decodes
        assert!(
            raster_cache::lookup_in(cache.path(), &video, 1, raster_cache::KIND_VIDEO).is_some()
        );
    }

    #[test]
    fn ffmpeg_thumbnail_recreates_a_deleted_thumbnail_dir() {
        // "rm -rf ~/.cache/rfm is always safe" — even mid-session it
        // must not degrade every video preview until restart.
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let gone = cache.path().join("wiped-mid-session");
        let video = make_video(tmp.path(), "long.mp4", 15);
        ffmpeg_thumbnail(&gone, &video, 1).unwrap();
        let name = raster_cache::entry_name(&video, 1, raster_cache::KIND_VIDEO);
        assert!(gone.join(name).is_file());
    }

    #[test]
    fn video_thumbnail_dir_honors_the_privacy_opt_out() {
        fn fake_fallback() -> &'static Path {
            Path::new("/fallback")
        }
        let cache: Option<&'static Path> = Some(Path::new("/cache"));
        // cache available → cache dir
        assert_eq!(video_thumbnail_dir_from(cache, true, fake_fallback), cache);
        // enabled but unavailable → temp fallback keeps thumbnails alive
        assert_eq!(
            video_thumbnail_dir_from(None, true, fake_fallback),
            Some(Path::new("/fallback"))
        );
        // opted out (preview_cache = false) → no location at all:
        // nothing about the user's files may be written anywhere
        assert_eq!(video_thumbnail_dir_from(None, false, fake_fallback), None);
    }

    #[test]
    fn video_preview_writes_nothing_when_persistence_is_opted_out() {
        // The raster cache is uninitialized in unit tests, which counts
        // as opted out: the video preview must degrade to mediainfo
        // text and write no thumbnail anywhere — the shipped config
        // promises "nothing about your files is written to disk".
        let tmp = tempfile::tempdir().unwrap();
        let video = make_video(tmp.path(), "clip.mp4", 15);
        let preview = video_preview(&video, SystemTime::now());
        assert!(matches!(preview, Preview::Text { .. }));
        assert!(
            thumbnail_dir_entries_of(&temp_dir().join("rfm-thumbnails"), &video).is_empty(),
            "opt-out must not write into the temp fallback dir"
        );
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

    /// `sh -c <script>` with stdin null — the shape both real callers
    /// (ffmpeg, pdftoppm/mutool) hand to `run_bounded`.
    fn sh(script: &str) -> std::process::Command {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(script).stdin(Stdio::null());
        cmd
    }

    #[test]
    fn run_bounded_kills_a_child_that_outlives_the_deadline() {
        let started = std::time::Instant::now();
        let result = run_bounded(&mut sh("sleep 30"), Duration::from_millis(200));
        assert!(result.is_err(), "an overrunning child must surface as Err");
        // The point is "not 30s": the child was killed at the deadline,
        // not waited for. 2s is a generous CI margin.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the call must return promptly after the deadline, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn run_bounded_returns_output_of_a_fast_child() {
        let out = run_bounded(
            &mut sh("echo out; echo err 1>&2"),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn run_bounded_does_not_deadlock_on_a_stderr_flood() {
        // 200 KiB of stderr — well past the 64 KiB pipe buffer. A naive
        // try_wait poll without drain threads deadlocks here: the child
        // blocks on the full pipe, the poll waits on the child, forever
        // (well, until the deadline — but the output would be lost).
        let out = run_bounded(
            &mut sh("head -c 200000 /dev/zero | tr '\\0' x 1>&2"),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(out.stderr.len(), 200000);
    }

    #[test]
    fn run_bounded_surfaces_a_nonzero_exit() {
        // Policy: a non-zero exit is the caller's domain (both call
        // sites branch on `out.status.success()`), not an Err here.
        let out = run_bounded(&mut sh("exit 3"), Duration::from_secs(10)).unwrap();
        assert!(!out.status.success());
    }
}

#[cfg(test)]
mod new_type_tests {
    use super::*;

    /// 100×50 with a centered red rect — the corners show the background.
    const TEST_SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect x="25" y="0" width="50" height="50" fill="red"/></svg>"#;

    #[test]
    fn svg_render_rasterises_to_the_960x540_bound() {
        // Vectors are scaled UP to the preview bound (unlike bitmap
        // thumbnails): 100×50 aspect-fits 960×540 as 960×480.
        let img = native_svg_render(TEST_SVG).expect("a valid svg must render");
        assert_eq!((img.width(), img.height()), (960, 480));
        let rgba = img.to_rgba8();
        // JPEG cache has no alpha: the background must be white, not
        // transparent/black.
        assert_eq!(rgba.get_pixel(0, 0).0, [255, 255, 255, 255]);
        let center = rgba.get_pixel(480, 240).0;
        assert!(
            center[0] > 200 && center[1] < 60 && center[2] < 60,
            "the red rect must be rendered: {center:?}"
        );
    }

    #[test]
    fn svg_render_of_garbage_is_none() {
        assert!(native_svg_render(b"<not-svg").is_none());
    }

    #[test]
    fn svg_preview_of_garbage_falls_back_to_text() {
        // Malformed SVG: never a broken Preview::Image, the bat/text
        // path shows the raw XML instead.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("broken.svg");
        std::fs::write(&path, b"<not-svg").unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        let cache = tempfile::tempdir().unwrap();
        match svg_preview_in(Some(cache.path()), &path, modified) {
            Preview::Text { lines } => assert!(!lines.is_empty(), "{lines:?}"),
            _ => panic!("expected the text fallback"),
        }
        assert_eq!(
            std::fs::read_dir(cache.path()).unwrap().count(),
            0,
            "no cache entry for an unrenderable svg"
        );
    }

    #[test]
    fn cached_svg_preview_stores_on_miss_and_hits_without_source() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pic.svg");
        std::fs::write(&path, TEST_SVG).unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();

        // miss: render + store
        match svg_preview_in(Some(cache.path()), &path, modified) {
            Preview::Image { img, .. } => assert!(img.is_some()),
            _ => panic!("expected an image preview"),
        }
        let entry = cache.path().join(raster_cache::entry_name(
            &path,
            mtime_secs(modified),
            raster_cache::KIND_SVG,
        ));
        assert!(entry.is_file(), "miss must store the render");

        // Delete the SOURCE: a second call with the same mtime must still
        // yield pixels — proof they came from the cache, not a re-render.
        std::fs::remove_file(&path).unwrap();
        match svg_preview_in(Some(cache.path()), &path, modified) {
            Preview::Image { img, .. } => assert!(img.is_some(), "hit must serve cached pixels"),
            _ => panic!("expected an image preview from the cache"),
        }
    }

    #[test]
    fn svg_dispatch_reaches_the_render_arm() {
        // mime 0.3 splits image/svg+xml into subtype "svg" + suffix
        // "xml", so an arm matching the subtype against "svg+xml" never
        // fires and .svg mis-lands on the bitmap decode (caught by the
        // e2e smoke: "native image decode failed" for pic.svg).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pic.svg");
        std::fs::write(&path, TEST_SVG).unwrap();
        match FilePreview::new(path).preview {
            Preview::Image { img, .. } => assert!(img.is_some()),
            Preview::Text { lines } => {
                panic!("svg must dispatch to the render arm, got text: {lines:?}")
            }
        }
    }

    #[test]
    fn font_dispatch_reaches_the_sample_arm() {
        // font/ttf → ("font", _); pins the dispatch end-to-end.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sample.ttf");
        std::fs::write(&path, TEST_FONT).unwrap();
        match FilePreview::new(path).preview {
            Preview::Image { img, .. } => assert!(img.is_some()),
            Preview::Text { lines } => {
                panic!("ttf must dispatch to the font arm, got text: {lines:?}")
            }
        }
    }

    /// OFL-licensed ASCII subset of Noto Sans Regular (see
    /// `testdata/OFL.txt`) — hermetic, no system-font dependency.
    const TEST_FONT: &[u8] = include_bytes!("testdata/subset-noto-sans.ttf");

    #[test]
    fn native_font_sample_rasterises_the_pangram() {
        let (info, img) = native_font_sample(TEST_FONT).expect("the fixture font must render");
        assert!(
            info.iter().any(|l| l.contains("Noto Sans")),
            "family name expected: {info:?}"
        );
        let gray = img.to_luma8();
        let dark = gray.pixels().filter(|p| p.0[0] < 128).count();
        assert!(dark > 200, "expected >200 lit pixels, got {dark}");
        assert!(img.width() <= 960, "sample width stays bounded");
    }

    #[test]
    fn font_info_includes_family_and_style() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sample.ttf");
        std::fs::write(&path, TEST_FONT).unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        let mime: mime::Mime = "font/ttf".parse().unwrap();
        match font_preview_in(None, &path, modified, &mime) {
            Preview::Image { img, info } => {
                assert!(img.is_some());
                let joined = info.join("\n");
                assert!(joined.contains("Noto Sans"), "{joined}");
                assert!(joined.contains("Regular"), "{joined}");
                assert!(joined.contains("Size:"), "{joined}");
                assert!(joined.contains("Modified:"), "{joined}");
            }
            _ => panic!("expected an image preview"),
        }
    }

    #[test]
    fn native_font_sample_on_garbage_is_an_error() {
        // Err is what routes font_preview to the stat fallback.
        assert!(native_font_sample(b"not a font").is_err());
    }

    #[test]
    fn font_preview_of_a_woff_degrades_to_stat() {
        // Documented deviation: no MSRV-1.83 pure-Rust woff decoder, so
        // woff/woff2 land in the font arm and degrade to the stat block.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("web.woff");
        std::fs::write(&path, b"wOFFxxxxxxxxxxxxxxxxxxxx").unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        let mime: mime::Mime = "font/woff".parse().unwrap();
        match font_preview_in(None, &path, modified, &mime) {
            Preview::Text { lines } => {
                let joined = lines.join("\n");
                assert!(joined.contains("Size:"), "{joined}");
                assert!(joined.contains("MIME type:"), "{joined}");
            }
            _ => panic!("expected the stat fallback"),
        }
    }

    #[test]
    fn font_extensions_route_to_the_font_arm() {
        // mime_guess maps ttf/woff2 to font/*, but otf/woff to the
        // legacy application/font-* aliases (verified on 2.0.5); the
        // dispatch must cover both spellings. Nonexistent paths prove
        // the resolution never reads content.
        for ext in ["ttf", "otf", "woff", "woff2"] {
            let path = PathBuf::from(format!("/nonexistent/sample.{ext}"));
            let mime = get_mime_type(&path);
            let routed = matches!(
                (mime.type_().as_str(), mime.subtype().as_str()),
                ("font", _) | ("application", "font-sfnt") | ("application", "font-woff")
            );
            assert!(routed, ".{ext} resolved to {mime}, missing the font arm");
        }
    }

    /// Builds `dir/<file_name>` as a zip container with real member
    /// bodies — the docx/odt/xlsx/epub fixtures are all just zips.
    fn make_container(dir: &Path, file_name: &str, members: &[(&str, &str)]) -> PathBuf {
        use std::io::Write;
        let container = dir.join(file_name);
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&container).unwrap());
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in members {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        container
    }

    #[test]
    fn docx_preview_shows_body_text_with_tags_stripped() {
        let tmp = tempfile::tempdir().unwrap();
        let docx = make_container(
            tmp.path(),
            "report.docx",
            &[(
                "word/document.xml",
                "<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:t> World</w:t></w:p></w:body></w:document>",
            )],
        );
        match doc_preview(&docx, DocKind::Docx) {
            Preview::Text { lines } => {
                assert!(lines.iter().any(|l| l.contains("Hello World")), "{lines:?}");
                assert!(
                    lines.iter().all(|l| !l.contains('<')),
                    "tags must be stripped: {lines:?}"
                );
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn odt_preview_reads_content_xml() {
        let tmp = tempfile::tempdir().unwrap();
        let odt = make_container(
            tmp.path(),
            "notes.odt",
            &[(
                "content.xml",
                "<office:document-content><office:body><office:text><text:p>An OpenDocument paragraph.</text:p></office:text></office:body></office:document-content>",
            )],
        );
        match doc_preview(&odt, DocKind::Odt) {
            Preview::Text { lines } => assert!(
                lines
                    .iter()
                    .any(|l| l.contains("An OpenDocument paragraph.")),
                "{lines:?}"
            ),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn xlsx_preview_lists_shared_strings() {
        let tmp = tempfile::tempdir().unwrap();
        let xlsx = make_container(
            tmp.path(),
            "table.xlsx",
            &[(
                "xl/sharedStrings.xml",
                "<sst><si><t>Alpha</t></si><si><t>Beta</t></si></sst>",
            )],
        );
        match doc_preview(&xlsx, DocKind::Xlsx) {
            Preview::Text { lines } => {
                // one shared string per line
                assert!(lines.iter().any(|l| l == "Alpha"), "{lines:?}");
                assert!(lines.iter().any(|l| l == "Beta"), "{lines:?}");
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn epub_preview_follows_container_and_spine() {
        let tmp = tempfile::tempdir().unwrap();
        let epub = make_container(
            tmp.path(),
            "novel.epub",
            &[
                (
                    "META-INF/container.xml",
                    r#"<container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "OEBPS/content.opf",
                    r#"<package><manifest><item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="ch1"/></spine></package>"#,
                ),
                (
                    "OEBPS/ch1.xhtml",
                    "<html><head><title>One</title><style>p { color: red }</style></head><body><p>It was a dark and stormy night.</p></body></html>",
                ),
            ],
        );
        match doc_preview(&epub, DocKind::Epub) {
            Preview::Text { lines } => {
                // Exact match: the head <title> ("One") must NOT leak
                // into the first paragraph ("OneIt was a dark...").
                assert!(
                    lines.iter().any(|l| l == "It was a dark and stormy night."),
                    "{lines:?}"
                );
                assert!(
                    lines.iter().all(|l| !l.contains("color")),
                    "style content must be skipped: {lines:?}"
                );
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn office_text_is_capped_at_128_lines_and_scrubbed() {
        let tmp = tempfile::tempdir().unwrap();
        let mut body = String::from("<w:document><w:body>");
        // First paragraph carries \r\n inside its text node — it must
        // stay ONE line (the scrub convention), not smear across two.
        body.push_str("<w:p><w:t>evil\r\nline</w:t></w:p>");
        for i in 0..129 {
            body.push_str(&format!("<w:p><w:t>paragraph {i}</w:t></w:p>"));
        }
        body.push_str("</w:body></w:document>");
        let docx = make_container(tmp.path(), "long.docx", &[("word/document.xml", &body)]);
        match doc_preview(&docx, DocKind::Docx) {
            Preview::Text { lines } => {
                assert_eq!(lines.len(), 128, "the 128-line cap applies");
                assert!(
                    lines[0].contains("evilline") && !lines[0].contains(['\r', '\n']),
                    "newlines must be scrubbed: {:?}",
                    lines[0]
                );
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn corrupt_container_falls_back_to_zip_listing() {
        // A docx-named file that IS a valid zip but lacks the primary
        // part: treated as a plain zip so the preview is never empty.
        let tmp = tempfile::tempdir().unwrap();
        let hollow = make_container(
            tmp.path(),
            "hollow.docx",
            &[("word/nothing-here.xml", "<w:document/>")],
        );
        match doc_preview(&hollow, DocKind::Docx) {
            Preview::Text { lines } => assert!(
                lines.iter().any(|l| l.contains("word/nothing-here.xml")),
                "expected the zip listing: {lines:?}"
            ),
            _ => panic!("expected a text preview"),
        }
        // Total garbage: the zip fallback errors too — still text
        // (the unzip-fallback error block), never empty, never a panic.
        let bogus = tmp.path().join("garbage.docx");
        std::fs::write(&bogus, b"definitely not a zip").unwrap();
        match doc_preview(&bogus, DocKind::Docx) {
            Preview::Text { lines } => assert!(!lines.is_empty()),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn office_extensions_route_to_the_doc_arms() {
        // Pins the dispatch spelling: mime 0.3 splits "epub+zip" into
        // subtype "epub" + suffix "zip" (same trap as image/svg+xml),
        // while the vnd.* types carry no suffix. Nonexistent paths
        // prove the resolution never reads content.
        for ext in ["docx", "xlsx", "pptx", "odt", "ods", "odp", "epub"] {
            let path = PathBuf::from(format!("/nonexistent/file.{ext}"));
            let mime = get_mime_type(&path);
            let routed = matches!(
                (mime.type_().as_str(), mime.subtype().as_str()),
                (
                    "application",
                    "vnd.openxmlformats-officedocument.wordprocessingml.document"
                        | "vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                        | "vnd.openxmlformats-officedocument.presentationml.presentation"
                        | "vnd.oasis.opendocument.text"
                        | "vnd.oasis.opendocument.spreadsheet"
                        | "vnd.oasis.opendocument.presentation"
                        | "epub"
                )
            );
            assert!(routed, ".{ext} resolved to {mime}, missing the doc arms");
        }
    }

    #[test]
    fn docx_dispatch_reaches_the_doc_arm() {
        // End-to-end: FilePreview::new on a real .docx must produce the
        // extracted body text, not the generic stat block.
        let tmp = tempfile::tempdir().unwrap();
        let docx = make_container(
            tmp.path(),
            "report.docx",
            &[(
                "word/document.xml",
                "<w:document><w:body><w:p><w:t>Dispatched body text</w:t></w:p></w:body></w:document>",
            )],
        );
        match FilePreview::new(docx).preview {
            Preview::Text { lines } => assert!(
                lines.iter().any(|l| l.contains("Dispatched body text")),
                "{lines:?}"
            ),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn sqlite_preview_lists_tables_with_row_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("data.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE t1(a);
             INSERT INTO t1 VALUES (1);
             INSERT INTO t1 VALUES (2);
             CREATE TABLE t2(b);",
        )
        .unwrap();
        drop(conn);
        let lines = native_sqlite_lines(&db).unwrap();
        assert!(
            lines[0].contains("SQLite") && lines[0].contains("2 tables"),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("t1") && l.contains('2') && !l.contains("t2")),
            "t1 must count 2 rows: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("t2") && l.contains('0')),
            "t2 must count 0 rows: {lines:?}"
        );
    }

    #[test]
    fn sqlite_table_names_are_scrubbed_and_quoted() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("evil.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        // An embedded quote in the identifier: COUNT(*) only succeeds
        // when the listing re-quotes it by doubling; a crafted name
        // with \n must render on ONE line (scrub convention).
        conn.execute_batch(
            "CREATE TABLE \"evil\"\"name\"(x);
             INSERT INTO \"evil\"\"name\" VALUES (1);
             CREATE TABLE \"bad\nname\"(y);",
        )
        .unwrap();
        drop(conn);
        let lines = native_sqlite_lines(&db).unwrap();
        assert!(
            lines
                .iter()
                .any(|l| l.contains("evil\"name") && l.contains('1') && !l.contains('?')),
            "the quoted table must count correctly: {lines:?}"
        );
        assert!(
            lines.iter().all(|l| !l.contains(['\r', '\n'])),
            "newlines must be scrubbed: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("badname")),
            "the scrubbed name must still be listed: {lines:?}"
        );
    }

    #[test]
    fn sqlite_listing_is_capped_at_128_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("many.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        for i in 0..130 {
            conn.execute_batch(&format!("CREATE TABLE table_{i:03}(x);"))
                .unwrap();
        }
        drop(conn);
        // 2 header lines + 126 tables = the shared 128-line cap.
        assert_eq!(native_sqlite_lines(&db).unwrap().len(), 128);
    }

    #[test]
    fn sqlite_preview_of_garbage_falls_back_to_stat() {
        // Only the 16-byte magic + garbage: the open/master query fails
        // and the stat block (design fallback) must show instead.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("broken.sqlite");
        let mut data = b"SQLite format 3\0".to_vec();
        data.extend_from_slice(&[0xffu8; 100]);
        std::fs::write(&db, data).unwrap();
        let mime: mime::Mime = "application/vnd.sqlite3".parse().unwrap();
        match sqlite_preview(&db, &mime) {
            Preview::Text { lines } => {
                let joined = lines.join("\n");
                assert!(joined.contains("Size:"), "{joined}");
                assert!(joined.contains("MIME type:"), "{joined}");
            }
            _ => panic!("expected the stat fallback"),
        }
    }

    #[test]
    fn extensionless_sqlite_magic_routes_to_the_sqlite_arm() {
        // .db/.sqlite/.sqlite3 get no mime_guess answer, so routing is
        // sniff-driven: infer maps the `SQLite format 3\0` magic to
        // application/vnd.sqlite3 (no get_mime_type special case).
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("statefile");
        let mut data = b"SQLite format 3\0".to_vec();
        data.extend_from_slice(&[0u8; 100]);
        std::fs::write(&db, data).unwrap();
        let mime = get_mime_type(&db);
        assert_eq!(
            (mime.type_().as_str(), mime.subtype().as_str()),
            ("application", "vnd.sqlite3"),
            "sniff resolved {mime}"
        );
    }

    #[test]
    fn sqlite_dispatch_reaches_the_listing_arm() {
        // End-to-end: FilePreview::new on a real .sqlite file must show
        // the table listing, not the generic stat block.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("app.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE sessions(x);").unwrap();
        drop(conn);
        match FilePreview::new(db).preview {
            Preview::Text { lines } => assert!(
                lines.iter().any(|l| l.contains("sessions")),
                "expected the table listing: {lines:?}"
            ),
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn cached_font_preview_stores_on_miss_and_hits_without_source() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sample.ttf");
        std::fs::write(&path, TEST_FONT).unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        let mime: mime::Mime = "font/ttf".parse().unwrap();

        // miss: rasterise + store
        match font_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, .. } => assert!(img.is_some()),
            _ => panic!("expected an image preview"),
        }
        let entry = cache.path().join(raster_cache::entry_name(
            &path,
            mtime_secs(modified),
            raster_cache::KIND_FONT,
        ));
        assert!(entry.is_file(), "miss must store the sample");

        // Delete the SOURCE: the second call must still yield pixels —
        // proof they came from the cache, not a re-rasterisation.
        std::fs::remove_file(&path).unwrap();
        match font_preview_in(Some(cache.path()), &path, modified, &mime) {
            Preview::Image { img, .. } => assert!(img.is_some(), "hit must serve cached pixels"),
            _ => panic!("expected an image preview from the cache"),
        }
    }

    // ---- C5: extra archive formats (.7z, .tar.zst/.tar.xz/.tar.bz2) ----

    use std::io::Write;

    /// A small tar stream built in memory via the tar crate.
    fn tar_bytes(names: &[&str]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for name in names {
            let mut header = tar::Header::new_gnu();
            header.set_size(7);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, *name, &b"content"[..])
                .unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn expect_lines(preview: Preview) -> Vec<String> {
        match preview {
            Preview::Text { lines } => lines,
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn tar_zst_preview_lists_members() {
        let tmp = tempfile::tempdir().unwrap();
        let compressed = ruzstd::encoding::compress_to_vec(
            &tar_bytes(&["a.txt", "b.txt"])[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let path = tmp.path().join("archive.tar.zst");
        std::fs::write(&path, compressed).unwrap();
        let lines = expect_lines(zst_preview(&path));
        assert!(
            lines.iter().any(|l| l.contains("a.txt")) && lines.iter().any(|l| l.contains("b.txt")),
            "{lines:?}"
        );
    }

    #[test]
    fn tar_xz_preview_lists_members() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("archive.tar.xz");
        let mut writer = lzma_rust2::XzWriter::new(
            File::create(&path).unwrap(),
            lzma_rust2::XzOptions::with_preset(1),
        )
        .unwrap();
        writer.write_all(&tar_bytes(&["a.txt", "b.txt"])).unwrap();
        writer.finish().unwrap();
        let lines = expect_lines(xz_preview(&path));
        assert!(
            lines.iter().any(|l| l.contains("a.txt")) && lines.iter().any(|l| l.contains("b.txt")),
            "{lines:?}"
        );
    }

    #[test]
    fn tar_bz2_preview_lists_members() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("archive.tar.bz2");
        let mut writer =
            bzip2::write::BzEncoder::new(File::create(&path).unwrap(), bzip2::Compression::fast());
        writer.write_all(&tar_bytes(&["a.txt", "b.txt"])).unwrap();
        writer.finish().unwrap();
        let lines = expect_lines(bz2_preview(&path));
        assert!(
            lines.iter().any(|l| l.contains("a.txt")) && lines.iter().any(|l| l.contains("b.txt")),
            "{lines:?}"
        );
    }

    #[test]
    fn zst_of_a_non_tar_file_previews_the_decompressed_text() {
        // The gz-arm behavior generalised: a compressed NON-tar file
        // must show its decompressed head as text, not an error.
        let tmp = tempfile::tempdir().unwrap();
        let compressed = ruzstd::encoding::compress_to_vec(
            &b"hello from a zstd text file\r\nsecond line\n"[..],
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let path = tmp.path().join("notes.txt.zst");
        std::fs::write(&path, compressed).unwrap();
        let lines = expect_lines(zst_preview(&path));
        assert!(
            lines[0].contains("hello from a zstd text file") && !lines[0].contains('\r'),
            "{lines:?}"
        );
        assert_eq!(lines[1], "second line");
    }

    #[test]
    fn compressed_garbage_degrades_to_a_text_preview() {
        // Unreadable streams route to the tar-binary fallback, which
        // itself degrades to error text - never a panic, never empty.
        let tmp = tempfile::tempdir().unwrap();
        let garbage = [0xffu8; 64];
        for (name, preview_fn) in [
            ("g.tar.zst", zst_preview as fn(&Path) -> Preview),
            ("g.tar.xz", xz_preview),
            ("g.tar.bz2", bz2_preview),
        ] {
            let path = tmp.path().join(name);
            std::fs::write(&path, garbage).unwrap();
            let lines = expect_lines(preview_fn(&path));
            assert!(!lines.is_empty(), "{name}: {lines:?}");
        }
    }

    #[test]
    fn sevenz_preview_lists_names_and_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("a.txt"), b"content").unwrap();
        std::fs::write(src.join("b.txt"), b"content").unwrap();
        let path = tmp.path().join("archive.7z");
        sevenz_rust::compress_to_path(&src, &path).unwrap();
        let lines = expect_lines(sevenz_preview(&path));
        assert!(
            lines.iter().any(|l| l.contains("a.txt")) && lines.iter().any(|l| l.contains("b.txt")),
            "{lines:?}"
        );
        // `size  name` columns like the zip/tar listings.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("7 B") && l.contains("a.txt")),
            "{lines:?}"
        );
    }

    #[test]
    fn sevenz_list_caps_at_128_and_scrubs() {
        let tmp = tempfile::tempdir().unwrap();

        // Cap: 140 members list as exactly 128 lines.
        let big = tmp.path().join("big");
        std::fs::create_dir(&big).unwrap();
        for i in 0..140 {
            std::fs::write(big.join(format!("file-{i:03}.txt")), b"x").unwrap();
        }
        let big_archive = tmp.path().join("big.7z");
        sevenz_rust::compress_to_path(&big, &big_archive).unwrap();
        assert_eq!(native_sevenz_list(&big_archive).unwrap().len(), 128);

        // Scrub: a member name with an embedded newline stays one line.
        let evil = tmp.path().join("evil");
        std::fs::create_dir(&evil).unwrap();
        std::fs::write(evil.join("evil\nname.txt"), b"x").unwrap();
        let evil_archive = tmp.path().join("evil.7z");
        sevenz_rust::compress_to_path(&evil, &evil_archive).unwrap();
        let lines = native_sevenz_list(&evil_archive).unwrap();
        assert!(
            lines.iter().any(|l| l.contains("evilname.txt")),
            "newline must be scrubbed out of the member name: {lines:?}"
        );
        assert!(lines.iter().all(|l| !l.contains('\n')));
    }

    #[test]
    fn sevenz_of_garbage_degrades_to_a_text_preview() {
        // Err from the native lister routes to the 7z shell-out, whose
        // absence still yields error text - never empty, never a panic.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("garbage.7z");
        std::fs::write(&path, [0xffu8; 64]).unwrap();
        assert!(native_sevenz_list(&path).is_err());
        let lines = expect_lines(sevenz_preview(&path));
        assert!(!lines.is_empty(), "{lines:?}");
    }

    /// CRC-32 (IEEE, reflected) — just enough to craft a start header
    /// that passes sevenz-rust's checksum verification.
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xedb8_8320 & mask);
            }
        }
        !crc
    }

    /// A syntactically valid 32-byte 7z start header (correct magic and
    /// CRC) declaring the given next-header offset/size.
    fn sevenz_start_header(offset: u64, size: u64) -> Vec<u8> {
        let mut start = Vec::new();
        start.extend_from_slice(&offset.to_le_bytes());
        start.extend_from_slice(&size.to_le_bytes());
        start.extend_from_slice(&0u32.to_le_bytes()); // next-header CRC
        let mut file = vec![b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c, 0, 4];
        file.extend_from_slice(&crc32(&start).to_le_bytes());
        file.extend_from_slice(&start);
        file
    }

    #[test]
    fn sevenz_rejects_an_implausible_next_header_as_an_err() {
        // sevenz-rust allocates `vec![0; next_header_size]` before any
        // read: a 32-byte crafted file declaring a multi-TB header
        // aborts the whole process (allocation failure is not an Err
        // the fallback could catch). The arm must reject it up front.
        let tmp = tempfile::tempdir().unwrap();
        let bomb = tmp.path().join("bomb.7z");
        std::fs::write(&bomb, sevenz_start_header(0, 0x0000_ffff_ffff_ffff)).unwrap();
        assert!(native_sevenz_list(&bomb).is_err());

        // A header that merely lies outside the file (plausible size,
        // absurd offset) is rejected by the same length check.
        let out_of_bounds = tmp.path().join("oob.7z");
        std::fs::write(&out_of_bounds, sevenz_start_header(u64::MAX - 40, 8)).unwrap();
        assert!(native_sevenz_list(&out_of_bounds).is_err());
    }

    /// gzip `data` in memory (the .svgz / bomb fixtures).
    fn gzipped(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn read_bounded_truncates_at_the_cap() {
        // The font/svg arms rely on this bound: a mislabeled huge file
        // yields a truncated buffer (whose parse fails → fallback),
        // never the whole file in memory.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("big.bin");
        std::fs::write(&path, [0u8; 100]).unwrap();
        assert_eq!(read_bounded(&path, 10).unwrap().len(), 10);
        assert_eq!(read_bounded(&path, 1000).unwrap().len(), 100);
    }

    #[test]
    fn svgz_bomb_is_rejected_by_the_inflate_bound() {
        // A ~4 MiB gzip can inflate to gigabytes (usvg itself has no
        // output bound, and an allocation failure would abort rfm,
        // skipping the text fallback): over-budget inflate must be
        // None, sending svg_preview_in to the bat fallback.
        let bomb = gzipped(&vec![b' '; (SVGZ_INFLATED_MAX + 1024) as usize]);
        assert!(
            bomb.len() < SVG_SOURCE_MAX as usize,
            "the bomb must pass the source-read cap to prove the inflate bound"
        );
        assert!(inflate_svgz_bounded(bomb).is_none());
        // Plain XML and a sane gzip pass through unchanged/inflated.
        assert_eq!(
            inflate_svgz_bounded(TEST_SVG.to_vec()).as_deref(),
            Some(TEST_SVG)
        );
        assert_eq!(
            inflate_svgz_bounded(gzipped(TEST_SVG)).as_deref(),
            Some(TEST_SVG)
        );
    }

    #[test]
    fn sqlite_counts_are_skipped_past_the_scan_budget() {
        // Above the budget every count is `?` — the names still list.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("huge.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE t1(a);
             INSERT INTO t1 VALUES (1);",
        )
        .unwrap();
        drop(conn);
        let lines = native_sqlite_lines_bounded(&db, 0).unwrap();
        assert!(lines[0].contains("1 table"), "{lines:?}");
        assert!(
            lines.iter().any(|l| l.contains("t1") && l.contains('?')),
            "counts must be skipped for an over-budget database: {lines:?}"
        );
    }

    #[test]
    fn svgz_still_renders_through_the_bounded_inflate() {
        // .svgz (gzip'd SVG) must keep rendering once the arm inflates
        // the gzip layer itself instead of trusting usvg's unbounded
        // auto-decompression.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("pic.svgz");
        std::fs::write(&path, gzipped(TEST_SVG)).unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        match svg_preview_in(None, &path, modified) {
            Preview::Image { img, .. } => {
                let img = img.expect("svgz must render");
                assert_eq!((img.width(), img.height()), (960, 480));
            }
            Preview::Text { lines } => panic!("expected a rendered svgz, got text: {lines:?}"),
        }
    }

    #[test]
    fn compressed_tar_listing_stops_at_the_decompression_budget() {
        // tar over a non-Seek stream reaches the next header by
        // read-and-discarding the content in between: a crafted first
        // member spanning the whole scan budget must truncate the
        // listing there instead of decompressing gigabytes (CPU DoS on
        // the preview task) to reach later headers.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("padded.tar.gz");
        let gz = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(gz);
        let budget = 64 * 1024 * 1024; // = TAR_SCAN_MAX
        let mut header = tar::Header::new_gnu();
        header.set_size(budget);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "big.bin", io::repeat(0).take(budget))
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(7);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "late.txt", &b"content"[..])
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let lines = expect_lines(gz_preview(&path));
        assert!(lines.iter().any(|l| l.contains("big.bin")), "{lines:?}");
        assert!(
            lines.iter().all(|l| !l.contains("late.txt")),
            "the scan budget must stop the listing: {lines:?}"
        );
    }
}

#[cfg(test)]
mod pdf_tests {
    use super::*;
    use lopdf::{
        content::{Content, Operation},
        dictionary, Document, Object, Stream,
    };

    /// Default per-page content: one text block with a page marker.
    fn page_ops(n: usize) -> Vec<Operation> {
        vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![100.into(), 600.into()]),
            Operation::new(
                "Tj",
                vec![Object::string_literal(format!("Hello PDF page {n}"))],
            ),
            Operation::new("ET", vec![]),
        ]
    }

    /// Catalog/Pages/Page skeleton with /Info (Title = `title`,
    /// Author with an embedded newline — deliberate, lopdf hands /Info
    /// strings through raw — and a Producer). One page per element of
    /// `pages`, each with the given content operations.
    fn build_pdf(title: &str, pages: Vec<Vec<Operation>>) -> Document {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut kids: Vec<Object> = Vec::new();
        let count = pages.len() as i64;
        for ops in pages {
            let content = Content { operations: ops };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
            });
            kids.push(page_id.into());
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids,
                "Count" => count,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        let info_id = doc.add_object(dictionary! {
            "Title" => Object::string_literal(title),
            "Author" => Object::string_literal("Prob\ne Author"),
            "Producer" => Object::string_literal("rfm test producer"),
        });
        doc.trailer.set("Info", info_id);
        doc
    }

    fn save_pdf(dir: &Path, name: &str, mut doc: Document) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn make_pdf(dir: &Path, name: &str, title: &str, n_pages: usize) -> PathBuf {
        let pages = (1..=n_pages).map(page_ops).collect();
        save_pdf(dir, name, build_pdf(title, pages))
    }

    /// Detection-only /Encrypt fixture: lopdf's `is_encrypted` checks
    /// the trailer; the crafted O/U never authenticate with "".
    fn make_encrypted_pdf(dir: &Path) -> PathBuf {
        let mut doc = build_pdf("Secret Title", vec![page_ops(1), page_ops(2)]);
        let enc_id = doc.add_object(dictionary! {
            "Filter" => "Standard",
            "V" => 1,
            "R" => 2,
            "O" => Object::string_literal(vec![0u8; 32]),
            "U" => Object::string_literal(vec![0u8; 32]),
            "P" => -1,
        });
        doc.trailer.set("Encrypt", enc_id);
        save_pdf(dir, "locked.pdf", doc)
    }

    /// Page-1 Contents is a FlateDecode stream inflating to
    /// `inflated_len` bytes — mostly padding around a marker text that
    /// must never surface in a preview (proof the stream was dropped,
    /// not inflated and parsed).
    fn make_bomb_pdf(dir: &Path, inflated_len: usize) -> PathBuf {
        use std::io::Write;
        let mut doc = build_pdf("Bomb", vec![page_ops(1)]);
        let mut inflated = b"BT /F1 24 Tf (BOMB MARKER) Tj ET\n".to_vec();
        inflated.resize(inflated_len, b' ');
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&inflated).unwrap();
        let compressed = enc.finish().unwrap();
        let bomb_id = doc.add_object(
            Stream::new(dictionary! { "Filter" => "FlateDecode" }, compressed)
                .with_compression(false),
        );
        let page_id = *doc.get_pages().get(&1).unwrap();
        doc.get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set("Contents", bomb_id);
        save_pdf(dir, "bomb.pdf", doc)
    }

    /// Two objects referencing each other, page-1 Contents pointing
    /// into the loop.
    fn make_loop_pdf(dir: &Path) -> PathBuf {
        let mut doc = build_pdf("Loop", vec![page_ops(1)]);
        let a_id = doc.new_object_id();
        let b_id = doc.new_object_id();
        doc.objects.insert(a_id, Object::Reference(b_id));
        doc.objects.insert(b_id, Object::Reference(a_id));
        let page_id = *doc.get_pages().get(&1).unwrap();
        doc.get_object_mut(page_id)
            .and_then(Object::as_dict_mut)
            .unwrap()
            .set("Contents", a_id);
        save_pdf(dir, "loop.pdf", doc)
    }

    fn text_tier_lines(path: &Path) -> Vec<String> {
        match pdf_text_tier(path).unwrap() {
            Preview::Text { lines } => lines,
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn pdf_text_tier_shows_page_count_title_and_first_page_text() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "report.pdf", "Quarterly Report", 3);
        let lines = text_tier_lines(&pdf);
        let joined = lines.join("\n");
        assert!(joined.contains("PDF · 3 pages"), "{joined}");
        assert!(joined.contains("Quarterly Report"), "{joined}");
        assert!(joined.contains("Hello PDF page 1"), "{joined}");
        assert!(
            !joined.contains("Hello PDF page 2"),
            "page 1 only: {joined}"
        );
    }

    #[test]
    fn pdf_info_strings_are_scrubbed() {
        // The Author fixture value carries an embedded newline (lopdf
        // hands /Info strings through raw — proven); it must render as
        // ONE line.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "report.pdf", "Title", 1);
        let lines = text_tier_lines(&pdf);
        assert!(
            lines.iter().all(|l| !l.contains(['\r', '\n'])),
            "newlines must be scrubbed: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("Probe Author")),
            "the scrubbed author must still show: {lines:?}"
        );
    }

    #[test]
    fn pdf_text_is_capped_at_128_lines() {
        // 200 BT..ET blocks extract as 200 lines; the preview output is
        // capped at the shared 128.
        let tmp = tempfile::tempdir().unwrap();
        let mut ops = Vec::new();
        for i in 0..200 {
            ops.push(Operation::new("BT", vec![]));
            ops.push(Operation::new("Tf", vec!["F1".into(), 12.into()]));
            ops.push(Operation::new("Td", vec![10.into(), 10.into()]));
            ops.push(Operation::new(
                "Tj",
                vec![Object::string_literal(format!("body line {i}"))],
            ));
            ops.push(Operation::new("ET", vec![]));
        }
        let pdf = save_pdf(tmp.path(), "long.pdf", build_pdf("Long", vec![ops]));
        let lines = text_tier_lines(&pdf);
        assert!(lines.len() <= 128, "cap must hold, got {}", lines.len());
        assert!(lines.iter().any(|l| l.contains("body line 0")), "{lines:?}");
        assert!(
            lines.iter().all(|l| !l.contains("body line 199")),
            "the tail past the cap must be dropped"
        );
    }

    #[test]
    fn pdf_lines_are_length_capped() {
        // A PDF can emit one multi-megabyte text run with no newlines;
        // the retained line is truncated (char-boundary safe).
        let tmp = tempfile::tempdir().unwrap();
        let huge = "A".repeat(100 * 1024);
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 12.into()]),
            Operation::new("Tj", vec![Object::string_literal(huge)]),
            Operation::new("ET", vec![]),
        ];
        let pdf = save_pdf(tmp.path(), "wide.pdf", build_pdf("Wide", vec![ops]));
        let lines = text_tier_lines(&pdf);
        assert!(
            lines.iter().all(|l| l.chars().count() <= PDF_LINE_MAX),
            "every line must be capped at {PDF_LINE_MAX} chars"
        );
        assert!(
            lines.iter().any(|l| l.contains("AAAA")),
            "the truncated run must still show: {lines:?}"
        );
    }

    #[test]
    fn encrypted_pdf_shows_encrypted_line_and_page_count() {
        // Metadata = file Size/Modified, NOT /Info strings: in a real
        // encrypted PDF the strings are encrypted garbage (the
        // dictionaries are not, which is why the page count reads).
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_encrypted_pdf(tmp.path());
        let lines = text_tier_lines(&pdf);
        assert_eq!(lines[0], "encrypted PDF (2 pages)", "{lines:?}");
        let joined = lines.join("\n");
        assert!(joined.contains("Size:"), "{joined}");
        assert!(joined.contains("Modified:"), "{joined}");
        assert!(
            !joined.contains("Secret Title"),
            "/Info strings must not show for an encrypted pdf: {joined}"
        );
    }

    #[test]
    fn truncated_pdf_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "whole.pdf", "T", 1);
        let bytes = std::fs::read(&pdf).unwrap();
        let cut = tmp.path().join("cut.pdf");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).unwrap();
        assert!(pdf_text_tier(&cut).is_err());
    }

    #[test]
    fn garbage_pdf_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("bogus.pdf");
        std::fs::write(&bogus, b"%PDF-1.4\nnot a pdf").unwrap();
        assert!(pdf_text_tier(&bogus).is_err());
    }

    #[test]
    fn oversized_pdf_is_an_error() {
        // The size pre-check replaces a literal bounded read
        // (load_filtered takes a path and slurps): past source_max the
        // tier bails before parsing anything.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "big.pdf", "Big", 1);
        let len = std::fs::metadata(&pdf).unwrap().len();
        assert!(load_pdf_guarded_bounded(&pdf, len - 1, PDF_DECOMP_BUDGET).is_err());
        // The same file parses fine under the real cap.
        assert!(load_pdf_guarded_bounded(&pdf, PDF_SOURCE_MAX, PDF_DECOMP_BUDGET).is_ok());
    }

    #[test]
    fn flate_bomb_content_stream_is_dropped_not_inflated() {
        // lopdf's decompress_zlib is an unbounded read_to_end; the
        // guard filter must size-verify and drop the stream BEFORE
        // lopdf ever inflates it. 16 MiB inflated vs a 1 MiB budget.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_bomb_pdf(tmp.path(), 16 * 1024 * 1024);
        let doc = load_pdf_guarded_bounded(&pdf, PDF_SOURCE_MAX, 1024 * 1024).unwrap();
        let lines = native_pdf_lines(&doc, &pdf);
        let joined = lines.join("\n");
        assert!(
            !joined.contains("BOMB MARKER"),
            "the bomb stream must be dropped, not parsed: {joined}"
        );
        // The metadata header survives the dropped body.
        assert!(joined.contains("PDF · 1 page"), "{joined}");
    }

    #[test]
    fn pdf_render_defaults_off_when_uninitialized() {
        // The raster-cache convention: uninitialized == opted out.
        // Unit tests never call set_pdf_render, so every other test
        // stays hermetic (no probe spawns, no external renders).
        assert!(!pdf_render_enabled());
    }

    /// Cache-dir entries belonging to `pdf` (16-hex hash prefix match),
    /// any kind — the leftovers detector for the render tests.
    fn cache_entries_of(dir: &Path, pdf: &Path) -> Vec<String> {
        let prefix = raster_cache::entry_name(pdf, 0, raster_cache::KIND_PDF)[..17].to_string();
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with(&prefix))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn pdf_render_hits_the_cache_without_spawning() {
        // A pre-seeded entry must be served BEFORE any renderer spawn:
        // this passes on a machine without pdftoppm (no skip guard —
        // that absence is the point).
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let pdf = tmp.path().join("doc.pdf");
        std::fs::write(&pdf, b"never parsed on the hit path").unwrap();
        let img = image::DynamicImage::ImageRgb8(image::RgbImage::new(8, 8));
        raster_cache::store_in(cache.path(), &pdf, 42, raster_cache::KIND_PDF, &img).unwrap();
        match pdf_render_first_page_in(cache.path(), PdfRenderer::Pdftoppm, &pdf, 42) {
            Ok(Preview::Image { img, .. }) => assert!(img.is_some()),
            other => panic!("expected a cache-hit image preview, got {other:?}"),
        }
    }

    #[test]
    fn pdftoppm_renders_page_one_into_the_cache() {
        let Some(renderer) = pdf_renderer() else {
            eprintln!("skipping: no pdf renderer installed");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "doc.pdf", "Rendered", 2);
        match pdf_render_first_page_in(cache.path(), renderer, &pdf, 7).unwrap() {
            Preview::Image { img, .. } => assert!(img.is_some()),
            other => panic!("expected an image preview, got {other:?}"),
        }
        let name = raster_cache::entry_name(&pdf, 7, raster_cache::KIND_PDF);
        let entry = cache.path().join(&name);
        assert!(entry.is_file(), "the render must land in the cache");
        let cached = raster_cache::lookup_in(cache.path(), &pdf, 7, raster_cache::KIND_PDF)
            .expect("the cached entry must decode");
        assert!(
            cached.width() <= 960 && cached.height() <= 540,
            "bounded render: {}x{}",
            cached.width(),
            cached.height()
        );
        // atomicity: no .part siblings left behind
        assert_eq!(cache_entries_of(cache.path(), &pdf), vec![name]);
    }

    #[test]
    fn pdf_render_failure_leaves_no_part_files() {
        let Some(renderer) = pdf_renderer() else {
            eprintln!("skipping: no pdf renderer installed");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let garbage = tmp.path().join("broken.pdf");
        std::fs::write(&garbage, b"%PDF-1.4\nnot a pdf").unwrap();
        assert!(pdf_render_first_page_in(cache.path(), renderer, &garbage, 7).is_err());
        assert!(
            cache_entries_of(cache.path(), &garbage).is_empty(),
            "a failed render must leave nothing behind"
        );
    }

    #[test]
    fn kind_pdf_coexists_with_other_kinds() {
        // The raster-cache key discriminator covers the new kind: same
        // source and mtime, different producer → distinct entries.
        let pdf = Path::new("/some/doc.pdf");
        let pdf_entry = raster_cache::entry_name(pdf, 100, raster_cache::KIND_PDF);
        let img_entry = raster_cache::entry_name(pdf, 100, raster_cache::KIND_IMAGE);
        assert_ne!(pdf_entry, img_entry);
        assert!(pdf_entry.ends_with("-pdf-p1-960.jpg"), "{pdf_entry}");
    }

    #[test]
    fn pdf_preview_uses_text_tier_when_render_disabled() {
        // renderer None == pdf_render off (or no tool): the composed
        // preview is the text tier, and nothing touches a cache dir.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "report.pdf", "Tiered", 3);
        let modified = pdf.metadata().unwrap().modified().unwrap();
        let mime: mime::Mime = "application/pdf".parse().unwrap();
        match pdf_preview_in(None, None, &pdf, modified, &mime) {
            Preview::Text { lines } => {
                assert!(
                    lines.iter().any(|l| l.contains("PDF · 3 pages")),
                    "{lines:?}"
                );
            }
            _ => panic!("expected the text tier"),
        }
    }

    #[test]
    fn pdf_preview_of_garbage_falls_back_to_stat() {
        // Composition floor: a PDF never shows a bare error panel —
        // garbage degrades text tier -> stat block.
        let tmp = tempfile::tempdir().unwrap();
        let garbage = tmp.path().join("broken.pdf");
        std::fs::write(&garbage, b"%PDF-1.4\nnot a pdf").unwrap();
        let modified = garbage.metadata().unwrap().modified().unwrap();
        let mime: mime::Mime = "application/pdf".parse().unwrap();
        match pdf_preview_in(None, None, &garbage, modified, &mime) {
            Preview::Text { lines } => {
                let joined = lines.join("\n");
                assert!(joined.contains("Size:"), "{joined}");
                assert!(joined.contains("MIME type:"), "{joined}");
            }
            _ => panic!("expected the stat fallback"),
        }
    }

    #[test]
    fn file_preview_dispatches_pdf_to_the_text_tier() {
        // End-to-end: FilePreview::new on a .pdf must produce the
        // text-tier lines, not the plain stat block the old
        // ("application", _) catch-all served. Extensionless %PDF
        // routing is covered by opener.rs's
        // extensionless_pdf_bytes_get_application_pdf.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_pdf(tmp.path(), "whatever.pdf", "Dispatched", 1);
        match FilePreview::new(pdf).preview {
            Preview::Text { lines } => {
                let joined = lines.join("\n");
                assert!(joined.contains("PDF · 1 page"), "{joined}");
                assert!(joined.contains("Dispatched"), "{joined}");
                assert!(
                    !joined.contains("MIME type:"),
                    "must not land on the stat catch-all: {joined}"
                );
            }
            _ => panic!("expected a text preview"),
        }
    }

    #[test]
    fn reference_loop_pdf_terminates() {
        // lopdf's DEREF_LIMIT bounds the chase; the tier must simply
        // return (metadata Ok, or Err) instead of spinning.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_loop_pdf(tmp.path());
        match pdf_text_tier(&pdf) {
            Ok(Preview::Text { lines }) => {
                assert!(lines.iter().any(|l| l.contains("PDF")), "{lines:?}")
            }
            Ok(_) => panic!("expected a text preview"),
            Err(_) => {} // an error is a valid terminating outcome
        }
    }

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    /// Hand-assemble a minimal PDF-1.5 whose cross-reference is a
    /// `/Filter /FlateDecode` xref STREAM (which lopdf's `save_to` never
    /// emits — it writes classic tables). The xref stream inflates to
    /// `xref_padding` bytes of trailing filler beyond the 25 real entry
    /// bytes; a large `xref_padding` is a compression-ratio bomb that
    /// lopdf inflates during xref parsing, BEFORE the object guard runs.
    fn make_xref_stream_pdf(dir: &Path, name: &str, xref_padding: usize) -> PathBuf {
        // W = [1 2 1]: type(1) offset(2) gen(1). Objects fit under 64 KiB
        // so 2-byte offsets suffice.
        fn entry(v: &mut Vec<u8>, t: u8, off: u32, gen: u8) {
            v.push(t);
            v.push((off >> 8) as u8);
            v.push(off as u8);
            v.push(gen);
        }
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n");
        let mut off = [0usize; 5];
        off[1] = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        off[2] = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        off[3] = buf.len();
        buf.extend_from_slice(
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>\nendobj\n",
        );
        // Object 4 is the xref stream itself.
        off[4] = buf.len();
        let mut xref = Vec::new();
        entry(&mut xref, 0, 0, 255); // object 0: the free-list head
        for &obj_off in &off[1..=4] {
            entry(&mut xref, 1, obj_off as u32, 0);
        }
        xref.resize(xref.len() + xref_padding, 0);
        let compressed = zlib_compress(&xref);
        let dict = format!(
            "<< /Type /XRef /Size 5 /Index [0 5] /W [1 2 1] /Root 1 0 R /Length {} /Filter /FlateDecode >>",
            compressed.len()
        );
        buf.extend_from_slice(format!("4 0 obj\n{dict}\nstream\n").as_bytes());
        buf.extend_from_slice(&compressed);
        buf.extend_from_slice(b"\nendstream\nendobj\n");
        buf.extend_from_slice(format!("startxref\n{}\n%%EOF", off[4]).as_bytes());
        let path = dir.join(name);
        std::fs::write(&path, buf).unwrap();
        path
    }

    #[test]
    fn valid_xref_stream_pdf_loads() {
        // Guards against over-rejection: a legitimate (tiny) xref stream
        // must still parse through the pre-scan and into lopdf.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_xref_stream_pdf(tmp.path(), "modern.pdf", 0);
        let doc = load_pdf_guarded_bounded(&pdf, PDF_SOURCE_MAX, PDF_DECOMP_BUDGET)
            .expect("a valid xref-stream pdf must load");
        assert_eq!(doc.get_pages().len(), 1);
    }

    #[test]
    fn xref_stream_flate_bomb_is_rejected_before_lopdf_inflates_it() {
        // The bomb inflates to ~4 MiB from a few hundred compressed
        // bytes; under a 1 MiB budget the pre-scan must reject the file
        // WITHOUT handing it to lopdf (whose xref parsing would inflate
        // it fully — the object guard never sees an xref stream). This
        // is the compressed-xref-stream OOM class from cursor nav.
        let tmp = tempfile::tempdir().unwrap();
        let pdf = make_xref_stream_pdf(tmp.path(), "bomb.pdf", 4 * 1024 * 1024);
        // Sanity: on-disk size stays tiny (a real ratio bomb).
        assert!(
            std::fs::metadata(&pdf).unwrap().len() < 64 * 1024,
            "the compressed bomb must be small on disk"
        );
        let res = load_pdf_guarded_bounded(&pdf, PDF_SOURCE_MAX, 1024 * 1024);
        assert!(
            res.is_err(),
            "the xref-stream decompression bomb must be rejected"
        );
    }

    #[test]
    fn chained_flate_filters_are_charged_super_linearly() {
        // A stream with /Filter [/FlateDecode /FlateDecode] inflates up
        // to 1032^2 : 1, not 1032 : 1. The pre-fix single multiply
        // (content.len() * 1032) kept ~980 bytes under the 64 MiB
        // budget; the true bound 980 * 1032^2 ~= 1 GiB is over budget,
        // so the charge must report None (drop the stream).
        let content = vec![0u8; 980];
        let stream = Stream::new(
            dictionary! {
                "Filter" => vec![Object::from("FlateDecode"), Object::from("FlateDecode")],
            },
            content,
        )
        .with_compression(false);
        assert!(
            pdf_stream_charge(&stream, PDF_DECOMP_BUDGET).is_none(),
            "a two-stage flate chain must be charged 1032^2, over budget"
        );
        // A single FlateDecode of the same size is still counted exactly
        // (and here decodes to near-nothing, well under budget).
        let single = Stream::new(
            dictionary! { "Filter" => "FlateDecode" },
            zlib_compress(b"hello"),
        )
        .with_compression(false);
        assert!(pdf_stream_charge(&single, PDF_DECOMP_BUDGET).is_some());
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

    #[test]
    fn resized_rgb_pixel_keying_reuses_cache() {
        // The cache is keyed on the *requested* dimensions — the unit is the
        // caller's business (D8). Graphics tiers request the pixel box
        // (cells x cell-pixel-size) instead of cell-derived dims; the
        // protocol never changes mid-session, so the single-entry cache
        // still never thrashes at pixel scale.
        let mut p = tiny_preview();
        p.resized_rgb(240, 320); // 30x20 cells at 8x16 px
        p.resized_rgb(240, 320);
        assert_eq!(p.resize_count(), 1, "same pixel box must hit the cache");
        // A resize changes the pixel box -> recompute.
        p.resized_rgb(256, 320);
        assert_eq!(p.resize_count(), 2);
    }

    #[test]
    fn graphics_row_budget_matches_half_block_split() {
        // The graphics image gets the full height when there are no info
        // lines, else 2/3 of it — mirroring the half-block branch's
        // 2*height / 4*height/3 *pixel* budget (2 px per cell row).
        assert_eq!(image_rows(30, false), 30);
        assert_eq!(image_rows(30, true), 20);
        assert_eq!(image_rows(1, true), 0);
        assert_eq!(image_rows(0, false), 0);
    }

    #[test]
    fn graphics_geometry_requirements_per_protocol() {
        // The test binary never calls graphics::init(), so no cell geometry
        // was ever reported. Kitty survives that with the assumed 8x16 cell
        // (the terminal scales into the c=/r= rectangle, D2); sixel places
        // a raw raster and MUST have real geometry -> None forces the
        // half-block fallback. HalfBlock is not a graphics emitter at all.
        assert_eq!(
            geometry_for(GraphicsProtocol::Kitty),
            Some(ASSUMED_CELL),
            "kitty falls back to the assumed cell size"
        );
        assert_eq!(geometry_for(GraphicsProtocol::Sixel), None);
        assert_eq!(geometry_for(GraphicsProtocol::HalfBlock), None);
    }
}

#[cfg(test)]
mod graphics_draw_tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// The image branch owns its whole pane region (full-repaint
    /// invariant): the strip beside a narrower-than-pane raster must be
    /// painted on EVERY frame, including gated ones that write zero raster
    /// bytes — otherwise the previous preview's cells persist there.
    #[test]
    fn graphics_draw_repaints_strip_beside_narrow_image() {
        let _g = graphics::emit_test_guard();
        // Portrait source, much narrower than the pane: 10x100 fitted into
        // the 39x20-cell pane (assumed 8x16 kitty cell -> 312x320 px)
        // scales to 32x320 px -> 4 cells wide, 20 rows tall.
        let src = DynamicImage::ImageRgb8(RgbImage::from_pixel(10, 100, Rgb([1, 2, 3])));
        let mut cache = None;
        let mut sink = Vec::new();
        let rows = draw_graphics(
            GraphicsProtocol::Kitty,
            &mut sink,
            &mut cache,
            &src,
            false,
            &(0..40),
            &(0..20),
            Path::new("/strip.png"),
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(rows, 20);
        let s = String::from_utf8_lossy(&sink);
        assert!(s.contains("\x1b_G"), "raster not transmitted: {s:?}");
        // Strip = columns 5..40 (0-based; CSI is 1-based) for all 20 rows.
        let strip_first = format!("\x1b[1;6H{}", " ".repeat(35));
        let strip_last = format!("\x1b[20;6H{}", " ".repeat(35));
        assert!(s.contains(&strip_first), "strip row 0 not painted: {s:?}");
        assert!(s.contains(&strip_last), "strip row 19 not painted: {s:?}");

        // Gated repaint (same key): zero raster bytes, strip still painted.
        let mut sink = Vec::new();
        let rows = draw_graphics(
            GraphicsProtocol::Kitty,
            &mut sink,
            &mut cache,
            &src,
            false,
            &(0..40),
            &(0..20),
            Path::new("/strip.png"),
            SystemTime::UNIX_EPOCH,
        )
        .unwrap();
        assert_eq!(rows, 20);
        let s = String::from_utf8_lossy(&sink);
        assert!(!s.contains("\x1b_G"), "gated frame retransmitted: {s:?}");
        assert!(
            s.contains(&strip_first) && s.contains(&strip_last),
            "gated frame must still repaint the strip: {s:?}"
        );
    }
}
