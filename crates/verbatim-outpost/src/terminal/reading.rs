//! On-demand reading (`phase6-design.md`, "Terminal decisions", Dickson
//! 2026-10-07): the outpost reads a focused terminal's new output at once
//! while Core has room for it, and only notes that it changed while Core's
//! output queue is full; Core asks for it when it hands the last line of a
//! group to speech. A pure transition function over every state and event,
//! which the worker drives.

/// Where a focused terminal's reading stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    /// Each change of the text is read at once and its output sent.
    #[default]
    Live,
    /// Core's queue is full: changes are only noted.
    Held {
        /// Whether the text changed since the last read.
        changed: bool,
    },
    /// Core asked for a read, and the read was disturbed by the terminal
    /// writing to it: the answer is owed, and the next change, which the
    /// writing raises, reads again. Then reading holds (`hold`) or goes
    /// live, as the request asked.
    Owed {
        /// Whether reading holds once the answer is given.
        hold: bool,
    },
}

/// What happens to a terminal's reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The terminal's text changed.
    TextChanged,
    /// Core's queue is full ([`verbatim_model::TextOp::TerminalHold`]).
    Hold,
    /// Core asks for what is new, then to hold or not
    /// ([`verbatim_model::TextOp::TerminalRead`]).
    Read {
        /// Whether to hold after it.
        hold: bool,
    },
    /// Speech was cut off ([`verbatim_model::TextOp::TerminalCancel`]).
    Cancel,
    /// The terminal gained the focus: what it holds is not new.
    Focused,
    /// A read made to answer Core ([`Action::Reply`]) was disturbed by the
    /// terminal writing to it, so it found nothing to trust.
    ReplyUnsettled,
}

/// What the worker does for a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    Nothing,
    /// Read, and send what is new as output.
    Report,
    /// Read, and answer Core's request (the one asked now, or the one
    /// owed) with what is new.
    Reply,
    /// Answer Core's request with nothing new, without reading: nothing
    /// changed since the last read.
    ReplyEmpty,
    /// Read to the end and answer the cancel with it, for typing echo
    /// only.
    ReplySilently,
    /// Read only to note where the text is now.
    Baseline,
}

impl Reading {
    /// The state after `event`, and what to do. An answer owed when the
    /// state leaves [`Reading::Owed`] by anything but [`Action::Reply`] is
    /// the worker's to give, empty.
    #[must_use]
    pub fn next(self, event: Event) -> (Self, Action) {
        let after_read = |hold: bool| {
            if hold {
                Self::Held { changed: false }
            } else {
                Self::Live
            }
        };
        match (self, event) {
            (_, Event::Cancel) => (Self::Live, Action::ReplySilently),
            (_, Event::Focused) => (Self::Live, Action::Baseline),
            (Self::Live, Event::TextChanged) => (Self::Live, Action::Report),
            (Self::Held { .. }, Event::TextChanged) => {
                (Self::Held { changed: true }, Action::Nothing)
            }
            (Self::Owed { hold }, Event::TextChanged) => (after_read(hold), Action::Reply),
            (Self::Live, Event::Hold) => (Self::Held { changed: false }, Action::Nothing),
            (Self::Held { changed }, Event::Hold) => (Self::Held { changed }, Action::Nothing),
            (Self::Owed { .. }, Event::Hold) => (Self::Owed { hold: true }, Action::Nothing),
            (Self::Held { changed: true }, Event::Read { hold }) => {
                (after_read(hold), Action::Reply)
            }
            (Self::Live | Self::Held { changed: false }, Event::Read { hold }) => {
                (after_read(hold), Action::ReplyEmpty)
            }
            // Core asks once at a time; a second request takes the first's
            // place.
            (Self::Owed { .. }, Event::Read { hold })
            | (Self::Owed { hold }, Event::ReplyUnsettled) => {
                (Self::Owed { hold }, Action::Nothing)
            }
            (Self::Live, Event::ReplyUnsettled) => (Self::Owed { hold: false }, Action::Nothing),
            (Self::Held { .. }, Event::ReplyUnsettled) => {
                (Self::Owed { hold: true }, Action::Nothing)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Event, Reading};

    #[test]
    fn every_state_and_event() {
        let held = Reading::Held { changed: false };
        let changed = Reading::Held { changed: true };
        let owed = Reading::Owed { hold: true };
        let owed_live = Reading::Owed { hold: false };
        let cases = [
            (
                Reading::Live,
                Event::TextChanged,
                Reading::Live,
                Action::Report,
            ),
            (Reading::Live, Event::Hold, held, Action::Nothing),
            (
                Reading::Live,
                Event::Read { hold: true },
                held,
                Action::ReplyEmpty,
            ),
            (
                Reading::Live,
                Event::Read { hold: false },
                Reading::Live,
                Action::ReplyEmpty,
            ),
            (
                Reading::Live,
                Event::Cancel,
                Reading::Live,
                Action::ReplySilently,
            ),
            (
                Reading::Live,
                Event::Focused,
                Reading::Live,
                Action::Baseline,
            ),
            (
                Reading::Live,
                Event::ReplyUnsettled,
                owed_live,
                Action::Nothing,
            ),
            (held, Event::TextChanged, changed, Action::Nothing),
            (held, Event::Hold, held, Action::Nothing),
            (held, Event::Read { hold: true }, held, Action::ReplyEmpty),
            (
                held,
                Event::Read { hold: false },
                Reading::Live,
                Action::ReplyEmpty,
            ),
            (held, Event::Cancel, Reading::Live, Action::ReplySilently),
            (held, Event::Focused, Reading::Live, Action::Baseline),
            (held, Event::ReplyUnsettled, owed, Action::Nothing),
            (changed, Event::TextChanged, changed, Action::Nothing),
            (changed, Event::Hold, changed, Action::Nothing),
            (changed, Event::Read { hold: true }, held, Action::Reply),
            (
                changed,
                Event::Read { hold: false },
                Reading::Live,
                Action::Reply,
            ),
            (changed, Event::Cancel, Reading::Live, Action::ReplySilently),
            (changed, Event::Focused, Reading::Live, Action::Baseline),
            (changed, Event::ReplyUnsettled, owed, Action::Nothing),
            (owed, Event::TextChanged, held, Action::Reply),
            (owed_live, Event::TextChanged, Reading::Live, Action::Reply),
            (owed_live, Event::Hold, owed, Action::Nothing),
            (owed, Event::Hold, owed, Action::Nothing),
            (
                owed,
                Event::Read { hold: false },
                owed_live,
                Action::Nothing,
            ),
            (owed, Event::Cancel, Reading::Live, Action::ReplySilently),
            (owed, Event::Focused, Reading::Live, Action::Baseline),
            (owed, Event::ReplyUnsettled, owed, Action::Nothing),
            (owed_live, Event::ReplyUnsettled, owed_live, Action::Nothing),
        ];
        for (state, event, after, action) in cases {
            assert_eq!(state.next(event), (after, action), "{state:?} on {event:?}");
        }
    }
}
