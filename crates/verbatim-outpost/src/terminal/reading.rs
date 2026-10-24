//! On-demand reading (`docs/design/terminal-model.md`, "When to read",
//! Dickson 2026-10-07): the outpost reads a focused terminal at once while
//! Core has room for what it says, and only notes that it changed while
//! Core's output queue is full; Core asks for a read when it hands the last
//! line of a group to speech. A pure transition function over every state
//! and event, which the worker drives. A read is one snapshot, which the
//! terminal answers from one locked state of its buffer, so no read is ever
//! disturbed by the terminal writing to it.

/// Where a focused terminal's reading stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reading {
    /// Each change of the text is read at once and what it says sent.
    #[default]
    Live,
    /// Core's queue is full: changes are only noted.
    Held {
        /// Whether the text changed since the last read.
        changed: bool,
    },
}

/// What happens to a terminal's reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// The terminal's text changed, or something else asks for a read: its
    /// caret moved in Windows Terminal, or a caret key's watch found its
    /// evidence.
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
}

/// What the worker does for a transition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing.
    Nothing,
    /// Read, and send what it says.
    Report,
    /// Read, and answer Core's request with what it says, as one that
    /// waited for Core's queue.
    Reply,
    /// Answer Core's request with nothing, without reading: nothing
    /// changed since the last read.
    ReplyEmpty,
    /// Read to the end and answer the cancel with it: what changed while
    /// held, of which Core speaks only the answers to keys.
    ReplySilently,
    /// Read only to note where the text is now: the first read after the
    /// focus arrived, which speaks nothing.
    Baseline,
}

impl Reading {
    /// The state after `event`, and what to do.
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
            // A cut reads only what was held back from Core. Live, or held
            // with nothing changed, Core already has everything read
            // before the cut, and a read now would find only what the key
            // that cut speech did, taking it from the report that would
            // speak it.
            (Self::Held { changed: true }, Event::Cancel) => (Self::Live, Action::ReplySilently),
            (Self::Live | Self::Held { changed: false }, Event::Cancel) => {
                (Self::Live, Action::ReplyEmpty)
            }
            (_, Event::Focused) => (Self::Live, Action::Baseline),
            (Self::Live, Event::TextChanged) => (Self::Live, Action::Report),
            (Self::Held { .. }, Event::TextChanged) => {
                (Self::Held { changed: true }, Action::Nothing)
            }
            (Self::Live, Event::Hold) => (Self::Held { changed: false }, Action::Nothing),
            (Self::Held { changed }, Event::Hold) => (Self::Held { changed }, Action::Nothing),
            (Self::Held { changed: true }, Event::Read { hold }) => {
                (after_read(hold), Action::Reply)
            }
            (Self::Live | Self::Held { changed: false }, Event::Read { hold }) => {
                (after_read(hold), Action::ReplyEmpty)
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
                Action::ReplyEmpty,
            ),
            (
                Reading::Live,
                Event::Focused,
                Reading::Live,
                Action::Baseline,
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
            (held, Event::Cancel, Reading::Live, Action::ReplyEmpty),
            (held, Event::Focused, Reading::Live, Action::Baseline),
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
        ];
        for (state, event, after, action) in cases {
            assert_eq!(state.next(event), (after, action), "{state:?} on {event:?}");
        }
    }
}
