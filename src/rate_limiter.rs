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
