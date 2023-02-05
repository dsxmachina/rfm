use std::{
    fs::canonicalize,
    io::{stdout, Stdout, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

use cached::{Cached, SizedCache};
use crossterm::{
    cursor,
    event::{Event, EventStream},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use futures::{FutureExt, StreamExt};
use tokio::fs::read_dir;

use crate::{
    commands::{Command, CommandParser},
    panel::{DirElem, DirPanel, MillerPanels, Panel, PanelChange, PreviewPanel},
};

// Unifies the management of key-events,
// redrawing and querying content.
//
pub struct PanelManager {
    // Managed panels
    panels: MillerPanels,

    // Event-stream from the terminal
    event_reader: EventStream,

    // command-parser
    parser: CommandParser,

    // Handle to the standard-output
    stdout: Stdout,

    // Cache with directory content
    cache: SizedCache<PathBuf, Vec<DirElem>>,

    /// Weather or not to show hidden files
    show_hidden: bool,
}

/// Reads the content of a directory asynchronously
async fn directory_content(path: PathBuf) -> Result<Vec<DirElem>> {
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

impl PanelManager {
    pub fn new() -> Result<Self> {
        let stdout = stdout();
        let event_reader = EventStream::new();
        let parser = CommandParser::new();
        let mut panels = MillerPanels::new()?;
        let cache = SizedCache::with_size(100);
        panels.draw()?;

        Ok(PanelManager {
            panels,
            event_reader,
            parser,
            stdout,
            cache,
            show_hidden: false,
        })
    }

    async fn parse(&mut self, path: PathBuf) -> Vec<DirElem> {
        let result = tokio::spawn(directory_content(path.clone()));
        match result.await {
            Ok(Ok(elements)) => {
                self.cache.cache_set(path, elements.clone());
                elements
            }
            Ok(Err(_)) => {
                // TODO: Save error somewhere
                Vec::new()
            }
            Err(_) => {
                // TODO: Save error somewhere
                Vec::new()
            }
        }
    }

    fn panel_from_parent(&mut self, path: PathBuf) -> DirPanel {
        // Always use absolute paths
        if let Some(parent) = path.parent().and_then(|p| canonicalize(p).ok()) {
            // Lookup cache and reply with some panel
            if let Some(elements) = self.cache.cache_get(&path) {
                DirPanel::with_selection(elements.clone(), parent, Some(path.as_path()))
            } else {
                DirPanel::loading(path)
            }
        } else {
            DirPanel::empty()
        }
    }

    fn panel_from_path(&mut self, path: PathBuf) -> DirPanel {
        // Always use absolute paths
        if let Some(path) = canonicalize(path).ok() {
            // Lookup cache and reply with some panel
            if let Some(elements) = self.cache.cache_get(&path) {
                DirPanel::new(elements.clone(), path)
            } else {
                DirPanel::loading(path)
            }
        } else {
            DirPanel::empty()
        }
    }

    fn preview_panel<P: AsRef<Path>>(&mut self, selection: Option<P>) -> Panel {
        if let Some(path) = selection {
            let path = path.as_ref();
            if path.is_dir() {
                Panel::Dir(self.panel_from_path(path.into()))
            } else {
                Panel::Preview(PreviewPanel::new(path.into()))
            }
        } else {
            Panel::Empty
        }
    }

    async fn update_left(&mut self, path: PathBuf) -> Result<()> {
        // Prepare the temporary panel
        let left = self.panel_from_parent(path.clone());
        self.panels.update_left(left)?;

        // Then the one that will replace it
        if let Some(parent) = path.parent() {
            let left_elements = self.parse(parent.to_path_buf()).await;
            self.panels.update_left(DirPanel::with_selection(
                left_elements,
                parent.to_path_buf(),
                Some(path.as_path()),
            ))?;
        } else {
            // Otherwise use an empty panel
            self.panels.update_left(DirPanel::empty())?;
        }
        Ok(())
    }

    // Update mid automatically updates the right side
    async fn update_mid(&mut self, path: PathBuf) -> Result<()> {
        // Prepare the temporary panels
        let mid = self.panel_from_path(path.clone());
        let right = self.preview_panel(mid.selected_path());
        self.panels.update_mid(mid)?;
        self.panels.update_right(right)?;

        // Then the updated ones that will replace them
        let mid_elements = self.parse(path.clone()).await;
        let mid = DirPanel::new(mid_elements, path);

        let right = if let Some(path) = mid.selected_path() {
            if path.is_dir() {
                let right_elements = self.parse(path.to_path_buf()).await;
                Panel::Dir(DirPanel::new(right_elements, path.to_path_buf()))
            } else {
                Panel::Preview(PreviewPanel::new(path.to_path_buf()))
            }
        } else {
            Panel::Empty
        };

        self.panels.update_mid(mid)?;
        self.panels.update_right(right)?;

        Ok(())
    }

    async fn update_right<P: AsRef<Path>>(&mut self, maybe_path: Option<P>) -> Result<()> {
        // Prepare the temporary panels
        let right = self.preview_panel(maybe_path.as_ref().clone());
        self.panels.update_right(right)?;

        // Then the updated ones that will replace them
        let right = if let Some(path) = maybe_path {
            let path = path.as_ref();
            if path.is_dir() {
                let right_elements = self.parse(path.to_path_buf()).await;
                Panel::Dir(DirPanel::new(right_elements, path.to_path_buf()))
            } else {
                Panel::Preview(PreviewPanel::new(path.to_path_buf()))
            }
        } else {
            Panel::Empty
        };
        self.panels.update_right(right)?;

        Ok(())
    }

    /// Immediately response with some panel (e.g. from the cache, or an empty one),
    /// and then trigger a new parse of the filesystem under the hood.
    async fn update_panels(&mut self, change: PanelChange) -> Result<()> {
        match change {
            PanelChange::Preview(maybe_path) => {
                self.update_right(maybe_path).await?;
            }
            PanelChange::All(path) => {
                self.update_left(path.clone()).await?;
                self.update_mid(path).await?;
            }
            PanelChange::Left(path) => {
                self.update_left(path).await?;
            }
            PanelChange::Open(path) => {
                self.open(path)?;
            }
            PanelChange::None => (),
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
        self.update_panels(PanelChange::All(".".into())).await?;
        loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                maybe_event = event_reader => {
                    match maybe_event {
                        Some(Ok(event)) => {
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
                        },
                        Some(Err(e)) => {
                            println!("Error: {e}\r");
                        }
                        None => break,
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
