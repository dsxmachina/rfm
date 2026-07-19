# Jump-marks Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add vim-style jump-marks to rfm: `m<letter>` saves the current directory + highlighted entry, `'<letter>` returns to it.

**Architecture:** The `CommandParser` (`src/engine/commands.rs`) gains two char-carrying `Command` variants and generates 26 `m<a-z>` / `'<a-z>` chords into its existing `StringPatriciaMap` — no parser-loop changes, exactly how `jump_to` already works. `PanelManager` (`src/panel/manager.rs`) stores a session-only `HashMap<char, JumpMark>` and handles the two commands in `handle_event`, reusing the existing `jump()` + `select_path()`. Marks are exposed in the debug-socket `state` reply for testing.

**Tech Stack:** Rust, crossterm, tokio, `patricia_tree::StringPatriciaMap`, serde/toml, `serde_json` (debug socket).

**Design doc:** `docs/plans/2026-07-19-jump-marks-design.md`

**Key terminology:** "marking" = existing Space-key item selection (`Command::Mark`), untouched. "jump-marks" = this new feature. Never conflate them.

---

## Background facts (verified against the code)

- `CommandParser::add_event` (commands.rs:572) buffers normal chars, clears the buffer when no prefix matches, and returns the `Command` on an exact match. `m` alone is a prefix of `ma..mz` so it waits for the second key; `'` is already a prefix of `''` (jump-previous) so `'a..'z` just extend that prefix set.
- Bindings are built in **two** places that BOTH need jump-marks: `from_config` (commands.rs:240, used when a `keys.toml` exists — main.rs:217) and `default_bindings` (commands.rs:414, used otherwise — main.rs:221/229).
- `jump()` (manager.rs:632) repositions all three panels but **early-returns when the target equals the current dir** (manager.rs:635). `new_panel_instant` populates the center panel synchronously (from cache or `from_path`), so `select_path` immediately after is valid — the existing `jump()` already relies on this for the left panel (manager.rs:648).
- `select_path(path, None)` (directory.rs) selects the matching entry, or falls back to the current index when the entry is gone. This IS the graceful-degrade path — no error needed.
- `StateSnapshot` (src/debug.rs:22) derives `Serialize` and is built at manager.rs:1063.

---

## Task 1: Add `Command` variants + `Display`

**Files:**
- Modify: `src/engine/commands.rs:128` (enum `Command`)
- Modify: `src/engine/commands.rs:210` (the `Command::Mark` arm of `Display`)

**Step 1: Add the variants.** After the `Mark,` line (commands.rs:151), add:

```rust
    /// Set a jump-mark (session-only) at the current location.
    SetJumpMark(char),
    /// Jump to a previously-set jump-mark.
    JumpToMark(char),
```

**Step 2: Add `Display` arms.** After the `Command::Mark => write!(f, "mark selected item"),` line (commands.rs:210), add:

```rust
            Command::SetJumpMark(c) => write!(f, "set jump-mark '{c}'"),
            Command::JumpToMark(c) => write!(f, "jump to mark '{c}'"),
```

**Step 3: Compile.**

Run: `cargo build 2>&1 | tail -20`
Expected: builds (unused variants may warn — that's fine; wired up next).

**Step 4: Commit.**

```bash
git add src/engine/commands.rs
git commit -m "feat(jump-marks): add SetJumpMark/JumpToMark command variants"
```

---

## Task 2: Parser generates jump-mark chords (TDD)

Add a helper that inserts the 26+26 chords, call it from both `default_bindings` and `from_config`, and test via `add_event`.

**Files:**
- Modify: `src/engine/commands.rs` (new helper method; call sites at ~414 and ~319)
- Test: `src/engine/commands.rs` (inline `#[cfg(test)]` — add near the existing `test_split`)

**Step 1: Write the failing tests.** Add at the bottom of `src/engine/commands.rs`:

```rust
#[cfg(test)]
mod jump_mark_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn m_then_letter_sets_mark() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('m')), Command::None)); // waits
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
    }

    #[test]
    fn apostrophe_then_letter_jumps_to_mark() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('\'')), Command::None)); // waits
        assert!(matches!(p.add_event(key('a')), Command::JumpToMark('a')));
    }

    #[test]
    fn double_apostrophe_still_jumps_previous() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('\'')), Command::None));
        assert!(matches!(
            p.add_event(key('\'')),
            Command::Move(Move::JumpPrevious)
        ));
    }

    #[test]
    fn m_then_unbound_key_is_noop() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('1')), Command::None)); // buffer cleared
        // buffer is clear again: a lone 'j' now moves down
        assert!(matches!(p.add_event(key('j')), Command::Move(Move::Down)));
    }
}
```

**Step 2: Run tests to verify they fail.**

Run: `cargo test jump_mark_tests 2>&1 | tail -25`
Expected: FAIL — `SetJumpMark`/`JumpToMark` never produced (parser has no such chords yet).

**Step 3: Add the helper method.** Inside `impl CommandParser`, after `add_user_commands` (commands.rs:333), add:

```rust
    /// Generate the `<set-prefix><a-z>` and `<jump-prefix><a-z>` chords for
    /// jump-marks. Prefixes are usually a single key (`m` / `'`) but any
    /// string works, mirroring `jump_to`.
    fn insert_jump_marks(&mut self, set: Vec<String>, jump: Vec<String>) {
        for prefix in set {
            for c in 'a'..='z' {
                self.key_commands
                    .insert(format!("{prefix}{c}"), Command::SetJumpMark(c));
            }
        }
        for prefix in jump {
            for c in 'a'..='z' {
                self.key_commands
                    .insert(format!("{prefix}{c}"), Command::JumpToMark(c));
            }
        }
    }
```

**Step 4: Call it from `default_bindings`.** In `default_bindings`, the parser is built as a struct literal (commands.rs:545). Replace the tail:

```rust
        CommandParser {
            key_commands,
            mod_commands,
            buffer: "".to_string(),
        }
```

with:

```rust
        let mut parser = CommandParser {
            key_commands,
            mod_commands,
            buffer: "".to_string(),
        };
        parser.insert_jump_marks(vec!["m".to_string()], vec!["'".to_string()]);
        parser
```

**Step 5: Run tests to verify they pass.**

Run: `cargo test jump_mark_tests 2>&1 | tail -25`
Expected: PASS (all 4).

**Step 6: Commit.**

```bash
git add src/engine/commands.rs
git commit -m "feat(jump-marks): generate m<letter>/'<letter> chords (default bindings)"
```

---

## Task 3: Wire jump-marks into `from_config` + `[jump_marks]` config (TDD)

Config-driven prefixes, defaulting to `m`/`'` when the section/fields are absent.

**Files:**
- Modify: `src/engine/commands.rs` (new `JumpMarks` struct; `KeyConfig` field; `from_config` call)
- Test: `src/engine/commands.rs` (add to `jump_mark_tests`)

**Step 1: Write the failing test.** Add to `mod jump_mark_tests`:

```rust
    #[test]
    fn from_config_without_jump_marks_section_defaults_to_m_and_apostrophe() {
        // Minimal keys.toml body WITHOUT a [jump_marks] section.
        let toml = r#"
[general]
search = ["/"]
mark = [" "]
next = ["n"]
previous = ["N"]
view_trash = ["gT"]
toggle_hidden = ["zh"]
quit = ["q"]

[movement]
up = ["k"]
down = ["j"]
left = ["h"]
right = ["l"]
top = ["gg"]
bottom = ["G"]
page_forward = ["ctrl-f"]
page_backward = ["ctrl-b"]
half_page_forward = ["ctrl-d"]
half_page_backward = ["ctrl-u"]
jump_previous = ["''"]
jump_to = []

[manipulation]
rename = ["rename"]
mkdir = ["mkdir"]
touch = ["touch"]
cut = ["dd"]
copy = ["yy"]
delete = ["delete"]
paste = ["pp"]
paste_overwrite = ["po"]
zip = ["zip"]
tar = ["tar"]
extract = ["extract"]
"#;
        let cfg: KeyConfig = toml::from_str(toml).expect("parse keys.toml");
        let mut p = CommandParser::from_config(cfg);
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
        assert!(matches!(p.add_event(key('\'')), Command::None));
        assert!(matches!(p.add_event(key('b')), Command::JumpToMark('b')));
    }
```

**Step 2: Run to verify it fails.**

Run: `cargo test jump_mark_tests::from_config 2>&1 | tail -25`
Expected: FAIL — `from_config` inserts no jump-marks yet (the chords resolve to `Command::None`).

**Step 3: Add the config struct.** After the `Movement` struct (commands.rs:81), add:

```rust
#[derive(Deserialize, Debug, Default)]
struct JumpMarks {
    /// Prefix key(s) for setting a mark (default: `m`).
    set: Option<Vec<String>>,
    /// Prefix key(s) for jumping to a mark (default: `'`).
    jump: Option<Vec<String>>,
}
```

**Step 4: Add the field to `KeyConfig`** (commands.rs:96). Change:

```rust
pub struct KeyConfig {
    general: General,
    movement: Movement,
    manipulation: Manipulation,
}
```

to:

```rust
pub struct KeyConfig {
    general: General,
    movement: Movement,
    manipulation: Manipulation,
    #[serde(default)]
    jump_marks: JumpMarks,
}
```

**Step 5: Call the helper in `from_config`.** Just before `parser` is returned (commands.rs:319), add:

```rust
        parser.insert_jump_marks(
            config.jump_marks.set.unwrap_or_else(|| vec!["m".to_string()]),
            config
                .jump_marks
                .jump
                .unwrap_or_else(|| vec!["'".to_string()]),
        );
```

**Step 6: Run to verify it passes.**

Run: `cargo test jump_mark_tests 2>&1 | tail -25`
Expected: PASS (all 5).

**Step 7: Commit.**

```bash
git add src/engine/commands.rs
git commit -m "feat(jump-marks): configurable [jump_marks] prefixes, default m/'"
```

---

## Task 4: `JumpMark` storage + `PanelManager` field

**Files:**
- Modify: `src/panel/manager.rs` (new struct; new field; constructor init)

**Step 1: Add the `JumpMark` struct.** Near the top of `src/panel/manager.rs` (after the imports / before `pub struct PanelManager`), add:

```rust
/// A session-only vim-style jump-mark: a directory plus the entry that was
/// highlighted there when the mark was set.
#[derive(Debug, Clone)]
struct JumpMark {
    dir: PathBuf,
    entry: Option<PathBuf>,
}
```

**Step 2: Add the field** to `struct PanelManager`, after `previous: PathBuf,` (manager.rs:120):

```rust
    /// Session-only vim-style jump-marks, keyed by letter.
    jump_marks: std::collections::HashMap<char, JumpMark>,
```

**Step 3: Initialise it in the constructor.** Find where `previous: ".".into(),` is set (manager.rs:193) and add right after:

```rust
            jump_marks: std::collections::HashMap::new(),
```

**Step 4: Compile.**

Run: `cargo build 2>&1 | tail -20`
Expected: builds (field unused → warning is OK; used in the next task).

**Step 5: Commit.**

```bash
git add src/panel/manager.rs
git commit -m "feat(jump-marks): session-only JumpMark storage on PanelManager"
```

---

## Task 5: Handle the commands in `handle_event`

**Files:**
- Modify: `src/panel/manager.rs` (command match, near the `Command::Mark` arm at ~1462)

**Step 1: Add the two arms.** In the `match` inside `handle_event`, after the `Command::Mark => { ... }` arm (manager.rs:1462), add:

```rust
                        Command::SetJumpMark(c) => {
                            let dir = self.center.panel().path().to_path_buf();
                            let entry = self
                                .center
                                .panel()
                                .selected_path()
                                .map(|p| p.to_path_buf());
                            info!("jump-mark '{c}' set -> {}", dir.display());
                            self.jump_marks.insert(c, JumpMark { dir, entry });
                        }
                        Command::JumpToMark(c) => match self.jump_marks.get(&c).cloned() {
                            None => warn!("jump-mark '{c}' not set"),
                            Some(mark) => {
                                if !mark.dir.exists() {
                                    warn!(
                                        "jump-mark '{c}' -> {} no longer exists",
                                        mark.dir.display()
                                    );
                                } else {
                                    self.jump(mark.dir);
                                    if let Some(entry) = mark.entry {
                                        self.center.panel_mut().select_path(&entry, None);
                                        self.right.new_panel_delayed(
                                            self.center.panel().selected_path(),
                                        );
                                    }
                                    self.mark_dirty();
                                }
                            }
                        },
```

**Step 2: Verify `info!`/`warn!` are in scope.** They are already used throughout manager.rs (e.g. `error!` at 641, `warn!` elsewhere). If `warn` or `info` is not imported, add it to the `use log::{...}` line at the top of the file.

Run: `cargo build 2>&1 | tail -20`
Expected: builds cleanly (jump_marks field now read → no unused warning).

**Step 3: Run the full test suite + clippy.**

Run: `cargo test 2>&1 | tail -15 && cargo clippy 2>&1 | tail -20`
Expected: all tests pass; no new clippy warnings.

**Step 4: Commit.**

```bash
git add src/panel/manager.rs
git commit -m "feat(jump-marks): set/jump handlers wired into handle_event"
```

---

## Task 6: Expose jump-marks in the debug socket `state`

**Files:**
- Modify: `src/debug.rs:22` (`StateSnapshot`)
- Modify: `src/panel/manager.rs:1063` (snapshot construction)

**Step 1: Add the field to `StateSnapshot`.** After `redo_depth: usize,` (debug.rs:51), add:

```rust
    /// Session-only jump-marks: letter -> directory. Sorted for determinism.
    pub jump_marks: std::collections::BTreeMap<String, PathBuf>,
```

**Step 2: Populate it.** In the `StateSnapshot { ... }` literal (manager.rs:1063), after `redo_depth: self.undo.redo_depth(),` add:

```rust
            jump_marks: self
                .jump_marks
                .iter()
                .map(|(c, m)| (c.to_string(), m.dir.clone()))
                .collect(),
```

**Step 3: Build.**

Run: `cargo build 2>&1 | tail -20`
Expected: builds.

**Step 4: Commit.**

```bash
git add src/debug.rs src/panel/manager.rs
git commit -m "feat(jump-marks): expose jump_marks in debug-socket state"
```

---

## Task 7: Document `[jump_marks]` in `examples/keys.toml`

**Files:**
- Modify: `examples/keys.toml` (add after the `[movement]` block, before `[manipulation]`)

**Step 1: Add the documented section.** Insert before the `# Keybindings related to directory and file manipulation` line:

```toml
# Vim-style jump-marks (session-only): press <set><letter> to remember the
# current directory + highlighted entry, and <jump><letter> to return to it.
# Marks use letters a-z and are forgotten on quit. This whole section is
# optional; if omitted, `m` and `'` are used.
[jump_marks]
set  = [ "m" ]      # m<letter> sets a jump-mark (e.g. `ma`)
jump = [ "'" ]      # '<letter> jumps to a jump-mark (e.g. `'a`)
```

**Step 2: Sanity-check the file still parses** by running the config-default test again:

Run: `cargo test jump_mark_tests 2>&1 | tail -10`
Expected: PASS (unchanged; this is a docs edit).

**Step 3: Commit.**

```bash
git add examples/keys.toml
git commit -m "docs(jump-marks): document optional [jump_marks] section in keys.toml"
```

---

## Task 8: Interactive integration test (tmux + debug socket)

Manual/scripted verification per the CLAUDE.md harness. This exercises the full set→navigate-away→jump-back→re-select cycle plus the `''` regression.

**Step 1: Build the release-debug binary.**

Run: `cargo build 2>&1 | tail -5`

**Step 2: Run this script** (writes to the scratchpad, isolated fixture):

```bash
set -e
FIXTURE=$(mktemp -d)
mkdir -p "$FIXTURE/dirA" "$FIXTURE/dirB"
touch "$FIXTURE/dirB"/{x,y,z}.txt
SOCK=/tmp/rfm-jm.sock
tmux kill-session -t rfm-jm 2>/dev/null || true; rm -f "$SOCK"
tmux new-session -d -s rfm-jm -x 120 -y 30
tmux send-keys -t rfm-jm "./target/debug/rfm --debug-socket $SOCK $FIXTURE" Enter
until [ -S "$SOCK" ]; do sleep 0.1; done
q(){ echo "$1" | socat - UNIX-CONNECT:"$SOCK"; }

# Enter dirB (first entry, dirs sort first) and select y.txt
tmux send-keys -t rfm-jm l; q await-idle          # into dirB
tmux send-keys -t rfm-jm j; q await-idle          # x -> y
# Set jump-mark 'a'
tmux send-keys -t rfm-jm m a; q await-idle
echo "--- after set (expect jump_marks.a -> .../dirB, selection y.txt) ---"
q state
# Navigate away: back out to fixture root, into dirA
tmux send-keys -t rfm-jm h; q await-idle
tmux send-keys -t rfm-jm k; q await-idle          # onto dirA (sorts first)
tmux send-keys -t rfm-jm l; q await-idle          # into dirA
echo "--- after navigate away (expect cwd .../dirA) ---"
q state
# Jump back to mark 'a'
tmux send-keys -t rfm-jm "'" a; q await-idle
echo "--- after jump (expect cwd .../dirB, selection y.txt) ---"
q state
q "entries center"
# Regression: '' jump-previous still works
tmux send-keys -t rfm-jm "'" "'"; q await-idle
echo "--- after '' (expect cwd back to dirA) ---"
q state
tmux kill-session -t rfm-jm; rm -rf "$FIXTURE" "$SOCK"
```

**Step 3: Assert on the output:**
- After set: `state.jump_marks` contains `"a": ".../dirB"`, `state.selection == "y.txt"`.
- After navigate away: `state.cwd` ends in `/dirA`.
- After jump: `state.cwd` ends in `/dirB` AND `state.selection == "y.txt"` AND `entries center` shows `y.txt` with `selected: true`.
- After `''`: `state.cwd` ends in `/dirA` (jump-previous unbroken).

**Step 4: Edge case — deleted entry.** Re-run manually: set mark on `y.txt`, `rm` it (`q log` will confirm the watcher fired), jump back → land in `dirB`, no crash, cursor falls back to another entry. Confirm no panic in `q log`.

**Step 5: Final full verification.**

Run: `cargo test 2>&1 | tail -15 && cargo clippy 2>&1 | tail -20 && cargo build 2>&1 | tail -5`
Expected: all green, no new warnings.

**Step 6: Commit** any test-script artifact only if kept; otherwise nothing to commit here (verification task).

---

## Done criteria

- `m<letter>` sets, `'<letter>` returns to dir + entry; `''` still jump-previous.
- Absent `[jump_marks]` config → defaults `m`/`'`; existing configs unaffected.
- 5 parser unit tests pass; interactive cycle verified; `cargo clippy` clean.
- Deleted dir/entry degrade gracefully (warn / index fallback), no panic.
- Not in scope: disk persistence, uppercase/global marks, marks-list overlay.
