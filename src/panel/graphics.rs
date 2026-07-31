//! Terminal graphics protocol support for image previews.
//!
//! This module owns which protocol (kitty graphics / sixel / half-block
//! fallback) the preview column may use, and the cell→pixel geometry needed
//! to place real pixels into a cell layout. The detection core is pure —
//! env lookups and probe replies are injected — so the whole decision matrix
//! is unit-testable without a terminal. The emitters (kitty/sixel) land in
//! later steps; until then the resolved protocol is `HalfBlock` everywhere
//! unless detection says otherwise.

use crate::config::ImageProtocolChoice;

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
pub fn cell_geometry_from_winsize(cols: u16, rows: u16, xpx: u16, ypx: u16) -> Option<CellGeometry> {
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
#[allow(dead_code)]
pub fn pixel_box(cols: u16, rows: u16, geo: CellGeometry) -> (u32, u32) {
    (
        cols as u32 * geo.cell_w as u32,
        rows as u32 * geo.cell_h as u32,
    )
}

/// Cells needed to cover `px` pixels (ceil division) — used for the kitty
/// `c=`/`r=` placement and the erase region (steps 4–6). A zero cell size
/// yields zero cells instead of dividing.
#[allow(dead_code)]
pub fn cells_for(px: u32, cell_px: u16) -> u16 {
    if cell_px == 0 {
        return 0;
    }
    px.div_ceil(cell_px as u32).min(u16::MAX as u32) as u16
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
        move |key: &str| {
            owned
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        }
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
                resolve(choice, Some(GraphicsProtocol::HalfBlock), Some(&contradicting)),
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
            resolve(ImageProtocolChoice::Auto, Some(GraphicsProtocol::Kitty), None),
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

    #[test]
    fn protocol_names_match_config_values() {
        assert_eq!(GraphicsProtocol::Kitty.name(), "kitty");
        assert_eq!(GraphicsProtocol::Sixel.name(), "sixel");
        assert_eq!(GraphicsProtocol::HalfBlock.name(), "half-block");
    }
}
