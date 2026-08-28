# Run 3 — Section 10: Shell commands and escaping

- Date: 2026-08-28
- Branch/commit: `feat/kitty-unicode-placeholders` @ `2beb383`
- Executor harness: tmux session `tp3-10`, socket `/tmp/tp3-10.sock`, 120x30 pane,
  quiet fixture parent (`PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`), scratch
  `--config`, `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs,
  `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`, binary launched directly as
  the tmux session command. All socket reads wrapped in `timeout 12`.
- Tools: zoxide PRESENT (`HAVE_ZOXIDE=1`), socat, jq, tmux all available.

**Tally: 15 steps (10.1–10.14 incl. 10.8b) — 15 pass, 0 fail, 0 skip. No new bugs.**

No `state` query ever timed out (no wedge). No `error.log` and no `INJECTED*`
artifact in the repo root after teardown; teardown completed (session killed,
socket and all tempdirs removed).

---

## 10.1 — Launch baseline: hostile names listed literally, no overlay — PASS

Socket: `mode=="normal"` (no decision-flow overlay — the four command keys
`o/x/b/v` collided with nothing), `cwd==$FIXTURE`,
`selection=="a directory with spaces"`, `selected_idx==0`, `total==5`,
`queue_active==null`, `queue_len==0`, `marked==[]`. `entries center` returned
exactly the five names in protocol order (`a directory with spaces`,
`Bilder & Videos`, `zz $(touch INJECTED-zox) dir`, `$(touch INJECTED).txt`,
`plain.txt`), `selected:true` only on the first.
Screen: all five names rendered literally (one `&`, `$()` intact); reverse-video
highlight (`[0;7m`/`[7m`) on the `a directory with spaces` row; top row shows the
selected item's path (per README, correct).
Note: the `entries` socket reply is a **bare JSON array**, not `{"entries":[...]}` —
recorded as protocol feedback for jq snippets.

## 10.2 — Background command: queue_active while running, log trail, file created — PASS

Socket: first `state` right after `o` + `await-idle`: `queue_active=="marker"`,
`queue_len==0`, `mode=="normal"`. After the poll: `queue_active==null`,
`queue_len==0`. `log 30` contained, in order:
`INFO Queueing command 'marker': sleep 2; touch marker-ran.txt`,
`INFO Executing command 'marker': ...`,
`INFO Command 'marker' completed successfully`.
Disk: `$FIXTURE/marker-ran.txt` exists.
Screen: panels unchanged, no overlay. **The protocol's screen expectation
("log widget shows the Queueing/completed INFO lines") does not match current
intended behavior**: the collapsed log widget (default `show_log=false`) shows
only the newest WARN/ERROR line (`draw_log`, manager.rs:823-828 — commit
`33c8428` "always show warnings and errors in log output"); INFO lines appear
only in the expanded widget. Verified live twice with sub-second captures
(neither the Queueing nor the completed line ever rendered; the failer WARN in
10.5 rendered fine, proving the widget itself works). Filed as protocol
feedback (stale expectation), not as a bug. The verification re-run added one
extra `marker` execution (harmless: re-touches `marker-ran.txt`).

## 10.3 — Watcher picks up the command's output file — PASS

Socket: `entries center` grew to 6 with `marker-ran.txt` in the expected sort
slot (after `$(touch INJECTED).txt`, before `plain.txt`); selection unchanged
(`a directory with spaces` still `selected:true`); fresh TRACE
`panel-update: center <- $FIXTURE` lines in `log 200`.
Screen: `marker-ran.txt` visible in the center column; belief and screen agree.

## 10.4 — queue_len: three rapid sends expose the internal queue — PASS

`o o o` sent; polled `state` every ~0.25 s. Sample distribution:
8×`{active:"marker",len:0}` → 8×`{active:"marker",len:1}` →
7×`{active:"marker",len:0}` → `{active:null,len:0}` — **`queue_len==1` observed**
(during the 2nd execution), active for ~6 s, clean final state. `log 60` counted
5 `Executing command 'marker'` + 5 `completed successfully` lines — consistent
with 2 earlier runs (10.2 + its verification re-run) plus these 3. UI stayed
responsive throughout (every state query answered instantly).

## 10.5 — Failing command: exit code + stderr in log, UI stays responsive — PASS

Socket `log 40` contained all of:
`INFO Queueing command 'failer': echo boom-stdout; echo boom-stderr >&2; exit 7`,
`INFO Executing command 'failer': ...`, `DEBUG [failer] boom-stdout`,
`WARN [failer] boom-stderr`, and `WARN Command 'failer' failed with exit code 7`.
**Deviation from protocol text: the failure line is WARN, not ERROR** — this is
the intentional log-level policy change (commit `da35a98` downgrades recoverable
user-op failures from `error!` to `warn!`). Recorded as protocol feedback
(update the expected level in 10.5 and the 10.14 sweep), not a bug.
Screen: collapsed log widget showed
`warn: Command 'failer' failed with exit code 7` while fresh; panels undisturbed.
Responsiveness: `j` advanced selection to `Bilder & Videos` (idx 1); `gg`
returned to idx 0. `queue_active==null` at the end.

## 10.6 — `$@` interpolation with hostile names: no word-split, no execution — PASS

Navigation: `jjj` → `selection=='$(touch INJECTED).txt'` (idx 3). `Space`
(marks + auto-advance), `j`, `Space` → `state.marked` held exactly the two
absolute paths `$FIXTURE/$(touch INJECTED).txt` and `$FIXTURE/plain.txt`.
After `b`: `log 30` shows
`Executing command 'lister': printf "%s\n" '/tmp/.../fx/$(touch INJECTED).txt' /tmp/.../fx/plain.txt > cmd-args.txt`
— the hostile path **single-quoted** (plain.txt legitimately unquoted; escaping
is need-based), `Command 'lister' completed successfully`, no `failed` line.
`state.marked==[]` after dispatch.
Disk: `cmd-args.txt` has **exactly 2 lines**, both absolute paths intact (no
word-split at the space, no quote characters in content); `$FIXTURE/INJECTED`
does **not** exist — no command substitution executed.
Screen: no mark prefixes remain; `cmd-args.txt` appeared in the center column
after the watcher poll. (The trailing `x` on the top row is the preview column
rendering plain.txt's 2-byte content — not a mark artifact.)

## 10.7 — Interactive command: blocking foreground run — PASS

`await-idle` returned only after the blocking run. `log 20`:
`INFO Running interactive command 'inter': touch interactive-ran.txt` and
`INFO Command 'inter' completed successfully`; **no** Queueing/Executing lines
for `inter`; `queue_active==null`; `mode=="normal"`.
Screen: UI intact after the run; `interactive-ran.txt` appeared in the center
column (watcher). Disk: `$FIXTURE/interactive-ran.txt` exists.

## 10.8 — Watcher: external create/delete from plain bash — PASS

`touch $FIXTURE/watched-new.txt` from the harness shell → `entries center`
gained it well inside the poll window (<1 s) and the pane showed it (grep count
1). `rm` → gone from both entries and pane (grep count 0). Selection preserved
(`plain.txt` before and after). Fresh `panel-update: center` TRACE lines for
both events. Belief and screen agreed at both checkpoints.

## 10.8b — Watcher: external MOVE (rename) in and out — `Modify(Name)` — PASS

Same-filesystem precondition verified: `stat -c %d` identical (65024) for
`$STAGE` and `$FIXTURE`, so `mv` was a true rename (MOVED_TO/MOVED_FROM →
`Modify(Name)`), not copy+unlink. Move **in**: `moved-in.txt` appeared in
`entries center` and on screen (<1 s) — the assertion that fails against the
historical regression held. Move **out**: entry and screen row both gone.
Selection `plain.txt` preserved across both events; fresh `panel-update` TRACE
lines for both. `$STAGE` removed afterward; fixture net-zero.

## 10.9 — Descend into "a directory with spaces" (+ zoxide add escaping) — PASS

After `gg` + `l`: `cwd` ends with `/a directory with spaces`,
`selection=="inner.txt"`, `left_path==$FIXTURE`, `total==1`. `log 30`:
`Executing command 'zoxide add current-dir': zoxide add '/tmp/.../fx/a directory with spaces'`
— path single-quoted — then `Command 'zoxide add current-dir' completed successfully`;
no failure line. Screen: top row path with spaces intact; center `inner.txt`;
left column shows the fixture listing with `a direc~` (truncated at pane width)
highlighted.

## 10.10 — zoxide database holds the exact path — PASS

`_ZO_DATA_DIR=$ZO zoxide query -l` printed exactly one line:
`/tmp/tmp.FgjOqQlMxm/fx/a directory with spaces` — byte-identical to
`$(realpath "$FIXTURE")/a directory with spaces`; no fragment entries.
`state.seq` stable across the query (78 → 78).

## 10.11 — Descend into "Bilder & Videos" — PASS

`h` restored root selection `a directory with spaces`; `j` →
`Bilder & Videos`; `l` → `cwd` ends with `/Bilder & Videos` (single `&` in the
JSON string), `selection=="vorhanden.txt"`, `total==1`. Newest
`Executing command 'zoxide add current-dir':` line shows
`'/tmp/.../fx/Bilder & Videos'` single-quoted, then `completed successfully`
(exit-0-despite-backgrounding ruled out by the quoting in the log line itself).
`zoxide query -l` now lists the full `.../Bilder & Videos` path as one line
(plus the 10.9 entry). Screen: top row renders `Bilder & Videos` with exactly
one `&`; center shows `vorhanden.txt`.

## 10.12 — Watcher inside an `&`-named directory — PASS

TRACE `watching /tmp/.../fx/Bilder & Videos` present in the log history from
the 10.11 descent. External `touch "…/neu & datei.txt"` → `entries center`
gained the name intact, `total` 1→2, pane showed it literally. External `rm` →
gone, `total` back to 1, pane row removed. `selection=="vorhanden.txt"`
throughout, highlight stayed on it.

## 10.13 — Descend into the `$(...)`-named directory: no injection anywhere — PASS

`h`, `j` → `selection=="zz $(touch INJECTED-zox) dir"`, `l` → `cwd` ends with
the literal `/zz $(touch INJECTED-zox) dir`, `selection=="inner2.txt"`. Newest
`Executing command 'zoxide add current-dir':` line shows the path single-quoted
with `$(touch INJECTED-zox)` intact inside the quotes; `completed successfully`.
`find "$FIXTURE" -name 'INJECTED*'` printed nothing; neither
`$FIXTURE/INJECTED-zox` nor `<repo>/INJECTED-zox` exists. Screen: top row shows
the literal name; center `inner2.txt`.

## 10.14 — Return to root: final integrity sweep — PASS

`h` → `cwd==$FIXTURE`, `selection=="zz $(touch INJECTED-zox) dir"` (restored
from history), `queue_active==null`, `queue_len==0`, `marked==[]`,
`mode=="normal"`. `entries center` == `ls -A` exactly: the 3 dirs +
`$(touch INJECTED).txt`, `cmd-args.txt`, `interactive-ran.txt`,
`marker-ran.txt`, `plain.txt` — 8 entries; no `INJECTED*`, no
`watched-new.txt`, no stray artifacts. Log history contains **zero ERROR
lines**; the only WARNs are the intentional failer pair
(`[failer] boom-stderr`, `Command 'failer' failed with exit code 7` — WARN by
design, see 10.5). No `zoxide add` failure lines. Screen matches the entries
reply; highlight on `zz $(touch INJECTED-zox) dir`; no garbled cells.
Observed but not filed: the left column still shows the badge `fx  5` while
`fx` holds 8 entries — this is the KNOWN-ISSUES KI-1 class (child-count badge
cached in `DirElem::normalize`, parent-mtime-gated reuse); not re-filed.

---

## Protocol feedback

1. **10.2 screen expectation is stale.** The collapsed (default) log widget
   deliberately shows only the newest WARN/ERROR line since commit `33c8428`
   (`draw_log`, `src/panel/manager.rs:823-828`); INFO lines like
   `Queueing command 'marker'...` / `completed successfully` never render
   unless the widget is expanded. Rewrite 10.2's screen expectation to "no
   warn/error line appears in the collapsed widget; INFO trail is asserted via
   the socket only" (or have the step toggle the expanded log first).
2. **10.5/10.14 expect ERROR for command failure; it is now WARN.** Commit
   `da35a98` downgraded recoverable user-op failures to `warn!` — the failure
   line is `WARN Command 'failer' failed with exit code 7`. Update the expected
   level in 10.5 and the 10.14 sweep wording ("no ERROR lines other than the
   intentional failer" → there are now no ERROR lines at all; the failer pair
   are WARNs). The collapsed widget shows the WARN, so the on-screen assertion
   still works.
3. **`entries` reply shape.** The socket `entries` reply is a bare JSON array
   (`[{name, marked, hidden, selected}, ...]`), not an object with an `entries`
   key — worth one line in the README so executors don't fumble the first jq.
4. **10.4 log-count wording.** "log 40 contains three Executing lines" assumes
   no earlier marker runs; any 10.2 re-query/re-run makes the count higher.
   Suggest "at least three new" or count deltas.
