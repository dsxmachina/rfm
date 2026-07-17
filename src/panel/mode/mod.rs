//! The modal-mode seam: every modal mode implements [`ModalInput`] —
//! keys in, [`ModeOp`]s out, self-drawn in its assigned [`ModalRegion`].
//! Adapters are pure state machines: they never touch panels, the
//! filesystem, or redraw flags. The PanelManager applies ops centrally.

use std::io::Stdout;
use std::ops::Range;
use std::path::PathBuf;

use crossterm::{
    cursor,
    event::KeyEvent,
    style::{Color, Print, PrintStyledContent, Stylize},
    QueueableCommand, Result,
};

use crate::config::color::color_main;
use crate::panel::input::Input;

use super::Draw;

mod create_item;
mod rename;
mod search;
pub use create_item::CreateItemMode;
pub use rename::RenameMode;
pub use search::SearchMode;

/// Draws the shared FooterLine prompt: a reversed label in the main
/// color, a space, then the live input in the mode's input color.
pub(crate) fn draw_footer_prompt(
    stdout: &mut Stdout,
    x_range: Range<u16>,
    y_range: Range<u16>,
    label: &str,
    input: &Input,
    color: Color,
) -> Result<()> {
    stdout
        .queue(cursor::MoveTo(x_range.start, y_range.start))?
        .queue(PrintStyledContent(
            label.bold().with(color_main()).reverse(),
        ))?
        .queue(Print(" "))?;
    input.print(stdout, color)
}

/// The effect a modal mode requests from the application.
///
/// Ops that conclude the mode (FinishSearch, Rename, Create, Exit) make
/// the manager return to Normal after applying them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeOp {
    /// Nothing to apply
    None,
    /// Change the visible directory (consoles navigate live)
    Cd(PathBuf),
    /// Live-update the center panel's search highlight
    UpdateSearch(String),
    /// Conclude the search with the final pattern and leave the mode
    FinishSearch(String),
    /// Live-preview the rename target name
    RenamePreview(String),
    /// Perform the rename and leave the mode
    Rename { to: String },
    /// Live-preview the element being created (footer shows it)
    CreatePreview { name: String, is_dir: bool },
    /// Create the element and leave the mode
    Create { name: String, is_dir: bool },
    /// Leave the mode without a concluding action; cleanup says what to undo
    Exit { cleanup: Cleanup },
}

/// What the manager must undo when a modal mode is cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cleanup {
    None,
    /// Clear the search highlight
    Search,
    /// Clear the rename preview
    RenamePreview,
    /// Clear the new-element preview
    CreatePreview,
    /// Navigate back (consoles cancel to their starting directory)
    CdTo(PathBuf),
}

/// Screen area a modal mode is granted for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalRegion {
    /// Centered overlay (consoles)
    ConsoleOverlay,
    /// Single input line in the footer (search/rename/create)
    FooterLine,
}

/// The interface every modal mode implements.
pub trait ModalInput: Draw + Send + Sync {
    /// Handle one key event, returning the op the manager should apply.
    /// Esc arrives here like any other key — cancel semantics belong to
    /// the mode, not the manager.
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp;

    /// Where this modal wants to render.
    fn region(&self) -> ModalRegion;

    /// Mode string for the debug socket / state snapshot.
    fn name(&self) -> &'static str;
}
