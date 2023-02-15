use std::{
    collections::VecDeque,
    fs::canonicalize,
    io::{stdout, Stdout, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use cached::{Cached, SizedCache};
use crossterm::{
    cursor,
    event::{Event, EventStream},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use futures::{FutureExt, StreamExt};
use notify_rust::Notification;
use tokio::task::JoinHandle;
use tokio::{fs::read_dir, sync::mpsc};

use crate::{
    commands::{Command, CommandParser},
    content::SharedCache,
    panel::{
        BasePanel, DirElem, DirPanel, FilePreview, MillerPanels, PanelAction, PanelContent,
        PanelState, PreviewPanel, Select,
    },
};

// Unifies the management of key-events,
// redrawing and querying content.
//
pub struct PanelManager {
    /// Managed panels
    panels: MillerPanels,

    /// Event-stream from the terminal
    event_reader: EventStream,

    /// command-parser
    parser: CommandParser,

    /// Handle to the standard-output
    stdout: Stdout,

    /// Cache with directory content
    directory_cache: SharedCache<Vec<DirElem>>,

    /// Cache for file previews
    preview_cache: SharedCache<FilePreview>,

    /// Receiver for incoming dir-panels
    dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,

    /// Receiver for incoming preview-panels
    prev_rx: mpsc::Receiver<(FilePreview, PanelState)>,

    /// Sends request for new content
    content_tx: mpsc::Sender<(PathBuf, PanelState)>,
}

impl PanelManager {
    pub fn new(
        directory_cache: SharedCache<Vec<DirElem>>,
        preview_cache: SharedCache<FilePreview>,
        dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,
        prev_rx: mpsc::Receiver<(FilePreview, PanelState)>,
        content_tx: mpsc::Sender<(PathBuf, PanelState)>,
    ) -> Result<Self> {
        let stdout = stdout();
        let event_reader = EventStream::new();
        let parser = CommandParser::new();
        let panels = MillerPanels::new()?;

        Ok(PanelManager {
            panels,
            event_reader,
            parser,
            stdout,
            directory_cache,
            preview_cache,
            dir_rx,
            prev_rx,
            content_tx,
        })
    }

    fn tmp_panel_from_parent(&self, path: PathBuf) -> DirPanel {
        // Always use absolute paths
        if let Some(parent) = path.parent().and_then(|p| canonicalize(p).ok()) {
            // Lookup cache and reply with some panel
            if let Some(elements) = self.directory_cache.get(&path) {
                let mut tmp = DirPanel::new(elements, parent);
                tmp.select(path.as_path());
                tmp
            } else {
                DirPanel::loading(parent)
            }
        } else {
            DirPanel::empty()
        }
    }

    fn tmp_panel_from_path(&self, path: PathBuf) -> DirPanel {
        // Always use absolute paths
        if let Some(path) = canonicalize(path).ok() {
            // Lookup cache and reply with some panel
            if let Some(elements) = self.directory_cache.get(&path) {
                DirPanel::new(elements, path)
            } else {
                DirPanel::loading(path)
            }
        } else {
            DirPanel::empty()
        }
    }

    fn tmp_preview_panel<P: AsRef<Path>>(&self, selection: Option<P>) -> PreviewPanel {
        if let Some(path) = selection {
            let path = path.as_ref().to_path_buf();
            if path.is_dir() {
                // Check directory cache
                if let Some(elements) = self.directory_cache.get(&path) {
                    PreviewPanel::Dir(DirPanel::new(elements, path))
                } else {
                    PreviewPanel::Dir(DirPanel::loading(path))
                }
            } else {
                // Check file-preview cache
                if let Some(preview) = self.preview_cache.get(&path) {
                    PreviewPanel::File(preview)
                } else {
                    PreviewPanel::loading(path)
                }
            }
        } else {
            PreviewPanel::Dir(DirPanel::empty())
        }
    }

    // async fn update_left(&mut self, path: PathBuf) -> Result<()> {
    //     // Prepare the temporary panel
    //     let left = self.panel_from_parent(path.clone());
    //     self.panels.update_left(left)?;
    //     // Schedule parsing the directory
    //     if let Some(parent) = path.parent() {
    //         self.parse(parent.to_path_buf(), Select::Left);
    //     }
    //     Ok(())
    // }

    // // Update mid automatically updates the right side
    // async fn update_mid(&mut self, path: PathBuf) -> Result<()> {
    //     // Prepare the temporary panels
    //     let mid = self.panel_from_path(path.clone());
    //     let right = self.preview_panel(mid.selected_path());
    //     self.panels.update_mid(mid)?;
    //     self.panels.update_right(right)?;

    //     self.parse(path.clone(), Select::Mid);
    //     Ok(())
    // }

    // async fn update_right<P: AsRef<Path>>(&mut self, maybe_path: Option<P>) -> Result<()> {
    //     // Prepare the temporary panels
    //     let right = self.preview_panel(maybe_path.as_ref().clone());
    //     self.panels.update_right(right)?;
    //     if let Some(path) = maybe_path {
    //         self.parse(path.as_ref().to_path_buf(), Select::Right);
    //     }
    //     Ok(())
    // }

    /// Immediately response with some panel (e.g. from the cache, or an empty one),
    /// and then trigger a new parse of the filesystem under the hood.
    async fn update_panels(&mut self, change: PanelAction) -> Result<()> {
        match change {
            PanelAction::UpdatePreview(maybe_path) => {
                let right = self.tmp_preview_panel(maybe_path);
                let path = right.path();
                self.panels.update_preview(
                    right,
                    PanelState {
                        state_cnt: self.panels.state_right().state_cnt + 1,
                        hash: self.panels.state_right().hash,
                        panel: Select::Right,
                    },
                );
                if let Some(path) = path {
                    self.content_tx
                        .send((path, self.panels.state_right()))
                        .await
                        .expect("Receiver dropped or closed");
                }
            }
            PanelAction::UpdateMidRight((mid_path, maybe_path)) => {
                let right = self.tmp_preview_panel(maybe_path);
                let path = right.path();
                self.panels.update_preview(
                    right,
                    PanelState {
                        state_cnt: self.panels.state_right().state_cnt + 1,
                        hash: self.panels.state_right().hash,
                        panel: Select::Right,
                    },
                );
                self.content_tx
                    .send((mid_path, self.panels.state_mid()))
                    .await
                    .expect("Receiver dropped or closed");
                if let Some(path) = path {
                    self.content_tx
                        .send((path, self.panels.state_right()))
                        .await
                        .expect("Receiver dropped or closed");
                }
            }
            PanelAction::UpdateAll(path) => {
                let path = if path.is_absolute() {
                    path
                } else {
                    path.canonicalize()?
                };
                let path = path.canonicalize()?;
                // Update left
                let left = self.tmp_panel_from_parent(path.clone());
                let mid = self.tmp_panel_from_path(path.clone());
                let right = self.tmp_preview_panel(mid.selected_path());

                let left_path = left.path().to_path_buf();
                self.panels.update_panel(
                    left,
                    PanelState {
                        state_cnt: self.panels.state_left().state_cnt + 1,
                        hash: self.panels.state_left().hash,
                        panel: Select::Left,
                    },
                );
                self.content_tx
                    .send((left_path, self.panels.state_left()))
                    .await
                    .expect("Receiver dropped or closed");

                self.panels.update_panel(
                    mid,
                    PanelState {
                        state_cnt: self.panels.state_mid().state_cnt + 1,
                        hash: self.panels.state_mid().hash,
                        panel: Select::Mid,
                    },
                );
                self.content_tx
                    .send((path, self.panels.state_mid()))
                    .await
                    .expect("Receiver dropped or closed");

                self.panels.update_preview(
                    right,
                    PanelState {
                        state_cnt: self.panels.state_right().state_cnt + 1,
                        hash: self.panels.state_right().hash,
                        panel: Select::Right,
                    },
                );
            }
            PanelAction::UpdateLeft(path) => {
                let path = if path.is_absolute() {
                    path
                } else {
                    path.canonicalize()?
                };
                let left = self.tmp_panel_from_parent(path.clone());
                let left_path = left.path().to_path_buf();
                self.panels.update_panel(
                    left,
                    PanelState {
                        state_cnt: self.panels.state_left().state_cnt + 1,
                        hash: self.panels.state_left().hash,
                        panel: Select::Left,
                    },
                );
                self.content_tx
                    .send((left_path, self.panels.state_left()))
                    .await
                    .expect("Receiver dropped or closed");
            }
            PanelAction::Open(path) => {
                self.open(path)?;
            }
            PanelAction::None => (),
        }
        // Redraw panels
        self.panels.draw()?;
        Ok(())
    }

    fn open(&self, path: PathBuf) -> Result<()> {
        let absolute = if path.is_absolute() {
            path
        } else {
            path.canonicalize()?
        };
        // Image
        // If the selected item is a file,
        // we need to open it
        if let Some(ext) = absolute.extension().and_then(|ext| ext.to_str()) {
            match ext {
                "png" | "bmp" | "jpg" | "jpeg" => {
                    std::process::Command::new("sxiv")
                        .stderr(Stdio::null())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .arg(absolute.clone())
                        .spawn()
                        .expect("failed to run sxiv");
                }
                _ => {
                    // Everything else with vim
                    std::process::Command::new("nvim")
                        .arg(absolute)
                        .spawn()
                        .expect("failed to run neovim")
                        .wait()
                        .expect("error");
                }
            }
        } else {
            // Try to open things without extensions with vim
            std::process::Command::new("nvim")
                .arg(absolute)
                .spawn()
                .expect("failed to run neovim")
                .wait()
                .expect("error");
        }
        Ok(())
    }

    pub async fn run(mut self) -> Result<()> {
        // Initialize panels
        self.panels.draw()?;

        if let Some(path) = self.panels.selected_path() {
            self.content_tx
                .send((path.to_path_buf(), self.panels.state_right()))
                .await
                .expect("Receiver dropped or closed");
        }

        loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                // Check incoming new dir-panels
                result = self.dir_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break;
                    }
                    let (panel, state) = result.unwrap();
                    // Notification::new()
                    //     .summary("incoming-dir-content")
                    //     .body(&format!("{}", panel.path().display()))
                    //     .show()
                    //     .unwrap();
                    if let Select::Mid = &state.panel {
                        if let Some(path) = panel.selected_path() {
                            self.content_tx
                                .send((path.to_path_buf(), self.panels.state_right()))
                                .await
                                .expect("Receiver dropped or closed");
                        }
                    }
                    self.panels.update_panel(panel, state);
                    self.panels.draw()?;
                }
                // Check incoming new preview-panels
                result = self.prev_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break;
                    }
                    let (preview, panel_state) = result.unwrap();
                    // Notification::new()
                    //     .summary("incoming-preview")
                    //     .show()
                    //     .unwrap();
                    self.panels.update_preview(PreviewPanel::File(preview), panel_state);
                    self.panels.draw()?;
                }
                // Check incoming new events
                result = event_reader => {
                    // Shutdown if reader has been dropped
                    if result.is_none() {
                        break;
                    }
                    let event = result.unwrap()?;
                    if let Event::Key(key_event) = event {
                        match self.parser.add_event(key_event) {
                            Command::Move(direction) => {
                                let change = self.panels.move_cursor(direction);
                                self.update_panels(change).await?;
                            }
                            Command::ToggleHidden => {
                                self.panels.toggle_hidden()?;
                            }
                            Command::Quit => break,
                            Command::None => (),
                        }
                    }
                    if let Event::Resize(sx, sy) = event {
                        self.panels.terminal_resize((sx, sy))?;
                    }
                }
            }
        }
        // Cleanup after leaving this function
        self.stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?
            .queue(cursor::Show)?
            .flush()?;
        Ok(())
    }
}
