use std::fs::OpenOptions;

use crossterm::{
    event::{Event, EventStream, KeyCode},
    style::PrintStyledContent,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
    ExecutableCommand,
};
use futures::{FutureExt, StreamExt};
use log::{debug, error, info, trace, Level};
use tempfile::TempDir;
use tokio::sync::watch;

use crate::{
    command_queue::{zoxide_add_dir, QueueStatus, QueuedCommand},
    config::color::{color_dir_path, color_main},
    debug::{ClipboardInfo, DebugRequest, EntryInfo, PaneId, StateSnapshot},
    engine::commands::{CloseCmd, Command, CommandParser},
    engine::OpenEngine,
    logger::LogBuffer,
    util::{copy_item, get_destination, move_item, print_metadata, rename_safe},
};

/// Expand the command template by replacing $@ with the given paths
fn expand_command(cmd: &str, paths: &[PathBuf], separator: &str) -> String {
    let paths_str: String = paths
        .iter()
        .map(|p| shell_escape::escape(p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(separator);

    cmd.replace("$@", &paths_str)
}

/// Receive a debug request, or stay pending forever when debug mode is off.
async fn recv_debug(rx: &mut Option<mpsc::Receiver<DebugRequest>>) -> Option<DebugRequest> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

use self::console::{Console, ConsoleOp, DirConsole, Zoxide};

use super::{input::Input, *};

struct Redraw {
    left: bool,
    center: bool,
    right: bool,
    console: bool,
    log: bool,
    header: bool,
    footer: bool,
}

impl Redraw {
    fn any(&self) -> bool {
        self.left
            || self.center
            || self.right
            || self.console
            || self.header
            || self.footer
            || self.log
    }
}

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
    /// Weather or not we want to cut or copy the items.
    ///
    /// `True`  : Cut
    /// `False` : Copy
    cut: bool,
}

// enum Operation {
//     MoveItems { from: Vec<PathBuf>, to: PathBuf },
//     CopyItems { from: Vec<PathBuf>, to: PathBuf },
//     Mkdir { path: PathBuf },
//     Move(Movement),
// }

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

    // /// Undo/Redo stack
    // stack: Vec<Operation>,
    /// Miller-Columns layout
    layout: MillerColumns,

    /// Show hidden files
    show_hidden: bool,

    /// Show log
    show_log: bool,

    /// Elements that needs to be redrawn
    redraw: Redraw,

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

    /// Sender for queueing background commands
    command_tx: mpsc::UnboundedSender<QueuedCommand>,

    /// Receiver for status of the background command queue
    command_status_rx: watch::Receiver<QueueStatus>,

    /// Receiver for debug-socket requests (`--debug-socket`)
    debug_rx: Option<mpsc::Receiver<DebugRequest>>,

    /// Event-loop iteration counter, exposed via the debug socket
    debug_seq: u64,
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
        command_tx: mpsc::UnboundedSender<QueuedCommand>,
        command_status_rx: watch::Receiver<QueueStatus>,
        debug_rx: Option<mpsc::Receiver<DebugRequest>>,
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

        Ok(PanelManager {
            left,
            center,
            right,
            mode: Mode::Normal,
            logger,
            clipboard: None,
            layout,
            opener,
            // stack: Vec::new(),
            show_hidden: false,
            show_log: false,
            redraw: Redraw {
                left: true,
                center: true,
                right: true,
                log: true,
                console: true,
                header: true,
                footer: true,
            },
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
            command_tx,
            command_status_rx,
            debug_rx,
            debug_seq: 0,
        })
    }

    // fn redraw_header(&mut self) {
    //     self.redraw.header = true;
    // }

    fn redraw_footer(&mut self) {
        self.redraw.footer = true;
    }

    fn redraw_panels(&mut self) {
        self.redraw.left = true;
        self.redraw.center = true;
        self.redraw.right = true;
        self.redraw.header = true;
        self.redraw.footer = true;
        self.redraw.log = true;
    }

    fn redraw_left(&mut self) {
        self.redraw.left = true;
        self.redraw.log = true;
    }

    fn redraw_center(&mut self) {
        self.redraw.center = true;
        // if something changed in the center,
        // also redraw header and footer
        self.redraw.footer = true;
        self.redraw.header = true;
        self.redraw.log = true;
    }

    fn redraw_right(&mut self) {
        self.redraw.right = true;
        self.redraw.log = true;
    }

    fn redraw_console(&mut self) {
        self.redraw.console = true;
    }

    fn redraw_everything(&mut self) {
        self.redraw.header = true;
        self.redraw.footer = true;
        self.redraw.left = true;
        self.redraw.center = true;
        self.redraw.right = true;
        self.redraw.console = true;
    }

    fn redraw_log(&mut self) {
        self.redraw.log = true;
    }

    fn draw_log(&mut self) -> Result<()> {
        if !self.redraw.log {
            return Ok(());
        }

        let mut y = self.layout.footer().saturating_sub(2); // or 3, if we have the advanced command preview

        let print_level = |level| match level {
            log::Level::Error => PrintStyledContent("error".red().bold()),
            log::Level::Warn => PrintStyledContent("warn".yellow().bold()),
            log::Level::Info => PrintStyledContent("info".with(color_main()).bold()),
            log::Level::Debug => PrintStyledContent("debug".dark_blue()),
            log::Level::Trace => PrintStyledContent("trace".grey()),
        };

        if self.show_log {
            for (level, line) in self.logger.get().into_iter().rev() {
                queue!(
                    self.stdout,
                    cursor::MoveTo(0, y),
                    Clear(ClearType::CurrentLine),
                    print_level(level),
                    style::Print(": "),
                    style::PrintStyledContent(line.grey()),
                    style::Print("  "),
                )?;
                y = y.saturating_sub(1);
            }
        } else if let Some((level, line)) = self
            .logger
            .get()
            .into_iter()
            .rev()
            .find(|(level, _)| *level <= Level::Warn)
        {
            queue!(
                self.stdout,
                cursor::MoveTo(0, y),
                Clear(ClearType::CurrentLine),
                print_level(level),
                style::Print(": "),
                style::PrintStyledContent(line.grey()),
                style::Print("  "),
            )?;
        }
        self.redraw.log = false;
        Ok(())
    }

    // Prints our header
    fn draw_header(&mut self) -> Result<()> {
        if !self.redraw.header {
            return Ok(());
        }
        let prompt = format!(
            "{}@{}",
            whoami::username(),
            whoami::fallible::hostname().unwrap_or_else(|e| e.to_string())
        );
        let absolute = self
            .center
            .panel()
            .selected_path()
            .and_then(|f| f.canonicalize().ok())
            .unwrap_or_else(|| self.center.panel().path().to_path_buf());
        let file_name = absolute
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default();
        let absolute = absolute.to_str().unwrap_or_default();

        let (prefix, suffix) = absolute.split_at(absolute.len() - file_name.len());

        queue!(
            self.stdout,
            cursor::MoveTo(0, 0),
            Clear(ClearType::CurrentLine),
            style::PrintStyledContent(prompt.with(color_main()).bold()),
            style::Print(" "),
            style::PrintStyledContent(prefix.to_string().with(color_dir_path()).bold()),
            style::PrintStyledContent(suffix.to_string().bold()),
        )?;
        self.redraw.header = false;
        Ok(())
    }

    // Prints a footer
    fn draw_footer(&mut self) -> Result<()> {
        if !self.redraw.footer {
            return Ok(());
        }
        // Common operation at the start
        queue!(
            self.stdout,
            cursor::MoveTo(0, self.layout.footer()),
            Clear(ClearType::CurrentLine),
        )?;

        if let Mode::Search { input } = &self.mode {
            self.stdout
                .queue(PrintStyledContent(
                    "Search".bold().with(color_main()).reverse(),
                ))?
                .queue(Print(" "))?;
            input.print(&mut self.stdout, style::Color::Red)?;
            return self.stdout.flush();
        }
        if let Mode::Rename { input } = &self.mode {
            self.stdout
                .queue(PrintStyledContent(
                    "Rename:".bold().with(color_main()).reverse(),
                ))?
                .queue(Print(" "))?;
            input.print(&mut self.stdout, style::Color::Yellow)?;
            return self.stdout.flush();
        }
        if let Mode::CreateItem { input, is_dir } = &self.mode {
            let prompt = if *is_dir { "Make Directory:" } else { "Touch:" };
            self.stdout
                .queue(PrintStyledContent(
                    prompt.bold().with(color_main()).reverse(),
                ))?
                .queue(Print(" "))?;
            if *is_dir {
                input.print(&mut self.stdout, color_main())?;
            } else {
                input.print(&mut self.stdout, style::Color::Grey)?;
            }
            return self.stdout.flush();
        }
        let (permissions, metadata) = print_metadata(self.center.panel().selected_path());
        queue!(
            self.stdout,
            style::PrintStyledContent(permissions.dark_cyan()),
            Print("   "),
            Print(metadata)
        )?;

        // TODO: We could place this into its own line, and also print some recommendations
        let key_buffer = self.parser.buffer();
        let (n, m) = self.center.panel().index_vs_total();
        let n_files_string = format!("{n}/{m} ");

        // Okay, we CAN print the matching commands, but currently I am not very happy with this.
        if false {
            queue!(
                self.stdout,
                cursor::MoveTo(
                    // (self.layout.width() / 2).saturating_sub(key_buffer.len() as u16 / 2),
                    0,
                    self.layout.footer().saturating_sub(2),
                ),
                Clear(ClearType::CurrentLine),
                style::PrintStyledContent(key_buffer.clone().on_dark_grey()),
                Print("    "),
            )?;
            let key_buffer_len = key_buffer.chars().count();
            for (cmd, desc) in self.parser.matching_commands() {
                let sub_cmd: String = cmd.chars().skip(key_buffer_len).collect();
                queue!(
                    self.stdout,
                    style::PrintStyledContent(key_buffer.clone().on_dark_grey()),
                    style::PrintStyledContent(sub_cmd.dark_grey()),
                    Print(": "),
                    style::PrintStyledContent(desc.dark_grey()),
                    Print("   "),
                )?;
            }
        } else {
            queue!(
                self.stdout,
                cursor::MoveTo(
                    (self.layout.width() / 2).saturating_sub(key_buffer.len() as u16 / 2),
                    self.layout.footer()
                ),
                style::PrintStyledContent(key_buffer.dark_grey()),
            )?;
        }
        // ---
        queue!(
            self.stdout,
            cursor::MoveTo(
                self.layout
                    .width()
                    .saturating_sub(n_files_string.len() as u16),
                self.layout.footer(),
            ),
            style::Print(n_files_string),
        )?;
        self.redraw.footer = false;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        if !self.redraw.any() {
            return Ok(());
        }
        self.stdout.execute(BeginSynchronizedUpdate)?;
        self.stdout.queue(cursor::Hide)?;
        self.draw_footer()?;
        self.draw_header()?;
        self.draw_panels()?;
        self.draw_console()?;
        self.draw_log()?;
        self.stdout.execute(EndSynchronizedUpdate)?;
        Ok(())
    }

    fn draw_panels(&mut self) -> Result<()> {
        let (start, end) = (self.layout.y_range.start, self.layout.y_range.end);
        let height = if self.show_log {
            let cap = self.logger.capacity();
            start..end.saturating_sub(cap as u16)
        } else {
            start..end
        };
        if self.redraw.left {
            self.left.panel_mut().draw(
                &mut self.stdout,
                self.layout.left_x_range.clone(),
                height.clone(),
            )?;
            self.redraw.left = false;
        }
        if self.redraw.center {
            self.center.panel_mut().draw(
                &mut self.stdout,
                self.layout.center_x_range.clone(),
                height.clone(),
            )?;
            self.redraw.center = false;
        }
        if self.redraw.right {
            self.right.panel_mut().draw(
                &mut self.stdout,
                self.layout.right_x_range.clone(),
                height,
            )?;
            self.redraw.right = false;
        }
        Ok(())
    }

    fn draw_console(&mut self) -> Result<()> {
        if self.redraw.console {
            if let Mode::Console { console } = &mut self.mode {
                console.draw(
                    &mut self.stdout,
                    self.layout.left_x_range.start..self.layout.right_x_range.end,
                    self.layout.y_range.clone(),
                )?;
            }
            self.redraw.console = false;
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
        self.redraw_everything();
    }

    fn toggle_log(&mut self) {
        self.show_log = !self.show_log;
        if self.show_log {
            self.redraw_log();
        } else {
            // Redraw everything, so that the current log gets overdrawn by the panels
            self.redraw_everything();
        }
    }

    // fn select(&mut self, path: &Path) {
    //     if self.center.panel().selected_path() == Some(path) {
    //         return;
    //     }
    //     self.center.panel_mut().select_path(path);
    //     self.right
    //         .new_panel_delayed(self.center.panel().selected_path());
    //     self.redraw_center();
    //     self.redraw_right();
    // }

    fn move_up(&mut self, step: usize) {
        trace!("move-up");
        if self.center.panel_mut().up(step) {
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.redraw_center();
            self.redraw_right();
            self.rev_history.clear();
            // self.stack.push(Operation::Move(Movement::Up));
        }
    }

    fn move_down(&mut self, step: usize) {
        trace!("move-down");
        if self.center.panel_mut().down(step) {
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.redraw_center();
            self.redraw_right();
            self.rev_history.clear();
            // self.stack.push(Operation::Move(Movement::Down));
        }
    }

    fn move_right(&mut self) {
        trace!("move-right");
        if let Some(selected) = self.center.panel().selected_path().map(|p| p.to_path_buf()) {
            // If the selected item is a directory, all panels will shift to the left
            if selected.is_dir() {
                if let Some(cmd) = zoxide_add_dir(&selected) {
                    if let Err(e) = self.command_tx.send(cmd) {
                        error!("Failed to queue command: {}", e);
                    }
                }
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

                self.redraw_panels();
            } else {
                info!("Opening '{}'", selected.display());

                // Change working directory so that child processes gets spawned from the currently active directory.
                if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                    error!("Failed to set working-directory for process: {e}");
                }
                if let Err(e) = self.opener.open(selected) {
                    /* failed to open selected */
                    error!("Opening failed: {e}");
                }
                self.redraw_everything();
            }
            // self.stack.push(Operation::Move(Movement::Right));
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
        self.redraw_panels();
        // self.stack.push(Operation::Move(Movement::Left));
    }

    fn jump(&mut self, path: PathBuf) {
        trace!("jump-to {}", path.display());
        // Don't do anything, if the path hasn't changed
        if path.as_path() == self.center.panel().path() {
            return;
        }
        if path.exists() {
            if let Some(cmd) = zoxide_add_dir(&path) {
                if let Err(e) = self.command_tx.send(cmd) {
                    error!("Failed to queue command: {}", e);
                }
            }
            self.fwd_history.clear(); // Delete history when jumping
            self.rev_history.clear();
            self.previous = self.center.panel().path().to_path_buf();
            self.left.new_panel_instant(path.parent());
            self.left.panel_mut().select_path(&path, None);
            self.center.new_panel_instant(Some(&path));
            self.right
                .new_panel_delayed(self.center.panel().selected_path());
            self.redraw_panels();
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
        self.redraw_panels();
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

    /// Performs bulk rename on multiple files using the configured text editor.
    fn bulkrename(&mut self, files: Vec<PathBuf>) {
        use std::io::Write;

        if files.is_empty() {
            return;
        }

        // Get the directory where files are located (assume all in same directory)
        let dir = files[0].parent().unwrap_or(Path::new(".")).to_path_buf();

        // Create temp file with filenames
        let temp_file = match tempfile::Builder::new()
            .prefix("rfm-bulkrename-")
            .suffix(".txt")
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for bulkrename: {e}");
                return;
            }
        };
        let temp_path = temp_file.path().to_path_buf();

        // Write original filenames to temp file
        let original_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name())
            .filter_map(|n| n.to_str())
            .map(|s| s.to_string())
            .collect();

        if let Err(e) = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temp_path)?;
            for name in &original_names {
                writeln!(file, "{}", name)?;
            }
            file.flush()?;
            Ok(())
        })() {
            error!("Failed to write temp file for bulkrename: {e}");
            return;
        }

        // Keep temp file alive by keeping the handle
        let _temp_handle = temp_file;

        // Open editor and wait for it to close
        loop {
            match self.opener.open_text_blocking(temp_path.clone()) {
                Ok(status) if !status.success() => {
                    info!("Editor exited with non-zero status, aborting bulkrename");
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to open editor: {e}");
                    return;
                }
            }

            // Read edited filenames
            let new_content = match std::fs::read_to_string(&temp_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to read temp file after editing: {e}");
                    return;
                }
            };

            let new_names: Vec<&str> = new_content.lines().collect();

            // Check if unchanged - if so, exit silently
            if new_names.len() == original_names.len()
                && new_names
                    .iter()
                    .zip(original_names.iter())
                    .all(|(new, old)| *new == old)
            {
                info!("Bulkrename: no changes made");
                return;
            }

            // Validate: line count must match
            if new_names.len() != original_names.len() {
                error!(
                    "Bulkrename: line count mismatch ({} vs {}). Lines must not be added or removed.",
                    new_names.len(),
                    original_names.len()
                );
                // Rewrite original file and reopen
                if let Err(e) = (|| -> std::io::Result<()> {
                    let mut file = std::fs::File::create(&temp_path)?;
                    for name in &original_names {
                        writeln!(file, "{} # DO NOT add or remove lines", name)?;
                    }
                    file.flush()?;
                    Ok(())
                })() {
                    error!("Failed to rewrite temp file: {e}");
                    return;
                }
                continue;
            }

            // Check for errors: duplicates and conflicts
            let mut errors: Vec<Option<&str>> = vec![None; new_names.len()];
            let mut has_errors = false;

            // Check for duplicate target names within batch
            for i in 0..new_names.len() {
                for j in (i + 1)..new_names.len() {
                    if new_names[i] == new_names[j] && new_names[i] != original_names[i] {
                        errors[i] = Some("duplicate target name");
                        errors[j] = Some("duplicate target name");
                        has_errors = true;
                    }
                }
            }

            // Check for conflicts with existing files (but not with files being renamed in this batch)
            for (i, new_name) in new_names.iter().enumerate() {
                if *new_name == original_names[i] {
                    continue; // No change for this file
                }
                let target_path = dir.join(new_name);
                // Check if target exists and is not one of the files being renamed
                if target_path.exists() && !files.contains(&target_path) {
                    errors[i] = Some("already exists");
                    has_errors = true;
                }
            }

            if has_errors {
                // Rewrite file with error comments and reopen
                if let Err(e) = (|| -> std::io::Result<()> {
                    let mut file = std::fs::File::create(&temp_path)?;
                    for (i, new_name) in new_names.iter().enumerate() {
                        if let Some(err_msg) = errors[i] {
                            writeln!(file, "{} # {}", new_name, err_msg)?;
                        } else {
                            writeln!(file, "{}", new_name)?;
                        }
                    }
                    file.flush()?;
                    Ok(())
                })() {
                    error!("Failed to rewrite temp file with errors: {e}");
                    return;
                }
                continue;
            }

            // All validations passed - perform renames
            // We need to handle the case where files might be swapped (a->b, b->a)
            // So we first rename to temporary names, then to final names
            let mut temp_renames: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut final_renames: Vec<(PathBuf, PathBuf, String)> = Vec::new();

            // Use process ID and timestamp to create unique temp names
            let unique_id = format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );

            for (i, new_name) in new_names.iter().enumerate() {
                if *new_name == original_names[i] {
                    continue; // No change
                }
                let from = files[i].clone();
                let to = dir.join(new_name);

                // Check if target is also being renamed (swap scenario)
                let is_swap = files.iter().any(|f| *f == to);
                if is_swap {
                    // Rename to temp first (unique name to avoid conflicts)
                    let temp_name =
                        format!(".rfm-bulkrename-{}-{}-{}", unique_id, i, original_names[i]);
                    let temp_path = dir.join(&temp_name);
                    temp_renames.push((from.clone(), temp_path.clone()));
                    final_renames.push((temp_path, to, new_name.to_string()));
                } else {
                    final_renames.push((from, to, new_name.to_string()));
                }
            }

            // Execute temp renames first
            for (from, to) in &temp_renames {
                if let Err(e) = std::fs::rename(from, to) {
                    error!(
                        "Failed to rename {} -> {}: {e}",
                        from.display(),
                        to.display()
                    );
                    return;
                }
            }

            // Execute final renames with safety fallback
            let mut success_count = 0;
            for (from, to, new_name) in &final_renames {
                match rename_safe(from, to) {
                    Ok(actual_path) => {
                        if actual_path != *to {
                            info!(
                                "Renamed '{}' -> '{}' (adjusted due to conflict)",
                                from.display(),
                                actual_path.display()
                            );
                        }
                        success_count += 1;
                    }
                    Err(e) => {
                        error!("Failed to rename to '{}': {e}", new_name);
                    }
                }
            }

            info!("Bulkrename: successfully renamed {} files", success_count);
            return;
        }
    }

    fn handle_debug_request(&mut self, req: DebugRequest) {
        // Replies may fail if the client timed out; that is fine.
        match req {
            DebugRequest::State { reply } => {
                let _ = reply.send(self.state_snapshot());
            }
            DebugRequest::AwaitIdle { reply } => {
                // Answered here means the biased select found nothing else
                // ready — the event queue is drained.
                let _ = reply.send(self.debug_seq);
            }
            DebugRequest::Entries { pane, reply } => {
                let panel = match pane {
                    PaneId::Left => self.left.panel(),
                    PaneId::Center => self.center.panel(),
                };
                let selected_idx = panel.selected_idx();
                let entries = panel
                    .elements()
                    .enumerate()
                    .map(|(idx, elem)| EntryInfo {
                        name: elem.name().clone(),
                        marked: elem.is_marked(),
                        hidden: elem.is_hidden(),
                        selected: idx == selected_idx,
                    })
                    .collect();
                let _ = reply.send(entries);
            }
        }
    }

    fn state_snapshot(&self) -> StateSnapshot {
        let center = self.center.panel();
        // `index_vs_total()` returns a 1-based position; the snapshot
        // exposes a 0-based index into the visible entries.
        let (position, total) = center.index_vs_total();
        let queue = self.command_status_rx.borrow().clone();
        StateSnapshot {
            seq: self.debug_seq,
            mode: match &self.mode {
                Mode::Normal => "normal",
                Mode::Console { .. } => "console",
                Mode::CreateItem { is_dir: true, .. } => "mkdir",
                Mode::CreateItem { .. } => "touch",
                Mode::Search { .. } => "search",
                Mode::Rename { .. } => "rename",
            }
            .to_string(),
            cwd: center.path().to_path_buf(),
            selection: center
                .selected_path()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned()),
            selected_idx: position.saturating_sub(1),
            total,
            marked: center
                .elements()
                .filter(|e| e.is_marked())
                .map(|e| e.path().to_path_buf())
                .collect(),
            clipboard: self.clipboard.as_ref().map(|c| ClipboardInfo {
                files: c.files.clone(),
                op: if c.cut { "cut" } else { "copy" }.to_string(),
            }),
            show_hidden: self.show_hidden,
            left_path: self.left.panel().path().to_path_buf(),
            preview_path: self.right.panel().path().to_path_buf(),
            queue_active: queue.active,
            queue_len: queue.queued_count,
        }
    }

    pub async fn run(mut self) -> Result<CloseCmd> {
        // Initial draw
        self.redraw_everything();
        self.draw()?;

        let close_cmd = loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                biased;
                // Check incoming new logs
                () = self.logger.update() => {
                    self.redraw_log();
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
                        self.redraw_center();
                        self.redraw_right();
                        self.redraw_console();
                    } else if self.left.check_update(&state) {
                        self.left.update_panel(panel);
                        self.left.panel_mut().select_path(self.center.panel().path(), Some(self.center.panel().selected_idx()));
                        self.redraw_left();
                        self.redraw_console();
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
                        self.redraw_right();
                        self.redraw_console();
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
                // Debug socket requests; must stay the LAST branch of this
                // biased select: it may only win when nothing else is ready,
                // which is exactly the idle guarantee `await-idle` promises.
                Some(req) = recv_debug(&mut self.debug_rx) => {
                    self.handle_debug_request(req);
                    // Debug queries are not app events: skip draw() and the
                    // seq bump so polling `state` doesn't advance seq itself.
                    continue;
                }
            }
            // Always redraw what needs to be redrawn
            self.draw()?;
            self.debug_seq = self.debug_seq.wrapping_add(1);
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
                self.center.panel_mut().clear_rename_preview();
                self.redraw_panels();
                self.redraw_footer();
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
                            self.redraw_console();
                        }
                        Command::Search => {
                            self.mode = Mode::Search {
                                input: Input::empty(),
                            };
                            self.redraw_footer();
                        }
                        Command::Rename => {
                            // Check if multiple files are marked - use bulkrename
                            let marked = self.marked_items();
                            if marked.len() > 1 {
                                let files: Vec<PathBuf> =
                                    marked.iter().map(|e| e.path().to_path_buf()).collect();
                                self.unmark_all_items();
                                self.bulkrename(files);
                                self.left.reload();
                                self.center.reload();
                                self.right.reload();
                                self.redraw_everything();
                            } else {
                                // Single file rename - use existing inline mode
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
                                self.redraw_footer();
                            }
                        }
                        Command::Next => {
                            self.center.panel_mut().select_next_marked();
                            self.right
                                .new_panel_delayed(self.center.panel().selected_path());
                            self.redraw_center();
                            self.redraw_right();
                        }
                        Command::Previous => {
                            self.center.panel_mut().select_prev_marked();
                            self.right
                                .new_panel_delayed(self.center.panel().selected_path());
                            self.redraw_center();
                            self.redraw_right();
                        }
                        Command::Mkdir => {
                            self.mode = Mode::CreateItem {
                                input: Input::empty(),
                                is_dir: true,
                            };
                            self.redraw_footer();
                        }
                        Command::Touch => {
                            self.mode = Mode::CreateItem {
                                input: Input::empty(),
                                is_dir: false,
                            };
                            self.redraw_footer();
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
                            self.redraw_center();
                        }
                        Command::Delete => {
                            let files = self.marked_or_selected();
                            info!("Deleted {} items", files.len());
                            self.unmark_all_items();
                            // self.stack.push(Operation::MoveItems { from: files.clone(), to: trash_dir.path().to_path_buf() });
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
                            self.redraw_panels();
                        }
                        Command::Zip => {
                            let items = self.marked_or_selected();
                            if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            if let Err(e) = self.opener.zip(items) {
                                warn!("Failed to create zip-archive: {e}");
                            }
                            self.redraw_center();
                        }
                        Command::Tar => {
                            let items = self.marked_or_selected();
                            if let Err(e) = std::env::set_current_dir(self.center.panel().path()) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            if let Err(e) = self.opener.tar(items) {
                                warn!("Failed to create tar-archive: {e}");
                            }
                            self.redraw_center();
                        }
                        Command::Extract => {
                            if let Some(archive) = self.center.panel().selected_path() {
                                if let Err(e) =
                                    std::env::set_current_dir(self.center.panel().path())
                                {
                                    error!("Failed to set working-directory for process: {e}");
                                }
                                if let Err(e) = self.opener.extract(archive.to_owned()) {
                                    warn!("Failed to extract archive: {e}");
                                }
                                self.redraw_center();
                            } else {
                                warn!("Nothing extractable is selected");
                            }
                        }
                        Command::Quit => {
                            return Ok(Some(CloseCmd::QuitWithPath {
                                path: self.center.panel().path().to_path_buf(),
                            }));
                        }
                        Command::QuitWithoutPath => {
                            return Ok(Some(CloseCmd::Quit));
                        }
                        Command::UserCommand {
                            name,
                            cmd,
                            interactive,
                            separator,
                        } => {
                            let paths = self.marked_or_selected();
                            let expanded_cmd = expand_command(&cmd, &paths, &separator);
                            let working_dir = self.center.panel().path().to_path_buf();

                            if interactive {
                                // Run interactively in foreground
                                info!("Running interactive command '{}': {}", name, expanded_cmd);
                                if let Err(e) = std::env::set_current_dir(&working_dir) {
                                    error!("Failed to set working directory: {e}");
                                }
                                // TODO: Implement terminal suspend/resume for interactive commands
                                // For now, just run it blocking
                                let result = std::process::Command::new("sh")
                                    .arg("-c")
                                    .arg(&expanded_cmd)
                                    .current_dir(&working_dir)
                                    .status();
                                match result {
                                    Ok(status) => {
                                        if status.success() {
                                            info!("Command '{}' completed successfully", name);
                                        } else {
                                            let code = status.code().unwrap_or(-1);
                                            error!(
                                                "Command '{}' failed with exit code {}",
                                                name, code
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to run command '{}': {}", name, e);
                                    }
                                }
                                self.redraw_everything();
                            } else {
                                // Queue for background execution
                                info!("Queueing command '{}': {}", name, expanded_cmd);
                                let queued = QueuedCommand {
                                    name,
                                    cmd: expanded_cmd,
                                    working_dir,
                                };
                                if let Err(e) = self.command_tx.send(queued) {
                                    error!("Failed to queue command: {}", e);
                                }
                            }
                            self.unmark_all_items();
                        }
                        Command::None => {}
                    }
                    // Always redraw footer
                    self.redraw_footer();
                }
                Mode::Console { console } => {
                    match console.handle_key(key_event) {
                        ConsoleOp::Cd(path) => {
                            self.jump(path);
                        }
                        ConsoleOp::None => (),
                        ConsoleOp::Exit => {
                            self.mode = Mode::Normal;
                            self.redraw_panels();
                        }
                    }
                    self.redraw_console();
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
                            // self.stack.push(Operation::Mkdir { path: new_dir.clone() });
                            self.mode = Mode::Normal;
                            self.center.panel_mut().clear_new_element();
                            self.redraw_panels();
                        }
                        KeyCode::Tab => {
                            /* autocomplete here ? */
                            self.redraw_footer();
                        }
                        key_code => {
                            input.update(key_code, key_event.modifiers);
                            self.center
                                .panel_mut()
                                .inject_new_element(input.get().to_string(), *is_dir);
                            self.redraw_center();
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
                        self.redraw_center();
                        self.redraw_right();
                    } else {
                        input.update(key_event.code, key_event.modifiers);
                        self.center
                            .panel_mut()
                            .update_search(input.get().to_string());
                        self.redraw_center();
                    }
                }
                Mode::Rename { input } => {
                    if let KeyCode::Enter = key_event.code {
                        if let Some(from) = self.center.panel().selected_path() {
                            let to = from
                                .parent()
                                .map(|p| p.join(input.get()))
                                .unwrap_or_default();
                            // Don't rename if it's the same path
                            if from == to {
                                // No-op, just exit rename mode
                            } else if to.exists() {
                                // Prevent overwriting existing files
                                warn!("Cannot rename: '{}' already exists", to.display());
                            } else if let Err(e) = std::fs::rename(from, &to) {
                                error!("{e}");
                            }
                        }
                        self.mode = Mode::Normal;
                        self.center.panel_mut().clear_rename_preview();
                        self.center.reload();
                        self.right.reload();
                        self.redraw_panels();
                    } else {
                        input.update(key_event.code, key_event.modifiers);
                        let selected_idx = self.center.panel().selected_idx();
                        self.center
                            .panel_mut()
                            .inject_rename_preview(input.get().to_string(), selected_idx);
                        self.redraw_center();
                    }
                }
            }
        }
        if let Event::Resize(sx, sy) = event {
            self.layout = MillerColumns::from_size((sx, sy));
            self.redraw_everything();
        }
        Ok(None)
    }
}
