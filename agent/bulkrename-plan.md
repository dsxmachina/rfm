# Bulkrename Implementation Plan

## Overview

Extend the rename command to support bulk renaming when multiple files are marked.
Uses the user's configured text editor (via OpenEngine) to edit filenames.

## Implementation Steps

### Step 1: Add `open_blocking` to OpenEngine

**File:** `src/engine/opener.rs`

Add a new method `open_blocking()` that:
- Works like `open()` but always waits for the process to complete
- Returns `Result<ExitStatus>` so caller can check if editor closed normally
- Useful for operations that need the editor result (like bulkrename)

### Step 2: Modify Command::Rename handling

**File:** `src/panel/manager.rs`

Change the `Command::Rename` match arm:
- Check `marked_items()` to see if multiple files are marked
- If multiple files → call new `bulkrename()` method
- If single file (or none marked) → existing rename mode (unchanged)

### Step 3: Implement `bulkrename()` method

**File:** `src/panel/manager.rs`

New method `bulkrename(&mut self, files: Vec<PathBuf>) -> Result<()>`:

1. **Create temp file:**
   - Path: `/tmp/rfm-bulkrename-XXXXX` (use tempfile crate or manual)
   - Content: one filename per line (just the filename, not full path)

2. **Open editor:**
   - Call `self.opener.open_blocking(temp_file_path)`
   - Handle editor failure gracefully

3. **Parse and validate:**
   - Read the edited file
   - If unchanged from original → return early (no-op)
   - Check line count matches original file count
   - Check for duplicate target names within batch
   - Check for conflicts with existing files on disk

4. **Handle conflicts:**
   - If any validation fails, reopen editor with error comments
   - Format: `filename # error message here`
   - Loop until valid or user provides empty/unchanged file

5. **Execute renames:**
   - For each (old_name, new_name) pair where old != new
   - Use `rename_safe()` with underscore fallback
   - Log results

6. **Cleanup:**
   - Delete temp file
   - Unmark all items
   - Reload panels

### Step 4: Add `rename_safe()` helper

**File:** `src/util.rs`

New function `rename_safe(from: &Path, to: &Path) -> Result<PathBuf>`:
- Attempts `std::fs::rename(from, to)`
- If target exists, appends underscores until unique (like `get_destination`)
- Returns the actual path the file was renamed to

## Validation Rules

1. **Line count:** Must match original file count (no lines added/deleted)
2. **No duplicates:** No two lines can have the same target name
3. **No conflicts:** Target name must not exist on disk (unless same as source)
4. **Unchanged check:** If all lines match originals, skip silently

## Error Comment Format

When reopening editor with errors:
```
new-name.txt # already exists, choose different name
other-file.txt # duplicate target name
valid-name.txt
```

## Safety

- Always use underscore-appending as final fallback (race condition protection)
- Delete temp file even on error
- Don't modify original files if any validation fails
