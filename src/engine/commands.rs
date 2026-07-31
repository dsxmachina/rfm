use std::{
    collections::HashMap,
    fmt::Display,
    path::{Path, PathBuf},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use log::{trace, warn};
use patricia_tree::StringPatriciaMap;
use serde::Deserialize;

const DEFAULT_SET_MARK_PREFIX: &str = "m";
const DEFAULT_JUMP_MARK_PREFIX: &str = "'";

/// Map a named special-key binding string to its [`KeyCode`]. These keys don't
/// emit a `Char`, so they can't be matched via the keystroke buffer; the parser
/// binds them as oneshot events instead. Returns `None` for ordinary strings.
fn named_key(s: &str) -> Option<KeyCode> {
    match s {
        "Tab" => Some(KeyCode::Tab),
        "BackTab" => Some(KeyCode::BackTab),
        "Enter" => Some(KeyCode::Enter),
        _ => None,
    }
}

/// Where a binding string lives in the parser.
enum Route {
    /// Modifier chords (`ctrl-`/`alt-`/`meta-`) and named keys (`Tab`, ...):
    /// oneshot events in `mod_commands`.
    Event(KeyEvent),
    /// Everything else: a keystroke pattern in the patricia `key_commands`.
    Pattern(String),
}

/// The single source of truth for binding-string routing, shared by
/// `insert` and `insert_default` so claim checks and insertions can
/// never diverge. Returns `None` for unroutable strings (a bare
/// modifier prefix like `"ctrl-"`, or the empty string — every pattern
/// starts_with(""), so a claimed "" would prefix-drop ALL defaults).
fn route(b: &str) -> Option<Route> {
    if b.is_empty() {
        return None;
    }
    for (prefix, modifier) in [
        ("ctrl-", KeyModifiers::CONTROL),
        ("alt-", KeyModifiers::ALT),
        ("meta-", KeyModifiers::META),
    ] {
        if let Some(key) = b.strip_prefix(prefix) {
            let c = key.chars().next()?;
            return Some(Route::Event(KeyEvent::new(KeyCode::Char(c), modifier)));
        }
    }
    if let Some(code) = named_key(b) {
        return Some(Route::Event(KeyEvent::new(code, KeyModifiers::NONE)));
    }
    Some(Route::Pattern(b.to_string()))
}

/// A default binding that was skipped because its exact pattern was already
/// claimed (by a user binding, a user-defined command, or an earlier default).
#[derive(Debug)]
pub struct DroppedDefault {
    /// Display of the default command that lost.
    pub command: String,
    /// The colliding binding pattern.
    pub binding: String,
    /// Display of the command that claimed it.
    pub kept: String,
    /// True when the claiming binding came from the user (a binding or a
    /// user-defined command), false when an earlier DEFAULT claimed it.
    pub from_user: bool,
}

/// Snapshot of the key space claimed by pass 1 (user bindings + user
/// commands), taken before defaults are inserted — lets `insert_default`
/// tell a user conflict from a default-vs-default one.
struct UserClaims {
    events: std::collections::HashSet<KeyEvent>,
    patterns: std::collections::HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct ExpandedPath(PathBuf);

impl<S: AsRef<str>> From<S> for ExpandedPath {
    fn from(path: S) -> Self {
        let mut string = path.as_ref().to_string();

        // Replace with users home directory
        let home = std::env::var("HOME").unwrap_or_default();

        // Expand "~" and "$HOME"
        string = string.replace('~', &home);
        string = string.replace("$HOME", &home);

        ExpandedPath(string.into())
    }
}

impl AsRef<Path> for ExpandedPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl From<ExpandedPath> for PathBuf {
    fn from(path: ExpandedPath) -> Self {
        path.0
    }
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Manipulation {
    pub change_directory: Option<Vec<String>>,
    pub zoxide_query: Option<Vec<String>>,
    pub rename: Option<Vec<String>>,
    pub mkdir: Option<Vec<String>>,
    pub touch: Option<Vec<String>>,
    pub cut: Option<Vec<String>>,
    pub copy: Option<Vec<String>>,
    pub delete: Option<Vec<String>>,
    pub paste: Option<Vec<String>>,
    pub paste_overwrite: Option<Vec<String>>,
    pub zip: Option<Vec<String>>,
    pub tar: Option<Vec<String>>,
    pub extract: Option<Vec<String>>,
    pub undo: Option<Vec<String>>,
    pub redo: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Movement {
    pub up: Option<Vec<String>>,
    pub down: Option<Vec<String>>,
    pub left: Option<Vec<String>>,
    pub right: Option<Vec<String>>,
    pub top: Option<Vec<String>>,
    pub bottom: Option<Vec<String>>,
    pub page_forward: Option<Vec<String>>,
    pub page_backward: Option<Vec<String>>,
    pub half_page_forward: Option<Vec<String>>,
    pub half_page_backward: Option<Vec<String>>,
    pub jump_previous: Option<Vec<String>>,
    pub jump_to: Option<Vec<(String, String)>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Tabs {
    pub toggle_split: Option<Vec<String>>,
    pub focus_next: Option<Vec<String>>,
    pub new_tab: Option<Vec<String>>,
    pub close_tab: Option<Vec<String>>,
    // Four explicit `focus_tab_N` fields because the config layer is a plain
    // string -> Command table with no argument form; this mirrors the
    // `MAX_TABS = 4` cap in the panel manager. To support a 5th tab, add
    // `focus_tab_5` here (+ its parser line) AND bump `MAX_TABS`.
    pub focus_tab_1: Option<Vec<String>>,
    pub focus_tab_2: Option<Vec<String>>,
    pub focus_tab_3: Option<Vec<String>>,
    pub focus_tab_4: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct JumpMarks {
    /// Prefix key(s) for setting a mark (default: `m`).
    pub set: Option<Vec<String>>,
    /// Prefix key(s) for jumping to a mark (default: `'`).
    pub jump: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct General {
    pub search: Option<Vec<String>>,
    pub mark: Option<Vec<String>>,
    pub next: Option<Vec<String>>,
    pub previous: Option<Vec<String>>,
    pub view_trash: Option<Vec<String>>,
    pub toggle_hidden: Option<Vec<String>>,
    pub toggle_log: Option<Vec<String>>,
    pub quit: Option<Vec<String>>,
    pub quit_no_cd: Option<Vec<String>>,
}

/// Keybinding configuration. Doubles as the *user overlay* type (every
/// field `None` = "use default", `Some(vec![])` = explicit unbind) and,
/// parsed from the embedded defaults file, as the all-`Some` defaults
/// instance.
#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct KeyConfig {
    pub general: General,
    pub movement: Movement,
    pub manipulation: Manipulation,
    pub jump_marks: JumpMarks,
    pub tabs: Tabs,
}

impl KeyConfig {
    /// Completeness guard for the embedded defaults file: asserts that EVERY
    /// binding field is `Some`. The explicit list below IS the guard — when a
    /// new Command/field is added, extend this list AND document the binding
    /// in `examples/default-config.toml`, or the defaults test fails.
    #[cfg(test)]
    pub fn assert_complete(&self) {
        // Exhaustive destructuring (no `..`): adding a field to any section
        // breaks this test's compilation until it is covered below.
        let KeyConfig {
            general,
            movement,
            manipulation,
            jump_marks,
            tabs,
        } = self;
        let General {
            search,
            mark,
            next,
            previous,
            view_trash,
            toggle_hidden,
            toggle_log,
            quit,
            quit_no_cd,
        } = general;
        let Movement {
            up,
            down,
            left,
            right,
            top,
            bottom,
            page_forward,
            page_backward,
            half_page_forward,
            half_page_backward,
            jump_previous,
            jump_to,
        } = movement;
        let Manipulation {
            change_directory,
            zoxide_query,
            rename,
            mkdir,
            touch,
            cut,
            copy,
            delete,
            paste,
            paste_overwrite,
            zip,
            tar,
            extract,
            undo,
            redo,
        } = manipulation;
        let JumpMarks { set, jump } = jump_marks;
        let Tabs {
            toggle_split,
            focus_next,
            new_tab,
            close_tab,
            focus_tab_1,
            focus_tab_2,
            focus_tab_3,
            focus_tab_4,
        } = tabs;

        macro_rules! req {
            ($section:ident: $($f:ident),+ $(,)?) => {
                $(assert!(
                    $f.is_some(),
                    concat!("default missing: ", stringify!($section), ".", stringify!($f))
                );)+
            }
        }
        req!(general: search, mark, next, previous, view_trash, toggle_hidden,
            toggle_log, quit, quit_no_cd);
        req!(movement: up, down, left, right, top, bottom, page_forward,
            page_backward, half_page_forward, half_page_backward,
            jump_previous, jump_to);
        req!(manipulation: change_directory, zoxide_query, rename, mkdir,
            touch, cut, copy, delete, paste, paste_overwrite, zip, tar,
            extract, undo, redo);
        req!(jump_marks: set, jump);
        req!(tabs: toggle_split, focus_next, new_tab, close_tab, focus_tab_1,
            focus_tab_2, focus_tab_3, focus_tab_4);
    }
}

#[test]
fn test_split() {
    let s = "ctrl-f";
    let (_, key) = s.split_at(5);
    assert_eq!(key, "f");
}

#[derive(Debug, Clone)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    PageForward,
    PageBackward,
    HalfPageForward,
    HalfPageBackward,
    JumpTo(ExpandedPath),
    JumpPrevious,
}

/// Set of commands that the filemanager should perform during its runtime
#[derive(Debug, Clone)]
pub enum Command {
    Move(Move),
    Next,
    Previous,
    ToggleHidden,
    ToggleLog,
    ViewTrash,
    Zip,
    Tar,
    Extract,
    Cd {
        zoxide: bool,
    },
    Search,
    Rename,
    Mkdir,
    Touch,
    Cut,
    Copy,
    Delete,
    Paste {
        overwrite: bool,
    },
    Mark,
    /// Set a jump-mark (session-only) at the current location.
    SetJumpMark(char),
    /// Jump to a previously-set jump-mark.
    JumpToMark(char),
    Undo,
    Redo,
    /// Toggle split-view (stub until Task 7).
    ToggleSplit,
    /// Cycle focus forward through the open tabs.
    FocusNext,
    /// Open a new tab rooted at the focused tab's cwd.
    NewTab,
    /// Close the focused tab.
    CloseTab,
    /// Focus the `n`-th tab (1-based as configured; 0-based at dispatch).
    FocusTab(usize),
    Quit,
    QuitWithoutPath,
    /// User-defined shell command
    UserCommand {
        /// Display name
        name: String,
        /// Shell command template (with $@ placeholder)
        cmd: String,
        /// Run in foreground (suspend terminal)
        interactive: bool,
        /// Separator for $@ expansion
        separator: String,
    },
    None,
}

impl Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Command::Move(m) => match m {
                Move::Up => write!(f, "move up"),
                Move::Down => write!(f, "move down"),
                Move::Left => write!(f, "move left"),
                Move::Right => write!(f, "move right"),
                Move::Top => write!(f, "move to top"),
                Move::Bottom => write!(f, "move to bottom"),
                Move::PageForward => write!(f, "page forward"),
                Move::PageBackward => write!(f, "page backward"),
                Move::HalfPageForward => write!(f, "half page forward"),
                Move::HalfPageBackward => write!(f, "half page backward"),
                Move::JumpTo(path) => write!(f, "{}", path.0.display()),
                Move::JumpPrevious => write!(f, "jump back"),
            },
            Command::Next => write!(f, "next match"),
            Command::Previous => write!(f, "previous match"),
            Command::ToggleHidden => write!(f, "toggle hidden files"),
            Command::ToggleLog => write!(f, "toggle developer log"),
            Command::ViewTrash => write!(f, "go to trash"),
            Command::Zip => write!(f, "zip selected items"),
            Command::Tar => write!(f, "tar selected items"),
            Command::Extract => write!(f, "extract selected archive"),
            Command::Cd { .. } => write!(f, "enter 'cd' mode"),
            Command::Search => write!(f, "search for items"),
            Command::Rename => write!(f, "rename selected items"),
            Command::Mkdir => write!(f, "create a new directory"),
            Command::Touch => write!(f, "create a new file"),
            Command::Cut => write!(f, "cut selected items"),
            Command::Copy => write!(f, "copy selected items"),
            Command::Delete => write!(f, "delete selected items"),
            Command::Paste { overwrite } => {
                if *overwrite {
                    write!(f, "paste and overwrite")
                } else {
                    write!(f, "paste without overwrite")
                }
            }
            Command::Mark => write!(f, "mark selected item"),
            Command::SetJumpMark(c) => write!(f, "set jump-mark '{c}'"),
            Command::JumpToMark(c) => write!(f, "jump to mark '{c}'"),
            Command::Undo => write!(f, "undo"),
            Command::Redo => write!(f, "redo"),
            Command::ToggleSplit => write!(f, "toggle split view"),
            Command::FocusNext => write!(f, "focus next tab"),
            Command::NewTab => write!(f, "new tab"),
            Command::CloseTab => write!(f, "close tab"),
            Command::FocusTab(n) => write!(f, "focus tab {n}"),
            Command::Quit => write!(f, "quit"),
            Command::QuitWithoutPath => write!(f, "quit without changing path"),
            Command::UserCommand { name, .. } => write!(f, "{}", name),
            Command::None => write!(f, "no command"),
        }
    }
}

/// Set of commands that the filemanager should perform just before closing
pub enum CloseCmd {
    QuitWithPath { path: PathBuf },
    QuitErr { error: &'static str },
    Quit,
}

/// Takes the incoming key-events, and returns the corresponding command.
///
/// Uses a `StringPatriciaMap` to match patterns of keystrokes,
/// and a normal `HashMap` to match "oneshot"-commands,
/// that don't require any key combinations but may require a modifier.
pub struct CommandParser {
    key_commands: StringPatriciaMap<Command>,
    mod_commands: HashMap<KeyEvent, Command>,
    buffer: String,
}

impl CommandParser {
    /// Pass 1: user bindings + user-defined commands (they claim key space).
    /// Pass 2: defaults for every field the user did not mention; a default
    /// binding is dropped + reported when its pattern collides with a user
    /// claim exactly or as a strict prefix (in either direction) — see
    /// [`Self::insert_default`]. Prefix overlaps among the defaults
    /// themselves are fine (chord deferral, only for the jump-mark chords).
    pub fn build(
        defaults: &KeyConfig,
        user: &KeyConfig,
        user_commands: &crate::command_queue::CommandsConfig,
    ) -> (Self, Vec<DroppedDefault>) {
        let mut parser = CommandParser::new();
        let mut dropped = Vec::new();

        // The fixed field table: (user field, default field, command).
        // `jump_to` and the jump-mark prefixes are handled separately below.
        let table = [
            (&user.general.search, &defaults.general.search, Command::Search),
            (&user.general.mark, &defaults.general.mark, Command::Mark),
            (&user.general.next, &defaults.general.next, Command::Next),
            (&user.general.previous, &defaults.general.previous, Command::Previous),
            (&user.general.toggle_hidden, &defaults.general.toggle_hidden, Command::ToggleHidden),
            (&user.general.toggle_log, &defaults.general.toggle_log, Command::ToggleLog),
            (&user.general.view_trash, &defaults.general.view_trash, Command::ViewTrash),
            (&user.general.quit, &defaults.general.quit, Command::Quit),
            (&user.general.quit_no_cd, &defaults.general.quit_no_cd, Command::QuitWithoutPath),
            (&user.movement.up, &defaults.movement.up, Command::Move(Move::Up)),
            (&user.movement.down, &defaults.movement.down, Command::Move(Move::Down)),
            (&user.movement.left, &defaults.movement.left, Command::Move(Move::Left)),
            (&user.movement.right, &defaults.movement.right, Command::Move(Move::Right)),
            (&user.movement.top, &defaults.movement.top, Command::Move(Move::Top)),
            (&user.movement.bottom, &defaults.movement.bottom, Command::Move(Move::Bottom)),
            (
                &user.movement.page_forward,
                &defaults.movement.page_forward,
                Command::Move(Move::PageForward),
            ),
            (
                &user.movement.page_backward,
                &defaults.movement.page_backward,
                Command::Move(Move::PageBackward),
            ),
            (
                &user.movement.half_page_forward,
                &defaults.movement.half_page_forward,
                Command::Move(Move::HalfPageForward),
            ),
            (
                &user.movement.half_page_backward,
                &defaults.movement.half_page_backward,
                Command::Move(Move::HalfPageBackward),
            ),
            (
                &user.movement.jump_previous,
                &defaults.movement.jump_previous,
                Command::Move(Move::JumpPrevious),
            ),
            (
                &user.manipulation.change_directory,
                &defaults.manipulation.change_directory,
                Command::Cd { zoxide: false },
            ),
            (
                &user.manipulation.zoxide_query,
                &defaults.manipulation.zoxide_query,
                Command::Cd { zoxide: true },
            ),
            (&user.manipulation.rename, &defaults.manipulation.rename, Command::Rename),
            (&user.manipulation.mkdir, &defaults.manipulation.mkdir, Command::Mkdir),
            (&user.manipulation.touch, &defaults.manipulation.touch, Command::Touch),
            (&user.manipulation.cut, &defaults.manipulation.cut, Command::Cut),
            (&user.manipulation.copy, &defaults.manipulation.copy, Command::Copy),
            (&user.manipulation.delete, &defaults.manipulation.delete, Command::Delete),
            (
                &user.manipulation.paste,
                &defaults.manipulation.paste,
                Command::Paste { overwrite: false },
            ),
            (
                &user.manipulation.paste_overwrite,
                &defaults.manipulation.paste_overwrite,
                Command::Paste { overwrite: true },
            ),
            (&user.manipulation.zip, &defaults.manipulation.zip, Command::Zip),
            (&user.manipulation.tar, &defaults.manipulation.tar, Command::Tar),
            (&user.manipulation.extract, &defaults.manipulation.extract, Command::Extract),
            (&user.manipulation.undo, &defaults.manipulation.undo, Command::Undo),
            (&user.manipulation.redo, &defaults.manipulation.redo, Command::Redo),
            (&user.tabs.toggle_split, &defaults.tabs.toggle_split, Command::ToggleSplit),
            (&user.tabs.focus_next, &defaults.tabs.focus_next, Command::FocusNext),
            (&user.tabs.new_tab, &defaults.tabs.new_tab, Command::NewTab),
            (&user.tabs.close_tab, &defaults.tabs.close_tab, Command::CloseTab),
            (&user.tabs.focus_tab_1, &defaults.tabs.focus_tab_1, Command::FocusTab(1)),
            (&user.tabs.focus_tab_2, &defaults.tabs.focus_tab_2, Command::FocusTab(2)),
            (&user.tabs.focus_tab_3, &defaults.tabs.focus_tab_3, Command::FocusTab(3)),
            (&user.tabs.focus_tab_4, &defaults.tabs.focus_tab_4, Command::FocusTab(4)),
        ];

        // --- Pass 1: user bindings claim key space. `Some(vec![])` is an
        // explicit unbind: it inserts nothing, but still marks the command
        // as user-mentioned so pass 2 skips its defaults.
        for (user_field, _, cmd) in &table {
            if let Some(bindings) = user_field {
                parser.insert(bindings.clone(), cmd.clone());
            }
        }
        if let Some(jumps) = &user.movement.jump_to {
            for (keys, path) in jumps {
                parser
                    .key_commands
                    .insert(keys.clone(), Command::Move(Move::JumpTo(path.into())));
            }
        }
        parser.add_user_commands(user_commands);

        // Everything claimed so far came from the user (the arrow-key
        // built-ins from `new()` are unreachable via `route`, so they can
        // never collide with a default binding string).
        let user_claims = UserClaims {
            events: parser.mod_commands.keys().copied().collect(),
            patterns: parser.key_commands.keys().collect(),
        };

        // --- Pass 2: defaults for everything the user did not mention. An
        // exact pattern collision with a claimed key drops the default.
        for (user_field, default_field, cmd) in &table {
            if user_field.is_none() {
                for b in default_field.iter().flatten() {
                    parser.insert_default(b, cmd, &user_claims, &mut dropped);
                }
            }
        }
        // A user `jump_to` list replaces the whole default list; with no user
        // list, default entries are inserted individually so a single claimed
        // chord does not take the rest of the defaults down with it.
        if user.movement.jump_to.is_none() {
            for (keys, path) in defaults.movement.jump_to.iter().flatten() {
                parser.insert_default(
                    keys,
                    &Command::Move(Move::JumpTo(path.into())),
                    &user_claims,
                    &mut dropped,
                );
            }
        }

        // --- Jump-mark chords run last; `insert_chords` already skips
        // claimed patterns. Prefixes: user value if mentioned, else the
        // defaults' (all-`Some` by the completeness guard).
        let set = user
            .jump_marks
            .set
            .clone()
            .or_else(|| defaults.jump_marks.set.clone())
            .unwrap_or_else(|| vec![DEFAULT_SET_MARK_PREFIX.to_string()]);
        let jump = user
            .jump_marks
            .jump
            .clone()
            .or_else(|| defaults.jump_marks.jump.clone())
            .unwrap_or_else(|| vec![DEFAULT_JUMP_MARK_PREFIX.to_string()]);
        parser.insert_jump_marks(set, jump);

        (parser, dropped)
    }

    /// Add user-defined commands from config (pass 1: they claim key space).
    fn add_user_commands(&mut self, commands: &crate::command_queue::CommandsConfig) {
        for (name, entry) in commands {
            let cmd = Command::UserCommand {
                name: name.clone(),
                cmd: entry.cmd.clone(),
                interactive: entry.interactive,
                separator: entry.separator.clone(),
            };
            self.insert(entry.keys.clone(), cmd);
        }
    }

    /// Generate the `<set-prefix><a-z>` and `<jump-prefix><a-z>` chords for
    /// jump-marks. Prefixes are usually a single key (`m` / `'`) but any
    /// string works, mirroring `jump_to`.
    fn insert_jump_marks(&mut self, set: Vec<String>, jump: Vec<String>) {
        self.insert_chords(set, Command::SetJumpMark);
        self.insert_chords(jump, Command::JumpToMark);
    }

    /// Insert `<prefix><a-z>` chords, each producing `make(letter)`.
    /// Explicit bindings on the same keys are left untouched — the
    /// auto-generated chords always lose against them.
    fn insert_chords(&mut self, prefixes: Vec<String>, make: fn(char) -> Command) {
        for prefix in prefixes {
            for c in 'a'..='z' {
                let chord = format!("{prefix}{c}");
                if self.key_commands.get(&chord).is_none() {
                    self.key_commands.insert(chord, make(c));
                }
            }
        }
    }

    pub fn new() -> Self {
        let mut mod_commands = HashMap::new();
        // Insert basic arrow key movement
        mod_commands.insert(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            Command::Move(Move::Up),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Command::Move(Move::Down),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
            Command::Move(Move::Left),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            Command::Move(Move::Right),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
            Command::Move(Move::PageBackward),
        );
        mod_commands.insert(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            Command::Move(Move::PageForward),
        );
        CommandParser {
            key_commands: StringPatriciaMap::new(),
            mod_commands,
            buffer: "".to_string(),
        }
    }

    fn insert(&mut self, bindings: Vec<String>, cmd: Command) {
        for b in bindings {
            match route(&b) {
                Some(Route::Event(event)) => {
                    self.mod_commands.insert(event, cmd.clone());
                }
                Some(Route::Pattern(pattern)) => {
                    self.key_commands.insert(pattern, cmd.clone());
                }
                // route() rejects the empty string and bare modifier
                // prefixes like "ctrl-"; silence would hide the user's typo.
                None => warn!("ignoring malformed keybinding '{b}'"),
            }
        }
    }

    /// Insert a single *default* binding unless it collides with a claimed
    /// pattern: exactly (against a user binding, a user command, or an
    /// earlier default), or as a strict prefix in either direction against a
    /// USER claim — the matcher fires exact matches immediately, so either
    /// prefix relation would make one of the two bindings unreachable, and
    /// the user's must win. Default-vs-default prefix overlaps are left
    /// alone (the shipped defaults rely on chord deferral, e.g. `''`/`'a`).
    /// A collision is reported in `dropped` instead. Uses the same [`route`]
    /// as [`Self::insert`], so the two cannot diverge.
    fn insert_default(
        &mut self,
        binding: &str,
        cmd: &Command,
        user_claims: &UserClaims,
        dropped: &mut Vec<DroppedDefault>,
    ) {
        let conflict = |kept: &Command, from_user: bool| DroppedDefault {
            command: cmd.to_string(),
            binding: binding.to_string(),
            kept: kept.to_string(),
            from_user,
        };
        match route(binding) {
            Some(Route::Event(event)) => {
                if let Some(kept) = self.mod_commands.get(&event) {
                    dropped.push(conflict(kept, user_claims.events.contains(&event)));
                } else {
                    self.mod_commands.insert(event, cmd.clone());
                }
            }
            Some(Route::Pattern(pattern)) => {
                if let Some(kept) = self.key_commands.get(&pattern) {
                    dropped.push(conflict(kept, user_claims.patterns.contains(&pattern)));
                } else if let Some(kept) = user_claims
                    .patterns
                    .iter()
                    .find(|claim| {
                        claim.as_str() != pattern
                            && (claim.starts_with(&pattern) || pattern.starts_with(claim.as_str()))
                    })
                    .and_then(|claim| self.key_commands.get(claim))
                {
                    // Strict-prefix collision with a user claim: whichever
                    // pattern is shorter would fire first and make the other
                    // unreachable — drop the default either way.
                    dropped.push(conflict(kept, true));
                } else {
                    self.key_commands.insert(pattern, cmd.clone());
                }
            }
            None => warn!("ignoring malformed keybinding '{binding}'"),
        }
    }

    pub fn buffer(&self) -> String {
        self.buffer.clone()
    }

    pub fn matching_commands(&self) -> Vec<(String, String)> {
        if self.buffer.is_empty() {
            Vec::new()
        } else {
            self.key_commands
                .iter_prefix(&self.buffer)
                .filter(|(k, v)| {
                    // A jump-mark chord shadowed by a longer binding can
                    // never fire (see add_event) — don't advertise it.
                    !(matches!(v, Command::SetJumpMark(_) | Command::JumpToMark(_))
                        && self.key_commands.iter_prefix(k.as_str()).count() > 1)
                })
                .map(|(k, v)| (k.clone(), v.to_string()))
                .collect()
        }
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    /// Parse an event and return the command that is assigned to it
    pub fn add_event(&mut self, event: KeyEvent) -> Command {
        if let KeyCode::Backspace = event.code {
            self.buffer.pop();
            return Command::None;
        }
        match event.modifiers {
            // First parse for "normal" characters:
            KeyModifiers::NONE | KeyModifiers::SHIFT => {
                // Put character into buffer
                if let KeyCode::Char(c) = event.code {
                    if event.modifiers.contains(KeyModifiers::SHIFT) {
                        // uppercase
                        self.buffer.push(c.to_ascii_uppercase());
                    } else {
                        // lowercase
                        self.buffer.push(c.to_ascii_lowercase());
                    }
                }

                // Check if there are commands with that prefix
                if self.key_commands.iter_prefix(&self.buffer).count() == 0 {
                    self.buffer.clear();
                    return Command::None;
                }

                // Check if we have a valid command
                if let Some(command) = self.key_commands.get(&self.buffer) {
                    // Jump-mark chords are auto-generated over the whole
                    // alphabet, so they must not shadow explicit bindings:
                    // if a longer binding shares this prefix (`mkdir` over
                    // `mk`), keep collecting keys and let the binding win.
                    if matches!(command, Command::SetJumpMark(_) | Command::JumpToMark(_))
                        && self.key_commands.iter_prefix(&self.buffer).count() > 1
                    {
                        return Command::None;
                    }
                    self.buffer.clear();
                    trace!("Command: {:?}", command);
                    return command.clone();
                }
            }
            _ => {}
        }
        // If we have not returned yet,
        // always check if there is a oneshot command assigned to the
        // incoming event.
        if let Some(command) = self.mod_commands.get(&event) {
            self.buffer.clear();
            trace!("Command: {:?}", command);
            return command.clone();
        }
        Command::None
    }
}

/// Test helper: the all-`Some` defaults instance parsed from the embedded
/// default-config.toml (completeness guaranteed by `assert_complete`).
#[cfg(test)]
fn defaults() -> KeyConfig {
    let config: crate::config::Config = crate::config::default_tree().try_into().unwrap();
    config.keys
}

/// Test helper: a parser built purely from the defaults (no user overlay).
#[cfg(test)]
fn default_parser() -> CommandParser {
    CommandParser::build(&defaults(), &KeyConfig::default(), &Default::default()).0
}

#[cfg(test)]
mod jump_mark_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn m_then_letter_sets_mark() {
        let mut p = default_parser();
        assert!(matches!(p.add_event(key('m')), Command::None)); // waits
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
    }

    #[test]
    fn apostrophe_then_letter_jumps_to_mark() {
        let mut p = default_parser();
        assert!(matches!(p.add_event(key('\'')), Command::None)); // waits
        assert!(matches!(p.add_event(key('a')), Command::JumpToMark('a')));
    }

    #[test]
    fn double_apostrophe_still_jumps_previous() {
        let mut p = default_parser();
        assert!(matches!(p.add_event(key('\'')), Command::None));
        assert!(matches!(
            p.add_event(key('\'')),
            Command::Move(Move::JumpPrevious)
        ));
    }

    #[test]
    fn longer_binding_wins_over_mark_chord() {
        let mut p = default_parser();
        // "mkdir" shares the "mk" prefix with the auto-generated
        // SetJumpMark('k') chord; the explicit binding must win.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('k')), Command::None)); // defers, no mark
        assert!(matches!(p.add_event(key('d')), Command::None));
        assert!(matches!(p.add_event(key('i')), Command::None));
        assert!(matches!(p.add_event(key('r')), Command::Mkdir));
    }

    #[test]
    fn deferred_mark_chord_does_not_fire_on_mismatch() {
        let mut p = default_parser();
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('k')), Command::None)); // deferred
        assert!(matches!(p.add_event(key('x')), Command::None)); // buffer cleared, no mark
        assert!(matches!(p.add_event(key('j')), Command::Move(Move::Down)));
    }

    #[test]
    fn unshadowed_mark_chord_still_fires_from_defaults() {
        let mut p = default_parser();
        // No default binding starts with "ma", so the chord fires normally.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
    }

    #[test]
    fn mark_chord_does_not_clobber_equal_length_binding() {
        let toml = r#"
[general]
search = ["/"]
mark = [" "]
next = ["n"]
previous = ["N"]
view_trash = ["gT"]
toggle_hidden = ["zh"]
quit = ["q"]

[movement]
up = ["k"]
down = ["j"]
left = ["h"]
right = ["l"]
top = ["gg"]
bottom = ["G"]
page_forward = ["ctrl-f"]
page_backward = ["ctrl-b"]
half_page_forward = ["ctrl-d"]
half_page_backward = ["ctrl-u"]
jump_previous = ["''"]
jump_to = [["ma", "/tmp"]]

[manipulation]
rename = ["rename"]
mkdir = ["mkdir"]
touch = ["touch"]
cut = ["dd"]
copy = ["yy"]
delete = ["delete"]
paste = ["pp"]
paste_overwrite = ["po"]
zip = ["zip"]
tar = ["tar"]
extract = ["extract"]
"#;
        let cfg: KeyConfig = toml::from_str(toml).expect("parse KeyConfig");
        let mut p = CommandParser::build(&defaults(), &cfg, &Default::default()).0;
        // "ma" is an explicit jump_to binding; the auto-generated
        // SetJumpMark('a') chord must not overwrite it.
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(
            p.add_event(key('a')),
            Command::Move(Move::JumpTo(_))
        ));
    }

    #[test]
    fn shadowed_mark_chord_hidden_from_matching_commands() {
        let mut p = default_parser();
        p.add_event(key('m'));
        let keys: Vec<String> = p.matching_commands().into_iter().map(|(k, _)| k).collect();
        assert!(keys.iter().any(|k| k == "mkdir"));
        assert!(keys.iter().any(|k| k == "ma"));
        assert!(!keys.iter().any(|k| k == "mk")); // dead chord, don't advertise
    }

    #[test]
    fn m_then_unbound_key_is_noop() {
        let mut p = default_parser();
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('1')), Command::None)); // buffer cleared
        assert!(matches!(p.add_event(key('j')), Command::Move(Move::Down)));
    }

    #[test]
    fn user_config_without_jump_marks_section_defaults_to_m_and_apostrophe() {
        let toml = r#"
[general]
search = ["/"]
mark = [" "]
next = ["n"]
previous = ["N"]
view_trash = ["gT"]
toggle_hidden = ["zh"]
quit = ["q"]

[movement]
up = ["k"]
down = ["j"]
left = ["h"]
right = ["l"]
top = ["gg"]
bottom = ["G"]
page_forward = ["ctrl-f"]
page_backward = ["ctrl-b"]
half_page_forward = ["ctrl-d"]
half_page_backward = ["ctrl-u"]
jump_previous = ["''"]
jump_to = []

[manipulation]
rename = ["rename"]
mkdir = ["mkdir"]
touch = ["touch"]
cut = ["dd"]
copy = ["yy"]
delete = ["delete"]
paste = ["pp"]
paste_overwrite = ["po"]
zip = ["zip"]
tar = ["tar"]
extract = ["extract"]
"#;
        let cfg: KeyConfig = toml::from_str(toml).expect("parse KeyConfig");
        let mut p = CommandParser::build(&defaults(), &cfg, &Default::default()).0;
        assert!(matches!(p.add_event(key('m')), Command::None));
        assert!(matches!(p.add_event(key('a')), Command::SetJumpMark('a')));
        assert!(matches!(p.add_event(key('\'')), Command::None));
        assert!(matches!(p.add_event(key('b')), Command::JumpToMark('b')));
    }

    /// The `[tabs]` section wires its keys to the multi-tab commands: a
    /// single-key focus jump, two aliases (a plain key and a `ctrl-` chord)
    /// for `close_tab`, and `Tab` routed through the oneshot `mod_commands`
    /// map via the `named_key()` path.
    #[test]
    fn tabs_section_wires_focus_close_and_named_tab_key() {
        let toml = r#"
[general]
search = ["/"]
mark = [" "]
next = ["e"]
previous = ["N"]
view_trash = ["gT"]
toggle_hidden = ["zh"]
quit = ["Q"]

[movement]
up = ["k"]
down = ["j"]
left = ["h"]
right = ["l"]
top = ["gg"]
bottom = ["G"]
page_forward = ["ctrl-f"]
page_backward = ["ctrl-b"]
half_page_forward = ["ctrl-d"]
half_page_backward = ["ctrl-u"]
jump_previous = ["''"]
jump_to = []

[manipulation]
rename = ["rename"]
mkdir = ["mkdir"]
touch = ["touch"]
cut = ["dd"]
copy = ["yy"]
delete = ["delete"]
paste = ["pp"]
paste_overwrite = ["po"]
zip = ["zip"]
tar = ["tar"]
extract = ["extract"]

[tabs]
focus_next = ["Tab"]
new_tab = ["gn"]
close_tab = ["q", "ctrl-w"]
focus_tab_1 = ["1"]
"#;
        let cfg: KeyConfig = toml::from_str(toml).expect("parse KeyConfig");
        let mut p = CommandParser::build(&defaults(), &cfg, &Default::default()).0;

        // focus_tab_1 = ["1"] -> FocusTab(1)
        assert!(matches!(p.add_event(key('1')), Command::FocusTab(1)));

        // close_tab = ["q", "ctrl-w"]: the plain 'q' key ...
        assert!(matches!(p.add_event(key('q')), Command::CloseTab));
        // ... and the ctrl-w chord both map to CloseTab.
        let ctrl_w = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert!(matches!(p.add_event(ctrl_w), Command::CloseTab));

        // focus_next = ["Tab"] must route through the oneshot map as a bare
        // KeyCode::Tab (the named_key() path), since Tab emits no Char and so
        // never flows through the keystroke buffer.
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
        assert!(matches!(p.add_event(tab), Command::FocusNext));

        // new_tab = ["gn"] still resolves as a two-key chord.
        assert!(matches!(p.add_event(key('g')), Command::None));
        assert!(matches!(p.add_event(key('n')), Command::NewTab));
    }

}

#[cfg(test)]
mod builder_tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn press(p: &mut CommandParser, c: char) -> Command {
        p.add_event(key(c))
    }

    fn press2(p: &mut CommandParser, a: char, b: char) -> Command {
        p.add_event(key(a));
        p.add_event(key(b))
    }

    #[test]
    fn empty_user_config_gets_all_defaults() {
        let (mut p, dropped) =
            CommandParser::build(&defaults(), &KeyConfig::default(), &Default::default());
        assert!(dropped.is_empty());
        assert!(matches!(press(&mut p, 'u'), Command::Undo)); // new-feature default
        assert!(matches!(press(&mut p, '!'), Command::ToggleSplit)); // opt-in no more
    }

    #[test]
    fn user_binding_wins_default_dropped_and_reported() {
        // old-school config: q means quit
        let user: KeyConfig = toml::from_str("[general]\nquit = [\"q\", \"exit\"]").unwrap();
        let (mut p, dropped) = CommandParser::build(&defaults(), &user, &Default::default());
        assert!(matches!(press(&mut p, 'q'), Command::Quit)); // user wins
        assert!(dropped
            .iter()
            .any(|d| d.binding == "q" && d.command.contains("close") && d.from_user));
        // the non-conflicting half of the default survives:
        assert!(p
            .mod_commands
            .contains_key(&KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL)));
    }

    #[test]
    fn explicit_empty_list_unbinds_and_blocks_default() {
        let user: KeyConfig = toml::from_str("[manipulation]\nundo = []").unwrap();
        let (mut p, dropped) = CommandParser::build(&defaults(), &user, &Default::default());
        assert!(matches!(press(&mut p, 'u'), Command::None));
        assert!(dropped.is_empty()); // an unbind is not a conflict
    }

    #[test]
    fn user_command_keys_claim_before_defaults() {
        let mut cmds = crate::command_queue::CommandsConfig::default();
        cmds.insert(
            "mine".into(),
            toml::from_str("keys = [\"u\"]\ncmd = \"true\"").unwrap(),
        );
        let (mut p, dropped) = CommandParser::build(&defaults(), &KeyConfig::default(), &cmds);
        assert!(matches!(press(&mut p, 'u'), Command::UserCommand { .. }));
        assert!(dropped.iter().any(|d| d.binding == "u" && d.from_user));
    }

    #[test]
    fn key_config_parses_from_empty_and_partial_toml() {
        // empty: every field None
        let empty: KeyConfig = toml::from_str("").unwrap();
        assert!(empty.movement.up.is_none());
        assert!(empty.manipulation.undo.is_none());

        // partial: only what is written is Some; [] stays Some(empty)
        let partial: KeyConfig =
            toml::from_str("[manipulation]\nundo = []\n[movement]\nup = [\"k\"]").unwrap();
        assert_eq!(partial.movement.up, Some(vec!["k".into()]));
        assert_eq!(partial.manipulation.undo, Some(vec![]));
        assert!(partial.movement.down.is_none());
    }

    #[test]
    fn default_prefix_of_user_chord_is_dropped() {
        // user binds "ff"; default search has "f" (a strict prefix) → dropped + reported
        let mut cmds = crate::command_queue::CommandsConfig::default();
        cmds.insert(
            "mine".into(),
            toml::from_str("keys = [\"ff\"]\ncmd = \"true\"").unwrap(),
        );
        let (mut p, dropped) = CommandParser::build(&defaults(), &KeyConfig::default(), &cmds);
        assert!(dropped.iter().any(|d| d.binding == "f" && d.from_user));
        assert!(matches!(press2(&mut p, 'f', 'f'), Command::UserCommand { .. })); // chord reachable now
        // "/" and "search" (other search defaults, no prefix relation) must survive:
        assert!(matches!(press(&mut p, '/'), Command::Search));
    }

    #[test]
    fn user_pattern_prefix_of_default_drops_the_default() {
        // user binds bare "c"; defaults "cut"/"copy"/"cd" become unreachable → dropped
        let mut cmds = crate::command_queue::CommandsConfig::default();
        cmds.insert(
            "mine".into(),
            toml::from_str("keys = [\"c\"]\ncmd = \"true\"").unwrap(),
        );
        let (mut p, dropped) = CommandParser::build(&defaults(), &KeyConfig::default(), &cmds);
        assert!(matches!(press(&mut p, 'c'), Command::UserCommand { .. }));
        assert!(dropped.iter().any(|d| d.binding == "cut"));
        assert!(dropped.iter().any(|d| d.binding == "cd"));
        // unrelated defaults survive:
        assert!(matches!(press(&mut p, 'u'), Command::Undo));
    }

    #[test]
    fn empty_binding_string_is_rejected_not_mass_dropping() {
        // A typo like `search = [""]` must not claim the empty pattern:
        // every pattern starts_with(""), so a claimed "" would prefix-drop
        // ALL defaults and leave the keyboard dead.
        let user: KeyConfig = toml::from_str("[general]\nsearch = [\"\"]").unwrap();
        let (mut p, dropped) = CommandParser::build(&defaults(), &user, &Default::default());
        assert!(matches!(press(&mut p, 'j'), Command::Move(Move::Down)));
        assert!(!dropped.iter().any(|d| d.binding == "j"));
    }

    #[test]
    fn user_jump_to_replaces_whole_default_list() {
        let user: KeyConfig = toml::from_str("[movement]\njump_to = [[\"gz\", \"/tmp\"]]").unwrap();
        let (mut p, _) = CommandParser::build(&defaults(), &user, &Default::default());
        assert!(matches!(
            press2(&mut p, 'g', 'z'),
            Command::Move(Move::JumpTo(_))
        ));
        assert!(matches!(press2(&mut p, 'g', 'h'), Command::None)); // default list gone
    }
}
