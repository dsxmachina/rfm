use patricia_tree::{PatriciaMap, PatriciaSet};

use super::*;
use crate::content::dir_content;

#[derive(Default)]
pub struct DirConsole {
    input: String,
    path: PathBuf,
    rec_idx: usize,
    rec_total: usize,
    tmp_input: String,
    recommendations: PatriciaSet,
}

impl Draw for DirConsole {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let mut path = format!("{}", self.path.display());
        if path.ends_with('/') {
            path.pop();
        }
        let text = format!("{}/{}", path, self.input);

        let offset = if text.len() < (width / 2).into() {
            width / 4
        } else if text.len() < width.into() {
            ((width as usize - text.len()).saturating_sub(1) / 2) as u16
        } else {
            0
        };

        let rec_offset = offset.saturating_add(text.len() as u16);
        let rec_text = self
            .recommendation()
            .strip_prefix(&self.input)
            .unwrap_or("/")
            .to_string();

        if height >= 3 {
            for x in x_range {
                queue!(
                    stdout,
                    cursor::MoveTo(x, y_center.saturating_sub(1)),
                    PrintStyledContent("―".dark_green().bold()),
                    cursor::MoveTo(x, y_center.saturating_add(1)),
                    PrintStyledContent("―".dark_green().bold()),
                )?;
            }
        }
        let x_text = x_start.saturating_add(offset);
        let x_rec = x_start.saturating_add(rec_offset);
        queue!(
            stdout,
            // Clear line and print main text
            cursor::MoveTo(x_text, y_center),
            Clear(ClearType::CurrentLine),
            Print(text),
            // // Clear line and print main input
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(1)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("input: {}", self.input)),
            // // Clear line and print tmp-input
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(2)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("tmp  : {}", self.tmp_input)),
            // // Clear line and print path
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(3)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("path : {}", self.path.display())),
            // Print recommendation
            cursor::MoveTo(x_rec, y_center),
            PrintStyledContent(rec_text.dark_grey()),
            cursor::MoveTo(x_rec, y_center),
            cursor::Show,
            cursor::SetCursorStyle::DefaultUserShape,
            cursor::EnableBlinking,
        )?;
        Ok(())
    }
}

impl DirConsole {
    pub fn from_panel(panel: &DirPanel) -> Self {
        let path = panel.path().to_path_buf();
        let mut recommendations = PatriciaSet::new();
        for item in panel.elements() {
            if item.path().is_dir() && (panel.show_hidden() || !item.is_hidden()) {
                recommendations.insert(item.name());
            }
        }
        let rec_idx = panel.index();
        let rec_total = recommendations.len();
        DirConsole {
            path,
            recommendations,
            rec_total,
            rec_idx,
            ..Default::default()
        }
    }

    pub fn joined_input(&self) -> PathBuf {
        self.path.join(&self.input)
    }

    // pub fn open<P: AsRef<Path>>(&mut self, path: P) {
    //     self.path = path
    //         .as_ref()
    //         .to_path_buf()
    //         .canonicalize()
    //         .unwrap_or_default();

    //     // Delete existing recommendations
    //     self.change_dir(self.path.clone());
    // }

    fn change_dir(&mut self, path: PathBuf) {
        // remember path
        self.path = path;
        self.recommendations.clear();
        // parse directory and create recommendations
        let content = dir_content(self.path.clone());
        for item in content {
            if item.path().is_dir() && !item.is_hidden() {
                self.recommendations.insert(item.name());
            }
        }
        // clear input and recommendations
        self.clear();
        self.rec_total = self.recommendations.len();
        self.rec_idx = 0;
    }

    fn push_char(&mut self, character: char) {
        if character != '/' {
            self.input.push(character);
            self.tmp_input.push(character);
        }
    }

    fn recommendation(&self) -> String {
        let mut all_keys: Vec<String> = self
            .recommendations
            .iter_prefix(self.tmp_input.as_bytes())
            .flat_map(String::from_utf8)
            .collect();
        all_keys.sort_by_cached_key(|name| name.to_lowercase());
        all_keys
            .into_iter()
            .cycle()
            .nth(self.rec_idx)
            .unwrap_or_default()
    }

    pub fn insert(&mut self, character: char) -> Option<PathBuf> {
        // If we entered "..", we want to go up by one directory
        if self.input == ".." {
            self.clear();
            return self.del().map(|p| p.to_path_buf());
        }
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() && self.input != "." {
            self.change_dir(joined_path.clone());
            self.push_char(character);
            return Some(joined_path);
        }
        self.push_char(character);
        // self.active_rec = self.input.clone();

        self.rec_idx = 0; // reset recommendation index
        self.rec_total = self
            .recommendations
            .iter_prefix(self.input.as_bytes())
            .count();
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() && self.input != "." {
            self.change_dir(joined_path.clone());
            Some(joined_path)
        } else {
            None
        }
    }

    pub fn tab(&mut self) -> Option<PathBuf> {
        self.input = self.recommendation();
        self.rec_idx = self.rec_idx.saturating_add(1);
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() {
            if self.rec_total <= 1 {
                self.change_dir(joined_path.clone());
            }
            Some(joined_path)
        } else {
            None
        }
    }

    pub fn backtab(&mut self) -> Option<PathBuf> {
        self.rec_idx = self.rec_idx.saturating_sub(1);
        self.input = self.recommendation();
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() {
            if self.rec_total <= 1 {
                self.change_dir(joined_path.clone());
            }
            Some(joined_path)
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.tmp_input.clear();
    }

    pub fn up(&mut self) {
        self.input = self.recommendation();
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() && self.rec_total <= 1 {
            self.change_dir(joined_path);
        }
        self.rec_idx = self.rec_idx.saturating_sub(1);
        self.input = self.recommendation();
    }

    pub fn down(&mut self) {
        self.input = self.recommendation();
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() && self.rec_total <= 1 {
            self.change_dir(joined_path);
        }
        self.rec_idx = self.rec_idx.saturating_add(1);
        self.input = self.recommendation();
    }

    pub fn set_to(&mut self, input: String) {
        let mut all_keys: Vec<String> = self
            .recommendations
            .iter()
            .flat_map(String::from_utf8)
            .collect();
        all_keys.sort_by_cached_key(|name| name.to_lowercase());
        self.rec_idx = 0;
        for key in all_keys {
            if key == input {
                break;
            }
            self.rec_idx += 1;
        }
    }

    pub fn del(&mut self) -> Option<&Path> {
        if self.input.is_empty() {
            if let Some(parent) = self.path.parent().map(|p| p.to_path_buf()) {
                self.change_dir(parent);
                Some(self.path.as_path())
            } else {
                None
            }
        } else if self.rec_total == 0 {
            loop {
                self.input.pop();
                self.tmp_input.pop();
                if self
                    .recommendations
                    .iter_prefix(self.tmp_input.as_bytes())
                    .next()
                    .is_some()
                {
                    break;
                }
                if self.tmp_input.is_empty() {
                    break;
                }
            }
            None
        } else {
            self.clear();
            self.change_dir(self.path.clone());
            Some(self.path.as_path())
        }
    }
}

#[derive(Default)]
pub struct SearchConsole {
    input: String,
    rec_idx: usize,
    rec_total: usize,
    tmp_input: String,
    recommendations: PatriciaMap<usize>,
}

impl Draw for SearchConsole {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let text = format!("{}", self.input);

        let offset = if self.input.len() < (width / 2).into() {
            width / 4
        } else if self.input.len() < width.into() {
            ((width as usize - self.input.len()).saturating_sub(1) / 2) as u16
        } else {
            0
        };

        let rec_offset = offset.saturating_add(text.len() as u16);
        let rec_text = self
            .recommendation()
            .strip_prefix(&self.input)
            .unwrap_or("/")
            .to_string();

        if height >= 3 {
            for x in x_range {
                queue!(
                    stdout,
                    cursor::MoveTo(x, y_center.saturating_sub(1)),
                    PrintStyledContent("―".dark_green().bold()),
                    cursor::MoveTo(x, y_center.saturating_add(1)),
                    PrintStyledContent("―".dark_green().bold()),
                )?;
            }
        }
        let x_text = x_start.saturating_add(offset);
        let x_rec = x_start.saturating_add(rec_offset);
        queue!(
            stdout,
            // Clear line and print main text
            cursor::MoveTo(x_text, y_center),
            Clear(ClearType::CurrentLine),
            Print(text),
            // // Clear line and print main input
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(1)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("input: {}", self.input)),
            // // Clear line and print tmp-input
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(2)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("tmp  : {}", self.tmp_input)),
            // // Clear line and print path
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(3)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("path : {}", self.path.display())),
            // Print recommendation
            cursor::MoveTo(x_rec, y_center),
            PrintStyledContent(rec_text.dark_grey()),
            cursor::MoveTo(x_rec, y_center),
            cursor::Show,
            cursor::SetCursorStyle::DefaultUserShape,
            cursor::EnableBlinking,
        )?;
        Ok(())
    }
}

impl SearchConsole {
    pub fn from_panel(panel: &DirPanel) -> Self {
        let mut recommendations = PatriciaMap::new();
        for (idx, item) in panel.elements().enumerate() {
            if item.path().is_dir() && (panel.show_hidden() && !item.is_hidden()) {
                recommendations.insert(item.name(), idx);
            }
        }
        let rec_total = recommendations.len();
        SearchConsole {
            recommendations,
            rec_total,
            ..Default::default()
        }
    }

    fn push_char(&mut self, character: char) {
        self.input.push(character);
        self.tmp_input.push(character);
    }

    fn recommendation(&self) -> String {
        let mut all_keys: Vec<String> = self
            .recommendations
            .iter_prefix(self.tmp_input.as_bytes())
            .map(|(item, _)| item)
            .flat_map(String::from_utf8)
            .collect();
        all_keys.sort_by_cached_key(|name| name.to_lowercase());
        all_keys
            .into_iter()
            .cycle()
            .nth(self.rec_idx)
            .unwrap_or_default()
    }

    pub fn insert(&mut self, character: char, _panel: &mut DirPanel) {
        self.push_char(character);
        // self.active_rec = self.input.clone();

        self.rec_idx = 0; // reset recommendation index
        self.rec_total = self
            .recommendations
            .iter_prefix(self.input.as_bytes())
            .count();
    }

    pub fn tab(&mut self) {
        self.input = self.recommendation();
        self.rec_idx = self.rec_idx.saturating_add(1);
    }

    pub fn backtab(&mut self) {
        self.rec_idx = self.rec_idx.saturating_sub(1);
        self.input = self.recommendation();
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.tmp_input.clear();
    }

    pub fn del(&mut self) {
        if self.rec_total == 0 {
            loop {
                self.input.pop();
                self.tmp_input.pop();
                if self
                    .recommendations
                    .iter_prefix(self.tmp_input.as_bytes())
                    .next()
                    .is_some()
                {
                    break;
                }
                if self.tmp_input.is_empty() {
                    break;
                }
            }
        } else {
            self.input.pop();
            self.tmp_input.pop();
        }
    }
}
