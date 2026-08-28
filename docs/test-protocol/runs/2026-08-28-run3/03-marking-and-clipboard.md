# Run 3 — Section 03: Marking and clipboard

- **Date:** 2026-08-28
- **Branch:** feat/kitty-unicode-placeholders (binary `./target/debug/rfm`, pre-built)
- **Harness:** tmux session `tp3-03`, socket `/tmp/tp3-03.sock`, 120x30 pane,
  quiet fixture parent (`$(mktemp -d)/fx`), scratch `--config`, isolated
  `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR`, `env -u KITTY_WINDOW_ID -u
  GHOSTTY_RESOURCES_DIR`, binary launched directly as the session command.
- **Result: 14/14 PASS, 0 fail, 0 skip. No bugs filed.**

Every step ran the four-step loop (input → await-idle → socket assert →
capture-pane assert), all socket reads wrapped in `timeout 12`; no read ever
timed out (no wedge). Async pastes were gated with the section's `wait_undo`
poll before asserting.

## Per-step results

### 03.1 — Baseline: no marks, empty clipboard — PASS
Socket: `mode=normal`, `cwd=$FIXTURE`, `selection=dest`, `selected_idx=0`,
`total=6`, `marked=[]`, `clipboard=null`, `undo_depth=0`, `redo_depth=0`.
`entries center` returned the six entries in the expected order
(`dest, src2, a.txt, b & c file.txt, b.txt, c.txt`), all `marked:false`,
only `dest` selected. Screen showed all six names, no `x` markers.

### 03.2 — Space marks and auto-advances — PASS
After `j j` + `Space`: `marked=["$FIXTURE/a.txt"]`, `selection="b & c
file.txt"`, `selected_idx=3`. Entries: `a.txt` `marked:true,selected:false`;
`b & c file.txt` `selected:true`. Screen: leading `x` on the `a.txt` row only
(`│x🖹a.txt`).

### 03.3 — Mark multiple — PASS
After `Space Space`: `marked` = exactly {a.txt, b & c file.txt, b.txt} (abs
paths), `selection=c.txt`. Screen: `x` on those three rows, none on `c.txt`.

### 03.4 — Space toggles a mark off — PASS
`k` then `Space` on `b.txt`: `marked` back to {a.txt, b & c file.txt},
`selection=c.txt` (auto-advance on unmark confirmed). Screen: `x` gone from
the `b.txt` row.

### 03.5 — Esc unmarks all — PASS
`marked=[]`, `mode=normal`, `clipboard=null`; zero `x` rows on screen.

### 03.6 — Copy bare selection (`yy`) — PASS
`gg`, `j j`, `y y`: `clipboard={"files":["$FIXTURE/a.txt"],"op":"copy"}`,
`marked=["$FIXTURE/a.txt"]` (auto-mark side effect, not unmarked after),
`undo_depth=0`. `log 200` right after: INFO `copying 1 items`
(age 0.017s). Screen: `x` on `a.txt`.

### 03.7 — Paste into another directory — PASS
Pre-paste in `dest`: `cwd=$FIXTURE/dest`, `total=0`, `selection=null`.
After `pp` + `wait_undo 1`: `undo_depth=1`, `clipboard=null`, `marked=[]`;
`entries center` = exactly `a.txt` `selected:true`. Log: INFO `paste 1 items,
overwrite = false`. Screen header `.../dest/a.txt`, center shows `a.txt`.
Disk: both copies exist, `cmp` identical.

### 03.8 — Paste collision appends `_` to the full name — PASS
`yy` + `pp` in `dest`, `wait_undo 2`: `undo_depth=2`, `clipboard=null`;
entries exactly `a.txt`, `a.txt_` in that order (suffix after the extension).
Screen shows both. Disk: both files present, contents identical.

### 03.9 — Cross-directory cut moves the file — PASS
`G` + `dd`: `clipboard={"files":[".../dest/a.txt_"],"op":"cut"}`, log INFO
`cut 1 items`. `h` restored `selection=dest` in `$FIXTURE`; `j`, `l` into
`src2` (`total=0`). `pp` + `wait_undo 3`: `undo_depth=3`, `clipboard=null`,
`cwd=$FIXTURE/src2`, entries exactly `a.txt_`. Disk: present in `src2`,
gone from `dest`.

### 03.10 — Cut + paste into the same directory is a no-op — PASS
`dd` (clipboard cut `src2/a.txt_`), `pp`, sleep 1.2s, await-idle:
`undo_depth` still 3 (empty transaction not recorded), `clipboard=null`.
Log: WARN `from and to are identical` (age 1.21s) alongside the INFO
`paste 1 items, overwrite = false` for this paste. Entries/screen: only
`a.txt_`; no `a.txt__` on disk.

### 03.11 — Paste with empty clipboard is inert — PASS
seq 89 → 93 (input processed); `clipboard=null`, `undo_depth=3`,
`marked=[]`. `log 200`: no fresh `paste` line — youngest paste line age
7.59s (the 03.10 one), well over the ~1s elapsed since the keypress.
Screen unchanged (`a.txt_` only).

### 03.12 — Copy/paste a name with spaces and `&` — PASS
`h` (selection `src2`), `j j` → `selection=="b & c file.txt"`; `yy` →
`clipboard.files==["$FIXTURE/b & c file.txt"]`. Into `dest`, `pp` +
`wait_undo 4`: `undo_depth=4`; entries exactly `a.txt`, `b & c file.txt` —
name byte-identical. Screen renders `b & c file.txt` verbatim. Disk: file
exists, `cmp` identical to source.

### 03.13 — Multi-item cut of a marked set — PASS
`h`, `j j j j` → `selection=b.txt`; `Space Space` → `marked` = exactly
{b.txt, c.txt}, `selection=c.txt` (bottom clamp). `dd` →
`clipboard={"files":[".../b.txt",".../c.txt"],"op":"cut"}`, log INFO
`cut 2 items` (age 0.010s). `gg j l` → `cwd=$FIXTURE/src2`. `pp` +
`wait_undo 5`: `undo_depth=5`, `clipboard=null`, `marked=[]`; entries
exactly `a.txt_`, `b.txt`, `c.txt` in order; log INFO `paste 2 items,
overwrite = false`. Screen shows all three. Disk: both in `src2`, both gone
from `$FIXTURE`.

### 03.14 — Clipboard is global across tabs — PASS
In `src2`: `gg j` → `selection=b.txt`, `yy`. `gn` (polled seq stable):
`tabs` length 2, `focused=1`, `tabs[1].cwd=$FIXTURE/src2`,
`tabs[1].marked=[]`, and `clipboard` still
`{"files":[".../src2/b.txt"],"op":"copy"}` — global, survives the new tab.
Tab 2: `h`, `gg`, `l` into `dest`; `pp` + `wait_undo 6`: `undo_depth=6`
(global undo stack), `clipboard=null`; `entries 1 center` = exactly `a.txt`,
`b & c file.txt`, `b.txt`. Screen showed the `1 2` tab indicator in the
header row. Disk: `dest/b.txt` exists and `src2/b.txt` remains (copy).
Cleanup: verified `tabs` length 2 before pressing `q`; after: length 1,
`focused=0`, `cwd=$FIXTURE/src2`, `view=single`; screen back to the
single-tab Miller columns of `src2`.

## Bugs

None.

## Protocol feedback

1. 03.9's `log 5 contains "cut 1 items"` guidance: a naive `log 200` grep for
   the substring `cut` also matches unrelated lines (`CommandExecutor
   started`, the zoxide `Executing command ... current-dir` line). Harmless
   with a wide window, but the section could suggest grepping the exact
   phrase `cut N items` to avoid confusion.
2. 03.14 says "poll `state` until `seq` stabilizes (new tab loads async)" —
   worked as written; the new tab settled within ~2 polls at 0.2s. No change
   needed, just confirming the guidance is sufficient.
3. The section fixture's `c.txt` is 8 bytes (`charlie\n`) — the file-size
   column is a handy secondary identity check but is not asserted anywhere;
   fine as-is.

## Environment notes

- tmux, socat, jq present; zoxide present (isolated via `_ZO_DATA_DIR`, its
  `zoxide add` queue lines observed in the log as the section predicts —
  not treated as failures).
- No socket read ever timed out; no wedge observed.
- Teardown completed: session killed, socket removed, fixture parent and all
  temp dirs removed.
