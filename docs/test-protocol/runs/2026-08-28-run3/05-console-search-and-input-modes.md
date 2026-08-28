# Run 3 — Section 05: Console, search and input modes

- Date: 2026-08-28 · Branch: `feat/kitty-unicode-placeholders` · Binary: `./target/debug/rfm` (prebuilt)
- Harness: session `tp3-05`, socket `/tmp/tp3-05.sock`, quiet fixture parent
  (`PARENT=$(mktemp -d)`, `FIXTURE=$PARENT/fx`), scratch `--config`,
  `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs, launched directly as the
  tmux session command with `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`.
  All socket reads wrapped in `timeout 12`; none timed out (no wedge at any step).
- Environment: zoxide IS on PATH (`/run/current-system/sw/bin/zoxide`), socat present.
- `image_protocol` resolved to `half-block` inside tmux, as expected.

**Result: 12/12 steps PASS, 0 fail, 0 skip. No bugs filed.**

---

## Phase 1 — search mode

### 05.1 — Launch baseline — PASS
Socket: `mode=normal`, `cwd=$FIXTURE`, `total=6`, `selection="alpha dir"`,
`selected_idx=0`, `marked=[]`, `clipboard=null`. `entries center` listed exactly
the 6 fixture names; only `alpha dir` had `selected:true`, none marked.
Screen: all 6 rows drawn, `alpha dir` carried the reverse-video highlight
(`[7m` on its row via `capture-pane -e`), header showed the selected item's
path (`.../fx/alpha dir` — the documented header semantic), no overlay, no
footer prompt.

### 05.2 — Enter search; live highlight filters the render — PASS
After `/`: `mode=search`. After `-l report`: `mode=search`, `marked=[]`,
`total=6`, `selection/selected_idx` unchanged; `entries center` still listed
all 6 names, all `marked:false` (render-only filter confirmed — the documented
screen/socket divergence, not a stale-render bug).
Screen: center pane showed only `report-2.txt` and `report.txt`, each with the
substring `report` in bold red (`[1m[38;5;9m`); footer read
`Search report` with `Search` bold+reversed green (`[1;7m[38;5;2m`) and the
input in red.

### 05.3 — Live no-match shows the `(no match)` row — PASS
Sent `BSpace` ×6 (pattern emptied): all 6 rows re-shown, no bold, footer
`Search` with empty input — the empty-pattern render asserted per the note.
Then `-l zzz`: center pane collapsed to a single italic red row
` (no match)` (`[0;3m[38;5;9m`), footer `Search zzz`. Socket unchanged
throughout: `mode=search`, `marked=[]`, `total=6`, selection/idx stable.
Observation (not filed): the `(no match)` row renders a trailing `0` in the
size/count column — cosmetic rendering artifact of the placeholder row, not
asserted by the protocol.

### 05.4 — Esc cancels search — PASS
`mode=normal`, `marked=[]`, `total=6`, `selection="alpha dir"`,
`selected_idx=0`. `entries center`: no `marked:true`. Screen: all 6 rows, zero
occurrences of the red highlight SGR (`38;5;9m` count = 0), footer prompt gone,
normal metadata footer (`drwxr-xr-x ... 1/6`) restored.

### 05.5 — Enter commits search: matches marked, cursor jumps — PASS
`/` + `-l report` (mode=search confirmed) + `Enter`:
`mode=normal`, `total=6` unchanged, `marked` = exactly
`["$FIXTURE/report-2.txt","$FIXTURE/report.txt"]`, `selection="report-2.txt"`,
`selected_idx=4`. `entries center`: `marked:true` on exactly those two rows,
`selected:true` on `report-2.txt`.
Screen: no footer prompt; both `report*` rows drawn with the marked lead
indicator `x` in dark-yellow (`38;5;3m`); cursor reverse-video (`[0;7m`) on
`report-2.txt`.

### 05.6 — `n` / `N` cycle the marked items — PASS
- `n` #1: `selection="report.txt"`, `selected_idx=5`; highlight moved (`[0;7m`
  on report.txt row).
- `n` #2: wrapped to `selection="report-2.txt"`, `selected_idx=4`.
- `N`: `selection="report.txt"`, `selected_idx=5` (previous-with-wrap to the
  last marked).
`mode=normal` and `marked` unchanged throughout; both rows stayed
dark-yellow-marked the whole time.

## Phase 2 — the cd console

### 05.7 — Open cd console; overlay visible — PASS
`-l cd`: `mode="console"`, `cwd=$FIXTURE` unchanged (marks from 05.5 still
present, as the step allows). Screen: centered overlay with `┴…┴` / `┬…┬`
divider bars and a middle path line `​$FIXTURE/` followed by the dark-grey
recommendation (rendered as `/tmp/.../fx/beta dir` — only the `$FIXTURE/`
prefix asserted per the step). Miller columns visible above/below; no footer
search prompt.

### 05.8 — Esc cancels cd console to start dir — PASS
`mode=normal`, `cwd=$FIXTURE`; overlay gone (divider-bar glyph count 0),
`$FIXTURE` listing back in the center pane.

### 05.9 — cd console live descent + Enter commit — PASS
`-l cd` (mode=console) then `-l "alpha dir"`: while STILL in the console,
`cwd` had already moved to `$FIXTURE/alpha dir` (live `jump()` confirmed),
`total=0`, `preview_path="path-of-empty-panel"` (empty-dir sentinel), and the
overlay path line read `$FIXTURE/alpha dir/`. Transient
`queue_active:"zoxide add current-dir"` observed — the expected zoxide-add
side effect of the live cd. After `Enter`: `mode=normal`,
`cwd=$FIXTURE/alpha dir`, `total=0`; overlay gone, center pane `(empty)`,
header path ends in `/alpha dir`. (Per-tab marks cleared by the directory
change; not asserted by this phase.)

### 05.10 — Backspace at empty input walks to parent — PASS
Reopened console inside `alpha dir` (overlay line `$FIXTURE/alpha dir/`).
`BSpace` with empty input: `mode=console` still, `cwd=$FIXTURE` (live parent
walk), overlay line prefix `$FIXTURE/` with a recommendation suffix
(`alpha dir`) after it. `Enter`: `mode=normal`, `cwd=$FIXTURE`, listing
restored.

## Phase 3 — zoxide console

### 05.11 — Open zoxide console with `CD` — PASS
`send-keys -l CD` delivered the SHIFT-modified sequence fine (log:
`key-event: Char('D'), modifiers: SHIFT` → `Command: Cd { zoxide: true }` →
`mode: normal -> console`). `mode="console"` (same string as the cd console),
`cwd` unchanged. Screen: centered overlay with divider bars, empty input line
and a recommendation line below it; NO `zoxide is not installed` hint (correct
— zoxide is on PATH, so the hint assertion is N/A per the step). No query
typed / no jump committed, per the step's scope.

### 05.12 — Esc cancels zoxide console back to start — PASS
`mode=normal`, `cwd=$FIXTURE`; overlay gone, `$FIXTURE` listing restored.
Log trail confirms the cleanup path: `key-event: Esc (mode: console)` →
`jump-to $FIXTURE` → `mode: console -> normal`.

---

## Bugs

None. No socket/screen mismatch at any step; no `state` timeout (every reply
arrived well inside `timeout 12`).

## Protocol feedback

1. **05.3 `(no match)` row renders a stray `0` in the size column**
   (`│ (no match)                               0 │`). Cosmetic; the protocol
   only asserts the italic row text, so recorded here rather than as a bug —
   the section (or KNOWN-ISSUES) may want to either assert or explicitly
   ignore that trailing `0`.
2. **05.9 could mention the `queue_active: "zoxide add current-dir"`
   transient** — an executor diffing full `state` between steps will see it
   flicker in after the live cd; harmless and expected, worth a note so it is
   not mistaken for a stuck queue.
3. **05.9 side effect worth documenting:** the live cd clears the per-tab
   `marked` set left over from Phase 1 (marks are per-panel/per-dir). Phase 2
   correctly does not depend on `marked`, but the 05.7 setup paragraph muses
   about clearing marks — a one-line "the cd in 05.9 clears them anyway"
   would close that loop.
4. **05.11 recommendation line content with a seeded-but-fresh
   `_ZO_DATA_DIR`:** the overlay's lower line showed a bare `.` (rfm had
   already `zoxide add`ed the visited dirs during 05.9's live cd). Harmless,
   but the step's "path/recommendation line" wording could note the content is
   unspecified when the zoxide DB is young.
5. `send-keys -l CD` uppercase delivery worked reliably in this environment —
   the skip-gating on SHIFT delivery was not needed (kept as-is is fine; just
   confirming the primary path is exercisable).

## Teardown

Completed: `tmux kill-session -t tp3-05`, removed `/tmp/tp3-05.sock`, fixture
parent, and the CFG/CACHE/STATE/ZO tempdirs, plus the per-section env file.
