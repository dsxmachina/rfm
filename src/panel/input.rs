use std::io::Stdout;

use crossterm::{
    event::KeyCode,
    style::{Color, PrintStyledContent, Stylize},
    QueueableCommand,
};
use log::info;

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
            cursor: 0,
        }
    }

    /// Updates the input field
    pub fn update(&mut self, key_code: KeyCode) {
        match key_code {
            KeyCode::Char(c) => {
                if self.cursor == self.input.len() {
                    self.input.push(c.to_ascii_lowercase());
                } else {
                    self.input.insert(self.cursor, c);
                }
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor == self.input.len() {
                    self.input.pop();
                } else if self.cursor > 0 {
                    self.input.remove(self.cursor.saturating_sub(1));
                }
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
            }
            KeyCode::Right => {
                self.cursor = self.cursor.saturating_add(1).min(self.input.len());
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
        let left: String = self.input.chars().take(self.cursor).collect();
        let right: String = self.input.chars().skip(self.cursor).collect();

        let first = right.chars().next().unwrap_or(' ');
        let remainder: String = right.chars().skip(1).collect();
        info!("{left}|{first}|{remainder}");

        stdout
            .queue(PrintStyledContent(left.bold().with(color)))?
            .queue(PrintStyledContent(first.bold().with(color).underlined()))?
            .queue(PrintStyledContent(remainder.bold().with(color)))?;
        Ok(())
    }
}
