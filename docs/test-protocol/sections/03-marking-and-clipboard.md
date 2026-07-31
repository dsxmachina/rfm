# Section 03 — Marking and clipboard

Harness per README (N=03, SESSION=rfm-sec03, SOCK=/tmp/rfm-sec03.sock).

**Section fixture** (create BEFORE launching rfm):

```bash
mkdir -p "$FIXTURE/dest" "$FIXTURE/src2"
printf 'alpha\n'   > "$FIXTURE/a.txt"
printf 'bravo\n'   > "$FIXTURE/b.txt"
printf 'charlie\n' > "$FIXTURE/c.txt"
printf 'spaces\n'  > "$FIXTURE/b & c file.txt"
```

Sorted visible order in `$FIXTURE` (dirs first, then lowercase-name order;
space sorts before `.`): `dest`, `src2`, `a.txt`, `b & c file.txt`, `b.txt`,
`c.txt` — six entries, initial selection is `dest` (`selected_idx == 0`).

**Section helper — poll for async paste** (paste records its undo transaction
from a background task; `await-idle` does NOT cover it; poll `undo_depth`):

```bash
# wait_undo <target-depth>
wait_undo() {
  for i in $(seq 1 100); do
    d=$(echo state | socat - UNIX-CONNECT:$SOCK | jq .undo_depth)
    [ "$d" -ge "$1" ] && return 0
    sleep 0.1
  done
  return 1
}
```

After `wait_undo` succeeds, still run one `await-idle` before asserting
`entries`/screen (the reload triggered by the landed transaction must drain).

**Log assertions (steps 03.7/03.10/03.11):** assert `paste`/`copy`/`cut` INFO
lines on the socket `log`, queried with a **wide window (`log 200`) IMMEDIATELY
after the action** — under `--debug-socket` verbosity is TRACE and reload TRACE
lines flood a short `log 5`/`log 10` window, evicting the INFO line within a
tick. The quiet fixture parent (README) keeps the window from filling with
`/tmp` churn, but the wide-window-right-after rule still applies. Sample the
on-screen log widget 2–3× before asserting a line is *absent* (a single capture
can race a redraw).

Facts this section relies on (verified in source, `src/panel/manager.rs`
`Command::{Mark,Cut,Copy,Paste}` arms; `src/util.rs` `get_destination`/
`move_item`/`copy_item`; `src/panel/directory.rs` `mark_selected_item`):

- Space **toggles** the mark on the selected entry, then moves the cursor down
  (clamped at the bottom).
- Marked rows render with a leading `x` in the first cell of the row (color
  dark-yellow — `capture-pane -p` strips color, so assert the `x` character;
  use `capture-pane -e` only if you need the color/highlight escapes).
- `Esc` in normal mode unmarks everything.
- Cut/copy use marked-or-selected: with nothing marked, the current selection
  is **auto-marked as a side effect** and used — after a bare `yy`,
  `state.marked` contains the selection. Cut/copy do NOT unmark afterwards.
- Paste (`pp`) takes the clipboard (`state.clipboard` becomes `null`), unmarks
  everything in the focused tab, and runs move/copy in a blocking task.
- Collision without overwrite: target name gets `_` appended (repeatedly)
  until free — `a.txt` → `a.txt_` (suffix on the FULL name, incl. extension).
- Cut+paste into the source's own directory is a no-op per file (warn log
  `from and to are identical`); the resulting empty transaction is NOT
  recorded (undo_depth unchanged).
- Entering a directory queues `zoxide add` if zoxide is on PATH — harmless
  (isolated by `_ZO_DATA_DIR`), but it may appear in `queue_*`/`log`; do not
  treat it as a failure.
- Do NOT assert mark state of a directory you navigated away from and back
  to — panel caching may or may not preserve stale marks; only assert marks
  in the current center panel right after acting on it.

---

### 03.1 — Baseline: no marks, empty clipboard

**Action:** none (fresh launch); `echo state | socat - UNIX-CONNECT:$SOCK` and `echo "entries center" | socat - UNIX-CONNECT:$SOCK`

**Expect (socket):** `mode=="normal"`, `cwd=="$FIXTURE"`, `selection=="dest"`, `selected_idx==0`, `total==6`, `marked==[]`, `clipboard==null`, `undo_depth==0`, `redo_depth==0`. `entries center`: 6 entries in order `dest, src2, a.txt, b & c file.txt, b.txt, c.txt`, all `marked:false`, exactly `dest` has `selected:true`.

**Expect (screen):** center pane lists all six names; no row has a leading `x` marker.

**Note:** if the center still shows a loading placeholder, poll `state` until `seq` stabilizes before asserting.

### 03.2 — Space marks and auto-advances

**Action:** `tmux send-keys -t $SESSION j j` (→ `a.txt`), then `tmux send-keys -t $SESSION Space`; await-idle.

**Expect (socket):** `state.marked == ["$FIXTURE/a.txt"]` (absolute path); `selection=="b & c file.txt"`, `selected_idx==3` (cursor auto-advanced). `entries center`: `a.txt` has `marked:true, selected:false`; `b & c file.txt` has `selected:true, marked:false`.

**Expect (screen):** the `a.txt` row shows a leading `x` in its first column; no other row does.

### 03.3 — Mark multiple

**Action:** `tmux send-keys -t $SESSION Space Space` (marks `b & c file.txt`, advances to `b.txt`; marks `b.txt`, advances to `c.txt`); await-idle.

**Expect (socket):** `state.marked` contains exactly (as a set) `$FIXTURE/a.txt`, `$FIXTURE/b & c file.txt`, `$FIXTURE/b.txt`; `selection=="c.txt"`.

**Expect (screen):** rows `a.txt`, `b & c file.txt`, `b.txt` each show the leading `x`; `c.txt` does not.

### 03.4 — Space toggles a mark off

**Action:** `tmux send-keys -t $SESSION k` (up to `b.txt`), then `Space`; await-idle.

**Expect (socket):** `state.marked` is exactly `{$FIXTURE/a.txt, $FIXTURE/b & c file.txt}` — `b.txt` was toggled OFF; `selection=="c.txt"` (auto-advance still happens on unmark).

**Expect (screen):** `x` on `a.txt` and `b & c file.txt` only; `b.txt` row has a space in the marker column again.

### 03.5 — Esc unmarks all

**Action:** `tmux send-keys -t $SESSION Escape`; await-idle.

**Expect (socket):** `marked==[]`; `mode=="normal"` (Esc in normal mode is not a mode change); `clipboard` still `null`.

**Expect (screen):** no row shows a leading `x`.

### 03.6 — Copy bare selection (`yy`) — auto-mark side effect + clipboard

**Action:** `tmux send-keys -t $SESSION g g` (top → `dest`), `tmux send-keys -t $SESSION j j` (→ `a.txt`), then `tmux send-keys -t $SESSION y y`; await-idle.

**Expect (socket):** `clipboard == {"files":["$FIXTURE/a.txt"],"op":"copy"}`; `marked==["$FIXTURE/a.txt"]` (marked-or-selected auto-marked the selection; copy does not unmark); `undo_depth` still `0` (copy alone records nothing). `log 5` contains an INFO line `copying 1 items`.

**Expect (screen):** `a.txt` row shows the leading `x`.

### 03.7 — Paste into another directory (async; undo_depth poll)

**Action:** `tmux send-keys -t $SESSION g g` (→ `dest`), `tmux send-keys -t $SESSION l` (enter `dest` — empty dir), await-idle; assert `cwd=="$FIXTURE/dest"`, `total==0`, `selection==null`. Then `tmux send-keys -t $SESSION p p`; run `wait_undo 1`; await-idle.

**Expect (socket):** `undo_depth==1`; `clipboard==null` (taken by paste); `marked==[]` (paste unmarks). `entries center`: exactly one entry `a.txt`, `selected:true`. `log 10` contains `paste 1 items, overwrite = false`.

**Expect (screen):** center pane shows `a.txt`; header/path row shows `.../dest`.

**Disk:** `[ -f "$FIXTURE/dest/a.txt" ] && [ -f "$FIXTURE/a.txt" ]` (copy: source remains) and `cmp -s "$FIXTURE/dest/a.txt" "$FIXTURE/a.txt"`.

**Note:** `await-idle` alone is NOT sufficient here — the paste task and its undo hand-back are async; always `wait_undo` first.

### 03.8 — Paste collision appends `_` to the full name

**Setup:** still in `$FIXTURE/dest`, selection on `a.txt`.

**Action:** `tmux send-keys -t $SESSION y y` (clipboard = copy `dest/a.txt`), await-idle; then `tmux send-keys -t $SESSION p p`; `wait_undo 2`; await-idle.

**Expect (socket):** `undo_depth==2`; `clipboard==null`. `entries center`: exactly `a.txt` and `a.txt_` (in that order — the collision suffix goes AFTER the extension: `a.txt_`, NOT `a_.txt`).

**Expect (screen):** both `a.txt` and `a.txt_` visible in the center pane.

**Disk:** `[ -f "$FIXTURE/dest/a.txt" ] && [ -f "$FIXTURE/dest/a.txt_" ]` and `cmp -s "$FIXTURE/dest/a.txt" "$FIXTURE/dest/a.txt_"`.

### 03.9 — Cross-directory cut moves the file

**Action:** `tmux send-keys -t $SESSION G` (bottom → `a.txt_`), then `tmux send-keys -t $SESSION d d`; await-idle; assert `clipboard == {"files":["$FIXTURE/dest/a.txt_"],"op":"cut"}` and `log 5` contains `cut 1 items`. Then `tmux send-keys -t $SESSION h` (back to `$FIXTURE`; selection restored to `dest`), `tmux send-keys -t $SESSION j` (→ `src2`), `tmux send-keys -t $SESSION l` (enter `src2`, empty), await-idle. Then `tmux send-keys -t $SESSION p p`; `wait_undo 3`; await-idle.

**Expect (socket):** `undo_depth==3`; `clipboard==null`; `cwd=="$FIXTURE/src2"`. `entries center`: exactly one entry `a.txt_`.

**Expect (screen):** center pane shows `a.txt_`.

**Disk:** `[ -f "$FIXTURE/src2/a.txt_" ] && ! [ -e "$FIXTURE/dest/a.txt_" ]` (cut = source removed).

### 03.10 — Cut + paste into the same directory is a no-op

**Setup:** in `$FIXTURE/src2`, selection on `a.txt_`.

**Action:** `tmux send-keys -t $SESSION d d`, await-idle (clipboard = cut `src2/a.txt_`); then `tmux send-keys -t $SESSION p p`; sleep ~1s (there is no undo_depth bump to wait for — the transaction is empty); await-idle.

**Expect (socket):** `undo_depth` STILL `3` (empty transactions are not recorded); `clipboard==null` (paste still took it); `log 10` contains a WARN line with message `from and to are identical` and an INFO `paste 1 items, overwrite = false`. `entries center`: still exactly one entry `a.txt_` — no `a.txt__` created.

**Expect (screen):** center pane shows only `a.txt_`.

**Disk:** `[ -f "$FIXTURE/src2/a.txt_" ] && ! [ -e "$FIXTURE/src2/a.txt__" ]`.

### 03.11 — Paste with empty clipboard is inert

**Action:** note current `seq`; `tmux send-keys -t $SESSION p p`; await-idle.

**Expect (socket):** `clipboard==null`, `undo_depth==3`, `marked==[]`; `seq` increased (input was processed). `log 5`: NO new `paste N items` line — the only `paste 1 items` line is the one from 03.10 with `age_secs` clearly larger than this step's elapsed time (compare `age_secs` against when you pressed the keys; a fresh line would have `age_secs` < ~2).

**Expect (screen):** unchanged (`a.txt_` only).

### 03.12 — Copy/paste a name with spaces and `&`

**Action:** `tmux send-keys -t $SESSION h` (→ `$FIXTURE`, selection `src2`), `tmux send-keys -t $SESSION j j` (→ `a.txt` → `b & c file.txt`), await-idle; assert `selection=="b & c file.txt"`. Then `tmux send-keys -t $SESSION y y`; await-idle; assert `clipboard.files==["$FIXTURE/b & c file.txt"]`. Then `tmux send-keys -t $SESSION g g` (→ `dest`), `tmux send-keys -t $SESSION l` (enter `dest`), await-idle; then `tmux send-keys -t $SESSION p p`; `wait_undo 4`; await-idle.

**Expect (socket):** `undo_depth==4`; `entries center` in `dest`: exactly `a.txt` and `b & c file.txt` — the name is byte-identical, spaces and `&` intact (paste uses native fs calls, but a mangled name here would betray a path-handling bug).

**Expect (screen):** both names visible; `b & c file.txt` rendered verbatim.

**Disk:** `[ -f "$FIXTURE/dest/b & c file.txt" ] && cmp -s "$FIXTURE/dest/b & c file.txt" "$FIXTURE/b & c file.txt"`.

### 03.13 — Multi-item cut of a marked set

**Action:** `tmux send-keys -t $SESSION h` (→ `$FIXTURE`, selection `dest`), `tmux send-keys -t $SESSION j j j j` (dest→src2→a.txt→`b & c file.txt`→`b.txt`), await-idle; assert `selection=="b.txt"`. Then `tmux send-keys -t $SESSION Space Space` (marks `b.txt`, advance to `c.txt`; marks `c.txt`, cursor clamps at bottom — stays on `c.txt`); await-idle; assert `marked` == `{$FIXTURE/b.txt, $FIXTURE/c.txt}` and `selection=="c.txt"`. Then `tmux send-keys -t $SESSION d d`; await-idle; assert `clipboard=={"files":[".../b.txt",".../c.txt"],"op":"cut"}` and `log 5` contains `cut 2 items`. Then `tmux send-keys -t $SESSION g g` `j` `l` (top→`dest`, down→`src2`, enter), await-idle; assert `cwd=="$FIXTURE/src2"`. Then `tmux send-keys -t $SESSION p p`; `wait_undo 5`; await-idle.

**Expect (socket):** `undo_depth==5`; `clipboard==null`; `marked==[]`. `entries center` in `src2`: exactly `a.txt_`, `b.txt`, `c.txt` (that order). `log 10` contains `paste 2 items, overwrite = false`.

**Expect (screen):** the three file names in the center pane.

**Disk:** `[ -f "$FIXTURE/src2/b.txt" ] && [ -f "$FIXTURE/src2/c.txt" ] && ! [ -e "$FIXTURE/b.txt" ] && ! [ -e "$FIXTURE/c.txt" ]`.

**Note:** the auto-advance-at-bottom clamp is expected behavior, not a stuck cursor. Both cut items keep their `x` markers on screen until the paste unmarks (in the target tab) — do not assert marks in `$FIXTURE` after navigating away.

### 03.14 — Clipboard is global across tabs

**Action:** in `$FIXTURE/src2`: `tmux send-keys -t $SESSION g g j` (top `a.txt_`, down → `b.txt`), await-idle; assert `selection=="b.txt"`. Then `tmux send-keys -t $SESSION y y`; await-idle. Then `tmux send-keys -t $SESSION g n` (new tab); await-idle; poll `state` until `seq` stabilizes (new tab loads async).

**Expect (socket, after `gn`):** `tabs` has length 2, `focused==1`, `tabs[1].cwd=="$FIXTURE/src2"` (inherited), `tabs[1].marked==[]` (fresh tab, marks are per-tab); **`clipboard` is still `{"files":["$FIXTURE/src2/b.txt"],"op":"copy"}`** — the clipboard is manager-global, not per-tab.

**Action (cont.):** `tmux send-keys -t $SESSION h` (tab 2 → `$FIXTURE`), `tmux send-keys -t $SESSION g g` (→ `dest`), `tmux send-keys -t $SESSION l` (enter `dest`), await-idle; then `tmux send-keys -t $SESSION p p`; `wait_undo 6`; await-idle.

**Expect (socket):** `undo_depth==6` (undo stack is global too); `clipboard==null`; `entries 1 center` (tab index 1): exactly `a.txt`, `b & c file.txt`, `b.txt`.

**Disk:** `[ -f "$FIXTURE/dest/b.txt" ] && [ -f "$FIXTURE/src2/b.txt" ]`.

**Action (cleanup within section):** `tmux send-keys -t $SESSION q` (closes tab 2 — NOT quit, since 2 tabs are open); await-idle.

**Expect (socket):** `tabs` length 1, `focused==0`, `cwd=="$FIXTURE/src2"` (tab 1's cwd), `view=="single"`.

**Expect (screen):** single-tab Miller columns of `src2` again.

**Note:** press `q` only while `tabs` length is 2 — on the last tab `q` quits rfm. Deep tab semantics (split view, per-tab marked isolation under mutation, cross-tab source refresh after cut) live in section 06.

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$FIXTURE" "$CFG"; rm -f $SOCK` (plus the XDG/_ZO temp dirs per README).

**Section coverage gaps** (deliberately not covered here):
- `paste_overwrite` (`po` / `ctrl-V`) — not exercised at all; note that in current source the `overwrite` flag only reaches the log line, not `move_item`/`copy_item`, so its behavior vs. plain paste is questionable (worth a dedicated investigation step elsewhere).
- Alternate bindings `ctrl-x`/`ctrl-c`/`ctrl-v` and the word-bindings `cut`/`copy`/`paste` — only `dd`/`yy`/`pp` are driven (binding-equivalence is a config concern, section on keybindings/config).
- `n`/`N` (select next/previous marked item) and search-driven marking (`/` sets the same `is_marked` flag) — search section.
- Cutting/copying **directories** (recursive copy via `fs_extra`) — only regular files here.
- Paste failure paths (`Failed to move/copy ... :` error log, e.g. unwritable target, vanished source).
- Undoing/redoing the pastes performed here (undo section); per-tab mark isolation and cross-tab source-pane refresh after a cross-tab cut (section 06).
- Marks surviving hidden-file toggling and marks on hidden entries.
- Clipboard contents pointing at files deleted/renamed between cut and paste.
