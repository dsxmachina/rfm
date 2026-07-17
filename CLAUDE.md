# rfm — development notes for Claude

rfm is a terminal file manager (crossterm + tokio, Miller columns).
Build: `cargo build` — Tests: `cargo test` — Lint: `cargo clippy`

## Testing the TUI interactively

Drive the real binary in tmux; use the debug socket for synchronization
and state instead of sleeping and guessing.

```bash
# Setup (isolated fixture)
FIXTURE=$(mktemp -d); touch "$FIXTURE"/{a,b,c}.txt
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test \
  "./target/debug/rfm --debug-socket /tmp/rfm.sock $FIXTURE" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done   # socket appears ~instantly, but not instantly

# Drive + observe (the loop for every interaction)
tmux send-keys -t rfm-test j                           # 1. input
echo await-idle | socat - UNIX-CONNECT:/tmp/rfm.sock   # 2. wait, don't sleep
echo state | socat - UNIX-CONNECT:/tmp/rfm.sock        # 3. internal state
tmux capture-pane -t rfm-test -p                       # 4. actual screen

# Teardown (the socket file survives the kill — remove it too)
tmux kill-session -t rfm-test; rm -rf "$FIXTURE" /tmp/rfm.sock
```

Socket commands (one per connection, JSON reply):
- `state` — mode, cwd, selection, marked, clipboard, queue, seq counter
- `await-idle` — blocks until the event loop has drained (max 30s)
- `entries left|center` — the entries rfm *believes* the pane shows
- `log [n]` — last n retained log lines (level, age_secs, message).
  The retention history (200 lines) outlives the log widget's 1-line/sec
  eviction, so this is where errors live after they vanish from screen.
  Background command failures (exit codes, stderr) land here — check
  `log` first when something "silently" fails.

With `--debug-socket` active, verbosity is raised to TRACE (rfm targets
only; dependency noise is filtered, and the on-screen widget still shows
only info+). The history then contains a full causal trail per
interaction: `key-event:` (with mode), `mode: x -> y`, `draw:` (which
panes were considered dirty — the stale-pane diagnostic), `panel-update:`
(async panel arrivals), `jump-to`, `zoxide query`, watcher
`watching`/`unwatching`. Correlate via age_secs.

Reading the replies correctly:
- Directories sort before files: with a.txt/b.txt/c.txt + subdir, the
  initial selection is `subdir`, not `a.txt` — don't mis-assert.
- `state.selected_idx` is 0-based among VISIBLE entries; the `entries`
  reply lists ALL entries (incl. hidden) with a per-entry `selected`
  flag. Never index one with the other.
- `state.marked` holds absolute paths; `entries` has per-entry `marked`
  booleans. Mark key is Space (default keys.toml), and marking
  auto-advances the cursor to the next entry.
- `seq` advances by multiple ticks per keypress (~5 for one `j`).
  Never assume +1 deltas. Reliable invariants: seq is stable while
  nothing happens; seq increased after real input. Debug queries
  themselves never advance seq.

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
- On exit with errors, rfm writes `./error.log` from the full retained
  history (with line ages) — useful post-mortem when the session is gone.
- Background commands run via `sh -c`: any path interpolated into a
  queued command string MUST go through `shell_escape::escape`
  (see `zoxide_add_dir`, `expand_command`). Test fixture names with
  spaces and `&` (e.g. "a directory with spaces", "Bilder & Videos").
- Isolate zoxide in tests with `_ZO_DATA_DIR=$(mktemp -d)` in the tmux
  pane before launching rfm; seed with `zoxide add <path>`.
