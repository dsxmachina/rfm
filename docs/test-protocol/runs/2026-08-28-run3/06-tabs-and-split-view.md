# Run 3 — Section 06: Tabs and split view

- Date: 2026-08-28
- Branch: feat/kitty-unicode-placeholders (binary prebuilt at `./target/debug/rfm`)
- Session `tp3-06`, socket `/tmp/tp3-06.sock`, 120×30 tmux pane
- Isolation: quiet fixture parent (`$PARENT/fx`), scratch `--config`,
  `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs, `env -u
  KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR` (state confirmed
  `image_protocol: half-block`)

**Tally: 22 steps — 22 pass, 0 fail, 0 skip. No bugs filed.**

Every step ran the four-step loop (input → `await-idle` → socket assert →
`capture-pane` assert), all socket reads wrapped in `timeout 12`. No socket
read ever timed out; no socket/screen mismatch was observed anywhere in the
section.

---

## Per-step results

### 06.1 — Baseline: one tab, single view — PASS
Socket: `view=single`, `focused=0`, 1 tab, `tabs[0].cwd==$FIXTURE`,
`selection=dirA`, `selected_idx=0`, `marked=[]`. `entries center` listed the
5 fixture entries in the documented order with exactly one `selected:true`
(dirA). Screen: header `user@host` + path, NO tab strip in the top-right,
three columns, right column previews dirA (`a1.txt 6 B`).

### 06.2 — gn opens second tab, focuses it — PASS
Socket: 2 tabs, `focused=1`, `tabs[1].cwd==$FIXTURE`, `tabs[1].selection=dirA`,
tab 0 untouched, `view=single`; `log 200` contained `command: new tab`.
Screen (`-e`): tab strip appears; `1` in dark grey (`[38;5;8m`), `2`
bold+reverse (`[1;7m[38;5;2m`).

### 06.3 — Per-tab cwd is independent — PASS
After `j` + `l`: `focused=1`, `cwd==$FIXTURE/dirB`, `selection=b1.txt`,
`tabs[1].cwd==$FIXTURE/dirB` while `tabs[0].cwd==$FIXTURE` /
`tabs[0].selection=dirA` untouched. `entries 0 center` = root listing with
dirA selected; `entries 1 center` = `[b1.txt]` selected. Screen: header ends
`dirB/b1.txt`, center `b1.txt`, preview `bravo one`, strip `1 2`.

### 06.4 — Marks are per-tab — PASS
After Space: `marked==["$FIXTURE/dirB/b1.txt"]`, `tabs[1].marked` has it,
`tabs[0].marked==[]`; entry `marked:true` AND `selected:true` (cursor clamped
at the only entry). Screen (`-e`): row rendered `[0;7m[38;5;3m` (reverse +
dark-yellow) with the `x` mark prefix.

### 06.5 — Direct focus `1`; scalars mirror focused tab — PASS
`focused=0`, scalars mirror tab 0 (`cwd==$FIXTURE`, `selection=dirA`,
`marked=[]`); `tabs[1].marked` still holds `.../dirB/b1.txt` and
`entries 1 center` still shows `marked:true`. `log 200` has
`command: focus tab 1`. Screen: root listing, strip `1`(highlighted) `2`(grey).

### 06.6 — Tab cycles focus with wrap; preview refreshed — PASS
First Tab: `focused=1`, `preview_path` reached `$FIXTURE/dirB/b1.txt` (poll),
screen right column `bravo one`. Second Tab: `focused=0` (wrap),
`preview_path==$FIXTURE/dirA`, screen right column shows the dirA listing
(`a1.txt`). `log 200` contained `command: focus next tab` twice (an initial
`grep -c` of 1 was a line-count artifact on the single-line JSON reply — both
occurrences verified individually).

### 06.7 — Grow to 4 tabs — PASS
`gn` → 3 tabs, `focused=2`; `gn` → 4 tabs, `focused=3`. `tabs[2].cwd` and
`tabs[3].cwd` == `$FIXTURE` (clones of the focused root tab);
`tabs[1].cwd` still `$FIXTURE/dirB`. Screen: strip `1 2 3` then `1 2 3 4`,
focused number `[1;7m` highlighted, others `[38;5;8m`.

### 06.8 — MAX_TABS refusal — PASS
`gn` at 4 tabs: still 4 tabs, `focused=3` unchanged; `log 200` WARN
`max 4 tabs reached` (age 0.03 s). Screen: strip unchanged, log widget showed
`warn: max 4 tabs reached` within the 10 s TTL.

### 06.9 — Direct focus `3` and `4` — PASS
`3` → `focused=2` (strip highlight on `3`); `4` → `focused=3`,
`cwd==$FIXTURE`; `log 200` contained both `command: focus tab 3` and
`command: focus tab 4`.

### 06.10 — q closes non-last tab; focus clamps — PASS
3 tabs, `focused=2` (clamp min(3,2)), `cwd==$FIXTURE`, `view=single`, state
still answering; `log 200` `command: close tab`. Screen: strip `1 2 3` with
`3` highlighted, full Miller layout.

### 06.11 — ctrl-w also closes a tab — PASS
2 tabs, `focused=1`, surviving tab is the dirB tab: `cwd==$FIXTURE/dirB`,
`selection=b1.txt`, `tabs[0].cwd==$FIXTURE`. Screen: strip `1 2` (`2`
highlighted), center `b1.txt` (still with the 06.4 mark), preview `bravo one`.

### 06.12 — Split refused when terminal too narrow — PASS
Resized to 30×30, `!`: `view=single` unchanged, still 2 tabs (no orphan);
`log 200` WARN `terminal too narrow for split view` + TRACE
`command: toggle split`. Screen: cramped single view, no divider. Cleanup
resize back to 120×30 verified (state answers, layout normal).

### 06.13 — `!` enters split: two centers + divider, no preview — PASS
`view=split`, 2 tabs, `focused=1`, `command: toggle split` logged. Screen:
left half = tab 1's center (dirA, dirB, amp & file.txt, r1.txt,
spaced name.txt), divider `││` at x≈60, right half = `b1.txt`. Neither
`bravo one` nor `a1.txt` anywhere on screen (grep count 0). `-e`: right
half's cursor row bright (`[0;7m[38;5;3m` — marked color, active), left
half's cursor row dimmed (`[0;7m[38;5;8m`).

### 06.14 — Focus change in split; navigation moves only focused tab — PASS
Tab: `focused=0`, `view=split`; `-e` shows left cursor row now bright `[7m`,
right dimmed `[38;5;8m`. `j`: `tabs[0].selection=dirB`,
`tabs[1].selection=b1.txt` unchanged; `entries 0 center` selected on dirB,
`entries 1 center` on b1.txt. Screen matches. (`preview_path` not asserted,
per step note.)

### 06.15 — `!` back to single refreshes the stale preview — PASS
`view=single`, `focused=0`, 2 tabs; `preview_path` reached `$FIXTURE/dirB`
on poll. Screen: three-column layout, right column shows dirB's listing
(`b1.txt 10 B`) — NOT the pre-split dirA preview. The split→single
`refresh_focused_preview` works.

### 06.16 — Cross-tab cut, part 1 — PASS
Tab → `focused=1`, `selection=b1.txt`; `dd` →
`clipboard=={files:["$FIXTURE/dirB/b1.txt"],op:"cut"}`; `log 200` INFO
`cut 1 items`; file still on disk and still listed (cut is lazy). Screen
unchanged, log line visible.

### 06.17 — Cross-tab paste updates the non-focused source tab — PASS
`1` then `pp`; `undo_depth` reached 1 on poll. `clipboard==null`;
`entries 0 center` contains `b1.txt` (unmarked, not hidden);
`entries 1 center == []`; `tabs[1].total=0`, `tabs[1].selection=null`,
`tabs[1].marked=[]`. `log 200` INFO `paste 1 items, overwrite = false`, no
`Failed to move`. Disk: `$FIXTURE/b1.txt` exists, `$FIXTURE/dirB` empty.
Screen: center lists dirA, dirB, amp & file.txt, b1.txt, r1.txt,
spaced name.txt. Note: the right column showed `(empty)` — correct, not a
bug: tab 0's selection was still `dirB` (moved there in 06.14), and dirB is
now genuinely empty; `preview_path==$FIXTURE/dirB` confirmed.

### 06.18 — Cross-tab move of names with spaces and `&` — PASS
`G` → `spaced name.txt` (verified), Space; `gg` → dirA, `jj` →
`amp & file.txt` (verified), Space (cursor auto-advanced to b1.txt);
`state.marked` == exactly both paths. `dd`: `clipboard.op=cut`, files ==
both paths; INFO `cut 2 items`. Tab → tab 2 (`$FIXTURE/dirB`), `pp`;
`undo_depth` reached 2. `entries 1 center == [amp & file.txt,
spaced name.txt]`; `entries 0 center == [dirA, dirB, b1.txt, r1.txt]`;
INFO `paste 2 items, overwrite = false`; no `Failed to move`/ERROR in the
log. Disk verified both files in `$FIXTURE/dirB/`, gone from `$FIXTURE`.
Screen: both names rendered verbatim (no shell mangling of space or `&`),
preview shows `amp`.

### 06.19 — Shrink back to one tab — PASS
`q`: 1 tab, `focused=0`, `cwd==$FIXTURE`, `view=single`, rfm running.
Screen: tab strip GONE from the header row; single view listing dirA, dirB,
b1.txt, r1.txt.

### 06.20 — `!` with one tab auto-creates the second — PASS
2 tabs (auto-created), `focused=1`, `view=split`,
`tabs[0].cwd==tabs[1].cwd==$FIXTURE`. Screen: two identical listings with
divider, strip `1 2` (`2` highlighted); right half's cursor row bright
`[7m` (on dirA — fresh listing), left half's dimmed `[0;7m[38;5;8m` (on
b1.txt, tab 0's prior selection).

### 06.21 — Closing a tab in split reverts to single — PASS
`q`: 1 tab, `focused=0`, `view=single`, rfm running. Screen: single-view
Miller layout, no divider, no tab strip.

### 06.22 — Closing the LAST tab quits rfm — PASS
`q`, wait 1 s: socket connect refused (rfm removed the socket file on exit);
tmux session ended (rfm was the session command — `list-panes` reports the
session gone, which subsumes "no longer reports rfm"); no rfm UI in any
capture; no `./error.log` written (warns alone from 06.8/06.12 correctly did
not produce one).

---

## Bugs

None. No known issues (KI-1) were encountered either.

## Protocol feedback

1. **06.6 log-count pitfall worth a note:** the socket `log` reply is a
   single JSON line, so `grep -c 'command: focus next tab'` returns 1 even
   when the substring occurs twice (grep counts matching *lines*). The
   protocol's "contains ... (twice)" assertion should hint at `grep -o | wc -l`
   or splitting the reply — a naive count nearly mis-filed a pass as a fail.
2. **06.17 screen expectation could mention the preview column:** after the
   paste, tab 0's selection is still `dirB` (left there by 06.14), so the
   right column legitimately shows `(empty)` — an executor comparing against
   the step's center-listing-only expectation may double-take. One sentence
   ("preview shows empty dirB — expected") would save the verification detour.
3. **06.22 session-death nuance:** with the binary launched directly as the
   tmux session command (per README), quitting rfm kills the whole session,
   so `capture-pane` shows nothing and `list-panes` errors rather than
   showing a shell prompt. The step's "shell prompt again" wording only holds
   for send-keys-into-shell launches; suggest rewording to "pane/session
   gone OR shell prompt, and the socket refuses connections".
4. `resize-window` present (tmux ≥ 2.9), zoxide present — no degraded steps.
