# PDF Preview — Design

Status: proposed, 2026-07-30
Split out of Group C (`feature-better-previews.md`) because PDF has an
architectural fork the other new types don't. Depends on the dispatch/fallback
conventions and content sniffing from `…-preview-native-backends-design.md`, and
(for the image tier) the shared raster cache from
`…-preview-raster-cache-design.md`.

## Motivation

PDFs currently hit the `mediainfo` catch-all and show boilerplate. A useful PDF
preview is worth having, but "render a page to an image" and "show text/metadata"
are fundamentally different amounts of dependency and risk. This doc picks a
**tiered** approach so the useful-and-cheap tier ships without committing to the
heavy one.

## The fork

- **Text tier (native, no binary):** `lopdf` (or `pdf`) can read the document
  catalog — page count, title/author from the `/Info` dictionary — and extract
  the text of the first page(s). Pure Rust, no external process, always
  available. Output is a `Preview::Text` block. This is the **default tier** and
  the one to build first.
- **Image tier (external):** rasterising a page to a real thumbnail needs a PDF
  renderer. There is no production-grade pure-Rust one, so this tier shells out
  to **`pdftoppm`** (poppler) or **`mutool`** (mupdf) to render page 1 to a
  bitmap, then feeds it through `image_preview` as a `Preview::Image`. Optional,
  presence-checked like ffmpeg, and cached like a video thumbnail.

The tiers compose: attempt the image tier when its binary is present and the
feature is enabled, else fall back to the text tier, else fall back to the
stat-block. The text tier alone is a complete, shippable feature.

## Dispatch

Add an `("application", "pdf")` arm to `FilePreview::new`
(`src/panel/preview.rs:225`). PDFs are reliably detected by the `%PDF` magic, so
they also route correctly when extensionless via Group B sniffing.

```rust
("application", "pdf") => pdf_preview(&path, modified),
```

`pdf_preview` chooses the tier:

```rust
fn pdf_preview(path, modified) -> Preview {
    if pdf_image_enabled() && PDFTOPPM_INSTALLED {   // OnceCell probe, like ffmpeg
        match pdf_render_first_page(path, modified) { // cached raster
            Ok(p) => return p,
            Err(e) => log::debug!("pdf render failed, text tier: {e}"),
        }
    }
    match pdf_text_tier(path) {                       // lopdf
        Ok(p) => p,
        Err(e) => { log::debug!("pdf text tier failed: {e}"); native_stat(path) }
    }
}
```

## Text tier (native)

`pdf_text_tier`:
- Open with `lopdf::Document::load`.
- Info lines: page count, title/author/producer from `/Info` when present.
- Body: extract text of page 1 (lopdf `extract_text`), normalised to plain lines,
  scrubbed of `\r`/`\n`, capped at 128 lines — same conventions as `bat_preview`.
- Encrypted PDFs: `lopdf` reports this; show "encrypted PDF (N pages)" + metadata
  rather than an error.

Robustness: PDF parsers are a memory-safety and infinite-loop minefield on
malformed input. Keep the parse on the existing panel task but guard it —
catch panics is not idiomatic; instead rely on `lopdf` returning `Err`, and cap
work (only page 1, bounded text length). If lopdf proves fragile in practice,
revisit moving it behind `spawn_blocking` with a timeout (same concern as the
ffmpeg call, tracked in the Group D doc).

## Image tier (external, optional)

`pdf_render_first_page` mirrors `ffmpeg_thumbnail` (preview.rs:367):
- Probe the binary once via `OnceCell` (`pdftoppm -v` / `mutool -v`).
- Render page 1 at preview resolution to a temp name in the cache dir, then
  `rename` to the final cached name (atomicity rule from the raster-cache doc).
  `pdftoppm -jpeg -f 1 -l 1 -scale-to 960 <pdf> <prefix>`.
- Key the cache entry on `seahash(abs_path) + mtime` **plus a render-params
  discriminator** ("pdf-p1-960") so it never collides with a video thumbnail or
  a different render setting — see the raster-cache doc's keying section.
- On non-zero exit or missing output, delete any partial file and return `Err`
  so the caller drops to the text tier.

## Config

One switch, next to `preview_cache` in `[general]`:

```toml
pdf_render = false   # default: text/metadata tier only
```

Default **off** so the base install stays pure-Rust and no one is surprised by a
poppler/mupdf dependency. Setting it `true` opts into the image tier *if* a
renderer is installed; with the binary absent it silently stays on the text
tier. (If the project would rather auto-enable when a renderer is detected, that
is a one-line change — flagged for the implementer.)

## Error handling

- Best-effort throughout: image tier failure → text tier → stat-block. A PDF
  never shows a bare error panel.
- Corrupt/encrypted/huge PDFs: bounded work (page 1 only, capped text), `Err`
  surfaces as a graceful downgrade, partial rendered files are deleted like the
  ffmpeg path.

## Testing

- **Text tier (unit, terminal-free):** commit a tiny known PDF fixture (or
  generate one), assert page count + first-page text lines; assert an encrypted
  fixture shows the "encrypted" line rather than erroring; assert a truncated
  PDF returns `Err` (→ caller falls back).
- **Image tier (unit, guarded):** like the ffmpeg tests, skip when `pdftoppm`
  is absent; when present, assert a cache file is produced with the
  params-discriminated name and a decodable JPEG.
- **Routing:** an extensionless `%PDF` file reaches the pdf arm via Group B.
- **Integration (tmux + socket):** preview a PDF fixture with `pdf_render`
  off → text tier on screen; with it on and a renderer present → image preview;
  `log` history shows the tier actually taken.

## Out of scope

- Multi-page / scrollable PDF previews — page 1 only in v1.
- Rendering all pages or a page picker.
- Bundling a PDF renderer — the image tier stays an optional external.
- The generic raster cache mechanics — `…-preview-raster-cache-design.md`.
