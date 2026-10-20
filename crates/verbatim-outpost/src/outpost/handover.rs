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
//!   those are counted from then on: a handful of events, raised in the
//!   few milliseconds of the hand-over.
//! - Those are counted for [`TWIN_BOUND`], 15 seconds, after the
//!   "forwarding ended" arrived, the longest a late copy can trail it, as
//!   the constant's documentation shows. Once that has passed, the counts
//!   are dropped and no copy is counted again, so every copy is handled.
//!   Measuring this on the outpost's own monotonic clock, not by raise
//!   times, keeps the comparison of raise times, which wraps once two
//!   ticks are 2^31 ms (about 24.8 days) apart, to copies raised within
//!   seconds of the tick "forwarding ended" names, so an outpost that
//!   runs for weeks never counts its hook's copies as raised before it.
//!
//! The hand-over also tells which of the listener's copies it handles were
//! raised before the hook went in ([`Arrival::ListenerBeforeHook`]), so
//! that the intake can order a run of events raised in one tick of the
//! clock as they were raised (`intake::in_raised_order`): an outpost can
//! receive the listener's copy of an event after its own hook's copy of a
//! later one raised in the same tick, and the tick alone cannot order the
//! two. It is known in two cases:
//!
//! - The event was raised before the clock reading taken just before the
//!   hook went in.
//! - The copy has no twin while the hook has an unmatched copy of an event
//!   raised in the same tick. The listener's hook receives an
//!   application's events in the order they were raised, the listener
//!   sends them in that order and the supervisor relays them in order, so
//!   had the hooked event been raised first, its listener copy would have
//!   arrived first and matched it. And the outpost's hook receives them in
//!   that order too, so had it received this event, raised before the one
//!   it has, its copy would already be here as this one's twin. This rests
//!   on every listener copy of a hooked event raised in the tick reaching
//!   the outpost, and on the outpost's hook keeping every event the
//!   listener forwards. The outpost's hook drops what the listener drops
//!   for having no window or a window with no owner, as NVDA's callback
//!   drops an invalid window, but the two sources can still lose different
//!   copies of one event:
//!   - the listener's backlog replaces a waiting fact with a newer one of
//!     the same element and kind while a write to Core is in flight, which
//!     loses the listener's copy;
//!   - the listener drops an event when it finds no process to route it
//!     to, as for a console window whose console host it cannot find,
//!     which loses the listener's copy;
//!   - the window is destroyed between the two callbacks' reads of its
//!     owner, each callback reading it when it runs. When the outpost's
//!     hook reads first, the listener drops its copy and the hook keeps
//!     its own. When the listener reads first, it forwards its copy and
//!     the outpost's hook drops its own.
//!
//!   A lost listener copy leaves its hook copy unmatched, so a later
//!   listener copy raised in the same tick is taken as raised before the
//!   hook. A lost hook copy leaves its listener copy with no twin, and
//!   while the hook holds an unmatched copy of a later event of the same
//!   tick, as it usually does since hook copies arrive first, the listener
//!   copy is taken as raised before the hook, so it and every listener copy
//!   that arrived before it in that tick go ahead of the hook copies of
//!   the tick raised before it. In each case the order can be wrong only
//!   among the events of that tick, but for one loss: a focus on a
//!   window's client object wrongly judged raised before the hook, arriving
//!   after a hook copy of its window's foreground change from the same
//!   thread in that tick, is dropped, not reordered, since the intake drops
//!   a focus raised before its window's change (`intake::foreground_waits_after`),
//!   and its hook copy is then dropped as its twin. For example, the
//!   window's thread raises foreground changes G1 and G2 of the window and
//!   then the focus, in one tick; the listener's backlog replaces G1's
//!   copy with G2's; G2's copies meet, and G1's hook copy stays unmatched;
//!   the focus's listener copy, arriving before its hook copy, is judged
//!   raised before the hook, and the hook's copy of G2, which replaced G1's
//!   in the intake, drops it, where NVDA, seeing G1, G2 and the focus in
//!   that order, keeps the focus.
//!
//! Pure, so the rules are tested on their own; the outpost holds one under
//! a lock that its hook's callback and its reader take in turn, and queues
//! each copy it admits before it releases the lock, so the order in which
//! the copies reach it is the order they are judged in, and the order the
//! intake receives them in too.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::protocol::{FactKey, WinEventStamp, tick_is_after};

/// How long after the listener's "forwarding ended" reaches the outpost a
/// copy can still meet its twin: the longest a late copy can trail that
/// message. A listener copy arrives after it only when the listener's hook
/// thread received the event after the listener read its tick; that copy
/// then passes the listener's backlog and the supervisor's relay. The hook
/// thread makes no cross-process call, and delivered every event within
/// one tick of its raise (at most 16 ms in the 578 hook deliveries logged
/// in the end-to-end evidence of fixes rounds three to seven, where a twin
/// trailed its first copy by at most 6.3 ms). The relay's maximum
/// measured is 146 ms (`docs/design/focus-pipeline.md`, section 12.4). A
/// write stalled in the listener's backlog holds its pongs too, and the
/// supervisor replaces a listener that has answered no ping for three
/// intervals of 3 seconds, checked at each ping, so within 12 seconds,
/// after which nothing more comes from it. 15 seconds covers those with a
/// margin. A copy that came later still would be handled a second time.
pub(super) const TWIN_BOUND: Duration = Duration::from_secs(15);

/// Where a copy of a `WinEvent` came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Source {
    /// Forwarded by the focus listener, through the supervisor.
    Listener,
    /// Taken in by the outpost's own hook.
    Hook,
}

/// How a copy the hand-over has the outpost handle reached it, which
/// orders the events of a run raised in one tick (`intake::in_raised_order`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arrival {
    /// Through the outpost's own hook.
    Hook,
    /// Forwarded by the focus listener, not known to have been raised
    /// before the hook went in.
    Listener,
    /// Forwarded by the focus listener, and known to have been raised
    /// before the hook went in, so before every event the hook received.
    ListenerBeforeHook,
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
    /// The tick the listener's "forwarding ended" named, and when that
    /// message reached the outpost, once it is in: the listener forwards
    /// nothing raised after the tick.
    forwarded_through: Option<(u32, Instant)>,
    /// Whether the hand-over is over: [`TWIN_BOUND`] has passed since
    /// "forwarding ended" came, or no listener forwards to the outpost, so
    /// no copy is counted and nothing is kept.
    over: bool,
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
            over: false,
            listener: HashMap::new(),
            hook: HashMap::new(),
        }
    }

    /// Judges one copy of the event `key` names, raised as `stamp` says,
    /// from `source`: how it arrived, when it is to be handled, or `None`
    /// when it is dropped as the twin of a copy already handled. A
    /// listener's copy is known to have been raised before the hook went in
    /// when it was raised before the clock reading, or when it has no twin
    /// while the hook has an unmatched copy raised in the same tick (the
    /// module's documentation says why). `now` reads the outpost's
    /// monotonic clock, and is called only after "forwarding ended" and
    /// until [`TWIN_BOUND`] has passed, so nothing is read once the
    /// hand-over is over.
    pub(super) fn admit(
        &mut self,
        source: Source,
        key: FactKey,
        stamp: WinEventStamp,
        now: impl FnOnce() -> Instant,
    ) -> Option<Arrival> {
        let arrival = match source {
            Source::Hook => Arrival::Hook,
            Source::Listener if stamp.raised_before(self.hook_from) => Arrival::ListenerBeforeHook,
            Source::Listener => Arrival::Listener,
        };
        if !self.may_have_twin(source, stamp, now) {
            return Some(arrival);
        }
        // Asked before the twin is looked for: a twin is the same event,
        // so it only matters when there is none.
        let hooked_in_its_tick = arrival == Arrival::Listener
            && self
                .hook
                .keys()
                .any(|(_, hooked)| hooked.time == stamp.time);
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
            return None;
        }
        *own.entry(identity).or_default() += 1;
        Some(if hooked_in_its_tick {
            Arrival::ListenerBeforeHook
        } else {
            arrival
        })
    }

    /// The listener's "forwarding ended", which reached the outpost at
    /// `at`: it forwards nothing raised after `through`. Only the copies of
    /// events raised at or before it can meet their twins from now on, for
    /// [`TWIN_BOUND`] after `at`; the others are forgotten. A second one,
    /// from a listener that replaced the first and forwards nothing for
    /// this application, keeps the earlier of the two ticks and the time
    /// the first came.
    pub(super) fn forwarding_ended(&mut self, through: u32, at: Instant) {
        if self.over {
            return;
        }
        let ended = match self.forwarded_through {
            Some((earlier, came)) if tick_is_after(through, earlier) => (earlier, came),
            Some((_, came)) => (through, came),
            None => (through, at),
        };
        self.forwarded_through = Some(ended);
        for counts in [&mut self.listener, &mut self.hook] {
            counts.retain(|(_, stamp), _| !stamp.raised_after(ended.0));
        }
    }

    /// Ends the hand-over at once, for an outpost no listener forwards to:
    /// every copy is handled, and none is counted.
    pub(super) fn without_listener(&mut self) {
        self.end();
    }

    /// Whether a copy from `source` raised as `stamp` says can have a twin
    /// from the other source, now or later; ends the hand-over once
    /// [`TWIN_BOUND`] has passed since "forwarding ended" came, by the
    /// clock `now` reads.
    fn may_have_twin(
        &mut self,
        source: Source,
        stamp: WinEventStamp,
        now: impl FnOnce() -> Instant,
    ) -> bool {
        if self.over || source == Source::Listener && stamp.raised_before(self.hook_from) {
            return false;
        }
        let Some((through, came)) = self.forwarded_through else {
            return true;
        };
        if now().saturating_duration_since(came) > TWIN_BOUND {
            self.end();
            return false;
        }
        !stamp.raised_after(through)
    }

    /// The hand-over is over: the counts are dropped, and no copy is
    /// counted from now on.
    fn end(&mut self) {
        tracing::debug!(
            unmatched = self.listener.len() + self.hook.len(),
            "the MSAA hand-over is over; no copy is counted from now on"
        );
        self.over = true;
        self.listener = HashMap::new();
        self.hook = HashMap::new();
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

    /// The stand-in clock's one reading: every copy is admitted, and every
    /// "forwarding ended" comes, at this instant, unless a test reads it
    /// later.
    fn start() -> Instant {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *START.get_or_init(Instant::now)
    }

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
                handover.admit(source, key, stamp, start).is_some()
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
        handover.forwarding_ended(160, start());
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
        handover.forwarding_ended(160, start());
        assert_eq!(handover.counted(), 1, "only the copy raised at 150");
    }

    #[test]
    fn after_forwarding_ended_a_late_copy_from_the_hand_over_meets_its_twin() {
        // An event raised at 150, while the hook went in, reached the hook
        // at once and the listener only after its "forwarding ended".
        let mut handover = Handover::new(100);
        assert_eq!(judge(&mut handover, &[(Hook, 150)]), [true]);
        handover.forwarding_ended(160, start());
        assert_eq!(judge(&mut handover, &[(Listener, 150)]), [false]);
        // And the other way round.
        let mut handover = Handover::new(100);
        handover.forwarding_ended(160, start());
        assert_eq!(
            judge(&mut handover, &[(Listener, 150), (Hook, 150)]),
            [true, false]
        );
    }

    #[test]
    fn a_replacement_listeners_forwarding_ended_keeps_the_earlier_tick() {
        let mut handover = Handover::new(100);
        handover.forwarding_ended(160, start());
        // A listener that replaced the first, long after, forwards nothing
        // for this application: a hook copy raised later is not counted.
        handover.forwarding_ended(5000, start());
        assert_eq!(judge(&mut handover, &[(Hook, 4000)]), [true]);
        assert_eq!(handover.counted(), 0);
    }

    /// The `return_after_retirement` failure's hand-over: the hook's copy of
    /// a focus raised at 150, then the listener's copy of a foreground
    /// change raised in the same tick, then the listener's copy of the
    /// focus. The change is known to have been raised before the hook went
    /// in, since its copy came before the focus's listener copy and the
    /// hook has none of it; a listener copy with no hooked event in its
    /// tick is not, and one raised before the clock reading is.
    #[test]
    fn a_listener_copy_ahead_of_a_hooked_events_twin_in_its_tick_was_raised_before_the_hook() {
        let mut handover = Handover::new(100);
        let stamp = |time| WinEventStamp { thread: 3, time };
        let foreground = FactKey::Foreground(8);
        assert_eq!(
            handover.admit(Hook, focus(150).0, stamp(150), start),
            Some(Arrival::Hook)
        );
        assert_eq!(
            handover.admit(Listener, foreground.clone(), stamp(150), start),
            Some(Arrival::ListenerBeforeHook)
        );
        assert_eq!(
            handover.admit(Listener, focus(150).0, stamp(150), start),
            None
        );
        assert_eq!(
            handover.admit(Listener, foreground.clone(), stamp(166), start),
            Some(Arrival::Listener)
        );
        assert_eq!(
            handover.admit(Listener, foreground, stamp(99), start),
            Some(Arrival::ListenerBeforeHook)
        );
    }

    /// A listener copy raised in a tick whose hooked copy has already met
    /// its twin is not known to have been raised before the hook: the hook
    /// holds no unmatched copy of that tick.
    #[test]
    fn a_listener_copy_in_a_tick_whose_hooked_copy_met_its_twin_is_not_before_the_hook() {
        let mut handover = Handover::new(100);
        let stamp = |time| WinEventStamp { thread: 3, time };
        assert_eq!(
            handover.admit(Hook, focus(150).0, stamp(150), start),
            Some(Arrival::Hook)
        );
        assert_eq!(
            handover.admit(Listener, focus(150).0, stamp(150), start),
            None
        );
        assert_eq!(
            handover.admit(Listener, FactKey::Foreground(8), stamp(150), start),
            Some(Arrival::Listener)
        );
    }

    /// Once [`TWIN_BOUND`] has passed since "forwarding ended" came, the
    /// copy counted during the hand-over is dropped, and a hook copy raised
    /// 2^31 ms after the tick that message named, which the wrapping tick
    /// comparison takes as raised before it, is handled and not counted.
    #[test]
    fn past_the_bound_after_forwarding_ended_no_copy_is_counted_and_nothing_is_kept() {
        let mut handover = Handover::new(100);
        assert_eq!(judge(&mut handover, &[(Hook, 150)]), [true]);
        handover.forwarding_ended(160, start());
        assert_eq!(handover.counted(), 1, "the copy raised at 150");
        let past = || start() + TWIN_BOUND + Duration::from_millis(1);
        let (key, stamp) = focus(160u32.wrapping_add(1 << 31));
        for _ in 0..2 {
            assert_eq!(
                handover.admit(Hook, key.clone(), stamp, past),
                Some(Arrival::Hook)
            );
        }
        assert_eq!(handover.counted(), 0);
        assert!(handover.listener.is_empty() && handover.hook.is_empty());
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
