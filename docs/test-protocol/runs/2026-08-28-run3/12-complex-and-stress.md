# Run 3 — Section 12: Complex interactions & stress

- **Date:** 2026-08-28
- **Branch:** feat/kitty-unicode-placeholders (binary prebuilt at `./target/debug/rfm`)
- **Harness:** session `tp3-12`, socket `/tmp/tp3-12.sock`, fixture under quiet
  parent (`PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`), scratch `--config`,
  `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs, launched directly as
  the tmux session command with `-c "$FIXTURE"` (per the section's launch
  deviation) and `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`.
- **Result:** 13 steps — **12 pass, 0 fail, 1 skip** (12.12, stale premise).
- **Bugs filed:** none.
- **Max canary latency over the whole section:** **6 ms** (readings: 4–6 ms,
  every step; `await-idle` timed at 6 ms in 12.11). The event loop never wedged.

## Per-step results

### 12.1 — Baseline sanity + first canary — PASS
Socket: `mode=normal`, `cwd=$FIXTURE`, `total=22`, `selection=doomed`,
`selected_idx=0`, `marked=[]`, `undo_depth=0`; `entries center` = 22 objects,
exactly one `selected:true` (`doomed`), none hidden. No upgrade overlay.
Screen: Miller layout, `doomed`/`subdir` before files, right column shows
`x.txt`. Baseline capture saved. lat 5 ms. `seq=4`.

### 12.2 — Rapid-input burst down (20 × j) — PASS
`await-idle` returned immediately (`seq=25`). `selected_idx=20`, selection =
the 204-char `lll…l.txt`; exactly one `selected:true` matching
`state.selection`. SGR capture: exactly one highlighted row in the center
column (`[0;7m` on the `lll…~` row). No lost/duplicated keys. lat 5 ms.

### 12.3 — Rapid-input burst back up (20 × k) — PASS
`selection=doomed`, `selected_idx=0` — burst exactly reversible. Two idle
snapshots 1 s apart both `seq=45` (idle-stable, no runaway wakeups). Screen:
`doomed` highlighted. lat 5 ms.

### 12.4 — Terminal resize down (100×25) — PASS
`resize-window` available. State unchanged (`cwd`/`selection`/`selected_idx`/
`total`/`mode`). Capture 25 rows / ≤99 cols, full layout redrawn, header + 3
columns + footer present, highlight on `doomed`, no 120-col artifacts, no
blank regions. lat 5 ms.

### 12.5 — Resize back + capture diff — PASS
State identical to 12.1 on all asserted fields. Capture **byte-identical** to
`BASELINE_CAPTURE` (`diff` clean — not even log-widget differences). lat 4 ms.

### 12.6 — Unicode + long filenames — PASS
`entries center` contains all three names byte-exact (`grep -cF` = 1 each,
NFD matched via `printf 'cafe\xcc\x81-nfd.txt'` bytes). Cursor visits:
`G` → `🎉🎊emoji📁.txt` (idx 21, highlighted row shows the emoji name);
`k` → 204-char name (selection matches verbatim; screen shows it truncated
with `~` **inside** the center column, 3 column separators intact — no
overflow into the preview column); 16 × `k` (computed from entries index, per
the SELECT helper) → NFD name (state JSON `"selection":"café-nfd.txt"`
byte-matched the NFD form, `selected_idx=4`; highlighted row shows
`café-nfd.txt`). No WARN/ERROR in `log 20`. Header row shows the long path
truncated at terminal width (cosmetic, expected). lat 5 ms.

### 12.7 — External bulk churn while navigating — PASS
50 `churn*` created + `jkjkjkjkjk` during, deleted + `jkjk` after. `seq`
stabilized after 3 polls (135). Converged: `total=22`, no `churn*` in
`entries`, disk `ls | wc -l` = 22, `selection=café-nfd.txt` (idx 4 < total),
exactly one `selected:true`. No ERROR lines in `log 30`. Screen: zero `churn`
occurrences, highlight matches selection. lat 5 ms.

### 12.8 — cwd deleted underneath rfm — PASS
Entered `doomed` (`gg` + `l`, `cwd` confirmed). After `rm -rf doomed`: every
socket query answered in <1 s (no wedge); state coherent; no panic/backtrace
text, layout intact. After `h`: `cwd=$FIXTURE`, `total=21`, `mode=normal`,
`selection=subdir` (valid remaining entry), and **`preview_path=
$FIXTURE/subdir` immediately** — the stale-preview regression check holds
(preview column showed `inner.txt`). Screen: no `doomed` anywhere, valid
highlight. Left-column badge read `fx 22` (on-disk 21) — this is **KI-1**
(KNOWN-ISSUES), observed again, not re-filed; excluded from assertion per the
step text. lat 5–6 ms. Recorded actual deleted-cwd behavior in protocol
feedback below.

### 12.9 — Failing background command lands in log — PASS (with stale-protocol deviation, see feedback)
`x` `e` triggered the `[commands.fail]` binding. `log 200` (first poll)
contained, in order: `Queueing command 'fail': …` (INFO), `Executing command
'fail': …` (INFO), `[fail] boom-stderr` (WARN), `Command 'fail' failed with
exit code 3` — at level **WARN**, not the ERROR the protocol text expects.
This matches commit `da35a98` ("downgrade recoverable user-op failures from
error! to warn!") — intended behavior, protocol text is stale (feedback #1).
`queue_active=null`, `queue_len=0`. Widget showed
`warn: Command 'fail' failed with exit code 3` within TTL. lat 5 ms.

### 12.10 — Log-widget TTL: gone from screen, kept in history — PASS
> 11 s after the 12.9 capture: 3 capture samples — failure line **absent**
from screen (erased, i.e. the expiry triggered a redraw). `log 200` still
retained it: `('WARN', 37.4s, "Command 'fail' failed with exit code 3")`,
`age_secs >= 10`. Same WARN-vs-ERROR level deviation as 12.9 (one root cause,
one feedback item). lat 5 ms.

### 12.11 — Final wedge-canary sweep — PASS
5 latencies: 5, 5, 4, 5, 4 ms (all < 1000). Idle snapshots `seq=156/156`.
`await-idle` returned `{"idle":true}` in 6 ms. Screen: normal fixture
listing, no overlay/error text. **Section max latency: 6 ms.**

### 12.12 — error.log post-mortem on quit — SKIP (stale premise; behavior verified consistent with current code)
Pre-quit check: **zero** ERROR-level lines in the retained history — the
seeding step (12.9) now produces WARN (da35a98), so the step's premise ("this
deliberately seeds the ERROR") no longer holds. `Q` quit cleanly (session
gone in ~200 ms, rfm removed its own socket). `$FIXTURE/error.log`: **not
written** — which is *correct* under the current code: `print_all_errors`
(src/main.rs:404) writes `./error.log` only when `logger.get_errors()`
(Level::Error filter, src/logger.rs:52-60) is non-empty. No `error.log` in
the repo root either (launch `-c` deviation worked). The error.log write path
itself was therefore **not exercised** this run — the step as written cannot
be verified until the protocol supplies a genuine Level::Error seed
(feedback #2). Not an rfm bug.

### 12.13 — Final full-teardown checklist — PASS
1. Session killed; `has-session` fails. 2. Socket already removed by rfm's
clean-quit path; `rm -f` idempotent; gone. 3. Fixture held **exactly** the 21
expected entries (doomed deleted in 12.8; no `error.log` per 12.12 — counted
as consistent, not a stray); no `.part`/temp/zip strays; fixture parent
removed. 4. `$CFG`/`$CACHE`/`$STATE`/`$ZO` removed. 5. Repo tree: nothing new
except `docs/test-protocol/runs/2026-08-28-run3/` (run reports). 6. No
orphaned rfm processes. 7. Max canary latency 6 ms; bugs: none; feedback
below.

## Bugs

None filed. (KI-1 re-observed at 12.8 exactly as documented in
KNOWN-ISSUES.md — not re-filed per instructions.)

## Protocol feedback

1. **12.9/12.10 expected log level is stale.** Since `da35a98` ("downgrade
   recoverable user-op failures from error! to warn!"), `Command 'fail' failed
   with exit code 3` is emitted at **WARN**, not ERROR. Update both steps'
   expected trails (and the widget line reads `warn: …`). The rest of the
   trail (INFO Queueing/Executing, WARN `[fail] boom-stderr`) matches exactly.
2. **12.12 needs a new ERROR seed.** `error.log` is written on exit only when
   the retained history holds a Level::Error line (`print_all_errors`,
   src/main.rs:404-418; `get_errors`, src/logger.rs:52-60). The WARN downgrade
   means the 12.9 command failure no longer triggers it, so the step is
   currently unverifiable as written. Either (a) find a legitimate
   fatal/inoperable-class ERROR to seed (per the new log-level policy), or
   (b) rewrite 12.12 as the negative assertion ("clean quit after WARN-only
   history writes NO error.log") plus a separate ERROR-path test elsewhere.
3. **12.8 deleted-cwd actual behavior (recorded as requested):** after
   `rm -rf` of the cwd, rfm keeps a coherent-but-stale snapshot — `cwd` still
   the deleted path, `total=1`, `x.txt` still listed in center — while the
   *left* column refreshes to show the parent without `doomed`. All queries
   stay <10 ms; recovery on `h` is complete and correct, including the
   immediate `preview_path` update (the run-2 regression fix holds).
4. **12.5 diff was byte-identical including the log-widget area** — the
   "allow log-widget differences" carve-out was not needed under the quiet
   fixture parent; keep it anyway for slower machines where a widget line may
   still be live.
5. Harness notes: `tmux resize-window` available (no skip needed);
   `send-keys -l` not needed this section (no colliding literals);
   per-section env file `env-tp3-12.sh` in the scratchpad worked cleanly
   across Bash calls.

## Environment

- tmux, socat, python3 available; zoxide present (its `Executing command
  'zoxide add …'` INFO line appeared after entering `doomed`, as the protocol
  anticipates).
- `image_protocol` resolved to `half-block` under the `env -u` launch prefix
  (no graphics assertions in this section).
- `ls` is aliased to lsd on this host — raw `ls | wc -l` counts used for
  assertions were unaffected.
