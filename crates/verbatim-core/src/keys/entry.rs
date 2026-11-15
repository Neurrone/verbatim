//! A key or command as an event-queue entry (ADR 0003, "Keys and
//! commands"; `docs/design/readers-tell-core-asks-build1.md`, "Keys and
//! commands in this build"): the action the hook chose at the press, the
//! marks a later key puts on the caret keys ahead of it, and the speech
//! fence every entry carries.
//!
//! A press joins the queue behind what arrived before it and runs at its
//! turn with no time limit of its own. Report focus and report caret
//! location run as their own functions ([`crate::report_focus`],
//! [`crate::caret_location`]), each holding the queue until its answer;
//! every other action runs today's
//! handling of the key or command at its turn, the caret key's included,
//! until the caret key's own entry replaces it.

use serde::{Deserialize, Serialize};
use verbatim_model::request::{Press, PressAction};
use verbatim_model::{Effect, Input, KeySeq, ReviewCommand, TraceId};

use crate::entries::Entries;
use crate::event_queue::Cx;

/// An entry's speech fence: the newest key number when the entry joined
/// the queue. Every utterance its function speaks carries it, so a key
/// pressed after the entry arrived, whose press cut speech, fences that
/// speech off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Fence(KeySeq);

impl Fence {
    /// The key number speech carries, `None` before any key joined.
    pub(crate) fn key(self) -> Option<KeySeq> {
        (self.0 != KeySeq::NONE_YET).then_some(self.0)
    }

    /// The newer of two fences.
    pub(crate) fn max(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

/// What Core keeps of the keys that joined the queue: the newest key
/// number, every later arrival's fence, and what marks a caret key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Arrivals {
    newest: KeySeq,
}

impl Arrivals {
    /// A non-modifier key, command, braille gesture or key Core sends,
    /// numbered `key`, joined the queue.
    pub(crate) fn key_joined(&mut self, key: KeySeq) {
        self.newest = self.newest.max(key);
    }

    /// The fence of an entry joining now.
    pub(crate) fn fence(self) -> Fence {
        Fence(self.newest)
    }

    /// Whether the caret key numbered `key` is marked silent: a key joined
    /// behind it. The mark is read from the arrivals, never stored on the
    /// entry, so a key at the front is marked by the same arrival as one
    /// waiting. NVDA: a script waiting silences the caret script ahead of
    /// it (`docs/parity/text.md`).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the caret key's own entry reads it in step 5b of build 1"
        )
    )]
    pub(crate) fn marks(self, key: KeySeq) -> bool {
        self.newest > key
    }
}

/// A key or command in the queue: the hook's report under its trace, and
/// its fence, its own number.
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
}

/// Stamps every utterance of `said` with `fence`, inside a key's carried
/// effects too.
pub(crate) fn stamp(fence: Fence, said: &mut Effect) {
    match said {
        Effect::Speak(_, key) => *key = fence.key(),
        Effect::ForKey(_, inner) => stamp(fence, inner),
        _ => {}
    }
}

/// The press's function at its turn.
pub(crate) async fn run(mut cx: Cx<Entries>, entry: PressEntry) {
    let PressEntry {
        trace_id, press, ..
    } = entry;
    if let PressAction::Command { command, repeat } = press.action
        && let Some(own) = Own::of(command)
    {
        // Any command stops say-all, as any key does in NVDA.
        let stopped = crate::say_all::stop(&mut cx.facts_mut());
        for effect in stopped {
            cx.emit(effect);
        }
        let press = crate::key_line::ReviewPress {
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
        }
        return;
    }
    let Some(input) = today(trace_id, &press) else {
        return;
    };
    let effects = {
        let mut facts = cx.facts_mut();
        // What its handling asks outside the queue answers under its
        // trace, whose speech then carries its number.
        facts.key_origins.record(trace_id, press.key_seq);
        crate::reduce::waited_input(&mut facts, &input)
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
}

impl Own {
    fn of(command: ReviewCommand) -> Option<Self> {
        match command {
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

/// The input today's handling of `press` takes; `None` for a key passed
/// that does nothing more at its turn than its press's cut did.
fn today(trace_id: TraceId, press: &Press) -> Option<Input> {
    let key_seq = press.key_seq;
    let pressed_at_ms = press.pressed_at_us / 1_000;
    Some(match &press.action {
        PressAction::Command { command, repeat } => Input::Command {
            trace_id,
            key_seq,
            command: *command,
            pressed_at_ms,
            repeat: *repeat,
        },
        PressAction::Script { script, repeat } => Input::Script {
            trace_id,
            key_seq,
            repeat: *repeat,
            script: *script,
            pressed_at_ms,
        },
        PressAction::CaretKey {
            key, held: true, ..
        } => Input::HeldKey {
            trace_id,
            key_seq,
            key: *key,
            pressed_at_ms,
        },
        PressAction::CaretKey {
            key, held: false, ..
        } => Input::CaretKey {
            trace_id,
            key_seq,
            key: *key,
            pressed_at_ms,
        },
        PressAction::Typed { text } => Input::CharacterTyped {
            trace_id,
            key_seq,
            text: text.clone(),
            at_ms: pressed_at_ms,
        },
        PressAction::Clearing => Input::ClearingKey {
            trace_id,
            key_seq,
            at_ms: pressed_at_ms,
        },
        PressAction::EditingChord => Input::EditingChord {
            trace_id,
            key_seq,
            pressed_at_ms,
        },
        PressAction::Passed => return None,
    })
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
}
