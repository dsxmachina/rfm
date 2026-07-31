use serde::Deserialize;

use crate::command_queue::CommandsConfig;
use crate::engine::styles::StyleConfig;

#[derive(Deserialize, Debug)]
pub struct Config {
    pub colors: color::ColorConfig,
    pub general: GeneralConfig,
    #[serde(default)]
    pub styles: StyleConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
}

fn default_rate_limit_interval() -> u64 {
    500
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Debug)]
pub struct GeneralConfig {
    /// Move deleted files to the freedesktop trash (undoable) instead of
    /// deleting permanently. Defaults to `true`.
    #[serde(default = "default_true")]
    pub use_trash: bool,
    /// Persist image/video preview rasters in $XDG_CACHE_HOME/rfm so they
    /// survive restarts. Defaults to `true`.
    #[serde(default = "default_true")]
    pub preview_cache: bool,
    /// Render PDF page 1 to an image preview via pdftoppm/mutool when one
    /// of them is installed. Defaults to `false` (opt-in) so the base
    /// install stays pure-Rust — PDFs then get the native text tier.
    #[serde(default)]
    pub pdf_render: bool,
    /// Rate limit interval for preview updates in milliseconds
    #[serde(default = "default_rate_limit_interval")]
    pub rate_limit_interval_ms: u64,
    /// Use Nerd Font icons (requires a Nerd Font in your terminal)
    #[serde(default)]
    pub fancy_icons: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_cache_defaults_true_and_parses_false() {
        let g: GeneralConfig = toml::from_str("").unwrap();
        assert!(g.preview_cache);
        let g: GeneralConfig = toml::from_str("preview_cache = false").unwrap();
        assert!(!g.preview_cache);
    }

    #[test]
    fn pdf_render_defaults_false_and_parses_true() {
        // NOTE the opposite default of preview_cache: the external pdf
        // image tier is opt-in so the base install stays pure-Rust.
        let g: GeneralConfig = toml::from_str("").unwrap();
        assert!(!g.pdf_render);
        let g: GeneralConfig = toml::from_str("pdf_render = true").unwrap();
        assert!(g.pdf_render);
    }
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
    pub static COLOR_RENAME: OnceCell<Color> = OnceCell::new();

    #[derive(Deserialize, Debug)]
    pub struct ColorConfig {
        main: String,
        marked: String,
        highlight: String,
        dir_path: String,
        #[serde(default)]
        rename: Option<String>,
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
        let rename = config
            .rename
            .map(extract_color)
            .transpose()
            .context("Failed to set 'rename' color")?
            .unwrap_or(Color::Blue);
        COLOR_MAIN.set(main).expect("color must be unset");
        COLOR_MAIN.get_or_init(|| main);
        COLOR_MARKED.set(marked).expect("color must be unset");
        COLOR_HIGHLIGHT.set(highlight).expect("color must be unset");
        COLOR_DIR_PATH.set(dir_path).expect("color must be unset");
        COLOR_RENAME.set(rename).expect("color must be unset");
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
        COLOR_RENAME.set(Color::Blue).expect("color must be unset");
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

    #[inline]
    pub fn color_rename() -> Color {
        *COLOR_RENAME.get().expect("color must be set")
    }
}
