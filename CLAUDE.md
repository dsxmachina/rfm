# rfm — development notes for Claude

rfm is a terminal file manager (crossterm + tokio, Miller columns).
Build: `cargo build` — Tests: `cargo test` — Lint: `cargo clippy`

## Testing the TUI interactively

Drive the real binary in tmux; use the debug socket for synchronization
and state instead of sleeping and guessing.

```bash
# Setup (isolated fixture)
FIXTURE=$(mktemp -d); touch "$FIXTURE"/{a,b,c}.txt
tmux new-session -d -s rfm-test -x 120 -y 30
tmux send-keys -t rfm-test \
  "./target/debug/rfm --debug-socket /tmp/rfm.sock $FIXTURE" Enter
until [ -S /tmp/rfm.sock ]; do sleep 0.1; done   # socket appears ~instantly, but not instantly

# Drive + observe (the loop for every interaction)
tmux send-keys -t rfm-test j                           # 1. input
echo await-idle | socat - UNIX-CONNECT:/tmp/rfm.sock   # 2. wait, don't sleep
echo state | socat - UNIX-CONNECT:/tmp/rfm.sock        # 3. internal state
tmux capture-pane -t rfm-test -p                       # 4. actual screen

# Teardown (the socket file survives the kill — remove it too)
tmux kill-session -t rfm-test; rm -rf "$FIXTURE" /tmp/rfm.sock
```

Socket commands (one per connection, JSON reply):
- `state` — mode, cwd, selection, marked, clipboard, queue, seq counter,
  undo_depth/redo_depth
- `await-idle` — blocks until the event loop has drained (max 30s)
- `entries left|center` — the entries rfm *believes* the pane shows
- `log [n]` — last n retained log lines (level, age_secs, message).
  The retention history (200 lines, capacity-evicted only) outlives the
  log widget's per-line 10s display TTL, so this is where errors live
  after they vanish from screen.
  Background command failures (exit codes, stderr) land here — check
  `log` first when something "silently" fails.

With `--debug-socket` active, verbosity is raised to TRACE (rfm targets
only; dependency noise is filtered, and the on-screen widget still shows
only info+). The history then contains a full causal trail per
interaction: `key-event:` (with mode), `mode: x -> y`, `draw:` (which
panes were considered dirty — the stale-pane diagnostic), `panel-update:`
(async panel arrivals), `jump-to`, `zoxide query`, watcher
`watching`/`unwatching`. Correlate via age_secs.

Reading the replies correctly:
- Directories sort before files: with a.txt/b.txt/c.txt + subdir, the
  initial selection is `subdir`, not `a.txt` — don't mis-assert.
- `state.selected_idx` is 0-based among VISIBLE entries; the `entries`
  reply lists ALL entries (incl. hidden) with a per-entry `selected`
  flag. Never index one with the other.
- `state.marked` holds absolute paths; `entries` has per-entry `marked`
  booleans. Mark key is Space (default binding), and marking
  auto-advances the cursor to the next entry.
- `seq` advances by multiple ticks per keypress (~5 for one `j`).
  Never assume +1 deltas. Reliable invariants: seq is stable while
  nothing happens; seq increased after real input. Debug queries
  themselves never advance seq.

## Debugging visual bugs: diff belief against reality

`capture-pane` is ground truth (what the user sees); the socket is rfm's
belief. For "pane not updating"-style bugs, compare the two:
- `entries` correct, screen stale → bug in the render/flush path
  (dirty flags, draw()).
- `entries` also stale → bug in the state/update path (watchers,
  cache, DirManager).

If `state` times out (10s), the event loop itself is wedged — that is a
finding, not a tooling failure.

Caveats:
- `await-idle` does not cover async panel loads still in flight
  (DirManager/PreviewManager); if content looks like a loading
  placeholder, poll `state` until `seq` stabilizes.
- `--config` can point at a scratch dir to isolate config.
- Use `-x`/`-y` on tmux new-session for a deterministic pane size.
- On exit with errors, rfm writes `./error.log` from the full retained
  history (with line ages) — useful post-mortem when the session is gone.
- Background commands run via `sh -c`: any path interpolated into a
  queued command string MUST go through `shell_escape::escape`
  (see `zoxide_add_dir`, `expand_command`). Test fixture names with
  spaces and `&` (e.g. "a directory with spaces", "Bilder & Videos").
- Isolate zoxide in tests with `_ZO_DATA_DIR=$(mktemp -d)` in the tmux
  pane before launching rfm; seed with `zoxide add <path>`.
- Integration tests around the upgrade notice must export
  `XDG_STATE_HOME=$(mktemp -d)` in the tmux pane before launching rfm —
  the real state file records the notice as seen and suppresses it.

## Architecture: modal modes

Mode logic lives in `src/panel/mode/` — one adapter per file (the two
consoles currently share `console.rs`; split pending). Adapters are
pure state machines: `handle_key → ModeOp`, testable without a
terminal. All effects and the derived redraws are applied centrally in
`PanelManager::apply_mode_op` (manager.rs). The mode strings the debug
socket reports come from `ModalInput::name()`. `decision-flow`
(`decision_flow.rs`) is the generic multi-item choice overlay (y/n/per-item
keys, `A` = apply answer to all same-choice items, Esc accepts defaults);
today it backs the one-time upgrade notice, and is the foundation for
future guided flows.

## Architecture: configuration

One sparse-override `~/.config/rfm/config.toml`; the complete annotated
defaults live in `examples/default-config.toml` (rust-embed) — simultaneously
the behavior, the `--dump-config` output and the docs, guarded by
`config::defaults_tests`. Merge pipeline in `src/config/`: `load.rs::load()`
parses both as `toml::Value`, folds legacy `keys.toml`/`open.toml` in-memory
under `[keys]`/`[open]` (never rewrites disk; `--migrate-config` →
`load.rs::migrate()` does, with `*.bak` + refuse-to-clobber), deep-merges
user over defaults (`merge.rs`: tables merge, scalars/arrays replace),
deserializes typed with per-section error dropping + unknown-key typo
warnings. Keybinding conflicts: `CommandParser::build(defaults, user)` is
two-pass, user wins, dropped defaults are logged and returned. Startup shows
a one-time upgrade notice (decision-flow overlay) when defaults were dropped
against user bindings or legacy files were folded; the seen-version lives in
`$XDG_STATE_HOME/rfm/state.toml`.

## Architecture: tabs & split view

A `Tab` is one Miller-columns stack (`left`/`center`/`right` `ManagedPanel`s
with its own cwd, selection and fwd/rev history). `PanelManager` holds
`tabs: Vec<Tab>` + `focused: usize` + `view: ViewMode { Single, Split }`;
`MAX_TABS = 4`. All tabs live and are watched at once (each carries its own
file-watcher and content senders), so every tab's listing stays fresh even
off-screen.

Single view renders the focused tab's full Miller stack (left|center|right).
Split view (`!`) renders only the `center` column of two adjacent tabs side
by side — no preview column — the focused one bright, the other dimmed, with a
divider. `Tab` cycles focus; `!` toggles back to single (auto-creating a 2nd
tab if there was only one; refuses if the terminal is too narrow).

Operations route through `active()`/`active_mut()` (the focused tab).
Clipboard, undo/redo and `show_hidden` are **global** (manager-level); `marked`
is **per-tab** (lives in the panels). Async panel updates route to the owning
tab by `panel_id` (via `check_update`) across **all** tabs, not just the
focused one — required so a background tab's `center` stays fresh in split and
a cross-tab cut/copy-paste updates the (non-focused) source tab.

Preview efficiency: the `right`/preview panel is driven only when it is
actually on screen — i.e. the focused tab in single view. Navigation in split
skips the preview decode entirely, and `reload_all` reloads `right` only for
the focused single-view tab (but `left`/`center` for every tab). Whenever a
tab's preview *becomes* visible again — split→single, or a focus change
(`focus_next`/`focus_tab`) — `refresh_focused_preview` re-drives it from the
current center selection, so it is never stale (`new_panel_delayed`
short-circuits when the path is unchanged, keeping the refresh cheap).

Debug socket: `state` exposes `view` (`"single"`/`"split"`), `focused`, and a
`tabs[]` array (per-tab cwd/selection/marked); the scalar top-level fields
mirror the focused tab for single-tab scripts. `entries [<tab>] left|center`
takes an optional 0-based tab index (defaults to focused).

Keys: `!`=toggle_split, `Tab`=focus_next, `gn`=new_tab, `q`/`ctrl-w`=close_tab
(closing the *last* tab quits rfm, returning `CloseCmd::QuitWithPath`),
`1`-`4`=focus_tab_N. Defaults in `[keys.tabs]` — active for everyone (incl.
old configs) unless a user binding collides (user wins, default dropped with
a logged notice). Note `q` closes the focused tab (and quits on the last
one); `Q` / `exit` always quit outright.

## Architecture: rendering

Event-driven, not a render loop: the select loop draws only when an event
sets the single `dirty` bit (PanelManager). `draw()` repaints everything
in call order (footer → header → panels → log → console, overlay last —
that ordering *is* the z-order) and clears the bit. The console overlay
draws last on purpose: it parks the visible text cursor at its input line
(`cursor::Show`), and `draw_log` ends with its own `MoveTo` in the bottom
log region — drawing it after the console would strand the cursor there. No per-element dirty
flags: any state change calls `mark_dirty()`, so panels can't go stale.
Full repaint is cheap because every draw is a blit except the image
preview, whose resize is cached (FilePreview, keyed on cell dimensions).
Adding a window ≈ carve its layout region + a draw call at the right spot
in the sequence; no flag plumbing.

Log lines shown in the widget expire after DISPLAY_TTL (logger.rs, 10s);
the 1 s task wakes the UI only when a line actually expires.

Graphics-protocol image previews (`src/panel/graphics.rs` + the sixel
encoder in `graphics/sixel.rs`): the protocol (kitty | sixel |
half-block) is resolved once at startup — `graphics::init` in main.rs,
right after `enable_raw_mode` and before the EventStream exists — in
this order: explicit `image_protocol` config pins it (no probe); else
env heuristics ($TMUX / TERM=tmux*/screen* → half-block, kitty/WezTerm/
Ghostty vars → kitty); else a 250 ms poll-bounded probe (kitty APC
query + DA1; DA1 attribute `4` = sixel; kitty beats sixel; timeout →
half-block). Cell→pixel geometry comes from TIOCGWINSZ (CSI 14 t at
startup as fallback) and is refreshed on `Event::Resize`; sixel
*requires* it and degrades to half-block without it, kitty assumes an
8×16 cell. The resolved protocol is on the debug socket `state` as
`image_protocol`; decisions/emits are `graphics:` trace/debug log lines.

The emitters are the second exception to blit-cheapness, so re-emission
is gated: a module-global `EmitKey` (path, mtime, pixel box, origin) —
unchanged key = zero bytes written on repaint. Stale placements are
handled by frame reconcile, not per-panel plumbing: `begin_frame`
(manager `draw()`, allowed only in single view without a console
overlay) → the image draw claims its key → `end_frame` drops any
unclaimed live placement. Erase discipline: the reconcile runs AFTER the
draw pass, so it must never write cell content — kitty deletes by id
(`a=d,d=I`; pixels float above cells), sixel needs nothing at all
(sixel pixels ARE cell content and the frame's full repaint already
overwrote them; a space-overwrite here would wipe the freshly drawn
cells). The only cell writes the emitters do are for their own target
region, right before the raster; the image draw also repaints the pane
strip beside a narrower-than-pane raster every frame (`blank_cells`),
keeping the full-repaint invariant. The sixel raster is pre-fitted to
the pane pixel box and truncated to whole 6-row bands, so it cannot
overflow neighbouring panels. Emit errors fall back to the half-block
loop for that frame. Caveat: tmux/screen swallow both protocols — auto
always resolves to half-block there. Explicit pins are honored but NOT
passthrough-wrapped: pinned kitty inside tmux stays blank regardless of
allow-passthrough, pinned sixel renders only in a sixel-enabled tmux
build. The startup probe consumes any keystrokes typed during its
bounded window along with the reply bytes (accepted D1 trade-off).

## Architecture: native preview backends

Previews (`src/panel/preview.rs`) are native-first: each arm of the
dispatch in `FilePreview::new` tries an in-process backend and keeps the
old shell-out as last-resort fallback, so nothing regresses on exotic
inputs. Crates (all version-pinned for MSRV 1.83): `zip` (listing via
the central directory, `by_index_raw` never inflates), `tar` (streaming,
generic over `Read` — only the first 128 headers are read), `flate2`
(gzip), `x509-parser` (PEM/DER certs), `lofty` (audio tags/properties),
`image` (decode + native info lines, no mediainfo), `infer` (content
sniffing). Video is unchanged (ffmpeg thumbnail → mediainfo); generic
application/* gets a dependency-free stat block.

Line-based preview conventions: the 128-line cap applies everywhere,
and `bat_preview`'s `\r`/`\n` scrub (`scrub_line`) is applied to every
attacker-controlled string — archive member names, audio tags, cert
fields — one entry, one line.

The gzip arm decompresses one 512-byte block and sniffs the tar magic
(`ustar` at offset 257, covers POSIX and GNU): tar.gz chains head+rest
into `native_tar_list`; any other gzip shows its decompressed head as
text (bounded 64 KiB) — fixing the old everything-gzip-is-a-tar
mis-dispatch.

MIME detection (`get_mime_type`, `src/engine/opener.rs`): special-cased
extensions first, then mime_guess; only when there is no extension or
no real guess (octet-stream) is content sniffed — shebang, then `infer`
magic numbers, then a mostly-printable-UTF-8 heuristic, else the old
text/plain fallback. The sniff NEVER opens non-regular files (open() on
a FIFO blocks until a writer appears and would wedge the draw loop;
device nodes can block or have side effects) and caches results keyed
on (path, mtime), because per-entry styling re-sniffs every visible
extensionless file on each repaint.

PDF is tiered: optional external image tier (render page 1) → native
text tier (lopdf: `PDF · N pages`, /Info Title/Author/Producer, page-1
`extract_text`; encrypted docs show `encrypted PDF (N pages)` +
Size/Modified only) → stat block; never a bare error panel. lopdf is
pinned `>=0.36, <0.37` with `default-features = false, features =
["time"]` (0.37+ need rustc ≥ 1.85; defaults would pull edition2024
`jiff`, plus rayon — rayon staying OFF keeps `load_filtered`
single-threaded, which the guard's `thread_local!` budget relies on).
lopdf's stream decompression is unbounded, so `load_pdf_guarded` arms a
per-document budget (`PDF_DECOMP_BUDGET`, 64 MiB) and a
`load_filtered` guard filter that size-verifies every *object-loop*
stream BEFORE lopdf inflates it (counting zlib decode for a sole
FlateDecode / pessimistic `1032^k:1` for LZW or a k-stage inflating
chain — the single 1032 multiply undercounts a flate-of-flate bomb;
over budget → object dropped, budget zeroed). The guard filter does
NOT cover cross-reference STREAMS (`/Type /XRef /Filter /FlateDecode`,
plus any `/Prev` / `/XRefStm` chain): lopdf inflates those in
`decode_xref_stream` during xref parsing, BEFORE any FilterFunc exists,
so a 48 KiB ratio-bomb xref stream would OOM the process on plain
cursor navigation. `pdf_xref_streams_within_budget` pre-scans the raw
bytes and rejects the file before handing it to lopdf (same
counting/pessimistic charge; conservative — classic `xref` tables and
ambiguous cases pass through). The source itself is bounded by a
`PDF_SOURCE_MAX` (32 MiB) pre-check and lines by `PDF_LINE_MAX`. The image tier is gated by `pdf_render`
(config, default OFF — hard off, no auto-enable) + a two-candidate
probe (`pdftoppm -v`, else `mutool -v`; accepted on spawn AND (success
OR "version" in output)); it renders into the raster cache under kind
`pdf-p1-960` (bump the kind when page/scale change) and writes only
where `video_thumbnail_dir()` allows — `preview_cache = false` skips
the tier entirely (an external render IS a write).

Image decoding: `image` is pinned `>=0.25.5, <0.25.7` with
`default-features = false` and an explicit format list (= defaults
minus `avif`) — naive `image = "0.25"` resolves to 0.25.10 (rustc
1.88) and 0.25.8 pulls edition2024 crates; cargo 1.83 fails at
manifest parse. **image's `rayon` feature must stay OFF**: its
`ravif?/threading` weak-dep reference alone drags edition2024
`avif-serialize` into resolution and breaks the 1.83 build even with
no avif feature enabled (verified feature-by-feature). JPEG XL is
native via `jxl-oxide` (pinned `>=0.11, <0.12`, pure Rust; the
`image` feature is its ImageDecoder integration and needs image
≥ 0.25.5). AVIF/HEIC stay on the mediainfo fallback deliberately:
the decoders (dav1d, libheif) are system C libraries. EXIF
orientation is applied at decode time (`decode_upright`), so cached
rasters are upright by construction — the cache-hit path never
re-rotates (kind `img960u`; the `u` bump invalidates pre-orientation
entries). All decodes go through `arm_alloc_limits`: the
`into_decoder()`/`from_decoder()` split skips `decode()`'s
512 MiB `Limits::default()` reserve, and jxl-oxide's tracker starts
at usize::MAX — without the explicit reserve+set_limits a crafted
small file claiming huge dimensions materializes a multi-GB buffer
on cursor navigation (bomb-tested for both JPEG and JXL).

External producers (ffmpeg, pdftoppm/mutool) run under `run_bounded`
(`EXTERNAL_RENDER_DEADLINE`, 10s): kill-then-reap on overrun, capped
pipe drains, caller removes its `.part` on Err. The availability
probes (`ffmpeg -h`, `pdftoppm/mutool -v`) are bounded too
(`PROBE_DEADLINE`) — an unbounded probe `wait()` wedges the preview
path forever when the *binary's startup* hangs, defeating the render
deadline. The video thumbnail filter is `scale=120:-1,thumbnail`
(scale FIRST: `thumbnail` buffers its ~100-frame selection window at
whatever resolution it is fed — at input resolution that is ~3 GB
transient RSS for a 4K clip, at 120px it is negligible; selection on
scaled frames is still representative).

Diagnostics: every native-backend failure logs a debug-level
"... failed, trying <tool>" line before falling back. With
`--debug-socket` these land in the `log` history — a preview that
unexpectedly comes from a shell-out is visible there.

## Architecture: undo/redo

In-session, in-memory only (`src/undo/`, terminal-free + unit-tested).
Everything reduces to atomic reversible changes — `FsChange::{Create, Move,
Copy, Trash}`; one user action = one `Transaction` of N changes. The FS
primitives (`move_item`/`copy_item` in util.rs, `delete_file`, zip/tar)
return the `FsChange` they made, carrying the *actually-reached* path so
the `_`-suffix collision fallback stays correct. The command handlers in
`manager.rs` assemble transactions and call `undo.record()`; undo/redo are
applied via `apply_undo`/`apply_redo` without re-recording (they push onto
the opposite stack). The async paste task hands its transaction back over
`undo_tx`/`undo_rx` to be recorded on the main loop.

Non-reversible actions push a `Barrier` (permanent delete — trash off);
undo hitting it stops and reports, never reverting past it. External
commands/opener/extract are untracked (ignored). zip/tar are `no_redo`
(a re-run would only produce an empty archive). Trash-delete IS redoable:
`FsChange::undo/redo` take `&mut self` so `Trash::redo` can re-trash and
re-capture the fresh `TrashItem` (shared `capture_trashed` in `src/undo/`),
keeping the delete↔undo↔redo cycle consistent.
Keys: `u` / `ctrl-r` (`undo`/`redo` defaults in `[keys.manipulation]`,
default-on for old configs too, user-wins on collision).

## Architecture: trash

The freedesktop.org trash (via the `trash` crate, pinned `<5.2` for MSRV
1.83; native FS backend, no DBus). `use_trash` (config, default true) gates
it: on → `delete_file` calls `trash::delete` then re-finds the created
`TrashItem` via `os_limited::list()` (match original_parent+name, newest
`time_deleted`) and records `FsChange::Trash`; off → permanent delete +
`Barrier`. Undo restores via `os_limited::restore_all`. `gT` opens the
`TrashView` overlay mode (`src/panel/mode/trash_view.rs`) — a pure adapter
the manager fills from `os_limited::list()`; `r` emits
`ModeOp::RestoreFromTrash { items, cursor }` (restore to original; not
undo-recorded in v1). The view **stays open** after a restore: the manager
rebuilds a fresh `TrashView` from the now-smaller trash and holds the cursor
at its old slot (`set_cursor`, clamped) so the next item slides up into it —
several items can be restored in a row. It stays open even once empty
(showing "(the trash is empty)"). `q` / `Esc` close. Deferred:
empty-trash/`purge_all`, restore-to-arbitrary (the old `gT`+`dd` pull-out),
multi-select.

Debug socket `state` exposes
`undo_depth` / `redo_depth`.

## Architecture: preview raster cache

Persistent image/video preview rasters in `$XDG_CACHE_HOME/rfm/thumbnails/`
(`src/panel/raster_cache.rs`; created 0700 per the XDG basedir spec). The
filename is the entire metadata: `<seahash(abs path):016x>-<mtime_secs>-
<kind>.jpg`, with kinds `img960u` (image thumbnails, bounded to 960×540 —
never upscaled — EXIF-upright by construction; the `u` is the
orientation bump) and `vid120` (ffmpeg frames, `scale=120:-1`). Same
path+mtime with two kinds coexist; a store's stale-sibling sweep deletes
everything with the `<hash>-` prefix OUTSIDE the keep scope
`<hash>-<mtime>-` (old mtimes of any kind, plus their orphaned `.part`s).

Writes are atomic: same-dir `<final>.<pid>-<seq>.part` temp name (the
per-process counter matters — the directory preloader and the on-demand
preview task can store the same entry concurrently), then rename; the
ffmpeg producer keeps `.jpg` LAST in its part name so container inference
works. Lookups apply the corrupt-entry rule (decode failure → delete +
regenerate — both producers, never a blank preview). Everything is
best-effort: a cache fault only costs a recompute, and the video producer
re-creates the dir before each ffmpeg run, so `rm -rf ~/.cache/rfm` is
safe mid-session. Startup prune (spawn_blocking): 30-day age, then
oldest-first down to 256 MB (one shared budget).

`preview_cache` (config, default true) gates it via
`raster_cache::init(enabled)` in main. `false` is a privacy promise —
nothing about the user's files is written: images run in-memory
(`native_image_preview`), videos degrade to mediainfo text (no ffmpeg
thumbnail at all). Enabled-but-unavailable (no resolvable cache home) is
different: videos then fall back to the pre-cache `temp_dir()/
rfm-thumbnails` (7-day prune). See `video_thumbnail_dir_from`.

Testing: unit tests never call `init` (uninitialized == opted out); the
producers are dir-parameterized (`cached_image_preview_in`,
`ffmpeg_thumbnail(dir, …)`) so tests pass tempdirs. E2e: launch in tmux
with `XDG_CACHE_HOME=$(mktemp -d)` in the pane before rfm, then assert on
the entries in `$XDG_CACHE_HOME/rfm/thumbnails` and grep the socket `log`
for "raster cache hit".
