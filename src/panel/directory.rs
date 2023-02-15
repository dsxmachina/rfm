use super::*;
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
// TODO: Remove "pub"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirPanel {
    /// Elements of the directory
    pub elements: Vec<DirElem>,

    /// Number of non-hidden files
    pub non_hidden: Vec<usize>,

    /// Selected element
    pub selected: usize,

    /// Index in the `non_hidden` vector that is our current selection
    pub non_hidden_idx: usize,

    /// Path of the directory that the panel is based on
    pub path: PathBuf,

    /// Weather or not the panel is still loading some data
    pub loading: bool,

    /// Weather or not to show hidden files
    pub show_hidden: bool,

    /// Hash of the elements
    pub hash: u64,
}

impl Draw for DirPanel {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        // Calculate page-scroll
        let scroll: usize = {
            // if selected should be in the middle all the time:
            // bot = min(max-items, selected + height / 2)
            // scroll = min(0, bot - (height + 1))
            let h = (height.saturating_add(1)) as usize / 2;
            let bot = if self.show_hidden {
                self.elements.len().min(self.selected.saturating_add(h))
            } else {
                self.non_hidden
                    .len()
                    .min(self.non_hidden_idx.saturating_add(h))
                    .saturating_add(1)
            };
            bot.saturating_sub(height as usize)
        };

        // Then print new buffer
        let mut y_offset = 0 as u16;
        // Write "height" items to the screen
        for (idx, entry) in self
            .elements
            .iter()
            .enumerate()
            .skip(scroll)
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .take(height as usize)
        {
            let y = u16::try_from(y_range.start + y_offset).unwrap_or_else(|_| u16::MAX);
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("│".dark_green().bold()),
                entry.print_styled(self.selected == idx, width),
            )?;
            y_offset += 1;
        }

        for y in (y_range.start + y_offset)..y_range.end {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                PrintStyledContent("│".dark_green().bold()),
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
                PrintStyledContent(
                    format!("{}", self.path.display())
                        .with_exact_width(width.saturating_sub(2) as usize)
                        .dark_green()
                        .italic()
                ),
            )?;
        }

        Ok(())
    }
}

impl BasePanel for DirPanel {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn content_hash(&self) -> u64 {
        self.hash
    }

    fn update_content(&mut self, mut content: Self) {
        // Keep "hidden" state
        content.show_hidden = self.show_hidden;
        // If the content is for the same directory
        if content.path == self.path {
            // Set the selection accordingly
            if let Some(path) = self.selected_path() {
                content.select(path);
            }
        }
        *self = content;
    }
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
        let hash = hash_elements(&elements);

        DirPanel {
            elements,
            non_hidden,
            selected,
            non_hidden_idx: 0,
            path,
            loading: false,
            show_hidden: false,
            hash,
        }
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
    }

    pub fn set_hidden(&mut self, show_hidden: bool) {
        if self.show_hidden == show_hidden {
            // Nothing to do
            return;
        }
        if self.show_hidden && !show_hidden {
            // Currently we show hidden files, but we should stop that
            // -> non-hidden-idx needs to be updated to the value closest to selection
            for (idx, elem_idx) in self.non_hidden.iter().enumerate() {
                self.non_hidden_idx = idx;
                if *elem_idx >= self.selected {
                    break;
                }
            }
            self.selected = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        // Save value and change selection accordingly
        self.show_hidden = show_hidden;
    }

    pub fn loading(path: PathBuf) -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected: 0,
            non_hidden_idx: 0,
            path,
            loading: true,
            show_hidden: false,
            hash: 0,
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
            hash: 0,
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
            self.non_hidden_idx = self.non_hidden_idx.saturating_sub(step);
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
            if self.non_hidden_idx.saturating_add(step) >= self.non_hidden.len() {
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

    /// Returns a reference to the selected [`DirElem`].
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected(&self) -> Option<&DirElem> {
        self.elements.get(self.selected)
    }
}
