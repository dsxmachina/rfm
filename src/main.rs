use anyhow::Context;
use clap::Parser;
use content::{PanelCache, SHUTDOWN_FLAG};
use crossterm::{
    cursor,
    event::DisableMouseCapture,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap, EnableLineWrap,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
    QueueableCommand,
};
use engine::{
    commands::{CloseCmd, CommandParser},
    OpenEngine, SymbolEngine,
};
use log::{error, info, warn};
use logger::LogBuffer;
use panel::{init_miller_panels, manager::PanelManager};
use rust_embed::Embed;
use std::{
    fs::{File, OpenOptions},
    io::{stdout, IsTerminal, Write},
    path::PathBuf,
    time::Duration,
};
use tokio::sync::mpsc;
use util::xdg_config_home;

use crate::config::color::{colors_from_config, colors_from_default};

mod config;
mod content;
mod engine;
mod logger;
mod panel;
mod util;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Makes rfm act like a diretory chooser. Upon quitting
    /// it will write the full path of the last visited directory to CHOOSEDIR
    #[arg(long)]
    choosedir: Option<PathBuf>,
    /// Path to open (defaults to ".")
    path: Option<PathBuf>,
}

const ERROR_MSG: &str = "\
+------------------------------------------------------------------+
| Encountered an unexpected error. This is a bug!                  |
|                                                                  |
| If you want to help me out, please open an issue on              |
|                                                                  |
| https://github.com/dsxmachina/rfm/issues                         |
|                                                                  |
| and include the error message below.                             |
+------------------------------------------------------------------+
";

#[derive(Embed)]
#[folder = "examples/"]
struct Examples;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    // Check if we run from a terminal
    let mut stdout = stdout();
    if !stdout.is_terminal() {
        eprintln!("Error: Stdout handle does not refer to a terminal/tty");
        eprintln!();
        eprintln!("Please note: The output of rfm can be neither piped nor redirected.");
        std::process::exit(1);
    }

    let args = Args::parse();

    std::panic::set_hook(Box::new(|panic_info| {
        error!("{panic_info}");
    }));

    // Remember starting path
    let starting_path = if let Some(path) = args.path {
        path
    } else {
        std::env::current_dir().context("failed to get current directory from env")?
    };

    // Initialize logger
    let logger = LogBuffer::default()
        .with_level(log::Level::Debug)
        .with_capacity(15);
    log::set_boxed_logger(Box::new(logger.clone())).context("failed to initialize logger")?;
    log::set_max_level(log::LevelFilter::Info);

    // Spawn a task that periodically removes the oldest log line
    //
    // This automatically ensures that any error message will be removed after 2 * LOG_CAPACITY seconds
    let periodic_logger = logger.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            periodic_logger.remove_oldest();
        }
    });

    // --- Read config directory
    let config_dir = xdg_config_home()
        .context("failed to get $XDG_CONFIG_HOME")?
        .join("rfm");

    // Create config files and config directory, if they are not present
    if !config_dir.exists() {
        info!("Creating config directory: {}", config_dir.display());
        std::fs::create_dir(&config_dir).context("failed to create config directory")?;
    }

    // --- Set or generate color configuration
    let general_config_file = config_dir.join("config.toml");
    if !general_config_file.exists() {
        info!("Creating default config file for config.toml");
        let default = Examples::get("config.toml").expect("embedded config.toml");
        let mut file = File::create(&general_config_file).context(format!(
            "failed to create {}",
            general_config_file.display()
        ))?;
        file.write_all(&default.data)?;
    }

    // Weather or not we activate the trash
    let mut use_trash = false;

    if let Ok(content) = std::fs::read_to_string(&general_config_file) {
        match toml::from_str::<config::Config>(&content) {
            Ok(config) => {
                info!("Using general config: {}", general_config_file.display());
                colors_from_config(config.colors)?;
                use_trash = config.general.use_trash;
            }
            Err(e) => {
                warn!("Configuration error: {e}. Using default color config");
                colors_from_default();
            }
        }
    } else {
        info!("Using default color config");
        colors_from_default();
    }

    // --- Keyboard configuration
    let key_config_file = config_dir.join("keys.toml");
    if !key_config_file.exists() {
        info!("Creating default config file for keys.toml");
        let default = Examples::get("keys.toml").expect("embedded keys.toml");
        let mut file = File::create(&key_config_file)
            .context(format!("failed to create {}", key_config_file.display()))?;
        file.write_all(&default.data)?;
    }

    let parser = if let Ok(content) = std::fs::read_to_string(&key_config_file) {
        match toml::from_str(&content) {
            Ok(key_config) => {
                info!("Using keyboard config: {}", key_config_file.display());
                CommandParser::from_config(key_config)
            }
            Err(e) => {
                warn!("Configuration error: {e}. Using default keyboard bindings");
                CommandParser::default_bindings()
            }
        }
    } else {
        warn!(
            "Cannot find keyboard config '{}'. Using default keyboard bindings",
            key_config_file.display()
        );
        CommandParser::default_bindings()
    };

    // --- Opener configuration
    let open_config_file = config_dir.join("open.toml");
    if !open_config_file.exists() {
        info!("Creating default config file for open.toml");
        let default = Examples::get("open.toml").expect("embedded open.toml");
        let mut file = File::create(&open_config_file)
            .context(format!("failed to create {}", open_config_file.display()))?;
        file.write_all(&default.data)?;
    }

    let opener = if let Ok(content) = std::fs::read_to_string(&open_config_file) {
        match toml::from_str(&content) {
            Ok(open_config) => {
                info!("Using open-engine config: {}", open_config_file.display());
                OpenEngine::with_config(open_config)
            }
            Err(e) => {
                warn!("Configuration error: {e}. Using default open engine");
                OpenEngine::default()
            }
        }
    } else {
        info!("Using default open engine");
        OpenEngine::default()
    };

    enable_raw_mode()?;

    stdout
        .queue(DisableMouseCapture)?
        .queue(DisableLineWrap)?
        .queue(cursor::SavePosition)?
        // NOTE: We move to the alternate screen,
        // to not mess with the current content of the terminal
        .queue(EnterAlternateScreen)?
        .queue(cursor::Hide)?
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?;

    SymbolEngine::init();

    let directory_cache = PanelCache::with_size(16384);
    let preview_cache = PanelCache::with_size(4096);

    let (dir_tx, dir_rx) = mpsc::channel(32);
    let (prev_tx, prev_rx) = mpsc::channel(32);

    let (preview_tx, preview_rx) = mpsc::unbounded_channel();
    let (directory_tx, directory_rx) = mpsc::unbounded_channel();

    let dir_manager = content::DirManager::new(
        directory_cache.clone(),
        preview_cache.clone(),
        dir_tx,
        directory_rx,
    );

    let preview_manager = content::PreviewManager::new(preview_cache.clone(), prev_tx, preview_rx);

    let dir_mngr_handle = tokio::spawn(dir_manager.run());
    let prev_mngr_handle = tokio::spawn(preview_manager.run());

    let miller_panels = init_miller_panels(
        starting_path.clone(),
        directory_cache,
        preview_cache,
        directory_tx,
        preview_tx,
    );

    let panel_manager = PanelManager::new(
        miller_panels,
        use_trash,
        parser,
        dir_rx,
        prev_rx,
        logger.clone(),
        opener,
    )?;
    let panel_handle = tokio::spawn(panel_manager.run());

    // If the panel manager returns, we essentially want to shutdown the entire program.
    let panel_result = panel_handle.await;

    // Stop all blocking tasks by setting the shutdown handle to "true":
    SHUTDOWN_FLAG.store(true, std::sync::atomic::Ordering::Relaxed);

    // The .await here is okay, because the PanelManager dropped the queue sender,
    // which makes these two guys instantly return:
    dir_mngr_handle.abort();
    prev_mngr_handle.abort();

    // Be a good citizen, cleanup
    stdout
        .queue(EnableLineWrap)?
        .queue(Clear(ClearType::All))?
        .queue(LeaveAlternateScreen)?
        .queue(cursor::RestorePosition)?
        .queue(cursor::Show)?
        .flush()?;
    disable_raw_mode()?;

    match panel_result {
        Ok(Ok(close_cmd)) => {
            if let CloseCmd::QuitErr { error } = &close_cmd {
                error!("{error}");
                print_all_errors(&logger)?;
                return Ok(());
            }
            if let Some(choosedir) = args.choosedir {
                if !choosedir.exists() {
                    eprintln!("Error: {} does not exist!", choosedir.display());
                } else if !choosedir.is_file() {
                    eprintln!("Error: {} is not a file!", choosedir.display());
                }
                if choosedir.exists() && choosedir.is_file() {
                    let path = match close_cmd {
                        CloseCmd::QuitWithPath { path } => path,
                        _ => starting_path,
                    };
                    // Write output to file
                    let mut file = OpenOptions::new()
                        .write(true)
                        .truncate(true) // FIX: Use existing choosedir file instead of tmpfile
                        .open(choosedir.canonicalize()?)?;
                    file.write_all(format!("{}", path.display()).as_bytes())?;
                }
            }
        }
        Ok(e) => {
            e.context("panel manager returned an error")?;
        }
        e => {
            e.context("error in panel-manager task")??;
        }
    }
    print_all_errors(&logger)?;
    Ok(())
}

fn print_all_errors(logger: &LogBuffer) -> anyhow::Result<()> {
    let errors = logger.get_errors();
    if !errors.is_empty() {
        // Write error.log
        let log_output: String = logger
            .get()
            .into_iter()
            .map(|(level, msg)| format!("{level}: {msg}\n"))
            .collect();
        let mut log = std::fs::File::create("./error.log").context("failed to create error log")?;
        log.write_all(log_output.as_bytes())
            .context("failed to write to error log")?;
        eprintln!("{}", ERROR_MSG);
        eprintln!("Error:");
        for e in errors {
            eprintln!("{e}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, engine::commands::KeyConfig, engine::opener::OpenerConfig};

    #[test]
    fn embedded_key_config() {
        let config = Examples::get("keys.toml");
        assert!(config.is_some(), "missing embedded keys.toml config");
        let config = config.unwrap();
        let content = std::str::from_utf8(&config.data).expect("config must be valid utf-8");
        let parsed: Result<KeyConfig, _> = toml::from_str(content);
        assert!(parsed.is_ok(), "invalid keys.toml example");
    }

    #[test]
    fn embedded_open_config() {
        let config = Examples::get("open.toml");
        assert!(config.is_some(), "missing embedded keys.toml config");
        let config = config.unwrap();
        let content = std::str::from_utf8(&config.data).expect("config must be valid utf-8");
        let parsed: Result<OpenerConfig, _> = toml::from_str(content);
        assert!(parsed.is_ok(), "invalid keys.toml example");
    }

    #[test]
    fn embedded_general_config() {
        let config = Examples::get("config.toml");
        assert!(config.is_some(), "missing embedded keys.toml config");
        let config = config.unwrap();
        let content = std::str::from_utf8(&config.data).expect("config must be valid utf-8");
        let parsed: Result<Config, _> = toml::from_str(content);
        assert!(parsed.is_ok(), "invalid keys.toml example");
    }
}
