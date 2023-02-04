#![allow(unused_imports)]
use crossterm::{
    cursor::{self, position},
    event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{self, PrintStyledContent, Stylize},
    terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType, SetSize},
    QueueableCommand, Result,
};
use futures::{future::FutureExt, StreamExt};
use panel::MillerPanels;
use std::{
    cmp::Ordering,
    fmt::Display,
    fs::{canonicalize, read_dir},
    io::{self, stdout, Stdout, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

mod panel;

#[derive(Debug)]
enum Movement {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug)]
enum Command {
    Move(Movement),
    Resize(u16, u16),
    Quit,
    None,
}

const CTRL_C: KeyEvent = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
const LEFT: KeyEvent = KeyEvent::new(KeyCode::Char('h'), KeyModifiers::empty());
const DOWN: KeyEvent = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
const RIGHT: KeyEvent = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty());
const UP: KeyEvent = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::empty());
const Q: KeyEvent = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());

fn parse_events(event: Event) -> Command {
    if event == Event::Key(UP) {
        return Command::Move(Movement::Up);
    }

    if event == Event::Key(DOWN) {
        return Command::Move(Movement::Down);
    }

    if event == Event::Key(LEFT) {
        return Command::Move(Movement::Left);
    }

    if event == Event::Key(RIGHT) {
        return Command::Move(Movement::Right);
    }

    if event == Event::Key(CTRL_C) || event == Event::Key(Q) {
        return Command::Quit;
    }

    if let Event::Resize(sx, sy) = event {
        return Command::Resize(sx, sy);
    }

    Command::None
}

#[tokio::main]
async fn main() -> Result<()> {
    // let (cols, rows) = terminal::size()?;
    enable_raw_mode()?;
    let mut stdout = stdout();
    // Start with a clear screen
    stdout
        .queue(cursor::Hide)?
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?
        .flush()?;

    let terminal_size = terminal::size()?;

    let mut reader = EventStream::new();
    let mut panels = MillerPanels::new(terminal_size)?;
    panels.draw(&mut stdout)?;

    // Flush buffer in the end
    stdout.flush()?;
    loop {
        let event_reader = reader.next().fuse();
        tokio::select! {
            maybe_event = event_reader => {
                let mut redraw = false;
                match maybe_event {
                    Some(Ok(event)) => {
                        // Change state based on the event
                        match parse_events(event) {
                            Command::Move(direction) => {
                                match direction {
                                    Movement::Up => {
                                        redraw = panels.up()?;
                                    }
                                    Movement::Down => {
                                        redraw = panels.down()?;
                                    }
                                    Movement::Left => {
                                        redraw = panels.left()?;
                                    }
                                    Movement::Right => {
                                        redraw = panels.right()?;
                                    }
                                }
                            }
                            Command::Resize(sx, sy) => {
                                panels.terminal_resize((sx, sy));
                                    redraw = true;
                            },
                            Command::Quit => break,
                            Command::None => (),
                        }
                        if redraw {
                            // selected_path = content_mid[position_mid].path().into();
                            stdout.queue(Clear(ClearType::All))?;
                            panels.draw(&mut stdout)?;
                            stdout.flush()?;
                        }
                    },
                    Some(Err(e)) => {
                        println!("Error: {e}\r");
                    }
                    None => break,
                }
            }
        }
    }
    stdout
        .queue(Clear(ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?
        .queue(cursor::Show)?
        .flush()?;

    // Be a good citizen, cleanup
    // execute!(stdout, SetSize(cols, rows))?;
    disable_raw_mode()
}
