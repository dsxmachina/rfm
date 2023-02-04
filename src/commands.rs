use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use patricia_tree::PatriciaMap;

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

#[derive(Debug, Clone, Copy)]
pub enum Movement {
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy)]
pub enum Command {
    Move(Movement),
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

        // Quit
        key_commands.insert("q", Command::Quit);

        // Add something complex just to test
        key_commands.insert("asdf", Command::Quit);

        // --- Commands for modifier + key:
        let mut mod_commands = HashMap::new();

        // Quit
        mod_commands.insert(CTRL_C, Command::Quit);

        // Advanced movement
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
            Command::Move(Movement::Forward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
            Command::Move(Movement::Backward),
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
