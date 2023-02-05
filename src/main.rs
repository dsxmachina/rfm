#![allow(unused_imports)]
use crate::{
    commands::{Command, Movement},
    panel::MillerPanels,
};
use commands::CommandParser;
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

mod commands;
mod manager;
mod panel;

#[tokio::main]
async fn main() -> Result<()> {
    enable_raw_mode()?;

    let manager = PanelManager::new()?;
    if let Err(e) = manager.run().await {
        // Clear everything
        let mut stdout = stdout();
        stdout.execute(Clear(ClearType::All))?;
        // Print error
        eprintln!("Error: {e}");
    }

    // Be a good citizen, cleanup
    disable_raw_mode()
}
