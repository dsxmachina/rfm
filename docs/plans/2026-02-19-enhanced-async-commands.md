# Enhanced Async Commands Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend async-commands with selection mode constraints, mime-type routing, and output filename generation to replace hardcoded archive commands.

**Architecture:** Add three new config fields (`selection`, `output`, `cmd` as array) to `CommandConfigEntry`. The executor validates selection count, resolves output filename with collision avoidance, and routes to mime-specific commands when configured.

**Tech Stack:** Rust, serde (TOML deserialization), mime_guess crate (already in use)

---

## Overview

This plan extends the existing async-command infrastructure with:
1. **Selection mode** - `single`, `multi`, or `any` (default)
2. **Mime-type routing** - Array of `{ mime = "...", cmd = "..." }` for format-specific commands
3. **Output filename** - `$OUTPUT` placeholder with collision-safe generation

### Target Config Syntax

```toml
[commands.extract]
keys = ["ex"]
selection = "single"
cmd = [
  { mime = "application/gzip", cmd = "tar -xzf $@" },
  { mime = "application/zip", cmd = "unzip $@" },
]

[commands.zip]
keys = ["z"]
output = "archive.zip"
cmd = "zip -r $OUTPUT $@"

[commands.tar]
keys = ["t"]
output = "archive.tar.gz"
cmd = "tar -czf $OUTPUT $@"
```

---

## Task 1: Add SelectionMode enum

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

Add to the `#[cfg(test)]` module in `types.rs`:

```rust
#[test]
fn test_selection_mode_default() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["z"]
        cmd = "zip $@"
    "#).unwrap();
    assert_eq!(config.selection, SelectionMode::Any);
}

#[test]
fn test_selection_mode_single() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["ex"]
        cmd = "unzip $@"
        selection = "single"
    "#).unwrap();
    assert_eq!(config.selection, SelectionMode::Single);
}

#[test]
fn test_selection_mode_multi() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["rm"]
        cmd = "rm $@"
        selection = "multi"
    "#).unwrap();
    assert_eq!(config.selection, SelectionMode::Multi);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_selection_mode`
Expected: FAIL with "cannot find type `SelectionMode`"

**Step 3: Write minimal implementation**

Add before `CommandConfigEntry`:

```rust
/// Specifies how many items must be selected for a command to execute
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SelectionMode {
    /// Command works with exactly one selected item
    Single,
    /// Command requires multiple selected items
    Multi,
    /// Command works with any number of selected items (default)
    #[default]
    Any,
}
```

Update `CommandConfigEntry`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfigEntry {
    pub keys: Vec<String>,
    pub cmd: String,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default)]
    pub selection: SelectionMode,
}
```

Update `UserCommandConfig`:

```rust
#[derive(Debug, Clone)]
pub struct UserCommandConfig {
    pub name: String,
    pub keys: Vec<String>,
    pub cmd: String,
    pub interactive: bool,
    pub separator: String,
    pub selection: SelectionMode,
}

impl UserCommandConfig {
    pub fn from_entry(name: String, entry: CommandConfigEntry) -> Self {
        Self {
            name,
            keys: entry.keys,
            cmd: entry.cmd,
            interactive: entry.interactive,
            separator: entry.separator,
            selection: entry.selection,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_selection_mode`
Expected: PASS (all 3 tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add SelectionMode enum for selection constraints

Adds single/multi/any selection modes to user-defined commands.
Commands can now specify whether they work with one item, multiple
items, or any number of items.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add selection validation helper

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_selection_mode_validates_single() {
    assert!(SelectionMode::Single.validate(1).is_ok());
    assert!(SelectionMode::Single.validate(0).is_err());
    assert!(SelectionMode::Single.validate(2).is_err());
}

#[test]
fn test_selection_mode_validates_multi() {
    assert!(SelectionMode::Multi.validate(2).is_ok());
    assert!(SelectionMode::Multi.validate(5).is_ok());
    assert!(SelectionMode::Multi.validate(0).is_err());
    assert!(SelectionMode::Multi.validate(1).is_err());
}

#[test]
fn test_selection_mode_validates_any() {
    assert!(SelectionMode::Any.validate(0).is_ok());
    assert!(SelectionMode::Any.validate(1).is_ok());
    assert!(SelectionMode::Any.validate(100).is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_selection_mode_validates`
Expected: FAIL with "no method named `validate`"

**Step 3: Write minimal implementation**

Add impl block for `SelectionMode`:

```rust
impl SelectionMode {
    /// Validates whether the given count satisfies this selection mode.
    /// Returns Ok(()) if valid, or Err with a descriptive message.
    pub fn validate(&self, count: usize) -> Result<(), &'static str> {
        match self {
            SelectionMode::Single => {
                if count == 1 {
                    Ok(())
                } else {
                    Err("command requires exactly one selected item")
                }
            }
            SelectionMode::Multi => {
                if count >= 2 {
                    Ok(())
                } else {
                    Err("command requires multiple selected items")
                }
            }
            SelectionMode::Any => Ok(()),
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_selection_mode_validates`
Expected: PASS (all 3 tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add selection mode validation

Adds validate() method to SelectionMode that checks whether
a given item count satisfies the selection constraint.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add output filename field

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_output_field_parsing() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["z"]
        cmd = "zip -r $OUTPUT $@"
        output = "archive.zip"
    "#).unwrap();
    assert_eq!(config.output, Some("archive.zip".to_string()));
}

#[test]
fn test_output_field_default_none() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["ls"]
        cmd = "ls $@"
    "#).unwrap();
    assert_eq!(config.output, None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_output_field`
Expected: FAIL with "unknown field `output`"

**Step 3: Write minimal implementation**

Update `CommandConfigEntry`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfigEntry {
    pub keys: Vec<String>,
    pub cmd: String,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default)]
    pub selection: SelectionMode,
    /// Output filename template (used with $OUTPUT placeholder)
    #[serde(default)]
    pub output: Option<String>,
}
```

Update `UserCommandConfig`:

```rust
#[derive(Debug, Clone)]
pub struct UserCommandConfig {
    pub name: String,
    pub keys: Vec<String>,
    pub cmd: String,
    pub interactive: bool,
    pub separator: String,
    pub selection: SelectionMode,
    pub output: Option<String>,
}

impl UserCommandConfig {
    pub fn from_entry(name: String, entry: CommandConfigEntry) -> Self {
        Self {
            name,
            keys: entry.keys,
            cmd: entry.cmd,
            interactive: entry.interactive,
            separator: entry.separator,
            selection: entry.selection,
            output: entry.output,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_output_field`
Expected: PASS (both tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add output filename field

Commands can now specify an output filename template that will
be used with the $OUTPUT placeholder for generated filenames.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Add output filename generation with collision avoidance

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_generate_output_filename_simple() {
    let dir = std::env::temp_dir();
    let result = generate_output_filename("archive.zip", &dir);
    assert!(result.ends_with("archive.zip"));
}

#[test]
fn test_generate_output_filename_collision() {
    use std::fs::File;
    let dir = std::env::temp_dir().join("rfm_test_output");
    std::fs::create_dir_all(&dir).unwrap();

    // Create existing file
    let existing = dir.join("archive.zip");
    File::create(&existing).unwrap();

    let result = generate_output_filename("archive.zip", &dir);
    // Should generate archive_.zip
    assert!(result.ends_with("archive_.zip"), "got: {}", result.display());

    // Cleanup
    std::fs::remove_file(&existing).unwrap();
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn test_generate_output_filename_multiple_collisions() {
    use std::fs::File;
    let dir = std::env::temp_dir().join("rfm_test_output2");
    std::fs::create_dir_all(&dir).unwrap();

    // Create existing files
    File::create(dir.join("archive.tar.gz")).unwrap();
    File::create(dir.join("archive_.tar.gz")).unwrap();

    let result = generate_output_filename("archive.tar.gz", &dir);
    assert!(result.ends_with("archive__.tar.gz"), "got: {}", result.display());

    // Cleanup
    std::fs::remove_file(dir.join("archive.tar.gz")).unwrap();
    std::fs::remove_file(dir.join("archive_.tar.gz")).unwrap();
    let _ = std::fs::remove_dir(&dir);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_generate_output_filename`
Expected: FAIL with "cannot find function `generate_output_filename`"

**Step 3: Write minimal implementation**

Add function after `SelectionMode` impl:

```rust
use std::path::{Path, PathBuf};

/// Generates a collision-free output filename.
/// If the file exists, appends underscores before the extension.
/// Example: archive.zip -> archive_.zip -> archive__.zip
pub fn generate_output_filename<P: AsRef<Path>>(template: &str, working_dir: P) -> PathBuf {
    let working_dir = working_dir.as_ref();
    let path = Path::new(template);

    // Split into stem and full extension (handles .tar.gz)
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let (stem, ext) = if file_name.ends_with(".tar.gz") {
        let stem = file_name.trim_end_matches(".tar.gz");
        (stem.to_string(), ".tar.gz".to_string())
    } else if file_name.ends_with(".tar.bz2") {
        let stem = file_name.trim_end_matches(".tar.bz2");
        (stem.to_string(), ".tar.bz2".to_string())
    } else if file_name.ends_with(".tar.xz") {
        let stem = file_name.trim_end_matches(".tar.xz");
        (stem.to_string(), ".tar.xz".to_string())
    } else {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (stem, ext)
    };

    let mut current_stem = stem.clone();
    let mut result = working_dir.join(format!("{current_stem}{ext}"));

    while result.exists() {
        current_stem.push('_');
        result = working_dir.join(format!("{current_stem}{ext}"));
    }

    result
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_generate_output_filename`
Expected: PASS (all 3 tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add collision-safe output filename generation

Generates unique output filenames by appending underscores before
the extension when files already exist. Handles compound extensions
like .tar.gz correctly.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Add mime-type command routing structure

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_mime_command_parsing() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["ex"]
        selection = "single"
        cmd = [
            { mime = "application/gzip", cmd = "tar -xzf $@" },
            { mime = "application/zip", cmd = "unzip $@" },
        ]
    "#).unwrap();

    match config.cmd {
        CommandSpec::MimeRouted(entries) => {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].mime, "application/gzip");
            assert_eq!(entries[0].cmd, "tar -xzf $@");
        }
        _ => panic!("Expected MimeRouted"),
    }
}

#[test]
fn test_simple_command_parsing() {
    let config: CommandConfigEntry = toml::from_str(r#"
        keys = ["z"]
        cmd = "zip -r $OUTPUT $@"
    "#).unwrap();

    match config.cmd {
        CommandSpec::Simple(cmd) => assert_eq!(cmd, "zip -r $OUTPUT $@"),
        _ => panic!("Expected Simple"),
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_mime_command_parsing test_simple_command_parsing`
Expected: FAIL with "cannot find type `CommandSpec`"

**Step 3: Write minimal implementation**

Add before `CommandConfigEntry`:

```rust
/// A single mime-type to command mapping
#[derive(Debug, Clone, Deserialize)]
pub struct MimeCommand {
    /// Mime type pattern (e.g., "application/gzip", "image/*")
    pub mime: String,
    /// Shell command template
    pub cmd: String,
    /// Optional output filename for this mime type
    #[serde(default)]
    pub output: Option<String>,
}

/// Command specification: either a simple string or mime-routed array
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CommandSpec {
    /// Simple command string
    Simple(String),
    /// Array of mime-type specific commands
    MimeRouted(Vec<MimeCommand>),
}

impl Default for CommandSpec {
    fn default() -> Self {
        CommandSpec::Simple(String::new())
    }
}
```

Update `CommandConfigEntry` to use `CommandSpec`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CommandConfigEntry {
    pub keys: Vec<String>,
    pub cmd: CommandSpec,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default = "default_separator")]
    pub separator: String,
    #[serde(default)]
    pub selection: SelectionMode,
    #[serde(default)]
    pub output: Option<String>,
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_mime_command test_simple_command`
Expected: PASS (both tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add CommandSpec for mime-type routing

Commands can now be specified as either a simple string or an array
of mime-type specific commands. This enables format-aware operations
like extract to use different tools based on file type.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Add mime-type matching and command resolution

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_resolve_command_simple() {
    let spec = CommandSpec::Simple("ls $@".to_string());
    let result = spec.resolve_for_mime(None);
    assert_eq!(result, Some(("ls $@".to_string(), None)));
}

#[test]
fn test_resolve_command_mime_exact_match() {
    let spec = CommandSpec::MimeRouted(vec![
        MimeCommand { mime: "application/gzip".to_string(), cmd: "tar -xzf $@".to_string(), output: None },
        MimeCommand { mime: "application/zip".to_string(), cmd: "unzip $@".to_string(), output: None },
    ]);

    let result = spec.resolve_for_mime(Some("application/zip"));
    assert_eq!(result, Some(("unzip $@".to_string(), None)));
}

#[test]
fn test_resolve_command_mime_no_match() {
    let spec = CommandSpec::MimeRouted(vec![
        MimeCommand { mime: "application/gzip".to_string(), cmd: "tar -xzf $@".to_string(), output: None },
    ]);

    let result = spec.resolve_for_mime(Some("application/pdf"));
    assert_eq!(result, None);
}

#[test]
fn test_resolve_command_mime_with_output() {
    let spec = CommandSpec::MimeRouted(vec![
        MimeCommand {
            mime: "application/gzip".to_string(),
            cmd: "tar -xzf $@ -C $OUTPUT".to_string(),
            output: Some("extracted".to_string()),
        },
    ]);

    let result = spec.resolve_for_mime(Some("application/gzip"));
    assert_eq!(result, Some(("tar -xzf $@ -C $OUTPUT".to_string(), Some("extracted".to_string()))));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_resolve_command`
Expected: FAIL with "no method named `resolve_for_mime`"

**Step 3: Write minimal implementation**

Add impl block for `CommandSpec`:

```rust
impl CommandSpec {
    /// Resolves the command for a given mime type.
    /// Returns the command string and optional output template.
    /// For Simple commands, mime_type is ignored.
    /// For MimeRouted commands, returns None if no match found.
    pub fn resolve_for_mime(&self, mime_type: Option<&str>) -> Option<(String, Option<String>)> {
        match self {
            CommandSpec::Simple(cmd) => Some((cmd.clone(), None)),
            CommandSpec::MimeRouted(entries) => {
                let mime = mime_type?;
                entries.iter()
                    .find(|e| Self::mime_matches(&e.mime, mime))
                    .map(|e| (e.cmd.clone(), e.output.clone()))
            }
        }
    }

    /// Checks if a mime pattern matches a mime type.
    /// Supports exact match and prefix match (e.g., "image/*" matches "image/png").
    fn mime_matches(pattern: &str, mime_type: &str) -> bool {
        if pattern == mime_type {
            return true;
        }
        if pattern.ends_with("/*") {
            let prefix = pattern.trim_end_matches("/*");
            if mime_type.starts_with(prefix) && mime_type.chars().nth(prefix.len()) == Some('/') {
                return true;
            }
        }
        false
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_resolve_command`
Expected: PASS (all 4 tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add mime-type resolution for routed commands

Adds resolve_for_mime() method that finds the matching command
for a given mime type. Supports exact matches and wildcard prefixes
like "image/*".

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Update UserCommandConfig to handle new fields

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_user_command_config_from_entry_with_all_fields() {
    let entry = CommandConfigEntry {
        keys: vec!["ex".to_string()],
        cmd: CommandSpec::MimeRouted(vec![
            MimeCommand { mime: "application/zip".to_string(), cmd: "unzip $@".to_string(), output: None },
        ]),
        interactive: false,
        separator: " ".to_string(),
        selection: SelectionMode::Single,
        output: None,
    };

    let config = UserCommandConfig::from_entry("extract".to_string(), entry);
    assert_eq!(config.name, "extract");
    assert_eq!(config.selection, SelectionMode::Single);
    assert!(matches!(config.cmd, CommandSpec::MimeRouted(_)));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_user_command_config_from_entry_with_all_fields`
Expected: May pass if previous tasks done correctly, or FAIL if `cmd` field type mismatch

**Step 3: Verify implementation**

Ensure `UserCommandConfig` uses `CommandSpec`:

```rust
#[derive(Debug, Clone)]
pub struct UserCommandConfig {
    pub name: String,
    pub keys: Vec<String>,
    pub cmd: CommandSpec,
    pub interactive: bool,
    pub separator: String,
    pub selection: SelectionMode,
    pub output: Option<String>,
}

impl UserCommandConfig {
    pub fn from_entry(name: String, entry: CommandConfigEntry) -> Self {
        Self {
            name,
            keys: entry.keys,
            cmd: entry.cmd,
            interactive: entry.interactive,
            separator: entry.separator,
            selection: entry.selection,
            output: entry.output,
        }
    }
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_user_command_config_from_entry`
Expected: PASS

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
refactor(commands): update UserCommandConfig for new command features

Updates UserCommandConfig to use CommandSpec type and include
selection mode and output fields.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Update expand method to handle $OUTPUT

**Files:**
- Modify: `src/command_queue/types.rs`
- Test: `src/command_queue/types.rs` (inline tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_expand_with_output() {
    let dir = std::env::temp_dir();
    let expanded = expand_command_with_output(
        "zip -r $OUTPUT $@",
        &[PathBuf::from("/home/test/file.txt")],
        " ",
        Some("archive.zip"),
        &dir,
    );
    assert!(expanded.contains("/home/test/file.txt"));
    assert!(expanded.contains("archive.zip"));
    assert!(!expanded.contains("$OUTPUT"));
    assert!(!expanded.contains("$@"));
}

#[test]
fn test_expand_without_output() {
    let dir = std::env::temp_dir();
    let expanded = expand_command_with_output(
        "ls $@",
        &[PathBuf::from("/home/test")],
        " ",
        None,
        &dir,
    );
    assert!(expanded.contains("/home/test"));
    assert!(!expanded.contains("$OUTPUT"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_expand_with_output test_expand_without_output`
Expected: FAIL with "cannot find function `expand_command_with_output`"

**Step 3: Write minimal implementation**

Add function:

```rust
/// Expands a command template with paths and optional output filename.
/// - $@ is replaced with shell-escaped paths joined by separator
/// - $OUTPUT is replaced with a collision-safe output filename
pub fn expand_command_with_output<P: AsRef<Path>>(
    cmd: &str,
    paths: &[PathBuf],
    separator: &str,
    output_template: Option<&str>,
    working_dir: P,
) -> String {
    let paths_str: String = paths
        .iter()
        .map(|p| shell_escape::escape(p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(separator);

    let mut result = cmd.replace("$@", &paths_str);

    if let Some(template) = output_template {
        let output_path = generate_output_filename(template, working_dir);
        let output_str = shell_escape::escape(output_path.to_string_lossy());
        result = result.replace("$OUTPUT", &output_str);
    }

    result
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_expand_with_output test_expand_without_output`
Expected: PASS (both tests)

**Step 5: Commit**

```bash
git add src/command_queue/types.rs
git commit -m "$(cat <<'EOF'
feat(commands): add expand_command_with_output for $OUTPUT placeholder

New expand function handles both $@ (paths) and $OUTPUT (generated
output filename) placeholders. Output filenames are automatically
made unique to avoid collisions.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Export new types from module

**Files:**
- Modify: `src/command_queue/mod.rs`

**Step 1: Update exports**

```rust
mod executor;
mod types;

pub use executor::CommandExecutor;
pub use types::{
    CommandConfigEntry, CommandSpec, CommandsConfig, MimeCommand,
    QueueStatus, QueuedCommand, SelectionMode, UserCommandConfig,
    expand_command_with_output, generate_output_filename,
};
```

**Step 2: Run build to verify**

Run: `cargo build`
Expected: PASS (compiles successfully)

**Step 3: Commit**

```bash
git add src/command_queue/mod.rs
git commit -m "$(cat <<'EOF'
chore(commands): export new types and functions

Exports SelectionMode, CommandSpec, MimeCommand, and new helper
functions from the command_queue module.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Update Command enum to include new fields

**Files:**
- Modify: `src/engine/commands.rs`

**Step 1: Update UserCommand variant**

Find and update the `UserCommand` variant:

```rust
/// User-defined shell command
UserCommand {
    /// Display name
    name: String,
    /// Command specification (simple or mime-routed)
    cmd: crate::command_queue::CommandSpec,
    /// Run in foreground (suspend terminal)
    interactive: bool,
    /// Separator for $@ expansion
    separator: String,
    /// Selection mode constraint
    selection: crate::command_queue::SelectionMode,
    /// Output filename template
    output: Option<String>,
},
```

**Step 2: Update add_user_commands method**

```rust
pub fn add_user_commands(&mut self, commands: &crate::command_queue::CommandsConfig) {
    for (name, entry) in commands {
        let cmd = Command::UserCommand {
            name: name.clone(),
            cmd: entry.cmd.clone(),
            interactive: entry.interactive,
            separator: entry.separator.clone(),
            selection: entry.selection,
            output: entry.output.clone(),
        };
        self.insert(entry.keys.clone(), cmd);
    }
}
```

**Step 3: Run build to verify**

Run: `cargo build`
Expected: May have errors in manager.rs (expected, will fix in next task)

**Step 4: Commit**

```bash
git add src/engine/commands.rs
git commit -m "$(cat <<'EOF'
feat(commands): add selection/output fields to UserCommand variant

Updates Command::UserCommand to include selection mode constraints
and output filename template fields.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Update manager.rs to handle new command features

**Files:**
- Modify: `src/panel/manager.rs`

**Step 1: Find the UserCommand handler**

Locate the `Command::UserCommand` match arm (around line 1329).

**Step 2: Update handler with selection validation and mime routing**

Replace the existing handler:

```rust
Command::UserCommand {
    name,
    cmd,
    interactive,
    separator,
    selection,
    output,
} => {
    let paths = self.marked_or_selected();

    // Validate selection mode
    if let Err(msg) = selection.validate(paths.len()) {
        log::warn!("Command '{}': {}", name, msg);
        // Could show user feedback here
        continue;
    }

    let working_dir = self.center.panel().path().to_path_buf();

    // Resolve command based on mime type (for single selection)
    let (resolved_cmd, resolved_output) = if paths.len() == 1 {
        // Get mime type for single file
        let ext = paths[0].extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let mime = mime_guess::from_ext(ext)
            .first()
            .map(|m| format!("{}/{}", m.type_(), m.subtype()));

        match cmd.resolve_for_mime(mime.as_deref()) {
            Some((resolved, mime_output)) => {
                // Prefer mime-specific output over global output
                (resolved, mime_output.or(output.clone()))
            }
            None => {
                if matches!(cmd, crate::command_queue::CommandSpec::MimeRouted(_)) {
                    log::warn!(
                        "Command '{}': no handler for mime type {:?}",
                        name,
                        mime
                    );
                    continue;
                }
                // Simple command - use as-is
                match &cmd {
                    crate::command_queue::CommandSpec::Simple(s) => (s.clone(), output.clone()),
                    _ => continue,
                }
            }
        }
    } else {
        // Multiple files - use simple command form
        match &cmd {
            crate::command_queue::CommandSpec::Simple(s) => (s.clone(), output.clone()),
            crate::command_queue::CommandSpec::MimeRouted(_) => {
                log::warn!(
                    "Command '{}': mime-routed commands only work with single selection",
                    name
                );
                continue;
            }
        }
    };

    // Expand command with paths and output
    let expanded_cmd = crate::command_queue::expand_command_with_output(
        &resolved_cmd,
        &paths,
        &separator,
        resolved_output.as_deref(),
        &working_dir,
    );

    if interactive {
        // Run interactively in foreground
        info!("Running interactive command '{}': {}", name, expanded_cmd);
        if let Err(e) = std::env::set_current_dir(&working_dir) {
            error!("Failed to set working directory: {e}");
        }
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(&expanded_cmd)
            .current_dir(&working_dir)
            .status();
        match result {
            Ok(status) => {
                if status.success() {
                    info!("Command '{}' completed successfully", name);
                } else {
                    let code = status.code().unwrap_or(-1);
                    error!("Command '{}' failed with exit code {}", name, code);
                }
            }
            Err(e) => {
                error!("Failed to run command '{}': {}", name, e);
            }
        }
        self.redraw_everything();
    } else {
        // Queue for background execution
        info!("Queueing command '{}': {}", name, expanded_cmd);
        if let Some(ref cmd_tx) = self.command_tx {
            let queued = crate::command_queue::QueuedCommand {
                name,
                cmd: expanded_cmd,
                working_dir,
            };
            if let Err(e) = cmd_tx.send(queued) {
                error!("Failed to queue command: {}", e);
            }
        }
    }
    self.unmark_all_items();
}
```

**Step 3: Remove old expand_command helper if exists**

Search for and remove any standalone `expand_command` function that was previously used.

**Step 4: Run build to verify**

Run: `cargo build`
Expected: PASS

**Step 5: Commit**

```bash
git add src/panel/manager.rs
git commit -m "$(cat <<'EOF'
feat(commands): implement selection validation and mime routing in handler

Updates UserCommand handler to:
- Validate selection count against SelectionMode
- Resolve mime-routed commands based on file type
- Use new expand_command_with_output for $OUTPUT support
- Handle edge cases (no mime match, multi-select with mime routing)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Remove hardcoded archive commands

**Files:**
- Modify: `src/engine/commands.rs` - Remove Zip, Tar, Extract variants
- Modify: `src/engine/opener.rs` - Remove zip(), tar(), extract() methods
- Modify: `src/panel/manager.rs` - Remove Command::Zip/Tar/Extract handlers

**Step 1: Remove enum variants from commands.rs**

Remove these lines from the `Command` enum:
```rust
Zip,
Tar,
Extract,
```

Remove from `Display` impl:
```rust
Command::Zip => write!(f, "zip selected items"),
Command::Tar => write!(f, "tar selected items"),
Command::Extract => write!(f, "extract selected archive"),
```

**Step 2: Remove methods from opener.rs**

Remove the `zip()`, `tar()`, and `extract()` methods (approximately lines 261-333).

**Step 3: Remove handlers from manager.rs**

Remove the `Command::Zip`, `Command::Tar`, and `Command::Extract` match arms.

**Step 4: Run build to verify**

Run: `cargo build`
Expected: May have warnings about unused imports (fix them)

**Step 5: Commit**

```bash
git add src/engine/commands.rs src/engine/opener.rs src/panel/manager.rs
git commit -m "$(cat <<'EOF'
refactor(commands): remove hardcoded zip/tar/extract commands

These are now replaced by user-configurable async commands with
mime-type routing support.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: Add default archive commands to example config

**Files:**
- Modify: `examples/config.toml`

**Step 1: Add commands section**

Append to `examples/config.toml`:

```toml
# --- User-defined shell commands
#
# Commands can be executed on selected files. They run asynchronously
# in the background unless 'interactive = true' is set.
#
# Placeholders:
#   $@      - Selected file paths (shell-escaped)
#   $OUTPUT - Generated output filename (collision-safe)
#
# Options:
#   keys        - Key sequences to trigger the command
#   cmd         - Shell command template (string or array for mime-routing)
#   interactive - Run in foreground (default: false)
#   separator   - Path separator for $@ (default: " ")
#   selection   - "single", "multi", or "any" (default: "any")
#   output      - Output filename template for $OUTPUT
#

[commands.zip]
keys = ["z"]
cmd = "zip -r $OUTPUT $@"
output = "archive.zip"

[commands.tar]
keys = ["t"]
cmd = "tar -czf $OUTPUT $@"
output = "archive.tar.gz"

[commands.extract]
keys = ["ex"]
selection = "single"
cmd = [
    { mime = "application/gzip", cmd = "tar -xzf $@" },
    { mime = "application/x-tar", cmd = "tar -xf $@" },
    { mime = "application/zip", cmd = "unzip $@" },
    { mime = "application/x-7z-compressed", cmd = "7z x $@" },
    { mime = "application/x-rar", cmd = "unrar x $@" },
]
```

**Step 2: Run build to verify config parses**

Run: `cargo test` (any config parsing tests)
Expected: PASS

**Step 3: Commit**

```bash
git add examples/config.toml
git commit -m "$(cat <<'EOF'
docs(config): add example archive commands

Adds zip, tar, and extract commands to the example config,
demonstrating the new mime-routing and output generation features.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Update keys.toml to remove old archive bindings

**Files:**
- Modify: `examples/keys.toml`

**Step 1: Check for and remove old bindings**

Search for and remove any `z`, `t`, or `ex` bindings that pointed to old `zip`, `tar`, `extract` commands.

**Step 2: Commit if changes made**

```bash
git add examples/keys.toml
git commit -m "$(cat <<'EOF'
chore(config): remove old archive key bindings

Archive commands are now defined in config.toml with their own
key bindings.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Run full test suite

**Step 1: Run all tests**

Run: `cargo test`
Expected: All tests PASS

**Step 2: Run clippy**

Run: `cargo clippy`
Expected: No errors (warnings acceptable)

**Step 3: Test manually**

1. Start rfm
2. Select a file, press `z` - should create archive.zip (or archive_.zip if exists)
3. Select archive.zip, press `ex` - should extract with unzip
4. Select multiple files, press `ex` - should show warning (single-only)
5. Create .tar.gz, press `ex` - should extract with tar

**Step 4: Final commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
test: verify enhanced async commands work correctly

All tests pass. Manual testing confirms:
- Selection mode validation works
- Mime-type routing selects correct tool
- Output filename generation avoids collisions

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>
EOF
)"
```

---

## Summary

This plan adds three major features to async-commands:

1. **SelectionMode** (`single`/`multi`/`any`) - Validates item count before execution
2. **Mime-type routing** - Array of `{ mime, cmd }` entries for format-specific commands
3. **Output filename** - `$OUTPUT` placeholder with collision-safe generation

The implementation removes ~80 lines of hardcoded archive logic and replaces it with ~150 lines of flexible, configurable infrastructure that users can extend for any archive format or file operation.

---

Plan complete and saved to `docs/plans/2026-02-19-enhanced-async-commands.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach?