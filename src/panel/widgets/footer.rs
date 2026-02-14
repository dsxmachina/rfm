//! Footer widget for the bottom bar.
//!
//! Displays file metadata, key buffer, and position.

use std::io::Stdout;
use std::path::Path;
use crossterm::{
    cursor, queue,
    style::{self, Print, PrintStyledContent, Stylize},
    terminal::{Clear, ClearType},
    QueueableCommand,
    Result,
};

use crate::panel::{
    rect::Rect,
    widget::{Widget, z_index},
};
use crate::util::print_metadata;

/// Widget for the footer bar at the bottom of the screen.
///
/// Displays file permissions, metadata, key buffer, and position in list.
pub struct FooterWidget {
    /// Permissions string (e.g., "-rw-r--r--").
    permissions: String,
    /// Metadata string (e.g., "4.2 KB 2024-01-01").
    metadata: String,
    /// Current key buffer (for vim-style commands).
    key_buffer: String,
    /// Current position in list (e.g., "5/20").
    position: String,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl FooterWidget {
    /// Create a new footer widget.
    pub fn new() -> Self {
        Self {
            permissions: String::new(),
            metadata: String::new(),
            key_buffer: String::new(),
            position: String::new(),
            dirty: true,
        }
    }

    /// Update the footer with information about a selected file.
    pub fn set_file_info(&mut self, path: Option<&Path>) {
        let (permissions, metadata) = print_metadata(path);
        if self.permissions != permissions || self.metadata != metadata {
            self.permissions = permissions;
            self.metadata = metadata;
            self.dirty = true;
        }
    }

    /// Set the key buffer display.
    pub fn set_key_buffer(&mut self, buffer: String) {
        if self.key_buffer != buffer {
            self.key_buffer = buffer;
            self.dirty = true;
        }
    }

    /// Set the position display (e.g., "5/20").
    pub fn set_position(&mut self, current: usize, total: usize) {
        let new_position = format!("{}/{}", current, total);
        if self.position != new_position {
            self.position = new_position;
            self.dirty = true;
        }
    }
}

impl Default for FooterWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for FooterWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        queue!(
            stdout,
            cursor::MoveTo(area.x, area.y),
            Clear(ClearType::CurrentLine),
            PrintStyledContent(self.permissions.clone().dark_cyan()),
            Print("   "),
            Print(&self.metadata),
        )?;

        // Key buffer in center
        if !self.key_buffer.is_empty() {
            let center_x = area.x + area.width / 2;
            let buffer_offset = self.key_buffer.len() as u16 / 2;
            queue!(
                stdout,
                cursor::MoveTo(center_x.saturating_sub(buffer_offset), area.y),
                PrintStyledContent(self.key_buffer.clone().dark_grey()),
            )?;
        }

        // Position on the right
        if !self.position.is_empty() {
            let position_display = format!("{} ", self.position);
            let right_x = area.right().saturating_sub(position_display.len() as u16);
            queue!(
                stdout,
                cursor::MoveTo(right_x, area.y),
                Print(&position_display),
            )?;
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
        z_index::BARS
    }
}
