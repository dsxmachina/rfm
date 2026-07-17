//! Rename mode: yellow footer input seeded with the selected file's name,
//! ported verbatim from the PanelManager's old inline rename arm.
//!
//! Cancel semantics: Esc requests [`Cleanup::RenamePreview`] only. The old
//! blanket-Esc extras (clear_search, clear_new_element, unmark_all_items)
//! intentionally no longer run for rename — a modal mode undoes exactly
//! the traces it created (sanctioned in the mode-seam plan).

use std::io::Stdout;
use std::ops::Range;

use crossterm::{
    event::{KeyCode, KeyEvent},
    style::Color,
    Result,
};

use crate::panel::input::Input;
use crate::panel::Draw;

use super::{draw_footer_prompt, Cleanup, ModalInput, ModalRegion, ModeOp};

pub struct RenameMode {
    input: Input,
}

impl RenameMode {
    /// Seeds the input with the current file name; the cursor starts at
    /// the end of the name (`Input::from_str` semantics), so typed
    /// characters append.
    pub fn new(file_name: String) -> Self {
        Self {
            input: Input::from_str(file_name),
        }
    }
}

impl Draw for RenameMode {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        draw_footer_prompt(
            stdout,
            x_range,
            y_range,
            "Rename:",
            &self.input,
            Color::Yellow,
        )
    }
}

impl ModalInput for RenameMode {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        match key_event.code {
            KeyCode::Enter => ModeOp::Rename {
                to: self.input.get().to_string(),
            },
            KeyCode::Esc => ModeOp::Exit {
                cleanup: Cleanup::RenamePreview,
            },
            key_code => {
                self.input.update(key_code, key_event.modifiers);
                ModeOp::RenamePreview(self.input.get().to_string())
            }
        }
    }

    fn region(&self) -> ModalRegion {
        ModalRegion::FooterLine
    }

    fn name(&self) -> &'static str {
        "rename"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_appends_to_the_seeded_name() {
        // Input::from_str seeds the cursor at the end of the name,
        // so a typed character appends.
        let mut mode = RenameMode::new("old.txt".into());
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('x'))),
            ModeOp::RenamePreview("old.txtx".into())
        );
    }

    #[test]
    fn enter_renames_to_the_current_input() {
        let mut mode = RenameMode::new("old.txt".into());
        mode.handle_key(key(KeyCode::Char('x')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::Rename {
                to: "old.txtx".into()
            }
        );
    }

    #[test]
    fn esc_cancels_and_requests_preview_cleanup() {
        let mut mode = RenameMode::new("old.txt".into());
        mode.handle_key(key(KeyCode::Char('x')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Esc)),
            ModeOp::Exit {
                cleanup: Cleanup::RenamePreview
            }
        );
    }

    #[test]
    fn backspace_deletes_at_the_end_of_the_seeded_name() {
        let mut mode = RenameMode::new("old.txt".into());
        assert_eq!(
            mode.handle_key(key(KeyCode::Backspace)),
            ModeOp::RenamePreview("old.tx".into())
        );
    }

    #[test]
    fn enter_with_unchanged_name_still_emits_rename() {
        // The same-path guard lives at apply time (the manager compares
        // `from` against parent-join(to) and no-ops) — the adapter always
        // emits the op, exactly like the old inline arm reached its
        // Enter block regardless of whether the name changed.
        let mut mode = RenameMode::new("old.txt".into());
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::Rename {
                to: "old.txt".into()
            }
        );
    }
}
