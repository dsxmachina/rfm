# Freedesktop Trash Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the experimental tempdir trash with the freedesktop.org trash
(via the `trash` crate), integrate trash-delete into the undo stack, and add a
`gT` overlay Trash-View mode whose `r` restores an item to its original location.

**Architecture:** Delete routes through `trash::delete` (same-device, persistent,
interoperable). The just-trashed item is captured via `os_limited::list()` and
recorded as `FsChange::Trash { item, original }`; undo = `restore_all` (no_redo).
`gT` opens a pure `TrashView` modal adapter, populated by the manager from
`os_limited::list()`; it emits `ModeOp::RestoreFromTrash`.

**Tech Stack:** Rust, tokio, crossterm, `trash` crate. Tests: `cargo test`;
TUI e2e via tmux + debug socket (CLAUDE.md).

Design doc: `docs/plans/2026-07-18-freedesktop-trash-design.md`.

Key facts verified: `trash = ">=5.1.1, <5.2"` builds on rustc 1.83 (5.2 needs
1.85); native FS backend, **no DBus** in the tree. `TrashItem { id: OsString,
name: OsString, original_parent: PathBuf, time_deleted: i64 }` is `Clone + Eq`.
`trash::delete(path) -> Result<()>`, `os_limited::list() -> Result<Vec<TrashItem>>`,
`os_limited::restore_all<I: IntoIterator<Item=TrashItem>>(items) -> Result<()>`.

---

## Task 1: Add the dependency

**Files:** Modify `Cargo.toml` (dependencies, alphabetical near `toml`).

**Step 1:** Add:
```toml
# freedesktop trash (native FS backend, no DBus). 5.2 bumps MSRV to 1.85; cap below it.
trash = ">=5.1.1, <5.2"
```

**Step 2:** `cargo build` → PASS (resolves trash 5.1.1).

**Step 3:** Verify no DBus pulled in:
`cargo tree 2>/dev/null | grep -iE "dbus|zbus|ashpd|glib" || echo clean` → `clean`.

**Step 4: Commit** — `build: add the freedesktop `trash` crate (native FS, no DBus)`

---

## Task 2: `FsChange::Trash` + undo/redo

**Files:** Modify `src/undo/mod.rs` (FsChange enum ~15-24, undo/redo ~27-45, imports).

**Step 1: Write the failing test** (in the `mod tests` block):
```rust
#[test]
fn trash_change_undo_restores_original() {
    // Isolate the home trash so we never touch the developer's real trash.
    let home = tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", home.path());

    let work = tempdir().unwrap();
    let file = work.path().join("victim.txt");
    fs::write(&file, "data").unwrap();

    trash::delete(&file).unwrap();
    assert!(!file.exists());

    // Capture the just-trashed item.
    let item = trash::os_limited::list()
        .unwrap()
        .into_iter()
        .find(|i| i.original_parent == work.path() && i.name == "victim.txt")
        .expect("item is in the trash");

    let change = FsChange::Trash { item, original: file.clone() };
    change.undo().unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "data");
}
```

**Step 2:** `cargo test undo::tests::trash_change_undo_restores` → FAIL (no variant).

**Step 3: Implement.** Add to `FsChange`:
```rust
/// A file moved to the freedesktop trash. Undo restores it to `original`.
Trash { item: trash::TrashItem, original: PathBuf },
```
In `undo()`:
```rust
FsChange::Trash { item, .. } => {
    trash::os_limited::restore_all([item.clone()])?;
    Ok(())
}
```
In `redo()` (trash transactions are `no_redo`, so this is only for totality):
```rust
FsChange::Trash { .. } => Ok(()),
```
Note: `restore_all` errors with `RestoreCollision` if `original` is occupied;
that surfaces as our normal undo-abort (logged, kept on the stack).

**Step 4:** `cargo test undo::` → PASS.

**Step 5: Commit** — `feat(undo): FsChange::Trash — undo restores via the freedesktop trash`

---

## Task 3: `use_trash` defaults to true

**Files:** Modify `src/config.rs:20-29` (GeneralConfig), `src/main.rs:162`,
`examples/config.toml`, `examples/keys.toml`.

**Step 1:** `src/config.rs` — add a default fn and attribute:
```rust
fn default_true() -> bool { true }
```
```rust
#[serde(default = "default_true")]
pub use_trash: bool,
```

**Step 2:** `src/main.rs:162` — flip the no-config fallback:
```rust
let mut use_trash = true;
```

**Step 3:** `examples/config.toml` — set `use_trash = true` and replace the
"only safe on a single disk" caveat with a note that the freedesktop trash is
per-device (cheap everywhere) and persistent; deletes are undoable, permanent
deletes (`use_trash = false`) are a non-undoable barrier.

**Step 4:** `examples/keys.toml` — update the `view_trash` comment:
`# open the trash view (r restores an item to its original location)`.

**Step 5:** `cargo build && cargo test` → PASS.

**Step 6: Commit** — `feat(trash): default use_trash to true (delete is now cheap + undoable)`

---

## Task 4: `RestoreFromTrash` op + the `TrashView` adapter

**Files:** Modify `src/panel/mode/mod.rs` (ModeOp ~55-74, exports ~24-30);
Modify `src/panel/manager.rs` (`apply_mode_op` ~1140); Create
`src/panel/mode/trash_view.rs`.

**Step 1:** `mode/mod.rs` — add to `ModeOp`:
```rust
/// Restore the given trash items to their original location, then exit.
RestoreFromTrash { items: Vec<trash::TrashItem> },
```
and export the new mode:
```rust
mod trash_view;
pub use trash_view::TrashView;
```

**Step 2: Write the failing adapter tests** (`trash_view.rs` `mod tests`):
```rust
// helper to build a TrashView over N fake items requires TrashItem values;
// construct them via a small helper that trashes temp files, OR keep the
// adapter test focused on cursor/exit logic with an empty list + a stubbed
// item vec. Cursor logic (j/k bounds) and Esc→exit, r→RestoreFromTrash are
// the pure behaviours under test.
```
Concretely, test:
- `j`/`k` move the cursor and clamp at both ends (list of 3).
- `Esc` → `ModeOp::Exit { cleanup: Cleanup::None }`.
- `r` on a non-empty list → `ModeOp::RestoreFromTrash { items: vec![current] }`.
- `r` on an empty list → `ModeOp::None`.

(Build `TrashItem`s for the test by trashing temp files under an isolated
`XDG_DATA_HOME`, as in Task 2, then `os_limited::list()`.)

**Step 3: Implement `TrashView`:**
```rust
//! The trash overlay (`gT`): a read-only list of freedesktop trash items;
//! `r` restores the selected item to its original location. Pure adapter —
//! the manager supplies the item list (from os_limited::list) and applies
//! the restore.
use crossterm::event::{KeyCode, KeyEvent};
use super::{Cleanup, ModalInput, ModalRegion, ModeOp};
use crate::panel::*;

pub struct TrashView {
    items: Vec<trash::TrashItem>,
    cursor: usize,
}

impl TrashView {
    pub fn new(items: Vec<trash::TrashItem>) -> Self {
        Self { items, cursor: 0 }
    }
}

impl Draw for TrashView {
    fn draw(&mut self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        // Centered overlay: a title line, then one row per item —
        //   "<name>    <original_parent>/<name>    <deletion date>"
        // highlight the row at `cursor`. Empty list → "(trash is empty)".
        // Model the frame on console.rs's overlay drawing.
        // ... (draw implementation)
        Ok(())
    }
}

impl ModalInput for TrashView {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        match key_event.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.cursor + 1 < self.items.len() { self.cursor += 1; }
                ModeOp::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                ModeOp::None
            }
            KeyCode::Char('r') => match self.items.get(self.cursor) {
                Some(item) => ModeOp::RestoreFromTrash { items: vec![item.clone()] },
                None => ModeOp::None,
            },
            KeyCode::Esc | KeyCode::Enter => ModeOp::Exit { cleanup: Cleanup::None },
            _ => ModeOp::None,
        }
    }
    fn region(&self) -> ModalRegion { ModalRegion::ConsoleOverlay }
    fn name(&self) -> &'static str { "trash" }
}
```

**Step 4:** `apply_mode_op` (manager.rs) — add the arm:
```rust
ModeOp::RestoreFromTrash { items } => {
    let n = items.len();
    match trash::os_limited::restore_all(items) {
        Ok(()) => info!("wiederhergestellt: {n} Element(e) aus dem Papierkorb"),
        Err(e) => error!("Wiederherstellen fehlgeschlagen: {e}"),
    }
    self.mode = Mode::Normal;
    self.left.reload();
    self.center.reload();
    self.right.reload();
}
```
(Restore from the view is intentionally NOT pushed to the undo stack in v1.)

**Step 5:** `cargo build && cargo test mode::trash_view` → PASS.

**Step 6: Commit** — `feat(trash): TrashView overlay mode + RestoreFromTrash op`

---

## Task 5: Route delete through the trash crate; wire `gT`

**Files:** Modify `src/panel/manager.rs` — struct field (`trash_dir` ~116),
`new()` (~171-196), imports (~11 `TempDir`, ~14-22), `delete_file` (~709),
`Command::Delete` (~1345), `Command::ViewTrash` (~1269).

**Step 1:** Replace the field `trash_dir: Option<TempDir>` with `use_trash: bool`.
Remove `use tempfile::TempDir;`. In `new()`, delete the tempdir-creation block
and just store `use_trash`.

**Step 2:** Rewrite `delete_file` to use the crate + capture the item:
```rust
/// Deletes a file. With the trash on, moves it to the freedesktop trash and
/// returns the recorded change (undoable); otherwise deletes permanently and
/// returns None.
fn delete_file(&self, file: &Path) -> Option<FsChange> {
    if self.use_trash {
        if let Err(e) = trash::delete(file) {
            error!("Cannot trash {}: {e}", file.display());
            return None;
        }
        match capture_trashed(file) {
            Some(item) => Some(FsChange::Trash { item, original: file.to_path_buf() }),
            None => {
                warn!("trashed {} but could not locate it for undo", file.display());
                None
            }
        }
    } else {
        if file.is_file() {
            if let Err(e) = std::fs::remove_file(file) { error!("Cannot delete {}: {e}", file.display()); }
        } else if file.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(file) { error!("Cannot delete {}: {e}", file.display()); }
        }
        None
    }
}
```
Add a free helper (below `delete_file`):
```rust
/// Finds the trash item that `trash::delete(original)` just created: match by
/// original parent + name, newest deletion first.
fn capture_trashed(original: &Path) -> Option<trash::TrashItem> {
    let parent = original.parent()?;
    let name = original.file_name()?;
    let mut hits: Vec<trash::TrashItem> = trash::os_limited::list()
        .ok()?
        .into_iter()
        .filter(|i| i.original_parent == parent && i.name == name)
        .collect();
    hits.sort_by_key(|i| i.time_deleted);
    hits.pop() // greatest time_deleted
}
```

**Step 3:** `Command::Delete` — mark the transaction `no_redo` (re-trashing would
stale the stored item). Change the trash branch condition from
`self.trash_dir.is_some()` to `self.use_trash`, and:
```rust
if self.use_trash {
    let mut tx = Transaction::new(format!("delete ({} items)", files.len()));
    for file in files {
        if let Some(change) = self.delete_file(&file) { tx.push(change); }
    }
    self.undo.record(tx.no_redo());
} else {
    for file in files { self.delete_file(&file); }
    self.undo.barrier("permanentes Löschen");
}
```

**Step 4:** `Command::ViewTrash` — open the overlay instead of jumping:
```rust
Command::ViewTrash => {
    if self.use_trash {
        let items = trash::os_limited::list().unwrap_or_default();
        self.mode = Mode::Modal(Box::new(TrashView::new(items)));
    } else {
        warn!("Trash is disabled (use_trash = false) — nothing to show.");
    }
}
```
Add `TrashView` to the `use super::mode::{...}` import (manager.rs ~43).

**Step 5:** `cargo build && cargo test` → PASS. Fix any remaining references to
`trash_dir` (there should be none left).

**Step 6: Commit** — `feat(trash): delete via freedesktop trash; gT opens the trash view`

---

## Task 6: Integration test + docs + final verification

**Files:** Modify `CLAUDE.md` (trash paragraph); no code.

**Step 1: e2e (tmux + socket).** Isolated config with `use_trash = true` and an
isolated `XDG_DATA_HOME` (export it in the tmux pane before launching rfm so the
test never touches the real trash). Fixture `a.txt b.txt c.txt`. Verify:
- delete selected → leaves the pane; `state.undo_depth == 1`.
- `u` (undo) → file is back; log shows "rückgängig: delete".
- delete again → `gT` opens the trash view (mode == "trash"); `r` → file
  restored to the fixture dir; view closes.
Drive with `await-idle`; read `state`/`entries`; use `-l` for literal keyword
input (a bare `"delete"` sends the Delete key — see the undo work).

**Step 2:** Update `CLAUDE.md`'s trash/undo notes: freedesktop trash (per-device,
persistent), `gT` opens the TrashView (`r` restores), `use_trash` default true,
trash-delete is `FsChange::Trash` (restore_all, no_redo).

**Step 3:** `cargo test` (all green), `cargo clippy` (no new warnings from the
trash/trash_view code).

**Step 4: Commit** — `docs: record the freedesktop trash + trash-view in CLAUDE.md`

---

## Notes / edge cases

- **Env in unit tests:** the trash unit tests set `XDG_DATA_HOME` (process-global).
  Only the real-trashing tests read it, and there are few; if flakiness appears
  under parallelism, run those with `--test-threads=1` or gate them.
- **Restore collision:** `restore_all` errors if the original path is re-occupied.
  In undo it aborts+logs (kept on stack); in the view it logs an error. No clobber.
- **Empty trash view:** draws "(trash is empty)"; `r` is a no-op there.
- **Per-device visibility:** `os_limited::list()` aggregates home + top-directory
  trashes, so the view shows items regardless of which disk they were on.
- **Deferred (v1 out of scope):** empty-trash/`purge_all` + confirmation,
  restore-to-arbitrary (old dd/pp pull-out), multi-select, undoing a restore.
