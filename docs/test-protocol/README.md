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
FIXTURE=$(mktemp -d)                  # per-section fixture files: see section header
CFG=$(mktemp -d)                      # scratch config dir (--config)
CACHE=$(mktemp -d); STATE=$(mktemp -d); ZO=$(mktemp -d)

tmux new-session -d -s $SESSION -x 120 -y 30
tmux send-keys -t $SESSION \
  "XDG_CACHE_HOME=$CACHE XDG_STATE_HOME=$STATE _ZO_DATA_DIR=$ZO \
   ./target/debug/rfm --debug-socket $SOCK --config $CFG $FIXTURE" Enter
until [ -S $SOCK ]; do sleep 0.1; done
```

Teardown (end of every section, even after failures):

```bash
tmux kill-session -t $SESSION 2>/dev/null
rm -rf "$FIXTURE" "$CFG" "$CACHE" "$STATE" "$ZO"; rm -f $SOCK
```

### The verification loop — every step, no exceptions

```bash
tmux send-keys -t $SESSION <keys>                      # 1. input
echo await-idle | socat - UNIX-CONNECT:$SOCK           # 2. wait, never sleep
echo state      | socat - UNIX-CONNECT:$SOCK           # 3. rfm's belief
tmux capture-pane -t $SESSION -p                       # 4. what the user sees
```

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
