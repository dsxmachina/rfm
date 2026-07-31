# rfm e2e test protocol

A step-by-step checklist for testing rfm end-to-end, written to be executed by an
AI agent (or a patient human) with **no prior context**. Execution is double-blind
on every step: the tmux screen capture is what the user sees (ground truth), the
debug socket is what rfm believes — each step asserts **both**, and a mismatch
between them is itself a bug (stale-render class).

## Ground rules for the executor

1. **Run every section to the end.** A failing step is documented as a bug and
   execution continues (skip only steps that *depend* on the failed one; note the
   skip). Never stop the protocol because of a bug.
2. **Document every bug immediately** using the format in
   [bug-report-template.md](bug-report-template.md), while the tmux session still
   exists — capture the pane, the socket `state`, and `log 50` at failure time.
3. **A tooling failure is not a bug.** If socat/tmux misbehave, or a fixture
   recipe fails, fix the harness and note it as *protocol feedback*, not as an
   rfm bug. If `state` times out (10s), however, the event loop is wedged —
   that IS a finding.
4. **Record protocol feedback.** Every ambiguous expectation, wrong key, missing
   fixture recipe or flaky assertion goes in a "Protocol feedback" list in your
   run report — the protocol is iterated between runs.
5. **Never run mutating git commands** (stash/checkout/reset/clean). The repo may
   hold uncommitted user work.

## Prerequisites

- `cargo build` has been run; the binary is `./target/debug/rfm`.
- `tmux`, `socat` available. Optional (steps degrade gracefully and say how):
  `zoxide`, `ffmpeg`, `openssl`, `zip`.
- Run from the repo root.

## Shared harness (referenced by every section)

Each section `NN` uses its own session, socket and fixtures so sections can run
in parallel:

```bash
N=01                                  # section number
SESSION=rfm-sec$N
SOCK=/tmp/rfm-sec$N.sock
PARENT=$(mktemp -d)                   # fresh EMPTY parent — see "Quiet fixture parent" below
FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"  # per-section fixture files go INSIDE $FIXTURE
CFG=$(mktemp -d)                      # scratch config dir (--config)
CACHE=$(mktemp -d); STATE=$(mktemp -d); ZO=$(mktemp -d)

# Parallel-safe: clear any stale session/socket of the same name FIRST.
tmux kill-session -t $SESSION 2>/dev/null; rm -f $SOCK

# Launch the binary DIRECTLY as the session command (do NOT send-keys into an
# interactive login shell — a zsh history-search widget such as Atuin will
# intercept the typed command and rfm never launches). Isolation env is passed
# via `env` so it applies to the child regardless of the pane's shell.
tmux new-session -d -s $SESSION -x 120 -y 30 \
  "env XDG_CACHE_HOME=$CACHE XDG_STATE_HOME=$STATE _ZO_DATA_DIR=$ZO \
   ./target/debug/rfm --debug-socket $SOCK --config $CFG $FIXTURE"
until [ -S $SOCK ]; do sleep 0.1; done
```

**Quiet fixture parent (why `PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`).** rfm's
*left* (parent) panel watches `dirname($FIXTURE)`. A bare `FIXTURE=$(mktemp -d)`
makes that parent `/tmp` itself — a large, live directory that other processes
(and parallel executors) churn continuously. The watcher then re-scans it
~2×/sec, which (a) makes `seq` advance with **zero** user input (breaks every
"idle `seq` stable" assertion), (b) floods the 200-line socket `log` ring with
`Updating: /tmp` so rfm's own TRACE/DEBUG/INFO lines are evicted within ~1 s,
and (c) makes the preview preloader store thumbnails for stray sibling images
(breaks "total thumbnails == N" counts). Nesting the fixture one level down
under a freshly-`mkdir`'d parent leaves that parent **empty and quiet**, so all
three invariants become meaningful again.

**Per-section env files.** If a section persists its harness vars to disk
(e.g. to `source` between Bash calls), it MUST use a per-section filename such
as `env-sec$N.sh` in the scratchpad — **never** a shared `env.sh`. Parallel
executors share the scratchpad and clobbered each other's SESSION/SOCK/FIXTURE
through a common `env.sh` in run 1.

Teardown (end of every section, even after failures):

```bash
tmux kill-session -t $SESSION 2>/dev/null
rm -rf "$PARENT" "$CFG" "$CACHE" "$STATE" "$ZO"; rm -f $SOCK
```

### The verification loop — every step, no exceptions

Wrap **every** socket read in `timeout 12` — a wedged event loop must never
hang the executor. A reply arriving at all inside the timeout already proves
no-wedge; a timeout IS a finding (the loop is wedged), not a tooling failure.

```bash
tmux send-keys -t $SESSION <keys>                              # 1. input
echo await-idle | timeout 12 socat - UNIX-CONNECT:$SOCK        # 2. wait, never sleep
echo state      | timeout 12 socat - UNIX-CONNECT:$SOCK        # 3. rfm's belief
tmux capture-pane -t $SESSION -p                               # 4. what the user sees
```

### Technique notes (learned the hard way in run 1)

- **`send-keys -l` for literal tokens that are also tmux key names.** To type a
  literal multi-char string that tmux recognizes as a key name (`delete`,
  `space`, `up`, `down`, `home`, `end`, `tab`, `enter`, …), use
  `tmux send-keys -t $SESSION -l <text>`. A bare `send-keys delete` sends the
  Delete **key**, not the six letters — the typed command silently no-ops.
  Special keys you *do* want as keys (`Enter`, `Escape`, `C-u`, `Space`) are
  sent WITHOUT `-l`, each as its own `send-keys` call. **Mind the exact tmux
  key name:** backspace is `BSpace`, not `Backspace` — a bare `send-keys
  Backspace` types the nine letters into the input instead of deleting.
- **Reverse-video / highlight detection.** rfm emits the selection's
  reverse-video SGR as **both** `\x1b[7m` (plain reverse) and `\x1b[0;7m`
  (reset+reverse, on file rows). A highlight grep must match `[7m` **OR**
  `[0;7m`. Capture with `tmux capture-pane -t $SESSION -p -e` to include the
  SGR codes; exactly one row in the center column should carry the attribute.
- **Log assertions: socket first, wide window, sample the widget.** Under
  `--debug-socket` verbosity is TRACE and each keypress emits several TRACE
  lines, so a short `log N` window fills instantly. Assert command/INFO/DEBUG
  log lines via **`log 200` queried IMMEDIATELY after the action** (grep the
  reply in the same call). Treat the on-screen 10 s log widget as
  timing-fragile: a single `capture-pane` can race a redraw — sample it 2–3×
  before asserting a line is *absent*.
- **Portable timing.** `/usr/bin/time` and `bc` may be absent. For any step
  that needs a wall-clock measurement use `start=$(date +%s%N); …;
  end=$(date +%s%N)` and compare in nanoseconds, or simply rely on the
  `timeout 12` wrapper as the wedge detector (a reply inside it = no wedge).
- **SELECT navigation helper — never blind `j`.** Several steps navigate to a
  target row. Do NOT press `j` repeatedly until `state.selection == name`:
  after a rename/delete the target can sort *above* the current selection, so
  `j` overshoots and wraps, flooding the log ring. Instead: read `entries
  center`, compute the target's **visible** index, compute the current
  `state.selected_idx`, then step `j` (target below) or `k` (target above) by
  the exact shortest distance, polling `state` to confirm arrival. This keeps
  keystroke count minimal so log-history assertions survive.

Other socket queries: `entries [<tab>] left|center`, `log [n]`, and `state`
fields (verified against the binary): `seq, mode, view, focused,
tabs[]{cwd, selection, selected_idx, total, marked}, cwd, selection,
selected_idx, total, marked, clipboard, show_hidden, left_path, preview_path,
queue_active, queue_len, undo_depth, redo_depth, jump_marks, image_protocol`
(the scalar fields mirror the focused tab).

### Assertion pitfalls (bugs have been mis-filed over each of these)

- Directories sort before files → initial selection is the first *directory*.
- `state.selected_idx` indexes **visible** entries only; the `entries` reply
  lists **all** entries (incl. hidden) with per-entry `selected`/`marked`
  flags. Never index one with the other.
- Marking (Space) auto-advances the cursor.
- `seq` advances by multiple ticks per keypress. Valid invariants only:
  stable while idle; increased after real input. Debug queries never advance it.
- `await-idle` does **not** cover in-flight async panel/preview loads; if
  content looks like a placeholder, poll `state` until `seq` stabilizes.
- Log lines vanish from the *screen* after 10s but stay in the socket `log`
  history (200 lines). Background-command failures land there — check `log`
  first when something "silently" fails.
- Inside tmux the graphics protocol resolves to **half-block**: image previews
  are colored half-block cells. Assert "non-empty raster area", never glyphs.
- **Empty-panel sentinels.** For an *empty* directory, `state.preview_path` is
  the sentinel string `"path-of-empty-panel"`, not a real path; `left_path` at
  the filesystem root (`/`) is likewise the same sentinel, not a directory
  path. Do not mis-assert a real path against either.
- **The header/title row shows the *selected item's* path, not the cwd.** When a
  directory is selected the top row reads `.../<selected-dir>` while `cwd` is
  still its parent — this is correct, not a cwd change.
- On exit with errors rfm writes `./error.log` (cwd of the *rfm process*).

## Sections

Run in order for a full pass; each is self-contained for re-runs.

| # | File | Covers |
|---|------|--------|
| 01 | [sections/01-startup-and-movement.md](sections/01-startup-and-movement.md) | launch, layout, cursor movement, quit |
| 02 | [sections/02-navigation.md](sections/02-navigation.md) | enter/leave dirs, history, jumps, zoxide, hidden files |
| 03 | [sections/03-marking-and-clipboard.md](sections/03-marking-and-clipboard.md) | mark, copy/cut/paste, collisions |
| 04 | [sections/04-file-manipulation-and-trash.md](sections/04-file-manipulation-and-trash.md) | rename, mkdir/touch, delete, trash view + restore |
| 05 | [sections/05-console-search-and-input-modes.md](sections/05-console-search-and-input-modes.md) | console, search/filter, input overlays |
| 06 | [sections/06-tabs-and-split-view.md](sections/06-tabs-and-split-view.md) | tabs, split view, cross-tab updates |
| 07 | [sections/07-opener-and-mime.md](sections/07-opener-and-mime.md) | opener rules, MIME detection, sniffing, FIFO safety |
| 08 | [sections/08-previews-text-and-native.md](sections/08-previews-text-and-native.md) | text/archive/cert/PDF native preview backends |
| 09 | [sections/09-previews-images-and-raster-cache.md](sections/09-previews-images-and-raster-cache.md) | image previews, raster cache, privacy gate |
| 10 | [sections/10-shell-commands-and-escaping.md](sections/10-shell-commands-and-escaping.md) | background commands, shell escaping, watcher |
| 11 | [sections/11-undo-redo.md](sections/11-undo-redo.md) | undo/redo transactions, barriers, trash cycle |
| 12 | [sections/12-complex-and-stress.md](sections/12-complex-and-stress.md) | rapid input, resize, churn, unicode, error.log |

## Run report

Write the run report to `docs/test-protocol/runs/<date>-<label>.md`:
per-section step tally (pass/fail/skip), all bug reports (template format),
protocol feedback list, environment notes (tools missing, terminal, commit).
