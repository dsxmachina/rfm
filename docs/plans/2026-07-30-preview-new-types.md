# New Preview Types (SVG, Office, SQLite, Fonts, Extra Archives) — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes.

**Goal:** Dedicated previews for five type groups that today fall through to
binary-`bat` soup or the mediainfo/stat catch-all: C1 SVG (rendered raster),
C2 Office/OpenDocument/epub (extracted text), C3 SQLite (table listing),
C4 fonts (rasterised pangram sample), C5 extra archives (`.7z`,
`.tar.zst`/`.tar.xz`/`.tar.bz2`). Each group is one dispatch arm, one crate
set, independently landable, with the per-type fallback the design doc names —
a preview is never empty.

**Architecture:** Follows the two established conventions exactly
(CLAUDE.md "Architecture: native preview backends" / "preview raster cache"):

- Native-first + fallback: each new arm in `FilePreview::new`
  (`src/panel/preview.rs:225`) calls a `*_preview` wrapper that tries a
  `native_*` function (pure Rust, `anyhow::Result<..>`); on `Err` it logs at
  `debug` ("… failed, trying/falling back …") and runs the per-type fallback.
- Line previews: 128-line cap everywhere; every attacker-controlled string
  (document text, table names, font name-table strings, archive member names)
  goes through `scrub_line`.
- Raster-producing types (SVG, fonts) flow through the persistent raster
  cache via `raster_cache::lookup_in`/`store_in` with two new kinds —
  `svg960` and `font-s1-24` per the raster-cache-extension design — using the
  same dir-parameterized `*_in` producer pattern as `cached_image_preview_in`
  (unit tests pass tempdirs, never call `init`).
- MIME routing: extensions resolve via `mime_guess` in `get_mime_type`;
  extensionless/mislabeled files resolve via the existing `infer` sniff.

**Design doc:** `docs/plans/2026-07-30-preview-new-types-design.md`
(+ `…-preview-raster-cache-extension-design.md` for the cache kinds).

**Tech stack — every pin compile- AND run-verified on rustc 1.83.0** (the
MSRV toolchain; a scratch crate exercised each API end-to-end, then the full
set was added to *this* tree: `cargo build` and all 190 existing tests green,
`Cargo.lock` gained 45 packages, `uuid 1.12.1`/`ogg_pager 0.7.0` pins
untouched, only `smallvec` moves 1.13.2 → 1.15.2):

| Crate | Pin | Resolves to | Why this pin |
|---|---|---|---|
| `resvg` | `>=0.45, <0.46`, `default-features = false` | 0.45.1 (usvg 0.45.1, tiny-skia 0.11.4) | 0.46/0.47 declare rust-version 1.87; 0.45.1 declares 1.67.1. `resvg::usvg`/`resvg::tiny_skia` re-exports — no direct tiny-skia dep needed. |
| `quick-xml` | `0.38` | 0.38.4 | declares 1.56 (0.39–0.41 declare ≤1.79, also fine — 0.38 chosen conservatively). |
| `rusqlite` | `>=0.37, <0.38`, `features = ["bundled"]` | 0.37.0 (libsqlite3-sys 0.35.0, cc 1.4.0) | no declared rust-version; MSRV policy is "latest stable at release", so the cap is load-bearing: 0.37.0 is **compile-verified on 1.83.0**, 0.38+ is not. `bundled` avoids a system libsqlite3 (design §C3). |
| `ab_glyph` | `0.2` | 0.2.32 (owned_ttf_parser 0.25.1) | no declared MSRV; verified on 1.83.0. |
| `ttf-parser` | `0.25` | 0.25.1 | for the name table (ab_glyph exposes none); same minor as ab_glyph's owned_ttf_parser → one copy built. |
| `sevenz-rust` | `0.6.1` | 0.6.1 (lzma-rust 0.1.7, bit-set 0.6.0, nt-time 0.8.1) | declares 1.70. **Deviation to note:** the maintained successor `sevenz-rust2` declares 1.85 (latest 1.93) — unusable at MSRV 1.83, so we knowingly take the deprecated-but-functional predecessor. Its writer (`compress_to_path`) also builds the test fixtures. |
| `ruzstd` | `>=0.8, <0.8.2` | 0.8.1 | 0.8.2 switches to edition2024 (**cargo ≥1.85 — cargo 1.83 refuses to even parse it**; its crates.io rust_version is null, so only a build catches this); 0.8.3+ declares 1.87. Pure-Rust decode via `decoding::StreamingDecoder` (implements `Read`), plus `encoding::compress_to_vec` for hermetic fixtures. |
| `lzma-rust2` | `>=0.15, <0.16`, `default-features = false`, `features = ["std", "xz", "encoder"]` | 0.15.8 | pure-Rust XZ chosen over `xz2`/`liblzma` (C bindings). 0.15.x declares 1.82 and is still maintained (0.15.8 is newer than 0.16.0); 0.16+ declares 1.85. `XzReader` implements `Read`; `encoder` is test-only (hermetic `XzWriter` fixtures) but features are crate-global. |
| `bzip2` | `0.6` | 0.6.1 (libbz2-rs-sys 0.2.5) | declares 1.82; 0.6's default backend is the **pure-Rust** libbz2-rs-sys (verified: no `bzip2-sys` in the lock). |

Existing deps reused: `zip` (containers), `tar` (chained listings), `image`,
`infer`, `mime_guess`, `time`, `tempfile` (tests).

**Resolved design ambiguities** (decisions are deliberate; flag any pushback
before deviating):

- **Zip-container routing (design asked to "decide and document"):** no
  extension-first special case is needed. `mime_guess` maps
  docx/xlsx/pptx/odt/ods/odp/epub extensions to their specific
  `application/vnd.*` / `application/epub+zip` types (verified), and for
  extensionless files `infer` 0.22 already discriminates those containers
  from plain zip **by inspecting the zip content** (verified:
  `matchers/doc.rs`, `odf.rs`, `book.rs` return the same specific MIME
  strings). Both routes therefore land on the same new MIME-keyed match arms;
  a container infer can only see as generic zip falls into the existing zip
  arm — an acceptable listing, never empty.
- **SQLite routing:** `.db`/`.sqlite`/`.sqlite3` get no `mime_guess` answer
  (verified `None`) → `get_mime_type` sniffs → `infer` returns
  `application/vnd.sqlite3` from the `SQLite format 3\0` magic. No
  `get_mime_type` special-case needed; `.db` stays magic-verified (a `.db`
  that is not SQLite routes elsewhere — correct).
- **Zstd extensions:** `mime_guess` knows `.xz`/`.bz2`/`.7z` but returns
  `None` for `.zst`/`.tzst`/`.txz`/`.tbz2` (verified). Sniffing would still
  catch regular files (infer has zstd/xz/bzip2 magics), but costs a read and
  misses nothing — add `get_mime_type` special-cases for those four
  extensions instead (Task 6).
- **SVG background:** tiny-skia renders premultiplied RGBA on transparent;
  the raster cache stores JPEG via `to_rgb8()`, which would turn transparency
  into black. Fill the pixmap **white** before rendering — this also makes
  premultiplied == straight alpha, so the raw buffer converts directly to
  `image::RgbaImage`.
- **`ab_glyph` vs `fontdue`:** no concrete reason for fontdue found (both
  compile on 1.83); the design's default `ab_glyph` stands. It has no
  name-table API, hence `ttf-parser` alongside (version-matched).
- **woff/woff2 fonts — deviation to confirm:** design C4 lists woff2, but
  ttf-parser cannot read WOFF containers and there is no MSRV-1.83-worthy
  pure-Rust woff2 (brotli) decoder. woff/woff2 route into the font arm and
  degrade to the design's own fallback (stat block). ttf/otf (+ any sfnt the
  sniff finds) get the real sample. Documented in the arm comment.
- **`svgz`:** `mime_guess` maps it to `image/svg+xml` and
  `usvg::Tree::from_data` auto-detects the gzip magic and decompresses
  (verified in usvg 0.45.1 source) — no extra handling.
- **pptx primary part:** the design names parts for docx/odt/xlsx/epub only.
  pptx uses `ppt/slides/slideN.xml` in ascending-N order (same stripper).

**Per-task rule:** every task ends with `cargo build`, `cargo test`,
`cargo clippy --all-targets` all green, then a commit (trailer:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`). Do not proceed on
a red build. Tests are terminal-free, live in preview.rs's existing
`mod native_backend_tests` style (a new `mod new_type_tests` — sentence-style
names, tempfile fixtures).

---

### Task 1: Dependencies (MSRV-1.83-verified pins)

**Files:** `Cargo.toml` (keep alphabetical), `Cargo.lock` (committed).

**Step 1:** Add to `[dependencies]` (comment style mirrors the existing
`trash`/`zip` pins):

```toml
ab_glyph = "0.2"
# pure-Rust bzip2 backend (libbz2-rs-sys); 0.6 needs rustc 1.82
bzip2 = "0.6"
# pure-Rust LZMA/XZ; 0.16 bumps MSRV to 1.85; cap below it. "encoder" is
# only for hermetic test fixtures (XzWriter).
lzma-rust2 = { version = ">=0.15, <0.16", default-features = false, features = ["std", "xz", "encoder"] }
quick-xml = "0.38"
# resvg 0.46 bumps MSRV to 1.87; cap below it.
resvg = { version = ">=0.45, <0.46", default-features = false }
# MSRV policy is "latest stable at release"; 0.37 compile-verified on 1.83.
rusqlite = { version = ">=0.37, <0.38", features = ["bundled"] }
# ruzstd 0.8.2 switches to edition2024 (cargo >=1.85); cap below it.
ruzstd = ">=0.8, <0.8.2"
# deprecated in favor of sevenz-rust2, which needs rustc >=1.85.
sevenz-rust = "0.6.1"
# same minor as ab_glyph's owned_ttf_parser so only one copy is built.
ttf-parser = "0.25"
```

**Step 2:** `cargo build && cargo test` — green (verified: this exact set
resolves to resvg 0.45.1 / usvg 0.45.1 / tiny-skia 0.11.4 /
quick-xml 0.38.4 / rusqlite 0.37.0 + libsqlite3-sys 0.35.0 /
ab_glyph 0.2.32 + owned_ttf_parser 0.25.1 / ttf-parser 0.25.1 /
sevenz-rust 0.6.1 / ruzstd 0.8.1 / bzip2 0.6.1 + libbz2-rs-sys 0.2.5 /
lzma-rust2 0.15.8, and all 190 tests pass on rustc 1.83.0). The lock is
purely additive except `smallvec` 1.13.2 → 1.15.2; `uuid 1.12.1` and
`ogg_pager 0.7.0` must stay pinned — if cargo reports any `edition2024` /
"requires rustc" package, `cargo update -p <pkg> --precise <old>` it and
note it in the commit message.

**Step 3:** Commit: `chore(deps): new preview type crates (MSRV 1.83 pins)`

---

### Task 2: C1 — SVG rendered via resvg, cached as `svg960`

**Files:**
- Modify: `src/panel/raster_cache.rs` (one constant)
- Modify: `src/panel/preview.rs` (dispatch arm before `("image", _)`,
  new functions near `cached_image_preview`, tests)

**Step 1: Failing tests** (`mod new_type_tests`):

- `svg_render_rasterises_to_the_960x540_bound` — render
  `<svg xmlns="…" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>`;
  assert `Some`, dimensions `(960, 480)` (aspect-fit inside 960×540 — note
  vectors are *scaled up* to the bound, unlike bitmap thumbnails), and that
  a corner pixel is white-backed, not transparent/black.
- `svg_render_of_garbage_is_none` — `b"<not-svg"` → `None`.
- `svg_preview_of_garbage_falls_back_to_text` — a `broken.svg` file with
  garbage yields `Preview::Text` (the bat path), not an image.
- `cached_svg_preview_stores_on_miss_and_hits_without_source` — mirror
  `cached_image_preview_stores_on_miss_and_hits_without_full_decode`:
  miss → the `entry_name(path, mtime, KIND_SVG)` file exists; delete the
  source; second call still yields pixels.

**Step 2:** `cargo test new_type` — FAIL.

**Step 3: Implement.**

- `raster_cache.rs`:
  `pub(crate) const KIND_SVG: &str = "svg960";` (doc comment: resvg render,
  aspect-fit to 960×540, white background).
- `native_svg_render(data: &[u8]) -> Option<DynamicImage>` (API verified on
  resvg 0.45.1):

```rust
let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
let size = tree.size().to_int_size()
    .scale_to(resvg::tiny_skia::IntSize::from_wh(960, 540)?); // aspect-fit
let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())?;
pixmap.fill(resvg::tiny_skia::Color::WHITE); // JPEG cache has no alpha
let sx = size.width() as f32 / tree.size().width();
let sy = size.height() as f32 / tree.size().height();
resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
let img = image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.take())?;
Some(image::DynamicImage::ImageRgba8(img))
```

- `svg_preview(path, modified)` + dir-parameterized `svg_preview_in(cache_dir, …)`
  (the `cached_image_preview_in` pattern): `lookup_in(dir, path, mtime, KIND_SVG)`
  hit → `Preview::Image`; miss → read file (bounded, e.g. 4 MiB via
  `File::take`), `native_svg_render`, `store_in`, return. Info lines: source
  size/mtime + "svg" (reuse `image_info_lines` with the rendered dims, or a
  small svg-specific builder — rendered dims are fine, the source has no
  pixel dims). **Fallback** (design §error handling): render/parse failure →
  `log::debug!("svg render failed, falling back to bat: …")` →
  `bat_preview(path, false)` (XML text), never `Preview::Image { img: None }`.
- Dispatch: insert **before** `("image", _)`:
  `("image", "svg+xml") => svg_preview(&path, modified),`.

**Step 4:** `cargo test new_type` — PASS. **Step 5:** clippy, commit:
`feat(preview): render SVGs natively via resvg, cached as svg960`

---

### Task 3: C2 — Office / OpenDocument / epub text extraction

**Files:** `src/panel/preview.rs` (arms + functions + tests).

**Step 1: Failing tests** (fixtures built in-test with the existing
`zip::ZipWriter` helper style — extend `make_zip` into a
`make_container(dir, &[(name, content)])` that writes real member bodies):

- `docx_preview_shows_body_text_with_tags_stripped` — container with
  `word/document.xml` = `<w:document><w:body><w:p><w:r><w:t>Hello</w:t></w:r><w:t> World</w:t></w:p></w:body></w:document>`;
  assert a line contains `"Hello World"` and no `<`.
- `odt_preview_reads_content_xml` — `content.xml` with `<text:p>` body.
- `xlsx_preview_lists_shared_strings` — `xl/sharedStrings.xml` with two
  `<t>` entries → one line each.
- `epub_preview_follows_container_and_spine` — members
  `META-INF/container.xml` (rootfile → `OEBPS/content.opf`), the OPF
  (manifest + spine with one idref), `OEBPS/ch1.xhtml` with a paragraph;
  assert the paragraph text appears.
- `office_text_is_capped_at_128_lines_and_scrubbed` — 130 paragraphs → 128
  lines; a text node containing `\r\n` stays one line.
- `corrupt_container_falls_back_to_zip_listing` — a docx-named file that IS
  a valid zip but lacks `word/document.xml` → the zip listing lines (member
  names visible); total garbage → still `Preview::Text` (unzip-fallback
  error text). Never empty.

**Step 2:** FAIL. **Step 3: Implement.**

- `xml_text_lines(reader: impl BufRead, paragraph_tags: &[&str]) -> Vec<String>`
  via quick-xml (API verified): loop `read_event_into`; `Event::Text` →
  accumulate `t.decode()`; `Event::End` whose local name is in
  `paragraph_tags` (`p` covers `w:p`, `text:p`, XHTML `p`) → flush a line;
  skip text while inside `style`/`script` (epub XHTML heads). Cap at 128
  lines (stop parsing early), `scrub_line` each, bound the input with
  `.take(512 * 1024)` on the decompressed member reader.
- `native_doc_lines(path, kind) -> anyhow::Result<Vec<String>>` with a small
  `enum DocKind { Docx, Pptx, Xlsx, Odt, Epub }`:
  - Docx → `word/document.xml`; Odt (also ods/odp) → `content.xml`;
  - Xlsx → `xl/sharedStrings.xml`, each `<t>` text node its own line;
  - Pptx → members matching `ppt/slides/slide*.xml`, ascending, until cap;
  - Epub → parse `META-INF/container.xml` for the OPF path, the OPF for
    manifest (id→href) + spine idrefs, resolve hrefs against the OPF dir,
    strip spine documents in order until cap. Any missing piece → `Err`.
- `doc_preview(path, kind)`: `Ok` → `Preview::Text`; `Err` →
  `log::debug!("… falling back to zip listing")` → `zip_preview(path)`
  (design: "treat as a plain zip so the preview is never empty"; that arm
  already has the unzip/error-text chain).
- Dispatch arms (before the generic `("application", _)`):

```rust
("application", "vnd.openxmlformats-officedocument.wordprocessingml.document") => doc_preview(&path, DocKind::Docx),
("application", "vnd.openxmlformats-officedocument.spreadsheetml.sheet") => doc_preview(&path, DocKind::Xlsx),
("application", "vnd.openxmlformats-officedocument.presentationml.presentation") => doc_preview(&path, DocKind::Pptx),
("application", "vnd.oasis.opendocument.text")
| ("application", "vnd.oasis.opendocument.spreadsheet")
| ("application", "vnd.oasis.opendocument.presentation") => doc_preview(&path, DocKind::Odt),
("application", "epub+zip") => doc_preview(&path, DocKind::Epub),
```

**Step 4/5:** tests PASS, clippy, commit:
`feat(preview): text extraction for Office/OpenDocument/epub containers`

---

### Task 4: C3 — SQLite table listing via rusqlite

**Files:** `src/panel/preview.rs` (arm + functions + tests).

**Step 1: Failing tests**

- `sqlite_preview_lists_tables_with_row_counts` — build a DB in-test with
  rusqlite (`CREATE TABLE t1(a); INSERT …x2; CREATE TABLE t2(b);`); assert
  lines contain `t1` with `2` and `t2` with `0`.
- `sqlite_table_names_are_scrubbed_and_quoted` — a table named
  `CREATE TABLE "evil""name"(x)`-style (embedded quote) still counts
  correctly (quoting via `"` doubling), and a crafted name with `\n`
  renders on one line.
- `sqlite_listing_is_capped_at_128_lines` — 130 tables → 128 lines total
  (incl. header).
- `sqlite_preview_of_garbage_falls_back_to_stat` — a file with only the
  16-byte `SQLite format 3\0` magic + garbage → `Preview::Text` containing
  the stat-block markers (`Size:`/`MIME type:`), per design fallback.

**Step 2:** FAIL. **Step 3: Implement** (API verified on rusqlite 0.37.0):

```rust
fn native_sqlite_lines(path: &Path) -> anyhow::Result<Vec<String>> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(
        path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::ZERO)?; // fail fast, never block the panel task
    // header: "SQLite database · N tables", blank line, then
    // "{count:>8}  {name}" per table from sqlite_master (ORDER BY name),
    // COUNT(*) with the identifier quoted as "\"{}\"" after doubling
    // embedded quotes. Take 126 tables + 2 header lines = 128 cap.
    // scrub_line every name.
}
```

A count failure for one table (corrupt page) prints `?` for that row rather
than failing the whole listing; only an open/master-query failure is `Err`.
`sqlite_preview` wraps: `Err` → debug log → `stat_preview(&path, &mime)`.

- Dispatch arm: `("application", "vnd.sqlite3") => sqlite_preview(&path, &mime),`
  (routing is sniff-driven — see header notes; nothing to add in
  `get_mime_type`). Add a routing test in opener.rs's style if
  `get_mime_type` tests exist by now: an extensionless file starting with
  `SQLite format 3\0` + 100 bytes → `application/vnd.sqlite3`.

**Step 4/5:** PASS, clippy, commit:
`feat(preview): native SQLite table listing via rusqlite (bundled)`

---

### Task 5: C4 — font samples via ab_glyph + ttf-parser, cached as `font-s1-24`

**Files:**
- Add: `src/panel/testdata/subset-noto-sans.ttf` (+ `OFL.txt` alongside)
- Modify: `src/panel/raster_cache.rs` (one constant)
- Modify: `src/panel/preview.rs` (arms + functions + tests)

**Step 0: Test fixture.** Commit a small OFL-licensed ASCII subset font
(hermetic tests, no system-font dependency). Generated and verified here
(42 904 bytes, family name "Noto Sans" readable, pangram rasterises):

```bash
pyftsubset /path/to/NotoSans.ttf --unicodes=U+0020-007E \
  --no-hinting --layout-features='' \
  --output-file=src/panel/testdata/subset-noto-sans.ttf
```

Include the SIL OFL 1.1 text as `src/panel/testdata/OFL.txt` (Noto is OFL;
subsetting is permitted, keep the copyright notice). Tests load it via
`include_bytes!("testdata/subset-noto-sans.ttf")`.

**Step 1: Failing tests**

- `native_font_sample_rasterises_the_pangram` — on the fixture: returns
  `(info, img)`; > 200 non-background pixels; info contains "Noto Sans".
- `font_info_includes_family_and_style` — info lines contain family and
  subfamily (name IDs 1/2) plus size/mtime lines.
- `native_font_sample_on_garbage_is_an_error` — `b"not a font"` → `Err`
  (routes to the stat fallback).
- `font_preview_of_a_woff_degrades_to_stat` — a few bytes of `wOFF` magic →
  `Preview::Text` with stat markers (the documented woff deviation).
- `cached_font_preview_stores_on_miss_and_hits_without_source` — the
  `KIND_FONT` mirror of the SVG cache test.

**Step 2:** FAIL. **Step 3: Implement** (API verified on ab_glyph 0.2.32 /
ttf-parser 0.25.1):

- `raster_cache.rs`: `pub(crate) const KIND_FONT: &str = "font-s1-24";`
  — doc comment per the extension design: `s1` is the sample-string version;
  **changing the pangram or px size must bump this constant** (`s2`, `-32`),
  old entries then age out via the stale-sibling sweep.
- `native_font_sample(data: &[u8]) -> anyhow::Result<(Vec<String>, DynamicImage)>`:
  - Names: `ttf_parser::Face::parse(data, 0)`; family/subfamily via
    `face.names()` filtering `name_id::FAMILY`/`SUBFAMILY` + `is_unicode()`,
    `.to_string()`; `scrub_line` both (name tables are attacker-controlled).
  - Raster: `ab_glyph::FontRef::try_from_slice(data)?`,
    `font.as_scaled(PxScale::from(24.0))`; two rows on a white
    `GrayImage` (bounded width ≤ 960): `"The quick brown fox jumps over the lazy dog"`
    and `"0123456789 ?!&@%(){}[]"`. Per char: `scaled.scaled_glyph(c)`,
    set `glyph.position = point(x, baseline)`, advance by
    `scaled.h_advance(glyph.id)`, draw via `outline_glyph(glyph)` +
    `og.px_bounds()` offset (**note:** position is set on the glyph before
    `outline_glyph`; there is no `into_glyph_at` — verified). Dark pixels on
    white so the JPEG round-trip stays clean.
  - Return info lines: family, style, blank, `Size:`/`Modified:` from
    metadata.
- `font_preview(path, modified)` + `font_preview_in(cache_dir, …)`:
  lookup `KIND_FONT` → hit serves cached raster (re-derive info from
  ttf-parser if the source still reads, else just family unknown — keep it
  simple: re-read source for info; if unreadable, metadata lines only);
  miss → `native_font_sample`, `store_in`, `Preview::Image`. `Err` →
  debug log → `stat_preview` (design fallback).
- Dispatch arms:

```rust
// ttf (font/ttf), woff2 (font/woff2), … — woff/woff2 currently degrade to
// the stat fallback (no MSRV-1.83 pure-Rust woff decoder; see plan notes).
("font", _) => font_preview(&path, modified, &mime),
("application", "font-sfnt")      // .otf via mime_guess; ttf/otf via infer
| ("application", "font-woff") => font_preview(&path, modified, &mime),
```

**Step 4/5:** PASS, clippy, commit:
`feat(preview): rasterised font samples via ab_glyph, cached as font-s1-24`

---

### Task 6: C5 — extra archive formats (.7z, .tar.zst/.tar.xz/.tar.bz2)

**Files:** `src/engine/opener.rs` (extension special-cases),
`src/panel/preview.rs` (arms + functions + tests).

**Step 1: Failing tests** (all hermetic — encoders verified):

- `zst_extensions_resolve_without_sniffing` — `get_mime_type` on
  `foo.tar.zst`/`foo.tzst` (nonexistent path!) → `application/zstd`;
  `foo.txz` → `application/x-xz`; `foo.tbz2` → `application/x-bzip2`
  (nonexistent proves no content read).
- `tar_zst_preview_lists_members` — build a tar in-memory (existing
  `make_native_tar` bytes), compress via
  `ruzstd::encoding::compress_to_vec(&bytes[..], CompressionLevel::Fastest)`,
  assert member names listed.
- `tar_xz_preview_lists_members` — fixture via `lzma_rust2::XzWriter::new(file,
  XzOptions::with_preset(1))` (`encoder` feature).
- `tar_bz2_preview_lists_members` — fixture via `bzip2::write::BzEncoder`.
- `zst_of_a_non_tar_file_previews_the_decompressed_text` — the gz-arm
  behavior generalised: a zstd'd text file shows its head as text.
- `compressed_garbage_degrades_to_a_text_preview` — 0xff garbage named
  `.tar.zst`/`.tar.xz`/`.tar.bz2` → still `Preview::Text` (tar-binary
  fallback error text), never a panic.
- `sevenz_preview_lists_names_and_sizes` — fixture via
  `sevenz_rust::compress_to_path(dir_or_file, "t.7z")` (verified); assert
  `size  name` lines.
- `sevenz_list_caps_at_128_and_scrubs` and
  `sevenz_of_garbage_degrades_to_a_text_preview`.

**Step 2:** FAIL. **Step 3: Implement.**

- `get_mime_type` special-cases (top match, comment "mime_guess has no
  mapping for zstd and the compound tar extensions"):

```rust
Some("zst" | "tzst") => return "application/zstd".parse().unwrap(),
Some("txz") => return "application/x-xz".parse().unwrap(),
Some("tbz2") => return "application/x-bzip2".parse().unwrap(),
```

- Refactor `native_gz_preview`'s body into
  `native_compressed_preview(decoder: impl Read) -> anyhow::Result<Preview>`
  (the 512-byte head fill, `ustar`-at-257 sniff, chain into
  `native_tar_list`, else bounded-64 KiB text head — code moves verbatim);
  the gz arm becomes a 3-liner over it.
- New arms + wrappers, each `Err` → debug log →
  `cmd_to_preview("tar", tar_list(path))` (the system tar handles these
  when built with the codec — exactly today's fragile path, now demoted to
  fallback per design):

```rust
("application", "zstd")    => zst_preview(&path),  // ruzstd::decoding::StreamingDecoder::new(BufReader::new(File::open(..)?))?
("application", "x-xz")    => xz_preview(&path),   // lzma_rust2::XzReader::new(BufReader::new(File::open(..)?), true)
("application", "x-bzip2") => bz2_preview(&path),  // bzip2::read::BzDecoder::new(File::open(..)?)
("application", "x-7z-compressed") => sevenz_preview(&path),
```

- `native_sevenz_list(path)`: `sevenz_rust::Archive::open(path)?` (reads
  metadata only, no extraction), `.files.iter().take(128)` →
  `format!("{:>8}  {}", file_size_str(e.size()), e.name())`, `scrub_line`.
  Fallback: `cmd_to_preview("7z", Command::new("7z").arg("l").arg(path)…)`
  — design: "the existing shell-out … where present, else error text"
  (there is no current 7z shell-out; this adds it as fallback-only, giving
  encrypted-header archives a listing when the binary exists).

**Step 4/5:** PASS, clippy, commit:
`feat(preview): native 7z and zstd/xz/bzip2 tarball listings`

---

### Task 7: docs + e2e smoke pass

**Files:** `CLAUDE.md`, (no code).

**Step 1:** Extend CLAUDE.md "Architecture: native preview backends" with
one tight paragraph: the five new arms, their crates, the two new raster
kinds (`svg960`, `font-s1-24`, sample-string version rule), the woff2
limitation, the zst/txz/tbz2 `get_mime_type` special-cases, and that
container routing rides on infer's zip-content discrimination.

**Step 2: e2e smoke** (CLAUDE.md tmux workflow; isolate with
`XDG_CACHE_HOME=$(mktemp -d)` in the pane): fixture dir with one `.svg`,
one `.docx` (from the test fixtures), one `.sqlite`, one `.ttf`, one
`.tar.zst`; cursor over each; `await-idle`, then `log 40` via the socket —
assert no unexpected "falling back" lines for the happy paths, and two new
cache entries (`*-svg960.jpg`, `*-font-s1-24.jpg`) in
`$XDG_CACHE_HOME/rfm/thumbnails`. `capture-pane` to confirm rendered
output. Teardown per CLAUDE.md.

**Step 3:** Commit: `docs: new preview types in the architecture notes`

---

## Verification summary (what was actually proven on rustc 1.83.0)

A scratch crate + this tree, cargo/rustc 1.83.0, 2026-07-31:

- SVG: 100×50 fixture → 960×480 white-backed RGBA via resvg 0.45.1;
  garbage → `None`.
- quick-xml 0.38.4 text-node extraction round-trip.
- rusqlite 0.37.0 bundled: read-only open, `busy_timeout(0)`,
  sqlite_master + COUNT round-trip.
- Fonts: pyftsubset fixture (42 904 B) → family "Noto Sans" via
  ttf-parser 0.25.1, pangram rasterised via ab_glyph 0.2.32 (957 lit px).
- Archives: 7z listing of a sevenz-rust-written fixture; tar.zst/xz/bz2
  listings through ruzstd 0.8.1 / lzma-rust2 0.15.8 / bzip2 0.6.1 decoders
  chained into `tar`, with fixtures from both system tools **and** the
  crates' own encoders; garbage → `Err`.
- Whole-tree: deps added to this Cargo.toml → `cargo build` +
  190/190 tests green; lock additive (45 pkgs, smallvec 1.13.2→1.15.2),
  uuid/ogg_pager pins intact.

Known-blocked at MSRV 1.83 (flagged, not planned): `sevenz-rust2` (1.85+),
`resvg` ≥0.46 (1.87), `ruzstd` ≥0.8.2 (edition2024/1.87), `lzma-rust2`
≥0.16 (1.85), pure-Rust woff2 decoding (brotli stack).
