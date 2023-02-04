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
        if string.starts_with("~") {
            // Replace with users home directory
            let home = std::env::var("HOME").unwrap_or_default();
            string = string.replace("~", &home);
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
}

#[derive(Debug, Clone)]
pub enum Command {
    Move(Movement),
    ToggleHidden,
    Quit,
    None,
}

/// Takes the incoming key-events, and returns the corresponding command.
pub struct CommandParser {
    key_commands: PatriciaMap<Command>,
    mod_commands: HashMap<KeyEvent, Command>,
    buffer: String,
}

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
        // TODO: We need a mechanism to automatically expand "~" to home directory of user at runtime.
        key_commands.insert("gh", Command::Move(Movement::JumpTo("~".into())));
        key_commands.insert("gr", Command::Move(Movement::JumpTo("/".into())));
        key_commands.insert("gc", Command::Move(Movement::JumpTo("~/.config".into())));

        // custom jumps
        key_commands.insert("gp", Command::Move(Movement::JumpTo("~/Projekte".into())));
        key_commands.insert("gs", Command::Move(Movement::JumpTo("~/.scripts".into())));

        // Toggle hidden files
        key_commands.insert("zh", Command::ToggleHidden);

        // Quit
        key_commands.insert("q", Command::Quit);

        // --- Commands for modifier + key:
        let mut mod_commands = HashMap::new();

        // Quit
        mod_commands.insert(CTRL_C, Command::Quit);

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

    pub fn add_event(&mut self, event: KeyEvent) -> Command {
        match event.modifiers {
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
            _ => {
                if let Some(command) = self.mod_commands.get(&event) {
                    self.buffer.clear();
                    return command.clone();
                }
            }
        }
        Command::None
    }
}