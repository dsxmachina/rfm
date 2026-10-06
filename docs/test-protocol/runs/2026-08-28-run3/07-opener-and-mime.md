# Run 3 — Section 07: Opener and MIME handling

- **Date:** 2026-08-28
- **Branch:** feat/kitty-unicode-placeholders (binary prebuilt, not rebuilt)
- **Harness:** tmux session `tp3-07`, socket `/tmp/tp3-07.sock`, 120x30 pane,
  direct-launch with `env -u KITTY_WINDOW_ID -u GHOSTTY_RESOURCES_DIR` +
  scratch `--config`, `XDG_CACHE_HOME`/`XDG_STATE_HOME`/`_ZO_DATA_DIR` tempdirs.
  Quiet fixture parent (`PARENT=$(mktemp -d); FIXTURE=$PARENT/fx`).
- **Resolved `image_protocol`:** `half-block` (as expected inside tmux with the
  unset-var prefix).
- **Result: 18/18 steps PASS, 0 fail, 0 skip, no new bugs.**

Every step ran the four-step loop (input → await-idle → socket assert →
capture-pane assert); every socket read was wrapped in `timeout 12`.

---

## 07.1 — Launch: FIFO in the listing does not wedge startup — PASS
`await-idle` returned `{"idle":true,"seq":4}` immediately; `state` replied in
5 ms. `mode=="normal"`, `cwd==$FIXTURE`, `total==12`. `entries center` listed
all 12 names including `pipe`, exactly one `selected:true`
(`a file & test.txt`, first in sort order), none hidden/marked. Screen: header
`someone@work /…/fx/a file & test.txt`, 12 rows in center, footer
`-rw-r--r-- … text/plain`, index `1/12`.

## 07.2 — Footer mime: plain `.txt` via mime_guess — PASS
SELECT(note.txt): `selection=="note.txt"`, seq 13 (up from 4), stable across two
consecutive `state` queries. Footer: `-rw-r--r-- … 13 B … text/plain 7/12`.
Highlight: exactly one row carries `[0;7m` on `note.txt` (capture -e). Preview
column shows `hello opener`.

## 07.3 — `.ts` special-case beats mime_guess — PASS
Footer for `mod.ts`: `text/javascript` (not `video/mp2t`).

## 07.4 — `.zst` special-case — PASS
Footer for `data.zst`: `application/zstd` despite plain-text content.

## 07.5 — Extension wins over content — PASS
Footer for `lies.txt` (zip magic inside): `text/plain`, not `application/zip`.

## 07.6 — Sniff: extensionless shebang → text/plain — PASS
Footer for `script`: `text/plain`; preview shows `#!/bin/sh` / `echo hi`.

## 07.7 — Sniff: extensionless printable → text/plain — PASS
Footer for `readme`: `text/plain`; preview shows `just plain words`.

## 07.8 — Sniff: binary garbage → text/plain fallback, no crash — PASS
Footer for `garbage`: `text/plain`. `log 200` contained zero ERROR/WARN lines;
`state` responsive (seq 62).

## 07.9 — Sniff: extensionless PNG magic → image/png — PASS
Footer for `pngmagic`: `image/png` — proves the `infer` magic-number path.

## 07.10 — Sniff cache invalidates on mtime change — PASS
Rewrote `garbage` with PNG magic from the shell, re-selected: footer flipped to
`image/png` (seq 74 → 80) without needing the `zh`+`zh` repaint nudge (the
watcher reload sufficed).

## 07.11 — Open `.txt`: `[open.text].default` fires — PASS
After `l` on `note.txt`: mode normal, cwd unchanged, `undo_depth` 0 unchanged.
Log: INFO `Opening '$FIXTURE/note.txt'`, INFO `checking extensions: [("md", …)]`,
INFO `Opening '…/note.txt' with '$CFG/fake-opener.sh'`. Marker tail-2:
`TEXT-DEFAULT` + the absolute path. Full UI repainted, `note.txt` still
highlighted (one `[0;7m` row).

## 07.12 — Per-extension routing: `.md` hits `extensions` — PASS
Log shows `checking extensions:` + `Opening '…/notes.md' with …`. Marker
tail-2: `TEXT-MD` + `$FIXTURE/notes.md`. UI repainted, `notes.md` highlighted.

## 07.13 — Application mime → `[open.application]` — PASS
Log: `Opening '…/ar.zip' with '$CFG/fake-opener.sh'`. Marker tail-2:
`APP-DEFAULT` + path. Footer for `ar.zip`: `application/zip`.

## 07.14 — Shell-safety: spaces and `&` intact — PASS
Marker grew by exactly 2 lines: `TEXT-DEFAULT` then the EXACT string
`$FIXTURE/a file & test.txt` (verified `grep -Fx` full-line match — no
splitting, no quoting artifacts). No ERROR lines. UI repainted, selection
unchanged.

## 07.15 — Sniff drives open-rule choice: shebang `script` opens as text — PASS
Log: `Opening '…/script' with '$CFG/fake-opener.sh'`. Marker tail-2:
`TEXT-DEFAULT` + `$FIXTURE/script`.

## 07.16 — Failing opener binary: error surfaced, UI recovers — PASS
Log sequence: INFO `Opening '…/pngmagic'` → INFO
`Opening '…/pngmagic' with '/nonexistent/rfm-test-opener'` → **WARN**
`Opening failed: No such file or directory (os error 2)`. The protocol text
expects ERROR here; the binary emits WARN per the deliberate log-level policy
change (da35a98 "downgrade recoverable user-op failures from error! to warn!").
Load-bearing assertions all hold: failure surfaced in log AND in the collapsed
widget row (`warn: Opening failed: …` visible two rows above the footer),
mode normal, full UI repainted, `j`/`k` still move (selection returned to
`pngmagic`), `opened.log` did NOT grow (10 → 10 lines). Recorded as protocol
feedback, not a bug.

## 07.17 — FIFO selection: blank preview, no wedge — PASS
SELECT(pipe): `await-idle` idle, `state` replied in **4 ms**; seq 159 stable
across 3 polls (1 s apart). Footer: `prw-r--r-- … 0 B … text/plain 9/12` —
permissions start with `p`, sniff refused the non-regular file and fell back
to text/plain. Preview pane blank; `grep -ci loading` over the full pane = 0 —
the run-1 BUG-2 failure mode (stuck `Loading...` placeholder) is **fixed** on
this binary.

## 07.18 — Pressing `l` on the FIFO must not hang — PASS
`await-idle` returned in **6 ms** after `l`. Log: `Opening '$FIXTURE/pipe'` +
`Opening '$FIXTURE/pipe' with '$CFG/fake-opener.sh'`; no ERROR lines. Marker
tail-2: `TEXT-DEFAULT` + `$FIXTURE/pipe` (the fake opener never reads its
argument, so no block). `k` afterwards moved selection to `notes.md` — UI
live, raw mode intact.

---

## Protocol feedback

1. **07.16 expectation is stale: "Opening failed" is now WARN, not ERROR.**
   Commit da35a98 deliberately downgraded recoverable user-op failures to
   `warn!`. Update the step to expect WARN (and note the collapsed log widget
   still shows it, since the widget shows warn+). Grepping only for
   `level=="ERROR"` makes the step fail spuriously on current binaries.
2. **07.17's BUG-2 caveat can be retired.** The FIFO preview is now the
   intended blank panel (no `Loading...` placeholder); the "known bug"
   paragraph can be replaced by a plain "preview pane is blank" expectation.
3. Minor: when reading `opened.log` for the exact-path assertion (07.14),
   recommend `grep -Fx "$FIXTURE/…"` rather than eyeballing `cat -A` output —
   on systems where `cat` is aliased (bat), spaces render as `·` and look like
   corruption.

## Teardown
Session killed, socket removed, `$PARENT` `$CFG` `$CACHE` `$STATE` `$ZO`
tempdirs removed.
