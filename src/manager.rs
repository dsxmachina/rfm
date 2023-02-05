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
        DirElem, DirPanel, MillerPanels, Panel, PanelAction, PanelState, PreviewPanel, Select,
    },
};

/// Reads the content of a directory asynchronously
async fn panel_content(path: PathBuf, panel: Select) -> Result<(Vec<DirElem>, Select)> {
    // read directory
    let mut dir = read_dir(path).await?;
    let mut out = Vec::new();

    while let Some(item) = dir.next_entry().await? {
        let item_path = canonicalize(item.path())?;
        out.push(DirElem::from(item_path))
    }
    out.sort();
    Ok((out, panel))
}

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
    cache: SharedCache,

    /// Receiver for incoming dir-panels
    dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,

    /// Receiver for incoming preview-panels
    prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,

    /// Sends request for new content
    content_tx: mpsc::Sender<(PathBuf, PanelState)>,

    /// Weather or not to show hidden files
    show_hidden: bool,
}

impl PanelManager {
    pub fn new(
        cache: SharedCache,
        dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,
        prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,
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
            cache,
            dir_rx,
            prev_rx,
            content_tx,
            show_hidden: false,
        })
    }

    fn tmp_panel_from_parent(&self, path: PathBuf) -> DirPanel {
        // Always use absolute paths
        if let Some(parent) = path.parent().and_then(|p| canonicalize(p).ok()) {
            // Lookup cache and reply with some panel
            if let Some(elements) = self.cache.get(&path) {
                DirPanel::with_selection(elements, parent, path.as_path())
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
            if let Some(elements) = self.cache.get(&path) {
                DirPanel::new(elements, path)
            } else {
                DirPanel::loading(path)
            }
        } else {
            DirPanel::empty()
        }
    }

    fn tmp_preview_panel<P: AsRef<Path>>(&self, selection: Option<P>) -> Panel {
        if let Some(path) = selection {
            let path = path.as_ref();
            if path.is_dir() {
                Panel::Dir(self.tmp_panel_from_path(path.into()))
            } else {
                Panel::Preview(PreviewPanel::new(path.into()))
            }
        } else {
            Panel::Empty
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
            PanelAction::UpdateAll(path) => {
                // Update left
                let left = self.tmp_panel_from_parent(path.clone());
                let mid = self.tmp_panel_from_path(path.clone());
                let right = self.tmp_preview_panel(mid.selected_path());

                let left_path = left.path().to_path_buf();
                self.panels.update_panel(
                    left,
                    PanelState {
                        state_cnt: self.panels.state_left().state_cnt + 1,
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
                        panel: Select::Right,
                    },
                );
            }
            PanelAction::UpdateLeft(path) => {
                let left = self.tmp_panel_from_parent(path.clone());
                let left_path = left.path().to_path_buf();
                self.panels.update_panel(
                    left,
                    PanelState {
                        state_cnt: self.panels.state_left().state_cnt + 1,
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
        Ok(())
    }

    fn open(&self, path: PathBuf) -> Result<()> {
        let absolute = canonicalize(path)?;
        // If the selected item is a file,
        // we need to open it
        if let Some(ext) = absolute.extension().and_then(|ext| ext.to_str()) {
            match ext {
                "png" | "bmp" | "jpg" | "jpeg" => {
                    // Notification::new()
                    // .summary(&format!("Image: {}", absolute.display()))
                    // .show()
                    // .unwrap();
                    // Image
                    std::process::Command::new("sxiv")
                        .stderr(Stdio::null())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .arg(absolute)
                        .spawn()
                        .expect("failed to run sxiv");
                }
                _ => {
                    // Notification::new()
                    //     .summary(&format!("Other: {}", absolute.display()))
                    //     .show()
                    //     .unwrap();
                    // Everything else with vim
                    std::process::Command::new("nvim")
                        .arg(absolute)
                        .spawn()
                        .expect("failed to run neovim")
                        .wait()
                        .expect("error");
                }
            }
        }
        Ok(())
    }

    pub async fn run(mut self) -> Result<()> {
        // Initialize panels
        self.panels.draw()?;

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
                    self.panels.update_preview(Panel::Preview(preview), panel_state);
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
                                // TODO!
                                // self.panels.toggle_hidden()?;
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
