# Better previews — overview, options, and a plan

Status: exploratory notes, uncommitted. Captures the current preview
machinery, where external binaries could be dropped, what previews could be
added or enhanced, and how to group the work.

## How preview generation works today

`FilePreview::new` (`src/panel/preview.rs:216`) guesses the MIME type from the
file extension, then dispatches on `(type, subtype)`. Everything runs
**synchronously via `std::process::Command`** inside the async panel-load task.
The result is either `Preview::Text { lines }` (max 128 lines, ANSI colors
preserved) or `Preview::Image` (decoded via the `image` crate, pre-shrunk to
960×540, rendered as truecolor half-blocks `▄`, with a resize cache keyed on
cell dimensions).

MIME detection lives in `get_mime_type` (`src/engine/opener.rs:22`): a
hand-rolled special-case list for a few extensions, otherwise `mime_guess`
(extension-based only — no content sniffing).

### Current matrix

| File type | Method | External binary | Fallback if missing |
|---|---|---|---|
| Directory | `DirPanel` listing | none | — |
| `image/*` | `image` crate decode + thumbnail | **mediainfo** (info lines only) | empty info lines |
| `video/*` | frame grab at 10s → jpg, cached in `/tmp/rfm-thumbnails` (7-day prune) | **ffmpeg** + **mediainfo** | mediainfo text; nothing if that's missing too |
| `audio/*` | metadata dump | **mediainfo** | error text telling user to install it |
| `text/*`, textish `application/*` (json, xml, sh, js, sql, …) | syntax-highlighted head | **bat** | plain `BufReader` line read (built-in ✓) |
| `application/octet-stream`, msgpack, unknown | `bat --show-all` (binary) | **bat** | plain line read |
| `application/zip` | `unzip -l` | **unzip** | error text |
| `application/x-tar`, `gzip` | `tar --list` (capped 64 lines, process killed after) | **tar** | error text |
| `application/x-x509-ca-cert` | `openssl x509 -text -noout` | **openssl** | bat |
| any other `application/*` | metadata dump | **mediainfo** | error text |

External roster: **ffmpeg, mediainfo, bat, tar, unzip, openssl** — `ffmpeg` and
`openssl` are presence-checked once (`OnceCell`); the others just fail per-call.

## Where the external dependency could be dropped

Roughly in order of effort-to-payoff:

1. **zip listing → `zip` crate** (pure Rust). Trivial: open archive, iterate
   names + sizes. Kills the `unzip` dependency for previews entirely (extraction
   in `opener.rs` still uses it, but the same crate could take that over too).
2. **tar / tar.gz listing → `tar` + `flate2` crates** (pure Rust). Also easy,
   and *better* than today: `tar --list` on `.gz` relies on the system tar's
   decompressor; the crate version needs nothing. Bonus: show sizes/permissions,
   not just names, and drop the kill-before-reap zombie dance
   (`preview.rs:567`).
3. **Certificate preview → `x509-parser` crate**. Subject, issuer, SANs,
   validity — covers what people actually look at, drops `openssl`.
4. **Audio metadata → `lofty`** (pure Rust: ID3, Vorbis comments, FLAC, MP4 tags
   + duration/bitrate). Replaces `mediainfo` for audio with arguably nicer
   output (you control formatting).
5. **Image info lines**: currently from `mediainfo`, but the `image` crate
   already decoded the file — dimensions and color type are free; file size and
   mtime come from `metadata()`. That's most of what mediainfo prints. Easy win,
   drops one shell-out per image visit.
6. **Text highlighting → `syntect`**: this is literally bat's engine, so output
   quality is identical. Chunky dependency (compile time, binary size) and the
   plain-read fallback already works, so this is a taste call — but it's the only
   way to get colored previews without bat installed.
7. **Video is the one place external is justified.** No realistic pure-Rust
   decode-any-codec story; `ffmpeg` stays. But the *info* lines could come from
   `symphonia`/`mp4parse`-style container parsing instead of `mediainfo`, which
   would shrink the fallback chain to "ffmpeg or nothing".
8. The generic `application/* → mediainfo` catch-all mostly produces boilerplate;
   a native stat-block (size, mtime, mime, permissions) would be as informative
   and dependency-free.

Doing 1–5 would leave **ffmpeg (video thumbs) and bat (nicer text)** as the only
externals, both optional with graceful fallbacks — much cleaner than today,
where audio/zip/other files show an "install X" error message.

## Previews you could add or enhance

**New types, no external dep needed:**
- **SVG** → `resvg` renders to a raster; currently svg hits the bat path and
  shows XML soup.
- **PDF** → `pdftoppm`/`mutool` externally for a page image; pure-Rust rendering
  is weak, but `pdf`/`lopdf` can extract title/page-count/first-page text with no
  binary. (Decide which tier you want.)
- **Office/OpenDocument (docx/xlsx/odt) and epub** — all zip+XML, so once the
  `zip` crate is in, a "strip XML, show text" preview is cheap. Currently they
  land in the mediainfo catch-all.
- **SQLite** → `rusqlite`: table names + row counts.
- **Fonts (ttf/otf)** → `ab_glyph`/`fontdue`: render a pangram sample into an
  image preview. Very showy, pure Rust.
- **More archives**: `.7z` (`sevenz-rust`), `.zst`/`.xz`/`.bz2` tarballs
  (`zstd`/`xz2`/`bzip2` crates feeding the `tar` crate). Today a `.tar.zst` only
  works if the system tar+zstd cooperate; plain `.gz` of a non-tar file actively
  mis-previews (it goes to `tar --list`).

**Enhancements to existing previews:**
- **EXIF orientation**: `image` ignores the orientation tag, so phone photos
  preview sideways. `kamadak-exif` + a rotate fixes it.
- **Modern image formats**: `image` 0.24 has no AVIF/HEIC/JXL decode. Bump to
  0.25 + feature flags (or `jxl-oxide`) extends coverage.
- **Short-video thumbnails**: the hardcoded `-ss 00:00:10` fails for clips under
  10s and silently degrades to mediainfo. Probing duration first (ffprobe, or
  ffmpeg's `thumbnail` filter) would give every video a frame.
- **Higher-fidelity image rendering**: half-blocks cap you at 2 pixels per cell.
  Emitting the kitty graphics protocol or sixel where supported needs *no* new
  dependency (just escape sequences) and is the single biggest visual upgrade.
- **Persistent thumbnail cache**: the recent design commit (`1750ebd`,
  XDG_CACHE_HOME) already points here — the ffmpeg cache lives in `temp_dir()`
  and dies on reboot; the same cache could then also serve PDF page renders and
  SVG rasters.
- **Latency**: every preview shell-out is a blocking `Command::output()` on the
  panel task; mediainfo is called up to twice per video. Native replacements fix
  most of this for free, but the ffmpeg call might deserve a spawn-with-timeout
  regardless.

Structural note: because dispatch is purely extension-based (`mime_guess`), an
extensionless script or a mislabeled file falls into binary-bat mode. A tiny
content sniff (first bytes: shebang → text, magic numbers → real type; `infer`
crate or hand-rolled) would make the whole table more robust with no external
tool.

## How to group this, and rough effort

Effort is calendar-feel for one developer already fluent in the codebase: **S**
= a few hours, **M** = a day-ish, **L** = multiple days.

### Group A — Native replacements for existing externals (dependency diet)
Self-contained, each is a swap behind the existing dispatch arm with the current
shell-out kept as fallback. Highest value: turns "install X or see an error" into
"always works." Do these first; they de-risk everything after.

- Image info lines from the decoded image + `metadata()` — **S**
- zip listing via `zip` crate — **S**
- tar/tar.gz listing via `tar` + `flate2` — **S/M** (handle the `.gz`-of-non-tar
  mis-dispatch while here)
- Certificate via `x509-parser` — **S**
- Audio metadata via `lofty` — **M**
- Generic `application/*` native stat-block — **S**

Group A total: **~M–L** (2–4 days). New deps: `zip`, `tar`, `flate2`,
`x509-parser`, `lofty`. Removes preview-time need for `unzip`, `openssl`,
`mediainfo` (audio/image/other). Ships incrementally — each arm is its own PR.

### Group B — Robust dispatch (foundation for everything new)
Small but multiplies the value of Group C. Worth doing right after A.

- Content sniffing (`infer` or hand-rolled magic bytes) feeding `get_mime_type`
  — **M**. Fixes extensionless/mislabeled files and unlocks reliable routing for
  the new types below.

### Group C — New preview types (breadth)
Each is independent and gated behind Group B's routing. Pick by what you
actually open. All pure-Rust.

- SVG via `resvg` (image preview) — **M**
- Office/ODT/epub text extraction (reuses `zip` from A) — **M**
- SQLite via `rusqlite` — **S/M**
- Fonts via `ab_glyph`/`fontdue` (image preview) — **M**
- PDF text-tier via `lopdf` — **M**; PDF image-tier via external `pdftoppm` — **M**
- Extra archive formats (`.7z`, `.zst`/`.xz`/`.bz2`) — **M**

Group C total: **~L** if you do all of it; naturally sliced one type per PR.

### Group D — Existing-preview enhancements (polish)
Independent of the rest; schedule opportunistically.

- EXIF orientation via `kamadak-exif` — **S/M**
- Modern image formats (bump `image` to 0.25 / add `jxl-oxide`) — **S** for the
  bump, plus format-testing tail
- Short-video thumbnails (duration probe / `thumbnail` filter) — **M**
- Persistent thumbnail cache (XDG_CACHE_HOME, per design `1750ebd`) — **M**;
  becomes the shared sink for SVG/PDF/font rasters, so sequence it *after* the
  image-producing items of C if you want them to benefit
- ffmpeg spawn-with-timeout hardening — **S**

### Group E — Rendering fidelity (biggest visual jump, separate track)
- Kitty graphics / sixel output where the terminal supports it — **L**. No new
  dependency, but touches the draw path (`FilePreview::draw`) and needs terminal
  capability detection + graceful fallback to half-blocks. Standalone; do it when
  you want the "wow", not as part of the dependency cleanup.

### Suggested sequence
1. **Group A** — immediate correctness win, removes the ugly error states, small
   PRs. (~2–4 days)
2. **Group B** — cheap, and every new type depends on it. (~1 day)
3. **Group C**, cherry-picked by real usage — breadth on a solid base.
4. **Group D** interleaved as polish; land the persistent cache before the
   raster-producing C items if you want them cached.
5. **Group E** whenever the visual upgrade is worth a focused multi-day push.

"Everything" is genuinely substantial — realistically **2–3 weeks** end to end —
but A+B alone (under a week) already deletes four external dependencies from the
preview path and fixes the mis-dispatch and error-state rough edges, which is
probably 80% of the felt improvement.
