# Run 1 — Section 08: Previews — text and native backends

**Environment:** rfm `./target/debug/rfm` (develop, HEAD cf90c81); tmux 3.6a, 120×30 pane
(temporarily 120×145 for 08.4); socat 1.8.1.3. All fixture-creation tools present
(`zip`, `tar`, `gzip`, `openssl`, `base64`, `unzip`, `bat`). `image_protocol` resolved
to `half-block` (running inside tmux, as expected). No tools missing.

**Result:** 17 / 17 steps PASS, 0 FAIL, 0 SKIP. No rfm bugs found.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 08.1 | PASS | mode=normal, view=single, total=17, selection=a.txt, idx=0, image_protocol=half-block, preview_path=/work/a.txt; all 17 entries listed, only a.txt selected; preview blank (0-byte a.txt), no error text |
| 08.2 | PASS | note.txt: preview rows `hello`, `world`; seq stable across settle |
| 08.3 | PASS | main.rs: preview `fn main() {}` |
| 08.4 | PASS | big.txt @145 rows: exactly 128 numbered rows (line 1 @row2, 128 @row129); `129` absent; blank below; `grep -cE '│ 1?[0-9]{1,3}\s*$'`==128 |
| 08.5 | PASS | empty.txt: preview entirely blank, no error; state responsive |
| 08.6 | PASS | ar.zip: two rows `0 B  a.txt` / `0 B  b.txt` (8-col right-aligned size, 2 spaces); no `native zip list failed` for ar.zip |
| 08.7 | PASS | ar.tar: `-rw-r--r--      0 B  a.txt`; no tar-fail log |
| 08.8 | PASS | ar.tar.gz: identical `-rw-r--r--      0 B  a.txt` (gzip arm sniffed ustar → tar lister); no gzip-fail log |
| 08.9 | PASS | f.txt.gz: `plain gz body` text, no tar columns, no error, no gzip-fail log |
| 08.10 | PASS | cert.crt: Subject/Issuer CN=test.example, Not before/after dates, Serial hex, Sig. alg. 1.2.840.113549.1.1.11, SAN DNSName(test.example); padding correct; no cert-parse-fail log |
| 08.11 | PASS | doc.pdf: `PDF · 1 page`, `Title:    hello rfm`, blank, `hello from page one`; no Size:/MIME rows; no pdf-fail/render log; no pdf-p1-960 thumbnail in cache |
| 08.12 | PASS | f.wasm: abs path, blank, `Size:        1 B`, `Modified:` + ts, `MIME type:   application/wasm`, `Permissions: -rw-r--r--`; no stat-fail log |
| 08.13 | PASS | script: `#!/bin/sh` / `echo hi`, no `MIME type:` (shebang sniff → text) |
| 08.14 | PASS | readme: `just words`, no stat block |
| 08.15 | PASS | dangling: preview_path=="path-of-empty-panel", await-idle prompt (not wedged), preview blank, no stale `just words`, no error |
| 08.16 | PASS | bad.zip: screen shows `Archive:  <FIXTURE>/work/bad.zip` (unzip fallback); log DEBUG `native zip list failed, trying unzip: invalid Zip archive: Could not find EOCD` present (fresh session — see feedback) |
| 08.17 | PASS | Backend audit: the ONLY `failed, trying`/`falling back` line is 08.16's bad.zip unzip fallback; no tar/gzip/cert/pdf/stat/audio fallbacks; no error widgets on screen |

## Bug reports

None. All steps passed against both socket and screen.

## Protocol feedback

- **SELECT direction ambiguity — log-retention flooding hazard.** The `SELECT(name)` helper
  in the section header says "send `j` or `k` one keypress at a time until
  `state.selection == name`", but naively always pressing `j` overshoots and wraps around
  the 17-entry list, costing many keypresses. Each keypress emits ~3 TRACE lines
  (`key-event`, `Command: Move`, `move-up/down`); ~66 keypresses fills the entire 200-line
  log retention and **evicts every preview-backend DEBUG line**. On my first pass at 08.16
  the flooding had already evicted `native zip list failed` and the whole 200-line history
  was nothing but move-TRACE lines — the log assertion appeared to fail even though the
  fallback demonstrably happened (screen showed unzip output). Re-running on a **fresh
  session with minimal (5) keystrokes to bad.zip** reproduced the DEBUG line exactly as
  documented. **Recommendation:** the section should warn that log-history assertions
  (08.6, 08.9, 08.10, 08.11, 08.16, 08.17) are only reliable when navigation to the target
  used few keystrokes; or the SELECT helper should pick the shortest direction (j vs k by
  index) to minimise churn; or the audit steps should note that a fresh, minimally-navigated
  session may be needed to keep backend lines within retention.
- **08.17 (session-wide audit) is unreliable on a long-lived session** for the same reason:
  by the time all 16 prior steps' navigation has run, the 200-line log window holds only the
  most-recent move-TRACE lines and no early backend decisions survive. I performed the audit
  on a fresh session that visited each native-backend file once with minimal keystrokes; the
  result was clean (only the bad.zip fallback). The section could recommend this explicitly.
- **08.15 header wording:** for the broken symlink, the header row shows `.../work` (no file
  segment appended) rather than `.../work/dangling`, because the link target is unresolvable.
  The step only asserts the center/preview columns, so this is fine, but a note that the
  header may omit the filename for a dangling link would prevent a mis-assertion.
- **Benign lopdf WARN:** previewing doc.pdf emits `WARN Could not parse the encoding ...
  Using standard encoding as a fallback!` (from lopdf, Helvetica font). This is not an error
  and does not affect any assertion, but 08.11/08.17 auditors grepping loosely for `failed`
  could trip on nearby text — worth a one-line note that this WARN is expected.
- Fixture recipe, entry order (17 entries), and every expected preview string matched the
  live build exactly. No fixture corrections needed.
