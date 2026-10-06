# &#128448; rfm - A fast file-manager written in pure rust

[![Build](https://github.com/dsxmachina/rfm/actions/workflows/build.yml/badge.svg)](https://github.com/dsxmachina/rfm/actions/workflows/build.yml)
[![crates.io](https://badgers.space/crates/info/rfm-bin)](https://crates.io/crates/rfm-bin)

**rfm** is a terminal file manager with Miller-columns and VI-style keybindings,
built for speed. It takes a lot of inspiration from
[ranger](https://github.com/ranger/ranger) and [vifm](https://vifm.info/), while
staying snappy on huge directories and keeping the mental load low — there is no
separate "console mode", you just type commands straight into the application.

> rfm is beta, but has been in daily use for years without trouble. If something
> looks off, please open an issue or PR.

![rfm image](/img/rfm-img.png)

## Highlights

- ⚡ **Fast & responsive**, even on large directories — nothing blocks your workflow
- 🗂 **Miller-columns** layout with instant text & image previews (no dependencies required)
- 🪟 **Tabs & split view** — up to four tabs, plus a vifm-style side-by-side split
- ↩️ **Undo / redo** for almost every file operation
- 🗑 **Freedesktop trash** integration with a built-in trash browser (`gT`)
- 📌 **Jump-marks** — static directory shortcuts *and* vim-style `m`/`'` bookmarks
- 🎨 **Mime-type styling** with colors and optional Nerd Font icons
- 🔎 Live **search**, Emacs-style **`cd`** completion and **zoxide** support
- 🛠 **Custom shell commands** bound to your own keys
- 🔧 Fully **configurable** keybindings, colors and file associations

## Installation

Via cargo:

```shell
cargo install rfm-bin
```

Via nix flake:

```shell
nix run github:dsxmachina/rfm             # try it
nix profile install github:dsxmachina/rfm # install
```

From source:

```shell
git clone https://github.com/dsxmachina/rfm
cd rfm
cargo build --release
cp target/release/rfm /usr/local/bin/rfm
```

## Keybindings at a glance

| Key | Action | | Key | Action |
| --- | --- | --- | --- | --- |
| `h` `j` `k` `l` | move around | | `space` | mark / unmark |
| `gg` / `G` | top / bottom | | `n` / `N` | next / prev marked |
| `/` `f` | search | | `dd` / `yy` / `pp` | cut / copy / paste |
| `cd` | change directory | | `rename` `mkdir` `touch` | manipulate |
| `gn` / `Tab` | new tab / next tab | | `delete` | delete (to trash) |
| `!` | toggle split view | | `u` / `ctrl-r` | undo / redo |
| `zh` | toggle hidden | | `gT` | open trash view |
| `Q` | quit | | `q` | close tab (quits on last) |

Everything is configurable — this is just the default set. See the
[full usage guide](docs/usage.md) for the complete list.

## Configuration

rfm reads a single `~/.config/rfm/config.toml` holding **sparse overrides**:
the binary carries complete defaults, and your file contains only what you
want to change — an empty file is perfectly valid. Print the complete
annotated reference (every option with its default value) with:

```shell
rfm --dump-config
```

Older three-file configs (`config.toml` + `keys.toml` + `open.toml`) keep
working unchanged; `rfm --migrate-config` unifies them when you're ready.

See **[docs/configuration.md](docs/configuration.md)** for the full reference.

## Documentation

- **[Usage guide](docs/usage.md)** — every feature and its keybindings
- **[Configuration](docs/configuration.md)** — every config option in depth
- **[Extra features & integrations](docs/features.md)** — richer previews, zoxide, shell `cd`-on-exit
- **[Changelog](CHANGELOG.md)** — what's new in each release

## Design philosophy

- **Speed first** — you should almost never wait for the file manager. ranger is
  wonderful, but written in Python it gets sluggish on large directories.
- **Low mental load** — no modes where keys mean different things. Creating a
  directory, searching and jumping around all work the same way: just type.
- **Reversible** — if you go fast, you may go wrong, so operations are undoable.
- **Few dependencies** — the essentials work out of the box.

## License

[GPL-3.0](LICENSE)
