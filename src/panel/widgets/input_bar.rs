//! Input bar widget for mode-specific input.
//!
//! This widget displays a labeled input field for Search, Rename, and CreateItem modes.
//! It overlays the footer when active.

use std::any::Any;
use std::io::Stdout;
use crossterm::{
    cursor, queue,
    style::{self, Color, Print, PrintStyledContent, Stylize},
    terminal::{Clear, ClearType},
    QueueableCommand,
    Result,
};

use crate::config::color::color_main;
use crate::panel::{
    rect::Rect,
    widget::{Widget, z_index},
};

/// The type of input being collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputType {
    Search,
    Rename,
    Mkdir,
    Touch,
}

impl InputType {
    /// Get the label to display for this input type.
    pub fn label(&self) -> &'static str {
        match self {
            InputType::Search => "Search",
            InputType::Rename => "Rename:",
            InputType::Mkdir => "Make Directory:",
            InputType::Touch => "Touch:",
        }
    }

    /// Get the color for the input text.
    pub fn color(&self) -> Color {
        match self {
            InputType::Search => Color::Red,
            InputType::Rename => Color::Yellow,
            InputType::Mkdir => color_main(),
            InputType::Touch => Color::Grey,
        }
    }
}

/// Widget for displaying a labeled input field.
///
/// Used for Search, Rename, Mkdir, and Touch operations.
pub struct InputBarWidget {
    /// The type of input.
    input_type: InputType,
    /// Current input text.
    text: String,
    /// Cursor position in the input.
    cursor: usize,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl InputBarWidget {
    /// Create a new input bar widget.
    pub fn new(input_type: InputType) -> Self {
        Self {
            input_type,
            text: String::new(),
            cursor: 0,
            dirty: true,
        }
    }

    /// Create with initial text (e.g., for rename).
    pub fn with_text(input_type: InputType, text: String) -> Self {
        let cursor = text.len();
        Self {
            input_type,
            text,
            cursor,
            dirty: true,
        }
    }

    /// Set the input type.
    pub fn set_input_type(&mut self, input_type: InputType) {
        if self.input_type != input_type {
            self.input_type = input_type;
            self.dirty = true;
        }
    }

    /// Set the current text and cursor position.
    pub fn set_text(&mut self, text: String, cursor: usize) {
        if self.text != text || self.cursor != cursor {
            self.text = text;
            self.cursor = cursor.min(self.text.len());
            self.dirty = true;
        }
    }

    /// Get the current text.
    pub fn text(&self) -> &str {
        &self.text
    }
}

impl Widget for InputBarWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        let label = self.input_type.label();
        let color = self.input_type.color();

        queue!(
            stdout,
            cursor::MoveTo(area.x, area.y),
            Clear(ClearType::CurrentLine),
            PrintStyledContent(label.bold().with(color_main()).reverse()),
            Print(" "),
        )?;

        // Render input text with cursor
        let (left, right) = if self.cursor <= self.text.len() {
            self.text.split_at(self.cursor)
        } else {
            (self.text.as_str(), "")
        };

        let mut it = right.chars();
        let cursor_char = it.next().unwrap_or(' ');
        let remainder: String = it.collect();

        stdout
            .queue(PrintStyledContent(left.bold().with(color)))?
            .queue(PrintStyledContent(cursor_char.bold().with(color).underlined()))?
            .queue(PrintStyledContent(remainder.bold().with(color)))?;

        self.dirty = false;
        Ok(())
    }

    fn needs_redraw(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    fn mark_clean(&mut self) {
        self.dirty = false;
    }

    fn z_index(&self) -> u8 {
        // Higher than footer so it overlays
        z_index::CONSOLE
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
