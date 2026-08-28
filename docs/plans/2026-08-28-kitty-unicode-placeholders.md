# Kitty Unicode-Placeholder Protocol Arm — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `GraphicsProtocol::KittyUnicode` — kitty graphics transmitted as
a virtual placement (`U=1`) and rendered as U+10EEEE placeholder *cells* —
so image previews work inside tmux (outer terminal kitty ≥ 0.28 or Ghostty),
auto-detected via a passthrough-wrapped probe and pinnable as
`image_protocol = "kitty-unicode"`.

**Architecture:** The image data is transmitted with the existing kitty APC
path plus `U=1` (virtual placement; `c=`/`r=` pin the cell box, aspect-fit is
a no-op because the raster is already pre-fitted). The visible artifact is a
grid of U+10EEEE cells — ordinary text: row/column encoded as combining
diacritics, image id in the 24-bit foreground color. Placeholder cells are
CELL CONTENT (repaint overwrites them) but the transmitted image data is
id-addressed, so `LiveImage.id = Some(id)` and the reconcile still deletes by
id (`a=d,d=I`) — a hybrid of the kitty and sixel disciplines. Inside tmux
every APC (transmit chunks, delete, probe query) is wrapped in the
`ESC Ptmux;`/ESC-doubled passthrough envelope; the placeholder text passes
through tmux untouched — that is the entire point. Auto-detection inside tmux
probes through passthrough ONLY when fossil env hints (KITTY_WINDOW_ID,
GHOSTTY_RESOURCES_DIR) suggest a capable outer terminal — everyone else keeps
today's instant half-block, zero startup cost. `allow-passthrough on`
(tmux ≥ 3.3) is the user's job (documented); rfm never runs the tmux CLI. A
probe timeout (wrapped queries silently dropped) degrades to half-block.

**Tech stack:** no new dependencies. Embedded 297-entry diacritics table
(canonical source: `gen/rowcolumn-diacritics.txt` in the kitty repo).

**Researched facts this plan relies on (sources in the research workflow,
2026-08-28):**
- Combined transmit+virtual placement in one APC is spec-blessed:
  `a=T,U=1,q=2,f=24,s=,v=,i=,c=,r=` on chunk 0; `q=2` on every chunk.
- Placeholder encoding: U+10EEEE + diacritic(row) [+ diacritic(col)
  + diacritic(id MSB)]; trailing cells of a row may omit diacritics
  (left-neighbor inheritance; spec: "specifying only row diacritics of the
  first column" is sufficient per row). Fg color 38;2;R;G;B carries id bits
  23-16/15-8/7-0. Underline color (placement id) omitted — single virtual
  placement per image. NO other attributes (reserved). Never let a row wrap:
  absolute MoveTo per row.
- Diacritics table: 297 entries; [0..=11] are U+0305, U+030D, U+030E,
  U+0310, U+0312, U+033D, U+033E, U+033F, U+0346, U+034A, U+034B, U+034C.
  Clamp rows/cols to 297.
- Deletion: `a=d,d=I,i=` is correct for virtual placements (no d=U exists);
  it frees image data. Composited pixels also vanish when the placeholder
  cells are overwritten as text — repaint erases the visible image even
  without the delete.
- tmux passthrough: `\x1bPtmux;` + payload with ONLY 0x1b doubled
  (including the inner terminator's ESC) + `\x1b\\`. One envelope per APC
  chunk (4096-char base64 chunks stay far below tmux's 1 MiB DCS cap;
  oversize = silent drop). allow-passthrough: default off since tmux 3.3;
  pre-3.3 passthrough was unconditional; no in-band readback exists —
  probe-timeout is the detection.
- Probe-through-tmux: tmux consumes only its OWN startup DA1 (TTY_HAVEDA);
  later replies from the outer terminal are forwarded to the active pane
  byte-for-byte-in-practice. A PLAIN DA1 inside tmux is answered by tmux
  itself (instantly, describing tmux) — so inside tmux the probe must wrap
  BOTH the kitty APC query AND the DA1 fence (yazi's pattern); an unwrapped
  DA1 would fence before the outer reply arrives. CSI 14 t is answered by
  tmux with pane-local pixels (correct values for us; never wrap it).
  TIOCGWINSZ works in panes since tmux 3.1.
- Support matrix: kitty ≥ 0.28, Ghostty. NOT WezTerm/Konsole/xterm.js.
  Inside tmux, probe outcome maps kitty_ok → KittyUnicode ONLY; a sixel
  attr in a wrapped outer DA1 must NOT map to sixel (outer sixel ≠ tmux
  passes sixel) — anything else → HalfBlock.
- tmux env fossils: the server keeps the env of wherever it started;
  KITTY_WINDOW_ID/GHOSTTY_RESOURCES_DIR are stale after cross-terminal
  re-attach (both directions). Acceptable: wrong-positive costs one 250 ms
  probe timeout; wrong-negative is cured by the pin. TERM_PROGRAM inside
  tmux is "tmux" (tmux ≥ 3.2 sets it) — useless as an outer hint.
- e2e leverage: U+10EEEE + combining diacritics survive `capture-pane -p`
  byte-for-byte (verified on this machine) — placeholder cells are directly
  assertable in the tmux harness, pinned, with no capable outer terminal.

---

## Task 1: Enum plumbing end-to-end (compile-coupled, stub emitter)

Mirror of the iterm2 arm's Task 1 (commit 3025c23 is the template).

**Files:** `src/config/mod.rs`, `src/panel/graphics.rs`, `src/panel/preview.rs`

**Step 1 — failing tests first:**
- config `image_protocol_parses_all_values`: add
  `("kitty-unicode", ImageProtocolChoice::KittyUnicode),` (serde kebab-case
  of `KittyUnicode` is exactly that — same split as `HalfBlock`); optionally
  a negative row asserting `"kittyunicode"` errs.
- graphics `explicit_choice_wins_without_probe` table:
  `(ImageProtocolChoice::KittyUnicode, GraphicsProtocol::KittyUnicode),`
- `protocol_names_match_config_values`:
  `assert_eq!(GraphicsProtocol::KittyUnicode.name(), "kitty-unicode");`
- preview geometry test: `geometry_for(GraphicsProtocol::KittyUnicode)`
  equals the kitty arm (cell-sized, tolerates missing geometry).

**Step 2:** `cargo test` → E0599 compile errors = red.

**Step 3 — minimal implementation:**
- `ImageProtocolChoice::KittyUnicode` between `Kitty` and `Iterm2`; field doc
  → `auto | kitty | kitty-unicode | iterm2 | sixel | half-block`.
- `GraphicsProtocol::KittyUnicode` after `Kitty`; `name()` → `"kitty-unicode"`.
- `resolve()` pin arm; `init_from` explicit-pin arm becomes
  `Kitty | KittyUnicode | Iterm2 | Sixel` (geometry-probe-if-missing;
  sixel-only degrade untouched — KittyUnicode sizes in cells).
- `geometry_for`: join the `Kitty | Iterm2` arm.
- Dispatch arm in `draw_graphics`:
  `GraphicsProtocol::KittyUnicode => graphics::emit_kitty_unicode(stdout, key, rgb, cols, rows)?,`
- Stub `emit_kitty_unicode` returning `ErrorKind::Unsupported` (frame falls
  back to half-block → tree shippable between commits).

**Step 4:** green. **Step 5:** commit
`feat(graphics): plumb kitty-unicode protocol variant through config/resolve/dispatch`.

---

## Task 2: tmux passthrough wrapper + passthrough state + socket exposure

**Files:** `src/panel/graphics.rs`, `src/debug.rs`, `src/panel/manager.rs`,
`docs/test-protocol/README.md` (state-field list)

**Step 1 — failing tests:**
- `passthrough_wraps_and_doubles_escapes`: for input
  `b"\x1b_Ga=T;AAAA\x1b\\"` the wrapped form is exactly
  `b"\x1bPtmux;\x1b\x1b_Ga=T;AAAA\x1b\x1b\\\x1b\\"` — envelope prefix,
  every 0x1b doubled INCLUDING the inner terminator's, envelope `\x1b\\`
  appended; input without any ESC is wrapped verbatim; empty input yields
  just the envelope.
- `passthrough_state_defaults_off`: `graphics::passthrough()` is false in
  tests (OnceLock-style default, mirroring `protocol()`).
- debug.rs: extend the two literal `StateSnapshot` test constructors and the
  JSON assertion with the new field `graphics_passthrough: false`.

**Step 3 — implementation:**
- `fn wrap_passthrough(seq: &[u8]) -> Vec<u8>` (pure, sibling of `b64`).
- `static PASSTHROUGH: AtomicBool` + `pub fn passthrough() -> bool` +
  a test-visible setter following the file's existing init/test-seam
  conventions (`emit_test_guard` protected). Set in `init` (Task 5 wires the
  real value: in_tmux && resolved == KittyUnicode).
- `StateSnapshot.graphics_passthrough: bool` (docstring: whether graphics
  APC output is tmux-passthrough-wrapped), filled in manager.rs next to
  `image_protocol`; add to the README.md state-field list.

**Step 4:** green, full `cargo test`. **Step 5:** commit
`feat(graphics): tmux passthrough envelope + passthrough state on the debug socket`.

---

## Task 3: Diacritics table + placeholder grid renderer

**Files:** `src/panel/graphics.rs` (consider `src/panel/graphics/placeholder.rs`
as a sibling module like `sixel.rs` — decide by size; the table alone is
~300 lines)

**Step 1 — failing tests:**
- `diacritics_table_spot_checks`: length 297; [0]=='\u{0305}',
  [1]=='\u{030D}', [2]=='\u{030E}', [11]=='\u{034C}'.
- `placeholder_grid_emits_rows`: for origin (10,2), cols 3, rows 2,
  id 0x4d46: two rows, each = CUP to (row start) + `\x1b[38;2;0;77;70m`
  (0x4d46 → R 0, G 0x4d, B 0x46) + U+10EEEE + DIACRITICS[row] +
  DIACRITICS[0] + DIACRITICS[0 /*id MSB*/] + 2× bare U+10EEEE + `\x1b[39m`
  reset. Assert byte-exact per row (build expected strings in the test).
- `placeholder_grid_clamps_at_table_size`: rows/cols beyond 297 emit at most
  297 (no panic, no out-of-range index).

**Step 3 — implementation:**
- `static DIACRITICS: [char; 297]` — generate from the canonical list:
  fetch https://raw.githubusercontent.com/kovidgoyal/kitty/master/gen/rowcolumn-diacritics.txt
  (one hex codepoint per line, comments with `#`) and emit the array with a
  throwaway script into the source file. The spot-check test guards
  transcription; also assert in a test that the table is strictly ascending
  (the canonical list is) to catch garbled entries wholesale.
- `fn placeholder_grid(w: &mut impl Write, origin: (u16,u16), cols: u16, rows: u16, id: u32) -> io::Result<()>`:
  per row: `move_to` (never let a row wrap — absolute positioning per row),
  one fg-SGR carrying the id's low 24 bits, first cell with row+col0+MSB
  diacritics (MSB = `(id >> 24) as u8`, belt-and-braces so any u32 id is
  correct), remaining cells bare U+10EEEE (spec's left-neighbor inheritance),
  `\x1b[39m` after each row (fg only — placeholder cells must carry no other
  attributes; the file's SGR conventions apply).

**Step 4:** green. **Step 5:** commit
`feat(graphics): U+10EEEE placeholder grid renderer + rowcolumn diacritics table`.

---

## Task 4: Virtual-placement transmit + `emit_kitty_unicode`

**Files:** `src/panel/graphics.rs`

**Step 1 — failing tests** (sink-based, `emit_test_guard()`, mirror the
kitty/iterm2 emit tests; force the passthrough flag per-test via the Task-2
seam):
- `emit_kitty_unicode_transmits_virtual_placement_with_grid`: sink contains
  exactly one APC chunk-0 with `a=T`, `U=1`, `q=2`, `f=24`, `i=`, `c=12`,
  `r=4`, `s=`, `v=`; chunking still `m=1`/`m=0`; AFTER the last chunk the
  placeholder grid rows follow (U+10EEEE present, fg-SGR encodes the SAME id
  as `i=`); `Emitted::Transmitted`; LIVE holds `id: Some(same id)`.
- `emit_kitty_unicode_wraps_chunks_when_passthrough`: with passthrough ON,
  every APC chunk is individually enveloped (`count of "\x1bPtmux;"` ==
  chunk count), the placeholder grid text is NOT wrapped (no envelope after
  the last chunk), and the inner APC terminators are ESC-doubled. With
  passthrough OFF (sibling assertion): zero envelopes.
- `emit_kitty_unicode_gates_on_unchanged_key`: second emit → `Gated`, zero
  bytes (grid persists as cell content, image persists — nothing to redraw).
- `emit_kitty_unicode_end_frame_deletes_by_id`: unclaimed → `end_frame`
  writes `a=d,d=I,i=<id>` (wrapped iff passthrough) and forgets LIVE; the
  delete writes NO cell content. Claimed → empty sink. Mirror the kitty
  sibling test, plus the wrapped variant.

**Step 3 — implementation:**
- Extend `transmit_kitty` with a virtual-placement flag (adds `U=1`; keep
  `c=`/`r=` — they pin the fit box) OR add a thin `transmit_kitty_virtual`
  wrapper — choose whichever keeps the diff smaller; route every chunk
  through a `write_apc(w, bytes)` helper that consults `passthrough()`.
- `emit_kitty_unicode(w, key, rgb, cols, rows)`: gate → delete old (via
  `delete_placement`, which now wraps its APC when `passthrough()`) →
  `blank_region` (uniform with siblings; also clears half-block remnants) →
  transmit virtual → `placeholder_grid(w, key.origin_cell, cols, rows, id)`
  → LIVE `{key, id: Some(id)}`, claim, `Transmitted`. Log line
  `graphics: kitty-unicode emit ...` (debug level, e2e greps it).
- `delete_placement`: the APC delete goes through `write_apc` (classic-kitty
  placements outside tmux are unaffected — passthrough() is only ever true
  when the resolved protocol is KittyUnicode inside tmux, per Task 5).

**Step 4:** green, full suite. **Step 5:** commit
`feat(graphics): kitty-unicode virtual-placement emitter (U=1 + placeholder cells)`.

---

## Task 5: Detection — hinted probe through tmux

**Files:** `src/panel/graphics.rs`

Behavior spec:
- `detect_from_env` tmux arm (TMUX set, or TERM tmux*): if
  `KITTY_WINDOW_ID` or `GHOSTTY_RESOURCES_DIR` is present (fossil hint of a
  capable outer terminal) → return `None` (= fall through to the probe);
  otherwise `Some(HalfBlock)` exactly as today. TERM screen* stays
  instant-HalfBlock unconditionally (GNU screen has no passthrough).
- `init_from` Auto arm: compute `in_tmux` from env (TMUX set or TERM tmux*);
  pass it to the probe sender — inside tmux the kitty APC query AND the DA1
  fence are passthrough-wrapped (an unwrapped DA1 would be answered by tmux
  itself, instantly, fencing before the outer terminal's APC reply arrives);
  `CSI 14 t` stays unwrapped (tmux answers pane-local pixels — the values we
  want). Probe outcome mapping inside tmux: `kitty_ok` → `KittyUnicode`;
  ANYTHING else (incl. a sixel attr in the outer DA1) → `HalfBlock`.
  Outside tmux: mapping unchanged.
- Explicit `KittyUnicode` pin: geometry-probe-if-missing stays UNWRAPPED
  (DA1-from-tmux is a fine fence for the CSI 14 t reply; capability is
  pinned, not probed).
- End of `init`: set `PASSTHROUGH = in_tmux && resolved == KittyUnicode`.

**Step 1 — failing tests** (env-closure + `init_from` seam styles):
- `env_tmux_with_kitty_hint_falls_through_to_probe`: TMUX +
  KITTY_WINDOW_ID → `detect_from_env` returns `None` (today: Some(HalfBlock)
  — red).
- `env_tmux_without_hint_stays_half_block`: TMUX only → `Some(HalfBlock)`;
  `env_screen_term_stays_half_block_despite_hint`: TERM=screen-256color +
  KITTY_WINDOW_ID → `Some(HalfBlock)`.
- Existing `env_tmux_var_resolves_half_block` /
  `env_screen_and_tmux_term_resolve_half_block` / `env_tmux_beats_iterm_vars` /
  `init_auto_env_hit_skips_probe`: update fixtures so they stay hint-free
  (their intent — tmux wins instantly — is unchanged).
- `init_auto_tmux_hinted_probe_kitty_ok_resolves_kitty_unicode`: fake env
  TMUX+KITTY_WINDOW_ID, probe returns a kitty OK reply → protocol
  KittyUnicode, and the probe SENDER was asked for wrapped queries (extend
  the probe seam to record/assert the wrap flag, mirroring how `probe(true)`
  is asserted today).
- `init_auto_tmux_hinted_probe_timeout_is_half_block`: same env, silent
  probe → HalfBlock.
- `init_auto_outside_tmux_kitty_ok_still_classic_kitty`: regression — no
  tmux, kitty_ok → Kitty (NOT KittyUnicode).
- `init_kitty_unicode_pin_sets_passthrough_inside_tmux_only`: pin +
  TMUX env → `passthrough()` true; pin without TMUX → false.

**Step 3:** implement per the behavior spec (the probe seam
`&mut dyn FnMut(bool) -> Vec<u8>` grows the wrap dimension — extend the
signature the way the existing seams do, keeping `send_probe_and_read` the
single real implementation).

**Step 4:** green, full suite. **Step 5:** commit
`feat(graphics): auto-detect kitty-unicode inside tmux via hinted passthrough probe`.

---

## Task 6: Docs sweep — the tmux story changes

**Files:** `examples/default-config.toml`, `docs/configuration.md`,
`CLAUDE.md`, `src/debug.rs` (docstring), `src/config/mod.rs` (enum doc),
`docs/test-protocol/README.md`, `docs/test-protocol/sections/08-…`, `…/09-…`

The blanket statements "inside tmux auto always resolves to half-block" and
"rfm does not wrap its output in tmux's passthrough sequences" become
PARTIALLY false. Rewrite to distinguish the two halves everywhere:
placeholder CELLS are plain text and always survive tmux; the image-data APC
reaches the outer terminal only when passthrough-wrapped AND
`allow-passthrough on` is set (tmux ≥ 3.3; pre-3.3 unconditional) AND the
outer terminal is kitty ≥ 0.28/Ghostty.

- `examples/default-config.toml`: new `"kitty-unicode"` bullet after
  `"kitty"` (what it is, tmux requirements incl. `allow-passthrough on`,
  outer-terminal support matrix, also valid outside tmux); "auto" bullet:
  inside tmux, a hinted probe may now resolve kitty-unicode; rewrite the
  tmux paragraph (kitty/iterm2 pins still un-wrapped and blank; sixel note
  unchanged; kitty-unicode is the exception that IS wrapped); probe note
  gains the kitty-unicode pin + that auto-inside-tmux-with-hint also probes.
- `docs/configuration.md`: mirror all of the above (value list + prose).
- `CLAUDE.md` rendering section: protocol list; env-heuristics sentence
  (tmux + kitty/ghostty fossil hint → wrapped probe → kitty-unicode);
  erase-discipline sentence (placeholder cells are cell content, image data
  deletes by id — the hybrid); the "tmux swallows all three protocols"
  caveat rewritten; keep terse.
- `src/config/mod.rs` enum doc: the "(They are honored inside tmux too,
  but rfm emits raw sequences without tmux's passthrough wrapping…)"
  sentence — add the kitty-unicode exception.
- `src/debug.rs`: image_protocol docstring value list + the new
  `graphics_passthrough` docstring (if not already done in Task 2).
- `docs/test-protocol`: README harness note (half-block claim → "with the
  default auto config"; add that a kitty-unicode pin makes placeholder cells
  capture-assertable); sections 08/09 half-block assertions get a
  clarifying "(auto, no passthrough hint in the harness)" touch where they
  claim tmux FORCES half-block; section 09's coverage-gap line about
  untestable protocol pins → note kitty-unicode is now partially testable
  in-harness (cells, not pixels).

`cargo test` (defaults_tests) green. Commit:
`docs(config): document kitty-unicode and the new tmux passthrough story`.

---

## Task 7: e2e smoke via tmux + debug socket

No committed artifact. CLAUDE.md tmux-loop conventions (quiet fixture
parent, direct launch, scratch `--config`, `XDG_STATE_HOME`/`XDG_CACHE_HOME`
tempdirs, `await-idle`, teardown incl. socket). `cargo build` first. Real
JPEG fixture (ImageMagick).

1. Pin `image_protocol = "kitty-unicode"` → `state` reports
   `"image_protocol": "kitty-unicode"` and `"graphics_passthrough": true`
   (harness runs inside tmux).
2. Select the image; `log` contains the `graphics: kitty-unicode emit` line;
   zero ERROR/WARN lines.
3. `capture-pane -p | grep -c $'\U0010EEEE'` (or hexdump for `f4 8e bb ae`)
   ≥ 1 — placeholder rows are on screen even though the harness's outer
   terminal renders no pixels. `capture-pane -p -e` shows a `38;2;R;G;B`
   fg-SGR on those rows (id encoding present).
4. Navigate away to a text file → placeholder chars GONE from capture
   (repaint overwrote the cells; reconcile deleted the id — check `log` for
   no errors).
5. Auto regression: no pin, no hint vars in the harness env → `state` shows
   `"half-block"`, `graphics_passthrough: false`, and startup log has NO
   probe line for graphics (instant env resolution, unchanged cost).
6. Hinted-auto probe path: relaunch with `KITTY_WINDOW_ID=1` exported in the
   pane → `state` shows `"half-block"` (probe ran and timed out — the outer
   terminal is not kitty), startup log shows the probe-timeout reason, and
   startup took no pathological delay (await-idle returns; ~250 ms budget).

---

## Task 8: Final gates

Full `cargo test`, `cargo clippy --all-targets` (zero warnings),
`cargo build`. No new dependencies — no MSRV/pin verification needed.
