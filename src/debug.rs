//! Debug/testing interface: a Unix socket exposing internal state.
//!
//! Enabled with `rfm --debug-socket <path>`. Protocol: one connection per
//! command, one line in ("state" | "await-idle" | "entries <pane>"),
//! one JSON line out. Intended for development and agent-driven testing,
//! e.g. alongside `tmux send-keys` / `capture-pane`.

use std::{path::PathBuf, time::Duration};

use anyhow::Result;
use log::{error, info};
use serde::Serialize;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot},
    time::timeout,
};

/// Snapshot of the PanelManager state, built inside the event loop.
#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
    /// Monotonic counter, incremented once per event-loop iteration
    pub seq: u64,
    /// Current input mode: normal | console | search | rename | mkdir | touch
    pub mode: String,
    /// Path shown in the center panel
    pub cwd: PathBuf,
    /// File name of the current selection in the center panel
    pub selection: Option<String>,
    pub selected_idx: usize,
    pub total: usize,
    /// Paths of all marked items in the center panel
    pub marked: Vec<PathBuf>,
    pub clipboard: Option<ClipboardInfo>,
    pub show_hidden: bool,
    /// Path shown in the left panel
    pub left_path: PathBuf,
    /// Path the preview panel is showing
    pub preview_path: PathBuf,
    /// Currently running background command, if any
    pub queue_active: Option<String>,
    /// Number of queued background commands
    pub queue_len: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClipboardInfo {
    pub files: Vec<PathBuf>,
    /// "cut" or "copy"
    pub op: String,
}

/// One entry of a directory panel, as rfm believes it to be.
#[derive(Debug, Clone, Serialize)]
pub struct EntryInfo {
    pub name: String,
    pub marked: bool,
    pub hidden: bool,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneId {
    Left,
    Center,
}

/// Request forwarded into the PanelManager event loop.
pub enum DebugRequest {
    State { reply: oneshot::Sender<StateSnapshot> },
    AwaitIdle { reply: oneshot::Sender<u64> },
    Entries { pane: PaneId, reply: oneshot::Sender<Vec<EntryInfo>> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_serializes_to_json() {
        let snapshot = StateSnapshot {
            seq: 42,
            mode: "normal".into(),
            cwd: "/tmp/fixture".into(),
            selection: Some("b.txt".into()),
            selected_idx: 1,
            total: 3,
            marked: vec!["/tmp/fixture/a.txt".into()],
            clipboard: Some(ClipboardInfo {
                files: vec!["/tmp/fixture/a.txt".into()],
                op: "copy".into(),
            }),
            show_hidden: false,
            left_path: "/tmp".into(),
            preview_path: "/tmp/fixture/b.txt".into(),
            queue_active: None,
            queue_len: 0,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"seq\":42"));
        assert!(json.contains("\"mode\":\"normal\""));
        assert!(json.contains("\"selection\":\"b.txt\""));
        assert!(json.contains("\"op\":\"copy\""));
    }
}
