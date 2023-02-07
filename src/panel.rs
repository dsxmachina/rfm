use cached::{cached_result, SizedCache};
use crossterm::{
    cursor,
    event::EventStream,
    queue,
    style::{self, Print, PrintStyledContent, Stylize},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use notify_rust::Notification;
use pad::PadStr;
use std::{
    cmp::Ordering,
    fs::{canonicalize, read_dir, DirEntry},
    io::{stdout, Stdout, Write},
    mem,
    ops::Range,
    os::unix::prelude::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
};

use crate::commands::Movement;

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
    pub panel: Select,
}

/// An element of a directory.
///
/// Shorthand for saving a path together whith what we want to display.
/// E.g. a file with path `/home/user/something.txt` should only be
/// displayed as `something.txt`.
#[derive(Debug, Clone, PartialEq, Eq, Ord)]
pub struct DirElem {
    name: String,
    path: PathBuf,
    is_hidden: bool,
}

impl DirElem {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn print_styled(&self, selected: bool, max_len: u16) -> PrintStyledContent<String> {
        let name =
            format!(" {}", self.name).with_exact_width(usize::from(max_len).saturating_sub(1));
        if self.path.is_dir() {
            if selected {
                PrintStyledContent(name.dark_green().bold().negative())
            } else {
                PrintStyledContent(name.dark_green().bold())
            }
        } else {
            if selected {
                PrintStyledContent(name.grey().negative().bold())
            } else {
                PrintStyledContent(name.grey())
            }
        }
    }
}

impl<P: AsRef<Path>> From<P> for DirElem {
    fn from(path: P) -> Self {
        let name = path
            .as_ref()
            .file_name()
            .map(|p| p.to_str())
            .flatten()
            .map(|s| s.to_string())
            .unwrap_or_default();

        let is_hidden = name.starts_with(".");

        // Always use an absolute path here
        let path: PathBuf = canonicalize(path.as_ref()).unwrap_or_else(|_| path.as_ref().into());

        DirElem {
            path,
            name,
            is_hidden,
        }
    }
}

impl AsRef<DirElem> for DirElem {
    fn as_ref(&self) -> &DirElem {
        &self
    }
}

impl PartialOrd for DirElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.path.is_dir() {
            if other.path.is_dir() {
                return self
                    .name()
                    .to_lowercase()
                    .partial_cmp(&other.name().to_lowercase());
            } else {
                return Some(Ordering::Less);
            }
        } else {
            if other.path.is_dir() {
                return Some(Ordering::Greater);
            } else {
                return self
                    .name()
                    .to_lowercase()
                    .partial_cmp(&other.name().to_lowercase());
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewPanel {
    path: PathBuf,
}

impl PreviewPanel {
    pub fn new(path: PathBuf) -> Self {
        PreviewPanel { path }
    }

    /// Draws the panel in its current state.
    pub fn draw(
        &self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start + 1);
        let path = self
            .path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| Some(s.to_string()))
            .unwrap_or_default();
        let preview_string = format!("Preview of {}", path).with_exact_width(width as usize);
        queue!(
            stdout,
            cursor::MoveTo(x_range.start, y_range.start),
            PrintStyledContent("|".dark_green().bold()),
            cursor::MoveTo(x_range.start + 1, y_range.start),
            PrintStyledContent(preview_string.magenta()),
        )?;
        for y in y_range.start + 1..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("|".dark_green().bold()),
            )?;
            for x in x_range.start + 1..x_range.end {
                queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
            }
        }

        Ok(())
    }
}

pub enum Panel {
    /// Directory preview
    Dir(DirPanel),
    /// File preview
    Preview(PreviewPanel),
    /// No content
    Empty,
}

impl Panel {
    pub fn from_path<P: AsRef<Path>>(maybe_path: Option<P>) -> Result<Panel> {
        if let Some(path) = maybe_path {
            if path.as_ref().is_dir() {
                Ok(Panel::Dir(DirPanel::empty()))
            } else {
                Ok(Panel::Preview(PreviewPanel::new(path.as_ref().into())))
            }
        } else {
            Ok(Panel::Empty)
        }
    }

    pub fn empty() -> Panel {
        Panel::Empty
    }

    pub fn path(&self) -> Option<PathBuf> {
        match self {
            Panel::Dir(panel) => Some(panel.path.clone()),
            Panel::Preview(panel) => Some(panel.path.clone()),
            Panel::Empty => None,
        }
    }
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
        // let permissions = format!("{:o}", metadata.permissions().mode());
        let permissions = unix_mode::to_string(metadata.permissions().mode());

        // let x2 = permissions.len() as u16 + 1;

        queue!(
            stdout,
            cursor::MoveTo(0, y),
            Clear(ClearType::CurrentLine),
            style::PrintStyledContent(permissions.dark_cyan()),
            // cursor::MoveTo(x2, y),
        )?;
    }
    // queue!(
    //     stdout,
    //     cursor::MoveTo(0, 0),
    //     Clear(ClearType::CurrentLine),
    //     style::PrintStyledContent(prompt.dark_green().bold()),
    //     style::Print(" "),
    //     style::PrintStyledContent(prefix.to_string().dark_blue().bold()),
    //     style::PrintStyledContent(suffix.to_string().white().bold()),
    // )?;
    Ok(())
}

/// Type that indicates that one or more panels have changed.
pub enum PanelAction {
    /// The selection of the middle panel has changed,
    /// or we have moved to the right.
    /// Anyway, we need a new preview buffer.
    UpdatePreview(Option<PathBuf>),

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
    right: Panel,

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
        let mut stdout = stdout();
        // Start with a clear screen
        stdout
            .queue(cursor::Hide)?
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?;
        let terminal_size = terminal::size()?;
        let left = DirPanel::new(dir_content("..".into())?, "..".into());
        let mid = DirPanel::new(dir_content(".".into())?, ".".into());
        let right = Panel::empty();
        let ranges = Ranges::from_size(terminal_size);
        Ok(MillerPanels {
            left,
            mid,
            right,
            state_cnt: (0, 0, 0),
            ranges,
            prev: ".".into(),
            show_hidden: false,
            stdout,
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
        self.left.toggle_hidden();
        self.mid.toggle_hidden();
        if let Panel::Dir(panel) = &mut self.right {
            panel.toggle_hidden();
        };
        self.draw()
    }

    pub fn update_panel(&mut self, panel: DirPanel, panel_state: PanelState) -> PanelState {
        let state = match panel_state.panel {
            Select::Left => {
                if panel_state.state_cnt > self.state_cnt.0 {
                    self.update_left(panel)
                } else {
                    self.state_left()
                }
            }
            Select::Mid => {
                if panel_state.state_cnt > self.state_cnt.1 {
                    self.update_mid(panel)
                } else {
                    self.state_mid()
                }
            }
            Select::Right => {
                if panel_state.state_cnt > self.state_cnt.2 {
                    self.update_right(Panel::Dir(panel))
                } else {
                    self.state_right()
                }
            }
        };
        self.draw().unwrap();
        state
    }

    /// Exclusively updates the right (preview) panel
    pub fn update_preview(&mut self, preview: Panel, panel_state: PanelState) -> PanelState {
        if let Select::Right = &panel_state.panel {
            if panel_state.state_cnt > self.state_cnt.2 {
                return self.update_right(preview);
            }
        }
        self.state_right()
    }

    /// Updates the left panel and returns the updates panel-state
    fn update_left(&mut self, panel: DirPanel) -> PanelState {
        self.left = panel;
        self.state_cnt.0 += 1;
        self.state_left()
    }

    /// Updates the middle panel and returns the updates panel-state
    fn update_mid(&mut self, panel: DirPanel) -> PanelState {
        self.mid = panel;
        self.state_cnt.1 += 1;
        self.state_mid()
    }

    /// Updates the right panel and returns the updates panel-state
    fn update_right(&mut self, panel: Panel) -> PanelState {
        self.right = panel;
        self.state_cnt.2 += 1;
        self.state_right()
    }

    pub fn state_left(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.0,
            panel: Select::Left,
        }
    }

    pub fn state_mid(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.1,
            panel: Select::Mid,
        }
    }

    pub fn state_right(&self) -> PanelState {
        PanelState {
            state_cnt: self.state_cnt.2,
            panel: Select::Right,
        }
    }

    pub fn move_cursor(&mut self, movement: Movement) -> PanelAction {
        match movement {
            Movement::Up => self.up(1),
            Movement::Down => self.down(1),
            Movement::Left => self.left(),
            Movement::Right => self.right(),
            Movement::Top => self.up(usize::MAX),
            Movement::Bottom => self.down(usize::MAX),
            Movement::HalfPageForward => self.down(self.ranges.height() as usize / 2),
            Movement::HalfPageBackward => self.up(self.ranges.height() as usize / 2),
            Movement::PageForward => self.down(self.ranges.height() as usize),
            Movement::PageBackward => self.up(self.ranges.height() as usize),
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
    fn up(&mut self, step: usize) -> PanelAction {
        if self.mid.up(step) {
            PanelAction::UpdatePreview(self.mid.selected_path_owned())
        } else {
            PanelAction::None
        }
    }

    fn down(&mut self, step: usize) -> PanelAction {
        if self.mid.down(step) {
            PanelAction::UpdatePreview(self.mid.selected_path_owned())
        } else {
            PanelAction::None
        }
    }

    // TODO: We could improve, that we don't jump into directories where we do not have access
    fn right(&mut self) -> PanelAction {
        if let Some(selected) = self.mid.selected_path() {
            if selected.is_dir() {
                // TODO: Make this dumb again to get rid of some pitfalls
                PanelAction::UpdateAll(selected.to_path_buf())
                // // Remember path
                // self.prev = self.mid.path.clone();

                // // If the selected item is a directory,
                // // all panels will shift to the left,
                // // and the right panel needs to be recreated:

                // // We do this by swapping:
                // // | l | m | r |  will become | m | r | l |
                // // swap left and mid:
                // // | m | l | r |
                // mem::swap(&mut self.left, &mut self.mid);
                // if let Panel::Dir(panel) = &mut self.right {
                //     mem::swap(&mut self.mid, panel);
                // } else {
                //     // This should not be possible!
                //     panic!(
                //         "selected item cannot be a directory while right panel is not a dir-panel"
                //     );
                // }
                // // Recreate right panel
                // PanelAction::UpdatePreview(self.mid.selected_path_owned())
            } else {
                PanelAction::Open(selected.to_path_buf())
            }
        } else {
            PanelAction::None
        }
    }

    fn left(&mut self) -> PanelAction {
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
        self.right = Panel::Dir(self.mid.clone());
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
            self.show_hidden,
            self.ranges.left_x_range.clone(),
            self.ranges.y_range.clone(),
        )?;
        self.mid.draw(
            stdout,
            self.show_hidden,
            self.ranges.mid_x_range.clone(),
            self.ranges.y_range.clone(),
        )?;

        match &self.right {
            Panel::Dir(panel) => panel.draw(
                stdout,
                self.show_hidden,
                self.ranges.right_x_range.clone(),
                self.ranges.y_range.clone(),
            )?,
            Panel::Preview(panel) => panel.draw(
                stdout,
                self.ranges.right_x_range.clone(),
                self.ranges.y_range.clone(),
            )?,
            Panel::Empty => (),
        }
        self.stdout.queue(cursor::Hide)?;
        self.stdout.flush()?;
        Ok(())
    }
}

// A DirPanel can also be empty.
// We encode this as the vector being empty,
// because this will return `None` as selected_path when
// we query it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirPanel {
    /// Elements of the directory
    elements: Vec<DirElem>,
    /// Number of non-hidden files
    non_hidden: Vec<usize>,
    /// Selected element
    selected: usize,
    /// Index in the `non_hidden` vector that is our current selection
    non_hidden_idx: usize,
    /// Path of the directory that the panel is based on
    path: PathBuf,
    /// Weather or not the panel is still loading some data
    loading: bool,
    /// Weather or not to show hidden files
    show_hidden: bool,
}

impl DirPanel {
    pub fn new(elements: Vec<DirElem>, path: PathBuf) -> Self {
        let non_hidden = elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| !elem.is_hidden)
            .map(|(idx, _)| idx)
            .collect::<Vec<usize>>();

        let selected = *non_hidden.first().unwrap_or(&0);
        DirPanel {
            elements,
            non_hidden,
            selected,
            non_hidden_idx: 0,
            path,
            loading: false,
            show_hidden: false,
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
    }

    pub fn select(&mut self, selection: &Path) {
        self.selected = self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .find(|(_, elem)| elem.path() == selection)
            .map(|(idx, _)| idx)
            .unwrap_or(self.selected);

        // let mut selected = 0;
        // for elem in self
        //     .elements
        //     .iter()
        //     .filter(|elem| self.show_hidden || !elem.is_hidden)
        // {
        //     if elem.path() == selection {
        //         break;
        //     }
        //     selected += 1;
        // }
        // if selected == self.elements.len() {
        //     selected = self.elements.len().saturating_sub(1);
        // }
        // self.selected = selected;
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    // // TODO: Remove
    // pub fn with_selection(elements: Vec<DirElem>, path: PathBuf, selection: &Path) -> DirPanel {
    //     let mut selected = 0;
    //     for elem in elements.iter() {
    //         if elem.path() == selection {
    //             break;
    //         }
    //         selected += 1;
    //     }
    //     if selected == elements.len() {
    //         selected = elements.len().saturating_sub(1);
    //     }
    //     DirPanel {
    //         elements,
    //         selected,
    //         path,
    //         loading: false,
    //     }
    // }

    pub fn loading(path: PathBuf) -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected: 0,
            non_hidden_idx: 0,
            path,
            loading: true,
            show_hidden: false,
        }
    }

    /// Creates an empty dir-panel.
    ///
    /// Note: The path of this panel is not a valid path!
    pub fn empty() -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected: 0,
            non_hidden_idx: 0,
            path: "path-of-empty-panel".into(),
            loading: false,
            show_hidden: false,
        }
    }

    /// Move the selection "up" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn up(&mut self, step: usize) -> bool {
        if self.show_hidden {
            if self.selected == 0 {
                return false;
            }
            self.selected = self.selected.saturating_sub(step);
        } else {
            if self.non_hidden_idx == 0 {
                return false;
            }
            self.non_hidden_idx = self.selected.saturating_sub(step);
            self.selected = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        true
    }

    /// Move the selection "down" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn down(&mut self, step: usize) -> bool {
        if self.show_hidden {
            // If we are already at the end, do nothing and return
            if self.selected.saturating_add(1) == self.elements.len() {
                return false;
            }
            // If step is too big, just jump to the end
            if self.selected.saturating_add(step) >= self.elements.len() {
                // selected = len(elements) - 1
                self.selected = self.elements.len().saturating_sub(1);
            } else {
                // Otherwise just increase by step
                self.selected = self.selected.saturating_add(step);
            }
        } else {
            // If we are already at the end, do nothing and return
            if self.non_hidden_idx.saturating_add(1) == self.non_hidden.len() {
                return false;
            }
            if self.selected.saturating_add(step) >= self.non_hidden.len() {
                // idx = len(non_hidden) - 1
                self.non_hidden_idx = self.non_hidden.len().saturating_sub(1);
            } else {
                self.non_hidden_idx = self.non_hidden_idx.saturating_add(step);
            }
            self.selected = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        true
    }

    /// Returns the selcted path of the panel.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|elem| elem.path())
    }

    /// Returns the selcted path of the panel as an owned `PathBuf`.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path_owned(&self) -> Option<PathBuf> {
        self.selected_path().map(|p| p.to_path_buf())
    }

    // /// Returns a reference to the path of the panel.
    // pub fn path(&self) -> &Path {
    //     self.path.as_path()
    // }

    /// Returns a reference to the selected [`DirElem`].
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected(&self) -> Option<&DirElem> {
        self.elements.get(self.selected)
    }

    /// Draws the panel in its current state.
    pub fn draw(
        &self,
        stdout: &mut Stdout,
        show_hidden: bool,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        // Calculate page-scroll
        let scroll: usize = if self.elements.len() > height as usize {
            // if selected should be in the middle all the time:
            // bot = min(max-items, selected + height / 2)
            // scroll = min(0, bot - (height + 1))
            //
            let bot = self.elements.len().min(self.selected + height as usize / 2);
            bot.saturating_sub(height as usize)
        } else {
            0
        };

        // Then print new buffer
        let mut y_offset = 0 as u16;
        // Write "height" items to the screen
        for (idx, entry) in self
            .elements
            .iter()
            .enumerate()
            .skip(scroll)
            .filter(|(_, elem)| show_hidden || !elem.is_hidden)
            .take(height as usize)
        {
            let y = u16::try_from(y_range.start + y_offset).unwrap_or_else(|_| u16::MAX);
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("|".dark_green().bold()),
                entry.print_styled(self.selected == (idx + scroll), width),
            )?;
            y_offset += 1;
        }

        for y in (y_range.start + y_offset)..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("|".dark_green().bold()),
            )?;
            for x in x_range.start + 1..x_range.end {
                queue!(stdout, cursor::MoveTo(x, y), Print(" "),)?;
            }
        }

        // Check if we are loading or not
        if self.loading {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start + 2, y_range.start + 1),
                PrintStyledContent("Loading...".dark_green().bold().italic()),
                cursor::MoveTo(x_range.start + 2, y_range.start + 2),
                PrintStyledContent(format!("{}", self.path.display()).dark_green().italic()),
            )?;
        }

        Ok(())
    }
}
