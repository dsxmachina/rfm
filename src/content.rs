use std::{fs::canonicalize, io, path::PathBuf, sync::Arc};

use cached::{cached_result, Cached, SizedCache, TimedSizedCache};
use crossterm::terminal;
use notify_rust::Notification;
use parking_lot::Mutex;
use tokio::{fs::read_dir, sync::mpsc};

use crate::panel::{DirElem, DirPanel, PanelState, PreviewPanel, Select};

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

// NOTE: This takes way longer than the sync version
// /// Reads the content of a directory asynchronously
// async fn async_dir_content(path: PathBuf) -> Result<Vec<DirElem>, io::Error> {
//     // read directory
//     let mut dir = tokio::fs::read_dir(path).await?;
//     let mut out = Vec::new();

//     while let Some(item) = dir.next_entry().await? {
//         let item_path = canonicalize(item.path())?;
//         out.push(DirElem::from(item_path))
//     }
//     out.sort();
//     Ok(out)
// }

cached_result! {
    DIR_CONTENT: TimedSizedCache<PathBuf, Vec<DirElem>> = TimedSizedCache::with_size_and_lifespan(10, 1);
    fn dir_content(path: PathBuf) -> Result<Vec<DirElem>, io::Error> = {
        // read directory
        let dir = std::fs::read_dir(path)?;
        let mut out = Vec::new();
        for item in dir {
            let item_path = canonicalize(item?.path())?;
            out.push(DirElem::from(item_path))
        }
        out.sort();
        Ok(out)
    }
}

cached_result! {
    DIR_CONTENT_PREVIEW: TimedSizedCache<(PathBuf, usize), Vec<DirElem>> = TimedSizedCache::with_size_and_lifespan(10, 1);
    fn dir_content_preview(path: PathBuf, max_elem: usize) -> Result<Vec<DirElem>, io::Error> = {
        // read directory
        let dir = std::fs::read_dir(path)?;
        let mut out = Vec::new();
        for (idx, item) in dir.enumerate() {
            if idx >= max_elem {
                break;
            }
            let item_path = canonicalize(item?.path())?;
            out.push(DirElem::from(item_path))
        }
        out.sort();
        Ok(out)
    }
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
            let new_state = PanelState {
                state_cnt: state.state_cnt + 1,
                panel: state.panel.clone(),
            };
            if path.is_dir() {
                // Notification::new()
                //     .summary(&format!("parsing: {}", path.display()))
                //     .show()
                // .unwrap();
                // Parse directory
                let dir_path = path.clone();

                let result = if let Select::Right = state.panel {
                    // let (_, sy) = terminal::size().unwrap_or((128, 128));
                    tokio::task::spawn_blocking(move || dir_content_preview(dir_path, 256)).await
                } else {
                    // Parse entire directory
                    tokio::task::spawn_blocking(move || dir_content(dir_path)).await
                };

                if let Ok(Ok(content)) = result {
                    // Create dir-panel from content
                    let panel = DirPanel::new(content.clone(), path.clone());
                    // Send content back
                    let _ = self.dir_tx.send((panel, new_state)).await;
                    // Notification::new()
                    //     .summary(&format!("finished: {}", path.display()))
                    //     .show()
                    //     .unwrap();
                    // Cache result
                    self.cache.insert(path, content);
                }
            } else {
                // Create preview
                let _ = self
                    .prev_tx
                    .send((PreviewPanel::new(path), new_state))
                    .await;
            }
        }
    }
}