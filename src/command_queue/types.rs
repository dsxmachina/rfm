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

impl QueueStatus {
    /// Returns true if there's any activity (running or queued commands)
    pub fn is_active(&self) -> bool {
        self.active.is_some() || self.queued_count > 0
    }
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

/// Configuration for a user-defined command (parsed and ready to use)
#[derive(Debug, Clone)]
pub struct UserCommandConfig {
    /// Display name of the command
    pub name: String,
    /// Key sequences that trigger this command
    pub keys: Vec<String>,
    /// The shell command template (may contain $@ placeholder)
    pub cmd: String,
    /// Whether this command needs the terminal (runs in foreground)
    pub interactive: bool,
    /// Separator for paths when expanding $@ (default: space)
    pub separator: String,
}

impl UserCommandConfig {
    /// Create from a config entry with name
    pub fn from_entry(name: String, entry: CommandConfigEntry) -> Self {
        Self {
            name,
            keys: entry.keys,
            cmd: entry.cmd,
            interactive: entry.interactive,
            separator: entry.separator,
        }
    }
}

impl UserCommandConfig {
    /// Expand the command template by replacing $@ with the given paths
    pub fn expand(&self, paths: &[PathBuf]) -> String {
        let paths_str: String = paths
            .iter()
            .map(|p| shell_escape::escape(p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(&self.separator);

        self.cmd.replace("$@", &paths_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_single_path() {
        let config = UserCommandConfig {
            name: "test".to_string(),
            keys: vec!["zz".to_string()],
            cmd: "tar -czvf archive.tar.gz $@".to_string(),
            interactive: false,
            separator: " ".to_string(),
        };

        let paths = vec![PathBuf::from("/home/user/file.txt")];
        let expanded = config.expand(&paths);
        assert_eq!(expanded, "tar -czvf archive.tar.gz /home/user/file.txt");
    }

    #[test]
    fn test_expand_multiple_paths() {
        let config = UserCommandConfig {
            name: "test".to_string(),
            keys: vec!["zz".to_string()],
            cmd: "rm $@".to_string(),
            interactive: false,
            separator: " ".to_string(),
        };

        let paths = vec![
            PathBuf::from("/home/user/file1.txt"),
            PathBuf::from("/home/user/file2.txt"),
        ];
        let expanded = config.expand(&paths);
        assert_eq!(expanded, "rm /home/user/file1.txt /home/user/file2.txt");
    }

    #[test]
    fn test_expand_with_spaces_in_path() {
        let config = UserCommandConfig {
            name: "test".to_string(),
            keys: vec!["zz".to_string()],
            cmd: "cat $@".to_string(),
            interactive: false,
            separator: " ".to_string(),
        };

        let paths = vec![PathBuf::from("/home/user/my file.txt")];
        let expanded = config.expand(&paths);
        // shell_escape should quote paths with spaces
        assert!(expanded.contains("my file.txt") || expanded.contains("'my file.txt'"));
    }

    #[test]
    fn test_expand_with_custom_separator() {
        let config = UserCommandConfig {
            name: "test".to_string(),
            keys: vec!["zz".to_string()],
            cmd: "echo $@ | xargs -0 rm".to_string(),
            interactive: false,
            separator: "\0".to_string(),
        };

        let paths = vec![
            PathBuf::from("/home/user/file1.txt"),
            PathBuf::from("/home/user/file2.txt"),
        ];
        let expanded = config.expand(&paths);
        assert!(expanded.contains("\0"));
    }

    #[test]
    fn test_queue_status_is_active() {
        let status = QueueStatus::default();
        assert!(!status.is_active());

        let status = QueueStatus {
            active: Some("compress".to_string()),
            queued_count: 0,
        };
        assert!(status.is_active());

        let status = QueueStatus {
            active: None,
            queued_count: 1,
        };
        assert!(status.is_active());
    }
}
