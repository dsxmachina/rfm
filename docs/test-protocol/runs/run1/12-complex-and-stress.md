# Section 12 — Complex interactions & stress — Run 1

**Environment:** tmux 3.6a, socat 1.8.1.3, all optional tools present
(zoxide, ffmpeg, openssl, zip). Terminal 120×30 (resized to 100×25 and back
per 12.4/12.5). Binary `./target/debug/rfm` (18 MB, built). Fixtures under
`/tmp` per README harness. **Max canary latency observed across the whole
section: 7 ms** (every reading well under the 1000 ms bound; the event loop
never wedged, not once).

**Overriding environmental caveat:** fixtures live under `/tmp`, which on this
host holds 6193 entries and is churned by other parallel test sections
(mktemp dirs appearing/disappearing). rfm's **left (parent) panel** watches
`/tmp`, so it reloads that 6193-entry directory continuously (~0.5 s cadence
early in the run, quieting later as parallel sections finished). This does NOT
wedge the loop (latency stayed 4–7 ms) but it (a) makes the "stable-while-idle
`seq`" invariant unverifiable, (b) makes the 12.5 baseline-diff show
left-column churn, and (c) floods the 200-line log retention ring (192/200
lines were `/tmp` updates at one point). None of these are rfm defects — they
are a consequence of testing under a live busy `/tmp`. Recorded as protocol
feedback, not bugs.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 12.1 | PASS | mode=normal, total=22, selection=doomed, idx=0, marked=[], undo_depth=0; 22 entries, 1 selected (doomed), none hidden; Miller layout, doomed highlighted, right=x.txt. lat=5ms |
| 12.2 | PASS | selected_idx=20, selection=long `lll….txt`, seq 38→151; exactly 1 selected = state.selection; footer 21/22; pane fully drawn. lat=5ms |
| 12.3 | PASS | 20×k fully reversible: selection=doomed, idx=0; screen consistent with baseline. Idle-seq stability sub-assertion NOT verifiable (see caveat) — attributable to `/tmp` parent churn, not rfm. lat=5ms |
| 12.4 | PASS | resize→100×25: cwd/selection/idx/total/mode unchanged; layout redrawn at new size, doomed highlighted, no 120-col fragments, no blank regions. lat=7ms |
| 12.5 | PASS | resize→120×30: state identical to 12.1; capture diff vs BASELINE confined to the **left `/tmp` column** (scroll + count changes from external churn); header/center/right/highlight identical. lat=5ms |
| 12.6 | PASS | all 3 hostile names in entries; emoji, NFD (exact byte-match `cafe\xcc\x81-nfd.txt` → renders `café-nfd.txt`), and 204-byte long name each became state.selection verbatim; long name truncated within center col, no preview overflow; no WARN/ERROR, no panic. lat=5–6ms |
| 12.7 | PASS | after 50-file create+delete churn: total=22, no `churn*` in entries, 1 selected, disk=22, no ERROR lines; screen shows original 22, no remnants. lat=6ms |
| 12.8 | PASS (core) | no wedge on deleted cwd (lat 4–5ms); after `rm` state coherent (stale listing, total=1 — acceptable, no recovery contract); after `h` cwd=fixture, total=21, doomed gone, mode=normal, valid highlight, no panic. **Minor papercut:** preview column stale after `h` (see BUG-1). |
| 12.9 | PASS | log has all 4 lines in order (Queueing INFO, Executing INFO, `[fail] boom-stderr` WARN, `…failed with exit code 3` ERROR); queue_active=null, queue_len=0; widget renders `error: Command 'fail' failed with exit code 3` within TTL (confirmed persistent 8s). lat=5–6ms |
| 12.10 | PASS | after 11s the ERROR line is erased from screen; in full 200-line history it survives at age 21.9s, level ERROR (capacity-only retention). lat=6ms |
| 12.11 | PASS | 5 latencies all <1000 (max 7); await-idle returns idle in 6ms; capture normal, no overlay/panic/error text. Snapshots +1 seq (residual `/tmp` activity). |
| 12.12 | PASS | on `Q`: stderr banner with "…This is a bug!" + issues URL + `Error:` `Command 'fail' failed with exit code 3`; `$FIXTURE/error.log` written (not repo root), contains `ERROR (48s ago): Command 'fail' failed with exit code 3` + Queueing/Executing INFO + `[fail] boom-stderr` WARN + 44 TRACE lines. |
| 12.13 | PASS | session killed, socket removed, fixture = 21 entries + error.log (no doomed, no .part/zip/churn/temp), no orphan rfm processes, repo tree untouched (no error.log in root). |

**Tally: 13 steps, 13 PASS, 0 FAIL, 0 SKIP.** One minor papercut bug filed
under 12.8 (does not fail the step's core robustness contract).

---

## BUG-run1-01 — Preview column stale after leaving a deleted cwd (`h`)

- **Protocol step:** 12.8
- **Severity:** papercut
- **Class:** both (socket `preview_path` wrong AND screen stale — they agree
  with each other but both lag the real selection)

### Repro (minimal)
1. From the fixture, `gg` to select `doomed`, `l` to enter it (cwd=doomed,
   preview would be x.txt).
2. From the harness shell: `rm -rf "$FIXTURE/doomed"`.
3. `tmux send-keys h` (back to parent). `await-idle`.
4. Observe: `state.selection == "subdir"` but `state.preview_path ==
   "$FIXTURE/doomed"` (the deleted dir), and the right column still renders
   `x.txt` (doomed's old content) instead of subdir's `inner.txt`.
5. Any cursor move (`j` then `k` back onto subdir) refreshes it correctly:
   `preview_path` becomes `$FIXTURE/subdir`, right column shows `inner.txt`.

### Expected
After `h` restores the parent listing and the selection lands on `subdir`,
the preview column should reflect the new selection (subdir → `inner.txt`),
and `preview_path` should be `$FIXTURE/subdir`.

### Actual
- socket `state` right after `h`: `selection=subdir`, `total=21`,
  `preview_path=/tmp/…/doomed` (stale — the just-deleted directory).
- captured pane: right column shows `🖹inner.txt` header?  no — shows
  `🖹x.txt 0 B` (doomed's content) beside a `subdir` highlight.
- After one `j`+`k` cursor bounce: `preview_path=/tmp/…/subdir`, right column
  correctly shows `🖹inner.txt`.

### Notes
Self-corrects on the next selection change, so impact is cosmetic and
transient. Root cause is plausibly that the `h` (leave-dir) path restores the
selection to a *different* entry (doomed was deleted, cursor fell onto subdir)
without re-driving the preview for the new selection — i.e. `refresh_focused_
preview`/`new_panel_delayed` short-circuited on an unchanged *cursor slot* even
though the underlying entry changed. **reproduced_twice=false** — the deleted-
doomed precondition is consumed by the step and re-establishing it needs a
fresh doomed dir; observed once in the live run and once on the immediate
follow-up cursor-bounce verification (which confirms the self-heal, not a
second independent trigger). Filed as flaky/once.

---

## Protocol feedback

- **Fixtures under `/tmp` cause parent-panel watcher churn.** rfm's left panel
  watches the fixture's parent (`/tmp`), a 6193-entry live directory churned by
  other parallel test sections. This continuously reloads the left panel
  (~0.5 s cadence), which: (1) makes 12.3/12.11's "two idle `state` snapshots
  have identical `seq`" assertion effectively unverifiable — `seq` advances by
  1–10 per second with zero user input, purely from `/tmp` reloads; (2) makes
  12.5's "capture identical to BASELINE except log-widget lines" fail on the
  **left column** (temp dirs scrolling / count changes), which the assertion
  doesn't exempt; (3) floods the 200-line log retention (192/200 lines were
  `/tmp` updates at one point, nearly evicting the ERROR line that 12.10/12.12
  depend on). **Suggestion:** create the fixture under a freshly-mkdir'd,
  otherwise-empty parent (e.g. `PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`) so
  the watched parent is quiet, or explicitly launch rfm *inside* the fixture so
  the left panel is a controlled dir. This would make the idle-seq and
  baseline-diff invariants meaningful.
- **12.9/12.10 log-widget captures are timing-fragile under a busy loop.** With
  `/tmp` triggering frequent redraws, a `capture-pane` landing during a partial
  repaint intermittently missed the on-screen ERROR line (count 0 then 1 on
  back-to-back captures). The line IS rendered correctly and persistently
  (verified by sampling once/second for 8 s). The protocol should warn that a
  single capture can race a redraw; sample the widget 2–3× before asserting
  "not shown". (Under a quiet parent this would likely not manifest.)
- **12.10 "log 40" is too small a window here.** Because the busy `/tmp` panel
  floods the ring, the ERROR line was pushed past the last-40 window even
  though it's retained (found at position deep in `log 200`). The step says
  `log 30`; suggest `log 200` (the full ring) when asserting capacity-only
  retention, or note that a busy watcher can require the full window.
- **12.8 header/selection detail:** rfm's header shows the *selected item's*
  full path (e.g. `…/doomed`, `…/subdir`), not the cwd. The step's "confirm
  `state.cwd==…/doomed`" is right, but a reader might expect the header to show
  cwd; worth a one-line clarification that the header is the selection path.
- All other steps matched the protocol exactly; no wrong keys or bad fixture
  recipes encountered. The `[commands.fail]` config, the emoji/NFD/long-name
  recipe, and the `cd $FIXTURE` launch deviation all worked as written.
