//! The event queue's entries over the reducer's state (ADR 0003, "Core's
//! inbox and the event queue"; `docs/design/readers-tell-core-asks-build1.md`,
//! "How the code is organised"): what an entry is, how the queue's rules
//! read it, and which feature's async function runs it. Each kind's
//! function sits in its feature's module; only what crosses features is
//! here.
//!
//! [`SrState`] holds the queue as plain data ([`SrState::queue`]); the
//! front entry's paused function sits beside it in [`Paused`], which is
//! never cloned or written to a dump, and is rebuilt from the front's
//! checkpoint when it is missing ([`feed`]). The queue's functions share
//! the reducer's state, the focus announcement code included, until the
//! old types are deleted.
//!
//! Built beside the reducer's focus path: nothing in the shell feeds it
//! yet.

#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "built beside the reducer; the shell feeds the queue in a later step of build 1"
    )
)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{MsaaIds, Probe};
use verbatim_model::tell::{AcceptanceFacts, ElementToken, Event, EventObject, TellWindow};
use verbatim_model::{Effect as Said, WindowHandle};

use crate::event_queue::{
    Core, Cx, Effect, Foreground, Function, Logic, MsaaClass, MsaaKey, Place, ReplayMismatch,
    RequestWindow, State, Timed,
};
use crate::focus::acceptance::{self, Accepted};
use crate::focus::backend::by_class;
use crate::focus::versions::ReadBy;
use crate::state::SrState;

/// A UI Automation focus tell, parsed: its element's token and runtime id,
/// and the window its check found, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UiaFocus {
    /// The tell.
    pub(crate) tell: Event,
    /// The token the listener keeps the element behind.
    pub(crate) token: ElementToken,
    /// The element's runtime id, from the event's cache.
    pub(crate) runtime_id: RuntimeId,
}

/// An MSAA focus or foreground tell, parsed: a run candidate, with the ids
/// its event named and the facts of its window, which a `WinEvent` always
/// has (the listener makes it the desktop window when the event names
/// none).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Candidate {
    /// The tell.
    pub(crate) tell: Event,
    /// The ids its event named.
    pub(crate) ids: MsaaIds,
    /// The window its request names.
    pub(crate) window: RequestWindow,
}

impl Candidate {
    /// Whether it is a foreground tell.
    pub(crate) fn foreground(&self) -> bool {
        self.tell.kind == EventKind::Foreground
    }

    /// Its window's facts.
    pub(crate) fn facts(&self) -> Option<&AcceptanceFacts> {
        window_facts(&self.tell)
    }
}

/// An entry of the event queue.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Entry {
    /// A UI Automation focus.
    UiaFocus(Box<UiaFocus>),
    /// An MSAA focus or foreground tell, waiting as a run's candidate.
    Candidate(Box<Candidate>),
    /// A run of candidates at the front, oldest first.
    Run(Vec<Candidate>),
    /// The foreground record changed to this window: at its turn the
    /// readers' last focus is emptied when the window is a top-level
    /// window unrelated to the committed focus.
    ForegroundMarker {
        /// The window in front.
        window: WindowHandle,
        /// The top of its owner chain.
        root_owner: WindowHandle,
    },
}

impl fmt::Debug for Entry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UiaFocus(focus) => write!(
                formatter,
                "UiaFocus({:?} {:?} of {})",
                focus.tell.kind, focus.runtime_id, focus.tell.process.0
            ),
            Self::Candidate(candidate) => write!(
                formatter,
                "Candidate({:?} {:?} of {})",
                candidate.tell.kind, candidate.ids, candidate.tell.process.0
            ),
            Self::Run(candidates) => formatter
                .debug_list()
                .entries(
                    candidates
                        .iter()
                        .map(|candidate| (candidate.tell.kind, candidate.ids)),
                )
                .finish(),
            Self::ForegroundMarker { window, root_owner } => {
                write!(formatter, "ForegroundMarker({} {})", window.0, root_owner.0)
            }
        }
    }
}

/// The facts of `tell`'s window, `None` when it has none.
pub(crate) fn window_facts(tell: &Event) -> Option<&AcceptanceFacts> {
    match &tell.window {
        TellWindow::Found(facts) => Some(facts),
        TellWindow::NoWindow | TellWindow::Failed => None,
    }
}

/// The window a request about `tell` names, from its window's facts.
pub(crate) fn request_window(tell: &Event) -> Option<RequestWindow> {
    let facts = window_facts(tell)?;
    Some(RequestWindow {
        handle: facts.window.handle,
        top_level: facts.root.handle,
        root_owner: facts.root_owner.handle,
        process: facts.window.process,
        thread: facts.window.thread,
        by_class: by_class(&facts.window.class, &facts.root.class),
    })
}

impl Entry {
    /// The entry `tell` becomes, `None` for a kind whose entry a later
    /// step of the build adds, and for a tell whose object does not fit
    /// its kind, a reader's fault.
    pub(crate) fn of_tell(tell: Event) -> Option<Self> {
        match (tell.kind, &tell.object) {
            (EventKind::UiaFocus, EventObject::Uia { token, runtime_id }) => {
                let (token, runtime_id) = (*token, runtime_id.clone());
                Some(Self::UiaFocus(Box::new(UiaFocus {
                    tell,
                    token,
                    runtime_id,
                })))
            }
            (
                EventKind::MsaaFocus | EventKind::Foreground,
                EventObject::Msaa {
                    window,
                    object,
                    child,
                },
            ) => {
                let ids = MsaaIds {
                    window: *window,
                    object: *object,
                    child: *child,
                };
                let window = request_window(&tell)?;
                Some(Self::Candidate(Box::new(Candidate { tell, ids, window })))
            }
            _ => None,
        }
    }

    /// The tell of a single-tell entry.
    fn tell(&self) -> Option<&Event> {
        match self {
            Self::UiaFocus(focus) => Some(&focus.tell),
            Self::Candidate(candidate) => Some(&candidate.tell),
            Self::Run(_) | Self::ForegroundMarker { .. } => None,
        }
    }
}

/// Whether `tell` was accepted by something other than the foreground
/// relation: its request is not stopped when the foreground changes.
fn accepted_apart_from_foreground(tell: &Event) -> bool {
    match acceptance::accept(tell.kind, window_facts(tell), false) {
        Ok(
            Accepted::NoWindow
            | Accepted::BackgroundProgress
            | Accepted::ShowClass
            | Accepted::Toast
            | Accepted::AnyWindow
            | Accepted::DesktopWindow
            | Accepted::Topmost,
        ) => true,
        Ok(Accepted::CoreWindowActive | Accepted::SameRootOwner | Accepted::InForeground)
        | Err(_) => false,
    }
}

/// The queue's logic over the reducer's state.
#[derive(Debug)]
pub(crate) struct Entries;

impl Logic for Entries {
    type Entry = Entry;
    type Facts = SrState;
    type Said = Said;

    fn run(cx: Cx<Self>, entry: Entry) -> Function {
        match entry {
            Entry::UiaFocus(focus) => Box::pin(crate::focus::uia_focus::run(cx, *focus)),
            Entry::Run(candidates) => Box::pin(crate::focus::msaa_run::run(cx, candidates)),
            Entry::Candidate(candidate) => {
                Box::pin(crate::focus::msaa_run::run(cx, vec![*candidate]))
            }
            Entry::ForegroundMarker { window, root_owner } => Box::pin(async move {
                let mut cx = cx;
                cx.facts_mut()
                    .versions
                    .foreground_marker(window, root_owner);
            }),
        }
    }

    fn exempt_from_foreground(entry: &Entry, window: &RequestWindow) -> bool {
        match entry {
            Entry::Run(candidates) => candidates
                .iter()
                .rev()
                .find(|candidate| candidate.window == *window)
                .is_some_and(|candidate| accepted_apart_from_foreground(&candidate.tell)),
            other => other.tell().is_some_and(accepted_apart_from_foreground),
        }
    }

    fn withdraws(_: &SrState, _: &Entry, _: &Entry) -> bool {
        // Only a key withdraws a request, and keys join the queue in a
        // later step of the build.
        false
    }

    fn recovered(_: &SrState, _: &RequestWindow) -> Option<Entry> {
        // The read of the focus after a window answers again comes with the
        // re-take, in a later step of the build.
        None
    }

    fn place(_: &SrState, entry: &Entry) -> Place {
        match entry {
            Entry::Candidate(candidate) => Place::Msaa {
                key: MsaaKey {
                    kind: candidate.tell.kind,
                    window: candidate.ids.window,
                    object: candidate.ids.object,
                    child: candidate.ids.child,
                    thread: candidate.tell.thread,
                },
                class: if candidate.foreground() {
                    MsaaClass::Foreground
                } else {
                    MsaaClass::Focus
                },
                accepted: acceptance::accept(candidate.tell.kind, candidate.facts(), false).is_ok(),
            },
            Entry::UiaFocus(_) => Place::Other {
                merges: true,
                focus_kind: true,
                ends_run: false,
            },
            Entry::Run(_) | Entry::ForegroundMarker { .. } => Place::Other {
                merges: false,
                focus_kind: false,
                ends_run: false,
            },
        }
    }

    fn merges(_: &SrState, waiting: &Entry, arriving: &Entry) -> bool {
        match (waiting, arriving) {
            (Entry::UiaFocus(waiting), Entry::UiaFocus(arriving)) => {
                (waiting.tell.kind, &waiting.runtime_id, waiting.tell.thread)
                    == (
                        arriving.tell.kind,
                        &arriving.runtime_id,
                        arriving.tell.thread,
                    )
            }
            _ => false,
        }
    }

    fn exempt_from_cap(facts: &SrState, key: &MsaaKey) -> bool {
        facts.versions.last.as_ref().is_some_and(|last| {
            matches!(
                &last.read_by,
                ReadBy::Msaa { event: Some(ids), .. }
                    if (ids.window, ids.object, ids.child) == (key.window, key.object, key.child)
            )
        })
    }

    fn run_entry(candidates: Vec<Entry>) -> Entry {
        Entry::Run(
            candidates
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Candidate(candidate) => Some(*candidate),
                    _ => None,
                })
                .collect(),
        )
    }

    fn ends_at_foreground(_: &SrState, entry: &Entry, foreground: &Foreground) -> bool {
        let window = match entry {
            Entry::UiaFocus(focus) => request_window(&focus.tell),
            Entry::Candidate(candidate) => Some(candidate.window),
            Entry::Run(_) | Entry::ForegroundMarker { .. } => None,
        };
        window.is_some_and(|window| {
            !foreground.holds(&window) && !entry.tell().is_some_and(accepted_apart_from_foreground)
        })
    }

    fn probe(entry: &Entry) -> Option<Probe> {
        match entry {
            Entry::UiaFocus(focus) => focus.tell.checked.as_ref()?.probe,
            Entry::Candidate(_) | Entry::Run(_) | Entry::ForegroundMarker { .. } => None,
        }
    }

    fn foreground_marker(foreground: &Foreground) -> Option<Entry> {
        Some(Entry::ForegroundMarker {
            window: foreground.window,
            root_owner: foreground.root_owner,
        })
    }
}

/// The key of the front entry's paused function, kept beside the state:
/// the function itself sits in its thread's table of paused functions
/// ([`PAUSED`]), since it shares Core's state by reference and so cannot
/// travel with a state sent to another thread. It is never cloned or
/// written to a dump: a clone, a restored dump, a replay or a state moved
/// to another thread finds none under its key and rebuilds it from the
/// front's checkpoint.
#[derive(Default)]
pub(crate) struct Paused(Option<u64>);

thread_local! {
    /// Each thread's paused functions, by the key of the state that holds
    /// them.
    static PAUSED: RefCell<BTreeMap<u64, Core<Entries>>> = const { RefCell::new(BTreeMap::new()) };
}

/// The next key, unique across threads, so that a state moved to another
/// thread never finds another state's function under its key.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

impl Paused {
    /// Takes the paused function out of its thread's table.
    fn take(&mut self) -> Option<Core<Entries>> {
        let key = self.0.take()?;
        PAUSED.with(|paused| paused.borrow_mut().remove(&key))
    }

    /// Keeps `core` in this thread's table under a new key.
    fn keep(core: Core<Entries>) -> Self {
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        PAUSED.with(|paused| paused.borrow_mut().insert(key, core));
        Self(Some(key))
    }
}

/// Empties this thread's table, as a thread the states were moved to
/// finds it.
#[cfg(test)]
pub(crate) fn forget_paused_for_tests() {
    PAUSED.with(|paused| paused.borrow_mut().clear());
}

impl Drop for Paused {
    fn drop(&mut self) {
        if let Some(key) = self.0.take() {
            // A thread's table may already be gone as the thread exits.
            let _ = PAUSED.try_with(|paused| paused.borrow_mut().remove(&key));
        }
    }
}

impl Clone for Paused {
    fn clone(&self) -> Self {
        Self(None)
    }
}

impl PartialEq for Paused {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Paused {}

impl fmt::Debug for Paused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Paused")
    }
}

/// Handles one queue input on `state` and returns what Core did: the
/// queue's state and Core's facts are lent to the driver for the step,
/// with the front's paused function, rebuilt when the state has none.
///
/// # Errors
///
/// [`ReplayMismatch`] when the front's function cannot be rebuilt from its
/// checkpoint; the state is left as it was.
pub(crate) fn feed(
    state: &mut SrState,
    input: Timed<Entry>,
) -> Result<Vec<Effect<Said>>, ReplayMismatch> {
    let mut paused = state.paused.take();
    let queue = std::mem::take(&mut state.queue);
    let mut whole = State {
        facts: std::mem::take(state),
        queue,
    };
    let effects = Core::step(&mut paused, &mut whole, input);
    *state = whole.facts;
    state.queue = whole.queue;
    state.paused = paused.map(Paused::keep).unwrap_or_default();
    effects
}

#[cfg(test)]
#[path = "entries_tests.rs"]
mod tests;
