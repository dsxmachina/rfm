# Run 3 — Section 08: Previews, text and native backends

- Date: 2026-08-28, branch `feat/kitty-unicode-placeholders`, binary `./target/debug/rfm` (prebuilt)
- Session `tp3-08`, socket `/tmp/tp3-08.sock`, 120×30 pane
- Fixture: `PARENT=$(mktemp -d)`, launch dir `$PARENT/fx/work` (17 entries, per section recipe),
  scratch `--config`, `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs, `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR`
- Tools: tmux, socat, zip, tar, gzip, openssl, base64, **unzip present**, **bat present**, jq, python3
- Every socket read wrapped in `timeout 12`; no timeout ever fired — the event loop was never wedged.
- SELECT implemented per README helper (exact shortest j/k path, one await-idle per keypress);
  PREVIEW-SETTLE = sleep 0.7 → await-idle → poll `state` at 0.5 s until `seq` stable.

**Tally: 17 steps — 17 pass, 0 fail, 0 skip. No rfm bugs filed.**

| Step | Result | Notes |
|------|--------|-------|
| 08.1 | PASS | |
| 08.2 | PASS | |
| 08.3 | PASS | |
| 08.4 | PASS | exactly 128 preview rows, no `129` |
| 08.5 | PASS | |
| 08.6 | PASS | |
| 08.7 | PASS | |
| 08.8 | PASS | |
| 08.9 | PASS | |
| 08.10 | PASS | |
| 08.11 | PASS | |
| 08.12 | PASS | |
| 08.13 | PASS | |
| 08.14 | PASS | |
| 08.15 | PASS | |
| 08.16 | PASS | with harness caveat — see narrative; observable verified twice |
| 08.17 | PASS | 1 fallback hit (bad.zip), corroborated by fresh-session audit |

## Per-step narrative

### 08.1 — Launch baseline — PASS
Socket: `mode="normal"`, `view="single"`, `total=17`, `selection="a.txt"`,
`selected_idx=0`, `image_protocol="half-block"`, `preview_path=…/fx/work/a.txt`,
`seq=4` after `await-idle`. `entries center` listed exactly the 17 expected names,
only `a.txt` `selected:true`. Screen: header `someone@work …/fx/work/a.txt`; 17 rows in
center; preview column blank (a.txt is 0 B); no `Failed to open`/`Error:` anywhere.
SGR capture (`-e`) shows the `[0;7m` highlight on the `a.txt` row.

### 08.2 — Plain text — PASS
SELECT(note.txt) = j×14; settle at seq=19. `preview_path` ends `/work/note.txt`.
Preview rows 2–3: `hello`, `world`; nothing below.

### 08.3 — Source file — PASS
SELECT(main.rs) = k×1. Preview row 2: `fn main() {}`.

### 08.4 — 128-line cap — PASS
`resize-window -x 120 -y 145` (rfm repainted), SELECT(big.txt), settle.
`grep -cE '│ 1?[0-9]{1,3}\s*$'` on the capture = **128**; `│ 129\s*$` = 0 hits;
line `1` at screen row 2, `128` at row 129, rows below blank. Resized back to 30 rows.

### 08.5 — Empty file — PASS
SELECT(empty.txt). `preview_path` ends `/work/empty.txt`; follow-up `state` replied
instantly (seq=33). Preview column: 0 non-blank rows; no error strings.

### 08.6 — zip listing (native) — PASS
SELECT(ar.zip). Preview: exactly two member rows `     0 B  a.txt` / `     0 B  b.txt`
(size right-aligned in 8 cols, two spaces, name — the native format, not unzip's
`Archive:`-header format). `log 50`: no `native zip list failed` mentioning ar.zip.

### 08.7 — tar listing (native) — PASS
SELECT(ar.tar). One member row `-rw-r--r--      0 B  a.txt`; no `native tar list failed`.

### 08.8 — tar.gz (gzip arm sniffs ustar) — PASS
SELECT(ar.tar.gz). Identical row `-rw-r--r--      0 B  a.txt`; no
`native gzip preview failed`.

### 08.9 — Plain gzip head as text — PASS
SELECT(f.txt.gz). Preview row 2: `plain gz body`; no mode/size columns, no error,
no `native gzip preview failed` in log.

### 08.10 — PEM certificate (native x509) — PASS
SELECT(cert.crt). Rows in order:
`Subject:    CN=test.example`, `Issuer:     CN=test.example`,
`Not before: Aug 28 17:12:37 2026 +00:00`, `Not after:  Aug 29 17:12:37 2026 +00:00`,
`Serial:     5d:cc:…`, `Sig. alg.:  1.2.840.113549.1.1.11`,
`SAN:        DNSName(test.example)`. Label padding exact. No
`native cert parse failed, trying openssl` in log.

### 08.11 — PDF native text tier — PASS
SELECT(doc.pdf). Rows: `PDF · 1 page`, `Title:    hello rfm`, blank,
`hello from page one`. Zero `Size:`/`MIME type:` rows (no stat fallback).
Log: 0 × `pdf text tier failed`, 0 × `rendering pdf page 1`.
Filesystem: `$XDG_CACHE_HOME/rfm/thumbnails` contains no `*-pdf-p1-960.jpg` (glob empty).

### 08.12 — Generic application/* stat block — PASS
SELECT(f.wasm). Rows: `<abs path>/f.wasm`, blank, `Size:        1 B`,
`Modified:    2026-08-28 17:12:37`, `MIME type:   application/wasm`,
`Permissions: -rw-r--r--`. Padding exact. No `stat block failed` in log.

### 08.13 — Shebang sniff — PASS
SELECT(script). Preview `#!/bin/sh` / `echo hi`; 0 × `MIME type:` on screen.

### 08.14 — Printable sniff — PASS
SELECT(readme). Preview `just words`; no stat block.

### 08.15 — Broken symlink — PASS
SELECT(dangling); immediate `await-idle` returned `{"idle":true,"seq":73}` within
~1 s; `state` prompt. `preview_path == "path-of-empty-panel"` (sentinel). Header row is
`…/fx/work` with no filename segment (as specified). Preview column: 0 non-blank rows;
`just words` from 08.14 gone (no stale render). `dangling` row carries the `[0;7m`
highlight.

### 08.16 — Forged native failure → unzip fallback — PASS (with harness caveat)
SELECT(bad.zip), settle. **Screen** matched immediately: preview row
`Archive:  …/fx/work/bad.zip` (unzip installed). **Socket log did NOT initially
contain the line** `native zip list failed, trying unzip:` — the entire 200-line
ring held only key-event/Command/move TRACE triples (69 minimal-path keypresses
× 3 lines ≈ 207 > 200 capacity). Investigation (evidence below) showed this is a
protocol-flow artifact, not an rfm bug:

1. **Startup preloader.** A fresh session on the same fixture, queried ~2 s after
   launch with zero keystrokes, logs (14 lines total):
   `panel-update: left/center/preview …` and
   `DEBUG native zip list failed, trying unzip: invalid Zip archive: Could not find EOCD`.
   The preview backends run during the directory preload at launch, so the DEBUG
   line is emitted then — ~70 protocol keypresses later it has been
   capacity-evicted, and re-selecting bad.zip serves the cached preview without
   re-running the backend (no new log line, correct screen).
2. **Forced genuine load in the main session** (rewrote bad.zip → new mtime, then
   deselect/reselect): the line appeared exactly as specified:
   `DEBUG 1.28 native zip list failed, trying unzip: invalid Zip archive: Could not find EOCD`
   followed by `TRACE panel-update: preview <- …/bad.zip`, screen unchanged
   (`Archive: …`).

Observable verified twice (fresh-session preload + forced reload). rfm behavior
matches the spec ("THE observable for a shell-out preview"); only the protocol's
assumption of *when* the line is emitted is off. Filed as protocol feedback.

### 08.17 — Backend-choice audit — PASS
`log 200` on the main session: exactly **1** line matching
`failed, trying`/`falling back` — the bad.zip zip→unzip line. Explicitly absent
(0 hits each): `native tar list failed`, `native gzip preview failed`,
`native cert parse failed`, `pdf text tier failed`, `stat block failed`,
`native audio metadata failed`. Because eviction could have hidden startup-preload
fallbacks, the audit was **corroborated with a fresh-session log** taken right
after launch (before any eviction): 14 lines total, again exactly one
`failed, trying` hit (bad.zip). No error widget lines on screen. The benign lopdf
WARN did not appear in either ring (nothing to waive).

## Incidental observation (out of section scope — for the watcher sections)

Content-only file modification never wakes rfm: with the session idle,
`printf … > bad.zip` (truncate+rewrite) and later `printf … >> a.txt` (append)
both left `seq` unchanged (79 → 79 over 2.5 s) and the center listing kept showing
`a.txt 0 B` while the file was 15 B on disk. Reproduced twice. Create/Remove/Rename
are handled (cf. commit "reload listing panes on rename (Modify(Name))"); pure
`Modify(Data)` appears to be deliberately ignored (churn avoidance), at the cost of
stale sizes in the listing. Not filed as a section-08 bug (no protocol step covers
it); sections 10/12 should decide whether this is intended and add an explicit step.

## Protocol feedback

1. **08.16's log assertion races the startup preview preloader + ring eviction.**
   The `native zip list failed, trying unzip:` DEBUG line is emitted when the
   preview *loads* — which happens during the launch-time directory preload, not
   on later selection (selection serves the in-memory cache; no re-run, no new
   line). By step 16 the protocol's own minimal keypress budget (~69 presses × 3
   TRACE lines) exceeds the 200-line ring, so the startup line is always evicted.
   Fix options: (a) capture `log 200` immediately after launch in 08.1 and let
   08.16/08.17 assert against that snapshot; or (b) have 08.16 force a genuine
   load (touch bad.zip to bump mtime, deselect/reselect) before reading the log.
2. **08.6–08.11's "log contains NO `… failed` line" assertions are near-vacuous**
   for the same reason (the relevant load happened at startup; `log 50` post-step
   sees only keypress TRACE lines). The screen formats are native-distinctive
   (zip `{size:>8}  {name}` vs unzip's `Archive:` header, etc.), so the passes
   stand on screen evidence — the protocol could say so explicitly, or move the
   negative log assertions to a startup snapshot per item 1.
3. **08.17 should mandate the fresh-launch snapshot as the audit source** (or an
   explicit keypress budget), otherwise a fallback that fired during preload is
   invisible by audit time and the audit can pass falsely. This run corroborated
   with a second launch on the same fixture (zero keystrokes, ring size 14).
4. `grep -cE '│ 1?[0-9]{1,3}\s*$'` in 08.4 also matches 4-digit `1xxx` numbers in
   principle (`1?` + `{1,3}`); harmless here (cap is 128) but `'│ (12[0-8]|1[01][0-9]|[1-9][0-9]?)\s*$'`
   would be exact.
5. Section teardown line omits `$PARENT` (this run nested `fx/work` under a fresh
   parent per run-3 rules; the section's own recipe uses a bare `mktemp -d`
   fixture whose parent is `/tmp` — consider aligning the section recipe with the
   README's quiet-parent pattern; the `work/` nesting already achieves it, noted).

## Environment notes
- `unzip` and `bat` both installed (08.16 asserted the unzip-present branch).
- tmux resize-window available (08.4 in-place resize path used).
- No `state`/`await-idle` timeout occurred at any point (wedge detector clean).
