#![allow(dead_code)]
use clap::Parser;
use content::PanelCache;
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
