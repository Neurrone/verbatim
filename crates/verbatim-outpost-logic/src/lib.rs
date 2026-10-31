//! The outpost's reducer (`docs/design/ownership.md`): the pure state machine that holds every
//! remembered fact of the outpost's application, with no Windows type and
//! no application object. Inputs come in mailbox order; effects go out.
//! The outpost's shell (`verbatim-outpost`'s `outpost::shell` module) runs
//! it on a thread of its own, applies its effects, and keeps its time. It is
//! a logic crate: in the GPL tier, with no Windows dependency, so it builds
//! and is tested on any host.
//!
//! It holds the intake's order ([`intake`]), the gap record and its timer,
//! the reports Core may hold ([`held`]), the follow state ([`follow`]),
//! the hand-over's hold ([`hold`]), the arbitrator's verdicts
//! ([`verdicts`]), the worker generation in charge with its deadline, the moved-on grace and the walk's bound, the program in
//! flight, the queries not yet answered, and the idle barrier's round.
//! Programs, run by a worker on copies ([`Facts`]), make every call into
//! the application; their commits come back as inputs, and the reducer
//! applies a commit only while its request is live: its generation in
//! charge, and its walk not published at its bound.
//!
//! What it never does: call into an application, wait, read a clock, or
//! keep an object. Local reads of Windows' own state that never wait on an
//! application reach it through [`Locals`]. Time is the shell's: a
//! [`Now`] given with each input, and [`Reducer::next_timer`] for the
//! shell's timed wait.

mod entry;
mod follow;
mod gap_record;
mod held;
mod hold;
mod intake;
#[cfg(test)]
mod tests;
mod text;
mod verdicts;

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use verbatim_model::protocol::{
    EventTiming, GapAnswer, OutpostToCore, Query, QueryOutcome, WALK_DEADLINE,
};
use verbatim_model::{CallCounts, FocusGroup, NodeId, TextOp, TraceId};
use verbatim_reader_rules::MOVED_ON_GRACE;

pub use self::entry::{Entry, Item, Source, UiaEvent, UiaKind, WinKind, is_state_property};
pub use self::follow::{
    Applied, CARET_WATCH_BOUND, Change, DialogMemo, Facts, FocusPath, Follow, FollowState, GapNote,
    Group, OpenWatch, Owed, OwedWork, Poll, Report, Route, Subscription, Target, WatchChange,
    apply_local, wake_for,
};
pub use self::gap_record::{CopyDropped, GapRead};
pub use self::held::{Evidence, Focus, Held};
pub use self::intake::{Handling, Marks, Object, Planned, TimerAction};
pub use self::text::{
    Anchor, AnswerCut, CaretSeen, CutLine, KeptSelection, NodeAnchors, RangeKey, ReportCut,
    TextChange,
};
pub use self::verdicts::{Probe, Probed, VerdictChange, Verdicts};

/// The deadline for handling an event, a focus, or a focus-now query: NVDA's
/// `NORMAL_CORE_ALIVE_TIMEOUT` (`watchdog.py`), the time NVDA waits for an
/// application that is slow to answer before cancelling the call. An
/// application that is starting up, or the Start menu's search window as it
/// opens, can take two or three seconds to answer a read that then succeeds
/// (found live in the end-to-end suite); NVDA announces the focus late, and
/// a shorter deadline here dropped it for good.
pub const HANDLING_DEADLINE: Duration = Duration::from_secs(10);

/// The deadline for one navigation step or an activation.
const STEP_DEADLINE: Duration = Duration::from_millis(400);

/// The deadline for a text request: one read, or a few, of the node's text;
/// a caret key's watch is checked once within it and then kept open
/// outside it.
const TEXT_DEADLINE: Duration = Duration::from_secs(2);

/// One request of one worker generation: what a program's commits, the
/// store's mint tags and the walk's slot name it by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Request {
    /// The worker generation the request belongs to.
    pub generation: u64,
    /// The request's number within its generation.
    pub number: u64,
}

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

/// The local reads the reducer makes, which never wait on an application:
/// implemented live by the shell, and by a recording in tests.
pub trait Locals {
    /// Whether the system reports `hwnd`'s application hung.
    fn window_is_hung(&self, hwnd: isize) -> bool;
    /// Whether the user has left `window` for another thread of its
    /// application.
    fn moved_on(&self, window: isize) -> bool;
    /// The publication of the walk `request` armed, if it is still armed
    /// and not returned before its bound: its progress as it stands, as
    /// the change that reports it, completed by plain windows with local
    /// calls only, judged against `facts`. The walk is marked published, so
    /// its worker sends nothing more for it.
    fn publication(&mut self, request: Request, facts: &Facts) -> Option<Change>;
}

/// What a program is.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the intake to its program, as it always has"
)]
pub enum Work {
    /// One entry, handled as `handling` says, of batch `batch`, with its
    /// place in a focus group.
    Entry {
        /// The entry.
        entry: Entry,
        /// How the program handles it.
        handling: Handling,
        /// The batch it belongs to.
        batch: u64,
        /// Its place in a focus group, if it is in one.
        group: Option<FocusGroup>,
    },
    /// The work the followed focus owes.
    Owed(Owed),
    /// Answers `Outpost::settle` number `0`.
    Settle(u64),
    /// Drops the objects the shell hands it: what a release let go of.
    Drop,
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
/// check (`docs/design/ownership.md`): the read part read the caret and synced with the `WinEvent`
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
    /// The first check of a caret key's watch, which answers query
    /// `request_id`.
    Key {
        /// The query the check answers.
        request_id: u64,
        /// The query's trace.
        trace: TraceId,
        /// What the read part found.
        read: AnswerCut<u32>,
    },
    /// A check of the open watch, which the read part took out of the
    /// follow state, its query the program's to answer.
    Watch {
        /// The open watch, taken out of the follow state.
        open: Box<OpenWatch>,
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
    /// The caret events observed since the key of the watch the program
    /// may check was pressed that wait to be handled, a read of the
    /// watch's node covering one of its own (the outpost's `text_reads`).
    pub waiting: Vec<WaitingCaret>,
}

/// A caret event waiting in the intake, by what names its node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitingCaret {
    /// A UIA caret event of the element with this runtime id.
    Uia(Vec<i32>),
    /// The console's caret event of this window.
    Console(isize),
    /// The caret's location change or the text selection change of this
    /// edit control's window.
    Edit(isize),
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
    /// Mark the generation dead in its slot and start `replacement`;
    /// `count` abandoned workers have not returned.
    Abandon {
        /// The generation marked dead.
        dead: u64,
        /// The generation started in its place.
        replacement: u64,
        /// How many abandoned workers have not returned.
        count: usize,
        /// Why the generation was abandoned, for the log.
        reason: &'static str,
    },
    /// An abandoned worker returned; `count` have not.
    Returned {
        /// How many abandoned workers have not returned.
        count: usize,
    },
    /// The push-time copies of the intake's marks.
    Marks(Marks),
    /// The push-time copy of the arbitrator's verdicts, replaced.
    Verdicts(Arc<Verdicts>),
    /// A verdict of UIA for a window's lifetime was recorded: the event
    /// thread may narrow its hook (the outpost's `narrowing`).
    Narrow,
    /// Set the gap record's timer for this observation time, or cancel it.
    GapTimer(Option<u64>),
    /// A caret key's watch ended without evidence, for the log: why, and
    /// how long it waited.
    WatchEnded {
        /// The key's trace.
        trace: TraceId,
        /// Why the watch ended.
        why: &'static str,
        /// How long the watch waited.
        waited: Duration,
    },
}

/// Inputs, in mailbox order.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the mailbox into the intake, as it always has"
)]
pub enum Input {
    /// An entry from a producer: where it came from, and, for a UIA
    /// focus's read, what it answers of the gap record.
    Entry {
        /// The entry.
        entry: Entry,
        /// Where it came from.
        source: Source,
        /// For a UIA focus's read, what it answers of the gap record.
        read_answer: Option<GapAnswer>,
    },
    /// The outpost's hook is in, and a listener forwards to it.
    Hooked,
    /// No listener forwards to this outpost.
    WithoutListener,
    /// The listener's "forwarding ended", through this tick.
    ForwardingEnded(u32),
    /// Core withdraws query `0` if it has not started.
    Cancel(u64),
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
    /// A program's walk was armed, its bound at the time given.
    Armed {
        /// The request whose walk was armed.
        request: Request,
        /// The walk's bound, on the shell's clock.
        bound: Duration,
    },
    /// A program's walk returned; `past` when it returned past its bound,
    /// for the reducer to publish it from its progress.
    WalkReturned {
        /// The request whose walk returned.
        request: Request,
        /// Whether it returned past its bound.
        past: bool,
    },
    /// A program ended: whether it reported a focus that ends its group,
    /// whether it panicked, and the caret read it cut at its value change
    /// check, for the reducer to decide and finish.
    Finished {
        /// The request that ended.
        request: Request,
        /// Whether it reported a focus that ends its group.
        reported: bool,
        /// Whether it panicked.
        panicked: bool,
        /// The caret read it cut at its value change check, for the reducer to decide and finish.
        cut: Option<Box<Cut>>,
    },
    /// An abandoned worker returned from its call.
    Returned,
    /// What a release let go of, nodes and anchors by number, and whether
    /// there are objects to drop.
    Released {
        /// The nodes let go of, by number.
        nodes: HashSet<u64>,
        /// The text anchors let go of, by number.
        anchors: HashSet<u64>,
        /// Whether there are objects to drop.
        objects: bool,
    },
    /// A test times the gap record's bound: the reducer keeps no timer of
    /// its own for it from now on.
    GapTimerGiven,
    /// The gap record's bound was reached by a timer the shell was given.
    GapBound,
    /// The outpost is shutting down.
    Close,
}

/// What a program is doing, as the reducer follows it.
#[derive(Debug)]
struct Current {
    request: Request,
    /// The deadline and when it started, and the window the moved-on rule
    /// watches (0 for none).
    deadline: Duration,
    started: Duration,
    window: isize,
    /// The query the program answers, if it answers one, which an
    /// abandonment or a program that ends without answering answers.
    running: Option<(u64, TraceId)>,
    /// The bound of its walk, while armed.
    walk: Option<Duration>,
    /// Whether the moved-on rule was judged as the grace ended: from then
    /// on only a foreground fact judges it again.
    grace_judged: bool,
    /// Whether its walk was published at its bound: nothing more of its
    /// is applied.
    claimed: bool,
    /// The focus group try it is, if it is one.
    group: Option<GroupTry>,
    /// Whether it does work the idle barrier's answer waits for, as it
    /// waits for an open watch or an open gap record: it reads the gap
    /// record, or it took the caret key's watch from the facts.
    deferred: bool,
}

/// One try of a focus group, and what is left of it.
#[derive(Debug)]
struct GroupTry {
    number: u64,
    rank: u32,
    batch: u64,
    older: VecDeque<Entry>,
}

/// The reducer.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of the reducer's, each changed by its own inputs"
)]
pub struct Reducer {
    intake: intake::Intake,
    hold: hold::Hold,
    facts: Facts,
    /// The worker generation in charge.
    generation: u64,
    /// The last request number given.
    requests: u64,
    current: Option<Current>,
    /// The tries left of a focus group whose try was abandoned, or the
    /// gap's read a dropped copy asked for, to run before anything else.
    after: VecDeque<(Entry, Option<GroupTry>)>,
    /// Whether a release let go of objects for a worker to drop.
    to_drop: bool,
    /// The queries recorded and not yet answered.
    queries: BTreeSet<u64>,
    /// The idle barrier's round reached and not yet answered.
    idle_query: Option<u64>,
    /// How many focus groups have been tried.
    focus_groups: u64,
    /// Abandoned workers that have not returned.
    abandoned: usize,
    /// When the gap record's bound passes, on the observation clock.
    gap_due: Option<u64>,
    /// Whether a test's timer times the gap record's bound in place of the
    /// reducer's own.
    gap_timer_given: bool,
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
        self.generation
    }

    /// The queries recorded and not yet answered, which the shell's tokens
    /// match.
    #[must_use]
    pub fn queries(&self) -> &BTreeSet<u64> {
        &self.queries
    }

    /// Whether a program is in flight.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// Whether a commit of `request` is applied: it is the program in
    /// flight's, of the generation in charge, and its walk was not
    /// published at its bound.
    #[must_use]
    pub fn accepts(&self, request: Request) -> bool {
        self.current
            .as_ref()
            .is_some_and(|current| current.request == request && !current.claimed)
    }

    /// When the shell is next to call [`tick`](Self::tick): the program's
    /// deadline, the end of its moved-on grace, its walk's bound, the
    /// watch's wake while no program is in flight and no release is
    /// pending (only then does a tick take it), and the gap record's
    /// bound, on the shell's clock.
    #[must_use]
    pub fn next_timer(&self, now: Now) -> Option<Duration> {
        let mut times = Vec::new();
        if let Some(current) = &self.current
            && !self.stopping
        {
            times.push(current.deadline);
            let grace = current.started + MOVED_ON_GRACE;
            if current.window != 0 && now.at < grace {
                times.push(grace);
            }
            times.extend(current.walk);
        }
        if self.current.is_none() && !self.releasing {
            times.extend(self.intake.wake());
        }
        if let Some(due_us) = self.gap_due.filter(|_| !self.gap_timer_given) {
            let left = due_us.saturating_sub(now.observed_us);
            times.push(now.at + Duration::from_micros(left));
        }
        times.into_iter().min()
    }

    /// Takes `input` at `now`.
    pub fn input(&mut self, input: Input, now: Now, locals: &mut dyn Locals) -> Vec<Effect> {
        let mut effects = Vec::new();
        match input {
            Input::Entry {
                entry,
                source,
                read_answer,
            } => self.entry(entry, source, read_answer, now, locals, &mut effects),
            Input::Hooked => self.hold.hooked(),
            Input::WithoutListener => {
                let held = self.hold.without_listener();
                self.push_all(held, &mut effects);
            }
            Input::ForwardingEnded(through) => {
                if let Some(released) = self.hold.forwarding_ended(through) {
                    self.push_all(released, &mut effects);
                }
            }
            Input::Cancel(request_id) => {
                if self.intake.cancel(request_id) {
                    self.send(
                        reply(request_id, TraceId::mint(), QueryOutcome::NotStarted),
                        &mut effects,
                    );
                }
            }
            Input::Verdict { generation, change } => {
                self.record_verdict(generation, &change, &mut effects);
            }
            Input::Commit { request, change } => {
                if self.accepts(request) {
                    self.commit(request, &change, now, &mut effects);
                }
            }
            Input::Armed { request, bound } => {
                if let Some(current) = self.current_of(request) {
                    current.walk = Some(bound);
                }
            }
            Input::WalkReturned { request, past } => {
                let armed = self
                    .current_of(request)
                    .is_some_and(|current| current.walk.take().is_some() && !current.claimed);
                if armed && past {
                    self.publish(request, now, locals, &mut effects);
                }
            }
            Input::Finished {
                request,
                reported,
                panicked,
                cut,
            } => self.finished(request, (reported, panicked), cut, now, &mut effects),
            Input::Returned => {
                self.abandoned = self.abandoned.saturating_sub(1);
                effects.push(Effect::Returned {
                    count: self.abandoned,
                });
            }
            Input::Released {
                nodes,
                anchors,
                objects,
            } => {
                self.releasing = false;
                self.facts.held.release(&nodes);
                let ranges = Arc::make_mut(&mut self.facts.text).release(&nodes, &anchors);
                if !ranges.is_empty() {
                    effects.push(Effect::ForgetRanges(ranges));
                }
                let mut applied = Applied::default();
                self.facts.follow.release_groups(&nodes, &mut applied);
                self.subscription_effects(applied, now, &mut effects);
                self.to_drop |= objects;
            }
            Input::GapTimerGiven => self.gap_timer_given = true,
            Input::GapBound => self.gap_bound_entry(now),
            Input::Close => {
                self.stopping = true;
                let unparked = self.intake.close();
                if !unparked.is_empty() {
                    effects.push(Effect::Unpark(unparked));
                }
            }
        }
        self.pump(now, locals, &mut effects);
        self.answer_idle(&mut effects);
        effects
    }

    /// The shell's timed wait has ended at `now`: whatever is due is done.
    pub fn tick(&mut self, now: Now, locals: &mut dyn Locals) -> Vec<Effect> {
        let mut effects = Vec::new();
        if !self.stopping
            && let Some(current) = &self.current
        {
            let request = current.request;
            if current.walk.is_some_and(|bound| now.at >= bound) {
                self.publish(request, now, locals, &mut effects);
            }
            // The moved-on rule is judged once as the grace ends, and then
            // at each foreground fact, never at every wake.
            let grace_ended = self.current.as_mut().is_some_and(|current| {
                let ended = !current.grace_judged && now.at >= current.started + MOVED_ON_GRACE;
                current.grace_judged |= ended;
                ended
            });
            self.judge_abandonment(now, locals, grace_ended, &mut effects);
        }
        if !self.gap_timer_given && self.gap_due.is_some_and(|due_us| now.observed_us >= due_us) {
            self.gap_due = None;
            self.gap_bound_entry(now);
        }
        self.pump(now, locals, &mut effects);
        self.answer_idle(&mut effects);
        effects
    }

    /// The gap record's bound has been reached: its entry waits behind
    /// every marker queued before it.
    fn gap_bound_entry(&mut self, now: Now) {
        self.intake
            .push_gap_bound(Entry::own(Item::GapBound, TraceId::mint(), now.observed_us));
    }

    /// The program in flight, if it is `request`'s.
    fn current_of(&mut self, request: Request) -> Option<&mut Current> {
        self.current
            .as_mut()
            .filter(|current| current.request == request)
    }

    /// Takes in `entry`, from `source`: held while the hand-over's hold
    /// lasts if it came from the hook, and pushed into the intake
    /// otherwise.
    fn entry(
        &mut self,
        entry: Entry,
        source: Source,
        read_answer: Option<GapAnswer>,
        now: Now,
        locals: &mut dyn Locals,
        effects: &mut Vec<Effect>,
    ) {
        let foreground = matches!(
            entry.item,
            Item::Fact(
                verbatim_model::protocol::DeliveredFact::Foreground { .. },
                _
            )
        );
        match source {
            Source::Hook => {
                if let Some(entry) = self.hold.hooked_copy(entry) {
                    self.push(entry, read_answer, effects);
                }
            }
            Source::Listener => {
                self.hold.listener_copy(&entry);
                self.push(entry, read_answer, effects);
            }
            Source::Other => {
                let ends_reads = matches!(
                    entry.item,
                    Item::Fact(
                        verbatim_model::protocol::DeliveredFact::UiaFocusReadsEnded,
                        _
                    )
                );
                self.push(entry, read_answer, effects);
                if ends_reads && let Some(released) = self.hold.reads_ended() {
                    self.push_all(released, effects);
                }
            }
        }
        // A foreground fact judges again whether the user has moved on
        // from the program's window.
        if foreground {
            self.judge_abandonment(now, locals, true, effects);
        }
    }

    /// Pushes `entries` into the intake in order, as one batch.
    fn push_all(&mut self, entries: Vec<Entry>, effects: &mut Vec<Effect>) {
        for entry in entries {
            self.push(entry, None, effects);
        }
    }

    /// Pushes `entry` into the intake, recording a query.
    fn push(&mut self, entry: Entry, read_answer: Option<GapAnswer>, effects: &mut Vec<Effect>) {
        if let Item::Query { request_id, .. } = entry.item
            && !self.stopping
        {
            self.queries.insert(request_id);
        }
        let pushed = self.intake.push(entry, read_answer);
        if !pushed.unparked.is_empty() {
            effects.push(Effect::Unpark(pushed.unparked));
        }
        if let Some(action) = pushed.timer {
            self.gap_timer(action, effects);
        }
    }

    /// Does what a change to the gap record asks of its timer.
    fn gap_timer(&mut self, action: TimerAction, effects: &mut Vec<Effect>) {
        match action {
            TimerAction::Keep => {}
            TimerAction::Set(due_us) => {
                self.gap_due = Some(due_us);
                effects.push(Effect::GapTimer(Some(due_us)));
            }
            TimerAction::Cancel => {
                self.gap_due = None;
                effects.push(Effect::GapTimer(None));
            }
        }
    }

    /// Sends `message`; a reply answers its query.
    fn send(&mut self, message: OutpostToCore, effects: &mut Vec<Effect>) {
        if let OutpostToCore::Reply { request_id, .. } = &message {
            self.queries.remove(request_id);
            if let Some(current) = self.current.as_mut().filter(|current| {
                current
                    .running
                    .is_some_and(|(running, _)| running == *request_id)
            }) {
                current.running = None;
            }
        }
        effects.push(Effect::Send(Box::new(message)));
    }

    /// Applies the commit of `request`'s program: its report recorded for
    /// Core's judgement, then its messages sent, then the rest.
    fn commit(&mut self, request: Request, change: &Change, now: Now, effects: &mut Vec<Effect>) {
        if let Some(report) = &change.report {
            follow::record(&mut self.facts.held, report);
        }
        if let Some(message) = change.result.clone() {
            if let Some(current) = self.current_of(request) {
                current.running = None;
            }
            self.send(message, effects);
        }
        if let Some((_, message)) = change.aside.clone() {
            // The query it answers stops being any slot's as it is sent.
            self.send(message, effects);
        }
        if matches!(
            change.watch,
            Some(WatchChange::Open(..) | WatchChange::PutBack(_) | WatchChange::Read { .. })
        ) && let Some(current) = self.current_of(request)
        {
            current.running = None;
            current.deferred = false;
        }
        for note in &change.gap {
            self.gap_note(*note, effects);
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
        let mut applied = follow::apply(&mut self.facts, self.generation, change);
        let ranges = std::mem::take(&mut applied.ranges);
        if !ranges.is_empty() {
            effects.push(Effect::ForgetRanges(ranges));
        }
        if let Some(open) = &applied.taken
            && let Some(current) = self.current_of(request)
        {
            current.running = Some((open.request_id, open.trace));
            current.deferred = true;
        }
        let marks = applied.adopted.is_some()
            || change.drop_system_caret
            || change.absorb.is_some()
            || change.clear_redirect;
        self.subscription_effects(applied, now, effects);
        if marks {
            self.sync_marks(effects);
        }
    }

    /// Records a verdict of a program of `generation`'s: one from the
    /// generation in charge, and a probe that timed out or a window's
    /// forgetting from any generation, so a window whose probe timed out
    /// is not probed again within the negative verdict's lifetime even
    /// when the worker that probed it was abandoned. The push-time copy is
    /// replaced when the record changed, and the event thread nudged when
    /// a window was found to use UIA.
    fn record_verdict(
        &mut self,
        generation: u64,
        change: &VerdictChange,
        effects: &mut Vec<Effect>,
    ) {
        if generation != self.generation && !change.kept_from_any_generation() {
            return;
        }
        if !Arc::make_mut(&mut self.facts.verdicts).apply(change) {
            return;
        }
        effects.push(Effect::Verdicts(Arc::clone(&self.facts.verdicts)));
        if matches!(
            change,
            VerdictChange::Probed(Probe {
                probed: Probed::Uia,
                ..
            })
        ) {
            effects.push(Effect::Narrow);
        }
    }

    /// The shell's part of an [`Applied`]: the subscription moved, the
    /// elements no longer followed dropped, ended watches answered, and the
    /// watch's wake set.
    fn subscription_effects(&mut self, applied: Applied, now: Now, effects: &mut Vec<Effect>) {
        let Applied {
            ended,
            subscribe,
            unfollowed,
            wake,
            ..
        } = applied;
        for (open, why) in ended {
            effects.push(Effect::WatchEnded {
                trace: open.trace,
                why,
                waited: now.at.saturating_sub(open.opened),
            });
            self.send(watch_ended(&open, now.wall_us), effects);
        }
        if let Some(subscribe) = subscribe {
            effects.push(Effect::Subscribe(subscribe));
        }
        if !unfollowed.is_empty() {
            effects.push(Effect::Unfollow(unfollowed));
        }
        if let Some(wake) = wake {
            self.intake.wake_at(wake);
        }
    }

    /// The intake's marks follow the follow state's, and the mailbox is
    /// given a copy.
    fn sync_marks(&mut self, effects: &mut Vec<Effect>) {
        let marks = self.facts.marks.clone();
        self.intake.note_focus(
            marks.focused.clone(),
            marks.terminal,
            marks.system_caret_unused,
            marks.focus_redirected,
        );
        self.intake.absorb_console_updates(marks.absorbed);
        effects.push(Effect::Marks(marks));
    }

    /// A program's judgement for the gap record.
    fn gap_note(&mut self, note: GapNote, effects: &mut Vec<Effect>) {
        match note {
            GapNote::CopyDropped {
                window,
                top_level,
                at_us,
            } => {
                let (dropped, action) = self.intake.gap_copy_dropped(window, top_level, at_us);
                self.gap_timer(action, effects);
                if dropped == CopyDropped::ReadNow {
                    // Read at once, in the copy's place, as part of the
                    // same try of its group.
                    let group = self.current.as_mut().and_then(|current| {
                        current.group.as_ref().map(|group| GroupTry {
                            number: group.number,
                            rank: group.rank,
                            batch: group.batch,
                            older: VecDeque::new(),
                        })
                    });
                    self.after.push_front((
                        Entry::own(Item::GapRead { window }, TraceId::mint(), at_us),
                        group,
                    ));
                }
            }
            GapNote::Accepted {
                window,
                top_level,
                at_us,
                restamps,
            } => {
                let action = self.intake.gap_accepted(window, top_level, at_us, restamps);
                self.gap_timer(action, effects);
            }
            GapNote::WindowDestroyed(hwnd) => self.intake.gap_window_destroyed(hwnd),
        }
    }

    /// Publishes the walk `request` armed, if it still is, from its
    /// progress: the walk is claimed, so nothing more of its program is
    /// applied, and the publication is applied as a commit of its.
    fn publish(
        &mut self,
        request: Request,
        now: Now,
        locals: &mut dyn Locals,
        effects: &mut Vec<Effect>,
    ) {
        let Some(current) = self.current_of(request) else {
            return;
        };
        if current.claimed {
            return;
        }
        current.walk = None;
        let Some(change) = locals.publication(request, &self.facts) else {
            return;
        };
        self.commit(request, &change, now, effects);
        if let Some(current) = self.current_of(request) {
            current.claimed = true;
        }
    }

    /// Abandons the worker in charge if its program has passed its
    /// deadline, or, judged when `moved_on` says, has waited
    /// [`MOVED_ON_GRACE`] on a window the user has left for another thread
    /// of its application: a walk armed is published first, in the same
    /// transition.
    fn judge_abandonment(
        &mut self,
        now: Now,
        locals: &mut dyn Locals,
        moved_on: bool,
        effects: &mut Vec<Effect>,
    ) {
        if self.stopping {
            return;
        }
        let Some(current) = &self.current else {
            return;
        };
        let reason = if now.at >= current.deadline {
            Some("a call passed its deadline")
        } else if moved_on
            && current.window != 0
            && now.at >= current.started + MOVED_ON_GRACE
            && locals.moved_on(current.window)
        {
            Some("the user moved on from a slow window")
        } else {
            None
        };
        let Some(reason) = reason else {
            return;
        };
        let request = current.request;
        if current.walk.is_some() {
            self.publish(request, now, locals, effects);
        }
        let Some(current) = self.current.take() else {
            return;
        };
        let dead = self.generation;
        self.generation += 1;
        if self
            .facts
            .follow
            .owed
            .as_ref()
            .is_some_and(|owed| owed.attempting == Some(dead))
        {
            self.facts.follow.owed = None;
        }
        self.abandoned += 1;
        effects.push(Effect::Abandon {
            dead,
            replacement: self.generation,
            count: self.abandoned,
            reason,
        });
        if let Some((request_id, trace)) = current.running {
            // A read of the open watch's console poll leaves the watch in
            // the facts: it is taken out without a word, its query answered
            // here, once. A program that took the watch out to check it has
            // already removed it.
            if self
                .facts
                .follow
                .caret_watch
                .take_if(|open| open.request_id == request_id)
                .is_some()
            {
                self.intake.wake_at(None);
            }
            self.send(reply(request_id, trace, QueryOutcome::Abandoned), effects);
        }
        // A focus group's older entries are tried by the replacement, unless
        // the abandoned try was published.
        if let Some(group) = current.group
            && !current.claimed
        {
            self.continue_group(group);
        }
    }

    /// A program ended, having cut a caret read at its value change check
    /// when `cut` is given.
    fn finished(
        &mut self,
        request: Request,
        (reported, panicked): (bool, bool),
        cut: Option<Box<Cut>>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        let Some(current) = self.current.take_if(|current| current.request == request) else {
            return;
        };
        if let Some(cut) = cut.filter(|_| !panicked && !current.claimed) {
            self.dispatch_finish(current, cut, effects);
            return;
        }
        // A query its program left unanswered, because it panicked or
        // returned without its reply, still gets its one reply. A read of
        // the open watch's console poll that ended so ends the watch, which
        // answers its query, so the watch is not read again as it stood.
        if let Some((request_id, trace)) = current.running {
            if self
                .facts
                .follow
                .caret_watch
                .as_ref()
                .is_some_and(|open| open.request_id == request_id)
            {
                let mut applied = Applied::default();
                self.facts
                    .follow
                    .end_watch_of(request_id, "its read failed", &mut applied);
                self.subscription_effects(applied, now, effects);
                return;
            }
            self.send(
                reply(
                    request_id,
                    trace,
                    QueryOutcome::Failed("the outpost failed handling the query".to_owned()),
                ),
                effects,
            );
        }
        if let Some(group) = current.group {
            let ended = reported || current.claimed;
            let read_after = self
                .after
                .front()
                .is_some_and(|(entry, _)| matches!(entry.item, Item::GapRead { .. }));
            if ended {
                // The gap's read of an ended try is still made, outside it.
                if read_after && let Some((_, group)) = self.after.front_mut() {
                    *group = None;
                }
            } else if read_after {
                // The gap's read goes on the try, and its older entries
                // after it.
                if let Some((_, Some(read_group))) = self.after.front_mut() {
                    read_group.older = group.older;
                }
            } else {
                self.continue_group(group);
            }
        }
    }

    /// Queues the next older entry of `group`, if it has one.
    fn continue_group(&mut self, mut group: GroupTry) {
        if let Some(entry) = group.older.pop_front() {
            group.rank += 1;
            self.after.push_back((entry, Some(group)));
        }
    }

    /// Answers the idle barrier's query once no deferred work is pending:
    /// the hook's copies are not held at the start, no caret key's watch is
    /// open, the gap record is closed, and
    /// the program in flight neither reads it nor holds the watch. The program in flight is otherwise not waited for:
    /// the query is reached only with none in flight, so a program in
    /// flight then handles an entry that arrived after the query.
    fn answer_idle(&mut self, effects: &mut Vec<Effect>) {
        if self.hold.holding()
            || self
                .current
                .as_ref()
                .is_some_and(|current| current.deferred)
            || self.facts.follow.caret_watch.is_some()
            || self.intake.gap_open()
        {
            return;
        }
        if let Some(round) = self.idle_query.take() {
            effects.push(Effect::Send(Box::new(OutpostToCore::IdleAnswer { round })));
        }
    }

    /// A new request of the generation in charge.
    fn next_request(&mut self) -> Request {
        self.requests += 1;
        Request {
            generation: self.generation,
            number: self.requests,
        }
    }

    /// The program that drops what the shell still holds as the outpost
    /// shuts down, with no program in flight: releasing an object can call
    /// into the application, so a worker does it, as at runtime.
    pub fn shutdown_drop(&mut self) -> Program {
        Program {
            request: self.next_request(),
            facts: Facts::default(),
            work: Work::Drop,
            waiting: Vec::new(),
        }
    }

    /// Starts the next program, if none is in flight: owed work first, then
    /// what a program left to run after it, then a release's drop, then
    /// the intake's next entry; entries the reducer handles itself are
    /// handled on the way.
    fn pump(&mut self, now: Now, locals: &mut dyn Locals, effects: &mut Vec<Effect>) {
        while self.current.is_none() && !self.stopping && !self.releasing {
            if let Some(owed) = self
                .facts
                .follow
                .owed
                .as_mut()
                .filter(|owed| owed.attempting.is_none())
            {
                owed.attempting = Some(self.generation);
                let owed = owed.clone();
                // Whatever the owed read observes must reach it.
                if owed_clears_absorb(&owed) && self.facts.marks.absorbed.is_some() {
                    self.facts.marks.absorbed = None;
                    self.sync_marks(effects);
                }
                self.dispatch(
                    Work::Owed(owed),
                    (HANDLING_DEADLINE, 0, None),
                    None,
                    now,
                    effects,
                );
                continue;
            }
            if let Some((entry, group)) = self.after.pop_front() {
                self.start_entry(entry, Handling::Run, group, now, effects);
                continue;
            }
            if std::mem::take(&mut self.to_drop) {
                self.dispatch(Work::Drop, (WALK_DEADLINE, 0, None), None, now, effects);
                continue;
            }
            let Some((planned, batch)) =
                self.intake.next(now.at, |hwnd| locals.window_is_hung(hwnd))
            else {
                return;
            };
            match planned {
                Planned::Run(entry) => self.run(entry, batch, now, effects),
                Planned::Menu(entry) => {
                    self.dispatch_entry(entry, Handling::Menu, batch, None, now, effects);
                }
                Planned::Focus(entries) => {
                    self.focus_groups += 1;
                    let mut older: VecDeque<Entry> = entries.into();
                    let Some(newest) = older.pop_front() else {
                        continue;
                    };
                    let group = GroupTry {
                        number: self.focus_groups,
                        rank: 0,
                        batch,
                        older,
                    };
                    self.start_entry(newest, Handling::Tried(0), Some(group), now, effects);
                }
            }
        }
    }

    /// Starts `entry` as a try of `group`, or on its own.
    fn start_entry(
        &mut self,
        entry: Entry,
        handling: Handling,
        group: Option<GroupTry>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        let batch = group
            .as_ref()
            .map_or(self.intake_batch(), |group| group.batch);
        let handling = match &group {
            Some(group) => Handling::Tried(group.rank),
            None => handling,
        };
        self.dispatch_entry(entry, handling, batch, group, now, effects);
    }

    /// The number of the batch being handed out.
    fn intake_batch(&self) -> u64 {
        self.intake.batch_number()
    }

    /// An entry planned on its own: the reducer's own entries are handled
    /// here, and the rest dispatched.
    fn run(&mut self, entry: Entry, batch: u64, now: Now, effects: &mut Vec<Effect>) {
        match entry.item {
            Item::IdleQuery {
                round,
                behind_noted,
            } => {
                // A noted caret is work owed: the query goes directly
                // behind it, and waits for nothing more when it is reached
                // again.
                if !behind_noted && let Some(noted) = self.intake.take_idle() {
                    let query = Entry {
                        item: Item::IdleQuery {
                            round,
                            behind_noted: true,
                        },
                        ..entry
                    };
                    self.intake.push_ahead([noted, query]);
                } else {
                    // Answered before anything after it is dispatched.
                    self.idle_query = Some(round);
                    self.answer_idle(effects);
                }
            }
            Item::NodesHeld {
                nodes,
                anchors,
                acknowledged,
            } => {
                let live = self.live();
                self.releasing = true;
                effects.push(Effect::Release {
                    nodes,
                    anchors,
                    acknowledged,
                    live,
                });
            }
            Item::Settle(id) => {
                if self.intake.busy() {
                    // Goes to the back of the queue again.
                    let _ = self.intake.push(entry, None);
                } else if let Some(noted) = self.intake.take_idle() {
                    // A noted caret waits for nothing else to be ready, and
                    // the settle is: it goes first, and the settle after it.
                    let _ = self.intake.push(noted, None);
                    let _ = self.intake.push(entry, None);
                } else {
                    self.dispatch(
                        Work::Settle(id),
                        (HANDLING_DEADLINE, 0, None),
                        None,
                        now,
                        effects,
                    );
                }
            }
            Item::GapBound => match self.intake.gap_bound_reached(now.observed_us) {
                Ok(Some(GapRead { window, stamp_us })) => {
                    let read = Entry {
                        item: Item::GapRead { window },
                        observed_at_us: stamp_us,
                        ..entry
                    };
                    self.dispatch_entry(read, Handling::Run, batch, None, now, effects);
                }
                Ok(None) => {}
                Err(due_us) => self.gap_timer(TimerAction::Set(due_us), effects),
            },
            Item::Wake => {
                // A read of the open watch's console poll is a program of
                // its own, timed by the reducer's wake, under the text
                // deadline, its query the watch's until the read commits.
                let polled = self
                    .facts
                    .follow
                    .caret_watch
                    .as_ref()
                    .filter(|open| open.poll.is_some())
                    .map(|open| (open.request_id, open.trace));
                match polled {
                    Some(running) => {
                        let work = Work::Entry {
                            entry,
                            handling: Handling::Run,
                            batch,
                            group: None,
                        };
                        self.dispatch(work, (TEXT_DEADLINE, 0, Some(running)), None, now, effects);
                    }
                    None => self.dispatch_entry(entry, Handling::Run, batch, None, now, effects),
                }
            }
            _ => self.dispatch_entry(entry, Handling::Run, batch, None, now, effects),
        }
    }

    /// The live requests: the program in flight's, unless its walk was
    /// published.
    fn live(&self) -> Vec<Request> {
        self.current
            .iter()
            .filter(|current| !current.claimed && current.request.generation == self.generation)
            .map(|current| current.request)
            .collect()
    }

    /// Dispatches a program for `entry`.
    fn dispatch_entry(
        &mut self,
        entry: Entry,
        handling: Handling,
        batch: u64,
        group: Option<GroupTry>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        let (deadline, running) = budget(&entry);
        let window = intake::window_of(&entry.item);
        // Anything that may end a held terminal's hold reads every update
        // observed after it begins.
        if clears_absorb(&entry.item) && self.facts.marks.absorbed.is_some() {
            self.facts.marks.absorbed = None;
            self.sync_marks(effects);
        }
        let group_place = group.as_ref().map(|group| FocusGroup {
            number: group.number,
            rank: group.rank,
        });
        let work = Work::Entry {
            entry,
            handling,
            batch,
            group: group_place,
        };
        self.dispatch(work, (deadline, window, running), group, now, effects);
    }

    /// Dispatches `work` under `deadline`, concerning `window` (0 for none)
    /// and answering `running`, as a try of `group` when it is one. A read
    /// of the gap record is deferred work.
    fn dispatch(
        &mut self,
        work: Work,
        (deadline, window, running): (Duration, isize, Option<(u64, TraceId)>),
        group: Option<GroupTry>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        let request = self.next_request();
        let waiting = self.waiting(&work);
        let deferred = matches!(
            &work,
            Work::Entry {
                entry: Entry {
                    item: Item::GapRead { .. },
                    ..
                },
                ..
            }
        );
        self.current = Some(Current {
            request,
            deadline: now.at + deadline,
            started: now.at,
            window,
            running,
            walk: None,
            grace_judged: false,
            claimed: false,
            group,
            deferred,
        });
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            facts: self.facts.clone(),
            work,
            waiting,
        })));
    }

    /// Dispatches the program that finishes `cut`, the caret read the
    /// program `read` cut at its value change check, before anything else
    /// and even while the outpost shuts down, as the read's program would
    /// have gone on: it takes over that program's deadline, its moved-on
    /// grace and the query it answers. The check is decided here: every
    /// value change the control raised before the read part's sync is
    /// ahead of its end in the mailbox, so among the entries not yet
    /// dispatched, in the intake or held.
    fn dispatch_finish(&mut self, read: Current, cut: Box<Cut>, effects: &mut Vec<Effect>) {
        let waiting = self.intake.value_change_waiting(cut.edit)
            || self
                .hold
                .held()
                .any(|entry| edit_value_change(&entry.item) == Some(cut.edit));
        let request = self.next_request();
        self.current = Some(Current { request, ..read });
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            facts: self.facts.clone(),
            work: Work::Finish { cut, waiting },
            waiting: Vec::new(),
        })));
    }

    /// The caret events waiting in the intake observed since the key of
    /// the watch `work` may check was pressed, the open watch's or the one
    /// a caret key's query opens ([`Program::waiting`]).
    fn waiting(&self, work: &Work) -> Vec<WaitingCaret> {
        let pressed_ms = match work {
            Work::Entry {
                entry:
                    Entry {
                        item:
                            Item::Query {
                                query:
                                    Query::Text {
                                        op: TextOp::AwaitCaret(watch),
                                        ..
                                    },
                                ..
                            },
                        ..
                    },
                ..
            } => Some(watch.pressed_at_ms),
            _ => self
                .facts
                .follow
                .caret_watch
                .as_ref()
                .map(|open| open.watch.pressed_at_ms),
        };
        self.intake.waiting_carets(pressed_ms)
    }
}

/// The caret event `item` is, by what names its node, if it is one: a
/// UIA caret event, the console's caret event, or an edit control's caret
/// or selection change.
fn waiting_caret(item: &Item) -> Option<WaitingCaret> {
    match item {
        Item::Uia(UiaEvent {
            kind: UiaKind::TextSelection,
            parts,
            ..
        }) => Some(WaitingCaret::Uia(parts.runtime_id.clone())),
        Item::Msaa {
            kind: WinKind::ConsoleCaret,
            hwnd,
            ..
        } => Some(WaitingCaret::Console(*hwnd)),
        Item::Msaa {
            kind: WinKind::Caret | WinKind::TextSelectionChange,
            hwnd,
            ..
        } => Some(WaitingCaret::Edit(*hwnd)),
        _ => None,
    }
}

/// The edit control whose client area's value change `item` is, if it is
/// one: what a cut caret read's check looks for ([`Cut`]).
fn edit_value_change(item: &Item) -> Option<isize> {
    match item {
        Item::Msaa {
            kind: WinKind::ValueChange,
            hwnd,
            id_object: intake::OBJID_CLIENT,
            id_child: 0,
            ..
        } => Some(*hwnd),
        _ => None,
    }
}

/// Whether handling `item` may end a held terminal's hold, so its read
/// must see every update observed after it begins: a caret key, and
/// Core's on-demand reading but for a change of text.
fn clears_absorb(item: &Item) -> bool {
    matches!(
        item,
        Item::Query {
            query: Query::Text {
                op: TextOp::AwaitCaret(_)
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

/// The deadline for an entry, and the query it answers, if it is one.
fn budget(entry: &Entry) -> (Duration, Option<(u64, TraceId)>) {
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::FocusNow => HANDLING_DEADLINE,
                Query::Ancestors { .. }
                | Query::DumpTree { .. }
                | Query::ShellItems { .. }
                | Query::ShellItemPosition { .. } => WALK_DEADLINE,
                Query::Text { .. } => TEXT_DEADLINE,
                _ => STEP_DEADLINE,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(..)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::NotedCaret { .. }
        | Item::Settle(_)
        | Item::IdleQuery { .. }
        | Item::Wake
        | Item::GapBound
        | Item::GapRead { .. } => (HANDLING_DEADLINE, None),
        // Releasing thousands of objects after a tree dump takes a while.
        Item::NodesHeld { .. } => (WALK_DEADLINE, None),
    }
}

/// The reply to query `request_id`, traced `trace`, ending as `outcome`.
fn reply(request_id: u64, trace: TraceId, outcome: QueryOutcome) -> OutpostToCore {
    OutpostToCore::Reply {
        trace_id: trace,
        request_id,
        outcome,
        timing: EventTiming::default(),
    }
}

/// The `WatchEnded` answer to `open`, published at `published_at_us`,
/// which Core says nothing for.
#[must_use]
pub fn watch_ended(open: &OpenWatch, published_at_us: u64) -> OutpostToCore {
    OutpostToCore::Reply {
        trace_id: open.trace,
        request_id: open.request_id,
        outcome: QueryOutcome::Done(verbatim_model::protocol::QueryResult::Text(
            verbatim_model::TextReply::WatchEnded,
        )),
        timing: EventTiming {
            calls: open.calls,
            published_at_us,
            ..open.timing
        },
    }
}
