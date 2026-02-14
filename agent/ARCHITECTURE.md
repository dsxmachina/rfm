# UI Architecture

## Overview

The UI rendering uses a **Compositor** pattern that manages layers of widgets. Each widget knows how to render itself and track whether it needs redrawing. The compositor renders layers in z-index order (lowest first, highest on top).

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────┐
│                      Compositor                         │
│  ┌─────────────────────────────────────────────────┐   │
│  │ Layer (z=50): Modal Dialog                      │   │
│  │   └─ BoxWidget                                  │   │
│  ├─────────────────────────────────────────────────┤   │
│  │ Layer (z=40): Input Bar / Console               │   │
│  │   └─ InputBarWidget (search/rename/mkdir/touch) │   │
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

## Key Components

### Widget Trait (`src/panel/widget.rs`)

```rust
pub trait Widget {
    fn render(&mut self, stdout: &mut Stdout, area: Rect) -> Result<()>;
    fn needs_redraw(&self) -> bool;
    fn mark_dirty(&mut self);
    fn mark_clean(&mut self);
    fn z_index(&self) -> u8;
    fn is_modal(&self) -> bool;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}
```

### Z-Index Constants

| Z-Index | Constant | Purpose |
|---------|----------|---------|
| 10 | `BARS` | Header, Footer |
| 20 | `PANELS` | Left, Center, Right panels |
| 30 | `LOG` | Warning/error messages |
| 40 | `CONSOLE` | Input overlays (search, rename, mkdir) |
| 50 | `MODAL` | Confirmation dialogs |
| 60 | `NOTIFICATION` | Toast messages |

### Compositor (`src/panel/compositor.rs`)

- Manages a collection of layers
- Renders layers in z-index order
- Tracks dirty state for efficient redraws
- Supports modal layer detection

### Widgets (`src/panel/widgets/`)

| Widget | Purpose |
|--------|---------|
| `DirPanelWidget` | Wraps DirPanel for directory listing |
| `PreviewPanelWidget` | Wraps PreviewPanel for file previews |
| `HeaderWidget` | Top bar with username@host and path |
| `FooterWidget` | Bottom bar with permissions and position |
| `InputBarWidget` | Input field for search/rename/mkdir/touch |
| `LogWidget` | Log message display overlay |
| `BoxWidget` | Modal dialog box |

## Rendering Flow

1. State changes trigger `redraw_*` methods in PanelManager
2. Before rendering, `sync_to_compositor()` updates widget state
3. `compositor.render()` draws all visible layers in z-order
4. Console mode renders separately (complex interactive overlay)

```rust
fn draw(&mut self) -> Result<()> {
    // Sync all state to widgets
    self.sync_to_compositor();

    // Render via compositor
    self.compositor.render(&mut self.stdout)?;

    // Console renders separately
    self.draw_console()?;

    Ok(())
}
```

## Mode Handling

The `Mode` enum controls input handling:

```rust
enum Mode {
    Normal,
    Console { console: Box<dyn Console> },
    CreateItem { input: Input, is_dir: bool },
    Search { input: Input },
    Rename { input: Input },
}
```

- **Normal**: Commands parsed via CommandParser
- **Console**: Interactive cd/zoxide console
- **CreateItem/Search/Rename**: Input modes with InputBarWidget

## File Structure

```
src/panel/
├── compositor.rs      # Layer management
├── rect.rs            # Rect type for screen regions
├── widget.rs          # Widget trait and z_index constants
├── render.rs          # RenderContext helper
├── widgets/
│   ├── mod.rs
│   ├── panel_widget.rs   # DirPanelWidget, PreviewPanelWidget
│   ├── header.rs         # HeaderWidget
│   ├── footer.rs         # FooterWidget
│   ├── input_bar.rs      # InputBarWidget
│   └── log.rs            # LogWidget
├── manager.rs         # Event loop and state management
├── directory.rs       # DirPanel (unchanged)
├── preview.rs         # PreviewPanel (unchanged)
├── console.rs         # Console modes (unchanged)
└── input.rs           # Text input handling (unchanged)
```

## Adding New UI Elements

To add a new overlay (e.g., help screen):

1. Create widget in `src/panel/widgets/help.rs`
2. Add to `widgets/mod.rs`
3. Add layer ID field to PanelManager
4. Initialize layer (hidden) in `PanelManager::new()`
5. Show/hide layer when needed

```rust
// In PanelManager
self.compositor.show_layer(self.help_layer);
// ... later ...
self.compositor.hide_layer(self.help_layer);
```

## Design Principles

1. **Widgets own their rendering**: Each widget knows how to draw itself
2. **Compositor handles ordering**: Z-index determines draw order automatically
3. **Dirty tracking**: Only redraw what changed
4. **Incremental adoption**: Old code can coexist with new widgets
