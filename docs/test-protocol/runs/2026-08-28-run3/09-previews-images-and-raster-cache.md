# Run 3 — Section 09: Image previews & the raster cache

- Date: 2026-08-28, branch `feat/kitty-unicode-placeholders`, binary `./target/debug/rfm` (prebuilt)
- Harness: tmux session `tp3-09`, socket `/tmp/tp3-09.sock`, 120×30 pane, quiet
  fixture parent (`$PARENT/fx`), scratch `--config`, `XDG_CACHE_HOME`/`XDG_STATE_HOME`/
  `_ZO_DATA_DIR` tempdirs, `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`.
- Tools present: tmux, socat, ffmpeg, mediainfo, bat → no ffmpeg skips.
- Fixture mtimes: `M_BLUE=1577833200 M_RED=1609455600 M_SPC=1640991600
  M_CLIP=1672527600 M_BAD=1704063600`.

**Tally: 12 pass / 0 fail / 0 skip.** No rfm bugs filed. Every socket read was
wrapped in `timeout 12`; no `state` timeout occurred at any step (event loop
healthy throughout). Several expectation strings in the protocol are stale
relative to develop commit `7412fd7` (structured image info block) and the
preloader/in-memory-panel-cache behavior — details per step and in the
protocol-feedback list at the end. Steps were judged on substance (the
invariant the step guards), with the literal-text drift recorded as feedback.

## Per-step results

### 09.1 — Startup PNG preview + first cache write — PASS
- Socket: `selection=a-blue.png`, `selected_idx=0`,
  `preview_path=$FIXTURE/a-blue.png`, `mode=normal`,
  `image_protocol="half-block"` ✓ (`graphics: probe -> half-block (env)` in log).
- Screen: half-block raster present (18 pane lines of `▄` — see feedback F3:
  the draw path upscales small images to the cell box since `7412fd7`;
  the protocol's "NOT upscaled / 4 rows of 8 ▄" text is stale). Info block is
  the structured `label : value` form (`Format : PNG`, `Dimensions : 8 × 8`,
  `Aspect ratio : 1.000`, `Color : RGBA`, `Bit depth : 8-bit`,
  `File size : 79 B`) — semantically equivalent to the expected
  `8 × 8 Rgba8` / `png · 79 B`; the mtime line is intentionally omitted
  (footer shows it) — feedback F2.
- Log: contains ONE `raster cache hit for …/a-blue.png` — the protocol expects
  none, but the directory preloader stored the thumbnail first and the
  on-demand preview task then legitimately hit it (feedback F4; the same race
  the protocol already accepts for color drift).
- Disk: `$THUMBS` mode `700` ✓, exactly one
  `^[0-9a-f]{16}-1577833200-img960u\.jpg$` ✓, zero `.part` ✓. The preloader had
  also already stored `b-red`, `d img & spaces` (img960u) and `e-clip`
  (vid120) — expected preloader behavior, why per-mtime asserts are used.

### 09.2 — JPEG preview + second cache entry — PASS
- Socket: `selection=b-red.jpg`, `preview_path` correct, seq 4→6 (increased).
- Screen: raster block + `Format : JPEG`, `Dimensions : 8 × 8`, `Color : RGB`,
  `File size : 633 B`.
- Disk: `-$M_RED-img960u.jpg` count 1; blue entry still present.

### 09.3 — Revisit PNG — PASS (substance), stale log expectation
- Socket: `selection=a-blue.png` ✓.
- Expected `raster cache hit` line on revisit did NOT appear (hit count for
  a-blue stayed 1 — the startup one). Log trail shows why: on `k` there is no
  `panel-update: preview` and no raster-cache activity at all — the preview
  was served from rfm's **in-memory preview-panel cache**, one layer above the
  raster cache. Correct, cheaper behavior; the raster-cache lookup path itself
  was proven working by the startup hit in 09.1. Feedback F5.
- Screen: raster + info block again; `Color : RGBA` — NOT the protocol's
  expected `Rgb8` drift, because since `7412fd7` info lines derive from
  `upright_source_meta` (original file metadata), not the cached JPEG raster.
  The entire Rgb8/Rgba8 drift discussion is obsolete (feedback F2).
- Disk: `$M_BLUE` entry count still 1 ✓ (hit ≠ re-store).

### 09.4 — Text file: no cache write — PASS
- Socket: `selection=c-note.txt`, `preview_path` correct.
- Screen: `hello` / `world` in the right pane, no `▄` raster.
- Disk: thumbnail count before == after (4).

### 09.5 — Spaces and `&` in name — PASS
- Socket: `selection="d img & spaces.png"`, `preview_path` ends correctly;
  `log 20` had 0 ERROR/WARN lines.
- Screen: raster (18 `▄` lines) + `Format : PNG`, `8 × 8`, `RGBA`, `79 B`.
- Disk: `-$M_SPC-img960u.jpg` count 1.

### 09.6 — mtime bump: new entry, stale sibling swept — PASS
- `touch -m` → `M_BLUE_NEW=1787937396`; `gg` (two `g` presses) → top.
- Socket: `selection=a-blue.png`, `selected_idx=0`; no new
  `raster cache hit …a-blue.png` line (miss, as expected — count stayed 1).
- Screen: fresh raster + info block (`RGBA`).
- Disk: `-$M_BLUE_NEW-img960u.jpg` → 1; old `-$M_BLUE-` → 0 (swept);
  `-$M_RED-` → 1; `-$M_SPC-` → 1. (Aggregate counts deliberately not asserted.)

### 09.7 — Cache hygiene invariants — PASS
- All 4 files match `^[0-9a-f]{16}-[0-9]+-(img960u|vid120)\.jpg$` (0
  nonconforming). The `vid120` entry exists already because the preloader
  generated it at startup — the protocol's "at this point expect img960u only"
  parenthetical predates preloader-precomputes-videos (feedback F6); the
  regex invariant itself holds.
- `.part` count 0 ✓; mode `700` ✓; `state` answered promptly ✓.

### 09.8 — Video thumbnail via ffmpeg — PASS
- Socket: `selection=e-clip.mp4`; no `Could not run` lines.
- Screen: 18-line `▄` raster (red lavfi frame) + full mediainfo block
  (Format MPEG-4, Duration 1 s, 10.000 FPS).
- Disk: `-vid120.jpg` count 1, mtime component `1672527600` (== e-clip) ✓,
  no `.part`. Note: the thumbnail was generated by the startup preloader
  (`generating thumbnail …3f276d…-1672527600-vid120.jpg`, startup age), not by
  this visit — the visit consumed it.

### 09.9 — Video revisit: no re-run — PASS (substance), stale log expectation
- No NEW `generating thumbnail` line for e-clip after startup (both such lines
  carry startup ages) → ffmpeg did NOT re-run ✓; `-vid120.jpg` still 1 ✓;
  raster on screen ✓; f-bad settle succeeded en route.
- The expected `raster cache hit for …e-clip.mp4` log line never appears in
  this session — same root cause as 09.3: the preloader built the preview
  in-memory at startup, so no visit ever consults the raster cache
  (feedback F5). Correct behavior, unreachable assertion.

### 09.10 — Corrupt "video": graceful fallback — PASS
- Poll: `selection=f-bad.mp4`, seq stable across 0.25 s reads; `state` prompt
  throughout (no wedge).
- Log: `no ffmpeg thumbnail, falling back to mediainfo: ffmpeg did not produce
  a thumbnail (exit status: 183): … Invalid data found when processing input`
  present (emitted by the startup preloader — same event class the step
  expects).
- Screen: TEXT preview (mediainfo minimal block: Complete name /
  File size 19.0 Bytes), zero `▄` lines.
- Disk: `-$M_BAD-` count 0; no `.part` anywhere.

### 09.11 — `preview_cache = false`: privacy promise (images) — PASS
- Relaunch with `$CFG2` (`[general] preview_cache = false`), fresh `$CACHE2`.
- Socket: `selection=a-blue.png`, `image_protocol="half-block"`; `log 50` has
  0 `raster cache hit` and 0 `raster cache store failed` lines.
- Screen: image still previews — raster + `Format : PNG … RGBA … 79 B`.
- Disk: `$CACHE2/rfm/thumbnails` never created; `find "$CACHE2" -type f` → 0.

### 09.12 — cache off: video degrades to text — PASS
- Socket: exact DEBUG line found: `no ffmpeg thumbnail, falling back to
  mediainfo: on-disk thumbnails disabled (preview_cache = false)`.
- Screen: mediainfo text block for e-clip.mp4, zero `▄` lines.
- Disk: `$CACHE2` still 0 files; `/tmp/rfm-thumbnails` diffed empty-before vs
  after — no new file (snapshot method per protocol).

## Bugs

None. No rfm bug was found in this section; every deviation from the literal
protocol text traced to intentional develop-side changes (commit `7412fd7`,
2026-08-02 — after the protocol was last updated on 2026-07-31 runs) or to
documented preloader behavior.

## Protocol feedback

- **F1 (harness note):** `ls` in the executor's shell profile emits ANSI color
  codes even piped, silently zeroing every `ls "$THUMBS" | grep -c` count.
  Use `find "$THUMBS" -maxdepth 1 -type f -printf '%f\n'` in the section's
  disk-assert recipes.
- **F2:** All image info-line expectations (`8 × 8 Rgba8`, `png · 79 B`,
  timestamp line, and the whole 09.3 Rgb8-cache-hit-drift narrative) are stale
  since develop `7412fd7`: the footer is now a structured mediainfo-style
  block (`Format/Dimensions/Aspect ratio/Color/Bit depth/File size`), the
  mtime line is intentionally omitted (main footer shows it), and color
  derives from the original file's metadata (`upright_source_meta`), so there
  is no cache-hit color drift anymore. Steps 09.1/09.2/09.3/09.5/09.6/09.11
  need their expected strings rewritten.
- **F3:** 09.1's "small images are NOT upscaled (4 rows of 8 `▄`)" is stale:
  the half-block draw path resizes via `image::thumbnail(cellbox)`, which
  upscales small sources (8×8 renders as a pane-filling ~36×36 raster; the
  in-code comments confirm thumbnail() upscales and that "the draw path
  rescales to cell dimensions anyway"). The load-bearing assert (a line of
  `▄▄▄▄▄▄▄▄`) still holds; drop the no-upscale parenthetical.
- **F4:** 09.1's "log contains NO `raster cache hit` yet" is race-fragile:
  when the directory preloader wins the store race (the same race the section
  header already accepts for color drift), the on-demand first visit IS a
  logged hit. Expected-absence should become "0 or 1, depending on the
  preloader race".
- **F5:** 09.3's and 09.9's "revisit logs `raster cache hit`" are unreachable
  in a single session: revisits are served by the in-memory preview-panel
  cache and never consult the raster cache (no `panel-update: preview`, no
  cache activity in the trail). The raster-cache lookup is only exercised at
  first-build time (visible at startup when the preloader stored first, per
  F4) or across relaunches. Rewrite as either (a) assert the *disk* invariants
  only (entry count unchanged, no producer re-run — both still asserted and
  passing), or (b) add a relaunch-with-warm-cache step to observe the hit
  deterministically.
- **F6:** 09.7's "(at this point expect `img960u` only)" is stale — the
  startup preloader also generates the sibling video's `vid120` thumbnail
  immediately, so `vid120` legitimately exists before 09.8. The regex
  invariant already allows it; drop the parenthetical.
- **F7 (minor):** 09.10's expected ffmpeg-failure log line is emitted by the
  startup preloader, not by the visit itself — with a preloading build the
  step's log assert passes for a reason different from the one implied.
  Wording could say "present in the retained history (possibly from the
  preloader's earlier attempt)".
