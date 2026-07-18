# Undo-Stack — Design

Status: accepted (v1 scope), 2026-07-18
Scope owner: discussion in session; ready for implementation planning.

## Goal

Give rfm an in-session undo/redo stack that can reverse the file-manager
operations whose effect on the filesystem has a well-defined inverse.
Operations whose effect cannot be reversed (permanent delete, arbitrary
external commands) must never cause a *surprising* undo — they are handled
explicitly rather than silently skipped where it matters.

## Scope (v1)

**In scope — reversible operations (Tier 1 + Bulk-Rename):**

| Action            | Atomic change(s)                       |
|-------------------|----------------------------------------|
| Mkdir / Touch     | `Create` (×1)                          |
| Zip / Tar         | `Create` (the archive; sources intact) |
| Rename (single)   | `Move` (×1)                            |
| Paste (cut/move)  | `Move` (×N)                            |
| Paste (copy)      | `Copy` (×N)                            |
| Delete → Trash    | `Move` (original → trash_path)         |
| Bulk-Rename       | `Move` (×N, net from→final)            |

**Out of scope (v1), by decision:**

- Persistence across sessions (stack is in-memory, per session).
- Redo-stack serialization.
- Extract undo (tracking written paths / overwrite detection).
- Persistent / XDG-spec trash (current trash stays a session tmpdir).
- Undo-history UI (log-line feedback only, no panel).

## Decisions

- **Delete model:** permanent delete stays possible. Trash-backed delete
  is undoable (it is just a `Move`). Permanent delete pushes a **Barrier**
  (see below), it is never silently reversed.
- **Persistence:** in-memory, per session. No serialization, no
  "target changed since" reconciliation on disk. Matches the current
  session-tmpdir trash.
- **Redo:** yes. Undo + Redo. Each operation therefore knows both its
  forward (redo) and inverse (undo) form; recording a new transaction
  clears the redo stack.
- **External / out-of-scope actions (Extract, UserCommand, external
  open):** **ignored** by the undo stack. Undo skips over them and
  reverses the most recent *tracked* transaction. Accepted risk: after a
  shell command, undo may revert something "older" than the user expects.
  Rationale: treating every shell command as a barrier makes undo useless
  in practice.
- **Undo failure (a single change cannot be applied, e.g. target changed
  externally):** **abort at the first failing change**, log the error, and
  leave the (untouched-so-far) transaction on the stack. No half-reverted
  state without a way back.

## Core model

The whole scope collapses to **three** atomic, reversible filesystem
changes. Everything else is a combination of them.

```rust
enum FsChange {
    Create { path: PathBuf, is_dir: bool },   // undo: delete path · redo: create
    Move   { from: PathBuf, to: PathBuf },     // undo: to→from     · redo: from→to
    Copy   { from: PathBuf, to: PathBuf },     // undo: delete to    · redo: copy from→to
}
```

A single **user action** maps to a **transaction** — one undo unit, N
atomic changes:

```rust
struct Transaction { label: String, changes: Vec<FsChange> }
```

- **Undo** replays `changes` in **reverse** order, applying each inverse.
- **Redo** replays them in **forward** order.
- **Partial success is free:** if a paste completes 3 of 5 files, the
  transaction holds exactly those 3 changes; undo reverses exactly those.

The stack distinguishes reversible transactions from hard stops:

```rust
enum UndoEntry {
    Tx(Transaction),
    Barrier { reason: String },   // e.g. "permanentes Löschen"
}
```

Undo hitting a `Barrier` **stops** and reports
*"kann nicht rückgängig gemacht werden: <reason>"*; the stack stays intact
and nothing wrong is reverted.

### The `_`-suffix trap (solved)

copy / move / rename / trash append `_` to the target name on collision
(`get_destination` / `rename_safe`). The undo must reverse the
**actually-reached** path, not the *requested* one. The primitives already
return the reached path — that value goes into `FsChange::to`, so the
inverse is always correct.

## Where recording happens — the choke point

Filesystem mutations are scattered (`util.rs`, `manager.rs`,
`opener.rs`), but the **effects** are centralized in the command handlers
in `manager.rs` (`apply_mode_op` + the `Command` match). That is the layer
that records.

**Principle: primitives return the change, the handler collects it.**

```rust
// util.rs / manager.rs — primitives yield the change they made
fn move_item(from, to_dir) -> io::Result<FsChange>       // Move { from, to: reached }
fn copy_item(from, to_dir) -> io::Result<FsChange>       // Copy { from, to: reached }
fn create_item(path, is_dir) -> io::Result<FsChange>     // Create { .. }
fn delete_file(path) -> io::Result<Option<FsChange>>     // Some(Move) if trash, None if permanent
```

The handler assembles a transaction and pushes it:

```rust
// e.g. Command::Paste
let mut tx = Transaction::new("paste");
for f in clipboard.files {
    match move_item(f, cwd) {
        Ok(change) => tx.push(change),
        Err(e)     => log(e),   // partial success: only what worked lands in tx
    }
}
self.undo.record(tx);           // also clears the redo stack
```

The stack is a terminal-free module (`src/undo/`), unit-testable like the
mode adapters:

```rust
struct UndoStack { undo: Vec<UndoEntry>, redo: Vec<Transaction> }
impl UndoStack {
    fn record(&mut self, tx: Transaction) { self.redo.clear(); self.undo.push(UndoEntry::Tx(tx)); }
    fn barrier(&mut self, reason: String) { self.undo.push(UndoEntry::Barrier { reason }); }
    fn undo(&mut self) -> ... ;   // pop → hand to executor
    fn redo(&mut self) -> ... ;
}
```

**Crucial:** executing undo/redo must **not** record a new transaction
(else: infinite loop). The undo/redo apply path bypasses `record()` and
pushes onto the *other* stack instead.

**Async paste:** paste runs in `spawn_blocking`. The transaction is
recorded on the **completion path** (when the result comes back), not at
dispatch — same pattern as `panel-update` arrivals.

## Barriers & external changes

- **Permanent delete** (trash off) → `Barrier { reason: "permanentes
  Löschen" }`. In scope, clear semantics.
- **Extract / UserCommand / external open** → **ignored** (untracked). Not
  even a barrier; undo reverses the last tracked transaction. Accepted UX
  bet (see Decisions).
- **External FS changes:** undo is **best-effort per change**; on the first
  failure → abort, log, leave the transaction on the stack. The inverse
  reuses `get_destination` / `_` logic instead of blind overwrite, so undo
  never clobbers an unrelated file.

## UX

New `Command` variants + default keys (vim-style):

```toml
undo = [ "u" ]
redo = [ "ctrl-r" ]
```

Feedback via the existing logger (10s widget TTL): *"rückgängig: rename
a → b"*, *"redo: paste (3 Dateien)"*, or on barrier/failure *"kann nicht
rückgängig gemacht werden: …"*. The transaction `label` provides the text.

Debug socket: the `state` reply gains `undo_depth` / `redo_depth` for
deterministic tmux-harness testing (action → `await-idle` → `state` →
undo → diff `entries` against expectation).

## Edge cases

- **Zip/Tar redo** regenerates the same archive name; if it already exists
  on redo, `check_filename` appends `_`, so redo may produce a slightly
  different path. Accepted — consistent with existing behavior.
- **Touch on an existing file:** current code only opens the file (no
  truncate). If the file already existed, there is nothing to undo →
  record **no** transaction; only a genuinely newly-created file yields a
  `Create`.

## Affected files

- `src/undo/` — new module: `FsChange`, `Transaction`, `UndoEntry`,
  `UndoStack` (+ tests).
- `src/util.rs` — primitives return `FsChange`.
- `src/panel/manager.rs` — handlers collect transactions & call `record()`;
  new `apply_undo` / `apply_redo`; delete path sets a barrier on permanent
  delete; `state` snapshot extended.
- `src/engine/commands.rs` — `Command::Undo` / `Redo` + default keys.
- `examples/keys.toml` — documented bindings.
- async-paste completion path — record the transaction there.

## Testing

- `UndoStack` and `FsChange` inverse logic are terminal-free → unit tests
  like the mode adapters.
- One integration test per action type: run, snapshot state, undo, assert
  FS == initial; redo, assert FS == post-action.
- Round-trip properties on the FS: `redo ∘ undo == identity` and
  `undo ∘ redo == identity`.
