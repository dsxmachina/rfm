use std::{
    fs::canonicalize,
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    sync::Arc,
};

use cached::{cached_result, Cached, SizedCache, TimedSizedCache};
use crossterm::terminal;
use fasthash::MetroHasher;
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

cached_result! {
    DIR_CONTENT: TimedSizedCache<PathBuf, Vec<DirElem>> = TimedSizedCache::with_size_and_lifespan(10, 2);
    fn dir_content(path: PathBuf) -> Result<Vec<DirElem>, io::Error> = {
        // read directory
        let dir = std::fs::read_dir(path)?;
        let mut out = Vec::new();
        for item in dir {
            out.push(DirElem::from(item?.path()))
        }
        // out.sort();
        out.sort_by_cached_key(|a| a.name().to_lowercase());
        out.sort_by_cached_key(|a| !a.path().is_dir());
        Ok(out)
    }
}

cached_result! {
    DIR_CONTENT_PREVIEW: TimedSizedCache<(PathBuf, usize), Vec<DirElem>> = TimedSizedCache::with_size_and_lifespan(10, 2);
    fn dir_content_preview(path: PathBuf, max_elem: usize) -> Result<Vec<DirElem>, io::Error> = {
        // read directory
        let dir = std::fs::read_dir(path)?;
        let mut out = Vec::new();
        for (idx, item) in dir.enumerate() {
            if idx >= max_elem {
                break;
            }
            out.push(DirElem::from(item?.path()))
        }
        out.sort_by_cached_key(|a| a.name().to_lowercase());
        out.sort_by_cached_key(|a| !a.path().is_dir());
        Ok(out)
    }
}

// TODO: This is a dublicate - put this somewhere else
pub fn hash_elements(elements: &Vec<DirElem>) -> u64 {
    // let mut h: MetroHasher = Default::default();
    let mut h: fasthash::XXHasher = Default::default();
    for elem in elements.iter() {
        elem.name().hash(&mut h);
    }
    h.finish()
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
                // Notification::new()
                //     .summary(&format!("parsing: {}", path.display()))
                //     .show()
                // .unwrap();
                // Parse directory
                let dir_path = path.clone();

                let result = if let Select::Right = state.panel {
                    // let (_, sy) = terminal::size().unwrap_or((128, 128));
                    tokio::task::spawn_blocking(move || dir_content_preview(dir_path, 1024)).await
                } else {
                    // Parse entire directory
                    tokio::task::spawn_blocking(move || dir_content(dir_path)).await
                };

                if let Ok(Ok(content)) = result {
                    // Calculate new state
                    let new_state = PanelState {
                        state_cnt: state.state_cnt + 1,
                        hash: hash_elements(&content),
                        panel: state.panel.clone(),
                    };
                    // Only send new panel, if the content has changed
                    if state.hash != new_state.hash {
                        // Create dir-panel from content
                        let panel = DirPanel::new(content.clone(), path.clone());
                        // Send content back
                        let _ = self.dir_tx.send((panel, new_state)).await;
                    }
                    // Cache result
                    self.cache.insert(path, content);
                }
            } else {
                // Calculate new state
                let new_state = PanelState {
                    state_cnt: state.state_cnt + 1,
                    hash: 0,
                    panel: state.panel.clone(),
                };
                // Create preview
                let _ = self
                    .prev_tx
                    .send((PreviewPanel::new(path), new_state))
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_dir_hashing_speed() {
        let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
        // read directory
        let content = dir_content(path).unwrap();
        let now = Instant::now();
        let hash = hash_elements(&content);
        println!(
            "hashing {} elements took: {}ms",
            content.len(),
            now.elapsed().as_millis()
        );
        println!("hash={hash}");
        assert!(false);
    }

    // #[test]
    // fn test_dir_parsing_speed() {
    //     let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
    //     // read directory
    //     let now = Instant::now();
    //     let dir = std::fs::read_dir(path).unwrap();
    //     println!("read-dir: {}", now.elapsed().as_millis());
    //     let now = Instant::now();
    //     let mut out = Vec::new();
    //     for item in dir.skip(1) {
    //         let item_path = canonicalize(item.unwrap().path()).unwrap();
    //         out.push(DirElem::from(item_path))
    //     }
    //     println!("load-dir: {}", now.elapsed().as_millis());
    //     let now = Instant::now();

    //     out.sort_by_cached_key(|a| a.name().to_lowercase());
    //     out.sort_by_cached_key(|a| a.path().is_dir());
    //     // out.sort_by_key(|elem| {

    //     // })
    //     // out.sort();

    //     println!("sort: {}", now.elapsed().as_millis());

    //     println!("elements: {}", out.len());
    //     assert!(true);
    // }

    // #[test]
    // fn test_symlink_parent() {
    //     let path: PathBuf = "/home/someone/".into();
    //     // read directory
    //     let dir = std::fs::read_dir(path).unwrap();
    //     for item in dir {
    //         let entry = item.unwrap();
    //         let path_1 = entry.path();
    //         let path_2 = canonicalize(path_1.as_path()).unwrap();

    //         println!(
    //             "{}: {}",
    //             path_1.display(),
    //             path_1.parent().unwrap().display()
    //         );
    //         println!(
    //             "{}: {}",
    //             path_2.display(),
    //             path_2.parent().unwrap().display()
    //         );
    //         assert_eq!(path_1.parent(), path_2.parent());
    //     }
    //     assert!(false)
    // }
}
