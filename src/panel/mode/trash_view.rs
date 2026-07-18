//! The trash overlay (`gT`): a read-only list of freedesktop trash items.
//! `r` restores the item under the cursor to its original location.
//!
//! Pure adapter — the manager supplies the item list (from
//! `trash::os_limited::list`) at construction and applies the restore
//! centrally. The adapter itself never touches the filesystem.

use crossterm::event::{KeyCode, KeyEvent};
use crossterm::style::Color;
use time::OffsetDateTime;

use super::{Cleanup, ModalInput, ModalRegion, ModeOp};
use crate::config::color::{color_main, print_horizontal_bar, print_horz_bot, print_horz_top};
use crate::engine::StyleEngine;
use crate::panel::*;
use crate::util::ExactWidth;

pub struct TrashView {
    items: Vec<trash::TrashItem>,
    cursor: usize,
}

impl TrashView {
    /// Sorts the items newest-first (most recent deletion on top).
    pub fn new(mut items: Vec<trash::TrashItem>) -> Self {
        items.sort_by(|a, b| b.time_deleted.cmp(&a.time_deleted));
        Self { items, cursor: 0 }
    }
}

/// `YYYY-MM-DD HH:MM` (16 columns) for a unix timestamp.
fn format_date(unix: i64) -> String {
    OffsetDateTime::from_unix_timestamp(unix)
        .map(|t| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
            )
        })
        .unwrap_or_else(|_| "????-??-?? ??:??".to_string())
}

/// Truncate to `w` columns keeping the tail (the deepest path segments),
/// prefixing `…` when shortened.
fn fit_left(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= w {
        return s.to_string();
    }
    let tail: String = s.chars().skip(count - (w - 1)).collect();
    format!("…{tail}")
}

impl Draw for TrashView {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let x0 = x_range.start;
        let x_end = x_range.end;
        let width = x_end.saturating_sub(x0);
        let avail_h = y_range.end.saturating_sub(y_range.start) as usize;

        // Column geometry within a small horizontal padding.
        const PAD: u16 = 2;
        const DATE_W: usize = 16;
        let content_x = x0 + PAD;
        let content_w = width.saturating_sub(PAD * 2) as usize;
        let after_date = content_w.saturating_sub(DATE_W + 2);
        let name_w = (after_date * 2 / 5).max(8);
        let path_w = after_date.saturating_sub(name_w + 2);

        // Vertically centered band: hint + top bar + list + bottom bar.
        let empty = self.items.is_empty();
        let cap = avail_h.saturating_sub(6).max(1);
        let visible = if empty {
            1
        } else {
            self.items.len().min(cap).max(1)
        };
        let band = visible + 3;
        let band_top = y_range.start + (avail_h.saturating_sub(band) as u16) / 2;
        let hint_y = band_top;
        let top_bar_y = band_top + 1;
        let list_y0 = band_top + 2;
        let bottom_bar_y = list_y0 + visible as u16;

        // Divider x-positions (match the miller columns / cd overlay).
        let div_center = x0 + width / 8;
        let div_right = x0 + width / 2;

        // 1. Keybinding hint on its own row, above the frame.
        let hint = "Trash — j/k: move · r: restore · Esc: close";
        let hint_x = x0 + width.saturating_sub(hint.chars().count() as u16) / 2;
        queue!(
            stdout,
            cursor::Hide,
            cursor::MoveTo(x0, hint_y),
            Clear(ClearType::CurrentLine),
            cursor::MoveTo(hint_x, hint_y),
            PrintStyledContent(hint.bold().with(color_main())),
        )?;

        // 2. Top and bottom bars with junctions at the dividers.
        for x in x0..x_end {
            let junction = x == x0 || x == div_center || x == div_right;
            let top = if junction {
                print_horz_top()
            } else {
                print_horizontal_bar()
            };
            let bot = if junction {
                print_horz_bot()
            } else {
                print_horizontal_bar()
            };
            queue!(
                stdout,
                cursor::MoveTo(x, top_bar_y),
                top,
                cursor::MoveTo(x, bottom_bar_y),
                bot,
            )?;
        }

        // 3. Empty trash.
        if empty {
            let msg = "(the trash is empty)";
            let mx = x0 + width.saturating_sub(msg.chars().count() as u16) / 2;
            queue!(
                stdout,
                cursor::MoveTo(mx, list_y0),
                PrintStyledContent(msg.dark_grey()),
            )?;
            return Ok(());
        }

        // 4. List rows, windowed around the cursor: date | symbol name | path.
        let offset = self
            .cursor
            .saturating_sub(visible - 1)
            .min(self.items.len().saturating_sub(visible));
        let date_x = content_x;
        let sym_x = content_x + (DATE_W + 2) as u16;
        let name_x = sym_x + 2;
        let path_x = sym_x + name_w as u16 + 2;

        for (row, (idx, item)) in self
            .items
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .enumerate()
        {
            let y = list_y0 + row as u16;
            let date = format_date(item.time_deleted);
            let original = item.original_parent.join(&item.name);
            let style = StyleEngine::get_style(&original);
            let name = item
                .name
                .to_string_lossy()
                .exact_width(name_w.saturating_sub(2));
            let path = fit_left(&item.original_parent.display().to_string(), path_w);

            if idx == self.cursor {
                // Highlight bar across the content, then overprint the columns.
                queue!(
                    stdout,
                    cursor::MoveTo(x0, y),
                    Clear(ClearType::CurrentLine),
                    cursor::MoveTo(content_x, y),
                    PrintStyledContent(" ".repeat(content_w).with(color_main()).reverse()),
                    cursor::MoveTo(date_x, y),
                    PrintStyledContent(date.with(color_main()).reverse()),
                    cursor::MoveTo(sym_x, y),
                    PrintStyledContent(style.symbol.with(color_main()).reverse()),
                    cursor::MoveTo(name_x, y),
                    PrintStyledContent(name.with(color_main()).reverse()),
                    cursor::MoveTo(path_x, y),
                    PrintStyledContent(path.with(color_main()).reverse()),
                )?;
            } else {
                let sym_color = style.color.unwrap_or(Color::Grey);
                queue!(
                    stdout,
                    cursor::MoveTo(x0, y),
                    Clear(ClearType::CurrentLine),
                    cursor::MoveTo(date_x, y),
                    PrintStyledContent(date.dark_grey()),
                    cursor::MoveTo(sym_x, y),
                    PrintStyledContent(style.symbol.with(sym_color)),
                    cursor::MoveTo(name_x, y),
                    PrintStyledContent(name.grey()),
                    cursor::MoveTo(path_x, y),
                    PrintStyledContent(path.dark_grey()),
                )?;
            }
        }
        Ok(())
    }
}

impl ModalInput for TrashView {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        match key_event.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.cursor + 1 < self.items.len() {
                    self.cursor += 1;
                }
                ModeOp::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                ModeOp::None
            }
            KeyCode::Char('r') => match self.items.get(self.cursor) {
                Some(item) => ModeOp::RestoreFromTrash {
                    items: vec![item.clone()],
                },
                None => ModeOp::None,
            },
            KeyCode::Esc | KeyCode::Enter => ModeOp::Exit {
                cleanup: Cleanup::None,
            },
            _ => ModeOp::None,
        }
    }

    fn region(&self) -> ModalRegion {
        ModalRegion::ConsoleOverlay
    }

    fn name(&self) -> &'static str {
        "trash"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::Path;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Trash `n` temp files under an isolated home trash and return their items.
    /// Holds the shared trash-test lock across the env-mutating critical section
    /// (set `XDG_DATA_HOME` → trash → list) so parallel trash tests don't race.
    fn trashed_items(n: usize) -> Vec<trash::TrashItem> {
        let _guard = crate::undo::TRASH_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_DATA_HOME", home.path());
        let work = tempfile::tempdir().unwrap();
        for i in 0..n {
            let f = work.path().join(format!("file{i}.txt"));
            std::fs::write(&f, "x").unwrap();
            trash::delete(&f).unwrap();
        }
        let work_path = work.path().to_path_buf();
        // Leak the tempdirs so the trashed items keep resolving during the test.
        std::mem::forget(home);
        std::mem::forget(work);
        let mut items: Vec<_> = trash::os_limited::list()
            .unwrap()
            .into_iter()
            .filter(|i| i.original_parent == Path::new(&work_path))
            .collect();
        items.sort_by_key(|i| i.time_deleted);
        items
    }

    #[test]
    fn j_and_k_move_and_clamp_the_cursor() {
        let mut view = TrashView::new(trashed_items(3));
        assert_eq!(view.cursor, 0);
        view.handle_key(key(KeyCode::Char('k'))); // clamp at top
        assert_eq!(view.cursor, 0);
        view.handle_key(key(KeyCode::Char('j')));
        view.handle_key(key(KeyCode::Char('j')));
        assert_eq!(view.cursor, 2);
        view.handle_key(key(KeyCode::Char('j'))); // clamp at bottom
        assert_eq!(view.cursor, 2);
        view.handle_key(key(KeyCode::Char('k')));
        assert_eq!(view.cursor, 1);
    }

    #[test]
    fn r_emits_restore_for_the_current_item() {
        // Single item avoids depending on the (1-second-resolution) sort order.
        let items = trashed_items(1);
        let expected = items[0].clone();
        let mut view = TrashView::new(items);
        assert_eq!(
            view.handle_key(key(KeyCode::Char('r'))),
            ModeOp::RestoreFromTrash {
                items: vec![expected]
            }
        );
    }

    #[test]
    fn new_sorts_items_newest_first() {
        // time_deleted may collide at 1-second resolution, so assert the
        // ordering is non-increasing rather than a specific permutation.
        let view = TrashView::new(trashed_items(3));
        for pair in view.items.windows(2) {
            assert!(pair[0].time_deleted >= pair[1].time_deleted);
        }
    }

    #[test]
    fn r_on_empty_list_is_a_noop() {
        let mut view = TrashView::new(Vec::new());
        assert_eq!(view.handle_key(key(KeyCode::Char('r'))), ModeOp::None);
    }

    #[test]
    fn esc_exits_without_cleanup() {
        let mut view = TrashView::new(Vec::new());
        assert_eq!(
            view.handle_key(key(KeyCode::Esc)),
            ModeOp::Exit {
                cleanup: Cleanup::None
            }
        );
    }
}
