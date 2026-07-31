//! Debug/testing interface: a Unix socket exposing internal state.
//!
//! Enabled with `rfm --debug-socket <path>`. Protocol: one connection per
//! command, one line in
//! ("state" | "await-idle" | "entries [<tab>] left|center" | "log [n]"),
//! one JSON line out. Intended for development and agent-driven testing,
//! e.g. alongside `tmux send-keys` / `capture-pane`.
//!
//! `state` is tab-aware: it carries a `tabs` array (one [`TabSnapshot`] per
//! open tab), the `focused` tab index and the current `view` ("single" |
//! "split"). The scalar top-level fields (`cwd`, `selection`, `selected_idx`,
//! `total`, `marked`, `left_path`, `preview_path`) MIRROR the focused tab, so
//! existing single-tab scripts keep working.
//!
//! `entries` accepts an optional leading 0-based tab index:
//! `entries center` targets the focused tab, `entries 1 center` targets tab 1.

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

/// Per-tab view of a Miller-columns stack. One of these appears in
/// [`StateSnapshot::tabs`] for every open tab. The scalar top-level fields of
/// `StateSnapshot` mirror the focused tab's `TabSnapshot`.
#[derive(Debug, Clone, Serialize)]
pub struct TabSnapshot {
    /// Path shown in this tab's center panel
    pub cwd: PathBuf,
    /// File name of the current selection in this tab's center panel
    pub selection: Option<String>,
    /// 0-based index of the selection among *visible* entries
    pub selected_idx: usize,
    pub total: usize,
    /// Paths of all marked items in this tab's center panel
    pub marked: Vec<PathBuf>,
}

/// Snapshot of the PanelManager state, built inside the event loop.
#[derive(Debug, Clone, Serialize)]
pub struct StateSnapshot {
    /// Monotonic counter, incremented once per event-loop iteration
    pub seq: u64,
    /// Current input mode: normal | console | search | rename | mkdir | touch
    pub mode: String,
    /// View mode: "single" | "split"
    pub view: String,
    /// 0-based index of the focused tab within `tabs`
    pub focused: usize,
    /// One snapshot per open tab. The scalar fields below mirror `tabs[focused]`.
    pub tabs: Vec<TabSnapshot>,
    /// Path shown in the focused tab's center panel (mirrors `tabs[focused].cwd`)
    pub cwd: PathBuf,
    /// File name of the current selection in the focused tab's center panel
    pub selection: Option<String>,
    /// 0-based index of the selection among *visible* entries; do not use
    /// it to index the `entries` reply, which lists all elements
    /// (including hidden ones)
    pub selected_idx: usize,
    pub total: usize,
    /// Paths of all marked items in the focused tab's center panel
    pub marked: Vec<PathBuf>,
    pub clipboard: Option<ClipboardInfo>,
    pub show_hidden: bool,
    /// Path shown in the focused tab's left panel
    pub left_path: PathBuf,
    /// Path the focused tab's preview panel is showing
    pub preview_path: PathBuf,
    /// Currently running background command, if any
    pub queue_active: Option<String>,
    /// Number of queued background commands
    pub queue_len: usize,
    /// Number of entries on the undo stack (transactions + barriers)
    pub undo_depth: usize,
    /// Number of transactions available to redo
    pub redo_depth: usize,
    /// Session-only jump-marks: letter -> directory. Sorted for determinism.
    pub jump_marks: std::collections::BTreeMap<String, PathBuf>,
    /// Graphics protocol resolved for image previews at startup:
    /// "kitty" | "sixel" | "half-block"
    pub image_protocol: String,
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

/// One retained log line, with its age relative to the query.
#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    /// Log level: ERROR | WARN | INFO | DEBUG | TRACE
    pub level: String,
    /// Seconds since this line was logged
    pub age_secs: f64,
    pub message: String,
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
        /// 0-based tab index; `None` targets the focused tab.
        tab: Option<usize>,
        pane: PaneId,
        reply: oneshot::Sender<Vec<EntryInfo>>,
    },
    Log {
        count: Option<usize>,
        reply: oneshot::Sender<Vec<LogEntry>>,
    },
}

/// A parsed line-protocol command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    State,
    AwaitIdle,
    /// `entries [<tab>] left|center`. `tab` is a 0-based index; `None` targets
    /// the focused tab.
    Entries {
        tab: Option<usize>,
        pane: PaneId,
    },
    Log(Option<usize>),
}

fn parse_pane(word: &str) -> Option<PaneId> {
    match word {
        "left" => Some(PaneId::Left),
        "center" => Some(PaneId::Center),
        _ => None,
    }
}

pub fn parse_command(line: &str) -> Option<DebugCommand> {
    let mut words = line.split_whitespace();
    match (words.next()?, words.next()) {
        ("state", None) => Some(DebugCommand::State),
        ("await-idle", None) => Some(DebugCommand::AwaitIdle),
        // `entries <pane>` — focused tab.
        ("entries", Some(pane_or_tab)) => {
            if let Some(pane) = parse_pane(pane_or_tab) {
                // No trailing tokens allowed.
                return match words.next() {
                    None => Some(DebugCommand::Entries { tab: None, pane }),
                    Some(_) => None,
                };
            }
            // Otherwise the second word must be a 0-based tab index, followed
            // by the pane: `entries <tab> <pane>`.
            let tab: usize = pane_or_tab.parse().ok()?;
            let pane = parse_pane(words.next()?)?;
            match words.next() {
                None => Some(DebugCommand::Entries {
                    tab: Some(tab),
                    pane,
                }),
                Some(_) => None,
            }
        }
        ("log", None) => Some(DebugCommand::Log(None)),
        ("log", Some(count)) => count.parse().ok().map(|n| DebugCommand::Log(Some(n))),
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
        Some(DebugCommand::Entries { tab, pane }) => {
            let (tx, rx) = oneshot::channel();
            if request_tx
                .send(DebugRequest::Entries {
                    tab,
                    pane,
                    reply: tx,
                })
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
        Some(DebugCommand::Log(count)) => {
            let (tx, rx) = oneshot::channel();
            if request_tx
                .send(DebugRequest::Log { count, reply: tx })
                .await
                .is_err()
            {
                return MANAGER_GONE.into();
            }
            match timeout(Duration::from_secs(10), rx).await {
                Ok(Ok(lines)) => serde_json::to_string(&lines)
                    .unwrap_or_else(|e| format!(r#"{{"error":"serialize: {e}"}}"#)),
                _ => r#"{"error":"timeout: event loop did not respond within 10s"}"#.into(),
            }
        }
        None => {
            r#"{"error":"unknown command (try: state | await-idle | entries [<tab>] left|center | log [n])"}"#
                .into()
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
            view: "split".into(),
            focused: 1,
            tabs: vec![
                TabSnapshot {
                    cwd: "/tmp/one".into(),
                    selection: Some("x.txt".into()),
                    selected_idx: 0,
                    total: 2,
                    marked: vec![],
                },
                TabSnapshot {
                    cwd: "/tmp/fixture".into(),
                    selection: Some("b.txt".into()),
                    selected_idx: 1,
                    total: 3,
                    marked: vec!["/tmp/fixture/a.txt".into()],
                },
            ],
            // Scalar fields mirror the focused tab (index 1).
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
            undo_depth: 0,
            redo_depth: 0,
            jump_marks: std::collections::BTreeMap::new(),
            image_protocol: "half-block".into(),
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(json.contains("\"seq\":42"));
        assert!(json.contains("\"image_protocol\":\"half-block\""));
        assert!(json.contains("\"mode\":\"normal\""));
        // New tab-aware fields.
        assert!(json.contains("\"view\":\"split\""));
        assert!(json.contains("\"focused\":1"));
        assert!(json.contains("\"tabs\":["));
        assert!(
            json.contains("/tmp/one"),
            "tabs array missing tab 0: {json}"
        );
        // Backward-compat scalar fields still present, mirroring focused tab.
        assert!(json.contains("\"cwd\":\"/tmp/fixture\""));
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
        // `entries <pane>` -> focused tab (tab: None)
        assert!(matches!(
            parse_command("entries left"),
            Some(DebugCommand::Entries {
                tab: None,
                pane: PaneId::Left
            })
        ));
        assert!(matches!(
            parse_command("entries center"),
            Some(DebugCommand::Entries {
                tab: None,
                pane: PaneId::Center
            })
        ));
        // `entries <tab> <pane>` -> explicit 0-based tab index
        assert!(matches!(
            parse_command("entries 1 center"),
            Some(DebugCommand::Entries {
                tab: Some(1),
                pane: PaneId::Center
            })
        ));
        assert!(matches!(
            parse_command("entries 2 left"),
            Some(DebugCommand::Entries {
                tab: Some(2),
                pane: PaneId::Left
            })
        ));
        assert!(matches!(
            parse_command("entries 0 center"),
            Some(DebugCommand::Entries {
                tab: Some(0),
                pane: PaneId::Center
            })
        ));
        // whitespace tolerance
        assert!(matches!(
            parse_command("  state "),
            Some(DebugCommand::State)
        ));
        // unknown / malformed input
        assert!(parse_command("entries right").is_none());
        assert!(parse_command("entries 1 right").is_none());
        assert!(parse_command("entries x center").is_none());
        assert!(parse_command("entries 1").is_none());
        assert!(parse_command("entries center extra").is_none());
        assert!(parse_command("entries 1 center extra").is_none());
        assert!(parse_command("bogus").is_none());
        assert!(parse_command("").is_none());
    }

    #[test]
    fn parses_log_command() {
        assert!(matches!(
            parse_command("log"),
            Some(DebugCommand::Log(None))
        ));
        assert!(matches!(
            parse_command("log 50"),
            Some(DebugCommand::Log(Some(50)))
        ));
        assert!(parse_command("log abc").is_none());
        assert!(parse_command("log -5").is_none());
    }

    async fn fake_manager(mut rx: mpsc::Receiver<DebugRequest>) {
        while let Some(req) = rx.recv().await {
            match req {
                DebugRequest::State { reply } => {
                    let _ = reply.send(StateSnapshot {
                        seq: 7,
                        mode: "normal".into(),
                        view: "single".into(),
                        focused: 0,
                        tabs: vec![TabSnapshot {
                            cwd: "/tmp".into(),
                            selection: None,
                            selected_idx: 0,
                            total: 0,
                            marked: vec![],
                        }],
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
                        undo_depth: 0,
                        redo_depth: 0,
                        jump_marks: std::collections::BTreeMap::new(),
                        image_protocol: "half-block".into(),
                    });
                }
                DebugRequest::AwaitIdle { reply } => {
                    let _ = reply.send(7);
                }
                DebugRequest::Entries { tab, reply, .. } => {
                    // Echo the requested tab into the name so tests can verify
                    // the tab index survives the round-trip.
                    let name = match tab {
                        Some(n) => format!("tab{n}.txt"),
                        None => "a.txt".into(),
                    };
                    let _ = reply.send(vec![EntryInfo {
                        name,
                        marked: false,
                        hidden: false,
                        selected: true,
                    }]);
                }
                DebugRequest::Log { count, reply } => {
                    let _ = reply.send(vec![LogEntry {
                        level: "ERROR".into(),
                        age_secs: 2.5,
                        message: format!("fake failure (count={count:?})"),
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
        assert!(
            reply.contains("\"view\":\"single\"")
                && reply.contains("\"focused\":0")
                && reply.contains("\"tabs\":["),
            "state missing tab-aware fields: {reply}"
        );

        let reply = query(&socket, "await-idle").await;
        assert!(reply.contains("\"idle\":true"), "unexpected reply: {reply}");

        let reply = query(&socket, "entries center").await;
        assert!(
            reply.contains("\"name\":\"a.txt\""),
            "unexpected reply: {reply}"
        );

        // Targeted-tab entries carry the tab index through.
        let reply = query(&socket, "entries 1 center").await;
        assert!(
            reply.contains("\"name\":\"tab1.txt\""),
            "unexpected reply: {reply}"
        );

        let reply = query(&socket, "bogus").await;
        assert!(reply.contains("error"), "unexpected reply: {reply}");

        let reply = query(&socket, "log 5").await;
        assert!(
            reply.contains("\"level\":\"ERROR\"") && reply.contains("fake failure"),
            "unexpected reply: {reply}"
        );
    }
}
