use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use log::trace;
use patricia_tree::StringPatriciaMap;
use serde::Deserialize;

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
const CTRL_X: KeyEvent = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
const CTRL_V: KeyEvent = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
const CTRL_F: KeyEvent = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
const CTRL_SHIFT_V: KeyEvent = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::CONTROL);

#[derive(Debug, Clone)]
pub struct ExpandedPath(PathBuf);

impl<S: AsRef<str>> From<S> for ExpandedPath {
    fn from(path: S) -> Self {
        let mut string = path.as_ref().to_string();

        // Replace with users home directory
        let home = std::env::var("HOME").unwrap_or_default();

        // Expand "~" and "$HOME"
        string = string.replace('~', &home);
        string = string.replace("$HOME", &home);

        ExpandedPath(string.into())
    }
}

impl AsRef<Path> for ExpandedPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl From<ExpandedPath> for PathBuf {
    fn from(path: ExpandedPath) -> Self {
        path.0
    }
}

#[derive(Deserialize, Debug)]
struct Manipulation {
    change_directory: Option<Vec<String>>,
    zoxide_query: Option<Vec<String>>,
    rename: Vec<String>,
    mkdir: Vec<String>,
    touch: Vec<String>,
    cut: Vec<String>,
    copy: Vec<String>,
    delete: Vec<String>,
    paste: Vec<String>,
    paste_overwrite: Vec<String>,
    zip: Vec<String>,
    tar: Vec<String>,
    extract: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Movement {
    up: Vec<String>,
    down: Vec<String>,
    left: Vec<String>,
    right: Vec<String>,
    top: Vec<String>,
    bottom: Vec<String>,
    page_forward: Vec<String>,
    page_backward: Vec<String>,
    half_page_forward: Vec<String>,
    half_page_backward: Vec<String>,
    jump_previous: Vec<String>,
    jump_to: Vec<(String, String)>,
}

#[derive(Deserialize, Debug)]
struct General {
    search: Vec<String>,
    mark: Vec<String>,
    next: Vec<String>,
    previous: Vec<String>,
    view_trash: Vec<String>,
    toggle_hidden: Vec<String>,
    toggle_log: Option<Vec<String>>,
    quit: Vec<String>,
    quit_no_cd: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
pub struct KeyConfig {
    general: General,
    movement: Movement,
    manipulation: Manipulation,
}

#[test]
fn test_split() {
    let s = "ctrl-f";
    let (_, key) = s.split_at(5);
    assert_eq!(key, "f");
}

#[derive(Debug, Clone)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    PageForward,
    PageBackward,
    HalfPageForward,
    HalfPageBackward,
    JumpTo(ExpandedPath),
    JumpPrevious,
}

/// An executable shell command
///
/// If `multi` is set to `true`, multiple files can be selected and fed into the command.
#[derive(Debug, Clone)]
pub struct ShellCmd {
    pub cmd: String,
    pub args: String,
    pub multi: bool,
}

/// Set of commands that the filemanager should perform during its runtime
#[derive(Debug, Clone)]
pub enum Command {
    Move(Move),
    Next,
    Previous,
    ToggleHidden,
    ToggleLog,
    ViewTrash,
    Zip,
    Tar,
    Shell(Box<ShellCmd>),
    Extract,
    Cd { zoxide: bool },
    Search,
    Rename,
    Mkdir,
    Touch,
    Cut,
    Copy,
    Delete,
    Paste { overwrite: bool },
    Mark,
    Quit,
    QuitWithoutPath,
    None,
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Move(m) => match m {
                Move::Up => write!(f, "move up"),
                Move::Down => write!(f, "move down"),
                Move::Left => write!(f, "move left"),
                Move::Right => write!(f, "move right"),
                Move::Top => write!(f, "move to top"),
                Move::Bottom => write!(f, "move to bottom"),
                Move::PageForward => write!(f, "page forward"),
                Move::PageBackward => write!(f, "page backward"),
                Move::HalfPageForward => write!(f, "half page forward"),
                Move::HalfPageBackward => write!(f, "half page backward"),
                Move::JumpTo(path) => write!(f, "{}", path.0.display()),
                Move::JumpPrevious => write!(f, "jump back"),
            },
            Command::Next => write!(f, "next match"),
            Command::Previous => write!(f, "previous match"),
            Command::ToggleHidden => write!(f, "toggle hidden files"),
            Command::ToggleLog => write!(f, "toggle developer log"),
            Command::ViewTrash => write!(f, "go to trash"),
            Command::Zip => write!(f, "zip selected items"),
            Command::Tar => write!(f, "tar selected items"),
            Command::Shell(inner) => write!(f, "execute {} {} on selection", inner.cmd, inner.args),
            Command::Extract => write!(f, "extract selected archive"),
            Command::Cd { .. } => write!(f, "enter 'cd' mode"),
            Command::Search => write!(f, "search for items"),
            Command::Rename => write!(f, "rename selected items"),
            Command::Mkdir => write!(f, "create a new directory"),
            Command::Touch => write!(f, "create a new file"),
            Command::Cut => write!(f, "cut selected items"),
            Command::Copy => write!(f, "copy selected items"),
            Command::Delete => write!(f, "delete selected items"),
            Command::Paste { overwrite } => {
                if *overwrite {
                    write!(f, "paste and overwrite")
                } else {
                    write!(f, "paste without overwrite")
                }
            }
            Command::Mark => write!(f, "mark selected item"),
            Command::Quit => write!(f, "quit"),
            Command::QuitWithoutPath => write!(f, "quit without changing path"),
            Command::None => write!(f, "no command"),
        }
    }
}

/// Set of commands that the filemanager should perform just before closing
pub enum CloseCmd {
    QuitWithPath { path: PathBuf },
    QuitErr { error: &'static str },
    Quit,
}

/// Takes the incoming key-events, and returns the corresponding command.
///
/// Uses a `StringPatriciaMap` to match patterns of keystrokes,
/// and a normal `HashMap` to match "oneshot"-commands,
/// that don't require any key combinations but may require a modifier.
pub struct CommandParser {
    key_commands: StringPatriciaMap<Command>,
    mod_commands: HashMap<KeyEvent, Command>,
    buffer: String,
}

impl CommandParser {
    pub fn from_config(config: KeyConfig) -> Self {
        let mut parser = CommandParser::new();
        // General commands
        parser.insert(config.general.search, Command::Search);
        parser.insert(config.general.mark, Command::Mark);
        parser.insert(config.general.next, Command::Next);
        parser.insert(config.general.previous, Command::Previous);
        parser.insert(config.general.toggle_hidden, Command::ToggleHidden);
        parser.insert(
            config.general.toggle_log.unwrap_or_default(),
            Command::ToggleLog,
        );
        parser.insert(config.general.view_trash, Command::ViewTrash);
        parser.insert(config.general.quit, Command::Quit);
        if let Some(quit_cmd) = config.general.quit_no_cd {
            parser.insert(quit_cmd, Command::QuitWithoutPath);
        }

        // Movement commands
        parser.insert(config.movement.up, Command::Move(Move::Up));
        parser.insert(config.movement.down, Command::Move(Move::Down));
        parser.insert(config.movement.left, Command::Move(Move::Left));
        parser.insert(config.movement.right, Command::Move(Move::Right));
        parser.insert(config.movement.top, Command::Move(Move::Top));
        parser.insert(config.movement.bottom, Command::Move(Move::Bottom));
        parser.insert(
            config.movement.page_forward,
            Command::Move(Move::PageForward),
        );
        parser.insert(
            config.movement.page_backward,
            Command::Move(Move::PageBackward),
        );
        parser.insert(
            config.movement.half_page_forward,
            Command::Move(Move::HalfPageForward),
        );
        parser.insert(
            config.movement.half_page_backward,
            Command::Move(Move::HalfPageBackward),
        );
        parser.insert(
            config.movement.jump_previous,
            Command::Move(Move::JumpPrevious),
        );
        for (keys, path) in config.movement.jump_to {
            parser
                .key_commands
                .insert(keys, Command::Move(Move::JumpTo(path.into())));
        }
        // Manipulation commands
        parser.insert(
            config.manipulation.change_directory.unwrap_or_default(),
            Command::Cd { zoxide: false },
        );
        parser.insert(
            config.manipulation.zoxide_query.unwrap_or_default(),
            Command::Cd { zoxide: true },
        );
        parser.insert(config.manipulation.rename, Command::Rename);
        parser.insert(config.manipulation.mkdir, Command::Mkdir);
        parser.insert(config.manipulation.touch, Command::Touch);
        parser.insert(config.manipulation.cut, Command::Cut);
        parser.insert(config.manipulation.copy, Command::Copy);
        parser.insert(config.manipulation.delete, Command::Delete);
        parser.insert(config.manipulation.zip, Command::Zip);
        parser.insert(config.manipulation.tar, Command::Tar);
        parser.insert(config.manipulation.extract, Command::Extract);
        parser.insert(
            config.manipulation.paste,
            Command::Paste { overwrite: false },
        );
        parser.insert(
            config.manipulation.paste_overwrite,
            Command::Paste { overwrite: true },
        );

        parser
    }

    pub fn new() -> Self {
        let mut mod_commands = HashMap::new();
        // Insert basic arrow key movement
        mod_commands.insert(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            Command::Move(Move::Up),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Command::Move(Move::Down),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            Command::Move(Move::Left),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            Command::Move(Move::Right),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Command::Move(Move::PageBackward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Command::Move(Move::PageForward),
        );
        CommandParser {
            key_commands: StringPatriciaMap::new(),
            mod_commands,
            buffer: "".to_string(),
        }
    }

    fn insert(&mut self, bindings: Vec<String>, cmd: Command) {
        for b in bindings {
            // Check if b starts with "ctrl"
            if b.starts_with("ctrl-") {
                let (_, key) = b.split_at(5);
                if key.is_empty() {
                    continue;
                }
                self.mod_commands.insert(
                    KeyEvent::new(
                        KeyCode::Char(key.chars().next().unwrap()),
                        KeyModifiers::CONTROL,
                    ),
                    cmd.clone(),
                );
            } else if b.starts_with("alt-") {
                let (_, key) = b.split_at(4);
                if key.is_empty() {
                    continue;
                }
                self.mod_commands.insert(
                    KeyEvent::new(
                        KeyCode::Char(key.chars().next().unwrap()),
                        KeyModifiers::ALT,
                    ),
                    cmd.clone(),
                );
            } else if b.starts_with("meta-") {
                let (_, key) = b.split_at(5);
                if key.is_empty() {
                    continue;
                }
                self.mod_commands.insert(
                    KeyEvent::new(
                        KeyCode::Char(key.chars().next().unwrap()),
                        KeyModifiers::META,
                    ),
                    cmd.clone(),
                );
            } else {
                self.key_commands.insert(b, cmd.clone());
            }
        }
    }

    pub fn default_bindings() -> Self {
        // --- Commands for "normal" keys:
        let mut key_commands = StringPatriciaMap::new();
        // Basic movement commands
        key_commands.insert("h", Command::Move(Move::Left));
        key_commands.insert("j", Command::Move(Move::Down));
        key_commands.insert("k", Command::Move(Move::Up));
        key_commands.insert("l", Command::Move(Move::Right));

        key_commands.insert("gg", Command::Move(Move::Top));
        key_commands.insert("G", Command::Move(Move::Bottom));

        // Jump to something
        key_commands.insert("gh", Command::Move(Move::JumpTo("~".into())));
        key_commands.insert("gr", Command::Move(Move::JumpTo("/".into())));
        key_commands.insert("gc", Command::Move(Move::JumpTo("~/.config".into())));

        key_commands.insert("ge", Command::Move(Move::JumpTo("/etc".into())));
        key_commands.insert("gu", Command::Move(Move::JumpTo("/usr".into())));
        key_commands.insert("gN", Command::Move(Move::JumpTo("/nix/store".into())));

        // custom jumps
        key_commands.insert("gp", Command::Move(Move::JumpTo("~/Projekte".into())));
        key_commands.insert("gs", Command::Move(Move::JumpTo("~/.scripts".into())));
        key_commands.insert("gb", Command::Move(Move::JumpTo("~/Bilder".into())));
        key_commands.insert(
            "gw",
            Command::Move(Move::JumpTo("~/Bilder/wallpapers".into())),
        );
        key_commands.insert("gd", Command::Move(Move::JumpTo("~/Dokumente".into())));
        key_commands.insert("gD", Command::Move(Move::JumpTo("~/Downloads".into())));
        key_commands.insert(
            "gl",
            Command::Move(Move::JumpTo("~/Projekte/loadrunner-2021".into())),
        );
        key_commands.insert(
            "gL",
            Command::Move(Move::JumpTo(
                "~/Projekte/loadrunner-2021/lr-localization".into(),
            )),
        );
        key_commands.insert("gm", Command::Move(Move::JumpTo("~/Musik".into())));
        key_commands.insert("gN", Command::Move(Move::JumpTo("/nix/store".into())));
        key_commands.insert("gT", Command::ViewTrash);

        // Toggle hidden files
        key_commands.insert("zh", Command::ToggleHidden);

        // Toggle log visibility
        key_commands.insert("devlog", Command::ToggleLog);

        // Jump to previous location
        key_commands.insert("\'\'", Command::Move(Move::JumpPrevious));

        // Mark current file
        key_commands.insert(" ", Command::Mark);

        // Copy, Paste, Cut, Delete
        key_commands.insert("yy", Command::Copy);
        key_commands.insert("copy", Command::Copy);
        key_commands.insert("dd", Command::Cut);
        key_commands.insert("cut", Command::Cut);
        key_commands.insert("pp", Command::Paste { overwrite: false });
        key_commands.insert("paste", Command::Paste { overwrite: false });
        key_commands.insert("po", Command::Paste { overwrite: true });
        key_commands.insert("delete", Command::Delete);

        // Search
        key_commands.insert("/", Command::Search);
        key_commands.insert("n", Command::Next);
        key_commands.insert("N", Command::Previous);

        // cd, mkdir, touch
        key_commands.insert("cd", Command::Cd { zoxide: false });
        key_commands.insert("mkdir", Command::Mkdir);
        key_commands.insert("touch", Command::Touch);

        // Rename
        key_commands.insert("rename", Command::Rename);

        // Quit
        key_commands.insert("q", Command::Quit);

        // --- Commands for modifier + key:
        let mut mod_commands = HashMap::new();

        // Search
        mod_commands.insert(CTRL_F, Command::Search);

        // Copy, Paste, Cut
        mod_commands.insert(CTRL_C, Command::Copy);
        mod_commands.insert(CTRL_X, Command::Cut);
        mod_commands.insert(CTRL_V, Command::Paste { overwrite: false });
        mod_commands.insert(CTRL_SHIFT_V, Command::Paste { overwrite: true });

        // Escape from what you are doing
        // mod_commands.insert(CTRL_C, Command::Esc);

        // Advanced movement
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            Command::Move(Move::PageForward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Command::Move(Move::PageBackward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Command::Move(Move::HalfPageForward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Command::Move(Move::HalfPageBackward),
        );

        // Toggle hidden (backspace)
        // mod_commands.insert(
        //     KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        //     Command::ToggleHidden,
        // );

        CommandParser {
            key_commands,
            mod_commands,
            buffer: "".to_string(),
        }
    }

    pub fn buffer(&self) -> String {
        self.buffer.clone()
    }

    pub fn matching_commands(&self) -> Vec<(String, String)> {
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            self.key_commands
                .iter_prefix(&self.buffer)
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect()
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Parse an event and return the command that is assigned to it
    pub fn add_event(&mut self, event: KeyEvent) -> Command {
        if let KeyCode::Backspace = event.code {
            self.buffer.pop();
            return Command::None;
        }
        match event.modifiers {
            // First parse for "normal" characters:
            KeyModifiers::NONE | KeyModifiers::SHIFT => {
                // Put character into buffer
                if let KeyCode::Char(c) = event.code {
                    if event.modifiers.contains(KeyModifiers::SHIFT) {
                        // uppercase
                        self.buffer.push(c.to_ascii_uppercase());
                    } else {
                        // lowercase
                        self.buffer.push(c.to_ascii_lowercase());
                    }
                }

                // Check if there are commands with that prefix
                if self.key_commands.iter_prefix(&self.buffer).count() == 0 {
                    self.buffer.clear();
                    return Command::None;
                }

                // Check if we have a valid command
                if let Some(command) = self.key_commands.get(&self.buffer) {
                    self.buffer.clear();
                    trace!("Command: {:?}", command);
                    return command.clone();
                }
            }
            _ => {}
        }
        // If we have not returned yet,
        // always check if there is a oneshot command assigned to the
        // incoming event.
        if let Some(command) = self.mod_commands.get(&event) {
            self.buffer.clear();
            trace!("Command: {:?}", command);
            return command.clone();
        }
        Command::None
    }
}
