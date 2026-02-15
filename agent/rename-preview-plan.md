# Live Rename Preview Implementation Plan

## Overview
Implement live rename preview with repositioning, similar to the existing `new_element` preview for creating files/directories. The renamed entry will:
1. Show in **blue** color (new color constant)
2. Appear at its sorted position (live repositioning)
3. Hide the original entry during rename

## Color Scheme Reference
- **Red** (`color_highlight`): Search matches
- **Yellow** (`color_marked`): Marked items / new elements
- **Blue** (NEW `color_rename`): Rename preview

## Implementation Steps

### Step 1: Add new color constant for rename preview
**File:** `src/config.rs`

Add a new `COLOR_RENAME` static and helper function:
- Add `pub static COLOR_RENAME: OnceCell<Color> = OnceCell::new();`
- Add `color_rename()` helper function returning `Color::Blue`
- Update `colors_from_default()` to set the default blue color
- Update `ColorConfig` struct and `colors_from_config()` to support custom rename color

### Step 2: Add rename preview field to DirPanel
**File:** `src/panel/directory.rs`

Add to `DirPanel` struct:
```rust
/// Rename preview - (new_name, original_index, is_dir)
/// When set, the original item at original_index is hidden
/// and a preview with new_name is shown at the sorted position
rename_preview: Option<(String, usize, bool)>,
```

Add methods:
- `inject_rename_preview(&mut self, new_name: String, original_idx: usize)`
- `clear_rename_preview(&mut self)`

### Step 3: Update DirPanel::draw() for rename preview
**File:** `src/panel/directory.rs`

Modify the `draw()` function to handle rename preview similar to `new_element`:
1. Add a new branch after the `new_element` check
2. When `rename_preview` is set:
   - Calculate the new sorted position using `partition_point()`
   - Skip rendering the original item at `original_idx`
   - Insert the preview item at the calculated position with blue color
3. Preserve the suffix (file size) from the original item

### Step 4: Update Mode::Rename handler to inject preview
**File:** `src/panel/manager.rs`

Modify the `Mode::Rename { input }` handler:
1. On key press (non-Enter), call `inject_rename_preview()` with:
   - The current input text
   - The current selected index
2. On Enter (confirm), call `clear_rename_preview()` before the actual rename
3. On Esc (cancel), `clear_rename_preview()` is already called via the escape handler

### Step 5: Update escape handler to clear rename preview
**File:** `src/panel/manager.rs`

Add `clear_rename_preview()` call alongside existing `clear_new_element()` in the escape handler.

## Detailed Implementation

### config.rs changes
```rust
// Add after COLOR_DIR_PATH
pub static COLOR_RENAME: OnceCell<Color> = OnceCell::new();

// In ColorConfig struct - add:
rename: Option<String>,  // Optional to maintain backwards compatibility

// In colors_from_config - add:
let rename = config.rename
    .map(extract_color)
    .transpose()
    .context("Failed to set 'rename' color")?
    .unwrap_or(Color::Blue);
COLOR_RENAME.set(rename).expect("color must be unset");

// In colors_from_default - add:
COLOR_RENAME.set(Color::Blue).expect("color must be unset");

// Add helper function:
#[inline]
pub fn color_rename() -> Color {
    *COLOR_RENAME.get().expect("color must be set")
}
```

### directory.rs changes

```rust
// In DirPanel struct - add:
rename_preview: Option<(String, usize, bool)>,

// Add methods:
pub fn inject_rename_preview(&mut self, new_name: String, original_idx: usize) {
    let is_dir = self.elements
        .get(original_idx)
        .map(|e| e.path().is_dir())
        .unwrap_or(false);
    self.rename_preview = Some((new_name, original_idx, is_dir));
}

pub fn clear_rename_preview(&mut self) {
    self.rename_preview = None;
}
```

In `DirPanel::new()` and `DirPanel::loading()` and `DirPanel::empty()`:
```rust
rename_preview: None,
```

In `draw()` - add new branch after `new_element`:
```rust
} else if let Some((new_name, original_idx, is_dir)) = &self.rename_preview {
    // Similar to new_element but:
    // 1. Skip the item at original_idx
    // 2. Use color_rename() instead of color_highlight()
    // 3. Get the suffix from the original element
    let lowercase_name = new_name.to_lowercase();
    let original_suffix = self.elements
        .get(*original_idx)
        .map(|e| e.suffix.clone())
        .unwrap_or_default();

    let (partition, symbol) = if *is_dir {
        (
            self.elements
                .iter()
                .enumerate()
                .filter(|(idx, _)| idx != original_idx)
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .filter(|(_, elem)| elem.path().is_dir())
                .take_while(|(_, elem)| elem.lowercase < lowercase_name)
                .count(),
            "\u{1F4C1}",
        )
    } else {
        // Count directories + files that come before
        (
            self.elements
                .iter()
                .enumerate()
                .filter(|(idx, _)| idx != original_idx)
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .take_while(|(_, elem)| {
                    elem.path().is_dir() || elem.lowercase < lowercase_name
                })
                .count(),
            "\u{1F5B9} ",
        )
    };

    // Render loop skipping original_idx, inserting preview at partition
    // ... (full implementation in step 3)
}
```

### manager.rs changes

In `Mode::Rename { input }` handler (around line 1160-1163):
```rust
} else {
    input.update(key_event.code, key_event.modifiers);
    let selected_idx = self.center.panel().selected_idx();
    self.center
        .panel_mut()
        .inject_rename_preview(input.get().to_string(), selected_idx);
    self.redraw_center();
}
```

In escape handler (around line 895):
```rust
self.center.panel_mut().clear_new_element();
self.center.panel_mut().clear_rename_preview();  // Add this line
```

## Implementation Status

All steps completed:
- [x] Step 1: Add COLOR_RENAME constant in config.rs
- [x] Step 2: Add rename_preview field and methods to DirPanel
- [x] Step 3: Update DirPanel::draw() for rename preview
- [x] Step 4: Update Mode::Rename handler to inject preview
- [x] Step 5: Update escape handler to clear rename preview

## Testing Checklist
- [ ] Rename a file - see blue preview at correct position
- [ ] Rename a directory - see blue preview at correct position
- [ ] Rename to move item up alphabetically - preview appears higher
- [ ] Rename to move item down alphabetically - preview appears lower
- [ ] Press Enter - rename completes, list reloads
- [ ] Press Escape - preview clears, original restored
- [ ] Edge case: Empty name - handle gracefully
- [ ] Edge case: Same name - preview stays in place
- [ ] Hidden files: Works correctly when show_hidden is toggled

## Commits
1. "Add COLOR_RENAME constant for rename preview (blue)"
2. "Add rename_preview field and methods to DirPanel"
3. "Implement rename preview rendering in DirPanel::draw()"
4. "Wire up rename preview in Mode::Rename handler"
