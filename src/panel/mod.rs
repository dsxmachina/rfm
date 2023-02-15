use cached::{cached_result, SizedCache};
use crossterm::{
    cursor,
    event::EventStream,
    queue,
    style::{self, Colors, Print, PrintStyledContent, ResetColor, SetColors, Stylize},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use fasthash::MetroHasher;
use image::DynamicImage;
use notify_rust::Notification;
use pad::PadStr;
use std::{
    cmp::Ordering,
    fs::{canonicalize, read_dir, DirEntry, File},
    hash::{Hash, Hasher},
    io::{self, stdout, BufRead, Stdout, Write},
    mem,
    ops::Range,
    os::unix::prelude::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::UNIX_EPOCH,
};
use tokio::sync::mpsc;

use crate::{
    commands::Movement,
    content::{hash_elements, SharedCache},
};

mod directory;
mod manager;
mod preview;

pub use directory::{DirElem, DirPanel};
pub use preview::{FilePreview, Preview, PreviewPanel};

/// Basic trait that lets us draw something on the terminal in a specified range.
pub trait Draw {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()>;
}

/// Basic trait for managing the content of a panel
pub trait PanelContent: Draw + Clone + Send {
    /// Path of the panel
    fn path(&self) -> &Path;

    /// Hash of the panels content
    fn content_hash(&self) -> u64;

    /// Updates the content of the panel
    fn update_content(&mut self, content: Self);
}

/// Basic trait for our panels.
pub trait BasePanel: PanelContent {
    /// Creates an empty panel without content
    fn empty() -> Self;

    /// Creates a temporary panel to indicate that we are still loading
    /// some data
    fn loading(path: PathBuf) -> Self;
}

/// Combines all data that is necessary to update a panel.
///
/// Will be send as a request to the [`ContentManager`].
#[derive(Debug)]
pub struct PanelUpdate {
    pub path: PathBuf,
    pub panel_id: u64,
    pub state_cnt: u64,
    pub hash: u64,
}

pub struct ManagedPanel<PanelType: BasePanel> {
    /// Panel to be updated.
    panel: PanelType,

    /// Counter that increases everytime we update the panel.
    ///
    /// This prevents the manager from accidently overwriting the panel with older content
    /// that was requested before some other content, that is displayed now.
    /// Since the [`ContentManager`] works asynchronously we need this mechanism,
    /// because there is no guarantee that requests that were sent earlier,
    /// will also finish earlier.
    state_cnt: u64,

    /// ID of the panel that is managed by the updater.
    ///
    /// The ID is generated randomly upon creation of the PanelUpdater.
    /// When we send an update request to the [`ContentManager`], we attach the ID
    /// to the request, so that the [`PanelManager`] is able to know which panel needs to be updated.
    panel_id: u64,

    /// Cached panels from previous requests.
    ///
    /// When we want to create a new panel, we first look into the cache,
    /// if a panel for the specified path was already created in the past.
    /// If so, we still send an update request to the [`ContentManager`],
    /// to avoid working with outdated information.
    /// If the cache is empty, we generate a `loading`-panel (see [`DirPanel::loading`]).
    cache: SharedCache<PanelType>,

    /// Sends request for new panel content.
    content_tx: mpsc::Sender<PanelUpdate>,
}

impl<PanelType: BasePanel> ManagedPanel<PanelType> {
    pub fn new(
        panel: PanelType,
        cache: SharedCache<PanelType>,
        content_tx: mpsc::Sender<PanelUpdate>,
    ) -> Self {
        // Generate a random id here - because we only have three panels,
        // the chance of collision is pretty low.
        let panel_id = rand::random();
        ManagedPanel {
            panel,
            state_cnt: 0,
            panel_id,
            cache,
            content_tx,
        }
    }

    /// Generates a new panel for the given path.
    ///
    /// Uses cached values to instantly display something, while in the background
    /// the [`ContentManager`] is triggered to load new data.
    /// If the cache is empty, a generic "loading..." panel is created.
    /// An empty panel is created if the given path is `None`.
    pub async fn update_panel<P: AsRef<Path>>(&mut self, path: Option<P>) {
        if let Some(path) = path.and_then(|p| canonicalize(p.as_ref()).ok()) {
            let panel = self
                .cache
                .get(&path)
                .unwrap_or_else(|| PanelType::loading(path.clone()));
            self.panel.update_content(panel);
            // Send update request for given panel
            self.content_tx
                .send(PanelUpdate {
                    path,
                    panel_id: self.panel_id,
                    state_cnt: self.state_cnt + 1,
                    hash: self.panel.content_hash(),
                })
                .await
                .expect("Receiver dropped or closed");
        } else {
            self.panel.update_content(PanelType::empty());
        }
        // Increase state counter
        self.state_cnt += 1;
    }

    /// Inserts a pre-generated panel.
    ///
    /// The panel is only inserted, if the external state-counter is higher than
    /// the internally saved value.
    pub fn insert_panel(&mut self, panel: PanelType, state_cnt: u64) {
        if self.state_cnt < state_cnt {
            self.panel.update_content(panel);
            self.state_cnt += 1;
        }
    }
}

// TODO: Remove all of this

/// Enum to indicate which panel is selected for the given operation
#[derive(Debug, Clone)]
pub enum Select {
    Left,
    Mid,
    Right,
}

/// State of a panel. Is used to avoid updating the panel with "old" data.
#[derive(Debug, Clone)]
pub struct PanelState {
    pub state_cnt: u64,
    pub hash: u64,
    pub panel: Select,
}

#[derive(Clone)]
struct Ranges {
    left_x_range: Range<u16>,
    mid_x_range: Range<u16>,
    right_x_range: Range<u16>,
    y_range: Range<u16>,
    width: u16,
}

impl Ranges {
    pub fn from_size(terminal_size: (u16, u16)) -> Self {
        let (sx, sy) = terminal_size;
        Self {
            left_x_range: 0..(sx / 8),
            mid_x_range: (sx / 8)..(sx / 2),
            right_x_range: (sx / 2)..sx,
            y_range: 1..sy.saturating_sub(1), // 1st line is reserved for the header, last for the footer
            width: sx,
        }
    }

    pub fn height(&self) -> u16 {
        self.y_range.end.saturating_sub(self.y_range.start)
    }
}

// Prints our header
fn print_header<P: AsRef<Path>>(stdout: &mut Stdout, path: P) -> Result<()> {
    let prompt = format!("{}@{}", whoami::username(), whoami::hostname());
    let absolute = canonicalize(path.as_ref())?;
    let file_name = absolute
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    let absolute = absolute.to_str().unwrap_or_default();

    let (prefix, suffix) = absolute.split_at(absolute.len() - file_name.len());

    queue!(
        stdout,
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
fn print_footer(stdout: &mut Stdout, mid: &DirPanel, width: u16, y: u16) -> Result<()> {
    if let Some(selection) = mid.selected() {
        let path = selection.path();
        let metadata = path.metadata()?;
        let permissions = unix_mode::to_string(metadata.permissions().mode());

        queue!(
            stdout,
            cursor::MoveTo(0, y),
            Clear(ClearType::CurrentLine),
            style::PrintStyledContent(permissions.dark_cyan()),
            // cursor::MoveTo(x2, y),
        )?;
    }
    let (n, m) = if mid.show_hidden {
        (mid.selected.saturating_add(1), mid.elements.len())
    } else {
        (mid.non_hidden_idx.saturating_add(1), mid.non_hidden.len())
    };

    let n_files_string = format!("{n}/{m} ");

    queue!(
        stdout,
        cursor::MoveTo(width.saturating_sub(n_files_string.len() as u16), y),
        style::PrintStyledContent(n_files_string.white()),
    )?;
    Ok(())
}

/// Type that indicates that one or more panels have changed.
pub enum PanelAction {
    /// The selection of the middle panel has changed,
    /// or we have moved to the right.
    /// Anyway, we need a new preview buffer.
    UpdatePreview(Option<PathBuf>),

    /// We have moved right, therefore we need to update the preview panel,
    /// and trigger the content-manager to reload new data
    UpdateMidRight((PathBuf, Option<PathBuf>)),

    /// All buffers have changed. This happens when we jump around
    /// the directories.
    UpdateAll(PathBuf),

    /// We have moved to the left. The middle and right panels
    /// do not need an update, but the left one does.
    /// The given path is the path of the middle panel,
    /// so we can create the left-panel with the "from-parent" method.
    UpdateLeft(PathBuf),

    /// Indicate that we want to open something
    Open(PathBuf),

    /// Nothing has changed at all
    None,
}

/// Create a set of Panels in "Miller-Columns" style.
pub struct MillerPanels {
    // Panels
    left: DirPanel,
    mid: DirPanel,
    right: PreviewPanel,

    // Panel state counters
    state_cnt: (u64, u64, u64),

    // Data
    ranges: Ranges,
    // prev-path (after jump-mark)
    prev: PathBuf,

    show_hidden: bool,

    // handle to standard-output
    stdout: Stdout,
}

/// Reads the content of a directory
fn dir_content(path: PathBuf) -> Result<Vec<DirElem>> {
    // read directory
    let dir = read_dir(path)?;
    let mut out = Vec::new();
    for item in dir {
        let item_path = canonicalize(item?.path())?;
        out.push(DirElem::from(item_path))
    }
    out.sort();
    Ok(out)
}

impl MillerPanels {
    pub fn new() -> Result<Self> {
        let terminal_size = terminal::size()?;
        let parent_path = PathBuf::from("..").canonicalize()?;
        let current_path = PathBuf::from(".").canonicalize()?;
        let mut left = DirPanel::new(dir_content(parent_path.clone())?, parent_path);
        left.select(&current_path);
        let mid = DirPanel::new(dir_content(current_path.clone())?, current_path.clone());
        let right = PreviewPanel::empty();
        let ranges = Ranges::from_size(terminal_size);
        Ok(MillerPanels {
            left,
            mid,
            right,
            state_cnt: (0, 0, 0),
            ranges,
            prev: current_path,
            show_hidden: false,
            stdout: stdout(),
        })
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.mid.selected_path()
    }

    pub fn terminal_resize(&mut self, terminal_size: (u16, u16)) -> Result<()> {
        self.ranges = Ranges::from_size(terminal_size);
        self.draw()
    }

    pub fn toggle_hidden(&mut self) -> Result<()> {
        self.show_hidden = !self.show_hidden;
        self.left.set_hidden(self.show_hidden);
        self.mid.set_hidden(self.show_hidden);
        if let PreviewPanel::Dir(panel) = &mut self.right {
            panel.set_hidden(self.show_hidden);
        };
        self.draw()
    }

    pub fn update_panel(&mut self, panel: DirPanel, panel_state: PanelState) {
        match panel_state.panel {
            Select::Left => {
                if panel_state.state_cnt > self.state_cnt.0 {
                    self.update_left(panel);
                } else {
                    self.state_left();
                }
            }
            Select::Mid => {
                if panel_state.state_cnt > self.state_cnt.1 {
                    self.update_mid(panel);
                } else {
                    self.state_mid();
                }
            }
            Select::Right => {
                if panel_state.state_cnt > self.state_cnt.2 {
                    self.update_right(PreviewPanel::Dir(panel));
                } else {
                    self.state_right();
                }
            }
        };
    }

    /// Exclusively updates the right (preview) panel
    pub fn update_preview(&mut self, preview: PreviewPanel, panel_state: PanelState) {
        if let Select::Right = &panel_state.panel {
            if panel_state.state_cnt > self.state_cnt.2 {
                self.update_right(preview);
            }
        }
        // self.state_right()
    }

    /// Updates the left panel and returns the updates panel-state
    fn update_left(&mut self, panel: DirPanel) {
        self.left = panel;
        self.left.show_hidden = self.show_hidden;
        self.left.select(self.mid.path());
        self.state_cnt.0 += 1;
        // self.state_left()
    }

    /// Updates the middle panel and returns the updates panel-state
    fn update_mid(&mut self, panel: DirPanel) {
        self.mid = panel;
        self.mid.show_hidden = self.show_hidden;
        self.state_cnt.1 += 1;
        // self.state_mid()
    }

    /// Updates the right panel and returns the updates panel-state
    fn update_right(&mut self, panel: PreviewPanel) {
        self.right = panel;
        if let PreviewPanel::Dir(panel) = &mut self.right {
            panel.show_hidden = self.show_hidden;
        }
        self.state_cnt.2 += 1;
        // self.state_right()
    }

    pub fn state_left(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.0,
            hash: self.left.hash,
            panel: Select::Left,
        }
    }

    pub fn state_mid(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.1,
            hash: self.mid.hash,
            panel: Select::Mid,
        }
    }

    pub fn state_right(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.2,
            hash: self.right.content_hash(),
            panel: Select::Right,
        }
    }

    pub fn move_cursor(&mut self, movement: Movement) -> PanelAction {
        match movement {
            Movement::Up => self.move_up(1),
            Movement::Down => self.move_down(1),
            Movement::Left => self.move_left(),
            Movement::Right => self.move_right(),
            Movement::Top => self.move_up(usize::MAX),
            Movement::Bottom => self.move_down(usize::MAX),
            Movement::HalfPageForward => self.move_down(self.ranges.height() as usize / 2),
            Movement::HalfPageBackward => self.move_up(self.ranges.height() as usize / 2),
            Movement::PageForward => self.move_down(self.ranges.height() as usize),
            Movement::PageBackward => self.move_up(self.ranges.height() as usize),
            Movement::JumpTo(path) => self.jump(path.into()),
            Movement::JumpPrevious => self.jump(self.prev.clone()),
        }
    }

    fn jump(&mut self, path: PathBuf) -> PanelAction {
        if path.exists() {
            // Remember path
            self.prev = self.mid.path.clone();
            PanelAction::UpdateAll(path)
        } else {
            PanelAction::None
        }
    }

    // NOTE: The movement functions need to change here
    //
    // -> They just indicate which panel needs to change,
    // without actually changing it.
    //
    // We will create a "update panel" function,
    // that the manager can call, whenever a new panel is ready to be drawn.
    fn move_up(&mut self, step: usize) -> PanelAction {
        if self.mid.up(step) {
            PanelAction::UpdatePreview(self.mid.selected_path_owned())
        } else {
            PanelAction::None
        }
    }

    fn move_down(&mut self, step: usize) -> PanelAction {
        if self.mid.down(step) {
            PanelAction::UpdatePreview(self.mid.selected_path_owned())
        } else {
            PanelAction::None
        }
    }

    // TODO: We could improve, that we don't jump into directories where we do not have access
    fn move_right(&mut self) -> PanelAction {
        if let Some(selected) = self.mid.selected_path() {
            if selected.is_dir() {
                // Remember path
                self.prev = self.mid.path.clone();

                // If the selected item is a directory,
                // all panels will shift to the left,
                // and the right panel needs to be recreated:

                // We do this by swapping:
                // | l | m | r |  will become | m | r | l |
                // swap left and mid:
                // | m | l | r |
                mem::swap(&mut self.left, &mut self.mid);
                if let PreviewPanel::Dir(panel) = &mut self.right {
                    mem::swap(&mut self.mid, panel);
                } else {
                    // This should not be possible!
                    panic!(
                        "selected item cannot be a directory while right panel is not a dir-panel"
                    );
                }
                // Recreate right panel
                PanelAction::UpdateMidRight((self.mid.path.clone(), self.mid.selected_path_owned()))
            } else {
                PanelAction::Open(selected.to_path_buf())
            }
        } else {
            PanelAction::None
        }
    }

    fn move_left(&mut self) -> PanelAction {
        // If the left panel is empty, we cannot move left:
        if self.left.selected_path().is_none() {
            return PanelAction::None;
        }
        // Remember path
        self.prev = self.mid.path.clone();

        // All panels will shift to the right
        // and the left panel needs to be recreated:

        // Create right dir-panel from previous mid
        // | l | m | r |
        self.right = PreviewPanel::Dir(self.mid.clone());
        // | l | m | m |

        // swap left and mid:
        mem::swap(&mut self.left, &mut self.mid);
        // | m | l | m |

        PanelAction::UpdateLeft(self.mid.path.clone())
    }

    pub fn draw(&mut self) -> Result<()> {
        let stdout = &mut self.stdout;
        if let Some(path) = self.mid.selected_path() {
            print_header(stdout, path)?;
        } else {
            if let Some(path) = self.left.selected_path() {
                print_header(stdout, path)?;
            }
        }

        print_footer(
            stdout,
            &self.mid,
            self.ranges.width,
            self.ranges.y_range.end.saturating_add(1),
        )?;

        self.left.draw(
            stdout,
            self.ranges.left_x_range.clone(),
            self.ranges.y_range.clone(),
        )?;
        self.mid.draw(
            stdout,
            self.ranges.mid_x_range.clone(),
            self.ranges.y_range.clone(),
        )?;

        match &self.right {
            PreviewPanel::Dir(panel) => panel.draw(
                stdout,
                self.ranges.right_x_range.clone(),
                self.ranges.y_range.clone(),
            )?,
            PreviewPanel::File(panel) => panel.draw(
                stdout,
                self.ranges.right_x_range.clone(),
                self.ranges.y_range.clone(),
            )?,
        }
        self.stdout.queue(cursor::Hide)?;
        self.stdout.flush()?;
        Ok(())
    }
}
