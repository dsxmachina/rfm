# Run 3 — Section 11: Undo / Redo

**Environment:** Linux, tmux 120x30, branch `feat/kitty-unicode-placeholders`
(HEAD 2beb383), binary `./target/debug/rfm` (pre-built). Tools present: gnutar
1.35, zip, socat, tmux. Harness: SESSION=tp3-11, SOCK=/tmp/tp3-11.sock, quiet
fixture parent (`$PARENT/fx`), isolated `--config`, `XDG_CACHE_HOME`,
`XDG_STATE_HOME`, `_ZO_DATA_DIR`, and `XDG_DATA_HOME` (fresh trash home) per
section spec. `image_protocol` resolved to `half-block` (env -u prefix active).
Every socket read wrapped in `timeout 12`; no timeout ever fired — no wedge at
any point in the section.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 11.1 | **FAIL (screen only)** | Socket side fully correct: depths 0/0 before and after `u`/`C-r`, mode normal, selection `dest dir`, `log` has both INFO lines (`nichts rückgängig zu machen`, `nichts wiederherzustellen`), pane content unchanged with `dest dir` reverse-video highlighted. Screen side FAILED: the log widget never shows either INFO line — see BUG-3-11-01. |
| 11.2 | PASS | Rename a.txt→a.txt.bak: 1/0, disk + entries + screen agree. Undo: 0/1, `rückgängig: rename a.txt → a.txt.bak`, a.txt back (content `alpha`). Redo: 1/0, `wiederhergestellt: …`, .bak back. Final undo restores a.txt (0/1). |
| 11.3 | PASS | `yy` on a.txt → clipboard `{files:[…/a.txt], op:"copy"}`, `copying 1 items`. Enter `dest dir` (total 0), `pp` → undo_depth 1 after 1 poll, **redo_depth 0 (11.2's redo entry cleared by the new record)**, clipboard null, `paste 1 items, overwrite = false`, copy on disk, source intact. Undo: 0/1, `rückgängig: paste (1 items)`, dest empty on disk/socket/screen. |
| 11.4 | PASS | Redo: 1/0, `wiederhergestellt: paste (1 items)`, `dest dir/a.txt` back with content `alpha`, screen lists it. Cleanup undo → 0, dest dir empty. |
| 11.5 | PASS | `dd` on b.txt → clipboard op `cut`, `cut 1 items`. Paste in `dest dir`: 1/0, move semantics on disk (dest has b.txt, source gone), left pane no longer lists b.txt. Undo: 0/1, `rückgängig: paste (1 items)`, b.txt restored (content `bravo`), dest empty; `entries left` == captured left pane (both list b.txt) — no stale render. |
| 11.6 | PASS | Same-dir `yy`+`pp` on c.txt → 1/0 (redo from 11.5 cleared), both `c.txt` and `c.txt_` in entries/screen, `c.txt_` content `charlie`. Undo: 0/1, exactly `c.txt_` removed, `c.txt` untouched (content `charlie`). |
| 11.7 | PASS | Marked exactly {a.txt, c.txt, d & e.txt} (socket `marked` + per-entry flags agree), `copying 3 items`. Paste in dest dir: undo delta exactly +1 (one Transaction), `marked`==[], clipboard null, `paste 3 items, overwrite = false`, all 3 on disk, `d & e.txt` content `delta` (space+`&` survived), screen shows the literal name. Single-key undo: 0/1, exactly ONE `rückgängig: paste (3 items)` line, dest empty, originals intact. |
| 11.8 | PASS | `-l delete` on b.txt → 1/0, `Deleted 1 items` (log 200), `$TRASHHOME/Trash/files/b.txt` + `.trashinfo` exist. Undo: 0/1, `rückgängig: delete (1 items)`, b.txt back (`bravo`), trash files/ empty. Redo: 1/0, `wiederhergestellt: delete (1 items)`, fresh un-suffixed `b.txt` trash entry. Undo-the-redo: 0/1, restored again (`bravo`) — cycle stable. |
| 11.9 | **FAIL (screen only)** | Socket/disk fully correct: `tar` on `d & e.txt` → 1/0, `Creating tar.gz archive from 1 files`, `tar -tzf` lists exactly `d & e.txt`, row on screen. Undo → **0/0** (no_redo confirmed), `rückgängig: tar`, archive gone. `C-r` → still 0/0, `nichts wiederherzustellen` in log, no recreation on disk/entries/screen. Screen side FAILED on one sub-assert: the widget never shows `nichts wiederherzustellen` — same root cause, BUG-3-11-01. |
| 11.10 | PASS | `zip` on a.txt → 1/0, `Creating zip archive from 1 files`, output.zip exists + on screen. Undo → 0/0, `rückgängig: zip`, gone. `C-r` → 0/0, `nichts wiederherzustellen` (log), no reappearance. (Screen expectation here only asserts the archive row, which passed.) |
| 11.11 | PASS | Fresh relaunch, `use_trash = false`. (a) rename x.txt→x.txt.bak → 1/0. (b) `-l delete` on y.txt → **2/0** (Barrier counts), `Deleted 1 items`, y.txt gone, `$TRASHHOME2/Trash/` never even created (nothing trashed). (c) undo → STILL 2/0, WARN `kann nicht rückgängig gemacht werden: permanentes Löschen`, y.txt not restored, x.txt.bak NOT reverted; **the WARN line IS visible in the on-screen widget** (row 29: `warn: kann nicht rückgängig …`). (d) repeat undo → identical 2/0, second WARN line in log 200 (ages 15.8s/6.6s) — barrier never consumed. `C-r` → 2/0, `nichts wiederherzustellen` as the note predicts. |

**Tally: 9 pass / 2 fail (both from the single bug below, socket+disk semantics
correct in both) / 0 skip.**

## Bugs

## BUG-3-11-01 — Collapsed log widget never displays INFO lines (undo/redo feedback invisible on screen)

- **Protocol step:** 11.1 (primary) and 11.9 (same sub-assert); also voids the
  widget-visibility expectation implied in 11.2–11.8 "log widget shows …" lines
  (not counted as extra failures — one root cause).
- **Severity:** visual (papercut-leaning: all load-bearing state is correct and
  visible in the panes; but every INFO-level user feedback line — `rückgängig:
  …`, `wiederhergestellt: …`, `nichts rückgängig zu machen` — is invisible on
  screen unless the log widget is expanded)
- **Class:** render (socket `log` history has the lines at INFO; the screen's
  log region stays blank)
- **Reproduced twice:** yes — ≥4 independent keypresses (u, C-r, u again, C-r
  again in 11.1; again at 11.9's C-r), pane sampled 3× after each per README
  race guidance, never present. Counter-example proving the widget itself
  works: the WARN line in 11.11c rendered immediately (`warn: kann nicht
  rückgängig gemacht werden: permanentes Löschen` on row 29).

### Repro (minimal)
Fresh harness per README, any fixture. Press `u` (empty undo stack),
await-idle. Socket `log 10` contains INFO `nichts rückgängig zu machen`;
`tmux capture-pane -p` shows a blank log row (row 29 at -y 30). Press a key
that produces a WARN (e.g. undo into a barrier) → that line DOES appear.

### Expected
Protocol 11.1/11.9 and CLAUDE.md ("the on-screen widget still shows only
info+") expect INFO lines in the widget.

### Actual
- socket: `log 10` → `{"level":"INFO","age_secs":0.011,"message":"nichts
  wiederherzustellen"}` (state depths correct, 0/0)
- pane rows 28–30 at capture time:
  ```
  │              │                                            │
  <blank log row>
  drwxr-xr-x   someone users 4.0 K … 1/5
  ```

### Notes
Root cause located: `src/panel/manager.rs` `draw_log()` — the collapsed
(`show_log == false`) branch renders only the newest line with
`*level <= Level::Warn`, i.e. Warn/Error only; INFO is filtered out. The
filter dates to commit 33c8428 ("always show warnings and errors in log
output", 2024-05-12) — long-standing behavior, not a regression of this
branch. Either the code contradicts the documented intent (CLAUDE.md says
info+) or the docs/protocol contradict a deliberate design; routed as a bug
plus protocol feedback so the maintainer can pick a side. Run 1 marked 11.1
PASS while attributing missing widget lines to `/tmp` watcher churn — with
this run's quiet parent that explanation is eliminated; the line is filtered,
not evicted.

## Observations (not filed)

- **Stale child-count badge** (11.7): after pasting 3 files into `dest dir`,
  the left column briefly showed `dest dir 2` while it held 3 entries — same
  cached-`DirElem.suffix` class as KI-1 in KNOWN-ISSUES.md; not re-filed.
- The 11.8 trash entry was un-suffixed `b.txt` as required (fresh
  `$TRASHHOME`), and redo produced a fresh valid TrashItem (second restore
  succeeded with correct content).

## Protocol feedback

1. **11.1/11.9 screen expectations are unsatisfiable as written** against the
   current binary: the collapsed widget shows only Warn+ (manager.rs
   `draw_log`, since 33c8428/2024). Either fix the code to match CLAUDE.md's
   "widget shows info+" or rewrite the steps to assert "collapsed widget stays
   blank for INFO; socket `log` carries the line" (and keep 11.11c as the
   positive WARN-visibility probe — it works). Run 1's "evicted by /tmp churn"
   explanation for the same symptom was wrong; with a quiet parent the lines
   are simply filtered.
2. The async-paste poll rule worked perfectly, but is over-cautious for these
   fixtures: `undo_depth` reached 1 on the *first* poll every time (11.3,
   11.5, 11.6, 11.7). Keep the rule (it is the correctness boundary), just
   don't expect to need the 5 s.
3. 11.9/11.10: the archive is produced by a background command; the protocol
   asserts disk existence right after await-idle. It was near-instant here,
   but a "poll for the file (up to ~5 s)" note like the paste rule would make
   the step robust on slow disks.
4. The `-l` guidance for `delete` (11.8/11.11) is load-bearing and correct;
   sending `rename`/`tar`/`zip` without `-l` also works (none collide with
   tmux key names, though `-l` is harmless and I used it for `tar`/`zip`).
5. 11.11's prediction that `C-r` at the barrier yields
   `nichts wiederherzustellen` (never `redo blockiert:`) matched exactly.
