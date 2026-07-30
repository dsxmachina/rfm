//! The single-file config load pipeline: sparse `config.toml` over the
//! embedded defaults, with legacy `keys.toml`/`open.toml` fold-in.
//!
//! `load()` never logs itself — every diagnostic is appended to
//! [`LoadedConfig::warnings`] and logged by the caller, keeping the
//! pipeline pure enough to unit-test against a temp directory.

use std::path::Path;

use toml::Value;

use super::merge::{deep_merge, unknown_keys};
use super::{default_tree, Config};
use crate::engine::commands::KeyConfig;

/// Written to `config.toml` on first run (only when the file is absent —
/// a present-but-broken file is never overwritten).
const CONFIG_STUB: &str = "\
# rfm configuration — sparse overrides over the built-in defaults.
#
# An empty file is perfectly valid: you get the full default setup.
# See every available option together with its default value:
#
#     rfm --dump-config
#
# Copy any line or section from there into this file and change it.
";

/// Everything main() needs, plus warnings to log and upgrade-notice fodder.
pub struct LoadedConfig {
    pub config: Config,
    pub parser_input: KeyConfig, // user overlay (for build())
    pub warnings: Vec<String>,   // unknown keys, dropped sections, legacy hints
    pub legacy_folded: bool,     // keys.toml/open.toml were folded in
}

pub fn load(config_dir: &Path) -> LoadedConfig {
    let mut warnings = Vec::new();
    let mut legacy_folded = false;

    // --- 1. Read config.toml (absent → write the stub, start empty; a
    // present-but-unreadable/unparseable file is left alone and defaults run).
    let config_file = config_dir.join("config.toml");
    let mut user_tree = if config_file.exists() {
        match std::fs::read_to_string(&config_file) {
            Ok(content) => match content.parse::<Value>() {
                Ok(tree) => tree,
                Err(e) => {
                    warnings.push(format!(
                        "cannot parse {}: {e} — using the built-in defaults",
                        config_file.display()
                    ));
                    Value::Table(toml::map::Map::new())
                }
            },
            Err(e) => {
                warnings.push(format!(
                    "cannot read {}: {e} — using the built-in defaults",
                    config_file.display()
                ));
                Value::Table(toml::map::Map::new())
            }
        }
    } else {
        let _ = std::fs::create_dir_all(config_dir);
        if let Err(e) = std::fs::write(&config_file, CONFIG_STUB) {
            warnings.push(format!(
                "failed to write the config stub {}: {e}",
                config_file.display()
            ));
        }
        Value::Table(toml::map::Map::new())
    };

    // --- 2. Legacy fold-in: keys.toml → [keys], open.toml → [open]. A table
    // the user already wrote in config.toml wins over the legacy file.
    for (file, key) in [("keys.toml", "keys"), ("open.toml", "open")] {
        let path = config_dir.join(file);
        if !path.exists() {
            continue;
        }
        let table = user_tree
            .as_table_mut()
            .expect("a parsed TOML document is a table");
        if table.contains_key(key) {
            warnings.push(format!(
                "legacy {file} ignored: your config.toml already has a [{key}] section"
            ));
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|content| content.parse::<Value>().map_err(|e| e.to_string()));
        match parsed {
            Ok(tree) => {
                table.insert(key.to_string(), tree);
                legacy_folded = true;
            }
            Err(e) => warnings.push(format!("cannot parse legacy {file}: {e} — not folded in")),
        }
    }
    if legacy_folded {
        warnings.push(
            "legacy keys.toml/open.toml were folded into your configuration — run \
             `rfm --migrate-config` to unify everything into config.toml"
                .to_string(),
        );
    }

    // --- 3. Unknown user keys (typos), with a nearest-sibling suggestion.
    let defaults = default_tree();
    for path in unknown_keys(&defaults, &user_tree) {
        match suggest(&defaults, &path) {
            Some(s) => warnings.push(format!("unknown config key `{path}` — did you mean `{s}`?")),
            None => warnings.push(format!("unknown config key `{path}`")),
        }
    }

    // --- 4. The user overlay for the parser: only what the user wrote under
    // [keys] (extracted BEFORE merging, so defaults don't bleed into it).
    let parser_input = match user_tree.get("keys") {
        Some(keys) => match keys.clone().try_into::<KeyConfig>() {
            Ok(k) => k,
            Err(e) => {
                warnings.push(format!(
                    "cannot parse the [keys] section: {e} — using the default keybindings"
                ));
                KeyConfig::default()
            }
        },
        None => KeyConfig::default(),
    };

    // --- 5. Merge over the defaults and deserialize. On a typed error, drop
    // the offending user section (precise path in the warning) and retry from
    // a fresh defaults tree; bounded by the number of top-level user keys.
    let max_attempts = user_tree.as_table().map_or(0, |t| t.len()) + 1;
    let mut typed = None;
    for _ in 0..max_attempts {
        let mut merged = defaults.clone();
        deep_merge(&mut merged, user_tree.clone());
        let err = match serde_path_to_error::deserialize::<_, Config>(merged) {
            Ok(config) => {
                typed = Some(config);
                break;
            }
            Err(err) => err,
        };
        let full_path = err.path().to_string();
        let mut segments = err.path().iter().filter_map(|s| match s {
            serde_path_to_error::Segment::Map { key } => Some(key.clone()),
            _ => None,
        });
        let Some(first) = segments.next() else {
            warnings.push(format!("configuration error: {}", err.inner()));
            break;
        };
        // Drop the whole top-level user section — except under [keys], where
        // dropping only the sub-table keeps the other key groups alive.
        let (removed, dropped_key) = if first == "keys" {
            match segments.next() {
                Some(second) => (
                    user_tree
                        .get_mut("keys")
                        .and_then(Value::as_table_mut)
                        .and_then(|t| t.remove(&second)),
                    format!("keys.{second}"),
                ),
                None => (
                    user_tree.as_table_mut().and_then(|t| t.remove("keys")),
                    first,
                ),
            }
        } else {
            (
                user_tree.as_table_mut().and_then(|t| t.remove(&first)),
                first,
            )
        };
        warnings.push(format!(
            "config error at `{full_path}`: {} — ignoring your [{dropped_key}] section",
            err.inner()
        ));
        if removed.is_none() {
            // The error is not attributable to a user subtree — nothing left
            // to drop, fall back to pure defaults below.
            break;
        }
    }
    let config = typed.unwrap_or_else(|| {
        warnings.push("could not apply your configuration — using the built-in defaults".into());
        default_tree()
            .try_into()
            .expect("embedded default-config.toml must deserialize")
    });

    LoadedConfig {
        config,
        parser_input,
        warnings,
        legacy_folded,
    }
}

/// Nearest sibling of `dotted`'s last segment in the SAME defaults section,
/// within edit distance ≤ 2 (typo suggestions for unknown-key warnings).
fn suggest(defaults: &Value, dotted: &str) -> Option<String> {
    let mut segments: Vec<&str> = dotted.split('.').collect();
    let last = segments.pop()?;
    let mut node = defaults;
    for s in segments {
        node = node.get(s)?;
    }
    node.as_table()?
        .keys()
        .map(|k| (edit_distance(k, last), k))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, k)| k.clone())
}

/// Plain Levenshtein distance (the compared strings are short config keys).
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dir_creates_stub_and_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(dir.path());
        assert!(dir.path().join("config.toml").exists()); // stub written
        assert!(!dir.path().join("keys.toml").exists()); // legacy NOT created
        assert!(loaded.config.general.use_trash);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn sparse_override_applies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[general]\nfancy_icons = true\n[keys.manipulation]\nundo = []",
        )
        .unwrap();
        let loaded = load(dir.path());
        assert!(loaded.config.general.fancy_icons);
        assert_eq!(loaded.parser_input.manipulation.undo, Some(vec![]));
    }

    #[test]
    fn legacy_keys_and_open_fold_in() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "").unwrap();
        std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"x\"]").unwrap();
        std::fs::write(
            dir.path().join("open.toml"),
            "[text]\ndefault = { name = \"nano\", args = [], terminal = true }",
        )
        .unwrap();
        let loaded = load(dir.path());
        assert!(loaded.legacy_folded);
        assert_eq!(loaded.parser_input.movement.up, Some(vec!["x".into()]));
        assert!(loaded.warnings.iter().any(|w| w.contains("migrate-config")));
    }

    #[test]
    fn new_format_keys_table_beats_legacy_keys_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[keys.movement]\nup = [\"a\"]").unwrap();
        std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"b\"]").unwrap();
        let loaded = load(dir.path());
        assert_eq!(loaded.parser_input.movement.up, Some(vec!["a".into()]));
        assert!(loaded
            .warnings
            .iter()
            .any(|w| w.contains("keys.toml") && w.contains("ignored")));
    }

    #[test]
    fn broken_section_drops_only_that_section() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[general]\nfancy_icons = true\n[colors]\nmain = 42",
        )
        .unwrap(); // colors broken
        let loaded = load(dir.path());
        assert!(loaded.config.general.fancy_icons); // survived
        assert!(loaded.warnings.iter().any(|w| w.contains("colors"))); // reported
    }

    #[test]
    fn unknown_key_warns_but_loads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[general]\nuse_trsah = false").unwrap();
        let loaded = load(dir.path());
        assert!(loaded.config.general.use_trash); // typo ≠ applied
        assert!(loaded.warnings.iter().any(|w| w.contains("use_trsah")));
    }
}
