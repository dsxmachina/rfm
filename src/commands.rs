use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use patricia_tree::PatriciaMap;

use self::config::KeyConfig;

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
const CTRL_X: KeyEvent = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
const CTRL_P: KeyEvent = KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL);
const CTRL_F: KeyEvent = KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL);

#[derive(Debug, Clone)]
pub struct ExpandedPath(PathBuf);

impl From<&str> for ExpandedPath {
    fn from(path: &str) -> Self {
        let mut string = path.to_string();

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

mod config {
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct Manipulation {
        change_directory: Vec<String>,
        rename: Vec<String>,
        mkdir: Vec<String>,
        touch: Vec<String>,
        cut: Vec<String>,
        copy: Vec<String>,
        delete: Vec<String>,
        paste: Vec<String>,
        paste_overwrite: Vec<String>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Movement {
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
    pub struct General {
        search: Vec<String>,
        mark: Vec<String>,
        next: Vec<String>,
        previous: Vec<String>,
        view_trash: Vec<String>,
        toggle_hidden: Vec<String>,
        quit: Vec<String>,
    }

    #[derive(Deserialize, Debug)]
    pub struct KeyConfig {
        pub general: General,
        pub movement: Movement,
        pub manipulation: Manipulation,
    }

    #[test]
    fn test_config() {
        let content = std::fs::read_to_string("/home/someone/.config/rfm/keys.toml").unwrap();
        let result = toml::from_str(&content);
        if let Err(e) = &result {
            println!("{e}");
        }
        assert!(result.is_ok());
        let _config: KeyConfig = result.unwrap();
    }
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

#[derive(Debug, Clone)]
pub enum Command {
    Move(Move),
    Next,
    Previous,
    ToggleHidden,
    ViewTrash,
    Cd,
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
    None,
}

/// Takes the incoming key-events, and returns the corresponding command.
///
/// Uses a `PatriciaMap` to match patterns of keystrokes,
/// and a normal `HashMap` to match "oneshot"-commands,
/// that don't require any key combinations but may require a modifier.
pub struct CommandParser {
    key_commands: PatriciaMap<Command>,
    mod_commands: HashMap<KeyEvent, Command>,
    buffer: String,
}

// TODO: Make this configurable from a config-file
impl CommandParser {
    pub fn from_config(config: KeyConfig) -> Self {
        todo!()
    }
    pub fn new() -> Self {
        // --- Commands for "normal" keys:
        let mut key_commands = PatriciaMap::new();
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
        key_commands.insert("cd", Command::Cd);
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
        mod_commands.insert(CTRL_P, Command::Paste { overwrite: false });

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
                if self
                    .key_commands
                    .iter_prefix(self.buffer.as_bytes())
                    .count()
                    == 0
                {
                    self.buffer.clear();
                    return Command::None;
                }

                // Check if we have a valid command
                if let Some(command) = self.key_commands.get(self.buffer.as_bytes()) {
                    self.buffer.clear();
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
            return command.clone();
        }
        Command::None
    }
}
