# Run 2 — Section 05: Console, search and input modes

- **Binary:** `./target/debug/rfm` (rebuilt 2026-07-31 19:39, WITH the fix)
- **Session/socket:** `rfm-sec05` / `/tmp/rfm-sec05.sock`
- **Environment:** tmux 120x30, socat present, zoxide INSTALLED, image_protocol resolved to `half-block` (inside tmux, expected)
- **Fixture:** quiet parent `mktemp -d`, fixture nested at `$PARENT/fx` with `alpha dir/`, `beta dir/`, `report.txt`, `report-2.txt`, `notes.md`, `misc.log`

## Headline result

The run-1 bug at **05.2** (live-search highlight corrupted the visible
filename, e.g. `report-2.txt` -> `rreport2.txt`) is **FIXED** — see the
detailed evidence below. It now renders the filenames correctly.

## Per-step result table

| Step | Description | Socket | Screen | Result |
|------|-------------|--------|--------|--------|
| 05.1 | Launch baseline | mode normal, total 6, sel `alpha dir`, idx 0, marked [], clipboard null; entries set matches, only `alpha dir` selected | 6 rows, `alpha dir` highlighted, header path, no overlay/prompt | **PASS** |
| 05.2 | Enter search; live highlight | after `/`: mode search; after `report`: mode search, marked [], total 6, sel `alpha dir`, idx 0 unchanged; entries all marked:false | only `report-2.txt`/`report.txt` rows; `report` bold in red; footer `Search report`; **filenames NOT corrupted** | **PASS** (was FAIL in run 1) |
| 05.3 | Live search no match | mode search, marked [], total 6, sel/idx unchanged (both empty-pattern and `zzz`) | empty pattern shows all 6 rows no bold; `zzz` shows single italic `(no match)` row in red; footer `Search zzz` | **PASS** |
| 05.4 | Esc cancels search | mode normal, marked [], total 6, sel `alpha dir`, idx 0; no marked:true | 6 rows, no bold, footer restored | **PASS** |
| 05.5 | Enter commits search | mode normal, total 6, marked = both `report*` absolute paths, sel `report-2.txt`, idx 4; entries marked:true on exactly those two | both report rows marked (`x`, color 3 dark-yellow); cursor reverse-video on `report-2.txt`; no footer prompt | **PASS** |
| 05.6 | n/N cycle marked | n→`report.txt` idx 5; n→wraps `report-2.txt` idx 4; N→`report.txt` idx 5; mode normal throughout, 2 marked | cursor moves between report rows; both stay marked-colored | **PASS** |
| 05.7 | Enter cd console | mode console, cwd `$FIXTURE` unchanged | centered overlay, divider bars, middle line `$FIXTURE/beta dir` (prefix + recommendation); Miller columns behind; no footer prompt | **PASS** |
| 05.8 | Esc cancels cd console | mode normal, cwd `$FIXTURE` | overlay gone, `$FIXTURE` listing restored | **PASS** |
| 05.9 | cd descends live + Enter | while typing `alpha dir`: mode console, cwd moved to `$FIXTURE/alpha dir`; overlay path `.../alpha dir/`; after Enter: mode normal, cwd `$FIXTURE/alpha dir`, total 0 | overlay path `.../alpha dir/` while typing; after Enter overlay gone, center empty, header ends `/alpha dir` | **PASS** |
| 05.10 | cd Backspace → parent | in console overlay path `.../alpha dir/`; after BSpace: mode console, cwd `$FIXTURE`; after Enter: mode normal, cwd `$FIXTURE`, total 6 | overlay prefix `$FIXTURE/` after BSpace; after Enter overlay gone, `$FIXTURE` listing | **PASS** |
| 05.11 | Enter zoxide console | mode console, cwd `$FIXTURE` | centered overlay with divider bars + input line; zoxide installed → NO "not installed" hint (correct) | **PASS** |
| 05.12 | Esc cancels zoxide console | mode normal, cwd `$FIXTURE`, total 6 | overlay gone, `$FIXTURE` listing restored | **PASS** |

**Tally:** 12 steps, 12 PASS, 0 FAIL, 0 SKIP.

## Previously-failing step (05.2) — new result with evidence

**Result: PASS.** The filename corruption is gone.

Socket after typing `report` (live search, before Enter):
```
mode search, total 6, selection "alpha dir", selected_idx 0, marked []
entries center: all 6 names present, every row marked:false
```
(render-only filter — underlying state unchanged, exactly as documented.)

SGR-inclusive pane capture of the two matching center rows (the load-bearing
evidence — filenames render intact):
```
🖹<b><red>report</red></b><normal>-2.txt</normal>   0 B
🖹<b><red>report</red></b><normal>.txt</normal>     0 B
```
Raw SGR:
```
🖹[1m[38;5;9mreport[0m[38;5;7m-2.txt                          0 B
🖹[1m[38;5;9mreport[0m[38;5;7m.txt                            0 B
```
The matched substring `report` is bold in color 9 (highlight/red), the
remainder (`-2.txt` / `.txt`) follows in the normal color with no duplicated
leading character and no dropped hyphen. In run 1 this row rendered as the
corrupted `rreport2.txt`; that no longer occurs. Footer prompt renders
correctly as `Search report` (`Search` bold reversed green, `report` red).

## Protocol feedback

1. **Backspace key name.** Step 05.3's action text says
   "`tmux send-keys -t $SESSION Backspace`". tmux's key name for Backspace is
   **`BSpace`**, not `Backspace`. Sending the literal token `Backspace` types
   the nine-character word into the search input (observed: footer showed
   `Search reportBackspaceBackspace...` and the center pane went to
   `(no match)`), it does not delete a character. The README's own
   "colliding-name" caveat lists `Backspace` among keys "sent WITHOUT `-l`",
   but the correct tmux spelling is `BSpace`. Recommend updating 05.3 (and any
   other Backspace-using step, e.g. 05.10 which correctly needs the key) to use
   `BSpace`. Once `BSpace` was used, the step behaved exactly as specified.

2. Everything else in the section matched the binary precisely — no wrong keys,
   no ambiguous expectations. The uppercase `CD` zoxide binding was delivered
   fine via `send-keys -l CD` in this tmux, so 05.11/05.12 did not need to be
   skipped.
