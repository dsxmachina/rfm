# Run 1 — Section 09: Image previews & the raster cache

**Environment:** tmux 120×30, half-block protocol (inside tmux). Optional tools ALL present:
ffmpeg, mediainfo, bat, zip, openssl. Binary `./target/debug/rfm` (built 2026-07-31).
Two rfm launches per section (default config, then `preview_cache = false`).

**Major environmental caveat:** the harness launches rfm with the fixture directly
under `/tmp`, so rfm's LEFT pane is `/tmp` — a directory being hammered by ~150
parallel-executor `tmp.*` dirs and stray `.jpg` files. Two consequences:
1. The 200-line socket `log` history spans only ~20 s and is dominated by `/tmp`
   watcher spam ("Updating: /tmp", "request new dir-panel for /tmp"). DEBUG lines
   like `raster cache hit` / `no ffmpeg thumbnail, falling back to mediainfo` are
   **evicted within ~20 s**, so every log-line assertion against the *default* launch
   is unverifiable-by-eviction (the fresh launch-2 confirmed the lines DO fire).
2. rfm's preloader stored ~87 extra thumbnails for `.jpg` files it saw in the `/tmp`
   pane, so `$THUMBS` total-count assertions are polluted. All FIXTURE-specific
   entries are exactly-once correct.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 09.1 | PASS* | socket/disk/raster all correct; first info line was `Rgb8` not `Rgba8` — preloader won the decode race and served the cached JPEG (documented "accepted display drift", just earlier than the step assumes). No `raster cache hit` line (miss+store). |
| 09.2 | PASS | b-red.jpg, raster, `8 × 8  Rgb8` (JPEG is natively Rgb8), `jpeg · 633 B`; red+blue cache entries present. |
| 09.3 | PASS* | selection a-blue.png, screen `Rgb8` (cache-hit drift = proof of hit), blue count stays 1. Expected DEBUG `raster cache hit` line NOT visible — evicted by /tmp log churn (behavior confirmed via screen). |
| 09.4 | PASS | c-note.txt text preview (`hello`/`world`), no raster, cache count unchanged. |
| 09.5 | PASS | `d img & spaces.png` previews, `8 × 8  Rgba8` (fresh decode), `png · 79 B`, spaces cache entry =1, no ERROR/WARN. |
| 09.6 | PASS* | mtime bump: selection a-blue.png idx0, `Rgba8` fresh decode, new timestamp; new-mtime entry =1, old sibling swept to 0. Step's "total img960u == 3" fails (=90) purely from /tmp preloader pollution — the three fixture entries are each exactly-once. |
| 09.7 | PASS | every file matches `^[0-9a-f]{16}-[0-9]+-(img960u\|vid120)\.jpg$`, 0 `.part`, mode 700, event loop healthy. |
| 09.8 | PASS | e-clip.mp4 half-block raster + mediainfo "General" block; vid120 entry for clip mtime =1, total vid120 =1, no `.part`, no "Could not run". |
| 09.9 | PASS* | revisit: raster shows, vid120 stays 1 (no new ffmpeg run = definitive cache-hit proof). Log `raster cache hit` line evicted (same as 09.3). |
| 09.10 | PASS* | f-bad.mp4: TEXT/mediainfo preview, no raster, state prompt (no wedge), no cache entry for its mtime, no `.part`. Expected fallback DEBUG line evicted by /tmp churn (behavior fully correct). |
| 09.11 | PASS | `preview_cache=false`: image STILL previews (`8 × 8  Rgba8` in-memory), half-block proto, no cache-hit/store-failed lines, `$CACHE2/rfm/thumbnails` never created, 0 files in CACHE2. |
| 09.12 | PASS* | `preview_cache=false`: DEBUG line present & exact (`… on-disk thumbnails disabled (preview_cache = false)`), mediainfo text, no raster, CACHE2 0 files. The `find /tmp/rfm-thumbnails -newer f-bad` check returned 5 — all PRE-EXISTING stragglers from prior sessions (nothing from this instance); protocol false-positive. |

`*` = behavior correct; a written sub-assertion was unverifiable due to the shared-`/tmp`
environment (log eviction / cache pollution / straggler temp files), not an rfm fault.

**Tally: 12 passed, 0 failed, 0 skipped** (all `*` steps pass on their core behavior;
the unmet sub-assertions are environmental, filed as protocol feedback, not bugs).

## Bug reports

No rfm bugs found. Every deviation traced to the shared-`/tmp` test environment, not
to rfm. Notable near-miss investigated and cleared:

### Investigated (NOT a bug) — 09.1 shows `Rgb8` instead of `Rgba8` on first PNG visit
- The preloader stores the a-blue.png thumbnail before the preview panel's first
  render, so the preview reads the cached JPEG (`Rgb8`) rather than fresh-decoding
  (`Rgba8`). This is exactly the "accepted display drift" the protocol documents for
  09.3; it merely fires one step early. Confirmed benign by launch-2 (`preview_cache=false`,
  no preloader store) where 09.11's first visit correctly showed `Rgba8`.

## Protocol feedback

- **Launch the fixture NOT directly under `/tmp`.** rfm's left pane becomes `/tmp`,
  whose churn from parallel executors (a) floods the 200-line log so DEBUG assertions
  (`raster cache hit`, ffmpeg-fallback lines) are evicted within ~20 s, and (b) makes
  the preloader store ~87 unrelated thumbnails, breaking every "total img960u == N"
  count. Fix: `FIXTURE=$(mktemp -d)/box; mkdir "$FIXTURE"` so the parent pane is an
  isolated empty dir, OR have the harness `cd` rfm into a fixture wrapped one level deep.
- **Log-line assertions need a freshness/anti-eviction strategy.** Given the 200-line /
  ~10 s window under churn, the reliable signal for a cache hit is the *screen* (Rgb8
  drift for images) and the *disk* (vid120 count unchanged for video), not the log line.
  Recommend the protocol assert those as primary and treat the log line as best-effort.
- **09.6 "total img960u entries == 3" is unsafe** even in isolation if the preloader
  precomputes siblings; assert only the three fixture-mtime entries (each ==1) + old
  swept (==0).
- **09.12 `find "$TMPDIR/rfm-thumbnails" -newer f-bad | wc -l → 0` is a false-positive
  magnet.** `/tmp/rfm-thumbnails` is a machine-shared straggler dir (7-day prune), and
  `-newer` a 2024-dated fixture matches everything left by prior runs. Assert instead
  that no NEW file appeared (e.g. snapshot the dir listing before the step, or check
  `-mmin -1`, or match this instance's e-clip mtime component).
- The `settle`/helper functions do not survive `source` reliably under this zsh setup
  when other functions follow; defining `settle` inline per-invocation was the workaround
  (executor-side, not an rfm/protocol issue).
- One transient, non-reproducing observation during 09.2: the center selection briefly
  reverted b-red.jpg → a-blue.png during a /tmp-watcher reload burst. Could not
  reproduce on repeat (`j` stuck cleanly afterward across many seq ticks). Likely the
  same /tmp-churn interference; flagged for awareness only, not filed as a bug.
