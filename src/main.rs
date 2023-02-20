#![allow(dead_code)]
use clap::Parser;
use content::SharedCache;
use crossterm::{
    cursor,
    event::DisableMouseCapture,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap},
    QueueableCommand, Result,
};
use panel::manager::PanelManager;
use std::{
    fs::OpenOptions,
    io::{stdout, Write},
    path::PathBuf,
};
use tokio::sync::mpsc;

mod commands;
mod content;
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

    enable_raw_mode()?;

    // Initialize terminal
    let mut stdout = stdout();
    stdout
        .queue(DisableMouseCapture)?
        .queue(DisableLineWrap)?
        .queue(cursor::Hide)?
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?;

    let directory_cache = SharedCache::with_size(50);
    let preview_cache = SharedCache::with_size(50);

    let (dir_tx, dir_rx) = mpsc::channel(32);
    let (prev_tx, prev_rx) = mpsc::channel(32);

    let (preview_tx, preview_rx) = mpsc::unbounded_channel();
    let (directory_tx, directory_rx) = mpsc::unbounded_channel();

    let content_manager = content::Manager::new(
        directory_cache.clone(),
        preview_cache.clone(),
        directory_rx,
        preview_rx,
        dir_tx,
        prev_tx,
    );

    let content_handle = tokio::spawn(content_manager.run());

    let panel_manager = PanelManager::new(
        directory_cache,
        preview_cache,
        dir_rx,
        prev_rx,
        directory_tx,
        preview_tx,
    );
    let panel_handle = tokio::spawn(panel_manager.run());

    let panel_result = panel_handle.await;
    let content_result = content_handle.await;

    // Be a good citizen, cleanup
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
        Err(e) => eprintln!("{e}"),
    }
    match content_result {
        Err(e) => eprintln!("{e}"),
        _ => (),
    }
    Ok(())
}
