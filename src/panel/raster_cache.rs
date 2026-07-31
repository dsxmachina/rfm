//! Persistent preview raster cache in `$XDG_CACHE_HOME/rfm/thumbnails/`.
//!
//! Design docs:
//! - `docs/plans/2026-07-30-thumbnail-cache-design.md` (base)
//! - `docs/plans/2026-07-30-preview-raster-cache-extension-design.md`
//!   (the `kind` producer/params discriminator)
//!
//! The filename is the entire metadata:
//! `<seahash(abs path):016x>-<mtime_secs>-<kind>.jpg`. Every operation is
//! best-effort — a preview is never lost to a cache fault, only recomputed.

use image::{codecs::jpeg::JpegEncoder, DynamicImage};
use once_cell::sync::OnceCell;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

/// Kind tag for image thumbnails (bounded to 960×540 by the producer).
pub(crate) const KIND_IMAGE: &str = "img960";
/// Kind tag for ffmpeg video frames (`scale=120:-1`).
pub(crate) const KIND_VIDEO: &str = "vid120";

/// `<seahash(abs path):016x>-<mtime_secs>-<kind>.jpg` — the filename is
/// the entire metadata. `kind` must be filename-safe (`[a-z0-9-]`).
pub(crate) fn entry_name(path: &Path, mtime_secs: u64, kind: &str) -> String {
    format!(
        "{:016x}-{mtime_secs}-{kind}.jpg",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

/// `<hash>-` — everything ever cached for this source path.
fn hash_prefix(path: &Path) -> String {
    format!(
        "{:016x}-",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

/// `<hash>-<mtime>-` — the *keep scope* of the stale-sibling sweep:
/// all still-valid rasters of this path at this mtime, any kind.
fn keep_prefix(path: &Path, mtime_secs: u64) -> String {
    format!("{}{mtime_secs}-", hash_prefix(path))
}

/// Process-unique token for same-directory temp names: `<pid>-<seq>`.
/// The pid keeps concurrent rfm *instances* from clobbering each other's
/// in-flight writes; the per-process counter keeps concurrent stores
/// *within* one instance apart (the directory preloader and the
/// on-demand preview task can build the same entry at the same time).
/// Last rename wins, all writers wrote equivalent content.
pub(crate) fn part_token() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// Same-directory temp name for the atomic write: `<final>.<token>.part`.
fn part_name(final_name: &str) -> String {
    format!("{final_name}.{}.part", part_token())
}

const JPEG_QUALITY: u8 = 85;

/// Look a raster up in `dir`. Open failure (no entry) is a plain miss; a
/// successful open with a failed decode is a corrupt entry — delete it and
/// miss (design §error handling). Never deletes on mere absence.
pub(crate) fn lookup_in(
    dir: &Path,
    src: &Path,
    mtime_secs: u64,
    kind: &str,
) -> Option<DynamicImage> {
    let entry = dir.join(entry_name(src, mtime_secs, kind));
    match image::io::Reader::open(&entry).ok()?.decode() {
        Ok(img) => Some(img),
        Err(e) => {
            log::debug!("removing corrupt cache entry {}: {e}", entry.display());
            let _ = std::fs::remove_file(&entry);
            None
        }
    }
}

/// Store a raster in `dir` as JPEG (quality 85), atomically: write to a
/// same-dir `.part` temp name, then rename to the final name. The final
/// name never exists with partial content.
pub(crate) fn store_in(
    dir: &Path,
    src: &Path,
    mtime_secs: u64,
    kind: &str,
    img: &DynamicImage,
) -> anyhow::Result<()> {
    let name = entry_name(src, mtime_secs, kind);
    let entry = dir.join(&name);
    if entry.is_file() {
        // Still sweep stale siblings a previously interrupted run left
        // (ported from the thumb_cache implementation's skip path).
        cleanup_stale(dir, src, mtime_secs);
        return Ok(());
    }
    let part = dir.join(part_name(&name));
    let write = || -> anyhow::Result<()> {
        let mut out = BufWriter::new(File::create(&part)?);
        // to_rgb8() is load-bearing: JPEG in image 0.24 rejects RGBA input.
        JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY).encode_image(&img.to_rgb8())?;
        out.flush()?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&part, &entry) {
        // A failed rename must not leave the .part lingering until the
        // 30-day prune (its current-mtime name sits in the keep scope).
        let _ = std::fs::remove_file(&part);
        return Err(e.into());
    }
    cleanup_stale(dir, src, mtime_secs);
    Ok(())
}

/// The session-wide cache state, set once by [`init`] at startup:
/// `(config_enabled, resolved dir)`. The dir is `None` when persistence
/// is off — by config, or because the dir could not be resolved/created
/// (the flag tells those apart, see [`persistence_enabled`]). Unset
/// (e.g. in unit tests) behaves as disabled.
static RASTER_CACHE: OnceCell<(bool, Option<PathBuf>)> = OnceCell::new();

/// `create_dir_all` with mode 0700 on every directory it creates (the
/// XDG basedir spec's required mode for missing base directories):
/// cached thumbnails of the user's media must not become world-readable
/// just because rfm had to create the chain itself.
pub(crate) fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Resolve-and-create core of [`init`]: `<cache_home>/rfm/thumbnails`.
/// Any failure disables persistence for the session (warn once, never
/// fatal — design §error handling).
fn resolve_dir(enabled: bool, cache_home: anyhow::Result<PathBuf>) -> Option<PathBuf> {
    if !enabled {
        return None;
    }
    let dir = match cache_home {
        Ok(home) => home.join("rfm").join("thumbnails"),
        Err(e) => {
            log::warn!("preview raster cache disabled: {e}");
            return None;
        }
    };
    if let Err(e) = create_dir_all_private(&dir) {
        log::warn!(
            "preview raster cache disabled: cannot create {}: {e}",
            dir.display()
        );
        return None;
    }
    Some(dir)
}

/// Initialize the persistent cache once at startup. `enabled` comes from
/// the `preview_cache` config switch; resolution/creation failure runs the
/// session with persistence off.
pub fn init(enabled: bool) {
    let _ = RASTER_CACHE.set((enabled, resolve_dir(enabled, crate::util::xdg_cache_home())));
}

/// The cache directory, or `None` when persistence is disabled/unavailable.
pub fn dir() -> Option<&'static Path> {
    RASTER_CACHE.get()?.1.as_deref()
}

/// `false` iff the user opted out via `preview_cache = false` (or the
/// cache was never initialized, as in unit tests). Distinct from
/// [`dir`] returning `None`: enabled-but-unavailable still allows the
/// video producer's temp-dir fallback, while an explicit opt-out
/// promises that nothing about the user's files is written to disk.
pub fn persistence_enabled() -> bool {
    RASTER_CACHE.get().is_some_and(|(enabled, _)| *enabled)
}

/// Startup eviction for the session cache dir; intended for a
/// fire-and-forget `spawn_blocking` at startup. No-op when disabled.
pub fn prune() {
    if let Some(dir) = dir() {
        prune_dir(dir, MAX_AGE, MAX_TOTAL_BYTES, std::time::SystemTime::now());
    }
}

/// Startup prune: entries untouched for this long are dropped.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 3600);
/// Startup prune: after the age pass, evict oldest-first down to this
/// shared budget (all kinds, one budget — extension design).
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

/// Evict cache entries: first everything older than `max_age`, then the
/// oldest survivors until the directory fits `max_total_bytes`. All errors
/// ignored — concurrent-instance races are benign, and orphaned `.part`
/// files are regular files that age out like everything else.
fn prune_dir(
    dir: &Path,
    max_age: std::time::Duration,
    max_total_bytes: u64,
    now: std::time::SystemTime,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(now);
        files.push((entry.path(), mtime, meta.len()));
    }
    // Age pass
    files.retain(|(path, mtime, _)| {
        let expired = now
            .duration_since(*mtime)
            .map(|age| age > max_age)
            .unwrap_or(false);
        if expired {
            let _ = std::fs::remove_file(path);
        }
        !expired
    });
    // Size pass: oldest-first while over the cap, subtracting only on
    // successful removal.
    let mut total: u64 = files.iter().map(|(_, _, len)| len).sum();
    files.sort_by_key(|(_, mtime, _)| *mtime);
    for (path, _, len) in &files {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(path).is_ok() {
            total = total.saturating_sub(*len);
        }
    }
}

/// Delete every entry of `src`'s hash whose name is NOT in the keep scope
/// `<hash>-<mtime>-` (i.e. all rasters of older/newer mtimes, any kind,
/// plus their orphaned .part files). Same-mtime entries of other kinds are
/// valid and survive. Errors ignored — a racing instance may have removed
/// the file already.
pub(crate) fn cleanup_stale(dir: &Path, src: &Path, mtime_secs: u64) {
    let hash = hash_prefix(src);
    let keep = keep_prefix(src, mtime_secs);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(&hash) && !name.starts_with(&keep) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;
    use std::path::Path;

    /// 4×4 solid Rgb8 fixture.
    fn test_img() -> DynamicImage {
        DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([200, 30, 30])))
    }

    fn dir_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn store_then_lookup_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/pic.png");
        // miss before store
        assert!(lookup_in(dir.path(), src, 100, KIND_IMAGE).is_none());
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        // hit after, with correct dimensions
        let hit = lookup_in(dir.path(), src, 100, KIND_IMAGE).unwrap();
        assert_eq!((hit.width(), hit.height()), (4, 4));
        // wrong mtime misses; wrong kind misses
        assert!(lookup_in(dir.path(), src, 101, KIND_IMAGE).is_none());
        assert!(lookup_in(dir.path(), src, 100, KIND_VIDEO).is_none());
        // directory contains ONLY the final name — no .part leftovers
        assert_eq!(
            dir_files(dir.path()),
            vec![entry_name(src, 100, KIND_IMAGE)]
        );
    }

    #[test]
    fn same_path_and_mtime_with_two_kinds_coexist() {
        // extension design "key discriminator" test
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/clip.mp4");
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        store_in(dir.path(), src, 100, KIND_VIDEO, &test_img()).unwrap();
        assert!(lookup_in(dir.path(), src, 100, KIND_IMAGE).is_some());
        assert!(lookup_in(dir.path(), src, 100, KIND_VIDEO).is_some());
        assert_eq!(dir_files(dir.path()).len(), 2);
    }

    #[test]
    fn corrupt_entry_is_deleted_and_misses() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/pic.png");
        let entry = dir.path().join(entry_name(src, 100, KIND_IMAGE));
        std::fs::write(&entry, b"not a jpeg").unwrap();
        assert!(lookup_in(dir.path(), src, 100, KIND_IMAGE).is_none());
        assert!(!entry.exists(), "corrupt entry must be deleted");
    }

    #[test]
    fn missing_entry_is_a_plain_miss() {
        let dir = tempfile::tempdir().unwrap();
        assert!(lookup_in(dir.path(), Path::new("/nope.png"), 100, KIND_IMAGE).is_none());
        assert!(dir_files(dir.path()).is_empty(), "no delete-attempt noise");
    }

    #[test]
    fn store_removes_other_mtime_siblings_of_any_kind() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/pic.png");
        let other = Path::new("/other/pic.png");
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        store_in(dir.path(), src, 100, KIND_VIDEO, &test_img()).unwrap();
        store_in(dir.path(), other, 100, KIND_IMAGE, &test_img()).unwrap();
        // an orphaned .part of an even older mtime
        let orphan = dir
            .path()
            .join(format!("{}.999.part", entry_name(src, 90, KIND_IMAGE)));
        std::fs::write(&orphan, b"partial").unwrap();

        store_in(dir.path(), src, 200, KIND_IMAGE, &test_img()).unwrap();

        // the fresh entry hits
        assert!(lookup_in(dir.path(), src, 200, KIND_IMAGE).is_some());
        // BOTH (src,100,*) entries and the old-mtime .part are gone
        assert!(lookup_in(dir.path(), src, 100, KIND_IMAGE).is_none());
        assert!(lookup_in(dir.path(), src, 100, KIND_VIDEO).is_none());
        assert!(!orphan.exists());
        // (other,100,img960) untouched
        assert!(lookup_in(dir.path(), other, 100, KIND_IMAGE).is_some());
        assert_eq!(dir_files(dir.path()).len(), 2);
    }

    #[test]
    fn store_keeps_same_mtime_entries_of_other_kinds() {
        // The extension design's warning case (widened-glob regression):
        // same-mtime siblings of other kinds are NEVER swept.
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/clip.mp4");
        store_in(dir.path(), src, 100, KIND_VIDEO, &test_img()).unwrap();
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        assert!(lookup_in(dir.path(), src, 100, KIND_VIDEO).is_some());
        assert!(lookup_in(dir.path(), src, 100, KIND_IMAGE).is_some());
        assert_eq!(dir_files(dir.path()).len(), 2);
    }

    #[test]
    fn store_never_rewrites_an_existing_entry() {
        // Ported from the superseded thumb_cache tests: an existing final
        // entry is never rewritten, and the skip path still sweeps stale
        // siblings a previously interrupted run left behind.
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/photos/a.png");
        let entry = dir.path().join(entry_name(src, 100, KIND_IMAGE));
        let stale = dir.path().join(entry_name(src, 90, KIND_IMAGE));
        std::fs::write(&entry, b"sentinel").unwrap();
        std::fs::write(&stale, b"stale").unwrap();
        store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).unwrap();
        assert_eq!(std::fs::read(&entry).unwrap(), b"sentinel");
        assert!(!stale.exists(), "skip path still cleans stale siblings");
    }

    #[test]
    fn resolve_dir_disabled_or_unavailable_is_none() {
        let base = tempfile::tempdir().unwrap();
        // config switch off → disabled, and nothing is created
        assert!(resolve_dir(false, Ok(base.path().to_path_buf())).is_none());
        assert!(!base.path().join("rfm").exists());
        // no resolvable cache home → disabled for the session
        assert!(resolve_dir(true, Err(anyhow::anyhow!("no HOME"))).is_none());
        // cache home resolves but the dir cannot be created (a file is in
        // the way) → disabled for the session, no panic
        let blocked = base.path().join("blocked");
        std::fs::write(&blocked, b"file, not dir").unwrap();
        assert!(resolve_dir(true, Ok(blocked)).is_none());
    }

    #[test]
    fn resolve_dir_enabled_creates_and_returns_the_cache_dir() {
        let base = tempfile::tempdir().unwrap();
        let dir = resolve_dir(true, Ok(base.path().to_path_buf())).unwrap();
        assert_eq!(dir, base.path().join("rfm").join("thumbnails"));
        assert!(dir.is_dir());
    }

    #[test]
    fn part_names_are_unique_per_call() {
        // Two tasks INSIDE one process (directory preloader + on-demand
        // preview) can store the same entry concurrently; a pid-only
        // discriminator would make them share one temp file.
        assert_ne!(part_name("x.jpg"), part_name("x.jpg"));
    }

    #[test]
    fn store_removes_its_part_when_the_final_rename_fails() {
        // A directory squatting on the final name makes the rename fail;
        // the .part must not linger until the 30-day prune.
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new("/some/pic.png");
        let entry = dir.path().join(entry_name(src, 100, KIND_IMAGE));
        std::fs::create_dir(&entry).unwrap();
        assert!(store_in(dir.path(), src, 100, KIND_IMAGE, &test_img()).is_err());
        assert!(
            !dir_files(dir.path()).iter().any(|n| n.ends_with(".part")),
            "no .part litter after a failed rename"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_dir_creates_the_chain_with_mode_0700() {
        // XDG basedir spec: missing base dirs are created 0700 — cached
        // thumbnails of the user's media must not be world-readable.
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("cache-home");
        let dir = resolve_dir(true, Ok(home.clone())).unwrap();
        for d in [home.as_path(), &home.join("rfm"), dir.as_path()] {
            let mode = d.metadata().unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{} must be 0700", d.display());
        }
    }

    #[test]
    fn uninitialized_cache_counts_as_opted_out() {
        // Unit tests never call init(): both accessors must behave as
        // "persistence off" (the privacy-safe default).
        assert!(dir().is_none());
        assert!(!persistence_enabled());
    }

    #[test]
    fn store_writes_via_part_temp_name_in_same_dir() {
        // Atomicity contract: the temp name lives in the SAME directory
        // (rename must not cross filesystems) and never collides with the
        // final-name shape, so a crashed write can't leave a decodable-
        // looking partial under the final name.
        let src = Path::new("/some/pic.png");
        let name = entry_name(src, 100, KIND_IMAGE);
        let part = part_name(&name);
        assert!(part.starts_with(&name), "part name derives from final name");
        assert!(part.ends_with(".part"), "part suffix distinguishes temp");
        assert_ne!(part, name);
        assert!(!part.contains('/'), "same dir: a bare file name");
    }

    #[test]
    fn store_into_missing_dir_errors_without_final_file() {
        // best-effort: an unwritable cache dir yields Err, never a panic
        // and never a partial final entry.
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("missing-subdir");
        let src = Path::new("/some/pic.png");
        assert!(store_in(&gone, src, 100, KIND_IMAGE, &test_img()).is_err());
        assert!(!gone.exists());
    }

    /// Backdate `path`'s mtime to `now - age` (no `touch` shell-out).
    fn backdate(path: &Path, now: std::time::SystemTime, age: std::time::Duration) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(now - age)
            .unwrap();
    }

    #[test]
    fn prune_deletes_by_age() {
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = dir
            .path()
            .join(entry_name(Path::new("/old.png"), 100, KIND_IMAGE));
        let fresh = dir
            .path()
            .join(entry_name(Path::new("/fresh.png"), 100, KIND_IMAGE));
        std::fs::write(&old, vec![0u8; 10]).unwrap();
        std::fs::write(&fresh, vec![0u8; 10]).unwrap();
        backdate(&old, now, Duration::from_secs(31 * 24 * 3600));
        // size cap at u64::MAX so only the age rule fires
        prune_dir(dir.path(), MAX_AGE, u64::MAX, now);
        assert!(!old.exists(), "31-day-old entry must be pruned");
        assert!(fresh.exists(), "fresh entry must survive");
    }

    #[test]
    fn prune_enforces_size_cap_oldest_first() {
        use std::time::{Duration, SystemTime};
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        // Mix kinds in the names: one shared budget across producers
        // (extension design requirement).
        let oldest = dir
            .path()
            .join(entry_name(Path::new("/a.mp4"), 100, KIND_VIDEO));
        let mid = dir
            .path()
            .join(entry_name(Path::new("/b.png"), 100, KIND_IMAGE));
        let newest = dir
            .path()
            .join(entry_name(Path::new("/c.png"), 100, KIND_IMAGE));
        for (path, days) in [(&oldest, 3u64), (&mid, 2), (&newest, 1)] {
            std::fs::write(path, vec![0u8; 1000]).unwrap();
            backdate(path, now, Duration::from_secs(days * 24 * 3600));
        }
        prune_dir(dir.path(), MAX_AGE, 2500, now);
        assert!(!oldest.exists(), "oldest must be evicted to meet the cap");
        assert!(mid.exists());
        assert!(newest.exists());
    }

    #[test]
    fn entry_name_is_hexhash_mtime_kind_jpg() {
        let name = entry_name(Path::new("/some/pic.png"), 1700000000, KIND_IMAGE);
        let (hash, rest) = name.split_at(16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(rest, "-1700000000-img960.jpg");
        // same path+kind, different mtime → same hash prefix, different name
        let other = entry_name(Path::new("/some/pic.png"), 1700000001, KIND_IMAGE);
        assert_eq!(other[..17], name[..17]);
        assert_ne!(other, name);
        // different path → different hash prefix
        assert_ne!(
            entry_name(Path::new("/other.png"), 1700000000, KIND_IMAGE)[..17],
            name[..17]
        );
    }

    #[test]
    fn entry_names_differ_by_kind_only() {
        // The extension's key discriminator: same source, same mtime, two
        // producers → two distinct entries.
        let img = entry_name(Path::new("/some/clip.mp4"), 1700000000, KIND_IMAGE);
        let vid = entry_name(Path::new("/some/clip.mp4"), 1700000000, KIND_VIDEO);
        assert_ne!(img, vid);
        assert_eq!(img[..28], vid[..28], "hash+mtime prefix shared");
    }
}
