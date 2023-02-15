use std::{
    fs::canonicalize,
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    sync::Arc,
};

use cached::{cached, cached_result, Cached, SizedCache, TimedSizedCache};
use crossterm::terminal;
use fasthash::MetroHasher;
use notify_rust::Notification;
use parking_lot::Mutex;
use tokio::{fs::read_dir, sync::mpsc};

use crate::panel::{DirElem, DirPanel, FilePreview, PanelContent, PanelState, Select};

/// Cache that is shared by the content-manager and the panel-manager.
#[derive(Clone)]
pub struct SharedCache<Item: Clone> {
    cache: Arc<Mutex<SizedCache<PathBuf, Item>>>,
}

impl<Item: Clone> SharedCache<Item> {
    pub fn with_size(size: usize) -> Self {
        SharedCache {
            cache: Arc::new(Mutex::new(SizedCache::with_size(size))),
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<Item> {
        self.cache.lock().cache_get(&path).cloned()
    }

    pub fn insert(&self, path: PathBuf, item: Item) -> Option<Item> {
        self.cache.lock().cache_set(path, item)
    }
}

/// Receives commands to parse the directory or generate a new preview.
pub struct Manager {
    /// Check the path and respond accordingly
    rx: mpsc::Receiver<(PathBuf, PanelState)>,

    dir_tx: mpsc::Sender<(DirPanel, PanelState)>,

    prev_tx: mpsc::Sender<(FilePreview, PanelState)>,

    // TODO: This should save DirPanels
    directory_cache: SharedCache<Vec<DirElem>>,
    // TODO: This should save PreviewPanels
    preview_cache: SharedCache<FilePreview>,
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

cached! {
    FILE_PREVIEW: TimedSizedCache<PathBuf, FilePreview> = TimedSizedCache::with_size_and_lifespan(10, 5);
    fn get_file_preview(path: PathBuf) -> FilePreview = {
        FilePreview::new(path)
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
        directory_cache: SharedCache<Vec<DirElem>>,
        preview_cache: SharedCache<FilePreview>,
        rx: mpsc::Receiver<(PathBuf, PanelState)>,
        dir_tx: mpsc::Sender<(DirPanel, PanelState)>,
        prev_tx: mpsc::Sender<(FilePreview, PanelState)>,
    ) -> Self {
        Manager {
            rx,
            dir_tx,
            prev_tx,
            directory_cache,
            preview_cache,
        }
    }

    pub async fn run(mut self) {
        while let Some((path, state)) = self.rx.recv().await {
            if path.is_dir() {
                let dir_path = path.clone();

                let result = if let Select::Right = state.panel {
                    // TODO: We mix the preview cache with the "real" one here!
                    tokio::task::spawn_blocking(move || dir_content_preview(dir_path, 16538)).await
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
                    self.directory_cache.insert(path, content);
                }
            } else {
                // Create preview
                let preview = get_file_preview(path.clone());
                // Calculate new state
                let new_state = PanelState {
                    state_cnt: state.state_cnt + 1,
                    hash: preview.content_hash(),
                    panel: state.panel.clone(),
                };

                // Only send new panel, if the content may has changed
                if state.hash != new_state.hash {
                    let _ = self.prev_tx.send((preview.clone(), new_state)).await;
                }
                // Cache result
                self.preview_cache.insert(path, preview);
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
        assert!(true);
    }

    #[test]
    fn test_image_load_speed() {
        let now = Instant::now();
        let img = image::io::Reader::open("/home/someone/Bilder/wallpaper_source/abstract/hologram_scheme_scifi_139294_1920x1080.jpg").unwrap().decode().unwrap();
        let elapsed = now.elapsed().as_millis();
        println!("loading image took {elapsed}ms");
        let now = Instant::now();
        let small_img = img.thumbnail_exact(400, 300).into_rgb8();
        let elapsed = now.elapsed().as_millis();
        println!("processing image took {elapsed}ms");
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
