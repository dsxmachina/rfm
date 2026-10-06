# Run report — Section 01: Startup and basic movement

**Environment:** tmux 120x30; binary `./target/debug/rfm` (18 MB, built Jul 31).
All optional tools present: `zoxide`, `ffmpeg`, `openssl`, `zip`, `socat`, `tmux`.
Graphics protocol resolved to `half-block` (inside tmux, as expected).
Commit: develop @ cf90c81. No `error.log` written (clean exit).

**Note on environment artifact:** the shared harness places `FIXTURE=$(mktemp -d)`
directly under `/tmp`, so rfm's *left* (parent) panel is `/tmp` itself — a
directory with ~6170 entries that is continuously churned by other processes
(systemd, other test sessions). rfm's file-watcher on `/tmp` fires ~2×/sec,
each firing triggering a full re-scan ("Updating: /tmp" → "request new
dir-panel" → "panel-update: left <- /tmp"). Two consequences observed, both
environmental (not rfm bugs):
1. `seq` never stays stable while idle (advances every ~500 ms with no input).
2. The socket `log` history (200 lines) is ~97% "Updating: /tmp" spam, which
   evicts rfm's own TRACE lines (`move-left`, etc.) within ~1 s.
The on-screen render stays stable regardless (verified byte-identical across
two idle captures), and every movement's *state* + *screen* assertions hold.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 01.1  | PASS | Initial state/layout all correct; header shows selected-entry path, center 6 visible, left has fixture basename selected, preview shows clip.txt, footer `d…1/6`, highlight on `Bilder & Videos`. |
| 01.2  | PASS* | Debug-query-no-side-effect intent holds (screen byte-identical across idle). `seq`-identical sub-assertion NOT met — `seq` churns due to /tmp watcher (environmental, see note). |
| 01.3  | PASS | `j`→`many` idx1, seq up, log has `move-down`+`key-event Char('j')`, footer 2/6, preview `f00`. |
| 01.4  | PASS | `Down`→`subdir_a` idx2, footer 3/6, preview `inner.txt`. |
| 01.5  | PASS | `k`→`many`; `Up`→`Bilder & Videos` idx0; two `move-up` lines; footer 1/6. |
| 01.6  | PASS | `k` at top clamps idx0 (no wrap), seq up, no WARN/ERROR, footer 1/6. |
| 01.7  | PASS | `G`→`charlie.txt` idx5; entries array_pos=6≠idx5; footer 6/6; footer perms `-rw`; highlight present (emitted as `\x1b[0;7m`). |
| 01.8  | PASS | `j` at bottom clamps idx5, seq up, no WARN/ERROR, footer 6/6. |
| 01.9  | PASS | First `g`: mode normal, no move, buffer `g` rendered near footer center (overwrites mime char → `text/plaig`); second `g`→top idx0, buffer gone, footer 1/6. |
| 01.10 | PASS | `l`→`Bilder & Videos`, sel `clip.txt` total1, left_path fixture, entries-left has dir selected. zoxide executed with escaped path `'/tmp/.../Bilder & Videos'` and completed successfully (no failure line). Footer 1/1. |
| 01.11 | PASS* | `h`→fixture, sel `Bilder & Videos` restored, total6, footer 1/6, highlight correct. `move-left` TRACE line evicted by /tmp log spam (unverifiable, environmental). |
| 01.12 | PASS | `jj`→subdir_a idx2; Right→subdir_a/inner.txt; Left→fixture, sel `subdir_a` idx2 (restored off-index-0, not idx0); footer 3/6. |
| 01.13 | PASS | into `many` (f00, total40, 1/40); `C-f`→f27 idx27 (H=27), footer 28/40, highlight on f27. |
| 01.14 | PASS | `C-f`→f39 idx39 (clamp), footer 40/40, last row f39. |
| 01.15 | PASS | `C-b`→f12 idx12, footer 13/40, highlight f12. |
| 01.16 | PASS | `C-u`→f00 idx0 (saturate); `C-d`→f13 idx13, footer 14/40, highlight f13. |
| 01.17 | PASS | `NPage`→f39 idx39 (clamp); `PPage`→f12 idx12, footer 13/40, highlight f12. |
| 01.18 | PASS | `gg`→f00 idx0; `h`→fixture, sel `many` idx1 total6, footer 2/6, highlight many. |
| 01.19 | PASS | `Q` quits: process gone, socket dead, shell prompt back, no layout, terminal sane. |
| 01.20 | PASS | relaunch (mode normal, 1 tab); `q` on last tab quits: process gone, socket dead, prompt back. |

`*` = functionally PASS; a purely environmental sub-assertion (seq/log churn from the /tmp parent watcher) could not be verified. No rfm defect.

**Tally:** 20 steps — 20 PASS, 0 FAIL, 0 SKIP.

## Bug reports

None. No rfm defects found in this section.

## Protocol feedback

- **Fixture parent = /tmp breaks two socket-based invariants.** Because
  `FIXTURE=$(mktemp -d)` lands under `/tmp`, the *left* panel watches `/tmp`
  (~6170 entries here) which other processes churn constantly. This makes
  01.2's "`seq` identical between two idle `state` queries" assertion
  effectively impossible to satisfy on a busy machine (seq advances every
  ~500 ms with zero user input), and floods the 200-line `log` history with
  "Updating: /tmp" so that rfm's own TRACE lines (`move-left` in 01.11,
  `move-right`/`zoxide` in 01.10 if you wait) are evicted within ~1 s.
  Suggestion: nest the fixture one level down (`FIXTURE=$(mktemp -d)/box; mkdir`)
  or `mktemp -d` under a quiet base (e.g. `$XDG_RUNTIME_DIR` or a dedicated
  scratch root) so the parent panel is a small, stable directory. Then check
  the `log`/`seq` assertions immediately after the keypress (before churn
  evicts them) as a fallback.
- **Highlight detection substring is too narrow.** The section says to grep
  for `[7m`, but rfm emits the reverse-video SGR as `\x1b[0;7m` (reset+reverse
  combined) on file rows and as `\x1b[7m` elsewhere. A naive `grep '[7m'` (or a
  literal `\x1b[7m` match) misses the `0;7m` rows (would have caused a false
  FAIL on 01.7). The technique note should say "match `[7m` OR `[0;7m`" (i.e.
  the reverse attribute may appear inside a combined SGR).
- **01.1 footer mime for a directory.** The footer shows `text/plain` while a
  *directory* (`Bilder & Videos`) is selected. The section correctly says "do
  not assert a mime type for a directory selection", so this is not filed as a
  bug — but it is a slightly surprising display (a directory footer showing a
  text mime). Worth a dedicated check in a later section if intended behavior.
- **`log [n]` count semantics under spam.** `log 40` returned only the last 40
  retained lines; because of the /tmp spam these were all watcher noise. There
  is no way via the socket to filter by target/message, so verifying a specific
  TRACE line requires `log 200` and hoping it survived. A `log grep <substr>`
  socket verb would make trace-line assertions robust.
