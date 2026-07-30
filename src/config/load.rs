//! The single-file config load pipeline: sparse `config.toml` over the
//! embedded defaults, with legacy `keys.toml`/`open.toml` fold-in.
//!
//! `load()` never logs itself — every diagnostic is appended to
//! [`LoadedConfig::warnings`] and logged by the caller, keeping the
//! pipeline pure enough to unit-test against a temp directory.

use std::path::Path;

use anyhow::Context;
use toml::Value;

use super::merge::{deep_merge, diff_from_defaults, unknown_keys};
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
    pub default_keys: KeyConfig, // the pure defaults' keys (for build())
    pub warnings: Vec<String>,   // unknown keys, dropped sections, legacy hints
    pub legacy_folded: bool,     // keys.toml/open.toml were folded in
}

/// Steps 1–2 of the load pipeline, shared by `load` and `migrate`: read
/// config.toml and fold legacy keys.toml/open.toml in. Only `load` passes
/// `write_stub` (first-run stub when config.toml is absent) — `migrate`
/// must see the directory untouched.
fn effective_user_tree(config_dir: &Path, write_stub: bool) -> (Value, Vec<String>, bool) {
    let mut warnings = Vec::new();
    let mut legacy_folded = false;

    // --- 1. Read config.toml (absent → optionally write the stub, start
    // empty; a present-but-unreadable/unparseable file is left alone and
    // defaults run).
    let config_file = config_dir.join("config.toml");
    let mut user_tree = if config_file.exists() {
        match std::fs::read_to_string(&config_file) {
            Ok(content) => match content.parse::<Value>() {
                Ok(tree) => tree,
                Err(e) => {
                    warnings.push(format!(
                        "cannot parse {}: {e} — ignoring it",
                        config_file.display()
                    ));
                    Value::Table(toml::map::Map::new())
                }
            },
            Err(e) => {
                warnings.push(format!(
                    "cannot read {}: {e} — ignoring it",
                    config_file.display()
                ));
                Value::Table(toml::map::Map::new())
            }
        }
    } else {
        if write_stub {
            if let Err(e) = std::fs::create_dir_all(config_dir) {
                warnings.push(format!(
                    "cannot create config dir {}: {e}",
                    config_dir.display()
                ));
            }
            if let Err(e) = std::fs::write(&config_file, CONFIG_STUB) {
                warnings.push(format!(
                    "failed to write the config stub {}: {e}",
                    config_file.display()
                ));
            }
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

    (user_tree, warnings, legacy_folded)
}

/// Two-line header written atop the migrated config.toml.
const MIGRATE_HEADER: &str = "\
# migrated by rfm --migrate-config — sparse overrides over the built-in defaults.
# See every available option together with its default value: rfm --dump-config
";

/// Returns human-readable summary lines. Writes config.toml, renames legacy
/// files to *.bak. Refuses (before any FS mutation) if a backup this run
/// would create already exists — so an accidental re-run cannot destroy the
/// previous run's backups. Pure-ish: all paths under `config_dir`.
pub fn migrate(config_dir: &Path) -> anyhow::Result<Vec<String>> {
    let (user_tree, warnings, _) = effective_user_tree(config_dir, false);
    // parse problems etc. are part of the story
    let mut summary: Vec<String> = warnings
        .into_iter()
        .map(|w| format!("warning: {w}"))
        .collect();

    // Refuse before touching ANYTHING if a backup this run would create
    // already exists — re-running migrate must never destroy the previous
    // run's backups (the user's original files).
    let clobbered: Vec<String> = ["config.toml", "keys.toml", "open.toml"]
        .iter()
        .filter(|file| config_dir.join(file).exists())
        .map(|file| config_dir.join(format!("{file}.bak")))
        .filter(|bak| bak.exists())
        .map(|bak| bak.display().to_string())
        .collect();
    if !clobbered.is_empty() {
        anyhow::bail!(
            "refusing to migrate: {} already exists (a previous migration's backup) — \
             remove or rename it first",
            clobbered.join(", ")
        );
    }

    // The minimal overrides: everything in the effective user config that
    // differs from the embedded defaults (None → empty file, just the header).
    let diff = diff_from_defaults(&default_tree(), &user_tree);
    let body = match &diff {
        Some(tree) => toml::to_string_pretty(tree).context("cannot serialize the migrated config")?,
        None => String::new(),
    };

    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("cannot create config dir {}", config_dir.display()))?;

    let config_file = config_dir.join("config.toml");
    let mut backed_up = None;
    if config_file.exists() {
        let bak = config_dir.join("config.toml.bak");
        std::fs::rename(&config_file, &bak)
            .with_context(|| format!("cannot back up {}", config_file.display()))?;
        summary.push(format!(
            "backed up the previous config.toml to {}",
            bak.display()
        ));
        backed_up = Some(bak);
    }
    let content = if body.is_empty() {
        MIGRATE_HEADER.to_string()
    } else {
        format!("{MIGRATE_HEADER}\n{body}")
    };
    std::fs::write(&config_file, content).with_context(|| match &backed_up {
        Some(bak) => format!(
            "cannot write {}; your previous config.toml is preserved at {}",
            config_file.display(),
            bak.display()
        ),
        None => format!("cannot write {}", config_file.display()),
    })?;
    summary.push(if body.is_empty() {
        format!(
            "wrote {} — your setup matches the built-in defaults, so it is empty",
            config_file.display()
        )
    } else {
        format!("wrote {} with your overrides", config_file.display())
    });

    for file in ["keys.toml", "open.toml"] {
        let path = config_dir.join(file);
        if !path.exists() {
            continue;
        }
        let bak = config_dir.join(format!("{file}.bak"));
        std::fs::rename(&path, &bak)
            .with_context(|| format!("cannot rename legacy {}", path.display()))?;
        summary.push(format!("renamed legacy {file} to {file}.bak"));
    }

    Ok(summary)
}

pub fn load(config_dir: &Path) -> LoadedConfig {
    // --- 1.+2. config.toml (stub on first run) + legacy fold-in.
    let (mut user_tree, mut warnings, legacy_folded) = effective_user_tree(config_dir, true);
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
    // a fresh defaults tree. Every retry strictly shrinks the user tree (a
    // drop removes a node; an unattributable error breaks out), so the loop
    // terminates on its own — the node-count cap is purely defensive.
    let max_attempts = count_nodes(&user_tree) + 1;
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

    // The pure defaults' keybindings — the first pass of CommandParser::build.
    let default_keys: KeyConfig = defaults
        .get("keys")
        .cloned()
        .expect("embedded default-config.toml has a [keys] section")
        .try_into()
        .expect("embedded default [keys] must deserialize");

    LoadedConfig {
        config,
        parser_input,
        default_keys,
        warnings,
        legacy_folded,
    }
}

/// Total number of table keys in the tree, at every nesting level (the
/// defensive retry cap in step 5: one retry drops at most one node).
fn count_nodes(tree: &Value) -> usize {
    match tree.as_table() {
        Some(t) => t.len() + t.values().map(count_nodes).sum::<usize>(),
        None => 0,
    }
}

/// Nearest sibling of `dotted`'s last segment in the SAME defaults section,
/// within edit distance ≤ 2 (typo suggestions for unknown-key warnings).
/// Callers pass paths from `unknown_keys`, whose parent chain exists in the
/// defaults by construction; a missing parent just yields `None`.
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
        assert!(loaded
            .warnings
            .iter()
            .any(|w| w.contains("ignoring your [colors]"))); // drop, not typo, wording
    }

    // More broken [keys.*] subsections than top-level tables: each drop
    // consumes a retry while shrinking only a sub-key, so a counted bound
    // of "top-level keys + 1" exhausts before the salvage completes.
    #[test]
    fn multiple_broken_keys_subsections_salvage_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[general]\nfancy_icons = true\n\
             [keys.movement]\nup = 5\n\
             [keys.manipulation]\nrename = 5\n\
             [keys.jump_marks]\nset = 5\n\
             [keys.general]\nquit = [\"q\"]",
        )
        .unwrap();
        let loaded = load(dir.path());
        assert!(loaded.config.general.fancy_icons); // salvaged
        for section in ["keys.movement", "keys.manipulation", "keys.jump_marks"] {
            assert!(
                loaded
                    .warnings
                    .iter()
                    .any(|w| w.contains(&format!("ignoring your [{section}]"))),
                "missing drop warning for [{section}]"
            );
        }
        assert!(!loaded
            .warnings
            .iter()
            .any(|w| w.contains("built-in defaults"))); // no full fallback
    }

    #[test]
    fn unknown_key_warns_but_loads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[general]\nuse_trsah = false").unwrap();
        let loaded = load(dir.path());
        assert!(loaded.config.general.use_trash); // typo ≠ applied
        assert!(loaded
            .warnings
            .iter()
            .any(|w| w.contains("use_trsah")
                && w.contains("did you mean")
                && w.contains("`use_trash`")));
    }

    #[test]
    fn migrate_writes_minimal_diff_and_baks_legacy() {
        let dir = tempfile::tempdir().unwrap();
        // stale full copy pinning an old default + one real customization
        std::fs::write(
            dir.path().join("keys.toml"),
            "[movement]\nup = [\"k\"]\ndown = [\"j\"]\n[general]\nquit = [\"q\", \"exit\"]",
        )
        .unwrap();
        migrate(dir.path()).unwrap();

        let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
        let tree: toml::Value = written.parse().unwrap();
        // values equal to defaults were dropped:
        assert!(tree.get("keys").and_then(|k| k.get("movement")).is_none());
        // the real customization survived:
        assert_eq!(
            tree["keys"]["general"]["quit"],
            toml::Value::try_from(vec!["q", "exit"]).unwrap()
        );
        assert!(dir.path().join("keys.toml.bak").exists());
        assert!(!dir.path().join("keys.toml").exists());
    }

    /// Every regular file in `dir` with its content — for byte-identical
    /// "nothing changed" assertions.
    fn dir_snapshot(dir: &Path) -> Vec<(String, String)> {
        let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(e.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn migrate_refuses_to_clobber_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[general]\nfancy_icons = true").unwrap();
        std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"x\"]").unwrap();
        migrate(dir.path()).unwrap();
        let after_first = dir_snapshot(dir.path());

        // Run 2: config.toml exists AND config.toml.bak exists — must refuse
        // instead of clobbering the user's original backup.
        let err = migrate(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("config.toml.bak"),
            "error must name the offending backup, got: {err}"
        );
        assert_eq!(
            dir_snapshot(dir.path()),
            after_first,
            "a refused migration must leave every file untouched"
        );
    }

    #[test]
    fn migrate_load_equivalence() {
        // load(migrate(dir)) ≡ load(dir) — the invariant from the design doc
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[general]\nfancy_icons = true").unwrap();
        std::fs::write(dir.path().join("keys.toml"), "[movement]\nup = [\"x\"]").unwrap();
        let before = load(dir.path());
        migrate(dir.path()).unwrap();
        let after = load(dir.path());
        assert_eq!(
            after.config.general.fancy_icons,
            before.config.general.fancy_icons
        );
        assert_eq!(
            after.parser_input.movement.up,
            before.parser_input.movement.up
        );
        assert!(!after.legacy_folded);
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("use_trash", "use_trash"), 0); // identity
        assert_eq!(edit_distance("use_trsah", "use_trash"), 2); // transposition
        assert!(edit_distance("xyz", "use_trash") > 2); // over threshold
    }
}
