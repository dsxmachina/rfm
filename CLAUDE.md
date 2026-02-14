# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rfm** is a fast terminal file manager written in Rust, inspired by ranger. It uses Miller columns (three-panel layout) with VI-style keybindings. Published as `rfm-bin` on crates.io.

## Build & Development Commands

```bash
# Build (release)
cargo build --release

# Run tests
cargo test

# Run the application
cargo run -- [PATH]

# Install locally
cargo install --path .
```

The Nix flake provides a development shell with additional tools (clippy, rustfmt, cargo-machete, cargo-bloat):
```bash
nix develop
```

## Manual Testing with tmux

Since rfm is an interactive TUI, use tmux to run and verify changes:

```bash
# Start rfm in a detached tmux session
tmux new-session -d -s rfm-test -x 120 -y 35
tmux send-keys -t rfm-test '/path/to/rfm /some/directory' Enter
sleep 1

# Capture the screen to see the UI
tmux capture-pane -t rfm-test -p

# Send keystrokes to navigate
tmux send-keys -t rfm-test 'j'      # move down
tmux send-keys -t rfm-test 'k'      # move up
tmux send-keys -t rfm-test 'l'      # enter directory
tmux send-keys -t rfm-test 'h'      # go to parent

# Capture with ANSI color codes (for verifying colors)
tmux capture-pane -t rfm-test -e -p | od -c | head -50

# Clean up
tmux send-keys -t rfm-test 'q'
tmux kill-session -t rfm-test
```

Color codes in the capture: `033[38;5;Nm` where N is the 256-color index (e.g., 226 for yellow, 2 for green, 4 for blue).

## Architecture

### Module Structure

```
src/
├── main.rs              # Entry point, config loading, startup
├── config.rs            # Color and general settings
├── content.rs           # DirManager & PreviewManager (async content loaders with caching)
├── logger.rs            # Log buffering
├── util.rs              # File operations, display utilities
├── engine/
│   ├── commands.rs      # Keyboard input → Command enum parsing
│   ├── opener.rs        # MIME-type based file opening
│   └── symbols.rs       # File type icons/symbols
└── panel/
    ├── mod.rs           # Traits: Draw, PanelContent, BasePanel
    ├── manager.rs       # Main event loop and state management (core of the app)
    ├── directory.rs     # DirPanel - directory listing display
    ├── preview.rs       # PreviewPanel - file preview generation
    ├── console.rs       # DirConsole & ZoxideConsole (cd modes)
    └── input.rs         # Text input handling
```

### Key Design Patterns

- **Event-driven async architecture**: Uses Tokio runtime with crossterm's EventStream
- **Three-panel Miller columns**: Left (parent dir), Center (current dir), Right (preview)
- **Content caching**: DirManager and PreviewManager use LRU caches to avoid regenerating content
- **Channel-based updates**: PanelUpdate messages trigger async content loading

### Critical Files

- `panel/manager.rs` - The main event loop; understand this first when making changes
- `engine/commands.rs` - Maps keyboard sequences to Command enum variants
- `content.rs` - Async content loading and caching infrastructure

### Configuration

Config files are embedded via `rust-embed` from `examples/` and copied to `~/.config/rfm/` on first run:
- `config.toml` - Colors, general behavior
- `keys.toml` - Keyboard bindings, jump marks
- `open.toml` - File opener rules by MIME type/extension

### External Tool Integration

Optional external tools for enhanced previews:
- `bat` - Syntax highlighting in text previews
- `mediainfo` - Audio/video file metadata
- `tar`/`zip` - Archive previews
