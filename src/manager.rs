use std::io::{stdout, Stdout, Write};

use crossterm::{
    cursor,
    event::{Event, EventStream},
    terminal::{self, Clear, ClearType},
    QueueableCommand, Result,
};
use futures::{FutureExt, StreamExt};

use crate::{
    commands::{Command, CommandParser},
    panel::MillerPanels,
};

// Unifies the management of key-events,
// redrawing and querying content.
//
pub struct PanelManager {
    // Managed panels
    panels: MillerPanels,

    // Event-stream from the terminal
    event_reader: EventStream,

    // command-parser
    parser: CommandParser,

    // Handle to the standard-output
    stdout: Stdout,
}

impl PanelManager {
    pub fn new() -> Result<Self> {
        let mut stdout = stdout();
        // Start with a clear screen
        stdout
            .queue(cursor::Hide)?
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?;

        let terminal_size = terminal::size()?;
        let event_reader = EventStream::new();
        let parser = CommandParser::new();
        let panels = MillerPanels::new(terminal_size)?;
        panels.draw(&mut stdout)?;

        // Flush buffer in the end
        stdout.flush()?;

        Ok(PanelManager {
            panels,
            event_reader,
            parser,
            stdout,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                maybe_event = event_reader => {
                    let mut redraw = false;
                    match maybe_event {
                        Some(Ok(event)) => {
                            let command = match event {
                                Event::Key(key_event) => {
                                    self.parser.add_event(key_event)
                                },
                                Event::Resize(sx, sy) => {self.panels.terminal_resize((sx, sy)); redraw = true; Command::None }
                                _ => Command::None,
                            };

                            match command {
                                Command::Move(direction) => {
                                    redraw =  self.panels.move_cursor(direction)?;
                                }
                                Command::ToggleHidden => {
                                    redraw = self.panels.toggle_hidden()?;
                                }
                                Command::Quit => break,
                                Command::None => (),
                            }

                            if redraw {
                                // selected_path = content_mid[position_mid].path().into();
                                self.panels.draw(&mut self.stdout)?;
                                self.stdout.queue(cursor::Hide)?;
                                self.stdout.flush()?;
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
        // Cleanup after leaving this function
        self.stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?
            .queue(cursor::Show)?
            .flush()?;
        Ok(())
    }
}
