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
    OpenEngine, StyleEngine,
};
use log::{error, info, warn};
use logger::LogBuffer;
use panel::{manager::PanelManager, ContentHandles};
use std::{
    fs::OpenOptions,
    io::{stdout, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use util::xdg_config_home;

use crate::config::color::colors_from_config;

mod command_queue;
mod config;
mod content;
mod debug;
mod engine;
mod logger;
mod panel;
mod undo;
mod util;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Override default config dir (XDG_CONFIG_HOME)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Makes rfm act like a diretory chooser. Upon quitting
    /// it will write the full path of the last visited directory to CHOOSEDIR
    #[arg(long)]
    choosedir: Option<PathBuf>,
    /// Expose a debug/testing socket at the given path.
    ///
    /// Line-based protocol for development and automated testing:
    /// send "state", "await-idle" or "entries <pane>", receive one
    /// line of JSON. Example: echo state | socat - UNIX-CONNECT:<path>
    #[arg(long)]
    debug_socket: Option<PathBuf>,
    /// Print the complete annotated default configuration and exit
    #[arg(long)]
    dump_config: bool,
    /// Unify the configuration into one minimal config.toml (folding legacy
    /// keys.toml/open.toml in, renaming them to *.bak) and exit
    #[arg(long)]
    migrate_config: bool,
    /// Path to open (defaults to ".")
    path: Option<PathBuf>,
}

/// The config directory: `--config` wins, else `$XDG_CONFIG_HOME/rfm`.
/// Resolution only — creating the directory is the caller's business.
fn resolve_config_dir(cli_override: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match cli_override {
        Some(dir) => Ok(dir),
        None => Ok(xdg_config_home()
            .context("failed to get $XDG_CONFIG_HOME")?
            .join("rfm")),
    }
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.dump_config {
        print!("{}", config::default_config_str());
        return Ok(());
    }

    if args.migrate_config {
        let config_dir = resolve_config_dir(args.config)?;
        for line in config::load::migrate(&config_dir)? {
            println!("{line}");
        }
        return Ok(());
    }

    // Check if we run from a terminal
    let mut stdout = stdout();
    if !stdout.is_terminal() {
        eprintln!("Error: Stdout handle does not refer to a terminal/tty");
        eprintln!();
        eprintln!("Please note: The output of rfm can be neither piped nor redirected.");
        std::process::exit(1);
    }

    std::panic::set_hook(Box::new(|panic_info| {
        if undo::is_guarding_trash() {
            // A trash-crate assert we deliberately contain in guard_trash — keep
            // the raw assertion out of the visible ERROR log.
            log::debug!("contained trash-operation panic: {panic_info}");
        } else {
            error!("{panic_info}");
        }
    }));

    // Remember starting path
    let starting_path = if let Some(path) = args.path {
        path
    } else {
        std::env::current_dir().context("failed to get current directory from env")?
    };

    // Initialize logger
    //
    // With an active debug socket everything down to trace level is logged:
    // the verbose detail lands in the retention history (readable via the
    // `log` socket command) while the log widget still only shows Info+
    let (log_level, log_filter) = if args.debug_socket.is_some() {
        (log::Level::Trace, log::LevelFilter::Trace)
    } else {
        (log::Level::Debug, log::LevelFilter::Info)
    };
    let logger = LogBuffer::default().with_level(log_level).with_capacity(15);
    log::set_boxed_logger(Box::new(logger.clone())).context("failed to initialize logger")?;
    log::set_max_level(log_filter);

    // Spawn a task that periodically expires stale display log lines
    //
    // Each display line carries an Instant; once it is older than DISPLAY_TTL
    // it is dropped and the UI is woken (only when something was removed), so
    // an expired line disappears on its own rather than lingering until the
    // next keypress.
    let periodic_logger = logger.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            periodic_logger.remove_expired(Instant::now());
        }
    });

    // --- Read config directory
    let config_dir = resolve_config_dir(args.config)?;

    // Create the config directory, if it is not present
    if !config_dir.exists() {
        info!("Creating config directory: {}", config_dir.display());
        std::fs::create_dir(&config_dir).context("failed to create config directory")?;
    }

    // --- Load the configuration: sparse config.toml over the embedded
    // defaults, with legacy keys.toml/open.toml folded in (load() writes the
    // config.toml stub on first run).
    let loaded = config::load::load(&config_dir);
    for w in &loaded.warnings {
        warn!("{w}");
    }
    colors_from_config(loaded.config.colors)?;

    let use_trash = loaded.config.general.use_trash;
    let rate_limit_interval_ms = loaded.config.general.rate_limit_interval_ms;
    let fancy_icons = loaded.config.general.fancy_icons;
    info!("Using rate-limit of {rate_limit_interval_ms}ms");
    if fancy_icons {
        info!("Using Nerd Font icons");
    }
    if !loaded.config.commands.is_empty() {
        info!("Loaded {} user commands", loaded.config.commands.len());
    }

    let (parser, dropped) = CommandParser::build(
        &loaded.default_keys,
        &loaded.parser_input,
        &loaded.config.commands,
    );
    for d in &dropped {
        if d.from_user {
            warn!(
                "default `{}` → {} skipped: bound to {} in your config",
                d.binding, d.command, d.kept
            );
        } else {
            warn!(
                "default `{}` → {} skipped: `{}` already uses it",
                d.binding, d.command, d.kept
            );
        }
    }

    let opener = OpenEngine::with_config(loaded.config.open);

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

    StyleEngine::init_with_config(&loaded.config.styles, fancy_icons);

    let directory_cache = PanelCache::with_size(16384);
    let preview_cache = PanelCache::with_size(4096);

    // Channels from managers to PanelManager (bounded)
    let (dir_tx, dir_rx) = mpsc::channel(32);
    let (prev_tx, prev_rx) = mpsc::channel(32);

    // Channels from panels to managers (unbounded, rate-limited inside managers)
    let (directory_tx, directory_rx) = mpsc::unbounded_channel();
    let (preview_tx, preview_rx) = mpsc::unbounded_channel();

    let rate_limit_interval = Duration::from_millis(rate_limit_interval_ms);

    let dir_manager = content::DirManager::new(
        directory_cache.clone(),
        preview_cache.clone(),
        dir_tx,
        directory_rx,
        rate_limit_interval,
    );

    let preview_manager = content::PreviewManager::new(
        preview_cache.clone(),
        prev_tx,
        preview_rx,
        rate_limit_interval,
    );

    let dir_mngr_handle = tokio::spawn(dir_manager.run());
    let prev_mngr_handle = tokio::spawn(preview_manager.run());

    // Create command executor for background shell commands
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (command_status_tx, command_status_rx) =
        tokio::sync::watch::channel(command_queue::QueueStatus::default());
    let command_executor = command_queue::CommandExecutor::new(command_rx, command_status_tx);
    let cmd_exec_handle = tokio::spawn(command_executor.run());

    // Debug socket (only with --debug-socket)
    let debug_rx = if let Some(socket_path) = args.debug_socket.clone() {
        let (debug_tx, debug_rx) = mpsc::channel(8);
        tokio::spawn(debug::serve(socket_path, debug_tx));
        Some(debug_rx)
    } else {
        None
    };

    let handles = ContentHandles {
        directory_cache,
        preview_cache,
        directory_tx,
        preview_tx,
    };

    let panel_manager = PanelManager::new(
        starting_path.clone(),
        handles,
        use_trash,
        parser,
        dir_rx,
        prev_rx,
        logger.clone(),
        opener,
        command_tx,
        command_status_rx,
        debug_rx,
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
    cmd_exec_handle.abort();

    // Be a good citizen, cleanup
    stdout
        .queue(EnableLineWrap)?
        .queue(Clear(ClearType::All))?
        .queue(LeaveAlternateScreen)?
        .queue(cursor::RestorePosition)?
        .queue(cursor::Show)?
        .flush()?;
    disable_raw_mode()?;

    if let Some(socket_path) = &args.debug_socket {
        let _ = std::fs::remove_file(socket_path);
    }

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
        // Write error.log from the retained history (not just the short-lived
        // display buffer), so the report survives the periodic log eviction
        let now = std::time::Instant::now();
        let log_output: String = logger
            .history(logger::HISTORY_CAPACITY)
            .into_iter()
            .map(|(level, at, msg)| {
                let age = now.duration_since(at).as_secs();
                format!("{level} ({age}s ago): {msg}\n")
            })
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
    use crate::{
        config::{Config, Examples},
        engine::commands::KeyConfig,
        engine::opener::OpenerConfig,
    };

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
