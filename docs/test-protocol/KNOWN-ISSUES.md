# Known issues

Behaviors found by the test protocol that are **not** being hot-fixed, with the
root-cause analysis and the reason. These are deliberate deferrals, not
oversights — recorded so future runs don't re-file them and so the maintainer
can decide whether the trade-off is worth changing.

## KI-1 — Stale subdirectory child-count badge after a grandchild changes

- **Found:** run 2, while re-verifying the (fixed) preview bug at step 12.8.
- **Severity:** papercut (cosmetic). Reproduced twice.
- **Class:** render (a cached metadata annotation lags; the load-bearing
  content — center listing and preview — is always correct).

### Symptom
Enter a directory `doomed` inside `fx`, delete `doomed` from outside rfm while
inside it, then press `h` back to `fx`'s parent. The **left** column's
per-directory child-count badge for `fx` reads one higher than the on-disk
count (e.g. `fx  6` when `fx` now holds 5). It does not refresh on subsequent
`j`/`k` navigation. The center listing (correct count) and the preview column
are both right — only the left column's count annotation is stale.

### Root cause
The badge is `DirElem.suffix`, the subdirectory's child count, computed once in
`DirElem::normalize()` (`read_dir(path).count()`) and then cached
(`is_normalized`). On `h`, `ManagedPanel::new_panel_instant` serves the
grandparent panel from its cache and **returns early when the grandparent's own
mtime is unchanged**, skipping the background content refresh:

```
if mod_time == cached_mod_time { return; }   // src/panel/mod.rs
```

Deleting `doomed` changes **`fx`'s** mtime, but **not** the grandparent's
(`fx` is still present in it). Verified empirically: grandparent mtime
unchanged, `fx` mtime bumped. So the mtime-gated cache reuse legitimately
cannot see that `fx`'s child count changed, and the already-normalized `fx`
badge is never recomputed. A non-recursive watcher on the grandparent wouldn't
fire either — the change is one level deeper than it watches.

### Why it is not being hot-fixed
The staleness is inherent to a deliberate performance optimization: child-count
badges are computed once and reused, and cache reuse is gated on the
*containing* directory's mtime. Keeping every ancestor badge live would require
either (a) always kicking a background re-list on every navigation (removing the
mtime short-circuit — a real CPU cost on large directories, exactly what the
optimization avoids), or (b) recursive/relationship-aware cache invalidation
(broad change to the cache layer, regression risk). Neither is justified by a
cosmetic count badge, and a narrow special-case in `move_left` would only mask
this one path while polluting the code. The correct call is to record the
trade-off and let the maintainer decide.

### If the maintainer wants to fix it
The minimal option is to drop the early `return` in `new_panel_instant` so a
cached panel is still shown instantly but a background refresh is always
queued (the content manager re-lists and re-normalizes, refreshing badges a
frame later). Measure the navigation-time cost on large directories before
committing — that early return is the optimization being traded away.
