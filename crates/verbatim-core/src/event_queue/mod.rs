//! Core's event queue, its async driver and its request table (ADR 0003,
//! "Readers tell, Core asks"; `books/dev/src/reference/crates/verbatim-core.md`,
//! "Core's requests").
//!
//! The queue is one line of entries, of which only the front may have a
//! request out. Its function is an async function run on Core's thread
//! under five rules: only a request to a reader can be awaited; each
//! request names the type of its answer, which the queue decodes once;
//! every request gets exactly one outcome, so no paused function outlives
//! its entry; the recorder's checkpoint is the state at the front entry's
//! start plus the inputs since, so replay stays exact; and a state dump
//! shows each entry, the answers it has had and the request out.
//!
//! What an entry is and what its function does are the [`Logic`]'s; the
//! queue holds only what crosses features: the front, the waiting entries,
//! the requests and the one judge of their endings ([`Logic::ends`]).

// A borrow of the shared state held across an await would be taken again
// by the driver when the function's request ends.
#![deny(clippy::await_holding_refcell_ref)]

mod driver;
mod inbox;
mod limiter;
#[cfg(test)]
mod limiter_tests;
mod outposts;
pub(crate) mod pairs;
mod request;
mod table;
#[cfg(test)]
mod tests;

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use verbatim_model::{Backend, Effect, Pid, WindowHandle};

use crate::focus::backend::{BackendCache, JudgedWindow, WindowKey};
use crate::now::Now;

pub(crate) use driver::{Core, Cx, Queue, ReplayMismatch, State};
pub(crate) use inbox::{CoreRequest, Input, Timed};
pub(crate) use limiter::{Item, MsaaClass, MsaaKey, PUMP_WINDOW_US, Place};
#[cfg(test)]
pub(crate) use request::Stamps;
pub(crate) use request::{Outcome, Ran, Refused, Request};
pub(crate) use table::{Asker, Live};

/// A focus read's due time: 13 seconds from the moment Core decides to
/// ask, for its whole chain of requests (ADR 0003, "A frozen window").
pub(crate) const FOCUS_DUE_US: u64 = 13_000_000;
/// Any other request's due time, or a command's whole chain's: 11 seconds.
pub(crate) const OTHER_DUE_US: u64 = 11_000_000;
/// How often the shell checks a request's window for a hang while it is
/// out, and a recovering window's (ADR 0003, "A frozen window").
pub(crate) const HANG_ROUND_US: u64 = 500_000;

const _: () = assert!(verbatim_model::FOCUS_READ_DEADLINE.as_micros() == 13_000_000);
const _: () = assert!(verbatim_model::HANDLING_DEADLINE.as_micros() == 11_000_000);

pub(crate) use verbatim_model::request::{Instance, Reader};

/// Who answers a request: a reader process, or Core's plain reader, which
/// answers a request on a plain node at once, from Core's own facts, with
/// no call into any application (`crate::focus::plain`). Chosen where
/// every request begins, from the node the request names, so a request
/// from inside the event queue and one from outside it are routed alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Route {
    /// A reader process.
    Reader(Reader),
    /// Core's plain reader.
    Plain,
}

/// The window a request names, with what the foreground test and the
/// hang sound read of it: its top-level window and root owner, and the
/// application it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct RequestWindow {
    /// The window.
    pub(crate) handle: WindowHandle,
    /// Its top-level window.
    pub(crate) top_level: WindowHandle,
    /// The top of its owner chain.
    pub(crate) root_owner: WindowHandle,
    /// The application it belongs to.
    pub(crate) process: Pid,
    /// The thread that owns it, which keys its backend verdict.
    pub(crate) thread: u32,
    /// The backend its classes give, judged once at the tell's boundary;
    /// `None` when they give none and probes decide.
    pub(crate) by_class: Option<Backend>,
}

impl RequestWindow {
    /// The window as an answer reports a node's nearest window.
    pub(crate) fn node_window(&self) -> verbatim_model::request::NodeWindow {
        verbatim_model::request::NodeWindow {
            handle: self.handle,
            top_level: self.top_level,
            root_owner: self.root_owner,
            process: Some(self.process),
            thread: self.thread,
            desktop: false,
        }
    }

    /// The window as a caret key's binding names it.
    pub(crate) fn served_window(&self) -> verbatim_model::request::ServedWindow {
        verbatim_model::request::ServedWindow {
            handle: self.handle,
            top_level: self.top_level,
            root_owner: self.root_owner,
            process: self.process,
            thread: self.thread,
        }
    }

    /// The window a caret key's binding names, its classes not read, as
    /// [`RequestWindow::of_node`] names a node's.
    pub(crate) fn of_served(window: &verbatim_model::request::ServedWindow) -> Self {
        Self {
            handle: window.handle,
            top_level: window.top_level,
            root_owner: window.root_owner,
            process: window.process,
            thread: window.thread,
            by_class: None,
        }
    }

    /// The window a request on a node an answer read names: the node's
    /// nearest window with the facts the reader read of it. Its classes are
    /// not read, so probes decide its backend; a request on a node an
    /// outpost holds is read through that node's own backend. `None` for a
    /// window no process could be read as serving, on whose nodes no
    /// reader can be asked.
    pub(crate) fn of_node(window: &verbatim_model::request::NodeWindow) -> Option<Self> {
        Some(Self {
            handle: window.handle,
            top_level: window.top_level,
            root_owner: window.root_owner,
            process: window.process?,
            thread: window.thread,
            by_class: None,
        })
    }

    /// The window as the backend cache judges it.
    pub(crate) fn judged(&self) -> JudgedWindow {
        JudgedWindow {
            key: WindowKey {
                handle: self.handle.0,
                thread: self.thread,
            },
            by_class: self.by_class,
        }
    }
}

/// The foreground record: the window the listener last reported in front.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Foreground {
    /// The window.
    pub(crate) window: WindowHandle,
    /// The top of its owner chain.
    pub(crate) root_owner: WindowHandle,
    /// The application holding attention, the process serving the window;
    /// `None` when none could be read, so that no outpost is started for
    /// it and no reader asked about it (Claude, 2026-11-27).
    pub(crate) process: Option<Pid>,
    /// The thread that owns the window.
    pub(crate) thread: u32,
    /// The backend its class gives, `None` when probes decide.
    pub(crate) by_class: Option<Backend>,
    /// The ghost window in front when one stands in for `window`, which
    /// is the hung window: the keyboard hook's window.
    pub(crate) ghost: Option<WindowHandle>,
    /// Whether the window in front is a console host window, whose keys
    /// are translated with `thread`'s keyboard layout.
    pub(crate) console: bool,
    /// The integrity level of the window's process, `None` when unread.
    pub(crate) integrity: Option<u32>,
    /// Verbatim's own integrity level, `None` when unread.
    pub(crate) own_integrity: Option<u32>,
}

#[cfg(test)]
impl Foreground {
    /// A report's keyboard-hook facts left at their plainest, for a test's
    /// report to take with `..Foreground::BLANK`: no ghost, no console
    /// window, integrity unread.
    pub(crate) const BLANK: Self = Self {
        window: WindowHandle(0),
        root_owner: WindowHandle(0),
        process: None,
        thread: 0,
        by_class: None,
        ghost: None,
        console: false,
        integrity: None,
        own_integrity: None,
    };
}

impl Foreground {
    /// Whether `window` is in this foreground window's tree or owner chain:
    /// its top-level window or root owner is the foreground window or the
    /// foreground's root owner.
    pub(crate) fn holds(&self, window: &RequestWindow) -> bool {
        [window.top_level, window.root_owner]
            .iter()
            .any(|handle| *handle == self.window || *handle == self.root_owner)
    }

    /// The foreground window as a request names it; `None` when no
    /// process could be read as serving it, so no reader can be asked.
    pub(crate) fn window(&self) -> Option<RequestWindow> {
        Some(RequestWindow {
            handle: self.window,
            top_level: self.window,
            root_owner: self.root_owner,
            process: self.process?,
            thread: self.thread,
            by_class: self.by_class,
        })
    }

    /// Whether `window`'s application holds attention: it owns the
    /// foreground window, or `window` is inside the foreground window, as a
    /// Settings page's content (`SystemSettings`) is inside
    /// `ApplicationFrameHost`'s frame, which the foreground names
    /// (`docs/parity/focus-events.md`, "Event acceptance").
    pub(crate) fn attends(&self, window: &RequestWindow) -> bool {
        self.process == Some(window.process) || window.top_level == self.window
    }
}

/// What the front's request is judged for ([`Logic::ends`]).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Cause<'a, E> {
    /// `entry` arrived at the back, before the limiter places it.
    Arrived(&'a E),
    /// The foreground record changed, to the record the judge is given.
    Foreground,
    /// The queue settled after Core's facts changed outside it.
    Settled,
}

/// An entry's function, as the driver runs it.
pub(crate) type Function = Pin<Box<dyn Future<Output = ()>>>;

/// What the queue's entries are and what their functions do: Core's
/// features, which the queue knows only through this.
pub(crate) trait Logic: Sized + 'static {
    /// An entry of the queue.
    type Entry: Clone + Debug + PartialEq;
    /// Core's facts that the functions and the rules below read and write.
    type Facts: Clone + Debug + PartialEq + Default;

    /// The function of `entry`, run when it reaches the front.
    fn run(cx: Cx<Self>, entry: Self::Entry) -> Function;

    /// The listener's told element `entry` names, with the generation of
    /// the listener that keeps it, which Core holds while the entry waits
    /// or runs (`crate::state::SrState::held_tokens`).
    fn told_token(entry: &Self::Entry) -> Option<(u64, verbatim_model::tell::ElementToken)> {
        let _ = entry;
        None
    }

    /// Whether `entry`'s function asks nothing, ending in its first poll:
    /// no checkpoint is taken for it. One that asks fails the driver's
    /// check that a paused function has its checkpoint.
    fn asks_nothing(entry: &Self::Entry) -> bool {
        let _ = entry;
        false
    }

    /// Why the front's request out ends now, `front` being the front
    /// entry with its request, judged for `cause` with Core's facts, the
    /// backend verdicts at `now_us` and the foreground record: the one
    /// judge of a request ending before its answer. `None` while it is
    /// still awaited.
    fn ends(
        facts: &Self::Facts,
        front: (&Self::Entry, &Live),
        cause: Cause<'_, Self::Entry>,
        at: (&BackendCache, u64, Option<Foreground>),
    ) -> Option<verbatim_model::request::CancelReason>;

    /// The entry appended when `window`, in the recovery set, answers
    /// again at `now`; what Core's facts keep of the window may change, as
    /// a terminal is read again.
    fn recovered(facts: &mut Self::Facts, window: &RequestWindow, now: Now) -> Option<Self::Entry>;

    /// The requests Core's facts want sent now, outside the queue, in
    /// order with the queue's requests: taken whenever the queue settles,
    /// and by a function that sends them at once ([`Cx::send_wanted`]).
    fn wanted(facts: &mut Self::Facts) -> Vec<CoreRequest> {
        let _ = facts;
        Vec::new()
    }

    /// A request Core's facts asked under `token` ended with `outcome`, a
    /// result with who ran it, at `now` with `foreground` the foreground
    /// record and `front` the entry running, if one is: what the facts
    /// keep of it changes, what that says goes into `said`, and the entries
    /// returned join the back.
    fn facts_answered(
        facts: &mut Self::Facts,
        token: u64,
        outcome: Outcome<(verbatim_model::request::Reply, Ran)>,
        (now, foreground, front): (Now, Option<Foreground>, Option<&Self::Entry>),
        said: &mut Vec<Effect>,
    ) -> Vec<Self::Entry> {
        let _ = (facts, token, outcome, now, foreground, front, said);
        Vec::new()
    }

    /// How `entry` waits: its part in the limiter, judged on its tell's
    /// facts.
    fn place(facts: &Self::Facts, entry: &Self::Entry) -> Place;

    /// Whether the waiting entry `waiting` is the same as `arriving` for
    /// the merge: the same object, kind and raising thread, and for UI
    /// Automation the property or the notification's kind, processing
    /// and activity.
    fn merges(facts: &Self::Facts, waiting: &Self::Entry, arriving: &Self::Entry) -> bool;

    /// Whether an MSAA entry of `key` is exempt from its thread's cap: it
    /// is on the readers' last focus's ids.
    fn exempt_from_cap(facts: &Self::Facts, key: &MsaaKey) -> bool;

    /// The entry a run of MSAA focus and foreground candidates, oldest
    /// first, runs as when it reaches the front.
    fn run_entry(candidates: Vec<Self::Entry>) -> Self::Entry;

    /// Whether the waiting entry `entry` ends when `foreground` comes in
    /// front: its request would name a window outside the foreground's
    /// tree and owner chain, and it is not exempt.
    fn ends_at_foreground(
        facts: &Self::Facts,
        entry: &Self::Entry,
        foreground: &Foreground,
    ) -> bool;

    /// The answer Core's plain reader gives `request` when it names a plain
    /// node, given at once with no call into any application; `None` for a
    /// request a reader process answers (`crate::focus::plain`).
    fn plain_answer(
        facts: &Self::Facts,
        request: &verbatim_model::request::Question,
    ) -> Option<verbatim_model::request::Outcome> {
        let _ = (facts, request);
        None
    }

    /// The probe the tell of `entry` carries, recorded in the backend cache
    /// when it arrives, so that a merge of the entry cannot lose it.
    fn probe(entry: &Self::Entry) -> Option<verbatim_model::request::Probe> {
        let _ = entry;
        None
    }

    /// The entry Core appends at the listener's first foreground report,
    /// `foreground`: the read of the focus at startup.
    fn started(facts: &Self::Facts, foreground: &Foreground) -> Option<Self::Entry> {
        let _ = (facts, foreground);
        None
    }

    /// `instance` ended at `now`: what Core's facts keep of it change,
    /// what that says goes into `said`, and the entries returned join the
    /// back (the re-take, asked only with `retake`).
    fn reader_ended(
        facts: &mut Self::Facts,
        instance: Instance,
        (foreground, retake, now): (Option<Foreground>, bool, Now),
        said: &mut Vec<Effect>,
    ) -> Vec<Self::Entry> {
        let _ = (facts, instance, foreground, retake, now, said);
        Vec::new()
    }

    /// Whether Core starts an outpost for `process`: not for a process
    /// whose windows nothing is read from.
    fn watched(facts: &Self::Facts, process: verbatim_model::Pid) -> bool {
        let _ = (facts, process);
        true
    }

    /// The entries Core's facts ask to join the back now, such as a
    /// say-all's next read: taken after every input and every function's
    /// step, so they join in the order they were wanted.
    fn joining(facts: &mut Self::Facts) -> Vec<Self::Entry> {
        let _ = facts;
        Vec::new()
    }

    /// The waiting `entry` ended at `now` because the foreground changed
    /// ([`Logic::ends_at_foreground`]): what Core's facts keep of it
    /// change, and what that says goes into `said`.
    fn ended_at_foreground(
        facts: &mut Self::Facts,
        entry: &Self::Entry,
        now: Now,
        said: &mut Vec<Effect>,
    ) {
        let _ = (facts, entry, now, said);
    }

    /// The earliest time Core's facts want a wake at, such as a held key's
    /// release.
    fn due(facts: &Self::Facts) -> Option<u64> {
        let _ = facts;
        None
    }

    /// A timer came due at `now`, with `foreground` the foreground record:
    /// what Core's facts keep for that time happens, what that says goes
    /// into `said`, and the entries returned join the back.
    fn tick(
        facts: &mut Self::Facts,
        now: Now,
        foreground: Option<Foreground>,
        said: &mut Vec<Effect>,
    ) -> Vec<Self::Entry> {
        let _ = (facts, now, foreground, said);
        Vec::new()
    }

    /// Whether `arriving` is dropped at its arrival, judged against the
    /// entries ahead of it, the front and the items waiting, at `now_us`
    /// with the foreground record.
    fn dropped_at_arrival(
        facts: &Self::Facts,
        ahead: (
            Option<&Self::Entry>,
            &std::collections::VecDeque<Item<Self::Entry>>,
        ),
        arriving: &Self::Entry,
        backend: &BackendCache,
        now_us: (u64, Option<Foreground>),
    ) -> bool {
        let _ = (facts, ahead, arriving, backend, now_us);
        false
    }

    /// `window` was destroyed: what Core's facts keep of it change, and
    /// the requests Core sends as its own for it, each to a reader
    /// instance that is ready.
    fn window_destroyed(facts: &mut Self::Facts, window: WindowHandle) -> Vec<CoreRequest> {
        let _ = (facts, window);
        Vec::new()
    }

    /// `entry` joins the back of the queue at `now`: what Core's facts
    /// keep of the arrivals changes, and the entry is stamped with what it
    /// needs of them and of the front, `front`, with its request waiting
    /// or out. Called for every entry that joins, before the limiter
    /// places it; an entry dropped at its arrival never joins.
    fn arrival(
        facts: &mut Self::Facts,
        front: Option<(&Self::Entry, Option<&verbatim_model::request::Question>)>,
        entry: &mut Self::Entry,
        now: Now,
    ) {
        let _ = (facts, front, entry, now);
    }

    /// `said`, emitted by the front entry `entry`'s function, stamped with
    /// what it carries of its entry.
    fn stamp(entry: &Self::Entry, said: &mut Effect) {
        let _ = (entry, said);
    }

    /// The foreground marker joining the back when `foreground` comes in
    /// front, whose turn empties the readers' last focus when its window is
    /// unrelated to the committed focus.
    fn foreground_marker(foreground: &Foreground) -> Option<Self::Entry>;
}
