//! The MSAA hand-over between the focus listener and an outpost's own
//! `WinEvent` hook (`docs/design/focus-pipeline.md`, section 12.4, "The MSAA
//! hand-over, exactly"; decision 2 of its section 15).
//!
//! An application's focus, foreground, menu opening, alert, and tooltip
//! events reach its outpost through the outpost's own hook. Until that hook
//! is in, the listener forwards them; while it goes in, both can take the
//! same event. Each copy names the event by its kind, address, thread, and
//! time, which two different events share only when one thread raises the
//! same event twice within one tick of the clock. So copies are matched by
//! counting, never by a window of time: while copies can still meet their
//! twins, the outpost counts the unmatched copies of each identity from
//! each source. A copy whose identity the other source has an unmatched copy
//! of is that copy's twin, and is dropped, the twin having been handled at
//! its own place; any other copy is handled, and counted. Two identical
//! events raised in one tick arrive twice from each source and are matched
//! pair by pair, so both are handled. Nothing that arrived from one source
//! alone is ever dropped.
//!
//! When copies can still meet their twins:
//!
//! - A listener copy of an event raised before the hook went in has no
//!   twin: the hook was not there to take it. The outpost reads the clock
//!   just before it installs its hook, and such an event's time is before
//!   that reading.
//! - Until the listener's "forwarding ended" reaches the outpost, any other
//!   copy from either source may have a twin.
//! - The listener stops forwarding the application's events raised after
//!   the tick it reads when it is told the hook is in, the tick its
//!   "forwarding ended" names; events raised at or before that tick it
//!   still forwards, however late they reach it. Since the supervisor
//!   relays the listener's messages in order, every copy of a later event
//!   reached the outpost before the "forwarding ended". Once that message
//!   is in, only an event raised between the hook's reading and the
//!   listener's can have copies from both sources still to meet, so only
//!   those are counted from then on, for as long as the outpost runs: a
//!   handful of events, raised in the few milliseconds of the hand-over.
//!
//! Pure, so the rules are tested on their own; the outpost holds one under
//! a lock that its hook's callback and its reader take in turn, so the
//! order in which the copies reach it is the order they are judged in.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::protocol::{FactKey, WinEventStamp, tick_is_after};

/// Where a copy of a `WinEvent` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// Forwarded by the focus listener, through the supervisor.
    Listener,
    /// Taken in by the outpost's own hook.
    Hook,
}

/// What names one `WinEvent`: its kind and address, which the fact's key
/// carries, and the thread that raised it and when.
type Identity = (FactKey, WinEventStamp);

/// The hand-over's state in one outpost.
#[derive(Debug)]
pub(super) struct Handover {
    /// The clock just before the outpost installed its hook: no event
    /// raised before it reached the hook.
    hook_from: u32,
    /// The tick the listener's "forwarding ended" named, once it is in:
    /// the listener forwards nothing raised after it.
    forwarded_through: Option<u32>,
    /// The unmatched copies from the listener, by identity.
    listener: HashMap<Identity, usize>,
    /// The unmatched copies from the hook, by identity.
    hook: HashMap<Identity, usize>,
}

impl Handover {
    /// The hand-over of an outpost whose hook is installed after the clock
    /// read `hook_from`.
    pub(super) fn new(hook_from: u32) -> Self {
        Self {
            hook_from,
            forwarded_through: None,
            listener: HashMap::new(),
            hook: HashMap::new(),
        }
    }

    /// Judges one copy of the event `key` names, raised as `stamp` says,
    /// from `source`: whether it is to be handled, or dropped as the twin
    /// of a copy already handled.
    pub(super) fn admit(&mut self, source: Source, key: FactKey, stamp: WinEventStamp) -> bool {
        if !self.may_have_twin(source, stamp) {
            return true;
        }
        let identity = (key, stamp);
        let (own, other) = match source {
            Source::Listener => (&mut self.listener, &mut self.hook),
            Source::Hook => (&mut self.hook, &mut self.listener),
        };
        if let Some(count) = other.get_mut(&identity) {
            *count -= 1;
            if *count == 0 {
                other.remove(&identity);
            }
            tracing::debug!(
                ?source,
                key = ?identity.0,
                ?stamp,
                "a WinEvent's second copy is dropped"
            );
            return false;
        }
        *own.entry(identity).or_default() += 1;
        true
    }

    /// The listener's "forwarding ended": it forwards nothing raised after
    /// `through`. Only the copies of events raised at or before it can meet
    /// their twins from now on; the others are forgotten. A second one, from
    /// a listener that replaced the first and forwards nothing for this
    /// application, keeps the earlier of the two ticks.
    pub(super) fn forwarding_ended(&mut self, through: u32) {
        let through = match self.forwarded_through {
            Some(earlier) if tick_is_after(through, earlier) => earlier,
            _ => through,
        };
        self.forwarded_through = Some(through);
        for counts in [&mut self.listener, &mut self.hook] {
            counts.retain(|(_, stamp), _| !stamp.raised_after(through));
        }
    }

    /// Ends the hand-over at once, for an outpost no listener forwards to:
    /// every copy is handled.
    pub(super) fn without_listener(&mut self) {
        self.forwarding_ended(self.hook_from.wrapping_sub(1));
    }

    /// Whether a copy from `source` raised as `stamp` says can have a twin
    /// from the other source, now or later.
    fn may_have_twin(&self, source: Source, stamp: WinEventStamp) -> bool {
        if source == Source::Listener && stamp.raised_before(self.hook_from) {
            return false;
        }
        self.forwarded_through
            .is_none_or(|through| !stamp.raised_after(through))
    }

    /// How many unmatched copies are counted, from both sources.
    #[cfg(test)]
    fn counted(&self) -> usize {
        self.listener.values().chain(self.hook.values()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use Source::{Hook, Listener};

    /// A focus on child 1 of window 7, raised by thread 3 at `time`.
    fn focus(time: u32) -> (FactKey, WinEventStamp) {
        (
            FactKey::MsaaFocus(7, -4, 1),
            WinEventStamp { thread: 3, time },
        )
    }

    /// Hands `handover` copies of the focus above, each from its source and
    /// raised at its time, in order, and says which were handled.
    fn judge(handover: &mut Handover, copies: &[(Source, u32)]) -> Vec<bool> {
        copies
            .iter()
            .map(|&(source, time)| {
                let (key, stamp) = focus(time);
                handover.admit(source, key, stamp)
            })
            .collect()
    }

    #[test]
    fn two_identical_events_seen_by_both_are_both_handled_in_either_interleaving() {
        // Two focus events on one object, raised by one thread in one tick,
        // during the overlap: two copies from each source.
        for copies in [
            [(Listener, 200), (Listener, 200), (Hook, 200), (Hook, 200)],
            [(Hook, 200), (Hook, 200), (Listener, 200), (Listener, 200)],
            [(Hook, 200), (Listener, 200), (Hook, 200), (Listener, 200)],
            [(Listener, 200), (Hook, 200), (Hook, 200), (Listener, 200)],
        ] {
            let mut handover = Handover::new(100);
            let handled = judge(&mut handover, &copies);
            assert_eq!(
                handled.iter().filter(|&&handled| handled).count(),
                2,
                "{copies:?}: {handled:?}"
            );
            assert_eq!(handover.counted(), 0, "every copy met its twin");
        }
    }

    #[test]
    fn two_from_the_listener_and_one_from_the_hook_are_two_events() {
        // One raised just before the hook went in, in the same tick as one
        // raised just after it: the listener sees both, the hook one.
        for copies in [
            [(Listener, 100), (Listener, 100), (Hook, 100)],
            [(Hook, 100), (Listener, 100), (Listener, 100)],
            [(Listener, 100), (Hook, 100), (Listener, 100)],
        ] {
            let mut handover = Handover::new(100);
            let handled = judge(&mut handover, &copies);
            assert_eq!(
                handled.iter().filter(|&&handled| handled).count(),
                2,
                "{copies:?}: {handled:?}"
            );
        }
    }

    #[test]
    fn a_copy_from_one_source_alone_is_handled() {
        let mut handover = Handover::new(100);
        // Raised before the hook went in: only the listener has it, and it
        // is not counted, as it has no twin.
        assert_eq!(judge(&mut handover, &[(Listener, 50)]), [true]);
        assert_eq!(handover.counted(), 0);
        // During the overlap, each source's first copy is handled.
        assert_eq!(judge(&mut handover, &[(Hook, 150)]), [true]);
        handover.forwarding_ended(160);
        // Raised after the listener stopped: only the hook has it, and it
        // is not counted either. The copy above, raised during the
        // hand-over, is still counted, for a late twin.
        assert_eq!(
            judge(&mut handover, &[(Hook, 170), (Hook, 170)]),
            [true, true]
        );
        assert_eq!(handover.counted(), 1);
    }

    #[test]
    fn forwarding_ended_forgets_the_copies_raised_after_it() {
        let mut handover = Handover::new(100);
        assert_eq!(
            judge(&mut handover, &[(Hook, 150), (Hook, 170)]),
            [true, true]
        );
        handover.forwarding_ended(160);
        assert_eq!(handover.counted(), 1, "only the copy raised at 150");
    }

    #[test]
    fn after_forwarding_ended_a_late_copy_from_the_hand_over_meets_its_twin() {
        // An event raised at 150, while the hook went in, reached the hook
        // at once and the listener only after its "forwarding ended".
        let mut handover = Handover::new(100);
        assert_eq!(judge(&mut handover, &[(Hook, 150)]), [true]);
        handover.forwarding_ended(160);
        assert_eq!(judge(&mut handover, &[(Listener, 150)]), [false]);
        // And the other way round.
        let mut handover = Handover::new(100);
        handover.forwarding_ended(160);
        assert_eq!(
            judge(&mut handover, &[(Listener, 150), (Hook, 150)]),
            [true, false]
        );
    }

    #[test]
    fn a_replacement_listeners_forwarding_ended_keeps_the_earlier_tick() {
        let mut handover = Handover::new(100);
        handover.forwarding_ended(160);
        // A listener that replaced the first, long after, forwards nothing
        // for this application: a hook copy raised later is not counted.
        handover.forwarding_ended(5000);
        assert_eq!(judge(&mut handover, &[(Hook, 4000)]), [true]);
        assert_eq!(handover.counted(), 0);
    }

    #[test]
    fn an_outpost_no_listener_forwards_to_handles_every_copy() {
        let mut handover = Handover::new(100);
        handover.without_listener();
        assert_eq!(
            judge(&mut handover, &[(Hook, 100), (Hook, 100)]),
            [true, true]
        );
        assert_eq!(handover.counted(), 0);
    }
}
