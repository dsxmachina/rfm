use std::io::Stdout;

use crossterm::{
    event::{KeyCode, KeyModifiers},
    style::{Color, PrintStyledContent, Stylize},
    QueueableCommand,
};

pub struct Input {
    input: String,
    cursor: usize,
}

impl Input {
    /// Creates a new empty input element
    pub fn empty() -> Self {
        Self {
            input: "".to_owned(),
            cursor: 0,
        }
    }

    /// Creates a new input element from a string
    pub fn from_str<S: AsRef<str>>(string: S) -> Self {
        Self {
            input: string.as_ref().to_owned(),
            cursor: string.as_ref().len(),
        }
    }

    /// Helper function to safely decrease the cursor by one.
    ///
    /// Checks if the cursor lies on a char-boundary
    fn decrease_cursor(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        // A character can be up to four bytes - so we have to decrease the
        // cursor until we find a valid char-boundary
        while self.cursor > 0 && !self.input.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
        assert!(self.input.is_char_boundary(self.cursor));
    }

    /// Helper function to safely increase the cursor by one.
    ///
    /// Checks if the cursor lies on a char-boundary
    fn increase_cursor(&mut self) {
        self.cursor += 1;
        // A character can be up to four bytes - so we have to increase the
        // cursor up to 4 times (and we increased it by one already)
        for _ in 0..3 {
            if !self.input.is_char_boundary(self.cursor) {
                self.cursor += 1;
            }
        }
        // Saturate cursor at input length
        self.cursor = self.cursor.min(self.input.len());
        assert!(self.input.is_char_boundary(self.cursor));
    }

    /// Bulk delete helper: either delete left of cursor, or keep only extension
    fn bulk_delete(&mut self) {
        if self.cursor == self.input.len() {
            // Cursor at end: keep only the file extension
            if let Some(dot_pos) = self.input.rfind('.') {
                self.input = self.input[dot_pos..].to_owned();
                self.cursor = 0;
            } else {
                // No extension found, clear everything
                self.input.clear();
                self.cursor = 0;
            }
        } else {
            // Cursor not at end: delete everything left of cursor
            self.input = self.input[self.cursor..].to_owned();
            self.cursor = 0;
        }
    }

    /// Updates the input field
    pub fn update(&mut self, key_code: KeyCode, modifiers: KeyModifiers) {
        let has_shift = modifiers.contains(KeyModifiers::SHIFT);
        let has_ctrl = modifiers.contains(KeyModifiers::CONTROL);
        log::info!(
            "input-update: {}, input-len: {}, cursor: {}, shift: {has_shift}, ctrl: {has_ctrl}, keycode: {:?}",
            self.input,
            self.input.len(),
            self.cursor,
            key_code
        );
        match key_code {
            // Ctrl+U: bulk delete (common Unix shortcut for "kill line")
            KeyCode::Char('u') if has_ctrl => {
                self.bulk_delete();
            }
            KeyCode::Char(c) => {
                let insert_char = if has_shift {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                };
                if self.cursor == self.input.len() {
                    self.input.push(insert_char);
                } else {
                    self.input.insert(self.cursor, insert_char);
                }
                self.increase_cursor();
            }
            KeyCode::Backspace => {
                if has_shift || has_ctrl {
                    // Shift+Backspace or Ctrl+Backspace: bulk delete
                    self.bulk_delete();
                } else if self.cursor > 0 {
                    // Normal backspace: delete one character
                    self.decrease_cursor();
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                self.decrease_cursor();
            }
            KeyCode::Right => {
                self.increase_cursor();
            }
            _ => (),
        }
    }

    pub fn get(&self) -> &str {
        &self.input
    }

    pub fn print(&self, stdout: &mut Stdout, color: Color) -> crossterm::Result<()> {
        let (left, right) = self.input.as_str().split_at(self.cursor);

        let mut it = right.chars();
        let first = it.next().unwrap_or(' ');
        let remainder: String = it.collect();

        stdout
            .queue(PrintStyledContent(left.bold().with(color)))?
            .queue(PrintStyledContent(first.bold().with(color).underlined()))?
            .queue(PrintStyledContent(remainder.bold().with(color)))?;
        Ok(())
    }
}
