# Section 07 — Opener and MIME handling

N=07, SESSION=rfm-sec07, SOCK=/tmp/rfm-sec07.sock. Harness, launch line and the
per-step verification loop are defined in ../README.md — read that first.

Source of truth for this section: `src/engine/opener.rs` (`get_mime_type`,
`sniffed`/`sniff_mime`, `Application::open`, `OpenOptions::open`,
`OpenEngine::open`), `src/panel/manager.rs` (`move_right` → `MoveRight::OpenFile`,
`draw_footer`/`print_metadata`), `src/util.rs::print_metadata`,
`examples/default-config.toml` `[open.*]`.

Facts this section relies on (all verified in source):

- Opening is triggered by `move right` on a non-directory selection: keys `l`
  and the Right arrow (Enter is NOT bound to open in normal mode).
- The manager logs INFO `Opening '<abs path>'`; the resolved application logs
  INFO `Opening '<abs path>' with '<name>'`; a failed open (e.g. missing
  binary) logs ERROR `Opening failed: <err>`. When a section defines
  `extensions`, INFO `checking extensions: [...]` is logged before routing.
- Openers come from config `[open.<type>]` (`text|image|audio|video|application`),
  each an `OpenOptions { default, extensions }`; per-extension entries are
  checked first (exact extension string match), then `default`. An UNSET
  section falls back to the system opener (`opener::open` / xdg-open) —
  never trigger that path in tests (it may launch real GUI apps); every mime
  type opened in this section is pinned in the scratch config.
- `$EDITOR` is NOT consulted for normal opens (only in the bulk-rename
  blocking path, and only when `[open.text]` is unset). Do not test via
  `$EDITOR`; pin openers in config.
- `Application` spawns `name args... <path>` directly via `Command::new` — no
  shell — so paths with spaces/`&` must arrive as one intact argv element.
  `terminal = true` blocks the event loop until the child exits (the fake
  opener must exit immediately); by the time `await-idle` returns, the marker
  file is written.
- `get_mime_type` order: special-cased extensions (`ts`→`text/javascript`,
  `zst|tzst`→`application/zstd`, `txz`, `tbz2`, `nix`, `vue`, …) → if NO
  extension: content sniff → else `mime_guess`; only a missing/octet-stream
  guess falls through to the sniff. A known extension always wins over
  content. Sniff order: empty→None, `#!` shebang→`text/plain`, `infer` magic
  numbers (png/zip/pdf/elf/…), mostly-printable-UTF-8→`text/plain`, else None;
  the caller falls back to `text/plain` on None. The sniff does ONE bounded
  512-byte read, is cached keyed on (path, mtime), and NEVER opens
  non-regular files (`metadata().is_file()` guard — FIFOs/devices).
- The footer (bottom row) in normal mode shows
  `<permissions>   <user> <group> <size> <modified> <mime>` for the current
  selection plus an `n/m` index — the mime string of the selection is directly
  screen-assertable. The log widget occupies rows ABOVE the footer (footer−2
  upward) and never covers it; collapsed (default) it shows only warn+ lines.

## Section fixture and config

```bash
PARENT=$(mktemp -d); FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"   # quiet parent (README)
printf 'hello opener\n'            > "$FIXTURE/note.txt"
printf '# heading\n'               > "$FIXTURE/notes.md"
printf 'ampersand test\n'          > "$FIXTURE/a file & test.txt"
printf '#!/bin/sh\necho hi\n'      > "$FIXTURE/script"      # extensionless shebang
printf 'just plain words\n'        > "$FIXTURE/readme"      # extensionless printable
printf '\000\001\002\376\377\000'  > "$FIXTURE/garbage"     # extensionless binary junk
printf '\211PNG\r\n\032\n'         > "$FIXTURE/pngmagic"    # extensionless, PNG magic
printf 'not really zstd\n'         > "$FIXTURE/data.zst"
printf 'export const x = 1;\n'     > "$FIXTURE/mod.ts"
printf 'PK\003\004fake zip body\n' > "$FIXTURE/lies.txt"    # zip magic, .txt ext
printf 'PK\003\004fake zip body\n' > "$FIXTURE/ar.zip"
mkfifo "$FIXTURE/pipe"

CFG=$(mktemp -d)
cat > "$CFG/fake-opener.sh" <<EOF
#!/bin/sh
printf '%s\n' "\$@" >> "$CFG/opened.log"
EOF
chmod +x "$CFG/fake-opener.sh"
cat > "$CFG/config.toml" <<EOF
[open.text]
default = { name = "$CFG/fake-opener.sh", args = ["TEXT-DEFAULT"], terminal = true }
extensions = [
  ["md", { name = "$CFG/fake-opener.sh", args = ["TEXT-MD"], terminal = true }]
]

[open.application]
default = { name = "$CFG/fake-opener.sh", args = ["APP-DEFAULT"], terminal = true }

[open.image]
default = { name = "/nonexistent/rfm-test-opener", args = [], terminal = true }
EOF
```

Note the heredocs: `$CFG` expands at fixture-creation time (absolute paths get
baked in), while `\$@` stays literal inside the script. The fake opener only
appends its argv to `$CFG/opened.log` — it must NEVER read the file it is
handed (a read would hang on the FIFO step). Launch per README with
`--debug-socket $SOCK --config $CFG "$FIXTURE"`.

Helper used below — **SELECT(name)**: send `g` `g` (`tmux send-keys -t $SESSION g g`),
`await-idle`, then loop at most `state.total` times: if `state.selection ==
name` stop, else send `j` and `await-idle` again. Fail the step if the name is
never reached. (Never navigate by counting rows: sort order of this mixed
fixture is not part of the contract.)

Helper — **MARKER_LINES(n)**: `tail -n <n> "$CFG/opened.log"`.

---

### 07.1 — Launch: FIFO in the listing does not wedge startup
**Action:** Standard launch (fixture and config above). Then
`echo await-idle | socat - UNIX-CONNECT:$SOCK`, `echo state | socat - UNIX-CONNECT:$SOCK`,
`echo "entries center" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `await-idle` returns `{"idle":true,...}` promptly (well
under the 30 s cap). `state` replies within ~1 s (a 10 s timeout reply =
wedged event loop = bug). `mode=="normal"`, `cwd==$FIXTURE`, `total==12`.
`entries center` lists all 12 names including `pipe`, exactly one entry with
`selected:true`, none `hidden`, none `marked`.
**Expect (screen):** Header shows `user@host` and the fixture path; center pane
lists the 12 fixture names; bottom row shows a permissions string, metadata and
a mime type for whatever is initially selected; `1/12` index visible.
**Note:** No directories in the fixture, so the initial selection is the first
file in sort order — do not assert which one. The FIFO merely being listed and
stat-ed must not block anything; that is the point of this step.

### 07.2 — Footer mime: plain `.txt` via mime_guess
**Action:** SELECT(`note.txt`). Then `state`, capture-pane.
**Expect (socket):** `selection=="note.txt"`; seq increased since 07.1 and is
stable across two consecutive `state` queries with no input between them.
**Expect (screen):** Bottom row contains `text/plain` and starts with a regular
file permissions string (`-rw-...`); `note.txt` is the highlighted row in the
center pane; right pane shows the file's text preview (`hello opener`).

### 07.3 — Footer mime: `.ts` special-case beats mime_guess
**Action:** SELECT(`mod.ts`). Capture-pane.
**Expect (socket):** `selection=="mod.ts"`.
**Expect (screen):** Bottom row contains `text/javascript` — NOT `video/mp2t`
(what raw mime_guess would say for `.ts`). This proves the special-cased
extension table in `get_mime_type` is active.

### 07.4 — Footer mime: `.zst` special-case (no sniff for known-compound extensions)
**Action:** SELECT(`data.zst`). Capture-pane.
**Expect (socket):** `selection=="data.zst"`.
**Expect (screen):** Bottom row contains `application/zstd`, even though the
file content is plain text — the extension special-case answers without
sniffing.

### 07.5 — Extension wins over content: `.txt` with zip magic stays text
**Action:** SELECT(`lies.txt`). Capture-pane.
**Expect (socket):** `selection=="lies.txt"`.
**Expect (screen):** Bottom row contains `text/plain`, NOT `application/zip`.
mime_guess has a real answer for `.txt`, so the content (PK magic) is never
sniffed.

### 07.6 — Sniff: extensionless shebang script → text/plain
**Action:** SELECT(`script`). Capture-pane.
**Expect (socket):** `selection=="script"`.
**Expect (screen):** Bottom row contains `text/plain`; right pane shows the
script's lines (`#!/bin/sh`, `echo hi`) as a text preview.

### 07.7 — Sniff: extensionless printable text → text/plain
**Action:** SELECT(`readme`). Capture-pane.
**Expect (socket):** `selection=="readme"`.
**Expect (screen):** Bottom row contains `text/plain` (mostly-printable-UTF-8
heuristic); right pane previews `just plain words`.

### 07.8 — Sniff: extensionless binary garbage → text/plain fallback, no crash
**Action:** SELECT(`garbage`). Capture-pane. Also `echo "log 30" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `selection=="garbage"`; `log` contains no ERROR lines
caused by this selection; `state` stays responsive.
**Expect (screen):** Bottom row contains `text/plain` — the sniff returns None
for this byte pattern and the caller falls back to text/plain (documented
behavior; the footer cannot distinguish sniff-text from fallback — 07.9 can).
**Note:** The preview pane may render the junk bytes as (near-empty) text;
assert only that no error panel/log appears and the UI stays live.

### 07.9 — Sniff: extensionless PNG magic → image/png (proves `infer` is consulted)
**Action:** SELECT(`pngmagic`). Capture-pane.
**Expect (socket):** `selection=="pngmagic"`.
**Expect (screen):** Bottom row contains `image/png`. This is the step that
distinguishes real magic-number sniffing from the text/plain fallback: only
the `infer` path can produce `image/png` for an extensionless file.
**Note:** The PREVIEW of this file will fail to decode (it is 8 bytes of
magic, not a real PNG) — preview content is out of scope here; only the footer
mime matters. Debug-level `native ... failed` lines in `log` are acceptable.

### 07.10 — Sniff cache invalidates on mtime change
**Setup:** `garbage` is still the selection target from 07.8's fixture state.
**Action:** From the harness shell: `printf '\211PNG\r\n\032\n' > "$FIXTURE/garbage"`.
SELECT(`garbage`) (re-select to be deterministic), `await-idle`, capture-pane.
If the footer still shows the old value, force one repaint without changing
selection: send `z` `h` twice (toggle_hidden on and off), `await-idle`,
re-capture.
**Expect (socket):** `selection=="garbage"`; seq increased.
**Expect (screen):** Bottom row now contains `image/png` for `garbage` — the
(path, mtime) sniff cache re-sniffed after the rewrite instead of serving the
stale `text/plain`.
**Note:** The file watcher also fires on the rewrite (panel reload → repaint);
the `zh`+`zh` nudge is only the deterministic fallback. Do not assert on the
transient state between rewrite and repaint.

### 07.11 — Open a `.txt`: `[open.text].default` fires, marker written
**Action:** `rm -f "$CFG/opened.log"`. SELECT(`note.txt`). Send `l`.
`await-idle`. Then `state`, `log 30`, capture-pane, and read `$CFG/opened.log`.
**Expect (socket):** `mode=="normal"`, `cwd` unchanged (`$FIXTURE`),
`undo_depth` unchanged (opens are not undo-tracked). `log` contains INFO
`Opening '$FIXTURE/note.txt'` (manager) and INFO
`Opening '$FIXTURE/note.txt' with '$CFG/fake-opener.sh'` (application).
**Expect (screen):** Full UI is back after `await-idle` (header path + center
listing visible — `terminal=true` cleared the screen for the child, and the
manager repainted after it exited). `note.txt` still highlighted.
**Expect (fs):** `MARKER_LINES(2)` == `TEXT-DEFAULT` then the absolute path
`$FIXTURE/note.txt`, each on its own line.
**Note:** `terminal=true` blocks the event loop while the child runs — the
fake opener exits immediately, so `await-idle` returning implies the marker is
already on disk. No race.

### 07.12 — Per-extension routing: `.md` hits the `extensions` entry, not `default`
**Action:** SELECT(`notes.md`). Send `l`. `await-idle`. Read `log 30` and the
marker.
**Expect (socket):** `log` contains INFO `checking extensions:` and INFO
`Opening '$FIXTURE/notes.md' with '$CFG/fake-opener.sh'`; `mode=="normal"`.
**Expect (screen):** UI repainted, `notes.md` highlighted.
**Expect (fs):** `MARKER_LINES(2)` == `TEXT-MD` then `$FIXTURE/notes.md` — the
`md` extension entry won over `TEXT-DEFAULT` (`.md` is `text/markdown`, type
`text`, so it routes through `[open.text]` and its extension list first).

### 07.13 — Application mime routes through `[open.application]`
**Action:** SELECT(`ar.zip`). Send `l`. `await-idle`. Read `log 30` and the
marker.
**Expect (socket):** `log` contains `Opening '$FIXTURE/ar.zip' with
'$CFG/fake-opener.sh'`; `mode=="normal"`.
**Expect (screen):** UI repainted; footer for `ar.zip` shows
`application/zip`.
**Expect (fs):** `MARKER_LINES(2)` == `APP-DEFAULT` then `$FIXTURE/ar.zip`.

### 07.14 — Shell-safety: filename with spaces and `&` arrives as one intact argv
**Action:** SELECT(`a file & test.txt`). Send `l`. `await-idle`. Read the
marker.
**Expect (socket):** `log` contains `Opening '$FIXTURE/a file & test.txt'`;
no ERROR lines from this open.
**Expect (fs):** `MARKER_LINES(2)`: line 1 `TEXT-DEFAULT`, line 2 the EXACT
string `$FIXTURE/a file & test.txt` — one line, spaces and `&` intact, no
quoting artifacts, and no extra lines (which would indicate word-splitting).
The opener is spawned via `Command::new` with the path as a single arg — no
shell is involved; this step guards that invariant.
**Expect (screen):** UI repainted, selection unchanged.

### 07.15 — Sniff drives the open-rule choice: extensionless shebang opens as text
**Action:** SELECT(`script`). Send `l`. `await-idle`. Read the marker and
`log 30`.
**Expect (socket):** `log` contains `Opening '$FIXTURE/script' with
'$CFG/fake-opener.sh'`.
**Expect (fs):** `MARKER_LINES(2)` == `TEXT-DEFAULT` then `$FIXTURE/script` —
the shebang sniff produced `text/plain`, routing to `[open.text]` (not the
application section, not xdg-open).
**Expect (screen):** UI repainted.

### 07.16 — Failing opener binary: error surfaced, UI recovers
**Action:** SELECT(`pngmagic`) (mime `image/png` → `[open.image]` →
`/nonexistent/rfm-test-opener`). Send `l`. `await-idle`. Then `log 30`,
`state`, capture-pane, then send `j` and `k` and `await-idle` again.
**Expect (socket):** `log` contains ERROR `Opening failed:` (spawn error for
the missing binary, e.g. "No such file or directory") preceded by INFO
`Opening '$FIXTURE/pngmagic' with '/nonexistent/rfm-test-opener'`.
`mode=="normal"`; after the `j`/`k` pair, `state` responds normally and
`selection` is back on `pngmagic` — raw mode was restored by the guard, keys
still work.
**Expect (screen):** Full UI repainted after `await-idle` (the open path
clears the screen before resolving the app; the error path must not leave a
blank/cooked terminal). The collapsed log widget row (two rows above the
footer) may show the error line for its 10 s TTL.
**Expect (fs):** `opened.log` did NOT grow (no fake opener involved).

### 07.17 — FIFO selection: blank preview, no event-loop wedge
**Action:** SELECT(`pipe`). `echo await-idle | timeout 12 socat - UNIX-CONNECT:$SOCK`,
then `echo state | timeout 12 socat - UNIX-CONNECT:$SOCK`. Capture-pane.
**Timing:** `/usr/bin/time` and `bc` may be absent — do NOT rely on them. The
wedge guard is the `timeout 12` wrapper itself: a `state` reply arriving at all
inside 12 s already proves no-wedge. If you want an explicit number, bracket the
`state` call with `start=$(date +%s%N); …; end=$(date +%s%N)` and diff in
nanoseconds. A `timeout` with no reply (or a `{"error":"timeout: ..."}`) means
the sniff/preview path opened the FIFO and wedged — that is the regression this
step exists for.
**Expect (socket):** `await-idle` returns idle; `state` replies in well under
12 s (normally <1 s), `selection=="pipe"`, `seq` stable across ≥2 polls.
**Expect (screen):** Bottom row's permissions string starts with `p` (e.g.
`prw-...`) and the mime field shows `text/plain` (sniff refuses non-regular
files → fallback). The **load-bearing** assertion is the no-wedge / responsive
socket + the correct footer.
**Known bug (BUG-2, run 1 — being fixed via TDD):** on the current binary the
right (preview) pane stays stuck on a `Loading...` + path placeholder for a
selected FIFO instead of the intended blank `PreviewPanel::Empty` (the
directory-panel `loading` flag is never cleared for a non-regular file). Until
the fix lands, treat a stuck `Loading...` here as the documented BUG-2 failure
mode (screen only — no wedge, footer correct), not a fresh finding. Once fixed,
the preview pane should be blank.
**Note:** `await-idle` does not cover in-flight preview tasks; if anything
looks mid-load, poll `state` until `seq` is stable across two queries, then
assert.

### 07.18 — Pressing `l` on the FIFO must not hang
**Action:** With `pipe` selected, send `l`. `await-idle` (must return within
the 30 s cap — expect ~instant). Read `log 30`, the marker, and `state`.
**Expect (socket):** `await-idle` returns idle promptly; `mode=="normal"`;
`log` contains `Opening '$FIXTURE/pipe'` and `Opening '$FIXTURE/pipe' with
'$CFG/fake-opener.sh'`; no new ERROR lines.
**Expect (fs):** `MARKER_LINES(2)` == `TEXT-DEFAULT` then `$FIXTURE/pipe`.
**Expect (screen):** UI repainted, still responsive (send `k`, `await-idle`,
`state` moves the selection).
**Note:** Current behavior, deliberately enshrined only for its liveness
property: `move_right` treats any non-directory as openable, the mime for the
extensionless FIFO falls back to `text/plain`, and the text opener receives
the FIFO path. The load-bearing assertion is "nothing blocks" — the fake
opener must never read its argument (see fixture note). If routing details
change (e.g. rfm refuses to open non-regular files), update the routing
expectations but keep the no-hang assertions.

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$PARENT" "$CFG"; rm -f $SOCK` (`$PARENT` wraps the quiet-parent `$FIXTURE`).
(the FIFO is removed with the fixture dir; the session kill also reaps any
stuck opener child).

**Section coverage gaps (deliberate):**
- The unset-section fallback to the system opener (`opener::open`/xdg-open)
  and the "unknown mime-type" arm — untestable hermetically (would launch real
  desktop applications).
- `terminal = false` (detached GUI spawn + reaper thread) — asserting on a
  detached child's marker is racy; only `terminal = true` is exercised.
- `[open.audio]` / `[open.video]` sections — same mechanism as
  text/application (one shared `OpenOptions` code path), not repeated.
- `$EDITOR`/`vi` fallback in `open_text_blocking` — bulk-rename path, covered
  by the file-manipulation section, not here.
- Per-entry `[styles.*]` mime-based coloring — capture-pane text carries no
  color information; mime correctness is asserted via the footer string
  instead.
- Sniff-cache eviction at 4096 entries, device nodes (would need root), and
  symlink-to-regular-file sniffing (metadata follows symlinks) — not covered.
- Legacy `open.toml` fold-in to `[open]` — config-migration section's scope.
- Archive extract/zip/tar shell-outs (also in opener.rs) — archive section's
  scope.
