//! Search mode: live-highlighting footer input, ported verbatim from the
//! PanelManager's old inline search arm.

use std::io::Stdout;
use std::ops::Range;

use crossterm::{
    cursor,
    event::{KeyCode, KeyEvent},
    style::{self, Print, PrintStyledContent, Stylize},
    QueueableCommand, Result,
};

use crate::config::color::color_main;
use crate::panel::input::Input;
use crate::panel::Draw;

use super::{Cleanup, ModalInput, ModalRegion, ModeOp};

pub struct SearchMode {
    input: Input,
}

impl SearchMode {
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
        stdout
            .queue(cursor::MoveTo(x_range.start, y_range.start))?
            .queue(PrintStyledContent(
                "Search".bold().with(color_main()).reverse(),
            ))?
            .queue(Print(" "))?;
        self.input.print(stdout, style::Color::Red)?;
        Ok(())
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
