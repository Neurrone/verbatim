//! The event queue's entries over the reducer's state (ADR 0003, "Core's
//! inbox and the event queue"; `docs/design/readers-tell-core-asks-build1.md`,
//! "How the code is organised"): what an entry is, how the queue's rules
//! read it, and which feature's async function runs it. Each kind's
//! function sits in its feature's module; only what crosses features is
//! here.
//!
//! [`SrState`] holds the queue as plain data ([`SrState::queue`]); the
//! front entry's paused function sits beside it in a [`Runtime`], owned
//! by whoever runs the reducer, never cloned or written to a dump, and
//! rebuilt from the front's checkpoint when it is missing ([`feed`]). The queue's functions share
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

use std::fmt;

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{MsaaIds, OutsideOutcome, Probe, QueueEffect, QueueInput};
use verbatim_model::tell::{AcceptanceFacts, ElementToken, Event, EventObject, TellWindow};
use verbatim_model::{Earcon, Effect as Said, SpeechMark, TraceId, WindowHandle};

use crate::event_queue::Instance;
use crate::event_queue::{
    Core, Cx, Effect, Foreground, Function, Input, Logic, MsaaClass, MsaaKey, Outcome, Place,
    Refused, ReplayMismatch, RequestWindow, State, Timed,
};
use crate::focus::acceptance::{self, Accepted};
use crate::focus::backend::{BackendCache, by_class};
use crate::focus::menus::MenuCheck;
use crate::focus::retake::CurrentFocus;
use crate::focus::versions::ReadBy;
use crate::keys::entry::{Fence, PressEntry};
use crate::state::SrState;

/// The trace of the queue input being handled, which an entry Core makes
/// itself runs under; the raw 0 before the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InputTrace(pub(crate) TraceId);

impl Default for InputTrace {
    fn default() -> Self {
        Self(TraceId::from_raw(0))
    }
}

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
    /// The generation of the listener that keeps the element.
    pub(crate) listener: u64,
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
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
    /// Its speech fence, set when it joins the queue.
    pub(crate) fence: Fence,
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
    /// An MSAA menu start or end, popup start or end, or switch end: the
    /// menu slot's entry.
    Menu(Box<Candidate>),
    /// The check after a menu or switcher ended with no valid focus.
    MenuCheck(Box<MenuCheck>),
    /// A read of an application's current focus that Core makes itself.
    CurrentFocus(Box<CurrentFocus>),
    /// An MSAA name, value, state or description change, or a selection
    /// kind.
    MsaaChange(Box<crate::changes::MsaaChange>),
    /// A UI Automation property change.
    UiaChange(Box<crate::changes::UiaChange>),
    /// The foreground record changed to this window: at its turn the
    /// readers' last focus is emptied when the window is a top-level
    /// window unrelated to the committed focus.
    ForegroundMarker {
        /// The window in front.
        window: WindowHandle,
        /// The top of its owner chain.
        root_owner: WindowHandle,
        /// Its speech fence, set when it joins the queue; a marker says
        /// nothing.
        fence: Fence,
    },
    /// A key or command, its action chosen at the press.
    Press(Box<PressEntry>),
    /// Playback reached an index mark, with its speech fence.
    Mark(SpeechMark, Fence),
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
            Self::ForegroundMarker {
                window, root_owner, ..
            } => {
                write!(formatter, "ForegroundMarker({} {})", window.0, root_owner.0)
            }
            Self::Menu(candidate) => write!(
                formatter,
                "Menu({:?} {:?} of {})",
                candidate.tell.kind, candidate.ids, candidate.tell.process.0
            ),
            Self::MenuCheck(_) => formatter.write_str("MenuCheck"),
            Self::MsaaChange(change) => write!(
                formatter,
                "MsaaChange({:?} {:?} of {})",
                change.tell.kind, change.ids, change.tell.process.0
            ),
            Self::UiaChange(change) => write!(
                formatter,
                "UiaChange({:?} {:?} of {})",
                change.property, change.runtime_id, change.tell.process.0
            ),
            Self::CurrentFocus(read) => write!(
                formatter,
                "CurrentFocus({:?} {} of {})",
                read.why, read.window.handle.0, read.window.process.0
            ),
            Self::Press(press) => write!(
                formatter,
                "Press({:?} {:?})",
                press.press.key_seq, press.press.action
            ),
            Self::Mark(mark, _) => write!(formatter, "Mark({})", mark.0),
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
    /// The entry `tell`, told by `from`, becomes, `None` for a kind whose
    /// entry a later step of the build adds, and for a tell whose object
    /// does not fit its kind, a reader's fault.
    pub(crate) fn of_tell(from: Instance, tell: Event) -> Option<Self> {
        match (tell.kind, &tell.object) {
            (
                EventKind::UiaFocus | EventKind::UiaMenuOpened,
                EventObject::Uia { token, runtime_id },
            ) => {
                let (token, runtime_id) = (*token, runtime_id.clone());
                Some(Self::UiaFocus(Box::new(UiaFocus {
                    tell,
                    token,
                    runtime_id,
                    listener: from.generation,
                    fence: Fence::default(),
                })))
            }
            (
                kind @ (EventKind::MsaaFocus
                | EventKind::Foreground
                | EventKind::MenuStart
                | EventKind::MenuEnd
                | EventKind::PopupStart
                | EventKind::PopupEnd
                | EventKind::SwitchEnd),
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
                let candidate = Box::new(Candidate {
                    tell,
                    ids,
                    window,
                    fence: Fence::default(),
                });
                Some(match kind {
                    EventKind::MsaaFocus | EventKind::Foreground => Self::Candidate(candidate),
                    _ => Self::Menu(candidate),
                })
            }
            (
                kind,
                EventObject::Msaa {
                    window,
                    object,
                    child,
                },
            ) if crate::changes::is_msaa_change(kind) => {
                let ids = MsaaIds {
                    window: *window,
                    object: *object,
                    child: *child,
                };
                let window = request_window(&tell)?;
                Some(Self::MsaaChange(Box::new(crate::changes::MsaaChange {
                    tell,
                    ids,
                    window,
                    fence: Fence::default(),
                })))
            }
            (EventKind::UiaPropertyChanged, EventObject::Uia { runtime_id, .. }) => {
                let runtime_id = runtime_id.clone();
                crate::changes::uia_of_tell(tell, runtime_id)
                    .map(|change| Self::UiaChange(Box::new(change)))
            }
            _ => None,
        }
    }

    /// The tell of a single-tell entry.
    fn tell(&self) -> Option<&Event> {
        match self {
            Self::UiaFocus(focus) => Some(&focus.tell),
            Self::Candidate(candidate) | Self::Menu(candidate) => Some(&candidate.tell),
            Self::MsaaChange(change) => Some(&change.tell),
            Self::UiaChange(change) => Some(&change.tell),
            Self::Run(_)
            | Self::ForegroundMarker { .. }
            | Self::MenuCheck(_)
            | Self::CurrentFocus(_)
            | Self::Press(_)
            | Self::Mark(..) => None,
        }
    }

    /// Its speech fence; a run's is its newest candidate's.
    fn fence(&self) -> Fence {
        match self {
            Self::UiaFocus(focus) => focus.fence,
            Self::Candidate(candidate) | Self::Menu(candidate) => candidate.fence,
            Self::Run(candidates) => candidates
                .iter()
                .fold(Fence::default(), |fence, candidate| {
                    fence.max(candidate.fence)
                }),
            Self::MenuCheck(check) => check.fence,
            Self::CurrentFocus(read) => read.fence,
            Self::MsaaChange(change) => change.fence,
            Self::UiaChange(change) => change.fence,
            Self::ForegroundMarker { fence, .. } | Self::Mark(_, fence) => *fence,
            Self::Press(press) => press.fence,
        }
    }

    /// Sets its speech fence as it joins the queue.
    fn set_fence(&mut self, set: Fence) {
        match self {
            Self::UiaFocus(focus) => focus.fence = set,
            Self::Candidate(candidate) | Self::Menu(candidate) => candidate.fence = set,
            Self::Run(candidates) => {
                for candidate in candidates {
                    candidate.fence = set;
                }
            }
            Self::MenuCheck(check) => check.fence = set,
            Self::CurrentFocus(read) => read.fence = set,
            Self::MsaaChange(change) => change.fence = set,
            Self::UiaChange(change) => change.fence = set,
            Self::ForegroundMarker { fence, .. } | Self::Mark(_, fence) => *fence = set,
            Self::Press(press) => press.fence = set,
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

    fn run(mut cx: Cx<Self>, entry: Entry) -> Function {
        // The menu slot's entry judges whether the run before it executed
        // a focus; every other entry starts with none executed.
        let run_executed = std::mem::take(&mut cx.facts_mut().run_executed);
        match entry {
            Entry::Menu(candidate) => {
                Box::pin(crate::focus::menus::slot(cx, *candidate, run_executed))
            }
            Entry::MenuCheck(check) => Box::pin(crate::focus::menus::check(cx, *check)),
            Entry::CurrentFocus(read) => Box::pin(crate::focus::retake::run(cx, *read)),
            Entry::UiaFocus(focus) => Box::pin(crate::focus::uia_focus::run(cx, *focus)),
            Entry::Run(candidates) => Box::pin(crate::focus::msaa_run::run(cx, candidates)),
            Entry::Candidate(candidate) => {
                Box::pin(crate::focus::msaa_run::run(cx, vec![*candidate]))
            }
            Entry::MsaaChange(change) => Box::pin(crate::changes::msaa(cx, *change)),
            Entry::UiaChange(change) => Box::pin(async move {
                let mut cx = cx;
                crate::changes::uia(&mut cx, &change);
            }),
            Entry::Press(press) => Box::pin(crate::keys::entry::run(cx, *press)),
            Entry::Mark(mark, _) => Box::pin(async move {
                let mut cx = cx;
                crate::say_all::mark_entry(&mut cx, mark);
            }),
            Entry::ForegroundMarker {
                window, root_owner, ..
            } => Box::pin(async move {
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

    fn arrival(facts: &mut SrState, entry: &mut Entry) {
        if let Entry::Press(press) = entry {
            facts.arrivals.key_joined(press.press.key_seq);
        }
        entry.set_fence(facts.arrivals.fence());
    }

    fn stamp(entry: &Entry, said: &mut Said) {
        crate::keys::entry::stamp(entry.fence(), said);
    }

    fn withdraws(_: &SrState, _: &Entry, _: &Entry) -> bool {
        // Only a key withdraws a request, and keys join the queue in a
        // later step of the build.
        false
    }

    fn recovered(facts: &SrState, window: &RequestWindow) -> Option<Entry> {
        Some(Entry::CurrentFocus(Box::new(
            crate::focus::retake::recovered(facts, window),
        )))
    }

    fn started(facts: &SrState, foreground: &Foreground) -> Option<Entry> {
        Some(Entry::CurrentFocus(Box::new(
            crate::focus::retake::startup(facts, foreground),
        )))
    }

    fn kept_by(entry: &Entry, instance: Instance) -> bool {
        match entry {
            Entry::UiaFocus(focus) => {
                instance.reader == crate::event_queue::Reader::Listener
                    && focus.listener == instance.generation
            }
            _ => false,
        }
    }

    fn reader_ended(
        facts: &mut SrState,
        instance: Instance,
        foreground: Option<Foreground>,
    ) -> Vec<Entry> {
        if let crate::event_queue::Reader::Outpost(process) = instance.reader {
            let outpost = verbatim_model::OutpostId(instance.generation);
            facts.follows.outpost_ended(process, outpost);
            // The navigator object is one of the outpost's held nodes,
            // which end with it (Table 5): its commands say there is
            // none until a focus is committed again.
            if facts
                .navigator
                .as_ref()
                .is_some_and(|navigator| navigator.object.id.outpost() == outpost)
            {
                facts.navigator = None;
            }
        }
        crate::focus::retake::reader_ended(facts, instance, foreground)
            .map(|read| Entry::CurrentFocus(Box::new(read)))
            .into_iter()
            .collect()
    }

    fn window_destroyed(facts: &mut SrState, window: WindowHandle) {
        facts.follows.window_destroyed(window);
    }

    fn dropped_at_arrival(
        facts: &SrState,
        ahead: &[&Entry],
        arriving: &Entry,
        backend: &BackendCache,
        now_us: u64,
    ) -> bool {
        crate::focus::menus::opening_dropped(facts, ahead, arriving, backend, now_us)
    }

    fn place(facts: &SrState, entry: &Entry) -> Place {
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
            Entry::Menu(candidate) => Place::Msaa {
                key: MsaaKey {
                    kind: candidate.tell.kind,
                    window: candidate.ids.window,
                    object: candidate.ids.object,
                    child: candidate.ids.child,
                    thread: candidate.tell.thread,
                },
                class: MsaaClass::Menu,
                accepted: acceptance::accept(candidate.tell.kind, candidate.facts(), false).is_ok(),
            },
            Entry::MsaaChange(change) => Place::Msaa {
                key: MsaaKey {
                    kind: change.tell.kind,
                    window: change.ids.window,
                    object: change.ids.object,
                    child: change.ids.child,
                    thread: change.tell.thread,
                },
                class: MsaaClass::Other,
                accepted: acceptance::accept(
                    change.tell.kind,
                    window_facts(&change.tell),
                    facts.settings.report_background_progress_bars,
                )
                .is_ok(),
            },
            Entry::UiaFocus(_) => Place::Other {
                merges: true,
                focus_kind: true,
                ends_run: false,
            },
            Entry::UiaChange(_) => Place::Other {
                merges: true,
                focus_kind: false,
                ends_run: false,
            },
            Entry::CurrentFocus(_) | Entry::MenuCheck(_) => Place::Other {
                merges: false,
                focus_kind: true,
                ends_run: false,
            },
            Entry::Run(_) | Entry::ForegroundMarker { .. } | Entry::Press(_) | Entry::Mark(..) => {
                Place::Other {
                    merges: false,
                    focus_kind: false,
                    ends_run: false,
                }
            }
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
            (Entry::UiaChange(waiting), Entry::UiaChange(arriving)) => {
                (&waiting.runtime_id, waiting.property, waiting.tell.thread)
                    == (
                        &arriving.runtime_id,
                        arriving.property,
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
            Entry::Candidate(candidate) | Entry::Menu(candidate) => Some(candidate.window),
            Entry::CurrentFocus(read) => Some(read.window),
            Entry::MsaaChange(change) => Some(change.window),
            // A command resolves its object at its turn and is tested only
            // when it asks.
            Entry::Run(_)
            | Entry::ForegroundMarker { .. }
            | Entry::MenuCheck(_)
            | Entry::UiaChange(_)
            | Entry::Press(_)
            | Entry::Mark(..) => None,
        };
        window.is_some_and(|window| {
            !foreground.holds(&window) && !entry.tell().is_some_and(accepted_apart_from_foreground)
        })
    }

    fn probe(entry: &Entry) -> Option<Probe> {
        match entry {
            Entry::UiaFocus(focus) => focus.tell.checked.as_ref()?.probe,
            Entry::Candidate(_)
            | Entry::Run(_)
            | Entry::ForegroundMarker { .. }
            | Entry::Menu(_)
            | Entry::MenuCheck(_)
            | Entry::CurrentFocus(_)
            | Entry::MsaaChange(_)
            | Entry::UiaChange(_)
            | Entry::Press(_)
            | Entry::Mark(..) => None,
        }
    }

    fn foreground_marker(foreground: &Foreground) -> Option<Entry> {
        Some(Entry::ForegroundMarker {
            window: foreground.window,
            root_owner: foreground.root_owner,
            fence: Fence::default(),
        })
    }
}

/// The front entry's paused function, kept beside the state by whoever
/// runs the reducer (the reducer thread, the replay tool, a test) and
/// passed to [`crate::reduce`] with it. It is never cloned or written: a
/// runtime without the function, as after a clone or a restored dump,
/// rebuilds it from the front's checkpoint before the next input.
#[derive(Default)]
pub struct Runtime(Option<Core<Entries>>);

impl fmt::Debug for Runtime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if self.0.is_some() {
            "Runtime"
        } else {
            "Runtime(to rebuild)"
        })
    }
}

/// Handles one queue input on `state` and returns what Core did: the
/// queue's state and Core's facts are lent to the driver for the step,
/// with the front's paused function from `runtime`, rebuilt when it has
/// none.
///
/// # Errors
///
/// [`ReplayMismatch`] when the front's function cannot be rebuilt from its
/// checkpoint; the state is left as it was.
pub(crate) fn feed(
    state: &mut SrState,
    runtime: &mut Runtime,
    input: Timed<Entry>,
) -> Result<Vec<Effect<Said>>, ReplayMismatch> {
    let queue = std::mem::take(&mut state.queue);
    let mut whole = State {
        facts: std::mem::take(state),
        queue,
    };
    let effects = Core::step(&mut runtime.0, &mut whole, input);
    *state = whole.facts;
    state.queue = whole.queue;
    effects
}

/// Routes one queue input from the shell (`Input::Queue`) into the event
/// queue under `trace_id` at `at_us`, and returns what Core did as the
/// shell's effects: requests, cancels, hang checks and the sound, what
/// the entries' functions said, and the time Core next waits for. A tell
/// of a kind whose entry a later step of the build adds is not taken.
pub(crate) fn route(
    state: &mut SrState,
    runtime: &mut Runtime,
    (trace_id, at_us): (TraceId, u64),
    input: &QueueInput,
) -> Vec<Said> {
    let input = match input {
        QueueInput::Tell { from, tell } => match Entry::of_tell(*from, (**tell).clone()) {
            Some(entry) => Input::Entry(entry),
            None => return Vec::new(),
        },
        QueueInput::Answer { from, answer } => Input::Answer {
            from: *from,
            answer: answer.clone(),
        },
        QueueInput::RefusedHung(number) => Input::RefusedHung(*number),
        QueueInput::HangCheck { window, hung } => Input::HangCheck {
            window: *window,
            hung: *hung,
        },
        QueueInput::Foreground(report) => Input::Foreground(Foreground {
            window: report.window,
            root_owner: report.root_owner,
            process: report.process,
            thread: report.thread,
            // The foreground window is top-level: its own root.
            by_class: by_class(&report.class, &report.class),
        }),
        QueueInput::ReaderReady(instance) => Input::ReaderReady(*instance),
        QueueInput::ReaderEnded(instance) => Input::ReaderEnded(*instance),
        QueueInput::WindowDestroyed { window, process } => Input::WindowDestroyed {
            window: *window,
            process: *process,
        },
        QueueInput::Due => Input::Tick,
        QueueInput::Press(press) => Input::Entry(Entry::Press(Box::new(PressEntry::new(
            trace_id,
            (**press).clone(),
        )))),
        QueueInput::MarkReached(mark) => Input::Entry(Entry::Mark(*mark, Fence::default())),
        QueueInput::Ignored(processes) => {
            state.ignored.clone_from(processes);
            return Vec::new();
        }
    };
    state.queue_trace = InputTrace(trace_id);
    let effects = match feed(state, runtime, Timed { at_us, input }) {
        Ok(effects) => effects,
        Err(mismatch) => {
            return vec![Said::Queue(QueueEffect::Mismatch {
                replayed: mismatch.replayed,
                original: mismatch.original,
            })];
        }
    };
    let mut said: Vec<Said> = effects
        .into_iter()
        .map(|effect| match effect {
            Effect::Send { to, ask } => Said::Queue(QueueEffect::Send { to, ask }),
            Effect::Cancel { to, cancel } => Said::Queue(QueueEffect::Cancel { to, cancel }),
            Effect::CheckHung(windows) => Said::Queue(QueueEffect::CheckHung(windows)),
            Effect::NotRespondingSound => Said::PlayEarcon(Earcon::AppNotResponding),
            Effect::WindowDestroyed { to, window } => {
                Said::Queue(QueueEffect::WindowDestroyed { to, window })
            }
            Effect::AnswerDropped { number, late } => {
                Said::Queue(QueueEffect::AnswerDropped { number, late })
            }
            Effect::Undecodable(number) => Said::Queue(QueueEffect::Undecodable(number)),
            Effect::Said(said) => said,
            Effect::Answered { asker, outcome } => Said::Queue(QueueEffect::Answered {
                asker: asker.0,
                outcome: match outcome {
                    Outcome::Result(reply) => OutsideOutcome::Result(reply),
                    Outcome::Refused(Refused::Reader(refusal)) => OutsideOutcome::Refused(refusal),
                    Outcome::Refused(Refused::Undecodable) => OutsideOutcome::Undecodable,
                    Outcome::NotThere => OutsideOutcome::NotThere,
                    Outcome::NotResponding => OutsideOutcome::NotResponding,
                    Outcome::ReaderEnded => OutsideOutcome::ReaderEnded,
                    Outcome::NoLongerAwaited(reason) => OutsideOutcome::NoLongerAwaited(reason),
                },
            }),
        })
        .collect();
    said.push(Said::Queue(QueueEffect::WakeAt(state.queue.next_due())));
    said
}

#[cfg(test)]
#[path = "entries_tests.rs"]
pub(crate) mod tests;
