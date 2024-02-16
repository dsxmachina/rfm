use std::io::Stdout;

use crossterm::{
    event::{KeyCode, KeyModifiers},
    style::{Color, PrintStyledContent, Stylize},
    QueueableCommand,
};
use log::debug;

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
        self.cursor = self.cursor.saturating_sub(1);
        // A character can be up to four bytes - so we have to decrease the
        // cursor up to 4 times (and we increased it by one already)
        for _ in 0..3 {
            if !self.input.is_char_boundary(self.cursor) {
                self.cursor = self.cursor.saturating_sub(1);
            }
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

    /// Updates the input field
    pub fn update(&mut self, key_code: KeyCode, modifiers: KeyModifiers) {
        debug!(
            "input-update: {}, input-len: {}, cursor: {}",
            self.input,
            self.input.len(),
            self.cursor
        );
        match key_code {
            KeyCode::Char(c) => {
                let insert_char = if modifiers.contains(KeyModifiers::SHIFT) {
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
                self.decrease_cursor();
                if self.cursor == self.input.len() {
                    self.input.pop();
                } else if self.cursor > 0 {
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

    pub fn cursor(&self) -> u16 {
        self.cursor as u16
    }

    pub fn print(&self, stdout: &mut Stdout, color: Color) -> crossterm::Result<()> {
        let (left, right) = self.input.as_str().split_at(self.cursor);
        // let left: String = self.input.chars().take(self.cursor).collect();
        // let right: String = self.input.chars().skip(self.cursor).collect();

        let first = right.chars().next().unwrap_or(' ');
        let remainder: String = right.chars().skip(1).collect();

        stdout
            .queue(PrintStyledContent(left.bold().with(color)))?
            .queue(PrintStyledContent(first.bold().with(color).underlined()))?
            .queue(PrintStyledContent(remainder.bold().with(color)))?;
        Ok(())
    }
}
