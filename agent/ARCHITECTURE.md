# New UI Architecture Overview

## The Problem

The original `PanelManager` (~1200 lines) mixed several concerns:

```
PanelManager {
    // Manual redraw tracking
    redraw: Redraw {
        left: bool,
        center: bool,
        right: bool,
        console: bool,
        log: bool,
        header: bool,
        footer: bool,
    }
}
```

Every time something changed, you had to manually set the right boolean flags. Adding a new UI element (like a confirmation dialog) required:
1. Adding a new boolean flag
2. Adding a new `draw_*` method
3. Manually managing the draw order
4. Handling input routing

## The Solution: Layers and Widgets

### Core Concepts

```
┌─────────────────────────────────────────────────────────┐
│                      Compositor                         │
│  ┌─────────────────────────────────────────────────┐   │
│  │ Layer (z=50): Modal Dialog                      │   │
│  │   └─ BoxWidget                                  │   │
│  ├─────────────────────────────────────────────────┤   │
│  │ Layer (z=40): Console (cd/search/rename)        │   │
│  │   └─ ConsoleWidget                              │   │
│  ├─────────────────────────────────────────────────┤   │
│  │ Layer (z=30): Log Messages                      │   │
│  │   └─ LogWidget                                  │   │
│  ├─────────────────────────────────────────────────┤   │
│  │ Layer (z=20): Panels                            │   │
│  │   ├─ DirPanelWidget (left)                      │   │
│  │   ├─ DirPanelWidget (center)                    │   │
│  │   └─ PreviewPanelWidget (right)                 │   │
│  ├─────────────────────────────────────────────────┤   │
│  │ Layer (z=10): Header & Footer                   │   │
│  │   ├─ HeaderWidget                               │   │
│  │   └─ FooterWidget                               │   │
│  └─────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

**Widget**: A UI component that knows how to render itself and track if it needs redrawing.

**Layer**: A widget placed at a specific screen region with a z-index.

**Compositor**: Manages all layers, renders them in z-order, tracks dirty regions.

### Z-Index (Drawing Order)

Higher z-index = drawn on top:

| Z-Index | Layer Type | Example |
|---------|------------|---------|
| 10 | BARS | Header, Footer |
| 20 | PANELS | Left, Center, Right panels |
| 30 | LOG | Warning/error messages |
| 40 | CONSOLE | cd input, search, rename |
| 50 | MODAL | Confirmation dialogs |
| 60 | NOTIFICATION | Toast messages |

### The Widget Trait

```rust
pub trait Widget {
    /// Draw yourself in this area
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()>;

    /// Do you need to be redrawn?
    fn needs_redraw(&self) -> bool;

    /// Mark yourself as needing redraw
    fn mark_dirty(&mut self);

    /// Your z-index (higher = on top)
    fn z_index(&self) -> u8;

    /// Do you block input to layers below?
    fn is_modal(&self) -> bool;
}
```

### The Rect Type

Replaces the old `x_range: Range<u16>, y_range: Range<u16>` pattern:

```rust
// Old way
fn draw(&mut self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>)

// New way
fn render(&mut self, stdout: &mut Stdout, area: Rect)

// Rect provides helpful methods
area.x, area.y, area.width, area.height
area.left(), area.right(), area.top(), area.bottom()
area.contains(x, y)
area.intersects(&other)
area.inner(margin)  // shrink by margin
area.split_vertical(at)  // divide into two rects
```

### The Compositor

```rust
let mut compositor = Compositor::new();

// Add layers (order doesn't matter - z-index determines draw order)
let header_id = compositor.add_layer(
    Box::new(HeaderWidget::new()),
    layout.header_rect(),
);

let center_id = compositor.add_layer(
    Box::new(DirPanelWidget::new(center_panel)),
    layout.center_rect(),
);

// Later: show a modal
let modal_id = compositor.add_layer(
    Box::new(BoxWidget::with_title("Confirm Delete?")),
    Rect::new(20, 10, 40, 10),
);

// Render everything in correct order
compositor.render(&mut stdout)?;

// Hide the modal
compositor.hide_layer(modal_id);

// Mark something as needing redraw
compositor.mark_layer_dirty(center_id);
```

## File Structure

```
src/panel/
├── rect.rs            # Rect type for screen regions
├── widget.rs          # Widget trait + z_index constants
├── compositor.rs      # Layer management
├── render.rs          # RenderContext helper (optional use)
│
├── widgets/           # Widget implementations
│   ├── panel_widget.rs   # DirPanelWidget, PreviewPanelWidget
│   ├── header.rs         # HeaderWidget
│   └── footer.rs         # FooterWidget
│
└── (existing files unchanged)
```

## How It Simplifies Things

### Adding a Confirmation Dialog (Before)

```rust
// 1. Add field to Redraw struct
struct Redraw {
    // ... existing fields ...
    confirm_dialog: bool,  // NEW
}

// 2. Add state to PanelManager
confirm_dialog_visible: bool,
confirm_dialog_message: String,

// 3. Add draw method
fn draw_confirm_dialog(&mut self) -> Result<()> {
    if !self.redraw.confirm_dialog { return Ok(()); }
    // ... drawing code ...
}

// 4. Call it in the right order in draw()
fn draw(&mut self) -> Result<()> {
    self.draw_panels()?;
    self.draw_confirm_dialog()?;  // Must be after panels!
    // ...
}

// 5. Handle input routing
fn handle_event(&mut self, event: Event) {
    if self.confirm_dialog_visible {
        // Handle dialog input
    } else {
        // Normal input
    }
}
```

### Adding a Confirmation Dialog (After)

```rust
// Just add a layer when needed
let dialog = BoxWidget::with_title("Delete 3 files?");
let dialog_id = self.compositor.add_layer(
    Box::new(dialog),
    centered_rect(40, 10),
);

// Input routing is automatic (modal layers block input below)
if self.compositor.has_modal() {
    // Handle modal input
}

// Remove when done
self.compositor.remove_layer(dialog_id);
```

## Backward Compatibility

The new architecture is **additive**:
- Existing `Draw` trait still works
- Existing panel types unchanged
- `PanelManager` can adopt compositor gradually
- Widget wrappers adapt old types to new trait

You can use the new system for new features while keeping existing code working.
