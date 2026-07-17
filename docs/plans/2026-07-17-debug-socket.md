# Debug Socket Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `rfm --debug-socket <path>` — a Unix-socket debug interface exposing internal state (`state`), a race-free synchronization primitive (`await-idle`), and the panel content rfm *believes* it renders (`entries`), so an agent driving rfm in tmux can observe and synchronize instead of sleeping and guessing.

**Architecture:** A new `src/debug.rs` module owns the socket listener (tokio `UnixListener`, one line-based command per connection) and the request/response types. The listener never touches app state directly: it forwards each command as a `DebugRequest` (carrying a `oneshot` reply channel) over an `mpsc` into the `PanelManager` event loop. The loop gains one new `tokio::select!` branch, placed **last under `biased;`** — so a debug request is only served when no terminal event, panel update, or log is pending. That ordering *is* the `await-idle` implementation: when the branch fires, the queue is drained by construction. Ground truth for visuals stays `tmux capture-pane`; the socket provides the other side of the diff (what rfm thinks its state is).

**Tech Stack:** tokio (`UnixListener`, `mpsc`, `oneshot`, `watch` — all already in deps), serde + serde_json (serde present, serde_json to be added), existing `QueueStatus` watch channel (currently dropped unused in `main.rs:287`).

**Design rationale (from brainstorming session):**
- Visual bugs (stale pane, spacing) are diagnosed by **diffing** `entries`/`state` (rfm's belief) against `capture-pane` (terminal reality). Belief == reality but screen wrong → render/flush bug. Belief also wrong → state/update bug.
- `state` replies time out after 10s with an explicit error if the event loop is wedged — a hang is itself a diagnostic result, not a dead end.
- Known caveat (accepted, YAGNI): `await-idle` covers the event queue, **not** async panel loads still in flight inside `DirManager`/`PreviewManager`. If a pane shows a loading placeholder, poll `state` until `seq` stabilizes. A future `await-settled` can close this gap if it hurts in practice.

---

## Task 1: Add serde_json dependency

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add the dependency**

In `Cargo.toml` under `[dependencies]` (alphabetical, after `serde`):

```toml
serde_json = "1.0"
```

**Step 2: Verify it builds**

Run: `cargo build`
Expected: compiles without errors.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add serde_json dependency for debug socket"
```

---

## Task 2: Debug module — types and JSON serialization

**Files:**
- Create: `src/debug.rs`
- Modify: `src/main.rs` (add `mod debug;` next to the other `mod` declarations)

**Step 1: Write the failing test**

Create `src/debug.rs` with types and a serialization test (test first — the types are what make it pass):

```rust
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
```

Add to `src/main.rs` next to the other module declarations (`mod config;` etc.):

```rust
mod debug;
```

**Step 2: Run the test**

Run: `cargo test debug::tests::snapshot_serializes_to_json`
Expected: PASS (types and test land together; the point of the test is locking the wire format).

Note: unused-code warnings for the not-yet-used types are expected until Task 5/6.

**Step 3: Commit**

```bash
git add src/debug.rs src/main.rs
git commit -m "feat(debug): add debug socket types and wire format"
```

---

## Task 3: Command parsing

**Files:**
- Modify: `src/debug.rs`

**Step 1: Write the failing tests**

Append to the `tests` module in `src/debug.rs`:

```rust
    #[test]
    fn parses_commands() {
        assert!(matches!(parse_command("state"), Some(DebugCommand::State)));
        assert!(matches!(parse_command("await-idle"), Some(DebugCommand::AwaitIdle)));
        assert!(matches!(
            parse_command("entries left"),
            Some(DebugCommand::Entries(PaneId::Left))
        ));
        assert!(matches!(
            parse_command("entries center"),
            Some(DebugCommand::Entries(PaneId::Center))
        ));
        // whitespace tolerance
        assert!(matches!(parse_command("  state "), Some(DebugCommand::State)));
        // unknown input
        assert!(parse_command("entries right").is_none());
        assert!(parse_command("bogus").is_none());
        assert!(parse_command("").is_none());
    }
```

**Step 2: Run tests to verify they fail**

Run: `cargo test debug::tests::parses_commands`
Expected: FAIL — `DebugCommand` / `parse_command` not defined.

**Step 3: Write the implementation**

Add to `src/debug.rs` above the tests:

```rust
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
```

**Step 4: Run tests to verify they pass**

Run: `cargo test debug::tests`
Expected: both tests PASS.

**Step 5: Commit**

```bash
git add src/debug.rs
git commit -m "feat(debug): parse line-protocol commands"
```

---

## Task 4: Socket listener

**Files:**
- Modify: `src/debug.rs`

**Step 1: Write the failing integration test**

The listener is decoupled from the app by design, so it is testable with a fake manager answering the request channel. Append to `tests`:

```rust
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
        assert!(reply.contains("\"name\":\"a.txt\""), "unexpected reply: {reply}");

        let reply = query(&socket, "bogus").await;
        assert!(reply.contains("error"), "unexpected reply: {reply}");
    }
```

**Step 2: Run test to verify it fails**

Run: `cargo test debug::tests::serves_commands_over_socket`
Expected: FAIL — `serve` not defined.

**Step 3: Write the implementation**

Add to `src/debug.rs`:

```rust
/// Accept loop for the debug socket. Spawned once from main when
/// `--debug-socket` is given. One command per connection.
pub async fn serve(socket_path: PathBuf, request_tx: mpsc::Sender<DebugRequest>) -> Result<()> {
    // Remove a stale socket file from a previous (crashed) run
    let _ = std::fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;
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
            if request_tx.send(DebugRequest::State { reply: tx }).await.is_err() {
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
            if request_tx.send(DebugRequest::AwaitIdle { reply: tx }).await.is_err() {
                return MANAGER_GONE.into();
            }
            match timeout(Duration::from_secs(30), rx).await {
                Ok(Ok(seq)) => format!(r#"{{"idle":true,"seq":{seq}}}"#),
                _ => r#"{"idle":false,"error":"timeout: not idle within 30s"}"#.into(),
            }
        }
        Some(DebugCommand::Entries(pane)) => {
            let (tx, rx) = oneshot::channel();
            if request_tx.send(DebugRequest::Entries { pane, reply: tx }).await.is_err() {
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
```

**Step 4: Run all debug tests**

Run: `cargo test debug::`
Expected: all PASS.

**Step 5: Commit**

```bash
git add src/debug.rs
git commit -m "feat(debug): add unix socket listener for debug commands"
```

---

## Task 5: Wire DebugRequest handling into PanelManager

This task has no unit test — `PanelManager` requires a live terminal, so it is verified end-to-end in Task 7. Keep changes minimal and mechanical.

**Files:**
- Modify: `src/panel/manager.rs`

**Step 1: Add imports and fields**

Imports (extend the existing `use crate::{...}` block):

```rust
use crate::{
    command_queue::{QueueStatus, QueuedCommand},
    debug::{ClipboardInfo, DebugRequest, EntryInfo, PaneId, StateSnapshot},
    // ... existing items unchanged
};
use tokio::sync::watch;
```

Fields on `PanelManager` (after `command_tx`, around `src/panel/manager.rs:146`):

```rust
    /// Receiver for status of the background command queue
    command_status_rx: watch::Receiver<QueueStatus>,

    /// Receiver for debug-socket requests (`--debug-socket`)
    debug_rx: Option<mpsc::Receiver<DebugRequest>>,

    /// Event-loop iteration counter, exposed via the debug socket
    debug_seq: u64,
```

**Step 2: Extend the constructor**

`PanelManager::new` (`src/panel/manager.rs:150`) gets two new parameters after `command_tx`:

```rust
        command_tx: Option<mpsc::UnboundedSender<QueuedCommand>>,
        command_status_rx: watch::Receiver<QueueStatus>,
        debug_rx: Option<mpsc::Receiver<DebugRequest>>,
```

and initializes the struct fields:

```rust
            command_status_rx,
            debug_rx,
            debug_seq: 0,
```

**Step 3: Add the select branch**

Add a free helper function near `expand_command` at the top of `manager.rs`. It exists because `tokio::select!` needs a future that stays pending when debug mode is off (a plain `recv()` on an absent receiver can't express that):

```rust
/// Receive a debug request, or stay pending forever when debug mode is off.
async fn recv_debug(rx: &mut Option<mpsc::Receiver<DebugRequest>>) -> Option<DebugRequest> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}
```

In `run()` (`src/panel/manager.rs:1052`), make the select biased and append the debug branch **last** — the position is load-bearing: with `biased;`, this branch only wins when no log, panel update, or terminal event is ready, which is exactly the "idle" condition `await-idle` promises:

```rust
            tokio::select! {
                biased;
                // Check incoming new logs
                () = self.logger.update() => {
                    // ... existing body unchanged
                }
                // ... existing dir_rx, prev_rx, event_reader branches unchanged ...

                // Debug socket requests; must stay the LAST branch (see comment above)
                Some(req) = recv_debug(&mut self.debug_rx) => {
                    self.handle_debug_request(req);
                }
            }
```

At the bottom of the loop, directly after `self.draw()?;` (`src/panel/manager.rs:1111`):

```rust
            self.debug_seq = self.debug_seq.wrapping_add(1);
```

**Step 4: Implement the request handler and snapshot builder**

Add to `impl PanelManager`:

```rust
    fn handle_debug_request(&mut self, req: DebugRequest) {
        // Replies may fail if the client timed out; that is fine.
        match req {
            DebugRequest::State { reply } => {
                let _ = reply.send(self.state_snapshot());
            }
            DebugRequest::AwaitIdle { reply } => {
                // Answered here means the biased select found nothing else
                // ready — the event queue is drained.
                let _ = reply.send(self.debug_seq);
            }
            DebugRequest::Entries { pane, reply } => {
                let panel = match pane {
                    PaneId::Left => self.left.panel(),
                    PaneId::Center => self.center.panel(),
                };
                let selected_idx = panel.selected_idx();
                let entries = panel
                    .elements()
                    .enumerate()
                    .map(|(idx, elem)| EntryInfo {
                        name: elem.name().clone(),
                        marked: elem.is_marked(),
                        hidden: elem.is_hidden(),
                        selected: idx == selected_idx,
                    })
                    .collect();
                let _ = reply.send(entries);
            }
        }
    }

    fn state_snapshot(&self) -> StateSnapshot {
        let center = self.center.panel();
        let (selected_idx, total) = center.index_vs_total();
        let queue = self.command_status_rx.borrow().clone();
        StateSnapshot {
            seq: self.debug_seq,
            mode: match &self.mode {
                Mode::Normal => "normal",
                Mode::Console { .. } => "console",
                Mode::CreateItem { is_dir: true, .. } => "mkdir",
                Mode::CreateItem { .. } => "touch",
                Mode::Search { .. } => "search",
                Mode::Rename { .. } => "rename",
            }
            .to_string(),
            cwd: center.path().to_path_buf(),
            selection: center
                .selected_path()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned()),
            selected_idx,
            total,
            marked: center
                .elements()
                .filter(|e| e.is_marked())
                .map(|e| e.path().to_path_buf())
                .collect(),
            clipboard: self.clipboard.as_ref().map(|c| ClipboardInfo {
                files: c.files.clone(),
                op: if c.cut { "cut" } else { "copy" }.to_string(),
            }),
            show_hidden: self.show_hidden,
            left_path: self.left.panel().path().to_path_buf(),
            preview_path: self.right.panel().path().to_path_buf(),
            queue_active: queue.active,
            queue_len: queue.queued_count,
        }
    }
```

Accessor notes (all verified public): `DirPanel::elements()` (`directory.rs:853`), `name()`/`is_marked()`/`is_hidden()` on `DirElem` (`directory.rs:149-165`), `index_vs_total()` (`directory.rs:1116`), `path()` via the `PanelContent` trait. If `selected_idx()` turns out not to be an index into `elements()` order, use `index()` (`directory.rs:1100`) — check against the `up()`/`down()` implementations.

**Step 5: Verify it compiles (main.rs still has the old call — expect exactly one error)**

Run: `cargo build 2>&1 | head -30`
Expected: exactly one error at the `PanelManager::new` call site in `main.rs` (fixed in Task 6). No errors inside `manager.rs`.

**Step 6: Commit**

```bash
git add src/panel/manager.rs
git commit -m "feat(debug): handle debug requests in PanelManager event loop"
```

---

## Task 6: CLI flag and main.rs wiring

**Files:**
- Modify: `src/main.rs`

**Step 1: Add the CLI flag**

In `struct Args` (`src/main.rs:45`), after `choosedir`:

```rust
    /// Expose a debug/testing socket at the given path.
    ///
    /// Line-based protocol for development and automated testing:
    /// send "state", "await-idle" or "entries <left|center>", receive one
    /// line of JSON. Example: echo state | socat - UNIX-CONNECT:<path>
    #[arg(long)]
    debug_socket: Option<PathBuf>,
```

**Step 2: Spawn the listener and wire the channels**

After the command-executor setup (`src/main.rs:286-290`) — note `_command_status_rx` loses its underscore, it is finally consumed:

```rust
    let (command_status_tx, command_status_rx) =
        tokio::sync::watch::channel(command_queue::QueueStatus::default());
```

Then, before `init_miller_panels`:

```rust
    // Debug socket (only with --debug-socket)
    let debug_rx = if let Some(socket_path) = args.debug_socket.clone() {
        let (debug_tx, debug_rx) = mpsc::channel(8);
        tokio::spawn(debug::serve(socket_path, debug_tx));
        Some(debug_rx)
    } else {
        None
    };
```

Extend the `PanelManager::new` call (`src/main.rs:300`):

```rust
        Some(command_tx),
        command_status_rx,
        debug_rx,
```

**Step 3: Clean up the socket file on exit**

After the terminal-cleanup block that follows `panel_handle.await` (near `src/main.rs:325`):

```rust
    if let Some(socket_path) = &args.debug_socket {
        let _ = std::fs::remove_file(socket_path);
    }
```

**Step 4: Build and lint**

Run: `cargo build && cargo clippy -- -D warnings`
Expected: clean build. If clippy complains about pre-existing issues unrelated to this change, only fix the ones this change introduced.

**Step 5: Run the full test suite**

Run: `cargo test`
Expected: all tests PASS.

**Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(debug): add --debug-socket flag"
```

---

## Task 7: End-to-end verification via tmux

No code — this validates the whole chain against the real binary. Run each step and compare against expectations. If anything mismatches, debug before continuing (see @superpowers:systematic-debugging).

**Step 1: Build and prepare a fixture**

```bash
cargo build
FIXTURE=$(mktemp -d)
touch "$FIXTURE/a.txt" "$FIXTURE/b.txt" "$FIXTURE/c.txt"
mkdir "$FIXTURE/subdir"
SOCK=/tmp/rfm-debug-test.sock
```

**Step 2: Start rfm in tmux**

```bash
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test "./target/debug/rfm --debug-socket $SOCK $FIXTURE" Enter
sleep 1
```

**Step 3: Query initial state**

```bash
echo state | socat - UNIX-CONNECT:$SOCK
```

Expected: one JSON line; `"mode":"normal"`, `"cwd"` ending in the fixture dir, `"selection":"a.txt"` (first entry), `"total":4`, `"marked":[]`.

**Step 4: Drive it and verify synchronization**

```bash
tmux send-keys -t rfm-test j          # move down
echo await-idle | socat - UNIX-CONNECT:$SOCK
echo state | socat - UNIX-CONNECT:$SOCK
```

Expected: `await-idle` returns `{"idle":true,...}` promptly; `state` now shows `"selection":"b.txt"` and a higher `seq` than in step 3.

**Step 5: Verify entries and the visual diff**

```bash
tmux send-keys -t rfm-test Space      # mark b.txt (check keys.toml if unsure)
echo await-idle | socat - UNIX-CONNECT:$SOCK
echo "entries center" | socat - UNIX-CONNECT:$SOCK
tmux capture-pane -t rfm-test -p
```

Expected: entries JSON lists `a.txt`–`c.txt` + `subdir` with `"marked":true` on b.txt; capture-pane shows the same files — belief and reality agree.

**Step 6: Verify shutdown cleanup**

```bash
tmux send-keys -t rfm-test q
sleep 1
test -S $SOCK && echo "FAIL: socket not removed" || echo "OK: socket removed"
tmux kill-session -t rfm-test
rm -rf "$FIXTURE"
```

Expected: `OK: socket removed`.

---

## Task 8: Document the workflow in CLAUDE.md

**Files:**
- Create: `CLAUDE.md` (repo root — does not exist yet)

**Step 1: Write the doc**

```markdown
# rfm — development notes for Claude

rfm is a terminal file manager (crossterm + tokio, Miller columns).
Build: `cargo build` — Tests: `cargo test` — Lint: `cargo clippy`

## Testing the TUI interactively

Drive the real binary in tmux; use the debug socket for synchronization
and state instead of sleeping and guessing.

```bash
# Setup (isolated fixture + config)
FIXTURE=$(mktemp -d); touch "$FIXTURE"/{a,b,c}.txt
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test \
  "./target/debug/rfm --debug-socket /tmp/rfm.sock $FIXTURE" Enter

# Drive + observe (the loop for every interaction)
tmux send-keys -t rfm-test j                       # 1. input
echo await-idle | socat - UNIX-CONNECT:/tmp/rfm.sock   # 2. wait, don't sleep
echo state | socat - UNIX-CONNECT:/tmp/rfm.sock        # 3. internal state
tmux capture-pane -t rfm-test -p                       # 4. actual screen

# Teardown
tmux kill-session -t rfm-test; rm -rf "$FIXTURE"
```

Socket commands (one per connection, JSON reply):
- `state` — mode, cwd, selection, marked, clipboard, queue, seq counter
- `await-idle` — blocks until the event loop has drained (max 30s)
- `entries left|center` — the entries rfm *believes* the pane shows

## Debugging visual bugs: diff belief against reality

`capture-pane` is ground truth (what the user sees); the socket is rfm's
belief. For "pane not updating"-style bugs, compare the two:
- `entries` correct, screen stale → bug in the render/flush path
  (dirty flags, draw()).
- `entries` also stale → bug in the state/update path (watchers,
  cache, DirManager).

If `state` times out (10s), the event loop itself is wedged — that is a
finding, not a tooling failure.

Caveats:
- `await-idle` does not cover async panel loads still in flight
  (DirManager/PreviewManager); if content looks like a loading
  placeholder, poll `state` until `seq` stabilizes.
- `--config` can point at a scratch dir to isolate config.
- Use `-x`/`-y` on tmux new-session for a deterministic pane size.
```

**Step 2: Verify the recipe works as written**

Follow the CLAUDE.md commands verbatim once. Expected: they work copy-paste.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: add Claude testing workflow for the debug socket"
```

---

## Future extensions (explicitly out of scope — YAGNI until a real session needs them)

- `await-settled`: like await-idle, but also waits for in-flight panel loads.
- `entries right`: preview pane content (needs `PreviewPanel` content access).
- Last-known-snapshot via `watch`: lets `state` answer even while the event loop is wedged, with the age of the snapshot.
- Remote-control commands (`send-key`, event injection): only if tmux input fidelity actually becomes a problem.
- Action log: `log::debug!` at the event-dispatch point + file sink on the existing `LogBuffer`.
