# Undo-Stack Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an in-session undo/redo stack to rfm that reverses the reversible
file operations (rename, move, copy, mkdir, touch, zip, tar, trash-delete,
bulk-rename); permanent deletes become non-reversible barriers, external
commands are ignored.

**Architecture:** The whole scope collapses to three atomic reversible
filesystem changes — `Create`, `Move`, `Copy`. One user action = one
`Transaction` of N changes. The FS primitives return the change they made
(carrying the *actually-reached* path, so the `_`-suffix collision fallback is
handled); the command handlers in `manager.rs` (the central effect layer)
assemble transactions and push them onto a terminal-free `UndoStack`. Undo/redo
are new `Command`s applied without re-recording.

**Tech Stack:** Rust, tokio, crossterm. Tests: `cargo test`; TUI integration via
the tmux + debug-socket harness (see CLAUDE.md).

Design doc: `docs/plans/2026-07-18-undo-stack-design.md`.

---

## Key decisions baked into this plan

- **In-memory, per session.** No serialization.
- **Undo + Redo.** Redo replays the same atomic changes forward. Exception:
  zip/tar create a *content* file we cannot reproduce from an atomic `Create`
  (that would yield an empty archive), so their transaction is marked
  `redoable = false` — undoable (delete the archive), but not redoable.
- **Permanent delete → Barrier.** Trash-delete is a `Move` and is undoable.
- **External / out-of-scope (extract, UserCommand, open) → ignored** (untracked).
- **Undo failure → abort at first failing change**, push the transaction back
  onto the undo stack, do not record redo.
- **`_`-suffix on restore:** the inverse uses `rename_safe` (tries the exact
  target, appends `_` only if occupied) — never a blind overwrite.

---

## Task 1: The undo module (`src/undo/mod.rs`)

**Files:**
- Create: `src/undo/mod.rs`
- Modify: `src/main.rs:32-39` (add `mod undo;` in the module list)

**Step 1: Write the failing tests** (append to the new file)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn move_change_round_trips() {
        let d = tempdir().unwrap();
        let from = d.path().join("a.txt");
        let to = d.path().join("b.txt");
        fs::write(&from, "x").unwrap();
        fs::rename(&from, &to).unwrap();
        let c = FsChange::Move { from: from.clone(), to: to.clone() };
        c.undo().unwrap();
        assert!(from.exists() && !to.exists());
        c.redo().unwrap();
        assert!(to.exists() && !from.exists());
    }

    #[test]
    fn create_undo_removes_file_and_dir() {
        let d = tempdir().unwrap();
        let f = d.path().join("new.txt");
        fs::write(&f, "").unwrap();
        FsChange::Create { path: f.clone(), is_dir: false }.undo().unwrap();
        assert!(!f.exists());
        let sub = d.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner"), "").unwrap();
        FsChange::Create { path: sub.clone(), is_dir: true }.undo().unwrap();
        assert!(!sub.exists());
    }

    #[test]
    fn copy_undo_deletes_target_redo_recreates() {
        let d = tempdir().unwrap();
        let from = d.path().join("src.txt");
        let to = d.path().join("dst.txt");
        fs::write(&from, "data").unwrap();
        fs::copy(&from, &to).unwrap();
        let c = FsChange::Copy { from: from.clone(), to: to.clone() };
        c.undo().unwrap();
        assert!(from.exists() && !to.exists());
        c.redo().unwrap();
        assert_eq!(fs::read_to_string(&to).unwrap(), "data");
    }

    #[test]
    fn stack_records_undoes_and_redoes() {
        let d = tempdir().unwrap();
        let from = d.path().join("a");
        let to = d.path().join("b");
        fs::write(&from, "x").unwrap();
        fs::rename(&from, &to).unwrap();
        let mut s = UndoStack::new();
        let mut tx = Transaction::new("rename");
        tx.push(FsChange::Move { from: from.clone(), to: to.clone() });
        s.record(tx);
        assert_eq!(s.undo_depth(), 1);
        assert!(matches!(s.undo(), UndoOutcome::Done(_)));
        assert!(from.exists());
        assert_eq!(s.redo_depth(), 1);
        assert!(matches!(s.redo(), UndoOutcome::Done(_)));
        assert!(to.exists());
    }

    #[test]
    fn barrier_blocks_undo() {
        let mut s = UndoStack::new();
        s.barrier("permanentes Löschen");
        assert!(matches!(s.undo(), UndoOutcome::Blocked(_)));
        assert_eq!(s.undo_depth(), 1); // barrier stays on the stack
    }

    #[test]
    fn empty_transaction_is_not_recorded() {
        let mut s = UndoStack::new();
        s.record(Transaction::new("noop"));
        assert_eq!(s.undo_depth(), 0);
    }

    #[test]
    fn recording_clears_redo() {
        let d = tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        fs::write(&a, "x").unwrap();
        fs::rename(&a, &b).unwrap();
        let mut s = UndoStack::new();
        let mut tx = Transaction::new("m");
        tx.push(FsChange::Move { from: a.clone(), to: b.clone() });
        s.record(tx);
        s.undo();
        assert_eq!(s.redo_depth(), 1);
        let mut tx2 = Transaction::new("c");
        tx2.push(FsChange::Create { path: d.path().join("z"), is_dir: false });
        fs::write(d.path().join("z"), "").unwrap();
        s.record(tx2);
        assert_eq!(s.redo_depth(), 0);
    }
}
```

**Step 2: Run to verify it fails** — `cargo test undo::` → FAIL (module missing).

**Step 3: Write the implementation** (top of `src/undo/mod.rs`)

```rust
//! In-session undo/redo stack.
//!
//! Every reversible file operation maps to a `Transaction` of atomic
//! `FsChange`s. The stack is terminal-free and unit-testable; the manager
//! assembles transactions and applies undo/redo without re-recording.

use std::path::{Path, PathBuf};

use anyhow::Result;
use fs_extra::dir::CopyOptions;

use crate::util::rename_safe;

/// An atomic, reversible filesystem change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsChange {
    /// A path was created (mkdir/touch/zip/tar). Undo deletes it.
    Create { path: PathBuf, is_dir: bool },
    /// A path moved from → to (rename/move/trash). `to` is the reached path.
    Move { from: PathBuf, to: PathBuf },
    /// `from` was copied to `to`. Undo deletes `to`; `from` is untouched.
    Copy { from: PathBuf, to: PathBuf },
}

impl FsChange {
    /// Apply the inverse of this change.
    pub fn undo(&self) -> Result<()> {
        match self {
            FsChange::Create { path, .. } => remove_path(path),
            FsChange::Move { from, to } => restore(to, from),
            FsChange::Copy { to, .. } => remove_path(to),
        }
    }

    /// Re-apply this change forward.
    pub fn redo(&self) -> Result<()> {
        match self {
            FsChange::Create { path, is_dir } => create_empty(path, *is_dir),
            FsChange::Move { from, to } => restore(from, to),
            FsChange::Copy { from, to } => copy_exact(from, to),
        }
    }
}

/// Remove a file or directory tree. Missing target is treated as success.
fn remove_path(p: &Path) -> Result<()> {
    let res = if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };
    match res {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Move `src` to `dst`, appending `_` only if `dst` is occupied (never clobber).
fn restore(src: &Path, dst: &Path) -> Result<()> {
    rename_safe(src, dst)?;
    Ok(())
}

fn create_empty(path: &Path, is_dir: bool) -> Result<()> {
    if is_dir {
        std::fs::create_dir_all(path)?;
    } else {
        use std::fs::OpenOptions;
        OpenOptions::new().write(true).create(true).open(path)?;
    }
    Ok(())
}

fn copy_exact(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs_extra::dir::copy(from, to, &CopyOptions::default().copy_inside(true))?;
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// One user action's worth of changes — the unit of undo/redo.
#[derive(Debug, Clone)]
pub struct Transaction {
    pub label: String,
    pub changes: Vec<FsChange>,
    pub redoable: bool,
}

impl Transaction {
    pub fn new(label: impl Into<String>) -> Self {
        Transaction { label: label.into(), changes: Vec::new(), redoable: true }
    }
    pub fn push(&mut self, change: FsChange) {
        self.changes.push(change);
    }
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
    /// Mark as undoable-but-not-redoable (zip/tar: content can't be replayed).
    pub fn no_redo(mut self) -> Self {
        self.redoable = false;
        self
    }
}

enum UndoEntry {
    Tx(Transaction),
    Barrier { reason: String },
}

/// The outcome of a single undo/redo, for user feedback.
pub enum UndoOutcome {
    Done(String),
    Empty,
    Blocked(String),
    Failed(String),
}

#[derive(Default)]
pub struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<Transaction>,
}

impl UndoStack {
    pub fn new() -> Self {
        UndoStack::default()
    }

    pub fn record(&mut self, tx: Transaction) {
        if tx.is_empty() {
            return;
        }
        self.redo.clear();
        self.undo.push(UndoEntry::Tx(tx));
    }

    pub fn barrier(&mut self, reason: impl Into<String>) {
        self.redo.clear();
        self.undo.push(UndoEntry::Barrier { reason: reason.into() });
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    pub fn undo(&mut self) -> UndoOutcome {
        match self.undo.last() {
            None => return UndoOutcome::Empty,
            Some(UndoEntry::Barrier { reason }) => {
                return UndoOutcome::Blocked(reason.clone())
            }
            Some(UndoEntry::Tx(_)) => {}
        }
        let UndoEntry::Tx(tx) = self.undo.pop().unwrap() else { unreachable!() };
        // Apply inverses in reverse order; abort at the first failure.
        let mut i = tx.changes.len();
        while i > 0 {
            i -= 1;
            if let Err(e) = tx.changes[i].undo() {
                let msg = e.to_string();
                self.undo.push(UndoEntry::Tx(tx));
                return UndoOutcome::Failed(msg);
            }
        }
        let label = tx.label.clone();
        if tx.redoable {
            self.redo.push(tx);
        }
        UndoOutcome::Done(label)
    }

    pub fn redo(&mut self) -> UndoOutcome {
        let Some(tx) = self.redo.pop() else {
            return UndoOutcome::Empty;
        };
        for i in 0..tx.changes.len() {
            if let Err(e) = tx.changes[i].redo() {
                let msg = e.to_string();
                self.redo.push(tx);
                return UndoOutcome::Failed(msg);
            }
        }
        let label = tx.label.clone();
        self.undo.push(UndoEntry::Tx(tx));
        UndoOutcome::Done(label)
    }
}
```

**Step 4:** `cargo test undo::` → PASS.

**Step 5: Commit** — `feat(undo): atomic FsChange model + UndoStack (undo/redo/barrier)`

---

## Task 2: FS primitives return the change they made

**Files:**
- Modify: `src/util.rs:212-246` (`move_item`, `copy_item`)

**Step 1:** Change `move_item` to return `anyhow::Result<Option<FsChange>>`
(`None` for the identical-path no-op) and `copy_item` to return
`anyhow::Result<FsChange>`.

```rust
use crate::undo::FsChange;

pub fn move_item<P, Q>(source: P, destination: Q) -> anyhow::Result<Option<FsChange>>
where P: AsRef<Path>, Q: AsRef<Path>,
{
    let from = source.as_ref();
    let dest_name = from.file_name().and_then(|p| p.to_str())
        .map(|s| s.to_string()).unwrap_or_default();
    if from == destination.as_ref().join(&dest_name) {
        warn!("from and to are identical");
        return Ok(None);
    }
    let to = get_destination(&source, destination)?;
    std::fs::rename(from, &to)?;
    Ok(Some(FsChange::Move { from: from.to_path_buf(), to }))
}

pub fn copy_item<P, Q>(source: P, destination: Q) -> anyhow::Result<FsChange>
where P: AsRef<Path>, Q: AsRef<Path>,
{
    let from = source.as_ref();
    let to = get_destination(&source, destination)?;
    if from.is_dir() {
        fs_extra::dir::copy(from, &to, &CopyOptions::default().copy_inside(true))?;
    } else {
        std::fs::copy(from, &to)?;
    }
    Ok(FsChange::Copy { from: from.to_path_buf(), to })
}
```

**Step 2:** `cargo build` → will FAIL at the `Paste` call sites in
`manager.rs` (they ignore the old `()`); that is expected and fixed in Task 5.
For now confirm only util/undo compile in isolation is not possible, so proceed;
the build is green again after Task 5. **Commit after Task 5, not here** — but
still verify `cargo build 2>&1 | grep -c "move_item\|copy_item"` shows the errors
are confined to the paste site.

*(Note: keep Task 2 + Task 5 in one commit since they must land together.)*

---

## Task 3: Manager owns the stack + a transaction channel

**Files:**
- Modify: `src/panel/manager.rs` — struct (71-141), `new()` (143-210), imports
  (14-22), the `run()` select loop (1051-1119)

**Step 1:** Import and add fields.

- Import: `use crate::undo::{FsChange, Transaction, UndoStack, UndoOutcome};`
- Replace the commented `// stack: Vec<Operation>` (manager.rs:89-90) with:
  ```rust
  /// Undo/redo stack
  undo: UndoStack,
  /// Sender used by the async paste task to hand back its transaction
  undo_tx: mpsc::UnboundedSender<Transaction>,
  /// Receiver for transactions produced off-thread (paste)
  undo_rx: mpsc::UnboundedReceiver<Transaction>,
  ```

**Step 2:** In `new()`, before the `Ok(PanelManager { ... })`:
```rust
let (undo_tx, undo_rx) = mpsc::unbounded_channel();
```
and in the struct literal add `undo: UndoStack::new(), undo_tx, undo_rx,`
(replace the `// stack: Vec::new(),` line).

**Step 3:** Add a select branch in `run()` (alongside the other `recv()`
branches, before the debug branch):
```rust
// Transactions handed back by the async paste task
result = self.undo_rx.recv() => {
    if let Some(tx) = result {
        self.undo.record(tx);
        self.left.reload();
        self.center.reload();
        self.right.reload();
        self.mark_dirty();
    }
}
```

**Step 4:** `cargo build` → still failing on paste return types (Task 5).
No commit yet.

---

## Task 4: Record rename and create

**Files:**
- Modify: `src/panel/manager.rs` — `apply_rename` (1183-1201), `apply_create`
  (1209-1229)

**Step 1 (rename):** In `apply_rename`, on the successful branch record a Move.
The current code refuses when `to_path.exists()`, so the reached path is exactly
`to_path`:
```rust
} else if let Err(e) = std::fs::rename(from, &to_path) {
    error!("{e}");
} else {
    let mut tx = Transaction::new(format!(
        "rename {} → {}",
        from.file_name().unwrap_or_default().to_string_lossy(),
        to
    ));
    tx.push(FsChange::Move { from: from.to_path_buf(), to: to_path.clone() });
    self.undo.record(tx);
}
```
(Requires borrowing `from` before the move; capture `from.to_path_buf()` up top
if the borrow checker complains.)

**Step 2 (create):** In `apply_create`, wrap the success in a transaction:
```rust
let target = current_path.join(name.trim());
if let Err(e) = create_fn(target.clone()) {
    error!("{e}");
} else {
    let mut tx = Transaction::new(format!("create {}", name.trim()));
    tx.push(FsChange::Create { path: target, is_dir });
    self.undo.record(tx);
}
```

**Step 3:** `cargo build` — still blocked by paste (Task 5). Verify no *new*
errors in `apply_rename`/`apply_create` via
`cargo build 2>&1 | grep -A2 apply_rename`.

---

## Task 5: Record paste (async channel) — unblocks the build

**Files:**
- Modify: `src/panel/manager.rs` — `Command::Paste` arm (1357-1382)

**Step 1:** Rewrite the blocking task to build a transaction and send it:
```rust
Command::Paste { overwrite } => {
    self.unmark_all_items();
    let current_path = self.center.panel().path().to_path_buf();
    let clipboard = self.clipboard.take();
    let undo_tx = self.undo_tx.clone();
    tokio::task::spawn_blocking(move || {
        if let Some(clipboard) = clipboard {
            info!("paste {} items, overwrite = {}", clipboard.files.len(), overwrite);
            let mut tx = Transaction::new(format!("paste ({} items)", clipboard.files.len()));
            for file in clipboard.files.iter() {
                if clipboard.cut {
                    match move_item(file, &current_path) {
                        Ok(Some(change)) => tx.push(change),
                        Ok(None) => {}
                        Err(e) => error!("Failed to move {}: {e}", file.display()),
                    }
                } else {
                    match copy_item(file, &current_path) {
                        Ok(change) => tx.push(change),
                        Err(e) => error!("Failed to copy {}: {e}", file.display()),
                    }
                }
            }
            let _ = undo_tx.send(tx); // recorded + reload on the main loop
        }
    });
    self.left.reload();
    self.center.reload();
    self.right.reload();
}
```
(The `overwrite` flag is unused today — behaviour unchanged; leave as-is.)

**Step 2:** `cargo build` → PASS. `cargo test` → PASS.

**Step 3: Commit** — Tasks 2–5 together:
`feat(undo): record rename/create/move/copy transactions`

---

## Task 6: Record delete (Move-transaction or Barrier)

**Files:**
- Modify: `src/panel/manager.rs` — `delete_file` (709-730), `Command::Delete`
  arm (1345-1356)

**Step 1:** Make `delete_file` return the change (or `None` for permanent):
```rust
fn delete_file(&self, file: &Path) -> Option<FsChange> {
    if let Some(trash_path) = &self.trash_dir {
        let destination = get_destination(file, trash_path.path()).unwrap();
        match std::fs::rename(file, &destination) {
            Ok(()) => Some(FsChange::Move { from: file.to_path_buf(), to: destination }),
            Err(e) => { error!("Cannot delete {}: {e}", file.display()); None }
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

**Step 2:** In `Command::Delete`, collect or set a barrier:
```rust
Command::Delete => {
    let files = self.marked_or_selected();
    info!("Deleted {} items", files.len());
    self.unmark_all_items();
    if self.trash_dir.is_some() {
        let mut tx = Transaction::new(format!("delete ({} items)", files.len()));
        for file in files {
            if let Some(change) = self.delete_file(&file) { tx.push(change); }
        }
        self.undo.record(tx);
    } else {
        for file in files { self.delete_file(&file); }
        self.undo.barrier("permanentes Löschen");
    }
    self.left.reload();
    self.center.reload();
    self.right.reload();
}
```

**Step 3:** `cargo build && cargo test` → PASS.
**Step 4: Commit** — `feat(undo): trash-delete undoable, permanent delete is a barrier`

---

## Task 7: Record zip/tar (undoable, non-redoable)

**Files:**
- Modify: `src/engine/opener.rs:261-296` (`zip`, `tar` return the archive path)
- Modify: `src/panel/manager.rs` — `Command::Zip` (1383-1391), `Command::Tar`
  (1392-1400)

**Step 1:** Change `zip`/`tar` return type from `Result<()>` to
`Result<PathBuf>`, returning `archive_path` (currently `check_filename("output",
".", ...)` → a `./output.zip`-style relative path).

**Step 2:** In the handlers, resolve the archive against the panel path and
record a non-redoable `Create`:
```rust
Command::Zip => {
    let items = self.marked_or_selected();
    let dir = self.center.panel().path().to_path_buf();
    if let Err(e) = std::env::set_current_dir(&dir) {
        error!("Failed to set working-directory for process: {e}");
    }
    match self.opener.zip(items) {
        Ok(rel) => {
            let path = dir.join(rel.file_name().unwrap_or_default());
            let mut tx = Transaction::new("zip");
            tx.push(FsChange::Create { path, is_dir: false });
            self.undo.record(tx.no_redo());
        }
        Err(e) => warn!("Failed to create zip-archive: {e}"),
    }
}
```
Same shape for `Command::Tar` (label `"tar"`).

**Step 3:** `cargo build && cargo test` → PASS.
**Step 4: Commit** — `feat(undo): zip/tar archive creation is undoable (delete), non-redoable`

---

## Task 8: Record bulk-rename

**Files:**
- Modify: `src/panel/manager.rs` — `bulkrename` (886-957)

**Step 1:** Thread the *original* path through the final-rename bookkeeping so a
transaction can record `original → actual`. Change `final_renames` to carry the
original path:
```rust
// (PathBuf from, PathBuf to, String new_name, PathBuf original)
let mut final_renames: Vec<(PathBuf, PathBuf, String, PathBuf)> = Vec::new();
```
Push `files[i].clone()` as the 4th element in both the swap and non-swap arms
(the swap arm's `from` is the temp path; the original is still `files[i]`).

**Step 2:** Build a transaction as final renames succeed:
```rust
let mut tx = Transaction::new(format!("bulk rename ({} files)", final_renames.len()));
let mut success_count = 0;
for (from, to, new_name, original) in &final_renames {
    match rename_safe(from, to) {
        Ok(actual_path) => {
            if actual_path != *to {
                info!("Renamed '{}' -> '{}' (adjusted due to conflict)", from.display(), actual_path.display());
            }
            tx.push(FsChange::Move { from: original.clone(), to: actual_path });
            success_count += 1;
        }
        Err(e) => error!("Failed to rename to '{}': {e}", new_name),
    }
}
self.undo.record(tx);
info!("Bulkrename: successfully renamed {} files", success_count);
return;
```

**Step 3:** `cargo build && cargo test` → PASS.
**Step 4: Commit** — `feat(undo): record bulk-rename as one transaction`

---

## Task 9: Undo/Redo commands, keybindings, apply

**Files:**
- Modify: `src/engine/commands.rs` — `Command` enum (126-164), `Display`
  (166-213), `Manipulation` struct (48-63), `from_config` (293-309),
  `default_bindings` (463-471)
- Modify: `src/panel/manager.rs` — new arms in the Normal-mode match
  (near 1345), new `apply_undo`/`apply_redo` helpers
- Modify: `examples/keys.toml` — document `undo`/`redo`

**Step 1:** Add `Undo` and `Redo` to `Command`; add `Display` arms
(`"undo"` / `"redo"`).

**Step 2:** Config: add `undo: Option<Vec<String>>` and `redo: Option<Vec<String>>`
to `Manipulation`; in `from_config`:
```rust
parser.insert(config.manipulation.undo.unwrap_or_default(), Command::Undo);
parser.insert(config.manipulation.redo.unwrap_or_default(), Command::Redo);
```
In `default_bindings`:
```rust
key_commands.insert("u", Command::Undo);
// ctrl-r
mod_commands.insert(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL), Command::Redo);
```

**Step 3:** Manager arms (after `Command::Delete`):
```rust
Command::Undo => self.apply_undo(),
Command::Redo => self.apply_redo(),
```
Helpers:
```rust
fn apply_undo(&mut self) {
    match self.undo.undo() {
        UndoOutcome::Done(label) => info!("rückgängig: {label}"),
        UndoOutcome::Empty => info!("nichts rückgängig zu machen"),
        UndoOutcome::Blocked(r) => warn!("kann nicht rückgängig gemacht werden: {r}"),
        UndoOutcome::Failed(e) => error!("undo fehlgeschlagen: {e}"),
    }
    self.left.reload();
    self.center.reload();
    self.right.reload();
    self.mark_dirty();
}

fn apply_redo(&mut self) {
    match self.undo.redo() {
        UndoOutcome::Done(label) => info!("wiederhergestellt: {label}"),
        UndoOutcome::Empty => info!("nichts wiederherzustellen"),
        UndoOutcome::Blocked(r) => warn!("redo blockiert: {r}"),
        UndoOutcome::Failed(e) => error!("redo fehlgeschlagen: {e}"),
    }
    self.left.reload();
    self.center.reload();
    self.right.reload();
    self.mark_dirty();
}
```

**Step 4:** `examples/keys.toml` — under `[manipulation]` add:
```toml
undo = [ "u" ]
redo = [ "ctrl-r" ]
```

**Step 5:** `cargo build && cargo test` → PASS.
**Step 6: Commit** — `feat(undo): u / ctrl-r keybindings to undo and redo`

---

## Task 10: Expose depths on the debug socket + integration test

**Files:**
- Modify: `src/debug.rs:22` (`StateSnapshot` struct — add `undo_depth`,
  `redo_depth`)
- Modify: `src/panel/manager.rs` — `state_snapshot` (1013-1044)

**Step 1:** Add `pub undo_depth: usize,` and `pub redo_depth: usize,` to
`StateSnapshot` (and any serialization there); populate in `state_snapshot`:
```rust
undo_depth: self.undo.undo_depth(),
redo_depth: self.undo.redo_depth(),
```

**Step 2:** `cargo build && cargo test` → PASS.

**Step 3: Manual integration test** (harness from CLAUDE.md). Fixture with
`a.txt`; rename it, assert `undo_depth == 1`; undo; assert the file name is back
and `redo_depth == 1`; redo; assert renamed again. Drive via tmux, synchronise
with `await-idle`, read `state`, diff `entries`.

**Step 4: Commit** — `feat(debug): expose undo_depth/redo_depth in state snapshot`

---

## Final verification

- `cargo test` — all green.
- `cargo clippy` — no new warnings from the undo module.
- Harness smoke test: rename → undo → redo round-trip; trash-delete
  (`use_trash = true` config) → undo restores; permanent delete → undo reports
  the barrier.
- Update `CLAUDE.md` architecture section with a short "undo" paragraph and the
  `undo_depth`/`redo_depth` socket fields. Commit.
```
