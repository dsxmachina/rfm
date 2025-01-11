use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub colors: color::ColorConfig,
    pub general: GeneralConfig,
}

#[derive(Deserialize, Debug)]
pub struct GeneralConfig {
    pub use_trash: bool,
}

pub mod color {
    use anyhow::{anyhow, Context, Result};
    use crossterm::style::{Color, PrintStyledContent, Stylize};
    use once_cell::sync::OnceCell;
    use serde::Deserialize;

    pub static COLOR_MAIN: OnceCell<Color> = OnceCell::new();
    pub static COLOR_MARKED: OnceCell<Color> = OnceCell::new();
    pub static COLOR_HIGHLIGHT: OnceCell<Color> = OnceCell::new();
    pub static COLOR_DIR_PATH: OnceCell<Color> = OnceCell::new();

    #[derive(Deserialize, Debug)]
    pub struct ColorConfig {
        main: String,
        marked: String,
        highlight: String,
        dir_path: String,
    }

    fn extract_color(string: String) -> Result<Color> {
        let converted = string.to_ascii_lowercase().replace('-', "_");
        let color = converted
            .as_str()
            .try_into()
            .map_err(|_| anyhow!("'{}' is not a valid ANSI color", string))?;
        Ok(color)
    }

    pub fn colors_from_config(config: ColorConfig) -> Result<()> {
        let main = extract_color(config.main).context("Failed to set 'main' color")?;
        let marked = extract_color(config.marked).context("Failed to set 'marked' color")?;
        let highlight =
            extract_color(config.highlight).context("Failed to set 'highlight' color")?;
        let dir_path = extract_color(config.dir_path).context("Failed to set 'dir_path' color")?;
        COLOR_MAIN.set(main).expect("color must be unset");
        COLOR_MAIN.get_or_init(|| main);
        COLOR_MARKED.set(marked).expect("color must be unset");
        COLOR_HIGHLIGHT.set(highlight).expect("color must be unset");
        COLOR_DIR_PATH.set(dir_path).expect("color must be unset");
        Ok(())
    }

    pub fn colors_from_default() {
        COLOR_MAIN
            .set(Color::DarkGreen)
            .expect("color must be unset");
        COLOR_MARKED
            .set(Color::DarkYellow)
            .expect("color must be unset");
        COLOR_HIGHLIGHT
            .set(Color::Red)
            .expect("color must be unset");
        COLOR_DIR_PATH
            .set(Color::DarkBlue)
            .expect("color must be unset");
    }

    #[inline]
    pub fn print_vertical_bar() -> PrintStyledContent<&'static str> {
        PrintStyledContent("│".with(color_main()).bold())
    }

    #[inline]
    pub fn print_horizontal_bar() -> PrintStyledContent<&'static str> {
        // NOTE: This is a utf-8 character - it may be a good idea to query utf-8 support somewhere ?
        PrintStyledContent("─".with(color_main()).bold())
    }

    #[inline]
    pub fn print_horz_top() -> PrintStyledContent<&'static str> {
        PrintStyledContent("┴".with(color_main()).bold())
    }

    #[inline]
    pub fn print_horz_bot() -> PrintStyledContent<&'static str> {
        PrintStyledContent("┬".with(color_main()).bold())
    }

    #[inline]
    pub fn color_main() -> Color {
        *COLOR_MAIN.get().expect("color must be set")
    }

    #[inline]
    pub fn color_marked() -> Color {
        *COLOR_MARKED.get().expect("color must be set")
    }

    #[inline]
    pub fn color_highlight() -> Color {
        *COLOR_HIGHLIGHT.get().expect("color must be set")
    }

    #[inline]
    pub fn color_dir_path() -> Color {
        *COLOR_DIR_PATH.get().expect("color must be set")
    }
}
