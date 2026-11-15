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

use crate::focus::backend::{Backend, JudgedWindow, WindowKey};

pub(crate) use driver::{Core, Cx, Queue, ReplayMismatch, State};
#[cfg(test)]
pub(crate) use inbox::Input;
pub(crate) use inbox::{Effect, Timed};
pub(crate) use limiter::{MsaaClass, MsaaKey, Place};
pub(crate) use request::{Outcome, Request};

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

/// A reader Core asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Reader {
    /// The focus listener.
    Listener,
    /// The outpost of one application, by its process.
    Outpost(Pid),
}

/// One running instance of a reader: a request out goes to one instance,
/// and only that instance's answer or ending ends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Instance {
    /// The reader.
    pub(crate) reader: Reader,
    /// Its generation, never reused.
    pub(crate) generation: u64,
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

    /// Whether `arriving`, joining the back, withdraws the request of the
    /// front entry `front`.
    fn withdraws(facts: &Self::Facts, front: &Self::Entry, arriving: &Self::Entry) -> bool;

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

    /// The foreground marker joining the back when `foreground` comes in
    /// front, whose turn empties the readers' last focus when its window is
    /// unrelated to the committed focus.
    fn foreground_marker(foreground: &Foreground) -> Option<Self::Entry>;
}
