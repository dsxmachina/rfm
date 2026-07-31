# Section 06 — Tabs and split view

N=06, SESSION=rfm-sec06, SOCK=/tmp/rfm-sec06.sock. Harness, launch and the
per-step verification loop are defined in `../README.md` — read that first.
Every step below implies: send input → `await-idle` → assert socket → assert
`tmux capture-pane -t $SESSION -p` (add `-e` when the assertion is about
highlight/dim attributes, which plain `-p` strips).

## Section fixture

On top of the standard harness setup (`FIXTURE`, `CFG`, isolation env in the
tmux pane), create:

```bash
mkdir "$FIXTURE/dirA" "$FIXTURE/dirB"
printf 'alpha\n'      > "$FIXTURE/dirA/a1.txt"
printf 'bravo one\n'  > "$FIXTURE/dirB/b1.txt"
printf 'root one\n'   > "$FIXTURE/r1.txt"
printf 'spaced\n'     > "$FIXTURE/spaced name.txt"
printf 'amp\n'        > "$FIXTURE/amp & file.txt"
```

Visible order in `$FIXTURE` (directories sort before files, then
alphabetical): `dirA`, `dirB`, `amp & file.txt`, `b1.txt`(*after 06.17 only*),
`r1.txt`, `spaced name.txt`. Initial selection is **dirA** (first directory),
never `amp & file.txt`.

Launch per README: `rfm --debug-socket $SOCK --config $CFG "$FIXTURE"` in a
120×30 tmux session. The steps are stateful in order; the section runs in
isolation but individual steps assume their predecessors ran.

Key source facts this section relies on (verified in
`src/panel/manager.rs` / `src/panel/mod.rs` / `examples/default-config.toml
[keys.tabs]`): `gn`=new_tab (MAX_TABS=4), `Tab`=focus_next (wraps),
`1`-`4`=focus_tab_N (1-based in config, 0-based in `state.focused`),
`q`/`ctrl-w`=close_tab (last tab quits), `!`=toggle_split,
MIN_SPLIT_WIDTH=40 columns, new tabs are appended at the END of `tabs[]` and
focused, cloning the *focused* tab's cwd; closing clamps focus
(`focus_after_close`: min(focused, len-1)).

---

### 06.1 — Baseline: one tab, single view
**Action:** no input; query `state` and `entries center`.
**Expect (socket):** `view=="single"`, `focused==0`, `tabs` has exactly 1
element, `tabs[0].cwd==$FIXTURE`, `cwd==$FIXTURE`, `selection=="dirA"`,
`selected_idx==0`, `tabs[0].marked==[]`. `entries center` lists `dirA`,
`dirB`, `amp & file.txt`, `r1.txt`, `spaced name.txt` with exactly one
`selected:true` (on `dirA`).
**Expect (screen):** header row (row 0) shows `user@host` + the path; the
top-right corner does NOT contain a tab-number strip like `1 2` (numbers are
only drawn with >1 tab). Three columns visible; center lists the fixture
entries; right column previews `dirA` (shows `a1.txt`).
**Note:** the preview loads async — if the right column looks like a loading
placeholder, poll `state` until `seq` is stable before asserting it.

### 06.2 — gn opens a second tab at the current directory and focuses it
**Action:** `tmux send-keys -t $SESSION g n`
**Expect (socket):** `tabs` length 2, `focused==1`, `tabs[1].cwd==$FIXTURE`
(cwd inherited), `tabs[1].selection=="dirA"` (fresh listing, first dir),
`tabs[0]` unchanged. `view=="single"`. `log 30` contains a TRACE line with
substring `command: new tab`.
**Expect (screen):** top-right of the header row now shows the tab strip
`1 2`. With `capture-pane -e`, the `2` carries the reverse-video/bold
highlight, the `1` is dark grey.
**Note:** the new tab's panels load async — poll `seq` stable before
asserting `entries 1 center`.

### 06.3 — Per-tab cwd is independent
**Action:** `tmux send-keys -t $SESSION j` (select `dirB`), then
`tmux send-keys -t $SESSION l` (enter it).
**Expect (socket):** `focused==1`, `cwd==$FIXTURE/dirB`,
`selection=="b1.txt"`, `tabs[1].cwd==$FIXTURE/dirB`, but
`tabs[0].cwd==$FIXTURE` and `tabs[0].selection=="dirA"` — untouched.
`entries 0 center` still lists the root fixture entries with `dirA`
selected; `entries 1 center` (or `entries center`) lists only `b1.txt`
(selected).
**Expect (screen):** center column shows `b1.txt`; header path ends in
`dirB/b1.txt`; tab strip still `1 2` with `2` focused. Right column shows the
file preview of `b1.txt` (line `bravo one`) once loaded.
**Note:** entering a dir queues `zoxide add` if zoxide is installed — a log
line `Executing command 'zoxide'...` is normal, not a failure.

### 06.4 — Marks are per-tab
**Action:** `tmux send-keys -t $SESSION Space`
**Expect (socket):** `state.marked==["$FIXTURE/dirB/b1.txt"]` (absolute
path); `tabs[1].marked` has that one entry; `tabs[0].marked==[]`.
`entries center` shows `b1.txt` with `marked:true` (and still
`selected:true` — auto-advance clamps at the last/only entry, cursor stays).
**Expect (screen):** `b1.txt` row rendered in the marked color (with `-e`:
dark-yellow); still highlighted as the cursor row.

### 06.5 — Direct focus with `1`; scalar state mirrors the focused tab
**Action:** `tmux send-keys -t $SESSION 1`
**Expect (socket):** `focused==0`, `cwd==$FIXTURE`, `selection=="dirA"`,
`marked==[]` (top-level scalars mirror tab 0 now). `tabs[1].marked` STILL
contains `.../dirB/b1.txt` (per-tab persistence), and `entries 1 center`
still shows `b1.txt` `marked:true`. `log 30` contains TRACE
`command: focus tab 1`.
**Expect (screen):** center shows the root listing with `dirA` highlighted;
tab strip `1 2` with `1` now highlighted (verify via `-e`).

### 06.6 — Tab cycles focus with wrap; preview is refreshed on focus change
**Action:** `tmux send-keys -t $SESSION Tab` → verify; then
`tmux send-keys -t $SESSION Tab` again → verify.
**Expect (socket):** after first Tab: `focused==1`, `cwd==$FIXTURE/dirB`;
poll `state` until `preview_path=="$FIXTURE/dirB/b1.txt"` and `seq` stable.
After second Tab: `focused==0` (wrapped 1→0 with 2 tabs),
`preview_path=="$FIXTURE/dirA"`. `log 30` contains TRACE
`command: focus next tab` (twice).
**Expect (screen):** after first Tab the right column shows the `b1.txt`
preview (`bravo one`); after the second it shows the `dirA` directory
listing (`a1.txt`). Tab strip highlight follows.
**Note:** this asserts `refresh_focused_preview` on focus change — a stale
right column here (socket `preview_path` correct but screen showing the old
preview) is a render-path bug; both stale is a state-path bug.

### 06.7 — Grow to 4 tabs
**Action:** `tmux send-keys -t $SESSION g n`, verify, then
`tmux send-keys -t $SESSION g n`, verify.
**Expect (socket):** `tabs` length 3 then 4; after each `gn` `focused` is the
new last index (2, then 3); `tabs[2].cwd==$FIXTURE` and
`tabs[3].cwd==$FIXTURE` (cloned from the focused tab, which was tab 0 /
then tab 2 — both at `$FIXTURE`). `tabs[1].cwd` still `$FIXTURE/dirB`.
**Expect (screen):** tab strip `1 2 3` then `1 2 3 4`, focused number
highlighted.

### 06.8 — MAX_TABS refusal
**Action:** `tmux send-keys -t $SESSION g n`
**Expect (socket):** `tabs` length STILL 4, `focused` unchanged (3). `log 20`
contains a WARN line `max 4 tabs reached`.
**Expect (screen):** unchanged tab strip `1 2 3 4`; if within 10 s of the
keypress, the warning is visible in the log widget at the bottom.

### 06.9 — Direct focus `3` and `4`
**Action:** `tmux send-keys -t $SESSION 3` → verify `focused==2`; then
`tmux send-keys -t $SESSION 4` → verify `focused==3`.
**Expect (socket):** `focused` as above; `cwd==$FIXTURE` both times (tabs 2
and 3 are root clones). `log 30` contains TRACE `command: focus tab 3` and
`command: focus tab 4`.
**Expect (screen):** highlighted tab number moves 3 → 4.

### 06.10 — q closes the focused (non-last) tab; focus clamps back
**Action:** `tmux send-keys -t $SESSION q`
**Expect (socket):** `tabs` length 3, `focused==2` (was 3; clamp
min(3, 3-1)), `cwd==$FIXTURE` (the surviving clone, old tab index 2).
rfm is still running (`state` answers). `log 20` contains TRACE
`command: close tab`.
**Expect (screen):** tab strip `1 2 3`, `3` highlighted; still the full
single-view Miller layout.

### 06.11 — ctrl-w also closes a tab
**Action:** `tmux send-keys -t $SESSION C-w`
**Expect (socket):** `tabs` length 2, `focused==1`, and — because the
surviving tab index 1 is the dirB tab — `cwd==$FIXTURE/dirB`,
`selection=="b1.txt"`. `tabs[0].cwd==$FIXTURE`.
**Expect (screen):** tab strip `1 2` with `2` highlighted; center shows
`b1.txt`; right column previews it (poll `seq` stable).

### 06.12 — Split refused when the terminal is too narrow
**Setup:** `tmux resize-window -t $SESSION -x 30 -y 30`, then
`echo await-idle | socat - UNIX-CONNECT:$SOCK` (rfm handles the Resize
event).
**Action:** `tmux send-keys -t $SESSION '!'`
**Expect (socket):** `view=="single"` (unchanged), `tabs` length STILL 2 (no
orphan tab created — the width check runs before tab creation; here 2 tabs
already exist, so also assert no third appeared). `log 20` contains WARN
`terminal too narrow for split view` and TRACE `command: toggle split`.
**Expect (screen):** still a (cramped) single view, no divider `│` column
splitting two listings.
**Cleanup:** `tmux resize-window -t $SESSION -x 120 -y 30`, then
`await-idle`; confirm `state` answers and the layout is back to normal
width.
**Note:** MIN_SPLIT_WIDTH is 40 columns (src/panel/mod.rs); 30 is safely
below. If the local tmux lacks `resize-window` (< 2.9), skip this step and
record it as skipped.

### 06.13 — `!` enters split view: two center columns + divider, no preview
**Action:** `tmux send-keys -t $SESSION '!'`
**Expect (socket):** `view=="split"`, `tabs` length 2 (no auto-create — two
tabs already existed), `focused==1`. `log 20` contains TRACE
`command: toggle split`.
**Expect (screen):** two directory listings side by side with a vertical `│`
divider column at x≈60: the LEFT half is tab 1's center (`dirA`, `dirB`,
`amp & file.txt`, `r1.txt`, `spaced name.txt`), the RIGHT half is tab 2's
center (`b1.txt`). The first panel row therefore contains, left to right:
`dirA` … `│` … `b1.txt`. NO preview column: the text `bravo one` (b1.txt's
file preview) must NOT be on screen, nor `a1.txt` (dirA's dir preview).
With `-e`: the right half's cursor row is bright/active, the left half
dimmed. Tab strip `1 2` still in the header, `2` highlighted.

### 06.14 — Focus change in split; navigation only moves the focused tab
**Action:** `tmux send-keys -t $SESSION Tab` → verify; then
`tmux send-keys -t $SESSION j` → verify.
**Expect (socket):** after Tab: `focused==0`, `view=="split"` (focus change
does not leave split). After `j`: `tabs[0].selection=="dirB"`,
`tabs[1].selection=="b1.txt"` unchanged; `entries 0 center` has
`selected:true` on `dirB`; `entries 1 center` still on `b1.txt`.
**Expect (screen):** after Tab the bright cursor row is in the LEFT half
(`-e`), the right half dimmed; after `j` the left half's highlight sits on
`dirB`, the right half unchanged.
**Note:** in split view no preview is driven at all — `preview_path` may
legitimately lag the selection here; do not assert it in this step.

### 06.15 — `!` back to single refreshes the (possibly stale) preview
**Action:** `tmux send-keys -t $SESSION '!'`
**Expect (socket):** `view=="single"`, `focused==0`, `tabs` length 2 (no tab
destroyed). Poll `state` until `preview_path=="$FIXTURE/dirB"` and `seq`
stable — the preview was NOT driven while navigating in split (06.14 moved
the selection to `dirB`), so this asserts the split→single
`refresh_focused_preview`.
**Expect (screen):** full three-column layout again; center highlight on
`dirB`; right column shows the `dirB` directory listing (`b1.txt`). A right
column still showing the pre-split preview (`dirA` contents) is the exact
stale-preview bug this step exists to catch.

### 06.16 — Cross-tab cut, part 1: cut in tab 2
**Action:** `tmux send-keys -t $SESSION Tab` (→ tab 2, `$FIXTURE/dirB`);
verify `focused==1` and `selection=="b1.txt"`; then
`tmux send-keys -t $SESSION d d`.
**Expect (socket):** `clipboard=={files:["$FIXTURE/dirB/b1.txt"],
op:"cut"}`. `log 20` contains INFO `cut 1 items`. File still on disk and
still listed in `entries 1 center` (cut is lazy — nothing moves until
paste).
**Expect (screen):** center still lists `b1.txt`; log widget shows
`cut 1 items` if captured within 10 s.
**Note:** `dd` uses marked-or-selected; whether the 06.4 mark survived the
intervening reloads is irrelevant here since `b1.txt` is also the
selection — assert the clipboard content, not `marked`.

### 06.17 — Cross-tab paste updates the non-focused source tab
**Action:** `tmux send-keys -t $SESSION 1` (focus tab 1, `$FIXTURE`); then
`tmux send-keys -t $SESSION p p`; then poll `echo state | socat ...` until
`undo_depth==1`.
**Expect (socket):** `clipboard==null` (taken on paste). After the
`undo_depth` 0→1 bump: `entries 0 center` (== `entries center`) contains
`b1.txt` (unmarked, not hidden); `entries 1 center` is EMPTY (`[]`) —
the non-focused source tab's listing reflects the removal (`reload_all`
covers all tabs); `tabs[1].total==0`, `tabs[1].selection==null`,
`tabs[1].marked==[]`. `log 30` contains INFO `paste 1 items, overwrite =
false` and NO `Failed to move` line. Disk: `$FIXTURE/b1.txt` exists,
`$FIXTURE/dirB` is empty.
**Expect (screen):** single view of tab 1: center lists `dirA`, `dirB`,
`amp & file.txt`, `b1.txt`, `r1.txt`, `spaced name.txt`.
**Note:** paste is an async spawn_blocking task — `await-idle` alone is NOT
sufficient; the `undo_depth` poll is the completion signal. A stale
`entries 1 center` still listing `b1.txt` after the bump is the cross-tab
refresh bug this step targets.

### 06.18 — Cross-tab move of names with spaces and `&` (multi-item)
**Action:** in tab 1: `tmux send-keys -t $SESSION G` (bottom →
`spaced name.txt`), verify selection; `tmux send-keys -t $SESSION Space`
(mark; cursor clamps at bottom); `tmux send-keys -t $SESSION g g` (top →
`dirA`); `tmux send-keys -t $SESSION j j` (→ `dirB` → `amp & file.txt`);
`tmux send-keys -t $SESSION Space` (mark; cursor auto-advances to
`b1.txt`); verify `state.marked` == both paths; `tmux send-keys -t $SESSION
d d`; `tmux send-keys -t $SESSION Tab` (→ tab 2, `$FIXTURE/dirB`);
`tmux send-keys -t $SESSION p p`; poll `state` until `undo_depth==2`.
**Expect (socket):** after `dd`: `clipboard.op=="cut"`, `clipboard.files` ==
{`$FIXTURE/spaced name.txt`, `$FIXTURE/amp & file.txt`} (set, order
irrelevant); log INFO `cut 2 items`. After paste completes: `entries 1
center` contains `amp & file.txt` and `spaced name.txt`; `entries 0 center`
contains neither; no `Failed to move` in `log 30`; log INFO `paste 2 items,
overwrite = false`. Disk: both files in `$FIXTURE/dirB/`, gone from
`$FIXTURE`.
**Expect (screen):** center (tab 2 = `dirB`) lists `amp & file.txt` and
`spaced name.txt`, names rendered verbatim (no shell-mangling of the space
or `&`).
**Note:** the move itself is native fs code, but the surrounding session
(zoxide add on the earlier descent, reloads) must not choke on these
names — any ERROR in `log 30` mentioning either name fails the step.

### 06.19 — Shrink back to one tab
**Action:** `tmux send-keys -t $SESSION q` (closes focused tab 2).
**Expect (socket):** `tabs` length 1, `focused==0`, `cwd==$FIXTURE`,
`view=="single"`; rfm still running.
**Expect (screen):** tab strip GONE from the header row (numbers are only
drawn with >1 tab); normal three-column single view of `$FIXTURE` (now:
`dirA`, `dirB`, `b1.txt`, `r1.txt`).

### 06.20 — `!` with a single tab auto-creates the second tab
**Action:** `tmux send-keys -t $SESSION '!'`
**Expect (socket):** `tabs` length 2 (auto-created), `focused==1` (the new
tab), `view=="split"`, `tabs[0].cwd==tabs[1].cwd==$FIXTURE`.
**Expect (screen):** two identical `$FIXTURE` listings (`dirA`, `dirB`,
`b1.txt`, `r1.txt`) side by side with the `│` divider; header tab strip
`1 2` with `2` highlighted; right half bright, left dimmed (`-e`).
**Note:** only TRACE `command: toggle split` is logged here — the internal
auto-`new_tab` does not emit `command: new tab` (that trace is on the
keybinding dispatch path).

### 06.21 — Closing a tab in split with 2 tabs reverts to single view
**Action:** `tmux send-keys -t $SESSION q`
**Expect (socket):** `tabs` length 1, `focused==0`, `view=="single"`
(close_tab forces Single when one tab remains). rfm still running.
**Expect (screen):** single-view Miller layout, no divider, no tab strip.

### 06.22 — Closing the LAST tab quits rfm
**Action:** `tmux send-keys -t $SESSION q`, then wait ~1 s.
**Expect (socket):** the socket no longer answers — `echo state | socat -
UNIX-CONNECT:$SOCK` fails (connection refused) or returns nothing. Note the
socket FILE may still exist; its liveness, not its presence, is the
assertion.
**Expect (screen):** `capture-pane` shows the shell prompt again (no rfm
header/columns/footer); `tmux list-panes -t $SESSION -F '#{pane_current_command}'`
no longer reports rfm.
**Note:** this is `CloseCmd::QuitWithPath` — the same exit path as `Q`.
`--choose-dir` output is out of scope here. Do not expect `./error.log`:
warns alone (06.8, 06.12) do not produce it.

---

**Teardown:** `tmux kill-session -t $SESSION` (the session may already be
dead after 06.22 — ignore errors); `rm -rf "$FIXTURE" "$CFG"` and the
isolation temp dirs; `rm -f $SOCK`.

**Section coverage gaps:** deliberately not covered here — `Q`/`exit`
outright-quit and `--choose-dir` path reporting (session/quit section);
split view combined with modal overlays (console/trash/search) and the
graphics `begin_frame` single-view gate (rendering/preview sections);
image/graphics preview refresh across tabs (preview section — this section
only uses text/dir previews); undo/redo of the cross-tab paste
transactions (undo section); clipboard persistence across tab closes
(clipboard is manager-global — not exercised); per-tab file-watcher
freshness (external `touch` into a background tab's cwd); split view under
exactly-40-column boundary width; keybinding-collision behavior when a user
config rebinds `q`/`1`-`4` (config section); tab behavior at terminal
resize *while in* split view beyond the too-narrow refusal.
