# Tiered PDF Preview — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.
> **Do NOT use harness worktrees / `EnterWorktree` for this repo** (they
> check out a stale pre-refactor commit) and **never run mutating git**
> (`stash`/`checkout`/`reset`) — the repo holds user WIP stashes.

**Goal:** PDFs currently fall into the `("application", _)` stat-block
catch-all. Replace that with the tiered preview from the design doc: a
**native text tier** (lopdf: page count, /Info metadata, page-1 text) that is
always available and ships alone, plus an **optional external image tier**
(pdftoppm/mutool render of page 1 into the raster cache, kind `pdf-p1-960`)
behind a new `pdf_render` config switch, default **off**. Composition:
image tier (enabled + renderer present) → text tier → stat block. A PDF never
shows a bare error panel.

**Design doc:** `docs/plans/2026-07-30-preview-pdf-design.md`
(+ `…-preview-raster-cache-extension-design.md` for cache keying,
`…-preview-new-types.md` as the pattern exemplar).

**Architecture:** follows the two established conventions exactly
(CLAUDE.md "Architecture: native preview backends" / "preview raster cache"):

- Native-first + fallback: one new arm in `FilePreview::new`
  (`src/panel/preview.rs`, the `match` at ~line 225) → `pdf_preview` wrapper;
  every downgrade logs a `debug!("… failed, falling back …")` line (visible in
  the socket `log` history).
- Line previews: 128-line cap, `scrub_line` on every attacker-controlled
  string (/Info strings, extracted page text).
- Bounded work on attacker-controlled input is a **hard requirement** (this
  repo treats unbounded allocations reachable from cursor navigation as MAJOR
  bugs). lopdf's stream decompression is unbounded (`read_to_end` in
  `decompress_zlib`, verified in the 0.36.0 source) — the plan's central
  safety piece is a **guard filter** on `Document::load_filtered` that
  size-verifies every compressed stream against a decompression budget
  *before* lopdf ever inflates anything. Details in Task 2.
- Raster tier: dir-parameterized producer (`pdf_render_first_page_in`), the
  `.part` + `part_token()` + rename discipline, corrupt-entry rule via
  `lookup_in`, `store_in` for the atomic final write. Cache-off
  (`preview_cache = false`) is a privacy promise: the image tier is skipped
  entirely (an external render IS a write), mirroring the video path — the
  write location comes from the existing `video_thumbnail_dir()` policy.

## Tech stack — verified empirically on rustc/cargo 1.83.0 (2026-07-31)

**Crate choice: `lopdf`, pinned `>=0.36, <0.37`, `default-features = false,
features = ["time"]`.** Everything below was proven in a scratch crate AND by
adding the pin to *this* tree (build + all 231 tests green, then reverted):

- lopdf 0.36.0 declares `rust-version = "1.74"` and **compiles and runs on
  1.83.0**. Newer lopdf (0.37+…0.44) declare rust-version ≥ 1.85/1.88 —
  unusable at MSRV. `pdf-extract`/`pdf` were not needed (lopdf passed); do
  not substitute them without re-verifying.
- Default features would pull `jiff` (edition2024 — cargo 1.83 cannot parse
  it), `chrono`, and `rayon`; `default-features = false, features = ["time"]`
  avoids all three and reuses rfm's existing `time 0.3.37`. **rayon off is
  load-bearing twice**: fewer deps, and it keeps `load_filtered` single-
  threaded so the guard filter may use a `thread_local!` budget.
- In a *fresh* lock lopdf resolves `indexmap 2.14` (edition2024 → cargo 1.83
  refuses to parse). In **this tree's lock** it reuses the existing pins:
  verified additive-only — ~30 new packages (aes/md-5/stringprep/nom 8/
  rand 0.9/getrandom 0.3 as duplicates beside rfm's nom 7/rand 0.8 —
  acceptable), only `bitflags` moves 2.7.0 → 2.13.1; `uuid 1.12.1`,
  `ogg_pager 0.7.0`, `indexmap 2.7.0`, `time 0.3.37` untouched. `wasip2`/
  `wit-bindgen` land in the lock with rust-version > 1.83 but are wasm-only
  targets that are never downloaded/compiled on linux — harmless (and NOT
  edition2024).
- API round-trip proven: build-save-load, `get_pages()` count, /Info
  Title/Author/Producer access via `trailer.get(b"Info")` + `dereference`,
  `extract_text(&[1])`, `is_encrypted()`.
- Hostile input proven on 0.36.0:
  - object-reference loops → `Err`/empty in < 1 ms (`DEREF_LIMIT = 128` in
    `dereference`; `read_object` and the xref `/Prev` chain carry
    `already_seen` sets);
  - page-tree Kids cycles → terminate in < 1 ms (`PAGE_TREE_DEPTH_LIMIT =
    256` + `iter_limit = objects.len()`);
  - truncated and garbage files → `Err("failed parsing cross reference
    table…")`;
  - **flate bombs are NOT guarded by lopdf** (a 509 KiB stream inflating to
    512 MiB *would* be `read_to_end`'d) — the guard filter below drops it
    before lopdf inflates (proven: bomb object absent after `load_filtered`,
    `extract_text` returns cleanly, ~850 ms total);
  - /Info strings keep raw `\r`/`\n` (proven) → `scrub_line` is mandatory.
- `Document::load_filtered(path, fn)` exists only path-based (no `_mem`
  variant); its `FilterFunc` is a plain `fn` pointer
  (`fn((u32,u16), &mut Object) -> Option<((u32,u16), Object)>`), called for
  every parsed object **before** ObjStm expansion — returning `None` drops
  the object. This is the hook the guards ride on; unit tests therefore
  write fixture PDFs to tempdir paths (they already do for every other arm).

**Renderers on this machine:** `pdftoppm` and `mutool` are both **absent**
(checked 2026-07-31). All image-tier unit tests must skip gracefully when no
renderer is present, and the e2e "enabled" path here exercises the
silent-degrade branch; the renderer-present e2e block is included but marked
machine-dependent.

**Resolved design ambiguities** (deliberate; flag pushback before deviating):

- **"encrypted PDF (N pages) + metadata":** metadata = file Size/Modified
  lines, NOT /Info strings — in a real encrypted PDF strings are encrypted
  garbage (dictionaries/structure are not, which is why the page count still
  reads: proven). No decryption attempt in v1.
- **Bounded source read:** `load_filtered` takes a path and reads to end, so
  the bound is a `metadata().len() > PDF_SOURCE_MAX → Err` pre-check instead
  of a literal `read_bounded` — same intent, benign TOCTOU (a file that
  grows between check and read only wastes one preview's work). 32 MiB cap:
  larger PDFs are image-heavy scans whose text tier shows little anyway;
  they take the stat block (or the image tier when enabled — the renderers
  handle size themselves).
- **Where the image tier may write:** exactly the video-thumbnail policy —
  reuse `video_thumbnail_dir()` (cache dir / temp-fallback /
  `None` = preview_cache off → skip the tier). No new plumbing, and the
  temp-fallback's 7-day prune covers `pdf-p1-960` orphans automatically.
- **mutool output format:** `mutool draw` emits PNG (no JPEG), so the
  producer decodes the tool's `.part` output with
  `with_guessed_format()` and lets `store_in` re-encode the final JPEG —
  the `.part`+token discipline is kept, the *atomic final write* is
  `store_in`'s own part+rename. (pdftoppm could rename directly like ffmpeg,
  but one uniform decode→`store_in` path for both tools is simpler and
  normalizes quality/format; deviation from `ffmpeg_thumbnail` noted in the
  producer's doc comment.)
- **Auto-enable when a renderer is detected** (design flagged it as a
  possible one-liner): NOT done — `pdf_render = false` stays a hard off, as
  designed.
- **Probe robustness:** poppler/mupdf `-v` exit codes are not verified here
  (binaries absent). Probe accepts a tool when the process spawned AND
  (exit success OR its output contains "version") — belt-and-braces against
  a `-v`-exits-nonzero tool; verify against the real binaries during
  implementation if available.

**Per-task rule:** every task ends with `cargo build`, `cargo test`,
`cargo clippy --all-targets` all green, then a commit (trailer:
`Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`). Do not proceed on
a red build. Unit tests are terminal-free, in preview.rs's existing style
(new `mod pdf_tests` — sentence-style names, tempfile fixtures, no
`raster_cache::init`).

---

### Task 1: Dependency (MSRV-1.83-verified pin)

**Files:** `Cargo.toml` (keep alphabetical — between `lofty` and
`lzma-rust2`), `Cargo.lock` (committed).

**Step 1:** Add:

```toml
# pdf text tier; 0.36 declares rust-version 1.74, newer need >=1.88.
# default features would pull jiff (edition2024) + chrono + rayon —
# rayon off also keeps load_filtered single-threaded (the guard
# filter's thread_local budget relies on it).
lopdf = { version = ">=0.36, <0.37", default-features = false, features = ["time"] }
```

**Step 2:** `cargo build && cargo test` — green (verified: resolves to
lopdf 0.36.0, lock additive except `bitflags` 2.7.0 → 2.13.1, existing pins
`uuid 1.12.1` / `ogg_pager 0.7.0` / `indexmap 2.7.0` / `time 0.3.37` intact,
231/231 tests pass). If cargo reports any `edition2024` / "requires rustc"
package, `cargo update -p <pkg> --precise <old>` it back and note it in the
commit message.

**Step 3:** Commit: `chore(deps): lopdf for the pdf text tier (MSRV 1.83 pin)`

---

### Task 2: Text tier — guarded lopdf load + metadata + page-1 text

**Files:** `src/panel/preview.rs` (constants + functions near the other
native backends; tests in a new `mod pdf_tests`).

**Step 1: Test fixtures** (hermetic, lopdf-built — the builder API is
proven; ~40 lines, adapted from the probe):

- `make_pdf(dir, name, title, n_pages) -> PathBuf` — Catalog/Pages/Page
  objects, per-page content stream `BT /F1 24 Tf … (Hello PDF page N) Tj ET`,
  /Info with Title = `title`, Author = `"Prob\ne Author"` (embedded newline —
  deliberate), Producer. `doc.save_to(&mut Vec)` then `fs::write`.
- `make_encrypted_pdf(dir)` — same, plus a Standard-handler /Encrypt
  dictionary referenced from the trailer (detection-only fixture; lopdf's
  `is_encrypted()` checks the trailer — proven).
- `make_bomb_pdf(dir, inflated_len)` — page-1 Contents is a
  `FlateDecode` stream of zlib-compressed zeros,
  `Stream::new(dict!{"Filter"=>"FlateDecode"}, compressed).with_compression(false)`
  (proven pattern; `flate2` is already a dep).
- `make_loop_pdf(dir)` — two objects referencing each other, page Contents
  pointed at the loop.

**Step 2: Failing tests** (`mod pdf_tests`):

- `pdf_text_tier_shows_page_count_title_and_first_page_text` — 3-page
  fixture: lines contain `PDF · 3 pages`, the title, `Hello PDF page 1`;
  and NOT `Hello PDF page 2` (page 1 only).
- `pdf_info_strings_are_scrubbed` — the Author line with the embedded `\n`
  renders as ONE line (proven that lopdf hands it through raw).
- `pdf_text_is_capped_at_128_lines` — one page with 200 `Tj` lines (`Td`
  advances between them produce separate extract lines — if extract yields
  them joined, build 200 pages? no: assert on the *output line count* ≤ 128
  however extraction folds; the cap is on our lines).
- `pdf_lines_are_length_capped` — a single 100 KiB `Tj` string → the output
  line is truncated to `PDF_LINE_MAX` (char-boundary safe).
- `encrypted_pdf_shows_encrypted_line_and_page_count` — encrypted fixture →
  `Ok`, first line `encrypted PDF (2 pages)`, Size/Modified lines present,
  and the /Info title is NOT shown.
- `truncated_pdf_is_an_error` + `garbage_pdf_is_an_error` — half-truncated
  fixture bytes / `b"%PDF-1.4\nnot a pdf"` → `Err` (proven lopdf behavior).
- `oversized_pdf_is_an_error` — call the bounded core with a tiny
  `source_max` (e.g. 1 KiB) against a normal fixture → `Err` without parsing.
- `flate_bomb_content_stream_is_dropped_not_inflated` — bomb fixture
  (e.g. 16 MiB inflated) through the bounded core with a 1 MiB budget →
  returns quickly; body text absent; no panic/OOM. (The default-budget path
  is covered implicitly by every other test.)
- `reference_loop_pdf_terminates` — loop fixture → returns (Ok metadata or
  Err) — assert it completes; wrap in a generous time assertion only if
  cheap (< the test-suite default is fine).

**Step 3:** `cargo test pdf_tests` — FAIL. **Step 4: Implement.**

Constants (next to `SVG_SOURCE_MAX`):

```rust
/// PDFs above this take the stat block (or the image tier): the text
/// tier's value is capped anyway and load_filtered slurps the file.
const PDF_SOURCE_MAX: u64 = 32 * 1024 * 1024;
/// Total decompressed bytes lopdf may materialize per document.
const PDF_DECOMP_BUDGET: u64 = 64 * 1024 * 1024;
/// Longest retained preview line (chars) — a PDF can emit one
/// multi-megabyte text run with no newlines.
const PDF_LINE_MAX: usize = 1024;
```

The guard (proven design — see header):

```rust
thread_local! {
    /// Remaining decompression budget for the load_filtered call on
    /// this thread (the FilterFunc is a plain fn pointer, so the
    /// budget travels beside it; rayon is disabled, the filter runs on
    /// the loading thread).
    static PDF_BUDGET: Cell<u64> = const { Cell::new(0) };
}

/// lopdf's decompress_zlib is an unbounded read_to_end — verify every
/// stream BEFORE lopdf inflates it: counting-decompress FlateDecode
/// through a Take'd sink; charge LZWDecode pessimistically at the max
/// ratio (1032:1, no counting decoder at hand); DCT/ASCII85/plain pass
/// (lopdf never inflates DCT; ASCII85 is fixed-ratio; plain content is
/// bounded by PDF_SOURCE_MAX). Over budget => drop the object (None)
/// AND zero the budget: one bomb costs at most budget+1 counting
/// bytes, every later stream then drops after <=1 byte — total work
/// across a hostile file stays O(2 * budget). A dropped stream leaves
/// a hole; lopdf then errs or extracts nothing and the caller
/// degrades — exactly right for a hostile file.
fn pdf_guard_filter(id: (u32, u16), object: &mut Object) -> Option<((u32, u16), Object)>
```

(implementation as probed: `flate2::read::ZlibDecoder::new(content).take(budget + 1)`
→ `io::copy` to `io::sink()`; `n > budget` → zero budget, `None`; else
decrement and `Some((id, object.clone()))` — the clone is the API's shape and
is bounded by `PDF_SOURCE_MAX`.)

```rust
/// Bounded lopdf load: size pre-check, budget arm, guarded parse.
fn load_pdf_guarded(path: &Path) -> anyhow::Result<lopdf::Document> {
    load_pdf_guarded_bounded(path, PDF_SOURCE_MAX, PDF_DECOMP_BUDGET)
}
fn load_pdf_guarded_bounded(path, source_max, budget) -> anyhow::Result<Document>
// metadata().len() > source_max => bail; PDF_BUDGET.set(budget);
// Document::load_filtered(path, pdf_guard_filter)

/// Header + body lines: "PDF · N pages" (or "1 page"), Title/Author/
/// Producer from /Info when present (each dereference'd, lossy-UTF-8,
/// scrub_line'd, PDF_LINE_MAX-truncated), blank, then page-1 text via
/// extract_text(&[1]) split on '\n' — every line scrubbed + truncated,
/// 128 lines total. Encrypted docs (is_encrypted): "encrypted PDF
/// (N pages)" + Size/Modified lines, NO /Info strings (they are
/// encrypted garbage in a real encrypted file) and no extract_text.
fn native_pdf_lines(doc: &lopdf::Document, path: &Path) -> Vec<String>

fn pdf_text_tier(path: &Path) -> anyhow::Result<Preview>  // load + lines -> Preview::Text
```

Note `extract_text` on a page with dropped/looping content returns
`Ok("")`/`Err` fast (proven) — treat body-extraction failure as "no body",
keep the metadata header (the preview still shows page count), do NOT
propagate it as tier failure.

**Step 5:** `cargo test pdf_tests` — PASS. **Step 6:** clippy, commit:
`feat(preview): native pdf text tier via lopdf (bounded, guarded)`

---

### Task 3: Config switch `pdf_render` + renderer probe

**Files:** `src/config.rs`, `src/main.rs`, `examples/config.toml`,
`src/panel/preview.rs`.

**Step 1: Failing tests:**

- config: `pdf_render_defaults_false_and_parses_true` (mirror the existing
  `preview_cache` test — note the different default!).
- preview: `pdf_render_defaults_off_when_uninitialized` —
  `pdf_render_enabled()` is `false` without `set_pdf_render` (the
  raster-cache "uninitialized == opted out" convention; keeps every other
  test hermetic).

**Step 2: Implement.**

- `GeneralConfig`: `#[serde(default)] pub pdf_render: bool` (doc comment:
  render PDF page 1 via pdftoppm/mutool when installed; default off keeps
  the base install pure-Rust). Sits next to `preview_cache`.
- `main.rs`: read it beside `persist_previews` (local `pdf_render`,
  default `false`), then right after `raster_cache::init`:
  `panel::preview::set_pdf_render(pdf_render);`.
- `preview.rs`:

```rust
static PDF_RENDER: OnceCell<bool> = OnceCell::new();
pub fn set_pdf_render(enabled: bool) { let _ = PDF_RENDER.set(enabled); }
fn pdf_render_enabled() -> bool { PDF_RENDER.get().copied().unwrap_or(false) }

#[derive(Clone, Copy, Debug)]
enum PdfRenderer { Pdftoppm, Mutool }

/// OnceCell probe, ffmpeg-style but two candidates: `pdftoppm -v`,
/// else `mutool -v`. Present = spawned AND (success OR output contains
/// "version") — the -v exit codes are not uniform across packagings.
/// Only consulted when pdf_render_enabled(), so the base install
/// never spawns probes.
fn pdf_renderer() -> Option<PdfRenderer>
```

- `examples/config.toml`: comment block + `pdf_render = false` next to
  `preview_cache` (external dependency named, default off, silently stays
  on the text tier when no renderer is installed).

**Step 3:** tests PASS, clippy, commit:
`feat(config): pdf_render switch for the external pdf image tier`

---

### Task 4: Image tier — render page 1 into the raster cache (`pdf-p1-960`)

**Files:** `src/panel/raster_cache.rs` (one constant),
`src/panel/preview.rs` (producer + tests).

**Step 1: Failing tests** (`mod pdf_tests`; **every renderer-spawning test
must skip when no renderer is installed** — this machine has none:
`let Some(r) = pdf_renderer() else { eprintln!("skipping: no pdf renderer"); return; }`
— unlike the ffmpeg tests, which may assume their binary):

- `pdf_render_hits_the_cache_without_spawning` — `store_in` a pre-seeded
  image under `entry_name(path, mtime, KIND_PDF)`, then call
  `pdf_render_first_page_in` with `PdfRenderer::Pdftoppm` on a machine
  *without* pdftoppm (no skip guard here — the point is the hit path returns
  `Ok` before any spawn; on machines with pdftoppm it simply also passes).
- `pdftoppm_renders_page_one_into_the_cache` (skip-guarded) — real fixture
  PDF → `Ok(Preview::Image)`, the `*-pdf-p1-960.jpg` entry exists and
  decodes, dimensions bounded by 960×540.
- `pdf_render_failure_leaves_no_part_files` (skip-guarded) — garbage `.pdf`
  → `Err`, and the cache dir holds no `<hash>-`-prefixed leftovers (reuse
  the `thumbnail_dir_entries_of` helper style with `KIND_PDF`).
- `kind_pdf_coexists_with_other_kinds` — cheap `entry_name` assertion in
  raster_cache tests style (or extend `entry_names_differ_by_kind_only`).

**Step 2: Implement.**

- `raster_cache.rs`:

```rust
/// Kind tag for external PDF page renders: page 1, scale-to 960.
/// Changing the page or scale MUST bump this ("pdf-p2-…", "-1280") —
/// old entries then age out via the stale-sibling sweep.
pub(crate) const KIND_PDF: &str = "pdf-p1-960";
```

- `preview.rs` producer, mirroring `ffmpeg_thumbnail`'s shape (lookup →
  recreate dir → part-name render → validate → atomic finalize → sweep):

```rust
/// Render page 1 of `path` into `dir` under KIND_PDF. Discipline as
/// ffmpeg_thumbnail: lookup_in first (corrupt-entry rule), re-create
/// the dir (mid-session `rm -rf` safety), render to a same-dir
/// `<entry>.<part_token()>.part.<ext>` temp name (extension LAST so
/// the tools' format inference works: pdftoppm gets the prefix and
/// appends ".jpg" itself, mutool writes ".png"), then decode the part
/// (with_guessed_format — tool output format differs), delete it, and
/// finalize via store_in (its own part+rename gives the atomic final
/// write and the stale-sibling sweep). Non-zero exit, missing or
/// undecodable output => remove the part, Err (caller drops to the
/// text tier). Info lines: pdf metadata via load_pdf_guarded
/// (best-effort — unreadable source keeps just Size/Modified lines).
fn pdf_render_first_page_in(
    dir: &Path, renderer: PdfRenderer, path: &Path, mtime: u64,
) -> anyhow::Result<Preview>
```

Commands (`Stdio::null()` stdin, `output()` captured; stderr tail in the
error like ffmpeg):
  - pdftoppm: `pdftoppm -jpeg -f 1 -l 1 -scale-to 960 -singlefile <pdf> <prefix>`
    where `<prefix>` = `dir/<entry>.<token>.part` (tool writes
    `<prefix>.jpg`).
  - mutool: `mutool draw -o <prefix>.png -w 960 -h 540 <pdf> 1` (verify the
    exact fit flags against a real mutool at implementation time; only the
    output path + page-1 selection are load-bearing).
  - Bound the decoded render defensively (`thumbnail(960, 540)` if larger)
    before `store_in` — the cache must never hold an unbounded raster.

**Step 3:** tests PASS (skipping ones report so), clippy, commit:
`feat(preview): external pdf page-1 render tier, cached as pdf-p1-960`

---

### Task 5: Dispatch + tier composition

**Files:** `src/panel/preview.rs` (arm + `pdf_preview` + tests).

**Step 1: Failing tests:**

- `pdf_preview_uses_text_tier_when_render_disabled` — via the
  parameterized core with `renderer: None`: a fixture PDF yields
  `Preview::Text` containing `PDF · 3 pages`.
- `pdf_preview_of_garbage_falls_back_to_stat` — garbage `.pdf` →
  `Preview::Text` with the stat-block markers (`Size:` / `MIME type:`).
- `file_preview_dispatches_pdf_to_the_text_tier` — `FilePreview::new` on a
  fixture `whatever.pdf` → text-tier lines, NOT the plain stat block
  (proves the arm replaced the catch-all landing). Extensionless routing is
  already covered by opener.rs's `extensionless_pdf_bytes_get_application_pdf`.

**Step 2: Implement** (the design's `pdf_preview` pseudocode, testably
parameterized like the other `*_in` producers):

```rust
fn pdf_preview(path: &Path, modified: SystemTime, mime: &mime::Mime) -> Preview {
    let renderer = if pdf_render_enabled() { pdf_renderer() } else { None };
    // video_thumbnail_dir(): cache dir / temp fallback / None when
    // preview_cache = false — the image tier writes nothing then.
    pdf_preview_in(renderer, video_thumbnail_dir(), path, modified, mime)
}

fn pdf_preview_in(
    renderer: Option<PdfRenderer>, thumb_dir: Option<&Path>,
    path: &Path, modified: SystemTime, mime: &mime::Mime,
) -> Preview {
    if let (Some(r), Some(dir)) = (renderer, thumb_dir) {
        match pdf_render_first_page_in(dir, r, path, mtime_secs(modified)) {
            Ok(p) => return p,
            Err(e) => log::debug!("pdf render failed, falling back to text tier: {e}"),
        }
    }
    match pdf_text_tier(path) {
        Ok(p) => p,
        Err(e) => {
            log::debug!("pdf text tier failed, falling back to stat: {}: {e}", path.display());
            stat_preview(path, mime)
        }
    }
}
```

Dispatch arm — between the sqlite arm and the text-based `application/*`
block (comment: `%PDF` magic also routes extensionless files here via the
sniff; mime 0.3 keeps subtype "pdf" whole — no suffix trap):

```rust
("application", "pdf") => pdf_preview(&path, modified, &mime),
```

**Step 3:** full `cargo test` PASS, clippy, commit:
`feat(preview): tiered pdf dispatch (image -> text -> stat)`

---

### Task 6: e2e (tmux + socket) + docs

**Files:** `CLAUDE.md` (one paragraph), no code.

**Step 1: e2e — disabled path (default config).** Per the CLAUDE.md loop;
fixture names deliberately contain spaces and `&`. The fixture PDF is
handwritten-with-computed-xref (verified: lopdf parses it, 1 page, title
readable, text extracts):

```bash
FIXTURE=$(mktemp -d); CFG=$(mktemp -d); CACHE=$(mktemp -d)
# handwritten minimal pdf: body objects, then xref offsets computed
# with grep -b, trailer with /Root + /Info, startxref = wc -c.
# (script proven against lopdf 0.36.0; ~25 lines)
sh make_min_pdf.sh "$FIXTURE/quarterly report & notes.pdf"
printf '%%PDF-1.4\nnot a pdf' > "$FIXTURE/broken & torn.pdf"
tmux new-session -d -s rfm-pdf -x 120 -y 30
tmux send-keys -t rfm-pdf "XDG_CACHE_HOME=$CACHE ./target/debug/rfm \
  --debug-socket /tmp/rfm.sock --config $CFG $FIXTURE" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done
```

- cursor onto the good PDF → `await-idle` → poll `state` until `seq`
  stabilizes (preview loads are async) → `capture-pane`: expect
  `PDF · 1 page`, the title line, `Hello rfm PDF preview`. `log 40`: no
  "falling back" for this file.
- cursor onto `broken & torn.pdf` → `capture-pane`: stat block; `log 40`
  contains `pdf text tier failed, falling back to stat`.
- assert `$CACHE/rfm/thumbnails` contains NO `*-pdf-p1-960.jpg` (tier off).

**Step 2: e2e — enabled path.** Write `pdf_render = true` (plus the
required color/general keys — copy `examples/config.toml` into `$CFG` and
flip the flag), relaunch.

- **This machine (no pdftoppm/mutool — checked):** the run must be
  indistinguishable from Step 1 on screen (silent stay on the text tier),
  and `log` must show no error-level lines — this validates the
  degrade-without-noise promise.
- **Machine-dependent block (renderer present; run when available):**
  same loop, then assert exactly one `*-pdf-p1-960.jpg` in
  `$CACHE/rfm/thumbnails`, decodable; `capture-pane` shows an image
  preview (no `PDF ·` text lines in the preview column); revisiting the
  file after `await-idle` logs `raster cache hit`.
- Teardown: `tmux kill-session -t rfm-pdf; rm -rf "$FIXTURE" "$CFG" "$CACHE" /tmp/rfm.sock`.

**Step 3: CLAUDE.md** — extend "Architecture: native preview backends" with
one tight paragraph: the pdf arm's tiering, lopdf 0.36 pin rationale
(default-features off; rayon-off ↔ thread_local budget), the guard filter +
budgets (`PDF_SOURCE_MAX`/`PDF_DECOMP_BUDGET`), `pdf_render` default-off +
renderer probe, kind `pdf-p1-960` (bump rule), and that the image tier
rides the `video_thumbnail_dir()` write policy.

**Step 4:** Commit: `docs: pdf preview tiers in the architecture notes`

---

## Verification summary (what was actually proven, rustc/cargo 1.83.0, 2026-07-31)

- lopdf 0.36.0 (`rust-version 1.74`, `default-features = false,
  features = ["time"]`): compiles + runs in a scratch crate AND in this
  tree (231/231 tests green, lock additive, pins intact; reverted after).
  0.37+ need rustc ≥ 1.85; fresh-lock resolution pulls edition2024
  `indexmap 2.14`/`jiff` — this tree's lock avoids both.
- Probe program (6 checks, all passing): build/save/load round trip with
  page count + /Info + page-1 `extract_text`; `is_encrypted` + readable
  page count on an /Encrypt trailer; truncated + garbage → `Err`;
  contents ref-loop and Kids cycle terminate < 1 ms; a 512 MiB flate bomb
  (509 KiB compressed) is dropped by the `load_filtered` guard filter
  before inflation (`extract_text` then returns cleanly).
- lopdf source audit (0.36.0): `DEREF_LIMIT = 128`,
  `PAGE_TREE_DEPTH_LIMIT = 256` + `iter_limit`, xref `/Prev` and
  `read_object` cycle sets — loops guarded; `decompress_zlib`/`_lzw` are
  unbounded — the ONLY unguarded hazard, closed by our filter.
- /Info strings keep raw newlines → scrub_line mandatory (probe-proven).
- `pdftoppm`/`mutool`: absent on this machine; probe/e2e planned
  accordingly (skip-guards + silent-degrade assertion; renderer-present
  block marked machine-dependent).
- Handwritten e2e fixture PDF (computed-xref shell script, name with
  spaces + `&`): parsed by lopdf, 1 page, title + text extracted.
