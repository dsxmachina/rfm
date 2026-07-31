# High-Fidelity Image Rendering (Kitty / Sixel) — Implementation Plan

Design: `docs/plans/2026-07-30-preview-graphics-protocol-design.md`.
Branch: `feature/preview-5-graphics` (stacks on `feature/preview-4-pdf`).
TDD throughout: every step names its failing tests first, then the minimal
implementation. `cargo test` + `cargo clippy` green after every step.

## Resolved design decisions

These were left open or vague in the design doc; the plan pins them:

**D1 — Startup probe placement.** Detection runs in `main.rs` immediately
after `enable_raw_mode()?` (currently line 276) and before the
alternate-screen queue / `PanelManager::new` (which constructs the crossterm
`EventStream`, manager.rs:445). Raw mode is *required* for the probe (replies
arrive unbuffered, un-echoed, without a newline), and nothing else reads
stdin yet, so the probe can consume the reply bytes safely. Total probe
budget: 250 ms wall-clock, enforced with `poll(2)`-bounded reads; on timeout
the protocol is `HalfBlock` and any late reply bytes are drained by a final
zero-timeout poll loop so they cannot leak into the event stream as phantom
keys.

**D2 — Pixel geometry.** crossterm 0.26.1 has **no** pixel-size API
(verified in the vendored source: `terminal/sys/unix.rs` zeroes
`ws_xpixel`/`ws_ypixel`; `window_size()` first appears in 0.27). We use
`libc` directly: `ioctl(STDOUT_FILENO, TIOCGWINSZ)` for
`ws_xpixel`/`ws_ypixel`, and `libc::poll` for the bounded probe read.
`libc 0.2` is already in `Cargo.lock` (0.2.189) as a mandatory transitive
dependency of crossterm (and tokio, notify, …) on every unix build —
declaring it in `Cargo.toml` adds **zero new crates** to the build graph.
This satisfies the no-new-crates mandate; recorded here so nobody mistakes
the `Cargo.toml` line for a new dependency.
Fallback chain when `TIOCGWINSZ` reports zeros (common over ssh/serial):
append `CSI 14 t` (report text area in pixels → `ESC [ 4 ; h ; w t`) to the
startup probe batch. If both fail: Kitty may proceed with an assumed
8×16 px cell (kitty scales the image into the `c=`/`r=` cell rectangle, so
placement stays exact); Sixel **requires** real geometry (its raster cannot
be scaled by the terminal and must be pre-sized to the pane) — without
geometry, Sixel degrades to `HalfBlock`.

**D3 — Geometry refresh on resize.** The stored value is the *cell* size in
pixels (`px_per_cell_w = ws_xpixel / cols`, `px_per_cell_h = ws_ypixel /
rows`), not the window size, so a resize that keeps the font is already
correct. On `Event::Resize` (manager.rs:2257) we additionally call
`graphics::refresh_geometry()` — a pure `ioctl`, no stdin involvement, safe
mid-session. No mid-session `CSI 14 t` (its reply would land in the
`EventStream`).

**D4 — Erase/ownership model: frame claim + reconcile.** A graphics image
is not a cell blit, so the z-order invariant ("each draw fully paints its
region") is preserved by construction, not hope:
- `graphics::begin_frame(image_allowed: bool)` at the top of
  `PanelManager::draw` (manager.rs:857). `image_allowed` =
  `view == ViewMode::Single` **and** no `ModalRegion::ConsoleOverlay` modal
  is active (the trash view / console overlays draw over the panel area; a
  kitty placement would float above them).
- The `Preview::Image` branch of `FilePreview::draw` *claims* the frame with
  its emit key when it draws via a graphics protocol. When the frame is not
  `image_allowed`, it falls back to the half-block loop for that frame
  (cells respect z-order), claiming nothing.
- `graphics::end_frame(&mut stdout)` just before `EndSynchronizedUpdate`:
  if a live placement exists whose key was **not** claimed this frame, erase
  it (Kitty: delete-by-id; Sixel: overwrite the recorded cell region with
  spaces). This one hook covers *every* stale-image case — selection moved
  to a directory/text file, split toggle, overlay opened, tab switch —
  without touching `DirPanel`, `PreviewPanel`'s structure, or the draw
  order.

**D5 — Re-emission gating.** `EmitKey { path, mtime_secs, px_w, px_h,
origin_cell: (u16, u16) }`. The live placement (`Mutex<Option<LiveImage>>`
in the graphics module, parking_lot is already a dep) stores the key, the
kitty image id, and the occupied cell region. If the computed key equals the
live key, the image branch emits **nothing** for the raster region (it still
redraws the border and info-footer cells, which are cheap) — unrelated dirty
repaints cost zero retransmission. The state is module-global rather than
per-`FilePreview` because a new preview *replaces* the whole `FilePreview`
(preview.rs `update_content`), so a per-instance field could never erase its
predecessor's placement.

**D6 — Kitty specifics.** Direct transmission `a=T,f=24` (RGB; alpha is
already discarded by `resized_rgb`'s `into_rgb8`, same as half-block —
known, accepted limitation), base64 payload chunked at 4096 chars with
`m=1`/`m=0`, fresh `i=<id>` from an `AtomicU32` counter per transmission,
placement sized in cells via `c=`/`r=` computed from the same geometry as
the raster (so no aspect distortion), **`q=2` on every command** so the
terminal never sends a response into the event stream. Erase:
`ESC _ G a=d,d=I,i=<id>,q=2 ESC \` (capital `I` also frees the pixel data).
Base64 is hand-rolled (~15 lines + unit test) — the `base64` crate in the
lock is transitive-only and not worth the dependency debate.

**D7 — Sixel specifics.** Hand-rolled encoder, pure function
`Vec<u8> = sixel_encode(&RgbImage)`. Fixed 6×7×6 color cube (252 registers,
under the 256 limit): `idx = r6*42 + g7*6 + b6` with `r6 = r*6/256` etc.
Header `ESC P 0;1;0 q` (P2=1: zero bits leave the background) + raster
attributes `"1;1;<w>;<h>`; palette entries `#i;2;R;G;B` (0–100 scale)
emitted only for indices actually used, ascending (deterministic); bands of
6 rows with `#idx` color selects, RLE `!<n><ch>` for runs > 3, `$` between
colors, `-` between bands, `ESC \` terminator. The raster is pre-fit to the
pane pixel box and its height clamped down to a multiple of 6 so the final
band cannot bleed into the row below the pane. Optional Bayer-4×4 ordered
dithering is a follow-up polish item (step 8), not v1.

**D8 — Raster sizing & the resize cache.** `resized_rgb`
(preview.rs:49) is already keyed on the *requested* `(w, h)` — the unit is
the caller's business. Graphics tiers request the pixel box
(`fit(cols_for_image * cell_w, rows_for_image * cell_h)`), half-block keeps
requesting cell-derived dims; the protocol never changes mid-session, so the
single-entry cache never thrashes. No structural cache change — only the
call site computes differently. Row budget mirrors today's split: the image
gets the full height when `info` is empty, else `2*height/3` cells, info
lines below (preview.rs:114–118 keeps the same proportions). Note: sources
are pre-shrunk to ≤960×540 at decode (`native_image_preview`), which caps
graphics fidelity — acceptable, it matches the persistent raster cache and
still quadruples effective resolution vs half-blocks.

**D9 — tmux/screen.** `$TMUX` set, or `$TERM` starting with `tmux`/`screen`
→ `HalfBlock` in `auto`, no probe (tmux would answer DA1 itself and
passthrough is off by default). Explicit `image_protocol = "kitty"|"sixel"`
overrides even inside tmux (the documented escape hatch for passthrough
users).

**D10 — Testing channel for e2e.** tmux without passthrough swallows
unknown DCS/APC sequences, so `capture-pane -e` **cannot** show kitty/sixel
bytes. E2e assertions therefore lean on (a) the debug socket `state` field
`image_protocol`, (b) deterministic `trace!` lines in the retained log
(`log` socket command): `graphics: transmit kitty id=<n> <w>x<h>`,
`graphics: gated (unchanged)`, `graphics: erase id=<n>`,
`graphics: sixel emit <w>x<h>`, `graphics: probe -> <proto> (<reason>)`.
`capture-pane -e` is still used to assert the half-block fallback (truecolor
SGR + `▄` present) and that no `ESC P`/`ESC _ G` garbage leaks as literal
text.

---

## Step 1 — Config knob `image_protocol`

**Files:** `src/config.rs`, `src/main.rs`, `examples/config.toml`.

**Failing tests first** (`src/config.rs` `mod tests`):
- `image_protocol_defaults_to_auto` — `toml::from_str::<GeneralConfig>("")`
  yields `ImageProtocolChoice::Auto`.
- `image_protocol_parses_all_values` — `"auto"`, `"kitty"`, `"sixel"`,
  `"half-block"` all deserialize; `"halfblock"` errors (serde `rename` is
  exact).
- Extend `embedded_general_config` (main.rs) implicitly by editing the
  example file — it must still parse.

**Implementation:**
- `config.rs`: `#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
  #[serde(rename_all = "kebab-case")] pub enum ImageProtocolChoice { #[default] Auto,
  Kitty, Sixel, HalfBlock }`; field on `GeneralConfig`:
  `#[serde(default)] pub image_protocol: ImageProtocolChoice`.
- `main.rs`: local `let mut image_protocol = ImageProtocolChoice::Auto;`
  next to the `use_trash`/`pdf_render` locals; assign from
  `config.general.image_protocol` in the `Ok(config)` arm. (Consumed in
  step 3.)
- `examples/config.toml`: documented block next to `pdf_render` —
  values, default `auto`, the tmux-passthrough caveat, and that explicit
  values skip probing.

Verify: `cargo test config`, `cargo test embedded_general_config`.

## Step 2 — `graphics` module: pure detection, probe parsing, geometry math

**Files:** new `src/panel/graphics.rs`; `src/panel/mod.rs` (add
`pub mod graphics;`).

All of this step is terminal-free pure logic; the I/O shell comes in step 3.

**Failing tests first** (`graphics.rs` `mod tests`):
- Env heuristics (`detect_from_env(env: &dyn Fn(&str) -> Option<String>)
  -> Option<GraphicsProtocol>` — takes a lookup closure so tests never
  touch process env):
  - `env_tmux_var_resolves_half_block` (`TMUX=/tmp/…` → `Some(HalfBlock)`)
  - `env_screen_and_tmux_term_resolve_half_block` (`TERM=screen-256color`,
    `TERM=tmux-256color` → `Some(HalfBlock)`) — checked *before* the kitty
    vars so tmux-on-kitty stays safe.
  - `env_kitty_window_id_resolves_kitty`; `env_term_xterm_kitty_resolves_kitty`
  - `env_term_program_wezterm_and_ghostty_resolve_kitty`
  - `env_plain_xterm_is_inconclusive` (→ `None`, meaning "probe")
- Probe reply parser (`parse_probe(buf: &[u8]) -> ProbeOutcome` where
  `ProbeOutcome { kitty_ok: bool, sixel: bool, pixel_size: Option<(u16,u16)>,
  da1_seen: bool }`):
  - `kitty_ok_reply_detected` — buf containing `\x1b_Gi=4242;OK\x1b\\` and a
    DA1 reply → `kitty_ok`, `da1_seen`.
  - `kitty_error_reply_is_not_ok` — `\x1b_Gi=4242;ENOTSUP…` → `!kitty_ok`.
  - `da1_with_attribute_4_is_sixel` — `\x1b[?62;4;22c` → `sixel` (attribute
    list split on `;`, exact-match `"4"`, so `40`/`44` don't false-positive:
    covered by `da1_attribute_44_is_not_sixel`).
  - `da1_without_4_is_not_sixel` — `\x1b[?62;22c`.
  - `csi14t_reply_parses_pixel_size` — `\x1b[4;480;800t` →
    `pixel_size = Some((800, 480))` (order: height;width in the reply).
  - `garbage_prefix_is_skipped` — stray bytes before the replies don't break
    parsing.
- Resolution (`resolve(choice: ImageProtocolChoice, env_hit:
  Option<GraphicsProtocol>, probe: Option<&ProbeOutcome>) ->
  GraphicsProtocol`):
  - `explicit_choice_wins_without_probe` — `Kitty`/`Sixel`/`HalfBlock`
    map directly, probe irrelevant.
  - `auto_uses_env_hit_first`; `auto_probe_kitty_beats_sixel`;
    `auto_probe_sixel_without_kitty`; `auto_no_probe_data_is_half_block`
    (timeout path).
- Geometry (`CellGeometry { cell_w: u16, cell_h: u16 }`):
  - `cell_geometry_from_winsize` — (cols=100, rows=50, xpx=800, ypx=1000) →
    `cell_w=8, cell_h=20`.
  - `zero_winsize_pixels_yields_none` — falls through to the CSI-14t value.
  - `pixel_box_from_cell_span` — helper
    `pixel_box(cols: u16, rows: u16, geo: CellGeometry) -> (u32, u32)`.
  - `image_cell_span_from_raster` — `ceil` division back to cells
    (`cells_for(px, cell_px)`), used for `c=`/`r=` and the erase region.

**Implementation:** the enums (`GraphicsProtocol { Kitty, Sixel, HalfBlock }`
with `fn name(&self) -> &'static str` returning `"kitty"|"sixel"|"half-block"`),
the three pure functions, `CellGeometry` + math. No I/O, no statics yet.

Verify: `cargo test graphics`.

## Step 3 — I/O shell: libc geometry, bounded probe, startup wiring

**Files:** `Cargo.toml` (declare `libc = "0.2"` — resolves to the already
locked 0.2.189; **no new crate**, see D2 — with a comment saying exactly
that), `src/panel/graphics.rs`, `src/main.rs`, `src/panel/manager.rs`.

**Failing tests first:**
- `probe_read_respects_deadline` — the read loop is
  `read_probe_replies(fd: RawFd, deadline: Instant) -> Vec<u8>`; test with a
  `std::io::pipe`-style fd pair (use `std::os::unix::net::UnixStream::pair()`
  — std-only): a writer that sends the DA1 reply → returns early with the
  bytes; a writer that sends nothing → returns empty at ~deadline (assert
  elapsed < 400 ms, ≥ deadline). Uses `libc::poll` under the hood; the fd
  parameter keeps it testable without a tty.
- `init_with_explicit_choice_never_reads` — `graphics::init_from(choice,
  env_fn, io: Option<…>)` with `Kitty` and a panicking io closure resolves
  without touching io.
- Extend step-2 resolution tests if seams shift.

**Implementation:**
- `winsize_via_ioctl() -> Option<(cols, rows, xpx, ypx)>` using
  `libc::ioctl(1, libc::TIOCGWINSZ, &mut ws)` (unsafe block, `-1` check).
- `read_probe_replies` as above: loop `libc::poll(POLLIN, remaining_ms)` +
  `libc::read` into a buffer; stop when `parse_probe(..).da1_seen` or
  deadline; final zero-timeout drain (D1).
- `pub fn init(choice: ImageProtocolChoice)` — called from `main.rs` right
  after `enable_raw_mode()?`:
  1. explicit choice → set protocol (still run `winsize_via_ioctl` for
     geometry; probe `CSI 14 t` + DA1 only if pixels are zero *and* the
     choice needs geometry, i.e. not `HalfBlock`),
  2. auto → `detect_from_env(std::env::var)`; env hit → done;
  3. else write the probe batch to stdout **and flush**:
     `\x1b_Gi=4242,s=1,v=1,a=q,t=d,f=24,q=0;AAAA\x1b\\` + (`\x1b[14t` if
     needed) + `\x1b[c`, then `read_probe_replies` with a 250 ms deadline,
     `resolve(..)`, log `graphics: probe -> <proto> (<reason>)` at debug.
  4. Sixel without geometry → HalfBlock (D2). Store protocol in a
     `OnceCell<GraphicsProtocol>` (`FFMPEG_INSTALLED` pattern), geometry in
     `AtomicU32` (packed w|h) so `refresh_geometry` can update it.
- Accessors: `pub fn protocol() -> GraphicsProtocol` (defaults `HalfBlock`
  if uninitialized — keeps unit tests of the draw path safe),
  `pub fn cell_geometry() -> Option<CellGeometry>`,
  `pub fn refresh_geometry()` (ioctl-only re-read; keeps the old value when
  the ioctl reports zeros).
- `main.rs`: `panel::graphics::init(image_protocol);` between
  `enable_raw_mode()?` and the `stdout.queue(...)` block.
- `manager.rs` `Event::Resize` arm (line 2257): add
  `graphics::refresh_geometry();` before `mark_dirty()`.

Verify: `cargo test graphics`, `cargo clippy`, and a manual smoke:
`cargo build` + run in a real terminal, check the probe log line via the
debug socket.

## Step 4 — Kitty emitter: base64, chunking, id lifecycle, gating

**Files:** `src/panel/graphics.rs`.

All emitters write to `&mut impl Write` (not `Stdout`) so tests capture
bytes in a `Vec<u8>`.

**Failing tests first:**
- `base64_known_vectors` — `b""`, `b"f"`, `b"fo"`, `b"foo"`,
  `b"foobar"` → RFC 4648 strings (padding included).
- `kitty_transmit_is_chunked_and_terminated` — a raster whose base64
  exceeds 4096 chars: first chunk carries
  `a=T,f=24,s=<w>,v=<h>,i=<id>,c=<cols>,r=<rows>,q=2,m=1`, middle chunks
  `m=1`, final `m=0`; every chunk ≤ 4096 payload chars; all wrapped
  `\x1b_G…\x1b\\`.
- `kitty_small_image_single_chunk_m0` — tiny raster → one chunk, `m=0`.
- `kitty_delete_sequence` — `erase` emits `\x1b_Ga=d,d=I,i=<id>,q=2\x1b\\`
  plus space-overwrite of the recorded cell region (cells belt-and-braces:
  also correct on terminals that ignore the delete).
- `emit_state_gates_identical_key` — `KittyEmitter::emit(sink, key, raster…)`
  twice with the same key: second call writes zero bytes and returns
  `Emitted::Gated`.
- `emit_state_new_key_deletes_old_id_first` — second emit with a different
  key: output starts with the delete of the previous id, new id > old id.
- `end_frame_erases_unclaimed` — begin_frame(true); no claim; end_frame
  with a live image → delete emitted and live state cleared. And:
  `end_frame_keeps_claimed` — claim with the live key → no bytes.
- `begin_frame_disallowed_forces_no_claim` — with `image_allowed=false`,
  `should_use_graphics()` reports false even when protocol is Kitty.

**Implementation:** `fn b64(data: &[u8]) -> String` (table encoder);
`struct LiveImage { key: EmitKey, id: u32, region: (Range<u16>, Range<u16>) }`;
module statics `LIVE: Mutex<Option<LiveImage>>`,
`CLAIMED: Mutex<Option<EmitKey>>` + `FRAME_ALLOWED: AtomicBool`
(parking_lot Mutex, already a dep); `NEXT_ID: AtomicU32` starting at a
fixed base. Public seam used by the draw path in step 6:
`begin_frame(allowed)`, `end_frame(w)`, `emit_kitty(w, key, rgb, origin,
cols, rows) -> Result<Emitted>`. Trace logs per D10. (An `erase_live(w)`
"belt and braces" erase -- delete-by-id plus space overwrite -- was planned
here but dropped in review: an unconditional space overwrite after the
draw pass wipes freshly painted cells; the `end_frame` reconcile must
never write cell content, so only the delete-by-id survives.)

Verify: `cargo test graphics`.

## Step 5 — Sixel encoder

**Files:** `src/panel/graphics.rs` (or `graphics/sixel.rs` if the module
crosses ~600 lines — keep `panel/graphics.rs` the public face either way).

**Failing tests first:**
- `sixel_quantize_maps_extremes` — (0,0,0)→idx 0; (255,255,255)→idx 251;
  pure red/green/blue land on distinct expected indices.
- `sixel_header_and_raster_attributes` — output starts
  `\x1bP0;1;0q"1;1;<w>;<h>` and ends `\x1b\\`.
- `sixel_encodes_known_image_deterministically` — a constructed 4×6 RGB
  image (two colors, left half red, right half white): assert the **exact
  byte string** — palette defs for the two used indices ascending, one band,
  `#<red>!2~$#<white>!2~-` shape (the literal expected string is computed
  once by hand and frozen in the test).
- `sixel_rle_runs_and_literals` — runs of length ≤3 are emitted literally,
  ≥4 as `!<n><ch>`.
- `sixel_palette_registers_only_used_colors`.
- `sixel_height_clamped_to_band_multiple` — a 4×8 input raster is encoded
  as 6 rows (callers pre-clamp; the encoder asserts/truncates defensively).
- `emit_sixel_gates_and_erases_like_kitty` — same `EmitState` behavior;
  erase overwrites the recorded region with spaces (no DCS delete exists).

**Implementation:** `fn sixel_encode(rgb: &RgbImage) -> Vec<u8>` — one pass
to collect used palette indices + per-pixel index buffer; emit palette; per
6-row band, per used color, walk columns building the 6-bit char
(`0x3F + bits`), RLE on the fly. `emit_sixel(w, key, rgb, origin)` mirrors
`emit_kitty` (records region from raster px via `cells_for`). Performance
note in code: worst case 252 colors × width per band — bounded by pane size
(≤ ~200×60 cells ≈ 1600×1200 px), single-digit ms; fine for the gated path.

Verify: `cargo test sixel`.

## Step 6 — Draw-path dispatch + manager frame hooks

**Files:** `src/panel/preview.rs` (`FilePreview::draw` image branch),
`src/panel/manager.rs` (`draw()` only).

**Failing tests first:**
- `resized_rgb_pixel_keying_reuses_cache` (preview.rs) — request the same
  pixel box twice → `resize_count() == 1`; a changed box recomputes. (Mostly
  covered by existing tests; add one with pixel-scale numbers to document
  the new unit.)
- `graphics_row_budget_matches_half_block_split` — pure helper
  `image_rows(height: u16, has_info: bool) -> u16` returns `height` /
  `2*height/3`, mirroring today's `2*height` / `4*height/3` px behavior; the
  half-block branch is refactored to use it too (guards drift).
- Gating/erase integration is unit-covered in steps 4–5; the draw fn itself
  stays untestable off-terminal (takes `Stdout`) — asserted in step 7 e2e.

**Implementation:**
- `FilePreview::draw`, `Preview::Image` branch with `img.is_some()`:
  ```rust
  let proto = graphics::protocol();
  if proto != GraphicsProtocol::HalfBlock && graphics::frame_allows_image() {
      match draw_graphics(proto, stdout, resize_cache, src, info, &x_range, &y_range, path, modified) {
          Ok(rows_used) => { /* info lines + blanking below, then return */ }
          Err(e) => { log::debug!("graphics: emit failed, half-block fallback: {e}"); /* fall through */ }
      }
  }
  // existing half-block loop, unchanged
  ```
  `draw_graphics` computes: geometry (`cell_geometry()`; `None` + Kitty →
  assumed 8×16 per D2, `None` + Sixel → `Err` → fallback), the cell/pixel
  boxes (D8), pulls the raster via the existing `resized_rgb(resize_cache,
  src, px_w, px_h)`, builds `EmitKey` (path, `mtime_secs(modified)`, px box,
  origin `(x_range.start + 1, y_range.start)`), blanks the image cell region
  with spaces **only when re-emitting** (a gated frame must not stamp spaces
  over live sixel pixels), `MoveTo(origin)`, dispatches to
  `emit_kitty`/`emit_sixel`, claims the frame, returns the cell rows used so
  the existing info-line/blanking tail (preview.rs:161–174) runs unchanged
  below the image.
- `PanelManager::draw` (manager.rs:857): after `cursor::Hide` —
  `graphics::begin_frame(matches!(self.view, ViewMode::Single) &&
  !self.overlay_active())` where `overlay_active()` is a tiny helper on the
  existing `self.mode` (`Mode::Modal` with `region() == ConsoleOverlay`);
  before `EndSynchronizedUpdate` — `graphics::end_frame(&mut self.stdout)?`.
  Two lines; footer/header/panels/console/log ordering untouched, single
  dirty bit untouched.

Verify: `cargo test`, `cargo clippy`, then manual in a kitty/WezTerm/foot
terminal (the design's expected manual check): sharp image, navigate away →
erased, `j`/`k` over text files → no retransmit log spam, resize → re-emit,
`gT` overlay → image vanishes, `!` split → image vanishes, back → returns
(via `refresh_focused_preview`'s redraw).

## Step 7 — Debug socket + docs + e2e

**Files:** `src/debug.rs`, `src/panel/manager.rs` (`state_snapshot`,
line 1517), `examples/config.toml` (done in step 1 — re-verify), `CLAUDE.md`.

**Failing tests first:**
- `snapshot_serializes_to_json` (debug.rs, extend existing): snapshot with
  `image_protocol: "half-block".into()` → JSON contains
  `"image_protocol":"half-block"`.

**Implementation:** `StateSnapshot.image_protocol: String`, filled from
`graphics::protocol().name()` in `state_snapshot`. CLAUDE.md "Architecture:
rendering" gains a short paragraph: graphics previews are the second
exception to blit-cheapness; the claim/end_frame reconcile keeps the
z-order invariant; `image_protocol` is on `state`.

**E2e script** (per the CLAUDE.md loop; run manually and keep the recipe in
the plan-execution notes, not committed as CI — pixels can't render under
tmux):
1. Fixture with a real PNG (`convert`-free: write a tiny PNG via
   `cargo run --example` — no; simplest: check in nothing, generate with
   `printf` of a known base64 PNG in the script). Scratch `--config` dir
   whose `config.toml` leaves `image_protocol = "auto"`.
2. Launch in tmux (`TERM=tmux-256color` inside): `state` →
   `"image_protocol":"half-block"` (env heuristic — asserts D9);
   `capture-pane -e` contains `▄` with 24-bit SGR on the image selection.
3. Relaunch with `image_protocol = "sixel"` in the scratch config: `state`
   reports `"sixel"`; select the PNG, `await-idle`, poll `seq` stable; `log`
   contains `graphics: sixel emit`; press `k`/`j` back onto it after a
   noop-repaint (e.g. toggle log) → `log` shows `graphics: gated`; move to a
   text file → `log` shows the erase line and `capture-pane` shows no
   `\x1bP` leaking as literal text.
4. Same for `image_protocol = "kitty"` asserting transmit/gated/erase log
   lines and `state`.
5. Teardown per CLAUDE.md (kill session, rm fixture + socket).

Verify: `cargo test`, full manual e2e pass, `cargo clippy`.

## Step 8 (optional polish, separate commit) — ordered dithering for sixel

Bayer 4×4 threshold matrix applied during quantization
(`sixel_quantize_dithered`), behind nothing (always on — it's cheap).
Tests: `dither_shifts_midtones_deterministically` (fixed input → exact index
matrix), existing exact-byte test updated. Skip if visual results without it
are acceptable in the step-6 manual check.

## Commit plan

One commit per step (steps 1–7; 8 optional), each with tests green:
1. `feat(config): image_protocol switch for graphics previews`
2. `feat(preview): graphics protocol detection & geometry math`
3. `feat(preview): startup probe and pixel geometry via TIOCGWINSZ`
4. `feat(preview): kitty graphics emitter with id lifecycle and gating`
5. `feat(preview): hand-rolled sixel encoder`
6. `feat(preview): dispatch image previews through kitty/sixel`
7. `feat(debug): expose active image protocol; docs`
Trailer on every commit:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`

## Risks / open ends

- Terminals that answer the kitty query but render placements poorly
  (older Konsole): `image_protocol = "half-block"` is the escape hatch;
  detection prefers the probe's actual OK reply over `$TERM` guessing.
- `q=2` failing on ancient kitty versions (pre-0.19 lacked quiet mode) →
  stray APC responses in the event stream; crossterm drops unrecognized
  sequences, but if key-noise is observed the probe can gate on the kitty
  version… out of scope, documented caveat.
- The 960×540 decode cap (raster cache contract) bounds fidelity on very
  large panes; revisiting it belongs to the raster-cache track, not here.
