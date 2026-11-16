//! The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine that holds every
//! remembered fact of the outpost's application, with no Windows type and
//! no application object. Inputs come in mailbox order; effects go out.
//! The outpost's shell (`verbatim-outpost`'s `outpost::shell` module) runs
//! it on a thread of its own, applies its effects, and keeps its time. It is
//! a logic crate: in the GPL tier, with no Windows dependency, so it builds
//! and is tested on any host.
//!
//! It holds the intake's order ([`intake`]), the follow state ([`follow`]),
//! the arbitrator's verdicts
//! ([`Verdicts`]), Core's requests ([`waiting`]), the program in flight
//! with the worker generation in charge ([`charge`]), the queries not yet
//! answered, and the idle barrier's round.
//! Programs, run by a worker on copies ([`Facts`]), make every call into
//! the application; their commits come back as inputs, and the reducer
//! applies a commit only while its request is live: the program in flight
//! of the generation in charge.
//!
//! A program runs until it ends, with no deadline: Core judges a hang and
//! bounds its requests by their due times, and a worker is abandoned only
//! when Core cancels the request it runs because the foreground moved away
//! (`docs/design/readers-tell-core-asks-build1.md`, Table 14).
//!
//! What it never does: make a call of any kind, wait, read a clock, or
//! keep an object. Time is the shell's: a [`Now`] given with each input,
//! and [`Reducer::next_timer`] for the shell's timed wait.

// Non-test code never panics, overflows or truncates silently: arithmetic
// is checked or saturating, indexing goes through `get`, and casts through
// `From` or `TryFrom`. Tests are exempt, since a panic is how a test fails.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::exit,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    clippy::unwrap_in_result,
    clippy::unwrap_used
)]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::as_conversions,
        clippy::exit,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::panic_in_result_fn,
        clippy::string_slice,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::unwrap_in_result,
        clippy::unwrap_used
    )
)]

mod charge;
mod entry;
mod follow;
mod intake;
#[cfg(test)]
mod tests;
mod text;
mod waiting;
mod watch;

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use self::charge::{Abandoned, InCharge};
use self::watch::WatchCancelled;
use verbatim_model::protocol::{
    EventTiming, FrontWindow, OutpostToCore, Query, QueryOutcome, QueryResult, RequestId,
};
use verbatim_model::{
    CallCounts, FocusText, KeyFetch, NodeId, NormalizedEvent, ReadPurpose, TextOp, TraceId,
};

pub use self::charge::Request;
pub use self::entry::{Asker, Entry, Item, UiaEvent, UiaKind, WinKind, is_state_property};
pub use self::follow::{
    Applied, BarWatch, Change, DialogMemo, Facts, FocusCommit, FocusPath, FollowState, Followed,
    Group, LastReported, LogicError, MarkChange, MsaaReport, Origin, Owed, OwedProgram, OwedWork,
    Route, Subscribed, Subscription, Target, apply_local,
};
pub use self::intake::{Handling, Marks, Object, TerminalMark};
pub use self::text::{
    Anchor, AnswerCut, CutLine, FieldReads, KeptSelection, KeyNote, LastRead, NOTE_LIFE,
    NodeAnchors, NoteFetch, NoteOn, NoteReads, POLL_BOUND, POLL_INTERVAL, RangeKey, ReadSelection,
    ReportCut, TerminalReads, TextChange, TextFacts,
};
pub use self::waiting::{Asks, Cancelled};
pub use self::watch::{EVENT_WINDOW_US, WORD_AFTER_US, WatchProgram, WatchWork, Watches};
pub use verbatim_reader_rules::verdicts::{Probe, Probed, VerdictChange, Verdicts};

/// The time an input comes at: the shell's monotonic clock, from the
/// outpost's start, and the observation clock events are stamped with, in
/// microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Now {
    /// The shell's monotonic clock, from the outpost's start.
    pub at: Duration,
    /// The observation clock events are stamped with, in microseconds.
    pub observed_us: u64,
    /// The run's shared clock, in microseconds, which messages are stamped
    /// as published by.
    pub wall_us: u64,
}

/// What a program is.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the intake to its program, as it always has"
)]
pub enum Work {
    /// One entry, handled as `handling` says.
    Entry {
        /// The entry.
        entry: Entry,
        /// How the program handles it.
        handling: Handling,
    },
    /// The work the followed focus owes.
    Owed(OwedProgram),

    /// Drops the objects the shell hands it: what a release let go of.
    Drop,
    /// One read of the console poll, of this node under its note.
    NoteRead(NodeId),
    /// Finds the element of the foreground window for its progress bars
    /// ([`BarWatch`]).
    FindBar(isize),
    /// Core's request, answered once ([`waiting`]).
    Ask(Box<verbatim_model::request::Ask>),
    /// A caret key's watch's check or final read ([`watch`]), answered
    /// with [`Input::Answered`] when it ends the watch.
    Watch(Box<WatchProgram>),
    /// Finishes the caret read `cut`, cut at its value change check, with
    /// the reducer's decision: whether a value change of its edit control
    /// waited when the read part ended (`waiting`).
    Finish {
        /// The cut read.
        cut: Box<Cut>,
        /// Whether a value change of its edit control waited when the read part ended.
        waiting: bool,
    },
}

/// A caret read of an edit control's client area cut at its value change
/// check (`books/dev/src/explanation/design/ownership.md`): the read part read the caret and synced with the `WinEvent`
/// thread, so every value change the control raised before the sync waits
/// ahead of the read part's end in the mailbox. The part that finishes it
/// makes the comparison and the rest of the read's calls.
#[derive(Clone, Debug)]
pub struct Cut {
    /// The edit control's window, whose client area's value change the
    /// reducer looks for.
    pub edit: isize,
    /// The edit control's client area, and its text's length if the read
    /// part read it, so the finish reads it no more than the read uncut.
    pub node: NodeId,
    /// The text's length, when the read part read it.
    pub length: Option<u32>,
    /// When the read began, on the observation clock, committed with what
    /// the finish sends.
    pub read_at: Option<u64>,
    /// The timing of the entry the read part handled, and the calls it
    /// made that no message has carried, which the finish's message
    /// carries with its own.
    pub timing: EventTiming,
    /// The calls the read part made that no message has carried.
    pub calls: CallCounts,
    /// What finishes it.
    pub finish: Finish,
}

/// What a cut caret read was for.
#[derive(Clone, Debug)]
pub enum Finish {
    /// A caret event's report, of the event with this trace and
    /// observation.
    Report {
        /// The event's trace.
        trace: TraceId,
        /// The event's observation, in microseconds on the run's shared clock.
        observed_at_us: u64,
        /// What the read part found.
        read: ReportCut<u32>,
    },
    /// A held key's before read, which answers its request.
    Before {
        /// The request's query id.
        request_id: RequestId,
        /// The query's trace.
        trace: TraceId,
        /// What the read part found.
        read: ReportCut<u32>,
    },
    /// A key's read under its note.
    Note {
        /// The event's trace.
        trace: TraceId,
        /// The event's observation, in microseconds on the run's shared clock.
        observed_at_us: u64,
        /// The end of the note read under.
        until_us: u64,
        /// What the read part found.
        read: AnswerCut<u32>,
    },
}

/// A program for the worker in charge: its request, its copies, and its
/// work.
#[derive(Clone, Debug)]
pub struct Program {
    /// The request it runs as.
    pub request: Request,
    /// Its copies of the reducer's facts.
    pub facts: Facts,
    /// What it does.
    pub work: Work,
}

/// The rule of whose probe an outpost's reducer records from a worker
/// generation no longer in charge: a probe that timed out, and a destroyed
/// window's forgetting, so a window is not probed again more often than
/// NVDA probes it (`books/dev/src/reference/adr/0001-focus.md`, "One judge
/// per fact"). The outposts' own, so it is not on the shared type.
pub trait KeptFromAnyGeneration {
    /// Whether the reducer records it from a generation no longer in
    /// charge.
    fn kept_from_any_generation(&self) -> bool;
}

impl KeptFromAnyGeneration for VerdictChange {
    fn kept_from_any_generation(&self) -> bool {
        matches!(
            self,
            Self::Forget(_)
                | Self::Probed(Probe {
                    probed: Probed::TimedOut(_),
                    ..
                })
        )
    }
}

/// What the reducer asks of the shell, in order.
#[derive(Debug)]
pub enum Effect {
    /// Hand `Program` to the worker in charge.
    Dispatch(Box<Program>),
    /// Send a message to Core, its nodes recorded as carried. A reply
    /// answers its query's token.
    Send(Box<OutpostToCore>),
    /// Release the nodes and anchors no live request can still name, for
    /// Core's held lists, which acknowledge `acknowledged`; the shell
    /// answers with [`Input::Released`].
    Release {
        /// The nodes to release, by number.
        nodes: Vec<u64>,
        /// The text anchors to release, by number.
        anchors: Vec<u64>,
        /// The position of the last message Core's held lists acknowledge.
        acknowledged: u64,
        /// The requests still live, whose nodes and anchors stay.
        live: Vec<Request>,
    },
    /// Move the focus-following subscription.
    Subscribe(Subscription),
    /// Drop the shell's elements under these tokens, on a worker.
    Unfollow(Vec<u64>),
    /// Drop the objects parked under these keys, on a worker.
    Unpark(Vec<u64>),
    /// Let go of the UIA ranges the store keeps under these keys, by node,
    /// which no text fact names any more, for a worker to drop.
    ForgetRanges(Vec<(u64, RangeKey)>),
    /// Mark the generation dead in its charge and start `replacement`: the
    /// shell counts the abandoned workers that have not returned.
    Abandon {
        /// The generation marked dead.
        dead: u64,
        /// The generation started in its place.
        replacement: u64,
    },
    /// The push-time copy of the facts' marks, given after a transition
    /// that changed them, ahead of the transition's other effects so that
    /// no program it dispatches runs before the mailbox judges by them.
    Marks(Marks),
    /// A commit asked for what cannot occur: applied as nothing, for the
    /// shell to record as the logic error it is.
    LogicError(LogicError),
    /// A test's settle, number `0`, was reached: the shell answers whoever
    /// waits on it.
    Settled(u64),
}

/// Inputs, in mailbox order.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the mailbox into the intake, as it always has"
)]
pub enum Input {
    /// An entry from a producer.
    Entry(Entry),
    /// Core's word on where attention is.
    Front(FrontWindow),
    /// Core withdraws query `0` if it has not started.
    Cancel(RequestId),
    /// Core's note of a key with no before read, set as it arrives, not
    /// queued behind what waits; `console` when its node is the console
    /// host's focused text area, which polls.
    KeyNote {
        /// The key's node.
        node: NodeId,
        /// What the node's reads fetch.
        fetch: KeyFetch,
        /// Which node it is.
        on: NoteOn,
    },
    /// A verdict a program of generation `generation` probed, or a window
    /// it found destroyed, posted alone as it was made, even from an
    /// abandoned worker.
    Verdict {
        /// The worker generation of the program that made it.
        generation: u64,
        /// The verdict.
        change: VerdictChange,
    },
    /// A commit of a program's, its store writes made: the shell gives
    /// the reducer only a commit it accepts.
    Commit {
        /// The request the commit is of.
        request: Request,
        /// What the program found, to apply while the request is live.
        change: Box<Change>,
    },

    /// A program ended: whether it panicked, and the caret read it cut at
    /// its value change check, for the reducer to decide and finish.
    Finished {
        /// The request that ended.
        request: Request,
        /// Unread since focus groups went with the event queue; kept until
        /// the caret watch's seam test, which sets it, is next changed.
        reported: bool,
        /// Whether it panicked.
        panicked: bool,
        /// The caret read it cut at its value change check, for the reducer to decide and finish.
        cut: Option<Box<Cut>>,
    },
    /// What a release let go of, nodes and anchors by number, and whether
    /// there are objects to drop.
    Released {
        /// The nodes let go of, by number.
        nodes: BTreeSet<u64>,
        /// The text anchors let go of, by number.
        anchors: BTreeSet<u64>,
        /// Whether there are objects to drop.
        objects: bool,
    },
    /// Core's request, queued for the worker ([`waiting`]).
    Ask(Box<verbatim_model::request::Ask>),
    /// Core cancels a request ([`waiting`]).
    CancelAsk(verbatim_model::request::Cancel),
    /// A window was destroyed: Core's requests queued on it are answered
    /// "not there" ([`waiting`]).
    WindowDestroyed(verbatim_model::WindowHandle),
    /// The answer of program `request` to the Core request it runs: sent
    /// while the program is in flight, of the generation in charge.
    Answered {
        /// The program.
        request: Request,
        /// Its answer.
        answer: Box<verbatim_model::request::Answer>,
    },
    /// The outpost is shutting down.
    Close,
}

/// What the reducer keeps of the program in flight, beside what the work
/// in flight keeps of it (its request, times, walk and moved-on rule).
#[derive(Clone, Debug)]
struct Current {
    /// The query the program answers, if it answers one, which an
    /// abandonment or a program that ends without answering answers.
    running: Option<(RequestId, TraceId)>,
    /// The Core request it runs ([`Asks::running`]), if it runs one.
    asked: Option<verbatim_model::request::RequestNumber>,
    /// Whether it does work the idle barrier's answer waits for: a read of
    /// the console poll, which may still read the terminal under the key's
    /// trace.
    deferred: bool,
}

/// The reducer.
#[cfg_attr(test, derive(Clone, Debug))]
#[derive(Default)]
pub struct Reducer {
    /// Core's requests, run ahead of everything else.
    asks: Asks,
    /// The watches waiting, whose checks and final reads run ahead of
    /// Core's other requests.
    watches: Watches,
    intake: intake::Intake,
    facts: Facts,
    /// The program in flight, and the worker generation in charge.
    flight: InCharge<Current>,
    /// A key's final read, to run before anything else.
    after: VecDeque<(Entry, Handling)>,
    /// Whether a release let go of objects for a worker to drop.
    to_drop: bool,
    /// The queries recorded and not yet answered.
    queries: BTreeSet<RequestId>,
    /// The idle barrier's round reached and not yet answered.
    idle_query: Option<u64>,
    /// Whether a release was asked for and its [`Input::Released`] has not
    /// come: nothing is dispatched until it has, so no program is given a
    /// node it frees.
    releasing: bool,
    stopping: bool,
}

impl Reducer {
    /// A reducer whose generation 0 is in charge.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The facts as the reducer holds them.
    #[cfg(test)]
    #[must_use]
    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    /// The window of the focus this outpost follows, if any.
    #[must_use]
    pub fn followed_window(&self) -> Option<isize> {
        self.facts
            .follow
            .followed
            .as_ref()
            .and_then(|followed| followed.window)
    }

    /// The worker generation in charge.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.flight.generation()
    }

    /// The queries recorded and not yet answered, which the shell's tokens
    /// match.
    #[must_use]
    pub fn queries(&self) -> &BTreeSet<RequestId> {
        &self.queries
    }

    /// The tests' settles waiting in the intake, by number, which the
    /// shell's waiters match: a settle is answered as it is reached, so
    /// these are the settles not yet answered.
    #[must_use]
    pub fn settles(&self) -> BTreeSet<u64> {
        self.intake.settles()
    }

    /// Whether a program is in flight.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.flight.running().is_some()
    }

    /// Whether a commit of `request` is applied: it is the program in
    /// flight's, of the generation in charge.
    #[must_use]
    pub fn accepts(&self, request: Request) -> bool {
        self.flight.accepts(request)
    }

    /// When the shell is next to call [`tick`](Self::tick): the console
    /// poll's due time while no program is in flight, no release is pending
    /// and the outpost is not stopping (only then is it acted on), on the
    /// shell's clock.
    #[must_use]
    pub fn next_timer(&self, now: Now) -> Option<Duration> {
        let mut times = Vec::new();
        if !self.busy()
            && !self.releasing
            && !self.stopping
            && let Some(due_us) = self.facts.text.poll_due()
        {
            let left = due_us.saturating_sub(now.wall_us);
            // A time past the clock's range never comes.
            times.extend(now.at.checked_add(Duration::from_micros(left)));
        }
        if !self.busy()
            && !self.releasing
            && !self.stopping
            && let Some(due_us) = self.watches.next_due()
        {
            let left = due_us.saturating_sub(now.wall_us);
            times.extend(now.at.checked_add(Duration::from_micros(left)));
        }
        times.into_iter().min()
    }

    /// Takes `input` at `now`.
    pub fn input(&mut self, input: Input, now: Now) -> Vec<Effect> {
        let before = self.observed();
        let mut effects = Vec::new();
        match input {
            Input::Entry(entry) => {
                self.entry(entry, now, &mut effects);
            }
            Input::Front(front) => {
                let mut applied = Applied::default();
                self.facts.follow.front(front, &mut applied);
                Self::subscription_effects(applied, &mut effects);
            }
            Input::Cancel(request_id) => {
                if let Some(trace) = self.intake.cancel(request_id) {
                    self.send(
                        reply(request_id, trace, QueryOutcome::NotStarted),
                        &mut effects,
                    );
                }
            }
            Input::KeyNote { node, fetch, on } => {
                Arc::make_mut(&mut self.facts.text).set_note(node.number(), fetch, now.wall_us, on);
            }
            Input::Verdict { generation, change } => {
                self.record_verdict(generation, &change);
            }
            Input::Commit { request, change } => {
                if self.accepts(request) {
                    self.commit(request, &change, now, &mut effects);
                }
            }
            Input::Finished {
                request,
                panicked,
                cut,
                ..
            } => self.finished(request, panicked, cut, &mut effects),
            Input::Released {
                nodes,
                anchors,
                objects,
            } => self.released(&nodes, &anchors, objects, &mut effects),
            Input::Ask(_)
            | Input::CancelAsk(_)
            | Input::Answered { .. }
            | Input::WindowDestroyed(_) => {
                self.asked(input, now, &mut effects);
            }
            Input::Close => {
                self.stopping = true;
                self.asks.close();
                self.watches.close();
                let unparked = self.intake.close();
                if !unparked.is_empty() {
                    effects.push(Effect::Unpark(unparked));
                }
            }
        }
        self.pump(now, &mut effects);
        self.answer_idle(&mut effects);
        self.changed(&before, &mut effects);
        effects
    }

    /// The shell's timed wait has ended at `now`: whatever is due is done.
    pub fn tick(&mut self, now: Now) -> Vec<Effect> {
        let before = self.observed();
        let mut effects = Vec::new();
        self.pump(now, &mut effects);
        self.answer_idle(&mut effects);
        self.changed(&before, &mut effects);
        effects
    }

    /// Core's request, its cancel, or a program's answer to it (Tables 14
    /// and 15).
    fn asked(&mut self, input: Input, now: Now, effects: &mut Vec<Effect>) {
        match input {
            Input::Ask(ask) => {
                if self.stopping {
                    return;
                }
                match &ask.request {
                    verbatim_model::request::Request::Text {
                        node,
                        op: TextOp::Watch(watch),
                    } => {
                        self.watches
                            .arrive((ask.number, ask.window, *node), *watch, now.wall_us);
                        return;
                    }
                    verbatim_model::request::Request::Text {
                        op: TextOp::StartingRead(_),
                        ..
                    } => self.watches.starting_asked(ask.number, now.wall_us),
                    _ => {}
                }
                for replaced in self.asks.push(ask) {
                    effects.push(Effect::Send(Box::new(OutpostToCore::Answer(replaced))));
                }
            }
            Input::WindowDestroyed(window) => {
                let mut gone = self.watches.destroyed(window);
                gone.extend(self.asks.destroyed(window));
                for gone in gone {
                    effects.push(Effect::Send(Box::new(OutpostToCore::Answer(gone))));
                }
            }
            Input::CancelAsk(cancel) => self.cancel_ask(cancel, effects),
            Input::Answered { request, answer } => {
                if !self.accepts(request) {
                    return;
                }
                let answer = if self.watches.runs(request) {
                    self.watches.answered(request, *answer)
                } else {
                    if let Some(point) = started_point(&answer) {
                        self.watches.starting_answered(answer.number, point);
                    }
                    self.asks.answered(*answer)
                };
                if let Some(answer) = answer {
                    effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
                }
            }
            _ => {}
        }
    }

    /// Core's `cancel`: a queued request answered cancelled, the running one
    /// abandoned with its worker when the foreground moved away, or left
    /// alone (Table 14).
    fn cancel_ask(&mut self, cancel: verbatim_model::request::Cancel, effects: &mut Vec<Effect>) {
        match self.watches.cancel(cancel) {
            WatchCancelled::NotHere => {}
            WatchCancelled::Answered(answer) => {
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
                return;
            }
            WatchCancelled::Abandon(answer) => {
                if let Some(abandoned) = self.flight.abandon() {
                    self.abandoned(abandoned, effects);
                }
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
                return;
            }
            WatchCancelled::Nothing => return,
        }
        match self.asks.cancel(cancel) {
            Cancelled::Dropped(answer) => {
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
            }
            Cancelled::Abandon(answer) => {
                if let Some(abandoned) = self.flight.abandon() {
                    self.abandoned(abandoned, effects);
                }
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
            }
            Cancelled::Nothing => {}
        }
    }

    /// What the shell keeps a copy of, before a transition: the facts'
    /// marks, for the mailbox ([`changed`](Self::changed)).
    fn observed(&self) -> Marks {
        self.facts.marks()
    }

    /// Gives the shell what a transition changed of what it keeps a copy
    /// of, from `before`: the marks ahead of every other effect, so that a
    /// program the transition dispatched, which may read what a cleared
    /// mark would drop, starts only once the mailbox judges by them.
    fn changed(&self, marks: &Marks, effects: &mut Vec<Effect>) {
        let now = self.facts.marks();
        if now != *marks {
            effects.insert(0, Effect::Marks(now));
        }
    }

    /// What the reducer keeps of the program in flight, if it is
    /// `request`'s.
    fn current_of(&mut self, request: Request) -> Option<&mut Current> {
        if self
            .flight
            .running()
            .is_some_and(|(running, ..)| running == request)
        {
            self.flight.item_mut()
        } else {
            None
        }
    }

    /// Takes in `entry`, pushed into the intake.
    fn entry(&mut self, entry: Entry, now: Now, effects: &mut Vec<Effect>) {
        // The `Follow` subscription's caret and text changes are of the
        // followed node alone: a watch's event rule reads them.
        if let Item::Uia(UiaEvent {
            kind: UiaKind::TextSelection | UiaKind::TextChanged,
            ..
        }) = &entry.item
        {
            self.watches.heard(now.wall_us);
        }
        if self.key_arrived(&entry, now) {
            return;
        }
        self.push(entry, effects);
    }

    /// A read of the caret for a key, as it arrives: a before read sets
    /// the node's note and is queued; a final read goes ahead of queued
    /// reads, as nothing of it can wait behind them, and the note stays
    /// until it commits, so a read in flight still gets its facts.
    /// Whether the entry was taken here.
    fn key_arrived(&mut self, entry: &Entry, now: Now) -> bool {
        let Item::Query {
            request_id,
            query:
                Query::Text {
                    node_id,
                    op: TextOp::ReadCaret(read),
                },
        } = &entry.item
        else {
            return false;
        };
        if self.stopping {
            return false;
        }
        let on = match read.purpose {
            ReadPurpose::BeforeFieldKey => NoteOn::Field,
            ReadPurpose::BeforeTerminalKey => NoteOn::Terminal,
            ReadPurpose::Final => {
                let note_until = self
                    .facts
                    .text
                    .live_note(node_id.number(), now.wall_us)
                    .map(|note| note.until_us);
                self.queries.insert(*request_id);
                self.after
                    .push_back((entry.clone(), Handling::FinalRead { note_until }));
                return true;
            }
        };
        Arc::make_mut(&mut self.facts.text).set_note(node_id.number(), read.fetch, now.wall_us, on);
        false
    }

    /// Pushes `entry` into the intake at `now`, recording a query.
    fn push(&mut self, entry: Entry, effects: &mut Vec<Effect>) {
        if let Item::Query { request_id, .. } = entry.item
            && !self.stopping
        {
            self.queries.insert(request_id);
        }
        let pushed = self.intake.push(entry);
        if !pushed.unparked.is_empty() {
            effects.push(Effect::Unpark(pushed.unparked));
        }
    }

    /// Sends `message`; a reply answers its query.
    fn send(&mut self, message: OutpostToCore, effects: &mut Vec<Effect>) {
        if let OutpostToCore::Reply { request_id, .. } = &message {
            self.queries.remove(request_id);
            if let Some(current) = self.flight.item_mut().filter(|current| {
                current
                    .running
                    .is_some_and(|(running, _)| running == *request_id)
            }) {
                current.running = None;
            }
        }
        effects.push(Effect::Send(Box::new(message)));
    }

    /// Applies the commit of `request`'s program: its messages sent, then
    /// the rest.
    fn commit(&mut self, request: Request, change: &Change, now: Now, effects: &mut Vec<Effect>) {
        if let Some(mut message) = change.result.clone() {
            self.attach_note_facts(&mut message, now);
            if let Some(current) = self.current_of(request) {
                current.running = None;
            }
            self.send(message, effects);
        }
        if let Some((node_id, trace, observed_at_us, timing)) = change.noted_caret {
            // The event's timing goes with it: its read is skipped only
            // when a caret read since the event covers it.
            self.intake.when_idle(Entry {
                item: Item::NotedCaret { node_id },
                trace,
                observed_at_us,
                timing,
            });
        }
        let mut applied = follow::apply(&mut self.facts, self.flight.generation(), change);
        let ranges = std::mem::take(&mut applied.ranges);
        if !ranges.is_empty() {
            effects.push(Effect::ForgetRanges(ranges));
        }
        effects.extend(
            std::mem::take(&mut applied.logic_errors)
                .into_iter()
                .map(Effect::LogicError),
        );
        Self::subscription_effects(applied, effects);
    }

    /// A caret event's report with no facts whose node has a live note: its
    /// read was dispatched before the note arrived. The comparison needs no
    /// call, so it is attached here, before the commit replaces the last
    /// read, and the read can be its key's evidence.
    fn attach_note_facts(&mut self, message: &mut OutpostToCore, now: Now) {
        let OutpostToCore::Event { event, .. } = message else {
            return;
        };
        let (node, caret, selection_changes, facts) = match event {
            NormalizedEvent::CaretMoved {
                node_id,
                caret,
                selection_changes,
                facts,
                ..
            } => (*node_id, &*caret, &*selection_changes, facts),
            NormalizedEvent::FocusChanged { node, text, .. } => {
                let Some(FocusText::Caret {
                    caret,
                    changes,
                    facts,
                    ..
                }) = text.as_deref_mut()
                else {
                    return;
                };
                (node.id, &*caret, &changes.selection_changes, facts)
            }
            _ => return,
        };
        if facts.is_some()
            || self
                .facts
                .text
                .live_note(node.number(), now.wall_us)
                .is_none()
        {
            return;
        }
        *facts = Arc::make_mut(&mut self.facts.text)
            .facts_at_commit(
                node.number(),
                (caret, !selection_changes.is_empty()),
                now.wall_us,
            )
            .map(Box::new);
    }

    /// Records a verdict of a program of `generation`'s: one from the
    /// generation in charge, and a probe that timed out or a window's
    /// forgetting from any generation, so a window whose probe timed out
    /// is not probed again within the negative verdict's lifetime even
    /// when the worker that probed it was abandoned.
    fn record_verdict(&mut self, generation: u64, change: &VerdictChange) {
        if generation != self.generation() && !change.kept_from_any_generation() {
            return;
        }
        Arc::make_mut(&mut self.facts.verdicts).apply(change);
    }

    /// The shell's part of an [`Applied`]: the subscription moved and the
    /// elements no longer followed dropped.
    fn subscription_effects(applied: Applied, effects: &mut Vec<Effect>) {
        let Applied {
            subscribe,
            unfollowed,
            ..
        } = applied;
        if let Some(subscribe) = subscribe {
            effects.push(Effect::Subscribe(subscribe));
        }
        if !unfollowed.is_empty() {
            effects.push(Effect::Unfollow(unfollowed));
        }
    }

    /// What a release let go of: nodes and anchors by number, and whether
    /// there are objects to drop.
    fn released(
        &mut self,
        nodes: &BTreeSet<u64>,
        anchors: &BTreeSet<u64>,
        objects: bool,
        effects: &mut Vec<Effect>,
    ) {
        self.releasing = false;
        let ranges = Arc::make_mut(&mut self.facts.text).release(nodes, anchors);
        if !ranges.is_empty() {
            effects.push(Effect::ForgetRanges(ranges));
        }
        let mut applied = Applied::default();
        self.facts.follow.release_groups(nodes, &mut applied);
        Self::subscription_effects(applied, effects);
        self.to_drop |= objects;
    }

    /// The worker in charge was abandoned: its replacement is started, and
    /// the program's query, the live re-read its entry answers, the owed
    /// work it attempted are answered.
    fn abandoned(&mut self, abandoned: Abandoned<Current>, effects: &mut Vec<Effect>) {
        let Abandoned {
            dead,
            replacement,
            item: current,
        } = abandoned;
        self.facts.follow.bar_program_ended();
        if let Some(followed) = self.facts.follow.followed.as_mut()
            && followed
                .owed
                .as_ref()
                .is_some_and(|owed| owed.attempting == Some(dead))
        {
            followed.owed = None;
        }
        effects.push(Effect::Abandon { dead, replacement });
        if let Some((request_id, trace)) = current.running {
            self.send(reply(request_id, trace, QueryOutcome::Abandoned), effects);
        }
    }

    /// A program ended, having cut a caret read at its value change check
    /// when `cut` is given.
    fn finished(
        &mut self,
        request: Request,
        panicked: bool,
        cut: Option<Box<Cut>>,
        effects: &mut Vec<Effect>,
    ) {
        if self
            .flight
            .running()
            .is_none_or(|(running, ..)| running != request)
        {
            return;
        }
        if let Some(cut) = cut.filter(|_| !panicked) {
            self.dispatch_finish(cut, effects);
            return;
        }
        if self.watches.runs(request)
            && let Some(answer) = self.watches.ended(request)
        {
            effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
        }
        let Some(current) = self.flight.end(request) else {
            return;
        };
        self.facts.follow.bar_program_ended();
        if current.asked.is_some()
            && let Some(answer) = self.asks.ended()
        {
            effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
        }
        // A query it left unanswered still gets its one reply.
        if let Some((request_id, trace)) = current.running {
            self.send(
                reply(
                    request_id,
                    trace,
                    QueryOutcome::Failed("the outpost failed handling the query".to_owned()),
                ),
                effects,
            );
        }
        // A live re-read its program did not answer: its object was gone,
        // or a rule dropped its report.
    }

    /// Answers the idle barrier's query once no deferred work is pending:
    /// the program in flight is not a read of the console poll. The program
    /// in flight is otherwise not waited for:
    /// the query is reached only with none in flight, so a program in
    /// flight then handles an entry that arrived after the query.
    fn answer_idle(&mut self, effects: &mut Vec<Effect>) {
        if !self.watches.is_empty()
            || self
                .flight
                .running()
                .is_some_and(|(_, current)| current.deferred)
        {
            return;
        }
        if let Some(round) = self.idle_query.take() {
            effects.push(Effect::Send(Box::new(OutpostToCore::IdleAnswer { round })));
        }
    }

    /// The program that drops what the shell still holds as the outpost
    /// shuts down, with no program in flight: releasing an object can call
    /// into the application, so a worker does it, as at runtime. Nothing
    /// follows it, so it is numbered as a program and not kept in flight.
    pub fn shutdown_drop(&mut self) -> Program {
        Program {
            request: self.flight.next_request(),
            facts: Facts::default(),
            work: Work::Drop,
        }
    }

    /// Starts the next program, if none is in flight: owed work first, then
    /// what a program left to run after it, then a release's drop, then
    /// the intake's next entry; entries the reducer handles itself are
    /// handled on the way.
    fn pump(&mut self, now: Now, effects: &mut Vec<Effect>) {
        while !self.busy() && !self.stopping && !self.releasing {
            if let Some(program) = self.watches.due(now.wall_us) {
                let program = Box::new(program);
                let request = self.dispatch(Work::Watch(program.clone()), None, effects);
                self.watches.running(&program, request, now.wall_us);
                continue;
            }
            if let Some(ask) = self.asks.take_next() {
                self.dispatch(Work::Ask(ask), None, effects);
                continue;
            }
            let generation = self.flight.generation();
            if let Some(followed) = self.facts.follow.followed.as_mut()
                && let Some(owed) = followed
                    .owed
                    .as_mut()
                    .filter(|owed| owed.attempting.is_none())
            {
                owed.attempting = Some(generation);
                let owed = owed.clone();
                // Whatever the owed read observes must reach it.
                if owed_clears_absorb(&owed) {
                    self.facts.follow.absorbed = None;
                }
                let node = followed.node;
                self.dispatch(Work::Owed(OwedProgram { node, owed }), None, effects);
                continue;
            }
            if let Some((entry, handling)) = self.after.pop_front() {
                self.dispatch_entry(entry, handling, effects);
                continue;
            }
            if let Some(window) = self.facts.follow.bar.due() {
                self.facts.follow.bar = BarWatch::Finding { window };
                self.dispatch(Work::FindBar(window), None, effects);
                continue;
            }
            if std::mem::take(&mut self.to_drop) {
                self.dispatch(Work::Drop, None, effects);
                continue;
            }
            let Some(entry) = self.intake.next(&self.facts.marks()) else {
                // Nothing else is ready: the console poll's due time is
                // acted on, its read waited for by the idle answer since
                // after it the program reads the terminal.
                let due = (self
                    .facts
                    .text
                    .poll_due()
                    .is_some_and(|due| due <= now.wall_us))
                .then(|| Arc::make_mut(&mut self.facts.text).reach_poll(now.wall_us))
                .flatten();
                let Some(node) = due else { return };
                self.dispatch(Work::NoteRead(NodeId::new(node)), None, effects);
                if let Some(current) = self.flight.item_mut() {
                    current.deferred = true;
                }
                continue;
            };
            self.run(entry, now, effects);
        }
    }

    /// An entry handed out: the reducer's own entries are handled here, and
    /// the rest dispatched.
    fn run(&mut self, entry: Entry, now: Now, effects: &mut Vec<Effect>) {
        match entry.item {
            Item::Barrier {
                asker,
                behind_noted,
            } => {
                // A noted caret is work owed: the barrier goes directly
                // behind it, and waits for nothing more when it is reached
                // again.
                if !behind_noted && let Some(noted) = self.intake.take_idle() {
                    let barrier = Entry {
                        item: Item::Barrier {
                            asker,
                            behind_noted: true,
                        },
                        ..entry
                    };
                    self.intake.push_ahead([noted, barrier]);
                } else {
                    // Answered before anything after it is dispatched: a
                    // test's settle at once, Core's round once no deferred
                    // work is pending.
                    match asker {
                        Asker::Round(round) => {
                            self.idle_query = Some(round);
                            self.answer_idle(effects);
                        }
                        Asker::Settle(id) => effects.push(Effect::Settled(id)),
                    }
                }
            }
            Item::NodesHeld {
                nodes,
                mut anchors,
                acknowledged,
            } => {
                // The anchors a waiting watch names are spared (Table 15, I).
                anchors.extend(self.watches.anchors());
                let live = self.live();
                self.releasing = true;
                effects.push(Effect::Release {
                    nodes,
                    anchors,
                    acknowledged,
                    live,
                });
            }
            // A shell query reached at its deadline is answered at once with
            // no items, as one abandoned there would have nothing to say.
            Item::Query {
                request_id,
                query: Query::ShellItems { answer_by_us, .. },
            } if now.wall_us >= answer_by_us => {
                let outcome = QueryOutcome::Done(QueryResult::ShellItems(Vec::new()));
                self.send(reply(request_id, entry.trace, outcome), effects);
            }
            Item::Query {
                request_id,
                query: Query::ShellItemPosition { answer_by_us, .. },
            } if now.wall_us >= answer_by_us => {
                let none = verbatim_model::ShellCandidates {
                    candidates: Vec::new(),
                    complete: false,
                };
                let outcome = QueryOutcome::Done(QueryResult::ShellItemPosition(none));
                self.send(reply(request_id, entry.trace, outcome), effects);
            }
            _ => self.dispatch_entry(entry, Handling::Run, effects),
        }
    }

    /// The live requests: the program in flight's.
    fn live(&self) -> Vec<Request> {
        self.flight
            .running()
            .map(|(request, ..)| request)
            .into_iter()
            .collect()
    }

    /// Dispatches a program for `entry`.
    fn dispatch_entry(&mut self, entry: Entry, handling: Handling, effects: &mut Vec<Effect>) {
        let running = query_of(&entry);
        // Anything that may end a held terminal's hold reads every update
        // observed after it begins.
        if clears_absorb(&entry.item) {
            self.facts.follow.absorbed = None;
        }
        let work = Work::Entry { entry, handling };
        self.dispatch(work, running, effects);
    }

    /// Dispatches `work`, answering `running`. Verbatim moving the caret or
    /// selecting on a node clears its note, before its copy is taken.
    fn dispatch(
        &mut self,
        work: Work,
        running: Option<(RequestId, TraceId)>,
        effects: &mut Vec<Effect>,
    ) -> Request {
        if let Some(moved) = verbatims_caret_move(&work) {
            Arc::make_mut(&mut self.facts.text).clear_note(moved.number());
        }
        let current = Current {
            asked: match &work {
                Work::Ask(ask) => Some(ask.number),
                _ => None,
            },
            running,
            deferred: false,
        };
        let request = self.flight.start(current);
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            facts: self.facts.clone(),
            work,
        })));
        request
    }

    /// Dispatches the program that finishes `cut`, the caret read the
    /// program in flight cut at its value change check, before anything
    /// else and even while the outpost shuts down, as the read's program
    /// would have gone on: handed on to a new request, it takes over the
    /// query that program answers. The check is decided here: every
    /// value change the control raised before the read part's sync is
    /// ahead of its end in the mailbox, so among the entries not yet
    /// dispatched, in the intake.
    fn dispatch_finish(&mut self, cut: Box<Cut>, effects: &mut Vec<Effect>) {
        // No value change reaches the outpost: Core's event queue judges
        // them (`docs/design/readers-tell-core-asks-build1.md`, "Selection
        // and the caret's text facts"), so none waits here.
        let waiting = false;
        let Some(request) = self.flight.hand_on() else {
            return;
        };
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            facts: self.facts.clone(),
            work: Work::Finish { cut, waiting },
        })));
    }
}

/// The node `work` moves the caret or selects on for Verbatim, if it does:
/// the move becomes the last read, and clears the node's note.
fn verbatims_caret_move(work: &Work) -> Option<NodeId> {
    let Work::Entry {
        entry:
            Entry {
                item:
                    Item::Query {
                        query: Query::Text { node_id, op },
                        ..
                    },
                ..
            },
        ..
    } = work
    else {
        return None;
    };
    matches!(op, TextOp::MoveCaret(_) | TextOp::Select { .. }).then_some(*node_id)
}

/// Whether handling `item` may end a held terminal's hold, so its read
/// must see every update observed after it begins: a caret key, and
/// Core's on-demand reading but for a change of text.
fn clears_absorb(item: &Item) -> bool {
    matches!(
        item,
        Item::Query {
            query: Query::Text {
                op: TextOp::ReadCaret(_)
                    | TextOp::TerminalHold
                    | TextOp::TerminalRead { .. }
                    | TextOp::TerminalCancel,
                ..
            },
            ..
        }
    )
}

/// Whether attempting `owed` may end a held terminal's hold: a terminal's
/// first read.
fn owed_clears_absorb(owed: &Owed) -> bool {
    matches!(owed.work, OwedWork::FirstRead(_))
}

/// The query `entry` answers, if it is one.
fn query_of(entry: &Entry) -> Option<(RequestId, TraceId)> {
    match &entry.item {
        Item::Query { request_id, .. } => Some((*request_id, entry.trace)),
        _ => None,
    }
}

/// The starting point a starting read's `answer` minted, if it is one.
fn started_point(answer: &verbatim_model::request::Answer) -> Option<verbatim_model::TextAnchor> {
    match &answer.outcome {
        verbatim_model::request::Outcome::Result(verbatim_model::request::Reply::Text(
            verbatim_model::TextReply::Started(started),
        )) => Some(started.point),
        _ => None,
    }
}

/// The reply to query `request_id`, traced `trace`, ending as `outcome`.
fn reply(request_id: RequestId, trace: TraceId, outcome: QueryOutcome) -> OutpostToCore {
    OutpostToCore::Reply {
        trace_id: trace,
        request_id,
        outcome,
        timing: EventTiming::default(),
    }
}
