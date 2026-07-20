# Changelog

All notable changes to rfm are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-07-20

A big feature release: multi-tab browsing with a split view, a full undo/redo
stack, integration with the freedesktop trash, jump-marks, mime-type styling and
custom commands.

> **Upgrading:** the new keybindings (tabs, undo/redo, jump-marks, trash view)
> are **opt-in** — existing `keys.toml` files won't have them until you add them.
> See the updated [`examples/keys.toml`](examples/keys.toml) for the full set, and
> the new [docs/](docs/) directory for details.

### Added

- **Tabs & split view** — up to four tabs, each with its own directory,
  selection and history. New vifm-style side-by-side split view. Keys: `gn` (new
  tab), `Tab` (cycle focus), `1`–`4` (focus tab N), `!` (toggle split), `q` /
  `ctrl-w` (close tab; closing the last one quits).
- **Undo / redo** — an in-session stack for almost every file operation. Keys:
  `u` (undo), `ctrl-r` (redo). Rename, create, move, copy, delete and bulk-rename
  are each recorded as one reversible transaction.
- **Freedesktop trash** — deletes now go to the freedesktop.org trash instead of a
  throwaway directory, so they persist across sessions and integrate with your
  desktop's trash. A built-in **trash view** (`gT`) lists trashed items and
  restores them (`r`); the view stays open so you can restore several in a row.
  Trash-deletes are undoable and redoable.
- **Jump-marks** — vim-style session bookmarks: `m<letter>` to set, `'<letter>`
  to jump back. Prefixes are configurable.
- **Mime-type styling** — files are colored and given symbols based on their
  mime-type (yazi-like defaults), fully configurable under `[styles]`. Optional
  Nerd Font icons via `fancy_icons`.
- **Custom shell commands** — bind your own commands to keys via `[commands]` in
  `config.toml`, with `$@` path expansion and background or foreground execution.
- **Bulk-rename** — rename multiple marked files at once in your editor, as a
  single undoable transaction.
- **Zoxide integration** — `cd` through zoxide, and visited directories are added
  to the zoxide database automatically.
- **`--config`** flag to override the configuration directory.
- Graceful handling of permission-denied directories (shown as `(no access)` /
  `?` instead of failing).

### Changed

- **Trash behaviour**: `use_trash` now defaults to `true` and uses the persistent
  freedesktop trash (previously an ephemeral temp directory that was wiped on
  exit). With `use_trash = false`, deletes are permanent and not undoable.
- **Documentation**: the README is now a concise landing page; the full guides
  moved into a new [`docs/`](docs/) directory (usage, configuration, and extra
  features / integrations).
- **Performance & rendering**: reworked to an event-driven single-dirty-bit
  render model with a cached image-preview resize, and rate-limited panel updates
  for smoother browsing of large directories.

### Fixed

- Hardened all trash operations against panics from the underlying trash crate.
- Undo no longer panics when a trashed entry has already been removed out of band.
- Corrected directory-listing cache validation to use modification time.
- Numerous rename / bulk-rename / backspace edge cases.
