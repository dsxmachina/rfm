//! The trash overlay (`gT`): a read-only list of freedesktop trash items.
//! `r` restores the item under the cursor to its original location.
//!
//! Pure adapter — the manager supplies the item list (from
//! `trash::os_limited::list`) at construction and applies the restore
//! centrally. The adapter itself never touches the filesystem.

use crossterm::event::{KeyCode, KeyEvent};
use time::OffsetDateTime;

use super::{Cleanup, ModalInput, ModalRegion, ModeOp};
use crate::config::color::color_main;
use crate::panel::*;

pub struct TrashView {
    items: Vec<trash::TrashItem>,
    cursor: usize,
}

impl TrashView {
    /// `items` should already be ordered newest-first by the caller.
    pub fn new(items: Vec<trash::TrashItem>) -> Self {
        Self { items, cursor: 0 }
    }

    fn format_row(item: &trash::TrashItem) -> String {
        let name = item.name.to_string_lossy();
        let original = item.original_parent.join(&item.name);
        let when = OffsetDateTime::from_unix_timestamp(item.time_deleted)
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
            .unwrap_or_else(|_| "????-??-??".to_string());
        format!("{name}   {}   [{when}]", original.display())
    }
}

impl Draw for TrashView {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let x_start = x_range.start;
        let width = x_range.end.saturating_sub(x_range.start) as usize;
        let height = y_range.end.saturating_sub(y_range.start);

        // Title line.
        let title = " Trash — j/k: move · r: restore · Esc: close ";
        queue!(
            stdout,
            cursor::Hide,
            cursor::MoveTo(x_start, y_range.start),
            Clear(ClearType::CurrentLine),
            PrintStyledContent(title.bold().with(color_main()).reverse()),
        )?;

        if self.items.is_empty() {
            queue!(
                stdout,
                cursor::MoveTo(x_start, y_range.start.saturating_add(2)),
                Clear(ClearType::CurrentLine),
                PrintStyledContent("(the trash is empty)".dark_grey()),
            )?;
            return Ok(());
        }

        // Vertical window around the cursor (title takes one row).
        let visible_rows = height.saturating_sub(1) as usize;
        let offset = self
            .cursor
            .saturating_sub(visible_rows.saturating_sub(1))
            .min(self.items.len().saturating_sub(visible_rows).max(0));

        for (row, (idx, item)) in self
            .items
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible_rows)
            .enumerate()
        {
            let y = y_range.start.saturating_add(1).saturating_add(row as u16);
            let mut line = Self::format_row(item);
            if line.chars().count() > width {
                line = line.chars().take(width).collect();
            }
            queue!(
                stdout,
                cursor::MoveTo(x_start, y),
                Clear(ClearType::CurrentLine),
            )?;
            if idx == self.cursor {
                queue!(stdout, PrintStyledContent(line.with(color_main()).reverse()))?;
            } else {
                queue!(stdout, Print(line))?;
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
    fn trashed_items(n: usize) -> Vec<trash::TrashItem> {
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
        let items = trashed_items(2);
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
