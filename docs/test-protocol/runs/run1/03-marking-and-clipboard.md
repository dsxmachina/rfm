# Run report — Section 03: Marking and clipboard

**Environment:** terminal 120x30 (tmux). All optional tools present (zoxide,
ffmpeg, openssl, zip, jq). Binary `./target/debug/rfm`. Config isolated via
`--config` scratch dir; XDG_CACHE/STATE and `_ZO_DATA_DIR` isolated per README.
Fixtures placed under the session scratchpad (stable path) instead of `mktemp -d`
after the shared scratchpad `env.sh` was repeatedly clobbered by parallel
executors — see protocol feedback. This only affected the parent/left pane
(cosmetic clutter); every assertion in this section targets the center pane and
was unaffected.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 03.1 | PASS | Baseline: mode=normal, cwd=fixture, selection=dest, idx 0, total 6, marked=[], clipboard=null, undo/redo=0; 6 entries in expected order, no `x`. |
| 03.2 | PASS | Space marks a.txt, marked=[a.txt], cursor auto-advanced to `b & c file.txt` (idx 3); `x` on a.txt row only. |
| 03.3 | PASS | Space Space marks `b & c file.txt` + `b.txt`; marked set = {a.txt, b & c file.txt, b.txt}, selection c.txt; `x` on the three rows. |
| 03.4 | PASS | k up to b.txt, Space toggles it OFF; marked = {a.txt, b & c file.txt}, selection auto-advanced to c.txt; `x` gone from b.txt. |
| 03.5 | PASS | Esc unmarks all; marked=[], mode still normal, clipboard null; no `x`. |
| 03.6 | PASS | `yy` on a.txt: clipboard={files:[a.txt],op:copy}, marked=[a.txt] (auto-mark), undo_depth 0; INFO "copying 1 items"; `x` on a.txt. |
| 03.7 | PASS | Paste copy into empty dest: undo_depth 1, clipboard null, marked []; center = a.txt (selected); INFO "paste 1 items, overwrite = false" present in history; disk: dest/a.txt & src a.txt both exist, cmp equal. |
| 03.8 | PASS | Collision paste: undo_depth 2, center = {a.txt, a.txt_}; suffix on full name (`a.txt_`); disk both exist, cmp equal. |
| 03.9 | PASS | Cross-dir cut: `dd` clipboard cut of dest/a.txt_, INFO "cut 1 items"; paste in src2 → undo_depth 3, center = a.txt_; disk: src2/a.txt_ present, dest/a.txt_ removed. |
| 03.10 | PASS | Cut+paste same dir no-op: undo_depth STILL 3, clipboard null; WARN "from and to are identical" + INFO "paste 1 items"; center = a.txt_ only, no a.txt__. |
| 03.11 | PASS | Paste empty clipboard inert: seq increased (101→105), clipboard null, undo_depth 3, marked []; no fresh paste line (only prior one, age ~16s); screen unchanged. |
| 03.12 | PASS | Copy/paste `b & c file.txt`: clipboard path byte-identical, undo_depth 4, dest center = {a.txt, b & c file.txt} verbatim; disk cmp equal. |
| 03.13 | PASS | Multi-item cut of {b.txt,c.txt}: cursor clamps at c.txt, clipboard cut both, INFO "cut 2 items"/"paste 2 items"; src2 center = {a.txt_, b.txt, c.txt}; disk moved, fixture originals gone. |
| 03.14 | PASS | `gn` new tab inherits src2 cwd, tab1 marked=[], clipboard still global copy of src2/b.txt; paste in tab2/dest → undo_depth 6, `entries 1 center` = {a.txt, b & c file.txt, b.txt}, disk copy (both remain); `q` closes tab 2 → tabs 1, focused 0, cwd src2, view single. |

**Tally:** 14 steps — 14 PASS, 0 FAIL, 0 SKIP.

## Bug reports

None. Every step matched both the socket belief and the screen ground truth.

## Notes on soft observations (not bugs)

- 03.7 / 03.10 / 03.11: the protocol says `log 10`/`log 5` should contain the
  `paste ... items` line. Because `--debug-socket` raises verbosity to TRACE,
  the many panel-update TRACE lines emitted by the reload push the INFO paste
  line beyond a 5–10 line window almost immediately. The line IS present in the
  200-line retention history (verified with `log 25`/`log 30`), so the
  assertion holds — but a strict `log 10` grep can miss it. Recorded as
  protocol feedback, not a bug.

## Protocol feedback

- The shared scratchpad `env.sh` pattern collides across parallel executors:
  three different sections' `env.sh` were observed overwriting each other
  (my `N=03` file was clobbered to `N=10`, then `N=09`), and my first launched
  session/socket vanished before step 03.1 could run. Recommend the harness
  give each executor a **section-specific** env filename
  (e.g. `env-<N>.sh`) or instruct executors never to share a state file in the
  scratchpad. Worked around here by writing `sec03-env.sh` (unique) and using
  fixed-path fixtures under the scratchpad.
- Steps that assert a specific `paste`/`copy`/`cut` INFO line via `log N` with
  a small N are fragile under the TRACE verbosity that `--debug-socket`
  forces: reload TRACE lines flood the window. Suggest the protocol either
  ask for a wider window (`log 20`+) or filter by message, or note explicitly
  that the line may need `log 20`+ to appear.
- 03.7 disk assertion and log wording confirmed exactly (`overwrite = false`).
  No ambiguity found elsewhere; the section's stated facts (suffix on full
  name, empty-transaction not recorded, clipboard global, marks per-tab,
  cursor clamp at bottom) all held precisely.
