# Run 3 — Section 01: Startup and basic movement

- **Date:** 2026-08-28
- **Branch/commit:** `feat/kitty-unicode-placeholders` @ `2beb383`
- **Binary:** `./target/debug/rfm` (pre-built)
- **Harness:** tmux session `tp3-01` (120x30), socket `/tmp/tp3-01.sock`, quiet
  fixture parent (`PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`), scratch `--config`
  dir, isolated `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR`, launch prefix
  `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`, binary launched directly as
  the tmux session command. Every socket read wrapped in `timeout 12`.
- **Tools present:** tmux, socat, zoxide (zoxide path in 01.10 exercised for real).
- **Result: 20/20 steps PASS, 0 fail, 0 skip, no bugs filed.**

`image_protocol` resolved to `half-block`, `graphics_passthrough=false` — the
env isolation worked as intended on this kitty-unicode branch.

## Per-step results

### 01.1 — Launch: initial state and screen layout — PASS
Socket: `mode=normal`, `view=single`, `focused=0`, 1 tab, `cwd=$FIXREAL`,
`selection="Bilder & Videos"`, `selected_idx=0`, `total=6`, `marked=[]`,
`clipboard=null`, `show_hidden=false`, `undo_depth=0`, `redo_depth=0`,
`left_path=$PARENT`. `entries center` = 7-element array in the exact protocol
order, only `Bilder & Videos` selected, only `.hidden.txt` hidden, none marked.
`entries left`: `fx` selected.
Screen: header `someone@work $FIXREAL/Bilder & Videos` (selected-entry path),
no tab numbers; three columns; center shows exactly the 6 visible names in
order, `.hidden.txt` absent; preview column shows `clip.txt`; footer
`drwxr-xr-x … someone … 1/6`; `-e` capture shows the reverse-video row on
`Bilder & Videos`.

### 01.2 — Idle invariants — PASS
Two `state` reads with `entries center` and `log 5` in between: both `seq=5`,
full state JSON byte-identical, `capture-pane -p` byte-identical to the 01.1
capture (diff clean). (seq had ticked 4→5 once after launch from the async
preview arrival, before this step's first read — stable throughout the step
itself, which is the invariant.)

### 01.3 — `j` moves down — PASS
`selection="many"`, `selected_idx=1`, seq 5→6. `entries center`: selected moved
to `many` only. `log 20` contains TRACE `key-event: … Char('j') … (mode:
normal)` and TRACE `move-down`. Screen: header ends `/many`, footer `2/6`,
highlight on `many`, preview shows `f00`.

### 01.4 — Down arrow — PASS
`selection="subdir_a"`, `selected_idx=2`, another `move-down` TRACE. Footer
`3/6`, highlight on `subdir_a`, preview shows `inner.txt`.

### 01.5 — `k` and Up — PASS
After `k`: `many`/idx 1. After Up: `Bilder & Videos`/idx 0. Two `move-up`
TRACE lines in `log 20`. Footer `1/6`, header ends `/Bilder & Videos`,
highlight back on top row.

### 01.6 — `k` at top clamps — PASS
Selection unchanged (`Bilder & Videos`/0), seq 9→10 (key processed), zero
WARN/ERROR lines in log, pane byte-identical to the 01.5 final capture.

### 01.7 — `G` to bottom; index pitfall — PASS
`selection="charlie.txt"`, `selected_idx=5`; `entries center` has
`charlie.txt` selected at **array position 6** of 7 (≠ selected_idx 5 — the
hidden-entry offset asserted explicitly). Footer `6/6`, permissions start with
`-`, highlight on `charlie.txt`. Preview blank (empty file) as specified.

### 01.8 — `j` at bottom clamps — PASS
Unchanged `charlie.txt`/5, seq 11→12, no WARN/ERROR, pane byte-identical.

### 01.9 — `gg` chord — PASS
After first `g`: `mode=normal`, selection still `charlie.txt` (no movement).
The pending buffer IS rendered: footer reads `…text/plaig…` — the `g` printed
at width/2 (col 60) over the mime string; footer still `6/6`. After second
`g`: buffer gone (`text/plain` restored), `Bilder & Videos`/0, footer `1/6`,
highlight on top row.

### 01.10 — `l` into `Bilder & Videos` (spaces + `&`) — PASS
`cwd=$FIXREAL/Bilder & Videos`, `selection="clip.txt"`, idx 0, `total=1`,
`left_path=$FIXREAL`; `entries left` has `Bilder & Videos` selected. Log:
TRACE `move-right`, then
`Executing command 'zoxide add current-dir': zoxide add '/tmp/…/fx/Bilder & Videos'`
followed by `completed successfully` — escaping of spaces+`&` held, no
exit-code failure line. Screen: header ends `/Bilder & Videos/clip.txt`, left
column lists fixture entries (name truncated to `Bilder ~` in the narrow
column), center shows only `clip.txt`, footer `1/1`.

### 01.11 — `h` back to parent — PASS
`cwd=$FIXREAL`, `selection="Bilder & Videos"`, `total=6`; TRACE `move-left`
present (in `log 30` — a `log 10` window was already flushed past it by the
watch/unwatch/panel-update TRACE churn; see protocol feedback). Footer `1/6`,
highlight on `Bilder & Videos`, layout matches 01.1.

### 01.12 — Right/Left arrows; restore off index 0 — PASS
After `jj`: `subdir_a`/2. After Right: `cwd=$FIXREAL/subdir_a`,
`selection="inner.txt"`. After Left: `cwd=$FIXREAL`, `selection="subdir_a"`,
`selected_idx=2` — restored to the descended-from dir, not index 0. Footer
`3/6`, highlight on `subdir_a`.

### 01.13 — `ctrl-f` page forward — PASS
Setup: `k` → `many`, `l` → `cwd=$FIXREAL/many`, `f00`/0, `total=40`, footer
`1/40`. After `C-f`: `f27`/27 → **H=27 confirmed** (matches the geometry
facts; reused unchanged for 01.14–01.17). Footer `28/40`; center highlight on
`f27` (the second `[7m` row — the first is the left column's own highlight on
`many`, see protocol feedback).

### 01.14 — Page forward clamps — PASS
`f39`/39, footer `40/40`, last visible center row is `f39`, highlight on `f39`.

### 01.15 — `ctrl-b` — PASS
`f12`/12 (39−27), footer `13/40`.

### 01.16 — `ctrl-u` saturates, `ctrl-d` half page — PASS
After `C-u`: `f00`/0 (12−13 saturates). After `C-d`: `f13`/13 (H/2=13),
footer `14/40`.

### 01.17 — NPage/PPage — PASS
After `NPage`: `f39`/39 (clamped). After `PPage`: `f12`/12. Footer `13/40`,
center highlight on `f12`.

### 01.18 — `gg` in long listing, `h` out — PASS
After `gg`: `f00`/0. After `h`: `cwd=$FIXREAL`, `selection="many"`, idx 1,
`total=6`, footer `2/6`, center highlight on `many`. (Observed: the preview
column renders `many`'s remembered inner selection `f00` in reverse video —
consistent panel behavior, not asserted against by the protocol.)

### 01.19 — Quit with `Q` — PASS (screen sub-assert unverifiable, see feedback)
Footer present pre-quit (`2/6`). After `Q`: process gone within the poll
window; `state` connection fails (`No such file or directory` — rfm removed
the socket file on exit); rfm layout gone. The protocol's "pane shows a shell
prompt" sub-assert cannot be observed under the README's direct-launch
harness: rfm IS the session command, so the tmux session dies with it
(`can't find pane: tp3-01`). The load-bearing behavior (clean quit, dead
socket, no wedge) is fully verified; no path printed is unobservable for the
same reason (no `--choosedir` was passed).

### 01.20 — Relaunch; `q` on last tab quits — PASS
Fresh session relaunched with identical env/command (pane from 01.19 no longer
exists — see feedback); `state`: `mode=normal`, 1 tab, correct cwd. After `q`:
process gone, socket connection refused. `q`-closes-last-tab-quits confirmed.

## Bugs

None. No socket/screen mismatches, no wedges (every socket read answered well
inside `timeout 12`), no WARN/ERROR log lines from any movement step.

## Protocol feedback

1. **01.19/01.20 expectations conflict with the README's direct-launch
   harness.** The section expects "the pane shows a shell prompt (the tmux
   session itself stays alive because the shell survives rfm)" and 01.20 says
   to relaunch "in the same pane". With the binary as the tmux session command
   (mandated by the README to dodge shell widgets), the session dies when rfm
   exits — there is no shell and no pane left. Suggest rewording 01.19's
   screen expectation to "the tmux session/pane is gone (direct launch) OR
   shows a clean shell prompt (shell launch)" and 01.20's setup to "recreate
   the session with the identical launch command".
2. **01.19's socket expectation should mention the file disappears.** It says
   "the socket *file* may still exist on disk"; observed behavior is rfm
   unlinking it on clean exit (socat error is `No such file or directory`, not
   `Connection refused`). Both prove the socket is dead; the text could note
   both error shapes so executors don't stumble.
3. **01.11's `log 10` window is too small for the `move-left` assert.** A
   single `h` emits ~10+ TRACE lines (key-event, Command, move-left, history,
   watch/unwatch ×4, panel cache lines, set-left-panel), so `move-left` was
   already outside `log 10` by query time. It was found in `log 30`. Suggest
   the step say `log 30` (the README's "wide window" advice already implies
   this).
4. **Highlight greps must exclude the left column's own reverse-video row.**
   Once inside a subdirectory, the left panel highlights the cwd's entry
   (e.g. `many`), so two rows carry `[7m`/`[0;7m`. A grep-for-any-highlight +
   name check can false-match (in 01.13 the left-highlight row also contained
   center text `f14`). The section's "exactly one row inside the center
   column" wording is correct but easy to under-implement — a hint like
   "expect a second highlighted row from the left column when cwd is not the
   fixture root; match the name within the highlighted cell" would prevent
   mis-asserts. Additionally, after 01.18's `h`, the *preview* column renders
   the remembered inner selection (`f00`) in reverse video — a third
   legitimate highlight source worth a note.
5. **Minor:** with a quiet parent, `seq` can still tick once shortly after
   launch (async preview panel arrival, 4→5 here). 01.2's invariant holds
   *between* its own two reads; the step text is fine but executors should
   take the 01.2 baseline from its own first read, not from 01.1's reply.

## Teardown

Session killed, `$PARENT` `$CFG` `$CACHE` `$STATE` `$ZO` removed, socket file
removed (already unlinked by rfm). Verified `/tmp/tp3-01.sock` absent.
