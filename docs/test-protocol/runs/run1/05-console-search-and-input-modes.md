# Run 1 — Section 05: Console, search and input modes

**Environment:** terminal 120x30, tmux + socat present; `zoxide` IS installed
(so 05.11 exercises the real zoxide console, not the missing-binary hint path).
Fixtures under `/tmp` (mktemp) — `left` pane shows the many `tmp.*` siblings and
the log stream is full of routine `Updating: /tmp` lines (harness noise, not
errors). Binary `./target/debug/rfm`, branch develop @ cf90c81.

## Per-step results

| Step | Result | Note |
|------|--------|------|
| 05.1 | PASS | mode normal, total 6, sel `alpha dir` idx0, marked [], clipboard null; 6 entries, alpha dir highlighted, header path, no overlay/prompt. |
| 05.2 | **FAIL** | Socket correct (mode search, total 6, marked [], sel/idx unchanged, all 6 in `entries`); rows correctly filtered to the two `report*`; but the live-highlight overlay is drawn **1 cell too far right**, so the visible name is corrupted: `report-2.txt`→`rreport2.txt`, `report.txt`→`rreporttxt`. Render-class bug. Footer prompt `Search report` correct. |
| 05.3 | PASS | empty pattern re-shows all 6 (names render correctly, no overlay); `zzz` → single ` (no match)` row, footer `Search zzz`; socket mode search, total 6, marked [], unchanged. |
| 05.4 | PASS | Esc → mode normal, all 6 entries, no marks, names correct, prompt gone. |
| 05.5 | PASS | Enter → mode normal, total 6, marked = both report abs paths, sel `report-2.txt` idx4; both rows `x`-marked, cursor on report-2.txt, footer 5/6. |
| 05.6 | PASS | n→`report.txt` idx5; n→wraps `report-2.txt` idx4; N→`report.txt` idx5; mode normal, 2 marked throughout. |
| 05.7 | PASS | `cd` → mode console, cwd unchanged; centered overlay with divider bars, middle line shows `$FIXTURE/` + a dark-grey recommendation suffix (documented). |
| 05.8 | PASS | Esc → mode normal, cwd unchanged, overlay gone, listing restored. |
| 05.9 | PASS | typing `alpha dir` → cwd moves live to `$FIXTURE/alpha dir` (overlay path `.../alpha dir/`); Enter → mode normal, cwd `$FIXTURE/alpha dir`, total 0, header ends `/alpha dir`, `(empty)`. |
| 05.10 | PASS | Backspace-at-empty → cwd walks to parent `$FIXTURE`, overlay path line updates to `$FIXTURE/` (+ dark-grey `alpha dir` recommendation); Enter → mode normal, cwd `$FIXTURE`, total 6. |
| 05.11 | PASS | `CD` (uppercase delivered fine) → mode console, cwd unchanged, overlay + divider bars present. zoxide installed → no "not installed" hint (that assertion is N/A, per the step's own gate). |
| 05.12 | PASS | Esc → mode normal, cwd `$FIXTURE`, overlay gone, listing restored. |

Tally: 11 PASS, 1 FAIL, 0 SKIP (12 steps).

---

## BUG-run1-01 — Search live-highlight overlay misaligned, corrupts visible filename

- **Protocol step:** 05.2
- **Severity:** visual
- **Class:** render (socket/`entries` correct, screen corrupted)

### Repro (minimal)
Fresh harness (fixture per section header), then:
```
tmux send-keys -t $SESSION /
echo await-idle | socat - UNIX-CONNECT:$SOCK
tmux send-keys -t $SESSION -l report
echo await-idle | socat - UNIX-CONNECT:$SOCK
tmux capture-pane -t $SESSION -p          # rows 2-3 = the two matches
```
Reproduced twice (typed `report` twice, once after backspacing to empty).

### Expected
Center pane shows only the two rows `report-2.txt` and `report.txt`, each with
the substring `report` **bold in red** and the rest of the name intact.

### Actual
- socket at failure: `mode=search total=6 selection="alpha dir" selected_idx=0
  marked=[]`; `entries center` lists all 6 names, every row `marked:false` — all
  correct (filter is render-only, as documented).
- captured pane (the two match rows):
  ```
  │ 🖹rreport2.txt                          0 B │
  │ 🖹rreporttxt                            0 B │
  ```
  The visible names are corrupted: `report-2.txt`→`rreport2.txt`,
  `report.txt`→`rreporttxt`.
- `log 50`: no errors (only routine `Updating: /tmp` panel churn).

### Root cause (from source)
`directory.rs:457-482` draws the full styled name first, then overlays the bold
red pattern at `pattern_x = x_range.start + 4 + offset` — hard-coding the name's
start column at `bar(1) + lead(1) + symbol_width(2) = 4`. The file icon
(`🖹`) is assumed 2 cells wide (`unicode_width`, `print_styled` uses the same
2), but tmux renders it as **1 cell**, so the real name starts at column 3, not
4. The overlay therefore lands one column too far right: it leaves the name's
first char (`r`) visible, then writes bold `report` over chars 2..8
(overwriting the `-`/`.` separator), yielding `r`+`report`+`2.txt` =
`rreport2.txt`. Verified via `capture-pane -e` cell colors: leading `r` is
color 7 (normal), `report` is `\033[1m\033[38;5;9m` (bold red), trailing
`2.txt` is color 7 — exactly a right-shifted overlay.

### Notes
Reproduced twice — reliable. Purely visual: state, marks, `entries`, and the
committed-search behavior (05.5) are all correct; the empty-pattern render
(05.3) and post-commit render (05.5, `x` marks) show the names intact, so the
corruption is exclusively the live-highlight overlay path. The misalignment is a
fixed +1 offset tied to the icon-cell-width assumption in the terminal/tmux;
depends on the emoji glyph rendering as 1 cell. Suspected fix area:
`directory.rs:475` should compute the overlay x from the actual rendered
symbol width (as `print_styled` computes `symbol_width`) rather than the
constant `4`, or draw the highlight inline instead of as an overlay.

---

## Protocol feedback

- **05.7 / 05.10 overlay middle-line wording:** the step expects the console
  middle line to show "the current path `$FIXTURE/`" (05.7) / "updates to
  `$FIXTURE/`" (05.10). In practice the console ALSO appends its top
  recommendation as a dark-grey suffix, so the captured line reads e.g.
  `$FIXTURE/beta dir` (05.7) or `$FIXTURE/alpha dir` (05.10). This is documented
  console behavior (`console.rs:76-114`, `rec_text` drawn after `path`), not a
  bug — but the Expect block's literal `$FIXTURE/` invites a false FAIL. Suggest
  the step note that a dark-grey recommendation suffix is expected and to assert
  the `path` prefix + trailing slash only (which I did).
- **05.7 recommendation is `beta dir`, not `alpha dir`:** the note already warns
  recommendation ordering is case-folded/prefix-dependent (PatriciaSet), so this
  is fine — worth an explicit "do not assert which subdir is recommended".
- **05.11 zoxide-installed path is under-specified:** the Expect block only
  describes the missing-zoxide hint string. When zoxide IS installed (this env)
  the middle line shows a `.`/query input in red with no "not installed" hint.
  The step is passable (mode+overlay), but the Expect block should state what the
  installed case looks like (input line, no hint) so an executor doesn't try to
  assert the hint string against an installed host.
- **Backspace key name:** `tmux send-keys ... Backspace` and `BSpace` both work;
  the section text says `Backspace`. Minor — no change needed, just noting the
  literal-vs-keyname distinction held (Backspace sent WITHOUT `-l`).
- Fixtures under `/tmp` flood the `left` pane and the socket `log` with the
  neighbouring `tmp.*` dirs and `Updating: /tmp` lines; harmless here but it
  makes `log`-based failure triage noisier. Not actionable for this section.
