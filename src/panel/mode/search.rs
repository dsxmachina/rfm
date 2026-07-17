//! Search mode: live-highlighting footer input, ported verbatim from the
//! PanelManager's old inline search arm.
//!
//! Cancel semantics: Esc requests [`Cleanup::Search`] only. The old
//! blanket-Esc extras (clear_new_element, clear_rename_preview,
//! unmark_all_items) intentionally no longer run for search — a modal
//! mode undoes exactly the traces it created (sanctioned in the
//! mode-seam plan).

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

pub struct SearchMode {
    input: Input,
}

impl SearchMode {
    /// Starts with an empty input; the pattern builds up as keys arrive.
    pub fn new() -> Self {
        Self {
            input: Input::empty(),
        }
    }
}

impl Draw for SearchMode {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        draw_footer_prompt(stdout, x_range, y_range, "Search", &self.input, Color::Red)
    }
}

impl ModalInput for SearchMode {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        match key_event.code {
            KeyCode::Enter => ModeOp::FinishSearch(self.input.get().to_string()),
            KeyCode::Esc => ModeOp::Exit {
                cleanup: Cleanup::Search,
            },
            key_code => {
                self.input.update(key_code, key_event.modifiers);
                ModeOp::UpdateSearch(self.input.get().to_string())
            }
        }
    }

    fn region(&self) -> ModalRegion {
        ModalRegion::FooterLine
    }

    fn name(&self) -> &'static str {
        "search"
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
    fn typing_updates_search_live() {
        let mut mode = SearchMode::new();
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('f'))),
            ModeOp::UpdateSearch("f".into())
        );
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('o'))),
            ModeOp::UpdateSearch("fo".into())
        );
    }

    #[test]
    fn enter_finishes_search_with_pattern() {
        let mut mode = SearchMode::new();
        mode.handle_key(key(KeyCode::Char('f')));
        mode.handle_key(key(KeyCode::Char('o')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::FinishSearch("fo".into())
        );
    }

    #[test]
    fn esc_cancels_and_requests_search_cleanup() {
        let mut mode = SearchMode::new();
        mode.handle_key(key(KeyCode::Char('f')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Esc)),
            ModeOp::Exit {
                cleanup: Cleanup::Search
            }
        );
    }

    #[test]
    fn enter_with_empty_input_finishes_with_empty_pattern() {
        // Pin the old arm's permitted edge: Enter without any input
        // concludes the search with the empty pattern.
        let mut mode = SearchMode::new();
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::FinishSearch("".into())
        );
    }

    #[test]
    fn backspace_reaches_the_input() {
        let mut mode = SearchMode::new();
        mode.handle_key(key(KeyCode::Char('f')));
        mode.handle_key(key(KeyCode::Char('o')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Backspace)),
            ModeOp::UpdateSearch("f".into())
        );
    }
}
