//! Persistent thumbnail cache in $XDG_CACHE_HOME/rfm/thumbnails.
//!
//! The filename is the entire metadata:
//! `<seahash(abs path):016x>-<mtime_secs>.jpg`. Writes are atomic
//! (same-dir `.part` + rename); every store cleans stale siblings of the
//! same path-hash. See docs/plans/2026-07-30-thumbnail-cache-design.md.
use std::path::Path;

/// Cache filename for `path` at `mtime_secs`. Fixed-width hex hash +
/// `-` separator keeps entries for one source file prefix-scannable.
// TODO(thumbnail-cache Task 3): consumed by `store`/`lookup`.
#[allow(dead_code)]
pub(crate) fn entry_name(path: &Path, mtime_secs: u64) -> String {
    format!(
        "{:016x}-{mtime_secs}.jpg",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

// TODO(thumbnail-cache Task 4): consumed by the stale-sibling cleanup in `store`.
#[allow(dead_code)]
fn hash_prefix(path: &Path) -> String {
    format!(
        "{:016x}-",
        seahash::hash(path.as_os_str().as_encoded_bytes())
    )
}

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
        assert_ne!(
            entry_name(Path::new("/other.png"), 1700000000)[..17],
            name[..17]
        );
    }
}
