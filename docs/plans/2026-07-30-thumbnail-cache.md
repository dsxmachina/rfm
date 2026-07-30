# Persistent Thumbnail Cache — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes. Work
> directly on `develop`.

**Goal:** Persist image/video preview thumbnails in
`$XDG_CACHE_HOME/rfm/thumbnails/` so they survive restarts, replacing the
`/tmp` scheme in `ffmpeg_thumbnail`.

**Architecture:** A new `src/panel/thumb_cache.rs` module owns the cache:
filename = `<seahash(abs path):016x>-<mtime_secs>.jpg` (the filename is all
the metadata), atomic writes via same-dir `.part` + rename, stale-sibling
cleanup on store, startup prune (30 days / 256 MB). Core functions take an
explicit `dir: &Path` (unit-testable with `tempfile`); thin public wrappers
consult a `OnceCell<Option<PathBuf>>` initialized from config
(`[general] preview_cache`, default true). `image_preview` gains a caching
front-end; `ffmpeg_thumbnail` retargets from `temp_dir()` to the cache dir.

**Tech Stack:** existing deps only — `image` 0.24 (JPEG encode/decode),
`seahash`, `once_cell`, `tempfile` (tests). No new crates. MSRV 1.83
(`File::set_modified` needs 1.75 — OK).

**Design doc:** `docs/plans/2026-07-30-thumbnail-cache-design.md`

Every task ends with: `cargo test && cargo clippy --all-targets` clean,
then commit.

---

### Task 1: `xdg_cache_home()` in util.rs

**Files:**
- Modify: `src/util.rs` (next to `xdg_config_home`, ~line 361)

**Step 1: Write the failing test** (in util.rs's test area; the pure
`_from` variant avoids racy `set_var` in parallel tests)

```rust
#[test]
fn xdg_cache_home_prefers_env_then_home() {
    use std::ffi::OsString;
    assert_eq!(
        xdg_cache_home_from(Some(OsString::from("/xdg/cache")), Some(OsString::from("/home/u")))
            .unwrap(),
        PathBuf::from("/xdg/cache")
    );
    assert_eq!(
        xdg_cache_home_from(None, Some(OsString::from("/home/u"))).unwrap(),
        PathBuf::from("/home/u/.cache")
    );
    assert!(xdg_cache_home_from(None, None).is_err());
}
```

**Step 2:** `cargo test xdg_cache_home` — expect FAIL (not defined).

**Step 3: Implement**

```rust
/// $XDG_CACHE_HOME, else $HOME/.cache (mirrors `xdg_config_home`).
pub fn xdg_cache_home() -> anyhow::Result<PathBuf> {
    xdg_cache_home_from(
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
    )
}

fn xdg_cache_home_from(
    xdg_cache: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> anyhow::Result<PathBuf> {
    match (xdg_cache, home) {
        (Some(cache), _) => Ok(PathBuf::from(cache)),
        (None, Some(home)) => Ok(PathBuf::from(home).join(".cache")),
        (None, None) => Err(anyhow!(
            "Neither the XDG_CACHE_HOME nor the HOME environment variable was set."
        )),
    }
}
```

**Step 4:** `cargo test xdg_cache_home` — PASS.
**Step 5:** Commit: `feat(util): add xdg_cache_home helper`

---

### Task 2: thumb_cache module — entry naming

**Files:**
- Create: `src/panel/thumb_cache.rs`
- Modify: `src/panel/mod.rs` (add `pub mod thumb_cache;`)

**Step 1: Failing test** (bottom of the new file)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn entry_name_is_hexhash_dash_mtime_jpg() {
        let name = entry_name(Path::new("/some/pic.png"), 1700000000);
        let (hash, rest) = name.split_at(16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(rest, "-1700000000.jpg");
        // same path, different mtime → same prefix, different name
        let other = entry_name(Path::new("/some/pic.png"), 1700000001);
        assert_eq!(other[..17], name[..17]);
        assert_ne!(other, name);
        // different path → different prefix
        assert_ne!(entry_name(Path::new("/other.png"), 1700000000)[..17], name[..17]);
    }
}
```

**Step 2:** `cargo test entry_name` — FAIL.

**Step 3: Implement** (module header + function)

```rust
//! Persistent thumbnail cache in $XDG_CACHE_HOME/rfm/thumbnails.
//!
//! The filename is the entire metadata:
//! `<seahash(abs path):016x>-<mtime_secs>.jpg`. Writes are atomic
//! (same-dir `.part` + rename); every store cleans stale siblings of the
//! same path-hash. See docs/plans/2026-07-30-thumbnail-cache-design.md.
use std::path::{Path, PathBuf};

/// Cache filename for `path` at `mtime_secs`. Fixed-width hex hash +
/// `-` separator keeps entries for one source file prefix-scannable.
pub(crate) fn entry_name(path: &Path, mtime_secs: u64) -> String {
    format!("{:016x}-{mtime_secs}.jpg", seahash::hash(path.as_os_str().as_encoded_bytes()))
}

fn hash_prefix(path: &Path) -> String {
    format!("{:016x}-", seahash::hash(path.as_os_str().as_encoded_bytes()))
}
```

**Step 4:** `cargo test entry_name` — PASS.
**Step 5:** Commit: `feat(thumb-cache): entry naming scheme`

---

### Task 3: store/lookup round-trip, atomicity, corrupt entries

**Files:**
- Modify: `src/panel/thumb_cache.rs`

**Step 1: Failing tests**

```rust
use image::DynamicImage;

fn test_img() -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10])))
}

#[test]
fn store_then_lookup_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new("/photos/a.png");
    assert!(lookup_in(dir.path(), src, 100).is_none());
    store_in(dir.path(), src, 100, &test_img()).unwrap();
    let hit = lookup_in(dir.path(), src, 100).expect("cache hit");
    assert_eq!((hit.width(), hit.height()), (4, 4));
    // wrong mtime is a miss
    assert!(lookup_in(dir.path(), src, 101).is_none());
    // no .part leftovers — final name only
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, vec![entry_name(src, 100)]);
}

#[test]
fn corrupt_entry_is_deleted_and_misses() {
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new("/photos/a.png");
    let entry = dir.path().join(entry_name(src, 100));
    std::fs::write(&entry, b"not a jpeg").unwrap();
    assert!(lookup_in(dir.path(), src, 100).is_none());
    assert!(!entry.exists(), "corrupt entry must be removed");
}
```

**Step 2:** `cargo test thumb_cache` — FAIL.

**Step 3: Implement**

```rust
use image::codecs::jpeg::JpegEncoder;
use std::io::BufWriter;

const JPEG_QUALITY: u8 = 85;

/// Decode the cache entry for (`src_path`, `mtime_secs`), if present.
/// A corrupt entry is deleted and treated as a miss.
fn lookup_in(dir: &Path, src_path: &Path, mtime_secs: u64) -> Option<DynamicImage> {
    let entry = dir.join(entry_name(src_path, mtime_secs));
    match image::io::Reader::open(&entry).ok()?.decode() {
        Ok(img) => Some(img),
        Err(e) => {
            log::debug!("removing corrupt thumbnail {}: {e}", entry.display());
            let _ = std::fs::remove_file(&entry);
            None
        }
    }
}

/// Encode `img` as JPEG into the cache, atomically: write a same-dir
/// `.part` file, then rename. Never leaves a partial final entry.
fn store_in(dir: &Path, src_path: &Path, mtime_secs: u64, img: &DynamicImage) -> anyhow::Result<()> {
    let name = entry_name(src_path, mtime_secs);
    let final_path = dir.join(&name);
    if final_path.exists() {
        return Ok(());
    }
    let part = dir.join(format!("{name}.{}.part", std::process::id()));
    let write = (|| -> anyhow::Result<()> {
        let mut w = BufWriter::new(std::fs::File::create(&part)?);
        JpegEncoder::new_with_quality(&mut w, JPEG_QUALITY).encode_image(&img.to_rgb8())?;
        Ok(())
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    std::fs::rename(&part, &final_path)?;
    Ok(())
}
```

(`to_rgb8()` matters: JPEG in image 0.24 rejects RGBA input.)

**Step 4:** `cargo test thumb_cache` — PASS.
**Step 5:** Commit: `feat(thumb-cache): atomic store and lookup`

---

### Task 4: stale-sibling cleanup on store

**Files:**
- Modify: `src/panel/thumb_cache.rs`

**Step 1: Failing test**

```rust
#[test]
fn store_removes_stale_siblings_but_not_other_paths() {
    let dir = tempfile::tempdir().unwrap();
    let src = Path::new("/photos/a.png");
    let other = Path::new("/photos/b.png");
    store_in(dir.path(), src, 100, &test_img()).unwrap();
    store_in(dir.path(), other, 100, &test_img()).unwrap();
    // orphaned .part from a dead process, same prefix
    std::fs::write(dir.path().join(format!("{}.999.part", entry_name(src, 90))), b"x").unwrap();
    store_in(dir.path(), src, 200, &test_img()).unwrap();
    assert!(lookup_in(dir.path(), src, 200).is_some());
    assert!(lookup_in(dir.path(), src, 100).is_none(), "stale sibling gone");
    assert!(lookup_in(dir.path(), other, 100).is_some(), "other path untouched");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
}
```

**Step 2:** `cargo test stale_siblings` — FAIL.

**Step 3: Implement** — add to the end of `store_in` (after the rename)
a call to `cleanup_stale(dir, src_path, &name);` and:

```rust
/// Remove every entry sharing `src_path`'s hash prefix except `keep`.
/// Also catches orphaned `.part` files of that prefix. Errors ignored:
/// a racing instance may have removed the file already.
pub(crate) fn cleanup_stale(dir: &Path, src_path: &Path, keep: &str) {
    let prefix = hash_prefix(src_path);
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix) && name != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
```

**Step 4:** `cargo test thumb_cache` — PASS.
**Step 5:** Commit: `feat(thumb-cache): stale-sibling cleanup on store`

---

### Task 5: prune (age, then size cap oldest-first)

**Files:**
- Modify: `src/panel/thumb_cache.rs`

**Step 1: Failing tests** (`now` is a parameter — no `set_var`, no sleeps;
mtimes are set with `File::set_modified`)

```rust
use std::time::{Duration, SystemTime};

fn set_mtime(path: &Path, t: SystemTime) {
    std::fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

#[test]
fn prune_deletes_by_age() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let old = dir.path().join("old.jpg");
    let fresh = dir.path().join("fresh.jpg");
    std::fs::write(&old, [0u8; 10]).unwrap();
    std::fs::write(&fresh, [0u8; 10]).unwrap();
    set_mtime(&old, now - Duration::from_secs(31 * 24 * 3600));
    prune_dir(dir.path(), MAX_AGE, u64::MAX, now);
    assert!(!old.exists() && fresh.exists());
}

#[test]
fn prune_enforces_size_cap_oldest_first() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age_days) in [("a.jpg", 3), ("b.jpg", 2), ("c.jpg", 1)] {
        let p = dir.path().join(name);
        std::fs::write(&p, [0u8; 1000]).unwrap();
        set_mtime(&p, now - Duration::from_secs(age_days * 24 * 3600));
    }
    prune_dir(dir.path(), MAX_AGE, 2500, now); // fits two entries
    assert!(!dir.path().join("a.jpg").exists(), "oldest deleted first");
    assert!(dir.path().join("b.jpg").exists() && dir.path().join("c.jpg").exists());
}
```

**Step 2:** `cargo test prune` — FAIL.

**Step 3: Implement**

```rust
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Delete entries older than `max_age`, then oldest-first until the
/// directory is under `max_total_bytes`. Also reaps orphaned `.part`
/// files (they age out like everything else). Races with other
/// instances are benign — all errors are ignored.
fn prune_dir(dir: &Path, max_age: Duration, max_total_bytes: u64, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, SystemTime, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file()
                .then(|| (e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.retain(|(path, mtime, _)| {
        let expired = now.duration_since(*mtime).map_or(false, |age| age > max_age);
        if expired {
            let _ = std::fs::remove_file(path);
        }
        !expired
    });
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    files.sort_by_key(|(_, mtime, _)| *mtime);
    for (path, _, len) in files {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}
```

**Step 4:** `cargo test prune` — PASS.
**Step 5:** Commit: `feat(thumb-cache): startup prune (age + size cap)`

---

### Task 6: public API, config flag, main.rs wiring

**Files:**
- Modify: `src/panel/thumb_cache.rs` (statics + wrappers)
- Modify: `src/config.rs:25-36` (`GeneralConfig`)
- Modify: `src/main.rs` (~line 168 locals, ~line 181 config copy, init after
  the config block)
- Modify: `examples/config.toml` (`[general]` section)

**Step 1:** No new unit test — the wrappers are the untestable global
glue (OnceCell is process-wide); everything under them is covered by
Tasks 3–5. Add to thumb_cache.rs:

```rust
use crate::util::xdg_cache_home;
use once_cell::sync::OnceCell;

/// `Some(dir)` when the persistent cache is enabled and usable,
/// `None` when disabled by config or unavailable. Set once at startup.
static THUMB_CACHE_DIR: OnceCell<Option<PathBuf>> = OnceCell::new();

/// Resolve and create the cache dir. Failures disable persistence for
/// the session (warn once) — never fatal.
pub fn init(enabled: bool) {
    let dir = if enabled { resolve_dir() } else { None };
    let _ = THUMB_CACHE_DIR.set(dir);
}

fn resolve_dir() -> Option<PathBuf> {
    let dir = match xdg_cache_home() {
        Ok(cache) => cache.join("rfm").join("thumbnails"),
        Err(e) => {
            log::warn!("thumbnail cache disabled: {e}");
            return None;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("thumbnail cache disabled: cannot create {}: {e}", dir.display());
        return None;
    }
    Some(dir)
}

/// The active cache directory, if persistence is enabled.
pub fn dir() -> Option<&'static Path> {
    THUMB_CACHE_DIR.get().and_then(|d| d.as_deref())
}

pub fn lookup(src_path: &Path, mtime_secs: u64) -> Option<DynamicImage> {
    lookup_in(dir()?, src_path, mtime_secs)
}

/// Best-effort: a failed store only costs a future regeneration.
pub fn store(src_path: &Path, mtime_secs: u64, img: &DynamicImage) {
    if let Some(dir) = dir() {
        if let Err(e) = store_in(dir, src_path, mtime_secs, img) {
            log::debug!("thumbnail store failed for {}: {e}", src_path.display());
        }
    }
}

/// Run from `spawn_blocking` at startup; off the hot path.
pub fn prune() {
    if let Some(dir) = dir() {
        prune_dir(dir, MAX_AGE, MAX_TOTAL_BYTES, SystemTime::now());
    }
}
```

**Step 2:** config.rs — add to `GeneralConfig` after `use_trash`
(reuses the existing `default_true`):

```rust
/// Persist image/video preview thumbnails in $XDG_CACHE_HOME/rfm so
/// they survive restarts. Defaults to `true`.
#[serde(default = "default_true")]
pub preview_cache: bool,
```

**Step 3:** main.rs — follow the `use_trash` pattern exactly:
`let mut preview_cache = true;` next to the other locals (~line 168);
`preview_cache = config.general.preview_cache;` inside the `Ok(config)`
arm; and directly after the whole config if/else block:

```rust
panel::thumb_cache::init(preview_cache);
tokio::task::spawn_blocking(panel::thumb_cache::prune);
```

**Step 4:** examples/config.toml — document the key in `[general]`,
matching the style of the `use_trash` entry:

```toml
# Persist image/video preview thumbnails in $XDG_CACHE_HOME/rfm (usually
# ~/.cache/rfm) so they survive restarts. Set to false to keep previews
# in-memory only (no thumbnails of your files are written to disk).
# "rm -rf ~/.cache/rfm" is always safe.
# preview_cache = true
```

**Step 5:** `cargo build && cargo test && cargo clippy --all-targets` —
clean. Quick smoke: `XDG_CACHE_HOME=$(mktemp -d) ./target/debug/rfm --debug-socket /tmp/rfm.sock` in tmux, confirm `<tmpdir>/rfm/thumbnails/` exists, teardown.
**Step 6:** Commit: `feat(thumb-cache): config flag and startup wiring`

---

### Task 7: image previews through the cache

**Files:**
- Modify: `src/panel/preview.rs` (`FilePreview::new` ~line 225,
  `image_preview` ~line 305, `video_preview` ~line 344)

**Step 1:** Extract the mtime conversion already inlined in
`video_preview` (lines 344–347) into a helper both paths share:

```rust
fn mtime_secs(modified: SystemTime) -> u64 {
    modified
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .unwrap_or_default()
}
```

**Step 2:** Add the caching front-end (raw `image_preview` stays as-is —
the video path uses it to decode cache files, which must not re-enter
the cache):

```rust
/// Image preview via the persistent thumbnail cache: on a hit only the
/// small cached JPEG is decoded; on a miss the original is decoded,
/// thumbnailed, and stored for next time.
fn cached_image_preview(path: &Path, modified: SystemTime, info: Vec<String>) -> Preview {
    let mtime = mtime_secs(modified);
    if let Some(img) = thumb_cache::lookup(path, mtime) {
        log::debug!("thumbnail cache hit for {}", path.display());
        return Preview::Image { img: Some(img), info };
    }
    let preview = image_preview(path, info);
    if let Preview::Image { img: Some(img), .. } = &preview {
        thumb_cache::store(path, mtime, img);
    }
    preview
}
```

In `FilePreview::new` change the image arm to:

```rust
("image", _) => cached_image_preview(&path, modified, mediainfo(&path).unwrap_or_default()),
```

and simplify `video_preview`'s inline conversion to
`let modified = mtime_secs(modified);`. Import
`use super::thumb_cache;` (adjust to actual module path).

**Step 3:** `cargo test && cargo clippy --all-targets` — clean.

**Step 4: Verify live** (tmux + debug socket, per CLAUDE.md; needs ffmpeg
for fixture generation):

```bash
FIXTURE=$(mktemp -d); CACHE=$(mktemp -d)
mkdir "$FIXTURE/dir with spaces"
ffmpeg -f lavfi -i color=red:size=64x64 -frames:v 1 "$FIXTURE/dir with spaces/red.png"
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test "XDG_CACHE_HOME=$CACHE ./target/debug/rfm --debug-socket /tmp/rfm.sock $FIXTURE" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done
# navigate: select "dir with spaces", enter, select red.png → preview decodes
tmux send-keys -t rfm-test l
echo await-idle | socat - UNIX-CONNECT:/tmp/rfm.sock
ls "$CACHE/rfm/thumbnails/"           # expect one <16hex>-<mtime>.jpg
# restart rfm, same steps, then:
echo log | socat - UNIX-CONNECT:/tmp/rfm.sock | grep "cache hit"   # expect hit
tmux kill-session -t rfm-test; rm -rf "$FIXTURE" "$CACHE" /tmp/rfm.sock
```

**Step 5:** Commit: `feat(preview): image previews through the persistent thumbnail cache`

---

### Task 8: video thumbnails to the cache dir, atomically

**Files:**
- Modify: `src/panel/preview.rs` (`ffmpeg_thumbnail`, ~line 365)

**Step 1: Rewrite `ffmpeg_thumbnail`** — cache dir instead of
`temp_dir()` (which stays as the fallback when persistence is off, so
behavior without the cache matches today), `.part` + rename, and check
ffmpeg's exit status (today it's ignored; a failure should fall back to
mediainfo via the existing `Err` path in `video_preview`):

```rust
fn ffmpeg_thumbnail(path: impl AsRef<Path>, modified: u64) -> anyhow::Result<Preview> {
    static FALLBACK_DIR: OnceCell<PathBuf> = OnceCell::new();
    let dir = thumb_cache::dir()
        .unwrap_or_else(|| FALLBACK_DIR.get_or_init(temp_dir).as_path());
    let name = thumb_cache::entry_name(path.as_ref(), modified);
    let thumbnail = dir.join(&name);
    if thumbnail.exists() {
        log::debug!("thumbnail cache hit for {}", path.as_ref().display());
        return Ok(image_preview(thumbnail, mediainfo(path).unwrap_or_default()));
    }
    log::debug!("generating thumbnail {}", thumbnail.display());
    // ffmpeg infers the container from the extension — keep `.jpg` last.
    let part = dir.join(format!("{name}.{}.part.jpg", std::process::id()));
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.arg("-ss")
        .arg("00:00:10")
        .arg("-y")
        .arg("-i")
        .arg(path.as_ref())
        .arg("-vframes")
        .arg("1")
        .arg("-q:v")
        .arg("2")
        .arg("-vf")
        .arg("scale=120:-1")
        .arg(&part);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let status = cmd.spawn()?.wait()?;
    if !status.success() || !part.exists() {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("ffmpeg exited with {status}");
    }
    std::fs::rename(&part, &thumbnail)?;
    thumb_cache::cleanup_stale(dir, path.as_ref(), &name);
    Ok(image_preview(thumbnail, mediainfo(path).unwrap_or_default()))
}
```

Delete the old `THUMBNAIL_DIR` static. `cleanup_stale` and `entry_name`
are already `pub(crate)`; `thumb_cache` may need re-export or path
adjustment (`crate::panel::thumb_cache`).

**Step 2:** `cargo test && cargo clippy --all-targets` — clean.

**Step 3: Verify live** — as Task 7's script but with a video fixture:
`ffmpeg -f lavfi -i testsrc=duration=12:size=320x240:rate=10 "$FIXTURE/clip.mp4"`.
Preview it; expect exactly one `<16hex>-<mtime>.jpg` in the cache dir,
no `.part` leftovers; restart → `log` shows the cache-hit line. Then
`touch "$FIXTURE/clip.mp4"`, re-preview, confirm the old-mtime sibling
was replaced (still exactly one file for that prefix).

**Step 4:** Commit: `feat(preview): video thumbnails in the persistent cache, atomic writes`

---

### Task 9: docs

**Files:**
- Modify: `docs/configuration.md` (document `preview_cache` under
  `[general]`, matching the existing entries' style)
- Modify: `CLAUDE.md` (short "Architecture: thumbnail cache" section:
  location, filename-is-metadata scheme, atomic `.part`+rename,
  prune constants, `preview_cache` flag, and the test recipe of
  pointing `XDG_CACHE_HOME` at a scratch dir in the tmux pane)

**Steps:** write both, `cargo test` one last time, commit:
`docs: persistent thumbnail cache`.

---

## Verification checklist (end state)

- [ ] `cargo test` and `cargo clippy --all-targets` clean
- [ ] Image + video previews create cache entries; restart hits them
      (debug-socket `log` shows "thumbnail cache hit")
- [ ] No `.part` files remain after normal operation
- [ ] `touch`ing a source file replaces its entry (one file per prefix)
- [ ] `preview_cache = false` → no cache dir writes at all
- [ ] Fixture paths with spaces work throughout
