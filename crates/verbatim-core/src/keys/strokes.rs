//! Core's stroke line (`docs/design/readers-tell-core-asks-build1.md`,
//! "The hook and Core's stroke line", Table 13): the key transitions the
//! hook held or deferred that Core has not yet sent, in the hook's order.
//! The shell keeps no copy: it sends what Core names, and at exit releases
//! the keys Core lists that Windows still has down.
//!
//! A held stroke is sent when its key passes, at its turn, by its 150 ms
//! release, or when the foreground change ends its waiting entry, with
//! every deferred transition behind it up to the next unsent held stroke.
//! Releases fire in press order and keys reach the front in press order,
//! so the line is always sent from its front. Whether a key's stroke is
//! still to be sent is read from here, never stored on its entry: present
//! is unsent, a dropped mark is dropped, and gone is sent.
//!
//! [`StrokeLine::pass`], [`StrokeLine::defer`] and the drops are the only
//! writers, so the line M4.8's panic path reads can never be torn
//! (`docs/design/guardian.md`).

use std::collections::{BTreeSet, VecDeque};

use serde::{Deserialize, Serialize};
use verbatim_model::KeySeq;
use verbatim_model::request::QueueEffect;
use verbatim_model::stroke::{HeldStroke, SentTransition, Stroke};

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
    },
    /// A transition deferred behind a held stroke.
    Deferred {
        /// The key.
        stroke: Stroke,
        /// A press, or else a release.
        pressed: bool,
    },
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

/// The stroke line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StrokeLine {
    /// The transitions not yet sent, in the hook's order.
    line: VecDeque<InLine>,
    /// The keys whose held stroke was dropped, each kept until its entry's
    /// turn reads it.
    dropped: BTreeSet<KeySeq>,
}

impl StrokeLine {
    /// The hook held key `key`'s `stroke`, pressed at `pressed_at_us`; a
    /// key Core sends itself passes `None` and has no release.
    pub(crate) fn hold(&mut self, key: KeySeq, stroke: HeldStroke, pressed_at_us: Option<u64>) {
        self.line.push_back(InLine::Held {
            key,
            stroke,
            release_at_us: pressed_at_us.map(|at| at.saturating_add(RELEASE_AFTER_US)),
            passed: false,
        });
    }

    /// The hook deferred a transition: it joins the line, and goes at once
    /// when nothing unsent holds the line.
    pub(crate) fn defer(&mut self, stroke: Stroke, pressed: bool) -> Option<QueueEffect> {
        self.line.push_back(InLine::Deferred { stroke, pressed });
        self.send_ready()
    }

    /// Key `key` passes: its stroke goes, with what is behind it up to the
    /// next unsent held stroke, once nothing unsent is ahead of it.
    pub(crate) fn pass(&mut self, key: KeySeq) -> Option<QueueEffect> {
        let held = self.line.iter_mut().find_map(|item| match item {
            InLine::Held {
                key: held, passed, ..
            } if *held == key => Some(passed),
            InLine::Held { .. } | InLine::Deferred { .. } => None,
        })?;
        *held = true;
        self.send_ready()
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
                InLine::Held { .. } | InLine::Deferred { .. } => None,
            })
            .min()
    }

    /// Fires every release due by `now_us`: each such key passes.
    pub(crate) fn release_due(&mut self, now_us: u64) -> Option<QueueEffect> {
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
        self.send_ready()
    }

    /// Drops every stroke not yet sent, as the input desktop leaving the
    /// user's or the hook's line being reset does, in the same step that
    /// clears the hook's own record: each held stroke whose key has not
    /// passed leaves a dropped mark for its entry's turn.
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

    /// Sends what may go from the line's front: deferred transitions and
    /// held strokes whose key has passed, up to the first that holds the
    /// line.
    fn send_ready(&mut self) -> Option<QueueEffect> {
        let mut sent = Vec::new();
        while self.line.front().is_some_and(|front| !front.holds()) {
            match self.line.pop_front() {
                Some(InLine::Held { stroke, .. }) => sent.push(SentTransition::Held(stroke)),
                Some(InLine::Deferred { stroke, pressed }) => {
                    sent.push(SentTransition::Deferred { stroke, pressed });
                }
                None => break,
            }
        }
        (!sent.is_empty()).then_some(QueueEffect::SendStrokes(sent))
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

    fn sent(effect: Option<QueueEffect>) -> Vec<SentTransition> {
        match effect {
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
            sent(line.defer(stroke(X, 1), true)),
            [SentTransition::Deferred {
                stroke: stroke(X, 1),
                pressed: true
            }]
        );
        line.hold(KeySeq::new(1), held(DOWN, 2), Some(0));
        assert_eq!(sent(line.defer(stroke(X, 3), false)), []);
        line.hold(KeySeq::new(2), held(DOWN, 4), Some(10));
        assert_eq!(sent(line.defer(stroke(SHIFT, 5), true)), []);
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Unsent);
        assert_eq!(
            sent(line.pass(KeySeq::new(1))),
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
            sent(line.pass(KeySeq::new(2))),
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
        line.hold(KeySeq::new(1), held(DOWN, 1), Some(0));
        line.hold(KeySeq::new(2), held(X, 2), Some(5));
        assert_eq!(sent(line.pass(KeySeq::new(2))), []);
        assert_eq!(line.status(KeySeq::new(2)), StrokeStatus::Sent);
        assert_eq!(
            sent(line.pass(KeySeq::new(1))),
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
        line.hold(KeySeq::new(1), held(DOWN, 1), Some(1_000));
        line.hold(KeySeq::new(2), held(X, 2), Some(40_000));
        line.hold(KeySeq::new(3), held(DOWN, 3), None);
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
        line.hold(KeySeq::new(1), held(DOWN, 1), Some(0));
        assert_eq!(sent(line.defer(stroke(X, 2), true)), []);
        line.drop_unsent();
        assert_eq!(line.status(KeySeq::new(1)), StrokeStatus::Dropped);
        assert_eq!(line.next_release(), None);
        assert_eq!(
            sent(line.defer(stroke(X, 3), false)),
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
        line.hold(KeySeq::new(1), held(DOWN, 1), Some(0));
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
            assert_eq!(sent(line.defer(stroke(vk, sequence), pressed)), []);
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
        line.hold(KeySeq::new(1), held(DOWN, 1), Some(0));
        for (vk, pressed, sequence) in [
            (SHIFT, false, 2),
            (SHIFT, true, 3),
            (CONTROL, false, 4),
            (CONTROL, false, 5),
        ] {
            assert_eq!(sent(line.defer(stroke(vk, sequence), pressed)), []);
        }
        assert_eq!(line.exit(), [stroke(DOWN, 1), stroke(CONTROL, 5)]);
    }
}
