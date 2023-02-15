use crossterm::event::{Event, EventStream};
use futures::{FutureExt, StreamExt};

use crate::commands::{Command, CommandParser, Keyboard};

use super::{console::Console, *};

pub struct PanelManager {
    /// Left panel
    left: ManagedPanel<DirPanel>,
    /// Center panel
    center: ManagedPanel<DirPanel>,
    /// Right panel
    right: ManagedPanel<PreviewPanel>,

    /// Console panel
    console: Console,

    /// Miller-Columns layout
    layout: MillerColumns,

    /// Show hidden files
    show_hidden: bool,

    /// Show console
    show_console: bool,

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
            show_hidden: false,
            show_console: false,
            event_reader,
            previous: ".".into(),
            parser,
            stdout,
            dir_rx,
            prev_rx,
        }
    }

    // Prints our header
    fn print_header(&mut self) -> Result<()> {
        let prompt = format!("{}@{}", whoami::username(), whoami::hostname());
        let absolute = canonicalize(self.center.panel().path())?;
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
        Ok(())
    }

    // Prints a footer
    fn print_footer(&mut self) -> Result<()> {
        if let Some(selection) = self.center.panel().selected() {
            let path = selection.path();
            let metadata = path.metadata()?;
            let permissions = unix_mode::to_string(metadata.permissions().mode());

            queue!(
                self.stdout,
                cursor::MoveTo(0, self.layout.footer()),
                Clear(ClearType::CurrentLine),
                style::PrintStyledContent(permissions.dark_cyan()),
                // cursor::MoveTo(x2, y),
            )?;
        }

        let (n, m) = self.center.panel().index_vs_total();
        let n_files_string = format!("{n}/{m} ");

        queue!(
            self.stdout,
            cursor::MoveTo(
                self.layout
                    .width()
                    .saturating_sub(n_files_string.len() as u16),
                self.layout.footer(),
            ),
            style::PrintStyledContent(n_files_string.white()),
        )?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        self.draw_panels()?;
        if self.show_console {
            self.draw_console()?;
        }
        Ok(())
    }

    fn draw_panels(&mut self) -> Result<()> {
        self.print_header()?;
        self.print_footer()?;
        self.left.panel().draw(
            &mut self.stdout,
            self.layout.left_x_range.clone(),
            self.layout.y_range.clone(),
        )?;
        self.center.panel().draw(
            &mut self.stdout,
            self.layout.center_x_range.clone(),
            self.layout.y_range.clone(),
        )?;
        self.right.panel().draw(
            &mut self.stdout,
            self.layout.right_x_range.clone(),
            self.layout.y_range.clone(),
        )?;
        self.stdout.queue(cursor::Hide)?;
        self.stdout.flush()?;
        Ok(())
    }

    fn draw_console(&mut self) -> Result<()> {
        self.console.draw(
            &mut self.stdout,
            self.layout.left_x_range.start..self.layout.right_x_range.end,
            self.layout.y_range.clone(),
        )
    }

    fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.left.panel_mut().set_hidden(self.show_hidden);
        self.center.panel_mut().set_hidden(self.show_hidden);
        if let PreviewPanel::Dir(panel) = self.right.panel_mut() {
            panel.set_hidden(self.show_hidden);
        };
        self.draw_panels()
    }

    fn move_up(&mut self, step: usize) -> bool {
        if self.center.panel_mut().up(step) {
            self.right.new_panel(self.center.panel().selected_path());
            true
        } else {
            false
        }
    }

    fn move_down(&mut self, step: usize) -> bool {
        if self.center.panel_mut().down(step) {
            self.right.new_panel(self.center.panel().selected_path());
            true
        } else {
            false
        }
    }

    fn move_right(&mut self) -> bool {
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

                true
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
                false
            }
        } else {
            false
        }
    }

    fn move_left(&mut self) -> bool {
        // If the left panel is empty, we cannot move left:
        if self.left.panel().selected_path().is_none() {
            return false;
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

        true
    }

    fn jump(&mut self, path: PathBuf) -> bool {
        if path.exists() {
            self.previous = self.center.panel().path().to_path_buf();
            self.left.new_panel(path.parent());
            self.center.new_panel(Some(&path));
            // TODO: Update right panel whenever we update the center
            self.right.new_panel(self.center.panel().selected_path());
            true
        } else {
            false
        }
    }

    fn move_cursor(&mut self, movement: Movement) -> bool {
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
        }
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
    }

    pub async fn run(mut self) -> Result<()> {
        // Initialize panels
        self.draw_panels()?;

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

                    let updated;
                    // Find panel and update it
                    if self.center.check_update(&state) {
                        // Notification::new().summary("update-center").body(&format!("{:?}", state)).show().unwrap();
                        self.center.update_panel(panel);
                        // update preview (if necessary)
                        self.right.new_panel(self.center.panel().selected_path());
                        updated = true;
                    } else if self.left.check_update(&state) {
                        // Notification::new().summary("update-left").body(&format!("{:?}", state)).show().unwrap();
                        self.left.update_panel(panel);
                        self.left.panel_mut().select(self.center.panel().path());
                        updated = true;
                    } else {
                        // Notification::new().summary("unknown update").body(&format!("{:?}", state)).show().unwrap();
                        updated = false;
                    }
                    if updated {
                        self.draw_panels()?;
                    }
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
                        self.draw_panels()?;
                    }
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
                                if self.move_cursor(direction) {
                                    self.draw_panels()?;
                                }
                            }
                            Command::ToggleHidden => {
                                self.toggle_hidden()?;
                            }
                            Command::ShowConsole => {
                                self.show_console = true;
                                self.parser.set_console_mode(true);
                                self.draw_console()?;
                            }
                            Command::Input(input) => {
                                if self.show_console {
                                    match input {
                                        Keyboard::Char(c) => {
                                            self.console.insert(c);
                                            self.draw_console()?;
                                        }
                                        Keyboard::Backspace => {
                                            self.console.del();
                                            self.draw_console()?;
                                        }
                                        Keyboard::Enter | Keyboard::Esc=> {
                                            self.show_console = false;
                                            self.parser.set_console_mode(false);
                                            self.console.clear();
                                            self.draw_panels()?;
                                        }
                                    }
                                }
                            }
                            Command::Quit => break,
                            Command::None => (),
                        }
                    }
                    if let Event::Resize(sx, sy) = event {
                        self.layout = MillerColumns::from_size((sx, sy));
                        self.draw()?;
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
