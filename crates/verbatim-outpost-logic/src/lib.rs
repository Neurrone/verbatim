//! The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine that holds every
//! remembered fact of the outpost's application, with no Windows type and
//! no application object. Inputs come in mailbox order; effects go out.
//! The outpost's shell (`verbatim-outpost`'s `outpost::shell` module) runs
//! it on a thread of its own, applies its effects, and keeps its time. It is
//! a logic crate: in the GPL tier, with no Windows dependency, so it builds
//! and is tested on any host.
//!
//! It holds the intake's order ([`intake`]), the follow state ([`follow`]),
//! Core's requests ([`waiting`]), the program in flight
//! with the worker generation in charge ([`charge`]), and the idle
//! barrier's round.
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

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use self::charge::{Abandoned, InCharge};
use self::watch::WatchCancelled;
use verbatim_model::TextOp;
use verbatim_model::protocol::OutpostToCore;

pub use self::charge::Request;
pub use self::entry::{Asker, Entry, Item, UiaEvent, UiaKind, WinKind, is_state_property};
pub use self::follow::{Change, Facts, FocusPath, Route};
pub use self::text::{
    Anchor, KeptSelection, LastRead, NodeAnchors, RangeKey, ReadSelection, TextChange, TextFacts,
};
pub use self::waiting::{Asks, Cancelled};
pub use self::watch::{EVENT_WINDOW_US, WORD_AFTER_US, WatchProgram, WatchWork, Watches};

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
    /// One entry.
    Entry(Entry),

    /// Drops the objects the shell hands it: what a release let go of.
    Drop,
    /// Core's request, answered once ([`waiting`]).
    Ask(Box<verbatim_model::request::Ask>),
    /// A caret key's watch's check or final read ([`watch`]), answered
    /// with [`Input::Answered`] when it ends the watch.
    Watch(Box<WatchProgram>),
    /// Forgets the windows Core named destroyed, in the order named: each
    /// one's MSAA nodes, text facts and classic-read mark,
    /// so a reused handle inherits none of them.
    Forget(Vec<verbatim_model::WindowHandle>),
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

/// What the reducer asks of the shell, in order.
#[derive(Debug)]
pub enum Effect {
    /// Hand `Program` to the worker in charge.
    Dispatch(Box<Program>),
    /// Send a message to Core, its nodes recorded as carried.
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
    /// A commit of a program's, its store writes made: the shell gives
    /// the reducer only a commit it accepts.
    Commit {
        /// The request the commit is of.
        request: Request,
        /// What the program found, to apply while the request is live.
        change: Box<Change>,
    },

    /// A program ended, and whether it panicked.
    Finished {
        /// The request that ended.
        request: Request,
        /// Unread since focus groups went with the event queue; kept until
        /// the caret watch's seam test, which sets it, is next changed.
        reported: bool,
        /// Whether it panicked.
        panicked: bool,
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
    /// Core named a window destroyed: the queued reads its MSAA ids name
    /// and the watches waiting on it are answered "not there" at once
    /// ([`waiting`], [`watch`]), and the window is owed a forget, run
    /// before any later work.
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

/// What the reducer keeps of the program in flight.
#[derive(Clone, Debug)]
struct Current {
    /// The Core request it runs ([`Asks::running`]), if it runs one.
    asked: Option<verbatim_model::request::RequestNumber>,
}

/// The reducer.
#[cfg_attr(test, derive(Clone, Debug))]
#[derive(Default)]
pub struct Reducer {
    /// The windows Core named destroyed and not yet forgotten, each once,
    /// in the order named, forgotten together by one program ahead of
    /// everything else.
    forgets: Vec<verbatim_model::WindowHandle>,
    /// Core's requests, run ahead of everything but the forgets.
    asks: Asks,
    /// The watches waiting, whose checks and final reads run ahead of
    /// Core's other requests.
    watches: Watches,
    intake: intake::Intake,
    facts: Facts,
    /// The program in flight, and the worker generation in charge.
    flight: InCharge<Current>,
    /// Whether a release let go of objects for a worker to drop.
    to_drop: bool,
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

    /// The worker generation in charge.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.flight.generation()
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

    /// When the shell is next to call [`tick`](Self::tick): a waiting
    /// watch's next check while no program is in flight, no release is
    /// pending and the outpost is not stopping (only then is it acted on),
    /// on the shell's clock.
    #[must_use]
    pub fn next_timer(&self, now: Now) -> Option<Duration> {
        if self.busy() || self.releasing || self.stopping {
            return None;
        }
        let due_us = self.watches.next_due()?;
        let left = due_us.saturating_sub(now.wall_us);
        // A time past the clock's range never comes.
        now.at.checked_add(Duration::from_micros(left))
    }

    /// Takes `input` at `now`.
    pub fn input(&mut self, input: Input, now: Now) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Entry(entry) => {
                self.entry(entry, now, &mut effects);
            }
            Input::Commit { request, change } => {
                if self.accepts(request) {
                    self.commit(&change, &mut effects);
                }
            }
            Input::Finished { request, .. } => self.finished(request, &mut effects),
            Input::Released {
                nodes,
                anchors,
                objects,
            } => self.released(&nodes, &anchors, objects, &mut effects),
            // Not pumped here: the shell ticks the reducer once it has
            // drained its mailbox, so the destroys drained together are
            // forgotten by one program.
            Input::WindowDestroyed(window) => {
                self.destroyed(window, &mut effects);
                self.answer_idle(&mut effects);
                return effects;
            }
            Input::Ask(_) | Input::CancelAsk(_) | Input::Answered { .. } => {
                self.asked(input, now, &mut effects);
            }
            Input::Close => {
                self.stopping = true;
                self.forgets.clear();
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
        effects
    }

    /// The shell's timed wait has ended at `now`: whatever is due is done.
    pub fn tick(&mut self, now: Now) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.pump(now, &mut effects);
        self.answer_idle(&mut effects);
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
                if let verbatim_model::request::Request::Text {
                    node,
                    op: TextOp::Watch(watch),
                } = &ask.request
                {
                    self.watches
                        .arrive((ask.number, ask.window, *node), *watch, now.wall_us);
                    return;
                }
                for replaced in self.asks.push(ask) {
                    effects.push(Effect::Send(Box::new(OutpostToCore::Answer(replaced))));
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
                    self.asks.answered(*answer)
                };
                if let Some(answer) = answer {
                    effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
                }
            }
            _ => {}
        }
    }

    /// Core named `window` destroyed (Table 14): the queued reads its MSAA
    /// ids name and the watches waiting on it are answered "not there";
    /// every other request stays queued and runs after the forget, which
    /// the window is now owed. A running request answers what it found.
    fn destroyed(&mut self, window: verbatim_model::WindowHandle, effects: &mut Vec<Effect>) {
        if self.stopping {
            return;
        }
        let mut gone = self.watches.destroyed(window);
        gone.extend(self.asks.destroyed(window));
        for gone in gone {
            effects.push(Effect::Send(Box::new(OutpostToCore::Answer(gone))));
        }
        if !self.forgets.contains(&window) {
            self.forgets.push(window);
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
                    Self::abandoned(&abandoned, effects);
                }
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
                return;
            }
        }
        match self.asks.cancel(cancel) {
            Cancelled::Dropped(answer) => {
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
            }
            Cancelled::Abandon(answer) => {
                if let Some(abandoned) = self.flight.abandon() {
                    Self::abandoned(&abandoned, effects);
                }
                effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
            }
            Cancelled::Nothing => {}
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
        let pushed = self.intake.push(entry);
        if !pushed.unparked.is_empty() {
            effects.push(Effect::Unpark(pushed.unparked));
        }
    }

    /// Applies a commit of the program in flight: its message sent, then
    /// the rest.
    fn commit(&mut self, change: &Change, effects: &mut Vec<Effect>) {
        if let Some(message) = change.result.clone() {
            effects.push(Effect::Send(Box::new(message)));
        }
        let ranges = follow::apply(&mut self.facts, change);
        if !ranges.is_empty() {
            effects.push(Effect::ForgetRanges(ranges));
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
        self.to_drop |= objects;
    }

    /// The worker in charge was abandoned: its replacement is started.
    fn abandoned(abandoned: &Abandoned, effects: &mut Vec<Effect>) {
        effects.push(Effect::Abandon {
            dead: abandoned.dead,
            replacement: abandoned.replacement,
        });
    }

    /// Program `request` ended.
    fn finished(&mut self, request: Request, effects: &mut Vec<Effect>) {
        if self
            .flight
            .running()
            .is_none_or(|(running, ..)| running != request)
        {
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
        if current.asked.is_some()
            && let Some(answer) = self.asks.ended()
        {
            effects.push(Effect::Send(Box::new(OutpostToCore::Answer(answer))));
        }
    }

    /// Answers the idle barrier's query once no watch waits. The program
    /// in flight is not waited for: the query is reached only with none in
    /// flight, so a program in flight then handles an entry that arrived
    /// after the query.
    fn answer_idle(&mut self, effects: &mut Vec<Effect>) {
        if !self.watches.is_empty() {
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

    /// Starts the next program, if none is in flight: the forgets owed
    /// first, as one program, so every request Core sent after a window's
    /// destruction runs after its forget; then a watch's work due, Core's
    /// requests, a release's drop, then the intake's next entry; entries
    /// the reducer handles itself are handled on the way.
    fn pump(&mut self, now: Now, effects: &mut Vec<Effect>) {
        while !self.busy() && !self.stopping && !self.releasing {
            if !self.forgets.is_empty() {
                let windows = std::mem::take(&mut self.forgets);
                self.dispatch(Work::Forget(windows), effects);
                continue;
            }
            if let Some(program) = self.watches.due(now.wall_us) {
                let program = Box::new(program);
                let request = self.dispatch(Work::Watch(program.clone()), effects);
                self.watches.running(&program, request, now.wall_us);
                continue;
            }
            if let Some(ask) = self.asks.take_next() {
                self.dispatch(Work::Ask(ask), effects);
                continue;
            }
            if std::mem::take(&mut self.to_drop) {
                self.dispatch(Work::Drop, effects);
                continue;
            }
            let Some(entry) = self.intake.next() else {
                return;
            };
            self.run(entry, effects);
        }
    }

    /// An entry handed out: the reducer's own entries are handled here, and
    /// the rest dispatched.
    fn run(&mut self, entry: Entry, effects: &mut Vec<Effect>) {
        match entry.item {
            // Answered before anything after it is dispatched: a test's
            // settle at once, Core's round once no watch waits.
            Item::Barrier { asker } => match asker {
                Asker::Round(round) => {
                    self.idle_query = Some(round);
                    self.answer_idle(effects);
                }
                Asker::Settle(id) => effects.push(Effect::Settled(id)),
            },
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
            _ => {
                self.dispatch(Work::Entry(entry), effects);
            }
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

    /// Dispatches `work`.
    fn dispatch(&mut self, work: Work, effects: &mut Vec<Effect>) -> Request {
        let current = Current {
            asked: match &work {
                Work::Ask(ask) => Some(ask.number),
                _ => None,
            },
        };
        let request = self.flight.start(current);
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            facts: self.facts.clone(),
            work,
        })));
        request
    }
}
