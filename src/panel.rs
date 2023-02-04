use cached::{cached_result, SizedCache};
use crossterm::{
    cursor, queue,
    style::{self, PrintStyledContent, Stylize},
    Result,
};
use notify_rust::Notification;
use pad::PadStr;
use std::{
    cmp::Ordering,
    fs::{canonicalize, read_dir},
    io::Stdout,
    mem,
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Ord)]
pub struct DirElem {
    name: String,
    path: PathBuf,
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
        let path: PathBuf = path.as_ref().into();
        let name: String = path
            .file_name()
            .map(|p| p.to_str())
            .flatten()
            .map(|s| s.into())
            .unwrap_or_default();
        DirElem { path, name }
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
                return self.name().partial_cmp(other.name());
            } else {
                return Some(Ordering::Less);
            }
        } else {
            if other.path.is_dir() {
                return Some(Ordering::Greater);
            } else {
                return self.name().partial_cmp(other.name());
            }
        }
    }
}

cached_result! {
    DIRECTORY_CONTENT: SizedCache<PathBuf, Vec<DirElem>> = SizedCache::with_size(50);
    fn directory_content(path: PathBuf) -> Result<Vec<DirElem>> = {
        // read directory
        let dir = read_dir(path)?;
        let mut out = Vec::new();
        for item in dir {
            out.push(DirElem::from(item?.path()))
        }
        out.sort();
        Ok(out)
    }
}

struct PreviewPanel {}

enum Panel {
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
                Ok(Panel::Dir(DirPanel::from_path(path)?))
            } else {
                Ok(Panel::Preview(PreviewPanel {}))
            }
        } else {
            Ok(Panel::Empty)
        }
    }
}

// struct Panel {
//     // Something like 0.5 - 0.75
//     // (defined as fractions of terminal-width)
//     start_piece: u16,
//     end_piece: u16,
//     pieces: u16,
//     // Start and end x+y defined over the proportions
//     x_range: Range<u16>,
//     y_range: Range<u16>,
//     // Actual panel
//     panel: PanelType,
// }

// impl Panel {
//     pub fn new(terminal_size: (u16, u16), column: Column, panel_type: PanelType) -> Panel {
//         let (start_piece, end_piece, pieces) = column.dividers();

//         let x_start = start_piece * terminal_size.0 / pieces;
//         let x_end = end_piece * terminal_size.0 / pieces;
//         let x_range = x_start..x_end;
//         let y_range = 1..terminal_size.1; // 1st line is for the header

//         Panel {
//             start_piece,
//             end_piece,
//             pieces,
//             x_range,
//             y_range,
//             panel: panel_type,
//         }
//     }

//     pub fn dir(terminal_size: (u16, u16), column: Column, panel: DirPanel) -> Panel {
//         Panel::new(terminal_size, column, PanelType::Dir(panel))
//     }

//     pub fn empty(terminal_size: (u16, u16), column: Column) -> Panel {
//         Panel::new(terminal_size, column, PanelType::Empty)
//     }

//     pub fn draw(&self, stdout: &mut Stdout) -> Result<()> {
//         match self.panel {
//             PanelType::Dir(dir_panel) => dir_panel.draw(stdout, self.x_range, self.y_range),
//             PanelType::Preview(_) => todo!("drawing preview panels is not yet implemented"),
//             PanelType::Empty => Ok(()),
//         }
//     }

//     pub fn terminal_resize(&mut self, terminal_size: (u16, u16)) {
//         let x_start = self.start_piece * terminal_size.0 / self.pieces;
//         let x_end = self.end_piece * terminal_size.0 / self.pieces;
//         self.x_range = x_start..x_end;
//         self.y_range = 1..terminal_size.1;
//     }
// }

#[derive(Clone)]
struct Ranges {
    left_x_range: Range<u16>,
    mid_x_range: Range<u16>,
    right_x_range: Range<u16>,
    y_range: Range<u16>,
}

impl Ranges {
    pub fn from_size(terminal_size: (u16, u16)) -> Self {
        let (sx, sy) = terminal_size;
        Self {
            left_x_range: 0..(sx / 8),
            mid_x_range: (sx / 8)..(sx / 2),
            right_x_range: (sx / 2)..sx,
            y_range: 1..sy, // 1st line is reserved for the header
        }
    }
}

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
        style::PrintStyledContent(prompt.dark_green().bold()),
        style::Print(" "),
        style::PrintStyledContent(prefix.to_string().dark_blue().bold()),
        style::PrintStyledContent(suffix.to_string().white().bold()),
    )?;
    Ok(())
}

/// Create a set of Panels in "Miller-Columns" style.
pub struct MillerPanels {
    // Panels
    left: DirPanel,
    mid: DirPanel,
    right: Panel,
    // Data
    ranges: Ranges,
}

impl MillerPanels {
    pub fn new(terminal_size: (u16, u16)) -> Result<Self> {
        let left = DirPanel::from_parent(".")?;
        let mid = DirPanel::from_path(".")?;
        let right = Panel::from_path(mid.selected_path())?;
        let ranges = Ranges::from_size(terminal_size);
        Ok(MillerPanels {
            left,
            mid,
            right,
            ranges,
        })
    }

    pub fn terminal_resize(&mut self, terminal_size: (u16, u16)) {
        self.ranges = Ranges::from_size(terminal_size);
    }

    pub fn up(&mut self) -> Result<bool> {
        if self.mid.up() {
            // Change the other panels aswell
            self.right = Panel::from_path(self.mid.selected_path())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn down(&mut self) -> Result<bool> {
        if self.mid.down() {
            // Change the other panels aswell
            self.right = Panel::from_path(self.mid.selected_path())?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn right(&mut self) -> Result<bool> {
        if let Some(selected) = self.mid.selected_path() {
            if selected.is_dir() {
                // If the selected item is a directory,
                // all panels will shift to the left,
                // and the right panel needs to be recreated:

                // We do this by swapping:
                // | l | m | r |  will become | m | r | l |
                // swap left and mid:
                // | m | l | r |
                mem::swap(&mut self.left, &mut self.mid);
                if let Panel::Dir(panel) = &mut self.right {
                    mem::swap(&mut self.mid, panel);
                } else {
                    // This should not be possible!
                    panic!(
                        "selected item cannot be a directory while right panel is not a dir-panel"
                    );
                }
                // Recreate right panel
                self.right = Panel::from_path(self.mid.selected_path())?;
                return Ok(true);
            } else {
                // If the selected item is a file,
                // we need to open it
                // TODO: Implement opening
            }
        }
        Ok(false)
    }

    pub fn left(&mut self) -> Result<bool> {
        // If the left panel is empty, we cannot move left:
        if self.left.selected_path().is_none() {
            return Ok(false);
        }
        // All panels will shift to the right
        // and the left panel needs to be recreated:

        // Create right dir-panel from previous mid
        // | l | m | r |
        self.right = Panel::Dir(self.mid.clone());
        // | l | m | m |

        // swap left and mid:
        mem::swap(&mut self.left, &mut self.mid);
        // | m | l | m |

        // Create left panel from ancestor of selcted path
        if let Some(path) = self.mid.selected_path().map(|p| p.parent()).flatten() {
            self.left = DirPanel::from_parent(path)?;
        } else {
            self.left = DirPanel::empty();
        }
        Ok(true)
    }

    pub fn draw(&self, stdout: &mut Stdout) -> Result<()> {
        if let Some(path) = self.mid.selected_path() {
            print_header(stdout, path)?;
        } else {
            if let Some(path) = self.left.selected_path() {
                print_header(stdout, path)?;
            }
        }
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
            Panel::Dir(panel) => panel.draw(
                stdout,
                self.ranges.right_x_range.clone(),
                self.ranges.y_range.clone(),
            )?,
            Panel::Preview(_) => (), //todo!("drawing preview panels is not yet implemented"),
            Panel::Empty => (),
        }
        Ok(())
    }
}

// A DirPanel can also be empty.
// We encode this as the vector being empty,
// which is what we will query everytime
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirPanel {
    elements: Vec<DirElem>,
    selected: usize,
}

impl DirPanel {
    /// Creates a dir-panel for the given path.
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        // Notification::new()
        //     .summary("FromPath:")
        //     .body(&format!("{}", path.as_ref().display()))
        //     .show()
        //     .unwrap();
        let elements = directory_content(path.as_ref().into())?;
        Ok(DirPanel {
            elements,
            selected: 0,
        })
    }

    /// Creates a dir-panel for the parent of the given path.
    ///
    /// If the path has no parent, and empty dir-panel is returned.
    pub fn from_parent<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = canonicalize(path.as_ref())?;
        if let Some(parent) = path.parent() {
            // Notification::new()
            //     .summary("FromParent:")
            //     .body(&format!(
            //         "path: {}, parent: {}",
            //         path.display(),
            //         parent.display()
            //     ))
            //     .show()
            //     .unwrap();
            let elements = directory_content(parent.into())?;
            let mut selected = 0;
            for elem in elements.iter() {
                if elem.path() == path {
                    break;
                }
                selected += 1;
            }
            Ok(DirPanel { elements, selected })
        } else {
            Ok(Self::empty())
        }
    }

    /// Creates a dir-panel for an empty directory.
    pub fn empty() -> Self {
        // Notification::new()
        //     .summary("Empty:")
        //     .body("")
        //     .show()
        //     .unwrap();
        DirPanel {
            elements: Vec::new(),
            selected: 0,
        }
    }

    /// Move the selection "up" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn up(&mut self) -> bool {
        if self.selected > 0 {
            self.selected -= 1;
            true
        } else {
            false
        }
    }

    /// Move the selection "down" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn down(&mut self) -> bool {
        if self.selected + 1 < self.elements.len() {
            self.selected += 1;
            true
        } else {
            false
        }
    }

    /// Returns the selcted path of the panel.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|elem| elem.path())
    }

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
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        // Then print new buffer
        let mut idx = 0u16;
        // Write "height" items to the screen
        for entry in self.elements.iter().take(height as usize) {
            let y = u16::try_from(y_range.start + idx).unwrap_or_else(|_| u16::MAX);
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("|".dark_green().bold()),
                entry.print_styled(self.selected == idx as usize, width),
            )?;
            idx += 1;
        }
        for y in idx..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("|".dark_green().bold()),
            )?;
        }
        Ok(())
    }
}
