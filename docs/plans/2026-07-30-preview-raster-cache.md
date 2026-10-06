# Persistent Preview Raster Cache — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes.

**Goal:** Persist image/video preview rasters in
`$XDG_CACHE_HOME/rfm/thumbnails/` so they survive restarts, replacing the
`/tmp` scheme in `ffmpeg_thumbnail` — implementing the accepted base design
**and** its kind-discriminator extension in one branch.

**Design docs:**
- `docs/plans/2026-07-30-thumbnail-cache-design.md` (base, accepted)
- `docs/plans/2026-07-30-preview-raster-cache-extension-design.md` (extension)
- Supersedes the earlier `docs/plans/2026-07-30-thumbnail-cache.md` plan,
  which predates branch 1 (`feature/preview-1-native-backends`); every file
  and line reference below has been re-verified against the current tree.

**Architecture:** A new `src/panel/raster_cache.rs` module owns the cache:
filename = `<seahash(abs path):016x>-<mtime_secs>-<kind>.jpg` (the filename
is the entire metadata; `kind` is the producer/params discriminator from the
extension design), atomic writes via same-dir `.part` + rename, stale-sibling
cleanup on store scoped to *keep* everything matching `<hash>-<mtime>-*`,
startup prune (30 days / 256 MB) via `spawn_blocking`. Core functions take an
explicit `dir: &Path` (unit-testable with `tempfile`); thin public wrappers
consult a `OnceCell<Option<PathBuf>>` initialized from config
(`[general] preview_cache`, default true). `FilePreview::new`'s image arm
gains a caching front-end around `native_image_preview`; `ffmpeg_thumbnail`
retargets from `temp_dir()` to the cache dir.

**Tech stack:** existing deps only — `image` 0.24.9 (JPEG encode/decode),
`seahash` 4.1, `once_cell`, `tempfile` (dev). **No new crates.** MSRV 1.83
(`File::set_modified` needs 1.75 — OK).

Every task ends with: `cargo test && cargo clippy --all-targets` clean,
then commit (trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`).

---

## Resolved ambiguities (read before implementing)

1. **Kind parameter from the start.** Base and extension land together, so
   the cache API is `lookup(path, mtime, kind)` / `store(path, mtime, kind,
   img)` from day one — no two-arg API, no retrofit. Every requirement of
   *both* designs must still be individually satisfied (atomicity, JPEG q85,
   prune, config switch, error handling from the base; key discriminator and
   sweep scope from the extension). Kind constants for the two existing
   producers: `KIND_IMAGE = "img960"`, `KIND_VIDEO = "vid120"`.
2. **Module name is `raster_cache`, not `thumb_cache`.** The old plan named
   it `thumb_cache.rs`; with the extension it is a shared sink for all
   raster producers (SVG/PDF/font come later), matching the branch name.
   The on-disk directory stays `…/rfm/thumbnails/` exactly as both designs
   specify.
3. **Stale-sibling sweep scope** (the extension design's one code change to
   the base): on `store`, delete every entry whose name starts with
   `<hash>-` but does **not** start with `<hash>-<mtime>-`. Old-mtime
   entries of *any* kind die (the source changed, all derived rasters are
   invalid); same-mtime entries of *other* kinds survive (e.g. a future
   `pdf-p1`/`pdf-p2` pair, or the design's font-vs-image warning case).
   Consequence the extension design glosses over: a same-mtime kind bump
   (font `s1`→`s2`) is *not* swept — it ages out via the 30-day prune. No
   font producer exists in this branch, so nothing is lost here; noted for
   the follow-up branches.
4. **`main.rs` name collision.** `main.rs:281` already has
   `let preview_cache = PanelCache::with_size(4096);` (the in-memory panel
   cache). The config-derived boolean local is therefore named
   `persist_previews`; the config *key* stays `preview_cache` per the design.
5. **Branch-1 drift in the image arm.** `FilePreview::new` now dispatches
   `("image", _) => native_image_preview(&path, &mime)` (preview.rs:226) —
   info lines are derived natively from the decode, no mediainfo. The old
   plan's `cached_image_preview(path, modified, info)` shape is stale. The
   new front-end wraps `native_image_preview` and, on a cache hit, rebuilds
   the info lines *without* the full decode: original dimensions via the
   header-only `image::io::Reader::…::into_dimensions()`, byte size/mtime
   from `path.metadata()`, and the color token from the cached thumbnail
   (always `Rgb8` after the JPEG round-trip — accepted minor display drift;
   dimensions, the line users actually read, stay exact). This requires
   refactoring `image_info_lines` (preview.rs:301) to a
   `(width, height, color, byte_size, modified, subtype)` core.
6. **ffmpeg exit-status checking already exists.** Branch 1 rewrote
   `ffmpeg_thumbnail` (preview.rs:419) to check `out.status`, verify the
   file exists, remove partial files, and bail with the stderr tail. The old
   plan's "add status check" step is obsolete; Task 7 only retargets the
   directory, adopts the shared key, and makes the write atomic — the
   error handling is kept verbatim.
7. **Fallback when persistence is off/unavailable:** `ffmpeg_thumbnail`
   keeps today's behavior — `temp_dir()/rfm-thumbnails` with
   `create_dir_all` + the existing 7-day `prune_older_than` — so
   `preview_cache = false` matches current releases exactly. The image path
   simply skips lookup/store (full decode every time, as today).
8. **Extension `.jpg` is fixed** for both current kinds (both producers
   emit JPEG). The `<ext>` generality in the extension design's key scheme
   becomes relevant only for future producers; `entry_name` hardcodes
   `.jpg` and the follow-up branch generalizes it if ever needed.
9. **Miss vs corrupt in `lookup`:** open failure (no entry) is a plain
   miss; a successful open with a failed *decode* is corruption — delete
   the entry, log at debug, miss. Never delete on mere absence.
10. **`preview_cache = false` writes nothing** is verified end-to-end
    (Task 8), not as a unit test: the public wrappers guard on a
    process-global `OnceCell`, and unit tests share one process — a
    `init(false)` test would race every other test. All logic under the
    wrappers is covered dir-parameterized in Tasks 2–5.

---

### Task 1: `xdg_cache_home()` in util.rs

**Files:**
- Modify: `src/util.rs` (next to `xdg_config_home`, line 376)

**Step 1: Write the failing test** (in util.rs's test area; the pure `_from`
variant avoids racy `set_var` in parallel tests):

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

**Step 2:** `cargo test xdg_cache_home` — FAIL (not defined).

**Step 3: Implement** — `pub fn xdg_cache_home() -> anyhow::Result<PathBuf>`
reading `std::env::var_os("XDG_CACHE_HOME")` / `var_os("HOME")` and
delegating to the pure `fn xdg_cache_home_from(xdg: Option<OsString>,
home: Option<OsString>)`: `$XDG_CACHE_HOME`, else `$HOME/.cache`, else
`Err` (mirrors `xdg_config_home`'s error wording).

**Step 4:** `cargo test xdg_cache_home` — PASS.
**Stays green independently:** pure additive helper; nothing calls it yet.
**Step 5:** Commit: `feat(util): add xdg_cache_home helper`

---

### Task 2: raster_cache module — keyed entry naming with kind

**Files:**
- Create: `src/panel/raster_cache.rs`
- Modify: `src/panel/mod.rs` (add `pub mod raster_cache;` next to
  `mod preview;`, line 28 — `pub` because main.rs wires init/prune)

**Step 1: Failing tests** (bottom of the new file, `mod tests`):

```rust
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
    assert_ne!(entry_name(Path::new("/other.png"), 1700000000, KIND_IMAGE)[..17], name[..17]);
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
```

**Step 2:** `cargo test raster_cache` — FAIL.

**Step 3: Implement** — module header (doc comment pointing at both design
docs) plus:

```rust
pub(crate) const KIND_IMAGE: &str = "img960";
pub(crate) const KIND_VIDEO: &str = "vid120";

/// `<seahash(abs path):016x>-<mtime_secs>-<kind>.jpg` — the filename is
/// the entire metadata. `kind` must be filename-safe ([a-z0-9-]).
pub(crate) fn entry_name(path: &Path, mtime_secs: u64, kind: &str) -> String {
    format!("{:016x}-{mtime_secs}-{kind}.jpg",
        seahash::hash(path.as_os_str().as_encoded_bytes()))
}

/// `<hash>-` — everything ever cached for this source path.
fn hash_prefix(path: &Path) -> String { … }

/// `<hash>-<mtime>-` — the *keep scope* of the stale-sibling sweep:
/// all still-valid rasters of this path at this mtime, any kind.
fn keep_prefix(path: &Path, mtime_secs: u64) -> String { … }
```

**Step 4:** `cargo test raster_cache` — PASS.
**Stays green independently:** new module, no callers; `pub(crate)` items
avoid dead-code warnings via the tests (add `#[allow(dead_code)]` only if
clippy complains before Task 6 wires it — remove it there).
**Step 5:** Commit: `feat(raster-cache): kind-discriminated entry naming`

---

### Task 3: store/lookup round-trip, atomicity, corrupt entries, kind isolation

**Files:**
- Modify: `src/panel/raster_cache.rs`

**Step 1: Failing tests** (shared fixture
`fn test_img() -> DynamicImage` = 4×4 solid `Rgb8`):

```rust
#[test]
fn store_then_lookup_round_trips() {
    // miss before store; hit after with correct dimensions; wrong mtime
    // misses; wrong kind misses; directory contains ONLY the final name
    // (no .part leftovers).
}

#[test]
fn same_path_and_mtime_with_two_kinds_coexist() {
    // extension design "key discriminator" test: store KIND_IMAGE and
    // KIND_VIDEO for the same (path, mtime); both lookups hit; two files.
}

#[test]
fn corrupt_entry_is_deleted_and_misses() {
    // write b"not a jpeg" at entry_name(...); lookup_in → None; file gone.
}

#[test]
fn missing_entry_is_a_plain_miss() {
    // empty dir: lookup_in → None, dir still empty (no delete attempt noise).
}
```

**Step 2:** `cargo test raster_cache` — FAIL.

**Step 3: Implement** the dir-parameterized core:

```rust
const JPEG_QUALITY: u8 = 85;

fn lookup_in(dir: &Path, src: &Path, mtime_secs: u64, kind: &str) -> Option<DynamicImage> {
    let entry = dir.join(entry_name(src, mtime_secs, kind));
    match image::io::Reader::open(&entry).ok()?.decode() {
        Ok(img) => Some(img),
        Err(e) => {         // corrupt: delete, treat as miss (design §errors)
            log::debug!("removing corrupt cache entry {}: {e}", entry.display());
            let _ = std::fs::remove_file(&entry);
            None
        }
    }
}

fn store_in(dir: &Path, src: &Path, mtime_secs: u64, kind: &str,
            img: &DynamicImage) -> anyhow::Result<()> {
    // final-name-exists → Ok early. Else write JPEG q85 via
    // JpegEncoder::new_with_quality(BufWriter, 85).encode_image(&img.to_rgb8())
    // to dir.join(format!("{name}.{pid}.part")), remove the .part on any
    // write error, then fs::rename to the final name.
}
```

(`to_rgb8()` is load-bearing: JPEG in image 0.24 rejects RGBA input.)

**Step 4:** `cargo test raster_cache` — PASS.
**Stays green independently:** still no production callers; all tests use
`tempfile::tempdir()`, no globals, parallel-safe.
**Step 5:** Commit: `feat(raster-cache): atomic store and lookup`

---

### Task 4: stale-sibling cleanup, scoped to keep `<hash>-<mtime>-*`

**Files:**
- Modify: `src/panel/raster_cache.rs`

**Step 1: Failing tests:**

```rust
#[test]
fn store_removes_other_mtime_siblings_of_any_kind() {
    // store (src,100,img960), (src,100,vid120), (other,100,img960); plus an
    // orphaned "<entry_name(src,90,img960)>.999.part" file. Then
    // store (src,200,img960):
    //  - (src,200,img960) hits
    //  - BOTH (src,100,*) entries and the old-mtime .part are gone
    //  - (other,100,img960) untouched
    //  - dir holds exactly 2 files.
}

#[test]
fn store_keeps_same_mtime_entries_of_other_kinds() {
    // The extension design's warning case (widened-glob regression):
    // store (src,100,vid120); then store (src,100,img960) twice.
    // vid120 must still hit — same-mtime siblings are NEVER swept.
}
```

**Step 2:** `cargo test raster_cache` — FAIL (first test: old-mtime entries
survive; second passes trivially before, red via the first — if both pass,
tighten the first's file-count assert).

**Step 3: Implement** — call at the end of `store_in`, after the rename:

```rust
/// Delete every entry of `src`'s hash whose name is NOT in the keep
/// scope `<hash>-<mtime>-` (i.e. all rasters of older/newer mtimes,
/// any kind, plus their orphaned .part files). Same-mtime entries of
/// other kinds are valid and survive. Errors ignored — a racing
/// instance may have removed the file already.
pub(crate) fn cleanup_stale(dir: &Path, src: &Path, mtime_secs: u64) {
    let hash = hash_prefix(src);
    let keep = keep_prefix(src, mtime_secs);
    // read_dir; for names starting with `hash` but not `keep`: remove_file.
}
```

Note the signature takes `mtime_secs`, not a `keep: &str` filename — the
keep scope is a *prefix*, deliberately covering all kinds at that mtime.

**Step 4:** `cargo test raster_cache` — PASS.
**Stays green independently:** behavior change is internal to `store_in`,
which still has no production callers.
**Step 5:** Commit: `feat(raster-cache): stale-sibling cleanup scoped to hash+mtime`

---

### Task 5: prune (age, then size cap oldest-first)

**Files:**
- Modify: `src/panel/raster_cache.rs`

**Step 1: Failing tests** (`now` is a parameter — no `set_var`, no sleeps;
backdate with `File::options().write(true).open(p)?.set_modified(t)`,
mirroring nothing external — unlike the old `touch -d` test in
`external_cmd_tests`):

```rust
#[test]
fn prune_deletes_by_age() {
    // 31-day-old file dies, fresh file survives, with the size cap at
    // u64::MAX so only the age rule fires.
}

#[test]
fn prune_enforces_size_cap_oldest_first() {
    // three 1000-byte files aged 3/2/1 days, cap 2500 → exactly the
    // oldest is deleted; mix kinds in the names (img960/vid120) to pin
    // the extension's "one shared budget" requirement.
}
```

**Step 2:** `cargo test prune` — FAIL.

**Step 3: Implement:**

```rust
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

fn prune_dir(dir: &Path, max_age: Duration, max_total_bytes: u64, now: SystemTime) { … }
```

Collect `(path, mtime, len)` for regular files; delete age-expired ones;
sort survivors by mtime; delete oldest-first while the running total exceeds
the cap, subtracting only on successful removal. All errors ignored
(concurrent-instance races are benign). Orphaned `.part` files are regular
files and age out like everything else.

**Step 4:** `cargo test prune` — PASS.
**Stays green independently:** dir-parameterized, unused in production
until Task 6.
**Step 5:** Commit: `feat(raster-cache): startup prune (age + size cap)`

---

### Task 6: public API, `preview_cache` config flag, main.rs wiring

**Files:**
- Modify: `src/panel/raster_cache.rs` (static + wrappers)
- Modify: `src/config.rs:24-36` (`GeneralConfig`)
- Modify: `src/main.rs` (locals ~line 169, config copy ~line 180, init after
  the config if/else ending at line 201)
- Modify: `examples/config.toml` (`[general]`, next to the `use_trash`
  block at lines 13-21)

**Step 1: Failing test** for the config field (new `#[cfg(test)] mod tests`
in config.rs — `GeneralConfig` is fully serde-defaulted, so an empty table
deserializes):

```rust
#[test]
fn preview_cache_defaults_true_and_parses_false() {
    let g: GeneralConfig = toml::from_str("").unwrap();
    assert!(g.preview_cache);
    let g: GeneralConfig = toml::from_str("preview_cache = false").unwrap();
    assert!(!g.preview_cache);
}
```

`cargo test preview_cache` — FAIL (unknown field).

**Step 2: Implement config** — add to `GeneralConfig` after `use_trash`,
reusing `default_true`:

```rust
/// Persist image/video preview rasters in $XDG_CACHE_HOME/rfm so they
/// survive restarts. Defaults to `true`.
#[serde(default = "default_true")]
pub preview_cache: bool,
```

**Step 3: Wrappers** (no unit test — process-global `OnceCell` glue, see
resolved ambiguity 10; everything under it is covered by Tasks 3–5):

```rust
static RASTER_CACHE_DIR: OnceCell<Option<PathBuf>> = OnceCell::new();

pub fn init(enabled: bool)              // resolve+create dir, warn-once on failure → None
pub fn dir() -> Option<&'static Path>   // None = disabled or unavailable
pub fn lookup(src: &Path, mtime_secs: u64, kind: &str) -> Option<DynamicImage>
pub fn store(src: &Path, mtime_secs: u64, kind: &str, img: &DynamicImage)  // best-effort, log::debug on Err
pub fn prune()                          // for spawn_blocking at startup
```

`init` resolves `xdg_cache_home()?.join("rfm").join("thumbnails")`,
`create_dir_all`s it; on either failure `log::warn!` once and run the
session with persistence off (design §error handling) — never fatal.

**Step 4: main.rs wiring** — follow the `use_trash` pattern exactly, but
with the collision-free local (resolved ambiguity 4):
`let mut persist_previews = true;` next to `use_trash` (line 169);
`persist_previews = config.general.preview_cache;` in the `Ok(config)` arm
(line 180 vicinity); directly after the config if/else block (line 201):

```rust
panel::raster_cache::init(persist_previews);
tokio::task::spawn_blocking(panel::raster_cache::prune);
```

(`main` is `#[tokio::main]`, line 82 — `spawn_blocking` is legal here.)

**Step 5: examples/config.toml** — document in the `[general]` comment
block, matching the `use_trash` entry style:

```toml
#   - preview_cache = true  : persist image/video preview thumbnails in
#     $XDG_CACHE_HOME/rfm (usually ~/.cache/rfm) so they survive restarts.
#   - preview_cache = false : previews stay in-memory only; nothing about
#     your files is written to disk. "rm -rf ~/.cache/rfm" is always safe.
preview_cache = true
```

**Step 6:** `cargo build && cargo test && cargo clippy --all-targets` —
clean (drop any Task-2 `#[allow(dead_code)]`). Quick smoke:
`XDG_CACHE_HOME=$(mktemp -d) ./target/debug/rfm --debug-socket /tmp/rfm.sock`
in tmux; confirm `<tmpdir>/rfm/thumbnails/` exists; teardown per CLAUDE.md.
**Stays green independently:** flag defaults true, wrappers are inert until
a producer calls them (none do yet).
**Step 7:** Commit: `feat(raster-cache): public API, preview_cache flag, startup wiring`

---

### Task 7: image previews through the cache (`img960`)

**Files:**
- Modify: `src/panel/preview.rs` (`FilePreview::new` image arm line 226,
  `image_info_lines` line 301, `native_image_preview` line 327,
  `video_preview`'s mtime conversion lines 396-399)

**Step 1: Failing tests** (in `native_backend_tests`, tempfile fixtures):

```rust
#[test]
fn cached_image_preview_stores_on_miss_and_hits_without_full_decode() {
    // Write an 8×8 png; call cached_image_preview_in(cache_dir, ...) twice
    // (a dir-parameterized twin, same pattern as lookup_in/store_in, so the
    // test does not touch the process-global wrapper). After call 1 the
    // cache dir contains entry_name(path, mtime, KIND_IMAGE). Delete the
    // SOURCE file, call again with the same mtime: still returns
    // Preview::Image with Some(img) — proof the pixels came from the cache,
    // not a re-decode (the render-count-style probe of this module).
}

#[test]
fn cached_image_preview_hit_reports_original_dimensions() {
    // 1200×800 png (larger than the 960×540 thumbnail bound): on the
    // second (hit) call the info lines still contain "1200 × 800"
    // (header-read dims), not the thumbnail's.
}

#[test]
fn cached_image_preview_of_an_undecodable_image_falls_back_to_text() {
    // b"not an image" with .heic name: Preview::Text (mediainfo fallback
    // preserved through the front-end) and NO cache entry written.
}
```

**Step 2:** `cargo test cached_image_preview` — FAIL.

**Step 3: Implement:**

a. Extract the mtime conversion inlined in `video_preview` (lines 396-399)
   into `fn mtime_secs(modified: SystemTime) -> u64` and use it there.

b. Refactor `image_info_lines` to a parameter core (updates the existing
   `image_info_lines_contain_dimensions_format_and_size` test minimally):

```rust
fn image_info_lines(width: u32, height: u32, color: image::ColorType,
                    byte_size: u64, modified: SystemTime, subtype: &str) -> Vec<String>
```

   `native_image_preview` passes `(img.width(), img.height(), img.color(), …)`.

c. The caching front-end (dir-parameterized core + thin wrapper, mirroring
   raster_cache's own layering):

```rust
/// Image arm via the persistent raster cache: a hit decodes only the
/// small cached JPEG; a miss decodes the original via
/// native_image_preview and stores the thumbnail for next time.
fn cached_image_preview(path: &Path, modified: SystemTime, mime: &mime::Mime) -> Preview {
    let mtime = mtime_secs(modified);
    if let Some(img) = raster_cache::lookup(path, mtime, raster_cache::KIND_IMAGE) {
        log::debug!("raster cache hit for {}", path.display());
        let info = cached_image_info(path, &img, mime.subtype().as_str());
        return Preview::Image { img: Some(img), info };
    }
    let preview = native_image_preview(path, mime);
    if let Preview::Image { img: Some(img), .. } = &preview {
        raster_cache::store(path, mtime, raster_cache::KIND_IMAGE, img);
    }
    preview
}
```

   `cached_image_info` builds the same three lines: dimensions from
   `image::io::Reader::open(path)` + `into_dimensions()` (header-only, no
   pixel decode; on error fall back to the cached img's dimensions), color
   from the cached img (resolved ambiguity 5), size/mtime from
   `path.metadata()`.

d. `FilePreview::new` image arm becomes
   `("image", _) => cached_image_preview(&path, modified, &mime),` —
   `modified` is already computed at the top of `new` (line 217).

e. **`image_preview` (line 357) stays cache-free** — the video path uses it
   to decode cache files themselves, which must not re-enter the cache.

**Step 4:** `cargo test && cargo clippy --all-targets` — clean (existing
`native_image_preview_*` tests must still pass unchanged — the raw function
is untouched).
**Stays green independently:** with the wrapper uninitialized in tests,
`lookup`/`store` are no-ops → the front-end degrades to exactly
`native_image_preview`; the new tests use the `_in` twins.
**Step 5:** Commit: `feat(preview): image previews through the persistent raster cache`

---

### Task 8: video thumbnails to the cache dir (`vid120`), atomically

**Files:**
- Modify: `src/panel/preview.rs` (`ffmpeg_thumbnail` lines 419-480,
  `prune_older_than` stays for the fallback dir)
- Modify: `external_cmd_tests` (`ffmpeg_thumbnail_of_a_long_video_lands_in_the_thumbnail_dir`,
  line 1590)

**Step 1: Update the existing test FIRST** so it pins the new name scheme
(this is the failing test — it currently asserts the legacy
`{hash}{mtime}.jpg` name):

```rust
#[test]
fn ffmpeg_thumbnail_of_a_long_video_lands_in_the_thumbnail_dir() {
    // raster_cache::dir() is uninitialized in unit tests → the temp_dir
    // fallback is exercised deterministically.
    let thumbnail = temp_dir()
        .join("rfm-thumbnails")
        .join(raster_cache::entry_name(&video, 1, raster_cache::KIND_VIDEO));
    assert!(thumbnail.is_file());
    // atomicity: no .part siblings left behind
    …
}
```

`cargo test ffmpeg_thumbnail` — FAIL (old name still produced).
`ffmpeg_thumbnail_of_a_too_short_video_is_an_error` (line 1579) must keep
passing throughout, plus a new assertion there: the failed run leaves no
`.part` file behind.

**Step 2: Implement** — keep the existing structure (cache-hit fast path,
`out.status` + exists check, stderr-tail bail) and change only:

- Directory: `let dir = raster_cache::dir().map(Cow::from).unwrap_or_else(|| fallback_dir())`
  where the fallback keeps today's exact behavior — `temp_dir().join("rfm-thumbnails")`,
  `create_dir_all`, one-time 7-day `prune_older_than` via the existing
  `OnceCell` (resolved ambiguity 7). Delete no code paths users rely on
  with `preview_cache = false`.
- Name: `let name = raster_cache::entry_name(path.as_ref(), modified, raster_cache::KIND_VIDEO);`
  (drop the local seahash/`identifier` lines 421-423).
- Atomic write: ffmpeg targets
  `dir.join(format!("{name}.{}.part.jpg", std::process::id()))` — the
  extension is kept **last** so ffmpeg's container inference still works
  (design §read/write); on success `fs::rename(part, final)`, then
  `raster_cache::cleanup_stale(&dir, path.as_ref(), modified);` (also in
  the fallback dir — harmless, replaces nothing the 7-day prune wouldn't).
  On failure remove the `.part` (replaces the current partial-file removal
  of the final name, which can no longer be partial).
- Hit log line: reuse the exact `"raster cache hit for …"` wording from
  Task 7 so the e2e grep covers both producers.

**Step 3:** `cargo test && cargo clippy --all-targets` — clean.
**Stays green independently:** unit tests always exercise the fallback dir
(global uninitialized); live runs get the persistent dir from Task 6's init.
**Step 4:** Commit: `feat(preview): video thumbnails in the persistent cache, atomic writes`

---

### Task 9: end-to-end verification (tmux + debug socket)

No code — the CLAUDE.md loop, with `XDG_CACHE_HOME` pointed at a scratch
dir. Needs ffmpeg. Fixture names with spaces per the house rule.

```bash
cd <clone>   # all commands run in the clone
FIXTURE=$(mktemp -d); CACHE=$(mktemp -d); CONF=$(mktemp -d)
mkdir "$FIXTURE/Bilder & Videos"
ffmpeg -f lavfi -i color=red:size=64x64 -frames:v 1 "$FIXTURE/Bilder & Videos/red img.png"
ffmpeg -f lavfi -i testsrc=duration=15:size=320x240:rate=10 "$FIXTURE/Bilder & Videos/a clip.mp4"
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test "XDG_CACHE_HOME=$CACHE ./target/debug/rfm --debug-socket /tmp/rfm.sock '$FIXTURE/Bilder & Videos'" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done
```

1. **Populate:** select `a clip.mp4` then `red img.png` (`j`/`k`), after
   each: `await-idle`, then poll `state` until `seq` stabilizes
   (await-idle does not cover in-flight preview loads — CLAUDE.md caveat).
   `ls "$CACHE/rfm/thumbnails/"` → exactly two files matching
   `<16hex>-<mtime>-img960.jpg` and `<16hex>-<mtime>-vid120.jpg`,
   zero `.part` files. `capture-pane` shows the image preview.
2. **Restart-and-hit:** quit rfm (`Q`), relaunch same command line, select
   both files again, then
   `echo "log 100" | socat - UNIX-CONNECT:/tmp/rfm.sock | grep "raster cache hit"`
   → hits for both producers; no new files in the cache dir.
3. **Invalidation:** `touch "$FIXTURE/Bilder & Videos/red img.png"`,
   re-select; the old-mtime img960 sibling is replaced — still exactly one
   `img960` file for that hash prefix, and the `vid120` entry (different
   hash) is untouched.
4. **`preview_cache = false` writes nothing:** copy the default config to
   `$CONF/rfm/config.toml`, set `preview_cache = false`, relaunch with
   `--config $CONF/rfm` and a **fresh** `XDG_CACHE_HOME=$(mktemp -d)`;
   preview both fixtures; the new cache root must contain **no**
   `rfm/thumbnails` writes (dir may exist only if init created it —
   assert it is absent, since init(false) never resolves the dir).
   Previews still render (in-memory path).
5. **Teardown:** `tmux kill-session -t rfm-test; rm -rf "$FIXTURE" "$CACHE" "$CONF" /tmp/rfm.sock`

If anything "silently" fails, check `log` first (background/ffmpeg stderr
lands in the retention history).

**Step:** Commit only if fixes were needed (each fix returns to its task's
tests first).

---

### Task 10: docs

**Files:**
- Modify: `docs/configuration.md` (document `preview_cache` under
  `[general]`, next to the `use_trash` line 24, same style)
- Modify: `CLAUDE.md` (short "Architecture: preview raster cache" section:
  location, `<hash>-<mtime>-<kind>.jpg` filename-is-metadata scheme with
  the `img960`/`vid120` kinds, keep-scope of the stale sweep, atomic
  `.part`+rename, prune constants, `preview_cache` flag, temp-dir fallback,
  and the test recipe of pointing `XDG_CACHE_HOME` at a scratch dir in the
  tmux pane)

**Steps:** write both, `cargo test && cargo clippy --all-targets` one last
time, commit: `docs: persistent preview raster cache`.

---

## Verification checklist (end state)

- [ ] `cargo test` and `cargo clippy --all-targets` clean
- [ ] Image + video previews create `<hash>-<mtime>-<kind>.jpg` entries;
      restart hits them (debug-socket `log` shows "raster cache hit")
- [ ] Same path+mtime with two kinds coexist; store never sweeps
      same-mtime siblings of other kinds
- [ ] No `.part` files remain after normal operation or ffmpeg failure
- [ ] `touch`ing a source replaces all its old-mtime entries (any kind)
- [ ] `preview_cache = false` → no cache-dir writes at all; video
      thumbnails fall back to `temp_dir()/rfm-thumbnails` as today
- [ ] Fixture paths with spaces and `&` work throughout
- [ ] No new crates; MSRV 1.83 untouched
