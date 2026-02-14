# PanelManager Refactoring Plan

## Problem Statement

The current `src/panel/manager.rs` (1193 lines) combines:
1. Event loop handling
2. Application state management
3. Manual rendering logic with boolean flags
4. Mode-specific input handling

This entanglement makes it hard to:
- Add new UI elements (modals, confirmation dialogs)
- Control render ordering (z-index)
- Test components in isolation
- Understand the rendering flow

## Goals

1. **Separation of Concerns**: Event loop, state, and rendering should be distinct
2. **Z-Index Support**: Layers that determine render order without manual tracking
3. **Window/Modal Support**: Simple overlays for confirmations, help screens, etc.
4. **Maintain Simplicity**: No external TUI library, keep it lightweight
5. **Non-Breaking**: Incremental refactoring with working tests at each step

## Current Architecture Analysis

### What We Have Now

```
PanelManager {
    // Panels
    left: ManagedPanel<DirPanel>
    center: ManagedPanel<DirPanel>
    right: ManagedPanel<PreviewPanel>

    // Manual redraw flags
    redraw: Redraw {
        left: bool,
        center: bool,
        right: bool,
        console: bool,
        log: bool,
        header: bool,
        footer: bool,
    }

    // Mode = implicit UI layer
    mode: Mode { Normal, Console, CreateItem, Search, Rename }
}
```

### Rendering Flow (Current)

```
draw()
  ├─ draw_footer()   // always at bottom
  ├─ draw_header()   // always at top
  ├─ draw_panels()   // left, center, right
  ├─ draw_console()  // overlays panels if Mode::Console
  └─ draw_log()      // overlays panels if show_log
```

Problems:
- Z-ordering is implicit in the method call sequence
- Each `draw_*` method checks its own boolean flag
- Adding a modal requires adding another boolean + method + check

## Proposed Architecture

### Core Concepts

#### 1. Widget Trait (extends existing Draw)

```rust
pub trait Widget {
    /// Render the widget to the given area
    fn render(&mut self, ctx: &mut RenderContext, area: Rect);

    /// Whether this widget needs to be redrawn
    fn needs_redraw(&self) -> bool;

    /// Mark widget as needing redraw
    fn mark_dirty(&mut self);

    /// Mark widget as clean (just rendered)
    fn mark_clean(&mut self);

    /// Z-index for layering (higher = on top)
    fn z_index(&self) -> u8 { 0 }

    /// Whether this widget blocks input to layers below
    fn is_modal(&self) -> bool { false }
}
```

#### 2. RenderContext

```rust
pub struct RenderContext<'a> {
    stdout: &'a mut Stdout,
    screen_size: (u16, u16),
    // Damage tracking - areas that need redraw
    dirty_regions: Vec<Rect>,
}

impl RenderContext {
    pub fn queue(&mut self, ...) -> Result<()>;
    pub fn clear_region(&mut self, area: Rect) -> Result<()>;
}
```

#### 3. Layer System

```rust
pub struct Layer {
    widget: Box<dyn Widget>,
    area: Rect,
    visible: bool,
    z_index: u8,
}

pub struct Compositor {
    layers: Vec<Layer>,
}

impl Compositor {
    pub fn add_layer(&mut self, widget: Box<dyn Widget>, area: Rect, z_index: u8);
    pub fn remove_layer(&mut self, id: LayerId);
    pub fn render(&mut self, ctx: &mut RenderContext);
    pub fn mark_dirty(&mut self, layer_id: LayerId);
    pub fn mark_all_dirty(&mut self);
}
```

### New Rendering Flow

```
Compositor::render()
  ├─ Sort layers by z_index
  ├─ For each visible layer:
  │   ├─ If layer.needs_redraw() or overlapping dirty region:
  │   │   ├─ Clear layer area
  │   │   └─ layer.widget.render(ctx, layer.area)
  │   └─ layer.mark_clean()
  └─ Flush stdout
```

### Layer Z-Index Assignments

| Layer | Z-Index | Purpose |
|-------|---------|---------|
| Header | 10 | Top bar with path |
| Footer | 10 | Bottom bar with metadata |
| Left Panel | 20 | Parent directory |
| Center Panel | 20 | Current directory |
| Right Panel | 20 | Preview |
| Log Overlay | 30 | Warning/error messages |
| Console | 40 | cd/zoxide input |
| Modal/Dialog | 50 | Confirmations, help |

### Event Handling Separation

```rust
pub struct App {
    // State
    state: AppState,

    // UI composition
    compositor: Compositor,

    // Event handling
    mode: Mode,
}

pub struct AppState {
    // Panel management
    left: ManagedPanel<DirPanel>,
    center: ManagedPanel<DirPanel>,
    right: ManagedPanel<PreviewPanel>,

    // Navigation
    fwd_history: Vec<(PathBuf, PathBuf)>,
    rev_history: Vec<PathBuf>,

    // Clipboard, settings, etc.
    clipboard: Option<Clipboard>,
    show_hidden: bool,
}

impl App {
    pub fn handle_event(&mut self, event: Event) -> Option<CloseCmd> {
        // Dispatch based on mode
        match &mut self.mode {
            Mode::Normal => self.handle_normal_event(event),
            Mode::Console { console } => self.handle_console_event(event),
            // ...
        }
    }

    pub fn update_ui(&mut self) {
        // Update compositor layers based on state changes
        // Mark appropriate layers dirty
    }
}
```

## Implementation Phases

### Phase 1: Introduce Rect and RenderContext (Low Risk) - IN PROGRESS
- [x] Create `src/panel/rect.rs` with `Rect` struct
- [x] Create `src/panel/render.rs` with `RenderContext`
- [x] Add `draw_rect` method to Draw trait (backward compatible)
- [x] Add Rect-based accessors to MillerColumns
- [ ] Gradually migrate draw methods to use Rect (optional, can do as needed)
- [x] Tests pass, behavior unchanged

### Phase 2: Create Widget Trait (Low Risk) - COMPLETE
- [x] Define `Widget` trait in `src/panel/widget.rs`
- [x] Add z_index constants module
- [x] Add DirtyWrapper for existing panel types
- [x] Add BoxWidget for modal dialogs
- [x] Keep existing draw() working alongside Widget

### Phase 3: Implement Compositor (Medium Risk) - IN PROGRESS
- [x] Create `src/panel/compositor.rs`
- [x] Implement Layer management with z-index ordering
- [x] Add dirty region tracking
- [ ] Create panel wrapper widgets
- [ ] Integrate compositor with PanelManager
- [ ] Remove `Redraw` struct

### Phase 4: Separate Event Handling (Medium Risk)
- [ ] Create `src/app.rs` for App struct
- [ ] Move state from PanelManager to AppState
- [ ] Move event handling to App::handle_event
- [ ] PanelManager becomes thin wrapper or removed

### Phase 5: Add Modal Support (Low Risk)
- [ ] Create modal widget types
- [ ] Add show_modal() / hide_modal() to App
- [ ] Implement confirmation dialog
- [ ] Test with delete confirmation

## File Structure After Refactor

```
src/
├── app.rs              # App state and event handling
├── panel/
│   ├── mod.rs          # Existing traits + new Widget
│   ├── rect.rs         # Rect, Point types
│   ├── render.rs       # RenderContext
│   ├── widget.rs       # Widget trait
│   ├── compositor.rs   # Layer management
│   ├── widgets/
│   │   ├── mod.rs
│   │   ├── header.rs   # Header widget
│   │   ├── footer.rs   # Footer widget
│   │   ├── panel.rs    # Panel wrapper widget
│   │   └── modal.rs    # Modal/dialog widgets
│   ├── directory.rs    # (existing)
│   ├── preview.rs      # (existing)
│   └── input.rs        # (existing)
└── ...
```

## Progress Tracking

### Current Status: Phase 1

| Task | Status | Notes |
|------|--------|-------|
| Create Rect type | Pending | |
| Create RenderContext | Pending | |
| Convert draw methods | Pending | |

## Risk Mitigation

1. **Incremental Changes**: Each phase produces working code
2. **Test After Each Step**: Run `cargo test` and manual testing
3. **Git Commits**: Commit after each successful change
4. **Rollback Ready**: Keep original code until new code proven

## Open Questions

1. Should Widget own its area or receive it during render?
   - **Decision**: Receive during render (more flexible)

2. How to handle async panel updates with compositor?
   - **Decision**: Update through App, mark layer dirty

3. Should Console be a Widget or Mode-specific logic?
   - **Decision**: Widget (allows multiple overlays)
