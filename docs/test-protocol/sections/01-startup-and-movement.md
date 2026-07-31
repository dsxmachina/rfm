# Section 01 — Startup and basic movement

Covers: launch, initial screen layout (Miller columns, header, footer), initial
selection, cursor movement (`j`/`k`/`h`/`l`, arrow keys, `gg`/`G`, page keys),
seq/idle invariants, and quitting (`Q`, and `q` on the last tab).

Harness per README (`N=01`, `SESSION=rfm-sec01`, `SOCK=/tmp/rfm-sec01.sock`).

## Section fixture

Created before launch (in addition to the standard `FIXTURE=$(mktemp -d)`):

```bash
mkdir "$FIXTURE/Bilder & Videos" "$FIXTURE/many" "$FIXTURE/subdir_a"
touch "$FIXTURE/Bilder & Videos/clip.txt" "$FIXTURE/subdir_a/inner.txt"
for i in $(seq -w 0 39); do touch "$FIXTURE/many/f$i"; done
touch "$FIXTURE/.hidden.txt" "$FIXTURE/alpha.txt" "$FIXTURE/bravo.txt" "$FIXTURE/charlie.txt"
FIXREAL=$(realpath "$FIXTURE")   # header shows canonicalized paths
```

Sorting facts baked into all expectations below (source:
`src/content.rs` sorts case-insensitively by name, then stable-sorts
directories first):

- Visible order in `$FIXTURE` (show_hidden=false):
  `Bilder & Videos`, `many`, `subdir_a`, `alpha.txt`, `bravo.txt`, `charlie.txt`
  → **6 visible entries**, `state.total == 6`.
- The `entries center` reply additionally contains `.hidden.txt`
  (`hidden:true`), sorted after the directories and before `alpha.txt`
  (leading `.` sorts before letters) → **7 entries in the array**. Never index
  the `entries` array with `state.selected_idx`.
- Initial selection is the first *directory*: `Bilder & Videos` (not
  `alpha.txt`).

Geometry facts (tmux `-x 120 -y 30`, log widget hidden): panel rows are
`y=1..27` inclusive → **panel height H = 27**; half page = 13. (Source:
`MillerColumns::from_size` reserves row 0 for the header and the last row for
the footer, `panel_y_range` reserves 1 more row for the collapsed log; page
distances in `manager.rs` `Move::PageForward => move_down(panel_height)`,
`HalfPageForward => move_down(panel_height/2)`.) If a step's exact index
expectation fails by a constant offset for every page step, re-derive H from
the first `ctrl-f` result and re-check the *relative* deltas — a wrong H is a
harness-geometry issue, inconsistent deltas are an rfm bug.

Highlight-position technique used by the "Expect (screen)" items: plain
`capture-pane -p` strips attributes, so to locate the cursor row use
`tmux capture-pane -t $SESSION -p -e` and find the row whose cells carry the
reverse-video SGR (substring `[7m`; the selected row is printed with
`Attribute::Reverse`, `src/panel/directory.rs`). Exactly one row inside the
center column should carry it. Where a step says "highlight on `<name>`",
check that the `[7m`-carrying row of the center column contains `<name>`.

Launch (per README): export the isolation env in the pane, then
`./target/debug/rfm --debug-socket $SOCK --config $CFG $FIXTURE`, wait for
`$SOCK`.

---

### 01.1 — Launch: initial state and screen layout
**Action:** No input. `echo await-idle | socat - UNIX-CONNECT:$SOCK`, then `echo state | socat - UNIX-CONNECT:$SOCK`, `echo "entries center" | socat - UNIX-CONNECT:$SOCK`, `echo "entries left" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):**
- `mode == "normal"` (if it is `"decision-flow"`, the XDG_STATE_HOME isolation failed or defaults were dropped — that is a harness fault, fix isolation before continuing)
- `view == "single"`, `focused == 0`, `tabs` has exactly 1 element
- `cwd == $FIXREAL` (compare against realpath; rfm may canonicalize), `selection == "Bilder & Videos"`, `selected_idx == 0`, `total == 6`
- `marked == []`, `clipboard == null`, `show_hidden == false`, `undo_depth == 0`, `redo_depth == 0`
- `left_path` == parent directory of `$FIXREAL`
- `entries center`: 7-element array in order `Bilder & Videos`, `many`, `subdir_a`, `.hidden.txt`, `alpha.txt`, `bravo.txt`, `charlie.txt`; only `Bilder & Videos` has `selected:true`; only `.hidden.txt` has `hidden:true`; all `marked:false`
- `entries left`: the entry whose name is `basename $FIXTURE` has `selected:true`
**Expect (screen):**
- Row 1 (header): contains `@` (user@host prompt) and the path `$FIXREAL/Bilder & Videos` (path of the *selected* entry, not just cwd). No tab numbers on the right edge (single tab).
- Three columns: a narrow left column listing the parent dir (must include the fixture's basename), a center column showing exactly the 6 visible names in the order above (`.hidden.txt` absent), and a right (preview) column.
- Right column shows the directory listing of `Bilder & Videos`, i.e. contains `clip.txt`. This is an async panel load: if it looks empty/placeholder, poll `state` until `seq` stabilizes, then re-capture before asserting.
- Last row (footer): starts with a permissions string beginning with `d` (a directory is selected), contains the current username, and ends (right-aligned) with `1/6 `.
- Highlight (`-e` capture): reverse-video row in the center column contains `Bilder & Videos`.
**Note:** Do not assert exact permission bits (umask-dependent). Do not assert a mime type for a directory selection.

### 01.2 — Idle invariants: debug queries never advance seq
**Action:** `echo state | socat - UNIX-CONNECT:$SOCK` twice in a row (record `seq` from each), with `echo "entries center" | socat ...` and `echo "log 5" | socat ...` issued between them. No tmux input at all.
**Expect (socket):** Both `seq` values are **identical**. All other `state` fields identical to 01.1.
**Expect (screen):** `capture-pane -p` output byte-identical to the 01.1 capture (no repaint side effects from debug queries).
**Note:** This is the baseline for every later "seq increased" assertion. Never assert exact seq deltas (+~5 per keypress, not +1).

### 01.3 — `j` moves down
**Action:** Record `seq`. `tmux send-keys -t $SESSION j` → `await-idle` → `state`, `entries center`, `log 20`.
**Expect (socket):**
- `selection == "many"`, `selected_idx == 1`, `total == 6`, `mode == "normal"`
- `seq` strictly greater than the recorded value
- `entries center`: `selected:true` moved to `many`, all else unchanged
- `log` contains a TRACE line with message `move-down` and a TRACE `key-event:` line mentioning `Char('j')` (with `--debug-socket`, verbosity is TRACE)
**Expect (screen):** Header path now ends in `/many`. Footer right shows `2/6 `. Highlight on `many`. Preview column now shows the listing of `many` (contains `f00`); poll seq-stability before asserting the preview.

### 01.4 — Down arrow behaves like `j`
**Action:** `tmux send-keys -t $SESSION Down` → `await-idle` → `state`.
**Expect (socket):** `selection == "subdir_a"`, `selected_idx == 2`; `log` gains another `move-down` TRACE line.
**Expect (screen):** Footer `3/6 `, highlight on `subdir_a`, preview shows `inner.txt` (poll seq-stability).
**Note:** Arrow keys are hardcoded bindings (`CommandParser::new`, `src/engine/commands.rs`), independent of the `[keys.movement]` config.

### 01.5 — `k` and Up arrow move up
**Action:** `tmux send-keys -t $SESSION k` → `await-idle` → `state` (expect `many`). Then `tmux send-keys -t $SESSION Up` → `await-idle` → `state`, capture pane.
**Expect (socket):** After `k`: `selection == "many"`, `selected_idx == 1`. After Up: `selection == "Bilder & Videos"`, `selected_idx == 0`. `log` shows two `move-up` TRACE lines.
**Expect (screen):** Footer `1/6 `, highlight back on `Bilder & Videos`, header path ends in `/Bilder & Videos`.

### 01.6 — `k` at the top clamps (no wrap, no error)
**Action:** Record `seq`. `tmux send-keys -t $SESSION k` → `await-idle` → `state`, `log 10`.
**Expect (socket):** `selection == "Bilder & Videos"`, `selected_idx == 0` (unchanged — clamped, not wrapped to bottom). `seq` increased (the key WAS processed). No ERROR/WARN lines added to `log`.
**Expect (screen):** Identical center column and footer (`1/6 `) as 01.5's final capture.

### 01.7 — `G` jumps to the bottom; selected_idx vs entries-index pitfall
**Action:** `tmux send-keys -t $SESSION G` → `await-idle` → `state`, `entries center`.
**Expect (socket):**
- `selection == "charlie.txt"`, `selected_idx == 5` (0-based among the 6 VISIBLE entries)
- `entries center`: `charlie.txt` is at array position **6** (of 7, because `.hidden.txt` is in the array) and is the only `selected:true` entry. Assert explicitly that array position ≠ `selected_idx` here — this step exists to catch executors indexing `entries` with `selected_idx`.
**Expect (screen):** Footer `6/6 `, highlight on `charlie.txt`. Footer permissions string now starts with `-` (regular file). Preview column shows `charlie.txt`'s preview (empty file → blank preview pane is correct, not an error).

### 01.8 — `j` at the bottom clamps
**Action:** Record `seq`. `tmux send-keys -t $SESSION j` → `await-idle` → `state`.
**Expect (socket):** `selection == "charlie.txt"`, `selected_idx == 5` unchanged; `seq` increased; no new WARN/ERROR in `log`.
**Expect (screen):** Unchanged center column; footer still `6/6 `.

### 01.9 — `gg` chord: pending key buffer, then jump to top
**Action:** `tmux send-keys -t $SESSION g` → `await-idle` → `state` + capture pane. Then `tmux send-keys -t $SESSION g` (second g) → `await-idle` → `state` + capture.
**Expect (socket):** After first `g`: `mode == "normal"`, `selection` still `charlie.txt` (chord incomplete, nothing moved). After second `g`: `selection == "Bilder & Videos"`, `selected_idx == 0`.
**Expect (screen):** After first `g`: the pending key buffer `g` is rendered dark-grey near the horizontal center of the footer row (`draw_footer` prints `parser.buffer()` at `width/2`); footer still shows `6/6 `. After second `g`: buffer indicator gone, footer `1/6 `, highlight on `Bilder & Videos`.
**Note:** `g` is a chord prefix shared with `gT`/`gn`/`gh`… — a single `g` must never move by itself. If after the first `g` the buffer is not visible on screen but `state` is as expected, record it as a render finding, not a movement failure.

### 01.10 — `l` enters a directory (name with spaces and `&`)
**Action:** `tmux send-keys -t $SESSION l` → `await-idle` → `state`, `entries center`, `entries left`, `log 20`.
**Expect (socket):**
- `cwd == "$FIXREAL/Bilder & Videos"`, `selection == "clip.txt"`, `selected_idx == 0`, `total == 1`
- `left_path == $FIXREAL`
- `entries left`: `Bilder & Videos` has `selected:true` (parent column tracks where we came from)
- `log`: TRACE `move-right`. If `zoxide` is installed: an INFO `Executing command 'zoxide'...` (or `Queueing command 'zoxide'`) line, and NO subsequent `failed with exit code` line — the `&`-and-spaces dir name must survive shell escaping. If zoxide is absent, no such lines (skipped) — both outcomes pass, but a zoxide *failure* line is a bug.
**Expect (screen):** Header path ends in `/Bilder & Videos/clip.txt`. Left column now lists the fixture's entries with `Bilder & Videos` visible; center column shows only `clip.txt`; footer `1/1 `.

### 01.11 — `h` returns to the parent, selection restored
**Action:** `tmux send-keys -t $SESSION h` → `await-idle` → `state`, `log 10`.
**Expect (socket):** `cwd == $FIXREAL`, `selection == "Bilder & Videos"` (restored, not reset to top-of-list-by-default — here they coincide at idx 0, the restore is proven properly in 01.12), `total == 6`; TRACE `move-left` in `log`.
**Expect (screen):** Same layout as 01.1's capture: center shows the 6 visible names, footer `1/6 `, highlight on `Bilder & Videos`.

### 01.12 — Right/Left arrows navigate; selection restore proven off-index-0
**Action:** `tmux send-keys -t $SESSION j j` (two presses; selection → `subdir_a`) → `await-idle` → `state` (assert `selection == "subdir_a"`). Then `tmux send-keys -t $SESSION Right` → `await-idle` → `state`. Then `tmux send-keys -t $SESSION Left` → `await-idle` → `state` + capture.
**Expect (socket):** After Right: `cwd == "$FIXREAL/subdir_a"`, `selection == "inner.txt"`. After Left: `cwd == $FIXREAL`, `selection == "subdir_a"`, `selected_idx == 2` — the cursor is restored to the directory we descended from, NOT to index 0.
**Expect (screen):** Final capture: highlight on `subdir_a`, footer `3/6 `.

### 01.13 — Page forward (`ctrl-f`) in a long listing
**Setup:** Navigate into `many`: `tmux send-keys -t $SESSION k` (→ `many`), `await-idle`, assert `selection == "many"`; then `tmux send-keys -t $SESSION l`, `await-idle`, assert `cwd == "$FIXREAL/many"`, `selection == "f00"`, `total == 40`, footer `1/40 `.
**Action:** `tmux send-keys -t $SESSION C-f` → `await-idle` → `state` + capture.
**Expect (socket):** `selected_idx == 27` (`selection == "f27"`) — one full panel height H=27 down from index 0.
**Expect (screen):** Footer `28/40 `. Highlight on `f27`. The listing has scrolled: `f00` no longer necessarily visible; the visible window must contain `f27`.
**Note:** If H differs (see geometry facts above), assert `selected_idx == H` with the H derived here and reuse that H for 01.14–01.17.

### 01.14 — Page forward clamps at the bottom
**Action:** `tmux send-keys -t $SESSION C-f` → `await-idle` → `state` + capture.
**Expect (socket):** `selected_idx == 39`, `selection == "f39"` (27+27=54 clamps to last entry).
**Expect (screen):** Footer `40/40 `, highlight on `f39`, last visible row of the center column is `f39`.

### 01.15 — Page backward (`ctrl-b`)
**Action:** `tmux send-keys -t $SESSION C-b` → `await-idle` → `state`.
**Expect (socket):** `selected_idx == 12`, `selection == "f12"` (39 − 27).
**Expect (screen):** Footer `13/40 `, highlight on `f12`.

### 01.16 — Half page: `ctrl-u` (saturating) and `ctrl-d`
**Action:** `tmux send-keys -t $SESSION C-u` → `await-idle` → `state` (12 − 13 saturates at 0). Then `tmux send-keys -t $SESSION C-d` → `await-idle` → `state` + capture.
**Expect (socket):** After `C-u`: `selected_idx == 0`, `selection == "f00"`. After `C-d`: `selected_idx == 13`, `selection == "f13"` (half page = H/2 = 13).
**Expect (screen):** Final capture: footer `14/40 `, highlight on `f13`.

### 01.17 — PageDown/PageUp keys (hardcoded, unconfigured)
**Action:** `tmux send-keys -t $SESSION NPage` → `await-idle` → `state` (13+27=40 clamps to 39). Then `tmux send-keys -t $SESSION PPage` → `await-idle` → `state` + capture.
**Expect (socket):** After NPage: `selected_idx == 39`. After PPage: `selected_idx == 12`.
**Expect (screen):** Final footer `13/40 `, highlight on `f12`.
**Note:** tmux names: `NPage` = PageDown, `PPage` = PageUp. These are hardcoded `KeyCode::PageDown/PageUp` bindings in `CommandParser::new`, present even if a user unbinds `ctrl-f`/`ctrl-b`.

### 01.18 — `gg` to top in the long listing, then back out
**Action:** `tmux send-keys -t $SESSION g g` → `await-idle` → `state` (expect `f00`). Then `tmux send-keys -t $SESSION h` → `await-idle` → `state` + capture.
**Expect (socket):** After `gg`: `selected_idx == 0`, `selection == "f00"`. After `h`: `cwd == $FIXREAL`, `selection == "many"`, `selected_idx == 1`, `total == 6`.
**Expect (screen):** Footer `2/6 `, highlight on `many`.

### 01.19 — Quit with `Q`
**Action:** Record the pane's last-line shell prompt is NOT present (rfm footer is). `tmux send-keys -t $SESSION Q`. Then poll (up to ~5 s, 0.2 s interval) until `pgrep -f "rfm --debug-socket $SOCK"` returns nothing.
**Expect (socket):** A `state` query after exit must FAIL (socat connection refused or empty reply) — the socket is dead. (The socket *file* may still exist on disk; that is expected, teardown removes it.)
**Expect (screen):** `tmux capture-pane -t $SESSION -p` no longer shows the rfm layout (no `N/M ` footer counter, no three-column listing); the pane shows a shell prompt (the tmux session itself stays alive because the shell survives rfm). Terminal is sane: no raw-mode leftovers such as the alternate screen still active (prompt at the usual position, typed characters would echo).
**Note:** No `--choosedir` was passed, so nothing about a path should be printed on exit. `exit` is the other quit binding but is exercised as a typed command sequence elsewhere; here `Q` (single key) is the target.

### 01.20 — Relaunch; `q` on the last tab quits
**Setup:** `rm -f $SOCK`, then relaunch rfm in the same pane exactly as the section launch (same env exports still active in the pane's shell if the same pane is reused — otherwise re-export), wait for `until [ -S $SOCK ]`. Verify via `state`: `mode == "normal"`, `tabs` length 1.
**Action:** `tmux send-keys -t $SESSION q`. Poll until `pgrep -f "rfm --debug-socket $SOCK"` is empty.
**Expect (socket):** Socket dead (as in 01.19).
**Expect (screen):** Shell prompt back, rfm layout gone.
**Note:** `q` is bound to `close_tab` (`[keys.tabs]`), NOT `quit` — closing the *last* tab returns `CloseCmd::QuitWithPath` (`manager.rs close_tab`). With more than one tab open, `q` must NOT quit; that behavior belongs to the tabs section. If rfm is still running after `q`, check `state.tabs` length — a second tab would mean earlier steps leaked state (this section never creates tabs).

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$FIXTURE" "$CFG"; rm -f $SOCK` (plus the mktemp dirs exported as XDG_CACHE_HOME/XDG_STATE_HOME/_ZO_DATA_DIR).

**Section coverage gaps (deliberate):**
- Opening files with `l`/`Right` on a regular file (opener path) — covered by the opener section; this section only ever presses `l` on directories.
- `jump_previous` (`''`), `jump_to` chords (`gh`, `gr`, …), and jump-marks (`m<letter>` / `'<letter>`) — navigation-jumps section.
- `cd` console and zoxide query modes — console/modal section.
- Hidden-file toggling (`zh`) — only the *presence* of `.hidden.txt` in `entries` (and its absence on screen) is asserted here; toggling semantics live in the toggles/marking section.
- Tabs, split view, `q` with >1 tab, `ctrl-w`, tab-number header widget — tabs section.
- `--choosedir` output on quit, `quit_no_cd`, and `exit` (typed sequence quit binding) — CLI/session section.
- Resize behavior (`Event::Resize`, page-size change after resize) — not covered anywhere in this section; page math assumes the fixed 120x30 geometry.
- Preview *content* correctness (only "listing contains expected name" is asserted); preview backends have their own section.
- Movement while marked / search-`n`/`N` movement — marking and search sections.
