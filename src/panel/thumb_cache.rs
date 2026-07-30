//! Persistent thumbnail cache in $XDG_CACHE_HOME/rfm/thumbnails.
//!
//! The filename is the entire metadata:
//! `<seahash(abs path):016x>-<mtime_secs>.jpg`. Writes are atomic
//! (same-dir `.part` + rename); every store cleans stale siblings of the
//! same path-hash. See docs/plans/2026-07-30-thumbnail-cache-design.md.
use image::codecs::jpeg::JpegEncoder;
use image::DynamicImage;
use std::io::{BufWriter, Write};
use std::path::Path;

const JPEG_QUALITY: u8 = 85;

/// Cache filename for `path` at `mtime_secs`. Fixed-width hex hash +
/// `-` separator keeps entries for one source file prefix-scannable.
pub(crate) fn entry_name(path: &Path, mtime_secs: u64) -> String {
    format!("{}{mtime_secs}.jpg", hash_prefix(path))
}

// TODO(thumbnail-cache Task 4): also consumed by the stale-sibling cleanup in `store`.
fn hash_prefix(path: &Path) -> String {
    format!(
        "{:016x}-",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

/// Decode the cache entry for (`src_path`, `mtime_secs`), if present.
/// A corrupt entry is deleted and treated as a miss.
// TODO(thumbnail-cache Task 6): consumed by the public cache-dir wrappers.
#[allow(dead_code)]
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
// TODO(thumbnail-cache Task 6): consumed by the public cache-dir wrappers.
#[allow(dead_code)]
fn store_in(
    dir: &Path,
    src_path: &Path,
    mtime_secs: u64,
    img: &DynamicImage,
) -> anyhow::Result<()> {
    let name = entry_name(src_path, mtime_secs);
    let final_path = dir.join(&name);
    if final_path.exists() {
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
    std::fs::rename(&part, &final_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::DynamicImage;
    use std::path::Path;

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
