//! The work in flight: a reader's record of the one slow-call item a
//! worker is running (`books/dev/src/reference/crates/verbatim-reader-rules.md`). A worker is an outpost's worker
//! thread, or one of the focus listener's reading threads; each reader
//! embeds its own [`InFlight`], the outpost one and the listener one per
//! application, so every fact here has one owner.
//!
//! It holds which worker generation is in charge, the item's deadline,
//! its judgement, asked at [`MOVED_ON_GRACE`] after the item's start and
//! every [`MOVED_ON_GRACE`] after that until the deadline, as NVDA's
//! watchdog judges (`docs/nvda/main-loop-and-watchdog.md`): whether the
//! user has moved on from the item's window; and its abandonment, at its
//! deadline or once the user has moved on, the only two. A window found
//! hung abandons nothing: the reader waits for its application.
//!
//! Each transition answers with a [`Decision`]: nothing, one [`Ask`] that
//! the reader's shell answers by the matching transition, or an
//! abandonment, which carries the item out. The item leaves exactly once,
//! by [`InFlight::end`] or inside [`Abandoned`]. Times are microseconds on
//! the reader's own monotonic clock, passed in; nothing here reads a
//! clock or makes a call.

use crate::{MOVED_ON_GRACE, Window};

/// The moved-on grace, in microseconds, saturating at `u64::MAX`.
fn grace_us() -> u64 {
    u64::try_from(MOVED_ON_GRACE.as_micros()).unwrap_or(u64::MAX)
}

/// One request of one worker generation: what a worker's results name, so
/// that a dead generation's and an earlier request's are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Request {
    /// The worker generation the request belongs to.
    pub generation: u64,
    /// The request's number within its generation.
    pub number: u64,
}

/// The judgement of the item in flight: whether the user has moved on from
/// its window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    /// The item has no window: it is never judged.
    Never,
    /// The item's window is still to be learned.
    Unknown,
    /// The item's window is known; it is next judged at `next`.
    Known {
        /// The window.
        window: Window,
        /// When it is next judged.
        next: u64,
    },
    /// The item's worker names the window it calls into, which the reader's
    /// shell reads at each judgement; it is next judged at `next`.
    Named {
        /// When it is next judged.
        next: u64,
    },
}

/// The window an item is started with, which it is judged by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleWindow {
    /// The item is never judged.
    Never,
    /// Its window is still to be learned.
    Unknown,
    /// Its window.
    Known(Window),
    /// Its worker names the window it calls into, on the grace's schedule
    /// from its start.
    Named,
}

impl RuleWindow {
    /// The window a cache gives, `None` for an item never judged by the
    /// rule and a handle of 0 while the window is not known.
    #[must_use]
    pub const fn from_cached(handle: Option<isize>) -> Self {
        match handle {
            None => Self::Never,
            Some(handle) => match Window::new(handle) {
                Some(window) => Self::Known(window),
                None => Self::Unknown,
            },
        }
    }

    /// The window, when it is known.
    #[must_use]
    pub const fn known(self) -> Option<Window> {
        match self {
            Self::Known(window) => Some(window),
            Self::Never | Self::Unknown | Self::Named => None,
        }
    }
}

/// Why the item in flight was abandoned, which each reader puts in its own
/// words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The item ran past its deadline.
    Deadline,
    /// The user moved on from the item's window to another thread of its
    /// application.
    MovedOn,
}

/// What the record asks of its reader's shell, answered by the matching
/// transition before anything else is judged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Judge `request`'s item by its window: whether the user has left it
    /// for another thread of its application; answered by
    /// [`InFlight::judged`].
    Judge {
        /// The item asked about.
        request: Request,
        /// Its window; `None` for one its worker names, which the shell
        /// reads, the item staying while none is named.
        window: Option<Window>,
    },
}

/// An abandonment: the generation in charge died, the next took charge,
/// and the item left.
#[derive(Debug, PartialEq, Eq)]
pub struct Abandoned<T> {
    /// The generation that died.
    pub dead: u64,
    /// The generation now in charge.
    pub replacement: u64,
    /// Why.
    pub reason: Reason,
    /// The item.
    pub item: T,
}

/// What a transition decides.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision<T> {
    /// Nothing to do.
    Nothing,
    /// One ask, for the reader's shell to answer.
    Ask(Ask),
    /// The item was abandoned.
    Abandon(Abandoned<T>),
}

/// The item a worker is running.
#[derive(Clone, Debug)]
struct Running<T> {
    request: Request,
    started: u64,
    deadline: u64,
    moved: Moved,
    item: T,
}

/// A reader's work in flight ([module docs](self)).
#[derive(Clone, Debug)]
pub struct InFlight<T> {
    generation: u64,
    requests: u64,
    running: Option<Running<T>>,
}

impl<T> Default for InFlight<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            requests: 0,
            running: None,
        }
    }
}

impl<T> InFlight<T> {
    /// A record whose generation 0 is in charge, with nothing running.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts `item` at `now`, to be abandoned `deadline` microseconds
    /// later, with the window the moved-on rule judges it by. Nothing may
    /// be running.
    pub fn start(&mut self, item: T, deadline: u64, window: RuleWindow, now: u64) -> Request {
        debug_assert!(self.running.is_none(), "an item started over another");
        let request = self.next_request();
        let next = now.saturating_add(grace_us());
        let moved = match window {
            RuleWindow::Never => Moved::Never,
            RuleWindow::Unknown => Moved::Unknown,
            RuleWindow::Known(window) => Moved::Known { window, next },
            RuleWindow::Named => Moved::Named { next },
        };
        self.running = Some(Running {
            request,
            started: now,
            deadline: now.saturating_add(deadline),
            moved,
            item,
        });
        request
    }

    /// A new request for the item running, keeping its times and its
    /// moved-on rule; `None` with nothing running.
    pub fn hand_on(&mut self) -> Option<Request> {
        let request = Request {
            generation: self.generation,
            number: self.requests.saturating_add(1),
        };
        let running = self.running.as_mut()?;
        self.requests = self.requests.saturating_add(1);
        running.request = request;
        Some(request)
    }

    /// `request`'s item has learned its window, `window`, at `now`: an
    /// unknown window becomes known, its rule judged at the grace's end, or
    /// at once when the grace has passed.
    pub fn learned(&mut self, request: Request, window: Window, now: u64) -> Decision<T> {
        let Some(running) = self.running_of(request) else {
            return Decision::Nothing;
        };
        if running.moved != Moved::Unknown {
            return Decision::Nothing;
        }
        let grace_end = running.started.saturating_add(grace_us());
        running.moved = Moved::Known {
            window,
            next: grace_end.max(now),
        };
        if now >= grace_end {
            self.judge(now)
        } else {
            Decision::Nothing
        }
    }

    /// The reader's time has come at `now`: what is due is judged
    /// ([`judge`](Self::judge)).
    pub fn tick(&mut self, now: u64) -> Decision<T> {
        self.judge(now)
    }

    /// The answer to [`Ask::Judge`] for `request`: `None` when the item
    /// stays, else why it is abandoned, the user having moved on.
    pub fn judged(&mut self, request: Request, verdict: Option<Reason>) -> Decision<T> {
        if self.running_of(request).is_none() {
            return Decision::Nothing;
        }
        match verdict {
            Some(reason) => self.abandon(reason),
            None => Decision::Nothing,
        }
    }

    /// The item of `request`, once, if `request` is running.
    pub fn end(&mut self, request: Request) -> Option<T> {
        let running = self.running.take_if(|running| running.request == request)?;
        Some(running.item)
    }

    /// A new generation in charge without an abandonment, for a reader
    /// that starts a new worker: whatever the last one sends is refused.
    /// Nothing may be running.
    pub fn succeed(&mut self) -> u64 {
        debug_assert!(
            self.running.is_none(),
            "a generation succeeded over an item"
        );
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    /// Whether a result of `request` is applied: its item is running.
    #[must_use]
    pub fn accepts(&self, request: Request) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.request == request)
    }

    /// The generation in charge.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The item running, by its request.
    #[must_use]
    pub fn running(&self) -> Option<(Request, &T)> {
        self.running
            .as_ref()
            .map(|running| (running.request, &running.item))
    }

    /// The deadline of the item running.
    #[must_use]
    pub fn deadline(&self) -> Option<u64> {
        self.running.as_ref().map(|running| running.deadline)
    }

    /// The item running, to change what the reader keeps in it.
    pub fn item_mut(&mut self) -> Option<&mut T> {
        self.running.as_mut().map(|running| &mut running.item)
    }

    /// When something is next due: the sooner of the deadline and the
    /// next judgement while the window is known or named.
    #[must_use]
    pub fn next_wake(&self) -> Option<u64> {
        let running = self.running.as_ref()?;
        let mut due = running.deadline;
        if let Moved::Known { next, .. } | Moved::Named { next } = running.moved {
            due = due.min(next);
        }
        Some(due)
    }

    /// The judgments of the item at `now`, in this order, the first that
    /// applies deciding, so a transition makes at most one ask: its
    /// deadline, which abandons it, and the judgement of its window, once
    /// due.
    fn judge(&mut self, now: u64) -> Decision<T> {
        let Some(running) = self.running.as_mut() else {
            return Decision::Nothing;
        };
        let request = running.request;
        if now >= running.deadline {
            return self.abandon(Reason::Deadline);
        }
        let window = match &mut running.moved {
            Moved::Known { window, next } if now >= *next => {
                *next = now.saturating_add(grace_us());
                Some(*window)
            }
            Moved::Named { next } if now >= *next => {
                *next = now.saturating_add(grace_us());
                None
            }
            Moved::Never | Moved::Unknown | Moved::Known { .. } | Moved::Named { .. } => {
                return Decision::Nothing;
            }
        };
        Decision::Ask(Ask::Judge { request, window })
    }

    /// Abandons the item running for `reason`: it leaves, and the next
    /// generation takes charge. Called only with an item running: with
    /// none, a logic bug, a debug build stops and a release one decides
    /// nothing.
    fn abandon(&mut self, reason: Reason) -> Decision<T> {
        let Some(running) = self.running.take() else {
            debug_assert!(false, "an abandonment for {reason:?} with no item running");
            return Decision::Nothing;
        };
        let dead = self.generation;
        self.generation = self.generation.saturating_add(1);
        Decision::Abandon(Abandoned {
            dead,
            replacement: self.generation,
            reason,
            item: running.item,
        })
    }

    /// The item running, if it is `request`'s.
    fn running_of(&mut self, request: Request) -> Option<&mut Running<T>> {
        self.running
            .as_mut()
            .filter(|running| running.request == request)
    }

    /// A new request of the generation in charge, for work the record
    /// does not keep, which nothing is judged of and whose results are
    /// refused: a reader's last drop as it stops.
    pub fn next_request(&mut self) -> Request {
        self.requests = self.requests.saturating_add(1);
        Request {
            generation: self.generation,
            number: self.requests,
        }
    }
}

#[cfg(test)]
mod tests;
