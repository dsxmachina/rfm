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

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
