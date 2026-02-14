//! Render context for terminal drawing operations.
//!
//! This module provides `RenderContext`, a wrapper around terminal output
//! that tracks dirty regions and provides a cleaner rendering API.

use std::io::{Stdout, Write};

use crossterm::{
    cursor,
    queue,
    style::{self, Color, Print, PrintStyledContent, Stylize},
    terminal::{Clear, ClearType},
    QueueableCommand, Result,
};

use super::rect::Rect;

/// Context for rendering operations.
///
/// Wraps stdout and provides methods for common terminal operations.
/// Tracks dirty regions for efficient partial redraws.
pub struct RenderContext<'a> {
    stdout: &'a mut Stdout,
    /// Regions that need to be redrawn.
    dirty_regions: Vec<Rect>,
    /// Current clip region (if any).
    clip: Option<Rect>,
}

impl<'a> RenderContext<'a> {
    /// Create a new render context.
    pub fn new(stdout: &'a mut Stdout) -> Self {
        Self {
            stdout,
            dirty_regions: Vec::new(),
            clip: None,
        }
    }

    /// Get a reference to stdout for direct queue operations.
    ///
    /// Use this when you need to use crossterm's queue! macro directly.
    pub fn stdout(&mut self) -> &mut Stdout {
        self.stdout
    }

    /// Mark a region as dirty (needs redraw).
    pub fn mark_dirty(&mut self, region: Rect) {
        self.dirty_regions.push(region);
    }

    /// Clear dirty regions.
    pub fn clear_dirty(&mut self) {
        self.dirty_regions.clear();
    }

    /// Check if a region intersects with any dirty region.
    pub fn is_dirty(&self, region: &Rect) -> bool {
        if self.dirty_regions.is_empty() {
            return true; // If no dirty tracking, assume everything needs redraw
        }
        self.dirty_regions.iter().any(|dirty| dirty.intersects(region))
    }

    /// Set a clip region. All drawing operations will be clipped to this region.
    pub fn set_clip(&mut self, clip: Rect) {
        self.clip = Some(clip);
    }

    /// Clear the clip region.
    pub fn clear_clip(&mut self) {
        self.clip = None;
    }

    /// Move cursor to position (x, y).
    pub fn move_to(&mut self, x: u16, y: u16) -> Result<&mut Self> {
        self.stdout.queue(cursor::MoveTo(x, y))?;
        Ok(self)
    }

    /// Clear the current line.
    pub fn clear_line(&mut self) -> Result<&mut Self> {
        self.stdout.queue(Clear(ClearType::CurrentLine))?;
        Ok(self)
    }

    /// Clear from cursor to end of line.
    pub fn clear_to_eol(&mut self) -> Result<&mut Self> {
        self.stdout.queue(Clear(ClearType::UntilNewLine))?;
        Ok(self)
    }

    /// Clear a rectangular region by filling with spaces.
    pub fn clear_region(&mut self, region: Rect) -> Result<&mut Self> {
        for y in region.y..region.bottom() {
            self.stdout.queue(cursor::MoveTo(region.x, y))?;
            for _ in 0..region.width {
                self.stdout.queue(Print(' '))?;
            }
        }
        Ok(self)
    }

    /// Print a string at the current cursor position.
    pub fn print<S: AsRef<str>>(&mut self, text: S) -> Result<&mut Self> {
        self.stdout.queue(Print(text.as_ref()))?;
        Ok(self)
    }

    /// Print a styled string at the current cursor position.
    pub fn print_styled<D: std::fmt::Display>(&mut self, content: style::StyledContent<D>) -> Result<&mut Self> {
        self.stdout.queue(PrintStyledContent(content))?;
        Ok(self)
    }

    /// Print text at a specific position.
    pub fn print_at<S: AsRef<str>>(&mut self, x: u16, y: u16, text: S) -> Result<&mut Self> {
        self.move_to(x, y)?;
        self.print(text)
    }

    /// Print styled text at a specific position.
    pub fn print_styled_at<D: std::fmt::Display>(&mut self, x: u16, y: u16, content: style::StyledContent<D>) -> Result<&mut Self> {
        self.move_to(x, y)?;
        self.print_styled(content)
    }

    /// Draw a horizontal line of a character.
    pub fn hline(&mut self, x: u16, y: u16, width: u16, ch: char) -> Result<&mut Self> {
        self.move_to(x, y)?;
        for _ in 0..width {
            self.stdout.queue(Print(ch))?;
        }
        Ok(self)
    }

    /// Draw a vertical line of a character.
    pub fn vline(&mut self, x: u16, y: u16, height: u16, ch: char) -> Result<&mut Self> {
        for row in 0..height {
            self.move_to(x, y + row)?;
            self.stdout.queue(Print(ch))?;
        }
        Ok(self)
    }

    /// Fill a region with a character.
    pub fn fill(&mut self, region: Rect, ch: char) -> Result<&mut Self> {
        for y in region.y..region.bottom() {
            self.move_to(region.x, y)?;
            for _ in 0..region.width {
                self.stdout.queue(Print(ch))?;
            }
        }
        Ok(self)
    }

    /// Draw a simple box border around a region.
    pub fn draw_box(&mut self, region: Rect) -> Result<&mut Self> {
        if region.width < 2 || region.height < 2 {
            return Ok(self);
        }

        // Corners
        self.print_at(region.x, region.y, "┌")?;
        self.print_at(region.right() - 1, region.y, "┐")?;
        self.print_at(region.x, region.bottom() - 1, "└")?;
        self.print_at(region.right() - 1, region.bottom() - 1, "┘")?;

        // Top and bottom edges
        for x in region.x + 1..region.right() - 1 {
            self.print_at(x, region.y, "─")?;
            self.print_at(x, region.bottom() - 1, "─")?;
        }

        // Left and right edges
        for y in region.y + 1..region.bottom() - 1 {
            self.print_at(region.x, y, "│")?;
            self.print_at(region.right() - 1, y, "│")?;
        }

        Ok(self)
    }

    /// Flush the output buffer.
    pub fn flush(&mut self) -> Result<()> {
        self.stdout.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Most render context tests would require mocking stdout,
    // which is complex. We'll test the non-IO parts here.

    #[test]
    fn test_dirty_regions() {
        let mut stdout = std::io::stdout();
        let mut ctx = RenderContext::new(&mut stdout);

        // Initially no dirty regions means everything is considered dirty
        assert!(ctx.is_dirty(&Rect::new(0, 0, 10, 10)));

        // Mark a region dirty
        ctx.mark_dirty(Rect::new(5, 5, 10, 10));

        // Overlapping region should be dirty
        assert!(ctx.is_dirty(&Rect::new(0, 0, 10, 10)));

        // Non-overlapping region should not be dirty
        assert!(!ctx.is_dirty(&Rect::new(20, 20, 5, 5)));

        // Clear dirty regions
        ctx.clear_dirty();

        // After clearing, we're back to "everything dirty" behavior
        assert!(ctx.is_dirty(&Rect::new(20, 20, 5, 5)));
    }
}
