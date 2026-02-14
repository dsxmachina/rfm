//! Widget wrappers for DirPanel and PreviewPanel.
//!
//! These wrappers adapt the existing panel types to implement the Widget trait,
//! enabling them to be used with the Compositor.

use std::any::Any;
use std::io::Stdout;
use crossterm::Result;

use crate::panel::{
    rect::Rect,
    widget::{Widget, z_index},
    DirPanel, Draw,
};
use crate::panel::preview::PreviewPanel;

/// Widget wrapper for DirPanel.
///
/// Adapts DirPanel to the Widget trait for use with the Compositor.
pub struct DirPanelWidget {
    /// The wrapped panel.
    panel: DirPanel,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl DirPanelWidget {
    /// Create a new widget wrapping a DirPanel.
    pub fn new(panel: DirPanel) -> Self {
        Self {
            panel,
            dirty: true,
        }
    }

    /// Get a reference to the underlying panel.
    pub fn panel(&self) -> &DirPanel {
        &self.panel
    }

    /// Get a mutable reference to the underlying panel.
    ///
    /// Note: This automatically marks the widget as dirty.
    pub fn panel_mut(&mut self) -> &mut DirPanel {
        self.dirty = true;
        &mut self.panel
    }

    /// Replace the panel with a new one.
    pub fn set_panel(&mut self, panel: DirPanel) {
        self.panel = panel;
        self.dirty = true;
    }
}

impl Widget for DirPanelWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        self.panel.draw(stdout, area.x_range(), area.y_range())?;
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
        z_index::PANELS
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Widget wrapper for PreviewPanel.
///
/// Adapts PreviewPanel to the Widget trait for use with the Compositor.
pub struct PreviewPanelWidget {
    /// The wrapped panel.
    panel: PreviewPanel,
    /// Whether this widget needs to be redrawn.
    dirty: bool,
}

impl PreviewPanelWidget {
    /// Create a new widget wrapping a PreviewPanel.
    pub fn new(panel: PreviewPanel) -> Self {
        Self {
            panel,
            dirty: true,
        }
    }

    /// Get a reference to the underlying panel.
    pub fn panel(&self) -> &PreviewPanel {
        &self.panel
    }

    /// Get a mutable reference to the underlying panel.
    ///
    /// Note: This automatically marks the widget as dirty.
    pub fn panel_mut(&mut self) -> &mut PreviewPanel {
        self.dirty = true;
        &mut self.panel
    }

    /// Replace the panel with a new one.
    pub fn set_panel(&mut self, panel: PreviewPanel) {
        self.panel = panel;
        self.dirty = true;
    }
}

impl Widget for PreviewPanelWidget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()> {
        self.panel.draw(stdout, area.x_range(), area.y_range())?;
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
        z_index::PANELS
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
