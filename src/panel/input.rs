use std::io::Stdout;

use crossterm::{
    event::KeyCode,
    style::{Color, ContentStyle, PrintStyledContent, StyledContent, Stylize},
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
            cursor: 0,
        }
    }

    /// Updates the input field
    pub fn update(&mut self, key_code: KeyCode) {
        // TODO respect cursor position
        if let KeyCode::Char(c) = key_code {
            self.input.push(c.to_ascii_lowercase());
            self.cursor = self.cursor.saturating_add(1);
        }
        if let KeyCode::Backspace = key_code {
            self.input.pop();
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn get(&self) -> &str {
        &self.input
    }

    pub fn cursor(&self) -> u16 {
        self.cursor as u16
    }

    pub fn print(&self, stdout: &mut Stdout, color: Color) -> crossterm::Result<()> {
        stdout.queue(PrintStyledContent(self.input.clone().bold().with(color)))?;
        Ok(())
    }
}
