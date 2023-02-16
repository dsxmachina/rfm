use super::*;

#[derive(Default)]
pub struct Console {
    text: String,
    path: PathBuf,
}

impl Draw for Console {
    fn draw(&self, stdout: &mut Stdout, x_range: Range<u16>, y_range: Range<u16>) -> Result<()> {
        let width = x_range.end.saturating_sub(x_range.start);
        let height = y_range.end.saturating_sub(y_range.start);

        let x_start = x_range.start;
        let y_center = y_range.end.saturating_add(y_range.start) / 2;

        let offset = if self.text.len() < (width / 2).into() {
            width / 4
        } else if self.text.len() < width.into() {
            ((width as usize - self.text.len()).saturating_sub(1) / 2) as u16
        } else {
            0
        };

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
            Print(&self.text),
            cursor::Show,
            cursor::SetCursorStyle::DefaultUserShape,
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
        self.text = format!("{}", self.path.display());
    }

    pub fn insert(&mut self, character: char) {
        self.text.push(character);
    }

    pub fn clear(&mut self) {
        self.text.clear();
    }

    pub fn del(&mut self) {
        self.text.pop();
    }
}
