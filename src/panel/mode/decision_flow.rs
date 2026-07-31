//! The generic decision-flow overlay: a list of decision items — each with a
//! prompt, a few keyed choices, and optionally a safe default — answered one
//! keypress per item.
//!
//! Pure adapter — a consumer (via the manager) builds the item list at
//! construction; the answers travel back wholesale in
//! [`ModeOp::FlowResolved`], dispatched per [`FlowKind`]. The adapter itself
//! never interprets an answer.

use std::io::Stdout;
use std::ops::Range;

use crossterm::event::{KeyCode, KeyEvent};
use crossterm::style::PrintStyledContent;
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, queue, style::Stylize, Result};
use unicode_display_width::width as unicode_width;

use super::{ModalInput, ModalRegion, ModeOp};
use crate::config::color::{
    color_highlight, color_main, color_marked, print_horizontal_bar, print_horz_bot, print_horz_top,
};
use crate::panel::Draw;
use crate::util::ExactWidth;

/// Which consumer launched the flow; the manager dispatches results on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    UpgradeNotice,
    // future: Onboarding, BulkRenamePreview, ...
}

/// One keyed answer option of a [`DecisionItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub key: char,
    pub label: String,
}

/// One decision: a prompt, optional detail lines, and 2–4 choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionItem {
    pub prompt: String,
    pub detail: Vec<String>,
    /// 2..=4 enforced by [`DecisionFlow::new`].
    pub choices: Vec<Choice>,
    /// Preselected choice (`Enter` accepts it); `None` = user must answer.
    pub default: Option<usize>,
}

pub struct DecisionFlow {
    kind: FlowKind,
    title: String,
    items: Vec<DecisionItem>,
    cursor: usize,
    answers: Vec<Option<usize>>,
}

impl DecisionFlow {
    /// Panics on an empty item list or an item with fewer than 2 / more than
    /// 4 choices. Builders must avoid `'j'`/`'q'` as choice keys (they could
    /// never navigate/leave otherwise; `debug_assert`ed) — `'k'` is allowed
    /// because choice keys win over navigation on the current item, and the
    /// arrow keys always navigate.
    pub fn new(kind: FlowKind, title: String, items: Vec<DecisionItem>) -> Self {
        assert!(!items.is_empty(), "decision flow needs at least one item");
        for item in &items {
            assert!(
                (2..=4).contains(&item.choices.len()),
                "decision item needs 2..=4 choices"
            );
            debug_assert!(
                item.choices.iter().all(|c| c.key != 'j' && c.key != 'q'),
                "'j'/'q' are reserved for navigation/close"
            );
            if let Some(d) = item.default {
                assert!(d < item.choices.len(), "default index out of choice range");
            }
            debug_assert!(
                item.choices
                    .iter()
                    .enumerate()
                    .all(|(i, c)| item.choices[..i].iter().all(|prev| prev.key != c.key)),
                "duplicate choice keys within one item"
            );
        }
        let answers = vec![None; items.len()];
        Self {
            kind,
            title,
            items,
            cursor: 0,
            answers,
        }
    }

    /// The item the cursor is on. (Test-only so far — rendering reads the
    /// fields directly; consumers only ever see the resolved answers.)
    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Chosen choice index per item; `None` = still unanswered. (Test-only,
    /// see [`Self::cursor`].)
    #[cfg(test)]
    pub fn answers(&self) -> &[Option<usize>] {
        &self.answers
    }

    /// The flow's items, as built. (Test-only, see [`Self::cursor`].)
    #[cfg(test)]
    pub fn items(&self) -> &[DecisionItem] {
        &self.items
    }

    #[cfg(test)]
    fn items_mut_for_test(&mut self) -> &mut [DecisionItem] {
        &mut self.items
    }

    /// Record `choice` for the current item, then hand over to
    /// [`Self::advance_or_resolve`].
    fn answer(&mut self, choice: usize) -> ModeOp {
        self.answers[self.cursor] = Some(choice);
        self.advance_or_resolve()
    }

    /// Move the cursor to the next unanswered item (wrapping search from
    /// cursor+1); with none left the flow resolves.
    fn advance_or_resolve(&mut self) -> ModeOp {
        let n = self.items.len();
        let next = (1..=n)
            .map(|d| (self.cursor + d) % n)
            .find(|&i| self.answers[i].is_none());
        match next {
            Some(i) => {
                self.cursor = i;
                ModeOp::None
            }
            None => self.resolve(),
        }
    }

    /// Copy the current item's answer to every other unanswered item with
    /// the same choice set ([`same_choice_set`]); noop while the current
    /// item is unanswered. Resolves when that completes the flow.
    fn apply_to_all(&mut self) -> ModeOp {
        let Some(choice) = self.answers[self.cursor] else {
            return ModeOp::None;
        };
        for i in 0..self.items.len() {
            if self.answers[i].is_none()
                && same_choice_set(&self.items[i].choices, &self.items[self.cursor].choices)
            {
                self.answers[i] = Some(choice);
            }
        }
        self.advance_or_resolve()
    }

    /// All items answered → hand the unwrapped answers to the manager.
    fn resolve(&self) -> ModeOp {
        ModeOp::FlowResolved {
            kind: self.kind,
            answers: self
                .answers
                .iter()
                .map(|a| a.expect("all answered"))
                .collect(),
        }
    }
}

/// Two items take the same answers iff their `(key, label)` sequences are
/// identical — the basis for apply-to-all (`'A'`).
fn same_choice_set(a: &[Choice], b: &[Choice]) -> bool {
    a == b
}

/// Truncate to `w` columns (`~`-marked, via [`ExactWidth`]) only when the
/// text is wider; unlike `exact_width` it never pads shorter text.
fn fit(s: &str, w: usize) -> String {
    if unicode_width(s) as usize > w {
        s.exact_width(w)
    } else {
        s.to_string()
    }
}

/// One display row of the flattened item list: every item contributes its
/// prompt row; the cursor item additionally expands into its detail lines
/// and a choices row.
enum Row<'a> {
    Prompt(usize),
    Detail(&'a str),
    Choices,
}

impl Draw for DecisionFlow {
    fn draw(
        &mut self,
        stdout: &mut Stdout,
        x_range: Range<u16>,
        y_range: Range<u16>,
    ) -> Result<()> {
        let x0 = x_range.start;
        let x_end = x_range.end;
        let width = x_end.saturating_sub(x0);
        let avail_h = y_range.end.saturating_sub(y_range.start) as usize;

        // Content geometry within a small horizontal padding.
        const PAD: u16 = 2;
        const INDENT: u16 = 4;
        let content_x = x0 + PAD;
        let content_w = width.saturating_sub(PAD * 2) as usize;

        // Flatten the items into display rows and remember where the cursor
        // item's expanded block starts and ends (for the scroll window).
        let mut rows: Vec<Row> = Vec::new();
        let mut cursor_first = 0;
        let mut cursor_last = 0;
        for (i, item) in self.items.iter().enumerate() {
            if i == self.cursor {
                cursor_first = rows.len();
            }
            rows.push(Row::Prompt(i));
            if i == self.cursor {
                for d in &item.detail {
                    rows.push(Row::Detail(d));
                }
                rows.push(Row::Choices);
                cursor_last = rows.len() - 1;
            }
        }

        // Vertically centered band: hint + top bar + title + rows + bottom bar.
        let cap = avail_h.saturating_sub(7).max(1);
        let visible = rows.len().min(cap);
        let band = visible + 4;
        let band_top = y_range.start + (avail_h.saturating_sub(band) as u16) / 2;
        let hint_y = band_top;
        let top_bar_y = band_top + 1;
        let title_y = band_top + 2;
        let list_y0 = band_top + 3;
        let bottom_bar_y = list_y0 + visible as u16;
        // Degenerate terminals: the band may not fit at all — clamp every
        // write into the given range instead of drawing outside it.
        let in_range = |y: u16| y < y_range.end;

        // Window offset: keep the cursor's expanded block fully visible;
        // when the block alone overflows the window, its prompt row wins.
        let offset = cursor_last
            .saturating_sub(visible.saturating_sub(1))
            .min(cursor_first)
            .min(rows.len() - visible);

        // Divider x-positions (match the miller columns / cd overlay).
        let div_center = x0 + width / 8;
        let div_right = x0 + width / 2;

        queue!(stdout, cursor::Hide)?;

        // 1. Keybinding hint on its own row, above the frame; the answer
        // keys are the CURRENT item's choice keys.
        if in_range(hint_y) {
            let keys = self.items[self.cursor]
                .choices
                .iter()
                .map(|c| c.key.to_string())
                .collect::<Vec<_>>()
                .join("/");
            let hint = fit(
                &format!("j/k move · {keys} answer · A all · Enter default · Esc done"),
                content_w,
            );
            let hint_x = x0 + width.saturating_sub(unicode_width(&hint) as u16) / 2;
            queue!(
                stdout,
                cursor::MoveTo(x0, hint_y),
                Clear(ClearType::CurrentLine),
                cursor::MoveTo(hint_x, hint_y),
                PrintStyledContent(hint.bold().with(color_main())),
            )?;
        }

        // 2. Top and bottom bars with junctions at the dividers.
        for x in x0..x_end {
            let junction = x == x0 || x == div_center || x == div_right;
            let (top, bot) = if junction {
                (print_horz_top(), print_horz_bot())
            } else {
                (print_horizontal_bar(), print_horizontal_bar())
            };
            if in_range(top_bar_y) {
                queue!(stdout, cursor::MoveTo(x, top_bar_y), top)?;
            }
            if in_range(bottom_bar_y) {
                queue!(stdout, cursor::MoveTo(x, bottom_bar_y), bot)?;
            }
        }

        // 3. Title line.
        if in_range(title_y) {
            let title = fit(&self.title, content_w);
            queue!(
                stdout,
                cursor::MoveTo(x0, title_y),
                Clear(ClearType::CurrentLine),
                cursor::MoveTo(content_x, title_y),
                PrintStyledContent(title.bold().with(color_main())),
            )?;
        }

        // 4. Item rows, windowed around the cursor block.
        for (row_i, row) in rows.iter().enumerate().skip(offset).take(visible) {
            let y = list_y0 + (row_i - offset) as u16;
            if !in_range(y) {
                break;
            }
            queue!(stdout, cursor::MoveTo(x0, y), Clear(ClearType::CurrentLine))?;
            match row {
                Row::Prompt(i) if *i == self.cursor => {
                    // Highlight bar across the content, prompt overprinted.
                    let prompt = fit(&self.items[*i].prompt, content_w);
                    queue!(
                        stdout,
                        cursor::MoveTo(content_x, y),
                        PrintStyledContent(" ".repeat(content_w).with(color_main()).reverse()),
                        cursor::MoveTo(content_x, y),
                        PrintStyledContent(prompt.with(color_main()).reverse()),
                    )?;
                }
                Row::Prompt(i) => {
                    // Answered items carry a right-aligned `✓ <label>` badge;
                    // the prompt yields it the room.
                    let badge =
                        self.answers[*i].map(|c| format!("✓ {}", self.items[*i].choices[c].label));
                    let badge_w = badge.as_deref().map_or(0, |b| unicode_width(b) as usize);
                    let prompt_w = if badge_w > 0 {
                        content_w.saturating_sub(badge_w + 2)
                    } else {
                        content_w
                    };
                    let prompt = fit(&self.items[*i].prompt, prompt_w);
                    queue!(
                        stdout,
                        cursor::MoveTo(content_x, y),
                        PrintStyledContent(prompt.grey()),
                    )?;
                    if let Some(badge) = badge {
                        if badge_w <= content_w {
                            let bx = content_x + (content_w - badge_w) as u16;
                            queue!(
                                stdout,
                                cursor::MoveTo(bx, y),
                                PrintStyledContent(badge.with(color_marked())),
                            )?;
                        }
                    }
                }
                Row::Detail(d) => {
                    let text = fit(d, content_w.saturating_sub(INDENT as usize));
                    queue!(
                        stdout,
                        cursor::MoveTo(content_x + INDENT, y),
                        PrintStyledContent(text.dark_grey()),
                    )?;
                }
                Row::Choices => {
                    // `[k] keep yours   [a] adopt new` — the chosen (or, while
                    // unanswered, the default) choice highlighted.
                    let item = &self.items[self.cursor];
                    let chosen = self.answers[self.cursor].or(item.default);
                    let mut cx = content_x + INDENT;
                    let end_x = content_x + content_w as u16;
                    for (ci, choice) in item.choices.iter().enumerate() {
                        let remaining = end_x.saturating_sub(cx) as usize;
                        if remaining == 0 {
                            break;
                        }
                        let text = format!("[{}] {}", choice.key, choice.label);
                        let truncated = unicode_width(&text) as usize > remaining;
                        let text = fit(&text, remaining);
                        let shown_w = unicode_width(&text) as u16;
                        let styled = if Some(ci) == chosen {
                            text.with(color_highlight()).bold()
                        } else {
                            text.grey()
                        };
                        queue!(stdout, cursor::MoveTo(cx, y), PrintStyledContent(styled))?;
                        if truncated {
                            break;
                        }
                        cx += shown_w + 3;
                    }
                }
            }
        }
        Ok(())
    }
}

impl ModalInput for DecisionFlow {
    fn handle_key(&mut self, key_event: KeyEvent) -> ModeOp {
        // Choice keys of the CURRENT item take precedence over navigation
        // keys — and never act on any other item.
        if let KeyCode::Char(c) = key_event.code {
            if let Some(i) = self.items[self.cursor]
                .choices
                .iter()
                .position(|choice| choice.key == c)
            {
                return self.answer(i);
            }
        }
        match key_event.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.cursor + 1 < self.items.len() {
                    self.cursor += 1;
                }
                ModeOp::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                ModeOp::None
            }
            KeyCode::Char('A') => self.apply_to_all(),
            KeyCode::Enter => match self.items[self.cursor].default {
                Some(d) => self.answer(d),
                None => ModeOp::None,
            },
            KeyCode::Esc | KeyCode::Char('q') => {
                let all_answerable = self
                    .answers
                    .iter()
                    .zip(&self.items)
                    .all(|(a, item)| a.is_some() || item.default.is_some());
                if all_answerable {
                    for (a, item) in self.answers.iter_mut().zip(&self.items) {
                        if a.is_none() {
                            *a = item.default;
                        }
                    }
                    self.resolve()
                } else {
                    ModeOp::FlowAborted { kind: self.kind }
                }
            }
            _ => ModeOp::None,
        }
    }

    fn region(&self) -> ModalRegion {
        ModalRegion::ConsoleOverlay
    }

    fn name(&self) -> &'static str {
        "decision-flow"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// The keep/adopt choice pair shared across the fixtures.
    fn keep_adopt() -> Vec<Choice> {
        vec![
            Choice {
                key: 'k',
                label: "keep".into(),
            },
            Choice {
                key: 'a',
                label: "adopt".into(),
            },
        ]
    }

    fn two_items() -> DecisionFlow {
        DecisionFlow::new(
            FlowKind::UpgradeNotice,
            "test".into(),
            vec![
                DecisionItem {
                    prompt: "first".into(),
                    detail: vec![],
                    choices: keep_adopt(),
                    default: Some(0),
                },
                DecisionItem {
                    prompt: "second".into(),
                    detail: vec![],
                    choices: vec![
                        Choice {
                            key: 'y',
                            label: "yes".into(),
                        },
                        Choice {
                            key: 'n',
                            label: "no".into(),
                        },
                    ],
                    default: Some(1),
                },
            ],
        )
    }

    #[test]
    fn j_k_move_and_clamp_cursor() {
        let mut f = two_items();
        assert_eq!(f.cursor(), 0);
        f.handle_key(key(KeyCode::Char('j')));
        assert_eq!(f.cursor(), 1);
        f.handle_key(key(KeyCode::Char('j'))); // clamp at end
        assert_eq!(f.cursor(), 1);
        f.handle_key(key(KeyCode::Char('k')));
        assert_eq!(f.cursor(), 0);
        // 'k' is a choice of item 0 and choice keys win on the current item,
        // so clamping at the start is exercised with the arrow key.
        f.handle_key(key(KeyCode::Up)); // clamp at start
        assert_eq!(f.cursor(), 0);
    }

    #[test]
    fn choice_key_answers_and_auto_advances() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Char('a')));
        assert!(matches!(op, ModeOp::None)); // answering never completes early
        assert_eq!(f.answers()[0], Some(1));
        assert_eq!(f.cursor(), 1); // advanced to next unanswered
    }

    #[test]
    fn enter_accepts_default_of_current_item() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Enter));
        assert_eq!(f.answers()[0], Some(0));
    }

    #[test]
    fn answering_last_item_resolves_flow() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('k')));
        let op = f.handle_key(key(KeyCode::Char('n')));
        match op {
            ModeOp::FlowResolved { kind, answers } => {
                assert_eq!(kind, FlowKind::UpgradeNotice);
                assert_eq!(answers, vec![0, 1]);
            }
            other => panic!("expected FlowResolved, got {other:?}"),
        }
    }

    #[test]
    fn esc_with_all_defaults_resolves_with_defaults() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Esc));
        assert!(matches!(op, ModeOp::FlowResolved { answers, .. } if answers == vec![0, 1]));
    }

    #[test]
    fn esc_without_default_aborts() {
        let mut f = two_items();
        f.items_mut_for_test()[1].default = None; // #[cfg(test)] accessor
        let op = f.handle_key(key(KeyCode::Esc));
        assert!(matches!(
            op,
            ModeOp::FlowAborted {
                kind: FlowKind::UpgradeNotice
            }
        ));
    }

    #[test]
    fn wrong_key_is_noop() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Char('z')));
        assert!(matches!(op, ModeOp::None));
        assert_eq!(f.answers()[0], None);
    }

    #[test]
    fn answered_item_can_be_revisited_and_changed() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('a'))); // answer item 0, auto-advance to 1
        f.handle_key(key(KeyCode::Up)); // navigate back to item 0
        f.handle_key(key(KeyCode::Char('k'))); // re-answer item 0 with choice 0
        assert_eq!(f.answers()[0], Some(0));
    }

    #[test]
    fn auto_advance_wraps_to_earlier_unanswered_item() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('j'))); // down to item 1
        let op = f.handle_key(key(KeyCode::Char('n'))); // answer item 1
        assert!(matches!(op, ModeOp::None)); // item 0 still open, no resolve
        assert_eq!(f.cursor(), 0); // wrapped, not clamped off the end
    }

    #[test]
    fn enter_without_default_is_noop() {
        let mut f = two_items();
        f.items_mut_for_test()[0].default = None;
        let op = f.handle_key(key(KeyCode::Enter));
        assert!(matches!(op, ModeOp::None));
        assert_eq!(f.answers(), &[None, None]);
    }

    #[test]
    fn esc_preserves_explicit_answers_and_fills_only_the_rest() {
        let mut f = two_items();
        f.handle_key(key(KeyCode::Char('a'))); // item 0 → choice 1 (not its default 0)
        let op = f.handle_key(key(KeyCode::Esc));
        // Explicit answer kept; only item 1 falls back to its default.
        assert!(matches!(op, ModeOp::FlowResolved { answers, .. } if answers == vec![1, 1]));
    }

    #[test]
    fn q_closes_like_esc() {
        let mut f = two_items();
        let op = f.handle_key(key(KeyCode::Char('q')));
        assert!(matches!(op, ModeOp::FlowResolved { answers, .. } if answers == vec![0, 1]));
    }

    /// Three items where #0 and #2 share the keep/adopt choice set and #1
    /// carries a different (yes/no) one.
    fn three_items_two_kinds() -> DecisionFlow {
        DecisionFlow::new(
            FlowKind::UpgradeNotice,
            "test".into(),
            vec![
                DecisionItem {
                    prompt: "first".into(),
                    detail: vec![],
                    choices: keep_adopt(),
                    default: Some(0),
                },
                DecisionItem {
                    prompt: "second".into(),
                    detail: vec![],
                    choices: vec![
                        Choice {
                            key: 'y',
                            label: "yes".into(),
                        },
                        Choice {
                            key: 'n',
                            label: "no".into(),
                        },
                    ],
                    default: Some(1),
                },
                DecisionItem {
                    prompt: "third".into(),
                    detail: vec![],
                    choices: keep_adopt(),
                    default: Some(0),
                },
            ],
        )
    }

    /// Three items all sharing one keep/adopt choice set.
    fn three_items_same_kind() -> DecisionFlow {
        DecisionFlow::new(
            FlowKind::UpgradeNotice,
            "test".into(),
            ["first", "second", "third"]
                .into_iter()
                .map(|p| DecisionItem {
                    prompt: p.into(),
                    detail: vec![],
                    choices: keep_adopt(),
                    default: Some(0),
                })
                .collect(),
        )
    }

    #[test]
    fn apply_to_all_answers_remaining_items_with_same_choice_set() {
        // three items; #0 and #2 share a choice set, #1 differs
        let mut f = three_items_two_kinds();
        f.handle_key(key(KeyCode::Char('a'))); // answer #0 with choice 1
        f.handle_key(key(KeyCode::Up)); // back to #0
        let op = f.handle_key(key(KeyCode::Char('A'))); // apply to all
        assert_eq!(f.answers()[2], Some(1)); // same-set item answered
        assert_eq!(f.answers()[1], None); // different set untouched
        assert!(matches!(op, ModeOp::None)); // #1 still unanswered
        assert_eq!(f.cursor(), 1); // advanced onto the remaining open item
    }

    #[test]
    fn apply_to_all_can_resolve_the_flow() {
        let mut f = three_items_same_kind(); // all share one choice set
        f.handle_key(key(KeyCode::Char('a')));
        f.handle_key(key(KeyCode::Up));
        let op = f.handle_key(key(KeyCode::Char('A')));
        assert!(matches!(op, ModeOp::FlowResolved { .. }));
    }

    #[test]
    fn apply_to_all_without_answer_on_current_is_noop() {
        let mut f = three_items_same_kind();
        let op = f.handle_key(key(KeyCode::Char('A')));
        assert!(matches!(op, ModeOp::None));
    }
}
