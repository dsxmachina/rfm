use crossterm::{
    cursor, queue,
    style::{PrintStyledContent, Stylize},
    Result,
};
use std::{
    cmp::Ordering,
    fs::read_dir,
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
        let mut name = format!(" {}", self.name);
        name.truncate(usize::from(max_len));
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

pub enum PanelType {
    Dir(DirPanel),
    Preview(PreviewPanel),
}

pub enum Column {
    Left,
    Mid,
    Right,
}

impl Column {
    // x = dividers():
    // from = x.0 * width / x.2
    // to = x.1 * width / x.2
    pub fn dividers(&self) -> (u16, u16, u16) {
        match self {
            Column::Left => (0, 1, 8),
            Column::Mid => (1, 4, 8),
            Column::Right => (4, 8, 8),
        }
    }
}

pub struct Panel {
    // Something like 0.5 - 0.75
    // (defined as fractions of terminal-width)
    start_piece: u16,
    end_piece: u16,
    pieces: u16,
    // Start and end x+y defined over the proportions
    x_range: Range<u16>,
    y_range: Range<u16>,
    // Actual panel
    panel: PanelType,
}

impl Panel {
    pub fn new(terminal_size: (u16, u16), column: Column, panel_type: PanelType) -> Panel {
        let (start_piece, end_piece, pieces) = column.dividers();

        let x_start = start_piece * terminal_size.0 / pieces;
        let x_end = end_piece * terminal_size.0 / pieces;
        let x_range = x_start..x_end;
        let y_range = 1..terminal_size.1; // 1st line is for the header

        Panel {
            start_piece,
            end_piece,
            pieces,
            x_range,
            y_range,
            panel: panel_type,
        }
    }

    pub fn draw(&self, stdout: &mut Stdout) -> Result<()> {
        match self.panel {
            PanelType::Dir(dir_panel) => dir_panel.draw(stdout, self.x_range, self.y_range),
            PanelType::Preview(_) => todo!("drawing preview panels is not yet implemented"),
        }
    }

    pub fn terminal_resize(&mut self, terminal_size: (u16, u16)) {
        let x_start = self.start_piece * terminal_size.0 / self.pieces;
        let x_end = self.end_piece * terminal_size.0 / self.pieces;
        self.x_range = x_start..x_end;
        self.y_range = 1..terminal_size.1;
    }
}

// A DirPanel can also be empty.
// We encode this as the vector being empty,
// which is what we will query everytime
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirPanel {
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

#[test]
fn test_selection() {
    let v: Vec<u8> = Vec::new();
    assert!(v.get(1).is_none());
}

pub struct PreviewPanel {}
