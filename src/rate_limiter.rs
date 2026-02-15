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
