//! Widget trait for UI components.
//!
//! This module defines the `Widget` trait which extends the rendering
//! capabilities of UI components with:
//! - Dirty flag tracking for efficient redraws
//! - Z-index support for layering
//! - Modal support for blocking overlays
//! - Focus handling

use std::io::Stdout;
use crossterm::Result;
use super::rect::Rect;

/// Z-index constants for standard UI layers.
///
/// Higher values are drawn on top of lower values.
pub mod z_index {
    /// Background panels (left, center, right directories)
    pub const PANELS: u8 = 20;
    /// Header and footer bars
    pub const BARS: u8 = 10;
    /// Log messages overlay
    pub const LOG: u8 = 30;
    /// Console/input overlay (cd, search, rename)
    pub const CONSOLE: u8 = 40;
    /// Modal dialogs (confirmation, help)
    pub const MODAL: u8 = 50;
    /// Notifications/toasts
    pub const NOTIFICATION: u8 = 60;
}

/// Result of handling an event by a widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventResult {
    /// Event was not handled, propagate to next handler
    Ignored,
    /// Event was handled, stop propagation
    Consumed,
}

/// A widget that can be rendered and handle events.
///
/// Widgets are the basic building blocks of the UI. They can be composed
/// into layers managed by a `Compositor`.
pub trait Widget {
    /// Render the widget to the given area.
    ///
    /// The widget should draw itself within the bounds of `area`.
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()>;

    /// Check if this widget needs to be redrawn.
    ///
    /// Returns true if the widget's state has changed since the last render.
    fn needs_redraw(&self) -> bool {
        true // Default: always redraw
    }

    /// Mark this widget as needing a redraw.
    fn mark_dirty(&mut self) {
        // Default: no-op (subclasses should override)
    }

    /// Mark this widget as clean (just rendered).
    fn mark_clean(&mut self) {
        // Default: no-op (subclasses should override)
    }

    /// Get the z-index of this widget.
    ///
    /// Higher values are drawn on top of lower values.
    fn z_index(&self) -> u8 {
        z_index::PANELS
    }

    /// Check if this widget blocks input to widgets below it.
    ///
    /// Modal widgets should return true to prevent interaction
    /// with underlying layers.
    fn is_modal(&self) -> bool {
        false
    }

    /// Check if this widget can receive focus.
    fn focusable(&self) -> bool {
        false
    }

    /// Called when this widget gains focus.
    fn on_focus(&mut self) {
        // Default: no-op
    }

    /// Called when this widget loses focus.
    fn on_blur(&mut self) {
        // Default: no-op
    }
}

/// A wrapper that adds dirty flag tracking to any widget-like type.
///
/// This is useful for wrapping existing panel types that don't
/// have their own dirty tracking.
pub struct DirtyWrapper<T> {
    inner: T,
    dirty: bool,
}

impl<T> DirtyWrapper<T> {
    /// Create a new wrapper around a widget.
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            dirty: true, // Start dirty to ensure initial render
        }
    }

    /// Get a reference to the inner widget.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner widget.
    ///
    /// Note: This automatically marks the widget as dirty.
    pub fn inner_mut(&mut self) -> &mut T {
        self.dirty = true;
        &mut self.inner
    }

    /// Get a mutable reference without marking dirty.
    ///
    /// Use with caution - only when you know the change doesn't affect rendering.
    pub fn inner_mut_silent(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Check if dirty.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as dirty.
    pub fn set_dirty(&mut self) {
        self.dirty = true;
    }

    /// Mark as clean.
    pub fn set_clean(&mut self) {
        self.dirty = false;
    }
}

/// A simple widget that renders a box with optional title.
///
/// Useful for modals and dialogs.
pub struct BoxWidget {
    title: Option<String>,
    dirty: bool,
}

impl BoxWidget {
    /// Create a new box widget.
    pub fn new() -> Self {
        Self {
            title: None,
            dirty: true,
        }
    }

    /// Create a new box widget with a title.
    pub fn with_title<S: Into<String>>(title: S) -> Self {
        Self {
            title: Some(title.into()),
            dirty: true,
        }
    }

    /// Set the title.
    pub fn set_title<S: Into<String>>(&mut self, title: S) {
        self.title = Some(title.into());
        self.dirty = true;
    }

    /// Clear the title.
    pub fn clear_title(&mut self) {
        self.title = None;
        self.dirty = true;
    }
}

impl Default for BoxWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for BoxWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        use crossterm::{cursor, queue, style::Print, QueueableCommand};
        use crossterm::terminal::{Clear, ClearType};

        if area.width < 2 || area.height < 2 {
            return Ok(());
        }

        // Clear the area
        for y in area.y..area.bottom() {
            queue!(stdout, cursor::MoveTo(area.x, y), Clear(ClearType::UntilNewLine))?;
            for _ in 0..area.width {
                queue!(stdout, Print(' '))?;
            }
        }

        // Draw corners
        queue!(
            stdout,
            cursor::MoveTo(area.x, area.y),
            Print("┌"),
            cursor::MoveTo(area.right().saturating_sub(1), area.y),
            Print("┐"),
            cursor::MoveTo(area.x, area.bottom().saturating_sub(1)),
            Print("└"),
            cursor::MoveTo(area.right().saturating_sub(1), area.bottom().saturating_sub(1)),
            Print("┘"),
        )?;

        // Draw top edge (with optional title)
        let top_width = area.width.saturating_sub(2) as usize;
        if let Some(title) = &self.title {
            let title_display = if title.len() > top_width.saturating_sub(2) {
                &title[..top_width.saturating_sub(2)]
            } else {
                title.as_str()
            };
            let padding = top_width.saturating_sub(title_display.len());
            let left_pad = padding / 2;
            let right_pad = padding - left_pad;

            stdout.queue(cursor::MoveTo(area.x + 1, area.y))?;
            for _ in 0..left_pad {
                stdout.queue(Print("─"))?;
            }
            stdout.queue(Print(" "))?
                  .queue(Print(title_display))?
                  .queue(Print(" "))?;
            for _ in 0..(right_pad.saturating_sub(2)) {
                stdout.queue(Print("─"))?;
            }
        } else {
            stdout.queue(cursor::MoveTo(area.x + 1, area.y))?;
            for _ in 0..top_width {
                stdout.queue(Print("─"))?;
            }
        }

        // Draw bottom edge
        stdout.queue(cursor::MoveTo(area.x + 1, area.bottom().saturating_sub(1)))?;
        for _ in 0..top_width {
            stdout.queue(Print("─"))?;
        }

        // Draw side edges
        for y in (area.y + 1)..area.bottom().saturating_sub(1) {
            queue!(
                stdout,
                cursor::MoveTo(area.x, y),
                Print("│"),
                cursor::MoveTo(area.right().saturating_sub(1), y),
                Print("│"),
            )?;
        }

        self.mark_clean();
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
        z_index::MODAL
    }

    fn is_modal(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirty_wrapper() {
        let mut wrapper = DirtyWrapper::new(42);

        assert!(wrapper.is_dirty());

        wrapper.set_clean();
        assert!(!wrapper.is_dirty());

        let _ = wrapper.inner_mut();
        assert!(wrapper.is_dirty());

        wrapper.set_clean();
        let _ = wrapper.inner_mut_silent();
        assert!(!wrapper.is_dirty());
    }

    #[test]
    fn test_z_index_ordering() {
        assert!(z_index::BARS < z_index::PANELS);
        assert!(z_index::PANELS < z_index::LOG);
        assert!(z_index::LOG < z_index::CONSOLE);
        assert!(z_index::CONSOLE < z_index::MODAL);
        assert!(z_index::MODAL < z_index::NOTIFICATION);
    }
}
