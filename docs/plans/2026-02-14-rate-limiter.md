# Rate Limiter Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement a rate-limiter for the preview-manager that coalesces rapid filesystem events into controlled update intervals, replacing the freeze/unfreeze pattern.

**Architecture:** Create a `RateLimiter` struct that tracks per-panel+directory update requests using a HashMap. Each key maps to a state tracking last execution time and pending delayed request. The limiter intercepts requests before they reach the managers, allowing immediate execution if within rate limit, scheduling delayed execution if rate-limited, or dropping redundant requests if a delayed request is already pending.

**Tech Stack:** Rust, tokio (async runtime, timers, channels), std::collections::HashMap, std::time::Instant

---

## Background

### Current Flow
```
FileWatcher → PanelUpdate { state } → unbounded_channel → PreviewManager/DirManager → spawn task
```

### New Flow with Rate Limiter
```
FileWatcher → PanelUpdate { state } → RateLimiter → (rate-limited) → unbounded_channel → PreviewManager/DirManager
```

### Key Files
- `src/content.rs` - PreviewManager & DirManager (will receive rate-limited channel)
- `src/panel/mod.rs` - ManagedPanel sends updates, watcher callback (freeze logic to remove)
- `src/panel/manager.rs` - PanelManager (freeze calls to remove)
- `src/main.rs` - Channel setup, task spawning (will spawn rate-limiter)

### Rate Limiting Behavior
Given rate limit interval X (e.g., 500ms):
1. **First request for (panel_id, path):** Execute immediately
2. **Subsequent request within X seconds:** Schedule for execution at (first_time + X), mark as pending
3. **More requests while one is pending:** Drop (the pending one will cover them)
4. **After X seconds:** Execute pending request if any, reset state

---

## Task 1: Create RateLimiter Module

**Files:**
- Create: `src/rate_limiter.rs`
- Modify: `src/lib.rs` or `src/main.rs` (add module declaration)

**Step 1: Create the rate_limiter module file with basic structure**

```rust
// src/rate_limiter.rs
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::panel::PanelUpdate;

/// Key for rate-limiting: combination of panel ID and directory path
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitKey {
    panel_id: u64,
    path: std::path::PathBuf,
}

impl RateLimitKey {
    pub fn new(panel_id: u64, path: std::path::PathBuf) -> Self {
        Self { panel_id, path }
    }
}

/// Tracks the rate-limiting state for a single key
#[derive(Debug)]
struct RateLimitState {
    /// When the last request was allowed through
    last_allowed: Instant,
    /// Whether there's a pending delayed request
    has_pending: bool,
}

impl RateLimitState {
    fn new() -> Self {
        Self {
            last_allowed: Instant::now(),
            has_pending: false,
        }
    }
}

/// Rate limiter for panel update requests
pub struct RateLimiter {
    /// Minimum interval between updates for the same panel+path
    interval: Duration,
    /// Tracking state per panel+path combination
    states: HashMap<RateLimitKey, RateLimitState>,
    /// Channel to send rate-limited requests downstream
    tx: mpsc::UnboundedSender<PanelUpdate>,
}
```

**Step 2: Run `cargo check` to verify syntax**

Run: `cargo check 2>&1 | head -50`
Expected: Errors about missing module declaration (we'll fix next)

**Step 3: Add module declaration to main.rs**

In `src/main.rs`, add after other mod declarations:
```rust
mod rate_limiter;
```

**Step 4: Run `cargo check` again**

Run: `cargo check 2>&1 | head -50`
Expected: Errors about incomplete implementation (expected at this stage)

**Step 5: Commit**

```bash
git add src/rate_limiter.rs src/main.rs
git commit -m "feat(rate-limiter): add basic module structure

Introduces RateLimitKey and RateLimitState structs for tracking
per-panel+path rate limiting state.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Implement RateLimiter Core Logic

**Files:**
- Modify: `src/rate_limiter.rs`

**Step 1: Add the decision logic method**

Add to `RateLimiter` impl block in `src/rate_limiter.rs`:

```rust
impl RateLimiter {
    pub fn new(interval: Duration, tx: mpsc::UnboundedSender<PanelUpdate>) -> Self {
        Self {
            interval,
            states: HashMap::new(),
            tx,
        }
    }

    /// Process an incoming update request.
    /// Returns: (should_send_now, should_schedule_delayed)
    fn check_rate_limit(&mut self, key: &RateLimitKey) -> (bool, bool) {
        let now = Instant::now();

        match self.states.get_mut(key) {
            None => {
                // First request for this key - allow immediately
                self.states.insert(
                    key.clone(),
                    RateLimitState {
                        last_allowed: now,
                        has_pending: false,
                    },
                );
                (true, false)
            }
            Some(state) => {
                let elapsed = now.duration_since(state.last_allowed);
                if elapsed >= self.interval {
                    // Enough time has passed - allow immediately
                    state.last_allowed = now;
                    state.has_pending = false;
                    (true, false)
                } else if state.has_pending {
                    // Already have a pending request - drop this one
                    (false, false)
                } else {
                    // Rate limited but no pending - schedule delayed
                    state.has_pending = true;
                    (false, true)
                }
            }
        }
    }

    /// Mark that a delayed request has been executed
    fn mark_delayed_executed(&mut self, key: &RateLimitKey) {
        if let Some(state) = self.states.get_mut(key) {
            state.last_allowed = Instant::now();
            state.has_pending = false;
        }
    }
}
```

**Step 2: Run `cargo check`**

Run: `cargo check 2>&1 | head -50`
Expected: Should compile (or minor fixable errors)

**Step 3: Commit**

```bash
git add src/rate_limiter.rs
git commit -m "feat(rate-limiter): implement core rate-limiting logic

Adds check_rate_limit() method that determines whether to:
- Allow request immediately (first request or interval elapsed)
- Schedule delayed request (rate-limited, no pending)
- Drop request (rate-limited with pending already)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Implement Async Processing Loop

**Files:**
- Modify: `src/rate_limiter.rs`

**Step 1: Add the async run method**

Add imports at top of `src/rate_limiter.rs`:
```rust
use tokio::time::sleep;
```

Add to `RateLimiter` impl block:

```rust
    /// Run the rate limiter, processing incoming requests
    pub async fn run(
        mut self,
        mut rx: mpsc::UnboundedReceiver<PanelUpdate>,
    ) {
        // Channel for delayed request notifications
        let (delay_tx, mut delay_rx) = mpsc::unbounded_channel::<(RateLimitKey, PanelUpdate)>();

        loop {
            tokio::select! {
                // Handle incoming requests
                Some(update) = rx.recv() => {
                    let key = RateLimitKey::new(
                        update.state.panel_id(),
                        update.state.path().to_path_buf(),
                    );

                    let (send_now, schedule_delayed) = self.check_rate_limit(&key);

                    if send_now {
                        let _ = self.tx.send(update);
                    } else if schedule_delayed {
                        // Spawn a delayed send task
                        let delay_tx = delay_tx.clone();
                        let interval = self.interval;
                        let key_clone = key.clone();
                        let update_clone = update.clone();

                        tokio::spawn(async move {
                            sleep(interval).await;
                            let _ = delay_tx.send((key_clone, update_clone));
                        });
                    }
                    // else: drop the request (has_pending was true)
                }

                // Handle delayed request execution
                Some((key, update)) = delay_rx.recv() => {
                    self.mark_delayed_executed(&key);
                    let _ = self.tx.send(update);
                }

                else => break,
            }
        }
    }
```

**Step 2: Add Clone derive to PanelUpdate**

In `src/panel/mod.rs`, find `PanelUpdate` struct and add Clone:
```rust
#[derive(Clone)]
pub struct PanelUpdate {
    pub state: PanelState,
}
```

Also ensure `PanelState` has Clone (it likely does).

**Step 3: Add public accessor for panel_id in PanelState**

In `src/panel/mod.rs`, add to `PanelState` impl if not present:
```rust
pub fn panel_id(&self) -> u64 {
    self.panel_id
}
```

**Step 4: Run `cargo check`**

Run: `cargo check 2>&1 | head -50`
Expected: Should compile

**Step 5: Commit**

```bash
git add src/rate_limiter.rs src/panel/mod.rs
git commit -m "feat(rate-limiter): implement async processing loop

Adds run() method that:
- Receives incoming PanelUpdate requests
- Applies rate-limiting logic
- Spawns delayed tasks for scheduled requests
- Forwards allowed requests to downstream channel

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Integrate RateLimiter into main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Add configuration constant**

Near the top of `src/main.rs`, add:
```rust
use std::time::Duration;

/// Rate limit interval for preview updates (in milliseconds)
const RATE_LIMIT_INTERVAL_MS: u64 = 500;
```

**Step 2: Modify channel setup to insert rate limiter**

Find the channel setup section (around line 224-228). Change from:

```rust
let (preview_tx, preview_rx) = mpsc::unbounded_channel();
let (directory_tx, directory_rx) = mpsc::unbounded_channel();
```

To:

```rust
// Channels from rate-limiters to managers
let (preview_tx, preview_rx) = mpsc::unbounded_channel();
let (directory_tx, directory_rx) = mpsc::unbounded_channel();

// Channels from panels to rate-limiters
let (preview_input_tx, preview_input_rx) = mpsc::unbounded_channel();
let (directory_input_tx, directory_input_rx) = mpsc::unbounded_channel();

// Create rate limiters
let rate_limit_interval = Duration::from_millis(RATE_LIMIT_INTERVAL_MS);
let preview_rate_limiter = rate_limiter::RateLimiter::new(rate_limit_interval, preview_tx);
let directory_rate_limiter = rate_limiter::RateLimiter::new(rate_limit_interval, directory_tx);
```

**Step 3: Spawn rate limiter tasks**

After the rate limiter creation, add:

```rust
// Spawn rate limiter tasks
tokio::spawn(preview_rate_limiter.run(preview_input_rx));
tokio::spawn(directory_rate_limiter.run(directory_input_rx));
```

**Step 4: Update references to use input channels**

Find where `preview_tx` and `directory_tx` are passed to `ManagedPanel` or other components that send updates. Change these to use `preview_input_tx` and `directory_input_tx` instead.

Look for code like:
```rust
ManagedPanel::new(..., preview_tx.clone(), ...)
```

Change to:
```rust
ManagedPanel::new(..., preview_input_tx.clone(), ...)
```

**Step 5: Run `cargo check`**

Run: `cargo check 2>&1 | head -50`
Expected: Should compile

**Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(rate-limiter): integrate rate limiters into startup

Inserts rate limiters between panel update senders and managers.
Panels now send to rate limiter input channels, which forward
to manager channels after applying rate limiting.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Remove Freeze Logic from ManagedPanel

**Files:**
- Modify: `src/panel/mod.rs`

**Step 1: Remove freeze and unfreeze methods**

In `src/panel/mod.rs`, find and remove the `freeze()` and `unfreeze()` methods from `ManagedPanel` impl (around lines 372-386):

```rust
// DELETE these methods:
pub fn freeze(&mut self) {
    unwatch_path(&mut self.watcher, self.panel.path());
}

pub fn unfreeze(&mut self) {
    watch_path(&mut self.watcher, self.panel.path());
    self.reload();
}
```

**Step 2: Run `cargo check` to find call sites**

Run: `cargo check 2>&1 | head -100`
Expected: Errors showing where freeze/unfreeze are called

**Step 3: Commit (partial - method removal)**

```bash
git add src/panel/mod.rs
git commit -m "refactor(panel): remove freeze/unfreeze methods

Rate limiter now handles burst protection, making the freeze
pattern obsolete. Watchers remain active during all operations.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Remove Freeze Calls from PanelManager

**Files:**
- Modify: `src/panel/manager.rs`

**Step 1: Remove freeze/unfreeze calls around external app opening**

Find around line 629:
```rust
self.center.freeze();
```
And around line 639:
```rust
self.center.unfreeze();
```

Remove both calls (keep the surrounding code).

**Step 2: Remove freeze/unfreeze calls around zip creation**

Find around lines 1048 and 1052:
```rust
self.center.freeze();
// ... zip creation code ...
self.center.unfreeze();
```

Remove both calls.

**Step 3: Remove freeze/unfreeze calls around tar creation**

Find around lines 1060 and 1064:
```rust
self.center.freeze();
// ... tar creation code ...
self.center.unfreeze();
```

Remove both calls.

**Step 4: Remove freeze/unfreeze calls around archive extraction**

Find around lines 1068 and 1082:
```rust
self.center.freeze();
// ... extraction code ...
self.center.unfreeze();
```

Remove both calls.

**Step 5: Run `cargo check`**

Run: `cargo check 2>&1 | head -50`
Expected: Should compile without errors

**Step 6: Run `cargo build`**

Run: `cargo build 2>&1 | tail -20`
Expected: Build succeeds

**Step 7: Commit**

```bash
git add src/panel/manager.rs
git commit -m "refactor(manager): remove all freeze/unfreeze calls

With rate limiter in place, freeze/unfreeze pattern is no longer
needed. Watchers stay active during blocking operations and
the rate limiter prevents excessive updates.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Add Configuration for Rate Limit Interval

**Files:**
- Modify: `src/config.rs` (or equivalent config file)
- Modify: `src/main.rs`

**Step 1: Find config structure**

Run: `grep -r "struct.*Config" src/ --include="*.rs" | head -10`

Identify the main configuration struct.

**Step 2: Add rate_limit_interval field to config**

In the config struct, add:
```rust
/// Rate limit interval for preview updates in milliseconds
#[serde(default = "default_rate_limit_interval")]
pub rate_limit_interval_ms: u64,
```

Add default function:
```rust
fn default_rate_limit_interval() -> u64 {
    500
}
```

**Step 3: Update main.rs to use config value**

Replace the constant with config lookup:
```rust
let rate_limit_interval = Duration::from_millis(config.rate_limit_interval_ms);
```

**Step 4: Run `cargo check`**

Run: `cargo check 2>&1 | head -50`
Expected: Should compile

**Step 5: Commit**

```bash
git add src/config.rs src/main.rs
git commit -m "feat(config): add rate_limit_interval_ms setting

Users can now configure the rate limit interval in their config.
Defaults to 500ms if not specified.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Write Unit Tests for RateLimiter

**Files:**
- Modify: `src/rate_limiter.rs` (add tests module)

**Step 1: Add test module to rate_limiter.rs**

At the end of `src/rate_limiter.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_first_request_allowed() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key = RateLimitKey::new(1, PathBuf::from("/test"));

        let (send_now, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(send_now, "First request should be allowed immediately");
        assert!(!schedule_delayed, "First request should not schedule delayed");
    }

    #[test]
    fn test_rapid_request_schedules_delayed() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key = RateLimitKey::new(1, PathBuf::from("/test"));

        // First request
        limiter.check_rate_limit(&key);

        // Immediate second request
        let (send_now, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(!send_now, "Rapid second request should not send immediately");
        assert!(schedule_delayed, "Rapid second request should schedule delayed");
    }

    #[test]
    fn test_third_rapid_request_dropped() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key = RateLimitKey::new(1, PathBuf::from("/test"));

        // First request - allowed
        limiter.check_rate_limit(&key);

        // Second request - schedules delayed
        limiter.check_rate_limit(&key);

        // Third request - should be dropped
        let (send_now, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(!send_now, "Third rapid request should not send immediately");
        assert!(!schedule_delayed, "Third rapid request should be dropped (not schedule another delayed)");
    }

    #[test]
    fn test_different_panels_independent() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key1 = RateLimitKey::new(1, PathBuf::from("/test"));
        let key2 = RateLimitKey::new(2, PathBuf::from("/test"));

        // First panel
        let (send1, _) = limiter.check_rate_limit(&key1);
        assert!(send1);

        // Different panel - should also be allowed
        let (send2, _) = limiter.check_rate_limit(&key2);
        assert!(send2, "Different panel should be allowed independently");
    }

    #[test]
    fn test_different_paths_independent() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key1 = RateLimitKey::new(1, PathBuf::from("/test1"));
        let key2 = RateLimitKey::new(1, PathBuf::from("/test2"));

        // First path
        let (send1, _) = limiter.check_rate_limit(&key1);
        assert!(send1);

        // Different path - should also be allowed
        let (send2, _) = limiter.check_rate_limit(&key2);
        assert!(send2, "Different path should be allowed independently");
    }

    #[test]
    fn test_request_after_interval_allowed() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(10), tx);
        let key = RateLimitKey::new(1, PathBuf::from("/test"));

        // First request
        limiter.check_rate_limit(&key);

        // Wait for interval to pass
        std::thread::sleep(Duration::from_millis(15));

        // Second request after interval
        let (send_now, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(send_now, "Request after interval should be allowed");
        assert!(!schedule_delayed);
    }

    #[test]
    fn test_mark_delayed_executed_resets_state() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut limiter = RateLimiter::new(Duration::from_millis(100), tx);
        let key = RateLimitKey::new(1, PathBuf::from("/test"));

        // First request
        limiter.check_rate_limit(&key);

        // Second request - schedules delayed
        let (_, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(schedule_delayed);

        // Mark delayed as executed
        limiter.mark_delayed_executed(&key);

        // Third request - should schedule delayed again (not drop)
        let (send_now, schedule_delayed) = limiter.check_rate_limit(&key);
        assert!(!send_now);
        assert!(schedule_delayed, "After delayed executed, new request should schedule delayed again");
    }
}
```

**Step 2: Run the tests**

Run: `cargo test rate_limiter --lib -- --nocapture`
Expected: All tests pass

**Step 3: Commit**

```bash
git add src/rate_limiter.rs
git commit -m "test(rate-limiter): add comprehensive unit tests

Tests cover:
- First request allowed immediately
- Rapid requests schedule delayed execution
- Redundant requests dropped when pending exists
- Different panels/paths rate-limited independently
- Requests allowed after interval elapses
- State reset after delayed execution

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 9: Add Integration Test

**Files:**
- Create: `tests/rate_limiter_integration.rs`

**Step 1: Create integration test file**

```rust
// tests/rate_limiter_integration.rs
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};

// Note: This test requires the rate_limiter module to be accessible.
// If rfm is a binary crate, you may need to restructure to lib+bin
// or test via the binary itself.

#[tokio::test]
async fn test_rate_limiter_allows_first_request() {
    // This is a placeholder - actual test depends on crate structure
    // See Task 10 for manual testing approach
}
```

**Step 2: Document manual testing approach**

Since this is a TUI application, add to `rate-limiter.md` or create `docs/testing-rate-limiter.md`:

```markdown
## Testing the Rate Limiter

### Manual Test Procedure

1. Start the application: `cargo run`
2. Navigate to a directory with many files
3. Open a terminal in that directory
4. Run a command that creates many files rapidly:
   ```bash
   for i in {1..100}; do touch "testfile_$i.txt"; done
   ```
5. Observe the panel - it should update smoothly every ~500ms, not 100 times
6. Clean up: `rm testfile_*.txt`

### Verify with Debug Logging

Add temporary debug output to rate_limiter.rs:
```rust
if send_now {
    eprintln!("[RATE] Allowing request for {:?}", key);
} else if schedule_delayed {
    eprintln!("[RATE] Scheduling delayed for {:?}", key);
} else {
    eprintln!("[RATE] Dropping request for {:?}", key);
}
```

Run with: `cargo run 2> /tmp/rate-limit.log`
Then check the log to see rate limiting in action.
```

**Step 3: Run `cargo test`**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests pass

**Step 4: Commit**

```bash
git add tests/ docs/
git commit -m "test(rate-limiter): add integration test structure and manual test docs

Documents how to manually verify rate limiting behavior in the TUI
and how to add debug logging for verification.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 10: Final Verification and Cleanup

**Files:**
- Review all modified files

**Step 1: Run full test suite**

Run: `cargo test 2>&1`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cargo clippy 2>&1 | head -50`
Expected: No errors (warnings acceptable)

**Step 3: Build release**

Run: `cargo build --release 2>&1 | tail -10`
Expected: Build succeeds

**Step 4: Manual smoke test**

Run: `cargo run`
Expected: Application starts normally, panels update correctly

**Step 5: Verify rate limiting works**

In another terminal:
```bash
cd <directory shown in rfm>
for i in {1..50}; do touch "test_$i"; done
rm test_*
```
Expected: Panel updates smoothly without lag

**Step 6: Final commit**

```bash
git add -A
git commit -m "feat(rate-limiter): complete implementation

Rate limiter replaces freeze/unfreeze pattern:
- Allows immediate first request per panel+path
- Schedules delayed request if rate-limited
- Drops redundant requests when pending exists
- Configurable interval (default 500ms)

Removes all freeze/unfreeze calls from PanelManager.
Includes comprehensive unit tests.

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Summary

| Task | Description | Files |
|------|-------------|-------|
| 1 | Create RateLimiter module structure | `src/rate_limiter.rs`, `src/main.rs` |
| 2 | Implement core rate-limiting logic | `src/rate_limiter.rs` |
| 3 | Implement async processing loop | `src/rate_limiter.rs`, `src/panel/mod.rs` |
| 4 | Integrate into main.rs | `src/main.rs` |
| 5 | Remove freeze/unfreeze from ManagedPanel | `src/panel/mod.rs` |
| 6 | Remove freeze calls from PanelManager | `src/panel/manager.rs` |
| 7 | Add configuration option | `src/config.rs`, `src/main.rs` |
| 8 | Write unit tests | `src/rate_limiter.rs` |
| 9 | Add integration test docs | `tests/`, `docs/` |
| 10 | Final verification | All files |
