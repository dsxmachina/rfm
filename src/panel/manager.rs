use crossterm::event::{Event, EventStream, KeyCode};
use futures::{FutureExt, StreamExt};

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

struct Show {
    hidden: bool,
    console: bool,
}

pub struct PanelManager {
    /// Left panel
    left: ManagedPanel<DirPanel>,
    /// Center panel
    center: ManagedPanel<DirPanel>,
    /// Right panel
    right: ManagedPanel<PreviewPanel>,

    /// Console panel
    console: DirConsole,

    /// Miller-Columns layout
    layout: MillerColumns,

    /// Indicates what we want to show or hide
    show: Show,

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
            layout,
            console: Default::default(),
            show: Show {
                hidden: false,
                console: false,
            },
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
        let absolute = canonicalize(
            self.center
                .panel()
                .selected_path()
                .unwrap_or_else(|| self.center.panel().path()),
        )?;
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
            let metadata = path.metadata()?;
            let permissions = unix_mode::to_string(metadata.permissions().mode());

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
            style::PrintStyledContent(key_buffer.red()),
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
        if self.show.console {
            self.draw_console()?;
        }
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
            self.console.draw(
                &mut self.stdout,
                self.layout.left_x_range.start..self.layout.right_x_range.end,
                self.layout.y_range.clone(),
            )?;
            self.redraw.console = false;
        }
        Ok(())
    }

    fn toggle_hidden(&mut self) {
        self.show.hidden = !self.show.hidden;
        self.left.panel_mut().set_hidden(self.show.hidden);
        self.center.panel_mut().set_hidden(self.show.hidden);
        if let PreviewPanel::Dir(panel) = self.right.panel_mut() {
            panel.set_hidden(self.show.hidden);
        };
        self.redraw_everything();
    }

    fn select(&mut self, path: &Path) {
        if self.center.panel().selected_path() == Some(path) {
            return;
        }
        self.center.panel_mut().select(path);
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

                // Recreate mid and right
                self.center.new_panel(Some(&selected));
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
        self.left.panel_mut().select(self.center.panel().path());

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
            self.left.panel_mut().select(&path);
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
                        self.left.panel_mut().select(self.center.panel().path());
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
                        if !self.show.console {
                        match self.parser.add_event(key_event) {
                            Command::Move(direction) => {
                                self.move_cursor(direction);
                            }
                            Command::ToggleHidden => {
                                self.toggle_hidden();
                            }
                            Command::ShowConsole => {
                                pre_console_path = self.center.panel().path().to_path_buf();
                                self.show.console = true;
                                self.parser.set_console_mode(true);
                                self.console.open(self.center.panel().path());
                                let selected = self
                                    .center
                                    .panel()
                                    .selected_path()
                                    .and_then(|p| p.file_name())
                                    .and_then(|f| f.to_str())
                                    .and_then(|s| Some(s.to_string()))
                                    .unwrap_or_default();
                                self.console.set_to(selected);
                                self.redraw_console();
                            }
                            Command::Esc => {
                                // Stop whatever we are doing.
                                if self.show.console {
                                    self.show.console = false;
                                    self.parser.set_console_mode(false);
                                    self.console.clear();
                                    self.redraw_panels();
                                }
                                self.parser.clear_buffer();
                                self.redraw_footer();
                            }
                            Command::Quit => break,
                            Command::None => (),
                        }
                        } else {
                            match key_event.code {
                                KeyCode::Backspace => {
                                    if let Some(path) = self.console.del().map(|p| p.to_path_buf()) {
                                        self.jump(path);
                                    }
                                    self.redraw_console();
                                }
                                KeyCode::Enter => {
                                    self.show.console = false;
                                    self.parser.set_console_mode(false);
                                    self.console.clear();
                                    self.redraw_panels();
                                }
                                KeyCode::Down => {
                                    self.move_cursor(Movement::Down);
                                    self.console.down();
                                    // self.console.open(self.center.panel().path());
                                    self.redraw_console();
                                }
                                KeyCode::Up => {
                                    self.move_cursor(Movement::Up);
                                    self.console.up();
                                    // self.console.open(self.center.panel().path());
                                    self.redraw_console();
                                }
                                KeyCode::Left => {
                                    self.move_cursor(Movement::Left);
                                    self.console.open(self.center.panel().path());
                                    self.redraw_console();
                                }
                                KeyCode::Right => {
                                    self.move_cursor(Movement::Right);
                                    self.console.open(self.center.panel().path());
                                    self.redraw_console();
                                }
                                KeyCode::Tab  => {
                                    if let Some(path) = self.console.tab() {
                                        self.jump(path);
                                    }
                                    self.redraw_console();
                                }
                                KeyCode::BackTab  => {
                                    if let Some(path) = self.console.backtab() {
                                        self.jump(path);
                                    }
                                    self.redraw_console();
                                }
                                KeyCode::Char(c) => {
                                    if let Some(path) = self.console.insert(c) {
                                        self.jump(path);
                                    }
                                    self.redraw_console();
                                }
                                KeyCode::Esc => {
                                    self.show.console = false;
                                    self.parser.set_console_mode(false);
                                    self.console.clear();
                                    self.jump(pre_console_path.clone());
                                    self.redraw_panels();
                                }
                                _ => (),
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
