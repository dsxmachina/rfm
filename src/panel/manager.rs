use std::fs::OpenOptions;

use crossterm::{
    cursor,
    event::{Event, EventStream, KeyCode},
    terminal::{self, Clear, ClearType},
    QueueableCommand,
};
use futures::{FutureExt, StreamExt};
use log::{debug, error, info, trace, warn};
use tempfile::TempDir;

use crate::{
    engine::commands::{CloseCmd, Command, CommandParser},
    engine::OpenEngine,
    logger::LogBuffer,
    util::{copy_item, get_destination, move_item},
};

use self::console::{Console, ConsoleOp, DirConsole, Zoxide};

use super::{
    compositor::{Compositor, LayerId},
    input::Input,
    rect::Rect,
    widgets::{DirPanelWidget, PreviewPanelWidget, HeaderWidget, FooterWidget, InputBarWidget, InputType, LogWidget, LogEntry},
    *,
};

enum Mode {
    Normal,
    Console { console: Box<dyn Console> },
    CreateItem { input: Input, is_dir: bool },
    Search { input: Input },
    Rename { input: Input },
}

struct Clipboard {
    /// Items we put into the clipboard
    files: Vec<PathBuf>,
    /// Whether to cut or copy the items.
    cut: bool,
}

pub struct PanelManager {
    /// Left panel
    left: ManagedPanel<DirPanel>,
    /// Center panel
    center: ManagedPanel<DirPanel>,
    /// Right panel
    right: ManagedPanel<PreviewPanel>,

    /// Mode of operation
    mode: Mode,

    opener: OpenEngine,

    logger: LogBuffer,

    /// Clipboard
    clipboard: Option<Clipboard>,

    /// Miller-Columns layout
    layout: MillerColumns,

    /// Show hidden files
    show_hidden: bool,

    /// Show log
    show_log: bool,

    /// Console needs redraw (special case - renders separately)
    console_dirty: bool,

    /// Event-stream from the terminal
    event_reader: EventStream,

    /// History when going "forward"
    fwd_history: Vec<(PathBuf, PathBuf)>,

    /// History when going "backwards"
    rev_history: Vec<PathBuf>,

    /// Previous path
    previous: PathBuf,
    pre_console_path: PathBuf,

    /// Trash directory. If `None`, the trash mechanism should not be used.
    trash_dir: Option<TempDir>,

    /// command-parser
    parser: CommandParser,

    /// Handle to the standard-output
    stdout: Stdout,

    /// Receiver for incoming dir-panels
    dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,

    /// Receiver for incoming preview-panels
    prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,

    /// Compositor for all rendering
    compositor: Compositor,
    /// Layer ID for header
    header_layer: LayerId,
    /// Layer ID for footer
    footer_layer: LayerId,
    /// Layer ID for left panel
    left_layer: LayerId,
    /// Layer ID for center panel
    center_layer: LayerId,
    /// Layer ID for right panel
    right_layer: LayerId,
    /// Layer ID for log display
    log_layer: LayerId,
    /// Layer ID for input bar (search, rename, etc.)
    input_bar_layer: LayerId,
}

impl PanelManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        miller_panels: MillerPanels,
        use_trash: bool,
        parser: CommandParser,
        dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,
        prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,
        logger: LogBuffer,
        opener: OpenEngine,
    ) -> Result<Self> {
        // Prepare terminal
        let stdout = stdout();
        let event_reader = EventStream::new();
        let terminal_size = terminal::size()?;
        let layout = MillerColumns::from_size(terminal_size);

        // Split panels
        let (left, center, right) = miller_panels;

        // TODO: If the user has multiple disks, the temp-dir may be on another disk,
        // so deleting would effectively be a copy - which is not what we want here.
        // Add a mechanism to check, if the file that should get deleted is on the same disk or not
        //
        // -> For now we mark the feature as experimental and turn it off by default
        let trash_dir = if use_trash {
            let trash_dir = tempfile::tempdir()?;
            debug!("Using {} as temporary trash", trash_dir.path().display());
            Some(trash_dir)
        } else {
            None
        };

        // Initialize compositor with all widgets
        let mut compositor = Compositor::new();

        // Header and footer (z-index: BARS = 10)
        let header_layer = compositor.add_layer(
            Box::new(HeaderWidget::new()),
            layout.header_rect(),
        );
        let footer_layer = compositor.add_layer(
            Box::new(FooterWidget::new()),
            layout.footer_rect(),
        );

        // Panels (z-index: PANELS = 20)
        let left_layer = compositor.add_layer(
            Box::new(DirPanelWidget::new(left.panel().clone())),
            layout.left_rect(),
        );
        let center_layer = compositor.add_layer(
            Box::new(DirPanelWidget::new(center.panel().clone())),
            layout.center_rect(),
        );
        let right_layer = compositor.add_layer(
            Box::new(PreviewPanelWidget::new(right.panel().clone())),
            layout.right_rect(),
        );

        // Log display (z-index: LOG = 30) - initially hidden
        let log_layer = compositor.add_layer(
            Box::new(LogWidget::new()),
            layout.footer_rect(), // Will be positioned dynamically
        );
        compositor.hide_layer(log_layer);

        // Input bar for search/rename/etc (z-index: CONSOLE = 40) - initially hidden
        let input_bar_layer = compositor.add_layer(
            Box::new(InputBarWidget::new(InputType::Search)),
            layout.footer_rect(),
        );
        compositor.hide_layer(input_bar_layer);

        Ok(PanelManager {
            left,
            center,
            right,
            mode: Mode::Normal,
            logger,
            clipboard: None,
            layout,
            opener,
            show_hidden: false,
            show_log: false,
            console_dirty: false,
            event_reader,
            fwd_history: Vec::new(),
            rev_history: Vec::new(),
            previous: ".".into(),
            pre_console_path: ".".into(),
            trash_dir,
            parser,
            stdout,
            dir_rx,
            prev_rx,
            compositor,
            header_layer,
            footer_layer,
            left_layer,
            center_layer,
            right_layer,
            log_layer,
            input_bar_layer,
        })
    }

    /// Mark that panels need to be redrawn (all three + header/footer).
    fn mark_panels_dirty(&mut self) {
        self.compositor.mark_layer_dirty(self.left_layer);
        self.compositor.mark_layer_dirty(self.center_layer);
        self.compositor.mark_layer_dirty(self.right_layer);
        self.compositor.mark_layer_dirty(self.header_layer);
        self.compositor.mark_layer_dirty(self.footer_layer);
        self.compositor.mark_layer_dirty(self.log_layer);
    }

    /// Mark center panel and related UI elements as dirty.
    fn mark_center_dirty(&mut self) {
        self.compositor.mark_layer_dirty(self.center_layer);
        self.compositor.mark_layer_dirty(self.header_layer);
        self.compositor.mark_layer_dirty(self.footer_layer);
    }

    /// Mark everything as needing redraw.
    fn mark_all_dirty(&mut self) {
        self.compositor.mark_all_dirty();
        self.console_dirty = true;
    }

    fn draw(&mut self) -> Result<()> {
        // Sync all state to compositor widgets (widgets track their own dirty state)
        self.sync_to_compositor();

        // Render via compositor (only redraws dirty layers)
        self.compositor.render(&mut self.stdout)?;

        // Console renders separately (interactive overlay)
        if self.console_dirty {
            self.draw_console()?;
            self.console_dirty = false;
        }

        Ok(())
    }

    /// Sync all state to compositor widgets.
    ///
    /// Widgets internally track whether their state changed and only mark
    /// themselves dirty if the new state differs from the old.
    fn sync_to_compositor(&mut self) {
        // Calculate height adjustment for log display
        let height_adjustment = if self.show_log {
            self.logger.capacity() as u16
        } else {
            0
        };

        // Sync header - widget checks if path changed
        let path = self.center.panel().selected_path()
            .and_then(|f| f.canonicalize().ok())
            .unwrap_or_else(|| self.center.panel().path().to_path_buf());
        self.compositor.with_widget_mut(self.header_layer, |w| {
            if let Some(widget) = w.as_any_mut().downcast_mut::<HeaderWidget>() {
                widget.set_path(path);
            }
        });

        // Sync footer and input bar based on mode
        match &self.mode {
            Mode::Normal => {
                // Hide input bar, show footer
                self.compositor.hide_layer(self.input_bar_layer);
                self.compositor.show_layer(self.footer_layer);

                let selected_path = self.center.panel().selected_path();
                let key_buffer = self.parser.buffer();
                let (n, m) = self.center.panel().index_vs_total();

                self.compositor.with_widget_mut(self.footer_layer, |w| {
                    if let Some(widget) = w.as_any_mut().downcast_mut::<FooterWidget>() {
                        widget.set_file_info(selected_path);
                        widget.set_key_buffer(key_buffer);
                        widget.set_position(n, m);
                    }
                });
            }
            Mode::Search { input } | Mode::Rename { input } | Mode::CreateItem { input, .. } => {
                // Show input bar, hide footer
                self.compositor.show_layer(self.input_bar_layer);
                self.compositor.hide_layer(self.footer_layer);

                let input_type = match &self.mode {
                    Mode::Search { .. } => InputType::Search,
                    Mode::Rename { .. } => InputType::Rename,
                    Mode::CreateItem { is_dir: true, .. } => InputType::Mkdir,
                    Mode::CreateItem { is_dir: false, .. } => InputType::Touch,
                    _ => InputType::Search,
                };

                self.compositor.with_widget_mut(self.input_bar_layer, |w| {
                    if let Some(widget) = w.as_any_mut().downcast_mut::<InputBarWidget>() {
                        widget.set_input_type(input_type);
                        widget.set_text(input.get().to_string(), input.get().len());
                    }
                });
            }
            Mode::Console { .. } => {
                // In console mode, hide both - console renders itself
                self.compositor.hide_layer(self.input_bar_layer);
                self.compositor.hide_layer(self.footer_layer);
            }
        }

        // Sync log display
        if self.show_log {
            self.compositor.show_layer(self.log_layer);
            let log_height = self.logger.capacity() as u16;
            let log_y = self.layout.footer().saturating_sub(log_height);
            let log_area = Rect::new(0, log_y, self.layout.width(), log_height);
            self.compositor.set_layer_area(self.log_layer, log_area);

            self.compositor.with_widget_mut(self.log_layer, |w| {
                if let Some(widget) = w.as_any_mut().downcast_mut::<LogWidget>() {
                    let entries: Vec<LogEntry> = self.logger.get()
                        .into_iter()
                        .map(|(level, msg)| LogEntry { level, message: msg })
                        .collect();
                    widget.set_entries(entries);
                    widget.set_show_all(true);
                }
            });
        } else {
            // Still show single warning/error if present
            let has_warning = self.logger.get()
                .into_iter()
                .any(|(level, _)| level <= log::Level::Warn);

            if has_warning {
                self.compositor.show_layer(self.log_layer);
                let log_y = self.layout.footer().saturating_sub(2);
                let log_area = Rect::new(0, log_y, self.layout.width(), 1);
                self.compositor.set_layer_area(self.log_layer, log_area);

                self.compositor.with_widget_mut(self.log_layer, |w| {
                    if let Some(widget) = w.as_any_mut().downcast_mut::<LogWidget>() {
                        let entries: Vec<LogEntry> = self.logger.get()
                            .into_iter()
                            .map(|(level, msg)| LogEntry { level, message: msg })
                            .collect();
                        widget.set_entries(entries);
                        widget.set_show_all(false);
                    }
                });
            } else {
                self.compositor.hide_layer(self.log_layer);
            }
        }

        // Sync panels - widgets track if content changed
        let left_area = self.layout.left_rect().with_height_reduced(height_adjustment);
        self.compositor.set_layer_area(self.left_layer, left_area);
        self.compositor.with_widget_mut(self.left_layer, |w| {
            if let Some(widget) = w.as_any_mut().downcast_mut::<DirPanelWidget>() {
                widget.set_panel(self.left.panel().clone());
            }
        });

        let center_area = self.layout.center_rect().with_height_reduced(height_adjustment);
        self.compositor.set_layer_area(self.center_layer, center_area);
        self.compositor.with_widget_mut(self.center_layer, |w| {
            if let Some(widget) = w.as_any_mut().downcast_mut::<DirPanelWidget>() {
                widget.set_panel(self.center.panel().clone());
            }
        });

        let right_area = self.layout.right_rect().with_height_reduced(height_adjustment);
        self.compositor.set_layer_area(self.right_layer, right_area);
        self.compositor.with_widget_mut(self.right_layer, |w| {
            if let Some(widget) = w.as_any_mut().downcast_mut::<PreviewPanelWidget>() {
                widget.set_panel(self.right.panel().clone());
            }
        });
    }

    fn draw_console(&mut self) -> Result<()> {
        // Console renders itself (complex interactive widget)
        if let Mode::Console { console } = &mut self.mode {
            console.draw(
                &mut self.stdout,
                self.layout.left_x_range.start..self.layout.right_x_range.end,
                self.layout.y_range.clone(),
            )?;
        }
        Ok(())
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.left.panel_mut().set_hidden(self.show_hidden);
        self.center.panel_mut().set_hidden(self.show_hidden);
        if let PreviewPanel::Dir(panel) = self.right.panel_mut() {
            panel.set_hidden(self.show_hidden);
        };
        // FIX: Re-selecting path. If we are in a hidden directory, we want to re-select the
        // correct path in the left panel.
        self.left.panel_mut().select_path(
            self.center.panel().path(),
            Some(self.center.panel().selected_idx()),
        );
        self.mark_all_dirty();
    }

    fn toggle_log(&mut self) {
        self.show_log = !self.show_log;
        if self.show_log {
            self.compositor.mark_layer_dirty(self.log_layer);
        } else {
            // Redraw everything, so that the current log gets overdrawn by the panels
            self.mark_all_dirty();
        }
    }

    fn move_up(&mut self, step: usize) {
        trace!("move-up");
        if self.center.panel_mut().up(step) {
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.mark_center_dirty();
            self.compositor.mark_layer_dirty(self.right_layer);
            self.rev_history.clear();
        }
    }

    fn move_down(&mut self, step: usize) {
        trace!("move-down");
        if self.center.panel_mut().down(step) {
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.mark_center_dirty();
            self.compositor.mark_layer_dirty(self.right_layer);
            self.rev_history.clear();
        }
    }

    fn move_right(&mut self) {
        trace!("move-right");
        if let Some(selected) = self.center.panel().selected_path().map(|p| p.to_path_buf()) {
            // If the selected item is a directory, all panels will shift to the left
            if selected.is_dir() {
                self.previous = self.center.panel().path().to_path_buf();
                debug!(
                    "push to history: {}, len={}",
                    self.previous.display(),
                    self.fwd_history.len()
                );

                // Remember forward history
                self.fwd_history.push((
                    self.left.panel().path().to_owned(),
                    self.left
                        .panel()
                        .selected_path()
                        .map(|p| p.to_owned())
                        .unwrap_or_default(),
                ));
                self.left.update_panel(self.center.panel().clone());
                self.center
                    .new_panel_instant(self.right.panel().maybe_path());

                if let Some(path) = self.rev_history.pop() {
                    info!(
                        "pop rev-history: {}, len={}",
                        path.display(),
                        self.rev_history.len()
                    );
                    info!("set-center-panel selection");
                    self.center.panel_mut().select_path(&path, None);
                }

                self.right
                    .new_panel_delayed(self.center.panel().selected_path());

                if let Some(path) = self.rev_history.last() {
                    info!("set-right-panel selection");
                    self.right.panel_mut().select_path(path);
                }

                self.mark_panels_dirty();
            } else {
                // NOTE: This is a blocking call, if we have a terminal application.
                // The watchers are still active in the background.
                // If the appication somehow triggers a watcher (e.g. by creating a swapfile),
                // the panel-update is never applied, which means the "state-counter" is never increased.
                // Any subsequent call to "update_panel", will go out with the same (old) state-counter,
                // which results in the "real" panel updates being ignored (because their counter is equal to the first update),
                // when the opener.open(...) function returns.
                // This is the reason, why we always see the swapfile after leaving vim atm.
                //
                // Solution:
                // "Freeze" the panel and deactivate the watchers while the open function is blocked.
                info!("Opening '{}'", selected.display());
                self.center.freeze();

                // Change working directory so that child processes gets spawned from the currently active directory.
                if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                    error!("Failed to set working-directory for process: {e}");
                }
                if let Err(e) = self.opener.open(selected) {
                    /* failed to open selected */
                    error!("Opening failed: {e}");
                }
                self.center.unfreeze();
                self.mark_all_dirty();
            }
            //
            self.unmark_left_right();
        }
    }

    fn move_left(&mut self) {
        trace!("move-left");
        // If the left panel is empty, we cannot move left:
        if self.left.panel().selected_path().is_none() {
            return;
        }
        if let Some(path) = self.right.panel().maybe_path() {
            info!(
                "push to rev-history: {}, len={}",
                path.display(),
                self.rev_history.len()
            );
            self.rev_history.push(path);
        }
        self.previous = self.center.panel().path().to_path_buf();
        self.right
            .update_panel(PreviewPanel::Dir(self.center.panel().clone()));
        self.center.update_panel(self.left.panel().clone());
        // | m | l | m |
        // TODO: When we followed some symlink we don't want to take the parent here.
        match self.fwd_history.pop() {
            Some((previous, selected)) => {
                debug!(
                    "using history: {}, selected={}, len={}",
                    previous.display(),
                    selected.display(),
                    self.fwd_history.len()
                );
                self.left.new_panel_instant(Some(previous));
                info!("set-left-panel selection");
                self.left.panel_mut().select_path(&selected, None);
            }
            None => {
                let parent = self.center.panel().path().parent();
                info!("using parent: {:?}", parent);
                self.left.new_panel_instant(parent);
                info!("set-left-panel selection");
                self.left
                    .panel_mut()
                    .select_path(self.center.panel().path(), None);
            }
        }

        self.unmark_left_right();

        // All panels needs to be redrawn
        self.mark_panels_dirty();
    }

    fn jump(&mut self, path: PathBuf) {
        trace!("jump-to {}", path.display());
        // Don't do anything, if the path hasn't changed
        if path.as_path() == self.center.panel().path() {
            return;
        }
        if path.exists() {
            self.fwd_history.clear(); // Delete history when jumping
            self.rev_history.clear();
            self.previous = self.center.panel().path().to_path_buf();
            self.left.new_panel_instant(path.parent());
            self.left.panel_mut().select_path(&path, None);
            self.center.new_panel_instant(Some(&path));
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.mark_panels_dirty();
        }
    }

    fn move_cursor(&mut self, movement: Move) {
        // NOTE: Movement functions needs to determine which panels require a redraw.
        match movement {
            Move::Up => self.move_up(1),
            Move::Down => self.move_down(1),
            Move::Left => self.move_left(),
            Move::Right => self.move_right(),
            Move::Top => self.move_up(usize::MAX),
            Move::Bottom => self.move_down(usize::MAX),
            Move::HalfPageForward => self.move_down(self.layout.height() as usize / 2),
            Move::HalfPageBackward => self.move_up(self.layout.height() as usize / 2),
            Move::PageForward => self.move_down(self.layout.height() as usize),
            Move::PageBackward => self.move_up(self.layout.height() as usize),
            Move::JumpTo(path) => self.jump(path.into()),
            Move::JumpPrevious => self.jump(self.previous.clone()),
        };
    }

    /// Returns a reference to all marked items.
    fn marked_items(&self) -> Vec<&DirElem> {
        let mut out = Vec::new();
        out.extend(self.left.panel().elements().filter(|e| e.is_marked()));
        out.extend(self.center.panel().elements().filter(|e| e.is_marked()));
        if let PreviewPanel::Dir(panel) = self.right.panel() {
            out.extend(panel.elements().filter(|e| e.is_marked()))
        }
        out
    }

    /// Unmarks all items in all panels
    fn unmark_all_items(&mut self) {
        self.center
            .panel_mut()
            .elements_mut()
            .for_each(|item| item.unmark());
        self.unmark_left_right();
    }

    /// Unmarks all items in the left and right panels.
    fn unmark_left_right(&mut self) {
        self.left
            .panel_mut()
            .elements_mut()
            .for_each(|item| item.unmark());

        if let PreviewPanel::Dir(panel) = self.right.panel_mut() {
            panel.elements_mut().for_each(|item| item.unmark());
        }
        self.mark_panels_dirty();
    }

    /// Returns all marked paths *or* the selected path.
    ///
    /// Note: This is an exclusive or - the selected path is not
    /// returned, when there are marked paths.
    /// If there are no marked paths, the selected path is automatically
    /// marked - and therefore it is returned by this function.
    fn marked_or_selected(&mut self) -> Vec<PathBuf> {
        let files: Vec<PathBuf> = self
            .marked_items()
            .iter()
            .map(|item| item.path().to_path_buf())
            .collect();
        // If we have nothing marked, take the current selection
        if files.is_empty() {
            self.center.panel_mut().mark_selected_item();
            if let Some(path) = self.center.panel().selected_path() {
                vec![path.to_path_buf()]
            } else {
                Vec::new()
            }
        } else {
            files
        }
    }

    /// Deletes a file or directory, based on the trash strategy.
    fn delete_file(&self, file: &Path) {
        // Check if we use the trash or not
        if let Some(trash_path) = &self.trash_dir {
            let destination = get_destination(file, trash_path.path()).unwrap();
            let result = std::fs::rename(file, &destination);
            if let Err(e) = result {
                error!("Cannot delete {}: {e}", file.display());
            }
        } else {
            if file.is_file() {
                let result = std::fs::remove_file(file);
                if let Err(e) = result {
                    error!("Cannot delete {}: {e}", file.display());
                }
            } else if file.is_dir() {
                let result = std::fs::remove_dir_all(file);
                if let Err(e) = result {
                    error!("Cannot delete {}: {e}", file.display());
                }
            }
        }
    }

    pub async fn run(mut self) -> Result<CloseCmd> {
        // Initial draw
        self.mark_all_dirty();
        self.draw()?;

        let close_cmd = loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                // Check incoming new logs
                () = self.logger.update() => {
                    self.compositor.mark_layer_dirty(self.log_layer);
                }
                // Check incoming new dir-panels
                result = self.dir_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break CloseCmd::QuitErr { error: "DirPanel receiver has been dropped" };
                    }
                    let (panel, state) = result.unwrap();

                    // Find panel and update it
                    if self.center.check_update(&state) {
                        self.center.update_panel(panel);
                        // update preview (if necessary)
                        self.right.new_panel_delayed(self.center.panel().selected_path());
                        self.mark_center_dirty();
                        self.compositor.mark_layer_dirty(self.right_layer);
                        self.console_dirty = true;
                    } else if self.left.check_update(&state) {
                        self.left.update_panel(panel);
                        self.left.panel_mut().select_path(self.center.panel().path(), Some(self.center.panel().selected_idx()));
                        self.compositor.mark_layer_dirty(self.left_layer);
                        self.console_dirty = true;
                    } else {
                        // Reduce log level here, this is not that important
                        debug!("unknown panel update: {:?}", state);
                    }
                }
                // Check incoming new preview-panels
                result = self.prev_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break CloseCmd::QuitErr { error: "Preview receiver has been dropped" };
                    }
                    let (panel, state) = result.unwrap();

                    if self.right.check_update(&state) {
                        self.right.update_panel(panel);
                        self.compositor.mark_layer_dirty(self.right_layer);
                        self.console_dirty = true;
                    }
                }
                // Check incoming new events
                result = event_reader => {
                    // Shutdown if reader has been dropped
                    match result {
                        Some(event) => {
                            if let Some(close_cmd) = self.handle_event(event?)? {
                                break close_cmd;
                            }
                        }
                        None => break CloseCmd::QuitErr { error: "event-reader has been dropped" },
                    }
                }
            }
            // Always redraw what needs to be redrawn
            self.draw()?;
        };
        // Cleanup after leaving this function
        self.stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?
            .queue(cursor::Show)?
            .flush()?;

        Ok(close_cmd)
    }

    /// Handles the terminal events.
    ///
    /// Returns Ok(true) if the application needs to shut down.
    fn handle_event(&mut self, event: Event) -> Result<Option<CloseCmd>> {
        if let Event::Key(key_event) = event {
            // If we hit escape - go back to normal mode.
            if let KeyCode::Esc = key_event.code {
                if let Mode::Console { .. } = self.mode {
                    self.jump(self.pre_console_path.clone());
                }
                self.mode = Mode::Normal;
                self.parser.clear();
                self.center.panel_mut().clear_search();
                self.center.panel_mut().clear_new_element();
                self.mark_panels_dirty();
                self.compositor.mark_layer_dirty(self.footer_layer);
                self.unmark_all_items();
            }
            match &mut self.mode {
                Mode::Normal => {
                    match self.parser.add_event(key_event) {
                        Command::Move(direction) => {
                            self.move_cursor(direction);
                        }
                        Command::ViewTrash => {
                            if let Some(trash_path) = &self.trash_dir {
                                self.jump(trash_path.path().to_path_buf());
                            } else {
                                warn!("Trash feature is not activated - therefore there is no trash-directory to jump to.")
                            }
                        }
                        Command::ToggleHidden => self.toggle_hidden(),
                        Command::ToggleLog => self.toggle_log(),
                        Command::Cd { zoxide } => {
                            self.pre_console_path = self.center.panel().path().to_path_buf();
                            self.mode = if zoxide {
                                // TODO WIP: Test out zoxide console
                                Mode::Console {
                                    console: Box::new(Zoxide::from_panel(self.center.panel())),
                                }
                            } else {
                                Mode::Console {
                                    console: Box::new(DirConsole::from_panel(self.center.panel())),
                                }
                            };
                            self.console_dirty = true;
                        }
                        Command::Search => {
                            self.mode = Mode::Search {
                                input: Input::empty(),
                            };
                            self.compositor.mark_layer_dirty(self.footer_layer);
                        }
                        Command::Rename => {
                            let selected = self
                                .center
                                .panel()
                                .selected_path()
                                .and_then(|p| p.file_name())
                                .and_then(|f| f.to_owned().into_string().ok())
                                .unwrap_or_default();
                            self.mode = Mode::Rename {
                                input: Input::from_str(selected),
                            };
                            self.compositor.mark_layer_dirty(self.footer_layer);
                        }
                        Command::Next => {
                            self.center.panel_mut().select_next_marked();
                            self.right
                                .new_panel_delayed(self.center.panel().selected_path());
                            self.mark_center_dirty();
                            self.compositor.mark_layer_dirty(self.right_layer);
                        }
                        Command::Previous => {
                            self.center.panel_mut().select_prev_marked();
                            self.right
                                .new_panel_delayed(self.center.panel().selected_path());
                            self.mark_center_dirty();
                            self.compositor.mark_layer_dirty(self.right_layer);
                        }
                        Command::Mkdir => {
                            self.mode = Mode::CreateItem {
                                input: Input::empty(),
                                is_dir: true,
                            };
                            self.compositor.mark_layer_dirty(self.footer_layer);
                        }
                        Command::Touch => {
                            self.mode = Mode::CreateItem {
                                input: Input::empty(),
                                is_dir: false,
                            };
                            self.compositor.mark_layer_dirty(self.footer_layer);
                        }
                        Command::Mark => {
                            self.center.panel_mut().mark_selected_item();
                            self.move_cursor(Move::Down);
                        }
                        Command::Cut => {
                            let files = self.marked_or_selected();
                            info!("cut {} items", files.len());
                            self.clipboard = Some(Clipboard { files, cut: true });
                        }
                        Command::Copy => {
                            let files = self.marked_or_selected();
                            info!("copying {} items", files.len());
                            self.clipboard = Some(Clipboard { files, cut: false });
                            self.mark_center_dirty();
                        }
                        Command::Delete => {
                            let files = self.marked_or_selected();
                            info!("Deleted {} items", files.len());
                            self.unmark_all_items();
                            for file in files {
                                self.delete_file(&file);
                            }
                            self.left.reload();
                            self.center.reload();
                            self.right.reload();
                        }
                        Command::Paste { overwrite } => {
                            self.unmark_all_items();
                            let current_path = self.center.panel().path().to_path_buf();
                            let clipboard = self.clipboard.take();
                            tokio::task::spawn_blocking(move || {
                                if let Some(clipboard) = clipboard {
                                    info!(
                                        "paste {} items, overwrite = {}",
                                        clipboard.files.len(),
                                        overwrite
                                    );
                                    for file in clipboard.files.iter() {
                                        if clipboard.cut {
                                            if let Err(e) = move_item(file, &current_path) {
                                                error!("Failed to move {}: {e}", file.display());
                                            }
                                        } else if let Err(e) = copy_item(file, &current_path) {
                                            error!("Failed to copy {}: {e}", file.display());
                                        }
                                    }
                                }
                            });
                            self.left.reload();
                            self.center.reload();
                            self.right.reload();
                            self.mark_panels_dirty();
                        }
                        Command::Zip => {
                            let items = self.marked_or_selected();
                            if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            self.center.freeze();
                            if let Err(e) = self.opener.zip(items) {
                                warn!("Failed to create zip-archive: {e}");
                            }
                            self.center.unfreeze();
                            self.mark_center_dirty();
                        }
                        Command::Tar => {
                            let items = self.marked_or_selected();
                            if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            self.center.freeze();
                            if let Err(e) = self.opener.tar(items) {
                                warn!("Failed to create tar-archive: {e}");
                            }
                            self.center.unfreeze();
                            self.mark_center_dirty();
                        }
                        Command::Extract => {
                            self.center.freeze();
                            if let Some(archive) = self.center.panel().selected_path() {
                                if let Err(e) =
                                    std::env::set_current_dir(self.center.panel().path())
                                {
                                    error!("Failed to set working-directory for process: {e}");
                                }
                                if let Err(e) = self.opener.extract(archive.to_owned()) {
                                    warn!("Failed to extract archive: {e}");
                                }
                                self.mark_center_dirty();
                            } else {
                                warn!("Nothing extractable is selected");
                            }
                            self.center.unfreeze();
                        }
                        Command::Quit => {
                            return Ok(Some(CloseCmd::QuitWithPath {
                                path: self.center.panel().path().to_path_buf(),
                            }));
                        }
                        Command::QuitWithoutPath => {
                            return Ok(Some(CloseCmd::Quit));
                        }
                        Command::None => {}
                    }
                    // Always redraw footer
                    self.compositor.mark_layer_dirty(self.footer_layer);
                }
                Mode::Console { console } => {
                    match console.handle_key(key_event) {
                        ConsoleOp::Cd(path) => {
                            self.jump(path);
                        }
                        ConsoleOp::None => (),
                        ConsoleOp::Exit => {
                            self.mode = Mode::Normal;
                            self.mark_panels_dirty();
                        }
                    }
                    self.console_dirty = true;
                }
                Mode::CreateItem { input, is_dir } => {
                    match key_event.code {
                        KeyCode::Enter => {
                            let current_path = self.center.panel().path();
                            let create_fn = if *is_dir {
                                |item| fs_extra::dir::create(item, false)
                            } else {
                                |item| {
                                    let _ = OpenOptions::new()
                                        .read(true)
                                        .append(true)
                                        .create(true)
                                        .open(item)?;
                                    Ok(())
                                }
                            };
                            if let Err(e) = create_fn(current_path.join(input.get().trim())) {
                                error!("{e}");
                            }
                            self.mode = Mode::Normal;
                            self.center.panel_mut().clear_new_element();
                            self.mark_panels_dirty();
                        }
                        KeyCode::Tab => {
                            /* autocomplete here ? */
                            self.compositor.mark_layer_dirty(self.footer_layer);
                        }
                        key_code => {
                            input.update(key_code, key_event.modifiers);
                            self.center
                                .panel_mut()
                                .inject_new_element(input.get().to_string(), *is_dir);
                            self.mark_center_dirty();
                        }
                    }
                }
                Mode::Search { input } => {
                    if let KeyCode::Enter = key_event.code {
                        self.center.panel_mut().finish_search(input.get());
                        self.center.panel_mut().select_next_marked();
                        self.right
                            .new_panel_delayed(self.center.panel().selected_path());
                        self.mode = Mode::Normal;
                        self.mark_center_dirty();
                        self.compositor.mark_layer_dirty(self.right_layer);
                    } else {
                        input.update(key_event.code, key_event.modifiers);
                        self.center
                            .panel_mut()
                            .update_search(input.get().to_string());
                        self.mark_center_dirty();
                    }
                }
                Mode::Rename { input } => {
                    if let KeyCode::Enter = key_event.code {
                        if let Some(from) = self.center.panel().selected_path() {
                            let to = from
                                .parent()
                                .map(|p| p.join(input.get()))
                                .unwrap_or_default();
                            if let Err(e) = std::fs::rename(from, to) {
                                error!("{e}");
                            }
                        }
                        self.mode = Mode::Normal;
                        self.center.reload();
                        self.right.reload();
                        self.mark_panels_dirty();
                    } else {
                        input.update(key_event.code, key_event.modifiers);
                        self.mark_center_dirty();
                    }
                }
            }
        }
        if let Event::Resize(sx, sy) = event {
            self.layout = MillerColumns::from_size((sx, sy));
            self.mark_all_dirty();
        }
        Ok(None)
    }
}
