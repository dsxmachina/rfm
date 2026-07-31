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
use crossterm::Result;

use super::{ModalInput, ModalRegion, ModeOp};
use crate::panel::Draw;

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

    /// The item the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Chosen choice index per item; `None` = still unanswered.
    pub fn answers(&self) -> &[Option<usize>] {
        &self.answers
    }

    pub fn items(&self) -> &[DecisionItem] {
        &self.items
    }

    #[cfg(test)]
    fn items_mut_for_test(&mut self) -> &mut [DecisionItem] {
        &mut self.items
    }

    /// Record `choice` for the current item, then advance to the next
    /// unanswered item (wrapping search from cursor+1); when none remain
    /// the flow resolves.
    fn answer(&mut self, choice: usize) -> ModeOp {
        self.answers[self.cursor] = Some(choice);
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

    /// All items answered → hand the unwrapped answers to the manager.
    fn resolve(&self) -> ModeOp {
        ModeOp::FlowResolved {
            kind: self.kind,
            answers: self.answers.iter().map(|a| a.expect("all answered")).collect(),
        }
    }
}

impl Draw for DecisionFlow {
    fn draw(
        &mut self,
        _stdout: &mut Stdout,
        _x_range: Range<u16>,
        _y_range: Range<u16>,
    ) -> Result<()> {
        // Rendering lands in a follow-up task; drawing nothing is safe.
        let _ = &self.title;
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

    fn two_items() -> DecisionFlow {
        DecisionFlow::new(
            FlowKind::UpgradeNotice,
            "test".into(),
            vec![
                DecisionItem {
                    prompt: "first".into(),
                    detail: vec![],
                    choices: vec![
                        Choice {
                            key: 'k',
                            label: "keep".into(),
                        },
                        Choice {
                            key: 'a',
                            label: "adopt".into(),
                        },
                    ],
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
        f.handle_key(key(KeyCode::Char('a'))); // answer item 0, cursor → 1
        f.handle_key(key(KeyCode::Char('k'))); // 'k' is not a choice of item 1 → noop
        f.handle_key(key(KeyCode::Char('k'))); // still noop; go back instead:
        f.handle_key(key(KeyCode::Char('k'))); // (kept noop on purpose — see below)
        f.handle_key(key(KeyCode::Up));
        f.handle_key(key(KeyCode::Char('k'))); // re-answer item 0
        assert_eq!(f.answers()[0], Some(0));
    }
}
