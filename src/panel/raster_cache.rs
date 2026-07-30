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

// TODO(raster-cache wiring): remove the allow once the producers and
// main.rs init consume this module.
#![allow(dead_code)]

use image::{codecs::jpeg::JpegEncoder, DynamicImage};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
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
    format!("{:016x}-", seahash::hash(path.as_os_str().as_encoded_bytes()))
}

/// `<hash>-<mtime>-` — the *keep scope* of the stale-sibling sweep:
/// all still-valid rasters of this path at this mtime, any kind.
fn keep_prefix(path: &Path, mtime_secs: u64) -> String {
    format!("{}{mtime_secs}-", hash_prefix(path))
}

/// Same-directory temp name for the atomic write: `<final>.<pid>.part`.
/// The pid keeps concurrent rfm instances from clobbering each other's
/// in-flight writes; last rename wins, both wrote equivalent content.
fn part_name(final_name: &str) -> String {
    format!("{final_name}.{}.part", std::process::id())
}

const JPEG_QUALITY: u8 = 85;

/// Look a raster up in `dir`. Open failure (no entry) is a plain miss; a
/// successful open with a failed decode is a corrupt entry — delete it and
/// miss (design §error handling). Never deletes on mere absence.
fn lookup_in(dir: &Path, src: &Path, mtime_secs: u64, kind: &str) -> Option<DynamicImage> {
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
fn store_in(
    dir: &Path,
    src: &Path,
    mtime_secs: u64,
    kind: &str,
    img: &DynamicImage,
) -> anyhow::Result<()> {
    let name = entry_name(src, mtime_secs, kind);
    let entry = dir.join(&name);
    if entry.exists() {
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
    std::fs::rename(&part, &entry)?;
    cleanup_stale(dir, src, mtime_secs);
    Ok(())
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
        assert_eq!(dir_files(dir.path()), vec![entry_name(src, 100, KIND_IMAGE)]);
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
