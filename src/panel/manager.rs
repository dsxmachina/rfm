use std::fs::OpenOptions;

use crossterm::{
    event::{Event, EventStream, KeyCode},
    style::PrintStyledContent,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
    ExecutableCommand,
};
use futures::{FutureExt, StreamExt};
use log::{debug, error, info, trace, warn, Level};
use tokio::sync::watch;

use crate::{
    command_queue::{zoxide_add_dir, QueueStatus, QueuedCommand},
    config::color::{color_dir_path, color_main},
    debug::{ClipboardInfo, DebugRequest, EntryInfo, LogEntry, PaneId, StateSnapshot, TabSnapshot},
    engine::commands::{CloseCmd, Command, CommandParser},
    engine::OpenEngine,
    logger::LogBuffer,
    undo::{capture_trashed, FsChange, Transaction, UndoOutcome, UndoStack},
    util::{copy_item, move_item, print_metadata, rename_safe},
};

/// Expand the command template by replacing $@ with the given paths
fn expand_command(cmd: &str, paths: &[PathBuf], separator: &str) -> String {
    let paths_str: String = paths
        .iter()
        .map(|p| shell_escape::escape(p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(separator);

    cmd.replace("$@", &paths_str)
}

/// Receive a debug request, or stay pending forever when debug mode is off.
async fn recv_debug(rx: &mut Option<mpsc::Receiver<DebugRequest>>) -> Option<DebugRequest> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

use super::mode::{
    Cleanup, CreateItemMode, DirConsole, ModalInput, ModalRegion, ModeOp, RenameMode, SearchMode,
    TrashEntry, TrashView, Zoxide,
};
use super::*;

enum Mode {
    Normal,
    Modal(Box<dyn ModalInput>),
}

struct Clipboard {
    /// Items we put into the clipboard
    files: Vec<PathBuf>,
    /// Weather or not we want to cut or copy the items.
    ///
    /// `True`  : Cut
    /// `False` : Copy
    cut: bool,
}

// enum Operation {
//     MoveItems { from: Vec<PathBuf>, to: PathBuf },
//     CopyItems { from: Vec<PathBuf>, to: PathBuf },
//     Mkdir { path: PathBuf },
//     Move(Movement),
// }

/// A session-only vim-style jump-mark: a directory plus the entry that was
/// highlighted there when the mark was set.
#[derive(Debug, Clone)]
struct JumpMark {
    dir: PathBuf,
    entry: Option<PathBuf>,
}

/// Upper bound on the number of tabs. NOTE: the `focus_tab_1..4` key bindings
/// in `commands.rs` mirror this value — to change it, update both.
const MAX_TABS: usize = 4;

/// How the screen is laid out. `Single` renders the focused tab's full Miller
/// stack (left|center|right); `Split` renders only the `center` column of two
/// adjacent tabs side by side. Closing down to one tab resets to `Single`.
enum ViewMode {
    Single,
    Split,
}

/// Pure index math for tab focus, extracted so it can be unit-tested without
/// standing up a whole [`PanelManager`]. The methods below call these so the
/// tested logic *is* the shipped logic.
///
/// Next focus index after cycling forward through `len` tabs.
fn next_focus(focused: usize, len: usize) -> usize {
    (focused + 1) % len
}

/// Focus index to land on after removing the currently-focused tab, given the
/// number of tabs remaining. Clamps into the shrunk range (removing the last
/// tab steps focus back one; removing an earlier one keeps the same index).
fn focus_after_close(focused: usize, len_after_removal: usize) -> usize {
    focused.min(len_after_removal.saturating_sub(1))
}

/// Whether another tab may be opened without exceeding [`MAX_TABS`].
fn can_add_tab(len: usize) -> bool {
    len < MAX_TABS
}

/// One independent Miller-columns stack: its own cwd, selection and
/// navigation history. Tabs are the unit the split view shows two of.
struct Tab {
    left: ManagedPanel<DirPanel>,
    center: ManagedPanel<DirPanel>,
    right: ManagedPanel<PreviewPanel>,
    fwd_history: Vec<(PathBuf, PathBuf)>,
    rev_history: Vec<PathBuf>,
    previous: PathBuf,
}

/// Outcome of [`Tab::move_right`], describing the manager-level side-effects
/// the caller must still apply. The pure panel/history mechanics have already
/// happened inside the method; this only carries what `PanelManager` needs to
/// touch its own (non-tab-local) state.
enum MoveRight {
    /// Nothing was selected; the manager should do nothing.
    None,
    /// Descended into the carried directory. The manager should record it with
    /// zoxide and unmark the (now shifted) left/right panels.
    Descended(PathBuf),
    /// The selection was a file, not a directory. The manager should set the
    /// process cwd to `cwd` and open `file` with its opener, then unmark.
    OpenFile { file: PathBuf, cwd: PathBuf },
}

impl Tab {
    /// Drives the `right`/preview panel to match the current center selection.
    ///
    /// This is the single place that turns "center selection" into a preview
    /// load; the navigation methods call it (gated on `drive_preview`) and the
    /// manager also calls it directly to *refresh* a tab's preview when it
    /// becomes visible again after preview-driving was skipped (split view, or
    /// a background single-view tab). `new_panel_delayed` is a no-op when the
    /// path is unchanged, so an extra refresh is cheap and safe.
    fn refresh_preview(&mut self) {
        let selected = self.center.panel().selected_path().map(|p| p.to_path_buf());
        self.right.new_panel_delayed(selected.as_deref());
    }

    /// Moves the selection cursor up by `step`, refreshing the preview (when
    /// `drive_preview`) and clearing the reverse-history when it actually moves.
    ///
    /// `drive_preview` is false in split view, where no preview is drawn — the
    /// manager refreshes it on the way back to single view instead of decoding
    /// previews that are never shown.
    ///
    /// Returns `true` if the cursor moved (so the manager can `mark_dirty`).
    fn move_up(&mut self, step: usize, drive_preview: bool) -> bool {
        if self.center.panel_mut().up(step) {
            if drive_preview {
                self.refresh_preview();
            }
            self.rev_history.clear();
            true
        } else {
            false
        }
    }

    /// Moves the selection cursor down by `step`. See [`Tab::move_up`].
    fn move_down(&mut self, step: usize, drive_preview: bool) -> bool {
        if self.center.panel_mut().down(step) {
            if drive_preview {
                self.refresh_preview();
            }
            self.rev_history.clear();
            true
        } else {
            false
        }
    }

    /// Descends into the selected entry (if it is a directory) or reports that
    /// the selection is a file to open. Shifts the Miller columns left and
    /// maintains the forward/reverse history.
    ///
    /// `drive_preview` gates only the final preview load: the Miller-column
    /// shift (which updates `center` — visible in split) always happens, so
    /// split freshness is preserved; only the never-drawn preview decode is
    /// skipped in split view.
    fn move_right(&mut self, drive_preview: bool) -> MoveRight {
        let selected = self.center.panel().selected_path().map(|p| p.to_path_buf());
        let Some(selected) = selected else {
            return MoveRight::None;
        };
        // If the selected item is a directory, all panels will shift to the left
        if selected.is_dir() {
            self.previous = self.center.panel().path().to_path_buf();
            debug!(
                "push to history: {}, len={}",
                self.previous.display(),
                self.fwd_history.len()
            );

            // Remember forward history
            self.fwd_history.push((
                self.left.panel().path().to_owned(),
                self.left
                    .panel()
                    .selected_path()
                    .map(|p| p.to_owned())
                    .unwrap_or_default(),
            ));
            let center_clone = self.center.panel().clone();
            self.left.update_panel(center_clone);
            let right_path = self.right.panel().maybe_path();
            self.center.new_panel_instant(right_path);

            if let Some(path) = self.rev_history.pop() {
                info!(
                    "pop rev-history: {}, len={}",
                    path.display(),
                    self.rev_history.len()
                );
                info!("set-center-panel selection");
                self.center.panel_mut().select_path(&path, None);
            }

            if drive_preview {
                let center_selected =
                    self.center.panel().selected_path().map(|p| p.to_path_buf());
                self.right.new_panel_delayed(center_selected.as_deref());

                if let Some(path) = self.rev_history.last() {
                    info!("set-right-panel selection");
                    let path = path.clone();
                    self.right.panel_mut().select_path(&path);
                }
            }

            MoveRight::Descended(selected)
        } else {
            let cwd = self.center.panel().path().to_path_buf();
            MoveRight::OpenFile {
                file: selected,
                cwd,
            }
        }
    }

    /// Ascends into the parent directory, shifting the Miller columns right and
    /// restoring the previous selection from the forward-history.
    ///
    /// Returns `true` if it moved (i.e. the left panel was non-empty), so the
    /// manager can unmark and `mark_dirty`.
    fn move_left(&mut self) -> bool {
        // If the left panel is empty, we cannot move left:
        if self.left.panel().selected_path().is_none() {
            return false;
        }
        if let Some(path) = self.right.panel().maybe_path() {
            info!(
                "push to rev-history: {}, len={}",
                path.display(),
                self.rev_history.len()
            );
            self.rev_history.push(path);
        }
        self.previous = self.center.panel().path().to_path_buf();
        let center_clone = self.center.panel().clone();
        self.right.update_panel(PreviewPanel::Dir(center_clone));
        let left_clone = self.left.panel().clone();
        self.center.update_panel(left_clone);
        // | m | l | m |
        // TODO: When we followed some symlink we don't want to take the parent here.
        match self.fwd_history.pop() {
            Some((previous, selected)) => {
                debug!(
                    "using history: {}, selected={}, len={}",
                    previous.display(),
                    selected.display(),
                    self.fwd_history.len()
                );
                self.left.new_panel_instant(Some(previous));
                info!("set-left-panel selection");
                self.left.panel_mut().select_path(&selected, None);
            }
            None => {
                let center_path = self.center.panel().path().to_path_buf();
                let parent = center_path.parent();
                info!("using parent: {:?}", parent);
                self.left.new_panel_instant(parent);
                info!("set-left-panel selection");
                self.left.panel_mut().select_path(&center_path, None);
            }
        }
        true
    }

    /// Jumps directly to `path`, clearing all navigation history.
    ///
    /// Returns `Some(path)` when the jump happened (so the manager can record
    /// it with zoxide), or `None` when it was a no-op (same path or missing).
    fn jump(&mut self, path: PathBuf, drive_preview: bool) -> Option<PathBuf> {
        // Don't do anything, if the path hasn't changed
        if path.as_path() == self.center.panel().path() {
            return None;
        }
        if !path.exists() {
            return None;
        }
        self.fwd_history.clear(); // Delete history when jumping
        self.rev_history.clear();
        self.previous = self.center.panel().path().to_path_buf();
        self.left.new_panel_instant(path.parent());
        self.left.panel_mut().select_path(&path, None);
        self.center.new_panel_instant(Some(&path));
        if drive_preview {
            self.refresh_preview();
        }
        Some(path)
    }

    /// Builds a fresh, fully-wired [`Tab`] rooted at `cwd`, reusing the same
    /// construction path as the initial panels ([`init_miller_panels`]): left =
    /// parent, center = `cwd`, right = center's selection. The caches and
    /// content senders are cloned per panel, so the new tab gets its own live
    /// file-watcher and real async content updates, exactly like the initial
    /// stack.
    fn new_at(cwd: &Path, handles: ContentHandles) -> Tab {
        let (left, center, right) = init_miller_panels(cwd.to_path_buf(), handles);
        Tab {
            left,
            center,
            right,
            fwd_history: Vec::new(),
            rev_history: Vec::new(),
            // A fresh tab's jump-previous points at its own cwd, so an
            // immediate jump-back is a harmless no-op instead of jumping to the
            // process CWD (".").
            previous: cwd.to_path_buf(),
        }
    }
}

pub struct PanelManager {
    /// Independent Miller-columns stacks. Exactly one for now; multi-tab
    /// support arrives in a later task. `focused` indexes the active tab.
    tabs: Vec<Tab>,
    focused: usize,

    /// Single vs. split view. Only `Single` is rendered for now.
    view: ViewMode,

    /// Handles retained to spawn new tabs dynamically: the panel caches and the
    /// per-panel content-request senders. Cloned into each new tab's
    /// `ManagedPanel`s so a dynamically-created tab gets a live watcher and real
    /// async updates.
    handles: ContentHandles,

    /// Mode of operation
    mode: Mode,

    opener: OpenEngine,

    logger: LogBuffer,

    /// Clipboard
    clipboard: Option<Clipboard>,

    /// Undo/redo stack
    undo: UndoStack,

    /// Sender used by the async paste task to hand back its transaction
    undo_tx: mpsc::UnboundedSender<Transaction>,

    /// Receiver for transactions produced off-thread (paste)
    undo_rx: mpsc::UnboundedReceiver<Transaction>,

    /// Miller-Columns layout
    layout: MillerColumns,

    /// Show hidden files
    show_hidden: bool,

    /// Show log
    show_log: bool,

    /// Whether the screen needs to be repainted
    dirty: bool,

    /// Event-stream from the terminal
    event_reader: EventStream,

    /// Session-only vim-style jump-marks, keyed by letter.
    jump_marks: std::collections::HashMap<char, JumpMark>,

    /// Whether deletes go to the freedesktop trash (undoable) or are permanent.
    use_trash: bool,

    /// command-parser
    parser: CommandParser,

    /// Handle to the standard-output
    stdout: Stdout,

    /// Receiver for incoming dir-panels
    dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,

    /// Receiver for incoming preview-panels
    prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,

    /// Sender for queueing background commands
    command_tx: mpsc::UnboundedSender<QueuedCommand>,

    /// Receiver for status of the background command queue
    command_status_rx: watch::Receiver<QueueStatus>,

    /// Receiver for debug-socket requests (`--debug-socket`)
    debug_rx: Option<mpsc::Receiver<DebugRequest>>,

    /// Event-loop iteration counter, exposed via the debug socket
    debug_seq: u64,
}

impl PanelManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        starting_path: PathBuf,
        handles: ContentHandles,
        use_trash: bool,
        parser: CommandParser,
        dir_rx: mpsc::Receiver<(DirPanel, PanelState)>,
        prev_rx: mpsc::Receiver<(PreviewPanel, PanelState)>,
        logger: LogBuffer,
        opener: OpenEngine,
        command_tx: mpsc::UnboundedSender<QueuedCommand>,
        command_status_rx: watch::Receiver<QueueStatus>,
        debug_rx: Option<mpsc::Receiver<DebugRequest>>,
    ) -> Result<Self> {
        // Prepare terminal
        let stdout = stdout();
        let event_reader = EventStream::new();
        let terminal_size = terminal::size()?;
        let layout = MillerColumns::from_size(terminal_size);

        let (undo_tx, undo_rx) = mpsc::unbounded_channel();

        // Single construction path: the initial tab is built exactly like any
        // dynamically-created one, cloning the retained handles.
        let tab = Tab::new_at(&starting_path, handles.clone());

        Ok(PanelManager {
            tabs: vec![tab],
            focused: 0,
            view: ViewMode::Single,
            handles,
            mode: Mode::Normal,
            logger,
            clipboard: None,
            layout,
            opener,
            undo: UndoStack::new(),
            undo_tx,
            undo_rx,
            show_hidden: false,
            show_log: false,
            dirty: true,
            event_reader,
            jump_marks: std::collections::HashMap::new(),
            use_trash,
            parser,
            stdout,
            dir_rx,
            prev_rx,
            command_tx,
            command_status_rx,
            debug_rx,
            debug_seq: 0,
        })
    }

    fn active(&self) -> &Tab {
        &self.tabs[self.focused]
    }

    fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.focused]
    }

    /// Opens a new tab rooted at the focused tab's current directory and
    /// focuses it. No-op (with a warning) once [`MAX_TABS`] is reached.
    fn new_tab(&mut self) {
        if !can_add_tab(self.tabs.len()) {
            warn!("max {MAX_TABS} tabs reached");
            return;
        }
        let cwd = self.active().center.panel().path().to_path_buf();
        let tab = Tab::new_at(&cwd, self.handles.clone());
        self.tabs.push(tab);
        self.focused = self.tabs.len() - 1;
        self.mark_dirty();
    }

    /// Closes the focused tab. Closing the *last* remaining tab quits rfm
    /// (returning the same [`CloseCmd::QuitWithPath`] as [`Command::Quit`], so
    /// `--choose-dir` still reports the focused tab's cwd). Otherwise the tab is
    /// removed, focus clamps into the shrunk range, and dropping back to a
    /// single tab reverts to [`ViewMode::Single`]; returns `None`.
    fn close_tab(&mut self) -> Option<CloseCmd> {
        if self.tabs.len() == 1 {
            // Last tab: closing it exits the file manager.
            return Some(CloseCmd::QuitWithPath {
                path: self.active().center.panel().path().to_path_buf(),
            });
        }
        self.tabs.remove(self.focused);
        self.focused = focus_after_close(self.focused, self.tabs.len());
        if self.tabs.len() == 1 {
            self.view = ViewMode::Single;
        }
        // Closing the focused tab always changes which tab is focused (and may
        // drop split→single), so refresh the now-focused tab's preview — it was
        // never driven while off-screen/in-split (same reasoning as focus_next).
        // No-op in split and short-circuits on an unchanged path, so it's cheap.
        self.refresh_focused_preview();
        self.mark_dirty();
        None
    }

    /// Cycles focus forward through the open tabs, wrapping at the end.
    fn focus_next(&mut self) {
        self.focused = next_focus(self.focused, self.tabs.len());
        self.refresh_focused_preview();
        self.mark_dirty();
    }

    /// Focuses tab `n` if it exists; otherwise a no-op.
    fn focus_tab(&mut self, n: usize) {
        if n < self.tabs.len() {
            self.focused = n;
            self.refresh_focused_preview();
            self.mark_dirty();
        }
    }

    /// Refreshes the now-focused tab's preview so it matches its current center
    /// selection — needed because preview-driving is skipped while a tab is in
    /// split view or is a non-focused single-view tab (see `reload_all` /
    /// `preview_visible`). No-op in split view (no preview drawn) and cheap when
    /// the preview is already current (`new_panel_delayed` short-circuits on an
    /// unchanged path).
    fn refresh_focused_preview(&mut self) {
        if self.preview_visible() {
            self.active_mut().refresh_preview();
        }
    }

    /// Toggle split-view. Entering split auto-creates a second tab (cloning the
    /// focused cwd) when there is only one, then switches to
    /// [`ViewMode::Split`] — unless the terminal is too narrow for a usable
    /// split, in which case it stays single and warns.
    fn toggle_split(&mut self) {
        match self.view {
            ViewMode::Split => {
                self.view = ViewMode::Single;
                // Preview-driving was skipped while in split; refresh the
                // now-visible focused tab's preview to match its current
                // selection (it may be stale from navigation done in split).
                self.refresh_focused_preview();
            }
            ViewMode::Single => {
                // Check the width BEFORE creating a tab, so a too-narrow
                // terminal doesn't leave an orphan second tab behind.
                if self.layout.split_halves().is_none() {
                    warn!("terminal too narrow for split view");
                } else {
                    if self.tabs.len() < 2 {
                        self.new_tab(); // clones focused cwd, focuses the new tab
                    }
                    self.view = ViewMode::Split;
                }
            }
        }
        self.mark_dirty();
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether the focused tab's preview is currently on screen. In split view
    /// no preview is drawn at all, so navigation skips driving/decoding it; the
    /// preview is refreshed on the way back to single view (`toggle_split`) and
    /// on focus changes (`focus_next`/`focus_tab`) instead.
    fn preview_visible(&self) -> bool {
        matches!(self.view, ViewMode::Single)
    }

    /// Reloads every tab's panels from disk (after a filesystem mutation).
    ///
    /// All tabs — not just the active one — are reloaded: a cross-tab
    /// cut/copy-paste mutates the source tab's directory, which may be a
    /// different (non-focused) tab than the paste target. Reloading only the
    /// active tab would leave the source tab showing a stale listing (a moved
    /// file still visible). Each `reload()` is a cheap async request whose
    /// result is routed back to the owning tab by `panel_id` (see the `dir_rx`
    /// handler), so reloading unchanged tabs is harmless.
    ///
    /// `left`/`center` are reloaded for ALL tabs (a background tab's center is
    /// visible in split view, and a cross-tab mutation touches the source tab).
    /// `right`/preview is reloaded only for the focused tab in single view — the
    /// only place a preview is actually drawn — to avoid re-decoding off-screen
    /// previews. On the next focus change / split→single the now-focused tab's
    /// preview is refreshed anyway (`refresh_focused_preview`).
    fn reload_all(&mut self) {
        let focused = self.focused;
        let preview_visible = self.preview_visible();
        for (idx, tab) in self.tabs.iter_mut().enumerate() {
            tab.left.reload();
            tab.center.reload();
            if preview_visible && idx == focused {
                tab.right.reload();
            }
        }
    }

    /// Records a freshly created archive (`archive` is the opener's result, a
    /// path relative to `dir`) as an undoable-but-not-redoable transaction —
    /// archive content can't be replayed on redo.
    fn record_archive(&mut self, label: &str, dir: &Path, archive: std::io::Result<PathBuf>) {
        match archive {
            Ok(rel) => {
                let path = dir.join(rel.file_name().unwrap_or_default());
                let mut tx = Transaction::new(label);
                tx.push(FsChange::Create {
                    path,
                    is_dir: false,
                });
                self.undo.record(tx.no_redo());
            }
            Err(e) => warn!("Failed to create {label}-archive: {e}"),
        }
    }

    fn draw_log(&mut self) -> Result<()> {
        let bottom = self.layout.footer().saturating_sub(2); // or 3, if we have the advanced command preview
        let mut y = bottom;

        let print_level = |level| match level {
            log::Level::Error => PrintStyledContent("error".red().bold()),
            log::Level::Warn => PrintStyledContent("warn".yellow().bold()),
            log::Level::Info => PrintStyledContent("info".with(color_main()).bold()),
            log::Level::Debug => PrintStyledContent("debug".dark_blue()),
            log::Level::Trace => PrintStyledContent("trace".grey()),
        };

        // Perma-redraw: own the entire log region. When expanded the widget can
        // span the bottom `capacity` rows; collapsed it uses a single row. Blank
        // every row first so lines dropped by TTL/capacity don't leave stale
        // glyphs behind (previously only drawn rows were cleared).
        let span = if self.show_log {
            self.logger.capacity() as u16
        } else {
            1
        };
        let top = bottom.saturating_sub(span.saturating_sub(1));
        for row in top..=bottom {
            queue!(
                self.stdout,
                cursor::MoveTo(0, row),
                Clear(ClearType::CurrentLine),
            )?;
        }

        if self.show_log {
            for (level, line) in self.logger.get().into_iter().rev() {
                queue!(
                    self.stdout,
                    cursor::MoveTo(0, y),
                    print_level(level),
                    style::Print(": "),
                    style::PrintStyledContent(line.grey()),
                    style::Print("  "),
                )?;
                y = y.saturating_sub(1);
            }
        } else if let Some((level, line)) = self
            .logger
            .get()
            .into_iter()
            .rev()
            .find(|(level, _)| *level <= Level::Warn)
        {
            queue!(
                self.stdout,
                cursor::MoveTo(0, y),
                print_level(level),
                style::Print(": "),
                style::PrintStyledContent(line.grey()),
                style::Print("  "),
            )?;
        }
        Ok(())
    }

    // Prints our header
    fn draw_header(&mut self) -> Result<()> {
        let prompt = format!(
            "{}@{}",
            whoami::username(),
            whoami::fallible::hostname().unwrap_or_else(|e| e.to_string())
        );
        let absolute = self
            .active()
            .center
            .panel()
            .selected_path()
            .and_then(|f| f.canonicalize().ok())
            .unwrap_or_else(|| self.active().center.panel().path().to_path_buf());
        let file_name = absolute
            .file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default();
        let absolute = absolute.to_str().unwrap_or_default();

        let (prefix, suffix) = absolute.split_at(absolute.len() - file_name.len());

        queue!(
            self.stdout,
            cursor::MoveTo(0, 0),
            Clear(ClearType::CurrentLine),
            style::PrintStyledContent(prompt.with(color_main()).bold()),
            style::Print(" "),
            style::PrintStyledContent(prefix.to_string().with(color_dir_path()).bold()),
            style::PrintStyledContent(suffix.to_string().bold()),
        )?;

        // Ranger-style tab numbers, right-aligned, when more than one tab is
        // open. The focused tab's (1-based) number is highlighted, the rest
        // muted. Numbers only — no paths — and they take priority on the right
        // edge (the cwd text on the left is drawn first, so if the header is
        // full the numbers overwrite its tail).
        if self.tabs.len() > 1 {
            // Width: one digit per tab plus a separating space between each.
            let width = (self.tabs.len() * 2).saturating_sub(1) as u16;
            let x = self.layout.width().saturating_sub(width);
            queue!(self.stdout, cursor::MoveTo(x, 0))?;
            for i in 0..self.tabs.len() {
                if i > 0 {
                    queue!(self.stdout, style::Print(" "))?;
                }
                let label = (i + 1).to_string();
                if i == self.focused {
                    queue!(
                        self.stdout,
                        style::PrintStyledContent(label.with(color_main()).reverse().bold()),
                    )?;
                } else {
                    queue!(
                        self.stdout,
                        style::PrintStyledContent(label.dark_grey()),
                    )?;
                }
            }
        }
        Ok(())
    }

    // Prints a footer
    fn draw_footer(&mut self) -> Result<()> {
        // Common operation at the start
        queue!(
            self.stdout,
            cursor::MoveTo(0, self.layout.footer()),
            Clear(ClearType::CurrentLine),
        )?;

        if let Mode::Modal(modal) = &mut self.mode {
            match modal.region() {
                ModalRegion::FooterLine => {
                    let y = self.layout.footer();
                    let width = self.layout.width();
                    modal.draw(&mut self.stdout, 0..width, y..y.saturating_add(1))?;
                    return Ok(());
                }
                // Overlay modals don't own the footer; fall through to the
                // normal footer rendering (same as console mode today).
                ModalRegion::ConsoleOverlay => {}
            }
        }
        let (permissions, metadata) = print_metadata(self.active().center.panel().selected_path());
        queue!(
            self.stdout,
            style::PrintStyledContent(permissions.dark_cyan()),
            Print("   "),
            Print(metadata)
        )?;

        // TODO: We could place this into its own line, and also print some recommendations
        let key_buffer = self.parser.buffer();
        let (n, m) = self.active().center.panel().index_vs_total();
        let n_files_string = format!("{n}/{m} ");

        // Okay, we CAN print the matching commands, but currently I am not very happy with this.
        if false {
            queue!(
                self.stdout,
                cursor::MoveTo(
                    // (self.layout.width() / 2).saturating_sub(key_buffer.len() as u16 / 2),
                    0,
                    self.layout.footer().saturating_sub(2),
                ),
                Clear(ClearType::CurrentLine),
                style::PrintStyledContent(key_buffer.clone().on_dark_grey()),
                Print("    "),
            )?;
            let key_buffer_len = key_buffer.chars().count();
            for (cmd, desc) in self.parser.matching_commands() {
                let sub_cmd: String = cmd.chars().skip(key_buffer_len).collect();
                queue!(
                    self.stdout,
                    style::PrintStyledContent(key_buffer.clone().on_dark_grey()),
                    style::PrintStyledContent(sub_cmd.dark_grey()),
                    Print(": "),
                    style::PrintStyledContent(desc.dark_grey()),
                    Print("   "),
                )?;
            }
        } else {
            queue!(
                self.stdout,
                cursor::MoveTo(
                    (self.layout.width() / 2).saturating_sub(key_buffer.len() as u16 / 2),
                    self.layout.footer()
                ),
                style::PrintStyledContent(key_buffer.dark_grey()),
            )?;
        }
        // ---
        queue!(
            self.stdout,
            cursor::MoveTo(
                self.layout
                    .width()
                    .saturating_sub(n_files_string.len() as u16),
                self.layout.footer(),
            ),
            style::Print(n_files_string),
        )?;
        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        if !self.dirty {
            return Ok(());
        }
        self.stdout.execute(BeginSynchronizedUpdate)?;
        self.stdout.queue(cursor::Hide)?;
        self.draw_footer()?;
        self.draw_header()?;
        self.draw_panels()?;
        self.draw_console()?;
        self.draw_log()?;
        self.stdout.execute(EndSynchronizedUpdate)?;
        self.dirty = false;
        Ok(())
    }

    fn draw_panels(&mut self) -> Result<()> {
        let (start, end) = (self.layout.y_range.start, self.layout.y_range.end);
        let height = if self.show_log {
            let cap = self.logger.capacity();
            start..end.saturating_sub(cap as u16)
        } else {
            start..end
        };

        // In split view, draw two adjacent tabs' center columns side by side.
        // Fall back to the single-view layout if the terminal is too narrow to
        // split (defensive: `toggle_split` already guards against this).
        if matches!(self.view, ViewMode::Split) {
            if let Some((left_half, right_half, divider_x)) = self.layout.split_halves() {
                return self.draw_split(height, left_half, right_half, divider_x);
            }
        }

        let left_x = self.layout.left_x_range.clone();
        let center_x = self.layout.center_x_range.clone();
        let right_x = self.layout.right_x_range.clone();
        // Direct field access (not `active_mut()`): the panels live in
        // `self.tabs`, which is disjoint from `self.stdout` — the method
        // call would borrow all of `self` and conflict with `&mut self.stdout`.
        let tab = &mut self.tabs[self.focused];
        // Single view: all three columns render with the bright highlight
        // (`active = true`), matching the pre-split appearance. The
        // active/inactive dimming distinction only applies in split view,
        // where it tells the two center columns apart (see `draw_split`).
        tab.left
            .panel_mut()
            .draw_active(&mut self.stdout, left_x, height.clone(), true)?;
        tab.center
            .panel_mut()
            .draw_active(&mut self.stdout, center_x, height.clone(), true)?;
        tab.right
            .panel_mut()
            .draw_active(&mut self.stdout, right_x, height, true)?;
        Ok(())
    }

    /// Split view: render the `center` column of two adjacent tabs into the two
    /// halves, the focused one active (bright cursor), the other dimmed, with a
    /// vertical divider between them.
    fn draw_split(
        &mut self,
        height: Range<u16>,
        left_half: Range<u16>,
        right_half: Range<u16>,
        divider_x: u16,
    ) -> Result<()> {
        // The visible window is [base, base+1]. In split view tabs.len() >= 2
        // always holds (see `toggle_split`), so this never underflows.
        let base = if self.focused + 1 < self.tabs.len() {
            self.focused
        } else {
            self.focused - 1
        };

        // Draw one tab at a time so each `&mut self.tabs[idx]` borrow is
        // released before the next, keeping it disjoint from `&mut self.stdout`.
        for (offset, x_range) in [(0usize, left_half), (1usize, right_half)] {
            let idx = base + offset;
            let active = idx == self.focused;
            let tab = &mut self.tabs[idx];
            tab.center
                .panel_mut()
                .draw_active(&mut self.stdout, x_range, height.clone(), active)?;
        }

        // Vertical divider between the two halves (muted).
        for y in height.clone() {
            queue!(
                self.stdout,
                cursor::MoveTo(divider_x, y),
                style::PrintStyledContent("│".with(color_main())),
            )?;
        }
        Ok(())
    }

    fn draw_console(&mut self) -> Result<()> {
        if let Mode::Modal(modal) = &mut self.mode {
            if modal.region() == ModalRegion::ConsoleOverlay {
                modal.draw(
                    &mut self.stdout,
                    self.layout.left_x_range.start..self.layout.right_x_range.end,
                    self.layout.y_range.clone(),
                )?;
            }
        }
        Ok(())
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let show_hidden = self.show_hidden;
        let tab = self.active_mut();
        tab.left.panel_mut().set_hidden(show_hidden);
        tab.center.panel_mut().set_hidden(show_hidden);
        if let PreviewPanel::Dir(panel) = tab.right.panel_mut() {
            panel.set_hidden(show_hidden);
        };
        // FIX: Re-selecting path. If we are in a hidden directory, we want to re-select the
        // correct path in the left panel.
        let center_path = tab.center.panel().path().to_path_buf();
        let center_idx = tab.center.panel().selected_idx();
        tab.left
            .panel_mut()
            .select_path(&center_path, Some(center_idx));
        // Toggling hidden files off can re-clamp the center selection onto a
        // different entry, so refresh the preview to match (as move_up/down do).
        let center_selected = tab.center.panel().selected_path().map(|p| p.to_path_buf());
        tab.right.new_panel_delayed(center_selected.as_deref());
        self.mark_dirty();
    }

    fn toggle_log(&mut self) {
        self.show_log = !self.show_log;
        self.mark_dirty();
    }

    fn move_up(&mut self, step: usize) {
        trace!("move-up");
        let drive = self.preview_visible();
        if self.active_mut().move_up(step, drive) {
            self.mark_dirty();
        }
    }

    fn move_down(&mut self, step: usize) {
        trace!("move-down");
        let drive = self.preview_visible();
        if self.active_mut().move_down(step, drive) {
            self.mark_dirty();
        }
    }

    fn move_right(&mut self) {
        trace!("move-right");
        let drive = self.preview_visible();
        match self.active_mut().move_right(drive) {
            MoveRight::None => {}
            MoveRight::Descended(dir) => {
                // Record the visited directory with zoxide.
                if let Some(cmd) = zoxide_add_dir(&dir) {
                    if let Err(e) = self.command_tx.send(cmd) {
                        error!("Failed to queue command: {}", e);
                    }
                }
                self.mark_dirty();
                self.unmark_left_right();
            }
            MoveRight::OpenFile { file, cwd } => {
                info!("Opening '{}'", file.display());
                // Change working directory so that child processes gets spawned
                // from the currently active directory.
                if let Err(e) = std::env::set_current_dir(&cwd) {
                    error!("Failed to set working-directory for process: {e}");
                }
                if let Err(e) = self.opener.open(file) {
                    /* failed to open selected */
                    error!("Opening failed: {e}");
                }
                self.mark_dirty();
                self.unmark_left_right();
            }
        }
    }

    fn move_left(&mut self) {
        trace!("move-left");
        if self.active_mut().move_left() {
            self.unmark_left_right();
            // All panels needs to be redrawn
            self.mark_dirty();
        }
    }

    fn jump(&mut self, path: PathBuf) {
        trace!("jump-to {}", path.display());
        let drive = self.preview_visible();
        if let Some(path) = self.active_mut().jump(path, drive) {
            if let Some(cmd) = zoxide_add_dir(&path) {
                if let Err(e) = self.command_tx.send(cmd) {
                    error!("Failed to queue command: {}", e);
                }
            }
            self.mark_dirty();
        }
    }

    fn move_cursor(&mut self, movement: Move) {
        match movement {
            Move::Up => self.move_up(1),
            Move::Down => self.move_down(1),
            Move::Left => self.move_left(),
            Move::Right => self.move_right(),
            Move::Top => self.move_up(usize::MAX),
            Move::Bottom => self.move_down(usize::MAX),
            Move::HalfPageForward => self.move_down(self.layout.height() as usize / 2),
            Move::HalfPageBackward => self.move_up(self.layout.height() as usize / 2),
            Move::PageForward => self.move_down(self.layout.height() as usize),
            Move::PageBackward => self.move_up(self.layout.height() as usize),
            Move::JumpTo(path) => self.jump(path.into()),
            Move::JumpPrevious => self.jump(self.active().previous.clone()),
        };
    }

    /// Returns a reference to all marked items.
    fn marked_items(&self) -> Vec<&DirElem> {
        let tab = self.active();
        let mut out = Vec::new();
        out.extend(tab.left.panel().elements().filter(|e| e.is_marked()));
        out.extend(tab.center.panel().elements().filter(|e| e.is_marked()));
        if let PreviewPanel::Dir(panel) = tab.right.panel() {
            out.extend(panel.elements().filter(|e| e.is_marked()))
        }
        out
    }

    /// Unmarks all items in all panels
    fn unmark_all_items(&mut self) {
        self.active_mut()
            .center
            .panel_mut()
            .elements_mut()
            .for_each(|item| item.unmark());
        self.unmark_left_right();
    }

    /// Unmarks all items in the left and right panels.
    fn unmark_left_right(&mut self) {
        let tab = self.active_mut();
        tab.left
            .panel_mut()
            .elements_mut()
            .for_each(|item| item.unmark());

        if let PreviewPanel::Dir(panel) = tab.right.panel_mut() {
            panel.elements_mut().for_each(|item| item.unmark());
        }
        self.mark_dirty();
    }

    /// Returns all marked paths *or* the selected path.
    ///
    /// Note: This is an exclusive or - the selected path is not
    /// returned, when there are marked paths.
    /// If there are no marked paths, the selected path is automatically
    /// marked - and therefore it is returned by this function.
    fn marked_or_selected(&mut self) -> Vec<PathBuf> {
        let files: Vec<PathBuf> = self
            .marked_items()
            .iter()
            .map(|item| item.path().to_path_buf())
            .collect();
        // If we have nothing marked, take the current selection
        if files.is_empty() {
            let tab = self.active_mut();
            tab.center.panel_mut().mark_selected_item();
            if let Some(path) = tab.center.panel().selected_path() {
                vec![path.to_path_buf()]
            } else {
                Vec::new()
            }
        } else {
            files
        }
    }

    /// Deletes a file or directory, based on the trash strategy.
    ///
    /// With the trash on, moves it to the freedesktop trash and returns the
    /// recorded change (undoable); otherwise deletes permanently, returns None.
    fn delete_file(&self, file: &Path) -> Option<FsChange> {
        if self.use_trash {
            // guard_trash contains any panic from the trash crate (it asserts
            // instead of erroring on odd states), so a delete can never crash.
            if let Err(e) = crate::undo::guard_trash(|| Ok(trash::delete(file)?)) {
                error!("Cannot trash {}: {e}", file.display());
                return None;
            }
            match capture_trashed(file) {
                Some(item) => Some(FsChange::Trash {
                    item,
                    original: file.to_path_buf(),
                }),
                None => {
                    warn!("trashed {} but could not locate it for undo", file.display());
                    None
                }
            }
        } else {
            if file.is_file() {
                if let Err(e) = std::fs::remove_file(file) {
                    error!("Cannot delete {}: {e}", file.display());
                }
            } else if file.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(file) {
                    error!("Cannot delete {}: {e}", file.display());
                }
            }
            None
        }
    }

    /// Performs bulk rename on multiple files using the configured text editor.
    fn bulkrename(&mut self, files: Vec<PathBuf>) {
        use std::io::Write;

        if files.is_empty() {
            return;
        }

        // Get the directory where files are located (assume all in same directory)
        let dir = files[0].parent().unwrap_or(Path::new(".")).to_path_buf();

        // Create temp file with filenames
        let temp_file = match tempfile::Builder::new()
            .prefix("rfm-bulkrename-")
            .suffix(".txt")
            .tempfile()
        {
            Ok(f) => f,
            Err(e) => {
                error!("Failed to create temp file for bulkrename: {e}");
                return;
            }
        };
        let temp_path = temp_file.path().to_path_buf();

        // Write original filenames to temp file
        let original_names: Vec<String> = files
            .iter()
            .filter_map(|p| p.file_name())
            .filter_map(|n| n.to_str())
            .map(|s| s.to_string())
            .collect();

        if let Err(e) = (|| -> std::io::Result<()> {
            let mut file = std::fs::File::create(&temp_path)?;
            for name in &original_names {
                writeln!(file, "{}", name)?;
            }
            file.flush()?;
            Ok(())
        })() {
            error!("Failed to write temp file for bulkrename: {e}");
            return;
        }

        // Keep temp file alive by keeping the handle
        let _temp_handle = temp_file;

        // Open editor and wait for it to close
        loop {
            match self.opener.open_text_blocking(temp_path.clone()) {
                Ok(status) if !status.success() => {
                    info!("Editor exited with non-zero status, aborting bulkrename");
                    return;
                }
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to open editor: {e}");
                    return;
                }
            }

            // Read edited filenames
            let new_content = match std::fs::read_to_string(&temp_path) {
                Ok(c) => c,
                Err(e) => {
                    error!("Failed to read temp file after editing: {e}");
                    return;
                }
            };

            let new_names: Vec<&str> = new_content.lines().collect();

            // Check if unchanged - if so, exit silently
            if new_names.len() == original_names.len()
                && new_names
                    .iter()
                    .zip(original_names.iter())
                    .all(|(new, old)| *new == old)
            {
                info!("Bulkrename: no changes made");
                return;
            }

            // Validate: line count must match
            if new_names.len() != original_names.len() {
                error!(
                    "Bulkrename: line count mismatch ({} vs {}). Lines must not be added or removed.",
                    new_names.len(),
                    original_names.len()
                );
                // Rewrite original file and reopen
                if let Err(e) = (|| -> std::io::Result<()> {
                    let mut file = std::fs::File::create(&temp_path)?;
                    for name in &original_names {
                        writeln!(file, "{} # DO NOT add or remove lines", name)?;
                    }
                    file.flush()?;
                    Ok(())
                })() {
                    error!("Failed to rewrite temp file: {e}");
                    return;
                }
                continue;
            }

            // Check for errors: duplicates and conflicts
            let mut errors: Vec<Option<&str>> = vec![None; new_names.len()];
            let mut has_errors = false;

            // Check for duplicate target names within batch
            for i in 0..new_names.len() {
                for j in (i + 1)..new_names.len() {
                    if new_names[i] == new_names[j] && new_names[i] != original_names[i] {
                        errors[i] = Some("duplicate target name");
                        errors[j] = Some("duplicate target name");
                        has_errors = true;
                    }
                }
            }

            // Check for conflicts with existing files (but not with files being renamed in this batch)
            for (i, new_name) in new_names.iter().enumerate() {
                if *new_name == original_names[i] {
                    continue; // No change for this file
                }
                let target_path = dir.join(new_name);
                // Check if target exists and is not one of the files being renamed
                if target_path.exists() && !files.contains(&target_path) {
                    errors[i] = Some("already exists");
                    has_errors = true;
                }
            }

            if has_errors {
                // Rewrite file with error comments and reopen
                if let Err(e) = (|| -> std::io::Result<()> {
                    let mut file = std::fs::File::create(&temp_path)?;
                    for (i, new_name) in new_names.iter().enumerate() {
                        if let Some(err_msg) = errors[i] {
                            writeln!(file, "{} # {}", new_name, err_msg)?;
                        } else {
                            writeln!(file, "{}", new_name)?;
                        }
                    }
                    file.flush()?;
                    Ok(())
                })() {
                    error!("Failed to rewrite temp file with errors: {e}");
                    return;
                }
                continue;
            }

            // All validations passed - perform renames
            // We need to handle the case where files might be swapped (a->b, b->a)
            // So we first rename to temporary names, then to final names
            // (from, to, new_name, original) — `original` is the pre-rename
            // path, needed so undo can reverse to it (from may be a temp path).
            let mut temp_renames: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut final_renames: Vec<(PathBuf, PathBuf, String, PathBuf)> = Vec::new();

            // Use process ID and timestamp to create unique temp names
            let unique_id = format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );

            for (i, new_name) in new_names.iter().enumerate() {
                if *new_name == original_names[i] {
                    continue; // No change
                }
                let from = files[i].clone();
                let original = files[i].clone();
                let to = dir.join(new_name);

                // Check if target is also being renamed (swap scenario)
                let is_swap = files.iter().any(|f| *f == to);
                if is_swap {
                    // Rename to temp first (unique name to avoid conflicts)
                    let temp_name =
                        format!(".rfm-bulkrename-{}-{}-{}", unique_id, i, original_names[i]);
                    let temp_path = dir.join(&temp_name);
                    temp_renames.push((from.clone(), temp_path.clone()));
                    final_renames.push((temp_path, to, new_name.to_string(), original));
                } else {
                    final_renames.push((from, to, new_name.to_string(), original));
                }
            }

            // Execute temp renames first
            for (from, to) in &temp_renames {
                if let Err(e) = std::fs::rename(from, to) {
                    error!(
                        "Failed to rename {} -> {}: {e}",
                        from.display(),
                        to.display()
                    );
                    return;
                }
            }

            // Execute final renames with safety fallback
            let mut tx = Transaction::new(format!("bulk rename ({} files)", final_renames.len()));
            let mut success_count = 0;
            for (from, to, new_name, original) in &final_renames {
                match rename_safe(from, to) {
                    Ok(actual_path) => {
                        if actual_path != *to {
                            info!(
                                "Renamed '{}' -> '{}' (adjusted due to conflict)",
                                from.display(),
                                actual_path.display()
                            );
                        }
                        tx.push(FsChange::Move {
                            from: original.clone(),
                            to: actual_path,
                        });
                        success_count += 1;
                    }
                    Err(e) => {
                        error!("Failed to rename to '{}': {e}", new_name);
                    }
                }
            }
            self.undo.record(tx);

            info!("Bulkrename: successfully renamed {} files", success_count);
            return;
        }
    }

    fn handle_debug_request(&mut self, req: DebugRequest) {
        // Replies may fail if the client timed out; that is fine.
        match req {
            DebugRequest::State { reply } => {
                let _ = reply.send(self.state_snapshot());
            }
            DebugRequest::AwaitIdle { reply } => {
                // Answered here means the biased select found nothing else
                // ready — the event queue is drained.
                let _ = reply.send(self.debug_seq);
            }
            DebugRequest::Entries { tab, pane, reply } => {
                let idx = tab.unwrap_or(self.focused);
                // Panic-safe: an out-of-range tab index yields an empty list
                // rather than panic-indexing `self.tabs`.
                let entries = match self.tabs.get(idx) {
                    None => Vec::new(),
                    Some(tab) => {
                        let panel = match pane {
                            PaneId::Left => tab.left.panel(),
                            PaneId::Center => tab.center.panel(),
                        };
                        let selected_idx = panel.selected_idx();
                        panel
                            .elements()
                            .enumerate()
                            .map(|(idx, elem)| EntryInfo {
                                name: elem.name().clone(),
                                marked: elem.is_marked(),
                                hidden: elem.is_hidden(),
                                selected: idx == selected_idx,
                            })
                            .collect()
                    }
                };
                let _ = reply.send(entries);
            }
            DebugRequest::Log { count, reply } => {
                let now = std::time::Instant::now();
                let lines = self
                    .logger
                    .history(count.unwrap_or(crate::logger::HISTORY_CAPACITY))
                    .into_iter()
                    .map(|(level, at, message)| LogEntry {
                        level: level.to_string(),
                        age_secs: now.duration_since(at).as_secs_f64(),
                        message,
                    })
                    .collect();
                let _ = reply.send(lines);
            }
        }
    }

    fn mode_name(&self) -> &'static str {
        match &self.mode {
            Mode::Normal => "normal",
            Mode::Modal(modal) => modal.name(),
        }
    }

    /// Builds a per-tab snapshot from a single tab's center panel, using the
    /// exact same logic the scalar top-level fields use (so the mirror and the
    /// `tabs` array agree). `index_vs_total()` returns a 1-based position; the
    /// snapshot exposes a 0-based index into the visible entries.
    fn tab_snapshot(tab: &Tab) -> TabSnapshot {
        let center = tab.center.panel();
        let (position, total) = center.index_vs_total();
        TabSnapshot {
            cwd: center.path().to_path_buf(),
            selection: center
                .selected_path()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().into_owned()),
            selected_idx: position.saturating_sub(1),
            total,
            marked: center
                .elements()
                .filter(|e| e.is_marked())
                .map(|e| e.path().to_path_buf())
                .collect(),
        }
    }

    fn state_snapshot(&self) -> StateSnapshot {
        let tabs: Vec<TabSnapshot> = self.tabs.iter().map(Self::tab_snapshot).collect();
        // Mirror the focused tab onto the scalar top-level fields for backward
        // compatibility with single-tab scripts.
        let focused = &tabs[self.focused];
        let active = self.active();
        let queue = self.command_status_rx.borrow().clone();
        StateSnapshot {
            seq: self.debug_seq,
            mode: self.mode_name().to_string(),
            view: match self.view {
                ViewMode::Single => "single",
                ViewMode::Split => "split",
            }
            .to_string(),
            focused: self.focused,
            cwd: focused.cwd.clone(),
            selection: focused.selection.clone(),
            selected_idx: focused.selected_idx,
            total: focused.total,
            marked: focused.marked.clone(),
            tabs,
            clipboard: self.clipboard.as_ref().map(|c| ClipboardInfo {
                files: c.files.clone(),
                op: if c.cut { "cut" } else { "copy" }.to_string(),
            }),
            show_hidden: self.show_hidden,
            left_path: active.left.panel().path().to_path_buf(),
            preview_path: active.right.panel().path().to_path_buf(),
            queue_active: queue.active,
            queue_len: queue.queued_count,
            undo_depth: self.undo.undo_depth(),
            redo_depth: self.undo.redo_depth(),
            jump_marks: self
                .jump_marks
                .iter()
                .map(|(c, m)| (c.to_string(), m.dir.clone()))
                .collect(),
        }
    }

    pub async fn run(mut self) -> Result<CloseCmd> {
        // Initial draw
        self.dirty = true;
        self.draw()?;

        let close_cmd = loop {
            let event_reader = self.event_reader.next().fuse();
            tokio::select! {
                biased;
                // Check incoming new logs
                () = self.logger.update() => {
                    self.mark_dirty();
                }
                // Check incoming new dir-panels
                result = self.dir_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break CloseCmd::QuitErr { error: "DirPanel receiver has been dropped" };
                    }
                    let (panel, state) = result.unwrap();

                    // Route the update to whichever tab owns the matching panel
                    // (matched by `panel_id` via `check_update`) — NOT just the
                    // focused tab. Background tabs must refresh too: their center
                    // panels are visible in split view, and a cross-tab move
                    // mutates the (non-focused) source tab's directory. Scope the
                    // `tab` borrow so it ends before `self.mark_dirty()`
                    // (a `&mut self` method), keeping the mark_dirty() invariant.
                    // A `for … in .iter_mut().enumerate()` loop (rather than
                    // `.any(...)`) because `panel` is MOVED into `update_panel`;
                    // a `FnMut` closure can't capture a value it moves out.
                    let focused = self.focused;
                    let preview_visible = self.preview_visible();
                    let updated = 'update: {
                        for (idx, tab) in self.tabs.iter_mut().enumerate() {
                            if tab.center.check_update(&state) {
                                trace!("panel-update: center <- {}", state.path().display());
                                tab.center.update_panel(panel);
                                // Only drive the preview when it is actually on
                                // screen (focused tab, single view). For an
                                // off-screen tab it is refreshed when it next
                                // becomes visible (refresh_focused_preview).
                                if preview_visible && idx == focused {
                                    let selected = tab
                                        .center
                                        .panel()
                                        .selected_path()
                                        .map(|p| p.to_path_buf());
                                    tab.right.new_panel_delayed(selected.as_deref());
                                }
                                break 'update true;
                            } else if tab.left.check_update(&state) {
                                trace!("panel-update: left <- {}", state.path().display());
                                tab.left.update_panel(panel);
                                let center_path = tab.center.panel().path().to_path_buf();
                                let center_idx = tab.center.panel().selected_idx();
                                tab.left.panel_mut().select_path(&center_path, Some(center_idx));
                                break 'update true;
                            }
                        }
                        // Reduce log level here, this is not that important
                        debug!("unknown panel update: {:?}", state);
                        false
                    };
                    if updated {
                        self.mark_dirty();
                    }
                }
                // Check incoming new preview-panels
                result = self.prev_rx.recv() => {
                    // Shutdown if sender has been dropped
                    if result.is_none() {
                        break CloseCmd::QuitErr { error: "Preview receiver has been dropped" };
                    }
                    let (panel, state) = result.unwrap();

                    // Same all-tabs routing as dir_rx above: background tabs'
                    // preview panels stay fresh too (a preview load driven while
                    // focused may land after focus moved on).
                    //
                    // Written as a `for`+labeled-block, not `.iter_mut().any(...)`:
                    // `panel` is MOVED into `update_panel`, and a `FnMut` closure
                    // can't capture a value it moves out on each call.
                    let updated = 'update: {
                        for tab in self.tabs.iter_mut() {
                            if tab.right.check_update(&state) {
                                trace!("panel-update: preview <- {}", state.path().display());
                                tab.right.update_panel(panel);
                                break 'update true;
                            }
                        }
                        false
                    };
                    if updated {
                        self.mark_dirty();
                    }
                }
                // Transactions handed back by the async paste task
                result = self.undo_rx.recv() => {
                    if let Some(tx) = result {
                        self.undo.record(tx);
                        self.reload_all();
                        self.mark_dirty();
                    }
                }
                // Check incoming new events
                result = event_reader => {
                    // Shutdown if reader has been dropped
                    match result {
                        Some(event) => {
                            if let Some(close_cmd) = self.handle_event(event?)? {
                                break close_cmd;
                            }
                        }
                        None => break CloseCmd::QuitErr { error: "event-reader has been dropped" },
                    }
                }
                // Debug socket requests; must stay the LAST branch of this
                // biased select: it may only win when nothing else is ready,
                // which is exactly the idle guarantee `await-idle` promises.
                Some(req) = recv_debug(&mut self.debug_rx) => {
                    self.handle_debug_request(req);
                    // Debug queries are not app events: skip draw() and the
                    // seq bump so polling `state` doesn't advance seq itself.
                    continue;
                }
            }
            // Repaint if any handler marked the screen dirty.
            self.draw()?;
            self.debug_seq = self.debug_seq.wrapping_add(1);
        };
        // Cleanup after leaving this function
        self.stdout
            .queue(Clear(ClearType::All))?
            .queue(cursor::MoveTo(0, 0))?
            .queue(cursor::Show)?
            .flush()?;

        Ok(close_cmd)
    }

    /// Applies the op a modal mode requested.
    ///
    /// This is the only place where modal-mode effects touch the panels:
    /// per-op panel mutations and the derived redraws all live here.
    /// Concluding ops return to [`Mode::Normal`] here or in their
    /// `apply_*` helpers (helpers own their mode reset).
    fn apply_mode_op(&mut self, op: ModeOp) {
        match op {
            ModeOp::None => {}
            ModeOp::Cd(path) => {
                self.jump(path);
            }
            ModeOp::UpdateSearch(pattern) => {
                self.active_mut().center.panel_mut().update_search(pattern);
            }
            ModeOp::FinishSearch(pattern) => {
                let tab = self.active_mut();
                tab.center.panel_mut().finish_search(&pattern);
                tab.center.panel_mut().select_next_marked();
                let selected = tab.center.panel().selected_path().map(|p| p.to_path_buf());
                tab.right.new_panel_delayed(selected.as_deref());
                self.mode = Mode::Normal;
            }
            ModeOp::RenamePreview(name) => {
                // The manager supplies the apply-time context: the preview
                // is anchored at the currently selected entry.
                let tab = self.active_mut();
                let selected_idx = tab.center.panel().selected_idx();
                tab.center
                    .panel_mut()
                    .inject_rename_preview(name, selected_idx);
            }
            ModeOp::Rename { to } => self.apply_rename(to),
            ModeOp::CreatePreview { name, is_dir } => {
                self.active_mut()
                    .center
                    .panel_mut()
                    .inject_new_element(name, is_dir);
            }
            ModeOp::Create { name, is_dir } => self.apply_create(name, is_dir),
            ModeOp::RestoreFromTrash { items, cursor } => {
                // Only restore entries that still exist: one may have been
                // removed out of band (another trash tool), and restore_all
                // panics on a missing entry. guard_trash contains any panic.
                let (live, gone): (Vec<_>, Vec<_>) = items
                    .into_iter()
                    .partition(|i| std::path::Path::new(&i.id).exists());
                if !gone.is_empty() {
                    warn!("{} trash entrie(s) vanished before restore; skipped", gone.len());
                }
                let n = live.len();
                if n > 0 {
                    match crate::undo::guard_trash(|| Ok(trash::os_limited::restore_all(live)?)) {
                        Ok(()) => info!("wiederhergestellt: {n} Element(e) aus dem Papierkorb"),
                        Err(e) => error!("Wiederherstellen fehlgeschlagen: {e}"),
                    }
                }
                // Restore from the trash view is intentionally not recorded on
                // the undo stack in v1.
                //
                // Stay in the trash view and refresh it from the now-smaller
                // trash so several items can be restored in a row; the cursor
                // is held at its old slot (clamped) — the next item slides up
                // into it. Rebuilds fresh from the FS so out-of-band changes
                // show too.
                let entries = crate::undo::list_trash_entries()
                    .into_iter()
                    .map(|(item, is_dir)| TrashEntry { item, is_dir })
                    .collect();
                let mut view = TrashView::new(entries);
                view.set_cursor(cursor);
                self.mode = Mode::Modal(Box::new(view));
                self.reload_all();
            }
            ModeOp::Exit { cleanup } => {
                self.apply_cleanup(cleanup);
                self.mode = Mode::Normal;
            }
        }
        // Every modal key event repaints (draw() perma-redraws now).
        self.mark_dirty();
    }

    /// Applies [`ModeOp::Rename`]: renames the selected entry to `to`
    /// (within its parent directory) and leaves the mode.
    ///
    /// Ported verbatim from the old inline rename Enter arm: same-path is
    /// a no-op, existing targets are never overwritten.
    fn apply_rename(&mut self, to: String) {
        if let Some(from) = self.active().center.panel().selected_path() {
            let from = from.to_path_buf();
            let to_path = from.parent().map(|p| p.join(&to)).unwrap_or_default();
            // Don't rename if it's the same path
            if from == to_path {
                // No-op, just exit rename mode
            } else if to_path.exists() {
                // Prevent overwriting existing files
                warn!("Cannot rename: '{}' already exists", to_path.display());
            } else if let Err(e) = std::fs::rename(&from, &to_path) {
                error!("{e}");
            } else {
                let mut tx = Transaction::new(format!(
                    "rename {} → {to}",
                    from.file_name().unwrap_or_default().to_string_lossy(),
                ));
                tx.push(FsChange::Move {
                    from,
                    to: to_path,
                });
                self.undo.record(tx);
            }
        }
        self.mode = Mode::Normal;
        let tab = self.active_mut();
        tab.center.panel_mut().clear_rename_preview();
        tab.center.reload();
        tab.right.reload();
        self.mark_dirty();
    }

    /// Applies [`ModeOp::Create`]: creates the new directory (`is_dir`)
    /// or file named `name` in the current directory and leaves the mode.
    ///
    /// Ported verbatim from the old inline CreateItem Enter arm: the
    /// typed name is trimmed here (at apply time), creation errors are
    /// only logged.
    fn apply_create(&mut self, name: String, is_dir: bool) {
        let current_path = self.active().center.panel().path().to_path_buf();
        let create_fn = if is_dir {
            |item| fs_extra::dir::create(item, false)
        } else {
            |item| {
                let _ = OpenOptions::new()
                    .read(true)
                    .append(true)
                    .create(true)
                    .open(item)?;
                Ok(())
            }
        };
        let target = current_path.join(name.trim());
        // Only a genuinely new path is undoable. `touch`/`mkdir` on an existing
        // entry succeeds (create-if-missing) but must NOT record a Create — its
        // undo would `remove` the user's pre-existing file/dir (data loss).
        let existed = target.exists();
        if let Err(e) = create_fn(target.clone()) {
            error!("{e}");
        } else if !existed {
            let mut tx = Transaction::new(format!("create {}", name.trim()));
            tx.push(FsChange::Create {
                path: target,
                is_dir,
            });
            self.undo.record(tx);
        }
        self.mode = Mode::Normal;
        self.active_mut().center.panel_mut().clear_new_element();
        self.mark_dirty();
    }

    /// Undoes the visible traces of a cancelled modal mode.
    fn apply_cleanup(&mut self, cleanup: Cleanup) {
        match cleanup {
            Cleanup::None => {}
            Cleanup::Search => self.active_mut().center.panel_mut().clear_search(),
            Cleanup::RenamePreview => {
                self.active_mut().center.panel_mut().clear_rename_preview()
            }
            Cleanup::CreatePreview => self.active_mut().center.panel_mut().clear_new_element(),
            Cleanup::CdTo(path) => self.jump(path),
        }
    }

    /// Undoes the most recent recorded transaction.
    fn apply_undo(&mut self) {
        match self.undo.undo() {
            UndoOutcome::Done(label) => info!("rückgängig: {label}"),
            UndoOutcome::Empty => info!("nichts rückgängig zu machen"),
            UndoOutcome::Blocked(reason) => {
                warn!("kann nicht rückgängig gemacht werden: {reason}")
            }
            UndoOutcome::Failed(e) => error!("undo fehlgeschlagen: {e}"),
        }
        self.reload_all();
        self.mark_dirty();
    }

    /// Re-applies the most recently undone transaction.
    fn apply_redo(&mut self) {
        match self.undo.redo() {
            UndoOutcome::Done(label) => info!("wiederhergestellt: {label}"),
            UndoOutcome::Empty => info!("nichts wiederherzustellen"),
            UndoOutcome::Blocked(reason) => warn!("redo blockiert: {reason}"),
            UndoOutcome::Failed(e) => error!("redo fehlgeschlagen: {e}"),
        }
        self.reload_all();
        self.mark_dirty();
    }

    /// Handles the terminal events.
    ///
    /// Returns Ok(true) if the application needs to shut down.
    fn handle_event(&mut self, event: Event) -> Result<Option<CloseCmd>> {
        if let Event::Key(key_event) = event {
            let mode_before = self.mode_name();
            trace!("key-event: {key_event:?} (mode: {mode_before})");
            match &mut self.mode {
                Mode::Normal => {
                    // Esc resets Normal-mode state: pending key chords and
                    // marks. Modal modes receive Esc themselves and return
                    // their own cancel op.
                    if let KeyCode::Esc = key_event.code {
                        self.parser.clear();
                        // The three clears are latent no-ops now that modals
                        // clean up after themselves — candidates for removal;
                        // the redraws stay regardless, because unmark is
                        // user-visible.
                        let tab = self.active_mut();
                        tab.center.panel_mut().clear_search();
                        tab.center.panel_mut().clear_new_element();
                        tab.center.panel_mut().clear_rename_preview();
                        self.unmark_all_items();
                    }
                    match self.parser.add_event(key_event) {
                        Command::Move(direction) => {
                            self.move_cursor(direction);
                        }
                        Command::ViewTrash => {
                            if self.use_trash {
                                // The manager owns filesystem access: resolve each
                                // item's dir-ness here so the adapter stays pure.
                                // list_trash_entries contains the trash crate's
                                // panic on dangling/corrupt entries and skips them.
                                let entries = crate::undo::list_trash_entries()
                                    .into_iter()
                                    .map(|(item, is_dir)| TrashEntry { item, is_dir })
                                    .collect();
                                self.mode = Mode::Modal(Box::new(TrashView::new(entries)));
                            } else {
                                warn!("Trash is disabled (use_trash = false) — nothing to show.");
                            }
                        }
                        Command::ToggleHidden => self.toggle_hidden(),
                        Command::ToggleLog => self.toggle_log(),
                        Command::Cd { zoxide } => {
                            // The console captures its starting path at
                            // construction (the same panel path the old
                            // manager-side field recorded before the seam).
                            self.mode = if zoxide {
                                Mode::Modal(Box::new(Zoxide::from_panel(
                                    self.active().center.panel(),
                                )))
                            } else {
                                Mode::Modal(Box::new(DirConsole::from_panel(
                                    self.active().center.panel(),
                                )))
                            };
                        }
                        Command::Search => {
                            self.mode = Mode::Modal(Box::new(SearchMode::new()));
                        }
                        Command::Rename => {
                            // Check if multiple files are marked - use bulkrename
                            let marked = self.marked_items();
                            if marked.len() > 1 {
                                let files: Vec<PathBuf> =
                                    marked.iter().map(|e| e.path().to_path_buf()).collect();
                                self.unmark_all_items();
                                self.bulkrename(files);
                                self.reload_all();
                            } else {
                                // Single file rename - modal mode seeded
                                // with the current file name
                                let selected = self
                                    .active()
                                    .center
                                    .panel()
                                    .selected_path()
                                    .and_then(|p| p.file_name())
                                    .and_then(|f| f.to_owned().into_string().ok())
                                    .unwrap_or_default();
                                self.mode = Mode::Modal(Box::new(RenameMode::new(selected)));
                            }
                        }
                        Command::Next => {
                            let tab = self.active_mut();
                            tab.center.panel_mut().select_next_marked();
                            let selected =
                                tab.center.panel().selected_path().map(|p| p.to_path_buf());
                            tab.right.new_panel_delayed(selected.as_deref());
                        }
                        Command::Previous => {
                            let tab = self.active_mut();
                            tab.center.panel_mut().select_prev_marked();
                            let selected =
                                tab.center.panel().selected_path().map(|p| p.to_path_buf());
                            tab.right.new_panel_delayed(selected.as_deref());
                        }
                        Command::Mkdir => {
                            self.mode = Mode::Modal(Box::new(CreateItemMode::new(true)));
                        }
                        Command::Touch => {
                            self.mode = Mode::Modal(Box::new(CreateItemMode::new(false)));
                        }
                        Command::Mark => {
                            self.active_mut().center.panel_mut().mark_selected_item();
                            self.move_cursor(Move::Down);
                        }
                        Command::Undo => self.apply_undo(),
                        Command::Redo => self.apply_redo(),
                        Command::FocusNext => {
                            trace!("command: focus next tab");
                            self.focus_next();
                        }
                        Command::NewTab => {
                            trace!("command: new tab");
                            self.new_tab();
                        }
                        Command::CloseTab => {
                            trace!("command: close tab");
                            if let Some(close) = self.close_tab() {
                                return Ok(Some(close));
                            }
                        }
                        Command::FocusTab(n) => {
                            trace!("command: focus tab {n}");
                            // Config is 1-based; tabs are indexed from 0.
                            self.focus_tab(n.saturating_sub(1));
                        }
                        Command::ToggleSplit => {
                            trace!("command: toggle split");
                            self.toggle_split();
                        }
                        Command::Cut => {
                            let files = self.marked_or_selected();
                            info!("cut {} items", files.len());
                            self.clipboard = Some(Clipboard { files, cut: true });
                        }
                        Command::Copy => {
                            let files = self.marked_or_selected();
                            info!("copying {} items", files.len());
                            self.clipboard = Some(Clipboard { files, cut: false });
                        }
                        Command::Delete => {
                            let files = self.marked_or_selected();
                            info!("Deleted {} items", files.len());
                            self.unmark_all_items();
                            if self.use_trash {
                                let mut tx =
                                    Transaction::new(format!("delete ({} items)", files.len()));
                                for file in files {
                                    if let Some(change) = self.delete_file(&file) {
                                        tx.push(change);
                                    }
                                }
                                // Redoable: redo re-trashes and re-captures the
                                // fresh TrashItem (see FsChange::Trash::redo).
                                self.undo.record(tx);
                            } else {
                                for file in files {
                                    self.delete_file(&file);
                                }
                                // Permanent deletion cannot be undone.
                                self.undo.barrier("permanentes Löschen");
                            }
                            self.reload_all();
                        }
                        Command::Paste { overwrite } => {
                            self.unmark_all_items();
                            let current_path = self.active().center.panel().path().to_path_buf();
                            let clipboard = self.clipboard.take();
                            let undo_tx = self.undo_tx.clone();
                            tokio::task::spawn_blocking(move || {
                                if let Some(clipboard) = clipboard {
                                    info!(
                                        "paste {} items, overwrite = {}",
                                        clipboard.files.len(),
                                        overwrite
                                    );
                                    let mut tx = Transaction::new(format!(
                                        "paste ({} items)",
                                        clipboard.files.len()
                                    ));
                                    for file in clipboard.files.iter() {
                                        if clipboard.cut {
                                            match move_item(file, &current_path) {
                                                Ok(Some(change)) => tx.push(change),
                                                Ok(None) => {}
                                                Err(e) => {
                                                    error!("Failed to move {}: {e}", file.display())
                                                }
                                            }
                                        } else {
                                            match copy_item(file, &current_path) {
                                                Ok(change) => tx.push(change),
                                                Err(e) => {
                                                    error!("Failed to copy {}: {e}", file.display())
                                                }
                                            }
                                        }
                                    }
                                    // Recorded (and panels reloaded) on the main loop.
                                    let _ = undo_tx.send(tx);
                                }
                            });
                            self.reload_all();
                        }
                        Command::Zip => {
                            let items = self.marked_or_selected();
                            let dir = self.active().center.panel().path().to_path_buf();
                            if let Err(e) = std::env::set_current_dir(&dir) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            let archive = self.opener.zip(items);
                            self.record_archive("zip", &dir, archive);
                        }
                        Command::Tar => {
                            let items = self.marked_or_selected();
                            let dir = self.active().center.panel().path().to_path_buf();
                            if let Err(e) = std::env::set_current_dir(&dir) {
                                error!("Failed to set working-directory for process: {e}");
                            }
                            let archive = self.opener.tar(items);
                            self.record_archive("tar", &dir, archive);
                        }
                        Command::Extract => {
                            let archive = self
                                .active()
                                .center
                                .panel()
                                .selected_path()
                                .map(|p| p.to_path_buf());
                            if let Some(archive) = archive {
                                let dir = self.active().center.panel().path().to_path_buf();
                                if let Err(e) = std::env::set_current_dir(&dir) {
                                    error!("Failed to set working-directory for process: {e}");
                                }
                                if let Err(e) = self.opener.extract(archive) {
                                    warn!("Failed to extract archive: {e}");
                                }
                            } else {
                                warn!("Nothing extractable is selected");
                            }
                        }
                        Command::Quit => {
                            return Ok(Some(CloseCmd::QuitWithPath {
                                path: self.active().center.panel().path().to_path_buf(),
                            }));
                        }
                        Command::QuitWithoutPath => {
                            return Ok(Some(CloseCmd::Quit));
                        }
                        Command::UserCommand {
                            name,
                            cmd,
                            interactive,
                            separator,
                        } => {
                            let paths = self.marked_or_selected();
                            let expanded_cmd = expand_command(&cmd, &paths, &separator);
                            let working_dir = self.active().center.panel().path().to_path_buf();

                            if interactive {
                                // Run interactively in foreground
                                info!("Running interactive command '{}': {}", name, expanded_cmd);
                                if let Err(e) = std::env::set_current_dir(&working_dir) {
                                    error!("Failed to set working directory: {e}");
                                }
                                // TODO: Implement terminal suspend/resume for interactive commands
                                // For now, just run it blocking
                                let result = std::process::Command::new("sh")
                                    .arg("-c")
                                    .arg(&expanded_cmd)
                                    .current_dir(&working_dir)
                                    .status();
                                match result {
                                    Ok(status) => {
                                        if status.success() {
                                            info!("Command '{}' completed successfully", name);
                                        } else {
                                            let code = status.code().unwrap_or(-1);
                                            error!(
                                                "Command '{}' failed with exit code {}",
                                                name, code
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        error!("Failed to run command '{}': {}", name, e);
                                    }
                                }
                            } else {
                                // Queue for background execution
                                info!("Queueing command '{}': {}", name, expanded_cmd);
                                let queued = QueuedCommand {
                                    name,
                                    cmd: expanded_cmd,
                                    working_dir,
                                };
                                if let Err(e) = self.command_tx.send(queued) {
                                    error!("Failed to queue command: {}", e);
                                }
                            }
                            self.unmark_all_items();
                        }
                        Command::SetJumpMark(c) => {
                            let dir = self.active().center.panel().path().to_path_buf();
                            let entry = self
                                .active()
                                .center
                                .panel()
                                .selected_path()
                                .map(|p| p.to_path_buf());
                            info!("jump-mark '{c}' set -> {}", dir.display());
                            self.jump_marks.insert(c, JumpMark { dir, entry });
                        }
                        Command::JumpToMark(c) => match self.jump_marks.get(&c).cloned() {
                            None => warn!("jump-mark '{c}' not set"),
                            Some(mark) => {
                                if !mark.dir.exists() {
                                    warn!(
                                        "jump-mark '{c}' -> {} no longer exists",
                                        mark.dir.display()
                                    );
                                } else {
                                    self.jump(mark.dir);
                                    if let Some(entry) = mark.entry {
                                        // jump() previewed the panel's default
                                        // selection; re-select the marked entry
                                        // and refresh the preview for it.
                                        let tab = self.active_mut();
                                        tab.center.panel_mut().select_path(&entry, None);
                                        let selected = tab
                                            .center
                                            .panel()
                                            .selected_path()
                                            .map(|p| p.to_path_buf());
                                        tab.right.new_panel_delayed(selected.as_deref());
                                    }
                                    self.mark_dirty();
                                }
                            }
                        },
                        Command::None => {}
                    }
                    // Every handled key event repaints.
                    self.mark_dirty();
                }
                Mode::Modal(modal) => {
                    let op = modal.handle_key(key_event);
                    self.apply_mode_op(op);
                }
            }
            let mode_after = self.mode_name();
            if mode_before != mode_after {
                trace!("mode: {mode_before} -> {mode_after}");
            }
        }
        if let Event::Resize(sx, sy) = event {
            self.layout = MillerColumns::from_size((sx, sy));
            self.mark_dirty();
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::PanelCache;
    use std::fs::{self, canonicalize};
    use tokio::sync::mpsc;

    /// Everything a fixture-backed [`Tab`] needs to stay alive: the tab plus
    /// the receiving ends of its content channels (dropping them would make the
    /// `ManagedPanel` senders panic on `.expect("Receiver dropped")`).
    struct TabFixture {
        tab: Tab,
        _dir_rx: mpsc::UnboundedReceiver<PanelUpdate>,
        _prev_rx: mpsc::UnboundedReceiver<PanelUpdate>,
        _tmp: tempfile::TempDir,
        root: PathBuf,
    }

    /// Builds a real `Tab` rooted at a fresh tempdir laid out like:
    ///
    /// ```text
    /// root/
    ///   sub/            <- a subdirectory (sorts first, initial selection)
    ///     inner1.txt
    ///     inner2.txt
    ///   zempty/         <- an empty subdirectory (sorts after `sub`, before files)
    ///   a.txt
    ///   b.txt
    ///   c.txt
    /// ```
    ///
    /// Note `sub` holds *two* files so a rev_history round-trip can restore a
    /// specific deeper selection (not merely the default first child), and
    /// `zempty` is deliberately named so it still sorts after `sub` — keeping
    /// `sub` the initial selection that the other tests rely on.
    ///
    /// The panels are populated synchronously via `new_panel_instant`
    /// (mirroring `init_miller_panels`), so navigation is observable without
    /// any async content manager running.
    fn fixture() -> TabFixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = canonicalize(tmp.path()).expect("canonicalize root");
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("inner1.txt"), b"x").unwrap();
        fs::write(root.join("sub").join("inner2.txt"), b"x").unwrap();
        fs::create_dir(root.join("zempty")).unwrap();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(name), b"x").unwrap();
        }

        let (dir_tx, dir_rx) = mpsc::unbounded_channel::<PanelUpdate>();
        let (prev_tx, prev_rx) = mpsc::unbounded_channel::<PanelUpdate>();
        let handles = ContentHandles {
            directory_cache: PanelCache::<DirPanel>::with_size(64),
            preview_cache: PanelCache::<PreviewPanel>::with_size(64),
            directory_tx: dir_tx,
            preview_tx: prev_tx,
        };

        let tab = Tab::new_at(&root, handles);

        TabFixture {
            tab,
            _dir_rx: dir_rx,
            _prev_rx: prev_rx,
            _tmp: tmp,
            root,
        }
    }

    fn center_path(tab: &Tab) -> PathBuf {
        tab.center.panel().path().to_path_buf()
    }

    fn center_selected(tab: &Tab) -> Option<PathBuf> {
        tab.center.panel().selected_path().map(|p| p.to_path_buf())
    }

    #[test]
    fn initial_selection_is_the_subdir() {
        // Directories sort before files, so `sub` is the initial selection.
        let f = fixture();
        assert_eq!(center_path(&f.tab), f.root);
        assert_eq!(center_selected(&f.tab), Some(f.root.join("sub")));
    }

    #[test]
    fn move_down_and_up_change_selection_within_bounds() {
        // Sort order is: sub, zempty (dirs first), then a.txt, b.txt, c.txt.
        let mut f = fixture();
        // sub -> zempty
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("zempty")));
        // zempty -> a.txt
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("a.txt")));
        // a.txt -> b.txt
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("b.txt")));
        // back up to a.txt
        assert!(f.tab.move_up(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("a.txt")));
    }

    #[test]
    fn move_up_at_top_is_noop() {
        let mut f = fixture();
        // Already on the first entry (`sub`); moving up must not underflow and
        // must report "did not move".
        assert!(!f.tab.move_up(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("sub")));
    }

    #[test]
    fn move_down_at_bottom_is_noop() {
        let mut f = fixture();
        // Jump to the very bottom.
        assert!(f.tab.move_down(usize::MAX, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("c.txt")));
        // A further move down must report "did not move".
        assert!(!f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("c.txt")));
    }

    #[test]
    fn move_right_into_subdir_shifts_panels_and_records_history() {
        let mut f = fixture();
        let old_center = center_path(&f.tab); // root
        let sub = f.root.join("sub");

        match f.tab.move_right(true) {
            MoveRight::Descended(dir) => assert_eq!(dir, sub),
            MoveRight::None => panic!("expected Descended, got None"),
            MoveRight::OpenFile { .. } => panic!("expected Descended, got OpenFile"),
        }

        // center is now the subdir; left is the old center.
        assert_eq!(center_path(&f.tab), sub);
        assert_eq!(f.tab.left.panel().path(), old_center.as_path());
        // one forward-history entry recording where we came from.
        assert_eq!(f.tab.fwd_history.len(), 1);
        assert_eq!(f.tab.fwd_history[0].0, f.root.parent().unwrap());
        // previous cwd is remembered.
        assert_eq!(f.tab.previous, old_center);
    }

    #[test]
    fn move_right_on_a_file_reports_openfile_without_shifting() {
        let mut f = fixture();
        // Move onto a regular file (sub -> zempty -> a.txt).
        assert!(f.tab.move_down(1, true));
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(f.root.join("a.txt")));
        let center_before = center_path(&f.tab);

        let outcome = f.tab.move_right(true);
        match outcome {
            MoveRight::OpenFile { file, cwd } => {
                assert_eq!(file, f.root.join("a.txt"));
                assert_eq!(cwd, f.root);
            }
            _ => panic!("expected OpenFile"),
        }
        // Panels did not shift; no history recorded.
        assert_eq!(center_path(&f.tab), center_before);
        assert!(f.tab.fwd_history.is_empty());
    }

    #[test]
    fn move_left_restores_selection_from_history() {
        let mut f = fixture();
        let root = f.root.clone();
        let sub = root.join("sub");

        // Descend into sub.
        assert!(matches!(f.tab.move_right(true), MoveRight::Descended(_)));
        assert_eq!(center_path(&f.tab), sub);

        // Go back left.
        assert!(f.tab.move_left());
        // We are back at root, and `sub` is re-selected from forward-history.
        assert_eq!(center_path(&f.tab), root);
        assert_eq!(center_selected(&f.tab), Some(sub));
        // forward-history was consumed.
        assert!(f.tab.fwd_history.is_empty());
    }

    #[test]
    fn move_left_stops_at_filesystem_root() {
        // Climbing left repeatedly must terminate: once the center reaches the
        // filesystem root, the left panel is empty and move_left is a no-op
        // (returns false) instead of looping or underflowing.
        let mut f = fixture();
        // Bound the loop generously; the tempdir depth is small.
        let mut moved = 0;
        while f.tab.move_left() {
            moved += 1;
            assert!(moved < 100, "move_left did not terminate");
        }
        // We reached a point where move_left reports "did not move".
        assert!(!f.tab.move_left());
        // Center is at the filesystem root (has no parent, or parent == self).
        let here = center_path(&f.tab);
        assert!(
            here.parent().is_none() || here.parent() == Some(here.as_path()),
            "expected filesystem root, got {}",
            here.display()
        );
    }

    #[test]
    fn jump_sets_center_and_clears_history() {
        let mut f = fixture();
        let root = f.root.clone();
        let sub = root.join("sub");

        // Create some history first.
        assert!(matches!(f.tab.move_right(true), MoveRight::Descended(_)));
        assert_eq!(f.tab.fwd_history.len(), 1);

        // Jump back up to root.
        let jumped = f.tab.jump(root.clone(), true);
        assert_eq!(jumped, Some(root.clone()));
        assert_eq!(center_path(&f.tab), root);
        assert_eq!(f.tab.left.panel().path(), root.parent().unwrap());
        // history cleared on jump.
        assert!(f.tab.fwd_history.is_empty());
        assert!(f.tab.rev_history.is_empty());
        // previous remembers where we jumped from.
        assert_eq!(f.tab.previous, sub);
    }

    #[test]
    fn jump_to_same_path_is_noop() {
        let mut f = fixture();
        let here = center_path(&f.tab);
        assert_eq!(f.tab.jump(here, true), None);
    }

    #[test]
    fn jump_to_missing_path_is_noop() {
        let mut f = fixture();
        let missing = f.root.join("does-not-exist");
        assert_eq!(f.tab.jump(missing, true), None);
    }

    #[test]
    fn move_right_after_move_left_restores_deeper_selection_via_rev_history() {
        // End-to-end exercise of the rev_history path: descend into `sub`,
        // pick a *specific* child (not the default first one), climb back out
        // (which pushes that child onto rev_history), then descend again. The
        // pop-rev-history / set-center-selection logic must restore the exact
        // deeper selection.
        let mut f = fixture();
        let root = f.root.clone();
        let sub = root.join("sub");
        let inner1 = sub.join("inner1.txt");
        let inner2 = sub.join("inner2.txt");

        // Descend into `sub`; default selection is the first child.
        assert!(matches!(f.tab.move_right(true), MoveRight::Descended(_)));
        assert_eq!(center_path(&f.tab), sub);
        assert_eq!(center_selected(&f.tab), Some(inner1.clone()));

        // Move down to `inner2.txt` — the deeper selection we want restored.
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(inner2.clone()));
        // rev_history is empty until we climb back out.
        assert!(f.tab.rev_history.is_empty());

        // Climb back to root; this pushes the highlighted child to rev_history.
        assert!(f.tab.move_left());
        assert_eq!(center_path(&f.tab), root);
        assert_eq!(center_selected(&f.tab), Some(sub.clone()));
        assert_eq!(f.tab.rev_history, vec![inner2.clone()]);

        // Descend again: rev_history is popped and the deeper `inner2.txt`
        // selection is restored (NOT the default `inner1.txt`).
        assert!(matches!(f.tab.move_right(true), MoveRight::Descended(_)));
        assert_eq!(center_path(&f.tab), sub);
        assert_eq!(
            center_selected(&f.tab),
            Some(inner2),
            "rev_history should restore the deeper selection, not default to {}",
            inner1.display()
        );
        // rev_history was consumed by the pop.
        assert!(f.tab.rev_history.is_empty());
    }

    #[test]
    fn move_right_in_empty_dir_reports_none() {
        // Descend into the empty subdirectory, then attempt to descend further.
        // With nothing selected there is nothing to descend into, so move_right
        // must report `None` and leave the panels untouched.
        let mut f = fixture();
        let zempty = f.root.join("zempty");

        // sub -> zempty, then descend into it.
        assert!(f.tab.move_down(1, true));
        assert_eq!(center_selected(&f.tab), Some(zempty.clone()));
        assert!(matches!(f.tab.move_right(true), MoveRight::Descended(_)));
        assert_eq!(center_path(&f.tab), zempty);
        // The empty dir has no selection.
        assert_eq!(center_selected(&f.tab), None);

        let center_before = center_path(&f.tab);
        let fwd_len_before = f.tab.fwd_history.len();

        // move_right on an empty directory: nothing selected -> None, no shift.
        assert!(matches!(f.tab.move_right(true), MoveRight::None));
        assert_eq!(center_path(&f.tab), center_before);
        assert_eq!(f.tab.fwd_history.len(), fwd_len_before);
    }

    // --- tab-index math (the exact functions the methods call) --------------

    #[test]
    fn next_focus_wraps_forward() {
        // 3 tabs: 0 -> 1 -> 2 -> 0.
        assert_eq!(next_focus(0, 3), 1);
        assert_eq!(next_focus(1, 3), 2);
        assert_eq!(next_focus(2, 3), 0); // wraps at the end
    }

    #[test]
    fn next_focus_single_tab_stays_put() {
        assert_eq!(next_focus(0, 1), 0);
    }

    #[test]
    fn focus_after_close_clamps_into_shrunk_range() {
        // Closing the last (focused=2) of 3 tabs (2 remain) steps focus back.
        assert_eq!(focus_after_close(2, 2), 1);
        // Closing an earlier tab (focused=0), 2 remain, keeps index 0.
        assert_eq!(focus_after_close(0, 2), 0);
        // Closing a middle tab (focused=1) of 3 (2 remain) keeps index 1.
        assert_eq!(focus_after_close(1, 2), 1);
        // Down to a single tab: focus must clamp to 0.
        assert_eq!(focus_after_close(1, 1), 0);
        assert_eq!(focus_after_close(0, 1), 0);
    }

    #[test]
    fn can_add_tab_respects_cap() {
        assert!(can_add_tab(1));
        assert!(can_add_tab(MAX_TABS - 1));
        // At the cap, no more tabs may be added.
        assert!(!can_add_tab(MAX_TABS));
        assert!(!can_add_tab(MAX_TABS + 1));
    }
}
