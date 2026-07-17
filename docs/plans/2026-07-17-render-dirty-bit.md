# Render: single dirty bit + image cache + log TTL — Implementation Plan

> **For Claude:** Execute task-by-task (subagent-driven), TDD where behavior is testable, e2e over the debug socket where it's visual. One commit per task.

**Goal:** Replace the 8-boolean `Redraw` dependency tree with a single `dirty: bool` (perma-redraw when set), after making full-screen repaint cheap by caching the image-preview resize; and give log lines a real per-line TTL. Deletes the stale-pane bug class (including the verified `Command::Cut` bug), the FooterLine stays-dirty quirk, and ~140 scattered `redraw_*()` calls.

**Design (settled with the maintainer — do not re-litigate):**
1. **Single dirty bit.** `PanelManager.dirty: bool`; `mark_dirty()` sets it. `draw()` early-returns when clear, otherwise repaints *everything* in call order and clears it. No per-element flags, no dependency edges, no per-element draw guards.
2. **Perma-redraw is cheap because** every `draw_*` is a small blit — except the image preview, which re-runs `thumbnail()` (resize) every draw. Fix that first with a render cache keyed on `(width, height)`; recompute only on resize / new image.
3. **Implicit z-order** stays: `draw()` calls footer → header → panels → console → log in sequence, overlay last. No explicit z-index.
4. **Event-driven, not a render loop.** The `tokio::select!` loop turns only on real events; idle = no draw. So a set-dirty-per-event rule has no idle cost.
5. **Log TTL.** Display buffer stores `(Level, Instant, String)`; the 1 s periodic task removes *all* lines older than a TTL (not one-per-tick) and wakes the loop (→ `mark_dirty`) only when it actually removed a visible line.

**Dirty-setting rule (foolproof by design):** every key event, every async panel/preview update, and every resize calls `mark_dirty()`. Debug-socket requests do NOT (read-only observers). Setting dirty on a no-op key is harmless (a redundant repaint), so err toward setting it.

**Verification:** unit tests where pure; e2e via the debug socket (CLAUDE.md) for anything visual. After the collapse, the `Command::Cut` mark must appear immediately (regression test of the old bug).

---

## Task 1: Image-preview render cache

**Files:** Modify `src/panel/preview.rs`

**Problem (verified):** `FilePreview::draw` for `Preview::Image` calls `img.thumbnail(width, thumbnail_height).into_rgb8()` on **every** draw (preview.rs:74-76). With perma-redraw this runs on every event while an image is shown. The source is already a ≤960×540 thumbnail from the async path, so the draw-time resize is redundant whenever `(width, thumbnail_height)` is unchanged.

**Step 1 — write the failing test.** Add a rebuild counter so caching is observable. In `preview.rs` give the image a small cache and a test:

```rust
#[cfg(test)]
mod render_cache_tests {
    use super::*;
    use image::{DynamicImage, RgbImage};

    fn tiny_preview() -> FilePreview {
        // 64x64 solid image, no info lines
        let img = DynamicImage::ImageRgb8(RgbImage::new(64, 64));
        FilePreview::from_image_for_test(img)   // small test constructor
    }

    #[test]
    fn resize_is_cached_across_equal_dimensions() {
        let mut p = tiny_preview();
        let a = p.resized_rgb(20, 30).clone();
        let b = p.resized_rgb(20, 30).clone();       // same dims → cache hit
        assert_eq!(p.resize_count(), 1, "second call must hit the cache");
        assert_eq!(a.dimensions(), b.dimensions());
    }

    #[test]
    fn resize_recomputes_when_dimensions_change() {
        let mut p = tiny_preview();
        p.resized_rgb(20, 30);
        p.resized_rgb(21, 30);                         // width changed → miss
        assert_eq!(p.resize_count(), 2);
    }
}
```

Run: `cargo test render_cache` → FAIL (`resized_rgb`/`resize_count`/`from_image_for_test` undefined).

**Step 2 — implement.** Store the cache so the resize memoizes. Because `draw` matches on `&self.preview` while mutating a cache, use disjoint field borrows or put the cache **inside** the `Preview::Image` variant. Recommended shape — a helper on `FilePreview`:

```rust
struct ResizeCache {
    w: u32,
    h: u32,
    rgb: image::RgbImage,
    count: u32,   // test-only observability; cheap to keep
}
// field on FilePreview: resize_cache: Option<ResizeCache>

impl FilePreview {
    /// Resized RGB for the given cell dimensions, memoized. Only re-runs
    /// image::thumbnail when the requested size changes.
    fn resized_rgb(&mut self, w: u32, h: u32) -> &image::RgbImage {
        let src = /* the Option<DynamicImage> from self.preview; early-out handled by caller */;
        let need = self.resize_cache.as_ref().map_or(true, |c| c.w != w || c.h != h);
        if need {
            let prev = self.resize_cache.as_ref().map_or(0, |c| c.count);
            let rgb = src.thumbnail(w, h).into_rgb8();
            self.resize_cache = Some(ResizeCache { w, h, rgb, count: prev + 1 });
        }
        &self.resize_cache.as_ref().unwrap().rgb
    }
    #[cfg(test)] fn resize_count(&self) -> u32 { self.resize_cache.as_ref().map_or(0, |c| c.count) }
}
```

Then rewrite the `Preview::Image` arm of `draw` to call `self.resized_rgb(width, thumbnail_height)` instead of the inline `thumbnail(...)`, keeping the half-block pixel loop unchanged (it reads from the returned `RgbImage`). Add the `from_image_for_test` constructor behind `#[cfg(test)]`. Adjust the borrow structure so `draw` compiles (destructure `self` or read the needed dims first, then call `resized_rgb`).

Note: the cache lives on the `FilePreview`, which is replaced wholesale when a new preview arrives from the async pipeline — so a new image naturally gets a fresh (empty) cache. Only resize (width/height change) invalidates within one preview.

Run: `cargo test render_cache` → PASS; `cargo test` → full suite green; `cargo fmt --check`.

**Step 3 — e2e.** Debug socket + tmux (CLAUDE.md): open a dir with an image, select it (image renders in the right pane, verify via capture-pane), press a key that redraws without changing the preview and confirm it still renders; check `log` for the `converting img:` debug line firing once per size, not per keypress. Clean up.

**Step 4 — commit:** `feat(preview): cache the image resize instead of recomputing per draw`
(trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`)

---

## Task 2: Collapse `Redraw` → single `dirty: bool`

**Files:** Modify `src/panel/manager.rs`

This is the big mechanical change; behavior must be preserved (visual regressions are the risk). No unit test (needs a terminal); verified by e2e + the Cut regression.

**Step 1 — read first.** Note every `redraw_*` method (manager.rs ~242-289), every call site (~140), the `draw()` structure (~469-492, per-element guards + clears), the FooterLine stays-dirty early return in `draw_footer` (~390-400), and the loop's draw call + `debug_seq` handling.

**Step 2 — replace the mechanism.**
- Delete `struct Redraw`, its `any()`, the `redraw: Redraw` field, and all `redraw_*` methods. Add `dirty: bool` field (init `true`) and:
  ```rust
  fn mark_dirty(&mut self) { self.dirty = true; }
  ```
- Replace every `self.redraw_x()` / `self.redraw.x = true` call site with `self.mark_dirty()`. (Many collapse to duplicates in one function — that's fine, keep one.)
- `draw()` becomes:
  ```rust
  fn draw(&mut self) -> Result<()> {
      if !self.dirty { return Ok(()); }
      self.stdout.execute(BeginSynchronizedUpdate)?;
      self.stdout.queue(cursor::Hide)?;
      self.draw_footer()?;
      self.draw_header()?;
      self.draw_panels()?;
      self.draw_console()?;
      self.draw_log()?;
      self.stdout.execute(EndSynchronizedUpdate)?;
      self.dirty = false;
      Ok(())
  }
  ```
- In each `draw_footer/header/left/center/right/console/log`: remove the `if redraw.X` guard and the `redraw.X = false` clear — they now always paint when `draw()` runs. **Delete the FooterLine stays-dirty early-return quirk** (the modal footer now repaints whenever `dirty` is set, which every modal keystroke sets); the modal still draws itself via the existing `ModalRegion::FooterLine` delegation, just without the special return-without-clear.
- Keep `draw_console`'s internal "only draw the overlay when in a ConsoleOverlay modal" check and `draw_log`'s `show_log` check — those are content conditions, not dirty flags.

**Step 3 — the loop.** In `run()`: replace the initial `redraw_everything(); draw()` with `self.dirty = true; self.draw()?;`. In the select arms, replace flag-setting with `mark_dirty()`; the **debug arm keeps its `continue`** (no dirty, no draw, no seq bump — unchanged). Every key/panel/resize arm marks dirty. `debug_seq` increment at loop bottom stays as-is.

**Step 4 — verify + Cut regression.**
- `cargo build` (5 pre-existing warnings), `cargo test` (green), `cargo clippy`, `cargo fmt --check`.
- e2e sweep (debug socket): navigate (selection + preview update), toggle hidden, toggle log, enter/exit each modal, resize — all repaint correctly; overlay close repaints cleanly (panels paint over where the overlay was — now automatic).
- **Cut regression:** on a fixture with an unmarked selection, send the Cut key, then `entries center` / capture-pane and confirm the item shows as marked immediately (the old bug left it unmarked until the next redraw). Compare with Copy (should be identical now).
- Confirm no idle repaints: after the sweep, sit idle and confirm `seq` is stable (debug queries don't bump it) and nothing repaints.

**Step 5 — commit:** `refactor(render): replace the Redraw flag tree with a single dirty bit`

---

## Task 3: Per-line log TTL

**Files:** Modify `src/logger.rs`, `src/main.rs`

**Step 1 — failing test** in `logger.rs`:

```rust
#[test]
fn expired_lines_are_removed_by_ttl() {
    let buffer = LogBuffer::default();
    log_line(&buffer, Level::Info, "old");
    // nothing expired yet
    assert!(!buffer.remove_expired(Instant::now()));
    assert_eq!(buffer.get().len(), 1);
    // far enough in the future → the line is past its TTL
    let future = Instant::now() + DISPLAY_TTL + Duration::from_secs(1);
    assert!(buffer.remove_expired(future), "must report it removed a line");
    assert!(buffer.get().is_empty());
}

#[test]
fn remove_expired_reports_false_when_nothing_expires() {
    let buffer = LogBuffer::default();
    log_line(&buffer, Level::Info, "fresh");
    assert!(!buffer.remove_expired(Instant::now()));
}
```

Run → FAIL (`remove_expired`/`DISPLAY_TTL` undefined; display buffer has no timestamp).

**Step 2 — implement.**
- Change the display buffer element type from `(Level, String)` to `(Level, Instant, String)`. `get()` returns `(Level, String)` still (strip the instant) so `draw_log` and callers are unchanged — or return the instant too and adjust `draw_log`; prefer keeping `get()`'s public shape `(Level, String)`.
- `log()` pushes `(level, Instant::now(), line)`; capacity bound unchanged.
- Add `pub const DISPLAY_TTL: Duration = Duration::from_secs(10);` (readable-but-transient; tune later — note in commit).
- Replace `remove_oldest` with:
  ```rust
  /// Drop all display lines older than DISPLAY_TTL relative to `now`.
  /// Returns true (and notifies) iff at least one line was removed, so the
  /// UI wakes and repaints only when something visibly changed.
  pub fn remove_expired(&self, now: Instant) -> bool {
      let mut buffer = self.buffer.lock();
      let before = buffer.len();
      buffer.retain(|(_, at, _)| now.duration_since(*at) < DISPLAY_TTL);
      let removed = buffer.len() < before;
      drop(buffer);
      if removed { self.notify.notify_one(); }
      removed
  }
  ```
- Update the history-eviction unit tests that call `remove_oldest()` (logger.rs ~161-163, 226) to use `remove_expired(Instant::now() + DISPLAY_TTL + 1s)` (evict all) — the history ring is unaffected by display expiry, which is the property they assert.
- **main.rs periodic task:** replace `periodic_logger.remove_oldest();` with `periodic_logger.remove_expired(Instant::now());` (still every 1 s). It now wakes the manager (via the notify inside) only when a line actually expires → the manager's `logger.update()` arm calls `mark_dirty()` → one repaint. Idle-empty buffer → nothing removed → no notify → no repaint.

**Step 3 — verify.** `cargo test` (green, incl. the two new + updated eviction tests). e2e: trigger a log line (e.g. a failing background command), confirm it shows, then sit idle and confirm it disappears ~`DISPLAY_TTL` later *on its own* (previously it lingered until the next keypress), and that the disappearance triggers exactly one repaint (`seq` bumps once, screen clears the line). Clean up.

**Step 4 — commit:** `feat(logger): per-line display TTL, wake the UI only on real expiry`

---

## Task 4: Docs + final review

**Files:** Modify `CLAUDE.md`; append outcome to this plan.

**Step 1 — CLAUDE.md** add under the architecture section:

```
## Architecture: rendering

Event-driven, not a render loop: the select loop draws only when an event
sets the single `dirty` bit (PanelManager). `draw()` repaints everything
in call order (footer → header → panels → console → log, overlay last —
that ordering *is* the z-order) and clears the bit. No per-element dirty
flags: any state change calls `mark_dirty()`, so panels can't go stale.
Full repaint is cheap because every draw is a blit except the image
preview, whose resize is cached (FilePreview, keyed on cell dimensions).
Adding a window ≈ carve its layout region + a draw call at the right spot
in the sequence; no flag plumbing.

Log lines shown in the widget expire after DISPLAY_TTL (logger.rs); the
1 s task wakes the UI only when a line actually expires.
```

**Step 2 — gates:** `cargo test`, `cargo clippy`, `cargo fmt --check` all clean.

**Step 3 — append `## Outcome`** to this plan: commit SHAs, test count, the Cut bug fixed, quirk deleted, honest note on any behavior change (log lines now expire on a fixed per-line TTL rather than one-pop-per-second; default TTL value).

**Step 4 — commit:** `docs: record the single-dirty-bit render model`

Then final whole-diff review of the four tasks.
