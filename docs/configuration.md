# Configuration

rfm is configured through a single file, `config.toml`, under
`$XDG_CONFIG_HOME/rfm/` (usually `~/.config/rfm/`). You can point rfm at a
different config directory with `--config <dir>`.

The file holds **sparse overrides**: rfm always starts from its complete
built-in defaults and applies only the lines you actually write. An empty (or
absent) `config.toml` is perfectly valid — you get the full default
experience, including the default keybindings for every feature.

## The reference: `rfm --dump-config`

The complete, annotated default configuration is embedded in the binary.
Print it at any time:

```shell
rfm --dump-config
```

It documents **every** option together with its default value and is the
single source of truth — the same file the binary parses for its defaults, so
documentation and behavior cannot drift.

To change something, copy just that line (or section) into your
`~/.config/rfm/config.toml` and edit it. For example, to switch off the trash
and turn on Nerd Font icons, the whole file is:

```toml
[general]
use_trash = false
fancy_icons = true
```

Everything not mentioned stays at its default. On first run rfm writes a
short commented stub `config.toml` pointing at `--dump-config`; it never
writes anything else to your config directory on its own.

Sections at a glance:

| Section | Purpose |
| --- | --- |
| `[general]` | behaviour switches (trash, icons, preview rate limit) |
| `[colors]` | the base UI colors |
| `[styles.*]` | per-mime-type colors and symbols for file listings |
| `[commands.*]` | user-defined shell commands with their own keybindings |
| `[keys.*]` | all keybindings, grouped by category |
| `[open.*]` | which application opens which mime-type |

## Keybindings: `[keys.*]`

Bindings map a command name to one or more key sequences. A sequence can be a
single key, several keys typed in a row, or a whole word:

```toml
[keys.general]
search = [ "/", "search", "f" ]   # any of these triggers "search"
```

Modifiers use the prefixes `ctrl-`, `alt-` and `meta-` (e.g. `ctrl-r`).

Categories: `[keys.general]`, `[keys.movement]`, `[keys.jump_marks]`,
`[keys.tabs]`, `[keys.manipulation]`. `rfm --dump-config` lists every
available command with a comment.

The semantics:

- **Absence means "use the default".** You only write the bindings you want
  to change; everything else — including bindings for features added in
  future versions — keeps working with its default keys.
- **Your bindings win.** If a default binding (of any command you did not
  mention) collides exactly with a key sequence you bound yourself, the
  default is dropped and a notice is logged at startup. (Mere prefix
  overlaps are fine — the key matcher waits for the longer chord, as it
  always has.) Example:
  if your config binds `quit = ["q"]`, the default `close_tab = ["q",
  "ctrl-w"]` loses `q` and keeps `ctrl-w`.
- **`cmd = []` unbinds.** To remove a default binding without replacing it,
  set it to the empty list, e.g. `undo = []`.
- **`jump_to` replaces, not merges.** Overrides are whole values: a `jump_to`
  list in your config replaces the entire default list, so include every
  entry you want to keep.

### Static jump-marks

Directory shortcuts under `[keys.movement]`:

```toml
[keys.movement]
jump_to = [ ["gh", "~"],
            ["gc", "~/.config"],
            ["gr", "/"] ]
```

Each entry is `["KEYS", "DIRECTORY"]`. `~` expands to `$HOME`; environment
variables are **not** supported.

### Vim-style jump-marks

Session jump-marks are configured under `[keys.jump_marks]` (defaults are
`m` / `'`):

```toml
[keys.jump_marks]
set  = [ "m" ]   # m<letter> remembers the current location
jump = [ "'" ]   # '<letter> returns to it
```

## General behaviour: `[general]`

```toml
[general]
use_trash = true       # delete to the freedesktop trash (undoable) instead of
                       # deleting permanently. See usage.md#trash and usage.md#undo--redo.
fancy_icons = false    # use Nerd Font icons (needs a Nerd Font in your terminal)
rate_limit_interval_ms = 500   # preview decode rate limit while scrolling
```

When `fancy_icons = true`, rfm renders file-type icons from the
[Nerd Fonts](https://www.nerdfonts.com/) project (like yazi). Your terminal must
be configured with a Nerd Font (e.g. *JetBrainsMono Nerd Font*). When disabled,
rfm uses plain Unicode symbols that render in any terminal.

## Colors: `[colors]`

For normal text rfm uses your terminal's fore/background colors. The accent
colors are configurable, each to any of the 16 standard terminal colors:

```toml
[colors]
main      = "dark-green"   # borders, directory names, the cursor
marked    = "dark-yellow"  # marked items
highlight = "red"          # search matches, new-item creation
dir_path  = "dark-blue"    # the top-row directory path
rename    = "blue"         # the inline rename preview
```

Available colors: `black`, `dark-red`, `dark-green`, `dark-yellow`, `dark-blue`,
`dark-magenta`, `dark-cyan`, `grey`, `dark-grey`, `red`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `white`.

## Mime-type styles: `[styles.*]`

Files are colored and given a symbol based on their mime-type by a unified
`StyleEngine` (defaults are close to [yazi](https://github.com/sxyazi/yazi)).
Override any of them under `[styles.<mime>]`:

```toml
[styles.image]              # matches all image/* types
color = "cyan"

[styles."text/markdown"]    # exact mime-type match
color = "blue"
symbol = ""

[styles."application/json"]
color = "dark-yellow"
symbol = "{}"
```

Matching tries the exact mime-type first (`text/markdown`), then the type prefix
(`image`). Both `color` and `symbol` are optional. The mime-type of the current
selection is shown in rfm's bottom status line, which is handy when writing rules.

## Custom commands: `[commands.*]`

Bind arbitrary shell commands to keys. `$@` expands to the marked (or selected)
paths, shell-escaped:

```toml
[commands.<name>]
keys        = [ "e" ]        # key sequence(s) that trigger the command
cmd         = "nvim $@"      # shell command template ($@ = the paths)
interactive = true           # optional (default false)
separator   = " "            # optional (default " "): how multiple paths are joined
```

- `interactive = false` (default): the command is **queued and run in the
  background**, so rfm stays responsive.
- `interactive = true`: the command runs in the **foreground** (for editors,
  pagers and other programs that need the terminal).

Commands run via `sh -c` in the currently focused directory. Note that custom
commands are **not** tracked by undo/redo. Custom-command keys take part in
the same user-wins conflict rule as `[keys.*]` bindings.

## Openers: `[open.*]`

Configures which application opens which file, keyed by mime-type. The only
built-in entry opens `text/*` files in `vim`; for every mime-type without an
entry, rfm falls back to your system's default application.

```toml
[open.text]
default = { name = "vim", args = [], terminal = true }

[open.application]
# Different apps per extension:
extensions = [
  ["pdf", { name = "zathura",     args = [], terminal = false } ],
  ["ppt", { name = "libreoffice", args = [], terminal = false } ],
]
```

- `terminal = true`: the program runs inside the current terminal session as a
  child process (for terminal apps like `vim`).
- `terminal = false`: a separate window is spawned and rfm keeps running (for GUI
  apps).

## Error handling

rfm always starts, whatever the config's state:

- **Unknown keys warn.** A typo like `pgae_forward` is reported at startup
  with a "did you mean `page_forward`?" suggestion instead of being silently
  ignored.
- **Errors degrade per section, not per file.** A broken value drops only the
  offending section from your overrides (reported with its exact TOML path,
  e.g. `keys.movement.jump_to[3]`); the rest of your config survives.
- Startup warnings outlive the on-screen log widget in the retained history
  (see `error.log` on exit, or the debug socket's `log` command).

## Legacy configurations

Older rfm versions used three files: `config.toml`, `keys.toml` and
`open.toml`. These keep working unchanged, forever:

- The old `config.toml` is already a valid sparse override of the new format
  (its sections kept their names and shapes).
- A lingering `keys.toml` / `open.toml` is folded in-memory under `[keys]` /
  `[open]` at startup — nothing on disk is ever touched. A startup hint
  reminds you that `rfm --migrate-config` exists. If your `config.toml`
  already has a `[keys]` (or `[open]`) section, the legacy file is ignored
  with a warning — the new format wins.
- Because old configs are sparse overrides too, they automatically gain the
  default keybindings of newly added features (dropped only where they'd
  collide with your own bindings — with a logged notice).

To unify everything on disk, run the explicit, opt-in migration:

```shell
rfm --migrate-config
```

It computes your effective configuration (including folded legacy files),
diffs it against the built-in defaults, writes only the differences as a
minimal `config.toml`, and renames `keys.toml` / `open.toml` (and a
pre-existing `config.toml`) to `*.bak`. Comments are lost — the reference is
`--dump-config`. It refuses to run if a `*.bak` it would create already
exists, so re-running it can never destroy a previous run's backups. It also
doubles as a "shrink my stale full-copy config" command: values that merely
restate the defaults are dropped.
