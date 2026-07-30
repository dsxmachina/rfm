# Preview Polish Items — Design

Status: proposed, 2026-07-30
The small enhancements from Group D of the preview overhaul
(`feature-better-previews.md`) that don't warrant a full design doc each. Every
item is a self-contained, independent change to the existing image/video preview
path — deliberately sparse; each is meant to be handed to an implementation
session on its own. None depend on the other preview docs (though D3/D4 pair
naturally with the video work, and the persistent-cache item is already covered
by its own doc `2026-07-30-thumbnail-cache-design.md` and is *not* repeated
here).

Shared context: image previews are produced by `image_preview`
(`src/panel/preview.rs:305`), video thumbnails by `ffmpeg_thumbnail`
(preview.rs:367), both drawn by `FilePreview::draw` (preview.rs:82).

---

## D1 — EXIF orientation

**Problem.** The `image` crate decode in `image_preview` (preview.rs:307)
ignores the EXIF orientation tag, so photos shot in portrait on a phone preview
rotated/mirrored.

**Change.** After decoding, read the orientation tag and apply the matching
rotate/flip before thumbnailing:
- Read with `kamadak-exif` (`exif::Reader` over the file), get
  `Tag::Orientation` (values 1–8).
- Map to `DynamicImage` ops (`rotate90`/`rotate180`/`rotate270` + `fliph`/`flipv`
  for the mirrored cases). Value 1 (or absent/unreadable) → no-op.
- Apply *before* `.thumbnail(960, 540)` so the cached/resized raster is already
  upright and the draw path needs no change.

**Dependency.** Add `kamadak-exif`. Only JPEG/TIFF/HEIF carry orientation; skip
formats without EXIF cheaply.

**Testing.** Unit, terminal-free: a fixture JPEG with orientation=6 decodes to a
raster whose dimensions are transposed vs the raw decode; orientation=1 is
unchanged; a PNG (no EXIF) is untouched.

**Caveat.** If the persistent raster cache (`…-thumbnail-cache-design.md`) is in
play, the cached thumbnail is the *corrected* one — correct by construction,
but note it so no one double-applies rotation on a cache hit.

---

## D2 — Modern image formats (AVIF / HEIC / JXL)

**Problem.** `image = "0.24"` (Cargo.toml:25) can't decode AVIF/HEIC/JXL, so
those show "Failed to load image" (preview.rs:176).

**Change.**
- Bump `image` to `0.25` and enable its `avif`/`avif-native` decode feature.
- HEIC and JXL are not fully covered even by 0.25: add `jxl-oxide` for JPEG XL,
  and gate HEIC behind an optional decoder (`libheif`-based crates pull a system
  dep — treat HEIC as best-effort / feature-flagged, not a hard requirement).
- The dispatch arm (`("image", _)`, preview.rs:226) is unchanged; only the
  decoder coverage widens. Decode failures still fall to the existing
  `img: None` "Failed to load" panel.

**Dependency.** `image` 0.25 (a churn bump — check the `image::io::Reader`
API used at preview.rs:307 for breaking changes), `jxl-oxide`.

**Testing.** Unit: tiny AVIF and JXL fixtures decode to a non-empty raster;
verify the 0.25 bump doesn't break the existing decode/thumbnail test path.

**Note.** This is a chunky compile-time/binary-size bump for a long-tail of
formats — worth flagging to the maintainer as a cost/benefit call before doing
it.

---

## D3 — Short-video thumbnails

**Problem.** `ffmpeg_thumbnail` hardcodes a 10-second seek
(`-ss 00:00:10`, preview.rs:392). Clips shorter than 10s produce no frame, so
every short video silently degrades to the mediainfo text fallback
(preview.rs:350–364).

**Change.** Pick a seek point inside the clip:
- Probe duration first (`ffprobe -show_entries format=duration`, or ffmpeg's
  `-show_format`), seek to ~10% or `min(10s, duration/2)`. Or,
- Simpler and probe-free: use ffmpeg's `thumbnail` video filter
  (`-vf "thumbnail,scale=120:-1"`) which picks a representative frame without a
  seek, sidestepping duration entirely. **Prefer this** — one fewer process and
  it also improves the frame chosen for long videos.
- Keep the current 10s-seek path only if the `thumbnail` filter proves slower on
  large files (it decodes a window of frames).

**Dependency.** None (ffmpeg already required for this path).

**Testing.** Extend `external_cmd_tests`: a <10s fixture video
(`make_video(..., 1)`) now yields a thumbnail instead of `Err`
(inverts the current `ffmpeg_thumbnail_of_a_too_short_video_is_an_error` test at
preview.rs:790 — update that test's expectation deliberately). Guarded to skip
when ffmpeg is absent, like the existing video tests.

---

## D4 — ffmpeg spawn-with-timeout hardening

**Problem.** `ffmpeg_thumbnail` calls `cmd.output()` (preview.rs:405), a
*blocking* wait on the panel task. A pathological or hung ffmpeg (corrupt file,
network mount stall) blocks that preview slot indefinitely.

**Change.** Bound the ffmpeg run:
- Spawn instead of `output()`, wait with a timeout (e.g. 10s). On timeout, kill
  the child (mirror the kill-then-reap discipline already in `tar_list`,
  preview.rs:567), delete any partial output file (the code already removes
  partial thumbnails on failure, preview.rs:414), and return `Err` → caller
  falls back to mediainfo/text.
- Since the preview path is already inside an async task, `tokio::process` +
  `tokio::time::timeout` is the clean fit; if staying on `std::process`, a
  wait-with-deadline loop or a helper thread works too.
- Apply the same treatment to the optional `pdftoppm` render if the PDF image
  tier lands (`…-preview-pdf-design.md`), and consider it for any future
  external raster producer — they share this failure mode.

**Dependency.** None.

**Testing.** Hard to force a real hang deterministically; unit-test the
timeout helper in isolation (a sleep longer than the deadline is killed and
returns `Err` within a bounded margin) rather than driving ffmpeg. Integration:
existing video previews still succeed within the timeout.

---

## Config

None of D1–D4 add config. (D2's HEIC decoder, if pursued, may be a Cargo
*feature* rather than a runtime switch.)

## Out of scope

- The persistent thumbnail cache — already designed in
  `2026-07-30-thumbnail-cache-design.md`; listed under Group D in the overview
  but not re-specified here.
- Anything touching the *draw* path (graphics protocols) — Group E doc.
- New file-type coverage — native-backends / new-types / PDF docs.
