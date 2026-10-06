# Bug report template

One report per bug, appended to the run report. Capture evidence **while the
tmux session still exists** — pane captures and socket history are gone after
teardown.

```markdown
## BUG-<run>-<nn> — <one-line title>

- **Protocol step:** <section>.<step> (e.g. 03.4)
- **Severity:** crash | data-loss | wrong-behavior | visual | papercut
- **Class:** state (socket wrong) | render (socket right, screen stale) |
  both | wedge (state times out)

### Repro (minimal)
Exact commands from a fresh harness — trimmed to the shortest sequence that
still reproduces. Verify the minimal repro actually reproduces before filing.

### Expected
What the protocol step expected (socket AND screen).

### Actual
- socket `state` (relevant fields) at failure:
- `log 50` excerpt (relevant lines, with age_secs):
- captured pane (trimmed to the relevant region):

### Notes
Flakiness (did it reproduce twice?), suspected area (file:line if obvious from
the log trail), related steps that also failed.
```

Severity guide: `data-loss` covers anything that destroys or corrupts user
files including a wrong undo; `wrong-behavior` is functional but incorrect;
`visual` is correct state rendered wrongly; `papercut` is cosmetic.

Rules:
- File **state vs render** correctly: `entries` correct + screen stale →
  render class; `entries` also wrong → state class. This routes the fix.
- One bug per report; if two steps fail from one root cause, one report,
  list both steps.
- Reproduce twice before filing as reliable; file once-only failures too,
  marked flaky.
