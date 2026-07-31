# Section 04 — File manipulation and trash — run1

**Environment:** rfm `./target/debug/rfm` (18 MB, built 2026-07-31 18:46). tmux
120x30, image_protocol resolved to `half-block` (inside tmux, expected). Tools
present: tmux, socat. No external tools needed for this section. Ran with unique
session/socket names `rfm-sec04-qa` / `/tmp/rfm-sec04-qa.sock` to avoid collision
with other concurrent executors (see Protocol feedback — the shared scratchpad
`env.sh` was clobbered by another section).

Isolation: fresh `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_STATE_HOME`,
`_ZO_DATA_DIR`, `--config` per phase. Trash landed in `$XDG_DATA_HOME/Trash`
(same filesystem as fixtures under `$TMPDIR`), so all trash rows were our own —
no foreign topdir rows appeared; both "empty trash" asserts (04.14) applied.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 04.1  | PASS | mode=normal, total=4, sel=`sub dir & stuff`, idx=0, undo/redo=0, clipboard=null, marked=[]; entries set matches; header path shown |
| 04.2  | PASS | mode=rename; footer `Rename: alpha.txt` seeded |
| 04.3  | PASS | mode=rename, undo=0; footer `Rename: renamed.txt` (C-u kept `.txt`); selected row previews `renamed.txt` |
| 04.4  | PASS | Esc → mode=normal, undo=0; entries has alpha.txt not renamed.txt; disk: alpha exists, renamed absent |
| 04.5  | PASS | Enter → mode=normal, undo=1; entries has renamed.txt not alpha.txt; disk correct |
| 04.6  | PASS | rename→bravo.txt refused; mode=normal, undo=1 unchanged; WARN `Cannot rename: '.../bravo.txt' already exists`; both files intact |
| 04.7  | PASS | mkdir mode + footer `Make Directory:`, phantom `made dir` shown; undo=2; jump_marks empty; dir created |
| 04.8  | PASS | touch mode + footer `Touch:`; undo=3; `made file.txt` created |
| 04.9  | PASS | touch+Esc: phantom `ghost.txt` shown then cleared; undo=3 unchanged; absent in entries/screen/disk |
| 04.10 | PASS | delete `made file.txt`: mode=normal, clipboard=null, undo=4; `Deleted 1 items`; gone from entries+fixture; in Trash/files + Trash/info |
| 04.11 | PASS | delete `amp & spaced.txt`: undo=5; `Deleted 1 items`; gone; in Trash/files |
| 04.12 | PASS | gT → mode=trash; hint line present; rows for both items with `$FIXTURE` parent; `amp & spaced.txt` top row + highlight (reverse video) |
| 04.13 | PASS | `r` restore: mode=trash (stays open), undo=5; `wiederhergestellt: 1 Element(e)...`; amp row gone, `made file.txt` slid up + highlighted; amp restored to disk |
| 04.14 | PASS | `r` restore last: mode=trash, undo=5; `(the trash is empty)` shown; made file restored; Trash/files empty |
| 04.15 | PASS | `q` closes view: mode=normal, undo=5, tabs count unchanged (1, did not close_tab/quit); both files back in entries+pane |
| 04.16 | PASS | delete dir `sub dir & stuff`→undo=6, gone; gT top row is dir (📁) + highlight; `r` restore; Esc→normal; dir back; `inner.txt` content survived |
| 04.17 | PASS | relaunch use_trash=false: mode=normal, total=2, sel=doomed.txt, undo=0; both listed |
| 04.18 | PASS | permanent delete: undo=1 (barrier), redo=0; `Deleted 1 items`; entries only keeper.txt; doomed gone; NO Trash dir created |
| 04.19 | PASS | `u`: WARN `kann nicht rückgängig gemacht werden: permanentes Löschen`; undo=1 unchanged, redo=0; doomed stays gone |
| 04.20 | PASS | gT: mode=normal (no overlay); WARN `Trash is disabled (use_trash = false) — nothing to show.` |

**Tally:** 20 steps, 20 PASS, 0 FAIL, 0 SKIP.

## Bug reports

None. Every step matched both the socket belief and the on-screen ground truth.

## Protocol feedback

- **Scratchpad collision between concurrent executors.** The orchestrator hands
  every section the SAME scratchpad directory. When I wrote my env vars to
  `scratchpad/env.sh`, a concurrently-running section (09) overwrote that exact
  file with its own paths; a later `source env.sh` then made my commands target
  section 09's socket/fixture. Recovered by locating my fixture on disk
  (`sub dir & stuff` + `alpha.txt` + `amp & spaced.txt` signature) and using a
  uniquely-named env file (`env-sec04-mine.sh`) plus unique session/socket names
  (`rfm-sec04-qa`, `/tmp/rfm-sec04-qa.sock`). Recommendation: the harness should
  tell executors to namespace scratchpad filenames per section (e.g.
  `env-$N.sh`), or give each executor an isolated scratch dir.
- **Left-over `rfm-sec04` tmux session not mine.** A `rfm-sec04` session existed
  running an injection-test fixture (`$(touch INJECTED).txt`, `plain.txt`) — not
  section 04's fixture — almost certainly created by another executor whose env
  leaked. I did NOT kill it (I only own `rfm-sec04-qa`). Harness note: the plain
  `rfm-sec04` / `/tmp/rfm-sec04.sock` names from the README are unsafe under
  parallel execution with env leakage; the section-specific names are the right
  guard but the README default recipe collides.
- **Navigation instruction ("press `j` until selection == X, max 6")** is
  direction-naive: after a rename/delete the target can sort ABOVE the current
  selection, so `j` never reaches it and the loop silently exhausts. I switched
  to `k` where the target was above. Suggest the protocol say "press `j`/`k`
  toward the target" (04.10/04.11/04.16 already hint `j`/`k`, but 04.2 says only
  `j`).
- **04.10 grep pitfall (self-inflicted, not a protocol flaw):** the substring
  `made file.txt` also appears inside the `log` "Deleted 1 items" trail context;
  assert against the full `entries center` JSON, not a bare grep. Noted for
  future runs.
