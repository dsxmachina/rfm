# Run report — Section 02 (Directory navigation)

**Environment:** tmux 120x30, terminal graphics protocol resolved to `half-block`
(inside tmux, expected). Tools present: `zoxide`, `socat`, `tmux`. `~/Music`,
`~/Pictures`, `~/Documents` MISSING (only `~/Downloads` exists) → 02.13 used
`gm`→~/Music. Binary: `./target/debug/rfm` (branch develop). No ERROR/WARN lines
in the socket log across the whole run.

## Step tally

| Step | Result | Note |
|------|--------|------|
| 02.1 | PASS | mode/view/cwd/selection/idx/total/show_hidden/marked/left_path/preview_path all match; `entries center` = 7 in order, `.hidden-*` hidden:true, alpha sole selected; screen shows 5 visible + nested preview + parent left. |
| 02.2 | PASS | cwd=alpha, selection=nested, total=1; `entries left` alpha selected; log has `move-right`, zoxide add executed+completed; screen center=nested, right=deep. |
| 02.3 | PASS | Right arrow → cwd=nested, selection=deep, preview=deep; screen center=deep left=nested right=leaf.txt. |
| 02.4 | PASS | cwd=deep, selection=leaf.txt, total=1; preview text `leaf content` rendered after async settle. |
| 02.5 | PASS | `h` → cwd=nested, selection=deep restored; log `move-left`; right=leaf.txt. |
| 02.6 | PASS | `l` re-descend → cwd=deep, selection=leaf.txt; log `pop rev-history`; layout matches 02.4. |
| 02.7 | PASS | Left×3 → cwd=$FIXTURE, selection=alpha, idx=0, total=5, left_path=/tmp; layout = 02.1. |
| 02.8 | PASS | `jj`→emptydir idx=2 preview shows `(empty)`; `l`→cwd=emptydir selection=null total=0 center `(empty)`; `h`→back to emptydir selected. |
| 02.9 | PASS | `k`+`l` into "Bilder & Videos" → cwd correct, selection=clip.txt; zoxide log path is shell-quoted `'…/Bilder & Videos'`, completed OK, NO exit-code failure; `h`→selection="Bilder & Videos". |
| 02.10 | PASS | `gr` → cwd=/, selection=bin (non-null), log `jump-to /`; screen shows etc AND usr. |
| 02.11 | PASS | `''` → cwd=$FIXTURE, selection=alpha, log `jump-to <FIXTURE>`; layout = 02.1. |
| 02.12 | PASS | `gh`→cwd=$HOME (top row shows HOME path); `''`→cwd=$FIXTURE. |
| 02.13 | PASS | `gm`→~/Music (missing): cwd/selection unchanged, seq increased (369→371), log `jump-to /home/someone/Music`; screen unchanged. |
| 02.14 | PASS | `gg`+`zh` → show_hidden=true, total=7, selection=alpha idx=1; `entries center` alpha sole selected; screen shows .hidden-dir + .hidden.txt. |
| 02.15 | PASS | `zh` → show_hidden=false, total=5, selection=alpha idx=0; no dotfiles in center. |
| 02.16 | PASS | Hidden ON + navigate to `.hidden.txt`, `zh` OFF re-clamps to `a.txt` (visible, hidden:false, sole selected), preview_path==cwd/a.txt (consistent). No dotfiles on screen. |
| 02.17 | PASS | `cd` → mode=console, cwd=$FIXTURE; overlay band with rule lines and the path text containing `tmp.aI80Nur0Eb/` (+ grey completion `alpha`). |
| 02.18 | PASS | type `alpha` → live nav cwd=$FIXTURE/alpha (still console); Enter → mode=normal, cwd stays; overlay gone, center=nested. |
| 02.19 | PASS | `cd`+BSpace → cwd=$FIXTURE (live parent, still console); Enter → normal, cwd=$FIXTURE; overlay path line shows the $FIXTURE path. |
| 02.20 | PASS | `cd`+`alpha`→cwd=$FIXTURE/alpha; Escape → normal, cwd rolled back to $FIXTURE. |
| 02.21 | PASS | `cd`+Tab → console, cwd=$FIXTURE/alpha (∈{alpha,Bilder & Videos,emptydir}); line shows completed subdir + `/`; Escape → normal, cwd=$FIXTURE. |
| 02.22 | PASS | `CD`+`zoxtarget` → console, cwd=$ZOXTARGET (live); log `zoxide query 'zoxtarget' -> 1 results`; band shows query + resolved path; Escape → normal, cwd=$FIXTURE. |
| 02.23 | PASS | `CD`+`zoxtarget`+Enter → normal, cwd=$ZOXTARGET, center `(empty)` total=0; `''` → cwd=$FIXTURE. |
| 02.24 | SKIP | N/A — conditional on zoxide NOT on PATH; zoxide IS installed. |
| 02.25 | PASS | `gg`+`ma` → jump_marks {"a":$FIXTURE}, log `jump-mark 'a' set`; `l` into alpha; `'a` → cwd=$FIXTURE, selection=alpha; screen alpha selected. |

**Totals:** 24 passed, 0 failed, 1 skipped (02.24 N/A).

## Bug reports

None. No failing steps.

## Protocol feedback

- 02.17 / 02.19 expectation wording ("path text ending with a trailing slash",
  "shows the $FIXTURE path with trailing `/`") is imprecise under `capture-pane`:
  the console pre-fills a directory autocompletion, so the visible line is
  `<cwd>/<grey-completion>` (e.g. `…/tmp.aI80Nur0Eb/alpha`) — the trailing slash
  is *between* the base path and the completion, not at end-of-line, and the grey
  color is invisible in a plain capture. The base path with `/` IS present, so the
  assertion holds, but the step should say "the base path (up to and including the
  trailing `/`) appears, possibly followed by a completion suggestion" to avoid a
  false FAIL by a literal reader.
- 02.8: `state.preview_path` for an empty directory is the sentinel string
  `"path-of-empty-panel"` (not a real path); harmless and the step doesn't assert
  it, but worth documenting so a future step doesn't mis-assert on preview_path
  for empty dirs. Same sentinel appears as `left_path` at `/` in 02.10.
- 02.13: needed a machine with a missing default jump target; on this box only
  `~/Downloads` exists so `gm`→~/Music worked. Fine, but the step could note that
  at least one of the four is expected missing on a typical dev box.
- All steps were reproducible on the first pass; no flakiness observed. Async
  preview polling (seq-stabilize) was only needed at 02.4/02.6/02.16 and settled
  within one poll.
