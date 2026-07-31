pub mod app_state;
pub mod load;
pub mod merge;

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
    // deserialize target for per-section [keys.*] error handling; the parser
    // reads the overlay + defaults instead
    #[allow(dead_code)]
    #[serde(default)]
    pub keys: crate::engine::commands::KeyConfig,
    #[serde(default)]
    pub open: crate::engine::opener::OpenerConfig,
}

/// The embedded `examples/` directory — shipped default/example config files.
#[derive(rust_embed::Embed)]
#[folder = "examples/"]
struct Examples;

/// The single source of truth for rfm's defaults: the complete, annotated
/// default configuration file embedded at compile time.
const DEFAULT_CONFIG_FILE: &str = "default-config.toml";

static DEFAULT_CONFIG: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
    let file = Examples::get(DEFAULT_CONFIG_FILE).expect("embedded default-config.toml");
    String::from_utf8(file.data.into_owned()).expect("default-config.toml must be valid UTF-8")
});

/// The embedded default configuration as a string (e.g. for `--dump-config`
/// and first-run file creation).
pub fn default_config_str() -> &'static str {
    &DEFAULT_CONFIG
}

/// The embedded default configuration parsed into a TOML tree.
pub fn default_tree() -> toml::Value {
    default_config_str()
        .parse()
        .expect("embedded default-config.toml must parse as TOML")
}

fn default_rate_limit_interval() -> u64 {
    500
}

fn default_true() -> bool {
    true
}

/// Which terminal graphics protocol the image preview may use.
///
/// `Auto` (the default) resolves at startup: env heuristics first, then a
/// bounded terminal probe; on any uncertainty it falls back to `HalfBlock`,
/// the universal cell-based renderer. Explicit values pin the protocol and
/// skip probing — the escape hatch for terminals that misreport. (They are
/// honored inside tmux too, but rfm emits raw sequences without tmux's
/// passthrough wrapping, so a pinned protocol only renders there if tmux
/// itself supports it.)
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ImageProtocolChoice {
    #[default]
    Auto,
    Kitty,
    Sixel,
    HalfBlock,
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
    /// Graphics protocol for image previews: auto | kitty | sixel |
    /// half-block. Defaults to `auto` (detect at startup, fall back to
    /// half-blocks on any uncertainty).
    #[serde(default)]
    pub image_protocol: ImageProtocolChoice,
    /// Rate limit interval for preview updates in milliseconds
    #[serde(default = "default_rate_limit_interval")]
    pub rate_limit_interval_ms: u64,
    /// Use Nerd Font icons (requires a Nerd Font in your terminal)
    #[serde(default)]
    pub fancy_icons: bool,
}

#[cfg(test)]
mod defaults_tests {
    use super::*;

    #[test]
    fn embedded_defaults_deserialize() {
        let config: Config = default_tree().try_into().unwrap();
        // spot checks
        assert!(config.general.use_trash);
        assert_eq!(config.general.rate_limit_interval_ms, 500);
    }

    /// Completeness guard: every binding field must be present (Some) in the
    /// defaults file — adding a Command without documenting it fails here.
    #[test]
    fn defaults_cover_every_binding_field() {
        let config: Config = default_tree().try_into().unwrap();
        config.keys.assert_complete(); // panics with the field name if None
    }

    #[test]
    fn preview_cache_defaults_true_and_parses_false() {
        let g: GeneralConfig = toml::from_str("").unwrap();
        assert!(g.preview_cache);
        let g: GeneralConfig = toml::from_str("preview_cache = false").unwrap();
        assert!(!g.preview_cache);
    }

    #[test]
    fn image_protocol_defaults_to_auto() {
        let g: GeneralConfig = toml::from_str("").unwrap();
        assert_eq!(g.image_protocol, ImageProtocolChoice::Auto);
    }

    #[test]
    fn image_protocol_parses_all_values() {
        for (raw, want) in [
            ("auto", ImageProtocolChoice::Auto),
            ("kitty", ImageProtocolChoice::Kitty),
            ("sixel", ImageProtocolChoice::Sixel),
            ("half-block", ImageProtocolChoice::HalfBlock),
        ] {
            let g: GeneralConfig = toml::from_str(&format!("image_protocol = \"{raw}\""))
                .unwrap_or_else(|e| panic!("'{raw}' must parse: {e}"));
            assert_eq!(g.image_protocol, want, "for input '{raw}'");
        }
        // serde rename is exact: no aliasing of the kebab-case value
        assert!(toml::from_str::<GeneralConfig>("image_protocol = \"halfblock\"").is_err());
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

    fn default_rename_color() -> String {
        "blue".into()
    }

    #[derive(Deserialize, Debug)]
    pub struct ColorConfig {
        main: String,
        marked: String,
        highlight: String,
        dir_path: String,
        #[serde(default = "default_rename_color")]
        rename: String,
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
        let rename = extract_color(config.rename).context("Failed to set 'rename' color")?;
        COLOR_MAIN.set(main).expect("color must be unset");
        COLOR_MARKED.set(marked).expect("color must be unset");
        COLOR_HIGHLIGHT.set(highlight).expect("color must be unset");
        COLOR_DIR_PATH.set(dir_path).expect("color must be unset");
        COLOR_RENAME.set(rename).expect("color must be unset");
        Ok(())
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
