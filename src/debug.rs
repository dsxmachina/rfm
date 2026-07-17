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
    /// 0-based index of the selection among *visible* entries; do not use
    /// it to index the `entries` reply, which lists all elements
    /// (including hidden ones)
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
    State {
        reply: oneshot::Sender<StateSnapshot>,
    },
    AwaitIdle {
        reply: oneshot::Sender<u64>,
    },
    Entries {
        pane: PaneId,
        reply: oneshot::Sender<Vec<EntryInfo>>,
    },
}

/// A parsed line-protocol command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    State,
    AwaitIdle,
    Entries(PaneId),
}

pub fn parse_command(line: &str) -> Option<DebugCommand> {
    let mut words = line.split_whitespace();
    match (words.next()?, words.next()) {
        ("state", None) => Some(DebugCommand::State),
        ("await-idle", None) => Some(DebugCommand::AwaitIdle),
        ("entries", Some("left")) => Some(DebugCommand::Entries(PaneId::Left)),
        ("entries", Some("center")) => Some(DebugCommand::Entries(PaneId::Center)),
        _ => None,
    }
}

/// Accept loop for the debug socket. Spawned once from main when
/// `--debug-socket` is given. One command per connection.
pub async fn serve(socket_path: PathBuf, request_tx: mpsc::Sender<DebugRequest>) -> Result<()> {
    // Remove a stale socket file from a previous (crashed) run
    let _ = std::fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(e) => {
            error!(
                "debug socket: failed to bind {}: {e}",
                socket_path.display()
            );
            return Err(e.into());
        }
    };
    info!("debug socket listening on {}", socket_path.display());
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(handle_connection(stream, request_tx.clone()));
            }
            Err(e) => error!("debug socket: accept failed: {e}"),
        }
    }
}

async fn handle_connection(stream: UnixStream, request_tx: mpsc::Sender<DebugRequest>) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let line = match lines.next_line().await {
        Ok(Some(line)) => line,
        _ => return,
    };
    let response = process_line(&line, &request_tx).await;
    let _ = write_half.write_all(response.as_bytes()).await;
    let _ = write_half.write_all(b"\n").await;
}

async fn process_line(line: &str, request_tx: &mpsc::Sender<DebugRequest>) -> String {
    const MANAGER_GONE: &str = r#"{"error":"panel manager is gone"}"#;
    match parse_command(line) {
        Some(DebugCommand::State) => {
            let (tx, rx) = oneshot::channel();
            if request_tx
                .send(DebugRequest::State { reply: tx })
                .await
                .is_err()
            {
                return MANAGER_GONE.into();
            }
            match timeout(Duration::from_secs(10), rx).await {
                Ok(Ok(snapshot)) => serde_json::to_string(&snapshot)
                    .unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#)),
                _ => r#"{"error":"timeout: event loop did not respond within 10s"}"#.into(),
            }
        }
        Some(DebugCommand::AwaitIdle) => {
            let (tx, rx) = oneshot::channel();
            if request_tx
                .send(DebugRequest::AwaitIdle { reply: tx })
                .await
                .is_err()
            {
                return MANAGER_GONE.into();
            }
            match timeout(Duration::from_secs(30), rx).await {
                Ok(Ok(seq)) => format!(r#"{{"idle":true,"seq":{seq}}}"#),
                _ => r#"{"idle":false,"error":"timeout: not idle within 30s"}"#.into(),
            }
        }
        Some(DebugCommand::Entries(pane)) => {
            let (tx, rx) = oneshot::channel();
            if request_tx
                .send(DebugRequest::Entries { pane, reply: tx })
                .await
                .is_err()
            {
                return MANAGER_GONE.into();
            }
            match timeout(Duration::from_secs(10), rx).await {
                Ok(Ok(entries)) => serde_json::to_string(&entries)
                    .unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#)),
                _ => r#"{"error":"timeout: event loop did not respond within 10s"}"#.into(),
            }
        }
        None => {
            r#"{"error":"unknown command (try: state | await-idle | entries left|center)"}"#.into()
        }
    }
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

    #[test]
    fn parses_commands() {
        assert!(matches!(parse_command("state"), Some(DebugCommand::State)));
        assert!(matches!(
            parse_command("await-idle"),
            Some(DebugCommand::AwaitIdle)
        ));
        assert!(matches!(
            parse_command("entries left"),
            Some(DebugCommand::Entries(PaneId::Left))
        ));
        assert!(matches!(
            parse_command("entries center"),
            Some(DebugCommand::Entries(PaneId::Center))
        ));
        // whitespace tolerance
        assert!(matches!(
            parse_command("  state "),
            Some(DebugCommand::State)
        ));
        // unknown input
        assert!(parse_command("entries right").is_none());
        assert!(parse_command("bogus").is_none());
        assert!(parse_command("").is_none());
    }

    async fn fake_manager(mut rx: mpsc::Receiver<DebugRequest>) {
        while let Some(req) = rx.recv().await {
            match req {
                DebugRequest::State { reply } => {
                    let _ = reply.send(StateSnapshot {
                        seq: 7,
                        mode: "normal".into(),
                        cwd: "/tmp".into(),
                        selection: None,
                        selected_idx: 0,
                        total: 0,
                        marked: vec![],
                        clipboard: None,
                        show_hidden: false,
                        left_path: "/".into(),
                        preview_path: "/tmp".into(),
                        queue_active: None,
                        queue_len: 0,
                    });
                }
                DebugRequest::AwaitIdle { reply } => {
                    let _ = reply.send(7);
                }
                DebugRequest::Entries { reply, .. } => {
                    let _ = reply.send(vec![EntryInfo {
                        name: "a.txt".into(),
                        marked: false,
                        hidden: false,
                        selected: true,
                    }]);
                }
            }
        }
    }

    async fn query(socket: &std::path::Path, cmd: &str) -> String {
        let mut stream = UnixStream::connect(socket).await.unwrap();
        stream.write_all(cmd.as_bytes()).await.unwrap();
        stream.write_all(b"\n").await.unwrap();
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply).await.unwrap();
        reply
    }

    #[tokio::test]
    async fn serves_commands_over_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("rfm-debug.sock");
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(fake_manager(rx));
        tokio::spawn(serve(socket.clone(), tx));

        // Wait for the socket file to appear
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let reply = query(&socket, "state").await;
        assert!(reply.contains("\"seq\":7"), "unexpected reply: {reply}");

        let reply = query(&socket, "await-idle").await;
        assert!(reply.contains("\"idle\":true"), "unexpected reply: {reply}");

        let reply = query(&socket, "entries center").await;
        assert!(
            reply.contains("\"name\":\"a.txt\""),
            "unexpected reply: {reply}"
        );

        let reply = query(&socket, "bogus").await;
        assert!(reply.contains("error"), "unexpected reply: {reply}");
    }
}
