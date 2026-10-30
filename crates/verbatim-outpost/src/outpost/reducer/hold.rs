//! The hand-over's hold (`docs/design/ownership-outpost-reducer.md`, "The
//! hand-over's hold"; Dickson, 2026-10-25 and 2026-10-31): how an
//! outpost's own `WinEvent` hook takes over from the focus listener.
//!
//! An application's focus, foreground, menu opening, alert, and tooltip
//! events reach its outpost through the outpost's own hook. Until that hook
//! is in, the listener forwards them; while it goes in, both can take the
//! same event. The listener stops forwarding the application's events
//! raised after the tick it reads when it is told the hook is in, the tick
//! its "forwarding ended" names; events raised at or before that tick it
//! still forwards, and since the supervisor relays the listener's messages
//! in order, every copy it forwards reaches the outpost before that
//! message.
//!
//! So the reducer holds the hook's whole stream from the hook's install
//! until "forwarding ended", and handles the listener's copies as they
//! come. At "forwarding ended" it drops the held copies of the kinds the
//! listener forwards raised at or before the named tick, since the
//! listener forwarded exactly those, and releases the rest in the hook's
//! order, as one batch, ahead of anything the hook delivers later. If the
//! listener's reads end first (`UiaFocusReadsEnded`), the hold is released,
//! dropping the held copies raised at or before the newest listener copy
//! handled. An outpost no listener forwards to holds nothing.
//!
//! Holding the whole stream, not only the forwarded kinds, keeps the hook's
//! own order, which the intake's ordering of a run, its caret event after
//! a terminal's update, and the caret key's watch read: holding only the
//! forwarded kinds would let a caret or value change pass a focus raised
//! before it. The cost is paid only at an outpost's start: the hook's
//! stream waits a median of 20.6 ms, at most 591.3 ms over 126 starts
//! (`docs/design/focus-pipeline-rework-remaining.md`, "The hand-over's
//! hold").

use verbatim_model::protocol::{DeliveredFact, WinEventStamp, tick_is_after};

use super::entry::{Entry, Item};

/// The hold's state in one outpost.
#[derive(Debug, Default)]
pub(in crate::outpost) struct Hold {
    state: State,
}

#[derive(Debug, Default)]
enum State {
    /// No hook is in yet: nothing comes from it to hold.
    #[default]
    Unhooked,
    /// The hook is in and the listener still forwards: the hook's copies
    /// are held, in the order they came.
    Holding {
        held: Vec<Entry>,
        /// The raise time of the newest listener copy of a forwarded kind
        /// handled, if any.
        newest_forwarded: Option<u32>,
    },
    /// The hold is released, or there is no listener: every copy is
    /// handled as it comes.
    Over,
}

impl Hold {
    /// The outpost's hook is in, and a listener forwards to the outpost:
    /// the hook's copies are held from now on.
    pub(in crate::outpost) fn hooked(&mut self) {
        if matches!(self.state, State::Unhooked) {
            self.state = State::Holding {
                held: Vec::new(),
                newest_forwarded: None,
            };
        }
    }

    /// No listener forwards to this outpost: nothing is held, and anything
    /// held is handed back to be handled.
    pub(in crate::outpost) fn without_listener(&mut self) -> Vec<Entry> {
        match std::mem::replace(&mut self.state, State::Over) {
            State::Holding { held, .. } => held,
            State::Unhooked | State::Over => Vec::new(),
        }
    }

    /// Whether the hook's copies are being held.
    pub(in crate::outpost) fn holding(&self) -> bool {
        matches!(self.state, State::Holding { .. })
    }

    /// The copies held, in the order they came.
    pub(in crate::outpost) fn held(&self) -> impl Iterator<Item = &Entry> {
        let held = match &self.state {
            State::Holding { held, .. } => held.as_slice(),
            State::Unhooked | State::Over => &[],
        };
        held.iter()
    }

    /// A copy from the outpost's own hook: held while the hold lasts,
    /// handed back to be handled otherwise.
    pub(in crate::outpost) fn hooked_copy(&mut self, entry: Entry) -> Option<Entry> {
        match &mut self.state {
            State::Holding { held, .. } => {
                held.push(entry);
                None
            }
            State::Unhooked | State::Over => Some(entry),
        }
    }

    /// A copy the listener forwarded, handled as it comes: while the hold
    /// lasts, its raise time is noted as the newest forwarded copy handled.
    pub(in crate::outpost) fn listener_copy(&mut self, entry: &Entry) {
        if let State::Holding {
            newest_forwarded, ..
        } = &mut self.state
            && let Some(time) = forwarded_time(entry)
            && newest_forwarded.is_none_or(|newest| tick_is_after(time, newest))
        {
            *newest_forwarded = Some(time);
        }
    }

    /// The listener's "forwarding ended": it forwarded every copy of its
    /// kinds raised at or before `through`, and none raised after. The held
    /// copies of those kinds raised at or before it are dropped, and the
    /// rest are returned, in the hook's order, to be handled as one batch.
    /// A later "forwarding ended", from a listener that replaced the first,
    /// finds the hold released and changes nothing.
    pub(in crate::outpost) fn forwarding_ended(&mut self, through: u32) -> Option<Vec<Entry>> {
        self.release(Some(through))
    }

    /// The listener's reads ended before its "forwarding ended" came: the
    /// held copies raised at or before the newest listener copy handled are
    /// dropped, and the rest returned as for
    /// [`forwarding_ended`](Self::forwarding_ended).
    pub(in crate::outpost) fn reads_ended(&mut self) -> Option<Vec<Entry>> {
        let State::Holding {
            newest_forwarded, ..
        } = &self.state
        else {
            return None;
        };
        let through = *newest_forwarded;
        self.release(through)
    }

    /// Releases the hold, dropping the held copies of the forwarded kinds
    /// raised at or before `through`, when given: `None` when there was no
    /// hold to release.
    fn release(&mut self, through: Option<u32>) -> Option<Vec<Entry>> {
        let State::Holding { held, .. } = std::mem::replace(&mut self.state, State::Over) else {
            return None;
        };
        Some(
            held.into_iter()
                .filter(|entry| {
                    through.is_none_or(|through| {
                        forwarded_time(entry).is_none_or(|time| tick_is_after(time, through))
                    })
                })
                .collect(),
        )
    }
}

/// When the event of `entry` was raised, if it is a copy of a kind the
/// listener forwards: a focus, foreground change, menu opening, alert, or
/// tooltip, which the hook takes in as the fact the listener forwards.
fn forwarded_time(entry: &Entry) -> Option<u32> {
    match &entry.item {
        Item::Fact(fact, Some(WinEventStamp { time, .. })) if forwarded(fact) => Some(*time),
        _ => None,
    }
}

/// Whether the listener forwards `fact` while the hook goes in.
fn forwarded(fact: &DeliveredFact) -> bool {
    matches!(
        fact,
        DeliveredFact::MsaaFocus { .. }
            | DeliveredFact::Foreground { .. }
            | DeliveredFact::MenuPopup { .. }
            | DeliveredFact::Alert { .. }
            | DeliveredFact::Show { .. }
    )
}

#[cfg(test)]
mod tests {
    use verbatim_model::TraceId;
    use verbatim_model::protocol::EventTiming;

    use super::super::entry::WinKind;
    use super::*;

    /// A focus on child `child` of window 7, raised by thread 3 at `time`.
    fn focus(child: i32, time: u32) -> Entry {
        Entry {
            item: Item::Fact(
                DeliveredFact::MsaaFocus {
                    hwnd: 7,
                    id_object: -4,
                    id_child: child,
                },
                Some(WinEventStamp { thread: 3, time }),
            ),
            trace: TraceId::mint(),
            observed_at_us: u64::from(time),
            timing: EventTiming::default(),
        }
    }

    /// A value change of window 7, which the listener never forwards.
    fn value_change(observed_at_us: u64) -> Entry {
        Entry {
            item: Item::Msaa {
                kind: WinKind::ValueChange,
                hwnd: 7,
                id_object: -4,
                id_child: 0,
                raiser: 3,
            },
            trace: TraceId::mint(),
            observed_at_us,
            timing: EventTiming::default(),
        }
    }

    fn observed(entries: &[Entry]) -> Vec<u64> {
        entries.iter().map(|entry| entry.observed_at_us).collect()
    }

    #[test]
    fn the_hooks_copies_are_held_until_forwarding_ends_and_those_forwarded_are_dropped() {
        let mut hold = Hold::default();
        hold.hooked();
        // The hook's copies of two focuses raised at 100 and 200, with a
        // value change between them, all held.
        assert!(hold.hooked_copy(focus(1, 100)).is_none());
        assert!(hold.hooked_copy(value_change(150)).is_none());
        assert!(hold.hooked_copy(focus(2, 200)).is_none());
        // The listener forwarded what was raised at or before 100.
        let released = hold.forwarding_ended(100).expect("the hold is released");
        assert_eq!(observed(&released), [150, 200]);
        assert!(!hold.holding());
        // From now on the hook's copies are handled as they come.
        assert_eq!(
            hold.hooked_copy(focus(3, 300))
                .map(|entry| entry.observed_at_us),
            Some(300)
        );
    }

    #[test]
    fn a_replacement_listeners_forwarding_ended_changes_nothing() {
        let mut hold = Hold::default();
        hold.hooked();
        assert!(hold.hooked_copy(focus(1, 100)).is_none());
        assert_eq!(
            hold.forwarding_ended(50)
                .map(|released| observed(&released)),
            Some(vec![100])
        );
        assert!(hold.forwarding_ended(200).is_none());
    }

    #[test]
    fn the_end_of_the_listeners_reads_releases_the_hold_past_its_newest_copy() {
        let mut hold = Hold::default();
        hold.hooked();
        hold.listener_copy(&focus(1, 100));
        assert!(hold.hooked_copy(focus(1, 100)).is_none());
        assert!(hold.hooked_copy(focus(2, 120)).is_none());
        let released = hold.reads_ended().expect("the hold is released");
        assert_eq!(observed(&released), [120]);
    }

    #[test]
    fn the_end_of_the_reads_with_no_listener_copy_drops_nothing() {
        let mut hold = Hold::default();
        hold.hooked();
        assert!(hold.hooked_copy(focus(1, 100)).is_none());
        let released = hold.reads_ended().expect("the hold is released");
        assert_eq!(observed(&released), [100]);
    }

    #[test]
    fn an_outpost_no_listener_forwards_to_holds_nothing() {
        let mut hold = Hold::default();
        assert!(hold.without_listener().is_empty());
        hold.hooked();
        assert!(!hold.holding());
        assert_eq!(
            hold.hooked_copy(focus(1, 100))
                .map(|entry| entry.observed_at_us),
            Some(100)
        );
    }

    #[test]
    fn a_copy_raised_across_the_clocks_wrap_is_judged_by_the_wrapping_tick() {
        let mut hold = Hold::default();
        hold.hooked();
        // Raised just after the clock wrapped, after a tick near its end.
        assert!(hold.hooked_copy(focus(1, 5)).is_none());
        let released = hold
            .forwarding_ended(u32::MAX - 5)
            .expect("the hold is released");
        assert_eq!(observed(&released), [5]);
    }
}
