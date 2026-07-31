//! Terminal graphics protocol support for image previews.
//!
//! This module owns which protocol (kitty graphics / sixel / half-block
//! fallback) the preview column may use, the cell→pixel geometry needed
//! to place real pixels into a cell layout, and the emitters themselves.
//! The detection core is pure — env lookups and probe replies are
//! injected — so the whole decision matrix is unit-testable without a
//! terminal; the emitters write to any `impl Write`, so their byte
//! streams, the `EmitKey` re-emission gating and the frame
//! claim-and-reconcile are unit-tested against `Vec<u8>` sinks.

use std::io::{self, Write};
use std::ops::Range;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::config::ImageProtocolChoice;

mod sixel;

/// Wall-clock budget for the whole startup probe (D1).
const PROBE_BUDGET: Duration = Duration::from_millis(250);

/// The graphics protocol the preview column draws images with.
///
/// `HalfBlock` is the universal fallback (today's `▄` truecolor renderer) —
/// it is what every code path resolves to until detection says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    Kitty,
    Sixel,
    HalfBlock,
}

impl GraphicsProtocol {
    /// Stable string form, matching the `image_protocol` config values and
    /// what the debug socket reports.
    pub fn name(&self) -> &'static str {
        match self {
            GraphicsProtocol::Kitty => "kitty",
            GraphicsProtocol::Sixel => "sixel",
            GraphicsProtocol::HalfBlock => "half-block",
        }
    }
}

/// Environment heuristics for `auto` detection. Returns `None` when the env
/// is inconclusive (meaning: run the terminal probe).
///
/// The lookup is injected so the whole matrix is testable without touching
/// the process environment. Multiplexers are checked *first*: tmux/screen
/// swallow graphics escapes without passthrough, so tmux-on-kitty must stay
/// on half-blocks even though the kitty vars leak through.
pub fn detect_from_env(env: &dyn Fn(&str) -> Option<String>) -> Option<GraphicsProtocol> {
    if env("TMUX").is_some() {
        return Some(GraphicsProtocol::HalfBlock);
    }
    let term = env("TERM").unwrap_or_default();
    if term.starts_with("tmux") || term.starts_with("screen") {
        return Some(GraphicsProtocol::HalfBlock);
    }
    if env("KITTY_WINDOW_ID").is_some() || term == "xterm-kitty" {
        return Some(GraphicsProtocol::Kitty);
    }
    if let Some(prog) = env("TERM_PROGRAM") {
        // WezTerm and Ghostty implement the kitty graphics protocol.
        if prog.eq_ignore_ascii_case("wezterm") || prog.eq_ignore_ascii_case("ghostty") {
            return Some(GraphicsProtocol::Kitty);
        }
    }
    None
}

/// What the startup probe replies revealed about the terminal.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// The kitty graphics query was answered with `OK`.
    pub kitty_ok: bool,
    /// Primary Device Attributes listed capability `4` (sixel).
    pub sixel: bool,
    /// `CSI 14 t` text-area size reply, as `(width_px, height_px)`.
    pub pixel_size: Option<(u16, u16)>,
    /// A DA1 reply arrived. DA1 is answered by essentially every terminal
    /// and is sent *last* in the probe batch, so this doubles as the
    /// "all replies are in" marker for the bounded read loop.
    pub da1_seen: bool,
}

/// Parse the byte stream read back after the probe batch. Tolerates garbage
/// before/between the replies (some terminals echo unrelated sequences).
pub fn parse_probe(buf: &[u8]) -> ProbeOutcome {
    let mut outcome = ProbeOutcome::default();
    let mut i = 0;
    while i < buf.len() {
        if buf[i] != 0x1b {
            i += 1;
            continue;
        }
        let rest = &buf[i + 1..];
        match rest.first() {
            // APC: kitty graphics reply `ESC _ G <payload> ESC \`
            Some(b'_') => {
                let body = &rest[1..];
                let end = find_st(body);
                let payload = &body[..end.unwrap_or(body.len())];
                if let Some(stripped) = payload.strip_prefix(b"G") {
                    // Payload is `<keys>;<response>`; success iff the
                    // response is exactly "OK".
                    if let Some(sep) = stripped.iter().position(|&b| b == b';') {
                        outcome.kitty_ok |= &stripped[sep + 1..] == b"OK";
                    }
                }
                i += 1 + 1 + end.map(|e| e + 2).unwrap_or(payload.len());
            }
            // CSI: DA1 reply `ESC [ ? <attrs> c` or size reply `ESC [ 4 ; h ; w t`
            Some(b'[') => {
                let body = &rest[1..];
                // The final byte of a CSI sequence is in 0x40..=0x7e.
                let Some(fin) = body.iter().position(|&b| (0x40..=0x7e).contains(&b)) else {
                    break; // truncated CSI at the buffer end
                };
                let params = &body[..fin];
                match body[fin] {
                    b'c' if params.first() == Some(&b'?') => {
                        outcome.da1_seen = true;
                        let attrs = std::str::from_utf8(&params[1..]).unwrap_or("");
                        outcome.sixel |= attrs.split(';').any(|a| a == "4");
                    }
                    b't' => {
                        // `ESC [ 4 ; <height> ; <width> t`
                        let params = std::str::from_utf8(params).unwrap_or("");
                        let mut parts = params.split(';');
                        if parts.next() == Some("4") {
                            if let (Some(Ok(h)), Some(Ok(w))) = (
                                parts.next().map(str::parse::<u16>),
                                parts.next().map(str::parse::<u16>),
                            ) {
                                outcome.pixel_size = Some((w, h));
                            }
                        }
                    }
                    _ => {}
                }
                i += 1 + 1 + fin + 1;
            }
            _ => i += 1,
        }
    }
    outcome
}

/// Find the ST terminator (`ESC \`) in `buf`, returning the index of its ESC.
fn find_st(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\x1b\\")
}

/// Combine the config choice, the env heuristic and the probe outcome into
/// the protocol to use. Explicit choices always win; `auto` prefers the env
/// hit, then the probe (kitty over sixel), and falls back to `HalfBlock` on
/// no data (the probe-timeout path).
pub fn resolve(
    choice: ImageProtocolChoice,
    env_hit: Option<GraphicsProtocol>,
    probe: Option<&ProbeOutcome>,
) -> GraphicsProtocol {
    match choice {
        ImageProtocolChoice::Kitty => GraphicsProtocol::Kitty,
        ImageProtocolChoice::Sixel => GraphicsProtocol::Sixel,
        ImageProtocolChoice::HalfBlock => GraphicsProtocol::HalfBlock,
        ImageProtocolChoice::Auto => {
            if let Some(hit) = env_hit {
                return hit;
            }
            match probe {
                Some(p) if p.kitty_ok => GraphicsProtocol::Kitty,
                Some(p) if p.sixel => GraphicsProtocol::Sixel,
                _ => GraphicsProtocol::HalfBlock,
            }
        }
    }
}

/// Size of one character cell in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellGeometry {
    pub cell_w: u16,
    pub cell_h: u16,
}

/// Derive the cell size from a window size report (cols/rows + total pixel
/// size). Returns `None` on any zero or degenerate input — `TIOCGWINSZ`
/// commonly reports zero pixel fields over ssh/serial, and a zero must never
/// reach a division.
pub fn cell_geometry_from_winsize(
    cols: u16,
    rows: u16,
    xpx: u16,
    ypx: u16,
) -> Option<CellGeometry> {
    if cols == 0 || rows == 0 || xpx == 0 || ypx == 0 {
        return None;
    }
    let cell_w = xpx / cols;
    let cell_h = ypx / rows;
    if cell_w == 0 || cell_h == 0 {
        return None;
    }
    Some(CellGeometry { cell_w, cell_h })
}

/// The pixel box covered by a span of cells — what the raster is fitted into.
/// Used by the graphics emitters (steps 4–6).
pub fn pixel_box(cols: u16, rows: u16, geo: CellGeometry) -> (u32, u32) {
    (
        cols as u32 * geo.cell_w as u32,
        rows as u32 * geo.cell_h as u32,
    )
}

/// Cells needed to cover `px` pixels (ceil division) — used for the kitty
/// `c=`/`r=` placement and the erase region (steps 4–6). A zero cell size
/// yields zero cells instead of dividing.
pub fn cells_for(px: u32, cell_px: u16) -> u16 {
    if cell_px == 0 {
        return 0;
    }
    px.div_ceil(cell_px as u32).min(u16::MAX as u32) as u16
}

/// Kitty `c=`/`r=` placement for a raster: ceil to cells, clamped to the
/// pane span so the image can never bleed into a neighbouring panel.
pub fn placement_cells(
    px_w: u32,
    px_h: u32,
    geo: CellGeometry,
    max_cols: u16,
    max_rows: u16,
) -> (u16, u16) {
    (
        cells_for(px_w, geo.cell_w).min(max_cols),
        cells_for(px_h, geo.cell_h).min(max_rows),
    )
}

// --- Frame claim + reconcile, kitty emitter (D4/D5/D6).

/// Identity of a transmitted preview image (D5). When the computed key
/// equals the live one, a repaint emits *nothing* for the raster region —
/// unrelated dirty repaints cost zero retransmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitKey {
    pub path: PathBuf,
    pub mtime_secs: u64,
    pub px_w: u32,
    pub px_h: u32,
    /// Top-left cell of the placement, 0-based screen coordinates.
    pub origin_cell: (u16, u16),
}

/// The placement currently on screen.
#[derive(Debug, Clone)]
struct LiveImage {
    key: EmitKey,
    /// Kitty image id; `None` for protocols without ids (sixel).
    id: Option<u32>,
}

/// Module-global (not per-`FilePreview`): a new preview *replaces* the whole
/// `FilePreview`, so a per-instance field could never erase its
/// predecessor's placement (D5).
static LIVE: Mutex<Option<LiveImage>> = Mutex::new(None);
/// The emit key claimed by this frame's draw pass, reconciled in
/// [`end_frame`].
static CLAIMED: Mutex<Option<EmitKey>> = Mutex::new(None);
/// Whether the current frame may place a graphics image (single view, no
/// console overlay). Defaults to `false` so draw-path unit tests (which
/// never run [`begin_frame`]) stay on the half-block fallback.
static FRAME_ALLOWED: AtomicBool = AtomicBool::new(false);
/// Fresh kitty image id per transmission. Fixed non-zero base so rfm's ids
/// are recognizable in terminal-side debugging.
static NEXT_ID: AtomicU32 = AtomicU32::new(0x4d46);

/// Result of an emit call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emitted {
    /// The raster was (re-)transmitted.
    Transmitted,
    /// The live placement already matches the key — nothing was written.
    Gated,
}

/// Mark the start of a draw pass (top of `PanelManager::draw`, D4).
/// `image_allowed`: single view and no console overlay — a kitty placement
/// floats above cells, so overlays must force the half-block fallback.
pub fn begin_frame(image_allowed: bool) {
    FRAME_ALLOWED.store(image_allowed, Ordering::Relaxed);
    *CLAIMED.lock() = None;
}

/// Whether the current frame may draw via a graphics protocol.
pub fn frame_allows_image() -> bool {
    FRAME_ALLOWED.load(Ordering::Relaxed)
}

/// Reconcile at the end of a draw pass (just before
/// `EndSynchronizedUpdate`): a live placement whose key was not claimed this
/// frame is stale — the selection moved, an overlay opened, the view split —
/// and is erased. This one hook covers every stale-image case (D4).
///
/// The reconcile must never write cell content: it runs AFTER the draw pass
/// has fully repainted the screen, so stamping spaces here would wipe the
/// freshly drawn cells (and any overlay on top). Kitty placements float
/// above cells and need the delete-by-id; id-less (sixel) placements *are*
/// cell content and were already overwritten by this frame's repaint — for
/// them the reconcile only forgets the live state.
pub fn end_frame(w: &mut impl Write) -> io::Result<()> {
    let claimed = CLAIMED.lock().take();
    let mut live = LIVE.lock();
    if let Some(l) = live.as_ref() {
        if claimed.as_ref() != Some(&l.key) {
            delete_placement(w, l)?;
            *live = None;
        }
    }
    Ok(())
}

/// Transmit `rgb` via the kitty graphics protocol, gated on `key`:
/// - live key equals `key` → write nothing, claim the frame, `Gated`;
/// - otherwise delete the previous placement (by id), blank the target
///   region (clears half-block remnants from fallback frames), place the
///   cursor at the origin and transmit base64-chunked with a fresh id.
///
/// Sized `c=`/`r=` cells so the terminal scales into exactly the pane
/// rectangle (exact clipping, D6). `q=2` everywhere: the terminal must
/// never answer into the event stream.
pub fn emit_kitty(
    w: &mut impl Write,
    key: EmitKey,
    rgb: &image::RgbImage,
    cols: u16,
    rows: u16,
) -> io::Result<Emitted> {
    if LIVE.lock().as_ref().map_or(false, |l| l.key == key) {
        *CLAIMED.lock() = Some(key);
        log::trace!("graphics: gated (unchanged)");
        return Ok(Emitted::Gated);
    }
    if let Some(old) = LIVE.lock().take() {
        delete_placement(w, &old)?;
    }
    let region = (
        key.origin_cell.0..key.origin_cell.0.saturating_add(cols),
        key.origin_cell.1..key.origin_cell.1.saturating_add(rows),
    );
    blank_region(w, &region)?;
    move_to(w, key.origin_cell)?;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    transmit_kitty(w, id, rgb, cols, rows)?;
    log::trace!(
        "graphics: transmit kitty id={id} {}x{}",
        rgb.width(),
        rgb.height()
    );
    *LIVE.lock() = Some(LiveImage {
        key: key.clone(),
        id: Some(id),
    });
    *CLAIMED.lock() = Some(key);
    Ok(Emitted::Transmitted)
}

/// Transmit `rgb` as a sixel raster, gated on `key` exactly like
/// [`emit_kitty`]:
/// - live key equals `key` → write nothing, claim the frame, `Gated`;
/// - otherwise blank the target region, place the cursor at the origin and
///   write the encoded raster.
///
/// Sixel pixels are ordinary cell content: the placement is recorded
/// id-less and needs no erase sequence — a predecessor's cells are
/// repainted by whichever draw owns them in the current full-repaint frame
/// (blanking them here could wipe panes drawn earlier in the same frame).
/// Only the emitter's own target region is blanked, right before the
/// raster, to clear remnants below the band-truncated pixel rows. The
/// caller pre-sizes the raster to the pane pixel box and the encoder
/// truncates to whole 6-row bands, so the raster cannot overflow into
/// neighbouring panels (D7).
pub fn emit_sixel(
    w: &mut impl Write,
    key: EmitKey,
    rgb: &image::RgbImage,
    cols: u16,
    rows: u16,
) -> io::Result<Emitted> {
    if LIVE.lock().as_ref().map_or(false, |l| l.key == key) {
        *CLAIMED.lock() = Some(key);
        log::trace!("graphics: gated (unchanged)");
        return Ok(Emitted::Gated);
    }
    // Forget the predecessor without touching its cells (delete_placement
    // is a no-op for id-less placements — see its doc).
    if let Some(old) = LIVE.lock().take() {
        delete_placement(w, &old)?;
    }
    let region = (
        key.origin_cell.0..key.origin_cell.0.saturating_add(cols),
        key.origin_cell.1..key.origin_cell.1.saturating_add(rows),
    );
    blank_region(w, &region)?;
    move_to(w, key.origin_cell)?;
    w.write_all(&sixel::sixel_encode(rgb))?;
    log::trace!("graphics: sixel emit {}x{}", rgb.width(), rgb.height());
    *LIVE.lock() = Some(LiveImage {
        key: key.clone(),
        id: None,
    });
    *CLAIMED.lock() = Some(key);
    Ok(Emitted::Transmitted)
}

/// Base64 payload chunk size in chars (kitty protocol limit).
const KITTY_CHUNK: usize = 4096;

/// Direct transmission `a=T,f=24`: the raw RGB bytes, base64, chunked at
/// 4096 chars with `m=1` continuations and a final `m=0`.
fn transmit_kitty(
    w: &mut impl Write,
    id: u32,
    rgb: &image::RgbImage,
    cols: u16,
    rows: u16,
) -> io::Result<()> {
    let payload = b64(rgb.as_raw());
    let bytes = payload.as_bytes();
    let mut chunks: Vec<&[u8]> = bytes.chunks(KITTY_CHUNK).collect();
    if chunks.is_empty() {
        chunks.push(&[]); // degenerate raster: still send one command
    }
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        let m = u8::from(i < last);
        if i == 0 {
            write!(
                w,
                "\x1b_Ga=T,f=24,s={},v={},i={},c={},r={},q=2,m={};",
                rgb.width(),
                rgb.height(),
                id,
                cols,
                rows,
                m
            )?;
        } else {
            write!(w, "\x1b_Gm={m};")?;
        }
        w.write_all(chunk)?;
        w.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// Remove a placement from the terminal. Kitty: `a=d,d=I` — capital `I`
/// also frees the pixel data. Id-less placements (sixel) have no delete
/// command and need none: their pixels are ordinary cell content, replaced
/// whenever the owning cells are repainted — every draw pass is a full
/// repaint, so writing spaces here would only destroy content drawn this
/// frame (or, after a resize, land on unrelated cells).
fn delete_placement(w: &mut impl Write, live: &LiveImage) -> io::Result<()> {
    if let Some(id) = live.id {
        write!(w, "\x1b_Ga=d,d=I,i={id},q=2\x1b\\")?;
        log::trace!("graphics: erase id={id}");
    }
    Ok(())
}

/// Overwrite a cell region with default-colored spaces. Public entry for
/// the preview draw path: the graphics image branch owns its whole pane
/// region and must repaint the strip beside a narrower-than-pane raster on
/// every frame — even when the raster emit itself is gated to zero bytes.
pub fn blank_cells(w: &mut impl Write, cols: Range<u16>, rows: Range<u16>) -> io::Result<()> {
    blank_region(w, &(cols, rows))
}

/// Overwrite a cell region with default-colored spaces.
fn blank_region(w: &mut impl Write, region: &(Range<u16>, Range<u16>)) -> io::Result<()> {
    let (cols, rows) = region;
    if cols.is_empty() || rows.is_empty() {
        return Ok(());
    }
    write!(w, "\x1b[0m")?; // SGR reset: spaces in default colors
    let blanks = " ".repeat(cols.len());
    for y in rows.clone() {
        move_to(w, (cols.start, y))?;
        w.write_all(blanks.as_bytes())?;
    }
    Ok(())
}

/// `CUP` to a 0-based cell (the CSI is 1-based). Raw escape rather than
/// crossterm so the emitters stay testable against any `Write` sink.
fn move_to(w: &mut impl Write, cell: (u16, u16)) -> io::Result<()> {
    write!(w, "\x1b[{};{}H", cell.1 as u32 + 1, cell.0 as u32 + 1)
}

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RFC 4648 base64 with padding. Hand-rolled (~15 lines): the `base64`
/// crate in the lock is transitive-only and not worth the dependency (D6).
fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(B64_TABLE[(n >> 18) as usize & 63] as char);
        out.push(B64_TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64_TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64_TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Test-only view of the live placement: `(key, id)`.
#[cfg(test)]
fn live_snapshot_for_test() -> Option<(EmitKey, Option<u32>)> {
    LIVE.lock().as_ref().map(|l| (l.key.clone(), l.id))
}

/// Reset the module-global emitter state between tests.
#[cfg(test)]
fn reset_emit_state_for_test() {
    *LIVE.lock() = None;
    *CLAIMED.lock() = None;
    FRAME_ALLOWED.store(false, Ordering::Relaxed);
}

/// The emitter state (LIVE/CLAIMED/FRAME_ALLOWED) is module-global by
/// design (D5) — cargo test runs threads in parallel, so every stateful
/// emitter test (here and in the preview draw-path tests) serializes on
/// this lock and starts from a clean slate.
#[cfg(test)]
static EMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Serialize a test that touches the module-global emitter state and reset
/// that state to a clean slate.
#[cfg(test)]
pub(crate) fn emit_test_guard() -> parking_lot::MutexGuard<'static, ()> {
    let guard = EMIT_TEST_LOCK.lock();
    reset_emit_state_for_test();
    guard
}

// --- I/O shell: resolved protocol + geometry, startup probe, ioctl.

/// The protocol resolved at startup. Read through [`protocol`], which
/// defaults to `HalfBlock` while unset so unit tests of the draw path (and
/// any accidental pre-init caller) stay on the universal fallback.
static PROTOCOL: OnceCell<GraphicsProtocol> = OnceCell::new();

/// Cell geometry, packed `cell_w << 16 | cell_h`; `0` means "unknown".
/// An atomic (not a OnceCell) because a font change mid-session alters the
/// cell size and [`refresh_geometry`] re-reads it on every terminal resize.
static GEOMETRY: AtomicU32 = AtomicU32::new(0);

fn pack_geometry(geo: CellGeometry) -> u32 {
    (geo.cell_w as u32) << 16 | geo.cell_h as u32
}

fn unpack_geometry(packed: u32) -> Option<CellGeometry> {
    if packed == 0 {
        return None;
    }
    Some(CellGeometry {
        cell_w: (packed >> 16) as u16,
        cell_h: packed as u16,
    })
}

/// The graphics protocol to draw image previews with.
pub fn protocol() -> GraphicsProtocol {
    *PROTOCOL.get().unwrap_or(&GraphicsProtocol::HalfBlock)
}

/// The current cell size in pixels, if the terminal ever reported one.
pub fn cell_geometry() -> Option<CellGeometry> {
    unpack_geometry(GEOMETRY.load(Ordering::Relaxed))
}

/// Re-read the cell geometry after a terminal resize. Pure ioctl — no stdin
/// involvement, safe mid-session (a `CSI 14 t` reply would land in the
/// crossterm EventStream instead). Keeps the old value when the ioctl
/// reports zeros, so a transient bad report cannot wipe good geometry.
pub fn refresh_geometry() {
    if let Some(geo) =
        winsize_via_ioctl().and_then(|(c, r, x, y)| cell_geometry_from_winsize(c, r, x, y))
    {
        GEOMETRY.store(pack_geometry(geo), Ordering::Relaxed);
    }
}

/// `TIOCGWINSZ` on stdout: `(cols, rows, xpixel, ypixel)`. The pixel fields
/// are commonly zero over ssh/serial — callers must treat them as optional.
fn winsize_via_ioctl() -> Option<(u16, u16, u16, u16)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: TIOCGWINSZ only fills the passed winsize struct.
    let ret = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if ret == -1 {
        return None;
    }
    Some((ws.ws_col, ws.ws_row, ws.ws_xpixel, ws.ws_ypixel))
}

/// Read probe replies from `fd` until the DA1 terminator reply arrived or
/// `deadline` passed, then drain any late bytes with zero-timeout polls so
/// they cannot leak into the crossterm EventStream as phantom keys (D1).
/// The fd is a parameter so tests drive it with a socketpair instead of a tty.
///
/// Known trade-off of the D1 drain: keystrokes typed before/during the
/// probe window are read into the same buffer as the replies and discarded
/// with them — type-ahead across the (bounded, <250 ms) probe is lost.
/// They cannot be re-injected: nothing can push bytes back into the tty
/// input queue without TIOCSTI (root-only on modern kernels), and the
/// crossterm EventStream that will own stdin does not exist yet.
fn read_probe_replies(fd: RawFd, deadline: Instant) -> Vec<u8> {
    let mut buf = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let timeout_ms = (remaining.as_millis().min(i32::MAX as u128) as i32).max(1);
        match poll_in(fd, timeout_ms) {
            PollResult::Ready => {
                if !read_chunk(fd, &mut buf) {
                    break; // EOF/error: nothing more will arrive
                }
                // DA1 is answered by essentially every terminal and is sent
                // last in the probe batch: seeing it means all replies are in.
                if parse_probe(&buf).da1_seen {
                    break;
                }
            }
            // poll() counts whole milliseconds and may wake a fraction
            // before the Instant deadline — loop back to the deadline check
            // instead of undershooting the budget.
            PollResult::Timeout => continue,
            PollResult::Interrupted => continue,
            PollResult::Error => break,
        }
    }
    // Final zero-timeout drain of late/partial reply bytes.
    while matches!(poll_in(fd, 0), PollResult::Ready) {
        if !read_chunk(fd, &mut buf) {
            break;
        }
    }
    buf
}

enum PollResult {
    Ready,
    Timeout,
    Interrupted,
    Error,
}

fn poll_in(fd: RawFd, timeout_ms: i32) -> PollResult {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: pfd outlives the call; nfds matches the single entry.
    let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    match ret {
        0 => PollResult::Timeout,
        r if r > 0 => PollResult::Ready,
        _ if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted => {
            PollResult::Interrupted
        }
        _ => PollResult::Error,
    }
}

/// Append one `read(2)` chunk to `buf`; false on EOF or error.
fn read_chunk(fd: RawFd, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 256];
    // SAFETY: chunk is a valid writable buffer of the passed length.
    let n = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
    if n <= 0 {
        return false;
    }
    buf.extend_from_slice(&chunk[..n as usize]);
    true
}

/// Write the probe batch to stdout and read the replies (bounded). The batch
/// is: a kitty graphics query (`q=0` — this one *wants* the reply; a 1x1
/// no-op transmission), optionally `CSI 14 t` when pixel geometry is still
/// missing, and DA1 last as the universal terminator.
fn send_probe_and_read(want_pixels: bool) -> Vec<u8> {
    use std::io::Write;
    let mut batch: Vec<u8> = b"\x1b_Gi=4242,s=1,v=1,a=q,t=d,f=24,q=0;AAAA\x1b\\".to_vec();
    if want_pixels {
        batch.extend_from_slice(b"\x1b[14t");
    }
    batch.extend_from_slice(b"\x1b[c");
    let mut out = std::io::stdout();
    if out.write_all(&batch).and_then(|_| out.flush()).is_err() {
        return Vec::new();
    }
    read_probe_replies(libc::STDIN_FILENO, Instant::now() + PROBE_BUDGET)
}

/// The full startup decision over injected seams (testable without a tty):
/// returns the protocol, the cell geometry (if any) and a reason string for
/// the log. `winsize` is the ioctl seam; `probe` writes the probe batch
/// (argument: also request `CSI 14 t` pixel size) and returns the reply
/// bytes. Explicit choices never probe unless they need missing geometry;
/// sixel without geometry degrades to half-block (D2: a sixel raster must
/// be pre-sized in pixels, while kitty scales into the cell rectangle).
fn init_from(
    choice: ImageProtocolChoice,
    env: &dyn Fn(&str) -> Option<String>,
    winsize: &mut dyn FnMut() -> Option<(u16, u16, u16, u16)>,
    probe: &mut dyn FnMut(bool) -> Vec<u8>,
) -> (GraphicsProtocol, Option<CellGeometry>, &'static str) {
    const NO_SIXEL_GEO: &str = "sixel needs pixel geometry, none available";
    let ws = winsize();
    let mut geometry = ws.and_then(|(c, r, x, y)| cell_geometry_from_winsize(c, r, x, y));
    // Combine a CSI-14t pixel reply with the ioctl cols/rows.
    let geo_from_probe = |outcome: &ProbeOutcome| {
        let (cols, rows) = ws.map(|(c, r, _, _)| (c, r))?;
        let (w, h) = outcome.pixel_size?;
        cell_geometry_from_winsize(cols, rows, w, h)
    };
    match choice {
        ImageProtocolChoice::HalfBlock => {
            (GraphicsProtocol::HalfBlock, geometry, "explicit config")
        }
        ImageProtocolChoice::Kitty | ImageProtocolChoice::Sixel => {
            if geometry.is_none() {
                // The only stdin read for an explicit choice: fetch the
                // pixel size when the ioctl reported zeros.
                geometry = geo_from_probe(&parse_probe(&probe(true)));
            }
            let proto = resolve(choice, None, None);
            if proto == GraphicsProtocol::Sixel && geometry.is_none() {
                return (GraphicsProtocol::HalfBlock, None, NO_SIXEL_GEO);
            }
            (proto, geometry, "explicit config")
        }
        ImageProtocolChoice::Auto => {
            if let Some(hit) = detect_from_env(env) {
                return (hit, geometry, "env");
            }
            let outcome = parse_probe(&probe(geometry.is_none()));
            if geometry.is_none() {
                geometry = geo_from_probe(&outcome);
            }
            let reason = if outcome.da1_seen {
                "probe replies"
            } else {
                "probe timeout"
            };
            let proto = resolve(ImageProtocolChoice::Auto, None, Some(&outcome));
            if proto == GraphicsProtocol::Sixel && geometry.is_none() {
                return (GraphicsProtocol::HalfBlock, None, NO_SIXEL_GEO);
            }
            (proto, geometry, reason)
        }
    }
}

/// Resolve the graphics protocol and cell geometry once at startup.
///
/// MUST be called after `enable_raw_mode()` and before the crossterm
/// EventStream exists: raw mode delivers the probe replies unbuffered and
/// un-echoed, and nothing else is reading stdin yet, so the reply bytes are
/// consumed here instead of surfacing as phantom key events.
pub fn init(choice: ImageProtocolChoice) {
    let (proto, geometry, reason) = init_from(
        choice,
        &|key| std::env::var(key).ok(),
        &mut winsize_via_ioctl,
        &mut send_probe_and_read,
    );
    let _ = PROTOCOL.set(proto);
    if let Some(geo) = geometry {
        GEOMETRY.store(pack_geometry(geo), Ordering::Relaxed);
    }
    log::debug!(
        "graphics: probe -> {} ({reason}), cell geometry {:?}",
        proto.name(),
        geometry
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ImageProtocolChoice;

    /// Build an env-lookup closure over a fixed set of (key, value) pairs.
    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    // --- env heuristics

    #[test]
    fn env_tmux_var_resolves_half_block() {
        // tmux without passthrough swallows graphics escapes — even when the
        // outer terminal is kitty, auto must stay on half-blocks.
        let env = env_of(&[
            ("TMUX", "/tmp/tmux-1000/default,42,0"),
            ("TERM", "xterm-256color"),
            ("KITTY_WINDOW_ID", "1"),
        ]);
        assert_eq!(detect_from_env(&env), Some(GraphicsProtocol::HalfBlock));
    }

    #[test]
    fn env_screen_and_tmux_term_resolve_half_block() {
        for term in ["screen-256color", "tmux-256color"] {
            // Checked before the kitty vars so tmux-on-kitty stays safe.
            let env = env_of(&[("TERM", term), ("KITTY_WINDOW_ID", "1")]);
            assert_eq!(
                detect_from_env(&env),
                Some(GraphicsProtocol::HalfBlock),
                "for TERM={term}"
            );
        }
    }

    #[test]
    fn env_kitty_window_id_resolves_kitty() {
        let env = env_of(&[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "7")]);
        assert_eq!(detect_from_env(&env), Some(GraphicsProtocol::Kitty));
    }

    #[test]
    fn env_term_xterm_kitty_resolves_kitty() {
        let env = env_of(&[("TERM", "xterm-kitty")]);
        assert_eq!(detect_from_env(&env), Some(GraphicsProtocol::Kitty));
    }

    #[test]
    fn env_term_program_wezterm_and_ghostty_resolve_kitty() {
        for prog in ["WezTerm", "ghostty"] {
            let env = env_of(&[("TERM", "xterm-256color"), ("TERM_PROGRAM", prog)]);
            assert_eq!(
                detect_from_env(&env),
                Some(GraphicsProtocol::Kitty),
                "for TERM_PROGRAM={prog}"
            );
        }
    }

    #[test]
    fn env_plain_xterm_is_inconclusive() {
        // None means "run the probe", not "half-block".
        let env = env_of(&[("TERM", "xterm-256color")]);
        assert_eq!(detect_from_env(&env), None);
        let env = env_of(&[]);
        assert_eq!(detect_from_env(&env), None);
    }

    // --- probe reply parsing

    #[test]
    fn kitty_ok_reply_detected() {
        let buf = b"\x1b_Gi=4242;OK\x1b\\\x1b[?62;4;22c";
        let outcome = parse_probe(buf);
        assert!(outcome.kitty_ok);
        assert!(outcome.da1_seen);
    }

    #[test]
    fn kitty_error_reply_is_not_ok() {
        let buf = b"\x1b_Gi=4242;ENOTSUPPORTED:unknown\x1b\\\x1b[?62;22c";
        let outcome = parse_probe(buf);
        assert!(!outcome.kitty_ok);
        assert!(outcome.da1_seen);
    }

    #[test]
    fn da1_with_attribute_4_is_sixel() {
        let outcome = parse_probe(b"\x1b[?62;4;22c");
        assert!(outcome.sixel);
        assert!(outcome.da1_seen);
    }

    #[test]
    fn da1_attribute_44_is_not_sixel() {
        // Attribute list is split on ';' and exact-matched — "44"/"40" must
        // not false-positive as sixel support.
        let outcome = parse_probe(b"\x1b[?62;40;44c");
        assert!(!outcome.sixel);
        assert!(outcome.da1_seen);
    }

    #[test]
    fn da1_without_4_is_not_sixel() {
        let outcome = parse_probe(b"\x1b[?62;22c");
        assert!(!outcome.sixel);
        assert!(outcome.da1_seen);
    }

    #[test]
    fn csi14t_reply_parses_pixel_size() {
        // Reply order is height;width — pixel_size is (width, height).
        let outcome = parse_probe(b"\x1b[4;480;800t");
        assert_eq!(outcome.pixel_size, Some((800, 480)));
        assert!(!outcome.da1_seen);
    }

    #[test]
    fn garbage_prefix_is_skipped() {
        let buf = b"junk\x07\x1b[Zmore\x1b_Gi=4242;OK\x1b\\\x1b[4;480;800t\x1b[?62;4c";
        let outcome = parse_probe(buf);
        assert!(outcome.kitty_ok);
        assert!(outcome.sixel);
        assert!(outcome.da1_seen);
        assert_eq!(outcome.pixel_size, Some((800, 480)));
    }

    #[test]
    fn empty_buffer_parses_to_nothing() {
        let outcome = parse_probe(b"");
        assert!(!outcome.kitty_ok && !outcome.sixel && !outcome.da1_seen);
        assert_eq!(outcome.pixel_size, None);
    }

    // --- resolution

    #[test]
    fn explicit_choice_wins_without_probe() {
        // Probe data (even a contradicting one) is irrelevant for explicit pins.
        let contradicting = ProbeOutcome {
            kitty_ok: true,
            sixel: true,
            pixel_size: None,
            da1_seen: true,
        };
        for (choice, want) in [
            (ImageProtocolChoice::Kitty, GraphicsProtocol::Kitty),
            (ImageProtocolChoice::Sixel, GraphicsProtocol::Sixel),
            (ImageProtocolChoice::HalfBlock, GraphicsProtocol::HalfBlock),
        ] {
            assert_eq!(resolve(choice, None, None), want);
            assert_eq!(
                resolve(
                    choice,
                    Some(GraphicsProtocol::HalfBlock),
                    Some(&contradicting)
                ),
                want
            );
        }
    }

    #[test]
    fn auto_uses_env_hit_first() {
        let probe = ProbeOutcome {
            kitty_ok: false,
            sixel: true,
            pixel_size: None,
            da1_seen: true,
        };
        assert_eq!(
            resolve(
                ImageProtocolChoice::Auto,
                Some(GraphicsProtocol::HalfBlock),
                Some(&probe)
            ),
            GraphicsProtocol::HalfBlock
        );
        assert_eq!(
            resolve(
                ImageProtocolChoice::Auto,
                Some(GraphicsProtocol::Kitty),
                None
            ),
            GraphicsProtocol::Kitty
        );
    }

    #[test]
    fn auto_probe_kitty_beats_sixel() {
        let probe = ProbeOutcome {
            kitty_ok: true,
            sixel: true,
            pixel_size: None,
            da1_seen: true,
        };
        assert_eq!(
            resolve(ImageProtocolChoice::Auto, None, Some(&probe)),
            GraphicsProtocol::Kitty
        );
    }

    #[test]
    fn auto_probe_sixel_without_kitty() {
        let probe = ProbeOutcome {
            kitty_ok: false,
            sixel: true,
            pixel_size: None,
            da1_seen: true,
        };
        assert_eq!(
            resolve(ImageProtocolChoice::Auto, None, Some(&probe)),
            GraphicsProtocol::Sixel
        );
    }

    #[test]
    fn auto_no_probe_data_is_half_block() {
        // Timeout path: nothing answered within the deadline.
        assert_eq!(
            resolve(ImageProtocolChoice::Auto, None, None),
            GraphicsProtocol::HalfBlock
        );
        let silent = ProbeOutcome::default();
        assert_eq!(
            resolve(ImageProtocolChoice::Auto, None, Some(&silent)),
            GraphicsProtocol::HalfBlock
        );
    }

    // --- geometry

    #[test]
    fn cell_geometry_from_winsize_divides() {
        let geo = cell_geometry_from_winsize(100, 50, 800, 1000).unwrap();
        assert_eq!(geo.cell_w, 8);
        assert_eq!(geo.cell_h, 20);
    }

    #[test]
    fn zero_winsize_pixels_yields_none() {
        // Common over ssh/serial: TIOCGWINSZ reports zero pixel fields.
        assert_eq!(cell_geometry_from_winsize(100, 50, 0, 0), None);
        assert_eq!(cell_geometry_from_winsize(100, 50, 800, 0), None);
        assert_eq!(cell_geometry_from_winsize(100, 50, 0, 1000), None);
        // Degenerate zero-size terminal: no division by zero.
        assert_eq!(cell_geometry_from_winsize(0, 0, 800, 1000), None);
        assert_eq!(cell_geometry_from_winsize(0, 50, 800, 1000), None);
        assert_eq!(cell_geometry_from_winsize(100, 0, 800, 1000), None);
        // More cols than pixels would truncate to a 0px cell — also None.
        assert_eq!(cell_geometry_from_winsize(1000, 50, 800, 1000), None);
    }

    #[test]
    fn pixel_box_from_cell_span() {
        let geo = CellGeometry {
            cell_w: 8,
            cell_h: 20,
        };
        assert_eq!(pixel_box(30, 15, geo), (240, 300));
        assert_eq!(pixel_box(0, 15, geo), (0, 300));
    }

    #[test]
    fn image_cell_span_from_raster() {
        // Ceil division back to cells: used for kitty c=/r= and erase regions.
        assert_eq!(cells_for(240, 8), 30);
        assert_eq!(cells_for(241, 8), 31);
        assert_eq!(cells_for(1, 8), 1);
        assert_eq!(cells_for(0, 8), 0);
        // Guard: a zero cell size never divides.
        assert_eq!(cells_for(240, 0), 0);
    }

    // --- I/O shell (step 3)

    #[test]
    fn probe_read_respects_deadline() {
        use std::io::Write;
        use std::os::fd::AsRawFd;
        use std::os::unix::net::UnixStream;
        use std::time::{Duration, Instant};

        // A reply on the fd returns early with the bytes.
        let (mut writer, reader) = UnixStream::pair().unwrap();
        writer.write_all(b"\x1b[?62;4c").unwrap();
        let start = Instant::now();
        let buf = read_probe_replies(
            reader.as_raw_fd(),
            Instant::now() + Duration::from_millis(250),
        );
        assert!(parse_probe(&buf).da1_seen, "reply bytes not read: {buf:?}");
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "did not return early"
        );

        // A silent peer returns empty at ~deadline, not before, never much after.
        let (_writer, reader) = UnixStream::pair().unwrap();
        let deadline = Duration::from_millis(120);
        let start = Instant::now();
        let buf = read_probe_replies(reader.as_raw_fd(), Instant::now() + deadline);
        let elapsed = start.elapsed();
        assert!(buf.is_empty());
        assert!(elapsed >= deadline, "returned before deadline: {elapsed:?}");
        assert!(
            elapsed < Duration::from_millis(400),
            "overslept: {elapsed:?}"
        );
    }

    #[test]
    fn init_with_explicit_choice_never_reads() {
        // Explicit config with usable ioctl geometry: no stdin read at all.
        let (proto, geo, _reason) = init_from(
            ImageProtocolChoice::Kitty,
            &|_| None,
            &mut || Some((100, 50, 800, 1000)),
            &mut |_| panic!("probe must not run for an explicit choice with geometry"),
        );
        assert_eq!(proto, GraphicsProtocol::Kitty);
        assert_eq!(
            geo,
            Some(CellGeometry {
                cell_w: 8,
                cell_h: 20
            })
        );
    }

    #[test]
    fn init_auto_env_hit_skips_probe() {
        let env = env_of(&[("TMUX", "/tmp/tmux-1000/default,42,0")]);
        let (proto, _geo, _reason) = init_from(
            ImageProtocolChoice::Auto,
            &env,
            &mut || Some((100, 50, 800, 1000)),
            &mut |_| panic!("probe must not run on an env hit"),
        );
        assert_eq!(proto, GraphicsProtocol::HalfBlock);
    }

    #[test]
    fn init_auto_probe_resolves_sixel_with_csi14t_geometry() {
        // ioctl has cols/rows but zero pixels (ssh case): the probe must ask
        // for CSI 14 t and its reply supplies the pixel geometry.
        let (proto, geo, _reason) = init_from(
            ImageProtocolChoice::Auto,
            &|_| None,
            &mut || Some((100, 50, 0, 0)),
            &mut |want_pixels| {
                assert!(want_pixels, "zero ioctl pixels must request CSI 14 t");
                b"\x1b[4;480;800t\x1b[?62;4c".to_vec()
            },
        );
        assert_eq!(proto, GraphicsProtocol::Sixel);
        assert_eq!(
            geo,
            Some(CellGeometry {
                cell_w: 8,
                cell_h: 9
            })
        );
    }

    #[test]
    fn init_sixel_without_geometry_degrades_to_half_block() {
        // Sixel rasters must be pre-sized to the pane; without any pixel
        // geometry the protocol cannot work and degrades to half-blocks.
        let (proto, geo, _reason) = init_from(
            ImageProtocolChoice::Sixel,
            &|_| None,
            &mut || None,
            &mut |_| Vec::new(), // probe answers nothing
        );
        assert_eq!(proto, GraphicsProtocol::HalfBlock);
        assert_eq!(geo, None);
        // Kitty scales into the cell rectangle, so it survives missing
        // geometry (an 8x16 cell is assumed later by the emitter).
        let (proto, geo, _reason) = init_from(
            ImageProtocolChoice::Kitty,
            &|_| None,
            &mut || None,
            &mut |_| Vec::new(),
        );
        assert_eq!(proto, GraphicsProtocol::Kitty);
        assert_eq!(geo, None);
    }

    #[test]
    fn init_auto_probe_timeout_is_half_block() {
        let (proto, _geo, reason) = init_from(
            ImageProtocolChoice::Auto,
            &|_| None,
            &mut || Some((100, 50, 800, 1000)),
            &mut |_| Vec::new(),
        );
        assert_eq!(proto, GraphicsProtocol::HalfBlock);
        assert!(reason.contains("timeout"), "reason was: {reason}");
    }

    #[test]
    fn geometry_packing_roundtrips() {
        let geo = CellGeometry {
            cell_w: 8,
            cell_h: 20,
        };
        assert_eq!(unpack_geometry(pack_geometry(geo)), Some(geo));
        // 0 is the "unset" sentinel.
        assert_eq!(unpack_geometry(0), None);
    }

    #[test]
    fn protocol_defaults_to_half_block_uninitialized() {
        // Unit tests (and any pre-init caller) must see the universal
        // fallback. NOTE: nothing in the test binary ever calls init(), so
        // the OnceCell stays empty for the whole test process.
        assert_eq!(protocol(), GraphicsProtocol::HalfBlock);
    }

    #[test]
    fn protocol_names_match_config_values() {
        assert_eq!(GraphicsProtocol::Kitty.name(), "kitty");
        assert_eq!(GraphicsProtocol::Sixel.name(), "sixel");
        assert_eq!(GraphicsProtocol::HalfBlock.name(), "half-block");
    }

    // --- kitty emitter (step 4)

    use image::{Rgb, RgbImage};
    use std::path::PathBuf;

    use super::emit_test_guard as emit_guard;

    fn test_key(name: &str, px_w: u32, px_h: u32) -> EmitKey {
        EmitKey {
            path: PathBuf::from(name),
            mtime_secs: 1,
            px_w,
            px_h,
            origin_cell: (10, 2),
        }
    }

    /// Split the emitted bytes into kitty APC sequences, returning
    /// `(control_keys, payload)` per `\x1b_G<keys>[;<payload>]\x1b\\`.
    /// Non-APC bytes (CSI moves, spaces) between sequences are skipped.
    fn apc_sequences(bytes: &[u8]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 3 <= bytes.len() {
            if &bytes[i..i + 3] != b"\x1b_G" {
                i += 1;
                continue;
            }
            let body_start = i + 3;
            let end = bytes[body_start..]
                .windows(2)
                .position(|w| w == b"\x1b\\")
                .expect("unterminated APC sequence")
                + body_start;
            let body = &bytes[body_start..end];
            let (keys, payload) = match body.iter().position(|&b| b == b';') {
                Some(p) => (&body[..p], &body[p + 1..]),
                None => (body, &b""[..]),
            };
            out.push((
                String::from_utf8_lossy(keys).into_owned(),
                String::from_utf8_lossy(payload).into_owned(),
            ));
            i = end + 2;
        }
        out
    }

    /// Value of `k=` in an APC control-key list, exact-matched per key.
    fn key_val(keys: &str, k: &str) -> Option<String> {
        keys.split(',')
            .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
            .map(str::to_string)
    }

    #[test]
    fn base64_known_vectors() {
        // RFC 4648 test vectors, padding included.
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn kitty_transmit_is_chunked_and_terminated() {
        let _g = emit_guard();
        // 64x64 RGB = 12288 raw bytes -> 16384 base64 chars -> 4 chunks.
        let rgb = RgbImage::from_pixel(64, 64, Rgb([1, 2, 3]));
        let mut sink = Vec::new();
        let emitted = emit_kitty(&mut sink, test_key("a.png", 512, 512), &rgb, 30, 15).unwrap();
        assert_eq!(emitted, Emitted::Transmitted);

        let apcs = apc_sequences(&sink);
        assert!(apcs.len() > 1, "base64 > 4096 chars must be chunked");
        let first = &apcs[0].0;
        for want in ["a=T", "f=24", "s=64", "v=64", "c=30", "r=15", "q=2", "m=1"] {
            assert!(
                first.split(',').any(|kv| kv == want),
                "first chunk missing {want}: {first}"
            );
        }
        assert!(key_val(first, "i").is_some(), "first chunk carries the id");
        for (i, (keys, payload)) in apcs.iter().enumerate() {
            assert!(payload.len() <= 4096, "chunk {i} payload over 4096");
            if i > 0 {
                let m = if i == apcs.len() - 1 { "m=0" } else { "m=1" };
                assert_eq!(keys, m, "continuation chunk {i}");
            }
        }
        // Reassembled payload is exactly the base64 of the raw RGB bytes.
        let joined: String = apcs.iter().map(|(_, p)| p.as_str()).collect();
        assert_eq!(joined, b64(rgb.as_raw()));
    }

    #[test]
    fn kitty_small_image_single_chunk_m0() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(2, 2, Rgb([9, 9, 9]));
        let mut sink = Vec::new();
        emit_kitty(&mut sink, test_key("b.png", 16, 32), &rgb, 2, 1).unwrap();
        let apcs = apc_sequences(&sink);
        assert_eq!(apcs.len(), 1, "tiny payload must be a single chunk");
        assert!(
            apcs[0].0.split(',').any(|kv| kv == "m=0"),
            "single chunk carries m=0: {}",
            apcs[0].0
        );
        assert_eq!(apcs[0].1, b64(rgb.as_raw()));
    }

    #[test]
    fn blank_cells_writes_spaces_per_row() {
        // 3 columns starting at col 5 (0-based), rows 2..4 -> two CUPs
        // (1-based CSI) each followed by three spaces.
        let mut sink = Vec::new();
        blank_cells(&mut sink, 5..8, 2..4).unwrap();
        let s = String::from_utf8_lossy(&sink);
        assert!(s.contains("\x1b[3;6H   "), "row 2 blank missing: {s:?}");
        assert!(s.contains("\x1b[4;6H   "), "row 3 blank missing: {s:?}");

        // Empty ranges write nothing at all.
        let mut sink = Vec::new();
        blank_cells(&mut sink, 5..5, 2..4).unwrap();
        assert!(sink.is_empty());
        let mut sink = Vec::new();
        blank_cells(&mut sink, 5..8, 4..4).unwrap();
        assert!(sink.is_empty());
    }

    #[test]
    fn emit_state_gates_identical_key() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(2, 2, Rgb([5, 5, 5]));
        let key = test_key("d.png", 16, 32);
        let mut sink = Vec::new();
        assert_eq!(
            emit_kitty(&mut sink, key.clone(), &rgb, 2, 1).unwrap(),
            Emitted::Transmitted
        );
        // Unrelated repaint: same key -> zero bytes, no retransmission.
        let mut sink = Vec::new();
        assert_eq!(
            emit_kitty(&mut sink, key, &rgb, 2, 1).unwrap(),
            Emitted::Gated
        );
        assert!(sink.is_empty(), "gated emit wrote bytes: {sink:?}");
    }

    #[test]
    fn emit_state_new_key_deletes_old_id_first() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(2, 2, Rgb([5, 5, 5]));
        let mut sink = Vec::new();
        emit_kitty(&mut sink, test_key("old.png", 16, 32), &rgb, 2, 1).unwrap();
        let (_, old_id) = live_snapshot_for_test().unwrap();
        let old_id = old_id.unwrap();

        let mut sink = Vec::new();
        emit_kitty(&mut sink, test_key("new.png", 16, 32), &rgb, 2, 1).unwrap();
        let apcs = apc_sequences(&sink);
        assert_eq!(
            apcs[0].0,
            format!("a=d,d=I,i={old_id},q=2"),
            "output must start with the delete of the previous id"
        );
        let new_id: u32 = key_val(&apcs[1].0, "i").unwrap().parse().unwrap();
        assert!(new_id > old_id, "fresh id per transmission");
    }

    #[test]
    fn end_frame_erases_unclaimed() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(2, 2, Rgb([5, 5, 5]));
        let mut sink = Vec::new();
        emit_kitty(&mut sink, test_key("e.png", 16, 32), &rgb, 2, 1).unwrap();
        let (_, id) = live_snapshot_for_test().unwrap();
        let id = id.unwrap();

        // A frame passes in which nothing claims the placement (selection
        // moved to a text file, overlay opened, split toggled, ...).
        begin_frame(true);
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        let s = String::from_utf8_lossy(&sink);
        // Capital I frees the pixel data too.
        assert!(
            s.contains(&format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")),
            "unclaimed placement not deleted: {s:?}"
        );
        // Delete-by-id ONLY: end_frame runs after the draw pass has fully
        // repainted the cells underneath — stamping spaces here would wipe
        // the freshly drawn content (and any overlay on top).
        assert!(
            !s.contains("\x1b["),
            "end_frame must not write cell content: {s:?}"
        );
        assert!(live_snapshot_for_test().is_none());

        // Idempotent: with no live image, end_frame writes nothing.
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        assert!(sink.is_empty());
    }

    #[test]
    fn end_frame_keeps_claimed() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(2, 2, Rgb([5, 5, 5]));
        let key = test_key("f.png", 16, 32);
        begin_frame(true);
        let mut sink = Vec::new();
        emit_kitty(&mut sink, key.clone(), &rgb, 2, 1).unwrap();
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        assert!(sink.is_empty(), "claimed placement must survive end_frame");
        assert!(live_snapshot_for_test().is_some());

        // A *gated* emit also claims: an unrelated repaint (same key) must
        // not let end_frame erase the still-valid placement. This is the
        // design's cost constraint (D5).
        begin_frame(true);
        let mut sink = Vec::new();
        assert_eq!(
            emit_kitty(&mut sink, key, &rgb, 2, 1).unwrap(),
            Emitted::Gated
        );
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        assert!(sink.is_empty(), "gated frame must keep the placement");
        assert!(live_snapshot_for_test().is_some());
    }

    #[test]
    fn begin_frame_disallowed_forces_no_claim() {
        let _g = emit_guard();
        begin_frame(false);
        assert!(!frame_allows_image());
        begin_frame(true);
        assert!(frame_allows_image());
    }

    // --- sixel emitter (step 5)

    #[test]
    fn emit_sixel_gates_like_kitty() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(4, 6, Rgb([255, 0, 0]));
        let key = test_key("s.png", 32, 16);

        // First emit transmits: cursor to the origin (10,2) -> CSI 3;11H,
        // then the DCS raster, and the live state records an id-less
        // placement (sixel has no delete command).
        let mut sink = Vec::new();
        assert_eq!(
            emit_sixel(&mut sink, key.clone(), &rgb, 2, 1).unwrap(),
            Emitted::Transmitted
        );
        let s = String::from_utf8_lossy(&sink);
        assert!(s.contains("\x1b[3;11H\x1bP0;1;0q"), "origin + DCS: {s:?}");
        assert!(s.ends_with("\x1b\\"), "unterminated DCS: {s:?}");
        let (live_key, id) = live_snapshot_for_test().expect("live after emit");
        assert_eq!(live_key, key);
        assert_eq!(id, None, "sixel placements are id-less");

        // Unrelated repaint: same key -> zero bytes.
        let mut sink = Vec::new();
        assert_eq!(
            emit_sixel(&mut sink, key.clone(), &rgb, 2, 1).unwrap(),
            Emitted::Gated
        );
        assert!(sink.is_empty(), "gated emit wrote bytes: {sink:?}");
    }

    #[test]
    fn emit_sixel_end_frame_reconciles_without_writes() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(4, 6, Rgb([255, 0, 0]));
        let key = test_key("t.png", 32, 16);
        let mut sink = Vec::new();
        emit_sixel(&mut sink, key.clone(), &rgb, 2, 1).unwrap();

        // A gated emit claims the frame: end_frame keeps the placement.
        begin_frame(true);
        let mut sink = Vec::new();
        assert_eq!(
            emit_sixel(&mut sink, key, &rgb, 2, 1).unwrap(),
            Emitted::Gated
        );
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        assert!(sink.is_empty(), "claimed sixel must survive end_frame");
        assert!(live_snapshot_for_test().is_some());

        // Unclaimed frame (selection moved away): the live state is
        // forgotten but NOTHING is written — sixel pixels are ordinary cell
        // content and the draw pass that just ran has already repainted the
        // whole region; stamping spaces here would wipe that fresh content
        // (blanked text previews, overlay hole-punch).
        begin_frame(true);
        let mut sink = Vec::new();
        end_frame(&mut sink).unwrap();
        assert!(
            sink.is_empty(),
            "sixel reconcile must not write: {:?}",
            String::from_utf8_lossy(&sink)
        );
        assert!(live_snapshot_for_test().is_none());
    }

    #[test]
    fn emit_sixel_new_key_skips_predecessor_blank() {
        let _g = emit_guard();
        let rgb = RgbImage::from_pixel(4, 6, Rgb([255, 0, 0]));
        let mut sink = Vec::new();
        emit_sixel(&mut sink, test_key("old.png", 32, 16), &rgb, 2, 1).unwrap();

        // New selection at a different origin: the old placement's region
        // must NOT be touched mid-frame — those cells belong to whichever
        // draw owns them in this frame's full repaint (blanking them could
        // wipe panes drawn earlier, e.g. after a resize). Only the new
        // target region is blanked, before the raster.
        let mut key = test_key("new.png", 32, 16);
        key.origin_cell = (0, 0);
        let mut sink = Vec::new();
        emit_sixel(&mut sink, key, &rgb, 2, 1).unwrap();
        let s = String::from_utf8_lossy(&sink);
        assert!(
            !s.contains("\x1b[3;11H"),
            "old region must not be blanked: {s:?}"
        );
        let target_blank = s.find("\x1b[1;1H").expect("target region blanked");
        let dcs = s.find("\x1bP").expect("new raster transmitted");
        assert!(target_blank < dcs, "blank must precede the raster: {s:?}");
    }

    #[test]
    fn placement_cells_clip_to_pane() {
        let geo = CellGeometry {
            cell_w: 8,
            cell_h: 16,
        };
        // Exact fit.
        assert_eq!(placement_cells(240, 320, geo, 40, 25), (30, 20));
        // One pixel over a cell boundary rounds up to the next cell...
        assert_eq!(placement_cells(241, 321, geo, 40, 25), (31, 21));
        // ...but never past the pane span (exact clipping, D6).
        assert_eq!(placement_cells(1000, 1000, geo, 40, 25), (40, 25));
        assert_eq!(placement_cells(0, 0, geo, 40, 25), (0, 0));
    }
}
