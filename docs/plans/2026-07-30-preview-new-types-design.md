# New Preview Types — Design

Status: proposed, 2026-07-30
Covers Group C of the preview overhaul (`feature-better-previews.md`) minus PDF,
which has its own doc. Depends on the dispatch/fallback conventions and content
sniffing from `…-preview-native-backends-design.md`.

## Motivation

Several common file types have no dedicated preview today and fall through to
either binary-`bat` (XML/text soup) or the `mediainfo` catch-all (boilerplate):
SVG, Office/OpenDocument, epub, SQLite databases, and fonts. Each has a
pure-Rust path to something genuinely useful. Extra archive formats
(`.7z`, `.zst`/`.xz`/`.bz2` tarballs) round out the archive story started in the
native-backends doc.

Each type below is **independent** — one dispatch arm, one crate, its own PR.
All rely on Group B content sniffing so they route even when the extension is
missing or wrong. Image-producing types (SVG, fonts) yield
`Preview::Image { img: Some(..), .. }` and should flow through the shared raster
cache (`…-preview-raster-cache-design.md`) rather than re-rasterising each visit.

## Dispatch

These types currently resolve to MIME types that land on the wrong arm. Add
explicit arms in `FilePreview::new` (`src/panel/preview.rs:225`) and, where
`mime_guess` mislabels them, a special-case entry in `get_mime_type`
(`src/engine/opener.rs:24`). Office/epub/SQLite are best keyed off sniffed magic
(they are all `application/*` blobs) — lean on Group B.

## Types

### C1 — SVG → `resvg`
Today `.svg` is `image/svg+xml`, so it either fails the raster `image` decode or
shows XML. Render with `resvg` (+ `usvg`, `tiny-skia`) to a `tiny_skia::Pixmap`
at preview resolution, convert to `image::RgbaImage` → `DynamicImage`, emit
`Preview::Image`. Size the raster to the same 960×540 bound `image_preview` uses
so the existing resize-cache draw path is unchanged. Cache the rendered raster
(raster-cache doc). Fallback on parse error: the current bat/text path.

### C2 — Office / OpenDocument / epub → `zip` + XML strip
docx/xlsx/pptx/odt/ods/odp/epub are all zip+XML containers, so this reuses the
`zip` crate added in the native-backends doc. Extract the primary content part
and show a **text** preview:
- docx → `word/document.xml`; odt → `content.xml`; epub → spine XHTML in order.
- xlsx → `xl/sharedStrings.xml` (first N strings) as a cheap first cut.
Strip tags to plain text (a minimal hand-rolled stripper, or `quick-xml` reading
text nodes — prefer `quick-xml`, it is small and already a common transitive
dep). Cap at 128 lines. This replaces today's mediainfo-catch-all for these.
Fallback: archive listing (treat as a plain zip) so the preview is never empty.

### C3 — SQLite → `rusqlite`
Magic `SQLite format 3\0`. Open read-only (`OpenFlags::SQLITE_OPEN_READ_ONLY`),
list tables from `sqlite_master`, and for each show name + `COUNT(*)`. Emit
`Preview::Text`. Guard against huge/locked DBs: read-only open plus a busy
timeout of 0 (fail fast rather than block the panel task). Bundled vs system
libsqlite: use `rusqlite` with the `bundled` feature to avoid adding a *system*
dependency while removing an external — matches the spirit of the overhaul.
Fallback: native stat-block.

### C4 — Fonts (ttf/otf/woff2) → `ab_glyph` / `fontdue`
Rasterise a pangram sample ("The quick brown fox…" + a digits/symbols row) at a
readable px size into an `image::GrayImage`/`RgbImage` → `DynamicImage`, emit
`Preview::Image`. Show the family/style name (from the font's name table) in the
info lines. `ab_glyph` is the lighter choice; `fontdue` gives nicer hinting.
Cache the rendered sample (raster-cache doc, keyed with a fixed sample-string
version so changing the pangram invalidates old entries). Fallback: stat-block.

### C5 — Extra archive formats
Extend the native archive listing (native-backends A2/A3) to more containers,
all pure-Rust:
- `.7z` → `sevenz-rust` (list entry names + sizes).
- `.tar.zst` / `.tar.xz` / `.tar.bz2` → `zstd` / `xz2` (or `liblzma`-free
  `ruzstd`/`xz`) / `bzip2` decoder feeding the `tar::Archive` from A3.
This fixes today's fragility where a `.tar.zst` only lists if the *system* tar
was built with zstd support. Same 128-line cap, same size/mode columns.
Fallback: the existing shell-out to `tar`/`7z` where present, else error text.

## Config

None. Each type is a strictly-additive dispatch arm. (Whether SQLite/Office
previews should be *disable-able* for privacy is a possible future switch; not
in v1 — noted in Out of scope.)

## Error handling

Uniform with the native-backends doc: every backend is best-effort. On parse or
open failure, log at `debug` and fall back to the per-type fallback named above
(never an empty panel). Image-producing types that fail rasterisation fall back
to a text/stat preview, not a broken `Preview::Image { img: None }` unless the
source is genuinely undecodable.

## Testing

Terminal-free unit tests with `tempfile` fixtures, mirroring
`external_cmd_tests`:

- **C1 SVG:** render a tiny known SVG, assert a non-empty `RgbaImage` of the
  expected bound; malformed SVG falls back to text.
- **C2 Office:** build a minimal docx/odt/epub zip fixture in-test, assert the
  body text appears and tags are stripped; a corrupt container falls back to a
  zip listing.
- **C3 SQLite:** create a DB with two tables + rows via `rusqlite`, assert both
  table names and correct counts; a locked/garbage file falls back to stat.
- **C4 fonts:** rasterise a bundled test font, assert non-empty raster + the
  family name in info lines.
- **C5 archives:** build `.7z` and `.tar.zst` fixtures, assert names listed and
  the 128-line cap; assert graceful fallback when the decoder rejects the input.
- **Routing:** an extensionless SQLite/PDF/zip file reaches the right arm via
  Group B sniffing (cross-check with the native-backends sniff tests).

## Out of scope

- **PDF** — separate doc (`…-preview-pdf-design.md`).
- The shared **raster cache** mechanics — `…-preview-raster-cache-design.md`;
  this doc only states that C1/C4 must use it.
- Privacy toggles for database/document previews — revisit if requested.
- Rendering fidelity (sixel/kitty) for the image-producing types — Group E doc;
  these types emit standard `Preview::Image` and benefit automatically.
