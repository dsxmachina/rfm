use crossterm::{
    cursor, queue,
    style::{self, Print, PrintStyledContent, Stylize},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use log::{debug, error, info, warn};
use notify::{RecommendedWatcher, Watcher};
use parking_lot::Mutex;
use std::{
    cmp::Ordering,
    fs::canonicalize,
    io::{stdout, Stdout, Write},
    ops::Range,
    os::unix::prelude::PermissionsExt,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::sync::mpsc;

use crate::{commands::Move, content::PanelCache};

mod console;
mod directory;
pub mod manager;
mod preview;
mod input;

pub use directory::{DirElem, DirPanel};
pub use preview::{FilePreview, Preview, PreviewPanel};

/// Basic trait that lets us draw something on the terminal in a specified range.
pub trait Draw {
    fn draw(&mut self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>)
        -> Result<()>;
}

/// Basic trait for managing the content of a panel
pub trait PanelContent: Draw + Clone + Send {
    /// Path of the panel
    fn path(&self) -> &Path;

    /// Access time of the path
    fn modified(&self) -> SystemTime;

    /// Updates the content of the panel
    fn update_content(&mut self, content: Self);
}

/// Basic trait for our panels.
pub trait BasePanel: PanelContent {
    /// Creates an empty panel without content
    fn empty() -> Self;

    /// Creates a temporary panel to indicate that we are still loading
    /// some data
    fn loading(path: PathBuf) -> Self;

    /// Creates a panel from some path
    fn from_path(path: PathBuf) -> Self;
}

#[derive(Debug, Clone)]
pub struct PanelState {
    /// ID of the panel that is managed by the updater.
    ///
    /// The ID is generated randomly upon creation.
    /// When we send an update request to the [`ContentManager`], we attach the ID
    /// to the request, so that the [`PanelManager`] is able to know which panel needs to be updated.
    panel_id: u64,

    /// Counter that increases everytime we update the panel.
    ///
    /// This prevents the manager from accidently overwriting the panel with older content
    /// that was requested before some other content, that is displayed now.
    /// Since the [`ContentManager`] works asynchronously we need this mechanism,
    /// because there is no guarantee that requests that were sent earlier,
    /// will also finish earlier.
    pub cnt: u64, // TODO: remove pub

    /// Path of the panel
    path: PathBuf,
}

impl Default for PanelState {
    fn default() -> Self {
        // Generate a random id here - because we only have three panels,
        // the chance of collision is pretty low.
        Self {
            panel_id: rand::random(),
            cnt: 0,
            path: PathBuf::default(),
        }
    }
}

impl PanelState {
    pub fn increase(&mut self) {
        self.cnt += 1;
    }

    pub fn increased(&self) -> Self {
        PanelState {
            panel_id: self.panel_id,
            cnt: self.cnt + 1,
            path: self.path.clone(),
        }
    }

    /// Returns `true` if the incoming panel-state:
    /// - has the same id
    /// - has a higher counter
    ///
    /// Otherwise it will return `false`.
    pub fn check_update(&self, other: &PanelState) -> bool {
        if self.panel_id == other.panel_id {
            self.cnt < other.cnt
        } else {
            false
        }
    }

    pub fn id(&self) -> u64 {
        self.panel_id
    }

    pub fn path(&self) -> PathBuf {
        self.path.clone()
    }
}

// Helper function to call 'unwatch' on some watcher
fn unwatch_path<P: AsRef<Path>>(watcher: &mut RecommendedWatcher, path: P) {
    let path = path.as_ref();
    if path.exists() && path.is_dir() {
        match watcher.unwatch(path) {
            Ok(_) => {
                debug!("unwatching {}", path.display());
            }
            Err(e) => {
                warn!("unwatch-error: {}", e);
            }
        }
    }
}

// Helper function to call 'watch' on some watcher
fn watch_path<P: AsRef<Path>>(watcher: &mut RecommendedWatcher, path: P) {
    let path = path.as_ref();
    if path.exists() && path.is_dir() {
        match watcher.watch(path, notify::RecursiveMode::NonRecursive) {
            Ok(_) => {
                debug!("watching {}", path.display());
            }
            Err(e) => {
                warn!("watch-error: {}", e);
            }
        }
    }
}

/// Combines all data that is necessary to update a panel.
///
/// Will be send as a request to the [`ContentManager`].
#[derive(Debug)]
pub struct PanelUpdate {
    pub state: PanelState,
}

pub struct ManagedPanel<PanelType: BasePanel> {
    /// Panel to be updated.
    panel: PanelType,

    /// State counter and identifier of the managed panel
    state: Arc<Mutex<PanelState>>,

    /// File-watcher that sends update requests if the content of the directory changes
    watcher: RecommendedWatcher,

    /// Cached panels from previous requests.
    ///
    /// When we want to create a new panel, we first look into the cache,
    /// if a panel for the specified path was already created in the past.
    /// If so, we still send an update request to the [`ContentManager`],
    /// to avoid working with outdated information.
    /// If the cache is empty, we generate a `loading`-panel (see [`DirPanel::loading`]).
    cache: PanelCache<PanelType>,

    /// Sends request for new panel content.
    content_tx: mpsc::UnboundedSender<PanelUpdate>,
}

impl<PanelType: BasePanel> ManagedPanel<PanelType> {
    pub fn new(
        cache: PanelCache<PanelType>,
        content_tx: mpsc::UnboundedSender<PanelUpdate>,
        reload_on_modify: bool,
    ) -> Self {
        let state = Arc::new(Mutex::new(PanelState::default()));
        let watcher_state = state.clone();
        let watcher_tx = content_tx.clone();
        let watcher = notify::recommended_watcher(
            move |res: std::result::Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    match event.kind {
                        notify::EventKind::Create(_) | notify::EventKind::Remove(_) => {
                            let state = watcher_state.lock().clone();
                            info!("Updating: {}", state.path().display());
                            if let Err(e) = watcher_tx.send(PanelUpdate { state }) {
                                error!("{e}");
                            }
                        }
                        notify::EventKind::Modify(_) => {
                            if reload_on_modify {
                                let state = watcher_state.lock().clone();
                                info!("Updating: {}", state.path().display());
                                if let Err(e) = watcher_tx.send(PanelUpdate { state }) {
                                    error!("{e}");
                                }
                            }
                        }
                        _ => (),
                    }
                }
            },
        )
        .expect("File-watcher error");
        ManagedPanel {
            panel: PanelType::empty(),
            state,
            watcher,
            cache,
            content_tx,
        }
    }

    pub fn check_update(&self, new_state: &PanelState) -> bool {
        self.state.lock().check_update(new_state)
    }

    /// Generates a new panel for the given path.
    ///
    /// The panel is created instantly, so there is no "loading..." or
    /// waiting for the content manager to fetch some data in the background.
    pub fn new_panel_instant<P: AsRef<Path>>(&mut self, path: Option<P>) {
        if let Some(path) = path.and_then(|p| canonicalize(p.as_ref()).ok()) {
            // Only create a new panel when the path has changed
            if path == self.panel.path() {
                debug!("new-panel-instant: same path, path = {}", path.display());
                return;
            }

            if let Some(cached) = self.cache.get(&path) {
                debug!("new-panel-instant: using cache, path = {}", path.display());
                let cached_mod_time = cached.modified();
                // Update panel with content from cache
                self.update_panel(cached);

                let mod_time = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or_else(SystemTime::now);

                // If the access time is has not changed, dont trigger an update
                // by returning early
                if mod_time == cached_mod_time {
                    debug!(
                        "new-panel-instant: access-time == cached-access-time, path = {}",
                        path.display()
                    );
                    return;
                }
            } else {
                self.update_panel(PanelType::from_path(path.clone()));
            }
            // Send an update request anyway, so that the content manager starts filling the cache
            self.content_tx
                .send(PanelUpdate {
                    state: self.state.lock().clone(),
                })
                .expect("Receiver dropped or closed");
        } else {
            debug!("new-panel-instant: empty panel");
            self.update(PanelType::empty());
        }
    }

    /// Generates a new panel for the given path.
    ///
    /// Uses cached values to instantly display something, while in the background
    /// the [`ContentManager`] is triggered to load new data.
    /// If the cache is empty, a generic "loading..." panel is created.
    /// An empty panel is created if the given path is `None`.
    pub fn new_panel_delayed<P: AsRef<Path>>(&mut self, path: Option<P>) {
        if let Some(path) = path.and_then(|p| canonicalize(p.as_ref()).ok()) {
            // Only create a new panel when the path has changed
            if path == self.panel.path() {
                return;
            }

            if let Some(cached) = self.cache.get(&path) {
                let cached_access_time = cached.modified();
                // Update panel with content from cache
                self.update_panel(cached);

                let access_time = path
                    .metadata()
                    .ok()
                    .and_then(|m| m.accessed().ok())
                    .unwrap_or_else(SystemTime::now);

                // If the access time is has not changed, dont trigger an update
                // by returning early
                if access_time == cached_access_time {
                    return;
                }
            } else {
                self.update_panel(PanelType::loading(path.clone()));
            }
            self.content_tx
                .send(PanelUpdate {
                    state: self.state.lock().clone(),
                })
                .expect("Receiver dropped or closed");
        } else {
            self.update(PanelType::empty());
        }
    }

    pub fn reload(&self) {
        self.content_tx
            .send(PanelUpdate {
                state: self.state.lock().clone(),
            })
            .expect("Receiver dropped or closed");
    }

    fn update(&mut self, panel: PanelType) {
        let mut state = self.state.lock();
        state.increase();
        state.path = panel.path().to_path_buf();
        self.panel.update_content(panel);
    }

    /// Freezes the panel in its current state.
    ///
    /// Deactivates all watchers so that the panel will receive no updates until we call "unfreeze".
    pub fn freeze(&mut self) {
        unwatch_path(&mut self.watcher, self.panel.path());
    }

    /// Unfreezes the panel in its current state.
    ///
    /// Re-activates all watchers so that the panel will receive new updates.
    /// Also refreshes the panel in case the content has changed since the last freeze.
    pub fn unfreeze(&mut self) {
        watch_path(&mut self.watcher, self.panel.path());
        self.reload();
    }

    /// Updates an existing panel.
    ///
    /// The panel is directly updated without any further checks!
    /// To check if an update is necessary, call [`check_update`] on the new panel state.
    pub fn update_panel(&mut self, panel: PanelType) {
        // Update watchers
        if self.panel.path() != panel.path() {
            unwatch_path(&mut self.watcher, self.panel.path());
            watch_path(&mut self.watcher, panel.path());
        }
        self.update(panel);
    }

    /// Returns a mutable reference to the managed panel
    pub fn panel_mut(&mut self) -> &mut PanelType {
        &mut self.panel
    }

    /// Returns a reference to the managed panel
    pub fn panel(&self) -> &PanelType {
        &self.panel
    }
}

#[derive(Clone)]
struct MillerColumns {
    left_x_range: Range<u16>,
    center_x_range: Range<u16>,
    right_x_range: Range<u16>,
    y_range: Range<u16>,
    width: u16,
}

impl MillerColumns {
    pub fn from_size(terminal_size: (u16, u16)) -> Self {
        let (sx, sy) = terminal_size;
        Self {
            left_x_range: 0..(sx / 8),
            center_x_range: (sx / 8)..(sx / 2),
            right_x_range: (sx / 2)..sx,
            y_range: 1..sy.saturating_sub(1), // 1st line is reserved for the header, last for the footer
            width: sx,
        }
    }

    pub fn footer(&self) -> u16 {
        self.y_range.end.saturating_add(1)
    }

    pub fn height(&self) -> u16 {
        self.y_range.end.saturating_sub(self.y_range.start)
    }

    pub fn width(&self) -> u16 {
        self.width
    }
}
