//! Log widget for displaying log messages.
//!
//! Shows warning and error messages as an overlay.

use std::any::Any;
use std::io::Stdout;
use crossterm::{
    cursor, queue,
    style::{Print, PrintStyledContent, Stylize},
    terminal::{Clear, ClearType},
    QueueableCommand,
    Result,
};
use log::Level;

use crate::config::color::color_main;
use crate::panel::{
    rect::Rect,
    widget::{Widget, z_index},
};

/// A log entry with level and message.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: Level,
    pub message: String,
}

/// Widget for displaying log messages.
///
/// Shows the most recent log entries, with warnings and errors highlighted.
pub struct LogWidget {
    /// Log entries to display.
    entries: Vec<LogEntry>,
    /// Whether to show all entries or just warnings/errors.
    show_all: bool,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl LogWidget {
    /// Create a new log widget.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            show_all: false,
            dirty: true,
        }
    }

    /// Set the log entries to display.
    pub fn set_entries(&mut self, entries: Vec<LogEntry>) {
        self.entries = entries;
        self.dirty = true;
    }

    /// Set whether to show all entries or just warnings/errors.
    pub fn set_show_all(&mut self, show_all: bool) {
        if self.show_all != show_all {
            self.show_all = show_all;
            self.dirty = true;
        }
    }

    /// Check if showing all entries.
    pub fn is_showing_all(&self) -> bool {
        self.show_all
    }
}

impl Default for LogWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for LogWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        if area.height == 0 {
            self.dirty = false;
            return Ok(());
        }

        let print_level = |level: Level| match level {
            Level::Error => PrintStyledContent("error".red().bold()),
            Level::Warn => PrintStyledContent("warn".yellow().bold()),
            Level::Info => PrintStyledContent("info".with(color_main()).bold()),
            Level::Debug => PrintStyledContent("debug".dark_blue()),
            Level::Trace => PrintStyledContent("trace".grey()),
        };

        // Filter entries based on show_all flag
        let entries_to_show: Vec<&LogEntry> = if self.show_all {
            self.entries.iter().rev().take(area.height as usize).collect()
        } else {
            // Only show the most recent warning/error
            self.entries
                .iter()
                .rev()
                .find(|e| e.level <= Level::Warn)
                .into_iter()
                .collect()
        };

        // Clear the area first
        for row in 0..area.height {
            queue!(
                stdout,
                cursor::MoveTo(area.x, area.y + row),
                Clear(ClearType::CurrentLine),
            )?;
        }

        // Render entries from bottom up
        let mut y = area.bottom().saturating_sub(1);
        for entry in entries_to_show {
            if y < area.y {
                break;
            }
            queue!(
                stdout,
                cursor::MoveTo(area.x, y),
                print_level(entry.level),
                Print(": "),
                PrintStyledContent(entry.message.clone().grey()),
            )?;
            y = y.saturating_sub(1);
        }

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
        z_index::LOG
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
