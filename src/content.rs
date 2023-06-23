use cached::{Cached, SizedCache};
use log::{debug, info, warn};
use parking_lot::Mutex;
use std::{
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};
use tokio::{sync::mpsc, task::spawn_blocking};
use walkdir::WalkDir;

use crate::panel::{
    DirElem, DirPanel, FilePreview, PanelContent, PanelState, PanelUpdate, PreviewPanel,
};

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

pub fn dir_content(path: PathBuf) -> Vec<DirElem> {
    // read directory
    match std::fs::read_dir(path) {
        Ok(dir) => {
            let mut out = Vec::new();
            for item in dir.into_iter().flatten() {
                out.push(DirElem::from(item.path()))
            }
            out
        }
        Err(_) => Vec::new(),
    }
}

pub fn hash_elements(elements: &[DirElem]) -> u64 {
    let mut h: fasthash::XXHasher = Default::default();
    for elem in elements {
        elem.name().hash(&mut h);
    }
    h.finish()
}

// TODO: Benchmark this guy
async fn fill_cache(
    path: PathBuf,
    directory_cache: PanelCache<DirPanel>,
    preview_cache: PanelCache<PreviewPanel>,
) {
    if !path.is_dir() {
        return;
    }
    let file_capacity = preview_cache.capacity() / 16;
    let dir_capacity = directory_cache.capacity() / 16;
    let mut dir_handles = Vec::new();
    let mut file_handles = Vec::new();
    for entry in WalkDir::new(&path).max_depth(2).into_iter().flatten() {
        if entry.file_type().is_dir() && dir_handles.len() < dir_capacity {
            let handle_path = entry.into_path();
            if directory_cache.requires_update(&handle_path) {
                dir_handles.push((
                    handle_path.clone(),
                    spawn_blocking(move || dir_content(handle_path)),
                ));
            }
        } else if entry.file_type().is_file()
            && entry.depth() == 1
            && file_handles.len() < file_capacity
        {
            let handle_path = entry.into_path();
            if preview_cache.requires_update(&handle_path) {
                file_handles.push((
                    handle_path.clone(),
                    spawn_blocking(move || FilePreview::new(handle_path)),
                ));
            }
        }
        // If we reached the max capacity that we want to fill the cache up with,
        // stop traversing the directory any further.
        if dir_handles.len() >= dir_capacity && file_handles.len() >= file_capacity {
            break;
        }
    }
    for (handle_path, handle) in file_handles {
        if let Ok(content) = handle.await {
            let panel = PreviewPanel::File(content);
            preview_cache.insert(handle_path, panel);
        }
    }
    for (handle_path, handle) in dir_handles {
        if let Ok(content) = handle.await {
            let panel = DirPanel::new(content, handle_path.clone());
            directory_cache.insert(handle_path.clone(), panel.clone());
            preview_cache.insert(handle_path, PreviewPanel::Dir(panel));
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
                if update.state.hash() != panel.content_hash() {
                    if self
                        .tx
                        .send((panel.clone(), update.state.increased().increased()))
                        .await
                        .is_err()
                    {
                        break;
                    };
                } else {
                    debug!(
                        "unchanged hash: {}, path = {}",
                        update.state.hash(),
                        update.state.path().display()
                    );
                }
                self.directory_cache
                    .insert(update.state.path().clone(), panel.clone());
                self.preview_cache
                    .insert(update.state.path().clone(), PreviewPanel::Dir(panel));
            }
            if update.state.path() != last_cache_path.as_path() {
                last_cache_path = update.state.path().to_path_buf();
                tokio::task::spawn(fill_cache(
                    update.state.path(),
                    self.directory_cache.clone(),
                    self.preview_cache.clone(),
                ));
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
                    if update.state.hash() != panel.content_hash()
                        && self
                            .tx
                            .send((panel.clone(), update.state.increased()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                    self.preview_cache.insert(update.state.path(), panel);
                }
            } else {
                // Create preview
                let file_path = update.state.path().clone();
                let result = spawn_blocking(move || FilePreview::new(file_path)).await;
                if let Ok(preview) = result {
                    let panel = PreviewPanel::File(preview);
                    if update.state.hash() != panel.content_hash()
                        && self
                            .tx
                            .send((panel.clone(), update.state.increased()))
                            .await
                            .is_err()
                    {
                        break;
                    }
                    self.preview_cache.insert(update.state.path(), panel);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // use super::*;
    // use patricia_tree::{PatriciaMap, PatriciaSet};
    // use std::time::Instant;
    // #[test]
    // fn test_dir_parsing_speed() {
    //     let parse_dir = |path: PathBuf| {
    //         // read directory
    //         let now = Instant::now();
    //         let mut content = dir_content(path);
    //         let elapsed = now.elapsed().as_millis();
    //         println!("parsing {} elements took: {elapsed}ms", content.len(),);

    //         let now = Instant::now();
    //         content.sort_by_cached_key(|a| a.name_lowercase().clone());
    //         content.sort_by_cached_key(|a| !a.path().is_dir());
    //         let elapsed = now.elapsed().as_millis();
    //         println!("sorting {} elements took: {elapsed}ms", content.len(),);

    //         let now = Instant::now();
    //         content.iter_mut().for_each(|e| e.normalize());
    //         let elapsed = now.elapsed().as_millis();
    //         println!("normalizing {} elements took: {elapsed}ms", content.len(),);
    //     };

    //     parse_dir("/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into());
    //     parse_dir("/nix/store".into());
    //     assert!(false);
    // }

    // #[test]
    // fn test_panel_creation_time() {
    //     let create_panel = |path: PathBuf| {
    //         // read directory
    //         let now = Instant::now();
    //         let content = dir_content(path.clone());
    //         let panel = DirPanel::new(content, path);
    //         let elapsed = now.elapsed().as_millis();
    //         println!(
    //             "creating panel with {} elements took: {elapsed}ms",
    //             panel.elements().len(),
    //         );
    //     };

    //     create_panel("/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into());
    //     create_panel("/nix/store".into());

    //     assert!(false);
    // }

    // #[test]
    // fn test_dir_hashing_speed() {
    //     let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
    //     // read directory
    //     let content = dir_content(path);
    //     let now = Instant::now();
    //     let hash = hash_elements(&content);
    //     let elapsed = now.elapsed().as_millis();
    //     println!("hashing {} elements took: {elapsed}ms", content.len(),);
    //     println!("hash={hash}");
    //     assert!(true);
    // }

    // #[test]
    // fn test_dir_parsing_speed() {
    //     let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
    //     // read directory
    //     let now = Instant::now();
    //     let content = dir_content(path);
    //     let elapsed = now.elapsed().as_millis();
    //     println!("parsing {} elements took: {elapsed}ms", content.len(),);
    //     assert!(false);
    // }

    // #[test]
    // fn test_patricia_tree_speed() {
    //     let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
    //     // read directory
    //     let content = dir_content(path);
    //     let mut set = PatriciaSet::new();
    //     let now = Instant::now();
    //     for item in content {
    //         set.insert(item.name_lowercase());
    //     }
    //     let elapsed = now.elapsed().as_millis();
    //     println!(
    //         "building tree from {} elements took: {elapsed}ms",
    //         set.len(),
    //     );
    //     assert!(false);
    // }

    // #[test]
    // fn test_patricia_map_speed() {
    //     let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
    //     // read directory
    //     let content = dir_content(path);
    //     let mut map = PatriciaMap::new();
    //     let now = Instant::now();
    //     for (idx, item) in content.iter().enumerate() {
    //         map.insert(item.name_lowercase(), idx);
    //     }
    //     let elapsed = now.elapsed().as_millis();
    //     println!("building map from {} elements took: {elapsed}ms", map.len(),);
    //     assert!(false);
    // }

    // #[test]
    // fn test_image_load_speed() {
    //     let now = Instant::now();
    //     let img = image::io::Reader::open("/home/someone/Bilder/wallpaper_source/abstract/hologram_scheme_scifi_139294_1920x1080.jpg").unwrap().decode().unwrap();
    //     let elapsed = now.elapsed().as_millis();
    //     println!("loading image took {elapsed}ms");
    //     let now = Instant::now();
    //     let _small_img = img.thumbnail_exact(400, 300).into_rgb8();
    //     let elapsed = now.elapsed().as_millis();
    //     println!("processing image took {elapsed}ms");
    //     assert!(true);
    // }

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
