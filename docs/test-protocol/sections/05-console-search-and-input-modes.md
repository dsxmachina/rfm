# Section 05 — Console, search and input modes

N=05, SESSION=rfm-sec05, SOCK=/tmp/rfm-sec05.sock. Harness per README.

Covers the modal input seam (`src/panel/mode/`, applied centrally in
`PanelManager::apply_mode_op`, manager.rs): the two navigation **consoles**
(`cd` and zoxide, both `DirConsole`/`Zoxide` reporting `mode == "console"`,
drawn as a centered `ConsoleOverlay`) and **search** mode (`SearchMode`,
`mode == "search"`, a `FooterLine` prompt). It asserts every mode entry via
BOTH the `state.mode` string and a visible overlay/prompt on the captured
pane, plus the exact Esc/Enter semantics each adapter implements.

Two facts about search that drive the assertions below, verified in
`src/panel/directory.rs`:
- **Live** search (`update_search`, before Enter) sets a highlight pattern
  and the center pane redraws **only the matching rows** (each with the
  substring bold in the `highlight` color = default `red`); no match →
  a single ` (no match)` row (`directory.rs:457-501`). This is a *render-time
  filter only* — the underlying entries, `total`, `selection` and
  `selected_idx` in `state`/`entries` are UNCHANGED while typing.
- **Enter** (`finish_search`, `directory.rs:891-897` + `apply_mode_op`
  manager.rs:1860-1867) does NOT filter: it **marks every entry whose
  lowercased name contains the pattern**, clears the live highlight, then
  `select_next_marked()` jumps the cursor to the first match. So the
  observable socket effect of Enter is `marked` gaining the matches and a
  `selected_idx` jump — *not* a reduced `total`.

The `search` command is bound to `/`, `search`, `f`; `cd` to `cd`;
`zoxide_query` to `CD`/`Cd`/`cD` (verified in `examples/default-config.toml`
`[keys.general] search` and `[keys.manipulation] change_directory` /
`zoxide_query`). `n` / `N` (`[keys.general] next` / `previous`) select the
next / previous **marked** item (`Command::Next`/`Previous`,
manager.rs:2219-2232) — the same marked set search populates.

## Section fixture

```bash
PARENT=$(mktemp -d); FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"   # quiet parent (README)
mkdir "$FIXTURE/alpha dir"
mkdir "$FIXTURE/beta dir"
touch "$FIXTURE/report.txt" "$FIXTURE/report-2.txt" "$FIXTURE/notes.md" "$FIXTURE/misc.log"
```

Sorted center-pane order (directories first, then files alphabetically):
`alpha dir`, `beta dir`, `misc.log`, `notes.md`, `report-2.txt`,
`report.txt` — **initial selection is `alpha dir`**, `total == 6`. The two
`report*` files give search a two-hit pattern (`report`); `alpha dir` /
`beta dir` are real subdirectories the `cd` console can descend into and
recommend.

**Typed-sequence conventions (same as section 04):** multi-char command
sequences (`cd`, `search`) are sent with `tmux send-keys -t $SESSION -l <seq>`
(literal mode) so tmux never treats them as key names; special keys (`Enter`,
`Escape`, `Backspace`, `Tab`) and single printable keys (`/`, `f`, `n`, `N`)
are sent WITHOUT `-l`, as their own send-keys calls. Avoid uppercase in typed
search patterns — the input widget normalises case and terminals do not report
SHIFT uniformly (the `zoxide_query` binding is uppercase-only, which is why the
zoxide step below is gated/skippable, see 05.7).

Every step runs the full README verification loop (await-idle → state →
capture-pane); the Expect blocks list only the step-specific assertions. The
`state` scalar fields mirror the focused (only) tab throughout.

---

## Phase 1 — search mode

### 05.1 — Launch baseline
**Action:** launch per README (`--debug-socket $SOCK --config $CFG $FIXTURE`);
`echo state | socat - UNIX-CONNECT:$SOCK`; `echo "entries center" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `mode == "normal"`, `cwd == $FIXTURE`, `total == 6`,
`selection == "alpha dir"`, `selected_idx == 0`, `marked == []`,
`clipboard == null`. `entries center` names (set equality): `alpha dir`,
`beta dir`, `misc.log`, `notes.md`, `report-2.txt`, `report.txt`; exactly
`alpha dir` has `selected:true`, none `marked:true`.
**Expect (screen):** center pane lists all 6 entries; `alpha dir` highlighted;
header shows the `$FIXTURE` path; no overlay, no footer prompt.

### 05.2 — Enter search mode; live highlight filters the render
**Action:** `tmux send-keys -t $SESSION /`; await-idle; assert `mode`; then
`tmux send-keys -t $SESSION -l report`; await-idle; capture-pane;
`echo "entries center" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** after `/`: `mode == "search"`. After typing `report`:
`mode == "search"` still; `marked == []` (nothing marked while typing —
`finish_search` has not run); `total == 6` UNCHANGED; `selection == "alpha dir"`
and `selected_idx == 0` UNCHANGED (live search never moves the cursor). The
`entries center` reply still lists all 6 names with `marked:false` on every row
(the filter is render-only).
**Expect (screen):** footer line shows the prompt `Search` (bold, reversed,
main/`dark-green`) followed by a space and the live input `report` (in the
search input color, `red`). The center pane now shows **only** the two rows
`report-2.txt` and `report.txt`, each with the substring `report` bold in the
`highlight` color (`red`); the four non-matching entries are not drawn.
**Note:** do NOT assert `total`/`selection` changed — the divergence between
the filtered *screen* and the full `entries` reply here is expected and IS the
documented behavior, not a stale-render bug.

### 05.3 — Live search with no match shows the `(no match)` row
**Action:** `tmux send-keys -t $SESSION BSpace` six times (clears `report`
to empty, then poll — or just clear and retype); simpler: `tmux send-keys -t
$SESSION -l zzz` to a now-non-matching pattern. Between each, await-idle.
Preferred sequence: from the `report` state of 05.2, send `Backspace` ×6 to
empty, then `-l zzz`; await-idle; capture-pane.
**Expect (socket):** `mode == "search"` throughout; `marked == []`;
`total == 6`; `selection`/`selected_idx` still unchanged.
**Expect (screen):** with input `zzz` the center pane shows a single italic row
` (no match)` (`highlight` color) — no entry rows. Footer prompt reads
`Search zzz`.
**Note:** Backspace reaches `Input::update` (`SearchMode::handle_key` routes
every non-Enter/non-Esc key to the input); an empty pattern re-shows all rows
(every name "contains" the empty string). Assert the empty-pattern render
(all 6 rows, no bold) if you pause at empty before typing `zzz`.

### 05.4 — Esc cancels search: mode normal, highlight cleared, nothing marked
**Action:** `tmux send-keys -t $SESSION Escape`; await-idle;
`echo "entries center" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `mode == "normal"`; `marked == []` (Esc requests
`Cleanup::Search` = `clear_search()` only — it never marks); `total == 6`;
`selection == "alpha dir"`, `selected_idx == 0` (unchanged). No `marked:true`
in `entries center`.
**Expect (screen):** all 6 entries listed again, no bold highlight; footer
prompt gone (normal metadata footer restored).

### 05.5 — Enter commits search: matches get MARKED, cursor jumps to first match
**Action:** re-enter search with `/`, `-l report`, await-idle (assert
`mode == "search"`), then `tmux send-keys -t $SESSION Enter`; await-idle;
`echo state | socat ...`; `echo "entries center" | socat ...`.
**Expect (socket):** `mode == "normal"` (FinishSearch concludes the mode);
`total == 6` UNCHANGED (search marks, never deletes/filters); `marked` contains
exactly the two absolute paths `$FIXTURE/report-2.txt` and `$FIXTURE/report.txt`
(order irrelevant); `entries center` shows `marked:true` on exactly those two
rows. `selection` jumped to the **first** matching entry in sort order —
`report-2.txt` (`report-2.txt` sorts before `report.txt`), so
`selection == "report-2.txt"` and `selected_idx == 4` (0-based among the 6
visible entries: `alpha dir`0 `beta dir`1 `misc.log`2 `notes.md`3
`report-2.txt`4 `report.txt`5).
**Expect (screen):** no footer prompt; the two `report*` rows carry the
`marked` lead indicator/color (`marked` = default `dark-yellow`); the cursor
highlight sits on `report-2.txt`.
**Note:** `select_next_marked` searches from `selected_idx + 1` first, then
wraps from the start (`directory.rs:960-986`). With the pre-Enter cursor at
`alpha dir` (idx 0) the first marked item at or after idx 1 is `report-2.txt`
— hence idx 4. Do not assert a `+1` seq delta; `seq` jumps by several ticks.

### 05.6 — `n` / `N` cycle the marked (matched) items
**Action:** from the 05.5 state (cursor on `report-2.txt`, both `report*`
marked): `tmux send-keys -t $SESSION n`; await-idle; `echo state | socat ...`.
Then `tmux send-keys -t $SESSION n` again; await-idle. Then
`tmux send-keys -t $SESSION N`; await-idle.
**Expect (socket):** after first `n`: `selection == "report.txt"`,
`selected_idx == 5` (next marked after idx 4). After second `n`: wraps to
`selection == "report-2.txt"`, `selected_idx == 4` (no marked item after idx 5
→ search restarts from the top). After `N`: `selection == "report.txt"`,
`selected_idx == 5` (previous marked before idx 4, wrapping to the last marked).
`mode == "normal"` throughout; `marked` unchanged (n/N only move the cursor).
**Expect (screen):** the cursor highlight moves between the two `report*` rows
accordingly; both rows stay `marked`-colored the whole time.
**Note:** `n`/`N` are `Command::Next`/`Previous` over the marked set — the same
set search populated. They are NOT search-history "next match" keys; there is
no separate live-search cursor. This is the only next/prev-match mechanism rfm
has (see coverage gaps).

---

## Phase 2 — the cd console

### 05.7 — Enter the `cd` console; overlay visible; Esc cancels to start dir
**Setup:** first clear the marks left by search so this phase starts clean:
`tmux send-keys -t $SESSION /` then `Enter` with an empty pattern? — no: an
empty `finish_search` marks *every* entry. Instead re-run a non-matching commit
is impossible to unmark via search. Simplest: relaunch is unnecessary — marks
do not affect `cd`. Proceed with marks present; assertions below do not depend
on `marked`.
**Action:** `tmux send-keys -t $SESSION -l cd`; await-idle; `echo state | socat ...`;
capture-pane.
**Expect (socket):** `mode == "console"` (BOTH `DirConsole` and `Zoxide` report
the literal string `"console"` — `console.rs` `name()`); `cwd == $FIXTURE`
UNCHANGED (the console navigates live but has not been given a directory yet).
**Expect (screen):** a centered overlay with horizontal divider bars above and
below a middle line that shows the current path prefix `$FIXTURE/` (the console
appends `/` after the path). A dark-grey recommendation suffix follows the path
(e.g. `$FIXTURE/beta dir`) — assert only the `$FIXTURE/` path prefix, and do NOT
assert which subdir is recommended (recommendation ordering is PatriciaSet /
case-folded, e.g. `beta dir` may precede `alpha dir`). The normal Miller columns
are still visible behind/around it. No footer search prompt.
**Note:** `cd` is a 2-char sequence; `c` alone is not a binding here, so the
mode only opens after the `d`. The mode string is `console`, not `cd`.

### 05.8 — Esc cancels the cd console back to the starting directory
**Action:** `tmux send-keys -t $SESSION Escape`; await-idle; `echo state | socat ...`.
**Expect (socket):** `mode == "normal"`; `cwd == $FIXTURE` (Esc requests
`Cleanup::CdTo(starting_path)` — `console.rs:325-329` — restoring the entry
directory; here we never moved, so it is a no-op cd back to `$FIXTURE`).
**Expect (screen):** overlay gone; center pane shows the `$FIXTURE` listing
again.

### 05.9 — cd console descends into a subdirectory live, Enter commits
**Action:** `tmux send-keys -t $SESSION -l cd`; await-idle (assert
`mode == "console"`); then type the subdir name so the console's live
`insert()` jumps into it — send `-l "alpha dir"` (a space is a valid char the
console appends); await-idle; `echo state | socat ...`; capture-pane. Then
`tmux send-keys -t $SESSION Enter`; await-idle; `echo state | socat ...`.
**Expect (socket):** while still in the console after typing the full existing
dir name, `mode == "console"` and `cwd` has moved to `$FIXTURE/alpha dir`
(`DirConsole::insert` returns `Some(joined_path)` once the input names an
existing directory → `ModeOp::Cd` → `jump()`; `cwd` updates live BEFORE Enter).
After Enter: `mode == "normal"`, `cwd == $FIXTURE/alpha dir` (Enter =
`ModeOp::Exit { Cleanup::None }`, keeping the live-navigated directory).
`total == 0` (the fixture subdir is empty).
**Expect (screen):** while typing, the overlay's path line shows
`$FIXTURE/alpha dir/`; after Enter the overlay is gone and the center pane is
the (empty) `alpha dir` contents; header path ends in `/alpha dir`.
**Note:** exact live-navigation timing (which keystroke triggers the jump) is
recommendation-dependent (`insert` weighs prefix recommendations vs. an exact
dir match, `console.rs:184-240`); assert only the END state — `cwd` under
`alpha dir` once the whole name is typed, and after Enter. If `cwd` has not
moved after typing the full name, poll `state` once more (a live `Cd` routes
through `jump()`; await-idle covers it, but re-poll if it looks stale) before
recording a bug.

### 05.10 — cd console Backspace with empty input walks to the parent
**Setup:** now inside `$FIXTURE/alpha dir` (from 05.9). Re-open the console:
`tmux send-keys -t $SESSION -l cd`; await-idle (assert `mode == "console"`,
overlay path line shows `$FIXTURE/alpha dir/`).
**Action:** `tmux send-keys -t $SESSION BSpace` (input is empty, so `del()`
walks up to the parent — `console.rs:275-282`); await-idle; `echo state | socat ...`.
**Expect (socket):** `mode == "console"` still; `cwd == $FIXTURE` (the console
`Cd`'d to the parent live). Then `tmux send-keys -t $SESSION Enter`; await-idle
→ `mode == "normal"`, `cwd == $FIXTURE`.
**Expect (screen):** overlay path line updates so its prefix is `$FIXTURE/`
after the Backspace (a dark-grey recommendation suffix, e.g. `$FIXTURE/alpha
dir`, follows it — assert only the `$FIXTURE/` prefix, not the recommended
subdir); after Enter the overlay is gone and the pane shows the `$FIXTURE`
listing.
**Note:** this is the console's own Backspace-at-empty behavior (parent walk),
distinct from search's Backspace (character delete). It is asserted via `cwd`,
not via input text.

---

## Phase 3 — the zoxide console (availability-gated)

### 05.11 — Enter the zoxide console; overlay + mode string
**Skip condition:** the `zoxide_query` binding is `CD`/`Cd`/`cD` (all contain a
capital letter). If tmux in this environment cannot deliver the SHIFT-modified
sequence reliably (see the section-header caveat), or if `zoxide` is not on
PATH, this phase degrades — see the per-step notes. Send the binding with
`tmux send-keys -t $SESSION -l CD` (uppercase literal); if `mode` does not
become `console`, record it as a protocol-feedback note (keybinding delivery),
not an rfm bug, and skip 05.12.
**Action:** `tmux send-keys -t $SESSION -l CD`; await-idle; `echo state | socat ...`;
capture-pane.
**Expect (socket):** `mode == "console"` (zoxide reports the SAME `"console"`
string as the cd console — `console.rs:585-587`).
**Expect (screen):** a centered overlay with divider bars; a middle input line
and, below it, a path/recommendation line. Two cases, gated on whether `zoxide`
is on PATH:
- **zoxide NOT installed:** the recommendation line reads exactly
  `zoxide is not installed` (in `red`) — shown immediately, before any keystroke
  (`Zoxide::with_availability`, `console.rs:378-395`).
- **zoxide INSTALLED (this env):** the middle line shows the query input line
  with **no** missing-zoxide hint. Assert the mode transition + overlay only;
  the "not installed" hint assertion is N/A and must not be expected.
**Note:** we deliberately do NOT type a query and commit a jump here — a real
zoxide `Cd` would move the pane to an arbitrary indexed directory
(unverifiable/host-dependent). Assert only the mode transition, the overlay,
and (if zoxide is missing) the hint string.

### 05.12 — Esc cancels the zoxide console back to start
**Action:** `tmux send-keys -t $SESSION Escape`; await-idle; `echo state | socat ...`.
**Expect (socket):** `mode == "normal"`; `cwd == $FIXTURE` (Esc =
`Cleanup::CdTo(starting_path)`, restoring the entry dir — `console.rs:524-528`,
and the unavailable branch `console.rs:500-502` uses the same cleanup).
**Expect (screen):** overlay gone; `$FIXTURE` listing restored.
**Note:** if zoxide IS installed, `cwd` must still be `$FIXTURE` after Esc even
if a live query had moved it — Esc always returns to `starting_path`. That is
the assertion; do not test a committed zoxide jump here.

---

**Teardown:** `tmux kill-session -t $SESSION 2>/dev/null; rm -rf "$PARENT"
"$CFG" "$CACHE" "$STATE" "$ZO"; rm -f $SOCK` (`$PARENT` wraps the quiet-parent `$FIXTURE`).

**Section coverage gaps (deliberate):**
- **Rename / mkdir / touch input modes** (`RenameMode`, `CreateItemMode` —
  `mode == "rename"`/`"mkdir"`/`"touch"`, footer prompts `Rename:`,
  `Make Directory:`, `Touch:`, the `C-u` bulk-delete, phantom preview) are
  fully covered in **section 04**; not duplicated here.
- The **trash view** modal (`gT`, `mode == "trash"`) is section 04.
- The **decision-flow overlay** (upgrade notice, `decision_flow.rs`) is only
  triggerable by a defaults-dropped/legacy-folded startup against a fresh
  `$XDG_STATE_HOME` — out of scope; belongs with a config-upgrade section.
- **Committed zoxide jumps** (typing a query and pressing Enter into an indexed
  directory) — host/zoxide-state dependent; only the mode transition, overlay,
  the missing-binary hint, and Esc-to-start are asserted (05.11–05.12). Seeding
  `_ZO_DATA_DIR` with `zoxide add` (per CLAUDE.md) and asserting a specific
  jump is deferred to the navigation section.
- **cd console Tab / BackTab recommendation cycling** (`console.rs:242-268`) —
  the recommendation completion UI is asserted only indirectly (the overlay is
  present); tab-completion cycling through `alpha dir`/`beta dir` is not
  exercised step-by-step (recommendation ordering is case-folded and
  prefix-dependent; a brittle screen assert). Noted for a future deep-dive.
- **Unmarking after a search commit:** search Enter marks matches and there is
  no in-section unmark; Phase 2/3 steps are written to not depend on `marked`.
  Toggling marks with Space is the marking section's concern.
- The `f` and `search` aliases for search, and the `Cd`/`cD` aliases for
  zoxide, are not each exercised — only the primary `/` and `CD` bindings —
  since they resolve to the identical `Command::Search`/`Command::Cd` path.
