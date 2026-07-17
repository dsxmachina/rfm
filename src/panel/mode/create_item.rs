//! Create-item mode: footer input for mkdir/touch, ported verbatim from
//! the PanelManager's old inline CreateItem arm.
//!
//! Cancel semantics: Esc requests [`Cleanup::CreatePreview`] only. The
//! old blanket-Esc extras (clear_search, clear_rename_preview,
//! unmark_all_items) intentionally no longer run for create — a modal
//! mode undoes exactly the traces it created (sanctioned in the
//! mode-seam plan).

use std::io::Stdout;
use std::ops::Range;

use crossterm::{
    event::{KeyCode, KeyEvent},
    style::Color,
    Result,
};

use crate::config::color::color_main;
use crate::panel::input::Input;
use crate::panel::Draw;

use super::{draw_footer_prompt, Cleanup, ModalInput, ModalRegion, ModeOp};

pub struct CreateItemMode {
    input: Input,
    is_dir: bool,
}

impl CreateItemMode {
    /// Starts with an empty input; `is_dir` picks mkdir over touch.
    pub fn new(is_dir: bool) -> Self {
        Self {
            input: Input::empty(),
            is_dir,
        }
    }
}

impl Draw for CreateItemMode {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let (label, color) = if self.is_dir {
            ("Make Directory:", color_main())
        } else {
            ("Touch:", Color::Grey)
        };
        draw_footer_prompt(stdout, x_range, y_range, label, &self.input, color)
    }
}

impl ModalInput for CreateItemMode {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        match key_event.code {
            KeyCode::Enter => ModeOp::Create {
                // Trimming stays at apply time (the old arm joined
                // `input.get().trim()` only when creating).
                name: self.input.get().to_string(),
                is_dir: self.is_dir,
            },
            // The old arm's autocomplete placeholder was a footer-only
            // redraw; the manager repaints on every modal key anyway
            // (apply_mode_op marks dirty), so plain None is exact parity.
            KeyCode::Tab => ModeOp::None,
            KeyCode::Esc => ModeOp::Exit {
                cleanup: Cleanup::CreatePreview,
            },
            key_code => {
                self.input.update(key_code, key_event.modifiers);
                ModeOp::CreatePreview {
                    name: self.input.get().to_string(),
                    is_dir: self.is_dir,
                }
            }
        }
    }

    fn region(&self) -> ModalRegion {
        ModalRegion::FooterLine
    }

    fn name(&self) -> &'static str {
        if self.is_dir {
            "mkdir"
        } else {
            "touch"
        }
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
    fn typing_previews_the_new_file() {
        let mut mode = CreateItemMode::new(false);
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('f'))),
            ModeOp::CreatePreview {
                name: "f".into(),
                is_dir: false
            }
        );
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('o'))),
            ModeOp::CreatePreview {
                name: "fo".into(),
                is_dir: false
            }
        );
    }

    #[test]
    fn typing_previews_the_new_directory() {
        let mut mode = CreateItemMode::new(true);
        assert_eq!(
            mode.handle_key(key(KeyCode::Char('d'))),
            ModeOp::CreatePreview {
                name: "d".into(),
                is_dir: true
            }
        );
    }

    #[test]
    fn enter_creates_with_the_current_input() {
        let mut mode = CreateItemMode::new(true);
        mode.handle_key(key(KeyCode::Char('d')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::Create {
                name: "d".into(),
                is_dir: true
            }
        );
    }

    #[test]
    fn enter_emits_the_name_untrimmed() {
        // The old inline arm trimmed at apply time
        // (`current_path.join(input.get().trim())`), while the live
        // preview stayed untrimmed — so the trim belongs to
        // apply_create, and the adapter emits the raw input.
        let mut mode = CreateItemMode::new(false);
        mode.handle_key(key(KeyCode::Char(' ')));
        mode.handle_key(key(KeyCode::Char('a')));
        mode.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::Create {
                name: " a ".into(),
                is_dir: false
            }
        );
    }

    #[test]
    fn esc_cancels_and_requests_preview_cleanup() {
        let mut mode = CreateItemMode::new(true);
        mode.handle_key(key(KeyCode::Char('d')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Esc)),
            ModeOp::Exit {
                cleanup: Cleanup::CreatePreview
            }
        );
    }

    #[test]
    fn tab_is_a_no_op_placeholder() {
        // The old arm's Tab branch was `/* autocomplete here ? */` plus
        // a footer redraw; the manager repaints on every modal key anyway
        // (apply_mode_op marks dirty), so plain None is exact parity.
        // Tab must not reach the input.
        let mut mode = CreateItemMode::new(false);
        mode.handle_key(key(KeyCode::Char('f')));
        assert_eq!(mode.handle_key(key(KeyCode::Tab)), ModeOp::None);
        assert_eq!(
            mode.handle_key(key(KeyCode::Enter)),
            ModeOp::Create {
                name: "f".into(),
                is_dir: false
            }
        );
    }

    #[test]
    fn backspace_deletes_the_last_char() {
        let mut mode = CreateItemMode::new(false);
        mode.handle_key(key(KeyCode::Char('f')));
        mode.handle_key(key(KeyCode::Char('o')));
        assert_eq!(
            mode.handle_key(key(KeyCode::Backspace)),
            ModeOp::CreatePreview {
                name: "f".into(),
                is_dir: false
            }
        );
    }

    #[test]
    fn name_is_the_debug_mode_string() {
        assert_eq!(CreateItemMode::new(true).name(), "mkdir");
        assert_eq!(CreateItemMode::new(false).name(), "touch");
    }
}
