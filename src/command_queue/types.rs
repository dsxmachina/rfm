use std::collections::HashMap;
use std::path::PathBuf;

use serde::Deserialize;

/// A command that has been queued for execution
#[derive(Debug, Clone)]
pub struct QueuedCommand {
    /// Display name of the command
    pub name: String,
    /// The shell command to execute (with placeholders already expanded)
    pub cmd: String,
    /// Working directory for the command
    pub working_dir: PathBuf,
}

/// Status of the command queue, for UI display
#[derive(Debug, Clone, Default)]
pub struct QueueStatus {
    /// Name of the currently running command, if any
    pub active: Option<String>,
    /// Number of commands waiting in the queue
    pub queued_count: usize,
}

/// TOML config entry for a user-defined command
#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfigEntry {
    /// Key sequences that trigger this command
    pub keys: Vec<String>,
    /// The shell command template (may contain $@ placeholder)
    pub cmd: String,
    /// Whether this command needs the terminal (runs in foreground)
    #[serde(default)]
    pub interactive: bool,
    /// Separator for paths when expanding $@ (default: space)
    #[serde(default = "default_separator")]
    pub separator: String,
}

fn default_separator() -> String {
    " ".to_string()
}

/// The full commands config section: HashMap<name, entry>
pub type CommandsConfig = HashMap<String, CommandConfigEntry>;
