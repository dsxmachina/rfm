//! Widget wrappers for existing panel types.
//!
//! This module provides Widget trait implementations that wrap the existing
//! panel types (DirPanel, PreviewPanel) so they can be used with the Compositor.

mod panel_widget;
mod header;
mod footer;
mod input_bar;
mod log;

pub use panel_widget::{DirPanelWidget, PreviewPanelWidget};
pub use header::HeaderWidget;
pub use footer::FooterWidget;
pub use input_bar::{InputBarWidget, InputType};
pub use log::{LogWidget, LogEntry};
