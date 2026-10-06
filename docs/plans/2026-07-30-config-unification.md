# Unified Config File Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace the three config files with one sparse-override `config.toml`
merged over embedded defaults, with user-wins keybinding conflicts and
in-memory legacy compatibility.

**Architecture:** A `toml::Value` deep-merge layer (user file over embedded
`examples/default-config.toml`) feeds the existing typed structs, whose key
fields all become `Option` (user overlay) resp. guaranteed-present (defaults).
`CommandParser` gains a two-pass `build(defaults, user)` with an exact-match
conflict rule. `main.rs` loses the three per-file blocks in favor of one
pipeline with legacy fold-in. Design: `2026-07-30-config-unification-design.md`.

**Tech Stack:** Rust, serde + `toml` 0.7 (`toml::Value` trees), `rust-embed`
(already embeds `examples/`), new dev-free dep `serde_path_to_error`, clap 4.

**Verification per task:** `cargo test` + `cargo clippy`. Integration via the
tmux + debug-socket harness (CLAUDE.md) in the final task.

---

## Ground rules for the executor

- The embedded reference `examples/default-config.toml` already exists and is
  final (committed in `6896113`). Do not restructure it; tasks below only
  *read* it.
- `examples/config.toml`, `examples/keys.toml`, `examples/open.toml` stay
  until Task 9 (current code still ships them).
- Never write to user config paths except in the explicitly listed places
  (first-run stub, `migrate-config`).
- Existing tests in `src/engine/commands.rs:770-1000` must keep passing at
  every commit (adapt them only where a task says so).

---

### Task 1: `toml::Value` deep-merge

**Files:**
- Create: `src/config/merge.rs`
- Modify: `src/config.rs` (becomes `src/config/mod.rs` — do the rename `git mv src/config.rs src/config/mod.rs` first, add `pub mod merge;`)

**Step 1: Write the failing tests** (in `src/config/merge.rs`)

```rust
use toml::Value;

/// Recursively merge `user` over `base`. Tables merge key-by-key; any
/// non-table value (scalars AND arrays) replaces the base value wholesale.
pub fn deep_merge(base: &mut Value, user: Value) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        s.parse::<Value>().unwrap()
    }

    #[test]
    fn scalar_replaces() {
        let mut base = v("a = 1\nb = 2");
        deep_merge(&mut base, v("a = 9"));
        assert_eq!(base, v("a = 9\nb = 2"));
    }

    #[test]
    fn tables_merge_recursively() {
        let mut base = v("[t]\nx = 1\ny = 2");
        deep_merge(&mut base, v("[t]\ny = 9"));
        assert_eq!(base, v("[t]\nx = 1\ny = 9"));
    }

    #[test]
    fn arrays_replace_wholesale() {
        // jump_to semantics: a user list REPLACES the default list
        let mut base = v("a = [1, 2, 3]");
        deep_merge(&mut base, v("a = [9]"));
        assert_eq!(base, v("a = [9]"));
    }

    #[test]
    fn empty_array_survives_merge() {
        // `undo = []` (unbind) must not be treated as "absent"
        let mut base = v("undo = [\"u\"]");
        deep_merge(&mut base, v("undo = []"));
        assert_eq!(base, v("undo = []"));
    }

    #[test]
    fn user_only_keys_are_kept() {
        // unknown keys survive the merge; they are warned about separately
        let mut base = v("a = 1");
        deep_merge(&mut base, v("zz = 5"));
        assert_eq!(base, v("a = 1\nzz = 5"));
    }

    #[test]
    fn type_mismatch_user_wins() {
        // user writes a scalar where a table is expected: user wins here,
        // the typed deserialize reports it with a precise path later
        let mut base = v("[t]\nx = 1");
        deep_merge(&mut base, v("t = 3"));
        assert_eq!(base, v("t = 3"));
    }
}
```

**Step 2: Run to verify failure**

Run: `cargo test config::merge -- --nocapture`
Expected: FAIL (todo! panic) — all 6 tests.

**Step 3: Implement**

```rust
pub fn deep_merge(base: &mut Value, user: Value) {
    match (base, user) {
        (Value::Table(b), Value::Table(u)) => {
            for (k, uv) in u {
                match b.get_mut(&k) {
                    Some(bv) => deep_merge(bv, uv),
                    None => {
                        b.insert(k, uv);
                    }
                }
            }
        }
        (b, u) => *b = u,
    }
}
```

**Step 4: Run tests** — Expected: 6 PASS. Also `cargo clippy`.

**Step 5: Commit** — `feat(config): toml deep-merge for sparse overrides`

---

### Task 2: unknown-key detection and defaults-diff

Same module, two pure functions. Both walk defaults/user trees in parallel.

**Files:**
- Modify: `src/config/merge.rs`

**Step 1: Failing tests**

```rust
/// Dotted paths of user keys that do not exist in the defaults tree.
/// Subtrees whose top-level key is in WILDCARD_TABLES accept arbitrary
/// keys ([styles.*], [commands.*], [open.*]) and are not checked.
pub fn unknown_keys(defaults: &Value, user: &Value) -> Vec<String> {
    todo!()
}

const WILDCARD_TABLES: &[&str] = &["styles", "commands", "open"];

/// The minimal tree such that deep_merge(defaults, diff) == effective.
/// Returns None when effective adds nothing over defaults.
pub fn diff_from_defaults(defaults: &Value, effective: &Value) -> Option<Value> {
    todo!()
}
```

Tests (same `mod tests`):

```rust
#[test]
fn unknown_key_reported_with_path() {
    let d = v("[keys.movement]\npage_forward = [\"ctrl-f\"]");
    let u = v("[keys.movement]\npgae_forward = [\"ctrl-f\"]");
    assert_eq!(unknown_keys(&d, &u), vec!["keys.movement.pgae_forward"]);
}

#[test]
fn wildcard_tables_accept_any_key() {
    let d = v("[styles]\n[commands]\n[open.text]\ndefault = { name = \"vim\", args = [], terminal = true }");
    let u = v("[styles.image]\ncolor = \"cyan\"\n[commands.checksum]\nkeys = [\"cs\"]\ncmd = \"sha256sum $@\"\n[open.video]\ndefault = { name = \"mpv\", args = [], terminal = true }");
    assert!(unknown_keys(&d, &u).is_empty());
}

#[test]
fn diff_drops_values_equal_to_default() {
    let d = v("[general]\nuse_trash = true\nfancy_icons = false");
    let e = v("[general]\nuse_trash = true\nfancy_icons = true");
    assert_eq!(
        diff_from_defaults(&d, &e).unwrap(),
        v("[general]\nfancy_icons = true")
    );
}

#[test]
fn diff_of_identical_trees_is_none() {
    let d = v("[general]\nuse_trash = true");
    assert!(diff_from_defaults(&d, &d.clone()).is_none());
}

#[test]
fn diff_keeps_explicit_unbind() {
    // undo = [] differs from default ["u"] and must survive migrate-config
    let d = v("[keys.manipulation]\nundo = [\"u\"]");
    let e = v("[keys.manipulation]\nundo = []");
    assert_eq!(diff_from_defaults(&d, &e).unwrap(), e);
}
```

**Step 2:** `cargo test config::merge` → new tests FAIL.

**Step 3: Implement** (recursive walks; `unknown_keys` recurses only through
tables, skipping recursion into `WILDCARD_TABLES` roots; `diff_from_defaults`
on tables keeps entries whose recursive diff is `Some`, on non-tables returns
`Some(effective)` iff `effective != default`).

**Step 4:** All tests PASS, clippy clean.

**Step 5: Commit** — `feat(config): unknown-key detection and defaults-diff`

---

### Task 3: unified `Config` shape + all-`Option` `KeyConfig`

The typed structs get their final shape. `KeyConfig` becomes the *user
overlay type*: every binding field `Option<Vec<String>>` (`None` = "use
default", `Some(vec![])` = explicit unbind). The same struct, parsed from the
embedded defaults file, is the *defaults instance* (all-`Some`, guaranteed by
Task 4's completeness test).

**Files:**
- Modify: `src/engine/commands.rs:63-144` (`Manipulation`, `Movement`,
  `Tabs`, `JumpMarks`, `General`, `KeyConfig`)
- Modify: `src/config/mod.rs` (`Config` gains `keys` + `open`)

**Step 1: Failing test** (in `src/engine/commands.rs` test module)

```rust
#[test]
fn key_config_parses_from_empty_and_partial_toml() {
    // empty: every field None
    let empty: KeyConfig = toml::from_str("").unwrap();
    assert!(empty.movement.up.is_none());
    assert!(empty.manipulation.undo.is_none());

    // partial: only what is written is Some; [] stays Some(empty)
    let partial: KeyConfig = toml::from_str(
        "[manipulation]\nundo = []\n[movement]\nup = [\"k\"]",
    )
    .unwrap();
    assert_eq!(partial.movement.up, Some(vec!["k".into()]));
    assert_eq!(partial.manipulation.undo, Some(vec![]));
    assert!(partial.movement.down.is_none());
}
```

**Step 2:** FAIL — required fields reject empty input.

**Step 3: Implement**

- Every field in `General`, `Movement`, `Manipulation` becomes
  `Option<Vec<String>>` (`jump_to: Option<Vec<(String, String)>>`); `Tabs`
  and `JumpMarks` already are. Add `#[serde(default)]` + `Default` derive to
  every section struct AND to each section field of `KeyConfig` so empty
  input parses.
- All section structs and their fields become `pub` (the builder in Task 5
  reads them field-by-field).
- In `src/config/mod.rs`:

```rust
#[derive(Deserialize, Debug)]
pub struct Config {
    pub colors: color::ColorConfig,
    pub general: GeneralConfig,
    #[serde(default)]
    pub styles: StyleConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
    #[serde(default)]
    pub keys: crate::engine::commands::KeyConfig,
    #[serde(default)]
    pub open: crate::engine::opener::OpenerConfig,
}
```

- `ColorConfig.rename` becomes plain `String` (the defaults file carries
  `rename = "blue"`); drop the `.map/.transpose/.unwrap_or` dance in
  `colors_from_config` (`src/config/mod.rs:75-80`) and delete the dead
  `COLOR_MAIN.get_or_init(|| main);` line (`mod.rs:82`) while touching it.
- **Temporary shim so this commit compiles and current behavior is
  unchanged:** `CommandParser::from_config` (`commands.rs:304-423`) switches
  its field accesses to `config.general.search.unwrap_or_default()` etc. —
  every field uniformly, replacing the mixed required/`unwrap_or_default`
  pattern. jump_marks keeps its `m`/`'` fallback. (This makes old
  `keys.toml` parsing *more* tolerant for one release of history; Task 5
  replaces `from_config` entirely.)

**Step 4:** `cargo test` — new test passes; the two `from_config_*` tests at
`commands.rs:898,949` still pass (their input TOML is now a valid partial).
`cargo build` + clippy clean.

**Step 5: Commit** — `refactor(config): unified Config shape, all-optional KeyConfig`

---

### Task 4: embedded defaults loader + completeness guard

**Files:**
- Modify: `src/config/mod.rs`
- Modify: `src/main.rs:78-80` (`Examples` embed stays; expose the default
  file's bytes via a function in `config`)

Move the rust-embed struct into `config/mod.rs` (it is config-owned now):

```rust
#[derive(rust_embed::Embed)]
#[folder = "examples/"]
pub struct Examples;

pub const DEFAULT_CONFIG_FILE: &str = "default-config.toml";

pub fn default_config_str() -> &'static str {
    // embedded at compile time; both unwraps are compile-time-guaranteed
    std::str::from_utf8(
        Examples::get(DEFAULT_CONFIG_FILE).expect("embedded default config").data
        // note: rust-embed returns Cow; leak once at startup or store in OnceCell
    )
    .expect("default config is utf-8")
}

pub fn default_tree() -> toml::Value {
    default_config_str().parse().expect("default config parses")
}
```

(Implementation detail: `Examples::get` returns an owned `Cow<[u8]>` —
store the parsed `&'static str` via `once_cell::sync::Lazy<String>` instead
of leaking; follow the existing OnceCell idiom from `config::color`.)
`main.rs` keeps using `Examples` through `config::Examples` for the
remaining legacy example writes until Task 6 removes them.

**Step 1: Failing tests** (in `src/config/mod.rs`)

```rust
#[cfg(test)]
mod defaults_tests {
    use super::*;

    #[test]
    fn embedded_defaults_deserialize() {
        let config: Config = default_tree().try_into().unwrap();
        // spot checks
        assert!(config.general.use_trash);
        assert_eq!(config.general.rate_limit_interval_ms, 500);
    }

    /// Completeness guard: every binding field must be present (Some) in the
    /// defaults file — adding a Command without documenting it fails here.
    #[test]
    fn defaults_cover_every_binding_field() {
        let config: Config = default_tree().try_into().unwrap();
        config.keys.assert_complete(); // panics with the field name if None
    }
}
```

`assert_complete` is a `#[cfg(test)]`-only method on `KeyConfig` listing
every field explicitly:

```rust
#[cfg(test)]
pub fn assert_complete(&self) {
    macro_rules! req {
        ($($f:expr, $n:literal;)+) => { $(assert!($f.is_some(), concat!("default missing: ", $n));)+ }
    }
    req! {
        self.general.search, "general.search";
        self.general.mark, "general.mark";
        // ... every field of every section, including
        self.general.quit_no_cd, "general.quit_no_cd";   // present as []
        self.manipulation.undo, "manipulation.undo";
        self.tabs.focus_tab_4, "tabs.focus_tab_4";
        self.jump_marks.set, "jump_marks.set";
        self.movement.jump_to, "movement.jump_to";
    }
}
```

(Write out the full list — ~35 lines. This explicit macro list IS the guard;
no reflection games.)

**Step 2:** Run `cargo test config::defaults_tests` — FAIL (functions
missing), then after wiring possibly a real finding: if any field is missing
from `examples/default-config.toml`, FIX THE TOML (that's the guard working).

**Step 3:** Implement as above.

**Step 4:** PASS + clippy.

**Step 5: Commit** — `feat(config): embedded default-config.toml as single source of truth`

---

### Task 5: two-pass `CommandParser::build` with user-wins conflict rule

**Files:**
- Modify: `src/engine/commands.rs`

**Step 1: Failing tests**

```rust
fn defaults() -> KeyConfig {
    crate::config::default_tree()
        .try_into::<crate::config::Config>().unwrap().keys
}

#[test]
fn empty_user_config_gets_all_defaults() {
    let (mut p, dropped) = CommandParser::build(&defaults(), &KeyConfig::default(), &Default::default());
    assert!(dropped.is_empty());
    assert!(matches!(press(&mut p, 'u'), Command::Undo));       // new-feature default
    assert!(matches!(press(&mut p, '!'), Command::ToggleSplit)); // opt-in no more
}

#[test]
fn user_binding_wins_default_dropped_and_reported() {
    // old-school config: q means quit
    let user: KeyConfig = toml::from_str("[general]\nquit = [\"q\", \"exit\"]").unwrap();
    let (mut p, dropped) = CommandParser::build(&defaults(), &user, &Default::default());
    assert!(matches!(press(&mut p, 'q'), Command::Quit));        // user wins
    assert!(dropped.iter().any(|d| d.binding == "q" && d.command.contains("close")));
    // the non-conflicting half of the default survives:
    assert!(p.mod_commands.contains_key(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)));
}

#[test]
fn explicit_empty_list_unbinds_and_blocks_default() {
    let user: KeyConfig = toml::from_str("[manipulation]\nundo = []").unwrap();
    let (mut p, dropped) = CommandParser::build(&defaults(), &user, &Default::default());
    assert!(matches!(press(&mut p, 'u'), Command::None));
    assert!(dropped.is_empty()); // an unbind is not a conflict
}

#[test]
fn user_command_keys_claim_before_defaults() {
    let mut cmds = crate::command_queue::CommandsConfig::default();
    cmds.insert("mine".into(), toml::from_str("keys = [\"u\"]\ncmd = \"true\"").unwrap());
    let (mut p, dropped) = CommandParser::build(&defaults(), &KeyConfig::default(), &cmds);
    assert!(matches!(press(&mut p, 'u'), Command::UserCommand { .. }));
    assert!(dropped.iter().any(|d| d.binding == "u"));
}

#[test]
fn user_jump_to_replaces_whole_default_list() {
    let user: KeyConfig = toml::from_str("[movement]\njump_to = [[\"gz\", \"/tmp\"]]").unwrap();
    let (mut p, _) = CommandParser::build(&defaults(), &user, &Default::default());
    assert!(matches!(press2(&mut p, 'g', 'z'), Command::Move(Move::JumpTo(_))));
    assert!(matches!(press2(&mut p, 'g', 'h'), Command::None)); // default list gone
}
```

(`press`/`press2` are tiny local helpers wrapping the existing key-event
feeding used by the tests at `commands.rs:780-825` — reuse their idiom.)

**Step 2:** FAIL (no `build`).

**Step 3: Implement**

```rust
#[derive(Debug)]
pub struct DroppedDefault {
    pub command: String, // Display of the default command that lost
    pub binding: String, // the colliding pattern
    pub kept: String,    // Display of the user command that claimed it
}

impl CommandParser {
    /// Pass 1: user bindings + user-defined commands (they claim key space).
    /// Pass 2: defaults for every field the user did not mention; a default
    /// binding whose exact pattern is already claimed is dropped + reported.
    /// Prefix overlaps are NOT conflicts — the patricia matcher already
    /// defers on longer candidates (see `longer_binding_wins_over_mark_chord`).
    pub fn build(
        defaults: &KeyConfig,
        user: &KeyConfig,
        user_commands: &CommandsConfig,
    ) -> (Self, Vec<DroppedDefault>) { ... }
}
```

Mechanics:

- A local closure `resolve = |field: &Option<Vec<String>>, def: &Option<Vec<String>>| -> (Vec<String>, bool /*from_user*/)`.
- Iterate a fixed field table (same order as today's `from_config`), pass 1
  inserting all `from_user` bindings, collecting them; then pass 2 walks the
  remaining fields and calls a new `insert_default(binding, cmd, &mut dropped)`
  which first checks `self.is_claimed(binding)`.
- `fn is_claimed(&self, b: &str) -> bool` mirrors `insert`'s routing
  (`commands.rs:494-542`): ctrl-/alt-/meta-/named → `mod_commands.contains_key`,
  else `key_commands.get(b).is_some()`. Extract the string→`KeyEvent` routing
  from `insert` into a shared helper `route(b) -> Route::{Event(KeyEvent), Pattern(&str)}`
  so `insert`/`is_claimed`/`insert_default` cannot diverge (DRY).
- `jump_to`: pass 2 inserts default entries **individually**, skipping claimed
  chords one by one (user `gh` custom keeps the other `g*` defaults).
- jump-mark chords (`insert_chords`, `commands.rs:449-458`) run last,
  unchanged — they already skip claimed patterns.
- `from_config` and `default_bindings` are **deleted**. Their callers:
  - tests at `commands.rs:781-994` — rewrite `default_bindings()` uses as
    `build(&defaults(), &KeyConfig::default(), &Default::default()).0` (one
    helper fn in the test module) and `from_config(cfg)` uses as
    `build(&defaults(), &cfg, &Default::default()).0`. The
    `from_config_without_jump_marks_section_defaults_to_m_and_apostrophe`
    test keeps passing because defaults supply `m`/`'`.
  - `main.rs:213-233` — temporary: replace with
    `CommandParser::build(&default_config.keys, &user_keys, &user_commands)`
    using placeholder wiring; Task 6 finalizes this block. Log each
    `DroppedDefault` with `warn!`.
  - `add_user_commands` (`commands.rs:426-436`) becomes private, called
    inside pass 1.

**Step 4:** Full `cargo test` — all new + all 12 pre-existing commands tests
green. clippy.

**Step 5: Commit** — `feat(keys): two-pass parser build, defaults-on with user-wins conflicts`

---

### Task 6: single-file load pipeline in `main.rs`

Replaces `main.rs:149-259` wholesale.

**Files:**
- Create: `src/config/load.rs` (the pipeline, testable without a terminal)
- Modify: `src/config/mod.rs` (`pub mod load;`)
- Modify: `src/main.rs:149-259`

**Step 1: Failing tests** (in `src/config/load.rs`, using `tempfile` — already a dep)

```rust
/// Everything main() needs, plus warnings to log and upgrade-notice fodder.
pub struct LoadedConfig {
    pub config: Config,
    pub parser_input: KeyConfig,          // user overlay (for build())
    pub warnings: Vec<String>,            // unknown keys, dropped sections, legacy hints
    pub legacy_folded: bool,              // keys.toml/open.toml were folded in
}

pub fn load(config_dir: &Path) -> LoadedConfig { ... }
```

```rust
#[test]
fn empty_dir_creates_stub_and_yields_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load(dir.path());
    assert!(dir.path().join("config.toml").exists());          // stub written
    assert!(!dir.path().join("keys.toml").exists());           // legacy NOT created
    assert!(loaded.config.general.use_trash);
    assert!(loaded.warnings.is_empty());
}

#[test]
fn sparse_override_applies() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"),
        "[general]\nfancy_icons = true\n[keys.manipulation]\nundo = []").unwrap();
    let loaded = load(dir.path());
    assert!(loaded.config.general.fancy_icons);
    assert_eq!(loaded.parser_input.manipulation.undo, Some(vec![]));
}

#[test]
fn legacy_keys_and_open_fold_in() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "").unwrap();
    std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"x\"]").unwrap();
    std::fs::write(dir.path().join("open.toml"), "[text]\ndefault = { name = \"nano\", args = [], terminal = true }").unwrap();
    let loaded = load(dir.path());
    assert!(loaded.legacy_folded);
    assert_eq!(loaded.parser_input.movement.up, Some(vec!["x".into()]));
    assert!(loaded.warnings.iter().any(|w| w.contains("migrate-config")));
}

#[test]
fn new_format_keys_table_beats_legacy_keys_toml() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[keys.movement]\nup = [\"a\"]").unwrap();
    std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"b\"]").unwrap();
    let loaded = load(dir.path());
    assert_eq!(loaded.parser_input.movement.up, Some(vec!["a".into()]));
    assert!(loaded.warnings.iter().any(|w| w.contains("keys.toml") && w.contains("ignored")));
}

#[test]
fn broken_section_drops_only_that_section() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"),
        "[general]\nfancy_icons = true\n[colors]\nmain = 42").unwrap(); // colors broken
    let loaded = load(dir.path());
    assert!(loaded.config.general.fancy_icons);                        // survived
    assert!(loaded.warnings.iter().any(|w| w.contains("colors")));     // reported
}

#[test]
fn unknown_key_warns_but_loads() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[general]\nuse_trsah = false").unwrap();
    let loaded = load(dir.path());
    assert!(loaded.config.general.use_trash);                          // typo ≠ applied
    assert!(loaded.warnings.iter().any(|w| w.contains("use_trsah")));
}
```

**Step 2:** FAIL.

**Step 3: Implement `load()`**

Add `serde_path_to_error = "0.1"` to `Cargo.toml`.

Pipeline (each step appends to `warnings` instead of logging directly — the
caller logs, keeping `load` pure enough for tests):

1. `config.toml` absent → write the stub (below), treat as empty tree.
   Unreadable/unparseable *file-level* → warning + empty tree (defaults run).
2. Legacy fold-in: for (`keys.toml` → `"keys"`, `open.toml` → `"open"`):
   file exists? user tree already has that table → "ignored" warning; else
   parse and insert under that key, set `legacy_folded`, add the
   `migrate-config` hint warning (once).
3. `unknown_keys(&defaults, &user_tree)` → warnings (with a nearest-match
   suggestion via simple case: same section, edit distance ≤ 2 — implement
   as a ~10-line helper, no dep).
4. Extract the *user overlay* `parser_input`: `user_tree.get("keys")` →
   `KeyConfig` (default if absent/broken — broken adds a warning).
5. `deep_merge(defaults_tree, user_tree)` → typed deserialize via
   `serde_path_to_error::deserialize`. On error: take the error path's first
   segment (or first two for `keys.*`), remove that subtree from the *user*
   tree, warning with the full precise path, re-merge, retry (bounded loop:
   max = number of top-level user keys; each iteration removes one).

Stub content (a `const` in `load.rs`):

```toml
# rfm configuration — sparse overrides over the built-in defaults.
#
# An empty file is perfectly valid: you get the full default setup.
# See every available option together with its default value:
#
#     rfm --dump-config
#
# Copy any line or section from there into this file and change it.
```

`main.rs:149-259` shrinks to:

```rust
let loaded = config::load::load(&config_dir);
for w in &loaded.warnings { warn!("{w}"); }
colors_from_config(loaded.config.colors)?;   // infallible path now: merged tree is complete
let (mut parser, dropped) =
    CommandParser::build(&/* defaults from default_tree() */, &loaded.parser_input, &loaded.config.commands);
for d in &dropped {
    warn!("default `{}` → {} skipped: bound to {} in your config", d.binding, d.command, d.kept);
}
let opener = OpenEngine::with_config(loaded.config.open);
// styles/fancy_icons/use_trash/rate_limit as before, from loaded.config
```

(`colors_from_default()` becomes dead in main — keep the function, it still
backs unit tests; delete its call sites.)

**Step 4:** `cargo test` + clippy + **smoke run**: per CLAUDE.md, launch in
tmux with `--config $(mktemp -d)` and `--debug-socket`, `await-idle`, check
`state` answers and `log` shows no unexpected warnings. Teardown per
CLAUDE.md (kill session, rm socket).

**Step 5: Commit** — `feat(config): single-file sparse config with legacy fold-in`

---

### Task 7: `--dump-config`

**Files:**
- Modify: `src/main.rs:45-64` (Args) and early `main()`

**Step 1:** Failing test is impractical for a CLI print — use a doc-checked
manual step instead (allowed deviation): add Args field

```rust
/// Print the complete annotated default configuration and exit
#[arg(long)]
dump_config: bool,
```

and in `main()` **before any terminal setup** (before line ~120):

```rust
if args.dump_config {
    print!("{}", config::default_config_str());
    return Ok(());
}
```

**Step 2: Verify**

Run: `cargo run -- --dump-config | head -5`
Expected: the default-config.toml header comment.
Run: `cargo run -- --dump-config | wc -l` — matches
`wc -l examples/default-config.toml`.

**Step 3: Commit** — `feat(cli): --dump-config prints annotated defaults`

---

### Task 8: `rfm migrate-config`

**Files:**
- Modify: `src/config/load.rs` (pure `migrate` function + tests)
- Modify: `src/main.rs` (Args flag + early exit)

**Step 1: Failing tests**

```rust
/// Returns human-readable summary lines. Writes config.toml, renames legacy
/// files to *.bak. Pure-ish: all paths under `config_dir`.
pub fn migrate(config_dir: &Path) -> anyhow::Result<Vec<String>> { ... }
```

```rust
#[test]
fn migrate_writes_minimal_diff_and_baks_legacy() {
    let dir = tempfile::tempdir().unwrap();
    // stale full copy pinning an old default + one real customization
    std::fs::write(dir.path().join("keys.toml"),
        "[movement]\nup = [\"k\"]\ndown = [\"j\"]\n[general]\nquit = [\"q\", \"exit\"]").unwrap();
    migrate(dir.path()).unwrap();

    let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    let tree: toml::Value = written.parse().unwrap();
    // values equal to defaults were dropped:
    assert!(tree.get("keys").and_then(|k| k.get("movement")).is_none());
    // the real customization survived:
    assert_eq!(tree["keys"]["general"]["quit"], toml::Value::try_from(vec!["q", "exit"]).unwrap());
    assert!(dir.path().join("keys.toml.bak").exists());
    assert!(!dir.path().join("keys.toml").exists());
}

#[test]
fn migrate_load_equivalence() {
    // load(migrate(dir)) ≡ load(dir) — the invariant from the design doc
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.toml"), "[general]\nfancy_icons = true").unwrap();
    std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"x\"]").unwrap();
    let before = load(dir.path());
    migrate(dir.path()).unwrap();
    let after = load(dir.path());
    assert_eq!(after.config.general.fancy_icons, before.config.general.fancy_icons);
    assert_eq!(after.parser_input.movement.up, before.parser_input.movement.up);
    assert!(!after.legacy_folded);
}
```

**Step 2:** FAIL.

**Step 3: Implement** — reuse the `load` pipeline's steps 1–2 to get the
effective user tree (without writing a stub!), `diff_from_defaults`, back up
a pre-existing `config.toml` to `config.toml.bak` before overwriting, write
`toml::to_string_pretty(&diff)` with a two-line header comment, rename
`keys.toml`/`open.toml` → `*.bak`. Refactor shared parts of `load` rather
than duplicating (extract `fn effective_user_tree(config_dir) -> (Value, Vec<String>, bool)`).

Args: `#[arg(long)] migrate_config: bool`; early in `main()`:

```rust
if args.migrate_config {
    for line in config::load::migrate(&config_dir)? { println!("{line}"); }
    return Ok(());
}
```

**Step 4:** `cargo test` + clippy; manual: create a scratch `--config` dir
with the old example trio, run
`cargo run -- --config <dir> --migrate-config`, inspect the written file.

**Step 5: Commit** — `feat(cli): migrate-config writes minimal unified config`

---

### Task 9: retire the legacy examples + docs

**Files:**
- Delete: `examples/config.toml`, `examples/keys.toml`, `examples/open.toml`
- Modify: `docs/configuration.md` (rewrite around sparse overrides,
  `--dump-config`, `migrate-config`, unbind syntax, jump_to replace-not-merge)
- Modify: `CLAUDE.md` (config architecture section: one file, merge pipeline,
  where defaults live; update the `--config` test-isolation caveat if wording
  changes)
- Modify: `CHANGELOG.md` (feature entry: unified config, defaults-on
  keybindings, both CLI flags; explicit note that old three-file configs keep
  working unchanged)

**Steps:** grep first — `rg -l "keys\.toml|open\.toml" src docs` — and fix
every remaining reference (the embedded-example validation tests in
`main.rs:439-472` move to `config/mod.rs` against `default-config.toml`
only). `cargo test` must not reference deleted files. Commit —
`docs(config): document unified config; drop legacy example trio`

---

### Task 10: integration pass (tmux + debug socket)

Scripted verification of the four fixture states from the design doc, per the
CLAUDE.md harness (setup/teardown exactly as documented there; use
`await-idle`, `state`, `log`; `_ZO_DATA_DIR` isolation not needed here).

| Fixture | Assert |
|---|---|
| empty `--config` dir | stub written; `u` triggers undo (check `state.undo_depth` after a rename); `!` toggles split (`state.view == "split"`); no legacy files created |
| legacy trio (old example files copied in) | `log` contains the migrate hint; old bindings work; `Tab` cycles tabs (new default active) |
| sparse new config (`undo = []`, `fancy_icons = true`) | `u` does nothing (`log` shows no undo, `undo_depth` unchanged) |
| conflicting config (`quit = ["q"]`) | `q` quits (session gone) instead of closing tab; before quitting, `log` contains the dropped-default line |

Each is a short shell script run manually (or a `tests/`-free checklist —
rfm has no integration test dir; keep them as documented commands in the PR
description). Fix anything found; final commit —
`test(config): integration verification of unified config states`.

---

## Deferred / explicitly out (from the design doc)

- Upgrade-notice decision flow (separate plan: `2026-07-30-decision-flow.md`;
  consumes `Vec<DroppedDefault>` + `legacy_folded` — both already surfaced by
  Task 5/6 signatures).
- `version` field, auto-rewrite, per-entry `jump_to` merge, live reload.
