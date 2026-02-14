//! Compositor for managing UI layers.
//!
//! The compositor manages a collection of layers, each containing a widget.
//! It handles:
//! - Rendering layers in z-index order
//! - Dirty region tracking for efficient redraws
//! - Modal layer management

use std::io::Stdout;
use crossterm::{
    cursor,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
    ExecutableCommand,
    QueueableCommand,
    Result,
};

use super::rect::Rect;
use super::widget::Widget;

/// Unique identifier for a layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerId(usize);

impl LayerId {
    /// Create a new layer ID.
    fn new(id: usize) -> Self {
        Self(id)
    }
}

/// A layer in the compositor.
///
/// Each layer contains a widget and has position/visibility properties.
pub struct Layer {
    /// The widget to render.
    widget: Box<dyn Widget + Send>,
    /// The area where this layer renders.
    area: Rect,
    /// Whether this layer is visible.
    visible: bool,
    /// Unique identifier for this layer.
    id: LayerId,
}

impl Layer {
    /// Create a new layer.
    fn new(id: LayerId, widget: Box<dyn Widget + Send>, area: Rect) -> Self {
        Self {
            widget,
            area,
            visible: true,
            id,
        }
    }

    /// Get the layer's z-index (delegated to widget).
    fn z_index(&self) -> u8 {
        self.widget.z_index()
    }

    /// Check if this layer is modal (delegated to widget).
    fn is_modal(&self) -> bool {
        self.widget.is_modal()
    }

    /// Check if this layer needs redraw (delegated to widget).
    fn needs_redraw(&self) -> bool {
        self.visible && self.widget.needs_redraw()
    }

    /// Render this layer.
    fn render(&mut self, stdout: &mut Stdout) -> Result<()> {
        if self.visible {
            self.widget.render(stdout, self.area)?;
            self.widget.mark_clean();
        }
        Ok(())
    }
}

/// The compositor manages all UI layers.
///
/// Layers are rendered in z-index order (lowest first, highest on top).
/// Modal layers block input to layers below them.
pub struct Compositor {
    /// All managed layers.
    layers: Vec<Layer>,
    /// Counter for generating unique layer IDs.
    next_id: usize,
    /// Dirty flag for full redraw.
    needs_full_redraw: bool,
}

impl Compositor {
    /// Create a new compositor.
    pub fn new() -> Self {
        Self {
            layers: Vec::new(),
            next_id: 0,
            needs_full_redraw: true,
        }
    }

    /// Add a layer with a widget at the specified area.
    ///
    /// Returns the layer ID for later reference.
    pub fn add_layer(&mut self, widget: Box<dyn Widget + Send>, area: Rect) -> LayerId {
        let id = LayerId::new(self.next_id);
        self.next_id += 1;
        self.layers.push(Layer::new(id, widget, area));
        self.needs_full_redraw = true;
        id
    }

    /// Remove a layer by ID.
    ///
    /// Returns the widget if the layer was found.
    pub fn remove_layer(&mut self, id: LayerId) -> Option<Box<dyn Widget + Send>> {
        if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
            self.needs_full_redraw = true;
            Some(self.layers.remove(pos).widget)
        } else {
            None
        }
    }

    /// Apply a function to a layer's widget.
    ///
    /// Returns None if the layer doesn't exist.
    pub fn with_widget<F, R>(&self, id: LayerId, f: F) -> Option<R>
    where
        F: FnOnce(&(dyn Widget + Send)) -> R,
    {
        self.layers.iter()
            .find(|l| l.id == id)
            .map(|l| f(l.widget.as_ref()))
    }

    /// Apply a mutable function to a layer's widget.
    ///
    /// Returns None if the layer doesn't exist.
    pub fn with_widget_mut<F, R>(&mut self, id: LayerId, f: F) -> Option<R>
    where
        F: FnOnce(&mut (dyn Widget + Send)) -> R,
    {
        self.layers.iter_mut()
            .find(|l| l.id == id)
            .map(|l| f(l.widget.as_mut()))
    }

    /// Set the area for a layer.
    pub fn set_layer_area(&mut self, id: LayerId, area: Rect) {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            if layer.area != area {
                layer.area = area;
                layer.widget.mark_dirty();
                self.needs_full_redraw = true;
            }
        }
    }

    /// Show a layer.
    pub fn show_layer(&mut self, id: LayerId) {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            if !layer.visible {
                layer.visible = true;
                layer.widget.mark_dirty();
                self.needs_full_redraw = true;
            }
        }
    }

    /// Hide a layer.
    pub fn hide_layer(&mut self, id: LayerId) {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            if layer.visible {
                layer.visible = false;
                self.needs_full_redraw = true;
            }
        }
    }

    /// Check if a layer is visible.
    pub fn is_layer_visible(&self, id: LayerId) -> bool {
        self.layers.iter()
            .find(|l| l.id == id)
            .map(|l| l.visible)
            .unwrap_or(false)
    }

    /// Mark all layers as needing redraw.
    pub fn mark_all_dirty(&mut self) {
        self.needs_full_redraw = true;
        for layer in &mut self.layers {
            layer.widget.mark_dirty();
        }
    }

    /// Mark a specific layer as needing redraw.
    pub fn mark_layer_dirty(&mut self, id: LayerId) {
        if let Some(layer) = self.layers.iter_mut().find(|l| l.id == id) {
            layer.widget.mark_dirty();
        }
    }

    /// Check if any layer needs to be redrawn.
    pub fn needs_redraw(&self) -> bool {
        self.needs_full_redraw || self.layers.iter().any(|l| l.needs_redraw())
    }

    /// Get the topmost modal layer, if any.
    pub fn topmost_modal(&self) -> Option<LayerId> {
        self.layers.iter()
            .filter(|l| l.visible && l.is_modal())
            .max_by_key(|l| l.z_index())
            .map(|l| l.id)
    }

    /// Check if there's a modal layer blocking input.
    pub fn has_modal(&self) -> bool {
        self.topmost_modal().is_some()
    }

    /// Render all visible layers in z-index order.
    ///
    /// This uses synchronized updates to prevent flicker.
    pub fn render(&mut self, stdout: &mut Stdout) -> Result<()> {
        if !self.needs_redraw() {
            return Ok(());
        }

        // Begin synchronized update
        stdout.execute(BeginSynchronizedUpdate)?;
        stdout.queue(cursor::Hide)?;

        // Sort layers by z-index for rendering
        let mut indices: Vec<usize> = (0..self.layers.len()).collect();
        indices.sort_by_key(|&i| self.layers[i].z_index());

        // Render each visible layer in order
        for i in indices {
            let layer = &mut self.layers[i];
            if layer.visible && (self.needs_full_redraw || layer.needs_redraw()) {
                layer.render(stdout)?;
            }
        }

        // End synchronized update
        stdout.execute(EndSynchronizedUpdate)?;

        self.needs_full_redraw = false;
        Ok(())
    }

    /// Get the number of layers.
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// Get the number of visible layers.
    pub fn visible_layer_count(&self) -> usize {
        self.layers.iter().filter(|l| l.visible).count()
    }
}

impl Default for Compositor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use crate::panel::widget::z_index;

    /// A simple test widget for unit tests.
    struct TestWidget {
        z: u8,
        dirty: bool,
        modal: bool,
    }

    impl TestWidget {
        fn new(z: u8) -> Self {
            Self { z, dirty: true, modal: false }
        }

        fn modal(z: u8) -> Self {
            Self { z, dirty: true, modal: true }
        }
    }

    impl Widget for TestWidget {
        fn render(&mut self, _stdout: &mut Stdout, _area: Rect) -> Result<()> {
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
            self.z
        }

        fn is_modal(&self) -> bool {
            self.modal
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    #[test]
    fn test_add_remove_layer() {
        let mut comp = Compositor::new();
        assert_eq!(comp.layer_count(), 0);

        let id1 = comp.add_layer(
            Box::new(TestWidget::new(z_index::PANELS)),
            Rect::new(0, 0, 10, 10),
        );
        assert_eq!(comp.layer_count(), 1);

        let id2 = comp.add_layer(
            Box::new(TestWidget::new(z_index::CONSOLE)),
            Rect::new(0, 0, 10, 10),
        );
        assert_eq!(comp.layer_count(), 2);

        // IDs should be unique
        assert_ne!(id1, id2);

        // Remove first layer
        comp.remove_layer(id1);
        assert_eq!(comp.layer_count(), 1);

        // Second layer should still be accessible
        assert!(comp.with_widget(id2, |_| ()).is_some());
    }

    #[test]
    fn test_layer_visibility() {
        let mut comp = Compositor::new();

        let id = comp.add_layer(
            Box::new(TestWidget::new(z_index::PANELS)),
            Rect::new(0, 0, 10, 10),
        );

        assert!(comp.is_layer_visible(id));
        assert_eq!(comp.visible_layer_count(), 1);

        comp.hide_layer(id);
        assert!(!comp.is_layer_visible(id));
        assert_eq!(comp.visible_layer_count(), 0);

        comp.show_layer(id);
        assert!(comp.is_layer_visible(id));
        assert_eq!(comp.visible_layer_count(), 1);
    }

    #[test]
    fn test_modal_detection() {
        let mut comp = Compositor::new();

        let _id1 = comp.add_layer(
            Box::new(TestWidget::new(z_index::PANELS)),
            Rect::new(0, 0, 10, 10),
        );

        assert!(!comp.has_modal());

        let id2 = comp.add_layer(
            Box::new(TestWidget::modal(z_index::MODAL)),
            Rect::new(0, 0, 10, 10),
        );

        assert!(comp.has_modal());
        assert_eq!(comp.topmost_modal(), Some(id2));
    }

    #[test]
    fn test_dirty_tracking() {
        let mut comp = Compositor::new();

        let id = comp.add_layer(
            Box::new(TestWidget::new(z_index::PANELS)),
            Rect::new(0, 0, 10, 10),
        );

        // Should need redraw initially
        assert!(comp.needs_redraw());

        // After marking all clean (simulating render)
        comp.needs_full_redraw = false;
        for layer in &mut comp.layers {
            layer.widget.mark_clean();
        }

        assert!(!comp.needs_redraw());

        // Mark dirty again
        comp.mark_layer_dirty(id);
        assert!(comp.needs_redraw());
    }
}
