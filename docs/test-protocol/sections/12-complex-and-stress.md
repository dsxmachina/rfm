# Section 12 — Complex interactions & stress

Robustness under rapid input, resize, external churn, deleted cwd, hostile
filenames, plus the error-reporting pipeline (log widget TTL, socket log
history, `./error.log` post-mortem). A latency canary runs through the whole
section: the event loop must never wedge.

Harness per README (`N=12`, `SESSION=rfm-sec12`, `SOCK=/tmp/rfm-sec12.sock`).

## Section fixture (created before launch)

```bash
for i in $(seq -w 1 15); do touch "$FIXTURE/file$i.txt"; done
mkdir "$FIXTURE/subdir";  touch "$FIXTURE/subdir/inner.txt"
mkdir "$FIXTURE/doomed";  touch "$FIXTURE/doomed/x.txt"
touch "$FIXTURE/a file with spaces.txt" "$FIXTURE/Bilder & Videos.txt"
touch "$FIXTURE/🎉🎊emoji📁.txt"                       # emoji name
touch "$FIXTURE/$(printf 'cafe\xcc\x81-nfd.txt')"       # 'e' + combining acute (NFD)
LONG=$(printf 'l%.0s' $(seq 1 200)); touch "$FIXTURE/$LONG.txt"   # 204-byte name
```

Total: 2 directories + 20 files = **22 entries**, none hidden. Directories sort
before files, so the initial selection is `doomed` (alphabetically before
`subdir`). Do NOT assert the relative sort order of the unicode/space/`&` file
names anywhere — collation is not part of this section's contract; assert
presence via the `entries` reply instead.

## Section config (written into `$CFG/config.toml` before launch)

```toml
[commands.fail]
keys = [ "xe" ]
cmd  = "echo boom-stderr >&2; exit 3"
# interactive defaults to false -> queued background command
```

(`x` has no default binding, so `xe` collides with nothing; no
"dropped default" notice and no upgrade overlay are expected at startup.)

## Section-specific launch deviation

`./error.log` (step 12.12) is written relative to rfm's **process cwd**, which
stays at the launch directory (rfm only chdirs for zip/tar/extract/interactive
commands — none used here). To make the post-mortem path deterministic, launch
rfm with its working directory set to `$FIXTURE`. Use the README direct-launch
form (binary as the session command — NOT `send-keys` into an interactive shell,
which Atuin/zsh history-search would intercept) with `tmux new-session -c` to
set the cwd, and an absolute path to the binary:

```bash
RFM=$(pwd)/target/debug/rfm
tmux kill-session -t $SESSION 2>/dev/null; rm -f $SOCK
tmux new-session -d -s $SESSION -x 120 -y 30 -c "$FIXTURE" \
  "env XDG_CACHE_HOME=$CACHE XDG_STATE_HOME=$STATE _ZO_DATA_DIR=$ZO \
   $RFM --debug-socket $SOCK --config $CFG $FIXTURE"
until [ -S $SOCK ]; do sleep 0.1; done
```

The fixture lives under the quiet README parent (`PARENT=$(mktemp -d);
FIXTURE=$PARENT/fx`), so the left/parent panel watches a quiet dir. THIS is what
makes the "two idle `state` snapshots report the same `seq`" invariant
(12.3/12.11) verifiable and the 12.5 baseline-diff clean — under a churning
`/tmp` parent `seq` advances with zero input and the left column scrolls on its
own.

## Latency canary (used throughout)

```bash
lat() {
  local t0=$(date +%s%N)
  echo state | socat - UNIX-CONNECT:$SOCK > /dev/null
  echo $(( ( $(date +%s%N) - t0 ) / 1000000 ))
}
```

**Run `lat` after EVERY step in this section and record the value.** Every
reading must be `< 1000` (ms). A reading ≥ 1000 ms — and especially a socket
timeout (10 s) — means the event loop is wedged: that is a finding, file it
with the step that preceded it. Debug queries themselves never advance `seq`,
so the canary does not perturb other assertions.

---

### 12.1 — Baseline sanity + first canary reading

**Action:** none beyond launch. `echo await-idle | socat - UNIX-CONNECT:$SOCK`,
then `echo state | socat ...`, `echo "entries center" | socat ...`, then
`tmux capture-pane -t $SESSION -p`. Run `lat`.

**Expect (socket):** `mode=="normal"` (no upgrade-notice overlay — the scratch
config has no conflicting bindings), `cwd=="$FIXTURE"`, `total==22`,
`selection=="doomed"`, `selected_idx==0`, `marked==[]`, `undo_depth==0`.
`entries center` returns 22 objects, exactly one with `"selected":true`
(name `doomed`), none `"hidden":true`. `lat` < 1000.

**Expect (screen):** Miller layout: header path shows the fixture dir; center
column lists `doomed` and `subdir` before the files; `doomed` is the
highlighted row; right column shows `x.txt` (dir preview of `doomed`; if it
still looks like a loading placeholder, poll `state` until `seq` stabilizes,
then re-capture). No error/overlay text.

**Note:** record this capture as `BASELINE_CAPTURE` (save to a file in the
scratchpad) — step 12.5 compares against it.

### 12.2 — Rapid-input burst down (20 × j)

**Action:** `tmux send-keys -t $SESSION "jjjjjjjjjjjjjjjjjjjj"` (one call, 20
`j` characters — a burst, no pacing). Then `await-idle`, `state`,
`entries center`, capture. Run `lat`.

**Expect (socket):** `await-idle` returns `{"idle":true,...}` well within its
30 s bound. `selected_idx==20` (0-based, started at 0, 22 visible entries — no
clamp hit). `seq` strictly greater than the 12.1 reading (do NOT assert an
exact delta; ~5 ticks per keypress). Exactly one `"selected":true` in
`entries`, and its `name` equals `state.selection`.

**Expect (screen):** the row showing `state.selection`'s name is the
highlighted row of the center column; the pane is fully drawn (no half-updated
rows, no duplicated highlight).

**Note:** no multi-key default sequence starts with `j`, so all 20 keys
dispatch individually — no sequence-timeout stalls expected.

### 12.3 — Rapid-input burst back up (20 × k)

**Action:** `tmux send-keys -t $SESSION "kkkkkkkkkkkkkkkkkkkk"`. Then
`await-idle`, `state`, capture. Run `lat`. Additionally take two `state`
snapshots ~1 s apart with no input in between.

**Expect (socket):** `selection=="doomed"`, `selected_idx==0` — the burst is
exactly reversible, no lost or double-applied keys. The two idle snapshots
report the **same** `seq` (stable-while-idle invariant — proves no runaway
self-wakeups after the burst).

**Expect (screen):** `doomed` highlighted again; capture is visually
consistent with 12.1's center column (log-widget area may differ).

### 12.4 — Terminal resize down

**Action:** capture the pane (pre-resize reference), then
`tmux resize-window -t $SESSION -x 100 -y 25` (tmux ≥ 2.9; if the command is
missing, note as protocol feedback and skip 12.4/12.5). Then `await-idle`,
`state`, capture. Run `lat`.

**Expect (socket):** `cwd`, `selection`, `selected_idx`, `total`, `mode` all
unchanged by the resize (resize redraws, it must not mutate navigation state).

**Expect (screen):** capture now is 100 columns wide / 25 rows; the full
layout is redrawn at the new size — header path line present, three panel
columns, highlighted row still `doomed`. No artifacts: no leftover characters
from the 120-column layout (e.g. truncated border fragments) and no blank
un-redrawn regions.

### 12.5 — Terminal resize back + capture diff

**Action:** `tmux resize-window -t $SESSION -x 120 -y 30`, `await-idle`,
capture. Run `lat`. Diff the capture against `BASELINE_CAPTURE` from 12.1.

**Expect (socket):** state identical to 12.1 for `cwd`/`selection`/
`selected_idx`/`total`/`mode`.

**Expect (screen):** the capture is **identical** to `BASELINE_CAPTURE`,
allowing differences only in the log-widget lines (bottom area) — any other
diff line (borders, entries, header, highlight) is a redraw artifact: file it
as a bug with both captures.

### 12.6 — Unicode + long filenames render without panic

**Action:** `echo "entries center" | socat - UNIX-CONNECT:$SOCK` and grep the
JSON for the three names. Then navigate the cursor onto each of them in turn:
easiest is `G` (bottom), then `k` repeatedly, checking `state.selection` after
each until each of the three names has been the selection once (order
unimportant, ≤ 25 keys total). Capture after each. Run `lat`.

**Expect (socket):** `entries` contains `"🎉🎊emoji📁.txt"`, the NFD name
(byte-match with `grep -F "$(printf 'cafe\xcc\x81-nfd.txt')"` — do not let the
shell/locale NFC-normalize it), and the 204-char `lll….txt` name. Each becomes
`state.selection` verbatim when the cursor lands on it. No new WARN/ERROR
lines in `log 20` from navigating them.

**Expect (screen):** each capture shows the highlighted row rendering the name:
the emoji name visibly contains `emoji`; the combining-char name renders as
`café-nfd.txt` (width may vary by terminal — assert the substring `-nfd.txt`);
the long name is displayed truncated **within** the center column — it must
not overflow into or corrupt the preview column. rfm is still running (no
panic text in the pane, socket still answers).

**Note:** while cursoring, the preview pane re-decodes per selection
(rate-limited 500 ms); ignore preview content here, only layout integrity
matters.

### 12.7 — External bulk churn while navigating

**Action:**
```bash
for i in $(seq -w 1 50); do touch "$FIXTURE/churn$i"; done   # burst-create
tmux send-keys -t $SESSION "jkjkjkjkjk"                       # navigate during churn
echo await-idle | socat - UNIX-CONNECT:$SOCK
rm -f "$FIXTURE"/churn*                                       # burst-delete
tmux send-keys -t $SESSION "jkjk"
echo await-idle | socat - UNIX-CONNECT:$SOCK
```
Then poll `state` (every ~0.5 s, up to 10 s) until `seq` is stable across two
consecutive polls (watcher events are async; `await-idle` does not cover
in-flight panel reloads). Then `state`, `entries center`, capture. Run `lat`.

**Expect (socket):** after convergence, `total==22` and `entries` contains no
name starting with `churn` — rfm's belief matches disk
(`ls "$FIXTURE" | wc -l` == 22). `selection` is some valid entry
(`selected_idx < total`); exactly one `"selected":true`. No ERROR lines in
`log 30` (transient churn must not error).

**Expect (screen):** center column shows the original 22-entry listing, no
`churn*` remnants, highlight on the row matching `state.selection`. A capture
still showing `churn*` while `entries` does not (or vice versa) is a
stale-render/stale-state bug — file with both outputs.

### 12.8 — cwd deleted underneath rfm

**Action:** navigate into `doomed`: `tmux send-keys -t $SESSION gg` (top =
`doomed`), then `l`. `await-idle`; confirm `state.cwd=="$FIXTURE/doomed"`.
Then from the harness shell: `rm -rf "$FIXTURE/doomed"`. Wait 1 s, then
`await-idle`, `state` (this IS the wedge probe), capture. Then
`tmux send-keys -t $SESSION h`, `await-idle`, `state`, capture. Run `lat`.

**Expect (socket):** every socket query replies promptly (< 1 s) — no wedge,
no 10 s timeout. After the `rm`, `state` still answers with a coherent
snapshot (cwd may still read the deleted path, or the listing may have gone
empty — record the actual behavior as protocol feedback; no explicit
deleted-cwd recovery path exists in manager.rs, so the contract here is
robustness only, not a specific recovery). After `h`: `cwd=="$FIXTURE"`,
`total==21` (doomed is gone), `mode=="normal"`. `selection` is a valid
*remaining* entry (doomed no longer exists), e.g. `subdir` — do NOT assert it
is `doomed`. **`preview_path` must match the new selection** (e.g.
`$FIXTURE/subdir`), not the deleted `doomed` — this is the regression check for
the fixed stale-preview bug; it must hold immediately after `h`, without a
cursor bounce.

**Expect (screen):** after the `rm`, the pane shows no panic/backtrace text
and rfm is still drawn (borders + header intact). After `h`, the fixture
listing is shown again without `doomed`, with a valid highlight row, and the
preview column reflects the current selection. **Known cosmetic issue (not a
failure):** the LEFT column's child-count badge for the fixture dir may read
one higher than the on-disk count (it reflects the count cached before the
deletion; see KNOWN-ISSUES.md) — assert only the center listing and the
preview column here, not the left-panel count badge.

**Note:** entering `doomed` queues `zoxide add` — if zoxide is installed, a
`Executing command 'zoxide'` info line may appear in the widget/log; not a
failure.

### 12.9 — Failing background command lands in log (screen + socket)

**Action:** `tmux send-keys -t $SESSION x e` (the `[commands.fail]` binding).
`await-idle`. Poll `echo "log 200" | socat - UNIX-CONNECT:$SOCK` (wide window,
every 0.5 s, up to 10 s — the queue executor is rate-limited at 500 ms) until
the failure line appears; use `log 200` (not a short window) so the INFO/WARN/
ERROR lines are not evicted by TRACE churn. Then `state` and capture
**immediately** (within the 10 s display TTL); sample the capture 2–3× since a
single one can race a redraw. Run `lat`. Record the wall-clock time of the
capture — 12.10 needs it.

**Expect (socket):** `log` history contains, in order:
`Queueing command 'fail': echo boom-stderr >&2; exit 3` (INFO),
`Executing command 'fail': ...` (INFO),
`[fail] boom-stderr` (WARN),
`Command 'fail' failed with exit code 3` (ERROR).
`state.queue_active==null` and `queue_len==0` after completion (don't assert
the transient non-null — the command finishes in milliseconds).

**Expect (screen):** the log widget (bottom of the pane) currently shows the
line `Command 'fail' failed with exit code 3` (widget shows info+ within its
10 s TTL). The main panels are unaffected.

**Note:** this deliberately seeds the ERROR that makes 12.12's `error.log`
exist — do not "clean it up".

### 12.10 — Log-widget TTL: gone from screen, kept in history

**Action:** wait until 11 s after the 12.9 capture (`sleep 11` is legitimate
here — we are testing the TTL itself). Then capture (sample 2–3×), and
`echo "log 200" | socat - UNIX-CONNECT:$SOCK` (the full ring — under any watcher
churn a short `log 30` window can push the retained ERROR line out even though
capacity-retention still holds it). Run `lat`.

**Expect (socket):** `log` STILL contains
`Command 'fail' failed with exit code 3` with `level=="ERROR"` and
`age_secs >= 10` — retention history is capacity-evicted only, the TTL does
not apply to it.

**Expect (screen):** the failure line is no longer anywhere in the capture —
the display TTL (10 s) expired it, and the expiry actually triggered a redraw
(the line must be *erased*, not merely stale on screen).

### 12.11 — Final wedge-canary sweep

**Action:** run `lat` 5 times in a row; take two `state` snapshots ~1 s apart
with no input; `echo await-idle | socat - UNIX-CONNECT:$SOCK` and time it.

**Expect (socket):** all 5 latencies < 1000 ms (report the max across the
WHOLE section in the run report); the two snapshots have identical `seq`
(idle-stable after everything this section threw at the loop); `await-idle`
returns `{"idle":true,...}` in < 1 s.

**Expect (screen):** capture is a normal, fully-drawn fixture listing;
no overlay, no error text.

### 12.12 — error.log post-mortem on quit

**Action:** `tmux send-keys -t $SESSION Q` (quit outright; capital Q). Wait
for exit: poll until the pane no longer shows the rfm layout (up to 5 s), then
`tmux capture-pane -t $SESSION -p`. Then from the harness shell:
`cat "$FIXTURE/error.log"`.

**Expect (screen, best-effort):** the pane shows rfm has exited. The stderr
banner (`Encountered an unexpected error. This is a bug!`,
`https://github.com/dsxmachina/rfm/issues`, and the `Error:` list including
`Command 'fail' failed with exit code 3`) is the *intended* output, but under
the README direct-launch form rfm is the tmux session's root command, so
`Q` terminates the session and `capture-pane` returns empty — the banner is
**not observable** this way. Treat the screen banner as best-effort and rely on
the **file** assertion below as the load-bearing check. (To observe the banner,
launch rfm under a wrapper shell — `tmux new-session … "…/rfm …; exec $SHELL"` —
or set `tmux set -t $SESSION remain-on-exit on` before quitting.)

**Expect (file):** `$FIXTURE/error.log` exists (process cwd = launch cwd = the
fixture, per the launch deviation) and contains a line matching
`ERROR (<N>s ago): Command 'fail' failed with exit code 3` (format
`{level} ({age}s ago): {msg}`), plus the surrounding retained history (the
INFO `Queueing`/`Executing` lines, the WARN `[fail] boom-stderr` line — with
`--debug-socket` verbosity even TRACE lines).

**Note:** the socket file may still exist after exit; teardown removes it. If
`error.log` is missing, first check it didn't land elsewhere
(`ls ./error.log` in the repo root — that would mean the pane `cd` was
skipped: harness error, not an rfm bug; rerun). If it truly wasn't written
despite the ERROR line having been in `log` — that's the bug to file.

### 12.13 — Final full-teardown checklist

Run and verify each item (this section ends the protocol run — leave nothing
behind):

1. `tmux kill-session -t $SESSION 2>/dev/null` — then `tmux has-session -t
   $SESSION 2>/dev/null` must fail.
2. `rm -f $SOCK` — then confirm `[ ! -e $SOCK ]`. Note: rfm removes its own
   socket on the clean `Q` quit path, so it may already be gone before the
   `rm` (the `rm -f` is idempotent); the socket only lingers when rfm is killed
   via `tmux kill-session` rather than quit.
3. Inspect `$FIXTURE` BEFORE deleting: expected leftovers are exactly the 21
   fixture entries (doomed was deleted in 12.8) plus `error.log` (12.12).
   Anything else (`.part` files, stray temp files, `output.zip`, …) is a
   finding — record it, then `rm -rf "$FIXTURE"`.
4. `rm -rf "$CFG" "$CACHE" "$STATE" "$ZO"`.
5. Confirm the repo working tree is untouched: no `error.log` in the repo
   root, `git status --porcelain` shows nothing new attributable to the run
   (do NOT run any mutating git command).
6. Verify no orphaned rfm processes: `pgrep -f "rfm --debug-socket $SOCK"`
   returns nothing.
7. Report: max canary latency observed, all bug reports, all protocol
   feedback.

---

**Teardown:** covered by 12.13 (this section's teardown IS the final
checklist).

**Section coverage gaps (deliberate):**
- No resize-while-in-overlay/console-mode or resize-in-split-view tests
  (split "too narrow" refusal is section-scoped elsewhere).
- Deleted-cwd is tested for robustness only, not for a specific recovery
  policy (none exists in source); deleting the *left* (parent) directory or
  the fixture root itself is not tested.
- No stress on image/graphics previews (emit reconcile under resize churn) —
  preview sections own that.
- Churn tests one create+delete cycle of 50 files, not sustained
  watcher-flood over minutes, and not churn in a *background tab*.
- Log-history capacity eviction (200-line ring overflow) is not exercised.
- error.log is verified for the background-command ERROR path only, not for
  panic-path (`error!("{panic_info}")`) post-mortems.
- Filename hostility: no newline-in-filename, no RTL/bidi override
  characters, no 255-byte-limit boundary probing.
- Rapid input is tested for j/k only, not burst-interleaved mode changes
  (e.g. mashing `/`+Esc) or burst tab creation/closing.
