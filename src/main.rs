#![allow(dead_code)]
use clap::Parser;
use commands::CommandParser;
use content::PanelCache;
use crossterm::{
    cursor,
    event::DisableMouseCapture,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap},
    QueueableCommand, Result,
};
use logger::LogBuffer;
use notify_rust::Notification;
use panel::manager::PanelManager;
use std::{
    fs::OpenOptions,
    io::{stdout, Write},
    path::PathBuf,
};
use tokio::sync::mpsc;

mod commands;
mod content;
mod logger;
mod opener;
mod panel;
mod util;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Makes rfm act like a diretory chooser. Upon quitting
    /// it will write the full path of the last visited directory to CHOOSEDIR
    #[arg(long)]
    choosedir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    std::panic::set_hook(Box::new(|panic_info| {
        let body;
        let summary;
        if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            body = format!("panic occurred: {s:?}");
        } else {
            body = "panic occurred".to_string();
        }
        if let Some(location) = panic_info.location() {
            summary = format!(
                "panic occurred in file '{}' at line {}",
                location.file(),
                location.line(),
            );
        } else {
            summary = "panic occurred somewhere".to_string();
        }
        if Notification::new()
            .summary(&summary)
            .body(&body)
            .show()
            .is_err()
        {
            eprintln!("{summary}: {body}");
        }
    }));

    enable_raw_mode()?;

    // Initialize terminal
    let mut stdout = stdout();
    stdout
        .queue(DisableMouseCapture)?
        .queue(DisableLineWrap)?
        .queue(cursor::SavePosition)?
        .queue(cursor::Hide)?
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?;

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

    // Read config file
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let config_dir = home.join(".config/rfm/");
    let key_config_file = config_dir.join("keys.toml");

    let parser: CommandParser;
    if let Ok(content) = std::fs::read_to_string(key_config_file) {
        let key_config = toml::from_str(&content).unwrap();
        parser = CommandParser::from_config(key_config);
    } else {
        parser = CommandParser::default_bindings();
    }

    let panel_manager = PanelManager::new(
        parser,
        directory_cache,
        preview_cache,
        dir_rx,
        prev_rx,
        directory_tx,
        preview_tx,
    )?;
    let panel_handle = tokio::spawn(panel_manager.run());

    let panel_result = panel_handle.await;
    let dir_mngr_result = dir_mngr_handle.await;
    let prev_mngr_result = prev_mngr_handle.await;

    // Be a good citizen, cleanup
    stdout
        .queue(Clear(ClearType::Purge))?
        .queue(cursor::MoveTo(0, 0))?
        .queue(cursor::Show)?
        .flush()?;
    disable_raw_mode()?;

    match panel_result {
        Ok(Ok(path)) => {
            if let Some(choosedir) = args.choosedir {
                if !choosedir.exists() {
                    eprintln!("Error: {} does not exist!", choosedir.display());
                } else if !choosedir.is_file() {
                    eprintln!("Error: {} is not a file!", choosedir.display());
                }
                if choosedir.exists() && choosedir.is_file() {
                    // Write output to file
                    let mut file = OpenOptions::new()
                        .write(true)
                        .open(choosedir.canonicalize()?)?;
                    file.write_all(format!("{}", path.display()).as_bytes())?;
                }
            }
        }
        Ok(Err(e)) => eprintln!("{e}"),
        Err(e) => eprintln!("Error in panel-task: {e}"),
    }
    if let Err(e) = dir_mngr_result {
        eprintln!("Error in dir-mngr-task: {e}");
    }
    if let Err(e) = prev_mngr_result {
        eprintln!("Error in preview-mngr-task: {e}");
    }
    Ok(())
}
