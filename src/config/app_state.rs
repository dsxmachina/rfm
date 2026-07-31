//! Persistent per-user application state (not configuration): small facts
//! rfm remembers across sessions, stored under `$XDG_STATE_HOME/rfm`.

use std::path::Path;

/// The name of the state file inside the state directory.
const STATE_FILE: &str = "state.toml";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AppState {
    /// The rfm version the one-time upgrade notice was last shown for.
    #[serde(default)]
    pub upgrade_notice_seen_for: Option<String>,
}

/// Read the state file from `state_dir`. A missing or broken file yields
/// the default state — state is best-effort, never an error.
pub fn read(state_dir: &Path) -> AppState {
    std::fs::read_to_string(state_dir.join(STATE_FILE))
        .ok()
        .and_then(|content| toml::from_str(&content).ok())
        .unwrap_or_default()
}

/// Write the state file into `state_dir`, creating the directory if needed.
///
/// Design rule: callers must degrade a write failure to a `warn!` ("may ask
/// again next start") — persisting state is best-effort and must never
/// surface as an error to the user.
pub fn write(state_dir: &Path, state: &AppState) -> anyhow::Result<()> {
    std::fs::create_dir_all(state_dir)?;
    let content = toml::to_string(state)?;
    std::fs::write(state_dir.join(STATE_FILE), content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).upgrade_notice_seen_for.is_none());
    }

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let s = AppState {
            upgrade_notice_seen_for: Some("0.5.0".into()),
        };
        write(dir.path(), &s).unwrap();
        assert_eq!(
            read(dir.path()).upgrade_notice_seen_for.as_deref(),
            Some("0.5.0")
        );
    }

    #[test]
    fn broken_file_reads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.toml"), "not = [valid").unwrap();
        assert!(read(dir.path()).upgrade_notice_seen_for.is_none());
    }
}
