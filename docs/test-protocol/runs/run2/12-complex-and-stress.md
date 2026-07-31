# Run 2 — Section 12: Complex interactions & stress

**Date:** 2026-07-31
**Binary:** `./target/debug/rfm` (rebuilt WITH the 12.8 fix)
**Terminal:** tmux 3.6a, 120x30 (resized to 100x25 and back during 12.4/12.5)
**Tools present:** tmux, socat, zoxide, ffmpeg (all available)
**Purpose:** Re-run to CONFIRM the run-1 fix for 12.8 (stale preview column
after deleting the entered dir then `h` back).

## Headline result

**12.8 — the previously-failing step — now PASSES.** After `rm -rf doomed`
(the entered cwd) then `h` back to the fixture, the preview (right) column
correctly shows `inner.txt` (the newly-selected `subdir`'s content), NOT the
deleted `doomed`'s stale `x.txt`. `preview_path` == `.../fx/subdir` matches the
screen. Reproduced cleanly TWICE (main run + a fresh minimal harness): x.txt
occurrences in the pane after `h` == 0 in both.

## Per-step results

| Step | Result | Notes |
|------|--------|-------|
| 12.1 Baseline sanity + first canary | PASS | mode=normal, cwd=fixture, total=22, selection=doomed, selected_idx=0, marked=[], undo_depth=0; entries=22 (one selected=doomed, none hidden); screen Miller layout correct, x.txt in preview. lat=6ms |
| 12.2 Rapid burst down (20×j) | PASS | selected_idx=20, seq 25>4, exactly one selected (long name) == state.selection; highlight on that row, pane fully drawn. lat=5ms |
| 12.3 Rapid burst up (20×k) | PASS | selection=doomed, selected_idx=0 (fully reversible); two idle snapshots both seq=45 (stable-while-idle). Screen consistent with 12.1. lat=5ms |
| 12.4 Resize down 100x25 | PASS | state unchanged (cwd/selection/selected_idx/total/mode); redrawn at 100 cols / 25 rows, no artifacts, no leftover fragments, doomed highlighted. lat=5ms |
| 12.5 Resize back 120x30 + baseline diff | PASS | state identical to 12.1; capture BYTE-IDENTICAL to BASELINE_CAPTURE (rows 1-28 and the log-widget row). lat=5ms |
| 12.6 Unicode + long filenames | PASS | entries contains emoji, NFD (byte-exact grep -F), 204-char long name. Each became state.selection verbatim (emoji idx21, long idx20, café-nfd.txt idx4 byte-exact). Long name truncated within center column, no preview overflow. No WARN/ERROR from navigation. lat=6ms |
| 12.7 External bulk churn | PASS | After convergence (seq stable 106): total=22, no churn* in entries, exactly one selected, disk count=22, no ERROR lines. Screen shows no churn remnants. lat=5ms |
| **12.8 cwd deleted underneath rfm** | **PASS (was FAIL in run 1)** | See detailed section below. No wedge; after `h`: cwd=fixture, total=21, mode=normal, **preview column shows inner.txt not stale x.txt**. lat=6ms/5ms |
| 12.9 Failing bg command in log | PASS | log 200 has all 4 lines in order with correct levels (INFO Queueing, INFO Executing, WARN [fail] boom-stderr, ERROR failed exit 3); queue_active=null, queue_len=0. Widget shows `error: Command 'fail' failed with exit code 3` on screen within TTL (3/3 samples). lat=5ms |
| 12.10 Log-widget TTL | PASS | After ~13s: ERROR line ABSENT from screen (0/3 samples — TTL expired + redrew); log 200 STILL retains it, level=ERROR, age_secs 14.2s and 45.8s (>=10). lat=6ms |
| 12.11 Final wedge-canary sweep | PASS | 5 latencies all <1000 (7,6,5,5,4); two idle snapshots both seq=137; await-idle idle:true in 5ms; capture normal fully-drawn, no overlay |
| 12.12 error.log post-mortem | PARTIAL PASS | FILE part PASS: `$FIXTURE/error.log` exists, contains `ERROR (Ns ago): Command 'fail' failed with exit code 3` (2×), plus INFO Queueing/Executing, WARN [fail] boom-stderr, and TRACE lines (--debug-socket verbosity). Format `{level} ({age}s ago): {msg}` correct. SCREEN banner part SKIP — see protocol feedback |
| 12.13 Final teardown checklist | PASS | session gone; socket removed; fixture = 21 entries + error.log, no stray .part/zip; doomed gone; no orphan sec12 rfm processes; no repo-root error.log; git working tree shows only pre-existing protocol-doc edits (I ran no mutating git) |

**Tally:** 12 PASS, 0 FAIL, 1 PARTIAL (12.12 file PASS / screen-banner SKIP).
Max canary latency observed across the whole section: **7 ms** (well under the
1000 ms wedge threshold; no reading ever approached it, no socket timeout).

## 12.8 — the previously-failing step, in detail (CONFIRMED FIXED)

**Sequence:** gg → l (enter `doomed`, cwd=`.../fx/doomed`, total=1) → harness
`rm -rf $FIXTURE/doomed` → wait 1s → wedge probe → `h` → poll seq stable.

**Wedge probe after rm:** `await-idle` returned `{"idle":true,...}` promptly,
lat=6ms, no panic/backtrace on screen, borders+header intact. rfm still believed
cwd=`.../fx/doomed`, selection=x.txt, total=1 (stale listing — the documented
robustness-only behavior, no explicit deleted-cwd recovery). Left column already
reflected doomed's removal.

**After `h` (the fix target):**
- socket: cwd=`.../fx`, total=21, mode=normal, selection=`subdir`,
  **preview_path=`.../fx/subdir`**
- screen preview (right) column:
  ```
  │ 📁fx      22 │ 📁subdir                                 1 │ 🖹inner.txt   0 B
  ```
  The preview shows **inner.txt** (subdir's child), NOT doomed's `x.txt`.
  `grep -c x.txt` on the pane after `h` == **0**.

In run 1 this preview column showed the deleted dir's stale `x.txt`. It is now
correct. Reproduced a second time in a fresh minimal harness (subdir + aaa.txt +
doomed): after `h`, preview_path=subdir, pane shows inner.txt, x.txt count == 0.

**Note on selection:** the protocol text (12.8) says "After h: cwd==$FIXTURE,
total==21" without naming the selection. Observed selection is `subdir` (the
first remaining directory) — correct, since `doomed` no longer exists to land on.

## Minor finding (not the focus bug; pre-existing, low severity)

**Stale left-panel child-count after deleting the selected parent-entry's dir.**
When the entered directory (`doomed`) is deleted while rfm is inside it, then `h`
returns to the parent: the LEFT (parent) column's per-directory child-count
annotation for `fx` stays stale — it shows `fx  22` when the fixture now holds 21
entries (disk-verified). It does not refresh on subsequent center navigation
(j/k). The CENTER listing (21 entries) and the PREVIEW column are both correct;
only the left column's count-badge is stale. Reproduced twice (main run showed
`fx 22` vs disk 21; a 7-entry minimal harness showed `fx 7` vs disk 6).

- Severity: papercut (cosmetic count badge; navigation/listing/preview all correct)
- Class: render (the count metadata on the parent panel is not recomputed)
- Distinct from the 12.8 preview bug (which IS fixed). A fresh entry that does
  NOT involve deleting the currently-selected left entry shows a correct count
  (the fresh minimal 12.8 repro with subdir selected showed `fx 3` correctly),
  so the trigger is narrow: the deleted dir was the left panel's selection.

I did not file this as a full BUG report because it is a minor pre-existing
cosmetic issue outside this re-run's fix-confirmation scope; recording it here
for the maintainers.

## Protocol feedback

1. **12.12 screen-banner assertion is unverifiable with the README direct-launch
   form.** The section launches rfm AS the tmux session command
   (`tmux new-session ... "env ... rfm ..."`). When rfm exits on `Q`, its process
   is the session's root command, so the session TERMINATES immediately — the
   pane is gone and `tmux capture-pane` returns empty (exit 1). The stderr banner
   (`Encountered an unexpected error. This is a bug!` +
   `https://github.com/dsxmachina/rfm/issues` + the `Error:` list) is printed to
   the closing pane and cannot be captured. The step's "Expect (screen): the pane
   shows ... shell prompt back and the stderr banner" is thus not observable under
   the mandated launch form. Suggest either: (a) launch rfm under a wrapper shell
   (`... rfm ...; exec $SHELL` or tmux `remain-on-exit on`) so the banner survives
   for capture, or (b) explicitly downgrade the 12.12 screen-banner check to
   "best-effort / may be unobservable under direct-launch" and rely on the
   error.log FILE contents (which fully confirm the ERROR path) plus the socket
   history as the authoritative assertion. The FILE part passed unambiguously.

2. **12.9 capture-within-TTL is timing-fragile if the failure-line poll loop is
   slow.** Polling `log 200` (each socat round-trip + 0.5s sleeps) plus the
   3-sample capture can easily consume >10s before the first capture, by which
   point the widget's 10s display TTL has expired and the line is gone from
   screen — producing a false "not on screen" reading. On re-trigger with an
   immediate capture the line was present (3/3). Suggest the step note explicitly:
   trigger, poll with a TIGHT loop, and capture the widget within a couple seconds;
   or re-trigger the command right before the capture. (Re-triggering is harmless —
   it just adds a second ERROR entry to history, which 12.10/12.12 tolerate.)

3. **Teardown item 2 (socket survives process exit): not observed here.** The
   README/12.13 says "the socket file survives process exit by design; confirm
   `[ ! -e $SOCK ]` after `rm`". In this run the socket `/tmp/rfm-sec12.sock` was
   ALREADY gone before the `rm` (rm reported no-such-file). rfm apparently removed
   its own socket on the `Q` quit path this time. Not a bug (the end state — socket
   absent — is what teardown wants), but the "survives exit" wording did not hold;
   worth reconciling the doc with actual behavior.

4. **12.8 selection expectation should be spelled out.** The step asserts cwd and
   total after `h` but not the selection; an executor might expect `doomed` (the
   pre-entry selection). Since `doomed` is deleted, rfm correctly lands on
   `subdir`. Recommend the step add "selection is a valid remaining entry (doomed
   is gone), e.g. subdir" to prevent a mis-filed assertion.

## Environment notes

- Commit context: branch develop; binary rebuilt with the fix. zoxide and ffmpeg
  present (12.8's `zoxide add doomed` ran and completed successfully — visible in
  error.log). A parallel executor's section-04 rfm process was running
  concurrently; it is not attributable to this section and was left untouched.
- No mutating git commands were run. Only mktemp fixtures and this report path
  were touched.
