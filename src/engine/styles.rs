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
    pub const TEXT: &str = "\u{1F5B9}"; // 🖹 - document
    pub const PDF: &str = "\u{1F4C4}"; // 📄 - document
    pub const MARKDOWN: &str = "\u{1F89B}"; // 🢛 - downward arrow
    pub const WORD: &str = "\u{1F4C4}"; // 📄 - document
    pub const EXCEL: &str = "\u{1F4CA}"; // 📊 - chart
    pub const POWERPOINT: &str = "\u{1F4CA}"; // 📊 - chart

    // Media
    pub const IMAGE: &str = "\u{1F5BB}"; // 🖻 - image
    pub const AUDIO: &str = "\u{266B}"; // ♫ - music note
    pub const VIDEO: &str = "\u{1F39E}"; // 🎞 - film frames

    // Archives
    pub const ARCHIVE: &str = "\u{1F5DC}"; // 🗜 - compression

    // Config
    pub const CONFIG: &str = "\u{2699}"; // ⚙ - gear

    // Code
    pub const CODE: &str = "\u{1F5B9}"; // 🖹 - document (generic for code)

    // Database
    pub const DATABASE: &str = "\u{1F5C3}"; // 🗃 - card file box

    // Font
    pub const FONT: &str = "\u{1F524}"; // 🔤 - input latin letters

    // Executable
    pub const EXECUTABLE: &str = "\u{2699}"; // ⚙ - gear

    // Default
    pub const DEFAULT: &str = "\u{1F5B9}"; // 🖹 - document
}

// =============================================================================
// Nerd Font Icons (require a patched Nerd Font)
// Icons from mime-types.md reference (Material Design Icons)
// =============================================================================
//
// NOTE: All of these are not standard unicode, so `unicode_width` will return the wrong
// width for these symbols. This is why (as an easy solution) we simply add a space " " at the end,
// to account for the correct width of the symbol.
#[allow(dead_code)]
mod nerd_icons {
    // Documents
    pub const TEXT: &str = "\u{F0219} "; // 󰈙 - md file
    pub const PDF: &str = "\u{F0226} "; // 󰈦 - md file_pdf
    pub const MARKDOWN: &str = "\u{F0354} "; // 󰍔 - md markdown
    pub const WORD: &str = "\u{F022C} "; // 󰈬 - md file_word
    pub const EXCEL: &str = "\u{F021B} "; // 󰈛 - md file_excel
    pub const POWERPOINT: &str = "\u{F0227} "; // 󰈧 - md file_powerpoint
    pub const OPENDOC_TEXT: &str = "\u{F022C} "; // 󰈬 - md file_word (ODT)
    pub const OPENDOC_SHEET: &str = "\u{F021B} "; // 󰈛 - md file_excel (ODS)
    pub const OPENDOC_PRES: &str = "\u{F0227} "; // 󰈧 - md file_powerpoint (ODP)
    pub const RTF: &str = "\u{F0219} "; // 󰈙 - md file
    pub const CSV: &str = "\u{F021B} "; // 󰈛 - md file_excel
    pub const LATEX: &str = "\u{F0219} "; // 󰈙 - md file
    pub const BIBTEX: &str = "\u{F0219} "; // 󰈙 - md file

    // Images
    pub const IMAGE: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const JPEG: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const PNG: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const GIF: &str = "\u{F0D78} "; // 󰵸 - md gif
    pub const BMP: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const SVG: &str = "\u{F0721} "; // 󰜡 - md svg
    pub const WEBP: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const ICO: &str = "\u{F0039} "; // 󰀹 - md image_filter_vintage
    pub const TIFF: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const PSD: &str = "\u{F01E5} "; // 󰇥 - md image_edit
    pub const XCF: &str = "\u{F021F} "; // 󰈟 - md file_image (GIMP)
    pub const RAW: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const HEIC: &str = "\u{F021F} "; // 󰈟 - md file_image
    pub const AVIF: &str = "\u{F021F} "; // 󰈟 - md file_image

    // Audio
    pub const AUDIO: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const MP3: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const FLAC: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const WAV: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const OGG: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const AAC: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const M4A: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const OPUS: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const MIDI: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const AIFF: &str = "\u{F0386} "; // 󰎆 - md file_music
    pub const WMA: &str = "\u{F0386} "; // 󰎆 - md file_music

    // Video
    pub const VIDEO: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const MP4: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const MKV: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const AVI: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const MOV: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const WMV: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const FLV: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const WEBM: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const MPEG: &str = "\u{F0567} "; // 󰕧 - md file_video
    pub const THREE_GPP: &str = "\u{F0567} "; // 󰕧 - md file_video

    // Archives
    pub const ARCHIVE: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const ZIP: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const GZIP: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const TAR: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const BZIP2: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const XZ: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const SEVENZ: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const RAR: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const ZSTD: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const LZ4: &str = "\u{F05C4} "; // 󰗄 - md folder_zip
    pub const DEB: &str = "\u{F08DA} "; // 󰣚 - md debian
    pub const RPM: &str = "\u{F08DB} "; // 󰣛 - md redhat
    pub const ISO: &str = "\u{F05EE} "; // 󰗮 - md disc
    pub const JAR: &str = "\u{F0B37} "; // 󰬷 - md language_java
    pub const DMG: &str = "\u{F0035} "; // 󰀵 - md apple
    pub const APK: &str = "\u{F0032} "; // 󰀲 - md android

    // Programming Languages
    pub const PYTHON: &str = "\u{F0320} "; // 󰌠 - md language_python
    pub const JAVA: &str = "\u{F0B37} "; // 󰬷 - md language_java
    pub const JAVASCRIPT: &str = "\u{F031E} "; // 󰌞 - md language_javascript
    pub const TYPESCRIPT: &str = "\u{F06E6} "; // 󰛦 - md language_typescript
    pub const CSHARP: &str = "\u{F031B} "; // 󰌛 - md language_csharp
    pub const CPP: &str = "\u{F0672} "; // 󰙲 - md language_cpp
    pub const C: &str = "\u{F0671} "; // 󰙱 - md language_c
    pub const RUST: &str = "\u{F1617} "; // 󱘗 - md language_rust
    pub const GO: &str = "\u{F07D3} "; // 󰟓 - md language_go
    pub const RUBY: &str = "\u{F0D2D} "; // 󰴭 - md language_ruby
    pub const PHP: &str = "\u{F031F} "; // 󰌟 - md language_php
    pub const SWIFT: &str = "\u{F06E5} "; // 󰛥 - md language_swift
    pub const KOTLIN: &str = "\u{F1219} "; // 󱈙 - md language_kotlin
    pub const SCALA: &str = "\u{F0617} "; // 󰘗 - md language_scala
    pub const CLOJURE: &str = "\u{E76A} "; //  - dev clojure
    pub const HASKELL: &str = "\u{F0C92} "; // 󰲒 - md language_haskell
    pub const ERLANG: &str = "\u{E7B1} "; //  - dev erlang
    pub const ELIXIR: &str = "\u{E62D} "; //  - seti elixir
    pub const LUA: &str = "\u{F08B1} "; // 󰢱 - md language_lua
    pub const PERL: &str = "\u{E769} "; //  - dev perl
    pub const R: &str = "\u{F07D4} "; // 󰟔 - md language_r
    pub const DART: &str = "\u{E798} "; //  - dev dart
    pub const OCAML: &str = "\u{E67A} "; //  - seti ocaml
    pub const FSHARP: &str = "\u{E7A7} "; //  - dev fsharp
    pub const LISP: &str = "\u{F0172} "; // 󰅲 - md lambda
    pub const FORTRAN: &str = "\u{F121A} "; // 󱈚 - md language_fortran
    pub const ASSEMBLY: &str = "\u{E6AB} "; //  - seti asm
    pub const ZIG: &str = "\u{E6A9} "; //  - seti zig
    pub const NIM: &str = "\u{E677} "; //  - seti nim
    pub const VLANG: &str = "\u{E6AC} "; //  - seti v
    pub const JULIA: &str = "\u{E624} "; //  - seti julia
    pub const CRYSTAL: &str = "\u{E62F} "; //  - seti crystal
    pub const NIX: &str = "\u{F1105} "; // 󱄅 - md nix
    pub const VUE: &str = "\u{F0844} "; // 󰡄 - md vuejs
    pub const SVELTE: &str = "\u{E697} "; //  - seti svelte
    pub const GRAPHQL: &str = "\u{F0877} "; // 󰡷 - md graphql
    pub const PROTO: &str = "\u{F0626} "; // 󰘦 - md code_json (protobuf)
    pub const SOLIDITY: &str = "\u{F0CD8} "; // 󰳘 - md ethereum
    pub const ASTRO: &str = "\u{E6B6} "; //  - seti astro
    pub const GRADLE: &str = "\u{E660} "; //  - seti gradle
    pub const GROOVY: &str = "\u{E775} "; //  - dev groovy

    // Web
    pub const HTML: &str = "\u{F031D} "; // 󰌝 - md language_html5
    pub const CSS: &str = "\u{F031C} "; // 󰌜 - md language_css3
    pub const SCSS: &str = "\u{E603} "; //  - seti sass
    pub const SASS: &str = "\u{E603} "; //  - seti sass
    pub const LESS: &str = "\u{E60B} "; //  - seti less
    pub const JSON: &str = "\u{F0626} "; // 󰘦 - md code_json
    pub const XML: &str = "\u{F05C0} "; // 󰗀 - md xml
    pub const YAML: &str = "\u{F022E} "; // 󰈮 - md file_code
    pub const TOML: &str = "\u{F0493} "; // 󰒓 - md cog
    pub const INI: &str = "\u{F0493} "; // 󰒓 - md cog
    pub const XSLT: &str = "\u{F05C0} "; // 󰗀 - md xml

    // Shell & Config
    pub const SHELL: &str = "\u{F018D} "; // 󰆍 - md console
    pub const BASH: &str = "\u{F1183} "; // 󱆃 - md bash
    pub const ZSH: &str = "\u{F1183} "; // 󱆃 - md bash
    pub const FISH: &str = "\u{F1183} "; // 󱆃 - md bash
    pub const POWERSHELL: &str = "\u{F0A0A} "; // 󰨊 - md powershell
    pub const BATCH: &str = "\u{F018D} "; // 󰆍 - md console
    pub const AWK: &str = "\u{F018D} "; // 󰆍 - md console
    pub const SED: &str = "\u{F018D} "; // 󰆍 - md console
    pub const MAKEFILE: &str = "\u{F1064} "; // 󱁤 - md file_cog
    pub const CMAKE: &str = "\u{F106F} "; // 󱁯 - md cmake
    pub const DOCKER: &str = "\u{F0868} "; // 󰡨 - md docker
    pub const VAGRANT: &str = "\u{F27D} "; //  - custom vagrant
    pub const TERRAFORM: &str = "\u{E69A} "; //  - seti terraform
    pub const ANSIBLE: &str = "\u{E6A0} "; //  - seti ansible
    pub const NGINX: &str = "\u{E776} "; //  - dev nginx
    pub const APACHE: &str = "\u{E769} "; //  - fa server
    pub const SYSTEMD: &str = "\u{F033A} "; // 󰌺 - md linux

    // Databases
    pub const SQL: &str = "\u{F01BC} "; // 󰆼 - md database
    pub const SQLITE: &str = "\u{F01BC} "; // 󰆼 - md database
    pub const MYSQL: &str = "\u{F01BC} "; // 󰆼 - md database
    pub const POSTGRES: &str = "\u{F01BC} "; // 󰆼 - md database
    pub const MONGODB: &str = "\u{F01BC} "; // 󰆼 - md database
    pub const REDIS: &str = "\u{F01BC} "; // 󰆼 - md database

    // Fonts
    pub const FONT: &str = "\u{F06D6} "; // 󰛖 - md format_font
    pub const TTF: &str = "\u{F06D6} "; // 󰛖 - md format_font
    pub const OTF: &str = "\u{F06D6} "; // 󰛖 - md format_font
    pub const WOFF: &str = "\u{F06D6} "; // 󰛖 - md format_font
    pub const WOFF2: &str = "\u{F06D6} "; // 󰛖 - md format_font
    pub const EOT: &str = "\u{F06D6} "; // 󰛖 - md format_font

    // Executables & Libraries
    pub const EXECUTABLE: &str = "\u{F0614} "; // 󰘔 - md application
    pub const WINDOWS_EXE: &str = "\u{F05B3} "; // 󰖳 - md microsoft_windows
    pub const SHARED_LIB: &str = "\u{F0614} "; // 󰘔 - md application
    pub const OBJECT: &str = "\u{F0224} "; // 󰈤 - md file
    pub const WASM: &str = "\u{E6A1} "; //  - seti wasm

    // Version Control
    pub const GIT: &str = "\u{F02A2} "; // 󰊢 - md git
    pub const GITIGNORE: &str = "\u{F02A2} "; // 󰊢 - md git

    // Misc
    pub const CERTIFICATE: &str = "\u{F0124} "; // 󰄤 - md certificate
    pub const KEY: &str = "\u{F0306} "; // 󰌆 - md key
    pub const LOCK: &str = "\u{F033E} "; // 󰌾 - md lock
    pub const LOG: &str = "\u{F0331} "; // 󰌱 - md file_document
    pub const BINARY: &str = "\u{F0224} "; // 󰈤 - md file
    pub const DIFF: &str = "\u{F0224} "; // 󰈤 - md file
    pub const PATCH: &str = "\u{F0224} "; // 󰈤 - md file
    pub const LICENSE: &str = "\u{F0FC3} "; // 󰿃 - md license
    pub const README: &str = "\u{F00BA} "; // 󰂺 - md book_open
    pub const TODO: &str = "\u{F0AE} "; //  - fa tasks
    pub const TORRENT: &str = "\u{E796} "; //  - seti bittorrent
    pub const CALENDAR: &str = "\u{F00F5} "; // 󰃵 - md calendar
    pub const CONTACT: &str = "\u{F007} "; //  - fa user
    pub const EMAIL: &str = "\u{F01EE} "; // 󰇮 - md email
    pub const PGP: &str = "\u{F0306} "; // 󰌆 - md key
    pub const KML: &str = "\u{F018B} "; // 󰆋 - md google_earth
    pub const ENV: &str = "\u{F066A} "; // 󰙪 - md file_cog
    pub const EDITORCONFIG: &str = "\u{F0493} "; // 󰒓 - md cog
    pub const NPMRC: &str = "\u{F0399} "; // 󰎙 - md npm
    pub const CARGO: &str = "\u{F1617} "; // 󱘗 - md language_rust
    pub const GOMOD: &str = "\u{F07D3} "; // 󰟓 - md language_go

    // Directory
    pub const DIRECTORY: &str = "\u{F024B} "; // 󰉋 - md folder

    // Default
    pub const DEFAULT: &str = "\u{F0224} "; // 󰈤 - md file
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

        StyleEngine {
            styles,
            fancy_icons,
        }
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
        styles.insert(mime::IMAGE_BMP, image_style);
        styles.insert("image/webp", image_style);
        styles.insert("image/tiff", image_style);
        styles.insert("image/x-raw", image_style);
        styles.insert("image/heic", image_style);
        styles.insert("image/heif", image_style);
        styles.insert("image/avif", image_style);
        styles.insert("image/x-xcf", image_style);

        // GIF has a special icon
        let gif_style = FileStyle::with_color(nerd_icons::GIF, Color::Magenta);
        styles.insert(mime::IMAGE_GIF, gif_style);

        // ICO has a special icon
        let ico_style = FileStyle::with_color(nerd_icons::ICO, Color::Magenta);
        styles.insert("image/x-icon", ico_style);

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
        styles.insert("audio/webm", audio_style);
        styles.insert("audio/x-ms-wma", audio_style);

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

        let nix_style = FileStyle::with_color(nerd_icons::NIX, Color::Cyan);
        styles.insert("text/x-nix", nix_style);

        let vue_style = FileStyle::with_color(nerd_icons::VUE, Color::Green);
        styles.insert("text/x-vue", vue_style);

        let svelte_style = FileStyle::with_color(nerd_icons::SVELTE, Color::Red);
        styles.insert("text/x-svelte", svelte_style);

        let graphql_style = FileStyle::with_color(nerd_icons::GRAPHQL, Color::Magenta);
        styles.insert("text/x-graphql", graphql_style);

        let proto_style = FileStyle::with_color(nerd_icons::PROTO, Color::Blue);
        styles.insert("text/x-protobuf", proto_style);

        let solidity_style = FileStyle::with_color(nerd_icons::SOLIDITY, Color::Blue);
        styles.insert("text/x-solidity", solidity_style);

        let astro_style = FileStyle::with_color(nerd_icons::ASTRO, Color::Red);
        styles.insert("text/x-astro", astro_style);

        let gradle_style = FileStyle::with_color(nerd_icons::GRADLE, Color::Blue);
        styles.insert("text/x-gradle", gradle_style);

        let groovy_style = FileStyle::with_color(nerd_icons::GROOVY, Color::Cyan);
        styles.insert("text/x-groovy", groovy_style);

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

        let windows_exe_style = FileStyle::with_color(nerd_icons::WINDOWS_EXE, Color::Blue);
        styles.insert("application/x-msdownload", windows_exe_style);
        styles.insert("application/x-dosexec", windows_exe_style);

        let shared_lib_style = FileStyle::with_color(nerd_icons::SHARED_LIB, Color::Blue);
        styles.insert("application/x-sharedlib", shared_lib_style);

        let object_style = FileStyle::with_color(nerd_icons::OBJECT, Color::White);
        styles.insert("application/x-object", object_style);

        let wasm_style = FileStyle::with_color(nerd_icons::WASM, Color::Magenta);
        styles.insert("application/wasm", wasm_style);

        // JAR files
        let jar_style = FileStyle::with_color(nerd_icons::JAR, Color::Red);
        styles.insert("application/x-java-archive", jar_style);

        // DMG files
        let dmg_style = FileStyle::with_color(nerd_icons::DMG, Color::White);
        styles.insert("application/x-apple-diskimage", dmg_style);

        // APK files
        let apk_style = FileStyle::with_color(nerd_icons::APK, Color::Green);
        styles.insert("application/vnd.android.package-archive", apk_style);

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

        // Email
        let email_style = FileStyle::with_color(nerd_icons::EMAIL, Color::Yellow);
        styles.insert("message/rfc822", email_style);

        // PGP encrypted
        let pgp_style = FileStyle::with_color(nerd_icons::PGP, Color::Green);
        styles.insert("application/pgp-encrypted", pgp_style);

        // KML (Google Earth)
        let kml_style = FileStyle::with_color(nerd_icons::KML, Color::Blue);
        styles.insert("application/vnd.google-earth.kml+xml", kml_style);

        // Git
        let git_style = FileStyle::with_color(nerd_icons::GIT, Color::Red);
        styles.insert("application/x-git", git_style);
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
