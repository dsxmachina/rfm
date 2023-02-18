use crossterm::event::{Event, EventStream, KeyCode};
use futures::{FutureExt, StreamExt};
use notify_rust::Notification;

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
            let permissions = if let Ok(metadata) = path.metadata() {
                unix_mode::to_string(metadata.permissions().mode())
            } else {
                String::from("unknown")
            };

            queue!(
                self.stdout,
                cursor::MoveTo(0, self.layout.footer()),
                Clear(ClearType::CurrentLine),
                style::PrintStyledContent(permissions.dark_cyan()),
            )?;
        }

        let key_buffer = self.parser.buffer();
        let (n, m) = self.center.panel().index_vs_total();
        let n_files_string = format!("{n}/{m} ");

        queue!(
            self.stdout,
            cursor::MoveTo(self.layout.width() / 3, self.layout.footer()),
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
        }
    }

    fn move_down(&mut self, step: usize) {
        if self.center.panel_mut().down(step) {
            self.right.new_panel(self.center.panel().selected_path());
            self.redraw_center();
            self.redraw_right();
        }
    }

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
                mem::swap(&mut self.left, &mut self.center);
                if let PreviewPanel::Dir(right) = self.right.panel_mut() {
                    // TODO: Check if this still works with the watchers
                    mem::swap(right, self.center.panel_mut())
                }

                // Recreate mid and right
                self.center
                    .content_tx
                    .send(PanelUpdate {
                        path: selected,
                        state: self.center.state.increased(),
                        hash: self.center.panel.content_hash(),
                    })
                    .expect("Receiver dropped or closed");
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
        }
    }

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
            .panel_mut()
            .update_content(PreviewPanel::Dir(self.center.panel().clone()));
        // | l | m | m |

        // swap left and mid:
        mem::swap(&mut self.left, &mut self.center);
        // | m | l | m |
        // TODO: When we followed some symlink we don't want to take the parent here.
        self.left.new_panel(self.center.panel().path().parent());
        self.left
            .panel_mut()
            .select_path(self.center.panel().path());

        // All panels needs to be redrawn
        self.redraw_panels();
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
                            self.mode = Mode::Normal;
                            self.jump(pre_console_path.clone());
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
                                    Command::ShowConsole => {
                                        pre_console_path = self.center.panel().path().to_path_buf();
                                        self.mode = Mode::Console { console: DirConsole::from_panel(self.center.panel()) };
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
                                        Notification::new().summary(&format!("Delete {} items", files.len())).show().unwrap();
                                        self.unmark_items();
                                        for f in files {
                                            if f.is_dir() {
                                                // let _ = std::fs::remove_dir_all(f);
                                            } else {
                                                // let _ = std::fs::remove_file(f);
                                            }
                                        }
                                    }
                                    Command::Paste { overwrite: _ } => {
                                        self.unmark_items();
                                        if let Some(clipboard) = &self.clipboard {
                                            let current_path = self.center.panel().path();
                                            Notification::new().summary(&format!("cut={}, n-items={}", clipboard.cut ,clipboard.files.len())).show().unwrap();
                                            let func = if clipboard.cut {
                                                std::fs::rename
                                            } else {
                                                |from, to| { std::fs::copy(from, to).map(|_| ()) }
                                            };
                                            for f in clipboard.files.iter() {
                                                let filename = f.file_name().unwrap_or_default();
                                                let to = current_path.join(filename);
                                                let _ = func(f, to);
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
                                    // TODO: This is not working correctly, therefore just leave it out
                                    // KeyCode::Down => {
                                    //     self.move_cursor(Movement::Down);
                                    //     self.console.down();
                                    //     // self.console.open(self.center.panel().path());
                                    //     self.redraw_console();
                                    // }
                                    // KeyCode::Up => {
                                    //     self.move_cursor(Movement::Up);
                                    //     self.console.up();
                                    //     // self.console.open(self.center.panel().path());
                                    //     self.redraw_console();
                                    // }
                                    // KeyCode::Left => {
                                    //     self.move_cursor(Movement::Left);
                                    //     self.console.open(self.center.panel().path());
                                    //     self.redraw_console();
                                    // }
                                    // KeyCode::Right => {
                                    //     self.move_cursor(Movement::Right);
                                    //     self.console.open(self.center.panel().path());
                                    //     self.redraw_console();
                                    // }
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
