# StyleEngine Implementation Plan

## Goal
Replace `SymbolEngine` with a unified `StyleEngine` that returns both symbols AND colors based on mime-type.

## Design Decisions
- **Fallback approach**: Try mime-type color first, fall back to grey for regular files
- **Config**: Hardcoded defaults + user overrides via TOML
- **Unified engine**: Single `StyleEngine` returning `FileStyle { symbol, color }`
- **Named colors only**: Match crossterm's Color enum (e.g., `Cyan`, `DarkMagenta`)
- **Nested config**: `[styles.image]`, `[styles."text/markdown"]` format

## Implementation Steps

### 1. Create `src/engine/styles.rs`
- Define `FileStyle` struct with `symbol: &'static str` and `color: Option<Color>`
- Create `StyleEngine` with patricia tree mapping mime-types to `FileStyle`
- Implement hardcoded defaults with colors:
  - `image/` → Cyan
  - `audio/` → Magenta
  - `video/` → Yellow
  - `text/markdown` → Blue
  - `application/pdf` → Red
  - `text/x-toml` → DarkCyan

### 2. Add config parsing
- Add `StyleConfig` to `src/config.rs`
- Parse `[styles.*]` sections from config.toml
- Merge user overrides on top of defaults

### 3. Refactor `DirElem::print_styled()`
- Call `StyleEngine::get_style(path)` instead of `SymbolEngine::get_symbol(path)`
- Use `style.color.unwrap_or(Color::Grey)` for regular files
- Keep existing logic for directories (main color) and executables (green)

### 4. Update module exports
- Replace `pub use symbols::SymbolEngine` with `pub use styles::StyleEngine`
- Update `main.rs` to initialize `StyleEngine`

### 5. Remove old `symbols.rs`
- Delete the file after migration is complete

## Files to Modify
- `src/engine/styles.rs` (new)
- `src/engine.rs` (update exports)
- `src/config.rs` (add StyleConfig)
- `src/panel/directory.rs` (use StyleEngine)
- `src/main.rs` (init StyleEngine, pass config)
- `examples/config.toml` (add styles section)

## Progress Tracking
- [x] Create StyleEngine with FileStyle struct
- [x] Add hardcoded default colors
- [x] Add config parsing for style overrides
- [x] Refactor DirElem to use StyleEngine
- [x] Remove old SymbolEngine
- [x] Test and verify

## Completed: 2026-02-15

All tasks completed successfully. The StyleEngine now provides both symbols and colors based on mime-type, with user-configurable overrides via the `[styles.*]` sections in config.toml.
