# Shared Raster Cache Extension — Design

Status: proposed, 2026-07-30
Extends the persistent thumbnail cache
(`2026-07-30-thumbnail-cache-design.md`, commit `1750ebd`) from an
image+video store into a shared sink for **all** raster-producing previews:
SVG, PDF page renders, and font samples. Depends on that doc landing first, and
serves the image-producing types from `…-preview-new-types-design.md` and the
PDF image tier from `…-preview-pdf-design.md`.

## Motivation

The thumbnail-cache design stores two kinds of raster: decoded image thumbnails
and ffmpeg video frames, both keyed on `seahash(abs_path) + mtime`. The new
preview types add three more *derived* rasters — an SVG rendered by resvg, a PDF
page rendered by pdftoppm, a pangram rasterised from a font. All are expensive
to produce and, like thumbnails, are pure functions of the source file plus a
few render parameters. They should live in the same cache instead of being
recomputed on every visit.

The one thing the base design does not handle: its key assumes the cached raster
is *the source file, thumbnailed*. A derived raster can be produced multiple
ways from the same source (PDF page 1 vs page 2; font sample at 24px vs 32px;
resvg at one bound vs another), so the key needs a **render-params
discriminator** to avoid collisions.

## Key scheme (the only real change)

Base design key (thumbnail-cache doc §"Layout and key scheme"):

```
<seahash(abs_path):016x>-<mtime_secs>.jpg
```

Generalise to include a producer/params tag:

```
<seahash(abs_path):016x>-<mtime_secs>-<kind>.<ext>
```

Where `<kind>` is a short stable string encoding producer + the parameters that
affect the pixels:

| Producer | `<kind>` example | Invalidated by |
|---|---|---|
| image thumbnail (existing) | `img960` | source mtime |
| video frame (existing) | `vid120` | source mtime |
| SVG render | `svg960` | source mtime |
| PDF page render | `pdf-p1-960` | source mtime |
| font sample | `font-s1-24` | source mtime + sample-string version `s1` |

- Existing entries keep working if the two current producers adopt `img*`/`vid*`
  kinds; a migration is unnecessary — mismatched old names simply miss and
  regenerate once (the base design already tolerates that).
- The sample-string version (`s1`) for fonts means changing the pangram bumps to
  `s2` and old samples become stale siblings, cleaned by the existing
  stale-sibling sweep — but note that sweep matches `<hash>-*`; with the `kind`
  suffix it must match `<hash>-<mtime>-*` so a font re-render doesn't wipe the
  image thumbnail of a same-named file. **This is the one code change to the
  base cache's `store` cleanup** and must be called out in implementation.

## Read/write path (unchanged mechanics)

Reuse the base design's two functions, extended with a `kind` argument:

```rust
fn lookup(path, mtime, kind: &str) -> Option<DynamicImage>
fn store(path, mtime, kind: &str, &DynamicImage)
```

Every new producer calls `lookup` before doing expensive work and `store` after:

- **SVG** (`svg_preview`): `lookup(path, mtime, "svg960")`; on miss, resvg-render,
  `store`, return `Preview::Image`.
- **Fonts** (`font_preview`): `lookup(path, mtime, "font-s1-24")`; on miss,
  rasterise the pangram, `store`.
- **PDF image tier** (`pdf_render_first_page`): the renderer (pdftoppm) writes to
  a temp file; instead of the ad-hoc temp path in the PDF doc, target the cache
  dir with the `pdf-p1-960` kind and the same temp-name + rename atomicity rule
  the base design mandates for ffmpeg. This unifies PDF caching with video
  caching — one code path.

Atomicity, eviction (30-day age + 256 MB cap), the `preview_cache` config
switch, and error handling are **inherited unchanged** from the base design.
Derived rasters count toward the same size cap, which is desirable — one budget
for everything.

## Config

None beyond the inherited `preview_cache` switch. When it is `false`, all
producers skip lookup/store and render every visit, exactly as the base design
specifies for images/videos.

## Error handling

Inherited verbatim: cache-dir failure disables persistence for the session;
corrupt-entry lookup deletes and regenerates; store/prune I/O errors are logged
at `debug` and swallowed. A derived raster is never lost to a cache fault, only
recomputed.

## Testing

Extends the base design's cache tests (terminal-free, `tempfile`,
`XDG_CACHE_HOME` scratch dir):

- **Key discriminator:** same source path+mtime with two different `kind`s
  produces two distinct entries that don't overwrite each other.
- **Stale-sibling scope:** re-rendering the font sample (`s1`→`s2`) removes only
  the old font entry, not the image thumbnail of a same-named/mtime file — the
  regression the widened glob could cause.
- **Producer round-trip:** each of SVG/PDF/font stores on miss and hits on the
  second call (assert no re-render via a render-count probe, mirroring the
  existing `resize_count` test at preview.rs:287).
- **Cap sharing:** derived rasters count toward the 256 MB prune (extend the
  base size-cap test with a mix of kinds).

## Out of scope

- Re-designing eviction/atomicity/config — owned by the base thumbnail-cache
  doc; this doc only adds the `kind` discriminator and the widened stale-sibling
  match.
- Caching **text** previews (archive listings, PDF text tier, Office text) —
  cheap to regenerate, excluded exactly as the base design excludes bat/text
  output.
- freedesktop thumbnail-spec interop (already out of scope in the base doc).
