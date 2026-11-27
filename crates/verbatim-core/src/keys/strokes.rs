//! Core's stroke line, [`StrokeLine`]: the key transitions the hook held
//! or deferred that Core has not yet sent, in the hook's order
//! (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event
//! queue"). Whether a key's stroke is still to be sent is read from here,
//! never stored on its entry: present is unsent, a dropped mark is
//! dropped, and gone is sent. [`StrokeLine::hold`], [`StrokeLine::behind`],
//! [`StrokeLine::defer`], the passes and the drops are its only writers. It is the one record of which keys
//! the focused terminal's model takes behind a held stroke: each send names
//! the terminal keys that went with it, in the order they reached the
//! program ([`Sent::reached`]). It also knows when the newest held stroke
//! of a user's key reached the program, which a terminal's flood policy
//! reads ([`StrokeLine::held_reached_us`]).

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use verbatim_model::KeySeq;
use verbatim_model::request::QueueEffect;
use verbatim_model::stroke::{HeldStroke, SentTransition, Stroke};

use crate::keys::terminal::TerminalKey;

/// How long after its press a held key is sent if its turn has not come:
/// the keyboard stays alive (ADR 0003, "Keys and commands").
pub(crate) const RELEASE_AFTER_US: u64 = 150_000;

/// One transition in line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum InLine {
    /// A held key's stroke.
    Held {
        /// Its key's number.
        key: KeySeq,
        /// The stroke and its gesture's modifiers.
        stroke: HeldStroke,
        /// When its 150 ms release fires; `None` for a key Core sends
        /// itself, which has none.
        release_at_us: Option<u64>,
        /// Its key has passed: it goes once nothing unsent is ahead.
        passed: bool,
        /// The key the focused terminal's model takes when it is sent, if
        /// it was bound to that terminal at its arrival.
        terminal: Option<TerminalKey>,
    },
    /// A transition deferred behind a held stroke.
    Deferred {
        /// The key.
        stroke: Stroke,
        /// A press, or else a release.
        pressed: bool,
    },
    /// The terminal keys of a press reported behind a held stroke, ahead of
    /// its own deferred transitions, as the hook reports them: they reach
    /// the program when what is ahead of them goes. It sends nothing and
    /// never holds the line.
    Keys(Vec<TerminalKey>),
}

impl InLine {
    /// Whether it holds the line: a held stroke whose key has not passed.
    fn holds(&self) -> bool {
        matches!(self, Self::Held { passed: false, .. })
    }
}

/// What the line knows of a held key's stroke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrokeStatus {
    /// Still to be sent.
    Unsent,
    /// Sent, or passed and going once what is ahead of it goes.
    Sent,
    /// Dropped at a desktop switch or a reset of the hook's line, never to
    /// be sent.
    Dropped,
}

/// What one send from the line's front sent.
#[derive(Debug, Default, PartialEq, Eq)]
#[must_use]
pub(crate) struct Sent {
    /// The transitions, as one effect; `None` when nothing went.
    pub(crate) strokes: Option<QueueEffect>,
    /// The terminal keys that reached the program with them, in order.
    pub(crate) reached: Vec<TerminalKey>,
}

/// The stroke line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StrokeLine {
    /// The transitions not yet sent, in the hook's order.
    line: VecDeque<InLine>,
    /// The keys whose held stroke was dropped, each kept until its entry's
    /// turn reads it.
    dropped: BTreeSet<KeySeq>,
    /// When the newest held stroke of a user's key was sent, reaching the
    /// program, in microseconds on the shared clock; 0 before any.
    held_reached_us: u64,
}

impl StrokeLine {
    /// The hook held key `key`'s `stroke`, pressed at `pressed_at_us`, `None`
    /// for a key Core sends itself, which has no release. `terminal` is the
    /// terminal key it is, reported with the send that sends it.
    pub(crate) fn hold(
        &mut self,
        key: KeySeq,
        stroke: HeldStroke,
        (pressed_at_us, terminal): (Option<u64>, Option<TerminalKey>),
    ) {
        self.line.push_back(InLine::Held {
            key,
            stroke,
            release_at_us: pressed_at_us.map(|at| at.saturating_add(RELEASE_AFTER_US)),
            passed: false,
            terminal,
        });
    }

    /// The terminal keys of a press reported while the line holds join it,
    /// to reach the program with what is ahead of them.
    pub(crate) fn behind(&mut self, keys: Vec<TerminalKey>) {
        if !keys.is_empty() {
            self.line.push_back(InLine::Keys(keys));
        }
    }

    /// The hook deferred a transition: it joins the line, and goes at once
    /// when nothing unsent holds the line.
    pub(crate) fn defer(&mut self, stroke: Stroke, pressed: bool, now_us: u64) -> Sent {
        self.line.push_back(InLine::Deferred { stroke, pressed });
        self.send_ready(now_us)
    }

    /// Key `key` passes at `now_us`: its stroke goes, with what is behind
    /// it up to the next unsent held stroke, once nothing unsent is ahead of
    /// it.
    pub(crate) fn pass(&mut self, key: KeySeq, now_us: u64) -> Sent {
        let held = self.line.iter_mut().find_map(|item| match item {
            InLine::Held {
                key: held, passed, ..
            } if *held == key => Some(passed),
            InLine::Held { .. } | InLine::Deferred { .. } | InLine::Keys(_) => None,
        });
        let Some(held) = held else {
            return Sent::default();
        };
        *held = true;
        self.send_ready(now_us)
    }

    /// What the line knows of key `key`'s stroke.
    pub(crate) fn status(&self, key: KeySeq) -> StrokeStatus {
        if self.dropped.contains(&key) {
            return StrokeStatus::Dropped;
        }
        let unsent = self.line.iter().any(
            |item| matches!(item, InLine::Held { key: held, passed: false, .. } if *held == key),
        );
        if unsent {
            StrokeStatus::Unsent
        } else {
            StrokeStatus::Sent
        }
    }

    /// Key `key`'s entry has read its dropped mark at its turn.
    pub(crate) fn forget_dropped(&mut self, key: KeySeq) {
        self.dropped.remove(&key);
    }

    /// Whether no transition waits to be sent.
    pub(crate) fn is_empty(&self) -> bool {
        self.line.is_empty()
    }

    /// Whether a held stroke whose key has not passed holds the line: a
    /// key pressed now is deferred behind it, reaching the application at
    /// its pass.
    pub(crate) fn holds(&self) -> bool {
        self.line.iter().any(InLine::holds)
    }

    /// The earliest 150 ms release still to fire.
    pub(crate) fn next_release(&self) -> Option<u64> {
        self.line
            .iter()
            .filter_map(|item| match item {
                InLine::Held {
                    release_at_us: Some(at),
                    passed: false,
                    ..
                } => Some(*at),
                InLine::Held { .. } | InLine::Deferred { .. } | InLine::Keys(_) => None,
            })
            .min()
    }

    /// Fires every release due by `now_us`: each such key passes.
    pub(crate) fn release_due(&mut self, now_us: u64) -> Sent {
        for item in &mut self.line {
            if let InLine::Held {
                release_at_us: Some(at),
                passed,
                ..
            } = item
                && *at <= now_us
            {
                *passed = true;
            }
        }
        self.send_ready(now_us)
    }

    /// When the newest held stroke of a user's key reached the program, in
    /// microseconds; 0 before any. A read begun before then shows what was
    /// written before that key, which cut speech at its press.
    pub(crate) fn held_reached_us(&self) -> u64 {
        self.held_reached_us
    }

    /// Drops every stroke not yet sent, as the input desktop leaving the
    /// user's or the hook's line being reset does, in the same step that
    /// clears the hook's own record: each held stroke whose key has not
    /// passed leaves a dropped mark for its entry's turn, and the terminal
    /// keys behind go with them, never reaching the program.
    pub(crate) fn drop_unsent(&mut self) {
        for item in std::mem::take(&mut self.line) {
            if let InLine::Held {
                key, passed: false, ..
            } = item
            {
                self.dropped.insert(key);
            }
        }
    }

    /// Exit: the line is dropped, and the keys whose last transition in it
    /// is a release are named, in the order of those releases, for the
    /// shell to release those Windows still has down. A held stroke counts
    /// as a release, since sending it would send its press and its
    /// release, its own release having been swallowed; a key whose last
    /// transition is a deferred press is down under the user's finger; a
    /// character typed as Unicode leaves no key down.
    pub(crate) fn exit(&mut self) -> Vec<Stroke> {
        let mut last: Vec<(Stroke, bool)> = Vec::new();
        for item in std::mem::take(&mut self.line) {
            let (stroke, pressed) = match item {
                InLine::Held { stroke, .. } => (stroke.stroke, false),
                InLine::Deferred { stroke, pressed } => (stroke, pressed),
                InLine::Keys(_) => continue,
            };
            if stroke.is_unicode() {
                continue;
            }
            // Moved to the end, so the releases go in the order of the last.
            last.retain(|(earlier, _)| earlier.vk != stroke.vk);
            last.push((stroke, pressed));
        }
        self.dropped.clear();
        last.into_iter()
            .filter(|&(_, pressed)| !pressed)
            .map(|(stroke, _)| stroke)
            .collect()
    }

    /// Sends, at `now_us`, what may go from the line's front: deferred
    /// transitions and held strokes whose key has passed, up to the first
    /// that holds the line, with the terminal keys among them.
    fn send_ready(&mut self, now_us: u64) -> Sent {
        let mut sent = Vec::new();
        let mut reached = Vec::new();
        while self.line.front().is_some_and(|front| !front.holds()) {
            match self.line.pop_front() {
                Some(InLine::Held {
                    stroke,
                    release_at_us,
                    terminal,
                    ..
                }) => {
                    // A key Core sends itself has no release, and is no
                    // user's key.
                    if release_at_us.is_some() {
                        self.held_reached_us = self.held_reached_us.max(now_us);
                    }
                    sent.push(SentTransition::Held(stroke));
                    reached.extend(terminal);
                }
                Some(InLine::Deferred { stroke, pressed }) => {
                    sent.push(SentTransition::Deferred { stroke, pressed });
                }
                Some(InLine::Keys(keys)) => reached.extend(keys),
                None => break,
            }
        }
        Sent {
            strokes: (!sent.is_empty()).then_some(QueueEffect::SendStrokes(sent)),
            reached,
        }
    }
}

#[cfg(test)]
mod tests {
    use verbatim_model::stroke::Modifier;

    use super::*;

    const SHIFT: u16 = 0xA0;
    const CONTROL: u16 = 0xA2;
    const ALT: u16 = 0x12;
    const RIGHT_ALT: u16 = 0xA5;
    const X: u16 = 0x58;
    const DOWN: u16 = 0x28;

    fn stroke(vk: u16, sequence: u32) -> Stroke {
        Stroke {
            vk,
            scan_code: 0,
            extended: false,
            sequence,
        }
    }

    fn held(vk: u16, sequence: u32) -> HeldStroke {
        HeldStroke {
            stroke: stroke(vk, sequence),
            modifiers: vec![Modifier::Shift],
        }
    }

    fn sent(sent: Sent) -> Vec<SentTransition> {
        assert_eq!(sent.reached, [], "no terminal key in these lines");
        match sent.strokes {
            Some(QueueEffect::SendStrokes(sent)) => sent,
            None => Vec::new(),
            Some(other) => panic!("a send: {other:?}"),
        }
    }

    /// A deferred transition with nothing unsent ahead goes at once; behind
    /// a held stroke it waits, and goes with that stroke when its key
    /// passes, up to the next unsent held stroke.
    #[test]
    fn a_held_stroke_goes_with_what_is_deferred_behind_it_up_to_the_next() {
        let mut line = StrokeLine::default();
        assert_eq!(
            sent(line.defer(stroke(X, 1), true, 0)),
            [SentTransition::Deferred {
                stroke: stroke(X, 1),
                pressed: true
            }]
        );
        line.hold(KeySeq::new(1), held(DOWN, 2), (Some(0), None));
        assert_eq!(sent(line.defer(stroke(X, 3), false, 0)), []);
        line.hold(KeySeq::new(2), held(DOWN, 4), (Some(10), None));
        assert_eq!(sent(line.defer(stroke(SHIFT, 5), true, 0)), []);
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Unsent);
        assert_eq!(
            sent(line.pass(KeySeq::new(1), 0)),
            [
                SentTransition::Held(held(DOWN, 2)),
                SentTransition::Deferred {
                    stroke: stroke(X, 3),
                    pressed: false
                },
            ]
        );
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Sent);
        assert_eq!(line.status(KeySeq::new(2)), StrokeStatus::Unsent);
        assert_eq!(
            sent(line.pass(KeySeq::new(2), 0)),
            [
                SentTransition::Held(held(DOWN, 4)),
                SentTransition::Deferred {
                    stroke: stroke(SHIFT, 5),
                    pressed: true
                },
            ]
        );
    }

    /// A key passed behind an unsent one (its waiting entry ended at a
    /// foreground change) goes with the front key's pass, in press order.
    #[test]
    fn a_stroke_passed_behind_an_unsent_one_goes_with_it_in_press_order() {
        let mut line = StrokeLine::default();
        line.hold(KeySeq::new(1), held(DOWN, 1), (Some(0), None));
        line.hold(KeySeq::new(2), held(X, 2), (Some(5), None));
        assert_eq!(sent(line.pass(KeySeq::new(2), 0)), []);
        assert_eq!(line.status(KeySeq::new(2)), StrokeStatus::Sent);
        assert_eq!(
            sent(line.pass(KeySeq::new(1), 0)),
            [
                SentTransition::Held(held(DOWN, 1)),
                SentTransition::Held(held(X, 2))
            ]
        );
    }

    /// Each held stroke's release fires 150 ms after its press, in press
    /// order; a key Core sends has none.
    #[test]
    fn a_held_stroke_is_released_150_ms_after_its_press() {
        let mut line = StrokeLine::default();
        line.hold(KeySeq::new(1), held(DOWN, 1), (Some(1_000), None));
        line.hold(KeySeq::new(2), held(X, 2), (Some(40_000), None));
        line.hold(KeySeq::new(3), held(DOWN, 3), (None, None));
        assert_eq!(line.next_release(), Some(151_000));
        assert_eq!(sent(line.release_due(150_999)), []);
        assert_eq!(
            sent(line.release_due(151_000)),
            [SentTransition::Held(held(DOWN, 1))]
        );
        assert_eq!(line.next_release(), Some(190_000));
        assert_eq!(
            sent(line.release_due(190_000)),
            [SentTransition::Held(held(X, 2))]
        );
        assert_eq!(line.next_release(), None);
        assert_eq!(line.status(KeySeq::new(3)), StrokeStatus::Unsent);
    }

    /// Dropping the unsent strokes leaves a dropped mark for each held key
    /// that had not passed, until its entry's turn; what is deferred goes
    /// with them, and what comes after goes at once.
    #[test]
    fn dropped_strokes_leave_a_mark_until_their_entrys_turn() {
        let mut line = StrokeLine::default();
        line.hold(KeySeq::new(1), held(DOWN, 1), (Some(0), None));
        assert_eq!(sent(line.defer(stroke(X, 2), true, 0)), []);
        line.drop_unsent();
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Dropped);
        assert_eq!(line.next_release(), None);
        assert_eq!(
            sent(line.defer(stroke(X, 3), false, 0)),
            [SentTransition::Deferred {
                stroke: stroke(X, 3),
                pressed: false
            }]
        );
        line.forget_dropped(KeySeq::new(1));
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Sent);
    }

    /// At exit the line is dropped, and every key whose last transition in
    /// it is a release is named, in the order of those releases: the held
    /// Down Arrow, right Alt, left Shift (pressed before the held key and
    /// released behind it), X and Alt; not left Control, still down under
    /// the user's finger, nor a character typed as Unicode. Which of them
    /// Windows still has down (not X, whose press was dropped with the
    /// line) is the shell's to read.
    #[test]
    fn at_exit_the_keys_released_behind_a_held_key_are_named() {
        let mut line = StrokeLine::default();
        line.hold(KeySeq::new(1), held(DOWN, 1), (Some(0), None));
        for (vk, pressed, sequence) in [
            (SHIFT, true, 2),
            (X, true, 3),
            (RIGHT_ALT, false, 4),
            (SHIFT, false, 5),
            (X, false, 6),
            (CONTROL, true, 7),
            (verbatim_model::stroke::VK_PACKET, false, 8),
            (ALT, false, 9),
        ] {
            assert_eq!(sent(line.defer(stroke(vk, sequence), pressed, 0)), []);
        }
        assert_eq!(
            line.exit(),
            [
                stroke(DOWN, 1),
                stroke(RIGHT_ALT, 4),
                stroke(SHIFT, 5),
                stroke(X, 6),
                stroke(ALT, 9)
            ]
        );
        assert_eq!(line, StrokeLine::default());
    }

    /// A key released behind a held key and pressed again is down under
    /// the user's finger, so it is not named; a key released twice is
    /// named once.
    #[test]
    fn only_a_keys_last_transition_decides_whether_it_is_named_at_exit() {
        let mut line = StrokeLine::default();
        line.hold(KeySeq::new(1), held(DOWN, 1), (Some(0), None));
        for (vk, pressed, sequence) in [
            (SHIFT, false, 2),
            (SHIFT, true, 3),
            (CONTROL, false, 4),
            (CONTROL, false, 5),
        ] {
            assert_eq!(sent(line.defer(stroke(vk, sequence), pressed, 0)), []);
        }
        assert_eq!(line.exit(), [stroke(DOWN, 1), stroke(CONTROL, 5)]);
    }
}
