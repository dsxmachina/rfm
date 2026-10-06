# Generic Decision-Flow Overlay Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A reusable overlay mode presenting a list of decision items with
keyed choices; first consumer is the config upgrade/conflict notice.

**Architecture:** A pure `ModalInput` adapter (`decision_flow.rs`, modeled on
`trash_view.rs`) emits two new `ModeOp` variants; `PanelManager::apply_mode_op`
dispatches per `FlowKind`. "Shown once" state lives in
`$XDG_STATE_HOME/rfm/state.toml`. Design: `2026-07-30-decision-flow-design.md`.

**Tech Stack:** Rust, crossterm `KeyEvent`/`KeyCode`, existing mode seam
(`src/panel/mode/mod.rs`: `ModalInput` trait at :109-120, `ModeOp` at :57-83,
`ModalRegion::ConsoleOverlay` drawn via `manager.rs:970-981`), `toml` for the
state file.

**DEPENDS ON:** the config-unification plan (`2026-07-30-config-unification.md`)
being fully implemented — this plan consumes `CommandParser::build`'s
`Vec<DroppedDefault>` and `LoadedConfig::legacy_folded`.

**Verification per task:** `cargo test` + `cargo clippy`; tmux + debug-socket
integration in the final task.

---

### Task 1: `DecisionFlow` data model + core state machine

**Files:**
- Create: `src/panel/mode/decision_flow.rs`
- Modify: `src/panel/mode/mod.rs` (add `pub mod decision_flow;` next to the
  existing mode modules)

**Step 1: Write the failing tests**

The adapter is terminal-free; tests drive `handle_key` directly, mirroring
`trash_view.rs:273-383` (reuse its `fn key(code) -> KeyEvent` helper idiom).

```rust
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    UpgradeNotice,
    // future: Onboarding, BulkRenamePreview, ...
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub key: char,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct DecisionItem {
    pub prompt: String,
    pub detail: Vec<String>,
    pub choices: Vec<Choice>,     // 2..=4 enforced by constructor
    pub default: Option<usize>,   // preselected choice; None = must answer
}

pub struct DecisionFlow {
    kind: FlowKind,
    title: String,
    items: Vec<DecisionItem>,
    cursor: usize,
    answers: Vec<Option<usize>>,
}
```

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent { KeyEvent::new(code, KeyModifiers::NONE) }

    fn two_items() -> DecisionFlow {
        DecisionFlow::new(
            FlowKind::UpgradeNotice,
            "test".into(),
            vec![
                DecisionItem {
                    prompt: "first".into(), detail: vec![],
                    choices: vec![
                        Choice { key: 'k', label: "keep".into() },
                        Choice { key: 'a', label: "adopt".into() },
                    ],
                    default: Some(0),
                },
                DecisionItem {
                    prompt: "second".into(), detail: vec![],
                    choices: vec![
                        Choice { key: 'y', label: "yes".into() },
                        Choice { key: 'n', label: "no".into() },
                    ],
                    default: Some(1),
                },
            ],
        )
    }

    #[test]
    fn j_k_move_and_clamp_cursor() {
        let mut f = two_items();
        assert_eq!(f.cursor(), 0);
        f.handle_key(key(KeyCode::Char('j')));
        assert_eq!(f.cursor(), 1);
        f.handle_key(key(KeyCode::Char('j')));   // clamp at end
        assert_eq!(f.cursor(), 1);
        f.handle_key(key(KeyCode::Char('k')));
        f.handle_key(key(KeyCode::Char('k')));   // clamp at start
        assert_eq!(f.cursor(), 0);
    }

    #[test]
    fn choice_key_answers_and_auto_advances() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Char('a')));
        assert!(matches!(op, ModeOp::None));      // answering never completes early
        assert_eq!(f.answers()[0], Some(1));
        assert_eq!(f.cursor(), 1);                // advanced to next unanswered
    }

    #[test]
    fn enter_accepts_default_of_current_item() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Enter));
        assert_eq!(f.answers()[0], Some(0));
    }

    #[test]
    fn answering_last_item_resolves_flow() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('k')));
        let op = f.handle_key(key(KeyCode::Char('n')));
        match op {
            ModeOp::FlowResolved { kind, answers } => {
                assert_eq!(kind, FlowKind::UpgradeNotice);
                assert_eq!(answers, vec![0, 1]);
            }
            other => panic!("expected FlowResolved, got {other:?}"),
        }
    }

    #[test]
    fn esc_with_all_defaults_resolves_with_defaults() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Esc));
        assert!(matches!(op, ModeOp::FlowResolved { answers, .. } if answers == vec![0, 1]));
    }

    #[test]
    fn esc_without_default_aborts() {
        let mut f = two_items();
        f.items_mut_for_test()[1].default = None;   // #[cfg(test)] accessor
        let op = f.handle_key(key(KeyCode::Esc));
        assert!(matches!(op, ModeOp::FlowAborted { kind: FlowKind::UpgradeNotice }));
    }

    #[test]
    fn wrong_key_is_noop() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Char('z')));
        assert!(matches!(op, ModeOp::None));
        assert_eq!(f.answers()[0], None);
    }

    #[test]
    fn answered_item_can_be_revisited_and_changed() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('a')));    // answer item 0, cursor → 1
        f.handle_key(key(KeyCode::Char('k')));    // 'k' is not a choice of item 1 → noop
        f.handle_key(key(KeyCode::Char('k'))); // still noop; go back instead:
        f.handle_key(key(KeyCode::Char('k'))); // (kept noop on purpose — see below)
        f.handle_key(key(KeyCode::Up));
        f.handle_key(key(KeyCode::Char('k')));    // re-answer item 0
        assert_eq!(f.answers()[0], Some(0));
    }
}
```

Note the deliberate rule the tests pin down: **choice keys act on the
*current* item only** — a keypress that is no item's navigation key and not a
choice key of the current item is a no-op (never "helpfully" answers another
item). Choice-key/navigation collisions ('k' as both a choice and cursor-up):
**choice keys win on the current item**; builders must avoid 'j'/'q' as
choice keys (constructor `debug_assert!`s this; arrow keys always navigate).

**Step 2: Run to verify failure**

Run: `cargo test decision_flow -- --nocapture`
Expected: compile FAIL (module missing) — that counts; then test FAIL after
scaffolding.

**Step 3: Implement**

`DecisionFlow::new` asserts non-empty items and 2..=4 choices each.
`handle_key` (pattern-match like `trash_view.rs:238-262`):

- `j`/`Down` → cursor+1 (clamp); `k`/`Up` → cursor−1 (clamp) — **after**
  checking whether the char is a choice key of the current item.
- char matching `items[cursor].choices[i].key` → `answers[cursor] = Some(i)`;
  advance cursor to the next unanswered item (wrapping search from
  cursor+1); if none unanswered remain → emit
  `ModeOp::FlowResolved { kind, answers: unwrap all }`.
- `Enter` → if `items[cursor].default` is `Some(d)`, same as pressing that
  choice's key; else no-op.
- `Esc`/`q` → if every unanswered item has a default: fill defaults, emit
  `FlowResolved`; else emit `FlowAborted { kind }`.
- Everything else → `ModeOp::None`.

Accessors used by tests (`cursor()`, `answers()`,
`#[cfg(test)] items_mut_for_test()`).

`ModeOp` gains the two variants in `src/panel/mode/mod.rs:57-83` (with doc
comments in the file's style):

```rust
/// A decision flow answered every item; the manager dispatches on `kind`.
FlowResolved { kind: FlowKind, answers: Vec<usize> },
/// A decision flow was abandoned with unanswerable items pending.
FlowAborted { kind: FlowKind },
```

Making `ModeOp` `Debug` (the tests print it) — add `#[derive(Debug)]` if not
present; `trash::TrashItem` in `RestoreFromTrash` is Debug, so this derives
cleanly. Stub `Draw`/`ModalInput` impls so it compiles:
`region() → ModalRegion::ConsoleOverlay`, `name() → "decision-flow"`,
`draw()` minimal (Task 3 finishes it).

**Step 4:** `cargo test decision_flow` → all PASS; full `cargo test` green
(the new ModeOp variants hit no existing exhaustive matches yet — `apply_mode_op`
gets its arms in Task 2; until then add temporary `_ => {}`? **No** — the
match at `manager.rs:1705-1780` is exhaustive: add the two arms now as
`ModeOp::FlowResolved { .. } | ModeOp::FlowAborted { .. } => { self.mode = Mode::Normal; }`
placeholder, replaced in Task 2). clippy clean.

**Step 5: Commit** — `feat(mode): decision-flow adapter core state machine`

---

### Task 2: apply-to-all + manager dispatch arms

**Files:**
- Modify: `src/panel/mode/decision_flow.rs`
- Modify: `src/panel/manager.rs:1705-1780` (`apply_mode_op`)

**Step 1: Failing tests** (adapter side)

```rust
#[test]
fn apply_to_all_answers_remaining_items_with_same_choice_set() {
    // three items; #0 and #2 share a choice set, #1 differs
    let mut f = three_items_two_kinds();
    f.handle_key(key(KeyCode::Char('a')));        // answer #0 with choice 1
    f.handle_key(key(KeyCode::Up));               // back to #0
    let op = f.handle_key(key(KeyCode::Char('A'))); // apply to all
    assert_eq!(f.answers()[2], Some(1));          // same-set item answered
    assert_eq!(f.answers()[1], None);             // different set untouched
    assert!(matches!(op, ModeOp::None));          // #1 still unanswered
}

#[test]
fn apply_to_all_can_resolve_the_flow() {
    let mut f = three_items_same_kind();          // all share one choice set
    f.handle_key(key(KeyCode::Char('a')));
    f.handle_key(key(KeyCode::Up));
    let op = f.handle_key(key(KeyCode::Char('A')));
    assert!(matches!(op, ModeOp::FlowResolved { .. }));
}

#[test]
fn apply_to_all_without_answer_on_current_is_noop() {
    let mut f = three_items_same_kind();
    let op = f.handle_key(key(KeyCode::Char('A')));
    assert!(matches!(op, ModeOp::None));
}
```

"Same choice set" = identical `(key, label)` sequences. `A` copies the
*current item's* answer to every other item with the same choice set whose
answer is `None`, then resolves if complete.

**Step 2:** FAIL.

**Step 3: Implement** the `'A'` arm + a `same_choice_set(a, b) -> bool`
helper. Then replace the Task-1 placeholder arms in `apply_mode_op`:

```rust
ModeOp::FlowResolved { kind, answers } => {
    self.mode = Mode::Normal;
    self.resolve_flow(kind, answers);   // per-kind dispatch, Task 5 fills UpgradeNotice
}
ModeOp::FlowAborted { kind } => {
    self.mode = Mode::Normal;
    info!("{kind:?} dismissed");
}
```

`resolve_flow` starts as a `match kind` with a logging stub. Follow the
existing arm style (`FinishSearch` at :1714-1720 for the mode reset +
`mark_dirty` at :1779 already covers redraw).

**Step 4:** `cargo test` + clippy.

**Step 5: Commit** — `feat(mode): apply-to-all and manager dispatch for decision flows`

---

### Task 3: overlay rendering

**Files:**
- Modify: `src/panel/mode/decision_flow.rs` (`Draw` impl)

No unit tests (the codebase does not unit-test `draw()` — TrashView's isn't
either); verified in Task 6's tmux pass. Model directly on
`trash_view.rs:80-234`:

- `region() == ModalRegion::ConsoleOverlay` → `draw_console`
  (`manager.rs:970-981`) hands the full panel x-range and y-range; compute a
  centered band exactly like `trash_view.rs` (`band_top = y_range.start +
  (avail_h - band) / 2`), width = x-range minus 2-cell margin.
- Layout, top to bottom:
  1. hint row: `j/k move · <choice keys> answer · A all · Enter default · Esc done` (centered, like the TrashView hint row)
  2. top bar (reuse `print_horizontal_bar` etc. from `config::color`)
  3. title line (bold, `color_main`)
  4. item window (scroll window arithmetic copied from TrashView's windowed
     rows): per item — cursor marker + prompt; under the cursor item also its
     `detail` lines (dimmed) and one choices row: `[k] keep yours   [a] adopt new`,
     the chosen/default one highlighted with `color_highlight`; answered
     non-cursor items show `✓ <chosen label>` right-aligned (`color_marked`).
  5. bottom bar.
- Truncate with `unicode_display_width` as TrashView does; never wrap.

**Step: verify visually** — temporary: launch with a hacked-in test flow
behind `--debug-socket` (or wait for Task 5 and verify then; acceptable to
defer visual check to Task 6). `cargo build` + clippy must pass.

**Commit** — `feat(mode): render decision-flow overlay`

---

### Task 4: `xdg_state_home` + seen-version state file

**Files:**
- Modify: `src/util.rs` (beside `xdg_config_home`, :376-386)
- Create: `src/config/app_state.rs` (`pub mod app_state;` in `config/mod.rs`)

**Step 1: Failing tests** (path-parameterized — no env mutation in tests;
only the thin resolver touches env)

```rust
// util.rs — mirror of xdg_config_home:
/// $XDG_STATE_HOME, falling back to ~/.local/state
pub fn xdg_state_home() -> anyhow::Result<PathBuf> { ... }
```

```rust
// app_state.rs
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub upgrade_notice_seen_for: Option<String>,  // rfm version string
}

pub fn read(state_dir: &Path) -> AppState { ... }          // missing/broken → Default
pub fn write(state_dir: &Path, state: &AppState) -> anyhow::Result<()> { ... } // creates dir

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).upgrade_notice_seen_for.is_none());
    }

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = AppState { upgrade_notice_seen_for: Some("0.5.0".into()) };
        write(dir.path(), &s).unwrap();
        assert_eq!(read(dir.path()).upgrade_notice_seen_for.as_deref(), Some("0.5.0"));
    }

    #[test]
    fn broken_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.toml"), "not = [valid").unwrap();
        assert!(read(dir.path()).upgrade_notice_seen_for.is_none());
    }
}
```

**Step 2:** FAIL. **Step 3:** implement (file = `state_dir/state.toml`;
`write` uses `create_dir_all`; failures in the *caller* degrade to a `warn!`,
never an error — the design's "may ask again next start" rule).

**Step 4:** PASS + clippy. **Step 5: Commit** —
`feat(state): xdg_state_home and rfm state.toml`

---

### Task 5: the upgrade-notice consumer

**Files:**
- Create: builder + resolver in `src/panel/manager.rs` (flow construction is
  manager-side per the design; keep it in a small `impl PanelManager` block
  near `apply_mode_op`)
- Modify: `src/main.rs` (trigger call after `PanelManager::new`, before the
  event loop)

**Step 1: Failing tests** (pure parts only — flow construction from inputs)

Builder is a free function so it tests without a manager:

```rust
pub fn build_upgrade_notice(
    dropped: &[DroppedDefault],
    legacy_folded: bool,
) -> Option<DecisionFlow> { ... }
```

```rust
#[test]
fn no_inputs_no_flow() {
    assert!(build_upgrade_notice(&[], false).is_none());
}

#[test]
fn dropped_default_becomes_keep_or_adopt_item_with_keep_default() {
    let d = DroppedDefault { command: "close tab".into(), binding: "q".into(), kept: "quit".into() };
    let flow = build_upgrade_notice(&[d], false).unwrap();
    let item = &flow.items()[0];
    assert!(item.prompt.contains('q') && item.prompt.contains("close tab"));
    assert_eq!(item.default, Some(0));                       // keep yours
    assert_eq!(item.choices[0].key, 'y');                    // keep *y*ours
    assert_eq!(item.choices[1].key, 'a');                    // *a*dopt
}

#[test]
fn legacy_fold_appends_migrate_item_defaulting_to_not_now() {
    let flow = build_upgrade_notice(&[], true).unwrap();
    let last = flow.items().last().unwrap();
    assert!(last.prompt.contains("migrate"));
    assert_eq!(last.choices[last.default.unwrap()].label.to_lowercase(), "not now");
}
```

**Step 2:** FAIL. **Step 3: Implement**

- Builder: one item per `DroppedDefault` —
  prompt `` default `q` → close tab conflicts with your `quit` ``,
  detail = the would-be config lines; choices `[y] keep yours` /
  `[a] adopt new (shows the lines to change)`; default = keep. Plus, if
  `legacy_folded`, the final migrate item (`[m] migrate now` /
  `[n] not now`, default not-now).
- Trigger in `main.rs` (after `PanelManager::new`, `manager.rs:334-346` call
  site):

```rust
let state_dir = util::xdg_state_home().map(|p| p.join("rfm"));
let version = env!("CARGO_PKG_VERSION");
if let Ok(state_dir) = &state_dir {
    let seen = config::app_state::read(state_dir);
    if seen.upgrade_notice_seen_for.as_deref() != Some(version) {
        panel_manager.maybe_show_upgrade_notice(&dropped, loaded.legacy_folded);
    }
}
```

- `maybe_show_upgrade_notice`: builds the flow; if `Some`, sets
  `self.mode = Mode::Modal(Box::new(flow))` + `mark_dirty()` (entry pattern
  = `ViewTrash` at `manager.rs:1926-1939`). Store `state_dir`+`version` on
  the manager (two small fields) so resolution can persist.
- `resolve_flow(FlowKind::UpgradeNotice, answers)`:
  - per conflict item answered "adopt": `info!` the exact config lines to
    change (from the item's detail; v1 = suggestion, no session rebinding).
  - migrate item answered "migrate now": run
    `config::load::migrate(&config_dir)` (manager needs `config_dir` — pass
    it into `maybe_show_upgrade_notice` and store it) and `info!` each
    summary line + "restart rfm to load the unified config".
  - always: write `AppState { upgrade_notice_seen_for: Some(version) }`
    (write failure → `warn!`, continue).
  - `FlowAborted` (only possible if a future item has no default — today
    unreachable for this kind): do NOT persist; it re-asks next start.

**Step 4:** `cargo test` + clippy + build.

**Step 5: Commit** — `feat(config): one-time upgrade notice via decision flow`

---

### Task 6: integration pass + docs

**Files:**
- Modify: `CLAUDE.md` (modal modes list: `decision-flow` mode name; note the
  `XDG_STATE_HOME` isolation requirement for tests)
- Modify: `CHANGELOG.md`
- Modify: `docs/configuration.md` (short "after an upgrade" paragraph)

tmux + debug-socket verification (per CLAUDE.md harness; **export
`XDG_STATE_HOME=$(mktemp -d)` in the tmux pane before launching** — without
it the notice may be suppressed by the real state file):

1. Fixture: `--config` dir containing the *old* example trio with
   `quit = ["q", "exit"]` in keys.toml → launch → `state` shows
   `"decision-flow"` mode; screen (capture-pane) shows the conflict item and
   the migrate item.
2. Press `y` (keep), then `n` (not now) → `await-idle` → `state` mode is
   `"normal"`; panels fully usable (`j` moves); `log` has no errors.
3. Relaunch same fixture + same `XDG_STATE_HOME` → mode is `"normal"`
   immediately (seen-version recorded).
4. Fresh `XDG_STATE_HOME`, answer the migrate item with `m` → config.toml
   written, `keys.toml.bak` exists, `log` mentions restart; relaunch →
   no notice, no legacy hint, bindings correct.
5. Esc on the notice → resolves with defaults (mode `"normal"`), state
   recorded — assert via relaunch.

Fix findings; final commit —
`docs+test: decision-flow integration pass and documentation`

---

## Deferred (design-doc backlog, NOT this plan)

- Onboarding flow (glyph test, color pick, opener PATH scan) — next consumer,
  own plan; the primitive built here is sufficient for it except live color
  preview (`ModeOp::Preview` channel, explicitly deferred).
- Answer-dependent branching, free-text items, `rfm doctor`.
