//! Hand-rolled sixel encoder (D7): a pure function from an RGB raster to the
//! DCS byte stream, no I/O and no state — deterministic by construction so
//! the golden-output test can freeze exact bytes.
//!
//! Palette: a fixed 6×7×6 color cube (252 registers, under the 256 limit),
//! `idx = r6*42 + g7*6 + b6` with `r6 = r*6/256` etc. Registers are defined
//! only for the indices actually used, in ascending order.
//!
//! The raster height is truncated down to a multiple of 6 (a sixel band is
//! six pixel rows): callers pre-fit the raster to the pane pixel box, and the
//! truncation guarantees the final band cannot bleed past it — a sixel
//! raster is placed by the terminal as-is, there is no `c=`/`r=` clipping
//! like kitty's.

use std::io::Write;

use image::{Rgb, RgbImage};

/// Number of palette registers in the fixed 6×7×6 cube.
const PALETTE_SIZE: usize = 252;

/// Map an RGB pixel onto the fixed 6×7×6 cube: `r6*42 + g7*6 + b6`.
pub(super) fn quantize(px: &Rgb<u8>) -> u8 {
    let r6 = px.0[0] as u32 * 6 / 256;
    let g7 = px.0[1] as u32 * 7 / 256;
    let b6 = px.0[2] as u32 * 6 / 256;
    (r6 * 42 + g7 * 6 + b6) as u8
}

/// The register color for a palette index, on the 0–100 scale sixel color
/// definitions use (`#i;2;R;G;B`).
pub(super) fn palette_rgb(idx: u8) -> (u8, u8, u8) {
    let r6 = (idx as u32) / 42;
    let g7 = (idx as u32 % 42) / 6;
    let b6 = idx as u32 % 6;
    // Levels span 0..=5 (r, b) and 0..=6 (g), stretched onto 0..=100.
    (
        (r6 * 100 / 5) as u8,
        (g7 * 100 / 6) as u8,
        (b6 * 100 / 5) as u8,
    )
}

/// Encode `rgb` as a complete sixel sequence:
/// `ESC P 0;1;0 q` (P2=1: zero bits leave the background untouched), raster
/// attributes `"1;1;<w>;<h>`, used palette registers ascending, then bands
/// of 6 rows — per color `#idx` + column sixels with `!<n>` RLE for runs
/// over 3, `$` between colors, `-` after each band — and the `ESC \` ST.
pub fn sixel_encode(rgb: &RgbImage) -> Vec<u8> {
    let w = rgb.width();
    // Truncate to whole bands so the raster cannot bleed below the pane.
    let h = rgb.height() - rgb.height() % 6;
    let mut out = Vec::new();
    let _ = write!(out, "\x1bP0;1;0q\"1;1;{w};{h}");

    // One quantization pass: per-pixel index buffer + the used-register set.
    let mut idx = vec![0u8; w as usize * h as usize];
    let mut used = [false; PALETTE_SIZE];
    for y in 0..h {
        for x in 0..w {
            let q = quantize(rgb.get_pixel(x, y));
            idx[(y * w + x) as usize] = q;
            used[q as usize] = true;
        }
    }
    for (i, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        let (r, g, b) = palette_rgb(i as u8);
        let _ = write!(out, "#{i};2;{r};{g};{b}");
    }

    for band in 0..h / 6 {
        let y0 = band * 6;
        let mut band_used = [false; PALETTE_SIZE];
        for y in y0..y0 + 6 {
            for x in 0..w {
                band_used[idx[(y * w + x) as usize] as usize] = true;
            }
        }
        let mut first = true;
        for (color, _) in band_used.iter().enumerate().filter(|(_, u)| **u) {
            if !first {
                out.push(b'$'); // carriage return: next color, same band
            }
            first = false;
            let _ = write!(out, "#{color}");
            // Column bit patterns for this color (bit 0 = top row).
            let mut cols = Vec::with_capacity(w as usize);
            for x in 0..w {
                let mut bits = 0u8;
                for dy in 0..6 {
                    if idx[((y0 + dy) * w + x) as usize] as usize == color {
                        bits |= 1 << dy;
                    }
                }
                cols.push(bits);
            }
            // Trailing empty columns paint nothing — drop them.
            let last = cols.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
            emit_rle(&mut out, &cols[..last]);
        }
        out.push(b'-'); // next band
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Emit column sixel chars (`0x3F + bits`), run-length encoding runs longer
/// than 3 as `!<n><ch>` and shorter ones literally (the `!` prefix costs 2+
/// bytes, so short runs are cheaper verbatim).
fn emit_rle(out: &mut Vec<u8>, cols: &[u8]) {
    let mut i = 0;
    while i < cols.len() {
        let ch = 0x3f + cols[i];
        let mut n = 1;
        while i + n < cols.len() && cols[i + n] == cols[i] {
            n += 1;
        }
        if n > 3 {
            let _ = write!(out, "!{n}");
            out.push(ch);
        } else {
            out.resize(out.len() + n, ch);
        }
        i += n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sixel_quantize_maps_extremes() {
        assert_eq!(quantize(&Rgb([0, 0, 0])), 0);
        assert_eq!(quantize(&Rgb([255, 255, 255])), 251);
        let red = quantize(&Rgb([255, 0, 0]));
        let green = quantize(&Rgb([0, 255, 0]));
        let blue = quantize(&Rgb([0, 0, 255]));
        assert_eq!(red, 210); // r6=5 -> 5*42
        assert_eq!(green, 36); // g7=6 -> 6*6
        assert_eq!(blue, 5); // b6=5
        assert!(red != green && green != blue && red != blue);
    }

    #[test]
    fn sixel_header_and_raster_attributes() {
        let rgb = RgbImage::from_pixel(4, 6, Rgb([0, 0, 0]));
        let out = sixel_encode(&rgb);
        assert!(
            out.starts_with(b"\x1bP0;1;0q\"1;1;4;6"),
            "header/raster attributes wrong: {:?}",
            String::from_utf8_lossy(&out)
        );
        assert!(out.ends_with(b"\x1b\\"), "missing ST terminator");
    }

    #[test]
    fn sixel_encodes_known_image_deterministically() {
        // 4x6, left half pure red, right half white — the golden output,
        // computed by hand once and frozen:
        //   header `ESC P 0;1;0 q` + attrs `"1;1;4;6`
        //   palette: red = idx 210 (100;0;0), white = idx 251 (100;100;100)
        //   one band: red cols 0-1 full (`~~`), `$`, white skips 2 (`??`)
        //   then cols 2-3 full (`~~`), band end `-`, ST.
        let mut rgb = RgbImage::new(4, 6);
        for y in 0..6 {
            for x in 0..4 {
                let px = if x < 2 { [255, 0, 0] } else { [255, 255, 255] };
                rgb.put_pixel(x, y, Rgb(px));
            }
        }
        let out = sixel_encode(&rgb);
        assert_eq!(
            String::from_utf8_lossy(&out),
            "\x1bP0;1;0q\"1;1;4;6#210;2;100;0;0#251;2;100;100;100#210~~$#251??~~-\x1b\\"
        );
    }

    #[test]
    fn sixel_rle_runs_and_literals() {
        // A run of 10 identical full columns compresses; a run of 3 stays
        // literal (the `!n` prefix would be longer).
        let long = sixel_encode(&RgbImage::from_pixel(10, 6, Rgb([0, 0, 0])));
        let s = String::from_utf8_lossy(&long);
        assert!(s.contains("#0!10~"), "run of 10 must be RLE: {s}");
        let short = sixel_encode(&RgbImage::from_pixel(3, 6, Rgb([0, 0, 0])));
        let s = String::from_utf8_lossy(&short);
        assert!(s.contains("#0~~~"), "run of 3 must be literal: {s}");
        assert!(!s.contains('!'), "no RLE marker for short runs: {s}");
    }

    #[test]
    fn sixel_palette_registers_only_used_colors() {
        let mut rgb = RgbImage::from_pixel(4, 6, Rgb([0, 0, 0]));
        rgb.put_pixel(0, 0, Rgb([255, 255, 255]));
        let out = sixel_encode(&rgb);
        let s = String::from_utf8_lossy(&out);
        // Exactly two `#i;2;R;G;B` register definitions, ascending.
        let defs: Vec<usize> = s.match_indices(";2;").map(|(i, _)| i).collect();
        assert_eq!(defs.len(), 2, "exactly the two used registers: {s}");
        assert!(s.contains("#0;2;0;0;0"));
        assert!(s.contains("#251;2;100;100;100"));
        let p0 = s.find("#0;2;").unwrap();
        let p251 = s.find("#251;2;").unwrap();
        assert!(p0 < p251, "palette must be ascending");
    }

    #[test]
    fn sixel_height_clamped_to_band_multiple() {
        // Callers pre-clamp; the encoder truncates defensively so the final
        // band can never bleed into the row below the pane.
        let rgb = RgbImage::from_pixel(4, 8, Rgb([0, 0, 0]));
        let out = sixel_encode(&rgb);
        let s = String::from_utf8_lossy(&out);
        assert!(
            s.starts_with("\x1bP0;1;0q\"1;1;4;6"),
            "height not clamped: {s}"
        );
        assert_eq!(s.matches('-').count(), 1, "exactly one band: {s}");
        // Degenerate: fewer than 6 rows encodes an empty raster, no bands.
        let tiny = sixel_encode(&RgbImage::from_pixel(4, 5, Rgb([0, 0, 0])));
        let s = String::from_utf8_lossy(&tiny);
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;4;0"), "not truncated: {s}");
        assert_eq!(s.matches('-').count(), 0);
    }
}
