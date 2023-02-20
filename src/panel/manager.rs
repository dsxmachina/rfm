use std::os::unix::prelude::MetadataExt;

use crossterm::event::{Event, EventStream, KeyCode};
use fs_extra::dir::CopyOptions;
use futures::{FutureExt, StreamExt};
use notify_rust::Notification;
use time::OffsetDateTime;
use users::{get_group_by_gid, get_user_by_uid};

use crate::commands::{Command, CommandParser};

use super::{console::DirConsole, *};

struct Redraw {
    left: bool,
    center: bool,
    right: bool,
    console: bool,
    header: bool,
    footer: bool,
}

impl Redraw {
    fn any(&self) -> bool {
        self.left || self.center || self.right || self.console || self.header || self.footer
    }
}

enum Mode {
    Normal,
    Console { console: DirConsole },
    Mkdir { console: DirConsole },
    Search { input: String },
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

enum Operation {
    MoveItems { from: Vec<PathBuf>, to: PathBuf },
    CopyItems { from: Vec<PathBuf>, to: PathBuf },
    Mkdir { path: PathBuf },
    Move(Movement),
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

    /// Clipboard
    clipboard: Option<Clipboard>,

    /// Undo/Redo stack
    stack: Vec<Operation>,

    /// Miller-Columns layout
    layout: MillerColumns,

    /// Indicates what we want to show or hide
    show_hidden: bool,

    /// Elements that needs to be redrawn
    redraw: Redraw,

    /// Event-stream from the terminal
    event_reader: EventStream,

    // TODO: Implement "history"
    /// Previous path
    previous: PathBuf,

    /// command-parser
    parser: CommandParser,

    /// Handle to the standard-output
    stdout: Stdout,

    /// Receiver for incoming dir-panels
    dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,

    /// Receiver for incoming preview-panels
    prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,
}

impl PanelManager {
    pub fn new(
        directory_cache: SharedCache<DirPanel>,
        preview_cache: SharedCache<PreviewPanel>,
        dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,
        prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,
        directory_tx: mpsc::UnboundedSender<PanelUpdate>,
        preview_tx: mpsc::UnboundedSender<PanelUpdate>,
    ) -> Self {
        let stdout = stdout();
        let event_reader = EventStream::new();
        let parser = CommandParser::new();
        let terminal_size = terminal::size().unwrap_or_default();
        let layout = MillerColumns::from_size(terminal_size);

        let mut left = ManagedPanel::new(directory_cache.clone(), directory_tx.clone());
        let mut center = ManagedPanel::new(directory_cache, directory_tx);
        let right = ManagedPanel::new(preview_cache, preview_tx);

        left.new_panel(Some(".."));
        center.new_panel(Some("."));

        PanelManager {
            left,
            center,
            right,
            mode: Mode::Normal,
            clipboard: None,
            layout,
            stack: Vec::new(),
            show_hidden: false,
            redraw: Redraw {
                left: true,
                center: true,
                right: true,
                console: true,
                header: true,
                footer: true,
            },
            event_reader,
            previous: ".".into(),
            parser,
            stdout,
            dir_rx,
            prev_rx,
        }
    }

    fn redraw_header(&mut self) {
        self.redraw.header = true;
    }

    fn redraw_footer(&mut self) {
        self.redraw.footer = true;
    }

    fn redraw_panels(&mut self) {
        self.redraw.left = true;
        self.redraw.center = true;
        self.redraw.right = true;
        self.redraw.header = true;
        self.redraw.footer = true;
    }

    fn redraw_left(&mut self) {
        self.redraw.left = true;
    }

    fn redraw_center(&mut self) {
        self.redraw.center = true;
        // if something changed in the center,
        // also redraw header and footer
        self.redraw.footer = true;
        self.redraw.header = true;
    }

    fn redraw_right(&mut self) {
        self.redraw.right = true;
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

    // Prints our header
    fn draw_header(&mut self) -> Result<()> {
        if !self.redraw.header {
            return Ok(());
        }
        let prompt = format!("{}@{}", whoami::username(), whoami::hostname());
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
            style::PrintStyledContent(prompt.dark_green().bold()),
            style::Print(" "),
            style::PrintStyledContent(prefix.to_string().dark_blue().bold()),
            style::PrintStyledContent(suffix.to_string().white().bold()),
        )?;
        self.redraw.header = false;
        Ok(())
    }

    // Prints a footer
    fn draw_footer(&mut self) -> Result<()> {
        if !self.redraw.footer {
            return Ok(());
        }
        if let Some(selection) = self.center.panel().selected() {
            let path = selection.path();
            let permissions;
            let other;
            if let Ok(metadata) = path.metadata() {
                permissions = unix_mode::to_string(metadata.permissions().mode());
                let modified = metadata
                    .modified()
                    .map(|t| OffsetDateTime::from(t))
                    .map(|t| {
                        format!(
                            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
                            t.year(),
                            u8::from(t.month()),
                            t.day(),
                            t.hour(),
                            t.minute(),
                            t.second()
                        )
                    })
                    .unwrap_or_else(|_| String::from("cannot read timestamp"));
                let user = get_user_by_uid(metadata.uid())
                    .and_then(|u| u.name().to_str().map(String::from))
                    .unwrap_or_default();
                let group = get_group_by_gid(metadata.gid())
                    .and_then(|g| g.name().to_str().map(String::from))
                    .unwrap_or_default();
                let size = metadata.size();
                let size_str = match size {
                    0..=1023 => format!("{}B", size),
                    1024..=1048575 => format!("{:.1}K", (size as f64) / 1024.),
                    1048576..=1073741823 => format!("{:.1}M", (size as f64) / 1048576.),
                    1073741824..=1099511627775 => format!("{:.2}G", (size as f64) / 1073741824.),
                    1099511627776..=1125899906842623 => {
                        format!("{:.3}T", (size as f64) / 1099511627776.)
                    }
                    1125899906842624..=1152921504606846976 => {
                        format!("{:4}P", (size as f64) / 1125899906842624.)
                    }
                    _ => format!("too big"),
                };
                other = format!("{user} {group} {size_str} {modified}");
            } else {
                permissions = String::from("------------");
                other = String::from("");
            }

            queue!(
                self.stdout,
                cursor::MoveTo(0, self.layout.footer()),
                Clear(ClearType::CurrentLine),
                style::PrintStyledContent(permissions.dark_cyan()),
                Print("   "),
                Print(other)
            )?;
        }

        let key_buffer = self.parser.buffer();
        let (n, m) = self.center.panel().index_vs_total();
        let n_files_string = format!("{n}/{m} ");

        queue!(
            self.stdout,
            cursor::MoveTo(
                (self.layout.width() / 2).saturating_sub(key_buffer.len() as u16 / 2),
                self.layout.footer()
            ),
            style::PrintStyledContent(key_buffer.dark_grey()),
            cursor::MoveTo(
                self.layout
                    .width()
                    .saturating_sub(n_files_string.len() as u16),
                self.layout.footer(),
            ),
            style::PrintStyledContent(n_files_string.white()),
        )?;
        self.redraw.footer = false;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        if !self.redraw.any() {
            return Ok(());
        }
        self.stdout.queue(cursor::Hide)?;
        self.draw_footer()?;
        self.draw_header()?;
        self.draw_panels()?;
        self.draw_console()?;
        self.stdout.flush()
    }

    fn draw_panels(&mut self) -> Result<()> {
        if self.redraw.left {
            self.left.panel().draw(
                &mut self.stdout,
                self.layout.left_x_range.clone(),
                self.layout.y_range.clone(),
            )?;
            self.redraw.left = false;
        }
        if self.redraw.center {
            self.center.panel().draw(
                &mut self.stdout,
                self.layout.center_x_range.clone(),
                self.layout.y_range.clone(),
            )?;
            self.redraw.center = false;
        }
        if self.redraw.right {
            self.right.panel().draw(
                &mut self.stdout,
                self.layout.right_x_range.clone(),
                self.layout.y_range.clone(),
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
            if let Mode::Mkdir { console } = &mut self.mode {
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

    fn undo(&mut self) {
        let last_operation = self.stack.pop();
        if last_operation.is_none() {
            return;
        }
        match last_operation.unwrap() {
            Operation::MoveItems { from, to } => {
                for item in from {
                    let current_path = item.components().last().map(|p| to.join(p));
                }
                todo!("move items back");
            }
            Operation::CopyItems { from, to } => {
                todo!("delete items");
            }
            Operation::Mkdir { path } => {
                todo!("remove directory");
            }
            Operation::Move(_) => {
                todo!("unmove");
            }
        }
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.left.panel_mut().set_hidden(self.show_hidden);
        self.center.panel_mut().set_hidden(self.show_hidden);
        if let PreviewPanel::Dir(panel) = self.right.panel_mut() {
            panel.set_hidden(self.show_hidden);
        };
        self.redraw_everything();
    }

    fn select(&mut self, path: &Path) {
        if self.center.panel().selected_path() == Some(path) {
            return;
        }
        self.center.panel_mut().select_path(path);
        self.right.new_panel(self.center.panel().selected_path());
        self.redraw_center();
        self.redraw_right();
    }

    fn move_up(&mut self, step: usize) {
        if self.center.panel_mut().up(step) {
            self.right.new_panel(self.center.panel().selected_path());
            self.redraw_center();
            self.redraw_right();
            self.stack.push(Operation::Move(Movement::Up));
        }
    }

    fn move_down(&mut self, step: usize) {
        if self.center.panel_mut().down(step) {
            self.right.new_panel(self.center.panel().selected_path());
            self.redraw_center();
            self.redraw_right();
            self.stack.push(Operation::Move(Movement::Down));
        }
    }

    // TODO: Make this more efficient - the swapping was too nice to give it up
    fn move_right(&mut self) {
        if let Some(selected) = self.center.panel().selected_path().map(|p| p.to_path_buf()) {
            // If the selected item is a directory, all panels will shift to the left
            if selected.is_dir() {
                self.previous = self.center.panel().path().to_path_buf();

                // Notification::new()
                //     .summary("move-right")
                //     .body(&format!("dir={}", selected.display()))
                //     .show()
                //     .unwrap();

                // swap left and mid:
                // mem::swap(&mut self.left, &mut self.center);
                // if let PreviewPanel::Dir(right) = self.right.panel_mut() {
                // TODO: Check if this still works with the watchers
                //     mem::swap(right, self.center.panel_mut())
                // }

                // // Recreate mid and right
                // self.center
                //     .content_tx
                //     .send(PanelUpdate {
                //         path: selected,
                //         state: self.center.state.lock().increased(),
                //     })
                //     .expect("Receiver dropped or closed");
                self.left.update_panel(self.center.panel().clone());
                self.center.new_panel(self.right.panel().maybe_path());
                self.right.new_panel(self.center.panel().selected_path());

                // All panels needs to be redrawn
                self.redraw_panels();
                // if let PreviewPanel::Dir(panel) = &mut self.right.panel_mut() {
                //     mem::swap(&mut self.center, panel);
                // } else {
                //     // This should not be possible!
                //     panic!(
                //         "selected item cannot be a directory while right panel is not a dir-panel"
                //     );
                // }
                // // Recreate right panel
                // PanelAction::UpdateMidRight((self.mid.path.clone(), self.mid.selected_path_owned()))
            } else {
                self.open(selected);
            }
            self.stack.push(Operation::Move(Movement::Right));
        }
    }

    // TODO: Make this more efficient - the swapping was too nice to give it up
    fn move_left(&mut self) {
        // If the left panel is empty, we cannot move left:
        if self.left.panel().selected_path().is_none() {
            return;
        }
        self.previous = self.center.panel().path().to_path_buf();

        // All panels will shift to the right
        // and the left panel needs to be recreated:

        // Create right dir-panel from previous mid
        // | l | m | r |
        self.right
            .update_panel(PreviewPanel::Dir(self.center.panel().clone()));
        // | l | m | m |

        // swap left and mid:
        // mem::swap(&mut self.left, &mut self.center);
        self.center.update_panel(self.left.panel().clone());
        // | m | l | m |
        // TODO: When we followed some symlink we don't want to take the parent here.
        self.left.new_panel(self.center.panel().path().parent());
        self.left
            .panel_mut()
            .select_path(self.center.panel().path());

        // All panels needs to be redrawn
        self.redraw_panels();
        self.stack.push(Operation::Move(Movement::Left));
    }

    fn jump(&mut self, path: PathBuf) {
        // Don't do anything, if the path hasn't changed
        if path.as_path() == self.center.panel().path() {
            return;
        }
        if path.exists() {
            self.previous = self.center.panel().path().to_path_buf();
            self.left.new_panel(path.parent());
            self.left.panel_mut().select_path(&path);
            self.center.new_panel(Some(&path));
            self.right.new_panel(self.center.panel().selected_path());
            self.redraw_panels();
        }
    }

    fn move_cursor(&mut self, movement: Movement) {
        // NOTE: Movement functions needs to determine which panels require a redraw.
        match movement {
            Movement::Up => self.move_up(1),
            Movement::Down => self.move_down(1),
            Movement::Left => self.move_left(),
            Movement::Right => self.move_right(),
            Movement::Top => self.move_up(usize::MAX),
            Movement::Bottom => self.move_down(usize::MAX),
            Movement::HalfPageForward => self.move_down(self.layout.height() as usize / 2),
            Movement::HalfPageBackward => self.move_up(self.layout.height() as usize / 2),
            Movement::PageForward => self.move_down(self.layout.height() as usize),
            Movement::PageBackward => self.move_up(self.layout.height() as usize),
            Movement::JumpTo(path) => self.jump(path.into()),
            Movement::JumpPrevious => self.jump(self.previous.clone()),
        };
    }

    fn open(&self, path: PathBuf) {
        let absolute = if path.is_absolute() {
            path
        } else {
            path.canonicalize().unwrap_or_default()
        };
        // Image
        // If the selected item is a file,
        // we need to open it
        if let Some(ext) = absolute.extension().and_then(|ext| ext.to_str()) {
            match ext {
                "png" | "bmp" | "jpg" | "jpeg" | "svg" => {
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
    fn unmark_items(&mut self) {
        self.left
            .panel_mut()
            .elements_mut()
            .for_each(|item| item.unmark());
        self.center
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

    pub async fn run(mut self) -> Result<()> {
        // Initial draw
        self.redraw_everything();
        self.draw()?;

        // Remember path before we jumped into console
        let mut pre_console_path: PathBuf = self.center.panel().path().to_path_buf();

        let trash_dir = tempfile::tempdir()?;

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

                    // Find panel and update it
                    if self.center.check_update(&state) {
                        // Notification::new().summary("update-center").body(&format!("{:?}", state)).show().unwrap();
                        self.center.update_panel(panel);
                        // update preview (if necessary)
                        self.right.new_panel(self.center.panel().selected_path());
                        self.redraw_center();
                        self.redraw_right();
                        self.redraw_console();
                    } else if self.left.check_update(&state) {
                        // Notification::new().summary("update-left").body(&format!("{:?}", state)).show().unwrap();
                        self.left.update_panel(panel);
                        self.left.panel_mut().select_path(self.center.panel().path());
                        self.redraw_left();
                        self.redraw_console();
                    } else {
                        // Notification::new().summary("unknown update").body(&format!("{:?}", state)).show().unwrap();
                    }
                    self.draw()?;
                }
                // Check incoming new preview-panels
                result = self.prev_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break;
                    }
                    let (panel, state) = result.unwrap();

                    if self.right.check_update(&state) {
                        self.right.update_panel(panel);
                        self.redraw_right();
                        self.redraw_console();
                    }
                    self.draw()?;
                }
                // Check incoming new events
                result = event_reader => {
                    // Shutdown if reader has been dropped
                    if result.is_none() {
                        break;
                    }
                    let event = result.unwrap()?;
                    if let Event::Key(key_event) = event {
                        // If we hit escape - go back to normal mode.
                        if let KeyCode::Esc = key_event.code {
                            if let Mode::Console{ .. } = self.mode {
                                self.jump(pre_console_path.clone());
                            }
                            self.mode = Mode::Normal;
                            self.parser.clear();
                            self.redraw_panels();
                            self.redraw_footer();
                        }
                        match &mut self.mode {
                            Mode::Normal => {
                                match self.parser.add_event(key_event) {
                                    Command::Move(direction) => {
                                        self.move_cursor(direction);
                                    }
                                    Command::ToggleHidden => {
                                        self.toggle_hidden();
                                    }
                                    Command::Cd => {
                                        pre_console_path = self.center.panel().path().to_path_buf();
                                        self.mode = Mode::Console { console: DirConsole::from_panel(self.center.panel()) };
                                        self.redraw_console();
                                    }
                                    Command::Mkdir => {
                                        self.mode = Mode::Mkdir { console: DirConsole::from_panel(self.center.panel()) };
                                        self.redraw_console();
                                    }
                                    Command::Mark => {
                                        self.center.panel_mut().mark_selected_item();
                                        self.move_cursor(Movement::Down);
                                    }
                                    Command::Cut => {
                                        self.clipboard = Some(Clipboard { files: self.marked_or_selected(), cut: true });
                                    }
                                    Command::Copy => {
                                        self.clipboard = Some(Clipboard { files: self.marked_or_selected(), cut: false });
                                    }
                                    Command::Delete => {
                                        let files = self.marked_or_selected();
                                        Notification::new()
                                            .summary(&format!("Delete {} items", files.len()))
                                            .body(&format!("{}", trash_dir.path().display())).show().unwrap();
                                        self.unmark_items();
                                        let options = CopyOptions::new().overwrite(true);
                                        self.stack.push(Operation::MoveItems { from: files.clone(), to: trash_dir.path().to_path_buf() });
                                        if let Err(e) = fs_extra::move_items(&files, trash_dir.path(), &options) {
                                                Notification::new().summary("error").body(&format!("{e}")).show().unwrap();
                                        }
                                    }
                                    Command::Paste { overwrite } => {
                                        self.unmark_items();
                                        if let Some(clipboard) = &self.clipboard {
                                            let current_path = self.center.panel().path();

                                            let options = CopyOptions::new().skip_exist(!overwrite).overwrite(overwrite);
                                            let result = if clipboard.cut {
                                                self.stack.push(Operation::MoveItems { from: clipboard.files.clone(), to: current_path.to_path_buf() });
                                                fs_extra::move_items(&clipboard.files, current_path, &options)
                                            } else {
                                                self.stack.push(Operation::CopyItems { from: clipboard.files.clone(), to: current_path.to_path_buf() });
                                                fs_extra::copy_items(&clipboard.files, current_path, &options)
                                            };
                                            if let Err(e) = result {
                                                Notification::new().summary("error").body(&format!("{e}")).show().unwrap();
                                            }
                                            self.redraw_panels();
                                        }
                                    }
                                    Command::Quit => break,
                                    Command::None => self.redraw_footer(),
                                }
                            }
                            Mode::Console{ console } => {
                                match key_event.code {
                                    KeyCode::Backspace => {
                                        if let Some(path) = console.del().map(|p| p.to_path_buf()) {
                                            self.jump(path);
                                        }
                                        self.redraw_console();
                                    }
                                    KeyCode::Enter => {
                                        self.mode = Mode::Normal;
                                        self.redraw_panels();
                                    }
                                    KeyCode::Tab  => {
                                        if let Some(path) = console.tab() {
                                            self.jump(path);
                                        }
                                        self.redraw_console();
                                    }
                                    KeyCode::BackTab  => {
                                        if let Some(path) = console.backtab() {
                                            self.jump(path);
                                        }
                                        self.redraw_console();
                                    }
                                    KeyCode::Char(c) => {
                                        if let Some(path) = console.insert(c) {
                                            self.jump(path);
                                        }
                                        self.redraw_console();
                                    }
                                    _ => (),
                                }
                            }
                            Mode::Mkdir { console } => {
                                match key_event.code {
                                    KeyCode::Backspace => {
                                        console.del();
                                        self.redraw_console();
                                    }
                                    KeyCode::Enter => {
                                        let new_dir = console.joined_input();
                                        self.stack.push(Operation::Mkdir { path: new_dir.clone() });
                                        if let Err(e) = fs_extra::dir::create(new_dir, false) {
                                            Notification::new().summary("error").body(&format!("{e}")).show().unwrap();
                                        }
                                        self.mode = Mode::Normal;
                                        self.redraw_panels();
                                    }
                                    KeyCode::Tab  => {
                                        console.tab();
                                        self.redraw_console();
                                    }
                                    KeyCode::BackTab  => {
                                        console.backtab();
                                        self.redraw_console();
                                    }
                                    KeyCode::Char(c) => {
                                        console.insert(c);
                                        self.redraw_console();
                                    }
                                    _ => (),
                                }
                            }
                            Mode::Search{ input: _ } => {
                                todo!()
                            }
                        }
                    }
                    if let Event::Resize(sx, sy) = event {
                        self.layout = MillerColumns::from_size((sx, sy));
                        self.redraw_everything();
                    }
                    self.draw()?;
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
