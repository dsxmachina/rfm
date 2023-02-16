use cached::{cached, cached_result, Cached, SizedCache, TimedSizedCache};
use parking_lot::Mutex;
use std::{
    hash::{Hash, Hasher},
    io,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::{sync::mpsc, task::spawn_blocking, time::timeout};

use crate::panel::{
    DirElem, DirPanel, FilePreview, PanelContent, PanelState, PanelUpdate, PreviewPanel,
};

const PREVIEW_TIMEOUT: Duration = Duration::from_secs(30);

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
        self.cache.lock().cache_get(path).cloned()
    }

    pub fn insert(&self, path: PathBuf, item: Item) -> Option<Item> {
        self.cache.lock().cache_set(path, item)
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

pub fn hash_elements(elements: &[DirElem]) -> u64 {
    // let mut h: MetroHasher = Default::default();
    let mut h: fasthash::XXHasher = Default::default();
    for elem in elements.iter() {
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
                    if !update.path.is_dir() {
                        continue;
                    }
                    // Notification::new().summary("recv update-request").body(&format!("{:?}", update.state)).show().unwrap();
                    let dir_path = update.path.clone();
                    let result = spawn_blocking(move || dir_content(dir_path)).await;
                    if let Ok(Ok(content)) = result {
                        // Only update when the hash has changed
                        let panel = DirPanel::new(content, update.path.clone());
                        if update.hash != panel.content_hash() {
                            self.dir_tx.send((panel.clone(), update.state.increased())).await.expect("Receiver dropped or closed");
                        } else {
                            // Notification::new().summary("unchanged hash").body(&format!("{}", update.hash)).show().unwrap();
                        }
                        self.directory_cache.insert(update.path, panel);
                    }
                }
                result = self.preview_rx.recv() => {
                    if result.is_none() {
                        break;
                    }
                    let update = result.unwrap();
                    if update.path.is_dir() {
                        // Notification::new()
                        //     .summary("Request Dir-Preview")
                        //     .body(&format!("{}", update.path.display()))
                        //     .show()
                        //     .unwrap();
                        let dir_path = update.path.clone();
                        let result = timeout(PREVIEW_TIMEOUT, spawn_blocking(move || dir_content_preview(dir_path, 16538))).await;
                        if let Ok(Ok(Ok(content))) = result {
                            let panel = PreviewPanel::Dir(DirPanel::new(content, update.path.clone()));
                            if update.hash != panel.content_hash() {
                                self.prev_tx.send((panel.clone(), update.state.increased())).await.expect("Receiver dropped or closed");
                            }
                            self.preview_cache.insert(update.path, panel);
                        }
                    } else {
                        // Create preview
                        let file_path = update.path.clone();
                        let result = timeout(PREVIEW_TIMEOUT, spawn_blocking(move || get_file_preview(file_path))).await;
                        if let Ok(Ok(preview)) = result {
                            let panel = PreviewPanel::File(preview);
                            if update.hash != panel.content_hash() {
                                self.prev_tx.send((panel.clone(), update.state.increased())).await.expect("Receiver dropped or closed");
                            }
                            self.preview_cache.insert(update.path, panel);
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
        let _small_img = img.thumbnail_exact(400, 300).into_rgb8();
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
