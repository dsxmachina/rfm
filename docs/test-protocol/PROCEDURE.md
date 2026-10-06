# Standardized rfm e2e test-and-fix procedure

This is the repeatable, AI-driven loop that produced and exercises the
[test protocol](README.md). Run it whenever you want a full behavioral sweep of
rfm with automatic bug triage and fixing. It is designed for an orchestrator
agent that dispatches fresh-context subagents (a `Workflow` is ideal, but the
phases work with plain subagents too).

The loop has five phases. Each phase's output is the next phase's input; the
orchestrator stays in the loop between phases and reads results before
proceeding.

```
Author ──► Execute ──► Triage ──► Fix (TDD) ──► Reverify ──► (finalize)
  │           │                        │            │
  └─ sections └─ bugs + feedback       └─ commits   └─ confirms fixes + protocol
```

## Phase 1 — Author / update the protocol

Only needed when features changed or the protocol doesn't exist. Fan out one
writer per section (see the section list in [README.md](README.md)); give each
writer: CLAUDE.md, the [conventions](#authoring-conventions) below, a
cross-section feature inventory, and two already-written sibling sections as
format exemplars. Every writer **re-verifies each key/command/mode-name/string
against source** before putting it in a step — never invent a binding.

## Phase 2 — Execute

One fresh-context executor per section, run in parallel (each uses its own
tmux session, socket and fixtures, so they don't collide). Each executor:

1. reads README.md (harness + verification loop + pitfalls), the bug-report
   template, and its section;
2. runs every step in order, asserting **both** the socket state and the
   captured screen;
3. documents every failure as a bug (reproduced twice where possible) and
   **runs to the end** regardless;
4. writes a detailed per-section report to `runs/<run>/<section>.md` and
   returns a structured summary (tallies + bug list + protocol feedback).

Use a JSON-schema-enforced structured return so the orchestrator can aggregate
without parsing prose. See the executor prompt template below.

## Phase 3 — Triage

The orchestrator reads all structured returns and separates:

- **Real rfm bugs** — a step failed because the binary misbehaved. Route to
  Phase 4. File the state-vs-render class correctly (it routes the fix): the
  `entries` reply correct but the screen stale → render; `entries` also wrong →
  state.
- **Protocol/harness problems** — wrong key in a step, bad fixture recipe,
  flaky assertion, missing external tool. These are NOT bugs; apply them as
  protocol refinements (Phase 5 feeds them back) and re-word the offending
  steps.

A once-only failure is still filed, marked flaky — but before fixing, confirm
it reproduces deterministically from a clean harness (run 1's 12.8 was filed
`reproduced_twice=false` yet turned out fully deterministic once the fixture
allowed no cursor move).

## Phase 4 — Fix, test-first, with rollback discipline

For each confirmed bug, follow `systematic-debugging` then
`test-driven-development`:

1. **Root cause before any fix.** Read the failing path in source; confirm the
   mechanism. Don't fix a symptom.
2. **Write the failing test first** and watch it fail *for the right reason*
   (assertion, not a typo). Prefer a fast, deterministic unit test that pins
   the exact wrong decision over a slow timeout/integration test — extract a
   pure seam if needed (run 1: `classify_preview` for the FIFO hang; the
   inline-highlight invariant "highlighting must not change the visible
   characters" for the search overlay; a `move_left` preview-follows-selection
   assertion for the stale preview).
3. **Minimal fix** to green. No "while I'm here" changes.
4. **Verify** the unit test, the full suite (`cargo test`), and clippy, then
   **reproduce the fix live** through the same tmux steps that first exposed it.
5. **Roll back non-fixes.** If a change doesn't actually fix the bug (test
   still red, or live repro unchanged), revert it — do not leave speculative
   edits in the tree. Commit one focused fix per bug.

## Phase 5 — Reverify + finalize

Re-run *only* the affected sections with fresh executors against the rebuilt
binary; confirm each previously-failing step now passes and no new bug
appeared. Simultaneously this validates the Phase-3 protocol refinements (the
re-run uses the updated harness). Update the run report. The job is done when
every found bug is fixed and the protocol is internally consistent.

## Executor prompt template (optimized)

The run-1 lessons are baked in — copy this when dispatching Phase 2/5 agents:

- Point at README.md, the bug template, and the one section file; say "obey
  literally."
- Mandate the four-step verification loop **per step** (input → `await-idle` →
  socket assert → capture-pane assert); a socket/screen mismatch is itself a
  bug.
- **Wrap every socket read in `timeout 12`** — a wedged loop must not hang the
  executor; a timeout on `state` IS a wedge finding.
- **Run to the end**; only skip steps that depend on a failed one; a step you
  cannot verify is a *skip*, not a pass.
- Capture evidence live (pane + `state` + `log 50/200`) at failure time, and
  reproduce twice before filing as reliable.
- Distinguish rfm bugs from harness/protocol problems; the latter go to
  `protocol_feedback`, not `bugs`.
- Forbid mutating git; restrict writes to mktemp fixtures + the report path;
  tear down tmux + socket + fixtures at the end.
- Return a **schema-enforced** structured object (tallies + bugs + feedback +
  report path); the detailed narrative goes to the report file, keeping the
  orchestrator's context lean.

## Authoring conventions

(Full detail lives in README.md — the load-bearing rules:)

- Fixtures nest under a **fresh empty parent** (`PARENT=$(mktemp -d);
  FIXTURE=$PARENT/fx`) so rfm's parent panel doesn't watch a churning dir.
- **Launch the binary directly** as the tmux session command (not via
  send-keys into a login shell, which an Atuin/history widget intercepts).
- Unique per-section session/socket names; `kill-session` + `rm -f $SOCK`
  before `new-session`; per-section env files, never a shared one.
- Assert log lines via the **socket `log 200`** queried right after the action
  (TRACE verbosity floods short windows; the on-screen widget expires in 10s).
- `send-keys -l <text>` for literals that collide with tmux key names
  (`delete`, `space`, `up`, `home`, `tab`, …).
- Reverse-video matches BOTH `[7m` and `[0;7m`; use `capture-pane -e` for SGR.
- Don't mis-assert empty-panel sentinels (`preview_path == path-of-empty-panel`)
  or the header, which shows the *selection* path, not cwd.

## What worked / what to improve (run 1 evaluation)

**Worked:** parallel fresh-context executors found 3 genuine bugs in one pass
with a 98.5% step pass rate; schema-enforced structured returns made
aggregation trivial; the double assertion (socket + screen) caught all three
bugs as render/state-class mismatches, which pointed straight at the fix site.

**Improved for next run:** the biggest friction was fixtures under `/tmp`
(watcher churn breaking idle-seq and log assertions) and a shared scratch env
file clobbered across parallel executors — both now fixed in the harness. A
future rfm enhancement that would materially help testability: a socket
`log grep <substr>` verb (assert a specific TRACE/INFO line without scanning
`log 200`). Left as a recommendation, not implemented, to avoid scope creep.
