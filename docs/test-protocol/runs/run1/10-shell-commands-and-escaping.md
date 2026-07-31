# Section 10 — Shell commands and escaping — run1

**Environment:** terminal 120x30; tools present: tmux, socat, jq, zoxide (HAVE_ZOXIDE=1);
image_protocol resolved to `half-block` (inside tmux, expected). rfm built at
`./target/debug/rfm`. Binary launched as the tmux session command directly
(`tmux new-session -d ... env XDG_... ./target/debug/rfm ...`) instead of typing
into an interactive shell, because the login zsh in the pane has Atuin history
search bound and it intercepted the `send-keys`-typed launch command (see
protocol feedback). External-shell file operations for steps 10.8/10.12 were run
from the Bash tool directly against `$FIXTURE` (equivalent to "the harness
shell", since the pane is now rfm itself).

**Result: 14 steps, 14 PASS, 0 FAIL, 0 SKIP. No rfm bugs found.** All shell-escaping
and injection assertions held: no `INJECTED` / `INJECTED-zox` file was ever created,
`$@` expansion single-quoted hostile paths, zoxide DB stored space/`&`/`$()` paths
as single unsplit lines.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 10.1 | PASS | mode normal (no overlay), 5 entries in given order, sel idx 0, names literal on screen (single `&`, literal `$()`). |
| 10.2 | PASS | queue_active=="marker" captured immediately; Queueing/Executing/completed (INFO) in socket log; marker-ran.txt created on disk. |
| 10.3 | PASS | watcher added marker-ran.txt (6 entries, correct position), selection unchanged, visible on screen; panel-update present. |
| 10.4 | PASS | three `o` sends; samples 8-13 showed queue_len==1; drained to null/0; 3 Executing + 3 completed lines. |
| 10.5 | PASS | failer: Queueing/Executing(INFO), boom-stdout(DEBUG), boom-stderr(WARN), failed exit 7(ERROR); `j` advanced sel, `gg` -> idx 0; UI responsive. |
| 10.6 | PASS | lister log shows hostile path single-quoted; marked cleared; cmd-args.txt has exactly 2 intact lines; NO INJECTED file. |
| 10.7 | PASS | "Running interactive command 'inter'" + completed (INFO), no Queueing/Executing, queue null, mode normal; interactive-ran.txt created; UI intact. |
| 10.8 | PASS | external touch/rm of watched-new.txt appeared then disappeared, selection (plain.txt) preserved, screen agreed, panel-update lines present. |
| 10.9 | PASS | descend into "a directory with spaces": cwd/sel/left_path/total correct; zoxide add logged single-quoted, completed; screen path intact. |
| 10.10 | PASS | `zoxide query -l` returned exactly `.../a directory with spaces` (one unsplit line); seq stable (1426 before/after). |
| 10.11 | PASS | descend into "Bilder & Videos": cwd `&` intact; zoxide add single-quoted, completed; DB gained full `.../Bilder & Videos` line; one `&` on screen. |
| 10.12 | PASS | external create/delete of `neu & datei.txt` in `&`-dir: total 1->2->1, name intact, sel preserved; `watching .../Bilder & Videos` TRACE present. |
| 10.13 | PASS | descend into `zz $(touch INJECTED-zox) dir`: literal cwd/sel; zoxide add single-quoted with `$()` intact; `find INJECTED*` empty; no INJECTED-zox in fixture or repo root; completed. |
| 10.14 | PASS | back at root, sel restored to `zz $(touch INJECTED-zox) dir` from history; queue null/marked empty/mode normal; entries == `ls -A` (8 entries); no unexpected ERROR lines. |

## Bug reports

None. No FAILED steps.

## Notes on degraded-but-passing assertions

- **10.2 / 10.14 log-widget on-screen assertion (degraded, still PASS):** The
  left pane watches `/tmp`, which is extremely busy in this environment (many
  sibling temp dirs from other parallel executors). The `log` history is flooded
  with `INFO Updating: /tmp` / `panel-update: left <- /tmp` lines, so the 200-line
  retention window evicts command log lines within seconds, and the on-screen
  10s log widget is dominated by /tmp churn. The *command* log lines
  (Queueing/Executing/completed/failed) were all reliably found by grepping
  `log 200` right after each action, satisfying the socket assertion. The
  intentional `failer` ERROR line (10.5) had aged out of the 200-line window by
  10.14, so 10.14's "no ERROR other than failer" check saw zero ERROR lines —
  which trivially satisfies it. This is an environment/harness artifact, not an
  rfm defect. Recorded under protocol feedback.

## Protocol feedback

- **Scratchpad is shared across parallel section executors.** My initial env
  file (`scratchpad/env.sh`) was overwritten by a concurrently-running section
  09 executor between two Bash calls, silently swapping SESSION/SOCK/FIXTURE to
  section 09's values. Recommendation: the harness/orchestrator should give each
  executor an isolated scratch path, or the protocol should instruct executors
  to name persisted env files per-section (I switched to `env-sec10.sh`).
- **Interactive-shell launch collides with Atuin (or any zsh history-search
  widget).** The README harness types the launch command into an interactive
  shell via `send-keys ... Enter`; with Atuin bound in the login shell the typed
  text opened Atuin's search UI instead of running the command (socket never
  appeared). Launching the binary as the tmux session's command directly
  (`tmux new-session -d ... env VARS ./target/debug/rfm ...`) is robust and
  avoids the interactive shell entirely. Suggest the README recommend this form,
  or `bash --noprofile --norc` for the pane, to be Atuin/zsh-agnostic.
- **`tmux new-session` reported "duplicate session" spuriously on first attempt**
  yet still delivered send-keys to a live rfm at the right cwd — likely a leftover
  session from a prior run of this or another executor sharing the `rfm-sec10`
  name. Not an rfm issue; noted for harness hygiene (kill-session before
  new-session).
- **Log-retention vs a busy parent dir (see degraded note above).** When the
  parent of the fixture is `/tmp` and other executors churn it, the 200-line log
  history and the 10s on-screen log widget both get swamped, weakening the
  "screen shows the log line" assertions in 10.2/10.5/10.14. Consider placing the
  section fixture under its own quiet mktemp parent whose parent is also quiet,
  or asserting the log-widget line within a tighter window right after the action.
- Steps 10.8/10.12 say "from the harness shell (NOT inside rfm)". Since the pane
  is now rfm itself, these external ops were run from the Bash tool directly on
  `$FIXTURE`. Behaviorally identical (external process mutates the watched dir);
  noting for clarity.
