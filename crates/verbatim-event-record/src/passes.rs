//! The `WinEvent` thread's passes, numbered as the focus listener will batch
//! them: a pass begins at the first `WinEvent` the thread's message loop
//! delivers while none is open, which posts the thread a pass message
//! carrying the pass's number, and ends when that message is retrieved. A
//! pass message naming a pass already ended is ignored. Pure, so that the
//! rule is tested without a message loop.

/// The passes so far: the last number given, and the open one, if any.
#[derive(Debug, Default)]
pub(crate) struct Passes {
    /// The number of the last pass opened, 0 before the first.
    last: u64,
    /// The pass open now, waiting for its message.
    open: Option<u64>,
}

/// What a `WinEvent`'s delivery means for the passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Delivered {
    /// The pass the event arrived in.
    pub pass: u64,
    /// Whether this event opened the pass, so that its message must be
    /// posted.
    pub opened: bool,
}

impl Passes {
    /// A `WinEvent` was delivered: the open pass, or a new one it opens.
    pub(crate) fn event(&mut self) -> Delivered {
        if let Some(pass) = self.open {
            return Delivered {
                pass,
                opened: false,
            };
        }
        self.last += 1;
        self.open = Some(self.last);
        Delivered {
            pass: self.last,
            opened: true,
        }
    }

    /// The pass message numbered `pass` was retrieved: it ends that pass
    /// when it is the open one, and is ignored otherwise.
    pub(crate) fn message(&mut self, pass: u64) {
        if self.open == Some(pass) {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn delivered(pass: u64, opened: bool) -> Delivered {
        Delivered { pass, opened }
    }

    #[test]
    fn the_first_event_opens_pass_one_and_asks_for_its_message() {
        let mut passes = Passes::default();
        assert_eq!(passes.event(), delivered(1, true));
    }

    #[test]
    fn events_before_the_pass_message_join_the_open_pass() {
        let mut passes = Passes::default();
        assert_eq!(passes.event(), delivered(1, true));
        assert_eq!(passes.event(), delivered(1, false));
        assert_eq!(passes.event(), delivered(1, false));
    }

    #[test]
    fn the_open_pass_message_ends_the_pass_and_the_next_event_opens_another() {
        let mut passes = Passes::default();
        assert_eq!(passes.event(), delivered(1, true));
        passes.message(1);
        assert_eq!(passes.event(), delivered(2, true));
        assert_eq!(passes.event(), delivered(2, false));
    }

    #[test]
    fn a_message_for_an_ended_pass_leaves_the_open_pass_open() {
        let mut passes = Passes::default();
        assert_eq!(passes.event(), delivered(1, true));
        passes.message(1);
        assert_eq!(passes.event(), delivered(2, true));
        passes.message(1);
        assert_eq!(passes.event(), delivered(2, false));
    }

    #[test]
    fn a_message_with_no_pass_open_opens_nothing() {
        let mut passes = Passes::default();
        passes.message(1);
        assert_eq!(passes.event(), delivered(1, true));
        passes.message(1);
        passes.message(1);
        assert_eq!(passes.event(), delivered(2, true));
    }
}
