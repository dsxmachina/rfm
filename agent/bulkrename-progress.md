# Bulkrename Implementation Progress

## Status: Complete

## Implementation Summary

### Changes Made

1. **`src/engine/opener.rs`**
   - Added `open_blocking` method to `Application` struct (always waits for process)
   - Added `open_blocking` method to `OpenOptions` struct
   - Added `open_text_blocking` method to `OpenEngine` (opens text files in blocking mode)
   - Falls back to `$EDITOR` or `vi` if no text opener is configured

2. **`src/util.rs`**
   - Added `rename_safe` function that renames files with underscore fallback
   - Added unit test `test_rename_safe` to verify the behavior

3. **`src/panel/manager.rs`**
   - Added `bulkrename` method that:
     - Creates temp file with filenames
     - Opens with configured text editor
     - Validates changes (unchanged check, line count, duplicates, conflicts)
     - Reopens editor with error comments if validation fails
     - Handles swap scenarios (a->b, b->a) with temporary renames
     - Uses `rename_safe` for final renames
   - Modified `Command::Rename` to detect bulk vs single rename
     - If >1 marked items → bulkrename
     - If 0-1 marked items → existing inline rename

### Validation Rules Implemented

1. **Unchanged check**: If all lines match originals, exit silently (no-op)
2. **Line count**: Must match original file count (no lines added/deleted)
3. **No duplicates**: No two lines can have the same target name
4. **No conflicts**: Target name must not exist on disk (unless being renamed in same batch)
5. **Underscore fallback**: Safety mechanism for race conditions

### Error Handling

- Editor non-zero exit → abort
- Validation errors → reopen editor with `# error message` comments
- I/O errors → logged and operation aborted

## Testing

- All 21 existing tests pass
- Added `test_rename_safe` unit test for the underscore fallback logic
