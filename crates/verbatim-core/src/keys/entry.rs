//! A key or command as an event-queue entry (ADR 0003, "Keys and
//! commands"; `books/dev/src/reference/crates/verbatim-core.md`, "Key numbers"): the action the
//! hook chose at the press, the
//! marks a later key puts on the caret keys ahead of it, and the speech
//! fence every entry carries.
//!
//! A press joins the queue behind what arrived before it and runs at its
//! turn with no time limit of its own. Report focus, report caret
//! location, the navigator's commands, the review commands over the
//! navigator's text and both say-alls run as their own functions
//! ([`crate::report_focus`], [`crate::caret_location`],
//! [`crate::navigator`], [`crate::review_text`], [`crate::say_all`]), each
//! holding the queue until its last answer;
//! a caret key runs its own entry ([`crate::keys::caret_key`]), and every
//! other action calls its handling at its turn: the toggles, the scripts,
//! and text typed.

use serde::{Deserialize, Serialize};
use verbatim_model::request::{Press, PressAction};
use verbatim_model::{Effect, KeySeq, ReviewCommand, TraceId};

use crate::entries::Entries;
use crate::event_queue::Cx;

/// An entry's speech fence: when the entry joined the queue, the newer of
/// the newest key to join the queue and the newest key whose press cut
/// speech, or for a caret tell the newest key to join alone
/// ([`Arrivals`]). Every utterance its function speaks carries it, so a
/// key pressed after the entry arrived, whose press cut speech, fences
/// that speech off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Fence(KeySeq);

impl Fence {
    /// The key number speech carries, `None` before any key joined.
    pub(crate) fn key(self) -> Option<KeySeq> {
        (self.0 != KeySeq::NONE_YET).then_some(self.0)
    }

    /// The fence of key `key`.
    pub(crate) fn of(key: KeySeq) -> Self {
        Self(key)
    }

    /// The newer of two fences.
    pub(crate) fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

/// What Core keeps of the keys that joined the queue and the keys that cut
/// speech: the newest key number to join, which marks a caret key, and the
/// newest to cut speech, which with it gives every later arrival's fence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Arrivals {
    /// The newest non-modifier key, command, braille gesture or key Core
    /// sends to join the queue.
    newest: KeySeq,
    /// The newest key whose press cut speech, a modifier, which joins no
    /// queue, included.
    cut: KeySeq,
}

impl Arrivals {
    /// A non-modifier key, command, braille gesture or key Core sends,
    /// numbered `key`, joined the queue.
    pub(crate) fn key_joined(&mut self, key: KeySeq) {
        self.newest = self.newest.max(key);
    }

    /// The key numbered `key` cut speech. The speech queue drops speech
    /// carrying a lower number, so speech of what arrives after the cut
    /// carries at least `key`; a modifier's cut marks no caret key, as a
    /// modifier alone queues no script in NVDA (`docs/parity/text.md`).
    pub(crate) fn speech_cut(&mut self, key: KeySeq) {
        self.cut = self.cut.max(key);
    }

    /// The fence of an entry an application raised, joining now: speech
    /// of an event that arrived after a key's cut is spoken past it, as
    /// NVDA queues the cut and runs the event after it.
    pub(crate) fn fence(self) -> Fence {
        Fence(self.newest.max(self.cut))
    }

    /// The fence of a caret tell's selection speech, read as it joins and
    /// again at its read's answer: the newest key to join the queue, whose
    /// words its selection speech stands for, never a later cut. NVDA speaks a selection key's selection inside the key's
    /// script, which runs before the cut a modifier pressed after it
    /// queues, so the cut silences it (`docs/parity/text.md`, "Speech cut
    /// off by a modifier").
    pub(crate) fn key_fence(self) -> Fence {
        Fence(self.newest)
    }

    /// Whether the caret key numbered `key` is marked silent: a key joined
    /// behind it. The mark is read from the arrivals, never stored on the
    /// entry, so a key at the front is marked by the same arrival as one
    /// waiting. NVDA: a script waiting silences the caret script ahead of
    /// it (`docs/parity/text.md`).
    pub(crate) fn marks(self, key: KeySeq) -> bool {
        self.newest > key
    }
}

/// A key or command in the queue: the hook's report under its trace, and
/// its fence, its own number.
/// A press of a review command: the command and how many times it was
/// repeated, which its function runs against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewPress {
    /// The key's trace.
    pub(crate) trace_id: TraceId,
    pub(crate) command: ReviewCommand,
    pub(crate) repeat: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PressEntry {
    /// The trace minted when the hook saw the key.
    pub(crate) trace_id: TraceId,
    /// The hook's report.
    pub(crate) press: Press,
    /// Set when it joins the queue.
    pub(crate) fence: Fence,
}

impl PressEntry {
    /// The press `press`, reported under `trace_id`, before it joins.
    pub(crate) fn new(trace_id: TraceId, press: Press) -> Self {
        Self {
            trace_id,
            press,
            fence: Fence::default(),
        }
    }

    /// Whether it is a review press that moves the review cursor over
    /// text: a review command or a move between command marks.
    pub(crate) fn reviews(&self) -> bool {
        matches!(
            self.press.action,
            PressAction::Command { command, .. }
                if matches!(Own::of(command), Some(Own::ReviewText | Own::CommandMark))
        )
    }
}

/// Stamps every utterance of `said` with `fence`, inside a key's carried
/// effects too, except say-all's, which carries its starting key's number
/// whichever entry hands it to speech (`crate::say_all`).
pub(crate) fn stamp(fence: Fence, said: &mut Effect) {
    match said {
        Effect::Speak(utterance, _) if utterance.say_all => {}
        Effect::Speak(_, key) => *key = fence.key(),
        Effect::ForKey(_, inner) => stamp(fence, inner),
        _ => {}
    }
}

/// The press's function at its turn, after the read of a postponed
/// dialog that it judges against (`crate::focus::shown`).
pub(crate) async fn run(mut cx: Cx<Entries>, entry: PressEntry) {
    crate::focus::shown::before_press(&mut cx, &entry).await;
    if let Some(caret) = crate::keys::caret_key::CaretPress::of(&entry) {
        crate::keys::caret_key::run(cx, caret).await;
        return;
    }
    let PressEntry {
        trace_id,
        press,
        fence,
    } = entry;
    let own = match press.action {
        PressAction::Command { command, .. } => Own::of(command),
        _ => None,
    };
    if let PressAction::Command { command, repeat } = press.action
        && let Some(own) = own
    {
        // Any command stops say-all, as any key does in NVDA.
        let stopped = crate::say_all::stop(&mut cx.facts_mut());
        for effect in stopped {
            cx.emit(effect);
        }
        let press = ReviewPress {
            trace_id,
            command,
            repeat,
        };
        match own {
            Own::ReportFocus => crate::report_focus::run(cx, trace_id, repeat).await,
            Own::CaretLocation => crate::caret_location::run(cx, trace_id).await,
            Own::Navigate(kind) => crate::navigator::navigate(cx, press, kind).await,
            Own::Activate => crate::navigator::activate(cx, trace_id).await,
            Own::ReportObject => crate::navigator::report_object(cx, press).await,
            Own::ToFocus => crate::navigator::to_focus(cx, trace_id).await,
            Own::ReviewText => crate::review_text::entry(cx, press, fence).await,
            Own::SayAllFromCaret => crate::say_all::from_caret(&mut cx, trace_id, fence),
            Own::CommandMark => crate::review_text::command_mark(cx, press, fence).await,
            Own::StatusBar => crate::terminal::commands::status_bar(cx, trace_id, repeat).await,
        }
        return;
    }
    let now = cx.now();
    let effects = {
        let mut facts = cx.facts_mut();
        // A held stroke is sent at its turn.
        let sent = facts.strokes.pass(press.key_seq, now.us());
        let mut effects: Vec<Effect> = sent.strokes.map(Effect::Queue).into_iter().collect();
        let facts = &mut *facts;
        crate::keys::terminal::reached(facts, sent.reached, now);
        match &press.action {
            PressAction::Command { command, .. } => {
                // Any command stops say-all, as any key does in NVDA.
                effects.extend(crate::say_all::stop(facts));
                effects.extend(match command {
                    ReviewCommand::ToggleFollowCaret
                    | ReviewCommand::ToggleTypedCharacters
                    | ReviewCommand::ToggleTypedWords => {
                        crate::reduce::toggle_setting(facts, trace_id, *command)
                    }
                    ReviewCommand::ToggleReportNewOutput => {
                        crate::terminal::toggle(facts, trace_id, now)
                    }
                    _ => Vec::new(),
                });
            }
            PressAction::Script { script, repeat } => {
                effects.extend(crate::shell::script(*script, *repeat, press.key_seq));
            }
            PressAction::Typed { text } => {
                effects.extend(crate::editing::character_typed(facts, trace_id, text));
            }
            PressAction::Control => {
                effects.extend(crate::editing::control_typed(facts, trace_id));
            }
            PressAction::Clearing => crate::editing::clearing_key(facts),
            // A caret key ran its own entry above; an editing chord's cut
            // of speech at the press was all it does.
            PressAction::CaretKey { .. } | PressAction::EditingChord => {}
        }
        effects
    };
    for effect in effects {
        cx.emit(effect);
    }
}

/// A command that runs as its own function on the queue, making its reads
/// as the queue's requests.
#[derive(Clone, Copy)]
enum Own {
    ReportFocus,
    CaretLocation,
    Navigate(verbatim_model::QueryKind),
    Activate,
    ReportObject,
    ToFocus,
    ReviewText,
    SayAllFromCaret,
    /// A move between command marks, in the focused terminal Core models;
    /// elsewhere it says nothing.
    CommandMark,
    /// "Report status bar".
    StatusBar,
}

impl Own {
    /// The command's own function at its turn, `None` for one that runs
    /// today's handling.
    fn of(command: ReviewCommand) -> Option<Self> {
        match command {
            command if crate::review_text::handles(command) => Some(Self::ReviewText),
            ReviewCommand::PreviousCommandMark | ReviewCommand::NextCommandMark => {
                Some(Self::CommandMark)
            }
            ReviewCommand::ReportStatusBar => Some(Self::StatusBar),
            ReviewCommand::SayAllFromCaret => Some(Self::SayAllFromCaret),
            ReviewCommand::ReportFocus => Some(Self::ReportFocus),
            ReviewCommand::ReportCaretLocation => Some(Self::CaretLocation),
            ReviewCommand::Parent => Some(Self::Navigate(verbatim_model::QueryKind::Parent)),
            ReviewCommand::NextSibling => {
                Some(Self::Navigate(verbatim_model::QueryKind::NextSibling))
            }
            ReviewCommand::PreviousSibling => {
                Some(Self::Navigate(verbatim_model::QueryKind::PreviousSibling))
            }
            ReviewCommand::FirstChild => {
                Some(Self::Navigate(verbatim_model::QueryKind::FirstChild))
            }
            ReviewCommand::Activate => Some(Self::Activate),
            ReviewCommand::ReportObject => Some(Self::ReportObject),
            ReviewCommand::ToFocus => Some(Self::ToFocus),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_key_marks_the_caret_keys_ahead_and_moves_the_fence() {
        let mut arrivals = Arrivals::default();
        assert_eq!(arrivals.fence().key(), None);
        arrivals.key_joined(KeySeq::new(4));
        assert!(
            !arrivals.marks(KeySeq::new(4)),
            "a key is not marked by itself"
        );
        arrivals.key_joined(KeySeq::new(5));
        assert!(arrivals.marks(KeySeq::new(4)));
        assert!(!arrivals.marks(KeySeq::new(5)));
        assert_eq!(arrivals.fence().key(), Some(KeySeq::new(5)));
    }

    /// A modifier's cut of speech moves the fence of what arrives after it,
    /// so that its speech outlives the cut, and marks no caret key ahead.
    #[test]
    fn a_modifiers_cut_moves_the_fence_and_marks_no_caret_key() {
        let mut arrivals = Arrivals::default();
        arrivals.key_joined(KeySeq::new(4));
        arrivals.speech_cut(KeySeq::new(5));
        assert!(!arrivals.marks(KeySeq::new(4)));
        assert_eq!(arrivals.fence().key(), Some(KeySeq::new(5)));
        // A caret tell's speech stands for the key before the cut.
        assert_eq!(arrivals.key_fence().key(), Some(KeySeq::new(4)));
        // The next key joins: its own number is the fence again.
        arrivals.key_joined(KeySeq::new(6));
        assert!(arrivals.marks(KeySeq::new(4)));
        assert_eq!(arrivals.fence().key(), Some(KeySeq::new(6)));
    }
}
