use notify_rust::Notification;
use patricia_tree::PatriciaSet;

use super::*;
use crate::content::dir_content;

#[derive(Default)]
pub struct Console {
    input: String,
    path: PathBuf,
    rec_idx: usize,
    rec_total: usize,
    tmp_input: String,
    recommendations: PatriciaSet,
}

impl Draw for Console {
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
            .unwrap_or_else(|| "/")
            .to_string();

        // TODO: Make this a box. Or something else.

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
            // cursor::MoveTo(x_start + offset - 7, y_center.saturating_add(1)),
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

impl Console {
    pub fn open<P: AsRef<Path>>(&mut self, path: P) {
        self.path = path
            .as_ref()
            .to_path_buf()
            .canonicalize()
            .unwrap_or_default();

        // Delete existing recommendations
        self.change_dir(self.path.clone());
    }

    fn change_dir(&mut self, path: PathBuf) {
        // remember path
        self.path = path;
        self.recommendations.clear();
        // parse directory and create recommendations
        let content = dir_content(self.path.clone()).unwrap_or_default();
        for item in content {
            if item.path().is_dir() && !item.is_hidden() {
                self.recommendations.insert(item.name());
            }
        }
        // clear input and recommendations
        self.input.clear();
        self.tmp_input.clear();
        self.rec_total = self.recommendations.len();
        self.rec_idx = 0;
    }

    fn recommendation(&self) -> String {
        let mut all_keys: Vec<String> = self
            .recommendations
            .iter_prefix(self.tmp_input.as_bytes())
            .map(|bytes| String::from_utf8(bytes))
            .flatten()
            .collect();
        all_keys.sort_by_cached_key(|name| name.to_lowercase());
        all_keys
            .into_iter()
            .cycle()
            .skip(self.rec_idx)
            .next()
            .unwrap_or_default()
    }

    pub fn insert(&mut self, character: char) -> Option<PathBuf> {
        let joined_path = self.path.join(&self.input);
        if joined_path.is_dir() {
            self.change_dir(joined_path.clone());
            self.input.push(character);
            self.tmp_input.push(character);
            return Some(joined_path);
        }
        if character != '/' {
            self.input.push(character);
            self.tmp_input.push(character);
            // self.active_rec = self.input.clone();

            self.rec_idx = 0; // reset recommendation index
            self.rec_total = self
                .recommendations
                .iter_prefix(self.input.as_bytes())
                .count();
        }

        let joined_path = self.path.join(&self.input);
        // Notification::new()
        //     .summary(&format!("{}", joined_path.display()))
        //     .body(&format!("{}", self.path.display()))
        //     .show()
        //     .unwrap();
        if joined_path.is_dir() {
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
        // Notification::new()
        //     .summary(&format!("{}", joined_path.display()))
        //     .body(&format!("{}", self.path.display()))
        //     .show()
        //     .unwrap();
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
            // self.active_rec.clear();
            if let Some(parent) = self.path.parent().map(|p| p.to_path_buf()) {
                self.change_dir(parent);
                Some(self.path.as_path())
            } else {
                None
            }
        } else {
            if self.rec_total == 0 {
                self.input.pop();
                self.tmp_input.pop();
                None
            } else {
                self.input.clear();
                self.tmp_input.clear();
                self.recommendations.clear();
                Some(self.path.as_path())
            }
        }
    }
}
