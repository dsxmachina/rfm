# Section 07 — Opener and MIME handling — run1

**Environment:** tmux 120x30, graphics protocol resolved to `half-block` (inside
tmux, as expected). Terminal `someone@work`. All required tools present (tmux,
socat, python3). No `/usr/bin/time` and no `bc` on this box — timing was done
with `date +%s` and by polling `seq` stability instead (no impact on assertions).
Binary: `./target/debug/rfm` (pre-built). Fixture/config per section header.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 07.1  | PASS | idle prompt, state <1s, mode normal, cwd correct, total=12, 12 entries incl `pipe`, 1 selected, none hidden/marked; footer + `1/12` visible |
| 07.2  | PASS | `note.txt` selected, seq 118 stable; footer `text/plain`, perms `-rw-r--r--`, preview `hello opener` |
| 07.3  | PASS | `mod.ts` footer `text/javascript` (not video/mp2t) |
| 07.4  | PASS | `data.zst` footer `application/zstd` (no sniff) |
| 07.5  | PASS | `lies.txt` footer `text/plain` (not application/zip) |
| 07.6  | PASS | `script` footer `text/plain`; preview shows `#!/bin/sh` + `echo hi` |
| 07.7  | PASS | `readme` footer `text/plain`; preview `just plain words` |
| 07.8  | PASS | `garbage` footer `text/plain`; no ERROR lines; state responsive; preview shows a `[bat warning]` note (not an error panel) |
| 07.9  | PASS | `pngmagic` footer `image/png` (infer magic path) |
| 07.10 | PASS | after rewriting `garbage` with PNG magic + re-select, footer flips to `image/png` (mtime cache invalidation; no nudge needed) |
| 07.11 | PASS | `l` on `note.txt`: mode normal, cwd/undo unchanged, both INFO Opening lines, marker `TEXT-DEFAULT` + path, UI repainted |
| 07.12 | PASS | `.md` routes via extensions entry: `checking extensions:` logged, marker `TEXT-MD` + path |
| 07.13 | PASS | `ar.zip` footer `application/zip`; opens via `[open.application]`, marker `APP-DEFAULT` + path |
| 07.14 | PASS | `a file & test.txt`: exactly 2 marker lines, line 2 exact path with spaces + `&` intact, no word-splitting |
| 07.15 | PASS | `script` shebang → `[open.text]`, marker `TEXT-DEFAULT` + path |
| 07.16 | PASS | image → `/nonexistent` binary: ERROR `Opening failed: No such file or directory`, mode normal, UI repainted, j/k work, opened.log did not grow |
| 07.17 | FAIL | no wedge / state responsive / footer `prw-r--r--` + `text/plain` all correct, BUT preview pane shows a stuck `Loading...` placeholder instead of a blank Empty preview |
| 07.18 | PASS | `l` on FIFO: await-idle ~0s (no hang), Opening lines, marker `TEXT-DEFAULT` + path, no ERROR, k moves selection afterward |

**Tally:** 17 PASS, 1 FAIL, 0 SKIP (of 18 steps).

---

## BUG-run1-01 — FIFO preview pane stuck on "Loading..." instead of blank Empty

- **Protocol step:** 07.17
- **Severity:** visual
- **Class:** render (screen shows a stale/stuck placeholder; the underlying
  directory-panel `loading` flag is never cleared for a FIFO, so state and
  screen agree on the wrong thing — the placeholder just never resolves)

### Repro (minimal)
Fresh harness per section header (fixture includes `mkfifo "$FIXTURE/pipe"`).
```
SELECT(pipe)              # gg then j until state.selection == "pipe"
echo await-idle | timeout 12 socat - UNIX-CONNECT:$SOCK
echo state      | timeout 12 socat - UNIX-CONNECT:$SOCK     # replies fast, seq stable
tmux capture-pane -t $SESSION -p                            # right pane still "Loading..."
```
Poll `seq` several times with `await-idle` between — it is fully stable
(e.g. 871→871→871→871→871), so this is not a mid-load transient.

### Expected
Right (preview) pane blank for the FIFO (`PreviewPanel::Empty` — a FIFO is
neither dir nor regular file). Footer perms start with `p` and mime shows
`text/plain`. (Load-bearing: `state` replies well under 10s, no wedge.)

### Actual
- socket `state`: `selection=="pipe"`, `mode=="normal"`,
  `preview_path=="/tmp/.../pipe"`, `seq` stable across ≥5 polls; state replies
  in <1s — **no wedge**, liveness intact.
- `log 30`: only `TRACE panel-update: left <- /tmp` repeats; NO panel-update
  ever arrives for the right/preview pane for the FIFO.
- captured pane (preview column):
  ```
  │ ... 18 B │ Loading...
  │ ... 16 B │ /tmp/tmp.iOQVqI3AEG/pipe
  │ ... 6 B  │
  ```
  Footer: `prw-r--r--   someone users 0 B 2026-07-31 17:03:03 text/plain   9/12`
  (footer perms + mime are exactly as expected).

Root cause (from source, `src/panel/directory.rs:731`): the preview column is
driven as a directory panel whose `loading` flag is set and never cleared for a
FIFO — the async directory load for a non-dir/non-regular target never
completes, so the `Loading...` + path placeholder persists indefinitely instead
of falling through to `PreviewPanel::Empty`.

### Notes
Reproduced twice (re-selected via `note.txt` → `pipe`; identical stuck
placeholder, seq stable at 871). The step's critical no-wedge / responsiveness
/ footer assertions ALL pass; only the "blank Empty preview" screen expectation
fails. Low severity (cosmetic — no hang, no data risk). Same FIFO in the listing
(07.1) and `l`-on-FIFO (07.18) are both fine — this is purely the preview
render of a selected FIFO.

---

## Protocol feedback

- Step 07.17 relies on `/usr/bin/time` implicitly via "time `echo state`". On
  this box `/usr/bin/time` and `bc` are absent; suggest the protocol specify a
  portable timing recipe (e.g. `start=$(date +%s%N); ...; end=$(date +%s%N)`)
  or just rely on the 12s `timeout` wrapper as the wedge detector (a reply at
  all within the timeout already proves no-wedge).
- 07.17 expects a **blank** FIFO preview (`PreviewPanel::Empty`). The real
  binary shows a persistent `Loading...` placeholder for a selected FIFO. Either
  the code should resolve the FIFO preview to Empty, or the protocol should be
  updated to expect the current (harmless) stuck-placeholder behavior. As
  written it reads as a strict expectation, so it is filed as a bug; flag for
  triage on whether it is intended.
- 07.8's preview for `garbage` shows a `[bat warning]: Binary content...`
  line. The step's Note already permits "near-empty text" / "no error panel";
  the bat-warning line is not an error panel, so treated as PASS. Worth noting
  explicitly in the step that a bat binary-content warning is acceptable.
- SELECT helper worked reliably; the mixed fixture's sort order (dirs none →
  files alpha) matched, and `gg`-then-`j` scanning is deterministic. No issues.
