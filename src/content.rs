use cached::{Cached, SizedCache};
use log::debug;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant, SystemTime},
};
use tokio::{sync::mpsc, task::spawn_blocking, time::sleep};
use walkdir::WalkDir;

use crate::panel::{
    DirElem, DirPanel, FilePreview, PanelContent, PanelState, PanelUpdate, PreviewPanel,
};

/// Shutdown flag
///
/// This is used to abort long running blocking tasks like `fill_cache`
pub static SHUTDOWN_FLAG: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

/// Cache that is shared by the content-manager and the panel-manager.
#[derive(Clone)]
pub struct PanelCache<Item: Clone> {
    inner: Arc<Mutex<SizedCache<PathBuf, Item>>>,
}

impl<Item: PanelContent> PanelCache<Item> {
    /// Creates a new cache with given size
    pub fn with_size(size: usize) -> Self {
        PanelCache {
            inner: Arc::new(Mutex::new(SizedCache::with_size(size))),
        }
    }

    /// Attempt to retrieve a cached value
    pub fn get(&self, path: &PathBuf) -> Option<Item> {
        self.inner.lock().cache_get(path).cloned()
    }

    /// Inserts a new key-value pair
    pub fn insert(&self, path: PathBuf, item: Item) -> Option<Item> {
        self.inner.lock().cache_set(path, item)
    }

    /// Returns the cache capacity
    pub fn capacity(&self) -> usize {
        self.inner.lock().cache_capacity().unwrap_or_default()
    }

    /// Checks if the modification time of the path differs from the
    /// modification time of the cached value.
    pub fn requires_update(&self, path: &PathBuf) -> bool {
        let path_modification = path
            .metadata()
            .and_then(|p| p.modified())
            .unwrap_or_else(|_| SystemTime::now());
        self.inner
            .lock()
            .cache_get(path)
            .map(|item| item.modified() < path_modification)
            .unwrap_or(true)
    }
}

/// Tracks the rate-limiting state for a single panel+path combination
#[derive(Debug)]
struct RateLimitState {
    /// When the last request was allowed through
    last_allowed: Instant,
    /// Whether there's a pending delayed request
    has_pending: bool,
}

/// Key for rate-limiting: combination of panel ID and directory path
type RateLimitKey = (u64, PathBuf);

/// Receives commands to parse the directory or generate a new preview.
pub struct DirManager {
    tx: mpsc::Sender<(DirPanel, PanelState)>,
    rx: mpsc::UnboundedReceiver<PanelUpdate>,
    directory_cache: PanelCache<DirPanel>,
    preview_cache: PanelCache<PreviewPanel>,
    /// Rate limit interval for updates
    rate_limit_interval: Duration,
    /// Rate limiting state per panel+path
    rate_states: HashMap<RateLimitKey, RateLimitState>,
}

/// Receives commands to parse the directory or generate a new preview.
pub struct PreviewManager {
    tx: mpsc::Sender<(PreviewPanel, PanelState)>,
    rx: mpsc::UnboundedReceiver<PanelUpdate>,
    preview_cache: PanelCache<PreviewPanel>,
    /// Rate limit interval for updates
    rate_limit_interval: Duration,
    /// Rate limiting state per panel+path
    rate_states: HashMap<RateLimitKey, RateLimitState>,
}

pub fn dir_content(path: impl AsRef<Path>) -> Vec<DirElem> {
    // read directory
    match std::fs::read_dir(path) {
        Ok(dir) => dir
            .into_iter()
            .flatten()
            .map(|p| DirElem::from(p.path()))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Walks the given directory path and fills both caches.
///
/// Since we most likely want to access a directory that the cursor went over,
/// it is smart to prepare the cache here. This allows us to be as fast as possible
/// with the generated previews.
fn fill_cache(
    path: PathBuf,
    directory_cache: PanelCache<DirPanel>,
    preview_cache: PanelCache<PreviewPanel>,
) {
    if !path.is_dir() {
        return;
    }
    let file_capacity = preview_cache.capacity() / 16;
    let dir_capacity = directory_cache.capacity() / 16;
    let mut n_dir_previews = 0;
    let mut n_file_previews = 0;
    for entry in WalkDir::new(&path).max_depth(2).into_iter().flatten() {
        if entry.file_type().is_dir() && n_dir_previews < dir_capacity {
            let dir_path = entry.into_path();
            if directory_cache.requires_update(&dir_path) {
                let content = dir_content(&dir_path);
                let panel = DirPanel::new(content, dir_path.clone());
                directory_cache.insert(dir_path.clone(), panel.clone());
                preview_cache.insert(dir_path, PreviewPanel::Dir(panel));
                n_dir_previews += 1;
            }
        } else if entry.file_type().is_file()
            && entry.depth() == 1
            && n_file_previews < file_capacity
        {
            let file_path = entry.into_path();
            if preview_cache.requires_update(&file_path) {
                let preview = FilePreview::new(file_path.clone());
                preview_cache.insert(file_path, PreviewPanel::File(preview));
                n_file_previews += 1;
            }
        }
        // If we reached the max capacity that we want to fill the cache up with,
        // stop traversing the directory any further.
        if n_dir_previews >= dir_capacity && n_file_previews >= file_capacity {
            break;
        }

        if SHUTDOWN_FLAG.load(std::sync::atomic::Ordering::Relaxed) {
            debug!("Shutdown requested");
            break;
        }
    }
}

impl DirManager {
    pub fn new(
        directory_cache: PanelCache<DirPanel>,
        preview_cache: PanelCache<PreviewPanel>,
        tx: mpsc::Sender<(DirPanel, PanelState)>,
        rx: mpsc::UnboundedReceiver<PanelUpdate>,
        rate_limit_interval: Duration,
    ) -> Self {
        DirManager {
            tx,
            rx,
            directory_cache,
            preview_cache,
            rate_limit_interval,
            rate_states: HashMap::new(),
        }
    }

    /// Check if a request should be rate-limited.
    /// Returns: (should_process_now, should_schedule_delayed)
    fn check_rate_limit(&mut self, key: &RateLimitKey) -> (bool, bool) {
        let now = Instant::now();

        match self.rate_states.get_mut(key) {
            None => {
                // First request for this key - allow immediately
                self.rate_states.insert(
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
                if elapsed >= self.rate_limit_interval {
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
        if let Some(state) = self.rate_states.get_mut(key) {
            state.last_allowed = Instant::now();
            state.has_pending = false;
        }
    }

    /// Process a single update request
    async fn process_update(&mut self, update: PanelUpdate, last_cache_path: &mut PathBuf) {
        if !update.state.path().is_dir() {
            return;
        }
        let dir_path = update.state.path().clone();
        debug!("request new dir-panel for {}", dir_path.display());
        let result = spawn_blocking(move || dir_content(dir_path)).await;
        if let Ok(content) = result {
            let panel = DirPanel::new(content, update.state.path().clone());
            if let Err(e) = self
                .tx
                .send((panel.clone(), update.state.increased().increased()))
                .await
            {
                debug!("Cannot send panel-update: {e}");
                return;
            };
            self.directory_cache
                .insert(update.state.path().clone(), panel.clone());
            self.preview_cache
                .insert(update.state.path().clone(), PreviewPanel::Dir(panel));
        }
        if update.state.path() != last_cache_path.as_path() {
            *last_cache_path = update.state.path().to_path_buf();
            let path = update.state.path();
            let dir_cache = self.directory_cache.clone();
            let prev_cache = self.preview_cache.clone();
            tokio::task::spawn_blocking(move || fill_cache(path, dir_cache, prev_cache));
        }
    }

    pub async fn run(mut self) {
        let mut last_cache_path = PathBuf::default();
        // Channel for delayed request notifications
        let (delay_tx, mut delay_rx) =
            mpsc::unbounded_channel::<(RateLimitKey, PanelUpdate)>();

        loop {
            tokio::select! {
                // Handle incoming requests
                Some(update) = self.rx.recv() => {
                    let key: RateLimitKey = (
                        update.state.panel_id(),
                        update.state.path().to_path_buf(),
                    );

                    let (process_now, schedule_delayed) = self.check_rate_limit(&key);

                    if process_now {
                        self.process_update(update, &mut last_cache_path).await;
                    } else if schedule_delayed {
                        // Spawn a delayed task
                        let delay_tx = delay_tx.clone();
                        let interval = self.rate_limit_interval;
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
                    self.process_update(update, &mut last_cache_path).await;
                }

                else => break,
            }
        }
    }
}

impl PreviewManager {
    pub fn new(
        preview_cache: PanelCache<PreviewPanel>,
        tx: mpsc::Sender<(PreviewPanel, PanelState)>,
        rx: mpsc::UnboundedReceiver<PanelUpdate>,
        rate_limit_interval: Duration,
    ) -> Self {
        PreviewManager {
            tx,
            rx,
            preview_cache,
            rate_limit_interval,
            rate_states: HashMap::new(),
        }
    }

    /// Check if a request should be rate-limited.
    /// Returns: (should_process_now, should_schedule_delayed)
    fn check_rate_limit(&mut self, key: &RateLimitKey) -> (bool, bool) {
        let now = Instant::now();

        match self.rate_states.get_mut(key) {
            None => {
                // First request for this key - allow immediately
                self.rate_states.insert(
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
                if elapsed >= self.rate_limit_interval {
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
        if let Some(state) = self.rate_states.get_mut(key) {
            state.last_allowed = Instant::now();
            state.has_pending = false;
        }
    }

    /// Process a single update request
    async fn process_update(&mut self, update: PanelUpdate) {
        if update.state.path().is_dir() {
            let dir_path = update.state.path().clone();
            let result = spawn_blocking(move || dir_content(dir_path)).await;
            if let Ok(content) = result {
                let panel =
                    PreviewPanel::Dir(DirPanel::new(content, update.state.path().clone()));
                if let Err(e) = self
                    .tx
                    .send((panel.clone(), update.state.increased()))
                    .await
                {
                    debug!("Cannot send panel-update: {e}");
                    return;
                }
                self.preview_cache.insert(update.state.path(), panel);
            }
        } else {
            // Create preview
            let file_path = update.state.path().clone();
            let result = spawn_blocking(move || FilePreview::new(file_path)).await;
            if let Ok(preview) = result {
                let panel = PreviewPanel::File(preview);
                if let Err(e) = self
                    .tx
                    .send((panel.clone(), update.state.increased()))
                    .await
                {
                    debug!("Cannot send panel-update: {e}");
                    return;
                }
                self.preview_cache.insert(update.state.path(), panel);
            }
        }
    }

    pub async fn run(mut self) {
        // Channel for delayed request notifications
        let (delay_tx, mut delay_rx) =
            mpsc::unbounded_channel::<(RateLimitKey, PanelUpdate)>();

        loop {
            tokio::select! {
                // Handle incoming requests
                Some(update) = self.rx.recv() => {
                    let key: RateLimitKey = (
                        update.state.panel_id(),
                        update.state.path().to_path_buf(),
                    );

                    let (process_now, schedule_delayed) = self.check_rate_limit(&key);

                    if process_now {
                        self.process_update(update).await;
                    } else if schedule_delayed {
                        // Spawn a delayed task
                        let delay_tx = delay_tx.clone();
                        let interval = self.rate_limit_interval;
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
                    self.process_update(update).await;
                }

                else => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper that implements the same rate-limiting logic as the managers
    struct RateLimitTester {
        rate_limit_interval: Duration,
        rate_states: HashMap<RateLimitKey, RateLimitState>,
    }

    impl RateLimitTester {
        fn new(interval: Duration) -> Self {
            Self {
                rate_limit_interval: interval,
                rate_states: HashMap::new(),
            }
        }

        fn check_rate_limit(&mut self, key: &RateLimitKey) -> (bool, bool) {
            let now = Instant::now();

            match self.rate_states.get_mut(key) {
                None => {
                    self.rate_states.insert(
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
                    if elapsed >= self.rate_limit_interval {
                        state.last_allowed = now;
                        state.has_pending = false;
                        (true, false)
                    } else if state.has_pending {
                        (false, false)
                    } else {
                        state.has_pending = true;
                        (false, true)
                    }
                }
            }
        }

        fn mark_delayed_executed(&mut self, key: &RateLimitKey) {
            if let Some(state) = self.rate_states.get_mut(key) {
                state.last_allowed = Instant::now();
                state.has_pending = false;
            }
        }
    }

    #[test]
    fn test_first_request_allowed() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key: RateLimitKey = (1, PathBuf::from("/test"));

        let (send_now, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(send_now, "First request should be allowed immediately");
        assert!(!schedule_delayed, "First request should not schedule delayed");
    }

    #[test]
    fn test_rapid_request_schedules_delayed() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key: RateLimitKey = (1, PathBuf::from("/test"));

        // First request
        tester.check_rate_limit(&key);

        // Immediate second request
        let (send_now, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(!send_now, "Rapid second request should not send immediately");
        assert!(schedule_delayed, "Rapid second request should schedule delayed");
    }

    #[test]
    fn test_third_rapid_request_dropped() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key: RateLimitKey = (1, PathBuf::from("/test"));

        // First request - allowed
        tester.check_rate_limit(&key);

        // Second request - schedules delayed
        tester.check_rate_limit(&key);

        // Third request - should be dropped
        let (send_now, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(!send_now, "Third rapid request should not send immediately");
        assert!(!schedule_delayed, "Third rapid request should be dropped");
    }

    #[test]
    fn test_different_panels_independent() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key1: RateLimitKey = (1, PathBuf::from("/test"));
        let key2: RateLimitKey = (2, PathBuf::from("/test"));

        // First panel
        let (send1, _) = tester.check_rate_limit(&key1);
        assert!(send1);

        // Different panel - should also be allowed
        let (send2, _) = tester.check_rate_limit(&key2);
        assert!(send2, "Different panel should be allowed independently");
    }

    #[test]
    fn test_different_paths_independent() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key1: RateLimitKey = (1, PathBuf::from("/test1"));
        let key2: RateLimitKey = (1, PathBuf::from("/test2"));

        // First path
        let (send1, _) = tester.check_rate_limit(&key1);
        assert!(send1);

        // Different path - should also be allowed
        let (send2, _) = tester.check_rate_limit(&key2);
        assert!(send2, "Different path should be allowed independently");
    }

    #[test]
    fn test_request_after_interval_allowed() {
        let mut tester = RateLimitTester::new(Duration::from_millis(10));
        let key: RateLimitKey = (1, PathBuf::from("/test"));

        // First request
        tester.check_rate_limit(&key);

        // Wait for interval to pass
        std::thread::sleep(Duration::from_millis(15));

        // Second request after interval
        let (send_now, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(send_now, "Request after interval should be allowed");
        assert!(!schedule_delayed);
    }

    #[test]
    fn test_mark_delayed_executed_resets_state() {
        let mut tester = RateLimitTester::new(Duration::from_millis(100));
        let key: RateLimitKey = (1, PathBuf::from("/test"));

        // First request
        tester.check_rate_limit(&key);

        // Second request - schedules delayed
        let (_, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(schedule_delayed);

        // Mark delayed as executed
        tester.mark_delayed_executed(&key);

        // Third request - should schedule delayed again (not drop)
        let (send_now, schedule_delayed) = tester.check_rate_limit(&key);
        assert!(!send_now);
        assert!(schedule_delayed, "After delayed executed, new request should schedule delayed again");
    }
}
