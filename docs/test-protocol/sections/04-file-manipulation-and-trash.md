# Section 04 — File manipulation and trash

Covers: rename flow (mode entry, live preview, Esc cancel, Enter commit, refuse-existing-target),
mkdir/touch (both are default bindings, verified in `examples/default-config.toml` `[keys.manipulation]`:
`rename = ["rename"]`, `mkdir = ["mkdir"]`, `touch = ["touch"]`, `delete = ["delete"]`),
delete-to-trash (`use_trash = true` default), the `gT` trash view (list, restore with `r`,
stays-open semantics, close with `q`/Esc), and a second launch with `use_trash = false`
(permanent delete + undo Barrier, `gT` disabled warning).

Uses the shared harness from README.md. N=04, SESSION=rfm-sec04, SOCK=/tmp/rfm-sec04.sock.

## Section fixture and extra isolation

**Extra isolation env (REQUIRED, beyond the README set):** export `XDG_DATA_HOME=$DATA`
(`DATA=$(mktemp -d)`) in the tmux pane BEFORE launching rfm. The `trash` crate resolves the
freedesktop home trash from `$XDG_DATA_HOME/Trash`; because `$DATA` and `$FIXTURE` both live
under `$TMPDIR` (same filesystem), trashed fixture files land in `$DATA/Trash/files/` — never
in the user's real trash.

```bash
PARENT=$(mktemp -d); FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"   # quiet parent (README)
DATA=$(mktemp -d); CFG=$(mktemp -d)
mkdir "$FIXTURE/sub dir & stuff"
touch "$FIXTURE/sub dir & stuff/inner.txt"
touch "$FIXTURE/alpha.txt" "$FIXTURE/bravo.txt" "$FIXTURE/amp & spaced.txt"
```

Launch per README, with `XDG_DATA_HOME=$DATA` exported in the pane alongside the standard
isolation vars, then `--debug-socket $SOCK --config $CFG $FIXTURE`.

**Fixture pitfalls baked into expectations below:**
- The directory `sub dir & stuff` sorts before all files, so the **initial selection is
  `sub dir & stuff`**, not `alpha.txt`.
- Two fixture names contain spaces and `&` on purpose (shell-interpolation coverage for
  trash/restore paths).
- Navigation instruction used throughout: "press `j` until `state.selection == "<name>"`"
  — poll `state` after each `j` (max 6 presses); do not assume a fixed index, entry order
  changes as files are created/renamed during the section.
- `trash::os_limited::list()` also scans topdir trashes (`.Trash-$UID`) on mounted volumes;
  on a machine with pre-existing topdir trash entries the `gT` list may contain foreign rows.
  All trash-list assertions below are therefore **membership** assertions; the two
  "empty trash" assertions (04.13, 04.14) note the skip condition.
- Avoid uppercase letters in typed names: the input widget normalizes case from the SHIFT
  modifier, and terminals do not report SHIFT uniformly for plain chars.
- Multi-char command sequences are typed with `tmux send-keys -l` (literal mode) so tmux
  never interprets them as key names (`delete` would otherwise be ambiguous). Special keys
  (`Enter`, `Escape`, `C-u`, `Space`) are sent WITHOUT `-l`, as separate send-keys calls.

Every step runs the full README verification loop (await-idle → state → capture-pane); the
Expect blocks list only the assertions specific to the step.

---

## Phase 1 — default config (use_trash = true)

### 04.1 — Launch baseline
**Action:** launch as above; `echo state | socat - UNIX-CONNECT:$SOCK`; `echo "entries center" | socat - UNIX-CONNECT:$SOCK`
**Expect (socket):** `mode == "normal"`, `cwd == $FIXTURE`, `total == 4`,
`selection == "sub dir & stuff"`, `selected_idx == 0`, `undo_depth == 0`, `redo_depth == 0`,
`clipboard == null`, `marked == []`. `entries center` names (set equality):
`sub dir & stuff`, `alpha.txt`, `amp & spaced.txt`, `bravo.txt`; exactly `sub dir & stuff`
has `selected:true`.
**Expect (screen):** center pane lists the 4 entries; `sub dir & stuff` row is highlighted;
header shows the `$FIXTURE` path.

### 04.2 — Enter rename mode (seeded input)
**Action:** navigate toward `alpha.txt` — press `j` or `k` **toward the target** (it may sort *above* the current selection after a prior rename/delete, so blind `j` can overshoot and wrap; see the SELECT helper in README — compute the target's visible index from `entries center` and step the shortest direction). Once `state.selection == "alpha.txt"`, then `tmux send-keys -t $SESSION -l rename`; await-idle.
**Expect (socket):** `mode == "rename"`.
**Expect (screen):** footer line shows the yellow prompt `Rename:` followed by the seeded
current name `alpha.txt` (RenameMode seeds the input with the file name, cursor at end).
**Note:** the trailing `e` completes the `rename` sequence; no intermediate key fires a
command (`r`, `re`, … are prefixes only of `rename`).

### 04.3 — Live rename preview
**Action:** `tmux send-keys -t $SESSION C-u` then `tmux send-keys -t $SESSION -l renamed`; await-idle.
**Expect (socket):** `mode == "rename"` still; `undo_depth == 0` (nothing applied yet).
**Expect (screen):** footer shows `Rename:` with input exactly `renamed.txt` — C-u with the
cursor at end is "bulk delete keeping the extension": `alpha.txt` → `.txt`, cursor to 0,
then the typed `renamed` prepends (Input::bulk_delete, src/panel/input.rs). The selected
row in the center pane shows the pending name `renamed.txt` (rename-preview injection, drawn
in the `rename` color, default blue).
**Note:** do NOT assert the `entries center` name for this row while the preview is pending;
the injected preview name vs. on-disk name in that reply is unspecified here.

### 04.4 — Esc cancels rename
**Action:** `tmux send-keys -t $SESSION Escape`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 0`; `entries center` still contains
`alpha.txt` and does NOT contain `renamed.txt`.
**Expect (screen):** center pane shows `alpha.txt` again (preview cleared); footer prompt gone.
Also assert on disk: `[ -e "$FIXTURE/alpha.txt" ] && [ ! -e "$FIXTURE/renamed.txt" ]`.

### 04.5 — Commit rename with Enter
**Action:** with selection still `alpha.txt` (re-verify via `state`, re-navigate if needed):
`tmux send-keys -t $SESSION -l rename`, then `C-u`, then `-l renamed`, then `Enter`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 1` (rename is undo-recorded);
`entries center` contains `renamed.txt`, not `alpha.txt`.
**Expect (screen):** center pane shows `renamed.txt`; no footer prompt.
Disk: `[ -e "$FIXTURE/renamed.txt" ] && [ ! -e "$FIXTURE/alpha.txt" ]`.
**Note:** do not assert which entry is selected after the reload.

### 04.6 — Rename refuses an existing target
**Action:** press `j`/`k` until `state.selection == "renamed.txt"`; `-l rename`, `C-u`,
`-l bravo`, `Enter` (attempts `renamed.txt` → `bravo.txt`); await-idle;
`echo "log 20" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `mode == "normal"` (apply_rename always leaves the mode);
`undo_depth == 1` unchanged; log contains a WARN line with substring
`Cannot rename: '` and `bravo.txt' already exists`.
**Expect (screen):** both `renamed.txt` and `bravo.txt` still listed; the warning is visible
in the log widget if captured within its 10s TTL (the socket `log` is authoritative).
Disk: both files still exist, unchanged.

### 04.7 — mkdir
**Action:** `tmux send-keys -t $SESSION -l mkdir`; await-idle; assert `mode == "mkdir"` and
the footer shows the prompt `Make Directory:` (main color). Then `-l "made dir"`, capture-pane
(the phantom new element `made dir` appears in the center pane while typing, highlight color),
then `Enter`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 2`; `entries center` contains
`made dir`.
**Expect (screen):** `made dir` listed among the directories (directories sort before files);
footer prompt gone.
Disk: `[ -d "$FIXTURE/made dir" ]`.
**Note:** the `m`→`k` prefix does NOT set jump-mark `k` — the parser defers the auto-generated
mark chord when a longer explicit binding (`mkdir`) shares the prefix (commands.rs,
`longer_binding_wins_over_mark_chord`). Assert `state.jump_marks` is empty.

### 04.8 — touch
**Action:** `tmux send-keys -t $SESSION -l touch`; await-idle; assert `mode == "touch"` and
footer prompt `Touch:` (grey). Then `-l "made file.txt"`, then `Enter`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 3`; `entries center` contains
`made file.txt`.
**Expect (screen):** `made file.txt` listed in the file section.
Disk: `[ -f "$FIXTURE/made file.txt" ]`.
**Note:** `t` and `to` are prefixes shared with `tar`/`touch`; only the full sequence fires.

### 04.9 — touch Esc cancels, phantom cleared
**Action:** `-l touch`; `-l ghost.txt` (phantom `ghost.txt` visible in the pane); `Escape`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 3` unchanged; `entries center` does
NOT contain `ghost.txt`.
**Expect (screen):** `ghost.txt` no longer anywhere in the pane (Cleanup::CreatePreview
removed the phantom).
Disk: `[ ! -e "$FIXTURE/ghost.txt" ]`.

### 04.10 — Delete a file to the trash
**Action:** navigate `j`/`k` **toward** `made file.txt` (shortest direction — see the README SELECT helper) until `state.selection == "made file.txt"`; `tmux send-keys -t $SESSION -l delete`; await-idle; `echo "log 20" | socat ...`.
**Expect (socket):** `mode == "normal"`; `clipboard == null` (the `d` prefix did NOT trigger
`dd`/cut — `d`,`de`,… defer until the sequence resolves); log contains INFO `Deleted 1 items`;
`undo_depth == 4` (trash delete is undo-recorded). Assert `made file.txt` is gone against the FULL `entries center` JSON (parse the array and confirm no member's `name == "made file.txt"`) — do NOT bare-grep the substring, which also appears in the `log` "Deleted 1 items" trail context.
**Expect (screen):** `made file.txt` gone from the center pane.
Disk: `[ ! -e "$FIXTURE/made file.txt" ]`; trash:
`ls "$DATA/Trash/files"` contains `made file.txt` (name may carry a suffix on collision —
substring match) and `ls "$DATA/Trash/info"` contains a matching `*.trashinfo`.
**Note:** if the trash-dir assert fails while the gT view (04.12) DOES show the item, the
file landed in a topdir trash (fixture on a different mount than `$DATA`) — record as an
environment note, not a product bug.

### 04.11 — Delete a name with spaces and `&`
**Action:** press `j`/`k` until `state.selection == "amp & spaced.txt"`; `-l delete`; await-idle.
**Expect (socket):** log INFO `Deleted 1 items`; `undo_depth == 5`; `entries center` no
longer contains `amp & spaced.txt`.
**Expect (screen):** entry gone.
Disk: `[ ! -e "$FIXTURE/amp & spaced.txt" ]`; `$DATA/Trash/files` listing contains
`amp & spaced.txt`.

### 04.12 — Open the trash view with gT
**Action:** `tmux send-keys -t $SESSION g T`; await-idle.
**Expect (socket):** `mode == "trash"`.
**Expect (screen):** centered overlay with the hint line
`Trash — j/k: move · r: restore · q/Esc: close`; list rows formatted
`YYYY-MM-DD HH:MM  <symbol> <name>  <original parent path>`; rows for `amp & spaced.txt`
and `made file.txt` are present with original-parent column showing the `$FIXTURE` path
(possibly left-truncated). Entries are sorted newest-first, so `amp & spaced.txt` (deleted
last) is the TOP row and carries the highlight bar (cursor starts at 0).
**Note:** membership asserts only — foreign topdir-trash rows may interleave on some hosts.

### 04.13 — Restore with r; view stays open, cursor slot held
**Action:** with the cursor on `amp & spaced.txt` (press `j`/`k` inside the view to reach it
if foreign rows displaced it — the highlight bar shows the cursor): `tmux send-keys -t $SESSION r`; await-idle; `echo "log 20" | socat ...`.
**Expect (socket):** `mode == "trash"` — the view STAYS open after a restore; log contains
INFO `wiederhergestellt: 1 Element(e) aus dem Papierkorb`.
**Expect (screen):** overlay still present; `amp & spaced.txt` row gone; the next row slid
up into the cursor slot and is highlighted (with only our two items: `made file.txt` is now
the highlighted top row).
Disk: `[ -f "$FIXTURE/amp & spaced.txt" ]` (restored to original path).

### 04.14 — Restore the last item; view stays open when empty
**Action:** cursor on `made file.txt`; press `r`; await-idle.
**Expect (socket):** `mode == "trash"` still.
**Expect (screen):** overlay still open; if the isolated trash is now truly empty (no foreign
topdir rows), the list area shows exactly `(the trash is empty)` (dark grey); otherwise skip
the empty-message assert and only assert both fixture rows are gone.
Disk: `[ -f "$FIXTURE/made file.txt" ]`.
**Note:** restores are deliberately NOT undo-recorded (v1) — assert `undo_depth == 5`
unchanged after closing the view (next step).

### 04.15 — Close the trash view with q; restored files visible in the pane
**Action:** `tmux send-keys -t $SESSION q`; await-idle.
**Expect (socket):** `mode == "normal"`; `undo_depth == 5`; number of open tabs UNCHANGED
(`tabs` length same as before — `q` inside the trash view closes the view, it must NOT reach
the close_tab binding); `entries center` contains both `amp & spaced.txt` and
`made file.txt` again.
**Expect (screen):** overlay gone; both restored files listed in the center pane.

### 04.16 — Delete a directory to trash, restore it via gT, close with Esc
**Action:** press `j`/`k` until `state.selection == "sub dir & stuff"`; `-l delete`;
await-idle (assert dir gone from `entries center`, `undo_depth == 6`,
`[ ! -e "$FIXTURE/sub dir & stuff" ]`). Then `g T` (assert `mode == "trash"`, top row is
`sub dir & stuff` with the directory symbol); press `r`; await-idle; then `Escape`.
**Expect (socket):** after Escape `mode == "normal"`; log contains
`wiederhergestellt: 1 Element(e) aus dem Papierkorb`.
**Expect (screen):** `sub dir & stuff` back in the center pane.
Disk: `[ -f "$FIXTURE/sub dir & stuff/inner.txt" ]` — the directory's CONTENT survived the
trash round-trip.

**Phase 1 teardown:** `tmux kill-session -t $SESSION; rm -f $SOCK` (keep nothing; Phase 2
uses fresh dirs).

---

## Phase 2 — use_trash = false (permanent delete + undo Barrier)

Coordination note: section 10 (undo/redo) covers barrier semantics in depth; this phase only
proves the delete-side contract (permanent removal, barrier recorded, undo blocked, gT
disabled).

### 04.17 — Relaunch with use_trash = false
**Setup:**
```bash
FIXTURE2=$(mktemp -d); DATA2=$(mktemp -d); CFG2=$(mktemp -d)
touch "$FIXTURE2/doomed.txt" "$FIXTURE2/keeper.txt"
printf '[general]\nuse_trash = false\n' > "$CFG2/config.toml"
```
**Action:** launch per README into the same SESSION/SOCK names (fresh tmux session), with
`XDG_DATA_HOME=$DATA2` exported in the pane, `--debug-socket $SOCK --config $CFG2 $FIXTURE2`;
`echo state | socat ...`.
**Expect (socket):** `mode == "normal"`, `total == 2`, `selection == "doomed.txt"` (no
directories in this fixture, plain files sort alphabetically, `doomed.txt` < `keeper.txt`),
`undo_depth == 0`.
**Expect (screen):** both files listed, `doomed.txt` highlighted.

### 04.18 — Permanent delete
**Action:** `tmux send-keys -t $SESSION -l delete`; await-idle; `echo "log 20" | socat ...`.
**Expect (socket):** log INFO `Deleted 1 items`; `undo_depth == 1` — this entry is a
**Barrier** (`permanentes Löschen`), not a transaction; `redo_depth == 0`; `entries center`
contains only `keeper.txt`.
**Expect (screen):** `doomed.txt` gone.
Disk: `[ ! -e "$FIXTURE2/doomed.txt" ]` AND `$DATA2/Trash` does not exist or contains no
`doomed.txt` (nothing was trashed anywhere).

### 04.19 — Undo is blocked by the barrier
**Action:** `tmux send-keys -t $SESSION u`; await-idle; `echo "log 20" | socat ...`.
**Expect (socket):** log contains a WARN line
`kann nicht rückgängig gemacht werden: permanentes Löschen`; `undo_depth == 1` UNCHANGED
(the barrier stays on the stack, undo never reverts past it); `redo_depth == 0`.
**Expect (screen):** `doomed.txt` still absent; the warning visible in the log widget if
captured within 10s.
Disk: `[ ! -e "$FIXTURE2/doomed.txt" ]` — nothing came back.

### 04.20 — gT is disabled with use_trash = false
**Action:** `tmux send-keys -t $SESSION g T`; await-idle; `echo "log 20" | socat ...`.
**Expect (socket):** `mode == "normal"` — the trash view did NOT open; log contains WARN
`Trash is disabled (use_trash = false) — nothing to show.`
**Expect (screen):** no overlay; normal Miller columns unchanged.

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$PARENT" "$DATA" "$CFG" "$FIXTURE2" "$DATA2" "$CFG2"; rm -f $SOCK` (`$PARENT` wraps the quiet-parent `$FIXTURE`).

**Section coverage gaps (deliberate):**
- Undo/redo of rename/mkdir/touch/trash-delete (the `u`/`ctrl-r` round-trips, redo re-trash,
  zip/tar `no_redo`) — section 10; this section only asserts `undo_depth` deltas and the
  barrier block.
- Bulk rename via editor (multiple marked items + `rename`) — interactive editor flow, not
  covered here.
- Deleting multiple marked items in one `delete` (`Deleted N items` with N>1) and
  mark-auto-advance interactions — marking section.
- Cut/copy/paste, paste-overwrite, `_`-suffix collision — clipboard section.
- Trash-view scrolling with more entries than the visible window (windowed list), and
  restore of an entry that vanished out-of-band (`trash entrie(s) vanished before restore`
  warn path).
- `mkdir`/`touch` on an already-existing name (create-if-missing succeeds but records no
  undo entry) — asserted only indirectly via undo_depth elsewhere.
- Rename preview appearance in the `entries` socket reply (unspecified here; screen-only
  assert in 04.3).
- Cross-filesystem trash placement (topdir `.Trash-$UID`) — environment-dependent, noted as
  caveat only.
