# Section 08 — Previews: text and native backends

N=08, SESSION=rfm-sec08, SOCK=/tmp/rfm-sec08.sock. Harness per README.

Scope: the right (preview) pane for text files and every native (in-process)
preview backend — plain text, source, line cap, empty file, zip/tar/tar.gz
listings, plain gzip head, PEM certificates, the PDF native text tier
(`pdf_render` stays default OFF), the generic application/* stat block,
extensionless content sniffing, broken symlinks, and the
"native backend failed → shell-out fallback" log line.

All expected strings in this section were verified against
`src/panel/preview.rs` (dispatch `FilePreview::new` ~line 364; listing
formats `native_zip_list`/`native_tar_list`; cert `native_cert_lines`;
PDF `native_pdf_lines`; stat `stat_block_lines`; fallback log lines) and
validated in a live run of the current develop build.

## Section fixture (Setup once, before launch)

**Important deviation from the plain README fixture:** rfm is launched in a
`work/` SUBDIRECTORY of `$FIXTURE`, not in `$FIXTURE` itself. Reason
(observed): when the cwd's parent is `/tmp`, the left panel points at `/tmp`
and preview/sniff churn over unrelated `/tmp` files (e.g. Chromium temp
files) floods the 200-line log retention, evicting the debug lines this
section asserts on. With `work/` the left panel is the near-empty
`$FIXTURE` and the log stays quiet.

```bash
FIXTURE=$(mktemp -d); CFG=$(mktemp -d)
mkdir "$FIXTURE"/work && cd "$FIXTURE"/work

printf 'hello\nworld\n' > note.txt                        # plain text
printf 'fn main() {}\n' > main.rs                         # source file
seq 1 300 > big.txt                                       # >128 lines
: > empty.txt                                             # empty file
touch a.txt b.txt
zip -q ar.zip a.txt b.txt                                 # zip
tar cf ar.tar a.txt                                       # tar
tar czf ar.tar.gz a.txt                                   # tar.gz
printf 'plain gz body\n' | gzip > f.txt.gz                # plain (non-tar) gz
openssl req -x509 -newkey rsa:2048 -nodes -keyout /dev/null \
  -subj '/CN=test.example' -addext 'subjectAltName=DNS:test.example' \
  -days 1 -out cert.crt 2>/dev/null                       # PEM cert (.crt ext)
printf 'x' > f.wasm                                       # generic application/*
printf '#!/bin/sh\necho hi\n' > script                    # extensionless, shebang
printf 'just words\n' > readme                            # extensionless, printable
ln -s /nonexistent dangling                               # broken symlink
printf 'not a zip' > bad.zip                              # forged native failure
# Minimal VALID one-page PDF (correct xref offsets; /Info Title; page-1 text).
# Byte-exact — trailing spaces in the xref are significant, hence base64:
base64 -d > doc.pdf <<'PDFEOF'
JVBERi0xLjQKMSAwIG9iajw8L1R5cGUvQ2F0YWxvZy9QYWdlcyAyIDAgUj4+IGVuZG9iagoyIDAg
b2JqPDwvVHlwZS9QYWdlcy9LaWRzWzMgMCBSXS9Db3VudCAxPj4gZW5kb2JqCjMgMCBvYmo8PC9U
eXBlL1BhZ2UvUGFyZW50IDIgMCBSL01lZGlhQm94WzAgMCAxMDAgMTAwXS9Db250ZW50cyA0IDAg
Ui9SZXNvdXJjZXM8PC9Gb250PDwvRjEgNSAwIFI+Pj4+Pj4gZW5kb2JqCjQgMCBvYmo8PC9MZW5n
dGggNDk+PnN0cmVhbQpCVCAvRjEgMTIgVGYgMTAgNTAgVGQgKGhlbGxvIGZyb20gcGFnZSBvbmUp
IFRqIEVUCmVuZHN0cmVhbSBlbmRvYmoKNSAwIG9iajw8L1R5cGUvRm9udC9TdWJ0eXBlL1R5cGUx
L0Jhc2VGb250L0hlbHZldGljYT4+IGVuZG9iago2IDAgb2JqPDwvVGl0bGUoaGVsbG8gcmZtKT4+
IGVuZG9iagp4cmVmCjAgNwowMDAwMDAwMDAwIDY1NTM1IGYgCjAwMDAwMDAwMDkgMDAwMDAgbiAK
MDAwMDAwMDA1MyAwMDAwMCBuIAowMDAwMDAwMTAzIDAwMDAwIG4gCjAwMDAwMDAyMTQgMDAwMDAg
biAKMDAwMDAwMDMwOSAwMDAwMCBuIAowMDAwMDAwMzcxIDAwMDAwIG4gCnRyYWlsZXI8PC9Sb290
IDEgMCBSL0luZm8gNiAwIFIvU2l6ZSA3Pj4Kc3RhcnR4cmVmCjQwNwolJUVPRgo=
PDFEOF
```

Launch per README (isolation env exported in the pane first), but with the
`work/` subdir as the start path:

```
./target/debug/rfm --debug-socket $SOCK --config $CFG $FIXTURE/work
```

Tool requirements: `zip`, `tar`, `gzip`, `openssl`, `base64` are needed only
to CREATE fixtures — every preview asserted here is native (in-process).
`bat` is optional: its absence changes styling only, never the text content
asserted below. `unzip` affects only the on-screen expectation of 08.16
(both outcomes specified there).

**Entry order** (17 entries, no directories, no hidden files — selection
starts at index 0):
`a.txt, ar.tar, ar.tar.gz, ar.zip, b.txt, bad.zip, big.txt, cert.crt,
dangling, doc.pdf, empty.txt, f.txt.gz, f.wasm, main.rs, note.txt, readme,
script`

## Section procedures

**SELECT(name):** step **toward** the target the shortest way — read `entries
center`, find `<name>`'s **visible** index, compare with `state.selected_idx`,
then send `j` (target below) or `k` (target above) exactly that many times, one
keypress per `await-idle`, confirming via `state.selection`. Do NOT press blind
`j` until it matches: overshooting wraps the list and each stray keypress emits
~3 TRACE lines that flood the 200-line log ring and **evict the very
preview-backend DEBUG lines this section asserts on** (08.6/08.9/08.10/08.11/
08.16/08.17). Minimal keystrokes keep those lines within retention. See the
README SELECT helper.

**PREVIEW-SETTLE:** after SELECT, previews are rate-limited
(`rate_limit_interval_ms` = 500, default) and load async — `await-idle` does
NOT cover them. Sleep 0.7 s, run `await-idle`, then poll `state` twice 0.5 s
apart until `seq` is unchanged between polls. Only then assert.

**Screen geometry:** 120×30 pane. Row 1 is the header
(`user@host <abs path of selected entry>`). The preview pane is the
rightmost column, starting around column 58–60; preview text appears to the
right of the second `│` separator on each row. Assert preview content as
"a captured line contains `│ <text>` in its right portion" — the chosen
fixture strings do not collide with center-column file names.

**Graphics note:** this section is text-only, but should a raster preview
ever appear (it must not, in these steps), remember tmux forces
`image_protocol == "half-block"`: rasters are colored half-block cells
(`▄`), assert non-empty colored area, never exact glyphs.

---

### 08.1 — Launch baseline: half-block protocol, empty-file initial preview
**Action:** launch per section fixture; `await-idle`; then `echo state | socat - UNIX-CONNECT:$SOCK` and `echo "entries center" | socat - UNIX-CONNECT:$SOCK`; `tmux capture-pane -t $SESSION -p`.
**Expect (socket):** `mode=="normal"`, `view=="single"`, `total==17`, `selection=="a.txt"`, `selected_idx==0`, `image_protocol=="half-block"` (rfm runs inside tmux), `preview_path` ends in `/work/a.txt`. `entries center` lists all 17 names above, exactly `a.txt` has `selected:true`.
**Expect (screen):** header row contains `<FIXTURE>/work/a.txt`; center column lists the 17 entries with `a.txt` on the highlighted row; the preview column is blank (a.txt is a 0-byte file — zero preview lines is correct, not an error; there must be NO `Failed to open` / `Error:` text in the right column).
**Note:** there are no directories in the fixture, so the dirs-sort-first rule leaves `a.txt` (alphabetically first file) selected — do not expect a directory.

### 08.2 — Plain text file
**Action:** SELECT(`note.txt`), PREVIEW-SETTLE, then `state` + capture-pane.
**Expect (socket):** `selection=="note.txt"`; `preview_path` ends in `/work/note.txt`; `seq` stable across the two settle polls.
**Expect (screen):** preview column shows `hello` on its first content row (row 2) and `world` on the next; no other preview text below.
**Note:** with `bat` installed the lines are syntax-styled; `capture-pane -p` strips colors, so the assertable text is identical either way.

### 08.3 — Source file (text/* arm)
**Action:** SELECT(`main.rs`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="main.rs"`; `preview_path` ends in `/work/main.rs`.
**Expect (screen):** preview column shows `fn main() {}` on its first content row.

### 08.4 — >128 lines is capped at exactly 128
**Setup:** this step temporarily resizes the tmux window so all 128 lines are on screen: `tmux resize-window -t $SESSION -x 120 -y 145` (rfm receives the Resize event and repaints).
**Action:** resize as above; `await-idle`; SELECT(`big.txt`); PREVIEW-SETTLE; capture-pane; afterwards resize back: `tmux resize-window -t $SESSION -x 120 -y 30`; `await-idle`.
**Expect (socket):** `selection=="big.txt"`; `preview_path` ends in `/work/big.txt`.
**Expect (screen):** preview column shows the numbers `1` through `128`, one per row (line `1` at screen row 2, line `128` at screen row 129); count of numbered preview rows is exactly 128; the string `129` appears NOWHERE in the preview column; all preview rows below row 129 are blank. Verified regex on the captured pane: `grep -cE '│ 1?[0-9]{1,3}\s*$'` == 128, and `grep -E '│ 129\s*$'` matches nothing.
**Note:** the cap (`take(128)` / bat `--line-range=0:128`) truncates silently — no ellipsis or "more lines" marker is expected. If `resize-window` is unavailable (tmux < 2.9), kill and relaunch the session with `-y 145` instead.

### 08.5 — Empty file: blank preview, no error
**Action:** SELECT(`empty.txt`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="empty.txt"`; `preview_path` ends in `/work/empty.txt`; a follow-up `state` query returns promptly (loop not wedged).
**Expect (screen):** preview column is entirely blank; no `Failed to open`, no `Error:` anywhere in the right column.

### 08.6 — zip listing (native, central-directory only)
**Action:** SELECT(`ar.zip`), PREVIEW-SETTLE, `state` + capture-pane + `echo "log 50" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `selection=="ar.zip"`; `preview_path` ends in `/work/ar.zip`; the log history contains NO line `native zip list failed` mentioning `ar.zip` (the native backend must succeed for a valid zip).
**Expect (screen):** preview column shows exactly two member lines, format `{size:>8}  {name}`: a row containing `0 B  a.txt` and the next containing `0 B  b.txt` (two spaces between size and name; size right-aligned in 8 columns).
**Note:** `bad.zip` will legitimately put a `native zip list failed` line into the log in step 08.16 — the socket assertion here is about ar.zip specifically; run this step before 08.16 (section order does).

### 08.7 — tar listing (native, streaming)
**Action:** SELECT(`ar.tar`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="ar.tar"`; `preview_path` ends in `/work/ar.tar`; log contains no `native tar list failed` line.
**Expect (screen):** preview column shows exactly one member line, format `{mode} {size:>8}  {name}`: `-rw-r--r--      0 B  a.txt` (mode string first — a `-` filetype bit, not `?`; then right-aligned size; two spaces; name).

### 08.8 — tar.gz listing (gzip arm sniffs the tar magic)
**Action:** SELECT(`ar.tar.gz`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="ar.tar.gz"`; `preview_path` ends in `/work/ar.tar.gz`; log contains no `native gzip preview failed` line.
**Expect (screen):** identical listing format to 08.7: one row `-rw-r--r--      0 B  a.txt`. This proves the gzip arm decompressed a head block, found `ustar` at offset 257 and chained into the native tar lister.

### 08.9 — Plain (non-tar) gzip shows its decompressed head as text
**Action:** SELECT(`f.txt.gz`), PREVIEW-SETTLE, `state` + capture-pane + `log 50`.
**Expect (socket):** `selection=="f.txt.gz"`; `preview_path` ends in `/work/f.txt.gz`; log contains NO `native gzip preview failed` line (the historic everything-gzip-is-a-tar mis-dispatch would surface as a tar error here).
**Expect (screen):** preview column shows the decompressed text `plain gz body` on its first content row; no tar-style mode/size columns, no error text.

### 08.10 — PEM certificate fields (native x509 parse)
**Action:** SELECT(`cert.crt`), PREVIEW-SETTLE, `state` + capture-pane + `log 50`.
**Expect (socket):** `selection=="cert.crt"`; `preview_path` ends in `/work/cert.crt`; log contains NO `native cert parse failed, trying openssl` line.
**Expect (screen):** preview column shows, in order, rows containing:
`Subject:    CN=test.example`, `Issuer:     CN=test.example` (self-signed),
`Not before: `, `Not after:  ` (each followed by a date),
`Serial:     ` (hex bytes), `Sig. alg.:  1.2.840.113549.1.1.11`
(the OID of sha256WithRSAEncryption — the native backend prints the raw OID),
then `SAN:        DNSName(test.example)`.
**Note:** label padding is significant (4 spaces after `Subject:`, etc.). `openssl` was needed only to create the fixture; the preview must come from the native parser (hence the log assertion).

### 08.11 — PDF native text tier (pdf_render default OFF)
**Action:** SELECT(`doc.pdf`), PREVIEW-SETTLE, `state` + capture-pane + `log 50`.
**Expect (socket):** `selection=="doc.pdf"`; `preview_path` ends in `/work/doc.pdf`; log contains NO `pdf text tier failed, falling back to stat` line and NO `rendering pdf page 1` line (render tier must not run with default config).
**Expect (screen):** preview column shows, in order: `PDF · 1 page` (singular — one page), `Title:    hello rfm` (from /Info; `Title:` + 4 spaces), a blank row, then the extracted page-1 text `hello from page one`. NO `Size:`/`MIME type:` rows (those would mean the text tier failed and the stat fallback rendered — that is a regression, not a pass).
**Note:** the fixture PDF must be byte-exact (base64 recipe): lopdf follows the xref table, and hand-typed offsets that don't match make the text tier fail down to the stat block. Also assert (filesystem): `$XDG_CACHE_HOME/rfm/thumbnails/` contains no `*-pdf-p1-960.jpg` entry.
**Benign WARN — do not trip on it:** previewing this PDF emits a lopdf `WARN Could not parse the encoding ... Using standard encoding as a fallback!` (Helvetica font). This is expected and harmless — a loose grep for `failed`/`warning` near the log must NOT treat it as a failure. The `pdf text tier failed` line is the only PDF failure signal that matters.

### 08.12 — Generic application/*: native stat block
**Action:** SELECT(`f.wasm`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="f.wasm"`; `preview_path` ends in `/work/f.wasm`; log contains no `stat block failed, trying mediainfo` line.
**Expect (screen):** preview column shows, in order: the absolute path `<FIXTURE>/work/f.wasm`, a blank row, `Size:        1 B`, `Modified:    ` followed by a `YYYY-MM-DD HH:MM:SS` timestamp, `MIME type:   application/wasm`, `Permissions: -rw-r--r--`.
**Note:** label padding: `Size:` + 8 spaces, `Modified:` + 4, `MIME type:` + 3, `Permissions:` + 1.

### 08.13 — Extensionless file with shebang (content sniff → text)
**Action:** SELECT(`script`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="script"`; `preview_path` ends in `/work/script`.
**Expect (screen):** preview column shows `#!/bin/sh` then `echo hi` — a text preview, NOT a stat block (`MIME type:` must not appear): the sniff resolved the shebang to text/plain.

### 08.14 — Extensionless printable file (UTF-8 heuristic sniff → text)
**Action:** SELECT(`readme`), PREVIEW-SETTLE, `state` + capture-pane.
**Expect (socket):** `selection=="readme"`; `preview_path` ends in `/work/readme`.
**Expect (screen):** preview column shows `just words` — text preview, no stat block.

### 08.15 — Broken symlink: Empty preview, loop stays responsive
**Action:** SELECT(`dangling`); then immediately `echo await-idle | socat - UNIX-CONNECT:$SOCK` (must return within a few seconds, `{"idle":true,...}`), then `state` + capture-pane.
**Expect (socket):** `selection=="dangling"`; `preview_path == "path-of-empty-panel"` (the literal placeholder for `PreviewPanel::Empty` — `is_file()`/`is_dir()` follow the link and both report false); `state` replies promptly (a 10 s timeout here = wedged event loop = bug, report it as a finding).
**Expect (screen):** center column still shows `dangling` on the highlighted row; the preview column is entirely blank — no error text, no stale content from the previously selected file (compare against 08.14's `just words`: it must be gone; stale right-pane content with a correct socket state is a render-path bug). Only assert the center and preview columns: the header (top row) shows `.../work` with **no** filename segment appended for a dangling link (the target is unresolvable) — do not assert `.../work/dangling` in the header.

### 08.16 — Forged native failure → shell-out fallback log line
**Action:** SELECT(`bad.zip`), PREVIEW-SETTLE, `state` + capture-pane + `echo "log 100" | socat - UNIX-CONNECT:$SOCK`.
**Expect (socket):** `selection=="bad.zip"`; the log history contains a DEBUG line starting `native zip list failed, trying unzip:` (observed full text: `native zip list failed, trying unzip: invalid Zip archive: Could not find EOCD`). This is THE observable for "preview unexpectedly came from a shell-out".
**Expect (screen):** with `unzip` installed: the preview column shows unzip's own output — a row containing `Archive:  <FIXTURE>/work/bad.zip` (unzip prints the header even for a bad archive; further unzip output/emptiness is tool-version-dependent, do not over-assert). Without `unzip`: the preview shows `Error: Could not run unzip` and `You must have unzip installed to get a preview for this file-type.`. Either way the pane is NOT blank-with-no-log.
**Note:** DEBUG lines are retained only because `--debug-socket` raises verbosity to TRACE; without the socket flag this assertion is impossible.

### 08.17 — Backend-choice audit: everything else was native
**Action:** `echo "log 200" | socat - UNIX-CONNECT:$SOCK`; filter messages containing `failed, trying` or `falling back`.
**Expect (socket):** the ONLY line matching the exact `failed, trying`/`falling back` fallback pattern is 08.16's `native zip list failed, trying unzip: ...` for `bad.zip`. Specifically absent: `native tar list failed`, `native gzip preview failed`, `native cert parse failed`, `pdf text tier failed`, `stat block failed`, `native audio metadata failed`. Any other hit means a preview asserted above silently came from a fallback — investigate that step's fixture/tooling before declaring the section passed.
**Two benign non-fallback lines that a loose grep for `failed`/`warning` can trip on — IGNORE both:** (1) lopdf's `WARN Could not parse the encoding ... Using standard encoding as a fallback!` from previewing `doc.pdf`, and (2) a `[bat warning]: Binary content ...` line (from the bat text preview of `garbage`-like content). Neither is a backend fallback; filter on the exact `failed, trying`/`falling back` substrings, not a bare `failed`/`warning`.
**Expect (screen):** n/a (log-only audit step); optionally capture-pane to confirm no error widget lines are visible.
**Note:** this audit only works because the section launched in `$FIXTURE/work` (quiet left panel). If unrelated `... failed, trying ...` lines from paths outside `$FIXTURE` appear, the retention was polluted by the environment — rerun the section with a clean fixture rather than waiving the assertion.

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$FIXTURE" "$CFG"; rm -f $SOCK` (plus the isolation tempdirs exported in the pane, if tracked).

**Section coverage gaps** (deliberate, for meta-review):
- Raster previews (image/PNG/JPEG/JXL/SVG/font sample), the raster cache
  (`img960u`/`vid120` entries, cache-hit log lines, `preview_cache=false`)
  and graphics-protocol behavior — covered by the graphics/raster section.
- Video (ffmpeg/mediainfo), audio (lofty) and zip-container documents
  (docx/xlsx/pptx/odt/epub), sqlite, 7z/zstd/xz/bzip2 arms — same native-first
  pattern, not exercised here.
- `pdf_render = true` (external pdftoppm/mutool image tier), encrypted PDFs
  (`encrypted PDF (N pages)`), and the PDF decompression-bomb guards
  (`PDF_SOURCE_MAX`, xref-stream pre-scan) — config-variant and adversarial
  PDF tests belong to a hardening section.
- FIFO / device-node selection (no-hang guarantee beyond the broken-symlink
  responsiveness check in 08.15).
- Directory previews (right pane as Miller column) — covered by navigation
  sections.
- Preview rate limiting (`rate_limit_interval_ms`) as a behavior under fast
  scrolling; here it is only waited out.
- bat present vs absent as an explicit A/B (content is identical by design;
  only styling differs).
- Filenames with spaces/`&`: previews never shell-interpolate the path on the
  native paths tested here; the shell-escape tests live in the
  command/archive sections.
