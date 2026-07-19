# Multi-Tab & Split-View — Design

Status: accepted (2026-07-19)
Branch: `feature/multi-tab-structure`

## Motivation

rfm today is hard-wired to a single, static ranger-like Miller-columns
stack. Moving files *between* two directories means constantly navigating
back and forth in that one stack, which is tedious. We want a vifm-like
two-panel view — toggled with `!`, focus switched with `Tab` — so two
directories are visible side by side and copying between them is trivial.

We generalize this into **multi-tab support**: each tab is a complete,
independent Miller-columns stack (its own cwd, selection, navigation
history). The split view simply shows two tabs next to each other.

## Conceptual model

- **Tabs are the foundation.** There is always ≥1 tab. A tab owns a full
  Miller stack (`left`/`center`/`right`) plus its own navigation history.
- **Single view** shows the focused tab as the classic 3-column Miller
  layout (unchanged from today).
- **Split view** shows **two adjacent tabs** side by side, each rendered
  as **only its `center` column** (no parent, no preview — kept compact,
  vifm-like). `!` toggles split; on a single tab it auto-creates a second
  tab (clone of the current cwd) so the split always has two panels.
- **`Tab`** cycles focus through all tabs (`focused = (focused+1) % len`);
  the split window always contains the focused tab.
- **N tabs**, capped at `MAX_TABS = 4`. A constant cap keeps resource use
  bounded (fixed number of live panels/watchers); more than four side by
  side is unrealistic anyway.

## Architecture

### The `Tab` unit

The current `PanelManager` holds `left`/`center`/`right` **directly**,
plus the per-stack navigation history (`fwd_history`, `rev_history`,
`previous`). That block *is* one Miller stack. We extract it:

```rust
const MAX_TABS: usize = 4;

struct Tab {
    left:   ManagedPanel<DirPanel>,      // parent column (drawn only in Single view of the focused tab)
    center: ManagedPanel<DirPanel>,      // the cwd — always live & watched
    right:  ManagedPanel<PreviewPanel>,  // preview (drawn only in Single view of the focused tab)
    fwd_history: Vec<(PathBuf, PathBuf)>,
    rev_history: Vec<PathBuf>,
    previous: PathBuf,
}

pub struct PanelManager {
    tabs: Vec<Tab>,          // 1..=MAX_TABS, enforced by a guard
    focused: usize,
    view: ViewMode,          // Single | Split
    clipboard: Option<Clipboard>,  // global
    undo: UndoStack,               // global
    show_hidden: bool,             // global
    mode: Mode,
    // logger, parser, stdout, debug_* … unchanged
}

enum ViewMode { Single, Split }
```

**All tabs stay live** and are updated in the background (each `center`
keeps its FS watcher). This is required because split renders two tabs at
once, and it eliminates any hydrate/dehydrate boundary — no cursor
save/restore, no reload flicker on switch. The `MAX_TABS = 4` cap keeps
this bounded.

`cwd`/`selected` are read from `center.panel()` (path + selection), not
duplicated; only the navigation history is genuine per-tab state.

**Why not lightweight state (path + selection only)?** `left`/`right` are
*derived* from `(cwd, selected)` — exactly what `jump()` already does. A
lean model would materialize panels only for what's drawn. But split needs
both visible tabs live simultaneously, so we keep all tabs warm and bound
the count instead; simpler, no reload flicker.

**Efficiency detail:** `center` runs live for *all* tabs, but the
expensive preview (`right`, e.g. image decoding) is only driven for the
focused tab in Single view — driving it for invisible tabs would be pure
waste.

### Layout & rendering

No new `Layout` enum — that would just duplicate `view`. `draw_panels()`
branches on `self.view`:

- **Single:** focused tab → `left`/`center`/`right` into the existing
  `MillerColumns` ranges (unchanged from today).
- **Split:** the two visible tabs → each renders **only `center`** into a
  50/50 half, with a 1-column divider. Split half-ranges are computed
  inline from terminal size (a small helper analogous to
  `MillerColumns::from_size`, no enum).

**Which two tabs:** a 2-wide window `[base, base+1]` chosen so the focused
tab is contained (`base = focused`, unless focus is the last tab →
`base = focused-1`).

**Focus indication:** `DirPanel::draw` gains an `active: bool` parameter —
the active panel gets the bright cursor/title, the inactive one is dimmed.

**Tab indicator (ranger-style):** just a numbering (`1 2 3`, active
highlighted), right-aligned into the existing header row, shown only when
`tabs > 1`. No dedicated row, so `y_range` does not shrink. Draw sequence
stays footer → header (incl. tab numbers) → panels → console → log →
overlay.

**Header/footer** read from the *focused* tab (`self.center` →
`self.active().center`).

### Focus routing & new commands

Helper pair:

```rust
fn active(&self)     -> &Tab     { &self.tabs[self.focused] }
fn active_mut(&mut self) -> &mut Tab { &mut self.tabs[self.focused] }
```

Two categories:

- **In-tab logic → `impl Tab`.** Navigation (`move_up/down/left/right`,
  `jump`) and selection are self-contained (only the tab's own 3 panels +
  its history). Move them onto `Tab`; `PanelManager` delegates
  (`self.active_mut().move_right()`). De-clutters `manager.rs`, makes `Tab`
  unit-testable.
- **Effectful ops → stay in `PanelManager`, target the active tab.**
  Cut/Copy/Paste, rename/create/delete, zip/tar, search still touch global
  state (clipboard, undo, logger) but use `self.active()` as the
  source/destination directory instead of `center`.

**Borrow hazard (the real pitfall):** methods that mutate *both* the tab
and global state (e.g. paste reads `active().center.path()` then writes
`clipboard`/`undo`) must not hold the tab borrow across the global
mutation. Pattern: pull paths/data out first, drop the borrow, then act.
~a dozen call sites.

**New commands** (`src/engine/commands.rs`), with default keys:

| Command | Key | Behavior |
|---|---|---|
| `ToggleSplit` | `!` | Single↔Split; on 1 tab, auto-create a 2nd (clone of cwd) |
| `FocusNext` | `Tab` | `focused = (focused+1) % len`; split window follows focus |
| `NewTab` | `gn` | new tab in current cwd (up to `MAX_TABS`, else ignore + log) |
| `CloseTab` | `gc` | close focused tab; last remaining forces Single |
| `FocusTab(n)` | `1`–`4` | jump directly to tab n (ranger-style) |

**Modal modes** (search/rename/create/cd/trash) apply to the active tab
via `apply_mode_op`. Still only one mode active at a time; it targets the
focused tab.

**Copy-between-tabs (the payoff):** mark in tab A → `y` (global clipboard)
→ `Tab` to B → `p` pastes into B's cwd. Works because clipboard is global
and paste targets the *active* directory.

### Debug socket & state exposure

`StateSnapshot` becomes tab-aware (the tmux+socket test methodology
depends on it):

```rust
struct StateSnapshot {
    seq: u64,
    mode: String,
    view: String,            // "single" | "split"
    focused: usize,
    tabs: Vec<TabSnapshot>,
    clipboard: Option<ClipboardInfo>,
    show_hidden: bool,
    queue_active, queue_len, undo_depth, redo_depth,   // unchanged
}

struct TabSnapshot {
    cwd: PathBuf,
    selection: Option<String>,
    selected_idx: usize,
    total: usize,
    marked: Vec<PathBuf>,
}
```

**Compatibility:** mirror the *focused* tab's fields onto the top level
(`cwd`, `selection`, `selected_idx`, `total`, `marked`) so existing
assertions against `state.cwd` keep working and mean the active tab; new
tests use `tabs[i]`/`focused`/`view`.

**Socket `entries` command:** extend syntax with an optional tab index,
defaulting to the focused tab: `entries center` = active tab,
`entries 1 center` = tab 1's center column. Existing usage stays valid.

`await-idle`, `seq`, `log` unchanged.

## Global vs. per-tab state

| State | Scope | Rationale |
|---|---|---|
| clipboard | global | copy between tabs (the core use case) |
| undo/redo | global | one timeline across tabs; undo is focus-independent (path-based) |
| show_hidden | **global** | toggle affects all tabs; per-tab is needless complexity |
| mode | global (1 active) | targets the focused tab |
| marked | **per tab** (per `DirPanel`) | Cut/Copy collects from the active tab → clipboard |

## Edge cases & rules

- `CloseTab` on the focus → focus moves to a neighbor (clamped). The last
  remaining tab forces `Single`; never 0 tabs.
- `NewTab` at `MAX_TABS` → ignore + log line.
- `!` in Split → back to Single. `!` in Single with 1 tab → create 2nd tab
  (clone cwd) + split. With ≥2 tabs → just split (focused + neighbor).
- Split window when focus is the last tab → `base = focused-1`.
- **Terminal too narrow for split** (e.g. < ~40 columns) → refuse split,
  stay Single + log; avoids unreadable 2×center.
- Externally deleted cwd of a tab → existing `no_access`/parent fallback
  applies per tab, unchanged.

## Testing strategy

- **`impl Tab` as a pure state machine** — unit-test `move_left/right/jump`,
  selection, history, terminal-free (like `src/undo/` and the mode
  adapters).
- **Tab management** (add/close/focus/toggle-split) is mostly index math →
  also unit-testable without a terminal.
- **Integration** via tmux + the extended socket: `!` → `view==split`,
  `Tab` → `focused` changes, copy-between-tabs → `tabs[1].cwd` contains the
  file, narrow-terminal fallback.
- `cargo build` / `clippy` / `cargo test` as the gate.

## Migration

The new keys (`!`, `Tab`, `gn`, `gc`, `1`–`4`) are opt-in in `keys.toml`
— like `undo`/`redo`, they appear in existing user configs only when
added. The shipped default `keys.toml` gets them.
