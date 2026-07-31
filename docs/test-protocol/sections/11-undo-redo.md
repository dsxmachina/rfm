# Section 11 — Undo / Redo

N=11, SESSION=rfm-sec11, SOCK=/tmp/rfm-sec11.sock. Harness per README.

Covers the in-session undo/redo stack (`src/undo/mod.rs`, applied in
`PanelManager::apply_undo`/`apply_redo`): single- and multi-change
transactions, async paste recording via `undo_tx`, trash-delete
undo/redo, the `_`-collision suffix, the permanent-delete Barrier, and
the `no_redo` archive semantics. `state.undo_depth` / `state.redo_depth`
are the spine of every assertion — **the undo stack counts transactions
AND barriers; the redo stack counts redoable transactions only.**

## Section fixture

```bash
PARENT=$(mktemp -d); FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"   # quiet parent (README)
printf 'alpha\n'   > "$FIXTURE/a.txt"
printf 'bravo\n'   > "$FIXTURE/b.txt"
printf 'charlie\n' > "$FIXTURE/c.txt"
printf 'delta\n'   > "$FIXTURE/d & e.txt"
mkdir "$FIXTURE/dest dir"
```

Sorted center-pane order (directories first): `dest dir`, `a.txt`,
`b.txt`, `c.txt`, `d & e.txt` — initial selection is **`dest dir`**,
`total` == 5.

**Extra isolation env (this section only):** the freedesktop trash must
be isolated AND disk-assertable, so create the temp dir on the host and
pass it into the pane:

```bash
TRASHHOME=$(mktemp -d)
# add XDG_DATA_HOME=$TRASHHOME to the env prefix of the rfm launch line
# (next to XDG_CACHE_HOME etc.). Trashed files then land in
# $TRASHHOME/Trash/files/ and their .trashinfo in $TRASHHOME/Trash/info/.
```

Both `mktemp -d` results live on the same mount (default `$TMPDIR`), so
trashing stays a cheap same-device rename — do not override `TMPDIR` for
only one of them.

German log strings are exact source strings (manager.rs
`apply_undo`/`apply_redo`), not placeholders:
- undo ok: `rückgängig: <label>`
- undo empty: `nichts rückgängig zu machen`
- undo blocked: `kann nicht rückgängig gemacht werden: <reason>`
- redo ok: `wiederhergestellt: <label>`
- redo empty: `nichts wiederherzustellen`
- redo blocked: `redo blockiert: <reason>` (unreachable via the UI today
  — barriers live only on the undo stack — do not wait for it)

Async-paste rule (applies to every paste step below): `paste` records
its transaction on the main loop via `undo_tx` **after** the blocking
task finishes. `await-idle` does NOT cover that task. After `pp`, poll
`echo state | socat - UNIX-CONNECT:$SOCK` until `undo_depth` reaches the
expected value (give it up to ~5 s for these tiny fixtures), THEN assert
everything else.

---

### 11.1 — Baseline: empty stacks, empty-undo and empty-redo messages
**Action:** `echo state | socat - UNIX-CONNECT:$SOCK`; then
`tmux send-keys -t $SESSION u`, await-idle; then
`tmux send-keys -t $SESSION C-r`, await-idle.
**Expect (socket):** initially `undo_depth`==0, `redo_depth`==0,
`mode`=="normal", `selection`=="dest dir". After `u` and after `C-r`
both depths are STILL 0. `log 10` contains INFO lines
`nichts rückgängig zu machen` (after u) and
`nichts wiederherzustellen` (after C-r).
**Expect (screen):** log widget (bottom area above the footer) shows
`nichts wiederherzustellen` (and, if within its 10 s TTL,
`nichts rückgängig zu machen`). Pane content unchanged: `dest dir`
highlighted, all 4 files listed.

### 11.2 — Rename → undo restores name → redo reapplies
**Action:** `tmux send-keys -t $SESSION j` (select `a.txt`), await-idle;
`tmux send-keys -t $SESSION rename` (the 6-char sequence enters rename
mode), await-idle — assert `mode`=="rename" via socket; the input is
seeded with `a.txt`, cursor at end. Then `tmux send-keys -t $SESSION
.bak Enter`, await-idle.
**Expect (socket):** `mode`=="normal", `undo_depth`==1, `redo_depth`==0.
`entries center` contains `a.txt.bak`, no `a.txt`.
`[ -f "$FIXTURE/a.txt.bak" ] && [ ! -f "$FIXTURE/a.txt" ]`.
**Expect (screen):** center pane lists `a.txt.bak`; no `a.txt` row.

**Action (undo):** `tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1. `log 10` contains
`rückgängig: rename a.txt → a.txt.bak`. Disk: `$FIXTURE/a.txt` back
(content `alpha`), `a.txt.bak` gone.
**Expect (screen):** center pane lists `a.txt` again, no `a.txt.bak`;
log widget shows the `rückgängig: rename …` line.

**Action (redo):** `tmux send-keys -t $SESSION C-r`, await-idle.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0. `log 10` contains
`wiederhergestellt: rename a.txt → a.txt.bak`. Disk: `a.txt.bak` back,
`a.txt` gone.
**Expect (screen):** `a.txt.bak` listed again.

**Action (restore for later steps):** `tmux send-keys -t $SESSION u`,
await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1;
`$FIXTURE/a.txt` exists again.
**Note:** `rename` is typed as a plain key sequence; its first chars
overlap other bindings (`r…`) but the parser resolves the full sequence.
Do not assert `selection` after undo/redo — `reload_all()` may move it.

### 11.3 — Copy-paste into subdir, async recording; new record clears redo
**Action:** select `a.txt` (send `gg` then `j`; verify via socket
`selection`=="a.txt"). `tmux send-keys -t $SESSION yy`, await-idle —
assert `clipboard`=={files:[".../a.txt"], op:"copy"} and `log 5` has
`copying 1 items`. Then `gg` (select `dest dir`), `l` (enter it) —
`cwd` now ends in `/dest dir`, `total`==0. Then
`tmux send-keys -t $SESSION pp` and **poll** `state` until
`undo_depth`==1.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0 — the redo entry
left over from 11.2 was **cleared by the new record** (this is the
redo-invalidation assertion, not a leftover). `clipboard`==null (taken
by paste). `log 10` has `paste 1 items, overwrite = false`.
`entries center` == [`a.txt`]. Disk: `"$FIXTURE/dest dir/a.txt"` exists,
`$FIXTURE/a.txt` still exists (copy, not move).
**Expect (screen):** center pane (now showing `dest dir`) lists `a.txt`;
header/path row shows `…/dest dir`.

**Action (undo removes the copy):** `tmux send-keys -t $SESSION u`,
await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1; `log 10` has
`rückgängig: paste (1 items)`. Disk: `"$FIXTURE/dest dir/a.txt"` gone,
`$FIXTURE/a.txt` untouched (content `alpha`).
**Expect (screen):** center pane empty (dest dir has no entries).

### 11.4 — Redo of copy-paste re-copies
**Action:** `tmux send-keys -t $SESSION C-r`, await-idle.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0; `log 10` has
`wiederhergestellt: paste (1 items)`. Disk: `"$FIXTURE/dest dir/a.txt"`
exists again with content `alpha`.
**Expect (screen):** `a.txt` listed in the center pane again.

**Action (clean up for later steps):** `tmux send-keys -t $SESSION u`,
await-idle — `undo_depth`==0, `"$FIXTURE/dest dir"` empty again.

### 11.5 — Cut-paste undo restores the original location
**Setup:** still inside `dest dir`; go back up:
`tmux send-keys -t $SESSION h`, await-idle — `cwd`==$FIXTURE.
**Action:** select `b.txt` (navigate with `j`/`k`, verify
`selection`=="b.txt"), `tmux send-keys -t $SESSION dd` — assert
`clipboard.op`=="cut", `log 5` has `cut 1 items`. Then `gg`, `l` (enter
`dest dir`), `pp`; **poll** `state` until `undo_depth`==1.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0. Disk:
`"$FIXTURE/dest dir/b.txt"` exists, `$FIXTURE/b.txt` gone (move).
`entries center` == [`b.txt`].
**Expect (screen):** `b.txt` in the center pane; the left pane (showing
$FIXTURE) no longer lists `b.txt`.

**Action (undo):** `tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1; `log 10` has
`rückgängig: paste (1 items)`. Disk: `$FIXTURE/b.txt` restored (content
`bravo`), `"$FIXTURE/dest dir/b.txt"` gone.
**Expect (screen):** center pane (dest dir) empty; left pane lists
`b.txt` again. Socket `entries left` must agree with the captured left
pane — a mismatch is a stale-render bug.

### 11.6 — Collision-suffix paste: undo removes exactly the `_` file
**Setup:** `tmux send-keys -t $SESSION h`, await-idle (back in
$FIXTURE).
**Action:** select `c.txt`, `yy`, then `pp` **in the same directory**;
poll `state` until `undo_depth`==1 (note: this new record clears the
redo entry from 11.5, so also `redo_depth`==0).
**Expect (socket):** `entries center` contains BOTH `c.txt` and `c.txt_`
(paste never overwrites; `get_destination` appends `_` until free).
Disk: `$FIXTURE/c.txt_` exists with content `charlie`; `$FIXTURE/c.txt`
untouched.
**Expect (screen):** both `c.txt` and `c.txt_` rows visible.

**Action (undo):** `tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1. Disk:
`$FIXTURE/c.txt_` gone; `$FIXTURE/c.txt` still present, content still
`charlie` — the undo removed precisely the `_`-suffixed copy (the
`FsChange::Copy` carries the actually-reached path).
**Expect (screen):** `c.txt_` row gone, `c.txt` still listed.

### 11.7 — Multi-item transaction: N files pasted, undone as ONE unit
**Action:** in $FIXTURE, select `a.txt` (`gg` then `j`), then mark three
files: `tmux send-keys -t $SESSION Space` (marks `a.txt`, cursor
auto-advances to `b.txt`), `j` (skip to `c.txt`), `Space` (marks
`c.txt`, advances to `d & e.txt`), `Space` (marks `d & e.txt`).
Await-idle; assert `state.marked` == the three absolute paths (order
irrelevant) and `entries center` shows `marked:true` on exactly those
three. Then `yy` (`log`: `copying 3 items`), `gg`, `l` (into
`dest dir`), `pp`; **poll** until `undo_depth`==1.
**Expect (socket):** `undo_depth`==1 — a delta of exactly +1 for three
files: one `Transaction`, three `FsChange::Copy`s. `marked`==[] (paste
unmarks). `log 10` has `paste 3 items, overwrite = false`. Disk: all of
`a.txt`, `c.txt`, `d & e.txt` exist in `"$FIXTURE/dest dir"`;
`d & e.txt` content is `delta` (space-and-`&` name survived — these fs
ops never go through a shell).
**Expect (screen):** three rows in the center pane, including the
literal name `d & e.txt`.

**Action (single-key undo):** `tmux send-keys -t $SESSION u`,
await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1; exactly ONE new
log line `rückgängig: paste (3 items)`. Disk: all three copies gone from
`dest dir`; all three originals untouched in $FIXTURE.
**Expect (screen):** center pane (dest dir) empty again.

### 11.8 — Trash-delete: undo restores from trash, redo re-trashes
**Setup:** `tmux send-keys -t $SESSION h` (back to $FIXTURE); select
`b.txt` (verify `selection`=="b.txt", `marked`==[]).
**Action:** `tmux send-keys -t $SESSION -l delete` (type the 6-char
sequence LITERALLY — `delete` is a tmux **key name**, so a bare
`send-keys delete` sends the Delete key and rfm never sees the
d-e-l-e-t-e binding, silently no-opping; the `-l` is required),
await-idle.
**Expect (socket):** `undo_depth`==1 (redo from 11.7 cleared,
`redo_depth`==0). `log 200` (queried immediately) has `Deleted 1 items`
(a short `log 10` window can be evicted by TRACE churn). `entries center`
has no `b.txt`.
Disk: `$FIXTURE/b.txt` gone; `$TRASHHOME/Trash/files/b.txt` exists and
`$TRASHHOME/Trash/info/b.txt.trashinfo` exists.
**Expect (screen):** no `b.txt` row.

**Action (undo restores):** `tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1; `log 10` has
`rückgängig: delete (1 items)`. Disk: `$FIXTURE/b.txt` back (content
`bravo`); `$TRASHHOME/Trash/files/b.txt` gone (restore_all removes the
entry and its .trashinfo).
**Expect (screen):** `b.txt` row back in the center pane.

**Action (redo re-trashes):** `tmux send-keys -t $SESSION C-r`,
await-idle.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0; `log 10` has
`wiederhergestellt: delete (1 items)`. Disk: `$FIXTURE/b.txt` gone;
`$TRASHHOME/Trash/files/b.txt` exists again (a FRESH trash entry —
`Trash::redo` re-captures the new TrashItem).
**Expect (screen):** `b.txt` row gone again.

**Action (cycle consistency — undo the redo):**
`tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** `undo_depth`==0, `redo_depth`==1; `$FIXTURE/b.txt`
restored again — proving the re-captured TrashItem was valid, the
delete↔undo↔redo cycle is stable.
**Note:** if the trash dir also received same-named files from earlier
runs the entry could be `b.txt_`-suffixed in `files/`; with the fresh
`$TRASHHOME` this must not happen — treat a suffixed name as a harness
isolation failure, not an rfm bug.

### 11.9 — tar archive: undoable but no_redo
**Skip** if `tar` is not on PATH (then also record protocol feedback).
**Action:** in $FIXTURE, select `d & e.txt` (bottom entry: `G`), send
`tar` (3-char sequence), await-idle.
**Expect (socket):** `undo_depth`==1, `redo_depth`==0. `log 10` has
`Creating tar.gz archive from 1 files`. `entries center` contains
`output.tar.gz`. Disk: `$FIXTURE/output.tar.gz` exists and
`tar -tzf "$FIXTURE/output.tar.gz"` lists exactly `d & e.txt` (name
with space and `&` passed as argv, no shell mangling).
**Expect (screen):** `output.tar.gz` row visible.

**Action (undo deletes the archive):** `tmux send-keys -t $SESSION u`,
await-idle.
**Expect (socket):** `undo_depth`==0 and **`redo_depth`==0** — the
transaction is `no_redo` (`Transaction::no_redo`, recorded via
`record_archive`): it is popped on undo but NOT pushed to the redo
stack. `log 10` has `rückgängig: tar`. Disk: `output.tar.gz` gone.
**Expect (screen):** `output.tar.gz` row gone.

**Action (redo is a no-op):** `tmux send-keys -t $SESSION C-r`,
await-idle.
**Expect (socket):** depths still 0/0; `log 5` has
`nichts wiederherzustellen`; `output.tar.gz` NOT recreated (disk and
`entries center`).
**Expect (screen):** log widget shows `nichts wiederherzustellen`; no
archive row.

### 11.10 — zip archive: same no_redo semantics (optional)
**Skip** if `zip` is not on PATH (log will show
`zip is not installed - install it to create zip archives`; record as
environment note, not a bug).
**Action:** select `a.txt`, send `zip`, await-idle; then `u`; then
`C-r`.
**Expect (socket):** after `zip`: `undo_depth`==1, `log` has
`Creating zip archive from 1 files`, `$FIXTURE/output.zip` exists.
After `u`: depths 0/0, `log` has `rückgängig: zip`, `output.zip` gone.
After `C-r`: depths still 0/0, `nichts wiederherzustellen`.
**Expect (screen):** `output.zip` appears, then disappears; no
reappearance after C-r.

### 11.11 — Barrier: permanent delete blocks undo, never reverts past it
**Setup (fresh relaunch with `use_trash = false`):** tear down the
running session (`tmux kill-session -t $SESSION; rm -f $SOCK`), then:

```bash
FIXTURE2=$(mktemp -d); TRASHHOME2=$(mktemp -d)
printf 'xx\n' > "$FIXTURE2/x.txt"
printf 'yy\n' > "$FIXTURE2/y.txt"
printf '[general]\nuse_trash = false\n' > "$CFG/config.toml"
# relaunch per README harness with $FIXTURE2, XDG_DATA_HOME=$TRASHHOME2
# and the same $SOCK/$SESSION names
```

**Action (a — recorded op BEFORE the barrier):** select `x.txt` (first
entry, no directories in FIXTURE2 → initial selection IS `x.txt`), send
`rename`, then `.bak`, `Enter`; await-idle.
**Expect (socket):** `undo_depth`==1; `$FIXTURE2/x.txt.bak` exists.

**Action (b — permanent delete):** select `y.txt`, send the delete
binding LITERALLY: `tmux send-keys -t $SESSION -l delete` (bare
`send-keys delete` sends the Delete key, not the binding — see 11.8),
await-idle.
**Expect (socket):** `undo_depth`==2 (the Barrier counts as a stack
entry), `redo_depth`==0. `log 200` has `Deleted 1 items`. Disk:
`$FIXTURE2/y.txt` gone AND `$TRASHHOME2/Trash/files/` does not contain
`y.txt` (nothing was trashed — permanent).
**Expect (screen):** `y.txt` row gone.

**Action (c — undo hits the barrier):** `tmux send-keys -t $SESSION u`,
await-idle.
**Expect (socket):** `undo_depth` STILL ==2 (the barrier is not popped),
`redo_depth`==0. `log 10` has the WARN line
`kann nicht rückgängig gemacht werden: permanentes Löschen`. Disk:
`y.txt` NOT restored, and — critically — `x.txt.bak` is STILL named
`x.txt.bak`: the rename from (a) was NOT reverted. Undo never reaches
past a barrier.
**Expect (screen):** log widget shows the warn line; pane still lists
`x.txt.bak`, no `y.txt`.

**Action (d — repeat):** `tmux send-keys -t $SESSION u`, await-idle.
**Expect (socket):** identical: depths 2/0, another
`kann nicht rückgängig gemacht werden: permanentes Löschen` line —
the barrier blocks forever, it is never consumed.
**Note:** `C-r` here yields `nichts wiederherzustellen` (empty redo
stack); the `redo blockiert:` string exists in source but is not
reachable through this flow — do not expect it.

**Teardown:** `tmux kill-session -t $SESSION 2>/dev/null; rm -rf
"$PARENT" "$FIXTURE2" "$CFG" "$CACHE" "$STATE" "$ZO" "$TRASHHOME"
"$TRASHHOME2"; rm -f $SOCK` (`$PARENT` is the quiet-parent wrapper of `$FIXTURE`).

**Section coverage gaps (deliberate):**
- mkdir/touch `Create` undo (incl. the touch-on-existing-file
  not-recorded rule) and bulk-rename transactions — belongs to the
  create/rename section.
- `paste_overwrite` (`po`) undo semantics.
- Undo of a cut-paste whose SOURCE sat in a different tab (cross-tab
  reload path) — tabs section.
- `UndoOutcome::Failed` paths (e.g. undo after the target was modified
  externally) and undo of a directory copy/move (only files tested).
- Trash-view (`gT`) restore interaction with a pending
  `FsChange::Trash` undo (the vanished-entry no-op guard in
  `FsChange::undo`) — trash section.
- Undo stack non-persistence across restarts (implicitly true — stack
  is in-memory — but not asserted here).
- Delete of multiple marked items in one transaction (single-item only;
  multi-item is covered for paste in 11.7).
