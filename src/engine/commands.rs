use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use log::trace;
use patricia_tree::StringPatriciaMap;
use serde::Deserialize;

const DEFAULT_SET_MARK_PREFIX: &str = "m";
const DEFAULT_JUMP_MARK_PREFIX: &str = "'";

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
const CTRL_X: KeyEvent = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
const CTRL_V: KeyEvent = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL);
const CTRL_F: KeyEvent = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);
const CTRL_SHIFT_V: KeyEvent = KeyEvent::new(KeyCode::Char('V'), KeyModifiers::CONTROL);

/// Map a named special-key binding string to its [`KeyCode`]. These keys don't
/// emit a `Char`, so they can't be matched via the keystroke buffer; the parser
/// binds them as oneshot events instead. Returns `None` for ordinary strings.
fn named_key(s: &str) -> Option<KeyCode> {
    match s {
        "Tab" => Some(KeyCode::Tab),
        "BackTab" => Some(KeyCode::BackTab),
        "Enter" => Some(KeyCode::Enter),
        _ => None,
    }
}

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

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Manipulation {
    pub change_directory: Option<Vec<String>>,
    pub zoxide_query: Option<Vec<String>>,
    pub rename: Option<Vec<String>>,
    pub mkdir: Option<Vec<String>>,
    pub touch: Option<Vec<String>>,
    pub cut: Option<Vec<String>>,
    pub copy: Option<Vec<String>>,
    pub delete: Option<Vec<String>>,
    pub paste: Option<Vec<String>>,
    pub paste_overwrite: Option<Vec<String>>,
    pub zip: Option<Vec<String>>,
    pub tar: Option<Vec<String>>,
    pub extract: Option<Vec<String>>,
    pub undo: Option<Vec<String>>,
    pub redo: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Movement {
    pub up: Option<Vec<String>>,
    pub down: Option<Vec<String>>,
    pub left: Option<Vec<String>>,
    pub right: Option<Vec<String>>,
    pub top: Option<Vec<String>>,
    pub bottom: Option<Vec<String>>,
    pub page_forward: Option<Vec<String>>,
    pub page_backward: Option<Vec<String>>,
    pub half_page_forward: Option<Vec<String>>,
    pub half_page_backward: Option<Vec<String>>,
    pub jump_previous: Option<Vec<String>>,
    pub jump_to: Option<Vec<(String, String)>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Tabs {
    pub toggle_split: Option<Vec<String>>,
    pub focus_next: Option<Vec<String>>,
    pub new_tab: Option<Vec<String>>,
    pub close_tab: Option<Vec<String>>,
    // Four explicit `focus_tab_N` fields because the config layer is a plain
    // string -> Command table with no argument form; this mirrors the
    // `MAX_TABS = 4` cap in the panel manager. To support a 5th tab, add
    // `focus_tab_5` here (+ its parser line) AND bump `MAX_TABS`.
    pub focus_tab_1: Option<Vec<String>>,
    pub focus_tab_2: Option<Vec<String>>,
    pub focus_tab_3: Option<Vec<String>>,
    pub focus_tab_4: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct JumpMarks {
    /// Prefix key(s) for setting a mark (default: `m`).
    pub set: Option<Vec<String>>,
    /// Prefix key(s) for jumping to a mark (default: `'`).
    pub jump: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct General {
    pub search: Option<Vec<String>>,
    pub mark: Option<Vec<String>>,
    pub next: Option<Vec<String>>,
    pub previous: Option<Vec<String>>,
    pub view_trash: Option<Vec<String>>,
    pub toggle_hidden: Option<Vec<String>>,
    pub toggle_log: Option<Vec<String>>,
    pub quit: Option<Vec<String>>,
    pub quit_no_cd: Option<Vec<String>>,
}

/// Keybinding configuration. Doubles as the *user overlay* type (every
/// field `None` = "use default", `Some(vec![])` = explicit unbind) and,
/// parsed from the embedded defaults file, as the all-`Some` defaults
/// instance.
#[derive(Deserialize, Debug, Default)]
pub struct KeyConfig {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub movement: Movement,
    #[serde(default)]
    pub manipulation: Manipulation,
    #[serde(default)]
    pub jump_marks: JumpMarks,
    #[serde(default)]
    pub tabs: Tabs,
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
    Extract,
    Cd {
        zoxide: bool,
    },
    Search,
    Rename,
    Mkdir,
    Touch,
    Cut,
    Copy,
    Delete,
    Paste {
        overwrite: bool,
    },
    Mark,
    /// Set a jump-mark (session-only) at the current location.
    SetJumpMark(char),
    /// Jump to a previously-set jump-mark.
    JumpToMark(char),
    Undo,
    Redo,
    /// Toggle split-view (stub until Task 7).
    ToggleSplit,
    /// Cycle focus forward through the open tabs.
    FocusNext,
    /// Open a new tab rooted at the focused tab's cwd.
    NewTab,
    /// Close the focused tab.
    CloseTab,
    /// Focus the `n`-th tab (1-based as configured; 0-based at dispatch).
    FocusTab(usize),
    Quit,
    QuitWithoutPath,
    /// User-defined shell command
    UserCommand {
        /// Display name
        name: String,
        /// Shell command template (with $@ placeholder)
        cmd: String,
        /// Run in foreground (suspend terminal)
        interactive: bool,
        /// Separator for $@ expansion
        separator: String,
    },
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
            Command::SetJumpMark(c) => write!(f, "set jump-mark '{c}'"),
            Command::JumpToMark(c) => write!(f, "jump to mark '{c}'"),
            Command::Undo => write!(f, "undo"),
            Command::Redo => write!(f, "redo"),
            Command::ToggleSplit => write!(f, "toggle split view"),
            Command::FocusNext => write!(f, "focus next tab"),
            Command::NewTab => write!(f, "new tab"),
            Command::CloseTab => write!(f, "close tab"),
            Command::FocusTab(n) => write!(f, "focus tab {n}"),
            Command::Quit => write!(f, "quit"),
            Command::QuitWithoutPath => write!(f, "quit without changing path"),
            Command::UserCommand { name, .. } => write!(f, "{}", name),
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
        parser.insert(config.general.search.unwrap_or_default(), Command::Search);
        parser.insert(config.general.mark.unwrap_or_default(), Command::Mark);
        parser.insert(config.general.next.unwrap_or_default(), Command::Next);
        parser.insert(
            config.general.previous.unwrap_or_default(),
            Command::Previous,
        );
        parser.insert(
            config.general.toggle_hidden.unwrap_or_default(),
            Command::ToggleHidden,
        );
        parser.insert(
            config.general.toggle_log.unwrap_or_default(),
            Command::ToggleLog,
        );
        parser.insert(
            config.general.view_trash.unwrap_or_default(),
            Command::ViewTrash,
        );
        parser.insert(config.general.quit.unwrap_or_default(), Command::Quit);
        parser.insert(
            config.general.quit_no_cd.unwrap_or_default(),
            Command::QuitWithoutPath,
        );

        // Movement commands
        parser.insert(
            config.movement.up.unwrap_or_default(),
            Command::Move(Move::Up),
        );
        parser.insert(
            config.movement.down.unwrap_or_default(),
            Command::Move(Move::Down),
        );
        parser.insert(
            config.movement.left.unwrap_or_default(),
            Command::Move(Move::Left),
        );
        parser.insert(
            config.movement.right.unwrap_or_default(),
            Command::Move(Move::Right),
        );
        parser.insert(
            config.movement.top.unwrap_or_default(),
            Command::Move(Move::Top),
        );
        parser.insert(
            config.movement.bottom.unwrap_or_default(),
            Command::Move(Move::Bottom),
        );
        parser.insert(
            config.movement.page_forward.unwrap_or_default(),
            Command::Move(Move::PageForward),
        );
        parser.insert(
            config.movement.page_backward.unwrap_or_default(),
            Command::Move(Move::PageBackward),
        );
        parser.insert(
            config.movement.half_page_forward.unwrap_or_default(),
            Command::Move(Move::HalfPageForward),
        );
        parser.insert(
            config.movement.half_page_backward.unwrap_or_default(),
            Command::Move(Move::HalfPageBackward),
        );
        parser.insert(
            config.movement.jump_previous.unwrap_or_default(),
            Command::Move(Move::JumpPrevious),
        );
        for (keys, path) in config.movement.jump_to.unwrap_or_default() {
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
        parser.insert(
            config.manipulation.rename.unwrap_or_default(),
            Command::Rename,
        );
        parser.insert(
            config.manipulation.mkdir.unwrap_or_default(),
            Command::Mkdir,
        );
        parser.insert(
            config.manipulation.touch.unwrap_or_default(),
            Command::Touch,
        );
        parser.insert(config.manipulation.cut.unwrap_or_default(), Command::Cut);
        parser.insert(config.manipulation.copy.unwrap_or_default(), Command::Copy);
        parser.insert(
            config.manipulation.delete.unwrap_or_default(),
            Command::Delete,
        );
        parser.insert(config.manipulation.zip.unwrap_or_default(), Command::Zip);
        parser.insert(config.manipulation.tar.unwrap_or_default(), Command::Tar);
        parser.insert(
            config.manipulation.extract.unwrap_or_default(),
            Command::Extract,
        );
        parser.insert(config.manipulation.undo.unwrap_or_default(), Command::Undo);
        parser.insert(config.manipulation.redo.unwrap_or_default(), Command::Redo);

        // Multi-tab / split-view commands
        parser.insert(
            config.tabs.toggle_split.unwrap_or_default(),
            Command::ToggleSplit,
        );
        parser.insert(
            config.tabs.focus_next.unwrap_or_default(),
            Command::FocusNext,
        );
        parser.insert(config.tabs.new_tab.unwrap_or_default(), Command::NewTab);
        parser.insert(config.tabs.close_tab.unwrap_or_default(), Command::CloseTab);
        parser.insert(
            config.tabs.focus_tab_1.unwrap_or_default(),
            Command::FocusTab(1),
        );
        parser.insert(
            config.tabs.focus_tab_2.unwrap_or_default(),
            Command::FocusTab(2),
        );
        parser.insert(
            config.tabs.focus_tab_3.unwrap_or_default(),
            Command::FocusTab(3),
        );
        parser.insert(
            config.tabs.focus_tab_4.unwrap_or_default(),
            Command::FocusTab(4),
        );
        parser.insert(
            config.manipulation.paste.unwrap_or_default(),
            Command::Paste { overwrite: false },
        );
        parser.insert(
            config.manipulation.paste_overwrite.unwrap_or_default(),
            Command::Paste { overwrite: true },
        );

        parser.insert_jump_marks(
            config
                .jump_marks
                .set
                .unwrap_or_else(|| vec![DEFAULT_SET_MARK_PREFIX.to_string()]),
            config
                .jump_marks
                .jump
                .unwrap_or_else(|| vec![DEFAULT_JUMP_MARK_PREFIX.to_string()]),
        );

        parser
    }

    /// Add user-defined commands from config
    pub fn add_user_commands(&mut self, commands: &crate::command_queue::CommandsConfig) {
        for (name, entry) in commands {
            let cmd = Command::UserCommand {
                name: name.clone(),
                cmd: entry.cmd.clone(),
                interactive: entry.interactive,
                separator: entry.separator.clone(),
            };
            self.insert(entry.keys.clone(), cmd);
        }
    }

    /// Generate the `<set-prefix><a-z>` and `<jump-prefix><a-z>` chords for
    /// jump-marks. Prefixes are usually a single key (`m` / `'`) but any
    /// string works, mirroring `jump_to`.
    fn insert_jump_marks(&mut self, set: Vec<String>, jump: Vec<String>) {
        self.insert_chords(set, Command::SetJumpMark);
        self.insert_chords(jump, Command::JumpToMark);
    }

    /// Insert `<prefix><a-z>` chords, each producing `make(letter)`.
    /// Explicit bindings on the same keys are left untouched — the
    /// auto-generated chords always lose against them.
    fn insert_chords(&mut self, prefixes: Vec<String>, make: fn(char) -> Command) {
        for prefix in prefixes {
            for c in 'a'..='z' {
                let chord = format!("{prefix}{c}");
                if self.key_commands.get(&chord).is_none() {
                    self.key_commands.insert(chord, make(c));
                }
            }
        }
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
            } else if let Some(code) = named_key(&b) {
                // Special keys that don't produce a `Char` (e.g. Tab) can't go
                // through the buffer-string path; bind them as oneshot events.
                self.mod_commands
                    .insert(KeyEvent::new(code, KeyModifiers::NONE), cmd.clone());
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

        // Undo / Redo
        key_commands.insert("u", Command::Undo);

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

        // Redo
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
            Command::Redo,
        );

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

        let mut parser = CommandParser {
            key_commands,
            mod_commands,
            buffer: "".to_string(),
        };
        parser.insert_jump_marks(
            vec![DEFAULT_SET_MARK_PREFIX.to_string()],
            vec![DEFAULT_JUMP_MARK_PREFIX.to_string()],
        );
        parser
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
                .filter(|(k, v)| {
                    // A jump-mark chord shadowed by a longer binding can
                    // never fire (see add_event) — don't advertise it.
                    !(matches!(v, Command::SetJumpMark(_) | Command::JumpToMark(_))
                        && self.key_commands.iter_prefix(k.as_str()).count() > 1)
                })
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
                    // Jump-mark chords are auto-generated over the whole
                    // alphabet, so they must not shadow explicit bindings:
                    // if a longer binding shares this prefix (`mkdir` over
                    // `mk`), keep collecting keys and let the binding win.
                    if matches!(command, Command::SetJumpMark(_) | Command::JumpToMark(_))
                        && self.key_commands.iter_prefix(&self.buffer).count() > 1
                    {
                        return Command::None;
                    }
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
    fn longer_binding_wins_over_mark_chord() {
        let mut p = CommandParser::default_bindings();
        // "mkdir" shares the "mk" prefix with the auto-generated
        // SetJumpMark('k') chord; the explicit binding must win.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('k')), Command::None)); // defers, no mark
        assert!(matches!(p.add_event(key('d')), Command::None));
        assert!(matches!(p.add_event(key('i')), Command::None));
        assert!(matches!(p.add_event(key('r')), Command::Mkdir));
    }

    #[test]
    fn deferred_mark_chord_does_not_fire_on_mismatch() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('k')), Command::None)); // deferred
        assert!(matches!(p.add_event(key('x')), Command::None)); // buffer cleared, no mark
        assert!(matches!(p.add_event(key('j')), Command::Move(Move::Down)));
    }

    #[test]
    fn unshadowed_mark_chord_still_fires_from_default_bindings() {
        let mut p = CommandParser::default_bindings();
        // No default binding starts with "ma", so the chord fires normally.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
    }

    #[test]
    fn mark_chord_does_not_clobber_equal_length_binding() {
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
jump_to = [["ma", "/tmp"]]

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
        // "ma" is an explicit jump_to binding; the auto-generated
        // SetJumpMark('a') chord must not overwrite it.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(
            p.add_event(key('a')),
            Command::Move(Move::JumpTo(_))
        ));
    }

    #[test]
    fn shadowed_mark_chord_hidden_from_matching_commands() {
        let mut p = CommandParser::default_bindings();
        p.add_event(key('m'));
        let keys: Vec<String> = p.matching_commands().into_iter().map(|(k, _)| k).collect();
        assert!(keys.iter().any(|k| k == "mkdir"));
        assert!(keys.iter().any(|k| k == "ma"));
        assert!(!keys.iter().any(|k| k == "mk")); // dead chord, don't advertise
    }

    #[test]
    fn m_then_unbound_key_is_noop() {
        let mut p = CommandParser::default_bindings();
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('1')), Command::None)); // buffer cleared
        assert!(matches!(p.add_event(key('j')), Command::Move(Move::Down)));
    }

    #[test]
    fn from_config_without_jump_marks_section_defaults_to_m_and_apostrophe() {
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

    /// The `[tabs]` section wires its keys to the multi-tab commands: a
    /// single-key focus jump, two aliases (a plain key and a `ctrl-` chord)
    /// for `close_tab`, and `Tab` routed through the oneshot `mod_commands`
    /// map via the `named_key()` path.
    #[test]
    fn from_config_tabs_section_wires_focus_close_and_named_tab_key() {
        let toml = r#"
[general]
search = ["/"]
mark = [" "]
next = ["e"]
previous = ["N"]
view_trash = ["gT"]
toggle_hidden = ["zh"]
quit = ["Q"]

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

[tabs]
focus_next = ["Tab"]
new_tab = ["gn"]
close_tab = ["q", "ctrl-w"]
focus_tab_1 = ["1"]
"#;
        let cfg: KeyConfig = toml::from_str(toml).expect("parse keys.toml");
        let mut p = CommandParser::from_config(cfg);

        // focus_tab_1 = ["1"] -> FocusTab(1)
        assert!(matches!(p.add_event(key('1')), Command::FocusTab(1)));

        // close_tab = ["q", "ctrl-w"]: the plain 'q' key ...
        assert!(matches!(p.add_event(key('q')), Command::CloseTab));
        // ... and the ctrl-w chord both map to CloseTab.
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert!(matches!(p.add_event(ctrl_w), Command::CloseTab));

        // focus_next = ["Tab"] must route through the oneshot map as a bare
        // KeyCode::Tab (the named_key() path), since Tab emits no Char and so
        // never flows through the keystroke buffer.
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(p.add_event(tab), Command::FocusNext));

        // new_tab = ["gn"] still resolves as a two-key chord.
        assert!(matches!(p.add_event(key('g')), Command::None));
        assert!(matches!(p.add_event(key('n')), Command::NewTab));
    }

    #[test]
    fn key_config_parses_from_empty_and_partial_toml() {
        // empty: every field None
        let empty: KeyConfig = toml::from_str("").unwrap();
        assert!(empty.movement.up.is_none());
        assert!(empty.manipulation.undo.is_none());

        // partial: only what is written is Some; [] stays Some(empty)
        let partial: KeyConfig =
            toml::from_str("[manipulation]\nundo = []\n[movement]\nup = [\"k\"]").unwrap();
        assert_eq!(partial.movement.up, Some(vec!["k".into()]));
        assert_eq!(partial.manipulation.undo, Some(vec![]));
        assert!(partial.movement.down.is_none());
    }
}
