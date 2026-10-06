# Preview Polish Items (D1–D4) — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes.

**Goal:** the four Group-D polish items on the image/video preview path:

- **D4** — a shared bounded-external-process helper (spawn + wait-with-
  deadline + kill-then-reap + partial-output cleanup) applied to
  `ffmpeg_thumbnail` and `pdf_render_first_page_in`.
- **D2** — `image` 0.24.9 → 0.25 (pinned, feature-trimmed) + JPEG XL decode
  via `jxl-oxide`. **AVIF and HEIC are explicitly skipped** — see the
  scoping decision below; the design sanctions this call.
- **D1** — EXIF orientation applied before thumbnail+store, so cached
  rasters are upright. Implemented via image 0.25's **native orientation
  API** (zero new deps) — a deliberate, evidence-backed deviation from the
  design's kamadak-exif route; kamadak-exif stays documented as the
  verified fallback.
- **D3** — short-video thumbnails via ffmpeg's `thumbnail` filter (the
  design's preferred, probe-free option); the existing
  `ffmpeg_thumbnail_of_a_too_short_video_is_an_error` test flips to expect
  success, deliberately.

**Design doc:** `docs/plans/2026-07-30-preview-polish-items-design.md`.
Shared context: `src/panel/preview.rs` (`native_image_preview` ~535,
`cached_image_preview_in` ~590, `video_preview` ~946, `ffmpeg_thumbnail`
~1028, `pdf_render_first_page_in` ~2230, `tar_list` ~2510 for the
kill-then-reap discipline) and `src/panel/raster_cache.rs`.

**Task order** (dependencies, not the design's numbering):
Task 1 = D4 (independent; lands first so D3's new ffmpeg command is born
bounded). Task 2 = D2a (image 0.25 bump — D1 builds on its API).
Task 3 = D1 (needs 0.25's `into_decoder`/`apply_orientation`).
Task 4 = D2b (JXL — needs 0.25 for jxl-oxide's `image` integration).
Task 5 = D3. Task 6 = docs. One commit per task, trailer
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## Tech stack — verified empirically on rustc/cargo 1.83.0 (2026-07-31)

All of this was proven by editing *this* tree's Cargo.toml, building, and
running the image/raster-cache test filters (31 tests green), then
reverting. Crate metadata lies; only these observed results count.

**`image`, pin `>=0.25.5, <0.25.7`, `default-features = false`, features
`["jpeg", "png", "gif", "webp", "bmp", "ico", "tiff", "pnm", "tga", "qoi",
"hdr", "exr", "ff", "dds"]`** (= `default-formats` minus `avif`, minus the
`rayon` feature):

- Naive `image = "0.25"` FAILS: the resolver picks 0.25.10 (requires Rust
  1.88) and drags `ravif 0.13` (1.85), `tiff 0.11.3` (1.85) and the
  edition2024 `aligned 0.4.3` in — cargo 1.83 cannot even parse that
  manifest. 0.25.8 fails too (`moxcms` edition2024). **0.25.7 does not
  exist** on crates.io. 0.25.5 and 0.25.6 both compile clean with the
  existing dep tree (tiff stays 0.9.1, zune-jpeg 0.4.21 replaces
  jpeg-decoder for the jpeg feature, gif 0.13.1/exr 1.73/png 0.17.16
  unchanged). The `>=0.25.5` floor is load-bearing: jxl-oxide 0.11's
  `image` integration requires image ≥ 0.25.5.
- **The `rayon` feature must stay OFF — empirically load-bearing.** image
  0.25's `rayon = ["dep:rayon", "ravif?/threading"]`: on cargo 1.83 that
  weak-dep reference is enough to pull `ravif 0.11.20` + `avif-serialize
  0.8.9` (edition2024) into resolution, and the build dies at manifest
  parse ("feature `edition2024` is required") even though no avif feature
  is enabled. Verified by adding features one at a time: every format
  feature is clean, `rayon` alone breaks it. Cost: image's internal
  parallel decode paths go single-threaded — fine for one ≤960×540
  thumbnail per preview (and jxl-oxide brings its own rayon for JXL).
- `image::io::Reader` still exists in 0.25.5/6 as a **deprecated alias**
  of `image::ImageReader` — the bump compiles before any migration (4
  deprecation warnings, enumerated in Task 2). `JpegEncoder::
  encode_image(&img.to_rgb8())` (raster_cache.rs:122), `into_dimensions`,
  `with_guessed_format`, `image::ColorType`, `DynamicImage::thumbnail`,
  `RgbImage`/`Rgb` (graphics.rs, sixel.rs) all compile unchanged.
- New in 0.25 and used by D1: `ImageReader::into_decoder()`,
  `ImageDecoder::orientation()` (JPEG/TIFF/WebP decoders parse the
  embedded EXIF chunk themselves; every other decoder returns
  `NoTransforms` via the default trait impl — the "skip formats without
  EXIF cheaply" and "bounded EXIF read" requirements are satisfied by
  construction), `image::metadata::Orientation` (`from_exif(1..=8)`),
  `DynamicImage::apply_orientation`. Proven end-to-end in a scratch
  crate: a JPEG with a spliced EXIF APP1 orientation=6 reports
  `Rotate90`, raw dims 20×10 → upright 10×20.

**`jxl-oxide`, pin `>=0.11, <0.12`, `features = ["image"]`** (default
`rayon` feature stays on):

- 0.12.0+ and 1.x FAIL on cargo 1.83 (`jxl-coding 1.0.1` is edition2024).
  0.11.4 compiles clean. Dep tree is pure Rust (brotli-decompressor,
  jxl-*, rayon, tracing, bytemuck) — no system C libraries.
- The `image` feature provides `jxl_oxide::integration::JxlDecoder<R>`
  implementing `image::ImageDecoder` (requires image ≥ 0.25.5, hence the
  floor above) → `DynamicImage::from_decoder` slots JXL into the existing
  raster path. Verified compiling against this tree with the final pins.

**kamadak-exif `0.6` (= 0.6.1): verified compiling on 1.83** (single tiny
dep, `mutate_once`), and `Reader::read_from_container` reads the spliced
orientation=6 fixture correctly. **NOT added by this plan** — D1 uses the
image-native API instead (see scoping) — but this is the verified pin if
review prefers the design's route: `kamadak-exif = "0.6"`, wrap the input
in `.take(1024 * 1024)` for the bounded read.

**Lockfile discipline:** never regenerate Cargo.lock from scratch — it
carries pins the manifest cannot express (uuid 1.12.1, ogg_pager 0.7.0, …).
Edit Cargo.toml, then let a plain `cargo check` update the lock minimally.

## Scoping decisions (deviations, all sanctioned by the design/caller)

1. **AVIF decode: SKIPPED.** image 0.25's `avif` feature is *encoder-only*
   (ravif + rgb, and ravif ≥0.13 needs Rust 1.85 anyway); decode is
   `avif-native` = `dav1d`, a **system C library binding** — categorically
   excluded. There is no drop-in pure-Rust AVIF decode: `rav1d` is a full
   AV1-decoder porting project with no image-crate integration (container
   parse + YUV conversion would be ours). AVIF files keep today's
   behavior: decode fails → mediainfo/info fallback panel.
2. **HEIC: SKIPPED** (design: best-effort only) — every decoder crate
   wraps system libheif. The existing
   `native_image_preview_of_an_undecodable_image_falls_back_to_text` test
   (a fake .heic) keeps guarding the fallback.
3. **D1 via image-native orientation instead of kamadak-exif.** Rationale:
   zero new dependencies, the orientation comes from the decoder that is
   already parsing the file (no second open/scan), and coverage is
   identical in practice — JPEG/TIFF/WebP are exactly the EXIF-carrying
   formats image can decode (HEIF isn't decodable at all, and JXL carries
   orientation in its own header, which jxl-oxide applies itself during
   decode). The kamadak-exif route stays documented above, MSRV-verified,
   as the fallback.
4. **D3 keeps cache kind `vid120`** (task requirement): `scale=120:-1`
   output semantics are unchanged; only the *frame choice* changes
   (representative first-batch frame instead of the 10s seek). A cached
   old-style frame is still an honest frame of the same video+mtime — not
   worth invalidating every user's cache. Deliberate call; if scale ever
   changes, THAT bumps the kind.

---

## Task 1 — D4: bounded external-process helper, adopted by ffmpeg + PDF

`FilePreview::new` runs inside `spawn_blocking` (content.rs:404), so a
*blocking* std::process helper is the right shape (the design offers both;
tokio::process would drag async through pure code and out of unit tests).

**1.1 (red)** In `preview.rs`'s `external_cmd_tests` module, add helper
tests (all `sh`-based, no ffmpeg needed):
- `run_bounded_kills_a_child_that_outlives_the_deadline`: `sh -c "sleep 30"`
  with a 200ms deadline returns `Err` and the call returns within a bounded
  margin (assert elapsed < 2s — generous; the point is "not 30s").
- `run_bounded_returns_output_of_a_fast_child`: `sh -c "echo out; echo err
  1>&2"` returns `Ok`, `status.success()`, stdout `out\n`, stderr `err\n`.
- `run_bounded_does_not_deadlock_on_a_stderr_flood`: a child writing
  ≥200 KiB to stderr (`head -c 200000 /dev/zero | tr '\0' x 1>&2`) —
  larger than the 64 KiB pipe buffer — still completes `Ok`. This pins the
  reader-thread requirement: a naive try_wait poll without draining would
  deadlock here.
- `run_bounded_surfaces_a_nonzero_exit`: `sh -c "exit 3"` → `Ok` with
  `!status.success()` (policy: non-zero exit is the *caller's* domain —
  both call sites already branch on `out.status.success()`).

**1.2 (green)** Implement in preview.rs:

```rust
/// Deadline for every external raster producer (ffmpeg, pdftoppm/mutool).
const EXTERNAL_RENDER_DEADLINE: Duration = Duration::from_secs(10);

fn run_bounded(cmd: &mut std::process::Command, deadline: Duration)
    -> io::Result<std::process::Output>
```

Spawn with stdout+stderr piped (callers already set stdin null); move each
pipe into a drain thread (`read_to_end` into a Vec — both producers emit
small output, ffmpeg's stderr chatter is the only real volume and stays in
memory only until the 3-line tail is taken). Main thread: `try_wait()`
poll loop (~25ms sleep) against `Instant::now() + deadline`. On deadline:
`kill()` then `wait()` — the kill-then-reap discipline from `tar_list`
(preview.rs:2527): kill fails harmlessly if the child just exited, the
wait prevents a zombie; join the drain threads (EOF arrives when the
killed child's pipes close) and return `Err(TimedOut)` naming the deadline.
On exit: join threads, assemble `Output { status, stdout, stderr }`.

**1.3 (adopt)** Replace `cmd.output()?` at the two call sites:
- `ffmpeg_thumbnail` (preview.rs:1063) and
- `pdf_render_first_page_in` (preview.rs:2280),
with `run_bounded(&mut cmd, EXTERNAL_RENDER_DEADLINE)`. **Timeout must
clean the part file**: today's `let out = cmd.output()?;` early-return
skips the `remove_file(&part)` cleanup — a killed ffmpeg/renderer leaves a
half-written `.part`. Restructure both sites so the `Err` path (spawn
failure *and* timeout) removes `part` before propagating; the existing
`!success || !part.exists()` branch keeps its cleanup. The `Err` falls to
the existing fallbacks (mediainfo / pdf text tier) — no new user-facing
error surface.
Out of scope (per design): mediainfo/bat/tar and the `-v`/`-h` presence
probes stay unbounded.

**1.4** `cargo test` (the ffmpeg-guarded integration tests still pass
within the deadline), `cargo clippy`. Commit:
`feat(preview): bound external raster producers with a kill-then-reap deadline`.

## Task 2 — D2a: image 0.25 bump + call-site migration

**2.1** Cargo.toml: replace `image = "0.24.9"` with the exact verified
spec from the tech-stack section (pin, default-features off, 14 format
features, NO rayon). `cargo check` — expect success and exactly 4
deprecation warnings.

**2.2** Migrate the deprecated alias `image::io::Reader` →
`image::ImageReader` at all four call sites (enumerated by the compiler,
verified by experiment):
- preview.rs:541 `native_image_preview` (decode)
- preview.rs:624 `cached_image_info` (`into_dimensions`)
- preview.rs:2292 `pdf_render_first_page_in` (`with_guessed_format` part
  decode)
- raster_cache.rs:93 `lookup_in`
No other API churn: resize/thumbnail paths (`thumbnail(960, 540)`,
`resized_rgb`, FilePreview's cell-keyed resize cache), `JpegEncoder::
new_with_quality(...).encode_image(&img.to_rgb8())`, `image::ColorType`,
and the graphics emitters' `RgbImage` use compile unchanged on 0.25.6
(empirically checked, tests green). Update the raster_cache comment
"JPEG in image 0.24 rejects RGBA" → still true in 0.25; say "0.24/0.25".

**2.3** Zero-warning build, then the behavioral net: `cargo test` — full
suite, not just the image filters (the JPEG backend swaps to zune-jpeg;
the 31 image/raster-cache tests were verified green under 0.25.6,
including the upscale-guard test `thumbnail()` semantics). Inspect
`git diff Cargo.lock`: expect image/zune additions and NO churn of the
comment-documented pins (uuid, ogg_pager, …).

**2.4** Commit: `feat(preview): image 0.25 (pinned for MSRV 1.83, avif-less)`.

## Task 3 — D1: EXIF orientation before thumbnail + store

**3.1 (red)** Test fixture helper in the native_backend_tests module: 
`jpeg_with_orientation(dir, name, w, h, orientation: u16) -> PathBuf` —
encode an RgbImage to JPEG in memory, splice a minimal EXIF APP1 right
after SOI (FFD8): `FFE1 <len> "Exif\0\0"` + little-endian TIFF header +
one-entry IFD0 (tag 0x0112, SHORT, value) — ~30 bytes, proven working
against image 0.25.6's jpeg decoder. Tests:
- `native_image_preview_applies_exif_orientation`: 20×10 with
  orientation=6 → `Preview::Image` raster is 10×20 (transposed), and the
  info line reports the **upright** dimensions.
- `native_image_preview_orientation_1_is_untouched`: dims stay 20×10.
- `native_image_preview_flips_mirrored_orientations`: 2×1 two-color image
  with orientation=2 (fliph) → pixel order swapped.
- `a_png_without_exif_is_untouched`: existing PNG path, dims unchanged
  (guards the cheap-skip).
- `cached_image_preview_stores_the_upright_raster`: run
  `cached_image_preview_in` (tempdir) on the orientation=6 fixture; then
  assert `raster_cache::lookup_in(...)` decodes to 10×20 — **the cached
  raster itself is upright** (the design's caveat: correct by
  construction, hit path must not re-apply), and a second call (the hit
  path) still reports 10×20 with upright info dims.

**3.2 (green)** In `native_image_preview`: replace the
`ImageReader::open(path).ok().and_then(|r| r.decode().ok())` chain with
open → `into_decoder()` → `orientation()` (on `Err` use
`Orientation::NoTransforms`) → `DynamicImage::from_decoder` →
`apply_orientation` — *then* the existing ≤960×540 bound/thumbnail and
info-line derivation (which thereby report upright dims), *then* the
caller's `store_in`. Note: `orientation()` must be called before
`from_decoder` consumes the decoder (verified).

**3.3** Hit-path info consistency: `cached_image_info` currently reads raw
header dimensions — for a transposed source it would disagree with the
miss path. Change it to `into_decoder()` + `decoder.dimensions()` +
`orientation()`, swapping w/h for the transposing values (Rotate90/270 ±
flip); fall back to the cached raster's dims as today.

**3.4** Nothing else changes: SVG/font/PDF rasters have no EXIF; the video
thumbnail is ffmpeg-produced (ffmpeg auto-applies rotation metadata);
JXL (Task 4) is oriented by jxl-oxide itself. `cargo test`, clippy.
Commit: `feat(preview): upright EXIF-oriented image previews (and cache)`.

## Task 4 — D2b: JPEG XL decode via jxl-oxide

**4.1** Cargo.toml: add the verified `jxl-oxide` spec (tech-stack
section). 

**4.2 (red)** Tests:
- Routing: `get_mime_type` on a `.jxl` path returns `image/jxl`
  (mime_guess 2.0.5 does not know jxl — add `Some("jxl") =>
  "image/jxl"` to the special-cased extension table in
  `src/engine/opener.rs`, the documented pattern for exactly this case).
- Decode: a tiny JXL fixture decodes to a non-empty raster. Fixture:
  encode at test time is not possible (jxl-oxide is decode-only) — embed a
  minimal hand-checked `.jxl` (a few hundred bytes, e.g. 8×8 solid color
  produced once with `cjxl`) via `include_bytes!` under
  `src/panel/testdata/` — the repo's existing binary-fixture convention
  (`testdata/subset-noto-sans.ttf`, preview.rs:3562); `#[ignore]`-free
  since decode is pure Rust. Assert `FilePreview::new` yields
  `Preview::Image` with `img: Some(_)` and correct dims.
- Fallback intact: a file named `broken.jxl` with garbage bytes falls to
  the text/info fallback (no panic, no bare error).

**4.3 (green)** In `native_image_preview`, factor the decode into a small
`decode_raster(path, mime)`: for `("image", "jxl")` build
`jxl_oxide::integration::JxlDecoder::new(BufReader<File>)` →
`DynamicImage::from_decoder` (orientation: applied by jxl-oxide
internally, do NOT also apply EXIF); everything else takes the Task-3
ImageReader path. Dispatch: `.jxl` then flows through the *existing*
`("image", _)` arm → `cached_image_preview` → store/hit under `img960`
like every other bitmap (raster-cache contract untouched). Decode failure
falls to the existing mediainfo/"Failed to load" path (bounded-input
posture matches the other image decoders').

**4.4** `cargo test`, clippy. Commit:
`feat(preview): decode JPEG XL natively via jxl-oxide`.

## Task 5 — D3: short-video thumbnails via the `thumbnail` filter

**5.1 (red — deliberate test inversion)** Rewrite
`ffmpeg_thumbnail_of_a_too_short_video_is_an_error` (preview.rs:3332) as
`ffmpeg_thumbnail_of_a_short_video_produces_a_thumbnail`: the 1-second
`make_video` fixture now yields `Ok(Preview::Image { img: Some(_), .. })`,
the `vid120` entry lands in the dir, and no `.part` siblings remain
(reuse `thumbnail_dir_entries_of`). This inversion is the point of D3 —
the old test enshrined the 10s-seek defect. Keep a genuine-failure test:
new `ffmpeg_thumbnail_of_an_invalid_video_is_an_error` — write garbage
bytes as `corrupt.mp4`, assert `Err` and an empty thumbnail dir (the
part-cleanup assertion the old test carried). Both guarded on ffmpeg's
presence like the existing video tests.

**5.2 (green)** In `ffmpeg_thumbnail`: drop `-ss 00:00:10`; the filter
becomes `-vf "thumbnail,scale=120:-1"` with `-frames:v 1` (modern spelling
of `-vframes 1`), keeping `-y`, `-q:v 2`, the `.part.jpg`-suffixed temp
name (extension LAST for container inference), rename, stale-sweep,
corrupt-entry re-lookup — the whole raster-cache discipline is untouched,
kind stays `vid120` (scoping decision 4). Verified on this machine's
ffmpeg: a 1s clip produces a frame (today: error), a 15s clip renders in
~0.05s — the filter examines only the first frame batch (default 100
frames), so cost is bounded without any duration probe; Task 1's deadline
bounds pathological inputs on top. Update the now-wrong comments: the
"videos shorter than the 10s seek" rationale in `ffmpeg_thumbnail`'s
error branch and in `video_preview`'s fallback comment (short videos
succeed now; the fallback remains for corrupt files/timeouts).

**5.3** `cargo test`, clippy. Commit:
`fix(preview): representative thumbnails for short (and all) videos`.

## Task 6 — docs + interactive verification

**6.1** CLAUDE.md "Architecture: native preview backends": one tight
paragraph — image 0.25 pin + why image/rayon must stay off (weak-dep
ravif/edition2024 trap), jxl-oxide pin (JXL, pure Rust), orientation
applied pre-store (cached rasters upright), AVIF/HEIC skipped (dav1d/
libheif are system C), ffmpeg `thumbnail` filter + the
`EXTERNAL_RENDER_DEADLINE` bound.

**6.2** Interactive smoke test per CLAUDE.md (tmux + debug socket,
`XDG_CACHE_HOME=$(mktemp -d)` exported in the pane): fixture dir with a
rotated phone JPEG (or the spliced fixture), a `.jxl`, a 3s video (name
one fixture "Bilder & Videos/clip 1.mp4" — the shell-escape convention);
navigate onto each, `await-idle`, then poll `state` until `seq`
stabilizes (preview loads are async and outrun await-idle), grep the
socket `log` for `raster cache hit`/`generating thumbnail` and the
absence of "falling back", and `capture-pane` for the drawn previews.
Assert the cache dir holds upright `img960` + `vid120` entries and no
`.part` files. Teardown incl. socket file.

**6.3** `cargo build && cargo test && cargo clippy` one last time.
Commit (docs): `docs: preview polish notes in CLAUDE.md`.
