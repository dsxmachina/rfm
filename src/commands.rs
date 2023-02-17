use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use patricia_tree::PatriciaMap;

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

#[derive(Debug, Clone)]
pub struct ExpandedPath(PathBuf);

impl From<&str> for ExpandedPath {
    fn from(path: &str) -> Self {
        let mut string = path.to_string();

        // Expand "~"
        if string.starts_with('~') {
            // Replace with users home directory
            let home = std::env::var("HOME").unwrap_or_default();
            string = string.replace('~', &home);
        }
        // TODO: Extract environment variables

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

#[derive(Debug, Clone)]
pub enum Movement {
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
    Move(Movement),
    ToggleHidden,
    ShowConsole,
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
    pub fn new() -> Self {
        // --- Commands for "normal" keys:
        let mut key_commands = PatriciaMap::new();
        // Basic movement commands
        key_commands.insert("h", Command::Move(Movement::Left));
        key_commands.insert("j", Command::Move(Movement::Down));
        key_commands.insert("k", Command::Move(Movement::Up));
        key_commands.insert("l", Command::Move(Movement::Right));

        key_commands.insert("gg", Command::Move(Movement::Top));
        key_commands.insert("G", Command::Move(Movement::Bottom));

        // Jump to something
        key_commands.insert("gh", Command::Move(Movement::JumpTo("~".into())));
        key_commands.insert("gr", Command::Move(Movement::JumpTo("/".into())));
        key_commands.insert("gc", Command::Move(Movement::JumpTo("~/.config".into())));

        key_commands.insert("ge", Command::Move(Movement::JumpTo("/etc".into())));
        key_commands.insert("gu", Command::Move(Movement::JumpTo("/usr".into())));
        key_commands.insert("gN", Command::Move(Movement::JumpTo("/nix/store".into())));

        // custom jumps
        key_commands.insert("gp", Command::Move(Movement::JumpTo("~/Projekte".into())));
        key_commands.insert("gs", Command::Move(Movement::JumpTo("~/.scripts".into())));
        key_commands.insert("gb", Command::Move(Movement::JumpTo("~/Bilder".into())));
        key_commands.insert(
            "gw",
            Command::Move(Movement::JumpTo("~/Bilder/wallpapers".into())),
        );
        key_commands.insert("gd", Command::Move(Movement::JumpTo("~/Dokumente".into())));
        key_commands.insert("gD", Command::Move(Movement::JumpTo("~/Downloads".into())));
        key_commands.insert(
            "gl",
            Command::Move(Movement::JumpTo("~/Projekte/loadrunner-2021".into())),
        );
        key_commands.insert(
            "gL",
            Command::Move(Movement::JumpTo(
                "~/Projekte/loadrunner-2021/lr-localization".into(),
            )),
        );
        key_commands.insert("gm", Command::Move(Movement::JumpTo("~/Musik".into())));
        key_commands.insert("gN", Command::Move(Movement::JumpTo("/nix/store".into())));

        // Toggle hidden files
        key_commands.insert("zh", Command::ToggleHidden);

        // Jump to previous location
        key_commands.insert("\'\'", Command::Move(Movement::JumpPrevious));

        // Show console
        key_commands.insert(":", Command::ShowConsole);
        key_commands.insert("cd", Command::ShowConsole);

        // Mark current file
        key_commands.insert(" ", Command::Mark);

        // Quit
        key_commands.insert("q", Command::Quit);

        // --- Commands for modifier + key:
        let mut mod_commands = HashMap::new();

        // Escape from what you are doing
        // mod_commands.insert(CTRL_C, Command::Esc);

        // Advanced movement
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            Command::Move(Movement::PageForward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Command::Move(Movement::PageBackward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
            Command::Move(Movement::HalfPageForward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
            Command::Move(Movement::HalfPageBackward),
        );

        // Toggle hidden (backspace)
        mod_commands.insert(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            Command::ToggleHidden,
        );

        CommandParser {
            key_commands,
            mod_commands,
            buffer: "".to_string(),
        }
    }

    pub fn clear_buffer(&mut self) {
        self.buffer.clear();
    }

    pub fn buffer(&self) -> String {
        self.buffer.clone()
    }

    /// Parse an event and return the command that is assigned to it
    pub fn add_event(&mut self, event: KeyEvent) -> Command {
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
