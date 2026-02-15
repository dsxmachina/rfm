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
    // Documents
    pub const TEXT: &str = "\u{1F5B9}";        // 🖹 - document
    pub const PDF: &str = "\u{1F4C4}";         // 📄 - document
    pub const MARKDOWN: &str = "\u{1F4DD}";    // 📝 - memo
    pub const WORD: &str = "\u{1F4C4}";        // 📄 - document
    pub const EXCEL: &str = "\u{1F4CA}";       // 📊 - chart
    pub const POWERPOINT: &str = "\u{1F4CA}";  // 📊 - chart

    // Media
    pub const IMAGE: &str = "\u{1F5BB}";       // 🖻 - image
    pub const AUDIO: &str = "\u{266B}";        // ♫ - music note
    pub const VIDEO: &str = "\u{1F39E}";       // 🎞 - film frames

    // Archives
    pub const ARCHIVE: &str = "\u{1F5DC}";     // 🗜 - compression

    // Config
    pub const CONFIG: &str = "\u{2699}";       // ⚙ - gear

    // Code
    pub const CODE: &str = "\u{1F5B9}";        // 🖹 - document (generic for code)

    // Database
    pub const DATABASE: &str = "\u{1F5C3}";    // 🗃 - card file box

    // Font
    pub const FONT: &str = "\u{1F524}";        // 🔤 - input latin letters

    // Executable
    pub const EXECUTABLE: &str = "\u{2699}";   // ⚙ - gear

    // Default
    pub const DEFAULT: &str = "\u{1F5B9}";     // 🖹 - document
}

// =============================================================================
// Nerd Font Icons (require a patched Nerd Font)
// Icons sourced from yazi file manager
// =============================================================================
#[allow(dead_code)]
mod nerd_icons {
    // Documents
    pub const TEXT: &str = "\u{F15C}";         //  - fa file_text
    pub const PDF: &str = "\u{EAEB}";          //  - md file_pdf_box
    pub const MARKDOWN: &str = "\u{E609}";     //  - seti markdown
    pub const WORD: &str = "\u{F1C2}";         //  - fa file_word
    pub const EXCEL: &str = "\u{F1C3}";        //  - fa file_excel
    pub const POWERPOINT: &str = "\u{F1C4}";   //  - fa file_powerpoint
    pub const OPENDOC_TEXT: &str = "\u{F1C2}"; //  - fa file_word (similar)
    pub const OPENDOC_SHEET: &str = "\u{F1C3}";//  - fa file_excel
    pub const OPENDOC_PRES: &str = "\u{F1C4}"; //  - fa file_powerpoint
    pub const RTF: &str = "\u{F15C}";          //  - fa file_text
    pub const LATEX: &str = "\u{F034}";        //  - fa text_height (TeX)
    pub const BIBTEX: &str = "\u{F034}";       //  - fa text_height

    // Images
    pub const IMAGE: &str = "\u{E60D}";        //  - seti image
    pub const JPEG: &str = "\u{E60D}";         //  - seti image
    pub const PNG: &str = "\u{E60D}";          //  - seti image
    pub const GIF: &str = "\u{E60D}";          //  - seti image
    pub const BMP: &str = "\u{E60D}";          //  - seti image
    pub const SVG: &str = "\u{F0721}";         // 󰜡 - md svg
    pub const WEBP: &str = "\u{E60D}";         //  - seti image
    pub const ICO: &str = "\u{E60D}";          //  - seti image
    pub const TIFF: &str = "\u{E60D}";         //  - seti image
    pub const PSD: &str = "\u{E7B8}";          //  - dev photoshop
    pub const XCF: &str = "\u{E60D}";          //  - seti image (GIMP)
    pub const RAW: &str = "\u{E60D}";          //  - seti image
    pub const HEIC: &str = "\u{E60D}";         //  - seti image
    pub const AVIF: &str = "\u{E60D}";         //  - seti image

    // Audio
    pub const AUDIO: &str = "\u{F001}";        //  - fa music
    pub const MP3: &str = "\u{F001}";          //  - fa music
    pub const FLAC: &str = "\u{F001}";         //  - fa music
    pub const WAV: &str = "\u{F001}";          //  - fa music
    pub const OGG: &str = "\u{F001}";          //  - fa music
    pub const AAC: &str = "\u{F001}";          //  - fa music
    pub const M4A: &str = "\u{F001}";          //  - fa music
    pub const OPUS: &str = "\u{F001}";         //  - fa music
    pub const MIDI: &str = "\u{F001}";         //  - fa music
    pub const AIFF: &str = "\u{F001}";         //  - fa music

    // Video
    pub const VIDEO: &str = "\u{E69F}";        //  - seti video
    pub const MP4: &str = "\u{E69F}";          //  - seti video
    pub const MKV: &str = "\u{E69F}";          //  - seti video
    pub const AVI: &str = "\u{E69F}";          //  - seti video
    pub const MOV: &str = "\u{E69F}";          //  - seti video
    pub const WMV: &str = "\u{E69F}";          //  - seti video
    pub const FLV: &str = "\u{E69F}";          //  - seti video
    pub const WEBM: &str = "\u{E69F}";         //  - seti video
    pub const MPEG: &str = "\u{E69F}";         //  - seti video
    pub const THREE_GPP: &str = "\u{E69F}";    //  - seti video

    // Archives
    pub const ARCHIVE: &str = "\u{F410}";      //  - oct file_zip
    pub const ZIP: &str = "\u{F410}";          //  - oct file_zip
    pub const GZIP: &str = "\u{F410}";         //  - oct file_zip
    pub const TAR: &str = "\u{F410}";          //  - oct file_zip
    pub const BZIP2: &str = "\u{F410}";        //  - oct file_zip
    pub const XZ: &str = "\u{F410}";           //  - oct file_zip
    pub const SEVENZ: &str = "\u{F410}";       //  - oct file_zip
    pub const RAR: &str = "\u{F410}";          //  - oct file_zip
    pub const ZSTD: &str = "\u{F410}";         //  - oct file_zip
    pub const LZ4: &str = "\u{F410}";          //  - oct file_zip
    pub const DEB: &str = "\u{E77D}";          //  - dev debian
    pub const RPM: &str = "\u{F316}";          //  - linux redhat
    pub const ISO: &str = "\u{E271}";          //  - disc

    // Programming Languages
    pub const PYTHON: &str = "\u{E606}";       //  - seti python
    pub const JAVA: &str = "\u{E738}";         //  - dev java
    pub const JAVASCRIPT: &str = "\u{E74E}";   //  - dev javascript
    pub const TYPESCRIPT: &str = "\u{E628}";   //  - seti typescript
    pub const CSHARP: &str = "\u{F031B}";      // 󰌛 - md language_csharp
    pub const CPP: &str = "\u{E61D}";          //  - seti cpp
    pub const C: &str = "\u{E61E}";            //  - seti c
    pub const RUST: &str = "\u{E7A8}";         //  - dev rust
    pub const GO: &str = "\u{E627}";           //  - seti go
    pub const RUBY: &str = "\u{E739}";         //  - dev ruby
    pub const PHP: &str = "\u{E608}";          //  - seti php
    pub const SWIFT: &str = "\u{E755}";        //  - dev swift
    pub const KOTLIN: &str = "\u{E634}";       //  - seti kotlin
    pub const SCALA: &str = "\u{E737}";        //  - dev scala
    pub const CLOJURE: &str = "\u{E76A}";      //  - dev clojure
    pub const HASKELL: &str = "\u{E777}";      //  - dev haskell
    pub const ERLANG: &str = "\u{E7B1}";       //  - dev erlang
    pub const ELIXIR: &str = "\u{E62D}";       //  - seti elixir
    pub const LUA: &str = "\u{E620}";          //  - seti lua
    pub const PERL: &str = "\u{E769}";         //  - dev perl
    pub const R: &str = "\u{F25D}";            //  - fa registered (R)
    pub const DART: &str = "\u{E798}";         //  - dev dart
    pub const OCAML: &str = "\u{E67A}";        //  - seti ocaml
    pub const FSHARP: &str = "\u{E7A7}";       //  - dev fsharp
    pub const LISP: &str = "\u{F0172}";        // 󰅲 - md lambda
    pub const FORTRAN: &str = "\u{F121A}";     // 󱈚 - md language_fortran
    pub const ASSEMBLY: &str = "\u{E6AB}";     //  - seti asm
    pub const ZIG: &str = "\u{E6A9}";          //  - seti zig
    pub const NIM: &str = "\u{E677}";          //  - seti nim
    pub const VLANG: &str = "\u{E6AC}";        //  - seti v
    pub const JULIA: &str = "\u{E624}";        //  - seti julia
    pub const CRYSTAL: &str = "\u{E62F}";      //  - seti crystal

    // Web
    pub const HTML: &str = "\u{E736}";         //  - dev html5
    pub const CSS: &str = "\u{E749}";          //  - dev css3
    pub const SCSS: &str = "\u{E603}";         //  - seti sass
    pub const SASS: &str = "\u{E603}";         //  - seti sass
    pub const LESS: &str = "\u{E60B}";         //  - seti less
    pub const JSON: &str = "\u{E60B}";         //  - seti json
    pub const XML: &str = "\u{F05C0}";         // 󰗀 - md xml
    pub const YAML: &str = "\u{E615}";         //  - seti yaml
    pub const TOML: &str = "\u{E6B2}";         //  - seti config
    pub const INI: &str = "\u{E615}";          //  - seti yaml (similar)
    pub const CSV: &str = "\u{F1C3}";          //  - fa file_excel
    pub const XSLT: &str = "\u{F05C0}";        // 󰗀 - md xml

    // Shell & Config
    pub const SHELL: &str = "\u{E795}";        //  - dev terminal
    pub const BASH: &str = "\u{E795}";         //  - dev terminal
    pub const ZSH: &str = "\u{E795}";          //  - dev terminal
    pub const FISH: &str = "\u{E795}";         //  - dev terminal
    pub const POWERSHELL: &str = "\u{EBC7}";   //  - md powershell
    pub const BATCH: &str = "\u{E795}";        //  - dev terminal
    pub const AWK: &str = "\u{E795}";          //  - dev terminal
    pub const SED: &str = "\u{E795}";          //  - dev terminal
    pub const MAKEFILE: &str = "\u{E779}";     //  - seti makefile
    pub const CMAKE: &str = "\u{E61E}";        //  - seti c (cmake)
    pub const DOCKER: &str = "\u{E7B0}";       //  - dev docker
    pub const VAGRANT: &str = "\u{F27D}";      //  - custom vagrant
    pub const TERRAFORM: &str = "\u{E69A}";    //  - seti terraform
    pub const ANSIBLE: &str = "\u{E6A0}";      //  - seti ansible
    pub const NGINX: &str = "\u{E776}";        //  - dev nginx
    pub const APACHE: &str = "\u{E769}";       //  - fa server
    pub const SYSTEMD: &str = "\u{E77D}";      //  - dev linux

    // Databases
    pub const SQL: &str = "\u{E706}";          //  - dev database
    pub const SQLITE: &str = "\u{E706}";       //  - dev database
    pub const MYSQL: &str = "\u{E704}";        //  - dev mysql
    pub const POSTGRES: &str = "\u{E76E}";     //  - dev postgresql
    pub const MONGODB: &str = "\u{E7A4}";      //  - dev mongodb
    pub const REDIS: &str = "\u{E76D}";        //  - dev redis

    // Fonts
    pub const FONT: &str = "\u{F031}";         //  - fa font
    pub const TTF: &str = "\u{F031}";          //  - fa font
    pub const OTF: &str = "\u{F031}";          //  - fa font
    pub const WOFF: &str = "\u{F031}";         //  - fa font
    pub const WOFF2: &str = "\u{F031}";        //  - fa font
    pub const EOT: &str = "\u{F031}";          //  - fa font

    // Executables & Libraries
    pub const EXECUTABLE: &str = "\u{F013}";   //  - fa cog
    pub const SHARED_LIB: &str = "\u{F487}";   //  - oct package
    pub const OBJECT: &str = "\u{F471}";       //  - oct file_binary
    pub const WASM: &str = "\u{E6A1}";         //  - seti wasm

    // Version Control
    pub const GIT: &str = "\u{E702}";          //  - dev git
    pub const GITIGNORE: &str = "\u{E702}";    //  - dev git

    // Misc
    pub const CERTIFICATE: &str = "\u{F0623}"; // 󰘣 - md certificate
    pub const KEY: &str = "\u{F084}";          //  - fa key
    pub const LOCK: &str = "\u{F023}";         //  - fa lock
    pub const LOG: &str = "\u{F0331}";         // 󰌱 - md file_document_outline
    pub const BINARY: &str = "\u{F471}";       //  - oct file_binary
    pub const DIFF: &str = "\u{F440}";         //  - oct diff
    pub const PATCH: &str = "\u{F440}";        //  - oct diff
    pub const LICENSE: &str = "\u{F0D95}";     // 󰶕 - md license
    pub const README: &str = "\u{F48A}";       //  - oct book
    pub const TODO: &str = "\u{F0AE}";         //  - fa tasks
    pub const TORRENT: &str = "\u{E796}";      //  - seti bittorrent
    pub const CALENDAR: &str = "\u{F073}";     //  - fa calendar
    pub const CONTACT: &str = "\u{F007}";      //  - fa user

    // Default
    pub const DEFAULT: &str = "\u{F15B}";      //  - fa file
}

/// Default file style (document icon, no color override)
fn default_style(fancy: bool) -> FileStyle {
    let symbol = if fancy {
        nerd_icons::DEFAULT
    } else {
        unicode_icons::DEFAULT
    };
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

        if fancy_icons {
            Self::insert_nerd_font_styles(&mut styles);
        } else {
            Self::insert_unicode_styles(&mut styles);
        }

        StyleEngine { styles, fancy_icons }
    }

    /// Insert styles using standard Unicode icons
    fn insert_unicode_styles(styles: &mut StringPatriciaMap<FileStyle>) {
        // Images - Magenta
        let image_style = FileStyle::with_color(unicode_icons::IMAGE, Color::Magenta);
        styles.insert(mime::IMAGE, image_style);
        styles.insert(mime::IMAGE_STAR, image_style);

        // Audio - Cyan
        let audio_style = FileStyle::with_color(unicode_icons::AUDIO, Color::Cyan);
        styles.insert(mime::AUDIO, audio_style);

        // Video - Yellow
        let video_style = FileStyle::with_color(unicode_icons::VIDEO, Color::Yellow);
        styles.insert(mime::VIDEO, video_style);

        // Archives - Yellow
        let archive_style = FileStyle::with_color(unicode_icons::ARCHIVE, Color::Yellow);
        styles.insert("application/zip", archive_style);
        styles.insert("application/gzip", archive_style);
        styles.insert("application/x-tar", archive_style);
        styles.insert("application/x-bzip2", archive_style);
        styles.insert("application/x-xz", archive_style);
        styles.insert("application/x-7z-compressed", archive_style);
        styles.insert("application/x-rar-compressed", archive_style);

        // Documents
        let text_style = FileStyle::with_color(unicode_icons::TEXT, Color::White);
        styles.insert("text/plain", text_style);

        let pdf_style = FileStyle::with_color(unicode_icons::PDF, Color::Red);
        styles.insert(mime::PDF, pdf_style);

        let md_style = FileStyle::with_color(unicode_icons::MARKDOWN, Color::White);
        styles.insert("text/markdown", md_style);

        let word_style = FileStyle::with_color(unicode_icons::WORD, Color::Blue);
        styles.insert(mime::WORD, word_style);
        styles.insert(mime::WORD_OOXML, word_style);
        styles.insert(mime::OPENDOC_TEXT, word_style);

        let excel_style = FileStyle::with_color(unicode_icons::EXCEL, Color::Green);
        styles.insert(mime::EXCEL, excel_style);
        styles.insert(mime::EXCEL_OOXML, excel_style);
        styles.insert(mime::OPENDOC_SPREADSHEET, excel_style);

        let ppt_style = FileStyle::with_color(unicode_icons::POWERPOINT, Color::Red);
        styles.insert(mime::POWERPOINT, ppt_style);
        styles.insert(mime::POWERPOINT_OOXML, ppt_style);
        styles.insert(mime::OPENDOC_PRESENTATION, ppt_style);

        // Config files
        let config_style = FileStyle::with_color(unicode_icons::CONFIG, Color::White);
        styles.insert("text/x-toml", config_style);
        styles.insert("application/json", config_style);
        styles.insert("application/x-yaml", config_style);
        styles.insert("text/x-yaml", config_style);
        styles.insert("text/yaml", config_style);
        styles.insert("application/xml", config_style);
        styles.insert("text/xml", config_style);

        // Code files
        let code_style = FileStyle::with_color(unicode_icons::CODE, Color::Green);
        styles.insert("text/x-python", code_style);
        styles.insert("text/x-java-source", code_style);
        styles.insert("application/javascript", code_style);
        styles.insert("text/javascript", code_style);
        styles.insert("application/typescript", code_style);
        styles.insert("text/x-c", code_style);
        styles.insert("text/x-c++", code_style);
        styles.insert("text/x-rust", code_style);
        styles.insert("text/x-go", code_style);
        styles.insert("text/x-ruby", code_style);
        styles.insert("application/x-php", code_style);
        styles.insert("text/x-shellscript", code_style);

        // Databases
        let db_style = FileStyle::with_color(unicode_icons::DATABASE, Color::DarkYellow);
        styles.insert("application/x-sqlite3", db_style);
        styles.insert("application/sql", db_style);

        // Fonts
        let font_style = FileStyle::with_color(unicode_icons::FONT, Color::Red);
        styles.insert("font/ttf", font_style);
        styles.insert("font/otf", font_style);
        styles.insert("font/woff", font_style);
        styles.insert("font/woff2", font_style);
        styles.insert("application/font-woff", font_style);
        styles.insert("application/font-woff2", font_style);

        // Executables
        let exec_style = FileStyle::with_color(unicode_icons::EXECUTABLE, Color::Green);
        styles.insert("application/x-executable", exec_style);
        styles.insert("application/x-sharedlib", exec_style);
        styles.insert("application/x-object", exec_style);
        styles.insert("application/wasm", exec_style);
    }

    /// Insert styles using Nerd Font icons
    fn insert_nerd_font_styles(styles: &mut StringPatriciaMap<FileStyle>) {
        // =====================================================================
        // Documents
        // =====================================================================
        let text_style = FileStyle::with_color(nerd_icons::TEXT, Color::White);
        styles.insert("text/plain", text_style);

        let pdf_style = FileStyle::with_color(nerd_icons::PDF, Color::Red);
        styles.insert(mime::PDF, pdf_style);

        let md_style = FileStyle::with_color(nerd_icons::MARKDOWN, Color::White);
        styles.insert("text/markdown", md_style);
        styles.insert("text/x-markdown", md_style);

        let word_style = FileStyle::with_color(nerd_icons::WORD, Color::Blue);
        styles.insert(mime::WORD, word_style);
        styles.insert(mime::WORD_OOXML, word_style);

        let opendoc_text_style = FileStyle::with_color(nerd_icons::OPENDOC_TEXT, Color::Blue);
        styles.insert(mime::OPENDOC_TEXT, opendoc_text_style);

        let excel_style = FileStyle::with_color(nerd_icons::EXCEL, Color::Green);
        styles.insert(mime::EXCEL, excel_style);
        styles.insert(mime::EXCEL_OOXML, excel_style);

        let opendoc_sheet_style = FileStyle::with_color(nerd_icons::OPENDOC_SHEET, Color::Green);
        styles.insert(mime::OPENDOC_SPREADSHEET, opendoc_sheet_style);

        let ppt_style = FileStyle::with_color(nerd_icons::POWERPOINT, Color::Red);
        styles.insert(mime::POWERPOINT, ppt_style);
        styles.insert(mime::POWERPOINT_OOXML, ppt_style);

        let opendoc_pres_style = FileStyle::with_color(nerd_icons::OPENDOC_PRES, Color::Red);
        styles.insert(mime::OPENDOC_PRESENTATION, opendoc_pres_style);

        let rtf_style = FileStyle::with_color(nerd_icons::RTF, Color::White);
        styles.insert("application/rtf", rtf_style);
        styles.insert("text/rtf", rtf_style);

        let latex_style = FileStyle::with_color(nerd_icons::LATEX, Color::Green);
        styles.insert("application/x-latex", latex_style);
        styles.insert("application/x-tex", latex_style);
        styles.insert("text/x-tex", latex_style);

        let bibtex_style = FileStyle::with_color(nerd_icons::BIBTEX, Color::Yellow);
        styles.insert("application/x-bibtex", bibtex_style);

        // =====================================================================
        // Images
        // =====================================================================
        let image_style = FileStyle::with_color(nerd_icons::IMAGE, Color::Magenta);
        styles.insert(mime::IMAGE, image_style);
        styles.insert(mime::IMAGE_STAR, image_style);
        styles.insert(mime::IMAGE_JPEG, image_style);
        styles.insert(mime::IMAGE_PNG, image_style);
        styles.insert(mime::IMAGE_GIF, image_style);
        styles.insert(mime::IMAGE_BMP, image_style);
        styles.insert("image/webp", image_style);
        styles.insert("image/x-icon", image_style);
        styles.insert("image/tiff", image_style);
        styles.insert("image/x-raw", image_style);
        styles.insert("image/heic", image_style);
        styles.insert("image/heif", image_style);
        styles.insert("image/avif", image_style);
        styles.insert("image/x-xcf", image_style);

        let svg_style = FileStyle::with_color(nerd_icons::SVG, Color::Yellow);
        styles.insert(mime::IMAGE_SVG, svg_style);
        styles.insert("image/svg+xml", svg_style);

        let psd_style = FileStyle::with_color(nerd_icons::PSD, Color::Cyan);
        styles.insert("image/vnd.adobe.photoshop", psd_style);
        styles.insert("image/x-photoshop", psd_style);

        // =====================================================================
        // Audio
        // =====================================================================
        let audio_style = FileStyle::with_color(nerd_icons::AUDIO, Color::Cyan);
        styles.insert(mime::AUDIO, audio_style);
        styles.insert("audio/mpeg", audio_style);
        styles.insert("audio/mp3", audio_style);
        styles.insert("audio/flac", audio_style);
        styles.insert("audio/x-flac", audio_style);
        styles.insert("audio/wav", audio_style);
        styles.insert("audio/x-wav", audio_style);
        styles.insert("audio/ogg", audio_style);
        styles.insert("audio/aac", audio_style);
        styles.insert("audio/x-aac", audio_style);
        styles.insert("audio/mp4", audio_style);
        styles.insert("audio/x-m4a", audio_style);
        styles.insert("audio/opus", audio_style);
        styles.insert("audio/midi", audio_style);
        styles.insert("audio/x-midi", audio_style);
        styles.insert("audio/aiff", audio_style);
        styles.insert("audio/x-aiff", audio_style);

        // =====================================================================
        // Video
        // =====================================================================
        let video_style = FileStyle::with_color(nerd_icons::VIDEO, Color::Yellow);
        styles.insert(mime::VIDEO, video_style);
        styles.insert("video/mp4", video_style);
        styles.insert("video/x-matroska", video_style);
        styles.insert("video/x-msvideo", video_style);
        styles.insert("video/quicktime", video_style);
        styles.insert("video/x-ms-wmv", video_style);
        styles.insert("video/x-flv", video_style);
        styles.insert("video/webm", video_style);
        styles.insert("video/mpeg", video_style);
        styles.insert("video/3gpp", video_style);
        styles.insert("video/3gpp2", video_style);

        // =====================================================================
        // Archives
        // =====================================================================
        let archive_style = FileStyle::with_color(nerd_icons::ARCHIVE, Color::Yellow);
        styles.insert("application/zip", archive_style);
        styles.insert("application/gzip", archive_style);
        styles.insert("application/x-gzip", archive_style);
        styles.insert("application/x-tar", archive_style);
        styles.insert("application/x-bzip2", archive_style);
        styles.insert("application/x-xz", archive_style);
        styles.insert("application/x-7z-compressed", archive_style);
        styles.insert("application/x-rar-compressed", archive_style);
        styles.insert("application/vnd.rar", archive_style);
        styles.insert("application/zstd", archive_style);
        styles.insert("application/x-zstd", archive_style);
        styles.insert("application/x-lz4", archive_style);

        let deb_style = FileStyle::with_color(nerd_icons::DEB, Color::Red);
        styles.insert("application/vnd.debian.binary-package", deb_style);
        styles.insert("application/x-deb", deb_style);

        let rpm_style = FileStyle::with_color(nerd_icons::RPM, Color::Red);
        styles.insert("application/x-rpm", rpm_style);

        let iso_style = FileStyle::with_color(nerd_icons::ISO, Color::Magenta);
        styles.insert("application/x-iso9660-image", iso_style);

        // =====================================================================
        // Programming Languages
        // =====================================================================
        let python_style = FileStyle::with_color(nerd_icons::PYTHON, Color::Yellow);
        styles.insert("text/x-python", python_style);
        styles.insert("application/x-python-code", python_style);

        let java_style = FileStyle::with_color(nerd_icons::JAVA, Color::Red);
        styles.insert("text/x-java-source", java_style);
        styles.insert("text/x-java", java_style);

        let js_style = FileStyle::with_color(nerd_icons::JAVASCRIPT, Color::Yellow);
        styles.insert("application/javascript", js_style);
        styles.insert("text/javascript", js_style);
        styles.insert("application/x-javascript", js_style);

        let ts_style = FileStyle::with_color(nerd_icons::TYPESCRIPT, Color::Blue);
        styles.insert("application/typescript", ts_style);
        styles.insert("text/typescript", ts_style);
        styles.insert("application/x-typescript", ts_style);

        let csharp_style = FileStyle::with_color(nerd_icons::CSHARP, Color::Magenta);
        styles.insert("text/x-csharp", csharp_style);

        let cpp_style = FileStyle::with_color(nerd_icons::CPP, Color::Blue);
        styles.insert("text/x-c++", cpp_style);
        styles.insert("text/x-c++src", cpp_style);
        styles.insert("text/x-c++hdr", cpp_style);

        let c_style = FileStyle::with_color(nerd_icons::C, Color::Blue);
        styles.insert("text/x-c", c_style);
        styles.insert("text/x-csrc", c_style);
        styles.insert("text/x-chdr", c_style);

        let rust_style = FileStyle::with_color(nerd_icons::RUST, Color::Red);
        styles.insert("text/x-rust", rust_style);
        styles.insert("text/rust", rust_style);

        let go_style = FileStyle::with_color(nerd_icons::GO, Color::Cyan);
        styles.insert("text/x-go", go_style);
        styles.insert("text/x-gosrc", go_style);

        let ruby_style = FileStyle::with_color(nerd_icons::RUBY, Color::Red);
        styles.insert("text/x-ruby", ruby_style);
        styles.insert("application/x-ruby", ruby_style);

        let php_style = FileStyle::with_color(nerd_icons::PHP, Color::Magenta);
        styles.insert("application/x-php", php_style);
        styles.insert("text/x-php", php_style);

        let swift_style = FileStyle::with_color(nerd_icons::SWIFT, Color::Red);
        styles.insert("text/x-swift", swift_style);

        let kotlin_style = FileStyle::with_color(nerd_icons::KOTLIN, Color::Magenta);
        styles.insert("text/x-kotlin", kotlin_style);

        let scala_style = FileStyle::with_color(nerd_icons::SCALA, Color::Red);
        styles.insert("text/x-scala", scala_style);

        let clojure_style = FileStyle::with_color(nerd_icons::CLOJURE, Color::Green);
        styles.insert("text/x-clojure", clojure_style);

        let haskell_style = FileStyle::with_color(nerd_icons::HASKELL, Color::Magenta);
        styles.insert("text/x-haskell", haskell_style);

        let erlang_style = FileStyle::with_color(nerd_icons::ERLANG, Color::Red);
        styles.insert("text/x-erlang", erlang_style);

        let elixir_style = FileStyle::with_color(nerd_icons::ELIXIR, Color::Magenta);
        styles.insert("text/x-elixir", elixir_style);

        let lua_style = FileStyle::with_color(nerd_icons::LUA, Color::Blue);
        styles.insert("text/x-lua", lua_style);

        let perl_style = FileStyle::with_color(nerd_icons::PERL, Color::Blue);
        styles.insert("text/x-perl", perl_style);
        styles.insert("application/x-perl", perl_style);

        let r_style = FileStyle::with_color(nerd_icons::R, Color::Blue);
        styles.insert("text/x-r", r_style);
        styles.insert("text/x-r-source", r_style);

        let dart_style = FileStyle::with_color(nerd_icons::DART, Color::Cyan);
        styles.insert("application/dart", dart_style);
        styles.insert("text/x-dart", dart_style);

        let ocaml_style = FileStyle::with_color(nerd_icons::OCAML, Color::Yellow);
        styles.insert("text/x-ocaml", ocaml_style);

        let fsharp_style = FileStyle::with_color(nerd_icons::FSHARP, Color::Cyan);
        styles.insert("text/x-fsharp", fsharp_style);

        let lisp_style = FileStyle::with_color(nerd_icons::LISP, Color::White);
        styles.insert("text/x-lisp", lisp_style);
        styles.insert("text/x-common-lisp", lisp_style);
        styles.insert("text/x-scheme", lisp_style);

        let fortran_style = FileStyle::with_color(nerd_icons::FORTRAN, Color::Magenta);
        styles.insert("text/x-fortran", fortran_style);

        let asm_style = FileStyle::with_color(nerd_icons::ASSEMBLY, Color::Red);
        styles.insert("text/x-asm", asm_style);
        styles.insert("text/x-assembly", asm_style);

        let zig_style = FileStyle::with_color(nerd_icons::ZIG, Color::Yellow);
        styles.insert("text/x-zig", zig_style);

        let nim_style = FileStyle::with_color(nerd_icons::NIM, Color::Yellow);
        styles.insert("text/x-nim", nim_style);

        let v_style = FileStyle::with_color(nerd_icons::VLANG, Color::Blue);
        styles.insert("text/x-v", v_style);

        let julia_style = FileStyle::with_color(nerd_icons::JULIA, Color::Magenta);
        styles.insert("text/x-julia", julia_style);

        let crystal_style = FileStyle::with_color(nerd_icons::CRYSTAL, Color::White);
        styles.insert("text/x-crystal", crystal_style);

        // =====================================================================
        // Web Technologies
        // =====================================================================
        let html_style = FileStyle::with_color(nerd_icons::HTML, Color::Red);
        styles.insert("text/html", html_style);
        styles.insert("application/xhtml+xml", html_style);

        let css_style = FileStyle::with_color(nerd_icons::CSS, Color::Blue);
        styles.insert("text/css", css_style);

        let scss_style = FileStyle::with_color(nerd_icons::SCSS, Color::Magenta);
        styles.insert("text/x-scss", scss_style);

        let sass_style = FileStyle::with_color(nerd_icons::SASS, Color::Magenta);
        styles.insert("text/x-sass", sass_style);

        let less_style = FileStyle::with_color(nerd_icons::LESS, Color::Blue);
        styles.insert("text/x-less", less_style);

        let json_style = FileStyle::with_color(nerd_icons::JSON, Color::Yellow);
        styles.insert("application/json", json_style);
        styles.insert("text/json", json_style);

        let xml_style = FileStyle::with_color(nerd_icons::XML, Color::Yellow);
        styles.insert("application/xml", xml_style);
        styles.insert("text/xml", xml_style);

        let yaml_style = FileStyle::with_color(nerd_icons::YAML, Color::DarkCyan);
        styles.insert("application/x-yaml", yaml_style);
        styles.insert("text/x-yaml", yaml_style);
        styles.insert("text/yaml", yaml_style);

        let toml_style = FileStyle::with_color(nerd_icons::TOML, Color::White);
        styles.insert("text/x-toml", toml_style);
        styles.insert("application/toml", toml_style);

        let ini_style = FileStyle::with_color(nerd_icons::INI, Color::White);
        styles.insert("text/x-ini", ini_style);

        let csv_style = FileStyle::with_color(nerd_icons::CSV, Color::Green);
        styles.insert("text/csv", csv_style);

        let xslt_style = FileStyle::with_color(nerd_icons::XSLT, Color::Yellow);
        styles.insert("application/xslt+xml", xslt_style);

        // =====================================================================
        // Shell & Config
        // =====================================================================
        let shell_style = FileStyle::with_color(nerd_icons::SHELL, Color::Green);
        styles.insert("text/x-shellscript", shell_style);
        styles.insert("application/x-sh", shell_style);
        styles.insert("application/x-shellscript", shell_style);
        styles.insert("text/x-sh", shell_style);

        let bash_style = FileStyle::with_color(nerd_icons::BASH, Color::Green);
        styles.insert("application/x-bash", bash_style);

        let zsh_style = FileStyle::with_color(nerd_icons::ZSH, Color::Green);
        styles.insert("text/x-zsh", zsh_style);

        let fish_style = FileStyle::with_color(nerd_icons::FISH, Color::Green);
        styles.insert("text/x-fish", fish_style);

        let powershell_style = FileStyle::with_color(nerd_icons::POWERSHELL, Color::Blue);
        styles.insert("application/x-powershell", powershell_style);

        let makefile_style = FileStyle::with_color(nerd_icons::MAKEFILE, Color::Yellow);
        styles.insert("text/x-makefile", makefile_style);

        let cmake_style = FileStyle::with_color(nerd_icons::CMAKE, Color::Green);
        styles.insert("text/x-cmake", cmake_style);

        let docker_style = FileStyle::with_color(nerd_icons::DOCKER, Color::Cyan);
        styles.insert("application/x-docker", docker_style);
        styles.insert("text/x-dockerfile", docker_style);

        // =====================================================================
        // Databases
        // =====================================================================
        let sql_style = FileStyle::with_color(nerd_icons::SQL, Color::DarkYellow);
        styles.insert("application/sql", sql_style);
        styles.insert("text/x-sql", sql_style);

        let sqlite_style = FileStyle::with_color(nerd_icons::SQLITE, Color::Blue);
        styles.insert("application/x-sqlite3", sqlite_style);
        styles.insert("application/vnd.sqlite3", sqlite_style);

        // =====================================================================
        // Fonts
        // =====================================================================
        let font_style = FileStyle::with_color(nerd_icons::FONT, Color::Red);
        styles.insert("font/ttf", font_style);
        styles.insert("font/otf", font_style);
        styles.insert("font/woff", font_style);
        styles.insert("font/woff2", font_style);
        styles.insert("application/font-sfnt", font_style);
        styles.insert("application/font-woff", font_style);
        styles.insert("application/font-woff2", font_style);
        styles.insert("application/vnd.ms-fontobject", font_style);
        styles.insert("application/x-font-ttf", font_style);
        styles.insert("application/x-font-otf", font_style);

        // =====================================================================
        // Executables & Libraries
        // =====================================================================
        let exec_style = FileStyle::with_color(nerd_icons::EXECUTABLE, Color::Green);
        styles.insert("application/x-executable", exec_style);
        styles.insert("application/x-mach-binary", exec_style);
        styles.insert("application/x-pie-executable", exec_style);

        let shared_lib_style = FileStyle::with_color(nerd_icons::SHARED_LIB, Color::Blue);
        styles.insert("application/x-sharedlib", shared_lib_style);

        let object_style = FileStyle::with_color(nerd_icons::OBJECT, Color::White);
        styles.insert("application/x-object", object_style);

        let wasm_style = FileStyle::with_color(nerd_icons::WASM, Color::Magenta);
        styles.insert("application/wasm", wasm_style);

        // =====================================================================
        // Miscellaneous
        // =====================================================================
        let cert_style = FileStyle::with_color(nerd_icons::CERTIFICATE, Color::Yellow);
        styles.insert("application/x-x509-ca-cert", cert_style);
        styles.insert("application/x-x509-user-cert", cert_style);
        styles.insert("application/pkix-cert", cert_style);

        let key_style = FileStyle::with_color(nerd_icons::KEY, Color::Yellow);
        styles.insert("application/x-pem-key", key_style);
        styles.insert("application/pgp-keys", key_style);

        let binary_style = FileStyle::with_color(nerd_icons::BINARY, Color::Red);
        styles.insert("application/octet-stream", binary_style);

        let diff_style = FileStyle::with_color(nerd_icons::DIFF, Color::Green);
        styles.insert("text/x-diff", diff_style);
        styles.insert("text/x-patch", diff_style);

        let torrent_style = FileStyle::with_color(nerd_icons::TORRENT, Color::Green);
        styles.insert("application/x-bittorrent", torrent_style);

        let calendar_style = FileStyle::with_color(nerd_icons::CALENDAR, Color::Red);
        styles.insert("text/calendar", calendar_style);

        let contact_style = FileStyle::with_color(nerd_icons::CONTACT, Color::Blue);
        styles.insert("text/x-vcard", contact_style);
        styles.insert("text/vcard", contact_style);
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

/// Mime type string constants for common types
mod mime {
    // Generic type prefixes
    pub const IMAGE: &str = "image";
    pub const AUDIO: &str = "audio";
    pub const VIDEO: &str = "video";

    // Image types
    pub const IMAGE_STAR: &str = "image/*";
    pub const IMAGE_JPEG: &str = "image/jpeg";
    pub const IMAGE_PNG: &str = "image/png";
    pub const IMAGE_GIF: &str = "image/gif";
    pub const IMAGE_BMP: &str = "image/bmp";
    pub const IMAGE_SVG: &str = "image/svg+xml";

    // Documents
    pub const PDF: &str = "application/pdf";
    pub const WORD: &str = "application/msword";
    pub const WORD_OOXML: &str =
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    pub const EXCEL: &str = "application/vnd.ms-excel";
    pub const EXCEL_OOXML: &str =
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
    pub const POWERPOINT: &str = "application/vnd.ms-powerpoint";
    pub const POWERPOINT_OOXML: &str =
        "application/vnd.openxmlformats-officedocument.presentationml.presentation";
    pub const OPENDOC_TEXT: &str = "application/vnd.oasis.opendocument.text";
    pub const OPENDOC_SPREADSHEET: &str = "application/vnd.oasis.opendocument.spreadsheet";
    pub const OPENDOC_PRESENTATION: &str = "application/vnd.oasis.opendocument.presentation";
}
