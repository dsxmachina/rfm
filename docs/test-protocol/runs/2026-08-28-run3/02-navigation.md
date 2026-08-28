# Run 3 — Section 02 — Directory navigation

- **Date:** 2026-08-28
- **Branch:** feat/kitty-unicode-placeholders (binary prebuilt at `./target/debug/rfm`)
- **Harness:** tmux session `tp3-02`, socket `/tmp/tp3-02.sock`, 120×30 pane,
  launched directly as the session command with `env -u KITTY_WINDOW_ID -u
  GHOSTTY_RESOURCES_DIR XDG_CACHE_HOME/XDG_STATE_HOME/_ZO_DATA_DIR` tempdirs
  and `--config` scratch dir. Quiet fixture parent (`PARENT=$(mktemp -d)`,
  `FIXTURE=$PARENT/fx`). zoxide/socat/tmux all present; `image_protocol`
  resolved to `half-block` as expected inside tmux.
- **Environment notes:** zoxide IS on PATH (02.22/02.23 ran, 02.24 skipped).
  `~/Music` missing → used for 02.13.

**Tally: 25 steps — 24 pass, 0 fail, 1 skip (02.24, conditional not met).**
No bugs filed.

---

## Per-step results

### 02.1 — Initial three-pane layout — PASS
Socket: `mode=normal`, `view=single`, `cwd=$FIXTURE`, `selection=alpha`,
`selected_idx=0`, `total=5`, `show_hidden=false`, `marked=[]`,
`left_path=$PARENT`, `preview_path=$FIXTURE/alpha`. `entries center` returned
exactly the 7 expected objects in order (`.hidden-dir`, `alpha`,
`Bilder & Videos`, `emptydir`, `.hidden.txt`, `a.txt`, `b.txt`), both hidden
entries flagged `hidden:true`, sole `selected:true` on `alpha`.
Screen: top row `$FIXTURE/alpha` (header shows selected item's path — per
README), center lists the 5 visible entries, no dotfiles anywhere, right
column shows `nested`, left column shows `fx`.

### 02.2 — Enter directory with `l` — PASS
Socket: `cwd=$FIXTURE/alpha`, `left_path=$FIXTURE`, `selection=nested`,
`total=1`, `preview_path=.../alpha/nested`. `entries left` = the 7 fixture
entries with `alpha` `selected:true`. `log` contained `move-right`,
`Executing command 'zoxide add current-dir': zoxide add /tmp/.../fx/alpha`
and `Command 'zoxide add current-dir' completed successfully`.
Screen: top row contains `/alpha`; left shows fixture listing (`a.txt`,
`emptydir` visible); center exactly `nested`; right shows `deep`.

### 02.3 — Enter directory with Right arrow — PASS
Socket: `cwd=.../alpha/nested`, `left_path=.../alpha`, `selection=deep`,
`preview_path=.../nested/deep`. Screen: center `deep`, left `nested`, right
`leaf.txt`.

### 02.4 — Deep descent to a file preview — PASS
Socket: `cwd=.../nested/deep`, `selection=leaf.txt`, `total=1`,
`preview_path=.../deep/leaf.txt`. After a 0.6 s settle poll (seq 16→18,
then stable) the preview column showed `leaf content`.

### 02.5 — Parent with `h` restores selection — PASS
Socket: `cwd=.../alpha/nested`, `selection=deep`. `log` contained TRACE
`move-left`. Screen: center `deep`, preview `leaf.txt`.

### 02.6 — Re-descend restores deeper selection (rev-history) — PASS
Socket: `cwd=.../nested/deep`, `selection=leaf.txt`. `log` contained
`pop rev-history: /tmp/.../deep/leaf.txt, len=0`. Screen identical to 02.4
(preview `leaf content` after settle).

### 02.7 — Left ×3 walks back to fixture root — PASS
Socket: `cwd=$FIXTURE`, `selection=alpha`, `selected_idx=0`, `total=5`,
`left_path=$PARENT`. Screen identical to 02.1 (right column `nested`).

### 02.8 — Empty directory: preview and descent — PASS
After `j j`: `selection=emptydir`, `selected_idx=2`,
`preview_path=$FIXTURE/emptydir`; preview column showed `(empty)`.
After `l`: `cwd=$FIXTURE/emptydir`, `selection=null`, `total=0`,
`preview_path` = sentinel `path-of-empty-panel` (as documented); center
showed `(empty)` and nothing else; top row contained `/emptydir`.
After `h`: `cwd=$FIXTURE`, `selection=emptydir`. All socket replies were
prompt — no wedge.

### 02.9 — Directory name with spaces and `&` — PASS
Socket: `cwd=$FIXTURE/Bilder & Videos`, `selection=clip.txt`. `log`:
`Executing command 'zoxide add current-dir': zoxide add '/tmp/.../fx/Bilder & Videos'`
(properly single-quoted) followed by `completed successfully`; `log 200`
contained **zero** `failed with exit code` lines. Screen: top row contained
`Bilder & Videos`, center showed `clip.txt`. After `h`: `cwd=$FIXTURE`,
`selection=Bilder & Videos`.

### 02.10 — `gr` → / — PASS
Socket: `cwd=/`, `selection=bin` (first visible entry), `left_path` =
sentinel `path-of-empty-panel` (documented for filesystem root). `log`
contained TRACE `jump-to /`. Screen: root listing with both `etc` and `usr`
present (grep count 2).

### 02.11 — `''` returns to pre-jump directory — PASS
Socket: `cwd=$FIXTURE`, `selection=alpha`. `log`: `jump-to /tmp/.../fx`.
Screen: 02.1 layout restored.

### 02.12 — `gh` → $HOME, `''` back — PASS
After `gh`: `cwd=/home/someone` (`total=121`); top row contained the $HOME
path. After `''`: `cwd=$FIXTURE`, fixture listing back.

### 02.13 — jump_to to a missing directory is a no-op — PASS
`~/Music` does not exist on this machine (`gm` chosen). seq before 73,
after `g m` await-idle seq 75 (input processed); `cwd` and `selection`
unchanged (`$FIXTURE`/`alpha`). `log` contained TRACE
`jump-to /home/someone/Music` — the jump refused the non-existent path.
Screen: unchanged fixture listing.

### 02.14 — Toggle hidden ON (`zh`) — PASS
Socket: `show_hidden=true`, `total=7`, `selection=alpha` (preserved),
`selected_idx=1`. `entries center` unchanged in content, `alpha` sole
`selected:true`. Screen: `.hidden-dir` first row, `.hidden.txt` between
`emptydir` and `a.txt`, alongside the 5 previously visible entries.

### 02.15 — Toggle hidden OFF (`zh`) — PASS
Socket: `show_hidden=false`, `total=5`, `selection=alpha`, `selected_idx=0`.
Screen: no `hidden`-named rows in the visible listing.

### 02.16 — Toggle hidden OFF while a hidden entry selected (re-clamp) — PASS
With hidden ON, `gg` + `j j j j` landed on `.hidden.txt` (`selected_idx=4`,
confirmed). After `zh` OFF: `show_hidden=false`, `selection=a.txt` (visible,
non-hidden), `selected_idx=3`; `entries center` shows exactly one
`selected:true` and it has `hidden:false`; `preview_path=$FIXTURE/a.txt`
== cwd/selection (preview refreshed). Screen: no dotfile rows; preview
column empty, consistent with the selected 0-byte file.

### 02.17 — Open the cd console — PASS
Socket: `mode=console`, `cwd` still `$FIXTURE`. Screen: overlay band around
the vertical center with rule lines above (`┴───…`) and below (`┬───…`); the
path line read `/tmp/.../fx/alpha` — base path up to and including the `/`
present, with the dark-grey completion (`alpha`) after it, as documented.

### 02.18 — cd console: typed completion navigates live, Enter accepts — PASS
After typing `alpha` (send-keys -l): `mode=console` AND
`cwd=$FIXTURE/alpha` — live navigation confirmed. After Enter:
`mode=normal`, `cwd=$FIXTURE/alpha`. Screen: overlay gone, top row contains
`/alpha`, center shows `nested`.

### 02.19 — cd console: Backspace on empty input goes to parent — PASS
After `BSpace` (correct tmux key name): still `mode=console`,
`cwd=$FIXTURE` (live parent navigation). Overlay line showed
`/tmp/.../fx/` + grey completion. After Enter: `mode=normal`,
`cwd=$FIXTURE`; fixture listing back.

### 02.20 — cd console: Esc reverts to starting directory — PASS
Mid-console after typing `alpha`: `mode=console`, `cwd=$FIXTURE/alpha`
(confirmed). After Escape: `mode=normal`, `cwd=$FIXTURE` — rollback
confirmed. Screen: overlay gone, fixture listing. Note: the top row still
reads `.../fx/alpha` because the header shows the *selected item's* path
(selection is `alpha`) — the step's "top row shows $FIXTURE (no /alpha)"
expectation conflicts with the README's header rule; asserted the socket
rollback + selection semantics instead (see protocol feedback).

### 02.21 — cd console: Tab cycles directory recommendations — PASS
After Tab: `mode=console`, `cwd=$FIXTURE/alpha` — member of the allowed set
{alpha, Bilder & Videos, emptydir}. Overlay line showed the completed
subdir path `/tmp/.../fx/alpha/`. After Escape: `cwd=$FIXTURE`,
`mode=normal`.

### 02.22 — zoxide console: query navigates live, Esc reverts — PASS
(zoxide on PATH.) After `CD`: `mode=console`, cwd unchanged. After typing
`zoxtarget`: `mode=console`, `cwd=$ZOXTARGET` (live). `log` contained
per-keystroke TRACE `zoxide query '…'` lines ending with
`zoxide query 'zoxtarget'`. Overlay showed the $ZOXTARGET path. After
Escape: `cwd=$FIXTURE`, `mode=normal`.

### 02.23 — zoxide console: Enter accepts; `''` returns — PASS
After Enter: `mode=normal`, `cwd=$ZOXTARGET`; center column `(empty)`
(seeded dir is empty). After `''`: `cwd=$FIXTURE`, fixture listing back.

### 02.24 — zoxide console without zoxide installed — SKIP (N/A)
Conditional: run only if `command -v zoxide` fails. zoxide IS installed on
this machine, so the step does not apply.

### 02.25 — Session jump-mark: `ma` set, `'a` jump back — PASS
After `gg` + `m a`: `jump_marks == {"a": "$FIXTURE"}`; `log` contained
`jump-mark 'a' set -> /tmp/.../fx`. After `l` (descend into alpha) then
`' a`: `cwd=$FIXTURE`, `selection=alpha`. SGR capture (`-e`): the `alpha`
row in the center column carries the reverse-video attribute (`[7m`).

---

## Bugs

None.

## Protocol feedback

1. **02.20 screen expectation contradicts the README header rule.** The step
   expects "top row shows $FIXTURE (no `/alpha`)" after Esc reverts, but the
   README pitfall list (correctly) documents that the header row shows the
   *selected item's* path — and after the revert the selection is `alpha`,
   so the top row legitimately reads `.../fx/alpha`. The step text should
   drop the "no `/alpha`" clause or reword it to "top row shows the selected
   entry's path under $FIXTURE".
2. **02.8 expectation text vs. sentinel note.** The Expect(socket) line says
   "after `j j`: `preview_path==$FIXTURE/emptydir`" while the sentinel note
   says the step "does not assert preview_path". In practice
   `preview_path` was the real `$FIXTURE/emptydir` path after `j j` (cursor
   on the dir) and the sentinel only appears after descending *into* the
   empty dir. The note could state that split explicitly (cursor-on-empty-dir
   → real path; inside-empty-dir → sentinel) — observed behavior matches
   that reading.
3. Minor: 02.22's suggested `tmux send-keys -t $SESSION CD` benefits from
   `-l` (literal) for symmetry with the README's literal-token rule; plain
   `CD` happens to work but `-l` removes any ambiguity. Same for typed words
   (`alpha`, `zoxtarget`).
