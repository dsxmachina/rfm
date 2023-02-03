use crossterm::{
    cursor, queue,
    style::{self, PrintStyledContent, Stylize},
    Result,
};
use pad::PadStr;
use std::{
    cmp::Ordering,
    fs::{canonicalize, read_dir},
    io::Stdout,
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

fn directory_content<P: AsRef<Path>>(path: P) -> Result<Vec<DirElem>> {
    // read directory
    let dir = read_dir(path)?;
    let mut out = Vec::new();
    for item in dir {
        out.push(DirElem::from(item?.path()))
    }
    out.sort();
    Ok(out)
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
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let elements = directory_content(path)?;
        Ok(DirPanel {
            elements,
            selected: 0,
        })
    }

    pub fn from_parent<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = canonicalize(path.as_ref())?;
        if let Some(parent) = path.parent() {
            let elements = directory_content(parent)?;
            let mut selected = 0;
            for elem in elements.iter() {
                if elem.path() == path {
                    break;
                }
                selected += 1;
            }
            Ok(DirPanel { elements, selected })
        } else {
            todo!()
        }
    }

    // Return true if the panel has changed
    pub fn up(&mut self) -> bool {
        if self.selected > 0 {
            self.selected -= 1;
            true
        } else {
            false
        }
    }

    // Return true if the panel has changed
    pub fn down(&mut self) -> bool {
        if self.selected + 1 < self.elements.len() {
            self.selected += 1;
            true
        } else {
            false
        }
    }

    pub fn replace(&mut self, other: DirPanel) {
        self.elements = other.elements;
        self.selected = other.selected;
    }

    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|elem| elem.path())
    }

    pub fn selected(&self) -> Option<&DirElem> {
        self.elements.get(self.selected)
    }

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

// #[test]
// fn test_selection() {
//     let v: Vec<u8> = Vec::new();
//     assert!(v.get(1).is_none());
// }
