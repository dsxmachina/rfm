# Generic decision-flow overlay — design

A reusable overlay mode that presents a list of decision items — each with a
prompt, a few choices, and optionally a safe default — and hands the answers
back to the manager. One primitive, many consumers; the first is the config
upgrade/conflict notice from the
[config unification design](2026-07-30-config-unification-design.md).

## Motivation

Several planned features share one interaction shape — "show the user a list
of things, let them decide per item, confirm":

| Consumer | Items | Status |
|---|---|---|
| Upgrade notice / conflict review | dropped default keybindings: keep yours vs adopt new; "migrate to unified config now?" | first consumer |
| First-run onboarding | Nerd-Font glyph test ("do you see  ?"), main color, trash on/off, opener proposals from a `PATH` scan | second consumer |
| Bulk-rename preview | `old → new` per file, collisions flagged | future |
| Trash tooling (deferred in CLAUDE.md) | purge_all confirmation, multi-select restore, restore-to-arbitrary | future |

Rather than building a one-off migration wizard (whose old→new mapping is
fully mechanical and needs no questions), we build the primitive and let the
consumers stay thin.

**Non-goals (decided):**

- **Paste/extract collision resolution** — explicitly out. Overwrites are
  undoable in rfm, so the existing `paste` / `paste_overwrite` split is
  enough; no per-file interactive resolution.
- **A nag framework.** Only questions *without a safe default* may block
  (glyph visibility, keep-vs-adopt). Anything with a sane default stays a log
  line. Flows never block startup; Esc always works; one-time triggers
  remember "don't ask again".

## Architecture fit

A pure mode adapter in `src/panel/mode/decision_flow.rs`, exactly like
`trash_view.rs`: `handle_key → ModeOp`, zero terminal knowledge, all effects
applied centrally in `PanelManager::apply_mode_op`. Rendered as an overlay —
drawn last in `draw()`, which *is* the z-order; `mark_dirty()` on every state
change. The debug socket reports it via `ModalInput::name()` as
`"decision-flow"`.

## Data model

```rust
pub struct DecisionFlow {
    kind: FlowKind,               // who launched this, for result dispatch
    title: String,
    items: Vec<DecisionItem>,
    cursor: usize,                // current item
    answers: Vec<Option<usize>>,  // chosen choice index per item
}

pub struct DecisionItem {
    prompt: String,          // "default `q` → close_tab conflicts with quit"
    detail: Vec<String>,     // extra lines: old → new preview, paths, ...
    choices: Vec<Choice>,    // 2–4 options
    default: Option<usize>,  // preselected; None = user must answer
}

pub struct Choice {
    key: char,               // one-key answer: 'k' keep, 'a' adopt, ...
    label: String,
}

pub enum FlowKind {
    UpgradeNotice,
    Onboarding,
    // future: BulkRenamePreview, TrashPurge, ...
}
```

The manager (or a helper per consumer) *builds* the flow — e.g. the config
loader's list of conflict-dropped defaults becomes `DecisionItem`s; a `PATH`
scan becomes opener proposals. The adapter only walks it. Item lists are
static for v1 — no answer-dependent branching (not needed by either initial
consumer; see *Deferred*).

## Adapter behavior

- `j`/`k` (and arrows) move between items; answering auto-advances to the
  next unanswered item — a flow of N items is N keypresses in the common case.
- A choice's `key` answers the current item. `Enter` accepts the item's
  `default` if it has one.
- `A` applies the current answer to all remaining items *with the same choice
  set* (classic "apply to all"; only shown when ≥2 such items remain).
- **Finish**: when every item with `default: None` is answered, `Enter` on
  the confirm row completes the flow.
- **Esc / q**: if every unanswered item has a default → complete with
  defaults. If not → abort; the manager interprets abort per `FlowKind`
  (upgrade notice: treat as "remind later"; onboarding: write nothing).
- A single-item flow renders as a compact confirm dialog — same adapter, no
  special case in the caller.

## Completion & wiring

```rust
ModeOp::FlowResolved { kind: FlowKind, answers: Vec<usize> }
ModeOp::FlowAborted  { kind: FlowKind }
```

`apply_mode_op` matches on `kind` and dispatches: `UpgradeNotice` answers
become config-edit suggestions in the log (never on-disk writes — see config
design) or trigger `migrate-config`; `Onboarding` answers are serialized into
a sparse first-run `config.toml`. The adapter stays consumer-agnostic; all
consumer logic lives with the manager, unit-testable next to it.

## Triggers & the "once" persistence

- Flows launch from explicit commands or one-time triggers — never
  synchronously blocking startup: rfm draws the normal UI, then opens the
  overlay on top (Esc immediately returns to the panels).
- "Shown once" state (upgrade notice per version, "don't ask again") must
  not live in the config (read-only for nix/chezmoi users). It goes to
  `$XDG_STATE_HOME/rfm/state.toml` (fallback `~/.local/state/rfm/`) — a new,
  rfm-owned, freely-writable location. Write failure degrades to "may ask
  again next start", never to an error.
- Trigger for the upgrade notice: legacy files folded in, or ≥1 default
  binding dropped by the conflict rule, and `state.toml` hasn't recorded this
  rfm version as seen.

## First consumers

**Upgrade notice (ships with config unification):** title "config updated for
rfm X.Y"; one item per conflict-dropped default (`keep yours` (default) /
`adopt new — moves your binding`), plus, when legacy files were folded, a
final item "migrate to unified config.toml now? (writes config.toml, renames
old files to .bak)" defaulting to *not now*.

**Onboarding (second, independent release):** trigger = no config at all.
Items: glyph test (no default — genuinely unanswerable automatically), main
color, use_trash, one item per opener proposal found on `PATH`
(vim/nvim, sxiv/imv, mpv, zathura…). Result: a personal sparse
`config.toml` instead of today's full template. Live color preview is
deferred (see below) — the color item shows named swatches in `detail`.

## Deferred

- Answer-dependent branching (dynamic item lists).
- Live preview side effects while an item is focused (e.g. repainting the UI
  in a candidate color) — needs a `ModeOp::Preview` channel; revisit with
  onboarding polish.
- Free-text answers (rename-style input inside a flow). Choices only for v1.
- `rfm doctor` — better served by plain CLI text output; not a flow.

## Testing

Unit (pure adapter, no terminal): navigation, answer/auto-advance, apply-to-
all scoping (only same-choice-set items), Enter-accepts-default,
Esc-with-defaults completes vs Esc-without aborts, single-item flow, answers
vector integrity.

Integration (tmux + debug socket): fixture with a conflicting legacy config →
assert mode `"decision-flow"` in `state`, drive keys, assert the resulting
log lines / written files; assert Esc returns to `"normal"` and rfm stays
fully usable.

## Touched files (implementation sketch)

- `src/panel/mode/decision_flow.rs` — the adapter (new).
- `src/panel/mode/mod.rs` — register the mode.
- `src/panel/manager.rs` — `apply_mode_op` arms for `FlowResolved`/`FlowAborted`,
  flow builders (conflict list → items), trigger logic, state.toml read/write.
- `src/util.rs` — `xdg_state_home()` beside `xdg_config_home()`.
- draw path — overlay renderer for the flow (same slot as existing overlay).
