# Multi-Tab & Split-View Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or
> superpowers:subagent-driven-development) to implement this plan
> task-by-task.

**Goal:** Add multi-tab support (each tab = a full Miller stack) and a
vifm-like split view (two adjacent tabs, center-column only), toggled with
`!`, focus cycled with `Tab`.

**Architecture:** Extract the current `left`/`center`/`right` + navigation
history out of `PanelManager` into a reusable `Tab`. `PanelManager` holds
`tabs: Vec<Tab>` (capped at `MAX_TABS = 4`, all live/watched), a `focused`
index, and a `view: ViewMode`. All operations route through
`active()`/`active_mut()`. Rendering branches on `view`. Global state
(clipboard, undo, show_hidden, mode) is shared across tabs.

**Tech Stack:** Rust, crossterm, tokio, notify (FS watchers), serde
(debug socket).

**Design doc:** `docs/plans/2026-07-19-multi-tab-split-view-design.md`

**Strategy note — keep the build green.** The refactor is behavior-
preserving until Phase 3. Phases 1–2 wrap the existing single stack in a
`Tab` with exactly one tab; existing tests + a tmux smoke test must keep
passing before any multi-tab behavior is added. Terminal-coupled parts
(rendering) are verified with the tmux + debug-socket harness described in
`CLAUDE.md`; pure logic (`Tab` navigation, tab-index math, snapshot shape,
command parsing) is unit-tested TDD-first.

**Per-task rule:** every task ends with `cargo build`, `cargo test`,
`cargo clippy` all green, then a commit. Do not proceed to the next task
on a red build.

---

## Phase 1 — Extract the `Tab` unit (behavior-preserving, single tab)

### Task 1: Introduce the `Tab` struct and route through `active()`

**Files:**
- Modify: `src/panel/manager.rs` (struct `PanelManager` ~79–159; all
  methods referencing `self.left`/`self.center`/`self.right`/
  `self.fwd_history`/`self.rev_history`/`self.previous`)
- Modify: `src/panel/mod.rs` if `Tab` needs `pub(crate)` visibility of
  `ManagedPanel` (already in the module).

**Step 1: Define `Tab` and the constant.** In `manager.rs`, above
`PanelManager`:

```rust
const MAX_TABS: usize = 4;

/// One independent Miller-columns stack: its own cwd, selection and
/// navigation history. Tabs are the unit the split view shows two of.
struct Tab {
    left: ManagedPanel<DirPanel>,
    center: ManagedPanel<DirPanel>,
    right: ManagedPanel<PreviewPanel>,
    fwd_history: Vec<(PathBuf, PathBuf)>,
    rev_history: Vec<PathBuf>,
    previous: PathBuf,
}
```

**Step 2: Move the fields into `Tab`.** Replace the six fields in
`PanelManager` (`left`, `center`, `right`, `fwd_history`, `rev_history`,
`previous`) with:

```rust
tabs: Vec<Tab>,
focused: usize,
```

**Step 3: Add the accessors.** In `impl PanelManager`:

```rust
fn active(&self) -> &Tab { &self.tabs[self.focused] }
fn active_mut(&mut self) -> &mut Tab { &mut self.tabs[self.focused] }
```

**Step 4: Reroute every reference.** Mechanically replace across
`manager.rs`:
- `self.left`   → `self.active().left`   / `self.active_mut().left`
- `self.center` → `self.active().center` / `self.active_mut().center`
- `self.right`  → `self.active().right`  / `self.active_mut().right`
- `self.fwd_history` / `self.rev_history` / `self.previous` →
  `self.active_mut().<field>`

**Borrow hazard:** where a method reads a path from the tab and then
mutates global state (`self.clipboard`, `self.undo`, `self.logger`) or
another tab, first bind the needed `PathBuf`/data to a local, let the
`active()` borrow drop, then mutate. Watch especially: paste, cut/copy,
rename, create, delete, zip/tar, and `state_snapshot`.

**Step 5: Fix construction.** Where `PanelManager` is built (the `new`/
constructor in `manager.rs`, and `src/main.rs` if it wires panels), build
one `Tab { left, center, right, fwd_history: vec![], rev_history: vec![],
previous }` and set `tabs: vec![tab], focused: 0`.

**Step 6: Build + existing tests.**
Run: `cargo build && cargo test && cargo clippy`
Expected: green. No behavior change — one tab only.

**Step 7: tmux smoke test.** Per `CLAUDE.md`: launch on a fixture, `j`,
`await-idle`, `state`, `capture-pane`. Confirm navigation, selection,
preview identical to before.

**Step 8: Commit.**
```bash
git add -A
git commit -m "refactor(panel): extract Tab unit; route ops through active()"
```

---

### Task 2: Move navigation onto `impl Tab` + unit tests

**Files:**
- Modify: `src/panel/manager.rs` (`move_up`/`move_down`/`move_left`/
  `move_right`/`jump` and helpers that only touch one stack)
- Test: inline `#[cfg(test)] mod tests` in `manager.rs` (or a new
  `src/panel/tab.rs` if `Tab` is split out — keep it in `manager.rs` for
  now to avoid a module shuffle).

**Step 1: Write failing unit tests for `Tab` navigation.** Construct a
`Tab` over a tempdir fixture and assert cursor/history behavior. Example:

```rust
#[test]
fn move_right_into_dir_pushes_history() {
    let tab = Tab::for_test(&fixture_with_subdir());
    // select the subdir, move_right, assert center.path() == subdir
    // and fwd_history has one entry
}
```

(If constructing `ManagedPanel` in a unit test is impractical because of
the content-manager channel, keep the navigation methods on `Tab` but
unit-test only the *history index math* by factoring it into a small pure
helper. Prefer a real fixture if the channel can be stubbed cheaply; do
NOT invent a fake that hides bugs.)

**Step 2: Run — expect FAIL (methods not on `Tab` yet).**
Run: `cargo test tab_`
Expected: FAIL / does not compile.

**Step 3: Move the methods.** Relocate `move_up/down/left/right/jump`
bodies into `impl Tab`, changing `self.left/center/right/*_history` →
`self.left/...` (now fields of `Tab`). In `PanelManager`, replace the
bodies with delegation: `self.active_mut().move_right()` etc.

**Step 4: Run — expect PASS.** `cargo test`
Expected: PASS (new + existing).

**Step 5: clippy + build + tmux smoke.** Navigation unchanged.

**Step 6: Commit.**
```bash
git commit -am "refactor(panel): move navigation onto impl Tab; unit-test it"
```

---

## Phase 2 — Tab management (multi-tab, still single view)

### Task 3: `ViewMode`, tab add/close/focus with unit tests

**Files:**
- Modify: `src/panel/manager.rs`
- Test: inline tests in `manager.rs`

**Step 1: Add the view enum + field.**
```rust
enum ViewMode { Single, Split }
```
Add `view: ViewMode` to `PanelManager`, init `ViewMode::Single`.

**Step 2: Write failing tests for index math.** Test the pure helpers
(they must not require a terminal):
```rust
#[test] fn focus_next_wraps() { /* focused 1 of 2 -> 0 */ }
#[test] fn close_focus_clamps_and_never_empties() { /* 2 tabs, close -> 1 tab, focused 0 */ }
#[test] fn new_tab_respects_cap() { /* at MAX_TABS, add is a no-op */ }
```
Structure these so the logic under test is a small method taking
`&mut self` and touching only `self.tabs`/`self.focused`; if building real
`Tab`s in tests is too heavy, extract the index decisions into pure
functions `fn next_focus(focused, len) -> usize` and
`fn focus_after_close(focused, len) -> usize` and test those directly.

**Step 3: Run — expect FAIL.** `cargo test focus_ new_tab_`

**Step 4: Implement the operations.**
```rust
fn new_tab(&mut self) {
    if self.tabs.len() >= MAX_TABS {
        warn!("max {MAX_TABS} tabs reached");
        return;
    }
    let cwd = self.active().center.panel().path().to_path_buf();
    self.tabs.push(Tab::new_at(&cwd, /* content manager handles */));
    self.focused = self.tabs.len() - 1;
    self.mark_dirty();
}

fn close_tab(&mut self) {
    if self.tabs.len() == 1 { return; }          // never 0 tabs
    self.tabs.remove(self.focused);
    self.focused = self.focused.min(self.tabs.len() - 1);
    if self.tabs.len() == 1 { self.view = ViewMode::Single; }
    self.mark_dirty();
}

fn focus_next(&mut self) {
    self.focused = (self.focused + 1) % self.tabs.len();
    self.mark_dirty();
}

fn focus_tab(&mut self, n: usize) {
    if n < self.tabs.len() { self.focused = n; self.mark_dirty(); }
}
```
`Tab::new_at(&cwd)` mirrors how `jump()` builds a stack: `left =
cwd.parent()`, `center = cwd`, `right = center.selected`. Reuse the same
`ManagedPanel` construction as the constructor in Task 1.

**Step 5: Run — expect PASS.** `cargo test`

**Step 6: Commit.**
```bash
git commit -am "feat(panel): tab add/close/focus with MAX_TABS cap"
```

---

### Task 4: Wire the new commands + default keys

**Files:**
- Modify: `src/engine/commands.rs` (the `Command` enum + parser/keymap)
- Modify: `src/panel/manager.rs` (dispatch in the normal-mode command match)
- Modify: `examples/keys.toml` (ship defaults)

**Step 1: Add command variants.** In `commands.rs` `Command`:
```rust
ToggleSplit,
FocusNext,
NewTab,
CloseTab,
FocusTab(usize),
```
Wire the config parsing exactly like existing opt-in commands (follow how
`undo`/`redo` were added — grep `Command::Undo` in `commands.rs`). Map key
strings: `"toggle_split"`, `"focus_next"`, `"new_tab"`, `"close_tab"`,
`"focus_tab_1".."focus_tab_4"` (or the project's existing convention —
match it).

**Step 2: Add default bindings to `examples/keys.toml`:**
```toml
toggle_split = "!"
focus_next   = "Tab"
new_tab      = "gn"
close_tab    = "gc"
focus_tab_1  = "1"
focus_tab_2  = "2"
focus_tab_3  = "3"
focus_tab_4  = "4"
```
(Use the exact key syntax the file already uses for chords like `gg`.)

**Step 3: Dispatch in `manager.rs`** normal-mode match:
```rust
Command::FocusNext => self.focus_next(),
Command::NewTab    => self.new_tab(),
Command::CloseTab  => self.close_tab(),
Command::FocusTab(n) => self.focus_tab(n.saturating_sub(1)),
Command::ToggleSplit => self.toggle_split(),   // stub until Task 7
```
Add a temporary `fn toggle_split(&mut self) {}` stub so it compiles;
implemented in Task 7.

**Step 4: Build + clippy.** `cargo build && cargo clippy`

**Step 5: tmux integration — tab management.** Launch with a scratch
`--config` whose `keys.toml` has the bindings. Send `gn`, `await-idle`,
`state` → assert `tabs.len()==2` once the socket exposes it (Task 5); for
now assert via `capture-pane` that a second tab exists / focus moved. Send
`1`/`2` and `Tab`; confirm focus changes.

**Step 6: Commit.**
```bash
git commit -am "feat(keys): commands for new/close/focus tab; default bindings"
```

---

## Phase 3 — Debug socket: make state tab-aware (so split is testable)

### Task 5: Tab-aware `StateSnapshot` + `entries <tab>`

**Files:**
- Modify: `src/debug.rs` (`StateSnapshot`, `PaneId`, `DebugRequest`,
  `parse_command`, the `#[cfg(test)]` fixtures)
- Modify: `src/panel/manager.rs` (`state_snapshot`, entries handler)

**Step 1: Write failing tests in `debug.rs`.**
- `parse_command("entries 1 center")` → `Entries { tab: Some(1), pane:
  Center }`; `entries center` → `tab: None`.
- `snapshot_serializes_to_json` updated to include `"view"`, `"focused"`,
  and a `"tabs"` array, while still exposing top-level `"cwd"`.

**Step 2: Run — expect FAIL.** `cargo test -p rfm debug` (or `cargo test
parses_ snapshot_`).

**Step 3: Extend the types.**
```rust
#[derive(Debug, Clone, Serialize)]
pub struct TabSnapshot {
    pub cwd: PathBuf,
    pub selection: Option<String>,
    pub selected_idx: usize,
    pub total: usize,
    pub marked: Vec<PathBuf>,
}
```
Add to `StateSnapshot`: `pub view: String`, `pub focused: usize`,
`pub tabs: Vec<TabSnapshot>`. **Keep** the existing top-level fields
(`cwd`, `selection`, `selected_idx`, `total`, `marked`, `left_path`,
`preview_path`) — they now mirror the focused tab for backward compat.

Extend `parse_command`:
```rust
("entries", Some("left"))   => Entries { tab: None, pane: Left },
("entries", Some("center")) => Entries { tab: None, pane: Center },
("entries", Some(idx)) => {
    let t: usize = idx.parse().ok()?;
    match words.next() {
        Some("left")   => Entries { tab: Some(t), pane: Left },
        Some("center") => Entries { tab: Some(t), pane: Center },
        _ => return None,
    }
}
```
Update `DebugRequest::Entries` and `DebugCommand::Entries` to carry
`tab: Option<usize>`.

**Step 4: Build `state_snapshot` from tabs.** In `manager.rs`, loop over
`self.tabs` to fill `tabs: Vec<TabSnapshot>`, set `focused`, `view`
(`"single"`/`"split"`), and mirror `self.tabs[self.focused]` onto the
top-level fields. Entries handler: resolve `tab.unwrap_or(self.focused)`,
bounds-check, read that tab's `left`/`center`.

**Step 5: Run — expect PASS.** `cargo test`

**Step 6: Commit.**
```bash
git commit -am "feat(debug): tab-aware StateSnapshot and 'entries <tab>'"
```

---

## Phase 4 — Split rendering

### Task 6: Split layout, active highlighting, tab numbers

**Files:**
- Modify: `src/panel/manager.rs` (`draw_panels`, `draw_header`,
  layout usage)
- Modify: `src/panel/mod.rs` (split half-range helper near `MillerColumns`)
- Modify: `src/panel/directory.rs` (`DirPanel::draw` gains `active: bool`)

**Step 1: Add the split-range helper** in `mod.rs`:
```rust
/// Two equal halves for split view, with a 1-column gap between them.
/// Returns None if the terminal is too narrow to be usable.
fn split_columns(size: (u16, u16)) -> Option<(Range<u16>, Range<u16>, Range<u16>)> {
    let (sx, sy) = size;
    if sx < 40 { return None; }           // narrow-terminal guard
    let mid = sx / 2;
    let left  = 0..mid.saturating_sub(0);
    let right = (mid + 1)..sx;            // +1 leaves a divider column
    let y = 1..sy.saturating_sub(1);
    Some((left, right, y))
}
```

**Step 2: Add `active: bool` to `DirPanel::draw`.** Thread it through so
the cursor/title uses the bright style when `active`, dimmed otherwise.
Update all call sites (single view passes `true` for center).

**Step 3: Branch `draw_panels` on `self.view`.**
- `Single`: draw `active()` `left`(false)/`center`(true)/`right` in the
  Miller ranges (unchanged).
- `Split`: compute the two visible tab indices `[base, base+1]` (`base =
  focused` unless `focused == tabs.len()-1` → `base = focused-1`); for each
  draw ONLY its `center` into its half, `active = (idx == focused)`; draw a
  divider column. If `split_columns` returns `None`, fall back to Single +
  `warn!`.

**Step 4: Tab numbers in the header.** In `draw_header`, when
`self.tabs.len() > 1`, render `1 2 3 …` right-aligned, the `focused` index
highlighted. Only numbers, no paths (ranger-style).

**Step 5: Build + clippy.**

**Step 6: tmux integration.** This is the key visual verification. Set up
two tabs (`gn`), force split (temporarily call `toggle_split` = set
`view = Split` — or land Task 7 first and reorder). Assert via socket:
`state.view == "split"`, and `entries 0 center` vs `entries 1 center`
differ. `capture-pane` shows two center columns + tab numbers + one active
(bright) one dimmed. Test a narrow pane (`-x 30`) → stays single + log.

**Step 7: Commit.**
```bash
git commit -am "feat(panel): split-view rendering, active highlight, tab numbers"
```

---

### Task 7: Implement `ToggleSplit` (`!`) with auto-second-tab

**Files:**
- Modify: `src/panel/manager.rs` (`toggle_split`)

**Step 1: Implement.**
```rust
fn toggle_split(&mut self) {
    match self.view {
        ViewMode::Split => { self.view = ViewMode::Single; }
        ViewMode::Single => {
            if self.tabs.len() < 2 {
                self.new_tab();                 // clone of cwd; sets focused = last
            }
            // guard: only enter split if the terminal is wide enough
            if split_columns(self.terminal_size()).is_some() {
                self.view = ViewMode::Split;
            } else {
                warn!("terminal too narrow for split view");
            }
        }
    }
    self.mark_dirty();
}
```
(`terminal_size()` — reuse whatever `draw` already uses to get `(sx, sy)`;
if it lives only in `MillerColumns`, add a small getter.)

**Step 2: Build + clippy.**

**Step 3: tmux integration — the full payoff flow.** On a fixture with
`src/` and `dst/` dirs:
1. Navigate tab 0 into `src/`, `!` → split, second tab clones cwd.
2. `Tab` to focus the other panel, navigate it into `dst/`.
3. In `src` panel: mark a file (`Space`), `y` (copy).
4. `Tab` to `dst` panel, `p` (paste), `await-idle`.
5. `state` → assert `tabs[<dst>].cwd` lists the pasted file (via
   `entries <dst> center`).
6. `!` again → back to single.

**Step 4: Commit.**
```bash
git commit -am "feat(panel): '!' toggles split, auto-creates second tab"
```

---

## Phase 5 — Polish & docs

### Task 8: Preview-drive only for focused tab; final sweep

**Files:**
- Modify: `src/panel/manager.rs` (wherever `right` / preview updates are
  triggered)

**Step 1:** Ensure the `right` (preview) panel is only *driven*
(`new_panel_delayed`/`reload`) for the focused tab in single view — skip
preview work for background/split tabs (they never draw it). Confirm no
regression to single-view preview via tmux.

**Step 2: Full regression.** `cargo test && cargo clippy && cargo build`.
Re-run the Task 1 and Task 7 tmux flows end to end.

**Step 3: Update `CLAUDE.md`.** Add a short "Architecture: tabs & split
view" section (Tab unit, ViewMode, MAX_TABS, `active()`, socket `view`/
`tabs`/`entries <n>`). Note the new keys are opt-in in `keys.toml`.

**Step 4: Commit.**
```bash
git commit -am "docs(claude): tabs & split-view architecture; preview-drive polish"
```

---

## Verification checklist (end state)

- [ ] `cargo build`, `cargo test`, `cargo clippy` clean.
- [ ] Single view identical to pre-refactor (smoke).
- [ ] `gn`/`gc`/`1`–`4`/`Tab` manage/focus tabs; cap at 4; never 0.
- [ ] `!` toggles split, auto-creates a 2nd tab, refuses on narrow term.
- [ ] Split shows two center columns, one active (bright) one dimmed,
      divider between, tab numbers in header.
- [ ] Copy-between-tabs: mark in A, `y`, `Tab`, `p` → file appears in B.
- [ ] Socket: `state.view`/`focused`/`tabs[]` correct; `entries <n>
      center` works; legacy top-level `cwd` mirrors focused tab.
- [ ] Global vs per-tab per the design table (clipboard/undo/show_hidden
      global; marked per-tab).
```
