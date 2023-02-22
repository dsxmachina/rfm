use cached::{cached, cached_result, Cached, SizedCache, TimedSizedCache};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::{
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    sync::Arc,
};
use tokio::{sync::mpsc, task::spawn_blocking};

use crate::panel::{
    DirElem, DirPanel, FilePreview, PanelContent, PanelState, PanelUpdate, PreviewPanel,
};

/// Cache that is shared by the content-manager and the panel-manager.
#[derive(Clone)]
pub struct SharedCache<Item: Clone> {
    inner: Arc<Mutex<SizedCache<PathBuf, Item>>>,
}

impl<Item: Clone> SharedCache<Item> {
    pub fn with_size(size: usize) -> Self {
        SharedCache {
            inner: Arc::new(Mutex::new(SizedCache::with_size(size))),
        }
    }

    pub fn get(&self, path: &PathBuf) -> Option<Item> {
        self.inner.lock().cache_get(path).cloned()
    }

    pub fn insert(&self, path: PathBuf, item: Item) -> Option<Item> {
        self.inner.lock().cache_set(path, item)
    }
}

/// Receives commands to parse the directory or generate a new preview.
pub struct Manager {
    preview_rx: mpsc::UnboundedReceiver<PanelUpdate>,
    directory_rx: mpsc::UnboundedReceiver<PanelUpdate>,

    dir_tx: mpsc::Sender<(DirPanel, PanelState)>,

    prev_tx: mpsc::Sender<(PreviewPanel, PanelState)>,

    directory_cache: SharedCache<DirPanel>,
    preview_cache: SharedCache<PreviewPanel>,
}

pub fn dir_content(path: PathBuf) -> Vec<DirElem> {
    // read directory
    match std::fs::read_dir(path) {
        Ok(dir) => {
            let mut out = Vec::new();
            for item in dir.into_iter().flatten() {
                out.push(DirElem::from(item.path()))
            }
            // out.sort();
            out.sort_by_cached_key(|a| a.name_lowercase().clone());
            out.sort_by_cached_key(|a| !a.path().is_dir());
            out
        }
        Err(_) => Vec::new(),
    }
}

fn dir_content_preview(path: PathBuf, max_elem: usize) -> Vec<DirElem> {
    // read directory
    match std::fs::read_dir(path) {
        Ok(dir) => {
            let mut out = Vec::new();
            for item in dir.into_iter().flatten().take(max_elem) {
                out.push(DirElem::from(item.path()))
            }
            // out.sort();
            out.sort_by_cached_key(|a| a.name_lowercase().clone());
            out.sort_by_cached_key(|a| !a.path().is_dir());
            out
        }
        Err(_) => Vec::new(),
    }
}

cached! {
    FILE_PREVIEW: TimedSizedCache<PathBuf, FilePreview> = TimedSizedCache::with_size_and_lifespan(10, 5);
    fn get_file_preview(path: PathBuf) -> FilePreview = {
        FilePreview::new(path)
    }
}

pub fn hash_elements(elements: &[DirElem]) -> u64 {
    let mut h: fasthash::XXHasher = Default::default();
    for elem in elements {
        elem.name().hash(&mut h);
    }
    h.finish()
}

impl Manager {
    pub fn new(
        directory_cache: SharedCache<DirPanel>,
        preview_cache: SharedCache<PreviewPanel>,
        directory_rx: mpsc::UnboundedReceiver<PanelUpdate>,
        preview_rx: mpsc::UnboundedReceiver<PanelUpdate>,
        dir_tx: mpsc::Sender<(DirPanel, PanelState)>,
        prev_tx: mpsc::Sender<(PreviewPanel, PanelState)>,
    ) -> Self {
        Manager {
            directory_rx,
            preview_rx,
            dir_tx,
            prev_tx,
            directory_cache,
            preview_cache,
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                biased;
                result = self.directory_rx.recv() => {
                    if result.is_none() {
                        break;
                    }
                    let update = result.unwrap();
                    if !update.state.path().is_dir() {
                        continue;
                    }
                    // Notification::new().summary("dir-request").body(&format!("{}", update.state.cnt)).show().unwrap();

                    // Then create the full version
                    let dir_path = update.state.path().clone();
                    let result = spawn_blocking(move || dir_content(dir_path)).await;
                    if let Ok(content) = result {
                        // Only update when the hash has changed
                        let panel = DirPanel::new(content, update.state.path().clone());
                        if update.state.hash() != panel.content_hash() {
                            if self.dir_tx.send((panel.clone(), update.state.increased().increased())).await.is_err() {break;};
                        } else {
                            // Notification::new().summary("unchanged hash").body(&format!("{}", update.state.hash())).show().unwrap();
                        }
                        self.directory_cache.insert(update.state.path().clone(), panel.clone());
                        self.preview_cache.insert(update.state.path(), PreviewPanel::Dir(panel));
                    }
                }
                result = self.preview_rx.recv() => {
                    if result.is_none() {
                        break;
                    }
                    let update = result.unwrap();
                    if update.state.path().is_dir() {
                        let dir_path = update.state.path().clone();
                        let result = spawn_blocking(move || dir_content_preview(dir_path, 16538)).await;
                        if let Ok(content) = result {
                            let panel = PreviewPanel::Dir(DirPanel::new(content, update.state.path().clone()));
                            if update.state.hash() != panel.content_hash() {
                                if self.prev_tx.send((panel.clone(), update.state.increased())).await.is_err() { break; };
                            }
                            self.preview_cache.insert(update.state.path(), panel);
                        }
                    } else {
                        // Create preview
                        let file_path = update.state.path().clone();
                        let result = spawn_blocking(move || get_file_preview(file_path)).await;
                        if let Ok(preview) = result {
                            let panel = PreviewPanel::File(preview);
                            if update.state.hash() != panel.content_hash() {
                                if self.prev_tx.send((panel.clone(), update.state.increased())).await.is_err() { break; };
                            }
                            self.preview_cache.insert(update.state.path(), panel);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use patricia_tree::{PatriciaMap, PatriciaSet};
    use std::time::Instant;

    #[test]
    fn test_dir_hashing_speed() {
        let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
        // read directory
        let content = dir_content(path).unwrap();
        let now = Instant::now();
        let hash = hash_elements(&content);
        let elapsed = now.elapsed().as_millis();
        println!("hashing {} elements took: {elapsed}ms", content.len(),);
        println!("hash={hash}");
        assert!(true);
    }

    #[test]
    fn test_dir_parsing_speed() {
        let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
        // read directory
        let now = Instant::now();
        let content = dir_content(path).unwrap();
        let elapsed = now.elapsed().as_millis();
        println!("parsing {} elements took: {elapsed}ms", content.len(),);
        assert!(false);
    }

    #[test]
    fn test_patricia_tree_speed() {
        let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
        // read directory
        let content = dir_content(path).unwrap();
        let mut set = PatriciaSet::new();
        let now = Instant::now();
        for item in content {
            set.insert(item.name_lowercase());
        }
        let elapsed = now.elapsed().as_millis();
        println!(
            "building tree from {} elements took: {elapsed}ms",
            set.len(),
        );
        assert!(false);
    }

    #[test]
    fn test_patricia_map_speed() {
        let path: PathBuf = "/home/someone/Bilder/ground_images/-3000_-2000_3000_2000_0".into();
        // read directory
        let content = dir_content(path).unwrap();
        let mut map = PatriciaMap::new();
        let now = Instant::now();
        for (idx, item) in content.iter().enumerate() {
            map.insert(item.name_lowercase(), idx);
        }
        let elapsed = now.elapsed().as_millis();
        println!("building map from {} elements took: {elapsed}ms", map.len(),);
        assert!(false);
    }

    #[test]
    fn test_image_load_speed() {
        let now = Instant::now();
        let img = image::io::Reader::open("/home/someone/Bilder/wallpaper_source/abstract/hologram_scheme_scifi_139294_1920x1080.jpg").unwrap().decode().unwrap();
        let elapsed = now.elapsed().as_millis();
        println!("loading image took {elapsed}ms");
        let now = Instant::now();
        let _small_img = img.thumbnail_exact(400, 300).into_rgb8();
        let elapsed = now.elapsed().as_millis();
        println!("processing image took {elapsed}ms");
        assert!(true);
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
