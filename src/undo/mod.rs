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
        Transaction {
            label: label.into(),
            changes: Vec::new(),
            redoable: true,
        }
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
        self.undo.push(UndoEntry::Barrier {
            reason: reason.into(),
        });
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
            Some(UndoEntry::Barrier { reason }) => return UndoOutcome::Blocked(reason.clone()),
            Some(UndoEntry::Tx(_)) => {}
        }
        let UndoEntry::Tx(tx) = self.undo.pop().unwrap() else {
            unreachable!()
        };
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
        let c = FsChange::Move {
            from: from.clone(),
            to: to.clone(),
        };
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
        FsChange::Create {
            path: f.clone(),
            is_dir: false,
        }
        .undo()
        .unwrap();
        assert!(!f.exists());
        let sub = d.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("inner"), "").unwrap();
        FsChange::Create {
            path: sub.clone(),
            is_dir: true,
        }
        .undo()
        .unwrap();
        assert!(!sub.exists());
    }

    #[test]
    fn copy_undo_deletes_target_redo_recreates() {
        let d = tempdir().unwrap();
        let from = d.path().join("src.txt");
        let to = d.path().join("dst.txt");
        fs::write(&from, "data").unwrap();
        fs::copy(&from, &to).unwrap();
        let c = FsChange::Copy {
            from: from.clone(),
            to: to.clone(),
        };
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
        tx.push(FsChange::Move {
            from: from.clone(),
            to: to.clone(),
        });
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
        tx.push(FsChange::Move {
            from: a.clone(),
            to: b.clone(),
        });
        s.record(tx);
        s.undo();
        assert_eq!(s.redo_depth(), 1);
        let mut tx2 = Transaction::new("c");
        tx2.push(FsChange::Create {
            path: d.path().join("z"),
            is_dir: false,
        });
        fs::write(d.path().join("z"), "").unwrap();
        s.record(tx2);
        assert_eq!(s.redo_depth(), 0);
    }
}
