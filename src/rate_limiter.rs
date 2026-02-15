// src/rate_limiter.rs
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;

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
}

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
