use std::path::Path;

use super::opener::get_mime_type;
use crossterm::style::Color;
use log::error;
use once_cell::sync::OnceCell;
use patricia_tree::StringPatriciaMap;

pub static STYLES: OnceCell<StyleEngine> = OnceCell::new();

/// Style information for a file based on its mime-type.
#[derive(Debug, Clone, Copy)]
pub struct FileStyle {
    pub symbol: &'static str,
    pub color: Option<Color>,
}

impl FileStyle {
    pub const fn new(symbol: &'static str, color: Option<Color>) -> Self {
        FileStyle { symbol, color }
    }

    pub const fn with_symbol(symbol: &'static str) -> Self {
        FileStyle {
            symbol,
            color: None,
        }
    }

    pub const fn with_color(symbol: &'static str, color: Color) -> Self {
        FileStyle {
            symbol,
            color: Some(color),
        }
    }
}

/// Default file style (document icon, no color override)
const DEFAULT_STYLE: FileStyle = FileStyle::new("\u{1F5B9}", None);

pub struct StyleEngine {
    styles: StringPatriciaMap<FileStyle>,
}

impl StyleEngine {
    pub fn new() -> Self {
        let mut styles = StringPatriciaMap::new();

        // Images - Yellow (like yazi)
        let image_style = FileStyle::with_color("\u{1F5BB}", Color::Yellow);
        styles.insert(mime::IMAGE, image_style);
        styles.insert(mime::IMAGE_BMP, image_style);
        styles.insert(mime::IMAGE_PNG, image_style);
        styles.insert(mime::IMAGE_JPEG, image_style);
        styles.insert(mime::IMAGE_GIF, image_style);
        styles.insert(mime::IMAGE_SVG, image_style);
        styles.insert(mime::IMAGE_STAR, image_style);

        // Audio - Magenta (like yazi)
        let audio_style = FileStyle::with_color("\u{266B}", Color::Magenta);
        styles.insert(mime::AUDIO, audio_style);

        // Video - Magenta (yazi groups audio/video together)
        let video_style = FileStyle::with_color("\u{1F39E}", Color::Magenta);
        styles.insert(mime::VIDEO, video_style);

        // Archives - Red (like yazi)
        let archive_style = FileStyle::with_color("\u{1F5DC}", Color::Red);
        styles.insert("application/zip", archive_style);
        styles.insert("application/gzip", archive_style);
        styles.insert("application/x-tar", archive_style);
        styles.insert("application/x-bzip2", archive_style);
        styles.insert("application/x-xz", archive_style);
        styles.insert("application/x-7z-compressed", archive_style);
        styles.insert("application/x-rar-compressed", archive_style);

        // PDF/Documents - Cyan (like yazi)
        let pdf_style = FileStyle::with_color("\u{202C}", Color::Cyan);
        styles.insert(mime::PDF, pdf_style);

        // Markdown - Blue
        let markdown_style = FileStyle::with_color("\u{1F89B}", Color::Blue);
        styles.insert("text/markdown", markdown_style);

        // Config files - DarkCyan
        let config_style = FileStyle::with_color("\u{2699}", Color::DarkCyan);
        styles.insert("text/x-toml", config_style);
        styles.insert("application/json", config_style);
        styles.insert("application/x-yaml", config_style);
        styles.insert("text/yaml", config_style);
        styles.insert("application/xml", config_style);
        styles.insert("text/xml", config_style);

        StyleEngine { styles }
    }

    /// Initialize the global style engine with default styles.
    pub fn init() {
        if STYLES.set(StyleEngine::new()).is_err() {
            error!("Style engine was already initialized.");
        }
    }

    /// Initialize the global style engine with user-provided style overrides.
    pub fn init_with_config(config: &StyleConfig) {
        let mut engine = StyleEngine::new();
        engine.apply_config(config);
        if STYLES.set(engine).is_err() {
            error!("Style engine was already initialized.");
        }
    }

    /// Apply user configuration overrides to the style engine.
    fn apply_config(&mut self, config: &StyleConfig) {
        for (mime_type, style_def) in &config.styles {
            // We need to leak the string to get a 'static lifetime
            // This is fine since the config is only loaded once at startup
            let mime_key: &'static str = Box::leak(mime_type.clone().into_boxed_str());

            // Get the existing style or use default
            let base_style = self
                .styles
                .get(mime_key)
                .copied()
                .unwrap_or(DEFAULT_STYLE);

            // Apply overrides
            let symbol = style_def
                .symbol
                .as_ref()
                .map(|s| -> &'static str { Box::leak(s.clone().into_boxed_str()) })
                .unwrap_or(base_style.symbol);

            let color = style_def
                .color
                .as_ref()
                .and_then(|c| parse_color(c))
                .or(base_style.color);

            self.styles.insert(mime_key, FileStyle::new(symbol, color));
        }
    }

    /// Get the style for a file based on its path.
    ///
    /// First tries an exact mime-type match, then falls back to the type prefix,
    /// and finally returns the default style if no match is found.
    pub fn get_style<P: AsRef<Path>>(path: P) -> FileStyle {
        if let Some(engine) = STYLES.get() {
            let mime_type = get_mime_type(path);
            // Try exact match first
            if let Some(style) = engine.styles.get(&mime_type) {
                return *style;
            }
            // Try type prefix (e.g., "image/" for "image/png")
            if let Some(style) = engine.styles.get(mime_type.type_()) {
                return *style;
            }
            // Return default
            DEFAULT_STYLE
        } else {
            error!("Style engine was not initialized.");
            DEFAULT_STYLE
        }
    }
}

/// Parse a color string into a crossterm Color.
///
/// Supports crossterm named colors like "Red", "DarkGreen", "Cyan", etc.
fn parse_color(s: &str) -> Option<Color> {
    let normalized = s.to_ascii_lowercase().replace(['-', '_'], "");
    match normalized.as_str() {
        "black" => Some(Color::Black),
        "darkgrey" | "darkgray" => Some(Color::DarkGrey),
        "red" => Some(Color::Red),
        "darkred" => Some(Color::DarkRed),
        "green" => Some(Color::Green),
        "darkgreen" => Some(Color::DarkGreen),
        "yellow" => Some(Color::Yellow),
        "darkyellow" => Some(Color::DarkYellow),
        "blue" => Some(Color::Blue),
        "darkblue" => Some(Color::DarkBlue),
        "magenta" => Some(Color::Magenta),
        "darkmagenta" => Some(Color::DarkMagenta),
        "cyan" => Some(Color::Cyan),
        "darkcyan" => Some(Color::DarkCyan),
        "white" => Some(Color::White),
        "grey" | "gray" => Some(Color::Grey),
        _ => {
            log::warn!("Unknown color '{}', ignoring", s);
            None
        }
    }
}

use serde::Deserialize;
use std::collections::HashMap;

/// Configuration for a single mime-type style.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StyleDef {
    pub symbol: Option<String>,
    pub color: Option<String>,
}

/// Configuration for all mime-type styles.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct StyleConfig {
    #[serde(flatten)]
    pub styles: HashMap<String, StyleDef>,
}
