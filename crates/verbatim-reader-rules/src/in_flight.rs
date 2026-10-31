//! The work in flight: a reader's record of the one slow-call item a
//! worker is running (`docs/design/logic-foundations.md`, "The work in
//! flight, shared by both readers"). A worker is an outpost's worker
//! thread, or one of the focus listener's reading threads; each reader
//! embeds its own [`InFlight`], the outpost one and the listener one per
//! application, so every fact here has one owner.
//!
//! It holds which worker generation is in charge, the item's deadline,
//! the moved-on rule, judged at [`MOVED_ON_GRACE`] after the item's start
//! and every [`MOVED_ON_GRACE`] after that until the deadline, as NVDA's
//! watchdog judges (`docs/nvda/main-loop-and-watchdog.md`), the
//! publication of the item's walk at its bound, and its abandonment.
//!
//! Each transition answers with a [`Decision`]: nothing, one [`Ask`] that
//! the reader's shell answers by the matching transition, or an
//! abandonment, which carries the item out. The item leaves exactly once,
//! by [`InFlight::end`] or inside [`Abandoned`]. Times are microseconds on
//! the reader's own monotonic clock, passed in; nothing here reads a
//! clock or makes a call.

use crate::MOVED_ON_GRACE;

/// The moved-on grace, in microseconds.
const GRACE_US: u64 = {
    let micros = MOVED_ON_GRACE.as_micros();
    assert!(micros <= u64::MAX as u128);
    #[expect(clippy::cast_possible_truncation, reason = "checked just above")]
    {
        micros as u64
    }
};

/// One request of one worker generation: what a worker's results name, so
/// that a dead generation's and an earlier request's are refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Request {
    /// The worker generation the request belongs to.
    pub generation: u64,
    /// The request's number within its generation.
    pub number: u64,
}

/// The walk of the item in flight, as far as its publication at its bound
/// goes. One state, so that a walk armed after its publication cannot be
/// represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walk {
    /// No bound is timed: the walk has not armed, has returned, or its
    /// publication is being asked for.
    Unarmed,
    /// The walk passed its checks, and is published once its bound, the
    /// time given, has passed.
    Armed(u64),
    /// The walk was published at its bound: nothing more of the item's is
    /// applied, and it is never armed again.
    Published,
}

/// The moved-on rule of the item in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    /// The item has no window: it is never judged so.
    Never,
    /// The item's window is still to be learned.
    Unknown,
    /// The item's window is known; the rule is next judged at `next`.
    Known {
        /// The window.
        window: isize,
        /// When the rule is next judged.
        next: u64,
    },
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
    /// Whether the user has left `window`, the window of `request`'s item,
    /// for another thread of its application; answered by
    /// [`InFlight::moved_on`].
    MovedOn {
        /// The item asked about.
        request: Request,
        /// Its window.
        window: isize,
    },
    /// Publish the walk of `request`'s item from its progress; answered by
    /// [`InFlight::published`], which carries `then_abandon` back.
    Publish {
        /// The item whose walk is published.
        request: Request,
        /// Why the item is abandoned once the publication is applied, when
        /// an abandonment asked for it.
        then_abandon: Option<Reason>,
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
    /// Whether its walk was published at its bound.
    pub published: bool,
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
    walk: Walk,
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
    /// later, with its window: `None` for an item never judged by the
    /// moved-on rule, `Some(0)` for one whose window is still to be
    /// learned, and `Some(window)` otherwise. Nothing may be running.
    pub fn start(&mut self, item: T, deadline: u64, window: Option<isize>, now: u64) -> Request {
        debug_assert!(self.running.is_none(), "an item started over another");
        let request = self.next_request();
        let moved = match window {
            None => Moved::Never,
            Some(0) => Moved::Unknown,
            Some(window) => Moved::Known {
                window,
                next: now.saturating_add(GRACE_US),
            },
        };
        self.running = Some(Running {
            request,
            started: now,
            deadline: now.saturating_add(deadline),
            walk: Walk::Unarmed,
            moved,
            item,
        });
        request
    }

    /// A new request for the item running, keeping its times, its walk and
    /// its moved-on rule; `None` with nothing running.
    pub fn hand_on(&mut self) -> Option<Request> {
        let request = Request {
            generation: self.generation,
            number: self.requests + 1,
        };
        let running = self.running.as_mut()?;
        self.requests += 1;
        running.request = request;
        Some(request)
    }

    /// `request`'s item has learned its window, `window`, at `now`: an
    /// unknown window becomes known, its rule judged at the grace's end, or
    /// at once when the grace has passed.
    pub fn learned(&mut self, request: Request, window: isize, now: u64) -> Decision<T> {
        let Some(running) = self.running_of(request) else {
            return Decision::Nothing;
        };
        if window == 0 || running.moved != Moved::Unknown {
            return Decision::Nothing;
        }
        let grace_end = running.started.saturating_add(GRACE_US);
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

    /// `request`'s walk passed its checks, its bound at `bound`; a walk
    /// published is never armed again.
    pub fn armed(&mut self, request: Request, bound: u64) {
        if let Some(running) = self.running_of(request)
            && running.walk != Walk::Published
        {
            running.walk = Walk::Armed(bound);
        }
    }

    /// `request`'s walk returned, `past` its bound or not: an armed walk
    /// past its bound is published; otherwise no bound is timed for it any
    /// more.
    pub fn walk_returned(&mut self, request: Request, past: bool) -> Decision<T> {
        let Some(running) = self.running_of(request) else {
            return Decision::Nothing;
        };
        if !matches!(running.walk, Walk::Armed(_)) {
            return Decision::Nothing;
        }
        running.walk = Walk::Unarmed;
        if past {
            Decision::Ask(Ask::Publish {
                request,
                then_abandon: None,
            })
        } else {
            Decision::Nothing
        }
    }

    /// The reader's time has come at `now`: what is due is judged
    /// ([`judge`](Self::judge)).
    pub fn tick(&mut self, now: u64) -> Decision<T> {
        self.judge(now)
    }

    /// The answer to [`Ask::MovedOn`] for `request`: when the user has
    /// moved on, an armed walk is published first and the item abandoned
    /// then, and any other is abandoned at once, published or not.
    pub fn moved_on(&mut self, request: Request, moved_on: bool) -> Decision<T> {
        let Some(running) = self.running_of(request) else {
            return Decision::Nothing;
        };
        if !moved_on {
            return Decision::Nothing;
        }
        if matches!(running.walk, Walk::Armed(_)) {
            running.walk = Walk::Unarmed;
            return Decision::Ask(Ask::Publish {
                request,
                then_abandon: Some(Reason::MovedOn),
            });
        }
        Decision::Abandon(self.abandon(Reason::MovedOn))
    }

    /// The answer to [`Ask::Publish`] for `request`, with `got` when the
    /// walk was published, which the reader applies before this: the walk
    /// is then published, and the abandonment the ask carried is made, or
    /// with none, what is due at `now` is judged again. An answer for an
    /// item no longer running is dropped, its abandonment with it.
    pub fn published(
        &mut self,
        request: Request,
        got: bool,
        then_abandon: Option<Reason>,
        now: u64,
    ) -> Decision<T> {
        let Some(running) = self.running_of(request) else {
            return Decision::Nothing;
        };
        if got {
            running.walk = Walk::Published;
        }
        match then_abandon {
            Some(reason) => Decision::Abandon(self.abandon(reason)),
            None => self.judge(now),
        }
    }

    /// The item of `request`, once, with whether its walk was published, if
    /// `request` is running.
    pub fn end(&mut self, request: Request) -> Option<(T, bool)> {
        let running = self.running.take_if(|running| running.request == request)?;
        Some((running.item, running.walk == Walk::Published))
    }

    /// A new generation in charge without an abandonment, for a reader
    /// that starts a new worker: whatever the last one sends is refused.
    /// Nothing may be running.
    pub fn succeed(&mut self) -> u64 {
        debug_assert!(
            self.running.is_none(),
            "a generation succeeded over an item"
        );
        self.generation += 1;
        self.generation
    }

    /// Whether a result of `request` is applied: its item is running and
    /// its walk was not published.
    #[must_use]
    pub fn accepts(&self, request: Request) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.request == request && running.walk != Walk::Published)
    }

    /// The generation in charge.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The item running, by its request, with whether its walk was
    /// published.
    #[must_use]
    pub fn running(&self) -> Option<(Request, &T, bool)> {
        self.running.as_ref().map(|running| {
            (
                running.request,
                &running.item,
                running.walk == Walk::Published,
            )
        })
    }

    /// The item running, to change what the reader keeps in it.
    pub fn item_mut(&mut self) -> Option<&mut T> {
        self.running.as_mut().map(|running| &mut running.item)
    }

    /// When something is next due: the soonest of the deadline, the bound
    /// while the walk is armed, and the next judgment of the moved-on rule
    /// while the window is known.
    #[must_use]
    pub fn next_wake(&self) -> Option<u64> {
        let running = self.running.as_ref()?;
        let mut due = running.deadline;
        if let Walk::Armed(bound) = running.walk {
            due = due.min(bound);
        }
        if let Moved::Known { next, .. } = running.moved {
            due = due.min(next);
        }
        Some(due)
    }

    /// The judgments of the item at `now`, in this order, the first that
    /// applies deciding, so a transition makes at most one ask: its
    /// deadline, which abandons it, publishing an armed walk first; its
    /// walk's bound, which publishes the walk; and the moved-on rule, once
    /// its next judgment is due, published or not. Judged the other way
    /// round, a late tick past both the bound and the deadline would
    /// abandon before the publication, losing what was read.
    fn judge(&mut self, now: u64) -> Decision<T> {
        let Some(running) = self.running.as_mut() else {
            return Decision::Nothing;
        };
        let request = running.request;
        if now >= running.deadline {
            if matches!(running.walk, Walk::Armed(_)) {
                running.walk = Walk::Unarmed;
                return Decision::Ask(Ask::Publish {
                    request,
                    then_abandon: Some(Reason::Deadline),
                });
            }
            return Decision::Abandon(self.abandon(Reason::Deadline));
        }
        if let Walk::Armed(bound) = running.walk
            && now >= bound
        {
            running.walk = Walk::Unarmed;
            return Decision::Ask(Ask::Publish {
                request,
                then_abandon: None,
            });
        }
        if let Moved::Known { window, next } = &mut running.moved
            && now >= *next
        {
            *next = now.saturating_add(GRACE_US);
            return Decision::Ask(Ask::MovedOn {
                request,
                window: *window,
            });
        }
        Decision::Nothing
    }

    /// Abandons the item running for `reason`: it leaves, and the next
    /// generation takes charge.
    ///
    /// # Panics
    ///
    /// Never: called only with an item running.
    fn abandon(&mut self, reason: Reason) -> Abandoned<T> {
        let running = self
            .running
            .take()
            .expect("an abandonment of the item running");
        let dead = self.generation;
        self.generation += 1;
        Abandoned {
            dead,
            replacement: self.generation,
            reason,
            item: running.item,
            published: running.walk == Walk::Published,
        }
    }

    /// The item running, if it is `request`'s.
    fn running_of(&mut self, request: Request) -> Option<&mut Running<T>> {
        self.running
            .as_mut()
            .filter(|running| running.request == request)
    }

    /// A new request of the generation in charge.
    fn next_request(&mut self) -> Request {
        self.requests += 1;
        Request {
            generation: self.generation,
            number: self.requests,
        }
    }
}

#[cfg(test)]
mod tests;
