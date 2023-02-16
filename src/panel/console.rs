use patricia_tree::PatriciaSet;

use super::*;
use crate::content::dir_content;

#[derive(Default)]
pub struct Console {
    input: String,
    path: PathBuf,
    rec_idx: usize,
    recommendations: PatriciaSet,
}

impl Draw for Console {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let text = format!("{}/{}", self.path.display(), self.input);

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
            .unwrap_or_default()
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
        queue!(
            stdout,
            cursor::MoveTo(x_start + offset, y_center),
            Clear(ClearType::CurrentLine),
            Print(text),
            cursor::MoveTo(x_start + rec_offset, y_center),
            PrintStyledContent(rec_text.dark_grey()),
            cursor::MoveTo(x_start + rec_offset, y_center),
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

        // parse directory
        let content = dir_content(self.path.clone()).unwrap_or_default();
        for item in content {
            self.recommendations.insert(item.name());
        }
        self.input.clear();
        self.rec_idx = 0;
    }

    fn recommendation(&self) -> String {
        self.recommendations
            .iter_prefix(self.input.as_bytes())
            .skip(self.rec_idx)
            .next()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    }

    pub fn insert(&mut self, character: char) {
        self.input.push(character);
        self.rec_idx = 0; // reset recommendation index
    }

    pub fn tab(&mut self) {
        self.rec_idx = self.rec_idx.saturating_add(1);
        if self.rec_idx
            >= self
                .recommendations
                .iter_prefix(self.input.as_bytes())
                .count()
        {
            self.rec_idx = 0;
        }
    }

    pub fn clear(&mut self) {
        self.input.clear();
    }

    pub fn del(&mut self) -> Option<&Path> {
        if self.input.is_empty() {
            self.path.parent()
        } else {
            self.input.pop();
            None
        }
    }
}
