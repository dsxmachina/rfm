# Preview Native Backends + Content Sniffing — Design

Status: proposed, 2026-07-30
Foundational doc for the preview overhaul (see `feature-better-previews.md`,
Groups A + B). Every other preview design doc builds on the dispatch and
fallback conventions established here.

## Motivation

`FilePreview::new` (`src/panel/preview.rs:216`) dispatches on the MIME type and,
for most non-text types, shells out to an external binary via
`std::process::Command`. When that binary is absent the preview degrades to an
error message ("You must have mediainfo installed…", `cmd_to_preview`,
preview.rs:532) rather than *anything useful*. The externals used purely for
previews are **mediainfo, unzip, openssl, tar** (ffmpeg and bat are handled
separately — see below).

Most of what these binaries produce is reconstructible in-process with a
pure-Rust crate, often with *better* output because we control the formatting.
This doc replaces them behind the existing dispatch arms, keeping each shell-out
as a last-resort fallback so nothing regresses on exotic inputs.

Two externals are deliberately **kept**:
- **ffmpeg** — no realistic pure-Rust decode-any-codec path; stays for video
  thumbnails (`ffmpeg_thumbnail`).
- **bat** — `syntect` (its engine) is a heavy dependency and the plain-read
  fallback (preview.rs:513) already works; keeping bat is a taste call tracked
  in the overview, out of scope here.

Group B (content sniffing) rides along because it touches the same dispatch
surface (`get_mime_type`) and multiplies the value of every native backend and
every new type in the sibling docs.

## Design principle: native-first, shell-out fallback

Introduce a per-arm helper that tries the native path and, only on failure,
falls back to the current external command. The `Preview` enum
(preview.rs:26) is unchanged — backends still yield `Preview::Text { lines }`
or `Preview::Image { .. }`.

Shape every converted arm like:

```rust
fn zip_list(path: &Path) -> Preview {
    match native_zip_list(path) {           // pure Rust
        Ok(lines) => Preview::Text { lines },
        Err(e) => {
            log::debug!("native zip list failed, trying unzip: {e}");
            cmd_to_preview("unzip", unzip_list(path))  // today's shell-out
        }
    }
}
```

The fallback keeps the existing error-text behaviour when *both* the crate and
the binary fail, so users on a weird archive are no worse off than today.

## Per-arm replacements (Group A)

All line-based previews keep the existing **128-line cap** (`take(128)`) and the
`\r`/`\n` scrubbing convention from `bat_preview`.

### A1 — Image info lines (drop mediainfo for images)
Dispatch arm: `("image", _)` (preview.rs:226).
Today `image_preview` decodes with the `image` crate *and* separately runs
`mediainfo` just for the info-line footer. The decode already yields everything
users read there:
- dimensions + color type — from the decoded `DynamicImage`
  (`img.width()/height()`, `img.color()`).
- file size + mtime — from `path.metadata()` (already fetched in
  `FilePreview::new`).
- format — from the MIME subtype we dispatched on.

Build the `info: Vec<String>` from those, delete the `mediainfo(&path)` call for
images. No new dependency. This alone removes one shell-out per image visit.

### A2 — zip listing via the `zip` crate
Dispatch arm: `("application", "zip")` (preview.rs:232).
`native_zip_list`: open with `zip::ZipArchive`, iterate `.file_names()` (or
`by_index` for sizes), format `name` + human size, cap at 128. Drops `unzip`.
The same crate can later take over extraction in `opener.rs:344` (out of scope
here, note it in the extraction code).

### A3 — tar / tar.gz listing via `tar` + `flate2`
Dispatch arms: `("application", "x-tar")` and `("application", "gzip")`
(preview.rs:230–231).
`native_tar_list`: wrap the file in `flate2::read::GzDecoder` when gzip-typed
(else read raw), feed `tar::Archive`, iterate `entries()` → path + size + mode.
Drops `tar` for previews and removes the kill-before-reap zombie dance
(`tar_list`, preview.rs:546–575) entirely.

**Fix the mis-dispatch while here:** `("application", "gzip")` currently always
goes to `tar --list`, so a plain `foo.txt.gz` (gzip of a *non*-tar file)
mis-previews. Detect the tar magic after decompression; if it is not a tar,
fall back to showing the decompressed head as text (reuse the plain-read path).

### A4 — Certificate via `x509-parser`
Dispatch arm: `("application", "x-x509-ca-cert")` (preview.rs:229).
Parse PEM/DER, print Subject, Issuer, Validity (not-before/not-after), SANs,
serial, signature algorithm — the fields people actually inspect. Drops
`openssl`. Keep the current `bat_preview` fallback (already the fallback at
preview.rs:483) for parse failures.

### A5 — Audio metadata via `lofty`
Dispatch arm: `("audio", _)` (preview.rs:227).
`lofty::read_from_path` → format, duration, bitrate/sample-rate/channels, and
common tags (title/artist/album/year). Format into `Preview::Text`. Drops
`mediainfo` for audio and replaces the raw mediainfo dump with a curated block.

### A6 — Generic `application/*` native stat-block
Dispatch arm: the catch-all `("application", _)` (preview.rs:254).
Today this runs `mediainfo` and usually prints boilerplate. Replace with a
dependency-free stat block: absolute path, size, mtime, MIME type, permissions
(reuse `unix_mode`, already a dependency). `mediainfo` stays only as the
fallback for types where it genuinely adds value (kept via the shell-out
fallback wrapper).

### Net dependency change
- **Added crates:** `zip`, `tar`, `flate2`, `x509-parser`, `lofty`.
- **Removed from the preview path:** `unzip`, `openssl`, and `mediainfo` for
  image/audio/generic. `mediainfo` remains only as a fallback; `ffmpeg` and
  `bat` unchanged.

## Content sniffing (Group B)

`get_mime_type` (`src/engine/opener.rs:22`) is extension-only (`mime_guess` +
a hand-rolled special-case list). An extensionless script or a mislabeled file
falls into binary-bat mode.

Add a **content sniff that runs only when the extension is unhelpful** — i.e.
when `mime_guess` returns `application/octet-stream` or `text/plain` with no
extension, or there is no extension at all. Order:

1. Existing special-case extension list (unchanged, highest priority — it exists
   precisely to override `mime_guess`).
2. `mime_guess` by extension.
3. **New:** if the result is the generic fallback, read the first ~256 bytes and
   sniff:
   - shebang (`#!`) or valid-UTF-8-and-mostly-printable → `text/plain`.
   - magic numbers via the `infer` crate (or a small hand-rolled table:
     `PK\x03\x04` zip, `%PDF` pdf, `\x1f\x8b` gzip, `SQLite format 3\0`, ELF,
     PNG/JPEG/GIF, `ustar`) → the real MIME type.

Keep it cheap: one bounded read, only on the fallback path, so the common
extension-hit case pays nothing. This is what makes the sibling docs' new types
(PDF, SQLite, Office) route reliably even when the extension lies or is missing.

`get_mime_type` is also used by the **opener** (opener.rs:183), so better
detection improves open-routing too — verify no opener test asserts the old
extensionless-file behaviour.

## Config

None. Native-first with shell-out fallback is strictly better than today and
needs no switch. (The persistent-cache switch and future bat/mediainfo toggles
live in their own docs.)

## Error handling

- Every native backend is best-effort: on `Err`, log at `debug` and fall back to
  the existing shell-out, which itself falls back to error text. A preview is
  never lost — at worst it matches today's behaviour.
- The content sniff never fails the caller: any read error → skip the sniff,
  return the `mime_guess` result.

## Testing

Follow the house pattern (`external_cmd_tests` in preview.rs): terminal-free unit
tests over `tempfile` fixtures, building real fixtures with the crates under
test.

- **A2/A3 archives:** build a fixture zip/tar/tar.gz, assert the native lister
  returns the expected names; assert the 128-line cap; assert a `foo.txt.gz`
  (non-tar gzip) falls back to text instead of erroring.
- **A4 cert:** generate a self-signed DER/PEM fixture, assert Subject/Validity
  lines appear.
- **A5 audio:** commit a tiny tagged fixture (or synthesize with `lofty`),
  assert title/duration lines.
- **A1 image:** assert info lines are derived without any `mediainfo` presence
  (test passes with mediainfo uninstalled).
- **Sniffing:** extensionless shebang script → `text/plain`; extensionless PDF
  bytes → `application/pdf`; a `.txt` file containing zip magic still trusts the
  extension (sniff only runs on the generic fallback).
- **Fallback:** point a backend at a corrupt archive and assert it degrades to
  the shell-out path (guarded so it is skipped when the binary is absent, like
  the existing ffmpeg/tar tests).

## Out of scope

- Replacing **bat** (`syntect`) and **ffmpeg** — kept by design.
- zip/tar **extraction** in `opener.rs` — same crates could take it over later;
  only note it in the extraction code.
- New preview *types* (SVG, PDF, SQLite, Office, fonts) — sibling doc
  `…-preview-new-types-design.md` and `…-preview-pdf-design.md`.
- Async/timeout hardening of the remaining ffmpeg shell-out — Group D doc.
