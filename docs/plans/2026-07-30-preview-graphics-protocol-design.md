# High-Fidelity Image Rendering (Kitty / Sixel) — Design

Status: proposed, 2026-07-30
Group E of the preview overhaul (`feature-better-previews.md`). Standalone: it
adds no dependency and does not touch the dispatch/backends work — it changes how
an already-decoded `Preview::Image` is *drawn*. Biggest visual upgrade in the
whole effort, and the most invasive to the draw path, so it lives on its own
track.

## Motivation

`FilePreview::draw` (`src/panel/preview.rs:82`) renders images as truecolor
half-blocks: two vertical pixels per character cell via the `▄` glyph with fg/bg
colors (preview.rs:131–158). That caps effective resolution at 2 px per cell and
always looks blocky. Terminals that implement a **graphics protocol** — the
kitty graphics protocol or DEC **sixel** — can display actual pixels, turning the
preview column into a near-real image. This is achievable with *no new crate*
(both protocols are escape sequences over stdout) and gives the single most
dramatic quality jump available.

## Approach: capability tiers with half-block fallback

Detect the best protocol the terminal supports once at startup and pick a
renderer per preview, always falling back to the current half-block path:

1. **kitty graphics protocol** — best quality, used by kitty, WezTerm, Ghostty.
2. **sixel** — widely supported (xterm with `--enable-sixel`, foot, WezTerm,
   mlterm, Konsole).
3. **half-blocks** — today's universal fallback, unchanged.

### Detection
Add a `GraphicsProtocol { Kitty, Sixel, HalfBlock }` resolved once via
`OnceCell` (the pattern already used for `FFMPEG_INSTALLED`, preview.rs:322):
- Kitty: query with the graphics-protocol detection escape and read the reply,
  or accept the pragmatic env heuristic (`$TERM`/`$KITTY_WINDOW_ID`/
  `$TERM_PROGRAM`) as a first cut; a proper `\e_Gi=…\e\\` + primary-DA probe is
  the robust version.
- Sixel: parse the response to Primary Device Attributes (`\e[c`) for the `4`
  (sixel) capability.
- Detection must be **non-blocking and time-bounded** — it reads a terminal
  reply on stdin, which the event loop owns; do it during init before raw-mode
  input handling starts, or behind a short timeout, so a non-responding terminal
  can't wedge startup. On any uncertainty, fall to `HalfBlock`.

Expose the resolved protocol on the debug socket `state` (like `view`) so the
tmux/socket harness can assert which path is active.

## Draw path changes

`FilePreview::draw` (preview.rs:82) currently owns the half-block loop. Refactor
its `Preview::Image` branch to dispatch on the protocol:

```rust
match graphics_protocol() {
    GraphicsProtocol::Kitty  => draw_kitty(stdout, rgb, x_range, y_range, info)?,
    GraphicsProtocol::Sixel  => draw_sixel(stdout, rgb, x_range, y_range, info)?,
    GraphicsProtocol::HalfBlock => { /* existing loop, unchanged */ }
}
```

Key constraints, because this bypasses the cell-blit model the rest of the
renderer assumes:

- **Sizing:** graphics protocols place pixels, but rfm lays out in *cells*. Need
  the cell→pixel ratio. Query it once (kitty/sixel report it, or `TIOCGWINSZ`
  `ws_xpixel/ws_ypixel`); derive target pixel dimensions from the panel's
  `x_range`/`y_range` cell span. Reuse the existing `resize_cache`
  (preview.rs:38) keyed on the target pixel size so re-draws don't re-encode.
- **Placement & clipping:** emit a `MoveTo(x_range.start+1, y_range.start)` then
  the image escape. The image must be clipped to the preview column; kitty
  supports explicit rows/cols, sixel needs the raster pre-sized to fit so it
  doesn't overflow into neighbouring panels.
- **Z-order / overwrite:** the draw sequence (footer → header → panels → console
  → log → overlay, per CLAUDE.md "Architecture: rendering") assumes each draw
  fully paints its region. A graphics image does not clear the cells it leaves —
  on selection change or panel resize the old image must be explicitly erased
  (kitty delete command; sixel: overwrite the region with spaces first). The
  info-line footer (preview.rs:161) stays as normal cell text below the image.
- **Full-repaint cost:** CLAUDE.md notes every draw is a cheap blit "except the
  image preview, whose resize is cached". A graphics image re-transmitted on
  every repaint is *not* cheap. Gate re-emission: only re-send the image when the
  source/selection or target pixel size changed, not on unrelated dirty repaints.
  This likely needs a small "last transmitted image id/params" on `FilePreview`.

## Config

One switch in `[general]`, to force a protocol or disable graphics for terminals
that misreport:

```toml
image_protocol = "auto"   # "auto" | "kitty" | "sixel" | "half-block"
```

`auto` runs detection; the explicit values pin the renderer (and skip probing).
Default `auto`. Documented caveat: multiplexers (tmux, screen) often break
passthrough — detection should treat "inside tmux without passthrough" as
`half-block` unless explicitly overridden.

## Error handling

- Detection failure or timeout → `HalfBlock`. Never wedge on a terminal reply.
- Any encode/emit error in `draw_kitty`/`draw_sixel` → log at `debug`, fall back
  to the half-block loop for that frame. A preview always renders *something*.
- Stale-image artifacts (image not cleared on some terminal) are a correctness
  bug, not a crash — the erase-before-draw rule above is the guard; test for it.

## Testing

Graphics protocols are hard to assert from `capture-pane` (tmux shows the escape
bytes, not pixels), so testing leans on state + logs and manual verification:

- **Unit (terminal-free):** protocol detection given synthetic env / DA replies
  resolves to the expected tier; explicit `image_protocol` config pins it
  without probing; pixel-dimension math from a cell range + cell/pixel ratio.
- **Integration (tmux + socket):** launch under a sixel-capable terminal in the
  pane, preview an image, assert `state` reports the active protocol; navigate
  away and back and assert (via `log`/state) the image is re-emitted only when
  params change, and that the region is erased on selection change (no leftover
  escape stream). Under plain `TERM=xterm` assert it falls to half-block.
- **Manual:** the blocky-vs-sharp difference is inherently visual — a manual
  check in kitty/WezTerm/foot is expected before shipping.

## Out of scope

- iTerm2's inline-image protocol (a third format) — add later if requested;
  the `GraphicsProtocol` enum leaves room.
- Animated/GIF playback in the preview.
- Graphics in the *split* view — split renders only center columns with no
  preview (CLAUDE.md "tabs & split view"); this is single-view preview only.
- tmux passthrough configuration — documented as a caveat, not solved here.
