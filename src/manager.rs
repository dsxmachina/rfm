use std::{
    fs::canonicalize,
    io::{stdout, Stdout, Write},
    path::PathBuf,
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

use crate::{
    commands::{Command, CommandParser},
    panel::{DirElem, DirPanel, MillerPanels, Panel, PanelChange},
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

impl PanelManager {
    pub fn new() -> Result<Self> {
        let mut stdout = stdout();
        // Start with a clear screen
        stdout
            .queue(cursor::Hide)?
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?;

        let terminal_size = terminal::size()?;
        let event_reader = EventStream::new();
        let parser = CommandParser::new();
        let mut panels = MillerPanels::new()?;
        let cache = SizedCache::with_size(100);
        panels.draw()?;

        // Flush buffer in the end
        // stdout.flush()?;

        Ok(PanelManager {
            panels,
            event_reader,
            parser,
            stdout,
            cache,
            show_hidden: false,
        })
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

    /// Immediately response with some panel (e.g. from the cache, or an empty one),
    /// and then trigger a new parse of the filesystem under the hood.
    fn update_panels(&mut self, change: PanelChange) -> Result<()> {
        match change {
            PanelChange::Preview(maybe_path) => {
                let right = Panel::from_path(maybe_path, self.show_hidden)?;
                self.panels.update_right(right)?;
            }
            PanelChange::All(path) => {
                let left = self.panel_from_parent(path.clone());
                let mid = self.panel_from_path(path);
                // TODO
                let right = Panel::from_path(mid.selected_path(), self.show_hidden)?;
                self.panels.update_left(left)?;
                self.panels.update_mid(mid)?;
                self.panels.update_right(right)?;
            }
            PanelChange::Left(path) => {
                let left = self.panel_from_parent(path.clone());
                self.panels.update_left(left)?;
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
                                        self.update_panels(change)?;
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
