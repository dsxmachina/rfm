# Section 02 — Directory navigation

N=02, SESSION=rfm-sec02, SOCK=/tmp/rfm-sec02.sock. Harness per README.

**Section fixture** (create before launch, in addition to the standard harness dirs):

```bash
PARENT=$(mktemp -d); FIXTURE="$PARENT/fx"; mkdir "$FIXTURE"   # quiet parent (README)
mkdir -p "$FIXTURE/alpha/nested/deep"
printf 'leaf content\n' > "$FIXTURE/alpha/nested/deep/leaf.txt"
mkdir -p "$FIXTURE/Bilder & Videos"
touch "$FIXTURE/Bilder & Videos/clip.txt"
mkdir "$FIXTURE/emptydir"
mkdir "$FIXTURE/.hidden-dir"
touch "$FIXTURE/a.txt" "$FIXTURE/b.txt" "$FIXTURE/.hidden.txt"

# zoxide seeding: a target dir with a name that matches nothing else
ZO_DATA=$(mktemp -d)          # use THIS as _ZO_DATA_DIR in the pane, not a fresh mktemp
ZOXTARGET=$(mktemp -d)/zoxtarget-e2e
mkdir -p "$ZOXTARGET"
command -v zoxide >/dev/null && _ZO_DATA_DIR="$ZO_DATA" zoxide add "$ZOXTARGET"
```

In the tmux pane, export the standard isolation env with `_ZO_DATA_DIR="$ZO_DATA"` (the
seeded dir above, NOT a new mktemp), then launch:
`./target/debug/rfm --debug-socket $SOCK --config $CFG $FIXTURE`.

**Sorting/visibility ground truth for this fixture** (lowercase-name sort, then
directories stable-sorted before files; hidden = leading `.`):

- Visible order (show_hidden=false, the default): `alpha`(0), `Bilder & Videos`(1),
  `emptydir`(2), `a.txt`(3), `b.txt`(4) → `total`=5, initial `selection`="alpha",
  `selected_idx`=0.
- Full `entries center` order (ALL entries incl. hidden):
  `.hidden-dir`, `alpha`, `Bilder & Videos`, `emptydir`, `.hidden.txt`, `a.txt`, `b.txt`
  (7 entries; `.hidden-dir` and `.hidden.txt` have `hidden:true`).
- rfm writes a first-run stub `config.toml` into `$CFG` — expected, harmless.
- Every successful directory descent/jump queues a `zoxide add current-dir` background
  command IF zoxide is on PATH; with zoxide absent nothing is queued (a debug log line
  `zoxide not found in PATH` appears once). Steps below that assert zoxide log lines are
  marked conditional on `command -v zoxide`.
- Note: Enter is NOT a default binding for entering a directory (only `l` from
  `[keys.movement] right` and the hardcoded Right arrow); do not test Enter in normal mode.

Steps are ordered and reuse state; each step lists the cwd it starts from.

---

### 02.1 — Initial three-pane layout
**Action:** none (immediately after launch + socket appears). `echo await-idle | socat - UNIX-CONNECT:$SOCK`, then `echo state | socat - UNIX-CONNECT:$SOCK`, `echo "entries center" | socat - UNIX-CONNECT:$SOCK`, `tmux capture-pane -t $SESSION -p`.
**Expect (socket):** `mode`=="normal", `view`=="single", `cwd`==$FIXTURE, `selection`=="alpha", `selected_idx`==0, `total`==5, `show_hidden`==false, `marked`==[], `left_path`==dirname($FIXTURE), `preview_path`==$FIXTURE/alpha. `entries center`: exactly 7 objects in the full order listed above; `.hidden-dir` and `.hidden.txt` have `hidden:true`; exactly one entry has `selected:true` and it is `alpha`.
**Expect (screen):** top row contains the $FIXTURE path. Center column lists `alpha`, `Bilder & Videos`, `emptydir`, `a.txt`, `b.txt`; neither `.hidden-dir` nor `.hidden.txt` appears anywhere in the center column. Right (preview) column shows the listing of `alpha`, i.e. contains `nested`. Left column shows the parent directory's listing and contains the basename of $FIXTURE.
**Note:** initial panel loads are async — if the capture looks like a loading placeholder, poll `state` until `seq` is stable across two polls 0.5s apart, then re-capture.

### 02.2 — Enter directory with `l`
**Action:** (from $FIXTURE, `alpha` selected) `tmux send-keys -t $SESSION l` → await-idle → `state`, `entries left`, `log 30`, capture-pane.
**Expect (socket):** `cwd`==$FIXTURE/alpha, `left_path`==$FIXTURE, `selection`=="nested", `total`==1, `preview_path`==$FIXTURE/alpha/nested. `entries left` lists the 7 fixture entries with `alpha` `selected:true`. `log` contains a TRACE line `move-right`. If zoxide installed: log contains `Executing command 'zoxide add current-dir'` (may still be queued — poll `log` again after 1s if absent) and later `Command 'zoxide add current-dir' completed successfully`.
**Expect (screen):** top row now contains `/alpha`. Left column shows the fixture listing (contains `a.txt`, `emptydir`); center shows exactly `nested`; right (preview) shows `deep`.

### 02.3 — Enter directory with Right arrow
**Action:** (from $FIXTURE/alpha) `tmux send-keys -t $SESSION Right` → await-idle → `state`, capture-pane.
**Expect (socket):** `cwd`==$FIXTURE/alpha/nested, `left_path`==$FIXTURE/alpha, `selection`=="deep", `preview_path`==.../nested/deep.
**Expect (screen):** center shows `deep`; left shows `nested`; right shows `leaf.txt`.

### 02.4 — Deep descent to a file preview
**Action:** `tmux send-keys -t $SESSION l` → await-idle → `state`, capture-pane.
**Expect (socket):** `cwd`==.../nested/deep, `selection`=="leaf.txt", `total`==1, `preview_path`==.../deep/leaf.txt.
**Expect (screen):** center shows `leaf.txt`; right (preview) shows the file's text content, i.e. contains `leaf content`.
**Note:** file previews load async and await-idle does not cover them — poll `state` until `seq` stabilizes before asserting the preview text.

### 02.5 — Parent with `h` restores selection (fwd-history)
**Action:** `tmux send-keys -t $SESSION h` → await-idle → `state`, `log 20`, capture-pane.
**Expect (socket):** `cwd`==$FIXTURE/alpha/nested, `selection`=="deep" (restored, not reset to first entry — trivially true here with one entry, the real assertion is 02.7). `log` contains TRACE `move-left`.
**Expect (screen):** center shows `deep`; right (preview) shows `leaf.txt` again (columns shifted right).

### 02.6 — Re-descend restores the deeper selection (rev-history)
**Action:** `tmux send-keys -t $SESSION l` → await-idle → `state`.
**Expect (socket):** `cwd`==.../nested/deep, `selection`=="leaf.txt" — the selection inside `deep` was restored from rev-history (log contains `pop rev-history:`).
**Expect (screen):** identical layout to 02.4 (center `leaf.txt`, preview shows `leaf content` after the async load settles).

### 02.7 — Left arrow ×3 walks back to the fixture root, selection restored
**Action:** `tmux send-keys -t $SESSION Left Left Left` → await-idle → `state`, capture-pane.
**Expect (socket):** `cwd`==$FIXTURE, `selection`=="alpha" (fwd-history restored the selection at every level, ending on the dir we originally descended into), `selected_idx`==0, `total`==5, `left_path`==dirname($FIXTURE).
**Expect (screen):** identical layout to 02.1 (center lists the 5 visible entries, right shows `nested`).

### 02.8 — Empty directory: preview and descent
**Action:** `tmux send-keys -t $SESSION j j` (selection: `Bilder & Videos` → `emptydir`) → await-idle → `state`, capture-pane. Then `tmux send-keys -t $SESSION l` → await-idle → `state`, capture-pane. Then `tmux send-keys -t $SESSION h` → await-idle → `state`.
**Expect (socket):** after `j j`: `selection`=="emptydir", `selected_idx`==2, `preview_path`==$FIXTURE/emptydir. After `l`: `cwd`==$FIXTURE/emptydir, `selection`==null, `total`==0. After `h`: `cwd`==$FIXTURE, `selection`=="emptydir".
**Expect (screen):** after `j j`: right (preview) column shows `(empty)`. After `l`: center column shows `(empty)` and nothing else in that column; top row contains `/emptydir`. After `h`: back to the fixture listing with `emptydir` still the selected row.
**Note:** await-idle must return promptly at every point — a `state` timeout here is a wedged-event-loop finding.
**Sentinel note:** for the empty `emptydir`, `state.preview_path` is the sentinel string `"path-of-empty-panel"`, not a real path; the same sentinel appears as `left_path` at filesystem root (`/`) in 02.10. Do not mis-assert a real path against either — this step does not assert `preview_path`.

### 02.9 — Directory name with spaces and `&` (shell-interpolation path)
**Action:** (from $FIXTURE, `emptydir` selected) `tmux send-keys -t $SESSION k` (selection: `Bilder & Videos`) → `tmux send-keys -t $SESSION l` → await-idle → `state`, `log 30`, capture-pane. Then `tmux send-keys -t $SESSION h` → await-idle.
**Expect (socket):** `cwd`=="$FIXTURE/Bilder & Videos", `selection`=="clip.txt". If zoxide installed: `log` contains `Executing command 'zoxide add current-dir': zoxide add ` with the path quoted/escaped, then `Command 'zoxide add current-dir' completed successfully`; there must be NO `failed with exit code` line for `zoxide add current-dir` (an unescaped `&` would background the command or fail — that is exactly the bug this step exists to catch). After `h`: `cwd`==$FIXTURE, `selection`=="Bilder & Videos".
**Expect (screen):** while inside: top row contains `Bilder & Videos`; center shows `clip.txt`.

### 02.10 — jump_to binding `gr` → /
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION g r` → await-idle → `state`, `log 20`, capture-pane.
**Expect (socket):** `cwd`=="/", `selection` non-null (first visible entry of /), `log` contains TRACE `jump-to /`.
**Expect (screen):** top row shows `/`; center lists root entries (contains `etc` and `usr`).
**Note:** jumping clears fwd/rev history by design; do not expect `h` after this to retrace the pre-jump path.

### 02.11 — jump_previous `''` returns to the pre-jump directory
**Action:** `tmux send-keys -t $SESSION "''"` → await-idle → `state`, `log 20`.
**Expect (socket):** `cwd`==$FIXTURE, `selection`=="alpha" (fresh panel → first visible entry), `log` contains TRACE `jump-to ` followed by the $FIXTURE path.
**Expect (screen):** back to the 02.1 layout.

### 02.12 — jump_to `gh` → $HOME, then `''` back
**Action:** `tmux send-keys -t $SESSION g h` → await-idle → `state`. Then `tmux send-keys -t $SESSION "''"` → await-idle → `state`.
**Expect (socket):** after `gh`: `cwd`==$HOME (the `~` in the default jump_to list is expanded at config parse). After `''`: `cwd`==$FIXTURE.
**Expect (screen):** after `gh`: top row contains the $HOME path. After `''`: fixture listing again.

### 02.13 — jump_to to a missing directory is a no-op (conditional)
**Setup:** pick the first default jump target that does NOT exist on this machine: `gm`→~/Music, `gp`→~/Pictures, `gd`→~/Documents, `gD`→~/Downloads. On a typical dev box at least one of the four is missing; if ALL four exist, SKIP this step (record as N/A). This step depends on machine state — assert the *behavior* (jump to a nonexistent path is a no-op), not any specific target.
**Action:** (from $FIXTURE) record `seq` from `state`; send the chosen two-key sequence (e.g. `tmux send-keys -t $SESSION g m`) → await-idle → `state`, capture-pane.
**Expect (socket):** `cwd`==$FIXTURE unchanged, `selection` unchanged; `seq` increased (input WAS processed). A TRACE `jump-to <target>` line may appear in `log` — the jump itself refuses non-existent paths.
**Expect (screen):** unchanged fixture listing.

### 02.14 — Toggle hidden ON (`zh`)
**Action:** (from $FIXTURE, run `tmux send-keys -t $SESSION g g` first to normalize the selection to the top → `selection`=="alpha") `tmux send-keys -t $SESSION z h` → await-idle → `state`, `entries center`, capture-pane.
**Expect (socket):** `show_hidden`==true, `total`==7, `selection`=="alpha" (selection preserved), `selected_idx`==1 (`.hidden-dir` now sorts before it among visible entries). `entries center` unchanged in content (it always lists all 7) — `alpha` still the sole `selected:true`.
**Expect (screen):** center column now shows `.hidden-dir` (first row) and `.hidden.txt`, alongside the 5 previously visible entries.

### 02.15 — Toggle hidden OFF (`zh`)
**Action:** `tmux send-keys -t $SESSION z h` → await-idle → `state`, capture-pane.
**Expect (socket):** `show_hidden`==false, `total`==5, `selection`=="alpha", `selected_idx`==0.
**Expect (screen):** `.hidden-dir` and `.hidden.txt` no longer appear in the center column.

### 02.16 — Toggle hidden OFF while a hidden entry is selected (re-clamp)
**Action:** `tmux send-keys -t $SESSION z h` (hidden ON) → await-idle. `tmux send-keys -t $SESSION g g` then `tmux send-keys -t $SESSION j j j j` (top = `.hidden-dir`, then 4 down → `.hidden.txt`) → await-idle → `state` (confirm `selection`==".hidden.txt"). Then `tmux send-keys -t $SESSION z h` (hidden OFF) → await-idle → `state`, `entries center`, capture-pane.
**Expect (socket):** `show_hidden`==false; `selection` is NOT ".hidden.txt" and not any hidden name — the selection was re-clamped onto a visible entry; in `entries center` exactly one entry has `selected:true` and that entry has `hidden:false`; `preview_path`==`cwd`/`selection` (the preview was refreshed to match the re-clamped selection).
**Expect (screen):** no dotfile rows in center; the right column matches the new `selection` (a directory listing if a dir is selected, file preview text if a file).
**Note:** the exact entry the clamp lands on is implementation-defined — assert the invariants (visible + preview consistent), not a specific name.

### 02.17 — Open the cd console
**Action:** (from $FIXTURE, mode normal) `tmux send-keys -t $SESSION c d` → await-idle → `state`, capture-pane.
**Expect (socket):** `mode`=="console"; `cwd` still $FIXTURE (opening the console does not navigate).
**Expect (screen):** a horizontal overlay band around the vertical center: a rule line above and below, and between them the current path. The console pre-fills a dark-grey autocompletion, so the visible line is `<cwd>/<completion>` — assert only that the base path up to and including the `/` is present (i.e. `$FIXTURE` basename followed by `/`), NOT a literal trailing slash at end-of-line, and do not assert the completion's value.

### 02.18 — cd console: typed completion navigates live, Enter accepts
**Action:** (console open from 02.17) `tmux send-keys -t $SESSION alpha` → await-idle → `state`. Then `tmux send-keys -t $SESSION Enter` → await-idle → `state`, capture-pane.
**Expect (socket):** after typing `alpha` (still in console, `mode`=="console"): `cwd`==$FIXTURE/alpha — the console navigates LIVE the moment the input resolves to the directory. After Enter: `mode`=="normal", `cwd`==$FIXTURE/alpha (accepted).
**Expect (screen):** after Enter the overlay band is gone; top row contains `/alpha`; center shows `nested`.
**Note:** intermediate keystrokes (`a`, `al`, …) do not navigate because no shorter prefix of `alpha` is itself a directory here; only assert cwd after the full word.

### 02.19 — cd console: Backspace on empty input goes to parent
**Action:** (from $FIXTURE/alpha, mode normal) `tmux send-keys -t $SESSION c d` → await-idle. `tmux send-keys -t $SESSION BSpace` → await-idle → `state`. Then `tmux send-keys -t $SESSION Enter` → await-idle → `state`.
**Expect (socket):** after BSpace (still `mode`=="console"): `cwd`==$FIXTURE (live parent navigation). After Enter: `mode`=="normal", `cwd`==$FIXTURE.
**Expect (screen):** while open, the overlay path line contains the $FIXTURE base path up to and including the `/` (a dark-grey completion may follow it — assert the base path + `/` prefix, not a literal trailing slash at end-of-line); after Enter the fixture listing is back.

### 02.20 — cd console: Esc reverts to the starting directory
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION c d` → await-idle. `tmux send-keys -t $SESSION alpha` → await-idle → `state` (confirm `cwd`==$FIXTURE/alpha, `mode`=="console"). Then `tmux send-keys -t $SESSION Escape` → await-idle → `state`, capture-pane.
**Expect (socket):** `mode`=="normal", `cwd`==$FIXTURE — the live navigation was rolled back to where the console was opened.
**Expect (screen):** overlay gone, fixture listing, top row shows $FIXTURE (no `/alpha`).

### 02.21 — cd console: Tab cycles directory recommendations
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION c d` → await-idle. `tmux send-keys -t $SESSION Tab` → await-idle → `state`. `tmux send-keys -t $SESSION Escape` → await-idle → `state`.
**Expect (socket):** after Tab: `mode`=="console" and `cwd` is one of the three direct non-hidden subdirs of $FIXTURE (`alpha`, `Bilder & Videos`, `emptydir`) — Tab completes to a recommendation and navigates live. After Escape: `cwd`==$FIXTURE, `mode`=="normal".
**Expect (screen):** while open, the overlay path line shows the completed subdir; after Escape, back to the fixture listing.
**Note:** which of the three Tab lands on first depends on the cursor position when the console was opened — assert set membership, not a specific name.

### 02.22 — zoxide console: query navigates live, Esc reverts (conditional: zoxide on PATH)
**Setup:** skip if `command -v zoxide` fails (see 02.24 instead). Requires the `$ZOXTARGET` seeding from the section fixture.
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION CD` → await-idle → `state` (confirm `mode`=="console"). `tmux send-keys -t $SESSION zoxtarget` → await-idle → `state`, `log 30`. Then `tmux send-keys -t $SESSION Escape` → await-idle → `state`.
**Expect (socket):** after typing: `mode`=="console", `cwd`==$ZOXTARGET (the query's best hit, navigated live); `log` contains a TRACE line starting `zoxide query '` (per-keystroke queries; the last one for the full input `zoxtarget`). After Escape: `cwd`==$FIXTURE, `mode`=="normal".
**Expect (screen):** while open, the overlay band shows the $ZOXTARGET path; after Escape, fixture listing.
**Note:** rfm's own descents in earlier steps added fixture dirs to the same `_ZO_DATA_DIR`; the query string `zoxtarget` matches only the seeded target, keeping the result deterministic.

### 02.23 — zoxide console: Enter accepts; `''` returns (conditional: zoxide on PATH)
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION CD` → await-idle. `tmux send-keys -t $SESSION zoxtarget` → await-idle. `tmux send-keys -t $SESSION Enter` → await-idle → `state`. Then `tmux send-keys -t $SESSION "''"` → await-idle → `state`.
**Expect (socket):** after Enter: `mode`=="normal", `cwd`==$ZOXTARGET. After `''`: `cwd`==$FIXTURE (the live-Cd recorded $FIXTURE as the previous directory).
**Expect (screen):** at $ZOXTARGET the center column is `(empty)` (the seeded dir is empty); after `''` the fixture listing returns.

### 02.24 — zoxide console without zoxide installed (conditional: zoxide NOT on PATH)
**Setup:** run only if `command -v zoxide` fails.
**Action:** (from $FIXTURE) `tmux send-keys -t $SESSION CD` → await-idle → `state`, capture-pane. `tmux send-keys -t $SESSION abc` → await-idle → `state`. `tmux send-keys -t $SESSION Escape` → await-idle → `state`.
**Expect (socket):** `mode`=="console" while open; `cwd`==$FIXTURE throughout (typing never navigates and never spawns a process); after Escape `mode`=="normal", `cwd`==$FIXTURE.
**Expect (screen):** the overlay band shows the literal text `zoxide is not installed` instead of a path, both immediately on open and after typing.

### 02.25 — Session jump-mark: `m a` set, `' a` jump back
**Action:** (from $FIXTURE, `state.selection` noted as S) `tmux send-keys -t $SESSION m a` → await-idle → `state`, `log 20`. Then `tmux send-keys -t $SESSION l` (descend into the selected dir; ensure S is a directory — press `gg` first so S=="alpha") → await-idle. Then `tmux send-keys -t $SESSION "'" a` → await-idle → `state`, `log 20`.
**Expect (socket):** after `ma`: `jump_marks` contains key "a" mapping to $FIXTURE; `log` contains `jump-mark 'a' set -> ` + $FIXTURE path. After `'a`: `cwd`==$FIXTURE and `selection`=="alpha" (the mark restores directory AND highlighted entry).
**Expect (screen):** after `'a`: fixture listing with the `alpha` row selected.
**Note:** send the apostrophe as its own send-keys argument (`tmux send-keys -t $SESSION "'" a`) to avoid shell-quoting accidents.

---

**Teardown:** `tmux kill-session -t $SESSION; rm -rf "$PARENT" "$ZO_DATA" "$(dirname "$ZOXTARGET")" "$CFG"; rm -f $SOCK` (`$PARENT` wraps the quiet-parent `$FIXTURE`; plus the standard XDG temp dirs from the harness).

**Section coverage gaps** (deliberately not covered here):
- Cursor movement within a listing beyond what navigation needs (`gg`/`G`/paging/half-paging) — cursor-movement section.
- Opening a FILE via `l`/Right (opener routing, `Opening '<file>'` log) — opener section.
- Tabs/split-view navigation (`gn`, `Tab`, `!`, per-tab cwd) and `entries <tab>` addressing — tabs section.
- Search-driven selection (`/`, `n`, `N`) and marking (Space) — search/marking section.
- Preview backend correctness (only "contains `leaf content`" and `(empty)` are asserted; formats, caches, protocols are the preview sections' job).
- File-watcher freshness (external `mkdir`/`rm` while rfm is running).
- cd-console `..`-input parent shortcut and BackTab cycling; zoxide Tab/BackTab result cycling; multi-word zoxide queries.
- Navigation through symlinked directories and permission-denied directories.
- `jump_previous` no-op semantics on a fresh tab (previous == own cwd).
- Hidden-name variants `__`-prefix and `.swp`-suffix (only dotfiles are exercised).
