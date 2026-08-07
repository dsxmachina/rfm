# Section 10 — Shell commands and escaping

Harness per README (N=10, SESSION=rfm-sec10, SOCK=/tmp/rfm-sec10.sock).

Scope: user-defined background/interactive commands (`[commands.*]`), the
command queue (`queue_active`/`queue_len`), failure reporting in the socket
`log`, shell-escaping of hostile file names (`$@` interpolation and the
`zoxide add` hook), and the file-watcher picking up changes made from OUTSIDE
rfm.

**Optional tool:** `zoxide`. Before starting, record availability:

```bash
HAVE_ZOXIDE=0; command -v zoxide >/dev/null && HAVE_ZOXIDE=1
```

Steps whose expectations differ by `HAVE_ZOXIDE` say so explicitly; nothing in
this section is skipped entirely without zoxide.

**Section fixture** (create BEFORE launching rfm — note the single quotes; two
names contain `$()` command substitutions that must NEVER execute):

```bash
mkdir "$FIXTURE/a directory with spaces"
mkdir "$FIXTURE/Bilder & Videos"
mkdir "$FIXTURE/"'zz $(touch INJECTED-zox) dir'
printf 'inner\n' > "$FIXTURE/a directory with spaces/inner.txt"
printf 'da\n'    > "$FIXTURE/Bilder & Videos/vorhanden.txt"
printf 'z\n'     > "$FIXTURE/"'zz $(touch INJECTED-zox) dir'/inner2.txt
printf 'x\n'     > "$FIXTURE/plain.txt"
touch "$FIXTURE/"'$(touch INJECTED).txt'
```

**Section config** (write BEFORE launching rfm; `--config $CFG` takes the
*directory*). The section names (`marker`, `failer`, `lister`, `inter`) are the
display names that appear verbatim in log lines and `queue_active`. The keys
`o`, `x`, `b`, `v` collide with no default binding (verified against
`examples/default-config.toml`), so no upgrade-notice overlay appears at
startup:

```bash
cat > "$CFG/config.toml" <<'EOF'
[commands.marker]
keys = ["o"]
cmd = "sleep 2; touch marker-ran.txt"

[commands.failer]
keys = ["x"]
cmd = "echo boom-stdout; echo boom-stderr >&2; exit 7"

[commands.lister]
keys = ["b"]
cmd = 'printf "%s\n" $@ > cmd-args.txt'

[commands.inter]
keys = ["v"]
cmd = "touch interactive-ran.txt"
interactive = true
EOF
```

Initial sorted visible order in `$FIXTURE` (dirs first, then lowercase-name
order; `$` = 0x24 sorts before letters): `a directory with spaces`,
`Bilder & Videos`, `zz $(touch INJECTED-zox) dir`, `$(touch INJECTED).txt`,
`plain.txt` — five entries, initial selection `a directory with spaces`
(`selected_idx == 0`). Later steps add files (`marker-ran.txt`,
`cmd-args.txt`, `interactive-ran.txt`, `watched-new.txt`); each step states
the layout it expects. No hidden files exist, so visible order == `entries`
reply order throughout this section.

**Launch and log assertions.** Launch per README — the binary is the tmux
session command directly (`tmux new-session -d … "env … ./target/debug/rfm …"`),
NOT typed into an interactive shell: a zsh history-search widget (Atuin)
intercepts the typed command and rfm never starts. The fixture lives under the
quiet README parent so the left panel does not watch a churning `/tmp`. For the
command-log assertions (10.2/10.4/10.5/10.7/10.14) query the socket `log` with a
**wide window (`log 200`) IMMEDIATELY after each action** and grep in the same
call — the TRACE stream evicts command INFO/WARN/ERROR lines from short windows
within seconds. Treat the 10 s on-screen log widget as timing-fragile (sample
2–3×). External file ops for 10.8/10.12 ("from the harness shell, NOT inside
rfm") run from the Bash tool directly against `$FIXTURE` since the pane IS rfm.

Facts this section relies on (verified in source):

- User commands: `src/panel/manager.rs` `Command::UserCommand` arm.
  Background (`interactive = false`, the default): logs
  `Queueing command '<name>': <expanded cmd>` (info) and sends to the
  executor. Interactive: logs
  `Running interactive command '<name>': <expanded cmd>` and runs `sh -c`
  **blocking on the event-loop thread** (no terminal suspend is implemented
  yet), then `Command '<name>' completed successfully` /
  `Command '<name>' failed with exit code <N>`. Both variants call
  `unmark_all_items()` after dispatch.
- `$@` expansion (`expand_command`, manager.rs): every path is passed through
  `shell_escape::escape` and the results are joined with `separator`
  (default `" "`). The expanded string is what `sh -c` receives.
- With nothing marked, `marked_or_selected` auto-marks the current selection
  and uses it; commands whose template has no `$@` still go through this
  (invisible, since unmark follows).
- Executor (`src/command_queue/executor.rs`): runs one command at a time via
  `sh -c` with `current_dir = ` the center panel's path at queue time. Logs:
  `Executing command '<name>': <cmd>` (info), first 10 stdout lines as
  `[<name>] <line>` (debug — retained in socket log because `--debug-socket`
  raises verbosity to TRACE), first 10 stderr lines as `[<name>] <line>`
  (warn), then `Command '<name>' completed successfully` (info) or
  `Command '<name>' failed with exit code <N>` (error).
- Queue status semantics: `queue_len` counts commands in the executor's
  *internal* queue, NOT the mpsc channel. The executor drains the channel
  into its queue only *after* finishing a command — so with 2 rapid sends,
  `queue_len` never shows 1; it takes **3** sends to observe `queue_len == 1`
  (during execution of the 2nd). `queue_active` is the running command's name.
- zoxide hook (`src/command_queue/mod.rs`): every directory *descent* or jump
  (not `h`/parent) queues `zoxide add <shell-escaped canonical path>` under
  the name `zoxide add current-dir` — but only when zoxide is on PATH; when
  absent, a one-time debug line
  `zoxide not found in PATH - visited directories will not be recorded` is
  logged instead and nothing is queued.
- Watchers (`src/panel/mod.rs`, notify crate): each visible pane's directory
  is watched non-recursively; external create/delete lands as an async panel
  update. TRACE lines `watching <path>` / `unwatching <path>` and
  `panel-update:` lines appear in the socket log. `await-idle` does NOT
  guarantee the notify event has arrived — poll `entries` (bounded) instead.
- Which notify events reload which pane is decided by the pure
  `reload_on_event(kind, reload_on_modify)` (`src/panel/mod.rs`, unit-tested):
  `Create` / `Remove` **and rename** reload every pane; plain content/metadata
  writes reload only the preview. The rename case is the subtle one — a file
  *moved* into or out of a directory (or renamed in place) does NOT arrive as
  `Create`/`Remove`; inotify reports it as `MOVED_TO`/`MOVED_FROM`, which the
  notify crate maps to `Modify(ModifyKind::Name(_))`. Historically the listing
  panes ignored all `Modify` events, so a moved-away file lingered as a phantom
  entry while its preview went empty (fixed by treating `Modify(Name)` as
  structural). Step 10.8b guards this; 10.8/10.12 (touch/rm) only cover the
  `Create`/`Remove` kinds.

**Section helper — poll a socket condition** (never bare-sleep-and-hope):

```bash
# poll "<shell command that exits 0 when the condition holds>" [tries=50]
poll() {
  for i in $(seq 1 "${2:-50}"); do sh -c "$1" >/dev/null 2>&1 && return 0; sleep 0.2; done
  return 1
}
# examples:
#   poll "echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'marker-ran.txt'"
#   poll "echo state | socat - UNIX-CONNECT:$SOCK | jq -e '.queue_active == null'"
```

---

### 10.1 — Launch baseline: hostile names listed literally, no overlay

**Action:** launch per README (fixture + config above). Then
`echo await-idle | socat - UNIX-CONNECT:$SOCK`, `echo state | socat ...`,
`echo "entries center" | socat ...`, `tmux capture-pane -t $SESSION -p`.
**Expect (socket):** `mode == "normal"` (no upgrade-notice overlay — the four
user-command keys collide with no default). `cwd == $FIXTURE`,
`selection == "a directory with spaces"`, `selected_idx == 0`, `total == 5`,
`queue_active == null`, `queue_len == 0`, `marked == []`. `entries center`
lists exactly the five names in the order given above, `selected:true` only on
`a directory with spaces`.
**Expect (screen):** center column shows all five names **literally**:
`a directory with spaces`, `Bilder & Videos` (ampersand rendered once),
`zz $(touch INJECTED-zox) dir`, `$(touch INJECTED).txt`, `plain.txt`.
Top row shows the fixture path. Selected-row highlight on
`a directory with spaces`.
**Note:** if `mode == "decision-flow"` here, a user-command key unexpectedly
collided with a default — that is protocol feedback (key choice), not an rfm
bug; press Escape (all items have defaults → resolves) and note it.

### 10.2 — Background command: queue_active while running, log trail, file created

**Action:** `tmux send-keys -t $SESSION o` then
`echo await-idle | socat - UNIX-CONNECT:$SOCK`. Immediately query `state`
(the command sleeps 2 s — do not dawdle). Then
`poll "echo state | socat - UNIX-CONNECT:$SOCK | jq -e '.queue_active == null'"`
and query `log 30`.
**Expect (socket):** first `state`: `queue_active == "marker"` (if you were
too slow and it is already `null`, rerun the step and query faster — note as
protocol feedback, not a bug). After the poll: `queue_active == null`,
`queue_len == 0`. `log 30` contains, in order:
`Queueing command 'marker': sleep 2; touch marker-ran.txt` (INFO),
`Executing command 'marker': sleep 2; touch marker-ran.txt` (INFO),
`Command 'marker' completed successfully` (INFO). `mode` stayed `"normal"`
throughout; a `j`/`k` sent during the sleep would be processed immediately
(background = non-blocking).
**Expect (screen):** log widget (bottom area) shows the
`Queueing command 'marker'...` / `Command 'marker' completed successfully`
lines while they are <10 s old. No overlay, panels unchanged.
**Expect (disk):** `[ -f "$FIXTURE/marker-ran.txt" ]` — the executor's
working dir is the center panel's path.

### 10.3 — Watcher picks up the command's output file

**Action:** no keys.
`poll "echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'marker-ran.txt'"`
then one `await-idle`, then `entries center` + `capture-pane`.
**Expect (socket):** `entries center` now has 6 entries; `marker-ran.txt`
appears among the files (visible order now:
`... dirs ..., $(touch INJECTED).txt, marker-ran.txt, plain.txt`). Selection
unchanged (`a directory with spaces` still `selected:true`). `log` contains a
TRACE `panel-update:` line younger than the step start.
**Expect (screen):** `marker-ran.txt` visible in the center column. Belief
and screen must agree — `entries` listing it while the pane does not is a
stale-render bug.

### 10.4 — queue_len: three rapid sends expose the internal queue

**Action:** `tmux send-keys -t $SESSION o o o` then `await-idle`. Then poll
`state` every ~0.3 s for up to 10 s, recording `queue_active`/`queue_len`
pairs, until `queue_active == null` sticks.
**Expect (socket):** `queue_active == "marker"` for ~6 s total; **at least one
sample shows `queue_len == 1`** (during the 2nd execution, after the executor
drained the channel). Final state: `queue_active == null`, `queue_len == 0`.
`log 40` contains three `Executing command 'marker': ...` lines and three
`Command 'marker' completed successfully` lines.
**Expect (screen):** log widget shows the most recent of those lines; UI
otherwise unchanged and responsive.
**Note:** two sends can NEVER show `queue_len == 1` (the channel is drained
into the counted queue only between executions) — that is why this step sends
three. If no sample shows `queue_len == 1`, retry once with faster polling
before filing a bug.

### 10.5 — Failing command: exit code + stderr in log, UI stays responsive

**Action:** `tmux send-keys -t $SESSION x`, `await-idle`, then
`poll "echo 'log 40' | socat - UNIX-CONNECT:$SOCK | grep -qF \"failed with exit code 7\""`.
Then `tmux send-keys -t $SESSION j`, `await-idle`, `state`, `capture-pane`.
Then `tmux send-keys -t $SESSION g g` (return cursor to top), `await-idle`.
**Expect (socket):** `log 40` contains all of:
`Queueing command 'failer': echo boom-stdout; echo boom-stderr >&2; exit 7`
(INFO), `Executing command 'failer': ...` (INFO), `[failer] boom-stdout`
(DEBUG), `[failer] boom-stderr` (WARN),
`Command 'failer' failed with exit code 7` (ERROR). After the `j`:
`selection` advanced to the next visible entry (event loop alive, nothing
wedged); after `gg`: `selected_idx == 0`. `queue_active == null` at the end.
**Expect (screen):** while <10 s old, the log widget shows the
`Command 'failer' failed with exit code 7` line (and the `boom-stderr` warn
line); panels render normally — a failing background command must not disturb
the layout or leave artifacts.
**Note:** this is the "silent failure" drill: nothing about the failure is
visible in the panels themselves; the socket `log` is the source of truth.

### 10.6 — `$@` interpolation with hostile names: no word-split, no execution

**Setup:** current visible layout (6 entries):
idx 0 `a directory with spaces`, 1 `Bilder & Videos`,
2 `zz $(touch INJECTED-zox) dir`, 3 `$(touch INJECTED).txt`,
4 `marker-ran.txt`, 5 `plain.txt`. Cursor at idx 0 (from 10.5's `gg`).
**Action:** `tmux send-keys -t $SESSION j j j`, `await-idle`; confirm via
`state` that `selection == '$(touch INJECTED).txt'` (abort and re-locate via
`entries center` if not). Then `tmux send-keys -t $SESSION Space` (marks,
auto-advances to idx 4), `tmux send-keys -t $SESSION j` (to idx 5),
`tmux send-keys -t $SESSION Space` (marks `plain.txt`; cursor clamps at
bottom), `await-idle`; confirm `state.marked` has exactly
`$FIXTURE/$(touch INJECTED).txt` and `$FIXTURE/plain.txt` (absolute paths).
Then `tmux send-keys -t $SESSION b`, `await-idle`, then
`poll "echo 'log 30' | socat - UNIX-CONNECT:$SOCK | grep -qF \"Command 'lister' completed successfully\""`.
**Expect (socket):** `log 30` contains
`Executing command 'lister': printf "%s\n" '...$(touch INJECTED).txt' ... > cmd-args.txt`
— the hostile path appears **single-quoted** in the expanded command line.
`state.marked == []` (user commands unmark after dispatch).
`Command 'lister' completed successfully` present; NO `failed with exit code`
line for `lister`.
**Expect (screen):** neither marked-row `x` prefix remains (marks cleared);
after the watcher catches up (poll `entries`), `cmd-args.txt` appears in the
center column.
**Expect (disk):**
- `cat "$FIXTURE/cmd-args.txt"` has **exactly 2 lines**: the absolute paths
  `$FIXTURE/$(touch INJECTED).txt` and `$FIXTURE/plain.txt`, each intact on
  one line (no splitting at the space, no quotes in the file content).
- `[ ! -e "$FIXTURE/INJECTED" ]` — the `$(touch INJECTED)` in the file name
  did NOT execute. If `INJECTED` exists, this is a **critical shell-injection
  bug**; capture `log 50` (the expanded command line) immediately.

### 10.7 — Interactive command: blocking foreground run

**Action:** `tmux send-keys -t $SESSION v`, then
`echo await-idle | socat - UNIX-CONNECT:$SOCK` (this returns only after the
interactive command finished — it blocks the event loop), then `state`,
`log 20`, `capture-pane`.
**Expect (socket):** `log 20` contains
`Running interactive command 'inter': touch interactive-ran.txt` (INFO) and
`Command 'inter' completed successfully` (INFO). NO `Queueing`/`Executing`
lines for `inter` (it never touches the queue); `queue_active == null` the
whole time. `mode == "normal"`.
**Expect (screen):** UI intact after the run (the command produced no
terminal output; rfm repaints normally). After polling `entries`,
`interactive-ran.txt` appears in the center column (watcher).
**Expect (disk):** `[ -f "$FIXTURE/interactive-ran.txt" ]`.
**Note:** interactive commands run `sh -c` blocking on the event-loop thread
(terminal suspend is a TODO in source). A long-running interactive command
would freeze the UI and time out `state` — that is expected behavior today,
not a wedge; do not test it here.

### 10.8 — Watcher: external create/delete from plain bash

**Action:** from the harness shell (NOT inside rfm):
`touch "$FIXTURE/watched-new.txt"`. Then
`poll "echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'watched-new.txt'"`,
one `await-idle`, `capture-pane`. Then `rm "$FIXTURE/watched-new.txt"` and
`poll "! (echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'watched-new.txt')"`,
`await-idle`, `capture-pane`.
**Expect (socket):** `entries center` gains `watched-new.txt` within the poll
window (≤10 s; typically <1 s), then loses it after the `rm`. Selection is
preserved across both updates (same `selection` name before/after; its
`selected_idx` may shift while the extra file exists). `log` shows fresh
TRACE `panel-update:` lines for both events.
**Expect (screen):** `watched-new.txt` appears in the center column, then
disappears. Screen and `entries` must agree at both checkpoints — `entries`
updated but screen stale is a render-path bug; both stale is a
watcher/DirManager bug (note which, per the README diagnosis rule).

### 10.8b — Watcher: external MOVE (rename) in and out — the `Modify(Name)` kind

Companion to 10.8. A **move** is not a create/delete: inotify reports it as
`MOVED_TO` / `MOVED_FROM`, which notify maps to `Modify(Name(_))`, a different
event kind than 10.8's `Create`/`Remove`. This step exists because that kind
was once dropped by the listing panes — a file moved away lingered as a phantom
entry while its preview went empty (the exact real-world report that motivated
the `reload_on_event` fix). 10.8 passing does NOT imply this passes.

**Critical harness requirement — same filesystem.** `mv` is a rename *only*
within one filesystem; across a filesystem boundary `mv` degrades to
copy-then-unlink, which emits `Create`+`Remove` and would make this step pass
**even against the bug**, defeating its purpose. Stage the file on the same fs
as `$FIXTURE` (both under `/tmp` via `mktemp` here) and **not** inside `$FIXTURE`
or `$PARENT` (the left pane watches `$PARENT` — staging there would pollute it):

```bash
STAGE=$(mktemp -d)                     # same fs as $FIXTURE (both /tmp), unwatched
printf 'moved\n' > "$STAGE/moved-in.txt"
```

**Action:** record the pre-move selection first
(`SEL=$(echo state | timeout 12 socat - UNIX-CONNECT:$SOCK | jq -r .selection)`).
Then move the staged file **into** the watched dir (a rename, not a copy):
`mv "$STAGE/moved-in.txt" "$FIXTURE/moved-in.txt"`. Poll for arrival:
`poll "echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'moved-in.txt'"`,
one `await-idle`, `capture-pane`. Then move it back **out**:
`mv "$FIXTURE/moved-in.txt" "$STAGE/moved-in.txt"`, poll for disappearance
`poll "! (echo 'entries center' | socat - UNIX-CONNECT:$SOCK | grep -qF 'moved-in.txt')"`,
`await-idle`, `state`, `capture-pane`.
**Expect (socket):** `entries center` gains `moved-in.txt` within the poll
window (≤10 s; typically <1 s) — **this is the assertion that fails against the
bug** (the move-in is a `Modify(Name(To))` the listing would have ignored, so
the file never appears) — then loses it after the move-out. Selection preserved
across both events (`state.selection == $SEL` before and after; its
`selected_idx` may shift while the extra file exists). `log` shows fresh TRACE
`panel-update:` lines for both events, and NO stale-listing mismatch.
**Expect (screen):** `moved-in.txt` appears in the center column, then
disappears; the pre-move selection stays highlighted. Belief and screen must
agree at both checkpoints (both stale = the watcher/`reload_on_event`
regression; `entries` fresh but screen stale = render path).
**Note (in-place rename).** The same `Modify(Name)` kind also fires for a rename
*within* the dir (`mv "$FIXTURE/plain.txt" "$FIXTURE/plain2.txt"`): the old name
must vanish and the new appear in one update. Not asserted as a separate step to
keep the fixture net-zero for 10.14, but it is the same code path — spot-check it
manually if `reload_on_event` is ever touched.
**Teardown of this step:** `rm -rf "$STAGE"` (the file was moved back into it).
Fixture is net-zero — `moved-in.txt` is gone, so 10.14's integrity sweep is
unaffected.

### 10.9 — Descend into "a directory with spaces" (+ zoxide add escaping)

**Action:** `tmux send-keys -t $SESSION g g`, `await-idle` (selection back to
`a directory with spaces`, `selected_idx == 0`); then
`tmux send-keys -t $SESSION l`, `await-idle`, `state`, `entries center`,
`capture-pane`. If `HAVE_ZOXIDE=1`:
`poll "echo 'log 30' | socat - UNIX-CONNECT:$SOCK | grep -qF \"Command 'zoxide add current-dir' completed successfully\""`.
**Expect (socket):** `cwd` ends with `/a directory with spaces`;
`selection == "inner.txt"`; `left_path == $FIXTURE`; `total == 1`.
If `HAVE_ZOXIDE=1`: `log 30` contains
`Executing command 'zoxide add current-dir': zoxide add '<canonical fixture path>/a directory with spaces'`
— the path **single-quoted** (shell_escape) — followed by
`Command 'zoxide add current-dir' completed successfully`; no `failed` line
for it. If `HAVE_ZOXIDE=0`: `log 50` contains the DEBUG line
`zoxide not found in PATH - visited directories will not be recorded`
(logged once per session, at the first descent — which is this one) and no
`zoxide add` execution lines.
**Expect (screen):** top row shows the path ending in
`a directory with spaces` (spaces intact); center column shows `inner.txt`;
left column shows the fixture listing with `a directory with spaces`
highlighted as the originating entry.

### 10.10 — zoxide database holds the exact path (skip body if HAVE_ZOXIDE=0)

**Action:** from the harness shell:
`_ZO_DATA_DIR=$ZO zoxide query -l` (`$ZO` is the README harness variable
exported into the pane before launch — same value, so this reads the same
isolated database).
**Expect (disk/db):** the output contains exactly one line equal to
`$(realpath "$FIXTURE")/a directory with spaces` — one entry, unsplit at the
spaces. A database entry of just `$(realpath "$FIXTURE")/a` (or several
fragment entries) = word-splitting escaped into zoxide — critical bug.
**Expect (socket):** `state` unchanged by this step (`seq` stable — debug
queries and outside shell commands advance nothing).
**Note:** if `HAVE_ZOXIDE=0`, record the step as "skipped: zoxide not
installed" and move on.

### 10.11 — Descend into "Bilder & Videos"

**Action:** `tmux send-keys -t $SESSION h`, `await-idle` (back at root,
selection restored to `a directory with spaces`); `tmux send-keys -t $SESSION j`,
`await-idle` (`selection == "Bilder & Videos"`); `tmux send-keys -t $SESSION l`,
`await-idle`, `state`, `entries center`, `capture-pane`. If `HAVE_ZOXIDE=1`,
poll for the second `zoxide add current-dir` completion as in 10.9.
**Expect (socket):** `cwd` ends with `/Bilder & Videos` (ampersand intact in
the JSON string); `selection == "vorhanden.txt"`; `total == 1`. If
`HAVE_ZOXIDE=1`: newest `Executing command 'zoxide add current-dir':` line
contains `'.../Bilder & Videos'` single-quoted, then
`completed successfully` — an unescaped `&` would background half the command
and typically still exit 0, so check the *quoting in the log line itself*,
and (if zoxide present) that `_ZO_DATA_DIR=$ZO zoxide query -l` now also
contains the full `.../Bilder & Videos` path as one line.
**Expect (screen):** top row shows the path ending in `Bilder & Videos`
rendered literally (exactly one `&`); center shows `vorhanden.txt`.

### 10.12 — Watcher inside an `&`-named directory

**Action:** from the harness shell:
`touch "$FIXTURE/Bilder & Videos/neu & datei.txt"`. Poll `entries center`
for `neu & datei.txt` (as in 10.8), `await-idle`, `capture-pane`. Then
`rm "$FIXTURE/Bilder & Videos/neu & datei.txt"`, poll for disappearance,
`await-idle`, `capture-pane`.
**Expect (socket):** `entries center` gains then loses `neu & datei.txt`
(name intact with spaces and `&`); `total` goes 1 → 2 → 1;
`selection == "vorhanden.txt"` preserved throughout.
**Expect (screen):** `neu & datei.txt` appears literally in the center
column, then disappears; `vorhanden.txt` stays highlighted.
**Note:** this proves the notify watch was registered on the `&`-named path
itself — a TRACE `watching .../Bilder & Videos` line exists in `log` from
step 10.11.

### 10.13 — Descend into the `$(...)`-named directory: no injection anywhere

**Action:** `tmux send-keys -t $SESSION h`, `await-idle` (root, selection
restored to `Bilder & Videos`); `tmux send-keys -t $SESSION j`, `await-idle`
(`selection == "zz $(touch INJECTED-zox) dir"`);
`tmux send-keys -t $SESSION l`, `await-idle`. If `HAVE_ZOXIDE=1`, poll for
the `zoxide add current-dir` completion line. Then from the harness shell:
`find "$FIXTURE" -name 'INJECTED*'`.
**Expect (socket):** `cwd` ends with `/zz $(touch INJECTED-zox) dir`
(literal); `selection == "inner2.txt"`. If `HAVE_ZOXIDE=1`: the newest
`Executing command 'zoxide add current-dir':` line shows the path
single-quoted with the `$(...)` intact inside the quotes, and it
`completed successfully`.
**Expect (disk):** `find` prints **nothing** — no `INJECTED`, no
`INJECTED-zox` anywhere under `$FIXTURE`. Also check rfm's process cwd side:
`[ ! -e "$FIXTURE/INJECTED-zox" ] && [ ! -e ./INJECTED-zox ]` (the executor
runs `zoxide add` with working dir `"."`; step 10.7 moved the process cwd to
`$FIXTURE`, but check the repo root too in case earlier steps ran first).
Any `INJECTED*` file existing = command substitution executed inside
`sh -c` = critical escaping bug.
**Expect (screen):** top row shows the literal directory name incl.
`$(touch INJECTED-zox)`; center shows `inner2.txt`.

### 10.14 — Return to root: final integrity sweep

**Action:** `tmux send-keys -t $SESSION h`, `await-idle`, `state`,
`entries center`, `log 50`, `capture-pane`. From the harness shell:
`ls -A "$FIXTURE"`.
**Expect (socket):** `cwd == $FIXTURE`; `selection ==
"zz $(touch INJECTED-zox) dir"` (restored from history);
`queue_active == null`, `queue_len == 0`; `marked == []`;
`mode == "normal"`. `entries center` names == exactly the `ls -A` output
(expected: the 3 dirs + `$(touch INJECTED).txt`, `cmd-args.txt`,
`interactive-ran.txt`, `marker-ran.txt`, `plain.txt` — 8 entries; no
`INJECTED*`, no `watched-new.txt`, no stray `sh` artifacts). `log 50`
contains no ERROR lines other than the intentional
`Command 'failer' failed with exit code 7` from 10.5 (a `zoxide add` failure
line, for instance, would be a finding).
**Expect (screen):** center column matches the entries reply; highlight on
`zz $(touch INJECTED-zox) dir`; no leftover overlay, no garbled cells from
the hostile names.

---

**Teardown:** per README (`tmux kill-session -t $SESSION`; `rm -rf "$FIXTURE"
"$CFG" "$CACHE" "$STATE" "$ZO"; rm -f $SOCK`). Also remove `./INJECTED-zox` /
`./error.log` from the repo root if (and only if) a failed step created them —
mention it in the run report.

**Section coverage gaps** (deliberate):
- Custom `separator` in `[commands.*]` (e.g. NUL-joined `$@` for `xargs -0`)
  — only the default `" "` separator is exercised.
- Long-running/hanging *interactive* commands (they block the event loop by
  design today; testing that would only prove the TODO in source).
- Spawn failure of the background shell itself
  (`Command '<name>' failed to execute: ...` — requires breaking `sh`, not
  reasonably testable).
- stdout truncation (`... (N more lines)` after 10 lines) and stderr
  truncation at 10 lines.
- User-command keybinding *collisions* with defaults (upgrade-notice flow) —
  covered by the configuration section, not here.
- `zoxide query` console mode (`CD`) — covered by the navigation section;
  here only the `zoxide add` hook is in scope.
- Backtick and newline characters in file names (only space, `&`, `$()`
  tested).
- Watcher behavior for the left pane / non-focused tabs, and watcher event
  storms (rapid create/delete loops). Rename detection (`Modify(Name)`) is now
  covered for the **center** pane in 10.8b; the same event reaching the **left**
  pane (move a sibling of the cwd) and a **non-focused tab** is still untested.
- `error.log` post-mortem content after the intentional `failer` error
  (would require quitting rfm mid-section).
