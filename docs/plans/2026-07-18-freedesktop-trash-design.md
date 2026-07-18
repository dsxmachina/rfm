# Freedesktop Trash — Design

Status: accepted (v1 scope), 2026-07-18
Supersedes the experimental tempdir trash (manager.rs `trash_dir: Option<TempDir>`).
Builds on the undo stack (`docs/plans/2026-07-18-undo-stack-design.md`).

## Motivation

The current trash is a single `tempfile::tempdir()` (in `$TMPDIR`, usually
`/tmp`); delete = `rename` into it. Two problems:

1. **Cross-device cost.** `rename(2)` is O(1) only *within* one filesystem.
   If the deleted file lives on a different mount than `/tmp`, the rename
   fails with `EXDEV` and `fs_extra` falls back to copy+delete —
   O(size), brutal for large files/trees.
2. **No persistence.** The tempdir is removed on exit, so "trashed" files
   are gone after the session. Not a real recycle bin.

The fix is the freedesktop.org Trash specification, which keeps the trash
**on the same device** as each deleted file (so delete stays a cheap
rename) and persists it in a standard, interoperable location.

## How the freedesktop trash works (recap)

A trash directory holds two subdirs:

```
Trash/
├── files/                 the actual trashed files (renamed here)
└── info/                  one <name>.trashinfo per item
```

`.trashinfo` is plain text: `[Trash Info]` + `Path=<original, url-encoded>`
+ `DeletionDate=<ISO-8601>`. Restore = read info → move `files/<name>`
back to `Path` → delete the info. Two-tier location:

- **Home trash:** `$XDG_DATA_HOME/Trash` (default `~/.local/share/Trash`),
  for files on the same device as `$HOME`.
- **Top-directory trash:** for a file on another mount, a trash on *that*
  device's mount point — `$topdir/.Trash/$uid` (sticky, admin-made) or the
  self-made fallback `$topdir/.Trash-$uid`. Keeps the rename same-device.

This is a **filesystem convention, not a desktop feature** — no daemon, no
DBus, no DE. It degrades gracefully to a well-organized private recycle bin
on headless boxes, and interoperates (gio/Nautilus/trash-cli) when present.

## Crate choice

Use the `trash` crate for the spec (rather than reimplementing topdir
discovery, name collisions, and the info format).

- **Version pin: `trash = ">=5.1.1, <5.2"`.** 5.2 raises MSRV to rustc
  1.85; the build host is 1.83 (source-tarball, no rustup). 5.1.1 builds on
  1.83.
- **Native FS backend, no DBus.** The crate has no portal/ashpd/zbus/glib
  feature at all; Linux deps are `libc`, `scopeguard`, `urlencoding`,
  `once_cell`, `log`, `chrono`. Verified the resolved tree contains no
  dbus/zbus/glib/ashpd. Headless-safe.

API used:
- `trash::delete<P: AsRef<Path>>(path) -> Result<(), Error>`
- `trash::os_limited::list() -> Result<Vec<TrashItem>, Error>` (all trashes)
- `trash::os_limited::restore_all<I: IntoIterator<Item=TrashItem>>(items) -> Result<(), Error>`
  (errors on `RestoreCollision` if the original path is occupied)
- `TrashItem { id: OsString /* path to the .trashinfo */, name: OsString,
  original_parent: PathBuf, time_deleted: i64 }`; original path =
  `original_parent.join(name)`.

## Decisions

- **`use_trash` default flips to `true`.** Delete is now cheap everywhere
  and undoable, so trashing is the safe default. (Existing configs that set
  `use_trash` explicitly are unaffected; only omission/no-config gets true.)
- **Undo integration:** trash-delete becomes a new atomic change
  `FsChange::Trash { item, original }`; undo = `restore_all([item])`, which
  per the freedesktop spec also removes the `.trashinfo` (the trash entry is
  gone, no dangling/duplicate entries). It **is redoable** (revised
  2026-07-18): redo re-trashes `original` and re-captures the *new*
  `TrashItem`, updating the stored change, so an arbitrarily long
  delete↔undo↔redo cycle stays consistent. This requires `FsChange::undo/redo`
  to take `&mut self`. (zip/tar remain `no_redo` — a re-run would produce an
  empty archive; delete has no such problem because the source file is back
  after undo.) Permanent delete (`use_trash = false`) stays a `Barrier`.
- **Trash browsing/restore = a dedicated overlay mode.** `gT` no longer
  jumps into a directory; it opens a Trash-View overlay (like the modals)
  listing `os_limited::list()` items with original path + deletion date.
  `r` restores the selected/marked item(s) to their original location.
- **Restore from the Trash-View is NOT recorded in the undo stack** (v1).
  It is an explicit action in a dedicated surface.

## Design

### 1. Delete path

`delete_file` (manager.rs) drops the tempdir branch:

- `use_trash = true` → `trash::delete(path)`, then identify the freshly
  created item: `os_limited::list()`, filter by `original_parent == path.parent()`
  and `name == path.file_name()`, take the one with the greatest
  `time_deleted`. Return `FsChange::Trash { item, original: path }`.
  (Uniqueness: at any instant a given original path has at most one live
  trash entry unless the same path was deleted, recreated, deleted again —
  the newest-`time_deleted` tiebreak picks the one we just made.)
- `use_trash = false` → permanent `remove_file`/`remove_dir_all`, return
  `None`; the handler pushes a `Barrier` (unchanged).

`Command::Delete` assembles the `Trash` changes into a transaction (as it
does today). The transaction is redoable (see the revised undo integration
above); `capture_trashed` lives in `src/undo/` so both the initial delete
and `Trash::redo` share it.

### 2. Undo model change

`FsChange` gains:

```rust
Trash { item: trash::TrashItem, original: PathBuf },
```

`FsChange::undo/redo` take `&mut self` (only `Trash::redo` mutates):
- `undo(&mut self)` → `trash::os_limited::restore_all([item.clone()])`. On
  `RestoreCollision` (original path re-occupied) the call errors; our
  existing undo semantics log it and keep the transaction on the stack.
- `redo(&mut self)` for `Trash` → `trash::delete(original)` (re-trash), then
  `capture_trashed(original)` to grab the *new* `TrashItem` and overwrite
  `self.item`. On failure, error out (undo/redo aborts + keeps the stack).
  `UndoStack::undo/redo` therefore pop into a `mut tx`.

The old `trash_dir: Option<TempDir>` field, its `new()` setup, and the
`FsChange::Move`-based trash-delete recording are removed.

### 3. Trash-View overlay mode

A new modal adapter `TrashView` implementing `ModalInput` (like
`SearchMode`/`DirConsole`), drawn as a centered overlay:

- **Construction (in the manager, keeping the adapter pure):** the
  `Command::ViewTrash` handler calls `os_limited::list()`, sorts by
  `time_deleted` (newest first), and passes the vec into
  `TrashView::new(items)`. The adapter never touches the filesystem.
- **State:** the item list + a cursor index (+ optional marks for
  multi-restore; single-select is fine for v1).
- **Keys:** `j`/`k` (and arrows) move the cursor; `r` restores the current
  (or marked) item(s); `Esc` closes. Each row shows `name`, original path,
  and a human deletion date.
- **Restore effect:** the adapter returns a new `ModeOp::RestoreFromTrash
  { items: Vec<TrashItem> }`. The manager applies it: `restore_all(items)`,
  log the outcome, reload panels, and — since the list changed — either
  refresh the view (re-`list()`) or close it. v1: **close the view after a
  restore** (simplest; reopen with `gT` to restore more).
- **Region:** either reuse `ModalRegion::ConsoleOverlay` with a taller draw
  area, or add `ModalRegion::Overlay`. Decide during implementation; the
  console overlay is the closest existing precedent.

`ModeOp` gains `RestoreFromTrash { items }`; `apply_mode_op` gets the arm.
The empty-trash case draws a single "(trash is empty)" line.

### 4. Config

- `GeneralConfig::use_trash` gets `#[serde(default = "default_true")]` so
  omission yields `true`; `main.rs`'s no-config fallback flips `false → true`.
- `examples/config.toml`: set `use_trash = true`, drop the "only safe on a
  single disk" caveat (no longer true), document the freedesktop trash +
  headless behavior.
- `examples/keys.toml`: `view_trash` comment updated ("open the trash
  view; `r` restores"). `gT` binding unchanged.

## Out of scope (v1)

- **Empty trash / purge** (`purge_all`) — later, with its own trash-view
  keybindings and a yes/no confirmation.
- **Restore-to-arbitrary-location** (the old `gT` + `dd`/`pp` "pull it out
  to here" flow) — later, as a trash-aware clipboard. v1 restores to the
  original location only.
- **Multi-select / per-item confirmation** in the trash view.
- **Undoing a restore.**

## Affected files

- `Cargo.toml` — add `trash = ">=5.1.1, <5.2"`.
- `src/undo/mod.rs` — `FsChange::Trash` variant + undo/redo arms.
- `src/panel/manager.rs` — `delete_file` (trash via crate + capture),
  `Command::Delete` (no_redo tx), drop `trash_dir`/tempdir setup,
  `Command::ViewTrash` (open TrashView), `apply_mode_op`
  (`RestoreFromTrash` arm).
- `src/panel/mode/` — new `trash_view.rs` adapter + `mod.rs` exports;
  `ModeOp::RestoreFromTrash`, maybe `ModalRegion::Overlay`.
- `src/config.rs` — `use_trash` serde default = true.
- `src/main.rs` — no-config fallback `use_trash = true`.
- `examples/config.toml`, `examples/keys.toml` — defaults + docs.

## Testing

- Unit: `FsChange::Trash` round-trip in a scratch `$XDG_DATA_HOME`
  (`trash::delete` a temp file → build the change → `undo()` restores it).
  Set `XDG_DATA_HOME` to a tempdir in the test to isolate the trash.
- Unit: `TrashView` adapter key handling (j/k moves cursor, `r` emits
  `RestoreFromTrash` with the selected item, Esc exits) — terminal-free,
  like the other mode adapters.
- Integration (tmux + socket): with `use_trash = true`, delete a file →
  it leaves the pane; `u` restores it (undo) AND, separately, `gT` shows it
  in the view and `r` restores it. Confirm cross-device is not required by
  pointing `XDG_DATA_HOME` at the same fs as the fixture.
