use std::path::Path;

use crate::opener::get_mime_type;
use log::error;
use once_cell::sync::OnceCell;
use patricia_tree::StringPatriciaMap;

pub static SYMBOLS: OnceCell<SymbolEngine> = OnceCell::new();

pub struct SymbolEngine {
    symbols: StringPatriciaMap<&'static str>,
}

impl SymbolEngine {
    pub fn new() -> Self {
        let mut symbols = StringPatriciaMap::new();
        symbols.insert(mime::IMAGE, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_BMP, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_PNG, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_JPEG, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_GIF, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_SVG, "\u{1F5BB}");
        symbols.insert(mime::IMAGE_STAR, "\u{1F5BB}");

        symbols.insert(mime::AUDIO, "\u{266B}");

        symbols.insert(mime::PDF, "\u{202C}");
        symbols.insert(mime::VIDEO, "\u{1F39E}");

        symbols.insert("text/markdown", "\u{1F89B}");
        symbols.insert("text/x-toml", "\u{2699}");

        SymbolEngine { symbols }
    }

    pub fn init() {
        if SYMBOLS.set(SymbolEngine::new()).is_err() {
            error!("Symbol engine was already initialized.");
        }
    }

    pub fn get_symbol<P: AsRef<Path>>(path: P) -> &'static str {
        if let Some(engine) = SYMBOLS.get() {
            let mime_type = get_mime_type(path);
            if let Some(icon) = engine.symbols.get(&mime_type) {
                return icon;
            } else if let Some(icon) = engine.symbols.get(mime_type.type_()) {
                return icon;
            } else {
                return "\u{1F5B9}";
            }
        } else {
            error!("Symbol engine was not initialized.");
        }
        " "
    }
}
