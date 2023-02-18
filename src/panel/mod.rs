use crossterm::{
    cursor, queue,
    style::{self, Print, PrintStyledContent, Stylize},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use notify::{RecommendedWatcher, Watcher};
use notify_rust::Notification;
use pad::PadStr;
use std::{
    cmp::Ordering,
    fs::canonicalize,
    io::{stdout, Stdout, Write},
    mem,
    ops::Range,
    os::unix::prelude::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::SystemTime,
};
use tokio::sync::mpsc;

use crate::{
    commands::Movement,
    content::{hash_elements, SharedCache},
};

mod console;
mod directory;
pub mod manager;
mod preview;

pub use directory::{DirElem, DirPanel};
pub use preview::{FilePreview, Preview, PreviewPanel};

/// Basic trait that lets us draw something on the terminal in a specified range.
pub trait Draw {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()>;
}

/// Basic trait for managing the content of a panel
pub trait PanelContent: Draw + Clone + Send {
    /// Path of the panel
    fn path(&self) -> &Path;

    /// Hash of the panels content
    fn content_hash(&self) -> u64;

    /// Access time of the path
    fn accessed(&self) -> SystemTime;

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
}

#[derive(Debug, Clone, Copy)]
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
    cnt: u64,
}

impl Default for PanelState {
    fn default() -> Self {
        // Generate a random id here - because we only have three panels,
        // the chance of collision is pretty low.
        Self {
            panel_id: rand::random(),
            cnt: 0,
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
}

/// Combines all data that is necessary to update a panel.
///
/// Will be send as a request to the [`ContentManager`].
#[derive(Debug)]
pub struct PanelUpdate {
    pub path: PathBuf,
    pub state: PanelState,
    pub hash: u64,
}

pub struct ManagedPanel<PanelType: BasePanel> {
    /// Panel to be updated.
    panel: PanelType,

    /// State counter and identifier of the managed panel
    state: PanelState,

    /// File-watcher that sends update requests if the content of the directory changes
    watcher: RecommendedWatcher,

    /// Cached panels from previous requests.
    ///
    /// When we want to create a new panel, we first look into the cache,
    /// if a panel for the specified path was already created in the past.
    /// If so, we still send an update request to the [`ContentManager`],
    /// to avoid working with outdated information.
    /// If the cache is empty, we generate a `loading`-panel (see [`DirPanel::loading`]).
    cache: SharedCache<PanelType>,

    /// Sends request for new panel content.
    content_tx: mpsc::UnboundedSender<PanelUpdate>,
}

impl<PanelType: BasePanel> ManagedPanel<PanelType> {
    pub fn new(
        cache: SharedCache<PanelType>,
        content_tx: mpsc::UnboundedSender<PanelUpdate>,
    ) -> Self {
        let watcher = notify::recommended_watcher(|res| {
            Notification::new()
                .summary(&format!("new-event: {:?}", res))
                .show()
                .unwrap();
            // if let Ok(event) = res {}
        })
        .expect("File-watcher error");
        ManagedPanel {
            panel: PanelType::empty(),
            state: PanelState::default(),
            watcher,
            cache,
            content_tx,
        }
    }

    pub fn check_update(&self, new_state: &PanelState) -> bool {
        self.state.check_update(new_state)
    }

    /// Generates a new panel for the given path.
    ///
    /// Uses cached values to instantly display something, while in the background
    /// the [`ContentManager`] is triggered to load new data.
    /// If the cache is empty, a generic "loading..." panel is created.
    /// An empty panel is created if the given path is `None`.
    pub fn new_panel<P: AsRef<Path>>(&mut self, path: Option<P>) {
        match self.watcher.unwatch(self.panel.path()) {
            Ok(_) => {
                Notification::new()
                    .summary("unwatching")
                    .body(&format!("{}", self.panel.path().display()))
                    .show()
                    .unwrap();
            }
            Err(e) => {
                Notification::new()
                    .summary("unwatch-error")
                    .body(&format!("{:?}", e))
                    .show()
                    .unwrap();
            }
        }
        // Increase state counter
        self.state.increase();
        if let Some(path) = path.and_then(|p| canonicalize(p.as_ref()).ok()) {
            // Only create a new panel when the path has changed
            if path == self.panel.path() {
                // Notification::new()
                //     .summary("No change for panel")
                //     .show()
                //     .unwrap();
                return;
            }

            // Watch new path
            match self
                .watcher
                .watch(path.as_path(), notify::RecursiveMode::NonRecursive)
            {
                Ok(_) => {
                    Notification::new()
                        .summary("watching")
                        .body(&format!("{}", path.display()))
                        .show()
                        .unwrap();
                }
                Err(e) => {
                    Notification::new()
                        .summary("watch-error")
                        .body(&format!("{:?}", e))
                        .show()
                        .unwrap();
                }
            }

            let access_time = path
                .metadata()
                .ok()
                .and_then(|m| m.accessed().ok())
                .unwrap_or_else(|| SystemTime::now());

            // self.panel.update_content(panel);
            if let Some(cached) = self.cache.get(&path) {
                let cached_access_time = cached.accessed();
                // Update panel with content from cache
                self.panel.update_content(cached);

                // If the access time is has not changed, dont trigger an update
                // by returning early
                if access_time == cached_access_time {
                    return;
                }
            } else {
                self.panel.update_content(PanelType::loading(path.clone()));
            }
            // Send update request for given panel
            // Notification::new()
            //     .summary("send update request")
            //     .body(&format!("{:?}", self.state.increased()))
            //     .show()
            //     .unwrap();
            self.content_tx
                .send(PanelUpdate {
                    path,
                    state: self.state,
                    hash: self.panel.content_hash(),
                })
                .expect("Receiver dropped or closed");
        } else {
            self.panel.update_content(PanelType::empty());
        }
    }

    /// Updates an existing panel.
    ///
    /// The panel is directly updated without any further checks!
    /// To check if an update is necessary, call [`check_update`] on the new panel state.
    pub fn update_panel(&mut self, panel: PanelType) {
        // Watch new panels path
        match self.watcher.unwatch(self.panel.path()) {
            Ok(_) => {
                Notification::new()
                    .summary("unwatching")
                    .body(&format!("{}", self.panel.path().display()))
                    .show()
                    .unwrap();
            }
            Err(e) => {
                Notification::new()
                    .summary("unwatch-error")
                    .body(&format!("{:?}", e))
                    .show()
                    .unwrap();
            }
        }
        match self
            .watcher
            .watch(panel.path(), notify::RecursiveMode::NonRecursive)
        {
            Ok(_) => {
                Notification::new()
                    .summary("watching")
                    .body(&format!("{}", panel.path().display()))
                    .show()
                    .unwrap();
            }
            Err(e) => {
                Notification::new()
                    .summary("watch-error")
                    .body(&format!("{:?}", e))
                    .show()
                    .unwrap();
            }
        }
        // let _ = self
        //     .watcher
        //     .watch(panel.path(), notify::RecursiveMode::NonRecursive);
        self.panel.update_content(panel);
        self.state.increase();
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
