#![allow(unused_imports)]
use crate::{
    commands::{Command, Movement},
    panel::MillerPanels,
};
use commands::CommandParser;
use content::SharedCache;
use crossterm::{
    cursor::{self, position},
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{self, PrintStyledContent, Stylize},
    terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType, SetSize},
    ExecutableCommand, QueueableCommand, Result,
};
use futures::{future::FutureExt, StreamExt};
use manager::PanelManager;
use std::{
    cmp::Ordering,
    fmt::Display,
    fs::{canonicalize, read_dir},
    io::{self, stdout, Stdout, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;
use tokio::sync::mpsc;

mod commands;
mod content;
mod manager;
// mod new_panel;
mod panel;
mod preview;

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;

    let cache = SharedCache::with_size(50);

    let (dir_tx, dir_rx) = mpsc::channel(32);
    let (preview_tx, preview_rx) = mpsc::channel(32);
    let (content_tx, content_rx) = mpsc::channel(32);

    let content_manager = content::Manager::new(cache.clone(), content_rx, dir_tx, preview_tx);
    let content_handle = tokio::spawn(content_manager.run());

    let panel_manager = PanelManager::new(cache, dir_rx, preview_rx, content_tx)?;
    let panel_handle = tokio::spawn(panel_manager.run());

    panel_handle.await??;
    content_handle.await?;

    // Be a good citizen, cleanup
    disable_raw_mode()
}
