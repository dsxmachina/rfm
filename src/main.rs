use clap::Parser;
use commands::{CloseCmd, CommandParser};
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
use log::{error, info, warn};
use logger::LogBuffer;
use notify_rust::Notification;
use opener::OpenEngine;
use panel::manager::PanelManager;
use rust_embed::Embed;
use std::{
    error::Error,
    fs::{File, OpenOptions},
    io::{stdout, IsTerminal, Write},
    path::PathBuf,
};
use symbols::SymbolEngine;
use tokio::sync::mpsc;
use util::xdg_config_home;

mod commands;
mod content;
mod logger;
mod opener;
mod panel;
mod symbols;
mod util;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Makes rfm act like a diretory chooser. Upon quitting
    /// it will write the full path of the last visited directory to CHOOSEDIR
    #[arg(long)]
    choosedir: Option<PathBuf>,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
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
        let output = format!("{panic_info}");
        let summary = "panic occured";
        if Notification::new()
            .summary(summary)
            .body(&output)
            .show()
            .is_err()
        {
            warn!("failed to generate notification");
        }
    }));

    // Remember starting path
    let starting_path = std::env::current_dir()?;

    // Initialize logger
    let logger = LogBuffer::default()
        .with_level(log::Level::Debug)
        .with_capacity(15);
    log::set_boxed_logger(Box::new(logger.clone())).expect("failed to initialize logger");
    log::set_max_level(log::LevelFilter::Info);

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

    // Read keybinding config
    let config_dir = xdg_config_home()?.join("rfm");

    // Create config files and config directory, if they are not present
    if !config_dir.exists() {
        info!("Creating config directory: {}", config_dir.display());
        std::fs::create_dir(&config_dir)?;
    }

    let key_config_file = config_dir.join("keys.toml");
    if !key_config_file.exists() {
        info!("Creating default config file for keys.toml");
        let default = Examples::get("keys.toml").expect("embedded keys.toml");
        let mut file = File::create(&key_config_file)?;
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

    // Read opener config
    let open_config_file = config_dir.join("open.toml");
    if !open_config_file.exists() {
        info!("Creating default config file for open.toml");
        let default = Examples::get("open.toml").expect("embedded open.toml");
        let mut file = File::create(&open_config_file)?;
        file.write_all(&default.data)?;
    }

    let opener = if let Ok(content) = std::fs::read_to_string(&open_config_file) {
        match toml::from_str(&content) {
            Ok(open_config) => {
                info!("Using open-engine config: {}", open_config_file.display());
                OpenEngine::with_config(open_config)
            }
            Err(e) => {
                if Notification::new()
                    .summary("Configuration Error")
                    .body(&format!("{e}"))
                    .show()
                    .is_err()
                {
                    warn!("failed to generate notification");
                }
                warn!("Configuration error: {e}. Using default open engine");
                OpenEngine::default()
            }
        }
    } else {
        info!("Using default open engine");
        OpenEngine::default()
    };

    let panel_manager = PanelManager::new(
        parser,
        directory_cache,
        preview_cache,
        dir_rx,
        prev_rx,
        directory_tx,
        preview_tx,
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
    let dir_mngr_result = dir_mngr_handle.await;
    let prev_mngr_result = prev_mngr_handle.await;

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
                eprintln!("{}", ERROR_MSG);
                eprintln!("{error}");
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
        Ok(Err(e)) => {
            error!("PanelManager returned an error: {e}");
        }
        Err(e) => {
            error!("PanelManager-task: {e}")
        }
    }
    if let Err(e) = dir_mngr_result {
        error!("dir-mngr-task: {e}");
    }
    if let Err(e) = prev_mngr_result {
        error!("preview-mngr-task: {e}");
    }
    // Print all errors
    let errors = logger.get_errors();
    if !errors.is_empty() {
        // Write error.log
        let log_output: String = logger
            .get()
            .into_iter()
            .map(|(level, msg)| format!("{level}: {msg}\n"))
            .collect();
        let mut log = std::fs::File::create("./error.log")?;
        log.write_all(log_output.as_bytes())?;
        eprintln!("{}", ERROR_MSG);
        eprintln!("Error:");
        for e in errors {
            eprintln!("{e}");
        }
    }
    Ok(())
}

#[test]
fn embedded_key_config() {
    assert!(Examples::get("keys.toml").is_some());
}

#[test]
fn embedded_open_config() {
    assert!(Examples::get("open.toml").is_some());
}
