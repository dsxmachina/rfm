# &#128448; rfm - A fast file-manager written in pure rust

[![Build](https://github.com/dsxmachina/rfm/actions/workflows/build.yml/badge.svg)](https://github.com/dsxmachina/rfm/actions/workflows/build.yml)
[![crates.io](https://badgers.space/crates/info/rfm-bin)](https://crates.io/crates/rfm-bin)

## &#9993; Brief description

**rfm** is a terminal file manager with VI-bindings (although you can configure the keybindings to whatever you like).
It shares a lot of similarity with [*ranger*](https://github.com/ranger/ranger), but also has some major differences in handling.

You can find a detailed list of features below.

Please note: rfm is considered beta. However, I use it as my daily filemanager for over a year now, without any problems. 
But in case you encounter something suspicious while using it, please open an issue or pull request.

### How does it look ?

Depending on your color configuration, something like this:

![rfm image](/img/rfm-img.png)

## &#128187; Installation

You can directly install `rfm` via cargo:
``` shell
cargo install rfm-bin
```

Or via nix flake:

``` shell
# Run it to see if you like it
nix run github:dsxmachina/rfm

# Install for user profile
nix profile install github:dsxmachina/rfm
```

Alternatively you can build everything from scratch:

``` shell
git clone https://github.com/dsxmachina/rfm
cd rfm
# Grab a coffee, while cargo is building: ☕
cargo build --release
# Copy the binary to some directory in your $PATH:
cp target/release/rfm /usr/local/bin/rfm
```

### Advanced Previews

rfm delivers text-file and image previews out-of-the-box without any additional dependencies.
However, for some file-types you can automatically get previews aswell, if certain programs are installed on your system.

#### Audio- & Video-Files

To display information about audio- and video-files, rfm relies on `mediainfo`.

You can install it via your distribution's package-manager:

``` shell
# Ubuntu
sudo apt install mediainfo

# Arch
sudo pacman -S mediainfo

# Nix
nix-env -iA nixpkgs.mediainfo
```

Note: `mediainfo` is also used as a preview engine for some `application/*` mime-types.

#### Syntax highlighting in Text-Previews

To get syntax highlighting in text-file previews, you can install `bat` via your package manager:

``` shell
# Ubuntu
sudo apt install bat

# Arch
sudo pacman -S bat

# Nix
nix-env -iA nixpkgs.bat
```

If `bat` is not present, text previews are generated with rfm's internal preview engine.

#### Tar- & Zip-Archives

For previews of `.tar.gz` or `.tar` files, `tar` must be present on your system.
Similarly, for previews of `.zip` files, `zip` must be installed.

You can install both via your distribution's package-manager:
``` shell
# Ubuntu
sudo apt install tar
sudo apt install zip

# Arch
sudo pacman -S tar
sudo pacman -S zip

# Nix
nix-env -iA nixpkgs.gnutar
nix-env -iA nixpkgs.zip
```

## &#128462; Configuration 

There are three configuration files 

- `config.toml` configures general behaviour and colors
- `keys.toml` for keyboard configuration and jump-marks
- `open.toml` to configure how to open files based on mime-type and/or extension

All files are located under `$XDG_CONFIG_DIR/rfm/` (which is usually `$HOME/.config/rfm`).
In case they are not present, they will be created automatically on the first start of rfm.

You can take a look at the config files, they contain a lot of comments and examples.

In the `open.toml`, eveything is commented out by default. If you don't set a specific application
in the `open.toml` for some mime-type, `rfm` will use the default application that is configured by your system.
In case you use a lot of terminal applications, it is highly recommended that you change the configuration to your liking.
Terminal applications can be inlined into your current terminal session if configured correctly.

## &#9000; Basic functions

A small and non-exhaustive overview of some basic features:

### Directory manipulation as keybindings

The following commands are accessible as basic keybindings (meaning you can just type into the application to execute them, without opening a console):

- Create a new directory (mkdir)
- Create a new file (touch)
- Rename a file or directory (rename)
- Delete a file or directory (delete)

Note: You can change the keybindings for this.

### Jump-marks

You can define custom jump-marks and bind them to any key-combination you want.
Jump-marks are defined in the `keys.toml` config file under `movement`:

```
[movement]
# ...
jump_to = [ ["gh", "~"],
            ["gc", "~/.config"],
            ["gr", "/"],
            ["ge", "/etc"],
            ["gu", "/usr"] ]
```

The `jump_to` attribute takes a list of tuples, where each tuple is a jump-mark defined as `["KEYS", "DIRECTORY_TO_JUMP_TO"]`.

### Marking files

The default binding for marking files is `space`.
You can jump around all marked files by hitting `n` or `N` (again, default bindings).
If you execute a cut, copy or delete operation, it is executed on all marked files.

Note: You can only mark files in the current direcory. If you leave the directory, all files are automatically unmarked.

### Searching

The default bindings for searching are `f`, `/` and `ctrl+f`.
You can search for files in the current directory. The search is case-insensitive.
The middle panel will only show files that match the current search pattern, while you are still typing.
When you hit `Enter` all files that match the desired pattern are automatically marked (so you can jump between them,
or execute a cut, copy or delete operation on them).

### Fast cd

Type `cd` and see what happens. You can use `tab` to toggle the recommendation.
The completion is quite similar to the one you find in`emacs`, so if you are used to that you should feel at home.

### Preview-Engine

There is a simple preview engine, that generates text previews of the currently selected file.
For images and text there is an inbuilt system to do it - for other mime-types the application relies on *mediainfo*.

### Trash

This feature is currently experimental and can be activated by setting the `use_trash` value in the `config.toml`:
``` toml
[general]
use_trash = true
```

If the trash is activated, deleting a file does not really delete it, but move it into a temporary *trash* directory.
This allows you to "undo" the delete operation, because you can always copy the files or directory from the trash to their original location.
The trash diretory will be deleted automatically if you close rfm, so you don't accidentely clutter your file-system with a lot of trash files.

Please Note: When you delete a file, that is not on the same disk than your temp directory, it will copy the file and thus be expensive.
You should be aware of this before activating !

### cd into the current directory on exit

If you leave rfm, you can make your shell jump into the current directory that the file-manager was in, 
by adding the following to your `.bashrc` (or `.zshrc` or whatever shell you use):

``` shell
function rfm-cd {
    # create a temp file and store the name
    tempfile="$(mktemp -t tmp.XXXXXX)"

    # run ranger and ask it to output the last path into the
    # temp file
    rfm --choosedir="$tempfile"

    # if the temp file exists read and the content of the temp
    # file was not equal to the current path
    test -f "$tempfile" &&
    if [ "$(cat -- "$tempfile")" != "$(echo -n `pwd`)" ]; then
        # change directory to the path in the temp file
        cd -- "$(cat "$tempfile")"
    fi

    # its not super necessary to have this line for deleting
    # the temp file since Linux should handle it on the next
    # boot
    rm -f -- "$tempfile"
}

alias rfm=rfm-cd
```

This is completely similar to ranger, so you can replace `ranger` with `rfm` in your `ranger-cd` function, and everything will work out-of-the-box.

## Design choices

The main design goals behind **rfm** are speed and simplicity:

- nothing should interrupt your workflow, you should almost never wait for the application to finish some task
- the mental load while using the application should be as low as possible, so no different "modes" where keys have different meanings
- everything should be un-doable (because if you go fast, you may go wrong)
- the application should have as little dependencies as possible

I absolutely *love* ranger and have a lot of admiration for it. 
However, if you work with large directories, ranger tends to become slow and unresponsive (because it is written in python) - which bugs me a lot.

Another thing that I found unintuitive is the seperation between console commands and normal commands that you can use with keybindings.
In my opinion, all the standard features should be accessible in the same way to reduce the overall mental load
(e.g. if you want to create a directory in ranger - which is a common task if you work with a file-manager - you have to enter console mode by hitting ":" and then type
"mkdir"; but functions like searching, movement and jumping around are accessible by just typing into the application).

## Features

A list of features that are planned to be implemented:

- [X] Basic Preview Engine for images without adding extra dependencies
- [X] Basic shell operations (move, delete, rename, touch, mkdir) on files and directories
- [X] Easy "cd" mode with smart autocompletion
- [X] Searchable panels + highlight items matching search patterns
- [x] Syntax highlighting in text previews
- [ ] Bulkrename with smart security checks
- [ ] Undo-Stack, that can undo every operation of the file-manager (even delete and other shell operations)
- [ ] config for custom shell commands / invoking external programs
- [x] basic interaction with archives
- [ ] execution of external commands (like zip and tar) in a separate thread
- [x] simple color configuration
