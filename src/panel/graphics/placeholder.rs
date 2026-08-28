//! U+10EEEE placeholder-cell renderer for kitty virtual placements: a pure
//! function from a cell grid + image id to the ordinary text the terminal
//! composites the transmitted image onto — no I/O beyond the passed sink and
//! no state, so the exact byte stream is frozen in tests.
//!
//! Each cell is U+10EEEE with the grid position encoded as combining
//! diacritics (row, column, id MSB — in that order) and the image id's low
//! 24 bits in the truecolor foreground. Trailing cells of a row carry no
//! diacritics: the spec's left-neighbor inheritance fills them in, keeping
//! the emitted text short. Placeholder cells must carry NO attributes
//! beyond the foreground color (the rest are reserved), and a row must
//! never wrap — the caller gets one absolute cursor move per row.

use std::io::{self, Write};

/// The placeholder character kitty composites virtual placements onto.
const PLACEHOLDER: char = '\u{10EEEE}';

/// The largest cell box a placeholder grid can address — the diacritics
/// table size (it also sizes [`DIACRITICS`], so the two cannot drift).
/// The emitter clamps its whole cell box to this BEFORE transmitting, so
/// the `c=`/`r=` fit box and the grid agree by construction; the clamp
/// inside [`placeholder_grid`] is a defensive second layer.
pub(super) const GRID_MAX: u16 = 297;

/// Row/column diacritics, index = grid position. Transcribed verbatim from
/// the canonical `gen/rowcolumn-diacritics.txt` in the kitty repo (one
/// combining char of class 230 per entry, sorted by codepoint); the tests
/// spot-check known entries and enforce the strict ascent.
#[rustfmt::skip]
pub(super) static DIACRITICS: [char; GRID_MAX as usize] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065A}', '\u{065B}', '\u{065D}', '\u{065E}', '\u{06D6}', '\u{06D7}', '\u{06D8}',
    '\u{06D9}', '\u{06DA}', '\u{06DB}', '\u{06DC}', '\u{06DF}', '\u{06E0}', '\u{06E1}', '\u{06E2}',
    '\u{06E4}', '\u{06E7}', '\u{06E8}', '\u{06EB}', '\u{06EC}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073A}', '\u{073D}', '\u{073F}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074A}', '\u{07EB}', '\u{07EC}', '\u{07ED}', '\u{07EE}',
    '\u{07EF}', '\u{07F0}', '\u{07F1}', '\u{07F3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081B}', '\u{081C}', '\u{081D}', '\u{081E}', '\u{081F}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082A}', '\u{082B}', '\u{082C}',
    '\u{082D}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0F82}', '\u{0F83}', '\u{0F86}', '\u{0F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CE0}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}',
    '\u{1DC5}', '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}',
    '\u{1DD2}', '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}',
    '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}',
    '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}',
    '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}',
    '\u{2DE3}', '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}',
    '\u{2DEB}', '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}',
    '\u{2DF3}', '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A67C}', '\u{A67D}',
    '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}',
    '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}',
    '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}', '\u{AAB0}', '\u{AAB2}', '\u{AAB3}', '\u{AAB7}',
    '\u{AAB8}', '\u{AABE}', '\u{AABF}', '\u{AAC1}', '\u{FE20}', '\u{FE21}', '\u{FE22}', '\u{FE23}',
    '\u{FE24}', '\u{FE25}', '\u{FE26}', '\u{10A0F}', '\u{10A38}', '\u{1D185}', '\u{1D186}', '\u{1D187}',
    '\u{1D188}', '\u{1D189}', '\u{1D1AA}', '\u{1D1AB}', '\u{1D1AC}', '\u{1D1AD}', '\u{1D242}', '\u{1D243}',
    '\u{1D244}',
];

/// Write the placeholder grid for one virtual placement: `cols`×`rows`
/// cells at `origin` (0-based screen cell), carrying `id`. Per row: an
/// absolute cursor move (a wrapped row would smear placeholder cells over
/// the neighbouring pane), one fg-SGR with the id's low 24 bits, the first
/// cell with row+column+id-MSB diacritics (the MSB explicitly, so any u32
/// id round-trips), the remaining cells bare (left-neighbor inheritance),
/// and the fg reset — no other attributes, they are reserved.
///
/// Rows/columns past the diacritics table are clamped: cells beyond index
/// 296 are unaddressable and simply not emitted.
pub(super) fn placeholder_grid(
    w: &mut impl Write,
    origin: (u16, u16),
    cols: u16,
    rows: u16,
    id: u32,
) -> io::Result<()> {
    let cols = (cols as usize).min(DIACRITICS.len());
    let rows = (rows as usize).min(DIACRITICS.len());
    if cols == 0 || rows == 0 {
        return Ok(());
    }
    let (r, g, b) = ((id >> 16) as u8, (id >> 8) as u8, id as u8);
    let id_msb = DIACRITICS[(id >> 24) as usize];
    for (row, &row_mark) in DIACRITICS.iter().enumerate().take(rows) {
        super::move_to(w, (origin.0, origin.1.saturating_add(row as u16)))?;
        write!(w, "\x1b[38;2;{r};{g};{b}m")?;
        let mut cells = String::with_capacity(cols * 4 + 12);
        cells.push(PLACEHOLDER);
        cells.push(row_mark);
        cells.push(DIACRITICS[0]);
        cells.push(id_msb);
        for _ in 1..cols {
            cells.push(PLACEHOLDER);
        }
        w.write_all(cells.as_bytes())?;
        write!(w, "\x1b[39m")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diacritics_table_spot_checks() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(DIACRITICS[1], '\u{030D}');
        assert_eq!(DIACRITICS[2], '\u{030E}');
        assert_eq!(DIACRITICS[11], '\u{034C}');
        assert_eq!(DIACRITICS[296], '\u{1D244}');
    }

    #[test]
    fn diacritics_table_strictly_ascending() {
        // The canonical list is sorted by codepoint: a garbled, duplicated
        // or dropped-and-refilled transcription shows up as an ordering
        // violation somewhere in the window scan.
        assert!(
            DIACRITICS.windows(2).all(|w| w[0] < w[1]),
            "table must be strictly ascending"
        );
    }

    #[test]
    fn placeholder_grid_emits_rows() {
        // 3x2 grid at origin (10,2), id 0x4d46: per row an absolute CUP
        // (1-based CSI), the fg-SGR carrying the id's low 24 bits
        // (R 0, G 0x4d=77, B 0x46=70), the first cell with row+col0+MSB
        // diacritics (row 0/1 = U+0305/U+030D, col 0 = U+0305, MSB 0 =
        // U+0305), two bare inheritance cells, and the fg reset.
        let mut sink = Vec::new();
        placeholder_grid(&mut sink, (10, 2), 3, 2, 0x4d46).unwrap();
        let want = concat!(
            "\x1b[3;11H\x1b[38;2;0;77;70m",
            "\u{10EEEE}\u{0305}\u{0305}\u{0305}\u{10EEEE}\u{10EEEE}\x1b[39m",
            "\x1b[4;11H\x1b[38;2;0;77;70m",
            "\u{10EEEE}\u{030D}\u{0305}\u{0305}\u{10EEEE}\u{10EEEE}\x1b[39m",
        );
        assert_eq!(String::from_utf8(sink).unwrap(), want);
    }

    #[test]
    fn placeholder_grid_clamps_at_table_size() {
        // Wider than the table: only 297 columns are addressable — the row
        // is capped there instead of indexing out of range.
        let mut sink = Vec::new();
        placeholder_grid(&mut sink, (0, 0), 400, 1, 1).unwrap();
        let s = String::from_utf8(sink).unwrap();
        assert_eq!(s.matches('\u{10EEEE}').count(), 297);
        // Taller than the table: at most 297 rows are emitted.
        let mut sink = Vec::new();
        placeholder_grid(&mut sink, (0, 0), 1, 400, 1).unwrap();
        let s = String::from_utf8(sink).unwrap();
        assert_eq!(s.matches("\x1b[39m").count(), 297);
    }

    #[test]
    fn placeholder_grid_empty_grid_writes_nothing() {
        for (cols, rows) in [(0, 2), (3, 0), (0, 0)] {
            let mut sink = Vec::new();
            placeholder_grid(&mut sink, (10, 2), cols, rows, 1).unwrap();
            assert!(sink.is_empty(), "{cols}x{rows} grid wrote: {sink:?}");
        }
    }
}
