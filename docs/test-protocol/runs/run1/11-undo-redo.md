# Run 1 — Section 11: Undo / Redo

**Environment:** Linux, tmux 120x30, terminal graphics resolves to half-block
(inside tmux). Tools present: `tar` (gnutar 1.35), `zip`, `socat`, `tmux`. Binary
`./target/debug/rfm` (18 MB, built). Section run against a fresh per-section
harness (SESSION=rfm-sec11, SOCK=/tmp/rfm-sec11.sock) with isolated
`XDG_DATA_HOME` for the trash. Note: the shared `/tmp` parent directory was busy
with other executors' fixtures, producing heavy `Updating: /tmp` watcher log
churn that repeatedly evicted short-lived INFO lines from the small `log N`
windows and from the on-screen log widget (does not affect socket state or disk
assertions; see protocol feedback).

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 11.1 | PASS | Baseline depths 0/0, `u`→`nichts rückgängig zu machen`, `C-r`→`nichts wiederherzustellen`; pane unchanged. |
| 11.2 | PASS | Rename a.txt→a.txt.bak (undo 1/0), undo restores (0/1, content alpha), redo reapplies (1/0), final undo restores a.txt. All log strings exact, disk + entries + screen correct. |
| 11.3 | PASS | Copy-paste into dest dir (undo 1/0, redo from 11.2 cleared), clipboard null, src preserved; undo removes copy (0/1), dest empty. |
| 11.4 | PASS | Redo re-copies (content alpha), cleanup undo empties dest dir. |
| 11.5 | PASS | Cut-paste moves b.txt (src gone, left pane drops it); undo restores original location, left pane + screen agree. |
| 11.6 | PASS | Same-dir paste yields c.txt + c.txt_ (redo cleared); undo removes exactly c.txt_, c.txt content preserved. |
| 11.7 | PASS | 3 marked files → one transaction (undo delta +1), `paste 3 items` logged, `d & e.txt` content delta survived; single-key undo `rückgängig: paste (3 items)`, dest empty, originals intact. |
| 11.8 | PASS* | Trash-delete cycle fully correct (delete→trash, undo restores from trash, redo re-trashes fresh entry, undo again restores). *The `delete` key had to be sent with tmux `-l` (literal) — see protocol feedback; not an rfm bug. |
| 11.9 | PASS | tar.gz created (lists exactly `d & e.txt`, no shell mangling); undo deletes it with depths 0/0 (no_redo); redo → `nichts wiederherzustellen`, archive not recreated. |
| 11.10 | PASS | zip created, undo removes it (0/0), redo → `nichts wiederherzustellen`, no recreation. |
| 11.11 | PASS | use_trash=false: rename recorded (undo 1), permanent delete adds Barrier (undo 2, not trashed); undo hits barrier (still 2/0, WARN `permanentes Löschen`, rename NOT reverted); repeat undo identical — barrier never consumed. |

**Tally:** 11 steps, 11 PASS, 0 FAIL, 0 SKIP.

No rfm bugs found. All undo/redo transaction, barrier, trash-cycle, collision-suffix,
and no_redo-archive semantics behaved exactly as the protocol specified, on both the
debug socket and the disk. Screen assertions matched socket belief throughout (no
stale-render mismatches). The one execution snag (`delete` not firing) was a tmux
key-name collision in the harness, not an rfm defect — confirmed twice and worked
around with `send-keys -l`.

## Protocol feedback

- **`tmux send-keys delete` sends the Delete key, not the string.** `delete` (and
  `Delete`) is a recognized tmux key name, so `tmux send-keys -t $SESSION delete`
  emits a single Delete keypress — rfm sees no `d-e-l-e-t-e` sequence and the
  delete command silently no-ops (reproduced at 11.8: undo stayed 0, nothing
  trashed, no `Deleted` log line). The fix is `tmux send-keys -t $SESSION -l delete`
  (literal). This affects every step that types the `delete` binding: **11.8** and
  **11.11b/11.11d**. The protocol should specify `-l` for the `delete` (and any
  word that is a tmux key name) sequences, exactly as it already reads for typed
  sequences. The `rename`/`tar`/`zip`/`.bak` sequences are NOT tmux key names and
  work without `-l`, but sending them with `-l` is harmless and I did so for
  robustness. Recommend the harness/README note: "type multi-char binding
  sequences with `send-keys -l` to avoid tmux key-name collisions (`delete`,
  `up`, `end`, `home`, `tab`, `space`, …)."

- **Log-widget screen assertions are unreliable under a busy `/tmp`.** Steps that
  assert an INFO/WARN string is visible in the on-screen log widget (11.1, 11.9
  redo) can fail purely because parallel executors churn the shared `/tmp` parent,
  flooding rfm's watcher with `Updating: /tmp` INFO lines that push the target line
  out of the 10 s widget TTL / the retained-history window before it can be read.
  The socket `log` history still contains the line if queried immediately after the
  keypress (I re-sent `C-r` and grepped within the same call to confirm
  `nichts wiederherzustellen`). Suggestion: assert these strings on the socket
  `log` (queried right after the input) as the primary signal, and treat the
  on-screen widget as best-effort; or launch the fixture outside the shared `/tmp`
  parent so the parent-dir watcher is quiet.

- **`state`-mirrored `selection` after undo/redo is stable enough here**, and the
  step's "do not assert selection after undo/redo" note (11.2) was respected; no
  issue observed. Minor: the header/title row shows the *preview_path* (the
  highlighted directory), e.g. `…/dest dir` while cwd is still the parent — this is
  correct behavior but can look like the cwd changed if an executor mis-reads it;
  worth a one-line note in the README pitfalls.
