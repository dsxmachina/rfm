# Run 2 — Section 07: Opener and MIME handling

- **Binary:** `./target/debug/rfm` (rebuilt with fix)
- **Branch/commit:** `develop` @ `6f3218a` — fix `56aa473 fix(preview): resolve
  non-regular files to Empty instead of hanging` is present in history.
- **Terminal:** tmux 120×30, `image_protocol` resolved to `half-block` (inside tmux).
- **Tools present:** tmux, socat, mkfifo (coreutils). All fixture recipes ran clean.
- **Fixture:** quiet parent `$(mktemp -d)/fx` with the 12-entry mixed fixture
  incl. the FIFO `pipe`; scratch `--config` with a fake opener + pinned
  `[open.text|application|image]`.
- **Result:** 18/18 PASS, 0 FAIL, 0 SKIP. **The run-1 bug (07.17) is FIXED.**

## Per-step results

| Step | Result | Evidence |
|------|--------|----------|
| 07.1 Launch, FIFO doesn't wedge | PASS | await-idle idle; state <1s; mode=normal; cwd=$FIXTURE; total=12; entries center lists all 12 incl. `pipe`, exactly one selected, none hidden/marked. Header + footer + `1/12` on screen. |
| 07.2 `.txt` mime via mime_guess | PASS | selection=note.txt; seq stable 13/13 across two idle polls; footer `-rw-r--r-- ... text/plain`; preview shows `hello opener`. |
| 07.3 `.ts` special-case | PASS | footer `text/javascript` (NOT video/mp2t). |
| 07.4 `.zst` special-case | PASS | footer `application/zstd` despite plain-text content (no sniff). |
| 07.5 ext wins over content | PASS | lies.txt footer `text/plain` (NOT application/zip); PK magic never sniffed. |
| 07.6 shebang sniff → text | PASS | script footer `text/plain`; preview shows `echo hi`. |
| 07.7 printable sniff → text | PASS | readme footer `text/plain`; preview `just plain words`. |
| 07.8 binary garbage → text fallback | PASS | garbage footer `text/plain`; `log 30` had no ERROR lines; state responsive. |
| 07.9 PNG magic → image/png | PASS | pngmagic footer `image/png` (proves `infer` path). |
| 07.10 sniff cache mtime invalidation | PASS | after rewriting garbage with PNG magic, footer flipped to `image/png` (mtime 17:42:10); no nudge needed — watcher repaint sufficed. |
| 07.11 open `.txt` default fires | PASS | mode=normal, cwd unchanged, undo_depth=0; INFO `Opening '.../note.txt'` + `... with '.../fake-opener.sh'`; marker `TEXT-DEFAULT` + abs path; UI repainted. |
| 07.12 `.md` extension routing | PASS | INFO `checking extensions:` + Opening with fake-opener; marker `TEXT-MD` + notes.md (ext entry beat default); footer `text/markdown`. |
| 07.13 application mime routing | PASS | footer `application/zip`; Opening with fake-opener logged; marker `APP-DEFAULT` + ar.zip. |
| 07.14 spaces & `&` intact argv | PASS | marker exactly 2 lines: `TEXT-DEFAULT` + `.../a file & test.txt` — no word-splitting, no quoting artifacts, no ERROR. |
| 07.15 shebang drives open-rule | PASS | Opening script with fake-opener; marker `TEXT-DEFAULT` + script (routed via [open.text]). |
| 07.16 failing opener binary | PASS | INFO Opening with `/nonexistent/rfm-test-opener` then ERROR `Opening failed: No such file or directory (os error 2)`; mode=normal; after j/k selection back on pngmagic (raw mode restored); opened.log did NOT grow (97==97 bytes); UI repainted. |
| **07.17 FIFO selection: blank preview, no wedge** | **PASS (FIXED)** | See below. |
| 07.18 pressing `l` on FIFO no hang | PASS | await-idle returned in 11ms; mode=normal; Opening lines logged; marker `TEXT-DEFAULT` + pipe; no new ERROR (0==0); after `k` selection moved (notes.md), UI live. |

## 07.17 — previously-failing step, new result: **PASS (bug fixed)**

Run 1 filed BUG-2: selecting the FIFO left the right (preview) pane stuck on a
`Loading...` + path placeholder forever (the directory-panel `loading` flag was
never cleared for a non-regular file). The fix commit
`56aa473 fix(preview): resolve non-regular files to Empty instead of hanging`
is in the tested binary.

**No-wedge (was already OK in run 1):**
- `await-idle` → `{"idle":true,"seq":158}`
- `state` reply latency: 7 ms (well under the 12 s wrapper)
- selection=`pipe`, mode=normal, seq stable `158` across 3 consecutive polls.

**Preview pane now blank (the fixed symptom):**
- `state.preview_path` == `"path-of-empty-panel"` (the `PreviewPanel::Empty`
  sentinel) — was a real `.../pipe` path with a `Loading...` panel in run 1.
- Captured right column (rows 2–13) is entirely empty — only the `│` divider,
  no text:
  ```
  │ 🖹pipe                                  0 B │
  ...                                          │   (all blank to the right)
  ```
- `grep -i loading` over the full capture returns **nothing** — no `Loading...`
  placeholder.

**Footer correct:** `prw-r--r--   someone users 0 B 2026-07-31 17:40:56 text/plain   9/12`
— permissions start with `p` (FIFO), mime `text/plain` (sniff refuses
non-regular files → fallback).

**Reproduced twice:** re-selected away (note.txt) and back to `pipe`; on the
second selection `preview_path` was again `path-of-empty-panel` and the right
pane again fully blank with no `Loading...`. Deterministic, not flaky.

## Protocol feedback

- None. The updated harness (quiet nested parent, direct-binary launch,
  `timeout 12` socket wraps, `send-keys -l` note, `log 200` assertions) worked
  cleanly for this section. The SELECT-by-polling helper handled the
  space/`&`-containing filename fine (no quotes in the name, so the simple
  `grep -o '"selection":"..."'` parse was safe).
- Minor observation (not a defect): 07.10's watcher-driven repaint already
  refreshed the footer to `image/png` before any `zh`+`zh` nudge was needed, so
  the deterministic-fallback branch of the step went untested — expected per the
  step's own note.
- No `error.log` was written to the repo cwd during or after the run.
