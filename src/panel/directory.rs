use std::{
    fmt::Write,
    fs::read_dir,
    os::unix::prelude::MetadataExt,
    slice::{Iter, IterMut},
    time::SystemTime,
};

use crossterm::{
    style::{Attribute, Color, SetAttribute, SetForegroundColor},
    Command,
};
use unicode_display_width::width as unicode_width;
use unix_mode::is_allowed;

use crate::{
    config::color::{color_highlight, color_main, color_marked, color_rename, print_vertical_bar},
    content::{dir_content, DirContent},
    engine::StyleEngine,
    util::{file_size_str, ExactWidth},
};

use super::*;

/// A styled directory entry that renders the symbol with one color
/// and the filename with another color.
pub struct StyledEntry {
    /// Leading character (space or 'x' for marked)
    lead: char,
    /// The file type symbol (icon)
    symbol: String,
    /// Color for the symbol
    symbol_color: Color,
    /// The filename
    name: String,
    /// The suffix (file size or dir count)
    suffix: String,
    /// Color for the filename and suffix
    text_color: Color,
    /// Whether to apply bold
    bold: bool,
    /// Whether to apply negative (inverse) style
    negative: bool,
    /// Whether the selection cursor should be muted (inactive split panel).
    /// Only meaningful together with `negative`.
    dimmed: bool,
    /// Optional live-search highlight as `(char_start, char_len)` within
    /// `name`. When set (and non-empty) the matched substring is drawn bold in
    /// the highlight color *inline*, so it stays aligned regardless of how wide
    /// the terminal renders the file icon. Only honored on the normal
    /// (non-`negative`) render path — search rows are never the selection.
    highlight: Option<(usize, usize)>,
}

impl StyledEntry {
    #[allow(clippy::too_many_arguments)]
    fn new(
        lead: char,
        symbol: String,
        symbol_color: Color,
        name: String,
        suffix: String,
        text_color: Color,
        bold: bool,
        negative: bool,
        dimmed: bool,
        highlight: Option<(usize, usize)>,
    ) -> Self {
        Self {
            lead,
            symbol,
            symbol_color,
            name,
            suffix,
            text_color,
            bold,
            negative,
            dimmed,
            highlight,
        }
    }

    /// Converts the `(char_start, char_len)` [`Self::highlight`] into a byte
    /// range within `name`, clamped to the name's bounds. Returns `None` when
    /// there is no highlight, it is empty, or it starts past the end of the
    /// (possibly truncated) name.
    fn highlight_byte_range(&self) -> Option<(usize, usize)> {
        let (char_start, char_len) = self.highlight?;
        if char_len == 0 {
            return None;
        }
        let total = self.name.chars().count();
        if char_start >= total {
            return None;
        }
        let char_end = char_start.saturating_add(char_len).min(total);
        let byte_start = self
            .name
            .char_indices()
            .nth(char_start)
            .map(|(b, _)| b)
            .unwrap_or(self.name.len());
        let byte_end = self
            .name
            .char_indices()
            .nth(char_end)
            .map(|(b, _)| b)
            .unwrap_or(self.name.len());
        Some((byte_start, byte_end))
    }
}

impl Command for StyledEntry {
    fn write_ansi(&self, f: &mut impl Write) -> std::fmt::Result {
        // If negative (selected), use inverse video for the whole line
        if self.negative {
            // Set foreground color first, then reverse - this ensures proper
            // inversion. On an inactive (dimmed) panel we mute the cursor by
            // reversing a dark-grey background instead of the bright text color,
            // so the focused panel's cursor stands out.
            let cursor_color = if self.dimmed {
                Color::DarkGrey
            } else {
                self.text_color
            };
            SetForegroundColor(cursor_color).write_ansi(f)?;
            SetAttribute(Attribute::Reverse).write_ansi(f)?;
            if self.bold && !self.dimmed {
                SetAttribute(Attribute::Bold).write_ansi(f)?;
            }
            // Format: lead + symbol + space + name + space + suffix + space
            write!(
                f,
                "{}{}{} {} ",
                self.lead, self.symbol, self.name, self.suffix
            )?;
            SetAttribute(Attribute::Reset).write_ansi(f)?;
        } else {
            // Normal rendering: symbol color, then text color
            if self.bold {
                SetAttribute(Attribute::Bold).write_ansi(f)?;
            }

            // Lead character with text color
            SetForegroundColor(self.text_color).write_ansi(f)?;
            write!(f, "{}", self.lead)?;

            // Symbol with symbol color
            SetForegroundColor(self.symbol_color).write_ansi(f)?;
            write!(f, "{}", self.symbol)?;

            // Name (with optional inline search highlight) and suffix.
            SetForegroundColor(self.text_color).write_ansi(f)?;
            match self.highlight_byte_range() {
                Some((start, end)) => {
                    // Draw the name in three inline slices so the highlight
                    // flows with the same cursor advancement as everything else
                    // and cannot land on the wrong cell (the icon-width bug).
                    write!(f, "{}", &self.name[..start])?;
                    SetForegroundColor(color_highlight()).write_ansi(f)?;
                    SetAttribute(Attribute::Bold).write_ansi(f)?;
                    write!(f, "{}", &self.name[start..end])?;
                    // Restore the normal name style for the remainder.
                    SetForegroundColor(self.text_color).write_ansi(f)?;
                    if !self.bold {
                        SetAttribute(Attribute::NormalIntensity).write_ansi(f)?;
                    }
                    write!(f, "{}", &self.name[end..])?;
                }
                None => write!(f, "{}", self.name)?,
            }
            write!(f, " {} ", self.suffix)?;

            SetAttribute(Attribute::Reset).write_ansi(f)?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        // Fallback to ANSI on Windows
        Ok(())
    }
}
/// An element of a directory.
///
/// Shorthand for saving a path together whith what we want to display.
/// E.g. a file with path `/home/user/something.txt` should only be
/// displayed as `something.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirElem {
    /// Name of the element.
    name: String,

    /// Lowercase name of the element.
    ///
    /// Is saved to save some computation time (and instead increase memory usage).
    lowercase: String,

    /// Full (canonicalized) path of the element
    path: PathBuf,

    /// Weather or not the file is an executable
    is_executable: bool,

    /// String to display either file-size or number of elements in directory
    suffix: String,

    /// True if element is a hidden file or directory.
    is_hidden: bool,

    /// True if the element is marked.
    ///
    /// Users can mark a selected item to perform operations on them.
    is_marked: bool,

    /// Weather or not we have calculated all values for that panel
    is_normalized: bool,
}

impl DirElem {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn name_lowercase(&self) -> &String {
        &self.lowercase
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_hidden(&self) -> bool {
        self.is_hidden
    }

    pub fn is_marked(&self) -> bool {
        self.is_marked
    }

    pub fn unmark(&mut self) {
        self.is_marked = false;
    }

    /// Creates a [`StyledEntry`] from the `DirElem` itself.
    ///
    /// The symbol is colored based on mime-type, while the filename uses a neutral color.
    /// If the element has not been normalized yet, we do so before we create the styled content.
    pub fn print_styled(&mut self, selected: bool, active: bool, max_len: u16) -> StyledEntry {
        self.print_styled_search(selected, active, max_len, None)
    }

    /// Like [`Self::print_styled`] but, when `search` is `Some(pattern)`
    /// (a lowercase substring known to match), the matched span of the
    /// displayed name is recorded as an inline highlight on the returned
    /// [`StyledEntry`]. `pattern` is matched case-insensitively against the
    /// (already truncated) display name; a match truncated off-screen simply
    /// yields no highlight.
    pub fn print_styled_search(
        &mut self,
        selected: bool,
        active: bool,
        max_len: u16,
        search: Option<&str>,
    ) -> StyledEntry {
        // Only print normalized items
        self.normalize();

        // First determine the symbol and colors
        let symbol: String;
        let mut symbol_color: Color;
        let mut text_color: Color;
        let mut bold = false;

        if self.path.is_dir() {
            symbol = "\u{1F4C1}".to_string();
            symbol_color = color_main();
            text_color = color_main();
            bold = true;
        } else if self.is_executable {
            let file_style = StyleEngine::get_style(self.path());
            symbol = file_style.symbol.to_string();
            symbol_color = file_style.color.unwrap_or(Color::Green);
            text_color = Color::Green;
            bold = true;
        } else {
            let file_style = StyleEngine::get_style(self.path());
            symbol = file_style.symbol.to_string();
            // Symbol gets the mime-type color
            symbol_color = file_style.color.unwrap_or(Color::Grey);
            // Text stays grey/neutral
            text_color = Color::Grey;
        }

        // Calculate name length accounting for actual symbol display width
        // Format: lead + symbol + name + space + suffix + space
        // Overhead: 1 (lead) + symbol_width + 2 (spaces) = 3 + symbol_width
        let symbol_width = unicode_width(&symbol) as usize;
        let name_len = usize::from(max_len)
            .saturating_sub(self.suffix.chars().count())
            .saturating_sub(symbol_width)
            .saturating_sub(3);
        let name = self.name.exact_width(name_len);

        let lead = if self.is_marked {
            // Override text color when marked
            text_color = color_marked();
            symbol_color = color_marked();
            'x'
        } else {
            ' '
        };

        // Mute the cursor only on the selected row of an inactive (split) panel.
        let dimmed = selected && !active;

        // Locate the search match within the displayed (truncated) name so the
        // renderer can highlight it inline.
        let highlight = search.and_then(|pattern| {
            let name_lc = name.to_lowercase();
            name_lc.find(pattern).map(|byte_off| {
                let char_start = name_lc[..byte_off].chars().count();
                (char_start, pattern.chars().count())
            })
        });

        StyledEntry::new(
            lead,
            symbol,
            symbol_color,
            name,
            self.suffix.clone(),
            text_color,
            bold,
            selected,
            dimmed,
            highlight,
        )
    }

    /// Normalizes the `DirElem` to make it viewable by the user.
    ///
    /// Normalization means that:
    /// - the path is canonicalized
    /// - the metadata was parsed
    /// - the file-size or directory-size is parsed
    ///
    /// All of these functions are rather expensive,
    /// so if we would do this directly when we parse a really large directory,
    /// it will eat up a lot of time.
    /// To work with the `DirElem` itself however, all of this is not necessary.
    /// It only becomes mandatory, once we want to display it.
    pub fn normalize(&mut self) {
        if self.is_normalized {
            return;
        }
        // Always use an absolute pathhere
        self.path.canonicalize().unwrap_or_default();

        let (mode, size) = self
            .path
            .metadata()
            .map(|m| (m.permissions().mode(), m.size()))
            .unwrap_or_default();

        self.is_executable =
            is_allowed(unix_mode::Accessor::User, unix_mode::Access::Execute, mode)
                | is_allowed(unix_mode::Accessor::Group, unix_mode::Access::Execute, mode)
                | is_allowed(unix_mode::Accessor::Other, unix_mode::Access::Execute, mode);

        self.suffix = if self.path.is_dir() {
            read_dir(&self.path)
                .map(|res| res.into_iter().count().to_string())
                .unwrap_or_else(|_| "?".into())
        } else {
            file_size_str(size)
        };

        self.is_normalized = true;
    }
}

impl<P: AsRef<Path>> From<P> for DirElem {
    fn from(path: P) -> Self {
        let name = path
            .as_ref()
            .file_name()
            .and_then(|p| p.to_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let lowercase = name.to_lowercase();
        let is_hidden = name.starts_with('.') || name.starts_with("__") || name.ends_with(".swp");

        // NOTE: We don't fully create the DirElem here with all of its information,
        // as this would take too much time.
        // We delay this until we call "normalize"
        let suffix = "".into();
        let is_executable = false;
        let path = path.as_ref().to_path_buf();

        DirElem {
            name,
            lowercase,
            path,
            is_hidden,
            suffix,
            is_executable,
            is_marked: false,
            is_normalized: false,
        }
    }
}

impl AsRef<DirElem> for DirElem {
    fn as_ref(&self) -> &DirElem {
        self
    }
}

impl PartialOrd for DirElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.path.is_dir() {
            if other.path.is_dir() {
                self.name()
                    .to_lowercase()
                    .partial_cmp(&other.name().to_lowercase())
            } else {
                Some(Ordering::Less)
            }
        } else if other.path.is_dir() {
            Some(Ordering::Greater)
        } else {
            return self
                .name()
                .to_lowercase()
                .partial_cmp(&other.name().to_lowercase());
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirPanel {
    /// Elements of the directory
    elements: Vec<DirElem>,

    /// Non-hidden elements (saved by their index)
    ///
    /// NOTE: The elements vector *must not change* over the lifetime of the panel.
    /// Otherwise the indizes in this vector would be invalid
    non_hidden: Vec<usize>,

    /// Active search term
    search: Option<String>,

    /// New element - e.g. when creating a new directory
    ///
    /// If boolean is true - the new element is going to be a directory.
    new_element: Option<(String, bool)>,

    /// Rename preview - (new_name, original_index, is_dir)
    ///
    /// When set, the original item at original_index is hidden
    /// and a preview with new_name is shown at the sorted position.
    rename_preview: Option<(String, usize, bool)>,

    /// Selected element
    selected_idx: usize,

    /// Index in the `non_hidden` vector that is our current selection
    non_hidden_idx: usize,

    /// Path of the directory that the panel is based on
    path: PathBuf,

    /// Last modification time.
    modified: SystemTime,

    /// Weather or not the panel is still loading some data
    loading: bool,

    /// Weather or not to show hidden files
    show_hidden: bool,

    /// Whether the directory could not be accessed (permission denied)
    no_access: bool,
}

impl Draw for DirPanel {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        // KNOWN DEBT: the `active` flag lives on the inherent `draw_active`
        // rather than the `Draw` trait, because the trait is shared with the
        // cursor-less modal adapters that have no use for it. This trait `draw`
        // is the `active=true` default for directories; split/single rendering
        // calls `draw_active` explicitly. If a third type ever needs `active`,
        // promote it into `Draw::draw` as a defaulted method instead.
        self.draw_active(stdout, x_range, y_range, true)
    }
}

impl DirPanel {
    /// Like [`Draw::draw`], but `active` controls the selection-cursor
    /// highlight: bright when this is the focused panel, dimmed otherwise
    /// (split view uses this to mute the inactive side).
    pub fn draw_active(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
        active: bool,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start.saturating_add(1));
        let height = y_range.end.saturating_sub(y_range.start);

        // Calculate page-scroll
        let h = (height.saturating_add(1)) as usize / 2;
        let bot = if self.show_hidden {
            self.elements.len().min(self.selected_idx.saturating_add(h))
        } else {
            self.non_hidden
                .len()
                .min(self.non_hidden_idx.saturating_add(h))
        };
        let scroll: usize = {
            // if selected should be in the middle all the time:
            // bot = min(max-items, selected + height / 2)
            // scroll = min(0, bot - (height + 1))
            bot.saturating_sub(height as usize)
        };

        // Then print new buffer
        let mut y_offset = 0_u16;

        if let Some(pattern) = &self.search {
            for entry in self
                .elements
                .iter_mut()
                .filter(|elem| self.show_hidden || !elem.is_hidden)
                .filter(|elem| elem.name_lowercase().contains(pattern))
            {
                let y = y_range.start + y_offset;
                if y > height {
                    break;
                }
                // Highlight the match inline (see `print_styled_search`): the
                // matched substring flows with the same cursor advancement as
                // the rest of the row, so it stays aligned no matter how many
                // cells the terminal spends on the file icon. The previous
                // absolute-positioned overlay assumed a 2-cell icon and shifted
                // one column right on 1-cell terminals, corrupting the name.
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y),
                    print_vertical_bar(),
                    entry.print_styled_search(false, active, width, Some(pattern.as_str())),
                )?;
                y_offset += 1;
            }
            if y_offset == 0 {
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y_range.start),
                    print_vertical_bar(),
                    PrintStyledContent(
                        " (no match)"
                            .exact_width(width.saturating_sub(2) as usize)
                            .with(color_highlight())
                            .italic()
                    ),
                )?;
                y_offset += 1;
            }
        } else if let Some((new_element, is_dir)) = &self.new_element {
            let lowercase_name = new_element.to_lowercase();
            let (partition, symbol) = if *is_dir {
                (
                    self.elements
                        // NOTE: This only works, because everything is sorted by name
                        .partition_point(|elem| {
                            elem.path().is_dir() && (elem.lowercase < lowercase_name)
                        }),
                    "\u{1F4C1}",
                )
            } else {
                (
                    self.elements
                        // NOTE: This only works, because everything is sorted by name
                        .partition_point(|elem| {
                            elem.path().is_dir() || (elem.lowercase < lowercase_name)
                        }),
                    "\u{1F5B9} ",
                )
            };
            log::debug!("new_element: {new_element}, partition-point: {partition}");

            // Write "height" items to the screen
            for (idx, entry) in self
                .elements
                .iter_mut()
                .enumerate()
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .skip(scroll)
                .take(height.saturating_sub(1) as usize)
            {
                if idx == partition && !new_element.is_empty() {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start, y_range.start + y_offset),
                        print_vertical_bar(),
                        PrintStyledContent(format!(" {symbol}").with(color_highlight())),
                        PrintStyledContent(
                            new_element
                                .exact_width(width.saturating_sub(4) as usize)
                                .with(color_highlight())
                        ),
                    )?;
                    y_offset += 1;
                }
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y_range.start + y_offset),
                    print_vertical_bar(),
                    entry.print_styled(self.selected_idx == idx, active, width),
                )?;
                y_offset += 1;
            }
            if y_offset as usize == partition && !new_element.is_empty() {
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y_range.start + y_offset),
                    print_vertical_bar(),
                    PrintStyledContent(format!(" {symbol}").with(color_highlight())),
                    PrintStyledContent(
                        new_element
                            .exact_width(width.saturating_sub(4) as usize)
                            .with(color_highlight())
                    ),
                )?;
                y_offset += 1;
            }
        } else if let Some((new_name, original_idx, is_dir)) = &self.rename_preview {
            // Rename preview: hide original item, show preview at sorted position
            let lowercase_name = new_name.to_lowercase();

            // Get the suffix from the original element (file size or dir count)
            let original_suffix = self
                .elements
                .get(*original_idx)
                .map(|e| {
                    // We need to normalize to get the suffix
                    let mut elem = e.clone();
                    elem.normalize();
                    elem.suffix.clone()
                })
                .unwrap_or_default();

            // Calculate where the renamed item should appear
            // We count how many items come before it (excluding the original)
            let partition = if *is_dir {
                self.elements
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| idx != original_idx)
                    .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                    .filter(|(_, elem)| elem.path().is_dir())
                    .take_while(|(_, elem)| elem.lowercase < lowercase_name)
                    .count()
            } else {
                // For files: count all dirs + files that come before alphabetically
                self.elements
                    .iter()
                    .enumerate()
                    .filter(|(idx, _)| idx != original_idx)
                    .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                    .take_while(|(_, elem)| elem.path().is_dir() || elem.lowercase < lowercase_name)
                    .count()
            };

            let symbol = if *is_dir { "\u{1F4C1}" } else { "\u{1F5B9} " };
            log::debug!(
                "rename_preview: {new_name}, original_idx: {original_idx}, partition: {partition}"
            );

            // Track position in the filtered list (excluding original)
            let mut visible_idx = 0_usize;

            for (idx, entry) in self
                .elements
                .iter_mut()
                .enumerate()
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .skip(scroll)
                .take(height as usize)
            {
                // Skip the original item being renamed
                if idx == *original_idx {
                    continue;
                }

                // Insert preview at the partition point
                if visible_idx == partition && !new_name.is_empty() {
                    // Format: " symbol name suffix " - rendered inverted like selection
                    let suffix_len = original_suffix.chars().count();
                    let name_width = (width as usize)
                        .saturating_sub(4) // lead + symbol overhead
                        .saturating_sub(suffix_len)
                        .saturating_sub(1); // spacing

                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start, y_range.start + y_offset),
                        print_vertical_bar(),
                        PrintStyledContent(
                            format!(
                                " {symbol}{} {} ",
                                new_name.exact_width(name_width),
                                original_suffix
                            )
                            .with(color_rename())
                            .reverse()
                        ),
                    )?;
                    y_offset += 1;

                    if y_offset >= height {
                        break;
                    }
                }

                // Render the normal entry
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y_range.start + y_offset),
                    print_vertical_bar(),
                    entry.print_styled(self.selected_idx == idx, active, width),
                )?;
                y_offset += 1;
                visible_idx += 1;

                if y_offset >= height {
                    break;
                }
            }

            // Handle case where preview should appear at the end
            if visible_idx == partition && !new_name.is_empty() && y_offset < height {
                let suffix_len = original_suffix.chars().count();
                let name_width = (width as usize)
                    .saturating_sub(4)
                    .saturating_sub(suffix_len)
                    .saturating_sub(2);

                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y_range.start + y_offset),
                    print_vertical_bar(),
                    PrintStyledContent(
                        format!(
                            " {symbol}{} {} ",
                            new_name.exact_width(name_width),
                            original_suffix
                        )
                        .with(color_rename())
                        .reverse()
                    ),
                )?;
                y_offset += 1;
            }
        } else {
            // Write "height" items to the screen
            for (idx, entry) in self
                .elements
                .iter_mut()
                .enumerate()
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .skip(scroll)
                .take(height as usize)
            {
                let y = y_range.start + y_offset;
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start, y),
                    print_vertical_bar(),
                    entry.print_styled(self.selected_idx == idx, active, width),
                )?;
                y_offset += 1;
            }
        }

        let tail = (y_range.start + y_offset)..y_range.end;
        for y in tail.clone() {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start, y),
                print_vertical_bar(),
            )?;
        }
        // One full-width run per row, not a space per cell — per-cell clears
        // are ~60x slower over a sixel image preview in xterm (see the
        // FilePreview Text arm in preview.rs).
        super::graphics::blank_cells(stdout, x_range.start + 1..x_range.end, tail)?;

        // Check if we are loading or not
        if self.loading {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start + 2, y_range.start + 1),
                PrintStyledContent("Loading...".with(color_main()).bold().italic()),
                cursor::MoveTo(x_range.start + 2, y_range.start + 2),
                PrintStyledContent(
                    format!("{}", self.path.display())
                        .exact_width(width.saturating_sub(2) as usize)
                        .with(color_main())
                        .italic()
                ),
            )?;
        } else if self.no_access {
            queue!(
                stdout,
                cursor::MoveTo(x_range.start + 1, y_range.start),
                PrintStyledContent("(no access)".red().italic()),
            )?;
        } else if self.elements.is_empty() {
            if let Some((new_element, is_dir)) = &self.new_element {
                if !new_element.is_empty() {
                    let symbol = if *is_dir { "\u{1F4C1}" } else { "\u{1F5B9} " };
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start + 1, y_range.start),
                        PrintStyledContent(format!(" {symbol}").with(color_highlight())),
                        PrintStyledContent(
                            new_element
                                .exact_width(width.saturating_sub(4) as usize)
                                .with(color_highlight())
                        ),
                    )?;
                } else {
                    queue!(
                        stdout,
                        cursor::MoveTo(x_range.start + 1, y_range.start),
                        PrintStyledContent("(empty)".dark_grey().italic()),
                    )?;
                }
            } else {
                queue!(
                    stdout,
                    cursor::MoveTo(x_range.start + 1, y_range.start),
                    PrintStyledContent("(empty)".dark_grey().italic()),
                )?;
            }
        }
        Ok(())
    }
}

impl PanelContent for DirPanel {
    fn path(&self) -> &Path {
        self.path.as_path()
    }

    fn modified(&self) -> SystemTime {
        self.modified
    }

    fn update_content(&mut self, mut content: Self) {
        // Keep "hidden" state
        content.show_hidden = self.show_hidden;
        // If the content is for the same directory
        if content.path == self.path {
            // Set the selection accordingly
            if let Some(path) = self.selected_path() {
                content.select_path(path, Some(self.selected_idx));
            }
        }
        *self = content;
    }
}

impl BasePanel for DirPanel {
    fn empty() -> Self {
        DirPanel::empty()
    }

    fn loading(path: PathBuf) -> Self {
        DirPanel::loading(path)
    }

    fn from_path(path: PathBuf) -> Self {
        let content = dir_content(path.clone());
        DirPanel::new(content, path)
    }
}

impl DirPanel {
    pub fn new(content: DirContent, path: PathBuf) -> Self {
        let (mut elements, no_access) = match content {
            DirContent::Ok(elements) => (elements, false),
            DirContent::NoAccess => (Vec::new(), true),
        };

        // Sort the elements before you use them
        elements.sort_by_cached_key(|a| a.name_lowercase().clone());
        elements.sort_by_cached_key(|a| !a.path().is_dir());
        // Normalize the first elements, so the first drawing is still really quick
        elements.iter_mut().take(128).for_each(|e| e.normalize());

        let non_hidden = elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| !elem.is_hidden)
            .map(|(idx, _)| idx)
            .collect::<Vec<usize>>();

        let selected = *non_hidden.first().unwrap_or(&0);

        let modified = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or_else(SystemTime::now);

        DirPanel {
            elements,
            non_hidden,
            selected_idx: selected,
            non_hidden_idx: 0,
            search: None,
            new_element: None,
            rename_preview: None,
            path,
            modified,
            loading: false,
            show_hidden: false,
            no_access,
        }
    }

    pub fn inject_new_element(&mut self, new_element: String, is_dir: bool) {
        self.new_element = Some((new_element, is_dir));
    }

    pub fn clear_new_element(&mut self) {
        self.new_element = None;
    }

    pub fn inject_rename_preview(&mut self, new_name: String, original_idx: usize) {
        let is_dir = self
            .elements
            .get(original_idx)
            .map(|e| e.path().is_dir())
            .unwrap_or(false);
        self.rename_preview = Some((new_name, original_idx, is_dir));
    }

    pub fn clear_rename_preview(&mut self) {
        self.rename_preview = None;
    }

    pub fn update_search(&mut self, pattern: String) {
        self.search = Some(pattern.to_lowercase());
    }

    /// Mark all items that contain the search pattern and clear the search afterwards.
    pub fn finish_search(&mut self, pattern: &str) {
        let pat = pattern.to_lowercase();
        for elem in self.elements.iter_mut() {
            elem.is_marked = elem.name_lowercase().contains(&pat);
        }
        self.search = None;
    }

    pub fn clear_search(&mut self) {
        self.search = None;
    }

    pub fn elements(&self) -> Iter<DirElem> {
        self.elements.iter()
    }

    pub fn elements_mut(&mut self) -> IterMut<DirElem> {
        self.elements.iter_mut()
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    pub fn mark_selected_item(&mut self) {
        if let Some(elem) = self.elements.get_mut(self.selected_idx) {
            elem.is_marked = !elem.is_marked;
        }
    }

    /// Changes the selection to the given path.
    ///
    /// If the path is not found, the selection remains unchanged.
    pub fn select_path(&mut self, selection: &Path, alt_idx: Option<usize>) {
        // Do nothing if the path is already selected
        if self.selected_path() == Some(selection) {
            return;
        }
        self.selected_idx = match self
            .elements
            .iter()
            .enumerate()
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .find(|(_, elem)| elem.path() == selection)
            .map(|(idx, _)| idx)
        {
            Some(idx) => {
                log::debug!("selecting {}, idx={}", selection.display(), idx);
                idx
            }
            None => {
                // In this case use the alt index, if given
                let new_idx = alt_idx.unwrap_or(self.selected_idx);
                log::debug!(
                    "selection not found {}, using new idx={}, n-elements={}",
                    selection.display(),
                    new_idx,
                    self.elements.len()
                );
                // Clamp the index, just in case
                new_idx.min(self.elements.len().saturating_sub(1))
            }
        };
        if !self.show_hidden {
            self.set_non_hidden_idx();
        }
    }

    /// Selects the next marked item
    pub fn select_next_marked(&mut self) {
        // Search from selected-idx to end
        if let Some(idx) = self
            .elements
            .iter()
            .enumerate()
            .skip(self.selected_idx + 1)
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .find(|(_, elem)| elem.is_marked)
            .map(|(idx, _)| idx)
        {
            self.selected_idx = idx;
        } else {
            // Search again from start
            self.selected_idx = self
                .elements
                .iter()
                .enumerate()
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .find(|(_, elem)| elem.is_marked)
                .map(|(idx, _)| idx)
                .unwrap_or(self.selected_idx);
        }
        if !self.show_hidden {
            self.set_non_hidden_idx();
        }
    }

    /// Selects the next marked item
    pub fn select_prev_marked(&mut self) {
        // Search from selected-idx to end
        if let Some(idx) = self
            .elements
            .iter()
            .enumerate()
            .rev()
            .filter(|(idx, _)| idx < &self.selected_idx)
            .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
            .find(|(_, elem)| elem.is_marked)
            .map(|(idx, _)| idx)
        {
            self.selected_idx = idx;
        } else {
            // Search again from end
            self.selected_idx = self
                .elements
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, elem)| self.show_hidden || !elem.is_hidden)
                .find(|(_, elem)| elem.is_marked)
                .map(|(idx, _)| idx)
                .unwrap_or(self.selected_idx);
        }
        if !self.show_hidden {
            self.set_non_hidden_idx();
        }
    }

    /// Sets non-hidden-idx to the value closest to selection
    fn set_non_hidden_idx(&mut self) {
        for (idx, elem_idx) in self.non_hidden.iter().enumerate() {
            self.non_hidden_idx = idx;
            if *elem_idx >= self.selected_idx {
                break;
            }
        }
    }

    pub fn set_hidden(&mut self, show_hidden: bool) {
        if self.show_hidden == show_hidden {
            // Nothing to do
            return;
        }
        if self.show_hidden && !show_hidden {
            // Currently we show hidden files, but we should stop that
            // -> non-hidden-idx needs to be updated to the value closest to selection
            self.set_non_hidden_idx();
            // Update selection accordingly for the next time we toggle hidden files
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        // Save value and change selection accordingly
        self.show_hidden = show_hidden;
    }

    pub fn loading(path: PathBuf) -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected_idx: 0,
            non_hidden_idx: 0,
            search: None,
            new_element: None,
            rename_preview: None,
            path,
            modified: SystemTime::now(),
            loading: true,
            show_hidden: false,
            no_access: false,
        }
    }

    /// Creates an empty dir-panel.
    ///
    /// Note: The path of this panel is not a valid path!
    pub fn empty() -> Self {
        DirPanel {
            elements: Vec::new(),
            non_hidden: Vec::new(),
            selected_idx: 0,
            non_hidden_idx: 0,
            search: None,
            new_element: None,
            rename_preview: None,
            modified: SystemTime::now(),
            path: "path-of-empty-panel".into(),
            loading: false,
            show_hidden: false,
            no_access: false,
        }
    }

    /// Move the selection "up" if possible.
    ///
    /// Returns true if the panel has changed and
    /// requires a redraw.
    pub fn up(&mut self, step: usize) -> bool {
        if self.show_hidden {
            if self.selected_idx == 0 {
                return false;
            }
            self.selected_idx = self.selected_idx.saturating_sub(step);
        } else {
            if self.non_hidden_idx == 0 {
                return false;
            }
            self.non_hidden_idx = self.non_hidden_idx.saturating_sub(step);
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
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
            if self.selected_idx.saturating_add(1) == self.elements.len() {
                return false;
            }
            // If step is too big, just jump to the end
            if self.selected_idx.saturating_add(step) >= self.elements.len() {
                // selected = len(elements) - 1
                self.selected_idx = self.elements.len().saturating_sub(1);
            } else {
                // Otherwise just increase by step
                self.selected_idx = self.selected_idx.saturating_add(step);
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
            self.selected_idx = *self.non_hidden.get(self.non_hidden_idx).unwrap_or(&0);
        }
        true
    }

    /// Returns the selected path of the panel.
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected_path(&self) -> Option<&Path> {
        self.selected().map(|elem| elem.path())
    }

    /// Returns the index of the selected item
    pub fn selected_idx(&self) -> usize {
        self.selected_idx
    }

    /// Returns either the selected-idx or non-hidden-idx,
    /// depending on weather or not we display hidden files.
    pub fn index(&self) -> usize {
        if self.show_hidden {
            self.selected_idx
        } else {
            self.non_hidden_idx
        }
    }

    /// Returns a reference to the selected [`DirElem`].
    ///
    /// If the panel is empty `None` is returned.
    pub fn selected(&self) -> Option<&DirElem> {
        self.elements.get(self.selected_idx)
    }

    /// Returns the selected index (starting at 1) and the total number of items.
    pub fn index_vs_total(&self) -> (usize, usize) {
        if self.show_hidden {
            (self.selected_idx.saturating_add(1), self.elements.len())
        } else {
            (self.non_hidden_idx.saturating_add(1), self.non_hidden.len())
        }
    }
}

#[cfg(test)]
mod styled_entry_tests {
    use super::*;
    use crossterm::style::Color;

    /// The highlight render path calls `color_highlight()`, which reads a
    /// global set once at startup. Seed it for the test (ignored if another
    /// test already set it).
    fn ensure_highlight_color() {
        let _ = crate::config::color::COLOR_HIGHLIGHT.set(Color::Red);
    }

    /// Remove every `ESC [ ... m` SGR sequence, leaving only visible glyphs.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for d in chars.by_ref() {
                    if d == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    fn entry(highlight: Option<(usize, usize)>) -> StyledEntry {
        StyledEntry::new(
            ' ',
            "\u{1F5B9}".to_string(), // 🖹 file icon
            Color::Grey,
            "report-2.txt".to_string(),
            "0 B".to_string(),
            Color::Grey,
            false,
            false, // not selected → normal (non-negative) render path
            false,
            highlight,
        )
    }

    /// The live-search highlight must recolor the matched substring WITHOUT
    /// changing which characters are visible. The old overlay drew the pattern
    /// at a hard-coded column and shifted it one cell right when the file icon
    /// rendered as a single cell, corrupting the name (report-2.txt →
    /// rreport2.txt). Regression guard for test-protocol step 05.2.
    #[test]
    fn search_highlight_preserves_visible_characters() {
        ensure_highlight_color();
        let mut plain = String::new();
        entry(None).write_ansi(&mut plain).unwrap();
        let mut highlighted = String::new();
        entry(Some((0, 6))).write_ansi(&mut highlighted).unwrap();

        assert_eq!(
            strip_ansi(&plain),
            strip_ansi(&highlighted),
            "highlighting must not alter the visible characters"
        );
        assert_ne!(
            plain, highlighted,
            "highlighting must actually apply styling to the match"
        );
    }

    /// A highlight range that runs past the (truncated) name must not panic or
    /// corrupt output — it is clamped to the name's char length.
    #[test]
    fn search_highlight_out_of_range_is_clamped() {
        ensure_highlight_color();
        let mut out = String::new();
        StyledEntry::new(
            ' ',
            "\u{1F5B9}".to_string(),
            Color::Grey,
            "ab".to_string(),
            "0 B".to_string(),
            Color::Grey,
            false,
            false,
            false,
            Some((1, 50)),
        )
        .write_ansi(&mut out)
        .unwrap();
        assert!(strip_ansi(&out).contains("ab"), "visible name intact: {out:?}");
    }
}
