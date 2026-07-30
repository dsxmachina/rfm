//! Persistent thumbnail cache in $XDG_CACHE_HOME/rfm/thumbnails.
//!
//! The filename is the entire metadata:
//! `<seahash(abs path):016x>-<mtime_secs>.jpg`. Writes are atomic
//! (same-dir `.part` + rename); every store cleans stale siblings of the
//! same path-hash. See docs/plans/2026-07-30-thumbnail-cache-design.md.
use crate::util::xdg_cache_home;
use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use once_cell::sync::OnceCell;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const JPEG_QUALITY: u8 = 85;
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

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
        log::warn!(
            "thumbnail cache disabled: cannot create {}: {e}",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// The active cache directory, if persistence is enabled.
pub fn dir() -> Option<&'static Path> {
    THUMB_CACHE_DIR.get().and_then(|d| d.as_deref())
}

/// Decoded cache entry for (`src_path`, `mtime_secs`), or `None` on
/// miss or when the cache is disabled.
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

/// Cache filename for `path` at `mtime_secs`. Fixed-width hex hash +
/// `-` separator keeps entries for one source file prefix-scannable.
pub(crate) fn entry_name(path: &Path, mtime_secs: u64) -> String {
    format!("{}{mtime_secs}.jpg", hash_prefix(path))
}

fn hash_prefix(path: &Path) -> String {
    format!(
        "{:016x}-",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

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
/// `.part` file, then rename. Never leaves a partial final entry, and
/// never rewrites an existing one. Cleans stale siblings (old mtimes,
/// orphaned `.part`s) of the same source path after a successful store —
/// and likewise on the skip path when the entry already exists.
/// Same-process concurrent stores are safe: both write equivalent bytes
/// for the same (path, mtime), and any corrupt outcome self-heals via
/// `lookup_in`'s delete-on-decode-failure. Cross-instance races cost at
/// most a regeneration: one instance's entry or in-flight `.part` may be
/// deleted by another's cleanup, and the next miss simply re-stores.
fn store_in(
    dir: &Path,
    src_path: &Path,
    mtime_secs: u64,
    img: &DynamicImage,
) -> anyhow::Result<()> {
    let name = entry_name(src_path, mtime_secs);
    let final_path = dir.join(&name);
    if final_path.exists() {
        // Still sweep stale siblings a previously interrupted run left.
        cleanup_stale(dir, src_path, &name);
        return Ok(());
    }
    let part = dir.join(format!("{name}.{}.part", std::process::id()));
    let write = (|| -> anyhow::Result<()> {
        let mut w = BufWriter::new(std::fs::File::create(&part)?);
        JpegEncoder::new_with_quality(&mut w, JPEG_QUALITY).encode_image(&img.to_rgb8())?;
        w.flush()?;
        Ok(())
    })();
    if let Err(e) = write {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&part, &final_path) {
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    cleanup_stale(dir, src_path, &name);
    Ok(())
}

/// Remove every entry sharing `src_path`'s hash prefix except `keep`.
/// Also catches orphaned `.part` files of that prefix. Errors ignored:
/// a racing instance may have removed the file already. Called from
/// `store_in` and directly by the ffmpeg path (`ffmpeg_thumbnail`).
pub(crate) fn cleanup_stale(dir: &Path, src_path: &Path, keep: &str) {
    let prefix = hash_prefix(src_path);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&prefix) && name != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Delete entries older than `max_age`, then oldest-first until the
/// directory is under `max_total_bytes`. Also reaps orphaned `.part`
/// files (they age out like everything else). Races with other
/// instances are benign — all errors are ignored.
fn prune_dir(dir: &Path, max_age: Duration, max_total_bytes: u64, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, SystemTime, u64)> = entries
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((e.path(), meta.modified().ok()?, meta.len()))
        })
        .collect();
    files.retain(|(path, mtime, _)| {
        let expired = now.duration_since(*mtime).is_ok_and(|age| age > max_age);
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    fn set_mtime(path: &Path, t: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

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

    #[test]
    fn store_removes_stale_siblings_but_not_other_paths() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/photos/a.png");
        let other = Path::new("/photos/b.png");
        store_in(dir.path(), src, 100, &test_img()).unwrap();
        store_in(dir.path(), other, 100, &test_img()).unwrap();
        // orphaned .part from a dead process, same prefix
        std::fs::write(
            dir.path().join(format!("{}.999.part", entry_name(src, 90))),
            b"x",
        )
        .unwrap();
        store_in(dir.path(), src, 200, &test_img()).unwrap();
        assert!(lookup_in(dir.path(), src, 200).is_some());
        assert!(
            lookup_in(dir.path(), src, 100).is_none(),
            "stale sibling gone"
        );
        assert!(
            lookup_in(dir.path(), other, 100).is_some(),
            "other path untouched"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn store_never_rewrites_an_existing_entry() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/photos/a.png");
        let entry = dir.path().join(entry_name(src, 100));
        let stale = dir.path().join(entry_name(src, 90));
        std::fs::write(&entry, b"sentinel").unwrap();
        std::fs::write(&stale, b"stale").unwrap();
        store_in(dir.path(), src, 100, &test_img()).unwrap();
        assert_eq!(std::fs::read(&entry).unwrap(), b"sentinel");
        assert!(!stale.exists(), "skip path still cleans stale siblings");
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
        assert_ne!(
            entry_name(Path::new("/other.png"), 1700000000)[..17],
            name[..17]
        );
    }
}
