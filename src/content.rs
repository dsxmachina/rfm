use cached::{Cached, SizedCache};
use log::debug;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::SystemTime,
};
use tokio::{sync::mpsc, task::spawn_blocking};
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

/// Receives commands to parse the directory or generate a new preview.
pub struct DirManager {
    tx: mpsc::Sender<(DirPanel, PanelState)>,
    rx: mpsc::UnboundedReceiver<PanelUpdate>,
    directory_cache: PanelCache<DirPanel>,
    preview_cache: PanelCache<PreviewPanel>,
}

/// Receives commands to parse the directory or generate a new preview.
pub struct PreviewManager {
    tx: mpsc::Sender<(PreviewPanel, PanelState)>,
    rx: mpsc::UnboundedReceiver<PanelUpdate>,
    preview_cache: PanelCache<PreviewPanel>,
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
    ) -> Self {
        DirManager {
            tx,
            rx,
            directory_cache,
            preview_cache,
        }
    }

    pub async fn run(mut self) {
        let mut last_cache_path = PathBuf::default();
        while let Some(update) = self.rx.recv().await {
            if !update.state.path().is_dir() {
                continue;
            }
            let dir_path = update.state.path().clone();
            debug!("request new dir-panel for {}", dir_path.display());
            let result = spawn_blocking(move || dir_content(dir_path)).await;
            if let Ok(content) = result {
                // Only update when the hash has changed
                let panel = DirPanel::new(content, update.state.path().clone());
                if let Err(e) = self
                    .tx
                    .send((panel.clone(), update.state.increased().increased()))
                    .await
                {
                    debug!("Cannot send panel-update: {e}");
                    continue;
                };
                self.directory_cache
                    .insert(update.state.path().clone(), panel.clone());
                self.preview_cache
                    .insert(update.state.path().clone(), PreviewPanel::Dir(panel));
            }
            if update.state.path() != last_cache_path.as_path() {
                last_cache_path = update.state.path().to_path_buf();
                let path = update.state.path();
                let dir_cache = self.directory_cache.clone();
                let prev_cache = self.preview_cache.clone();
                tokio::task::spawn_blocking(move || fill_cache(path, dir_cache, prev_cache));
            }
        }
    }
}

impl PreviewManager {
    pub fn new(
        preview_cache: PanelCache<PreviewPanel>,
        tx: mpsc::Sender<(PreviewPanel, PanelState)>,
        rx: mpsc::UnboundedReceiver<PanelUpdate>,
    ) -> Self {
        PreviewManager {
            tx,
            rx,
            preview_cache,
        }
    }

    pub async fn run(mut self) {
        while let Some(update) = self.rx.recv().await {
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
                        continue;
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
                        continue;
                    }
                    self.preview_cache.insert(update.state.path(), panel);
                }
            }
        }
    }
}
