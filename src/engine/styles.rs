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

    pub const fn with_color(symbol: &'static str, color: Color) -> Self {
        FileStyle {
            symbol,
            color: Some(color),
        }
    }
}

// =============================================================================
// Standard Unicode Icons (work in any terminal)
// =============================================================================
mod unicode_icons {
    pub const IMAGE: &str = "\u{1F5BB}";      // 🖻 - image
    pub const AUDIO: &str = "\u{266B}";       // ♫ - music note
    pub const VIDEO: &str = "\u{1F39E}";      // 🎞 - film frames
    pub const ARCHIVE: &str = "\u{1F5DC}";    // 🗜 - compression
    pub const PDF: &str = "\u{1F4C4}";        // 📄 - document
    pub const MARKDOWN: &str = "\u{1F4DD}";   // 📝 - memo
    pub const CONFIG: &str = "\u{2699}";      // ⚙ - gear
    pub const DEFAULT: &str = "\u{1F5B9}";    // 🖹 - document
}

// =============================================================================
// Nerd Font Icons (require a patched Nerd Font)
// Icons sourced from yazi file manager
// =============================================================================
mod nerd_icons {
    pub const IMAGE: &str = "\u{E60D}";       //  - seti image
    pub const AUDIO: &str = "\u{F001}";       //  - fa music
    pub const VIDEO: &str = "\u{E69F}";       //  - seti video
    pub const ARCHIVE: &str = "\u{F410}";     //  - oct file_zip
    pub const PDF: &str = "\u{EAEB}";         //  - md file_pdf_box
    pub const MARKDOWN: &str = "\u{E609}";    //  - seti markdown
    pub const TOML: &str = "\u{E6B2}";        //  - seti config
    pub const JSON: &str = "\u{E60B}";        //  - seti json
    pub const YAML: &str = "\u{E615}";        //  - seti yaml
    pub const XML: &str = "\u{F05C0}";        // 󰗀 - md xml
    pub const DEFAULT: &str = "\u{F15B}";     //  - fa file
}

/// Default file style (document icon, no color override)
fn default_style(fancy: bool) -> FileStyle {
    let symbol = if fancy { nerd_icons::DEFAULT } else { unicode_icons::DEFAULT };
    FileStyle::new(symbol, None)
}

pub struct StyleEngine {
    styles: StringPatriciaMap<FileStyle>,
    fancy_icons: bool,
}

impl StyleEngine {
    /// Create a new StyleEngine with the specified icon set.
    ///
    /// If `fancy_icons` is true, Nerd Font icons are used.
    /// Otherwise, standard Unicode icons are used.
    pub fn new(fancy_icons: bool) -> Self {
        let mut styles = StringPatriciaMap::new();

        // Select icon set based on fancy_icons
        let (img_icon, audio_icon, video_icon, archive_icon, pdf_icon, md_icon) = if fancy_icons {
            (
                nerd_icons::IMAGE,
                nerd_icons::AUDIO,
                nerd_icons::VIDEO,
                nerd_icons::ARCHIVE,
                nerd_icons::PDF,
                nerd_icons::MARKDOWN,
            )
        } else {
            (
                unicode_icons::IMAGE,
                unicode_icons::AUDIO,
                unicode_icons::VIDEO,
                unicode_icons::ARCHIVE,
                unicode_icons::PDF,
                unicode_icons::MARKDOWN,
            )
        };

        let (toml_icon, json_icon, yaml_icon, xml_icon) = if fancy_icons {
            (
                nerd_icons::TOML,
                nerd_icons::JSON,
                nerd_icons::YAML,
                nerd_icons::XML,
            )
        } else {
            (
                unicode_icons::CONFIG,
                unicode_icons::CONFIG,
                unicode_icons::CONFIG,
                unicode_icons::CONFIG,
            )
        };

        // Images - Magenta (yazi uses purple/violet RGB 160,116,196)
        let image_style = FileStyle::with_color(img_icon, Color::Magenta);
        styles.insert(mime::IMAGE, image_style);
        styles.insert(mime::IMAGE_BMP, image_style);
        styles.insert(mime::IMAGE_PNG, image_style);
        styles.insert(mime::IMAGE_JPEG, image_style);
        styles.insert(mime::IMAGE_GIF, image_style);
        styles.insert(mime::IMAGE_SVG, image_style);
        styles.insert(mime::IMAGE_STAR, image_style);

        // Audio - Cyan (yazi uses cyan/blue RGB 0,175,255)
        let audio_style = FileStyle::with_color(audio_icon, Color::Cyan);
        styles.insert(mime::AUDIO, audio_style);

        // Video - Yellow (yazi uses orange RGB 253,151,31)
        let video_style = FileStyle::with_color(video_icon, Color::Yellow);
        styles.insert(mime::VIDEO, video_style);

        // Archives - Yellow (yazi uses orange/gold RGB 236,165,23)
        let archive_style = FileStyle::with_color(archive_icon, Color::Yellow);
        styles.insert("application/zip", archive_style);
        styles.insert("application/gzip", archive_style);
        styles.insert("application/x-tar", archive_style);
        styles.insert("application/x-bzip2", archive_style);
        styles.insert("application/x-xz", archive_style);
        styles.insert("application/x-7z-compressed", archive_style);
        styles.insert("application/x-rar-compressed", archive_style);

        // PDF/Documents - Red (yazi uses dark red RGB 179,11,0)
        let pdf_style = FileStyle::with_color(pdf_icon, Color::Red);
        styles.insert(mime::PDF, pdf_style);

        // Markdown - Blue
        let markdown_style = FileStyle::with_color(md_icon, Color::White);
        styles.insert("text/markdown", markdown_style);

        // Config files - specific icons for fancy mode
        let toml_style = FileStyle::with_color(toml_icon, Color::DarkYellow);
        styles.insert("text/x-toml", toml_style);

        let json_style = FileStyle::with_color(json_icon, Color::Yellow);
        styles.insert("application/json", json_style);

        let yaml_style = FileStyle::with_color(yaml_icon, Color::DarkCyan);
        styles.insert("application/x-yaml", yaml_style);
        styles.insert("text/x-yaml", yaml_style);
        styles.insert("text/yaml", yaml_style);

        let xml_style = FileStyle::with_color(xml_icon, Color::Yellow);
        styles.insert("application/xml", xml_style);
        styles.insert("text/xml", xml_style);

        StyleEngine { styles, fancy_icons }
    }

    /// Initialize the global style engine with default styles.
    pub fn init(fancy_icons: bool) {
        if STYLES.set(StyleEngine::new(fancy_icons)).is_err() {
            error!("Style engine was already initialized.");
        }
    }

    /// Initialize the global style engine with user-provided style overrides.
    pub fn init_with_config(config: &StyleConfig, fancy_icons: bool) {
        let mut engine = StyleEngine::new(fancy_icons);
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
                .unwrap_or_else(|| default_style(self.fancy_icons));

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
            default_style(engine.fancy_icons)
        } else {
            error!("Style engine was not initialized.");
            default_style(false)
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
