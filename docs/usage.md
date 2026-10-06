# Usage

This page is a tour of rfm's features and their default keybindings. Every key
here is configurable under `[keys.*]` in `config.toml` — see
[configuration.md](configuration.md).

Keys that look like words (`cd`, `rename`, `delete`, `mkdir`) are typed directly
into the application, just like single keys — there is no separate "console mode".
This keeps the mental load low: creating a directory, searching, and moving
around all work the same way.

## Navigation

rfm uses **Miller columns**: the parent directory on the left, the current
directory in the middle, and a preview of the selected entry on the right.

| Key | Action |
| --- | --- |
| `h` `j` `k` `l` | left / down / up / right |
| `gg` / `G` | jump to top / bottom |
| `ctrl-f` / `ctrl-b` | page forward / backward |
| `ctrl-d` / `ctrl-u` | half-page forward / backward |
| `''` | jump back to the previous directory |
| `zh` | toggle hidden files |

## Tabs & split view

rfm supports up to **four tabs**, each with its own directory, selection and
history. In addition to the normal single (Miller-columns) view, two tabs can be
shown side by side in a vifm-style **split view**.

| Key | Action |
| --- | --- |
| `gn` | open a new tab at the current directory |
| `Tab` | cycle focus to the next tab |
| `1`–`4` | focus the N-th tab |
| `!` | toggle split view (auto-creates a 2nd tab if needed) |
| `q` / `ctrl-w` | close the focused tab (closing the last one quits rfm) |

In split view, the two focused tabs' current directories are shown next to each
other (no preview column); the focused side is bright, the other dimmed. `!`
toggles back to the full single view. All tabs stay live and watched even while
off-screen, so a background tab's listing never goes stale.

## Marking files

The default binding for marking is `space`. Cut, copy and delete operate on all
marked files; if nothing is marked, they act on the current selection.

| Key | Action |
| --- | --- |
| `space` | mark / unmark the selected item |
| `n` / `N` | jump to the next / previous marked item |

Marks are per-directory: leaving the directory clears them.

## Searching

| Key | Action |
| --- | --- |
| `/`, `f`, `ctrl-f` | start a search in the current directory |

The search is case-insensitive and filters the middle panel live as you type.
Hitting `Enter` marks every match, so you can immediately jump between them (`n` /
`N`) or run a cut / copy / delete on them.

## Changing directories

**Fast cd** — type `cd` and start typing a path. Completion works like Emacs;
`Tab` toggles the recommendation.

**Zoxide** — if [zoxide](https://github.com/ajeetdsouza/zoxide) is installed,
`CD` (shift-cd) opens a zoxide-backed jump mode. Cycle results with `Tab` /
`shift+Tab`. Directories you visit in rfm are added to the zoxide database
automatically. See [features.md](features.md#zoxide) for setup.

## Jump-marks

Two kinds of jump-marks exist:

- **Static jump-marks** — fixed shortcuts to directories, defined under
  `[keys.movement]` in `config.toml`.
  Defaults include `gh` → `~`, `gc` → `~/.config`, `gr` → `/`, `ge` → `/etc`,
  `gu` → `/usr`.
- **Vim-style jump-marks** — session-only bookmarks. Press `m<letter>` (e.g.
  `ma`) to remember the current directory and highlighted entry, and `'<letter>`
  (e.g. `'a`) to jump back. Marks use `a`–`z` and are forgotten on quit.

See [configuration.md](configuration.md#jump-marks) to customise both.

## Directory manipulation

All of these are typed like words and are reversible via [undo](#undo--redo)
(where applicable):

| Command | Action |
| --- | --- |
| `mkdir` | create a new directory |
| `touch` | create a new file |
| `rename` | rename the selected item |
| `rename` (on multiple marked items) | bulk-rename in your editor |
| `dd` / `yy` | cut / copy marked items |
| `pp` | paste (`po` to overwrite existing files) |
| `delete` | delete marked items (to trash, if enabled) |
| `zip` / `tar` | create an archive from marked items |
| `extract` | extract an archive in place |

**Bulk-rename**: mark several files and run `rename` — rfm opens the list in your
editor, and applying the edited names is recorded as a single undoable
transaction. Temporary names are used internally so renames that swap or shift
names don't collide.

## Undo / Redo

rfm keeps an in-session undo/redo stack, so if you go fast and go wrong you can
step straight back.

| Key | Action |
| --- | --- |
| `u` | undo the last operation |
| `ctrl-r` | redo |

Each user action (rename, create, move, copy, delete, bulk-rename) is recorded as
one reversible transaction, so a single `u` takes the whole action back.
Deleting to the [trash](#trash) is undoable **and** redoable. A **permanent**
delete (`use_trash = false`) is not reversible; undo stops at it rather than
reverting past it. External commands and opening files in other programs are not
tracked.

The stack lives only for the current session — it is not persisted across
restarts.

> `u` / `ctrl-r` are the default bindings and apply to pre-existing configs
> too — unless one of your own bindings uses the key, in which case yours
> wins and the default is dropped (with a startup notice). Unbind explicitly
> with `undo = []` / `redo = []` under `[keys.manipulation]`.

## Trash

By default, deleting a file moves it to the
[freedesktop.org trash](https://specifications.freedesktop.org/trash-spec/trashspec-1.0.html)
instead of removing it permanently. This is the same trash your desktop
environment uses, so deletes survive across sessions and show up in tools like
Nautilus, `gio` or `trash-cli` (and vice versa). Because the trash lives on the
same disk as each file, deleting stays a cheap rename even across multiple mounts.

Trash is controlled by `use_trash` in `config.toml` (enabled by default). With it
off, deletes are permanent and cannot be undone.

Press `gT` to open the **trash view** — a list of everything in the trash,
newest first:

| Key | Action |
| --- | --- |
| `j` / `k` | move the cursor |
| `r` | restore the item under the cursor to its original location |
| `q` / `Esc` | close the view |

Restoring keeps the view open and refreshes it, so you can restore several items
in a row.

## Custom shell commands

You can bind your own shell commands to keys via the `[commands]` section of
`config.toml`. The `$@` placeholder is replaced with the marked (or selected)
paths, shell-escaped:

```toml
[commands.open_in_editor]
keys = [ "e" ]
cmd  = "nvim $@"
interactive = true          # runs in the foreground (needs the terminal)

[commands.git_status]
keys = [ "gs" ]
cmd  = "git status > /tmp/rfm-git.txt"
# interactive defaults to false -> runs in the background
```

By default a command runs in the background (queued, non-blocking). Set
`interactive = true` for commands that need the terminal (editors, pagers). Use
the `separator` key to change how multiple paths are joined (default: a space).
See [configuration.md](configuration.md#custom-commands) for the full field
reference.

## Previews

Text and image previews work out of the box with no extra dependencies. For
audio/video metadata, syntax-highlighted text, and archive listings, rfm can use
optional external programs — see [features.md](features.md#previews).

## Leaving rfm in the current directory

You can have your shell `cd` into rfm's last directory on exit — see
[features.md](features.md#cd-into-rfms-last-directory-on-exit).
