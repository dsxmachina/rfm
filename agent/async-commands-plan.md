# Async Commands Implementation Plan

## Status: COMPLETE

## Overview

Add user-defined shell commands that execute asynchronously in a background queue, keeping the UI responsive.

## Design Decisions

- **Config format**: Inline tables in `[commands]` section of config.toml
- **Placeholder**: `$@` expands to marked_or_selected paths
- **Separator**: Space-separated by default, configurable per-command
- **Interactive flag**: `interactive = true` runs in foreground, suspends rfm
- **Output handling**: Log stdout/stderr to existing log system
- **Queue behavior**: Continue on error, process sequentially
- **Refresh**: File watchers handle directory updates automatically

## Config Example

```toml
[commands]
compress = { keys = ["zz"], cmd = "tar -czvf archive.tar.gz $@" }
extract_here = { keys = ["zx"], cmd = "tar -xzvf $@" }
open_vim = { keys = ["ev"], cmd = "vim $@", interactive = true }
null_safe = { keys = ["zd"], cmd = "xargs -0 rm", separator = "\0" }
```

## Architecture

```
src/command_queue/
├── mod.rs          # Public interface
├── executor.rs     # Background task that processes queue
└── types.rs        # QueuedCommand, QueueStatus, CommandConfigEntry
```

## Tasks

- [x] Task 1: Create command_queue module with types
- [x] Task 2: Implement CommandExecutor async task
- [x] Task 3: Add UserCommand config parsing
- [x] Task 4: Integrate UserCommand into Command enum and CommandParser
- [x] Task 5: Handle interactive commands (suspend/resume terminal)
- [x] Task 6: Wire up CommandExecutor in main.rs
- [x] Task 7: Add placeholder expansion ($@ → paths)
- [x] Task 8: Test and verify

## Implementation Summary

### New Files
- `src/command_queue/mod.rs` - Module exports
- `src/command_queue/types.rs` - QueuedCommand, QueueStatus, CommandConfigEntry, UserCommandConfig
- `src/command_queue/executor.rs` - CommandExecutor async task

### Modified Files
- `src/main.rs` - Wire up CommandExecutor, load user commands from config
- `src/config.rs` - Add `commands` field to Config struct
- `src/engine/commands.rs` - Add `UserCommand` variant, `add_user_commands()` method
- `src/panel/manager.rs` - Handle UserCommand, expand $@ placeholder
- `Cargo.toml` - Add shell-escape dependency

## Verification

- **Tests:** 20 passed, 0 failed
- **Clippy:** OK (only dead code warnings for future widget infrastructure)
- **Release build:** Success

## Future Work (Widget)

The infrastructure is ready for a UI widget in the bottom right corner:
- `QueueStatus` struct with `active` and `queued_count` fields
- `watch::Sender<QueueStatus>` channel for status updates
- `is_active()` helper method
