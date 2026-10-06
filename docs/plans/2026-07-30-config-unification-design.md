# Unified config file — design

One `config.toml` with sparse-override semantics replaces the
`config.toml` / `keys.toml` / `open.toml` trio. Old configs keep working
untouched and automatically gain new features' default keybindings.

Companion doc: [decision-flow overlay](2026-07-30-decision-flow-design.md)
(renders the upgrade/conflict notice described below).

## Problems today

- **New features are silently unbound for old configs.** New key fields are
  `Option` + `unwrap_or_default()` → empty vec (tabs, undo/redo, toggle_log
  in `commands.rs`). Old users never learn the features exist; the docs for
  them live in `examples/*.toml`, which is written to disk exactly once on
  first run and never seen again.
- **Three inconsistent fallback behaviors** for absent fields: real defaults
  (jump_marks → `m`/`'`), nothing (tabs/undo), and required-field parse
  failure (`[colors]` main/marked/highlight/dir_path).
- **All-or-nothing error handling.** One typo in `keys.toml` discards *every*
  custom binding (warn with 10 s widget TTL); a future required field in
  `config.toml` would discard trash/color settings the same way.
- **Two diverged sources of truth.** `CommandParser::default_bindings()`
  (commands.rs) is hand-maintained separately from `examples/keys.toml` and
  contains machine-specific personal bookmarks; `rate_limit_interval_ms`,
  `toggle_log` and `[commands]` are undocumented in the examples.
- **Incoherent file split.** Keybindings live in two files (`[commands]`
  entries in config.toml carry `keys = [...]`); `config.toml` and `keys.toml`
  both have a `[general]` section with unrelated meanings.

## Decisions

- **One file: `~/.config/rfm/config.toml`** (same name as the legacy general
  config — deliberately, see *Legacy compatibility*).
- **Sparse override semantics**: the binary carries complete defaults; the
  user file is a patch deep-merged on top. Empty/absent file = full default
  experience.
- **Default-on, user wins**: new features ship with default bindings that
  apply to everyone; any default that collides with an explicit user binding
  is dropped (logged, and surfaced via the upgrade notice).
- **Explicit unbind syntax**: `cmd = []` means "no binding"; absence means
  "default". Distinguishable because merging happens at the `toml::Value`
  layer, not via serde field defaults.
- **Never rewrite user files automatically.** Migration is explicit
  (`rfm migrate-config`); the legacy shim works in memory forever. This keeps
  nix home-manager / chezmoi / dotfile-repo setups (read-only config dirs)
  first-class.
- **No `version` field.** Sparse overrides make additive evolution free.
  A future rename/restructure is handled as a read-time alias in the loader
  first; versioned rewriting only if aliases ever pile up (YAGNI).

## Target format

Top-level sections keep the old `config.toml` names and shapes; the other two
files nest one level down:

```toml
[general]          # use_trash, fancy_icons, rate_limit_interval_ms
[colors]           # main, marked, highlight, dir_path, rename
[styles."text/markdown"]
[commands.checksum]      # user commands incl. their keys — now beside [keys]
[keys.general]           # old keys.toml [general]
[keys.movement]          # ... [movement], [manipulation], [jump_marks], [tabs]
[open.text]              # old open.toml [text], [image], ...
```

The complete annotated reference is `examples/default-config.toml` (already
written; it documents every option including the previously-undocumented
`rate_limit_interval_ms`, `rename` color, `toggle_log`, and `[commands.*]`).

## Defaults: single source of truth

`examples/default-config.toml` is embedded in the binary (rust-embed, like
the current examples) and **parsed at startup as the defaults tree**. It is
simultaneously the documentation (`--dump-config`) and the behavior — the two
cannot drift.

- `CommandParser::default_bindings()` is **deleted**, along with its personal
  bookmarks (`~/Projekte/loadrunner-2021`, `gN` → `/nix/store`, …). Default
  `jump_to` entries are the generic set from the example.
- A unit test asserts the embedded file parses into the typed `Config` and
  covers every field (defaults-completeness guard: adding a field without
  documenting it fails CI).

## Load pipeline

```
embedded default-config.toml ──parse──▶ defaults: toml::Value
user config.toml            ──parse──▶ user:     toml::Value
[legacy keys.toml/open.toml ──parse──▶ folded under user.keys / user.open]
deep_merge(defaults ◀── user) ──deserialize──▶ Config (typed)
```

- After the merge, every field is guaranteed present → the typed structs lose
  their `Option<Vec<String>>` noise (`KeyConfig`, `GeneralConfig` etc. become
  plain fields).
- Merge rule: tables merge recursively; any non-table value (including
  arrays) **replaces** the default wholesale. Consequence worth documenting:
  a user `jump_to` list replaces the whole default list, not per-entry (the
  example file says so explicitly at `jump_to`).
- **Unknown keys warn** ("unknown key `keys.movement.pgae_forward` — did you
  mean `page_forward`?"). Today typos are silently ignored.

### Key-space conflict rule (user wins)

Parser construction becomes two passes:

1. Insert every binding the user explicitly wrote.
2. Insert defaults only for commands the user did not mention, **skipping**
   any key/chord that collides with a pass-1 binding — exact or prefix
   collision, generalizing the jump-mark-chord rule from `0371f73`.

Each dropped default logs one line and feeds the upgrade notice. Worked
example: an old config with `quit = ["q", "exit"]` keeps `q` → quit; the new
default `close_tab = ["q", "ctrl-w"]` drops `q` and keeps `ctrl-w`.

## Legacy compatibility

The old `config.toml` **is already a valid sparse override of the new
format** — its sections keep their exact names and places. Old users of that
file need no shim at all (their full-copy file simply pins the old defaults
for the values it names, which is the correct reading of "the user wrote this
down").

Only `keys.toml` and `open.toml` need folding:

- If `config.toml` contains a `[keys]` (resp. `[open]`) table, a lingering
  `keys.toml` (resp. `open.toml`) is **ignored with a warning** — the new
  format wins.
- Otherwise the legacy file's tree is folded in-memory under `keys` / `open`
  and the normal merge pipeline runs. One startup line: "legacy keys.toml
  loaded — run `rfm migrate-config` when ready."
- Nothing on disk is ever touched by normal startup.

## CLI surface

- `rfm --dump-config` — print the embedded annotated default file (the
  discovery story for new options; users copy override lines from it).
- `rfm migrate-config` — explicit, opt-in: compute the user's effective tree
  (incl. folded legacy files), **diff it against the defaults**, write only
  the differences as a minimal `config.toml`, and rename `keys.toml` /
  `open.toml` to `*.toml.bak`. Comments are lost; the output is small and the
  reference is `--dump-config`. Doubles as a "shrink my stale full-copy
  config" command.

## First run

Write a ~10-line stub `config.toml` (header comment pointing at
`--dump-config`), **not** the full example — writing the full file is what
pinned today's users to 0.x defaults. `keys.toml` / `open.toml` are no longer
created.

## Error handling

- Parse errors degrade **per section**, not per file: a broken value drops
  that section from the user tree (rest survives), reported with its exact
  TOML path via `serde_path_to_error` (`keys.movement.jump_to[3]`-style).
- rfm always starts. Startup warnings outlive the 10 s widget TTL in the
  retained history (`log` socket command / `error.log`), and conflict-dropped
  defaults are additionally surfaced by the one-time upgrade notice
  (decision-flow doc).

## Testing

Unit (terminal-free):

- deep-merge semantics (table merge, array replace, `[]` unbind vs absent),
- conflict rule (exact + prefix collision, drop is logged),
- unknown-key detection,
- legacy fold-in incl. the "both `[keys]` and keys.toml present" precedence,
- defaults-completeness (embedded file ⇔ typed `Config`),
- migrate-config round-trip: `load(migrate(legacy_trio))` ≡ `load(legacy_trio)`.

Integration (tmux + debug socket, `--config` scratch dirs per CLAUDE.md):
four fixture states — empty dir (first-run stub), legacy trio, sparse new
`config.toml`, conflicting config (assert dropped default via `log` and that
the user's binding fires).

## Out of scope

- Automatic on-disk rewriting of user configs (never).
- `version` field / migration engine (alias-in-loader first if ever needed).
- Per-entry merging of `jump_to` (documented as whole-list replace).
- Live config reload.

## Touched files (implementation sketch)

- `src/config.rs` — merge pipeline, defaults loading, per-section error
  handling; structs lose `Option` fields.
- `src/engine/commands.rs` — two-pass parser build with conflict rule;
  `default_bindings()` deleted; `KeyConfig` fields become plain.
- `src/main.rs` — single-file loading + legacy fold-in; first-run stub;
  `--dump-config` / `migrate-config` CLI.
- `examples/default-config.toml` — becomes the embedded source of truth;
  `examples/{config,keys,open}.toml` deleted.
- `docs/configuration.md` — rewritten around sparse overrides.
