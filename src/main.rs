#![allow(dead_code)]
use content::SharedCache;
use crossterm::{
    cursor,
    event::DisableMouseCapture,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType, DisableLineWrap},
    QueueableCommand, Result,
};
use panel::manager::PanelManager;
use std::io::stdout;
use tokio::sync::mpsc;

mod commands;
mod content;
// mod manager;
mod panel;

#[tokio::main]
async fn main() -> Result<()> {
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

    panel_handle.await??;
    content_handle.await?;

    // Be a good citizen, cleanup
    disable_raw_mode()
}
