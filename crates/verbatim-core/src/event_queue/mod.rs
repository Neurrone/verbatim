//! Core's event queue, its async driver and its request table (ADR 0003,
//! "Readers tell, Core asks"; `docs/design/readers-tell-core-asks-build1.md`,
//! "The model", Table 2).
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
//! the requests and their stops. Built beside the reducer: nothing feeds it
//! yet.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "built beside the reducer; the focus rules switch over to it in a later step of build 1"
    )
)]
// A borrow of the shared state held across an await would be taken again
// by the driver when the function's request ends.
#![deny(clippy::await_holding_refcell_ref)]

mod driver;
mod inbox;
mod limiter;
#[cfg(test)]
mod limiter_tests;
pub(crate) mod pairs;
mod request;
mod table;
#[cfg(test)]
mod tests;

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use verbatim_model::{Pid, WindowHandle};

use crate::focus::backend::{Backend, BackendCache, JudgedWindow, WindowKey};

pub(crate) use driver::{Core, Cx, Queue, ReplayMismatch, State};
pub(crate) use inbox::{Effect, Input, Timed};
pub(crate) use limiter::{MsaaClass, MsaaKey, Place};
pub(crate) use request::{Outcome, Refused, Request};
pub(crate) use table::Asker;

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
            process: self.process,
            thread: self.thread,
            desktop: false,
        }
    }

    /// The window a request on a node an answer read names: the node's
    /// nearest window with the facts the reader read of it. Its classes are
    /// not read, so probes decide its backend; a request on a node an
    /// outpost holds is read through that node's own backend.
    pub(crate) fn of_node(window: &verbatim_model::request::NodeWindow) -> Self {
        Self {
            handle: window.handle,
            top_level: window.top_level,
            root_owner: window.root_owner,
            process: window.process,
            thread: window.thread,
            by_class: None,
        }
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
    /// The application holding attention.
    pub(crate) process: Pid,
    /// The thread that owns the window.
    pub(crate) thread: u32,
    /// The backend its class gives, `None` when probes decide.
    pub(crate) by_class: Option<Backend>,
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

    /// The foreground window as a request names it.
    pub(crate) fn window(&self) -> RequestWindow {
        RequestWindow {
            handle: self.window,
            top_level: self.window,
            root_owner: self.root_owner,
            process: self.process,
            thread: self.thread,
            by_class: self.by_class,
        }
    }

    /// Whether `window`'s application holds attention.
    pub(crate) fn attends(&self, window: &RequestWindow) -> bool {
        self.process == window.process
    }
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
    /// What a function emits, such as speech.
    type Said: Clone + Debug + PartialEq;

    /// The function of `entry`, run when it reaches the front.
    fn run(cx: Cx<Self>, entry: Self::Entry) -> Function;

    /// Whether `entry`, whose request out names `window`, was accepted by
    /// something other than the foreground relation, which exempts that
    /// request from the foreground stop.
    fn exempt_from_foreground(entry: &Self::Entry, window: &RequestWindow) -> bool;

    /// Whether `arriving`, joining the back, withdraws `request`, the
    /// request out of the front entry `front`, judged with the backend
    /// verdicts at `now_us`.
    fn withdraws(
        facts: &Self::Facts,
        front: (&Self::Entry, &verbatim_model::request::Request),
        arriving: &Self::Entry,
        backend: &BackendCache,
        now_us: u64,
    ) -> bool;

    /// The entry appended when `window`, in the recovery set, answers
    /// again.
    fn recovered(facts: &Self::Facts, window: &RequestWindow) -> Option<Self::Entry>;

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

    /// Whether the waiting `entry` is dropped because `instance`, which
    /// ended, kept its element.
    fn kept_by(entry: &Self::Entry, instance: Instance) -> bool {
        let _ = (entry, instance);
        false
    }

    /// `instance` ended: what Core's facts keep of it change, what that
    /// says goes into `said`, and the entries returned join the back (the
    /// re-take).
    fn reader_ended(
        facts: &mut Self::Facts,
        instance: Instance,
        foreground: Option<Foreground>,
        said: &mut Vec<Self::Said>,
    ) -> Vec<Self::Entry> {
        let _ = (facts, instance, foreground, said);
        Vec::new()
    }

    /// The entries Core's facts ask to join the back now, such as a
    /// say-all's next read: taken after every input and every function's
    /// step, so they join in the order they were wanted.
    fn joining(facts: &mut Self::Facts) -> Vec<Self::Entry> {
        let _ = facts;
        Vec::new()
    }

    /// Whether the front entry `front` no longer needs `request`, its
    /// request out: Core withdraws it, as when say-all stopped while its
    /// read was out.
    fn unneeded(
        facts: &Self::Facts,
        front: &Self::Entry,
        request: &verbatim_model::request::Request,
    ) -> bool {
        let _ = (facts, front, request);
        false
    }

    /// The waiting `entry` ended because the foreground changed
    /// ([`Logic::ends_at_foreground`]): what Core's facts keep of it
    /// change, and what that says goes into `said`.
    fn ended_at_foreground(
        facts: &mut Self::Facts,
        entry: &Self::Entry,
        said: &mut Vec<Self::Said>,
    ) {
        let _ = (facts, entry, said);
    }

    /// The earliest time Core's facts want a wake at, such as a held key's
    /// release.
    fn due(facts: &Self::Facts) -> Option<u64> {
        let _ = facts;
        None
    }

    /// A timer came due at `now_us`: what Core's facts keep for that time
    /// happens, and what that says goes into `said`.
    fn tick(facts: &mut Self::Facts, now_us: u64, said: &mut Vec<Self::Said>) {
        let _ = (facts, now_us, said);
    }

    /// Whether `arriving` is dropped at its arrival, judged against the
    /// entries `ahead` of it, the front first.
    fn dropped_at_arrival(
        facts: &Self::Facts,
        ahead: &[&Self::Entry],
        arriving: &Self::Entry,
        backend: &BackendCache,
        now_us: u64,
    ) -> bool {
        let _ = (facts, ahead, arriving, backend, now_us);
        false
    }

    /// `window` was destroyed: what Core's facts keep of it change.
    fn window_destroyed(facts: &mut Self::Facts, window: WindowHandle) {
        let _ = (facts, window);
    }

    /// `entry` joins the back of the queue: what Core's facts keep of the
    /// arrivals changes, and the entry is stamped with what it needs of
    /// them. Called for every entry that joins, before the limiter places
    /// it; an entry dropped at its arrival never joins.
    fn arrival(facts: &mut Self::Facts, entry: &mut Self::Entry) {
        let _ = (facts, entry);
    }

    /// `said`, emitted by the front entry `entry`'s function, stamped with
    /// what it carries of its entry.
    fn stamp(entry: &Self::Entry, said: &mut Self::Said) {
        let _ = (entry, said);
    }

    /// The foreground marker joining the back when `foreground` comes in
    /// front, whose turn empties the readers' last focus when its window is
    /// unrelated to the committed focus.
    fn foreground_marker(foreground: &Foreground) -> Option<Self::Entry>;
}
