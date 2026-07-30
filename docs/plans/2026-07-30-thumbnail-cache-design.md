# Persistent Thumbnail Cache — Design

Status: accepted (v1 scope), 2026-07-30
Replaces the `/tmp` video-thumbnail scheme in `ffmpeg_thumbnail`
(preview.rs) and extends persistence to image previews.

## Motivation

Image and video previews are the most expensive thing rfm computes:
decoding a full-resolution photo (or running ffmpeg over a video) takes
orders of magnitude longer than everything else in the preview path.
Today the results are cached at two levels, both of which forget:

1. **In-memory `PanelCache`** (content.rs) — dies with the process.
2. **Video thumbnails in `std::env::temp_dir()`** (`ffmpeg_thumbnail`,
   preview.rs) — a de-facto persistent cache, but `/tmp` is tmpfs on
   most modern distros (gone on reboot), `systemd-tmpfiles` reaps it
   after ~10 days regardless, and the predictable filenames in a
   world-writable directory are the textbook CWE-377 symlink hazard
   (ffmpeg runs with `-y` and follows symlinks).

Moving the store to `$XDG_CACHE_HOME/rfm/thumbnails/` makes the cache
survive reboots (the point of a thumbnail cache), lands it in
user-owned 0700 territory (hazard gone), and puts it where users
expect caches to live and be reclaimable. The price: we own eviction,
because nothing cleans `~/.cache` for us.

We use **our own format**, not the freedesktop thumbnail spec — the
spec mandates PNG with embedded `Thumb::URI`/`Thumb::MTime` metadata
and md5-of-URI naming for interop we don't need; JPEG at our exact
preview resolution is smaller and decodes faster.

## Layout and key scheme

```
$XDG_CACHE_HOME/rfm/thumbnails/          (fallback ~/.cache/rfm/thumbnails/)
└── <seahash(abs_path):016x>-<mtime_secs>.jpg
```

- `xdg_cache_home()` goes next to `xdg_config_home()` in util.rs:
  `$XDG_CACHE_HOME`, else `$HOME/.cache`, else error (cache then
  disabled for the session, see error handling).
- The key stays `seahash` of the absolute path + mtime — same
  invalidation semantics as today's `/tmp` names — but with the hash
  fixed-width hex and a `-` separator so entries for one source file
  are findable by prefix scan. mtime changes → new name → old entry is
  a stale sibling (cleaned up below). No headers, no index file: the
  filename is the entire metadata.
- Old `/tmp` entries are simply abandoned; the OS cleans them.

## Read/write paths

One small module (`src/panel/thumb_cache.rs` or inside preview.rs)
with two functions used by both media types:

- `lookup(path, mtime) -> Option<DynamicImage>` — hit = decode the
  small cached JPEG (fast; it is at most preview-sized).
- `store(path, mtime, &DynamicImage)` — encode JPEG (quality 85),
  write atomically, then best-effort delete stale siblings
  (`<hash>-*` with a different mtime).

**Images** (`image_preview`): on miss, decode the original,
`.thumbnail(960, 540)` exactly as today, then `store`. On hit, skip
the full-size decode entirely — this is the big win for photo
directories revisited across sessions.

**Videos** (`ffmpeg_thumbnail`): ffmpeg writes to the cache dir
instead of `temp_dir()`. To keep writes atomic it targets a temp name
in the same directory (`<final>.<pid>.part.jpg`, extension kept so
ffmpeg's format inference still works), then `rename` to the final
name. The existing `scale=120:-1` frame extraction is unchanged.

**Atomicity rule** (both paths): never create the final name with
partial content. A half-written JPEG in `/tmp` gets reaped; in
`~/.cache` it would be a permanently corrupt entry. Temp-name +
same-dir rename also makes concurrent rfm instances safe: last rename
wins, both wrote equivalent content.

## Eviction

Startup spawns one `spawn_blocking` prune task (fire-and-forget, off
the hot path):

1. Delete entries older than **30 days** (cache-file mtime).
2. If the directory still exceeds **256 MB**, delete oldest-first
   until under the cap.

Constants in code, not config (YAGNI — revisit if anyone hits them).
Plus the inline stale-sibling cleanup on every `store`, which handles
the common churn (edited files) without waiting for the prune.
Prune races with another instance are benign: ignore `NotFound`.

## Config

One switch, next to `use_trash` in `[general]`:

```toml
preview_cache = true   # default
```

`false` disables the persistent layer entirely (no reads, no writes) —
for privacy-sensitive setups, since thumbnails of deleted media
persist until pruned. Documented alongside: `rm -rf ~/.cache/rfm` is
always safe.

## Error handling

Every cache operation is best-effort and non-fatal:

- Cache dir can't be resolved/created → log a warning once, run the
  session with persistence off (in-memory caches still work).
- `lookup` decode failure (corrupt entry) → delete the entry, treat as
  miss, regenerate.
- `store`/prune I/O errors → `log::debug!`, continue. A preview is
  never lost to a cache failure, only regenerated.

## Testing

- **Unit** (terminal-free, tempfile fixtures, like undo/):
  key derivation and filename round-trip; stale-sibling cleanup on
  `store`; prune age rule; prune size cap deletes oldest-first;
  corrupt-entry lookup deletes and misses; `preview_cache = false`
  writes nothing.
- **Integration** (tmux + debug socket): set `XDG_CACHE_HOME` to a
  scratch dir in the tmux pane; preview an image fixture; assert the
  cache file appears; restart rfm; `log` history shows the cache-hit
  trace line instead of a decode. Fixture names with spaces per the
  shell-escape house rule (paths here never pass through `sh -c`, but
  the fixtures should prove it).

## Out of scope (v1)

- Caching non-media previews (bat/text output, archive listings) —
  cheap to regenerate, not worth the invalidation surface.
- freedesktop thumbnail spec interop.
- Config knobs for cap/age; a cache-stats debug-socket command.
- Migrating existing `/tmp` entries.
