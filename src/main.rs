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
use std::{
    cmp::Ordering,
    fmt::Display,
    fs::{canonicalize, read_dir},
    io::{self, stdout, Stdout, Write},
    path::{Path, PathBuf},
};
use thiserror::Error;

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

    Command::None
}

#[derive(Debug, Clone, PartialEq, Eq, Ord)]
struct DirElem {
    name: String,
    path: PathBuf,
}

impl DirElem {
    pub fn name(&self) -> &String {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl<P: AsRef<Path>> From<P> for DirElem {
    fn from(path: P) -> Self {
        let path: PathBuf = path.as_ref().into();
        let name: String = path
            .file_name()
            .map(|p| p.to_str())
            .flatten()
            .map(|s| s.into())
            .unwrap_or_default();
        DirElem { path, name }
    }
}

impl AsRef<DirElem> for DirElem {
    fn as_ref(&self) -> &DirElem {
        &self
    }
}

impl PartialOrd for DirElem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self.path.is_dir() {
            if other.path.is_dir() {
                return self.name().partial_cmp(other.name());
            } else {
                return Some(Ordering::Less);
            }
        } else {
            if other.path.is_dir() {
                return Some(Ordering::Greater);
            } else {
                return self.name().partial_cmp(other.name());
            }
        }
    }
}

async fn directory_content<P: AsRef<Path>>(path: P) -> Result<Vec<DirElem>> {
    // read directory
    let dir = read_dir(path)?;
    let mut out = Vec::new();
    for item in dir {
        out.push(DirElem::from(item?.path()))
    }
    out.sort();
    Ok(out)
}

fn print_dir_elem<Elem: AsRef<DirElem>>(
    dir_elem: Elem,
    selected: bool,
    max_x: u16,
) -> PrintStyledContent<String> {
    let entry = dir_elem.as_ref();
    let mut name = format!(" {}", entry.name());
    name.truncate(usize::from(max_x));
    if entry.path().is_dir() {
        if selected {
            PrintStyledContent(name.dark_green().bold().negative())
        } else {
            PrintStyledContent(name.dark_green().bold())
        }
    } else {
        if selected {
            PrintStyledContent(name.grey().negative().bold())
        } else {
            PrintStyledContent(name.grey())
        }
    }
}

fn print_panel<Iter, D>(
    stdout: &mut Stdout,
    content: Iter,
    position: usize,
    x: u16,
    max_x: u16,
    max_y: u16,
) -> Result<()>
where
    Iter: Iterator<Item = D>,
    D: AsRef<DirElem>,
{
    let max_len = max_x.saturating_sub(x);
    // Then print new buffer
    let mut idx = 0u16;
    // Write items
    for item in content.take(max_y as usize) {
        let entry = item.as_ref();
        let y = u16::try_from(idx + 1).unwrap_or_else(|_| u16::MAX);
        queue!(
            stdout,
            cursor::MoveTo(x, y),
            PrintStyledContent("|".dark_green().bold()),
            print_dir_elem(entry, usize::from(idx) == position, max_len)
        )?;
        idx += 1;
    }
    for y in idx..max_y {
        queue!(
            stdout,
            cursor::MoveTo(x, y),
            PrintStyledContent("|".dark_green().bold()),
        )?;
    }
    // Draw column
    Ok(())
}

fn print_header<P: AsRef<Path>>(stdout: &mut Stdout, path: P) -> Result<()> {
    let prompt = format!("{}@{}", whoami::username(), whoami::hostname());
    let absolute = canonicalize(path.as_ref())?;
    let file_name = absolute
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or_default();
    let absolute = absolute.to_str().unwrap_or_default();

    let (prefix, suffix) = absolute.split_at(absolute.len() - file_name.len());

    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        style::PrintStyledContent(prompt.dark_green().bold()),
        style::Print(" "),
        style::PrintStyledContent(prefix.to_string().dark_blue().bold()),
        style::PrintStyledContent(suffix.to_string().white().bold()),
    )?;
    Ok(())
}

// TODO: Write a function that creates a preview for files
fn print_preview(stdout: &mut Stdout, x: u16, max_y: u16) -> Result<()> {
    for y in 1..max_y {
        queue!(
            stdout,
            cursor::MoveTo(x, y),
            PrintStyledContent("|".dark_green().bold()),
        )?;
    }
    // Draw column
    Ok(())
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

    let mut reader = EventStream::new();

    // TODO Initialize position_left correctly
    let mut position_left: usize = 0;
    let mut position_mid: usize = 0;

    let mut content_left = directory_content("..").await?;
    let mut content_mid = directory_content(".").await?;

    // TODO Handle empty directories
    let mut selected_path: PathBuf = content_mid[position_mid].path().into();
    // TODO: Add ancestor_path - this would solve a few problems

    let mut content_right = directory_content(&selected_path).await?;

    stdout.queue(Clear(ClearType::All))?;
    print_header(&mut stdout, &selected_path)?;

    let (sx, sy) = terminal::size()?;
    let x0 = 1;
    let x1 = (1 * sx / 8).saturating_sub(1);
    let x2 = (sx / 2).saturating_sub(1);
    print_panel(&mut stdout, content_left.iter(), position_left, x0, x1, sy)?;
    print_panel(&mut stdout, content_mid.iter(), position_mid, x1, x2, sy)?;
    print_panel(&mut stdout, content_right.iter(), 0, x2, sx, sy)?;

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
                                        if position_mid > 0 {
                                            position_mid -= 1;
                                            redraw = true;
                                        }
                                    }
                                    Movement::Down => {
                                        if position_mid < content_mid.len().saturating_sub(1) {
                                            position_mid += 1;
                                            redraw = true;
                                        }
                                    }
                                    Movement::Left => {
                                        // TODO: Check if we can go a directory up
                                        content_right = content_mid.clone();
                                        content_mid = content_left.clone();
                                        position_mid = position_left;
                                        content_left = directory_content(selected_path.join("..")).await?;
                                        position_left = 0; // TODO: initialize this correctly

                                        redraw = true;
                                    }
                                    Movement::Right => {
                                        if selected_path.is_dir() {
                                            // NOTE: Swap this instead of cloning
                                            content_left = content_mid.clone();
                                            position_left = position_mid;

                                            content_mid = content_right.clone();
                                            position_mid = 0;
                                            redraw = true;
                                        } else {
                                            // TODO: Open file
                                        }
                                    }
                                }
                            }
                            Command::Quit => break,
                            Command::None => (),
                        }
                        if redraw {
                            selected_path = content_mid[position_mid].path().into();
                            stdout.queue(Clear(ClearType::All))?;
                            print_header(&mut stdout, &selected_path)?;
                            let (sx, sy) = terminal::size()?;
                            let x0 = 1;
                            let x1 = (1 * sx / 8).saturating_sub(1);
                            let x2 = (sx / 2).saturating_sub(1);
                            print_panel(&mut stdout, content_left.iter(), position_left, x0, x1, sy)?;
                            print_panel(&mut stdout, content_mid.iter(), position_mid, x1, x2, sy)?;
                            if selected_path.is_dir() {
                                content_right = directory_content(&selected_path).await?;
                                print_panel(&mut stdout, content_right.iter(), 0, x2, sx, sy)?;
                            } else {
                                print_preview(&mut stdout, x2, sy)?;
                            }
                            // Flush buffer in the end
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
