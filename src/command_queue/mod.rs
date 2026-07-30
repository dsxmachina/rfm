mod executor;
mod types;

use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

pub use executor::CommandExecutor;
pub use types::{
    CommandConfigEntry, CommandsConfig, QueueStatus, QueuedCommand, UserCommandConfig,
};

/// True when an executable named `name` exists in one of the
/// directories of `path_var` (a PATH-style list).
fn find_in_path(name: &str, path_var: &OsStr) -> bool {
    std::env::split_paths(path_var).any(|dir| dir.join(name).is_file())
}

/// Whether `zoxide` is on PATH — checked once per run, so on systems
/// without zoxide the hook is skipped instead of queueing a command
/// that fails (visibly, in the log) on every directory change.
fn zoxide_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let available = std::env::var_os("PATH")
            .map(|p| find_in_path("zoxide", &p))
            .unwrap_or(false);
        if !available {
            log::debug!("zoxide not found in PATH - visited directories will not be recorded");
        }
        available
    })
}

pub fn zoxide_add_dir(path: &Path) -> Option<QueuedCommand> {
    if !zoxide_available() {
        return None;
    }
    build_zoxide_add(path)
}

fn build_zoxide_add(path: &Path) -> Option<QueuedCommand> {
    if !path.is_dir() {
        return None;
    }
    let canonicalized = path.canonicalize().ok()?;
    // The command is executed via `sh -c`, so the path must be escaped
    // to survive word splitting (spaces) and metacharacters (&, $, ...)
    let cmd = format!(
        "zoxide add {}",
        shell_escape::escape(canonicalized.to_string_lossy())
    );
    Some(QueuedCommand {
        name: "zoxide add current-dir".to_string(),
        cmd,
        working_dir: ".".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the argument part of the generated command through `sh -c`
    /// (like the CommandExecutor does) and returns the words sh produces.
    fn words_seen_by_sh(cmd: &str) -> Vec<String> {
        let args = cmd.strip_prefix("zoxide add ").expect("zoxide add prefix");
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s\n' {args}"))
            .output()
            .expect("failed to run sh");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn zoxide_add_survives_shell_word_splitting() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a directory with spaces", "Bilder & Videos"] {
            let dir = tmp.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            let canonical = dir.canonicalize().unwrap();

            let cmd = build_zoxide_add(&dir).expect("dir exists").cmd;

            assert_eq!(
                words_seen_by_sh(&cmd),
                vec![canonical.to_string_lossy().to_string()],
                "sh must see the path as exactly one argument: {cmd}"
            );
        }
    }

    #[test]
    fn zoxide_add_ignores_non_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(build_zoxide_add(&file).is_none());
        assert!(build_zoxide_add(&tmp.path().join("missing")).is_none());
    }

    #[test]
    fn find_in_path_only_matches_existing_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let with_bin = tmp.path().join("with-bin");
        let empty = tmp.path().join("empty");
        std::fs::create_dir(&with_bin).unwrap();
        std::fs::create_dir(&empty).unwrap();
        std::fs::write(with_bin.join("somebin"), "").unwrap();

        let path_var = std::env::join_paths([empty.clone(), with_bin]).unwrap();
        assert!(find_in_path("somebin", &path_var));
        assert!(!find_in_path("zoxide-definitely-missing", &path_var));
        // A directory named like the binary must not count.
        let path_var = std::env::join_paths([tmp.path().to_path_buf()]).unwrap();
        assert!(!find_in_path("empty", &path_var));
    }
}
