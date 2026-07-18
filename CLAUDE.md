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
- `state` — mode, cwd, selection, marked, clipboard, queue, seq counter,
  undo_depth/redo_depth
- `await-idle` — blocks until the event loop has drained (max 30s)
- `entries left|center` — the entries rfm *believes* the pane shows
- `log [n]` — last n retained log lines (level, age_secs, message).
  The retention history (200 lines, capacity-evicted only) outlives the
  log widget's per-line 10s display TTL, so this is where errors live
  after they vanish from screen.
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

## Architecture: modal modes

Mode logic lives in `src/panel/mode/` — one adapter per file (the two
consoles currently share `console.rs`; split pending). Adapters are
pure state machines: `handle_key → ModeOp`, testable without a
terminal. All effects and the derived redraws are applied centrally in
`PanelManager::apply_mode_op` (manager.rs). The mode strings the debug
socket reports come from `ModalInput::name()`.

## Architecture: rendering

Event-driven, not a render loop: the select loop draws only when an event
sets the single `dirty` bit (PanelManager). `draw()` repaints everything
in call order (footer → header → panels → console → log, overlay last —
that ordering *is* the z-order) and clears the bit. No per-element dirty
flags: any state change calls `mark_dirty()`, so panels can't go stale.
Full repaint is cheap because every draw is a blit except the image
preview, whose resize is cached (FilePreview, keyed on cell dimensions).
Adding a window ≈ carve its layout region + a draw call at the right spot
in the sequence; no flag plumbing.

Log lines shown in the widget expire after DISPLAY_TTL (logger.rs, 10s);
the 1 s task wakes the UI only when a line actually expires.

## Architecture: undo/redo

In-session, in-memory only (`src/undo/`, terminal-free + unit-tested).
Everything reduces to three atomic reversible changes — `FsChange::{Create,
Move, Copy}`; one user action = one `Transaction` of N changes. The FS
primitives (`move_item`/`copy_item` in util.rs, `delete_file`, zip/tar)
return the `FsChange` they made, carrying the *actually-reached* path so
the `_`-suffix collision fallback stays correct. The command handlers in
`manager.rs` assemble transactions and call `undo.record()`; undo/redo are
applied via `apply_undo`/`apply_redo` without re-recording (they push onto
the opposite stack). The async paste task hands its transaction back over
`undo_tx`/`undo_rx` to be recorded on the main loop.

Non-reversible actions push a `Barrier` (permanent delete — trash off);
undo hitting it stops and reports, never reverting past it. External
commands/opener/extract are untracked (ignored). zip/tar are undoable
(delete the archive) but `no_redo` (content can't be replayed). Keys:
`u` / `ctrl-r` (opt-in `undo`/`redo` in keys.toml — pre-existing user
configs won't have them until added). Debug socket `state` exposes
`undo_depth` / `redo_depth`.
