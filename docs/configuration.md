# Configuration

rfm is configured through three files under `$XDG_CONFIG_HOME/rfm/` (usually
`~/.config/rfm/`):

| File | Purpose |
| --- | --- |
| `config.toml` | general behaviour, colors, mime-type styles, custom commands |
| `keys.toml` | keybindings and jump-marks |
| `open.toml` | which application opens which file type |

If the files don't exist, they are created with commented defaults on first
start. Fully worked examples live in [`examples/`](../examples). You can point
rfm at a different config directory with `--config <dir>`.

---

## `config.toml`

### General behaviour

```toml
[general]
use_trash = true       # delete to the freedesktop trash (undoable) instead of
                       # deleting permanently. See usage.md#trash and usage.md#undo--redo.
preview_cache = true   # persist image/video preview thumbnails in
                       # $XDG_CACHE_HOME/rfm/thumbnails (usually ~/.cache/rfm/thumbnails)
                       # so they survive restarts. Pruned automatically at startup
                       # (entries older than 30 days, capped at 256 MiB).
                       # When disabled, previews are cached in memory only.
fancy_icons = false    # use Nerd Font icons (needs a Nerd Font in your terminal)
```

When `fancy_icons = true`, rfm renders file-type icons from the
[Nerd Fonts](https://www.nerdfonts.com/) project (like yazi). Your terminal must
be configured with a Nerd Font (e.g. *JetBrainsMono Nerd Font*). When disabled,
rfm uses plain Unicode symbols that render in any terminal.

### Colors

For normal text rfm uses your terminal's fore/background colors. Four accent
colors are configurable, each to any of the 16 standard terminal colors:

```toml
[colors]
main      = "dark-green"   # borders, directory names, the cursor
marked    = "dark-yellow"  # marked items
highlight = "red"          # search matches, new-item creation
dir_path  = "dark-blue"    # the top-row directory path
```

Available colors: `black`, `dark-red`, `dark-green`, `dark-yellow`, `dark-blue`,
`dark-magenta`, `dark-cyan`, `grey`, `dark-grey`, `red`, `green`, `yellow`,
`blue`, `magenta`, `cyan`, `white`.

### Mime-type styles

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

### Custom commands

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
commands are **not** tracked by undo/redo.

---

## `keys.toml`

Bindings map a command name to one or more key sequences. A sequence can be a
single key, several keys typed in a row, or a whole word:

```toml
[general]
search = [ "/", "search", "f" ]   # any of these triggers "search"
```

Modifiers use the prefixes `ctrl-`, `alt-` and `meta-` (e.g. `ctrl-r`).

Categories: `[general]`, `[movement]`, `[jump_marks]`, `[tabs]`,
`[manipulation]`. See [`examples/keys.toml`](../examples/keys.toml) for every
available command with a comment. A few notable ones:

- **Tabs & split view** (`[tabs]`): `toggle_split`, `focus_next`, `new_tab`,
  `close_tab`, `focus_tab_1`…`focus_tab_4`.
- **Undo/redo** (`[manipulation]`): `undo`, `redo`.
- **Trash** (`[general]`): `view_trash`.

These are opt-in — pre-existing configs won't have the tab, undo/redo or
jump-mark bindings until you add them (or copy the shipped example).

### Jump-marks

**Static** jump-marks are directory shortcuts under `[movement]`:

```toml
[movement]
jump_to = [ ["gh", "~"],
            ["gc", "~/.config"],
            ["gr", "/"] ]
```

Each entry is `["KEYS", "DIRECTORY"]`. `~` expands to `$HOME`; environment
variables are **not** supported.

**Vim-style** session jump-marks are configured under `[jump_marks]` (the whole
section is optional; defaults are `m` / `'`):

```toml
[jump_marks]
set  = [ "m" ]   # m<letter> remembers the current location
jump = [ "'" ]   # '<letter> returns to it
```

---

## `open.toml`

Configures which application opens which file, keyed by mime-type. Everything is
commented out by default, in which case rfm falls back to your system's default
application.

```toml
[text]
default = { name = "vim", args = [], terminal = true }

[application]
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
