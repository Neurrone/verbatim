//! The outpost's reducer (`books/dev/src/explanation/design/ownership.md`): the pure state machine that holds every
//! remembered fact of the outpost's application, with no Windows type and
//! no application object. Inputs come in mailbox order; effects go out.
//! The outpost's shell (`verbatim-outpost`'s `outpost::shell` module) runs
//! it on a thread of its own, applies its effects, and keeps its time. It is
//! a logic crate: in the GPL tier, with no Windows dependency, so it builds
//! and is tested on any host.
//!
//! It holds the intake's order ([`intake`]), the reports Core may hold ([`held`]), the follow state ([`follow`]),
//! the hand-over's hold ([`hold`]), the arbitrator's verdicts
//! ([`verdicts`]), the record of hung windows, the program in flight with the worker generation in
//! charge, its deadline and its moved-on rule, kept by the work in flight
//! both readers share ([`verbatim_reader_rules::in_flight`]), with its
//! hang mark beside it, the queries not yet answered, and the idle
//! barrier's round.
//! Programs, run by a worker on copies ([`Facts`]), make every call into
//! the application; their commits come back as inputs, and the reducer
//! applies a commit only while its request is live: the program in flight
//! of the generation in charge.
//!
//! A hang is a notice, never a reason to abandon: the shell's check of the
//! program in flight's window, every half second from its start, and of
//! the window in front while a listener read's marker has waited half a
//! second, give [`Input::HangSeen`], which tells Core once per hang
//! ([`OutpostToCore::NotResponding`]) and keeps the window told ([`Hang`]).
//! A program is abandoned only at its deadline or once the user moved on.
//!
//! What it never does: make a call of any kind, wait, read a clock, or
//! keep an object. What it needs of Windows' own state it asks for by an
//! effect, which the shell answers with an input before it takes another
//! message ([`Effect::AskJudge`], [`Effect::CheckFront`],
//! [`Effect::JudgeTold`], [`Effect::CheckHung`]). Time is the
//! shell's: a [`Now`] given with each input, and [`Reducer::next_timer`]
//! for the shell's timed wait.

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

mod entry;
mod follow;
mod held;
mod hold;
mod intake;
#[cfg(test)]
mod tests;
mod text;
mod verdicts;

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use verbatim_model::protocol::{
    DeliveredFact, EventTiming, FrontWindow, OutpostToCore, Query, QueryOutcome, QueryResult,
    RequestId,
};
use verbatim_model::{
    CallCounts, FocusGroup, KeyFetch, NodeId, NormalizedEvent, ReadPurpose, TextOp, TraceId,
};
use verbatim_reader_rules::Window;
use verbatim_reader_rules::in_flight::{Abandoned, Ask, Decision, InFlight, RuleWindow};

pub use self::entry::{
    Asker, Entry, Front, Item, Source, UiaEvent, UiaKind, WinKind, is_state_property,
};
pub use self::follow::{
    Applied, BarWatch, Change, DialogMemo, Facts, FocusCommit, FocusPath, FollowState, Followed,
    Group, LastReported, LogicError, MarkChange, MsaaReport, Origin, Owed, OwedProgram, OwedWork,
    Report, Route, Subscribed, Subscription, Target, apply_local,
};
pub use self::held::{Evidence, Focus, Held, InFront};
pub use self::intake::{Handling, Marks, Object, Planned, TerminalMark};
pub use self::text::{
    Anchor, AnswerCut, CutLine, FieldReads, KeptSelection, KeyNote, LastRead, NOTE_LIFE,
    NodeAnchors, NoteFetch, NoteOn, NoteReads, POLL_BOUND, POLL_INTERVAL, RangeKey, ReadSelection,
    ReportCut, TerminalReads, TextChange, TextFacts,
};
pub use self::verdicts::{Probe, Probed, VerdictChange, Verdicts};

/// The deadline of every program but a focus read and a listener read's
/// report: one UIA request and a margin
/// ([`verbatim_model::HANDLING_DEADLINE`]), one constant for the outpost
/// and Core.
pub use verbatim_model::HANDLING_DEADLINE;

/// The deadline of a focus read ([`Query::ReadFocus`]) and of a listener
/// read's report ([`verbatim_model::FOCUS_READ_DEADLINE`]).
pub use verbatim_model::FOCUS_READ_DEADLINE;

/// One request of one worker generation: what a program's commits and the
/// store's mint tags name it by.
pub use verbatim_reader_rules::in_flight::Request;

/// Why a program in flight was abandoned.
pub use verbatim_reader_rules::in_flight::Reason;

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
    Owed(OwedProgram),

    /// Drops the objects the shell hands it: what a release let go of.
    Drop,
    /// One read of the console poll, of this node under its note.
    NoteRead(NodeId),
    /// Finds the element of the foreground window for its progress bars
    /// ([`BarWatch`]).
    FindBar(isize),
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
    /// The window its entry names, which its worker names in its charge at
    /// once; `None` for a program whose worker names the window it calls
    /// into, or that is never judged by one.
    pub window: Option<Window>,
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
    /// shell counts the abandoned workers that have not returned, those
    /// parked on a window seen hung apart from the rest.
    Abandon {
        /// The generation marked dead.
        dead: u64,
        /// The generation started in its place.
        replacement: u64,
        /// Why the generation was abandoned, for the log.
        reason: &'static str,
        /// Whether it is parked on a hung window: its program's hang mark,
        /// set by an [`Input::HangSeen`] of its request.
        on_hung: bool,
        /// What the abandoned worker had in hand, for the log.
        in_hand: InHand,
    },
    /// The push-time copy of the facts' marks, given after a transition
    /// that changed them, ahead of the transition's other effects so that
    /// no program it dispatches runs before the mailbox judges by them.
    Marks(Marks),
    /// The push-time copy of the arbitrator's verdicts, replaced.
    Verdicts(Arc<Verdicts>),
    /// A commit asked for what cannot occur: applied as nothing, for the
    /// shell to record as the logic error it is.
    LogicError(LogicError),
    /// A test's settle, number `0`, was reached: the shell answers whoever
    /// waits on it.
    Settled(u64),
    /// A verdict of UIA for a window's lifetime was recorded: the event
    /// thread may narrow its hook (the outpost's `narrowing`).
    Narrow,
    /// Check `request`'s program by its window, `window`, or, for `None`,
    /// the window its worker named in its charge for that request, if any:
    /// whether the window is hung or ghosted, by the outpost's readability
    /// reader, answered by [`Input::HangSeen`] when it is, then whether
    /// the user has left it for another thread of its application,
    /// answered by [`Input::Judged`], both before the shell takes another
    /// message.
    AskJudge {
        /// The program in flight asked about.
        request: Request,
        /// Its window, `None` for one its worker names.
        window: Option<Window>,
    },
    /// Check the window in front while a listener read's marker has waited
    /// past the grace: the foreground window, a ghost mapped to the window
    /// it stands in for, and, when the outpost's application serves that
    /// window, its readability, answered by [`Input::HangSeen`] with no
    /// request when it is hung or ghosted and by [`Input::FrontReadable`]
    /// when it can be read, before the shell takes another message. Local
    /// calls only; it judges the hang, never the read.
    CheckFront,
    /// Judge the told window `window` by the outpost's one judge of whether
    /// a window is hung, a local call, answered by [`Input::ToldJudged`]
    /// before the shell takes another message.
    JudgeTold {
        /// The window judged.
        window: Window,
        /// The window found hung while `window` was told, which is told in
        /// its place if `window` is not hung.
        waiting: Option<Window>,
    },
    /// Confirm that a window a commit named hung is hung now, by the
    /// outpost's one judge, a local call, answered by [`Input::HangSeen`]
    /// with no request when it is, before the shell takes another message.
    CheckHung {
        /// The window the commit named.
        window: Window,
    },
}

/// Inputs, in mailbox order.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the mailbox into the intake, as it always has"
)]
pub enum Input {
    /// An entry from a producer, and where it came from.
    Entry {
        /// The entry.
        entry: Entry,
        /// Where it came from.
        source: Source,
    },
    /// The outpost's hook is in, and a listener forwards to it.
    Hooked,
    /// No listener forwards to this outpost.
    WithoutListener,
    /// The listener's "forwarding ended", through this tick.
    ForwardingEnded(u32),
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
    /// A test that waits out a program's deadline shortens it: entries
    /// dispatched from now on that would have [`HANDLING_DEADLINE`] have
    /// this one. Verbatim never sends it.
    HandlingDeadline(Duration),
    /// The shell's answer to [`Effect::AskJudge`]: whether the user has
    /// moved on from the program's window, which abandons it; never while
    /// no window is named.
    Judged {
        /// The program asked about.
        request: Request,
        /// Whether the user moved on.
        moved_on: bool,
    },
    /// A window the shell found hung or ghosted: by the check of program
    /// `request`'s window ([`Effect::AskJudge`]), or, with no request, in
    /// front while a marker waits ([`Effect::CheckFront`]) or named hung by
    /// a commit ([`Effect::CheckHung`]). It is told as [`Hang`] says, and
    /// sets the program's hang mark when `request` is the program in
    /// flight; nothing is abandoned.
    HangSeen {
        /// The window found hung.
        window: Window,
        /// The program whose window it is, when its check found it.
        request: Option<Request>,
    },
    /// The shell's answer to [`Effect::CheckFront`] for a window of the
    /// outpost's application in front that can be read: if it is the
    /// window told, its hang has ended.
    FrontReadable {
        /// The window in front.
        window: Window,
    },
    /// The shell's answer to [`Effect::JudgeTold`], which brings back the
    /// effect's two windows. An answer for a window no longer told changes
    /// nothing.
    ToldJudged {
        /// The window judged.
        window: Window,
        /// The window waiting on the answer, if any.
        waiting: Option<Window>,
        /// Whether `window` is hung.
        hung: bool,
    },
    /// The outpost is shutting down.
    Close,
}

/// What a program had in hand, which the log names when its worker is
/// abandoned: the work, the entry's trace and the window it concerns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InHand {
    /// What the program does: the kind of entry it handles, or its other
    /// work.
    pub work: &'static str,
    /// The trace of the entry it handles, when it handles one.
    pub trace: Option<TraceId>,
    /// The window whose application it waits on, when it is known.
    pub window: Option<isize>,
}

impl InHand {
    /// What a program doing `work`, concerning `window`, has in hand.
    fn of(work: &Work, window: Option<Window>) -> Self {
        let (work, trace) = match work {
            Work::Entry { entry, .. } => (
                match entry.item {
                    Item::Msaa { .. } => "an MSAA event",
                    Item::Uia(_) => "a UIA event",
                    Item::Fact(..) => "a fact such as a focus or a foreground change",
                    Item::Query { .. } => "a query from Core",
                    Item::NodesHeld { .. } => "the nodes Core holds",
                    Item::NotedCaret { .. } => "a noted caret",
                    Item::ReadTerminal { .. } => "a terminal's read after a key",
                    Item::Barrier { .. } => "the idle barrier",
                },
                Some(entry.trace),
            ),
            Work::Owed(_) => ("the work the followed focus owes", None),
            Work::Drop => ("the drop of released objects", None),
            Work::NoteRead(_) => ("a read of the console poll", None),
            Work::FindBar(_) => ("the foreground window's element", None),
            Work::Finish { .. } => ("the finish of a cut caret read", None),
        };
        Self {
            work,
            trace,
            window: window.map(Window::get),
        }
    }
}

/// What the reducer keeps of the program in flight, beside what the work
/// in flight keeps of it (its request, times, walk and moved-on rule).
#[derive(Clone, Debug)]
struct Current {
    /// The query the program answers, if it answers one, which an
    /// abandonment or a program that ends without answering answers.
    running: Option<(RequestId, TraceId)>,
    /// Core's live re-read its entry answers, if any, until a commit sends
    /// the answer: one the program ends or is abandoned without answering
    /// is answered gone, so a pending live re-read always ends.
    live_reread: Option<u64>,
    /// The focus group try it is, if it is one.
    group: Option<GroupTry>,
    /// Whether it does work the idle barrier's answer waits for: a read of
    /// the console poll, which may still read the terminal under the key's
    /// trace.
    deferred: bool,
    /// What the program has in hand, for the log of its abandonment.
    in_hand: InHand,
    /// Whether the user moving on from its window abandons it: not for a
    /// query that is not about the focus, a tree dump or a shell item
    /// query, which is bounded by its deadline alone, as NVDA's watchdog
    /// judges a moved-on user against the focus object only
    /// (`docs/nvda/main-loop-and-watchdog.md`). Its window is still checked
    /// for a hang.
    moved_on_abandons: bool,
    /// Its hang mark: an [`Input::HangSeen`] of its request found its
    /// window hung, so its worker, once abandoned, counts as parked on a
    /// hung window ([`Effect::Abandon`]'s `on_hung`). [`Hang`] cannot tell
    /// it at abandonment, since a program whose worker names its window has
    /// no window to compare with the one told.
    hang_mark: bool,
}

/// One try of a focus group, and what is left of it.
#[derive(Clone, Debug)]
struct GroupTry {
    number: u64,
    rank: u32,
    batch: u64,
    older: VecDeque<Entry>,
}

/// The outpost's record of hung windows: one value per outpost, ended with
/// its generation. One hang is told per application, so a second window seen
/// hung while the told window is still hung is the same hang; a later hang in
/// another window, which a window still hung when the told window recovers
/// is, is told again and replaces it (Dickson, 2026-11-07, 2026-11-08). A
/// told window found not hung ends the hang, so its later freeze is a new
/// hang (Dickson, 2026-11-09).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Hang {
    /// No hang is being told.
    #[default]
    None,
    /// `window`'s hang was told to Core.
    Told {
        /// The window told.
        window: Window,
    },
}

/// The reducer.
#[cfg_attr(test, derive(Clone, Debug))]
#[derive(Default)]
pub struct Reducer {
    intake: intake::Intake,
    hold: hold::Hold,
    facts: Facts,
    /// The program in flight, and the worker generation in charge.
    flight: InFlight<Current>,
    /// The tries left of a focus group whose try was abandoned, to run
    /// before anything else.
    after: VecDeque<(Entry, Option<GroupTry>)>,
    /// Whether a release let go of objects for a worker to drop.
    to_drop: bool,
    /// The queries recorded and not yet answered.
    queries: BTreeSet<RequestId>,
    /// The idle barrier's round reached and not yet answered.
    idle_query: Option<u64>,
    /// How many focus groups have been tried.
    focus_groups: u64,
    /// A test's deadline in place of [`HANDLING_DEADLINE`]
    /// ([`Input::HandlingDeadline`]).
    handling_deadline: Option<Duration>,
    /// Whether a release was asked for and its [`Input::Released`] has not
    /// come: nothing is dispatched until it has, so no program is given a
    /// node it frees.
    releasing: bool,
    stopping: bool,
    /// The hang told to Core ([`OutpostToCore::NotResponding`], sent only
    /// by this record), judged live while it lasts on each input that takes
    /// an entry, applies a commit or starts a program. Nothing is told after
    /// [`Input::Close`].
    hang: Hang,
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

    /// The deadline for handling an event, a focus or a focus-now query:
    /// [`HANDLING_DEADLINE`], or a test's in its place.
    fn handling_deadline(&self) -> Duration {
        self.handling_deadline.unwrap_or(HANDLING_DEADLINE)
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

    /// When the shell is next to call [`tick`](Self::tick): what the
    /// program in flight next has due (its deadline or its next check),
    /// the front check of the first marker waiting for its read, and the
    /// console poll's due time while no program is in flight, no release
    /// is pending and the outpost is not stopping (only then is it acted
    /// on), on the shell's clock.
    #[must_use]
    pub fn next_timer(&self, now: Now) -> Option<Duration> {
        let mut times = Vec::new();
        if !self.stopping {
            if let Some(due) = self.flight.next_wake() {
                times.push(Duration::from_micros(due));
            }
            times.extend(self.intake.front_due());
        }
        if !self.busy()
            && !self.releasing
            && !self.stopping
            && let Some(due_us) = self.facts.text.poll_due()
        {
            let left = due_us.saturating_sub(now.wall_us);
            // A time past the clock's range never comes.
            times.extend(now.at.checked_add(Duration::from_micros(left)));
        }
        times.into_iter().min()
    }

    /// Takes `input` at `now`.
    pub fn input(&mut self, input: Input, now: Now) -> Vec<Effect> {
        let before = self.observed();
        let mut effects = Vec::new();
        let mut taken = false;
        match input {
            Input::Entry { entry, source } => {
                taken = true;
                self.entry(entry, source, now, &mut effects);
            }
            Input::Hooked => self.hold.hooked(),
            Input::WithoutListener => {
                let held = self.hold.without_listener();
                self.push_all(held, now, &mut effects);
            }
            Input::ForwardingEnded(through) => {
                if let Some(released) = self.hold.forwarding_ended(through) {
                    self.push_all(released, now, &mut effects);
                }
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
                self.record_verdict(generation, &change, &mut effects);
            }
            Input::Commit { request, change } => {
                if self.accepts(request) {
                    taken = true;
                    self.commit(request, &change, now, &mut effects);
                }
            }
            Input::Finished {
                request,
                reported,
                panicked,
                cut,
            } => self.finished(request, (reported, panicked), cut, &mut effects),
            Input::Released {
                nodes,
                anchors,
                objects,
            } => self.released(&nodes, &anchors, objects, &mut effects),
            Input::HandlingDeadline(deadline) => self.handling_deadline = Some(deadline),
            Input::Judged { request, moved_on } => {
                if !self.stopping {
                    let abandons = self
                        .current_of(request)
                        .is_some_and(|current| current.moved_on_abandons);
                    let decision = self
                        .flight
                        .judged(request, (moved_on && abandons).then_some(Reason::MovedOn));
                    self.decided(decision, &mut effects);
                }
            }
            Input::HangSeen { window, request } => {
                if !self.stopping {
                    if let Some(current) = request.and_then(|request| self.current_of(request)) {
                        current.hang_mark = true;
                    }
                    self.hang_seen(window, &mut effects);
                }
            }
            Input::FrontReadable { window } => self.hang_ended(window),
            Input::ToldJudged {
                window,
                waiting,
                hung,
            } => self.told_judged(window, waiting, hung, &mut effects),
            Input::Close => {
                self.stopping = true;
                self.hang = Hang::None;
                let unparked = self.intake.close();
                if !unparked.is_empty() {
                    effects.push(Effect::Unpark(unparked));
                }
            }
        }
        self.pump(now, &mut effects);
        self.answer_idle(&mut effects);
        self.judge_told(taken, &mut effects);
        self.changed(&before, &mut effects);
        effects
    }

    /// The shell's timed wait has ended at `now`: whatever is due is done.
    pub fn tick(&mut self, now: Now) -> Vec<Effect> {
        let before = self.observed();
        let mut effects = Vec::new();
        if !self.stopping {
            let decision = self.flight.tick(micros(now.at));
            self.decided(decision, &mut effects);
            if self.intake.front_check(now.at) {
                effects.push(Effect::CheckFront);
            }
        }
        self.pump(now, &mut effects);
        self.answer_idle(&mut effects);
        self.judge_told(false, &mut effects);
        self.changed(&before, &mut effects);
        effects
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

    /// Takes in `entry`, from `source`: held while the hand-over's hold
    /// lasts if it came from the hook, and pushed into the intake
    /// otherwise.
    fn entry(&mut self, entry: Entry, source: Source, now: Now, effects: &mut Vec<Effect>) {
        self.destroy_seen(&entry.item);
        if self.key_arrived(&entry, now) {
            return;
        }
        match source {
            Source::Hook => {
                if let Some(entry) = self.hold.hooked_copy(entry) {
                    self.push(entry, now, effects);
                }
            }
            Source::Listener => {
                self.hold.listener_copy(&entry);
                self.push(entry, now, effects);
            }
            Source::Other => {
                let ends_reads = matches!(
                    entry.item,
                    Item::Fact(
                        verbatim_model::protocol::DeliveredFact::UiaFocusReadsEnded,
                        _
                    )
                );
                self.push(entry, now, effects);
                if ends_reads && let Some(released) = self.hold.reads_ended() {
                    self.push_all(released, now, effects);
                }
            }
        }
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
                self.queries.insert(*request_id);
                self.after.push_back((entry.clone(), None));
                return true;
            }
        };
        Arc::make_mut(&mut self.facts.text).set_note(node_id.number(), read.fetch, now.wall_us, on);
        false
    }

    /// A destroyed window answers nothing more: a hang told of it ends.
    /// Applied as the destroy first reaches the reducer, from either
    /// source, even as a hook copy the hand-over's hold keeps back; a second
    /// copy changes nothing.
    fn destroy_seen(&mut self, item: &Item) {
        if let Item::Msaa {
            kind: WinKind::Destroy,
            hwnd,
            ..
        } = *item
            && let Some(window) = Window::new(hwnd)
        {
            self.hang_ended(window);
        }
    }

    /// Pushes `entries` into the intake in order, as one batch.
    fn push_all(&mut self, entries: Vec<Entry>, now: Now, effects: &mut Vec<Effect>) {
        for entry in entries {
            self.push(entry, now, effects);
        }
    }

    /// Pushes `entry` into the intake at `now`, recording a query.
    fn push(&mut self, entry: Entry, now: Now, effects: &mut Vec<Effect>) {
        if let Item::Query { request_id, .. } = entry.item
            && !self.stopping
        {
            self.queries.insert(request_id);
        }
        let pushed = self.intake.push(entry, &self.facts.marks(), now.at);
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

    /// `window` was found hung: told if no hang is, the same hang if it is
    /// the window told, and otherwise a new one only if the told window is
    /// found not hung.
    fn hang_seen(&mut self, window: Window, effects: &mut Vec<Effect>) {
        match self.hang {
            Hang::None => self.tell_hang(window, effects),
            Hang::Told { window: told } if told == window => {}
            Hang::Told { window: told } => effects.push(Effect::JudgeTold {
                window: told,
                waiting: Some(window),
            }),
        }
    }

    /// Tells Core of `window`'s hang, which becomes the hang told.
    fn tell_hang(&mut self, window: Window, effects: &mut Vec<Effect>) {
        self.hang = Hang::Told { window };
        self.send(
            OutpostToCore::NotResponding {
                window: window.get(),
            },
            effects,
        );
    }

    /// A hang told of `window` has ended; one of another window has not.
    fn hang_ended(&mut self, window: Window) {
        if self.hang == (Hang::Told { window }) {
            self.hang = Hang::None;
        }
    }

    /// Asks once that the told window be judged, with nothing waiting on
    /// the answer, on an input where an entry is `taken` from the mailbox, a
    /// commit applied or a program started. A cut read's finish continues
    /// the program already started, so it is not a program started.
    fn judge_told(&self, taken: bool, effects: &mut Vec<Effect>) {
        let started = effects.iter().any(
            |effect| matches!(effect, Effect::Dispatch(program) if !matches!(program.work, Work::Finish { .. })),
        );
        if let Hang::Told { window } = self.hang
            && (taken || started)
        {
            effects.push(Effect::JudgeTold {
                window,
                waiting: None,
            });
        }
    }

    /// The judgement of `window`, told when it was asked: not hung ends its
    /// hang, and the window waiting, if any, is a new one.
    fn told_judged(
        &mut self,
        window: Window,
        waiting: Option<Window>,
        hung: bool,
        effects: &mut Vec<Effect>,
    ) {
        if hung || self.hang != (Hang::Told { window }) {
            return;
        }
        match waiting {
            Some(waiting) => self.tell_hang(waiting, effects),
            None => self.hang = Hang::None,
        }
    }

    /// Applies the commit of `request`'s program: its report recorded for
    /// Core's judgement, then its messages sent, then the rest.
    fn commit(&mut self, request: Request, change: &Change, now: Now, effects: &mut Vec<Effect>) {
        if let Some(report) = &change.report {
            follow::record(&mut self.facts.held, report);
        }
        if let Some(mut message) = change.result.clone() {
            self.attach_note_facts(&mut message, now);
            if let Some(current) = self.current_of(request) {
                current.running = None;
                if current.live_reread.is_some()
                    && current.live_reread == live_reread_answer(&message)
                {
                    current.live_reread = None;
                }
            }
            self.send(message, effects);
        }
        // A window named hung is never taken at the commit's word: its call
        // may have begun before a hang or a destroy, and failed after it.
        // Asked before the told window's judgement, which ends the input,
        // so an answer that replaces the told window comes first.
        if let Some(window) = change.found
            && !self.stopping
        {
            effects.push(Effect::CheckHung { window });
        }
        if let Some((node_id, trace, observed_at_us, timing)) = change.noted_caret {
            // The event's timing goes with it: its read is skipped only
            // when a caret read since the event covers it.
            self.intake.when_idle(Entry {
                item: Item::NotedCaret { node_id },
                trace,
                observed_at_us,
                timing,
                front: crate::Front::Untested,
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
        let OutpostToCore::Event {
            event:
                NormalizedEvent::CaretMoved {
                    node_id,
                    caret,
                    selection_changes,
                    facts,
                    ..
                },
            ..
        } = message
        else {
            return;
        };
        if facts.is_some()
            || self
                .facts
                .text
                .live_note(node_id.number(), now.wall_us)
                .is_none()
        {
            return;
        }
        *facts = Arc::make_mut(&mut self.facts.text)
            .facts_at_commit(
                node_id.number(),
                (caret, !selection_changes.is_empty()),
                now.wall_us,
            )
            .map(Box::new);
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
        if generation != self.generation() && !change.kept_from_any_generation() {
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
        self.facts.held.release(nodes);
        let ranges = Arc::make_mut(&mut self.facts.text).release(nodes, anchors);
        if !ranges.is_empty() {
            effects.push(Effect::ForgetRanges(ranges));
        }
        let mut applied = Applied::default();
        self.facts.follow.release_groups(nodes, &mut applied);
        Self::subscription_effects(applied, effects);
        self.to_drop |= objects;
    }

    /// Does what the work in flight decided at `now`: asks the shell, or
    /// carries out an abandonment.
    fn decided(&mut self, decision: Decision<Current>, effects: &mut Vec<Effect>) {
        match decision {
            Decision::Nothing => {}
            Decision::Ask(Ask::Judge { request, window }) => {
                effects.push(Effect::AskJudge { request, window });
            }
            Decision::Abandon(abandoned) => self.abandoned(abandoned, effects),
        }
    }

    /// The worker in charge was abandoned: its replacement is started, and
    /// the program's query, the live re-read its entry answers, the owed
    /// work it attempted are answered.
    fn abandoned(&mut self, abandoned: Abandoned<Current>, effects: &mut Vec<Effect>) {
        let Abandoned {
            dead,
            replacement,
            reason,
            item: current,
            ..
        } = abandoned;
        let in_hand = current.in_hand;
        self.facts.follow.bar_program_ended();
        if let Some(followed) = self.facts.follow.followed.as_mut()
            && followed
                .owed
                .as_ref()
                .is_some_and(|owed| owed.attempting == Some(dead))
        {
            followed.owed = None;
        }
        effects.push(Effect::Abandon {
            dead,
            replacement,
            reason: match reason {
                Reason::Deadline => "a call passed its deadline",
                Reason::MovedOn => "the user moved on from a slow window",
            },
            on_hung: current.hang_mark,
            in_hand,
        });
        if let Some((request_id, trace)) = current.running {
            self.send(reply(request_id, trace, QueryOutcome::Abandoned), effects);
        }
        if let Some(request) = current.live_reread {
            self.send(OutpostToCore::LiveRereadGone { request }, effects);
        }
        // A focus group's older entries are tried by the replacement.
        if let Some(group) = current.group {
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
        let Some(current) = self.flight.end(request) else {
            return;
        };
        self.facts.follow.bar_program_ended();
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
        if let Some(request) = current.live_reread {
            self.send(OutpostToCore::LiveRereadGone { request }, effects);
        }
        if let Some(group) = current.group
            && !reported
        {
            self.continue_group(group);
        }
    }

    /// Queues the next older entry of `group`, if it has one.
    fn continue_group(&mut self, mut group: GroupTry) {
        if let Some(entry) = group.older.pop_front() {
            group.rank = group.rank.saturating_add(1);
            self.after.push_back((entry, Some(group)));
        }
    }

    /// Answers the idle barrier's query once no deferred work is pending:
    /// the hook's copies are not held at the start and the program in
    /// flight is not a read of the console poll. The program
    /// in flight is otherwise not waited for:
    /// the query is reached only with none in flight, so a program in
    /// flight then handles an entry that arrived after the query.
    fn answer_idle(&mut self, effects: &mut Vec<Effect>) {
        if self.hold.holding()
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
            window: None,
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
                self.dispatch(
                    Work::Owed(OwedProgram { node, owed }),
                    (self.handling_deadline(), None, None),
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
            if let Some(window) = self.facts.follow.bar.due() {
                self.facts.follow.bar = BarWatch::Finding { window };
                let deadline = self.handling_deadline();
                self.dispatch(
                    Work::FindBar(window),
                    (deadline, Window::new(window), None),
                    None,
                    now,
                    effects,
                );
                continue;
            }
            if std::mem::take(&mut self.to_drop) {
                let deadline = self.handling_deadline();
                self.dispatch(Work::Drop, (deadline, None, None), None, now, effects);
                continue;
            }
            let Some((planned, batch)) = self.intake.next(&self.facts.marks()) else {
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
                self.dispatch(
                    Work::NoteRead(NodeId::new(node)),
                    (self.handling_deadline(), None, None),
                    None,
                    now,
                    effects,
                );
                if let Some(current) = self.flight.item_mut() {
                    current.deferred = true;
                }
                continue;
            };
            match planned {
                Planned::Run(entry) => self.run(entry, batch, now, effects),
                Planned::Menu(entry) => {
                    self.dispatch_entry(entry, Handling::Menu, batch, None, now, effects);
                }
                Planned::Focus(entries) => {
                    self.focus_groups = self.focus_groups.saturating_add(1);
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
            _ => self.dispatch_entry(entry, Handling::Run, batch, None, now, effects),
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
    fn dispatch_entry(
        &mut self,
        entry: Entry,
        handling: Handling,
        batch: u64,
        group: Option<GroupTry>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        let (deadline, running) = budget(&entry, now, self.handling_deadline());
        let window = intake::window_of(&entry.item);
        // Anything that may end a held terminal's hold reads every update
        // observed after it begins.
        if clears_absorb(&entry.item) {
            self.facts.follow.absorbed = None;
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

    /// Dispatches `work` under `deadline`, concerning `window`, if any,
    /// and answering `running`, as a try of `group` when it is one. Verbatim
    /// moving the caret or selecting on a node clears its note, before its
    /// copy is taken.
    fn dispatch(
        &mut self,
        work: Work,
        (deadline, window, running): (Duration, Option<Window>, Option<(RequestId, TraceId)>),
        group: Option<GroupTry>,
        now: Now,
        effects: &mut Vec<Effect>,
    ) {
        if let Some(moved) = verbatims_caret_move(&work) {
            Arc::make_mut(&mut self.facts.text).clear_note(moved.number());
        }
        let live_reread = match &work {
            Work::Entry { entry, .. } => live_reread_answered_by(&entry.item),
            Work::Owed(_)
            | Work::Drop
            | Work::NoteRead(_)
            | Work::Finish { .. }
            | Work::FindBar(_) => None,
        };
        let current = Current {
            running,
            live_reread,
            group,
            deferred: false,
            in_hand: InHand::of(&work, window),
            hang_mark: false,
            moved_on_abandons: !is_query_off_the_focus(&work),
        };
        // A program with no window at dispatch that calls into the
        // application has its worker name the window it calls into, a tree
        // dump its application's main window and a shell item query the
        // taskbar's; a release's drop, which calls into many windows, is
        // judged by its deadline alone, so no wake judges a window never
        // named.
        let rule = match (window, &work) {
            (Some(window), _) => RuleWindow::Known(window),
            (None, Work::Drop | Work::FindBar(_)) => RuleWindow::Never,
            (
                None,
                Work::Entry { .. } | Work::Owed(_) | Work::NoteRead(_) | Work::Finish { .. },
            ) => RuleWindow::Named,
        };
        let request = self
            .flight
            .start(current, micros(deadline), rule, micros(now.at));
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            window,
            facts: self.facts.clone(),
            work,
        })));
    }

    /// Dispatches the program that finishes `cut`, the caret read the
    /// program in flight cut at its value change check, before anything
    /// else and even while the outpost shuts down, as the read's program
    /// would have gone on: handed on to a new request, it takes over that
    /// program's deadline, its walk, its moved-on rule and the query it
    /// answers. The check is decided here: every
    /// value change the control raised before the read part's sync is
    /// ahead of its end in the mailbox, so among the entries not yet
    /// dispatched, in the intake or held.
    fn dispatch_finish(&mut self, cut: Box<Cut>, effects: &mut Vec<Effect>) {
        let waiting = self.intake.value_change_waiting(cut.edit)
            || self
                .hold
                .held()
                .any(|entry| edit_value_change(&entry.item) == Some(cut.edit));
        let Some(request) = self.flight.hand_on() else {
            return;
        };
        effects.push(Effect::Dispatch(Box::new(Program {
            request,
            window: Window::new(cut.edit),
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

/// `duration` in microseconds, the work in flight's times.
fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// The deadline for an entry begun at `now`, and the query it answers, if
/// it is one, `handling` being [`HANDLING_DEADLINE`] or a test's in its
/// place: `handling` for every entry but a focus read and a listener
/// read's report, which have the two seconds [`FOCUS_READ_DEADLINE`] adds
/// to [`HANDLING_DEADLINE`] more ([`FOCUS_READ_DEADLINE`] when `handling`
/// is [`HANDLING_DEADLINE`]). A shell item query's deadline is the one it
/// carries, a due time on the shared clock fixed when the dialog asked, so
/// a query that waited behind other work has only what is left of it.
fn budget(entry: &Entry, now: Now, handling: Duration) -> (Duration, Option<(RequestId, TraceId)>) {
    let focus_read = handling.saturating_add(FOCUS_READ_DEADLINE.saturating_sub(HANDLING_DEADLINE));
    match &entry.item {
        Item::Query { request_id, query } => {
            let deadline = match query {
                Query::ReadFocus { .. } => focus_read,
                Query::ShellItems { answer_by_us, .. }
                | Query::ShellItemPosition { answer_by_us, .. } => {
                    Duration::from_micros(answer_by_us.saturating_sub(now.wall_us))
                }
                _ => handling,
            };
            (deadline, Some((*request_id, entry.trace)))
        }
        Item::Fact(DeliveredFact::UiaFocusRead { .. }, _) => (focus_read, None),
        Item::Fact(..)
        | Item::Msaa { .. }
        | Item::Uia(_)
        | Item::NotedCaret { .. }
        | Item::ReadTerminal { .. }
        | Item::Barrier { .. }
        | Item::NodesHeld { .. } => (handling, None),
    }
}

/// Whether `work` is a query that is not about the focus: a tree dump or a
/// shell item query, which the user moving on never abandons.
fn is_query_off_the_focus(work: &Work) -> bool {
    matches!(
        work,
        Work::Entry { entry, .. } if matches!(
            &entry.item,
            Item::Query {
                query: Query::DumpTree { .. }
                    | Query::ShellItems { .. }
                    | Query::ShellItemPosition { .. },
                ..
            }
        )
    )
}

/// Core's live re-read the program handling `item` answers: the listener's
/// read answering it or its answer of gone, or Core's live re-read of an
/// MSAA report; `None` for any other entry.
fn live_reread_answered_by(item: &Item) -> Option<u64> {
    match item {
        Item::Fact(DeliveredFact::UiaFocusRead { read, .. }, _) => read.answers,
        Item::Fact(
            DeliveredFact::UiaLiveRereadGone { request }
            | DeliveredFact::MsaaLiveReread { request, .. },
            _,
        ) => Some(*request),
        _ => None,
    }
}

/// The live re-read `message` answers: a focus report carrying its
/// request, or its answer of gone.
fn live_reread_answer(message: &OutpostToCore) -> Option<u64> {
    match message {
        OutpostToCore::Event {
            event: verbatim_model::NormalizedEvent::FocusChanged { answers, .. },
            ..
        } => *answers,
        OutpostToCore::LiveRereadGone { request } => Some(*request),
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
