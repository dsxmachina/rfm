//! Header widget for the top bar.
//!
//! Displays the username@hostname and current path.

use std::io::Stdout;
use std::path::PathBuf;
use crossterm::{
    cursor, queue,
    style::{self, Print, PrintStyledContent, Stylize},
    terminal::{Clear, ClearType},
    QueueableCommand,
    Result,
};

use crate::config::color::{color_dir_path, color_main};
use crate::panel::{
    rect::Rect,
    widget::{Widget, z_index},
};

/// Widget for the header bar at the top of the screen.
///
/// Displays: username@hostname path/to/current/directory
pub struct HeaderWidget {
    /// Current path to display.
    path: PathBuf,
    /// Username for prompt.
    username: String,
    /// Hostname for prompt.
    hostname: String,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl HeaderWidget {
    /// Create a new header widget.
    pub fn new() -> Self {
        Self {
            path: PathBuf::new(),
            username: whoami::username(),
            hostname: whoami::fallible::hostname().unwrap_or_else(|e| e.to_string()),
            dirty: true,
        }
    }

    /// Set the current path.
    pub fn set_path(&mut self, path: PathBuf) {
        if self.path != path {
            self.path = path;
            self.dirty = true;
        }
    }

    /// Get the current path.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Default for HeaderWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for HeaderWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        let prompt = format!("{}@{}", self.username, self.hostname);

        let absolute = self.path
            .canonicalize()
            .unwrap_or_else(|_| self.path.clone());

        let file_name = absolute
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default();

        let absolute_str = absolute.to_str().unwrap_or_default();
        let (prefix, suffix) = if absolute_str.len() >= file_name.len() {
            absolute_str.split_at(absolute_str.len() - file_name.len())
        } else {
            (absolute_str, "")
        };

        queue!(
            stdout,
            cursor::MoveTo(area.x, area.y),
            Clear(ClearType::CurrentLine),
            PrintStyledContent(prompt.with(color_main()).bold()),
            Print(" "),
            PrintStyledContent(prefix.to_string().with(color_dir_path()).bold()),
            PrintStyledContent(suffix.to_string().bold()),
        )?;

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
        z_index::BARS
    }
}
