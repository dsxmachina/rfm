# Jump-marks — design

Vim-style saved locations: set with `m<letter>`, return with `'<letter>`.

## Terminology (important)

Two similarly-named-but-distinct concepts must never blur in code or docs:

- **marking** — the *existing* Space-key selection of items for an operation
  (`Command::Mark`). Untouched by this feature.
- **jump-marks** — the *new* vim-style saved locations. All new identifiers use
  the `JumpMark` / `jump_marks` naming so there is zero collision with `Mark`.

## Scope (decided)

- A jump-mark remembers **directory + highlighted entry** (not directory alone).
- **Session-only**: marks live in memory and drop on quit, like undo / clipboard
  / history. No disk persistence.
- Mark names are **lowercase `a`–`z`** only (vim's buffer-local marks; no
  uppercase/global marks since there is no persistence).
- **MVP = set + jump only.** A `:marks`-style listing overlay is deferred; it is
  a clean additive follow-up using the `trash_view.rs` modal pattern.

## How vim does it (reference)

- Set: `m{a-z}` (lowercase buffer-local; uppercase global via viminfo).
- Jump: `` `{mark} `` → exact line+column; `'{mark}` → first non-blank of the
  line. rfm has only one notion of position (dir + one entry), so we use a
  single jump prefix (`'`) and skip the `` ` `` vs `'` split.
- In vim `m` / `'` are fixed-but-remappable key mappings, not config options.
  rfm routes *every* key through `keys.toml`, so we make the prefix a config
  entry (defaulting to `m` / `'`) — that is rfm-consistent, not vim-literal.

## Data model & storage

```rust
struct JumpMark {
    dir: PathBuf,            // cwd when the mark was set
    entry: Option<PathBuf>, // highlighted entry's full path (None if dir empty)
}
```

Stored session-scoped on `PanelManager`, alongside `previous` / history:

```rust
jump_marks: HashMap<char, JumpMark>,   // key ∈ 'a'..='z'
```

## Keybindings & parser wiring

No parser surgery — reuse the static-entry mechanism `jump_to` already uses.

`keys.toml` gains an **optional** section with two configurable prefixes:

```toml
[jump_marks]
set  = [ "m" ]   # m<letter> sets a jump-mark
jump = [ "'" ]   # '<letter> jumps to a jump-mark
```

The section (and each field) is optional: absent → defaults to `m` / `'`, via
the same `unwrap_or_default` pattern as `toggle_log` / `quit_no_cd`. Existing
user configs keep working untouched and gain jump-marks for free.

`Command` gains two char-carrying variants:

```rust
SetJumpMark(char),
JumpToMark(char),
```

In `from_config`, after reading the prefix strings, loop `'a'..='z'` and insert
26 combos per prefix into `key_commands`:

```rust
for prefix in config.jump_marks.set {            // usually just "m"
    for c in 'a'..='z' {
        parser.key_commands.insert(format!("{prefix}{c}"), Command::SetJumpMark(c));
    }
}
// same for jump → JumpToMark(c)
```

Why it composes cleanly:

- `m` is a prefix of `ma`..`mz`, so the buffer waits for the second key — just
  like `g` waits in `gg` / `gT`.
- `'` is already a prefix of `''` (jump-previous). Adding `'a`..`'z` only extends
  that prefix set; `''` still resolves to `JumpPrevious`, `'a` to `JumpToMark('a')`.
- `m` + an unbound key (e.g. `m1`) matches no prefix → buffer clears → no-op,
  same as any dead chord today.

## Jump behavior & error handling

Set (`m<letter>`, in `handle_event`):

```rust
Command::SetJumpMark(c) => {
    let dir = self.center.panel().path().to_path_buf();
    let entry = self.center.panel().selected_path().map(Path::to_path_buf);
    self.jump_marks.insert(c, JumpMark { dir, entry });
    info!("jump-mark '{c}' set → {}", dir.display());
}
```

Jump (`'<letter>`):

```rust
Command::JumpToMark(c) => match self.jump_marks.get(&c).cloned() {
    None => warn!("jump-mark '{c}' not set"),
    Some(mark) => {
        if !mark.dir.exists() {
            warn!("jump-mark '{c}' → {} no longer exists", mark.dir.display());
        } else {
            self.jump(mark.dir);                       // repositions all 3 panels
            if let Some(entry) = mark.entry {
                self.center.panel_mut().select_path(&entry, None);
                self.right.new_panel_delayed(self.center.panel().selected_path());
            }
            self.mark_dirty();
        }
    }
}
```

Edge cases — all graceful, all session-realistic:

- **Mark unset** → warn, no movement.
- **Marked dir deleted** during the session → warn, no movement (don't
  half-navigate).
- **Marked entry deleted**, dir survives → land in the dir; `select_path(entry,
  None)` finds no match and falls back to the current index (existing behavior).
  Correct dir, cursor just not on the vanished file. No error.
- **Already there** → `jump()` no-ops when `path == current`, but we still
  re-select the entry, so `'a` re-centers the cursor even in place.

## Testing & observability

Unit tests (`CommandParser`, no terminal):

- `m` `a` → `SetJumpMark('a')`; lone `m` leaves buffer pending.
- `'` `a` → `JumpToMark('a')`.
- `'` `'` → `Move::JumpPrevious` (regression guard for the extended `'` prefix).
- `m` `1` (unbound) → no command, buffer clears.
- `[jump_marks]` absent → prefixes default to `m` / `'`, all 52 combos register.

Debug-socket observability:

- Add `jump_marks` (a `{letter: dir}` map) to the `state` reply, mirroring
  `undo_depth` / `redo_depth`. Makes the set/jump cycle assertable without
  screen-scraping.

Interactive integration (tmux + socket, per CLAUDE.md harness):

1. Fixture: `dirA/` and `dirB/{x,y,z}.txt`.
2. In `dirB`, select `y.txt`, press `m` `a` → `await-idle` → assert
   `state.jump_marks` has `a → dirB`.
3. Navigate into `dirA` → assert cwd changed.
4. Press `'` `a` → `await-idle` → assert `state.cwd == dirB` and `entries center`
   shows `y.txt` selected.
5. Regression: `'` `'` → jump-previous still works.
6. Edge: delete the marked entry, jump again → land in `dirB`, no crash, cursor
   falls back.

Not tested: disk persistence (none exists) and the deferred overlay.

## Touched files

- `src/engine/commands.rs` — `Command::{SetJumpMark, JumpToMark}`; `[jump_marks]`
  config struct + defaults; combo generation in `from_config`.
- `src/panel/manager.rs` — `jump_marks` field; `handle_event` arms; `state`
  socket output.
- `examples/keys.toml` — documented optional `[jump_marks]` section.
