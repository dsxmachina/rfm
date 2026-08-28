# iTerm2 Inline-Images Protocol Arm — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add the iTerm2 inline-images protocol (OSC 1337 `File=`) as a fourth
graphics arm beside kitty/sixel/half-block, auto-detected for iTerm2 and
pinnable via `image_protocol = "iterm2"`.

**Architecture:** A new `GraphicsProtocol::Iterm2` variant with an
`emit_iterm2` sibling in `src/panel/graphics.rs` sharing the existing
EmitKey/LIVE/CLAIMED gating and frame reconcile. OSC 1337 images are **cell
content** (like sixel, no placement ids): `LiveImage.id = None`, erase =
the frame's own repaint, the reconcile never writes. Sizing is in **cells**
(`width=<cols>;height=<rows>`), so geometry is optional (kitty-side:
`cell_geometry().unwrap_or(ASSUMED_CELL)`). The payload is an encoded image
file — we JPEG-encode the pre-fitted raster (≤960×540, quality 85) and send
one un-chunked base64 OSC terminated by BEL. There is **no probe** for this
protocol (no DA1 attribute, no capability query implemented outside iTerm2):
detection is env-only, and only for iTerm2 itself (`TERM_PROGRAM=iTerm.app`,
`LC_TERMINAL=iTerm2` — survives ssh —, `ITERM_SESSION_ID`). WezTerm/Ghostty
stay on kitty (better protocol, already detected). VSCode is deliberately
NOT auto-detected: its `terminal.integrated.enableImages` ships off, so
`TERM_PROGRAM=vscode` does not imply images render — users pin `"iterm2"`.
tmux swallows OSC 1337 like the other protocols; the existing `$TMUX` →
half-block heuristic already covers it.

**Tech stack:** existing `image` crate (`jpeg` feature already in the pinned
list), hand-rolled `b64` helper (graphics.rs:515), no new dependencies.

**Key protocol facts (researched 2026-08-28, sources in git history of this
plan):**
- Sequence: `ESC ] 1337 ; File = k=v;k=v : <base64> BEL` (BEL safest; ST
  also legal). `inline=1` is mandatory (default 0 = file download, renders
  nothing). `width`/`height` bare numbers = cells. `preserveAspectRatio=1`
  (default) = fit-within-box, no stretch. `size=<pre-base64 byte count>` is
  advisory. `doNotMoveCursor=1` is a WezTerm extension; unknown keys are
  ignored elsewhere, so always sending it is harmless and prevents WezTerm
  scroll-at-bottom surprises.
- Renderers: iTerm2, WezTerm, mintty, VSCode (opt-in setting, 32 MiB cap —
  irrelevant at our payload sizes). NOT: Warp, Tabby, alacritty, plain tmux.
- Cursor moves past the image by default; rfm repositions with absolute
  `MoveTo` everywhere and `draw_log`/console re-home the cursor, so no
  special handling beyond `doNotMoveCursor=1`.

---

## Task 1: Enum plumbing end-to-end (compile-coupled, one commit)

Rust's exhaustive matches force both enum variants and every match arm into
one compile unit — this task is "make it compile with a stub emitter",
tests-first where possible.

**Files:**
- Modify: `src/config/mod.rs` (~L71-79 enum, ~L96-100 doc comment, ~L144-157 test)
- Modify: `src/panel/graphics.rs` (enum L33-38, `name()` L43-49, `resolve()`
  L166-186, `init_from` pin arm L755, test tables L963-967 + L1241-1243)
- Modify: `src/panel/preview.rs` (`geometry_for` L98-104, dispatch L154-165,
  test L5652-5658)

**Step 1 — failing tests first:**
- `src/config/mod.rs` `image_protocol_parses_all_values`: add row
  `("iterm2", ImageProtocolChoice::Iterm2),`
- `src/panel/graphics.rs` `resolve_direct_pins` table: add
  `(ImageProtocolChoice::Iterm2, GraphicsProtocol::Iterm2),`
- name() assertion block (~L1241): add `assert_eq!(GraphicsProtocol::Iterm2.name(), "iterm2");`
- `src/panel/preview.rs` geometry test (~L5652): assert
  `geometry_for(GraphicsProtocol::Iterm2)` equals the Kitty arm's value.

**Step 2:** `cargo test` → compile errors (variants don't exist). That is
this task's red.

**Step 3 — minimal implementation:**
- `ImageProtocolChoice`: insert `Iterm2,` between `Kitty` and `Sixel`.
  serde `rename_all = "kebab-case"` yields `"iterm2"` (single word — no
  hyphen before digits); the Step-1 parse test proves it. Update the
  `GeneralConfig.image_protocol` doc comment to
  `auto | kitty | iterm2 | sixel | half-block`.
- `GraphicsProtocol`: insert `Iterm2,` after `Kitty`; `name()` arm → `"iterm2"`.
- `resolve()`: `ImageProtocolChoice::Iterm2 => GraphicsProtocol::Iterm2,` in
  the explicit-pin arms.
- `init_from` explicit-pin arm L755: extend to
  `ImageProtocolChoice::Kitty | ImageProtocolChoice::Iterm2 | ImageProtocolChoice::Sixel`.
  The sixel-without-geometry degrade checks inside stay **sixel-only** —
  iTerm2 sizes in cells and must survive missing geometry exactly like kitty.
- `geometry_for` (preview.rs): merge into the kitty arm:
  `GraphicsProtocol::Kitty | GraphicsProtocol::Iterm2 => Some(graphics::cell_geometry().unwrap_or(ASSUMED_CELL)),`
- Dispatch arm (preview.rs ~L155):
  `GraphicsProtocol::Iterm2 => graphics::emit_iterm2(stdout, key, rgb, cols, rows)?,`
- Stub in graphics.rs (full version in Task 2):
  ```rust
  pub fn emit_iterm2(
      _w: &mut impl Write,
      _key: EmitKey,
      _rgb: &image::RgbImage,
      _cols: u16,
      _rows: u16,
  ) -> io::Result<Emitted> {
      Err(io::Error::new(io::ErrorKind::Unsupported, "iterm2 emitter: not yet implemented"))
  }
  ```
  (Emit errors already degrade to the half-block loop for the frame, so the
  tree stays shippable between commits.)

**Step 4:** `cargo test` → all green. **Step 5:** commit
`feat(graphics): plumb iterm2 protocol variant through config/resolve/dispatch`.

---

## Task 2: `emit_iterm2` (TDD against a `Vec<u8>` sink)

**Files:**
- Modify: `src/panel/graphics.rs` (replace stub; new tests mirroring the
  `emit_sixel`/`end_frame` tests at ~L1420-1560, using `emit_test_guard()`)

**Step 1 — failing tests** (mirror the existing sink-test scaffolding —
`emit_test_guard()`, `begin_frame(true)`, shared `EmitKey` helper if one
exists in that test mod):
- `emit_iterm2_single_osc_jpeg_payload`: emit a small RgbImage (e.g. 8×6)
  with cols=12, rows=4; assert the sink contains exactly one
  `"\x1b]1337;File="` occurrence; the argument list contains `inline=1`,
  `size=`, `width=12`, `height=4`, `preserveAspectRatio=1`,
  `doNotMoveCursor=1`; the payload between `:` and the trailing `"\x07"`
  starts with `"/9j/"` (base64 of the JPEG magic `FF D8 FF`) and contains
  no ESC byte (single un-chunked payload).
- `emit_iterm2_gates_on_unchanged_key`: second emit with the same key →
  `Emitted::Gated`, zero bytes written.
- `emit_iterm2_end_frame_reconciles_without_writes`: claimed placement
  survives `end_frame` (empty sink); unclaimed → `end_frame` writes nothing
  (cell-content discipline, id-less) but forgets LIVE, so a re-emit
  transmits again. Mirror `emit_sixel_end_frame_reconciles_without_writes`
  (~L1524).

**Step 2:** `cargo test emit_iterm2` → red (stub returns Unsupported).

**Step 3 — implementation** (sibling of `emit_sixel`, same skeleton):
```rust
/// Transmit `rgb` via the iTerm2 inline-images protocol (OSC 1337 File=),
/// gated on `key`: unchanged live key → claim + `Gated`, zero bytes.
/// Otherwise blank the target region (half-block remnants), park the
/// cursor at the origin and send ONE un-chunked base64 JPEG payload sized
/// in cells (`width=`/`height=`), `preserveAspectRatio=1` (the raster is
/// pre-fitted; this only guards cell-aspect drift — blank_cells repaints
/// any letterbox strip). Inline images are cell content like sixel — no
/// placement ids, the frame repaint erases them — so `id: None` and the
/// reconcile stays write-free. `doNotMoveCursor=1` is WezTerm-only sugar,
/// ignored elsewhere.
pub fn emit_iterm2(
    w: &mut impl Write,
    key: EmitKey,
    rgb: &image::RgbImage,
    cols: u16,
    rows: u16,
) -> io::Result<Emitted> {
    if LIVE.lock().as_ref().map_or(false, |l| l.key == key) {
        *CLAIMED.lock() = Some(key);
        log::trace!("graphics: gated (unchanged)");
        return Ok(Emitted::Gated);
    }
    if let Some(old) = LIVE.lock().take() {
        delete_placement(w, &old)?; // no-op for id-less; keeps emitters uniform
    }
    let region = (
        key.origin_cell.0..key.origin_cell.0.saturating_add(cols),
        key.origin_cell.1..key.origin_cell.1.saturating_add(rows),
    );
    blank_region(w, &region)?;
    move_to(w, key.origin_cell)?;

    let jpeg = encode_jpeg(rgb)?;
    log::debug!(
        "graphics: iterm2 emit {}x{}px, {} JPEG bytes into {cols}x{rows} cells",
        key.px_w, key.px_h, jpeg.len()
    );
    write!(
        w,
        "\x1b]1337;File=inline=1;size={};width={cols};height={rows};preserveAspectRatio=1;doNotMoveCursor=1:{}\x07",
        jpeg.len(),
        b64(&jpeg)
    )?;

    *LIVE.lock() = Some(LiveImage { key: key.clone(), id: None });
    *CLAIMED.lock() = Some(key);
    Ok(Emitted::Transmitted)
}

/// JPEG for the OSC 1337 payload (the protocol carries encoded image
/// FILES, not raw pixels — PNG/JPEG are the universally accepted formats,
/// JPEG is ~10x smaller for photographic previews). Quality 85 keeps a
/// 960×540 raster far below every implementation's size cap.
fn encode_jpeg(rgb: &image::RgbImage) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(rgb)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(out)
}
```
(Match the exact lock/claim ordering of `emit_kitty` L342-377 when writing
the final code — the above mirrors it; adapt if the real code differs.)

**Step 4:** `cargo test` → green. **Step 5:** commit
`feat(graphics): iterm2 OSC 1337 inline-images emitter`.

---

## Task 3: Env auto-detection (iTerm2 only)

**Files:** Modify `src/panel/graphics.rs` (`detect_from_env` L59-77 + its tests)

**Step 1 — failing tests** (mirror existing env-closure test style):
- `TERM_PROGRAM=iTerm.app` → `Some(Iterm2)`
- `LC_TERMINAL=iTerm2` (nothing else) → `Some(Iterm2)` (ssh case)
- `ITERM_SESSION_ID=w0t0p0:...` (nothing else) → `Some(Iterm2)`
- `TMUX=/tmp/...` + `TERM_PROGRAM=iTerm.app` → `Some(HalfBlock)` (tmux wins)
- `TERM_PROGRAM=WezTerm` → still `Some(Kitty)` (ordering regression guard)
- `TERM_PROGRAM=vscode` → `None` (falls to probe → half-block; images
  setting ships off, never auto-detect)

**Step 2:** red. **Step 3:** after the wezterm/ghostty check (L70-75), add:
```rust
// iTerm2: OSC 1337 has no probeable capability (no DA1 attr, and the
// Capabilities query is implemented by iTerm2 alone) — env is the only
// signal. LC_TERMINAL survives ssh via shell integration; the session id
// covers local sessions without it. VSCode/Warp/Tabby are deliberately
// absent: VSCode ships images off, the others don't render OSC 1337.
if env("TERM_PROGRAM").map_or(false, |v| v.eq_ignore_ascii_case("iTerm.app"))
    || env("LC_TERMINAL").map_or(false, |v| v.eq_ignore_ascii_case("iTerm2"))
    || env("ITERM_SESSION_ID").is_some()
{
    return Some(GraphicsProtocol::Iterm2);
}
```
(Match the surrounding closure/style exactly.)

**Step 4:** green. **Step 5:** commit
`feat(graphics): auto-detect iTerm2 via env (TERM_PROGRAM/LC_TERMINAL/session id)`.

---

## Task 4: Pin behavior in `init_from` (regression tests; may be green from Task 1)

**Files:** Modify `src/panel/graphics.rs` (tests near `init_*` L1131-1220)

Behavior to lock (arm already extended in Task 1 — these tests pin it down;
green-on-arrival is acceptable, they are regression guards):
- `init_iterm2_pin_with_geometry_never_probes`: explicit `Iterm2` + working
  ioctl winsize → protocol `Iterm2`, probe closure never invoked (mirror
  `init_with_explicit_choice_never_reads` L1131).
- `init_iterm2_pin_without_geometry_keeps_iterm2`: explicit `Iterm2`, no
  ioctl geometry, probe times out → STILL `Iterm2` with `None` geometry
  (contrast `init_sixel_without_geometry_degrades_to_half_block` L1185 —
  the degrade is sixel-only; iTerm2 sizes in cells like kitty).

Commit: `test(graphics): lock iterm2 pin/geometry semantics in init_from`.

---

## Task 5: Docs + annotated defaults

**Files:**
- Modify: `examples/default-config.toml` L80-104 (this IS the docs and the
  `--dump-config` output — CLAUDE.md contract)
- Modify: `docs/configuration.md` L118-119 (value list) + L145-161 (prose)

Changes:
- New bullet between kitty and sixel:
  ```
  #   - "iterm2"     : pin the iTerm2 inline-images protocol (iTerm2; also
  #                    WezTerm, mintty, and VSCode — VSCode only renders it
  #                    when terminal.integrated.enableImages is on, which
  #                    ships off, so it is never auto-detected).
  ```
- "auto" bullet: extend the env-heuristics parenthetical to
  `(kitty/WezTerm/Ghostty → kitty, iTerm2 → iterm2)`.
- tmux paragraph: add that pinning `"iterm2"` inside tmux also leaves the
  preview blank (tmux discards OSC 1337; no passthrough wrapping).
- `docs/configuration.md`: mirror both (value list line + prose paragraph).

Verify: `cargo test config::` (defaults_tests still green — the embedded
file must still deserialize). Commit:
`docs(config): document the iterm2 image_protocol value`.

---

## Task 6: e2e smoke via tmux + debug socket

No committed artifact; verification per CLAUDE.md's tmux loop. Inside tmux
the OSC is swallowed — the point is state/log/no-crash, not pixels:
1. Scratch config dir with `image_protocol = "iterm2"`; launch rfm in tmux
   with `--debug-socket`, fixture containing a real JPEG (generate with the
   `image` crate or copy a test asset).
2. `state` → `image_protocol == "iterm2"`.
3. Select the image, `await-idle`, poll `seq` stable; `log` → expect the
   `graphics: iterm2 emit ...` debug line, no error/warn lines.
4. `capture-pane` → pane strip beside/under the (invisible) raster intact,
   no escape garbage leaking into cells, footer/log rows uncorrupted.
5. Default-auto regression: launch WITHOUT the pin inside tmux → `state`
   still reports `half-block`.

---

## Task 7: Final gates

`cargo test` (full), `cargo clippy` (zero new warnings), `cargo build`.
Fix anything found, commit fixes separately. Do NOT bump MSRV; no new
dependencies were added, so no pin verification needed.
