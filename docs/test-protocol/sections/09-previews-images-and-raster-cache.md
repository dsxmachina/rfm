# Section 09 — Image previews & the raster cache

Harness per README (`N=09`, `SESSION=rfm-sec09`, `SOCK=/tmp/rfm-sec09.sock`).
This section runs **two rfm launches**: steps 09.1–09.10 with the default config
(`preview_cache = true`), steps 09.11–09.12 with a fresh cache dir and
`preview_cache = false`.

**Quiet fixture parent is REQUIRED here** (`PARENT=$(mktemp -d);
FIXTURE=$PARENT/fx; mkdir "$FIXTURE"`, per README). If the parent is a churning
`/tmp`, rfm's preloader stores thumbnails for stray sibling `.jpg` files and the
200-line log ring is flooded — both break this section's cache assertions. Even
with a quiet parent, the directory preloader precomputes the fixture's own
sibling images, so **assert only per-fixture-mtime entries (each == 1) and
old-swept siblings (== 0), never an aggregate `-img960u.jpg$` total count**; the
`Rgb8`-vs-`Rgba8` first-visit colour-mode drift is expected when the preloader
wins the decode race (see 09.1/09.3).

## Section fixture (create BEFORE launching rfm)

The base64 blobs are deterministic: an 8×8 solid-blue RGBA PNG (79 bytes) and an
8×8 solid-red JPEG (633 bytes). Each fixture file gets a distinct whole-second
mtime so raster-cache filenames (`<hash>-<mtime_secs>-<kind>.jpg`) are
distinguishable by their mtime component alone.

```bash
printf 'iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAFklEQVR4nGOUkzvxnwEPYMInOXwUAAAV0AITI9u7hwAAAABJRU5ErkJggg==' \
  | base64 -d > "$FIXTURE/a-blue.png"
touch -m -t 202001010000 "$FIXTURE/a-blue.png"

printf '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAoHBwgHBgoICAgLCgoLDhgQDg0NDh0VFhEYIx8lJCIfIiEmKzcvJik0KSEiMEExNDk7Pj4+JS5ESUM8SDc9Pjv/2wBDAQoLCw4NDhwQEBw7KCIoOzs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozs7Ozv/wAARCAAIAAgDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDmaKKK8g/RD//Z' \
  | base64 -d > "$FIXTURE/b-red.jpg"
touch -m -t 202101010000 "$FIXTURE/b-red.jpg"

printf 'hello\nworld\n' > "$FIXTURE/c-note.txt"

cp "$FIXTURE/a-blue.png" "$FIXTURE/d img & spaces.png"
touch -m -t 202201010000 "$FIXTURE/d img & spaces.png"

if command -v ffmpeg >/dev/null 2>&1; then
  ffmpeg -loglevel error -f lavfi -i color=c=red:s=64x64:d=1:r=10 -y "$FIXTURE/e-clip.mp4"
  touch -m -t 202301010000 "$FIXTURE/e-clip.mp4"
fi

printf 'not really a video\n' > "$FIXTURE/f-bad.mp4"
touch -m -t 202401010000 "$FIXTURE/f-bad.mp4"

THUMBS="$CACHE/rfm/thumbnails"
M_BLUE=$(stat -c %Y "$FIXTURE/a-blue.png")
M_RED=$(stat -c %Y "$FIXTURE/b-red.jpg")
M_SPC=$(stat -c %Y "$FIXTURE/d img & spaces.png")
```

Launch per README (`--config $CFG` with an **empty** `$CFG` — defaults apply,
`preview_cache = true`).

## Section helper: `settle`

`await-idle` does NOT cover in-flight preview decodes, and the preview panel is
rate-limited (`rate_limit_interval_ms = 500` by default) — after any cursor
move the right pane may lag up to ~500 ms behind. Every step below that asserts
preview content first runs this poll (no jq needed):

```bash
settle() { # $1 = absolute path the preview panel must reach
  for i in $(seq 1 60); do
    s1=$(echo state | socat - UNIX-CONNECT:$SOCK); q1=${s1#*\"seq\":}; q1=${q1%%,*}
    sleep 0.25
    s2=$(echo state | socat - UNIX-CONNECT:$SOCK); q2=${s2#*\"seq\":}; q2=${q2%%,*}
    printf '%s' "$s2" | grep -qF "\"preview_path\":\"$1\"" && [ "$q1" = "$q2" ] && return 0
  done
  echo "settle FAILED for $1" >&2; return 1
}
```

Fixture sort order (all regular files, no directories, all ASCII-lowercase
first letters): `a-blue.png`, `b-red.jpg`, `c-note.txt`, `d img & spaces.png`,
then (if created) `e-clip.mp4`, `f-bad.mp4`. Initial selection is `a-blue.png`
(first file — there are NO directories in this fixture, so the
directories-sort-first rule is moot here). Verify `state.selection` at every
step instead of trusting counted `j` presses.

---

### 09.1 — Startup: PNG previews as half-block raster, first cache write

**Action:** No keystrokes. `echo await-idle | socat - UNIX-CONNECT:$SOCK`, then
`settle "$FIXTURE/a-blue.png"`.

**Expect (socket):**
- `state.selection == "a-blue.png"`, `state.selected_idx == 0`,
  `state.preview_path == "$FIXTURE/a-blue.png"`, `state.mode == "normal"`.
- `state.image_protocol == "half-block"` — rfm runs inside tmux, so `auto`
  always resolves to half-block (no kitty/sixel probe result can apply).
- `echo "log 50" | socat - UNIX-CONNECT:$SOCK` contains NO
  `raster cache hit` line yet (first visit is a miss+store, which logs nothing
  on success).

**Expect (screen):** `tmux capture-pane -t $SESSION -p` shows in the right
(preview) pane:
- a half-block raster: at least one captured line contains `▄▄▄▄▄▄▄▄` (the 8×8
  image renders as 4 rows of 8 `▄` cells; small images are NOT upscaled).
- the three image info lines below the raster: `png · 79 B` and a timestamp
  line matching `YYYY-MM-DD HH:MM:SS`, plus a colour-mode line that is EITHER
  `8 × 8  Rgba8` (fresh in-memory decode) OR `8 × 8  Rgb8`. The `Rgb8` variant
  is **expected** on the first visit when the directory preloader wins the
  decode race and serves the already-stored JPEG thumbnail (accepted display
  drift — same as the documented 09.3 cache-hit drift, just one step early).
  Do not FAIL on `Rgb8` here.

**Expect (disk):**
- `$THUMBS` exists with mode `700` (`stat -c %a "$THUMBS"` → `700`).
- exactly one entry for the PNG:
  `ls "$THUMBS" | grep -c -- "-$M_BLUE-img960u.jpg$"` → `1`, and that filename
  matches `^[0-9a-f]{16}-$M_BLUE-img960u\.jpg$` (16-hex seahash prefix).
- no `*.part` files: `ls "$THUMBS" | grep -c '\.part$'` → `0`.

**Note:** The timestamp info line renders the mtime in UTC — with the
`202001010000` local-time fixture it may read `2020-01-01 …` or
`2019-12-31 …` depending on the host timezone. Assert the shape, not the date.

### 09.2 — JPEG preview + second cache entry

**Action:** `tmux send-keys -t $SESSION j` → `await-idle` →
`settle "$FIXTURE/b-red.jpg"`.

**Expect (socket):** `state.selection == "b-red.jpg"`,
`state.preview_path == "$FIXTURE/b-red.jpg"`. `seq` increased since 09.1
(never assert an exact delta).

**Expect (screen):** right pane shows a `▄▄▄▄▄▄▄▄` raster block and info lines
`8 × 8  Rgb8`, `jpeg · 633 B`, timestamp line.

**Expect (disk):** `ls "$THUMBS" | grep -c -- "-$M_RED-img960u.jpg$"` → `1`.
The 09.1 blue entry is still present (different hash+mtime, not swept).

### 09.3 — Revisit PNG: "raster cache hit" + cache-hit info drift

**Action:** `tmux send-keys -t $SESSION k` → `await-idle` →
`settle "$FIXTURE/a-blue.png"`.

**Expect (socket):**
- `state.selection == "a-blue.png"`.
- `echo "log 50" | socat - UNIX-CONNECT:$SOCK` contains a DEBUG line
  `raster cache hit for $FIXTURE/a-blue.png` (retained history; requires
  `--debug-socket`'s TRACE verbosity, which the harness always has).

**Expect (screen):** raster block again, but the first info line is now
`8 × 8  Rgb8` — NOT `Rgba8`. The cache-hit path reads color from the cached
JPEG thumbnail (always Rgb8 after the JPEG round-trip; documented accepted
display drift). `png · 79 B` and the timestamp line are unchanged.

**Expect (disk):** entry count for `$M_BLUE` still `1` (hit ≠ re-store).

**Note:** If the screen still shows `Rgba8`, the preview came from a fresh
decode, i.e. the cache lookup silently failed — check `log 50` for
`raster cache store failed` from 09.1 before filing.

### 09.4 — Text file: no cache write

**Action:** record `BEFORE=$(ls "$THUMBS" | wc -l)`. Then
`tmux send-keys -t $SESSION j j` (one at a time, `await-idle` between) →
`settle "$FIXTURE/c-note.txt"`.

**Expect (socket):** `state.selection == "c-note.txt"`,
`state.preview_path == "$FIXTURE/c-note.txt"`.

**Expect (screen):** right pane shows the text lines `hello` and `world`
(possibly syntax-styled by bat if installed — content is what matters). No `▄`
raster in the preview pane.

**Expect (disk):** `ls "$THUMBS" | wc -l` equals `$BEFORE` — text previews
never touch the raster cache.

**Note:** Pressing `j j` quickly may rate-limit-skip the intermediate
`d img & spaces.png`… wait — `c-note.txt` comes BEFORE `d img & spaces.png`;
from `a-blue.png` two `j` presses land on `c-note.txt` via `b-red.jpg`. The
intermediate `b-red.jpg` preview may be skipped by the rate limiter; that is
fine (it is already cached). Send the keys separately with `await-idle`
between to keep it deterministic.

### 09.5 — Image name with spaces and `&` previews and caches

**Action:** `tmux send-keys -t $SESSION j` → `await-idle` →
`settle "$FIXTURE/d img & spaces.png"`.

**Expect (socket):** `state.selection == "d img & spaces.png"`,
`state.preview_path` ends in `/d img & spaces.png`. `log 20` contains no new
ERROR/WARN lines from this step.

**Expect (screen):** raster block + info lines (`8 × 8  Rgba8` — first visit,
fresh decode — `png · 79 B`, timestamp).

**Expect (disk):** `ls "$THUMBS" | grep -c -- "-$M_SPC-img960u.jpg$"` → `1`.
The cache filename is a hash — the space/`&` never reaches a shell; this step
guards the hashing/store path against odd names.

### 09.6 — mtime bump: new cache entry, stale sibling swept

**Setup:** cursor is on `d img & spaces.png` (from 09.5) — the PNG must NOT be
selected while its mtime changes.

**Action:**
```bash
touch -m "$FIXTURE/a-blue.png"            # mtime -> now (differs from 2020 fixture value)
M_BLUE_NEW=$(stat -c %Y "$FIXTURE/a-blue.png")
echo await-idle | socat - UNIX-CONNECT:$SOCK    # let the watcher-driven reload drain
tmux send-keys -t $SESSION g g            # two keys: the 'gg' sequence = top
echo await-idle | socat - UNIX-CONNECT:$SOCK
settle "$FIXTURE/a-blue.png"
```

**Expect (socket):** `state.selection == "a-blue.png"`,
`state.selected_idx == 0`. No NEW `raster cache hit for …a-blue.png` line for
this visit (mtime changed → lookup miss; compare `log` line count/ages against
09.3).

**Expect (screen):** raster + info lines; first line `8 × 8  Rgba8` (fresh
decode after the miss); timestamp line now shows the new (current) mtime.

**Expect (disk):** assert only the specific fixture-mtime entries, never a total
count — the directory preloader precomputes sibling-image thumbnails (and, if
the parent isn't quiet, unrelated ones), so a `grep -c -- "-img960u.jpg$"` total
is polluted and unsafe.
- new entry: `ls "$THUMBS" | grep -c -- "-$M_BLUE_NEW-img960u.jpg$"` → `1`
- old sibling swept by the store: `ls "$THUMBS" | grep -c -- "-$M_BLUE-img960u.jpg$"` → `0`
- each surviving fixture entry present exactly once:
  `-$M_RED-img960u.jpg$` → `1` and `-$M_SPC-img960u.jpg$` → `1`
  (do NOT assert the aggregate `-img960u.jpg$` count == 3).

**Note:** The sweep runs as part of the store, i.e. only after the re-decode
completed — always `settle` before asserting the disk state.

### 09.7 — Cache directory hygiene invariants

**Action:** none (pure disk assertion after 09.1–09.6).

**Expect (disk):**
- every file in `$THUMBS` matches `^[0-9a-f]{16}-[0-9]+-(img960u|vid120)\.jpg$`
  (`vid120` only if 09.8 already ran — at this point expect `img960u` only)
- `find "$THUMBS" -name '*.part' | wc -l` → `0`
- `stat -c %a "$THUMBS"` → `700`

**Expect (socket):** `echo state | socat - UNIX-CONNECT:$SOCK` still answers
promptly (event loop healthy). **Expect (screen):** unchanged from 09.6.

### 09.8 — Video thumbnail via ffmpeg (SKIP if ffmpeg missing)

**Skip rule:** if `command -v ffmpeg` failed at fixture time, `e-clip.mp4` does
not exist — record `SKIP 09.8/09.9 (no ffmpeg)` and continue with 09.10.

**Action:** `tmux send-keys -t $SESSION G` (bottom → `f-bad.mp4`) →
`await-idle` → `tmux send-keys -t $SESSION k` (up → `e-clip.mp4`) →
`await-idle` → `settle "$FIXTURE/e-clip.mp4"`.

**Expect (socket):** `state.selection == "e-clip.mp4"`. `log 50` has no
`Could not run` lines for this file.

**Expect (screen):** right pane shows a half-block raster (`▄` cells — a red
frame from the lavfi clip). Below it: mediainfo text if `mediainfo` is
installed, otherwise no info lines at all (the video arm tolerates missing
mediainfo when the thumbnail succeeded).

**Expect (disk):** `ls "$THUMBS" | grep -c -- "-vid120.jpg$"` → `1`, with the
mtime component `$(stat -c %Y "$FIXTURE/e-clip.mp4")`. No `*.part` files.

**Note:** ffmpeg runs under a 10 s deadline (`EXTERNAL_RENDER_DEADLINE`); the
1-second 64×64 clip finishes far inside it, but `settle`'s 15 s budget exists
for exactly this.

### 09.9 — Video revisit: raster cache hit (SKIP with 09.8)

**Action:** `tmux send-keys -t $SESSION G` → `await-idle` →
`settle "$FIXTURE/f-bad.mp4"` (moves away; f-bad settles per 09.10 below —
if its `settle` times out, proceed, 09.10 covers it) → then
`tmux send-keys -t $SESSION k` → `await-idle` → `settle "$FIXTURE/e-clip.mp4"`.

**Expect (socket):** `log 50` contains
`raster cache hit for $FIXTURE/e-clip.mp4`.

**Expect (screen):** same raster as 09.8, and it must appear without a new
ffmpeg run — `ls "$THUMBS" | grep -c -- "-vid120.jpg$"` still `1`.

### 09.10 — Corrupt "video": graceful fallback, no cache garbage

**Action:** `tmux send-keys -t $SESSION G` → `await-idle` → poll `state` until
`selection == "f-bad.mp4"` and `seq` is stable across two reads 0.25 s apart
(do NOT require `preview_path` via `settle` blindly — on very old fallback
paths the preview may resolve slowly; the seq-stable poll is authoritative).

**Expect (socket):** one of, depending on installed tools:
- ffmpeg present: DEBUG log line starting
  `no ffmpeg thumbnail, falling back to mediainfo:` (the wrapped error contains
  `ffmpeg did not produce a thumbnail`).
- ffmpeg absent: no such line (the arm short-circuits to mediainfo).
In both cases `state` must answer promptly — a 10 s `state` timeout here is a
wedged event loop and a finding.

**Expect (screen):** a TEXT preview, never a blank error-free pane and never a
raster: mediainfo's block if `mediainfo` is installed, else exactly the
fallback text starting `Error: Could not run mediainfo` with the line
`You must have mediainfo installed to get a preview for this file-type.`.

**Expect (disk):** no cache entry for f-bad's mtime:
`ls "$THUMBS" | grep -c -- "-$(stat -c %Y "$FIXTURE/f-bad.mp4")-"` → `0`; no
`*.part` anywhere in `$THUMBS` (the failed producer must clean its temp file).

### 09.11 — Relaunch with `preview_cache = false`: privacy promise (images)

**Setup:** tear down the first launch only (keep `$FIXTURE`):
```bash
tmux kill-session -t $SESSION 2>/dev/null; rm -f $SOCK
CACHE2=$(mktemp -d); CFG2=$(mktemp -d)
printf '[general]\npreview_cache = false\n' > "$CFG2/config.toml"
# Direct-launch form (README): binary as the session command, not send-keys
# into an interactive shell (Atuin/zsh history-search would intercept it).
tmux new-session -d -s $SESSION -x 120 -y 30 \
  "env XDG_CACHE_HOME=$CACHE2 XDG_STATE_HOME=$STATE _ZO_DATA_DIR=$ZO \
   ./target/debug/rfm --debug-socket $SOCK --config $CFG2 $FIXTURE"
until [ -S $SOCK ]; do sleep 0.1; done
```

**Action:** `await-idle`, then `settle "$FIXTURE/a-blue.png"` (initial
selection).

**Expect (socket):** `state.selection == "a-blue.png"`,
`state.image_protocol == "half-block"`. `log 50` contains NO
`raster cache hit` line and NO `raster cache store failed` line — the cache is
opted out, not failing.

**Expect (screen):** the image STILL previews — raster block + info lines
`8 × 8  Rgba8`, `png · 79 B` (in-memory decode; disabling the cache disables
persistence, not previews).

**Expect (disk):** `[ ! -e "$CACHE2/rfm/thumbnails" ]` — the directory is
never even created. Stronger: `find "$CACHE2" -type f | wc -l` → `0`.

### 09.12 — `preview_cache = false`: video degrades to text by design (SKIP if ffmpeg missing)

**Action:** `tmux send-keys -t $SESSION G` → `await-idle` →
`tmux send-keys -t $SESSION k` → `await-idle` → poll `state` until
`selection == "e-clip.mp4"` and seq stable.

**Expect (socket):** DEBUG log line
`no ffmpeg thumbnail, falling back to mediainfo: on-disk thumbnails disabled (preview_cache = false)`
— ffmpeg is installed but deliberately not run (a thumbnail IS a write).

**Expect (screen):** TEXT preview only (mediainfo block, or the
`Error: Could not run mediainfo` fallback if mediainfo is absent). No `▄`
raster in the right pane.

**Expect (disk):**
- `find "$CACHE2" -type f | wc -l` → `0` still.
- The pre-cache temp fallback dir `${TMPDIR:-/tmp}/rfm-thumbnails` is **shared
  across machine runs** (7-day prune) and full of stragglers, so a bare
  `-newer "$FIXTURE/f-bad.mp4"` matches everything left by prior sessions — a
  false-positive magnet. **Snapshot the dir listing BEFORE this step** (e.g.
  `before=$(ls "${TMPDIR:-/tmp}/rfm-thumbnails" 2>/dev/null)`), run the step,
  then assert **no NEW file appeared** (`comm`/`diff` the after-listing against
  `$before`, or match this instance's `e-clip` mtime component) — do NOT rely on
  `-newer <fixture> | wc -l == 0`. With `preview_cache = false` nothing new may
  appear there.

---

**Teardown:** `tmux kill-session -t $SESSION 2>/dev/null;
rm -rf "$FIXTURE" "$CFG" "$CFG2" "$CACHE" "$CACHE2" "$STATE" "$ZO"; rm -f $SOCK`

**Section coverage gaps** (deliberate — for the meta-review):
- kitty/sixel graphics protocols (tmux forces half-block; explicit
  `image_protocol` pins untested here — needs a non-tmux/sixel-capable harness)
- SVG (`svg960`), font (`font-s1-24`) and PDF-render (`pdf-p1-960`) cache
  kinds; `pdf_render = true` end-to-end (PDF text tier is section-scope of the
  preview-types section)
- startup prune (30-day age / 256 MiB budget) and corrupt-cache-entry
  delete+regenerate rule (needs artificial cache seeding)
- enabled-but-unavailable cache fallback to `temp_dir()/rfm-thumbnails`
  (needs an unresolvable XDG cache home)
- EXIF-orientation uprighting (needs a rotated-EXIF JPEG fixture),
  decompression-bomb alloc limits, HEIC/AVIF mediainfo fallback
- concurrent same-entry stores (preloader-vs-on-demand `.part` race),
  cross-instance cache sharing
- `rate_limit_interval_ms` behavior itself (fast-scroll skipping) — only
  worked around here, not asserted
