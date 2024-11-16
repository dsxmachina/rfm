use patricia_tree::{PatriciaMap, PatriciaSet};

use super::*;
use crate::{color::print_horizontal_bar, content::dir_content};

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
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let mut path = format!("{}", self.path.display());
        if !path.ends_with('/') {
            path.push('/');
        }
        let path_len = path.chars().count() as u16;

        let text_len = path_len + self.input.chars().count() as u16;
        let offset = if text_len < (width / 2) {
            width / 4
        } else if text_len < width {
            (width - text_len).saturating_sub(1) / 2
        } else {
            0
        };

        let rec_offset = offset.saturating_add(text_len);
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
                    print_horizontal_bar(),
                    cursor::MoveTo(x, y_center.saturating_add(1)),
                    print_horizontal_bar(),
                )?;
            }
        }
        let x_path = x_start.saturating_add(offset);
        let x_text = x_path.saturating_add(path_len);
        let x_rec = x_start.saturating_add(rec_offset);

        queue!(
            stdout,
            // Clear line and print main text
            cursor::MoveTo(x_path, y_center),
            Clear(ClearType::CurrentLine),
            Print(path),
            cursor::MoveTo(x_text, y_center),
            PrintStyledContent(self.input.clone().green()),
            cursor::MoveTo(x_text, y_center),
            Print(self.tmp_input.clone()),
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
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(4)),
            // Clear(ClearType::CurrentLine),
            // Print(&format!("n-rec: {}", self.rec_total)),
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
        // TODO: We have to make a decision, where to insert the new character to.
        //
        // If there is an active recommendation (put to self.input),
        // and self.input + character is a directory -> jump into that directory.
        //
        // However, if self.input + character is also the prefix of another recommendation,
        // then we would like to proceed as normal and go to that recommendation instead.
        //
        // The recommendations should always win before changing a directory
        //

        // Check if self.input + character has at least one recommendation
        let mut input_and_char = self.input.clone();
        input_and_char.push(character);
        let n_possibilities = self
            .recommendations
            .iter_prefix(input_and_char.as_bytes())
            .count();

        // Check if self.path/self.input/ is a directory
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() && self.input != "." {
            // Now we have to make a decision here:
            if n_possibilities == 0 {
                // If there are no open recommendations for that character,
                // we can safely jump into the directory
                self.change_dir(joined_path.clone());
                self.push_char(character);
                return Some(joined_path);
            } else {
                // Only push the character to input
                self.input.push(character);
                self.tmp_input = self.input.clone();
            }
        } else {
            self.push_char(character);
        }
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
        } else if self.tmp_input != self.input {
            // Just return to what the user gave us
            self.input = self.tmp_input.clone();
            Some(self.path.as_path())
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
    tmp_input: String,
    recommendations: PatriciaMap<usize>,
}

impl Draw for SearchConsole {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let text = self.input.clone();
        let text_len = text.chars().count();

        let offset = if text_len < (width / 2).into() {
            width / 4
        } else if text_len < width as usize {
            ((width as usize - text_len).saturating_sub(1) / 2) as u16
        } else {
            0
        };

        let rec_offset = offset.saturating_add(text_len as u16);
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
                    print_horizontal_bar(),
                    cursor::MoveTo(x, y_center.saturating_add(1)),
                    print_horizontal_bar(),
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
}
