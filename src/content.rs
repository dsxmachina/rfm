use std::{fs::canonicalize, io, path::PathBuf, sync::Arc};

use cached::{Cached, SizedCache};
use parking_lot::Mutex;
use tokio::{fs::read_dir, sync::mpsc};

use crate::panel::{DirElem, DirPanel, PanelState, PreviewPanel};

/// Cache that is shared by the content-manager and the panel-manager.
#[derive(Clone)]
pub struct SharedCache {
    cache: Arc<Mutex<SizedCache<PathBuf, Vec<DirElem>>>>,
}

impl SharedCache {
    pub fn with_size(size: usize) -> Self {
        SharedCache {
            cache: Arc::new(Mutex::new(SizedCache::with_size(size))),
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<Vec<DirElem>> {
        self.cache.lock().cache_get(&path).cloned()
    }

    pub fn insert(&self, path: PathBuf, elements: Vec<DirElem>) -> Option<Vec<DirElem>> {
        self.cache.lock().cache_set(path, elements)
    }
}

/// Receives commands to parse the directory or generate a new preview.
pub struct Manager {
    /// Check the path and respond accordingly
    rx: mpsc::Receiver<(PathBuf, PanelState)>,

    dir_tx: mpsc::Sender<(DirPanel, PanelState)>,

    prev_tx: mpsc::Sender<(PreviewPanel, PanelState)>,

    cache: SharedCache,
}

/// Reads the content of a directory asynchronously
async fn dir_content(path: PathBuf) -> Result<Vec<DirElem>, io::Error> {
    // read directory
    let mut dir = read_dir(path).await?;
    let mut out = Vec::new();

    while let Some(item) = dir.next_entry().await? {
        let item_path = canonicalize(item.path())?;
        out.push(DirElem::from(item_path))
    }
    out.sort();
    Ok(out)
}

impl Manager {
    pub fn new(
        cache: SharedCache,
        rx: mpsc::Receiver<(PathBuf, PanelState)>,
        dir_tx: mpsc::Sender<(DirPanel, PanelState)>,
        prev_tx: mpsc::Sender<(PreviewPanel, PanelState)>,
    ) -> Self {
        Manager {
            rx,
            dir_tx,
            prev_tx,
            cache,
        }
    }

    pub async fn run(mut self) {
        while let Some((path, state)) = self.rx.recv().await {
            if path.is_dir() {
                // Parse directory
                let result = tokio::spawn(dir_content(path.clone())).await;
                if let Ok(Ok(content)) = result {
                    // Create dir-panel from content
                    let panel = DirPanel::new(content.clone(), path.clone());
                    // Send content back
                    self.dir_tx
                        .send((panel, state))
                        .await
                        .expect("Receiver dropped or closed");
                    // Cache result
                    self.cache.insert(path, content);
                }
            } else {
                // Create preview
                // TODO
            }
        }
    }
}
