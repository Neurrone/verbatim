//! The event queue's entries over the reducer's state (ADR 0003, "Core's
//! inbox and the event queue"): what an entry is, how the queue's rules
//! read it, and which feature's async function runs it. Each kind's
//! function sits in its feature's module; only what crosses features is
//! here.
//!
//! [`SrState`] holds the queue as plain data ([`SrState::queue`]); the
//! front entry's paused function sits beside it in a [`Runtime`], owned
//! by whoever runs the reducer, never cloned or written to a dump, and
//! rebuilt from the front's checkpoint when it is missing ([`feed`]). The queue's functions share
//! the reducer's state, the focus announcement code included.

use std::fmt;

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{MsaaIds, PressAction, Probe, Question, QueueEffect, QueueInput};
use verbatim_model::tell::{
    AcceptanceFacts, ElementToken, Event, EventObject, Payload, TellWindow,
};
use verbatim_model::{Effect as Said, SpeechMark, TraceId, WindowHandle};

use crate::event_queue::Instance;
use crate::event_queue::{
    Asker, Cause, Core, Cx, Foreground, Function, Input, Live, Logic, MsaaClass, MsaaKey,
    OTHER_DUE_US, Outcome, Place, Reader, ReplayMismatch, RequestWindow, State, Timed,
};
use crate::focus::acceptance::{self, Acceptance, Accepted};
use crate::focus::backend::BackendCache;
use crate::focus::follow::FollowRequest;
use crate::focus::menus::MenuCheck;
use crate::focus::retake::CurrentFocus;
use crate::focus::versions::ReadBy;
use crate::keys::entry::{Fence, PressEntry};
use crate::state::SrState;
use verbatim_model::class::by_class;
use verbatim_model::request::CancelReason;

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
    /// Whether Alt was down at its tell's arrival, by Core's key state.
    pub(crate) alt_down: bool,
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
    /// Whether a caret key's confirmation read its facts while it waited.
    pub(crate) confirmation: crate::focus::validity::Confirmation,
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
    /// A UI Automation progress bar's change, with its percentage and
    /// place.
    Progress(Box<crate::progress::ProgressTell>),
    /// A caret tell: the system caret's, or a UI Automation text-selection
    /// change.
    Caret(Box<crate::caret_tell::CaretTell>),
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
    /// The next read of the say-all started by the press of this trace,
    /// with its speech fence.
    SayAllRead(TraceId, Fence),
    /// A notification, toast, UI Automation selection, MSAA alert or show.
    Announcement(Box<crate::announcements::Announcement>),
    /// A full read's speech, spoken at its turn
    /// (`docs/design/readers-tell-core-asks-build3.md`, Table 3).
    TerminalSpeech(Box<crate::terminal::reading::SpeechEntry>),
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
            Self::Progress(bar) => write!(
                formatter,
                "Progress({:?} {} of {})",
                bar.runtime_id,
                bar.percent.whole(),
                bar.tell.process.0
            ),
            Self::Caret(caret) => write!(
                formatter,
                "Caret({:?} {:?} of {})",
                caret.tell.kind, caret.target, caret.tell.process.0
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
            Self::SayAllRead(run, _) => write!(formatter, "SayAllRead({run:?})"),
            Self::Announcement(announcement) => write!(
                formatter,
                "Announcement({:?} {:?} of {})",
                announcement.tell.kind, announcement.tell.object, announcement.tell.process.0
            ),
            Self::TerminalSpeech(speech) => {
                write!(formatter, "TerminalSpeech({:?})", speech.node)
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

/// The speech fence of the front entry, `cx`'s own, as its speech is
/// stamped ([`Logic::stamp`]); a run's is its newest candidate's.
pub(crate) fn front_fence(cx: &Cx<Entries>) -> Fence {
    cx.front().map_or(Fence::default(), |front| front.fence())
}

/// The window a request about `tell` names, from its window's facts;
/// `None` when it has none, or no process could be read as serving it, so
/// that no reader can be asked about it.
pub(crate) fn request_window(tell: &Event) -> Option<RequestWindow> {
    let facts = window_facts(tell)?;
    Some(RequestWindow {
        handle: facts.window.handle,
        top_level: facts.root.handle,
        root_owner: facts.root_owner.handle,
        process: facts.window.process?,
        thread: facts.window.thread,
        by_class: by_class(&facts.window.class, &facts.root.class),
    })
}

/// The window a reader's report of a hung window names, from its facets,
/// as [`request_window`] names a tell's; `None` when no process could be
/// read as serving it.
fn hung_window(hung: &verbatim_model::tell::Hung) -> Option<RequestWindow> {
    Some(RequestWindow {
        handle: hung.window.handle,
        top_level: hung.root.handle,
        root_owner: hung.root_owner.handle,
        process: hung.window.process?,
        thread: hung.window.thread,
        by_class: by_class(&hung.window.class, &hung.root.class),
    })
}

impl Entry {
    /// The entry `tell`, told by `from`, becomes, `None` for a kind whose
    /// entry a later step of the build adds, and for a tell whose object
    /// does not fit its kind, a reader's fault. An MSAA change and a UI
    /// Automation change store their acceptance, judged now with
    /// background progress bars reported as `background` says at the
    /// tell's arrival, as NVDA judges an event as it is queued
    /// (`docs/parity/focus-events.md`, "Event acceptance").
    pub(crate) fn of_tell(from: Instance, tell: Event, background: bool) -> Option<Self> {
        // A top-level window's show, outside NVDA's show classes, is the
        // read at a postponed dialog's show (`crate::focus::shown`).
        if let Some(read) = crate::focus::shown::of_show(&tell) {
            return Some(Self::CurrentFocus(Box::new(read)));
        }
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
                    alt_down: false,
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
                    confirmation: crate::focus::validity::Confirmation::default(),
                });
                Some(match kind {
                    EventKind::MsaaFocus | EventKind::Foreground => Self::Candidate(candidate),
                    _ => Self::Menu(candidate),
                })
            }
            (EventKind::NameChange, EventObject::Msaa { .. })
                if crate::announcements::on_desktop(&tell) =>
            {
                crate::announcements::Announcement::of_tell(from.generation, tell)
                    .map(|announcement| Self::Announcement(Box::new(announcement)))
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
                crate::changes::msaa_of_tell(tell, ids, background)
                    .map(|change| Self::MsaaChange(Box::new(change)))
            }
            (EventKind::SystemCaret | EventKind::UiaTextSelectionChanged, _) => {
                crate::caret_tell::CaretTell::of_tell(tell)
                    .map(|caret| Self::Caret(Box::new(caret)))
            }
            (EventKind::UiaProgress, EventObject::Followed { runtime_id }) => {
                let runtime_id = runtime_id.clone();
                crate::progress::ProgressTell::of_tell(tell, runtime_id, background)
                    .map(|bar| Self::Progress(Box::new(bar)))
            }
            (
                EventKind::UiaPropertyChanged,
                EventObject::Uia { runtime_id, .. } | EventObject::Followed { runtime_id },
            ) => {
                let runtime_id = runtime_id.clone();
                crate::changes::uia_of_tell(tell, runtime_id, background)
                    .map(|change| Self::UiaChange(Box::new(change)))
            }
            (
                EventKind::UiaNotification
                | EventKind::UiaWindowOpened
                | EventKind::UiaElementSelected
                | EventKind::Alert
                | EventKind::Show,
                _,
            ) => crate::announcements::Announcement::of_tell(from.generation, tell)
                .map(|announcement| Self::Announcement(Box::new(announcement))),
            _ => None,
        }
    }

    /// The acceptance of a single-tell entry's tell: the verdict an MSAA
    /// change or a UI Automation change stored as it was made, any other
    /// tell's judged on its own facts.
    fn accepted(&self) -> Option<Acceptance> {
        match self {
            Self::MsaaChange(change) => Some(change.accepted),
            Self::UiaChange(change) => Some(change.accepted),
            Self::Progress(bar) => Some(bar.accepted),
            other => other.tell().map(acceptance::accepted),
        }
    }

    /// The tell of a single-tell entry.
    fn tell(&self) -> Option<&Event> {
        match self {
            Self::UiaFocus(focus) => Some(&focus.tell),
            Self::Candidate(candidate) | Self::Menu(candidate) => Some(&candidate.tell),
            Self::MsaaChange(change) => Some(&change.tell),
            Self::UiaChange(change) => Some(&change.tell),
            Self::Progress(bar) => Some(&bar.tell),
            Self::Caret(caret) => Some(&caret.tell),
            Self::Announcement(announcement) => Some(&announcement.tell),
            Self::Run(_)
            | Self::ForegroundMarker { .. }
            | Self::MenuCheck(_)
            | Self::CurrentFocus(_)
            | Self::Press(_)
            | Self::Mark(..)
            | Self::SayAllRead(..)
            | Self::TerminalSpeech(_) => None,
        }
    }

    /// Its speech fence; a run's is its newest candidate's.
    pub(crate) fn fence(&self) -> Fence {
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
            Self::Progress(bar) => bar.fence,
            Self::Caret(caret) => caret.fence,
            Self::ForegroundMarker { fence, .. }
            | Self::Mark(_, fence)
            | Self::SayAllRead(_, fence) => *fence,
            Self::Press(press) => press.fence,
            Self::Announcement(announcement) => announcement.fence,
            Self::TerminalSpeech(speech) => speech.fence,
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
            Self::Progress(bar) => bar.fence = set,
            Self::Caret(caret) => caret.fence = set,
            Self::ForegroundMarker { fence, .. }
            | Self::Mark(_, fence)
            | Self::SayAllRead(_, fence) => *fence = set,
            Self::Press(press) => press.fence = set,
            Self::Announcement(announcement) => announcement.fence = set,
            Self::TerminalSpeech(speech) => speech.fence = set,
        }
    }
}

/// Whether `entry`, whose request out names `window`, was accepted by
/// something other than the foreground relation, which exempts that
/// request from the foreground stop.
pub(crate) fn exempt_from_foreground(entry: &Entry, window: &RequestWindow) -> bool {
    match entry {
        Entry::Run(candidates) => candidates
            .iter()
            .rev()
            .find(|candidate| candidate.window == *window)
            .is_some_and(|candidate| {
                acceptance::accepted(&candidate.tell).is_ok_and(Accepted::exempt_from_foreground)
            }),
        Entry::Announcement(announcement) => announcement.exempt_from_foreground(),
        other => other
            .accepted()
            .is_some_and(|accepted| accepted.is_ok_and(Accepted::exempt_from_foreground)),
    }
}

/// Whether `arriving`, joining the back, withdraws `request`, the request
/// out of the front entry `front`: a caret key's stage withdrawn by it, or
/// say-all's read withdrawn by a key.
fn withdraws(
    facts: &SrState,
    (front, request): (&Entry, &Question),
    arriving: &Entry,
    backend: &BackendCache,
    (now_us, foreground): (u64, Option<Foreground>),
) -> bool {
    if let Some(caret) = crate::keys::caret_key::of_entry(front) {
        let arrival = match arriving {
            Entry::Press(_) => crate::keys::caret_key::Arrival::Key,
            other => {
                let view = crate::focus::validity::View {
                    versions: &facts.versions,
                    in_front: foreground.map(|foreground| foreground.window),
                    backend,
                    now_us,
                    run_before_valid: false,
                };
                match crate::focus::validity::judge_alone(other, &view) {
                    Some((answer, trace, at_us)) => crate::keys::caret_key::Arrival::Focus {
                        pending: crate::focus::validity::pending(answer, trace),
                        at_us,
                    },
                    None => crate::keys::caret_key::Arrival::Other,
                }
            }
        };
        return crate::keys::caret_key::withdrawn_by(&caret, request, arrival);
    }
    // Any key stops say-all, so a key arriving withdraws its read.
    matches!((front, arriving), (Entry::SayAllRead(..), Entry::Press(_)))
}

/// Whether the front entry `front` no longer needs `request`, its request
/// out: the caret key's or say-all's.
fn unneeded(facts: &SrState, front: &Entry, request: &Question) -> bool {
    if let Some(caret) = crate::keys::caret_key::of_entry(front) {
        return crate::keys::caret_key::unneeded(facts, &caret, request);
    }
    match front {
        Entry::SayAllRead(run, _) => crate::say_all::read_unneeded(facts, *run),
        _ => false,
    }
}

/// The queue's logic over the reducer's state.
#[derive(Debug)]
pub(crate) struct Entries;

impl Logic for Entries {
    fn told_token(entry: &Entry) -> Option<(u64, ElementToken)> {
        match entry {
            Entry::UiaFocus(focus) => Some((focus.listener, focus.token)),
            Entry::Announcement(announcement) => match &announcement.kind {
                crate::announcements::Kind::Toast(kept)
                | crate::announcements::Kind::UiaSelection(kept, _) => {
                    Some((kept.listener, kept.token))
                }
                crate::announcements::Kind::Notification { .. }
                | crate::announcements::Kind::Alert { .. }
                | crate::announcements::Kind::DesktopName { .. }
                | crate::announcements::Kind::Show { .. } => None,
            },
            Entry::Candidate(_)
            | Entry::Run(_)
            | Entry::Menu(_)
            | Entry::MenuCheck(_)
            | Entry::CurrentFocus(_)
            | Entry::MsaaChange(_)
            | Entry::UiaChange(_)
            | Entry::Progress(_)
            | Entry::Caret(_)
            | Entry::ForegroundMarker { .. }
            | Entry::Press(_)
            | Entry::Mark(..)
            | Entry::SayAllRead(..)
            | Entry::TerminalSpeech(_) => None,
        }
    }

    type Entry = Entry;
    type Facts = SrState;

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
            Entry::UiaChange(change) => Box::pin(crate::changes::uia_entry(cx, *change)),
            Entry::Progress(bar) => Box::pin(async move {
                let mut cx = cx;
                crate::progress::uia(&mut cx, &bar);
            }),
            Entry::Caret(caret) => Box::pin(crate::caret_tell::run(cx, *caret)),
            Entry::Press(press) => Box::pin(crate::keys::entry::run(cx, *press)),
            Entry::Mark(mark, _) => Box::pin(crate::say_all::mark_entry(cx, mark)),
            Entry::SayAllRead(run, _) => Box::pin(crate::say_all::read_entry(cx, run)),
            Entry::Announcement(announcement) => {
                crate::announcements::Announcement::run(cx, *announcement)
            }
            Entry::TerminalSpeech(speech) => Box::pin(async move {
                let mut cx = cx;
                crate::terminal::reading::speech_turn(&mut cx, *speech);
            }),
            Entry::ForegroundMarker {
                window, root_owner, ..
            } => Box::pin(async move {
                let mut cx = cx;
                cx.facts_mut()
                    .versions
                    .foreground_marker(window, root_owner);
                crate::focus::shown::marker(&mut cx, window);
            }),
        }
    }

    fn asks_nothing(entry: &Entry) -> bool {
        matches!(entry, Entry::TerminalSpeech(_) | Entry::Progress(_))
    }

    fn ends(
        facts: &SrState,
        (front, live): (&Entry, &Live),
        cause: Cause<'_, Entry>,
        (backend, now_us, foreground): (&BackendCache, u64, Option<Foreground>),
    ) -> Option<CancelReason> {
        match cause {
            Cause::Arrived(arriving) => withdraws(
                facts,
                (front, &live.request),
                arriving,
                backend,
                (now_us, foreground),
            )
            .then_some(CancelReason::Withdrawn),
            Cause::Foreground => (!exempt_from_foreground(front, &live.window)
                && foreground.is_some_and(|record| !record.holds(&live.window)))
            .then_some(CancelReason::Foreground),
            Cause::Settled => {
                unneeded(facts, front, &live.request).then_some(CancelReason::Withdrawn)
            }
        }
    }

    fn arrival(facts: &mut SrState, front: Option<(&Entry, Option<&Question>)>, entry: &mut Entry) {
        // A UI Automation change or selection records whether its element
        // was on the focus line as it joined.
        match entry {
            Entry::UiaChange(change) => crate::changes::arrived(facts, front, change),
            Entry::Announcement(announcement) => announcement.arrived(facts),
            _ => {}
        }
        if let Entry::Press(press) = entry {
            facts.arrivals.key_joined(press.press.key_seq);
            // A key reaching the focused terminal is taken by its model as
            // it reaches the inbox, a held caret key's at its pass
            // (`docs/design/readers-tell-core-asks-build3.md`, "Keys and
            // ordering").
            crate::keys::terminal::arrived(facts, press);
            // A held key's stroke joins the stroke line as its report
            // arrives, in the hook's order.
            if let PressAction::CaretKey {
                held: Some(stroke), ..
            } = &press.press.action
            {
                facts.strokes.hold(
                    press.press.key_seq,
                    stroke.clone(),
                    Some(press.press.pressed_at_us),
                );
            }
        }
        // A caret tell's selection speech is its key's words, so it is
        // fenced as the key's.
        let fence = match entry {
            Entry::Caret(_) => facts.arrivals.key_fence(),
            _ => facts.arrivals.fence(),
        };
        entry.set_fence(fence);
    }

    fn stamp(entry: &Entry, said: &mut Said) {
        crate::keys::entry::stamp(entry.fence(), said);
    }

    fn joining(facts: &mut SrState) -> Vec<Entry> {
        crate::say_all::joining(facts)
            .map(|run| Entry::SayAllRead(run, Fence::default()))
            .into_iter()
            .collect()
    }

    fn ended_at_foreground(facts: &mut SrState, entry: &Entry, said: &mut Vec<Said>) {
        // A held key ended so says nothing; its key passes, its stroke
        // sent in press order (`books/dev/src/reference/crates/verbatim-core.md`, "The caret key on the event queue").
        if let Some(caret) = crate::keys::caret_key::of_entry(entry) {
            let now_us = facts.terminal_focus.now_us;
            said.extend(facts.strokes.pass(caret.key_seq, now_us).map(Said::Queue));
        }
    }

    fn due(facts: &SrState) -> Option<u64> {
        facts
            .strokes
            .next_release()
            .into_iter()
            .chain(crate::input_desktop::due(facts))
            .chain(crate::desktop::due(facts))
            .min()
    }

    fn tick(
        facts: &mut SrState,
        now_us: u64,
        foreground: Option<Foreground>,
        said: &mut Vec<Said>,
    ) -> Vec<Entry> {
        said.extend(facts.strokes.release_due(now_us).map(Said::Queue));
        crate::keys::terminal::released(facts, now_us / 1_000);
        said.extend(crate::desktop::tick(facts, now_us));
        crate::input_desktop::tick(facts, now_us, foreground.as_ref())
            .map(|read| Entry::CurrentFocus(Box::new(read)))
            .into_iter()
            .collect()
    }

    fn recovered(facts: &mut SrState, window: &RequestWindow) -> Option<Entry> {
        let now_us = facts.terminal_focus.now_us;
        crate::terminal::reading::recovered(facts, window, now_us);
        Some(Entry::CurrentFocus(Box::new(
            crate::focus::retake::recovered(facts, window),
        )))
    }

    fn wanted(facts: &mut SrState) -> Vec<crate::event_queue::CoreRequest> {
        facts.wants.take()
    }

    fn facts_answered(
        facts: &mut SrState,
        token: u64,
        outcome: Outcome<(verbatim_model::request::Reply, crate::event_queue::Stamps)>,
        (now_us, _): (u64, Option<Foreground>),
        said: &mut Vec<Said>,
    ) -> Vec<Entry> {
        if let Some(bar) = crate::progress::ended(facts, token) {
            let outcome = outcome.map(|(reply, _)| reply);
            said.extend(crate::progress::answered(facts, bar, outcome));
            return Vec::new();
        }
        let trace = facts.queue_trace.0;
        crate::terminal::reading::answered(facts, token, outcome, (now_us, trace), said)
    }

    fn started(facts: &SrState, foreground: &Foreground) -> Option<Entry> {
        crate::focus::retake::startup(facts, foreground)
            .map(|read| Entry::CurrentFocus(Box::new(read)))
    }

    fn reader_ended(
        facts: &mut SrState,
        instance: Instance,
        (foreground, retake): (Option<Foreground>, bool),
        said: &mut Vec<Said>,
    ) -> Vec<Entry> {
        if let crate::event_queue::Reader::Outpost(process) = instance.reader {
            let outpost = instance.outpost_id();
            facts.follows.outpost_ended(process, outpost);
            // The focused terminal and the models of the generation's
            // terminals go with it.
            said.extend(crate::terminal::reading::outpost_ended(facts, outpost));
            // Say-all's positions are anchors of the outpost's, which end
            // with it: say-all stops, its read withdrawn.
            said.extend(crate::say_all::outpost_ended(facts, outpost));
            // The navigator object is one of the outpost's held nodes,
            // which end with it (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation record"): its commands say there is
            // none until a focus is committed again.
            if facts
                .navigator_object()
                .is_some_and(|object| object.id.outpost() == Some(outpost))
            {
                facts.navigator = None;
            }
            // The review start marker's anchors ended with the generation
            // that minted them (`books/dev/src/reference/crates/verbatim-core.md`, "The reducer and its state").
            if facts
                .start_marker
                .is_some_and(|marker| marker.node.outpost() == Some(outpost))
            {
                facts.start_marker = None;
            }
        }
        let read = crate::focus::retake::reader_ended(facts, instance, foreground);
        read.filter(|_| retake)
            .map(|read| Entry::CurrentFocus(Box::new(read)))
            .into_iter()
            .collect()
    }

    fn watched(facts: &SrState, process: verbatim_model::Pid) -> bool {
        !facts.ignored.contains(&process)
    }

    fn window_destroyed(
        facts: &mut SrState,
        window: WindowHandle,
    ) -> Vec<crate::event_queue::CoreRequest> {
        facts.plain.window_destroyed(window);
        // A postponed dialog destroyed is said no more.
        if facts
            .versions
            .postponed
            .is_some_and(|postponed| postponed.dialog.handle == window)
        {
            facts.versions.postponed = None;
        }
        // Its answer goes to Core alone, which no terminal rule reads.
        facts
            .follows
            .window_destroyed(window)
            .into_iter()
            .map(FollowRequest::own)
            .collect()
    }

    fn plain_answer(
        facts: &SrState,
        request: &Question,
    ) -> Option<verbatim_model::request::Outcome> {
        crate::focus::plain::answer(&facts.plain, request)
    }

    fn dropped_at_arrival(
        facts: &SrState,
        ahead: (
            Option<&Entry>,
            &std::collections::VecDeque<crate::event_queue::Item<Entry>>,
        ),
        arriving: &Entry,
        backend: &BackendCache,
        now: (u64, Option<Foreground>),
    ) -> bool {
        crate::caret_tell::dropped_in_console(facts, arriving)
            || crate::focus::menus::opening_dropped(facts, ahead, arriving, backend, now)
    }

    fn place(_facts: &SrState, entry: &Entry) -> Place {
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
                accepted: acceptance::accepted(&candidate.tell).is_ok(),
                at_us: candidate.tell.at_us,
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
                accepted: acceptance::accepted(&candidate.tell).is_ok(),
                at_us: candidate.tell.at_us,
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
                accepted: change.accepted.is_ok(),
                at_us: change.tell.at_us,
            },
            Entry::UiaFocus(_) => Place::Other {
                merges: true,
                focus_kind: true,
                ends_run: false,
            },
            Entry::Caret(caret) => match &caret.target {
                crate::caret_tell::Target::System(ids) => Place::Msaa {
                    key: MsaaKey {
                        kind: caret.tell.kind,
                        window: ids.window,
                        object: ids.object,
                        child: ids.child,
                        thread: caret.tell.thread,
                    },
                    class: MsaaClass::Other,
                    accepted: acceptance::accepted(&caret.tell).is_ok(),
                    at_us: caret.tell.at_us,
                },
                crate::caret_tell::Target::Uia(_) => Place::Other {
                    merges: true,
                    focus_kind: false,
                    ends_run: false,
                },
            },
            // A UI Automation change is placed as NVDA's limiter places
            // it, with no thread cap: it counts `WinEvent`s only.
            Entry::UiaChange(_) | Entry::Progress(_) => Place::Other {
                merges: true,
                focus_kind: false,
                ends_run: false,
            },
            Entry::Announcement(announcement) => announcement.place(),
            Entry::CurrentFocus(_) | Entry::MenuCheck(_) => Place::Other {
                merges: false,
                focus_kind: true,
                ends_run: false,
            },
            Entry::Run(_)
            | Entry::ForegroundMarker { .. }
            | Entry::Press(_)
            | Entry::Mark(..)
            | Entry::SayAllRead(..)
            | Entry::TerminalSpeech(_) => Place::Other {
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
            (Entry::Caret(waiting), Entry::Caret(arriving)) => {
                crate::caret_tell::merges(waiting, arriving)
            }
            (Entry::Progress(waiting), Entry::Progress(arriving)) => waiting.merges(arriving),
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

    fn ends_at_foreground(facts: &SrState, entry: &Entry, foreground: &Foreground) -> bool {
        // A held key bound at its press to a node ends when that node's
        // window is outside the new foreground window's tree or owner
        // chain; once its stroke is sent it makes no request, and stays.
        if let Some(caret) = crate::keys::caret_key::of_entry(entry) {
            return caret.held
                && facts.strokes.status(caret.key_seq)
                    == crate::keys::strokes::StrokeStatus::Unsent
                && caret.bound.as_ref().is_some_and(|bound| {
                    !foreground.holds(&RequestWindow::of_served(&bound.window))
                });
        }
        let window = match entry {
            Entry::UiaFocus(focus) => request_window(&focus.tell),
            Entry::Candidate(candidate) | Entry::Menu(candidate) => Some(candidate.window),
            Entry::CurrentFocus(read) => Some(read.window),
            Entry::MsaaChange(change) => Some(change.window),
            Entry::Caret(caret) => caret.window(),
            Entry::Announcement(announcement) => {
                return announcement.window().is_some_and(|window| {
                    !foreground.holds(&window) && !announcement.exempt_from_foreground()
                });
            }
            // A command resolves its object at its turn and is tested only
            // when it asks.
            Entry::Run(_)
            | Entry::ForegroundMarker { .. }
            | Entry::MenuCheck(_)
            | Entry::UiaChange(_)
            | Entry::Progress(_)
            | Entry::Press(_)
            | Entry::Mark(..)
            | Entry::SayAllRead(..)
            | Entry::TerminalSpeech(_) => None,
        };
        window.is_some_and(|window| {
            !foreground.holds(&window)
                && !entry
                    .accepted()
                    .is_some_and(|accepted| accepted.is_ok_and(Accepted::exempt_from_foreground))
        })
    }

    fn probe(entry: &Entry) -> Option<Probe> {
        match entry {
            Entry::UiaFocus(focus) => focus.tell.checked.as_ref()?.probe,
            Entry::Announcement(announcement) => announcement.probe(),
            Entry::Progress(bar) => bar.tell.checked.as_ref()?.probe,
            Entry::Candidate(_)
            | Entry::Run(_)
            | Entry::ForegroundMarker { .. }
            | Entry::Menu(_)
            | Entry::MenuCheck(_)
            | Entry::CurrentFocus(_)
            | Entry::MsaaChange(_)
            | Entry::UiaChange(_)
            | Entry::Caret(_)
            | Entry::Press(_)
            | Entry::Mark(..)
            | Entry::SayAllRead(..)
            | Entry::TerminalSpeech(_) => None,
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
) -> Result<Vec<Said>, ReplayMismatch> {
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

/// What [`route`] made of a queue input.
#[derive(Debug)]
pub(crate) enum Routed {
    /// Core handled it: the shell's effects.
    Said(Vec<Said>),
    /// One of the speech side's reports or Core's timers, handled at once
    /// in the inbox by today's rules for the input it stands for, which
    /// the reducer runs as it runs that input, with its stamping, the
    /// terminal's interception and the queue's settling.
    AtOnce(Box<verbatim_model::Input>),
}

#[cfg(test)]
impl Routed {
    /// The shell's effects of an input Core handled in the queue.
    pub(crate) fn said(self) -> Vec<Said> {
        match self {
            Self::Said(said) => said,
            Self::AtOnce(input) => panic!("{input:?} is handled at once, not routed"),
        }
    }
}

/// Routes one queue input from the shell (`Input::Queue`) into the event
/// queue under `trace_id` at `at_us`, and returns what Core did as the
/// shell's effects: requests, cancels, hang checks and the sound, what
/// the entries' functions said, and the time Core next waits for; or, for
/// the speech side's reports and Core's timers, the input each stands
/// for, handled at once ([`Routed::AtOnce`]). A tell of a kind whose entry
/// a later step of the build adds is not taken.
#[expect(
    clippy::too_many_lines,
    reason = "one arm per queue input, with no wildcard"
)]
pub(crate) fn route(
    state: &mut SrState,
    runtime: &mut Runtime,
    (trace_id, at_us): (TraceId, u64),
    input: &QueueInput,
) -> Routed {
    state.terminal_focus.now_us = at_us;
    let input = match input {
        QueueInput::Tell { tell, .. } if tell.kind == EventKind::DesktopSwitch => {
            let Payload::DesktopSwitch { input_desktop_open } = tell.payload else {
                // A switch without its payload is the listener's fault.
                return Routed::Said(Vec::new());
            };
            return Routed::Said(desktop_switched(
                state,
                runtime,
                (trace_id, at_us),
                input_desktop_open,
            ));
        }
        // Every frame of an instance that is not its reader's current one
        // is dropped (`books/dev/src/reference/crates/verbatim-core.md`, "The outpost generation record").
        QueueInput::Tell { from, .. }
        | QueueInput::WindowDestroyed { from, .. }
        | QueueInput::WindowHung { from, .. }
            if !state.queue.admits(*from) =>
        {
            return Routed::Said(vec![Said::Queue(QueueEffect::StaleFrame(*from))]);
        }
        // A terminal's change is handled at once in the inbox
        // (`docs/design/readers-tell-core-asks-build3.md`, "Core's inbox:
        // the change tell"): its read is sent as the queue settles.
        QueueInput::Tell { tell, .. } if tell.kind == EventKind::TerminalChanged => {
            if let EventObject::Terminal { node, .. } = tell.object {
                crate::terminal::reading::told(state, node, tell.at_us, at_us);
            }
            Input::Settle
        }
        QueueInput::Tell { from, tell } => match Entry::of_tell(
            *from,
            (**tell).clone(),
            state.settings.report_background_progress_bars,
        ) {
            Some(Entry::UiaFocus(mut focus)) => {
                focus.alt_down = state.alt_down;
                Input::Entry(Entry::UiaFocus(focus))
            }
            Some(entry) => Input::Entry(entry),
            None => return Routed::Said(Vec::new()),
        },
        QueueInput::Answer { from, answer } => Input::Answer {
            from: *from,
            answer: answer.clone(),
        },
        QueueInput::RefusedHung(number) => Input::RefusedHung(*number),
        QueueInput::InFront { number, front } => Input::InFront {
            number: *number,
            front: front.clone(),
        },
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
            ghost: report.ghost,
            console: report.console,
            integrity: report.integrity,
            own_integrity: report.own_integrity,
        }),
        QueueInput::ReaderReady(instance) => Input::ReaderReady(*instance),
        QueueInput::ReaderEnded(instance) => Input::ReaderEnded(*instance),
        QueueInput::WindowDestroyed {
            window, process, ..
        } => Input::WindowDestroyed {
            window: *window,
            process: *process,
        },
        QueueInput::WindowHung { hung, .. } => {
            // A window no process is read as serving has no request out on
            // it, so none waits on its recovery.
            let Some(window) = hung_window(hung) else {
                return Routed::Said(Vec::new());
            };
            Input::WindowHung(window)
        }
        QueueInput::Due { in_front } => {
            // The checks after a menu, popup or switcher end that this wake
            // brings ask by what Windows named in front as the shell made
            // it (Dickson, 2026-11-26).
            if let Some(in_front) = in_front {
                crate::focus::menus::due(state, at_us, in_front);
            }
            Input::Tick
        }
        QueueInput::Press(press) => Input::Entry(Entry::Press(Box::new(PressEntry::new(
            trace_id,
            (**press).clone(),
        )))),
        QueueInput::MarkReached(mark) => Input::Entry(Entry::Mark(*mark, Fence::default())),
        QueueInput::AskOutside {
            asker,
            window,
            request,
        } => {
            // A window no process is read as serving has no outpost to
            // ask: its asker is answered at once.
            let Some(window) = RequestWindow::of_node(window) else {
                return Routed::Said(vec![Said::Queue(QueueEffect::Answered {
                    asker: *asker,
                    outcome: verbatim_model::request::Outcome::NotThere,
                })]);
            };
            Input::AskOutside {
                asker: Asker(*asker),
                reader: Reader::Outpost(window.process),
                window,
                request: request.clone(),
                due_after_us: OTHER_DUE_US,
            }
        }
        QueueInput::WithdrawOutside(asker) => Input::WithdrawOutside(Asker(*asker)),
        QueueInput::Ignored(processes) => {
            state.ignored.clone_from(processes);
            return Routed::Said(Vec::new());
        }
        QueueInput::Deferred { stroke, pressed } => {
            return Routed::Said(
                state
                    .strokes
                    .defer(*stroke, *pressed, at_us)
                    .map(Said::Queue)
                    .into_iter()
                    .collect(),
            );
        }
        QueueInput::Alt { down } => {
            state.alt_down = *down;
            return Routed::Said(Vec::new());
        }
        QueueInput::StrokesReset => {
            // The front's starting read, its stroke now dropped, is
            // withdrawn as the queue settles.
            state.strokes.drop_unsent();
            Input::Settle
        }
        QueueInput::Exit => {
            return Routed::Said(vec![Said::Queue(QueueEffect::ReleaseAtExit(
                state.strokes.exit(),
            ))]);
        }
        QueueInput::Lock(lock) => {
            crate::input_desktop::lock(state, *lock);
            return Routed::Said(Vec::new());
        }
        QueueInput::Process(fact) => Input::Process(*fact),
        QueueInput::SpeechCut { key } => {
            // A key press cut speech off: what arrives from now on speaks
            // past the cut, a terminal drops what waits, and say-all stops,
            // as NVDA's stops on any key.
            state.arrivals.speech_cut(*key);
            let mut said = crate::terminal::cut(state, at_us / 1_000);
            said.extend(crate::say_all::stop(state));
            // A say-all stopped with its read out withdraws it.
            said.extend(settle(state, runtime, trace_id));
            return Routed::Said(said);
        }
        QueueInput::UtteranceCancelled { marks, at_ms } => {
            return Routed::AtOnce(Box::new(verbatim_model::Input::UtteranceCancelled {
                marks: marks.clone(),
                at_ms: *at_ms,
            }));
        }
        QueueInput::UtteranceFailed { marks } => {
            return Routed::AtOnce(Box::new(verbatim_model::Input::UtteranceFailed {
                marks: marks.clone(),
            }));
        }
        QueueInput::TerminalDue { at_ms } => {
            return Routed::AtOnce(Box::new(verbatim_model::Input::TerminalTimer {
                trace_id,
                at_ms: *at_ms,
            }));
        }
        QueueInput::LockKeyRead { key, on } => {
            return Routed::AtOnce(Box::new(verbatim_model::Input::LockKeyToggled {
                trace_id,
                key: *key,
                on: *on,
            }));
        }
    };
    Routed::Said(run_input(state, runtime, (trace_id, at_us), input))
}

/// A switch of the input desktop at `at_us`, handled at once in the inbox
/// (`books/dev/src/reference/crates/verbatim-core.md`, "The secure desktop and the lock state"): what it says first, then its `Follow`s sent as Core's own
/// requests, and the queue settled, so that a starting read whose stroke
/// was dropped is withdrawn.
fn desktop_switched(
    state: &mut SrState,
    runtime: &mut Runtime,
    (trace_id, at_us): (TraceId, u64),
    open: bool,
) -> Vec<Said> {
    let switched = crate::input_desktop::switched(state, open, at_us);
    let requests = switched
        .follows
        .into_iter()
        .map(FollowRequest::own)
        .collect();
    let mut said = switched.said;
    said.extend(run_input(
        state,
        runtime,
        (trace_id, at_us),
        Input::Send(requests),
    ));
    said
}

/// Whether Core's facts, changed outside the queue, leave it to settle: a
/// say-all wants its next read, or stopped while its read was out
/// ([`settle`]).
pub(crate) fn unsettled(state: &SrState) -> bool {
    state.wants.any()
        || state
            .say_all
            .as_ref()
            .is_some_and(|say_all| say_all.run.read == crate::say_all::QueueRead::Wanted)
        || state
            .queue
            .front_ends::<Entries>(state, Cause::Settled)
            .is_some()
}

/// Settles the queue under `trace_id` after Core's facts changed outside
/// it, as the speech side's cut or failure changes say-all until they
/// reach the inbox: what they want joins the
/// back, and a request no longer needed is withdrawn.
pub(crate) fn settle(state: &mut SrState, runtime: &mut Runtime, trace_id: TraceId) -> Vec<Said> {
    let at_us = state.queue.now_us();
    run_input(state, runtime, (trace_id, at_us), Input::Settle)
}

/// Feeds `input` to the queue under `trace_id` at `at_us`, and returns
/// what Core did as the shell's effects.
fn run_input(
    state: &mut SrState,
    runtime: &mut Runtime,
    (trace_id, at_us): (TraceId, u64),
    input: Input<Entry>,
) -> Vec<Said> {
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
    let mut said = effects;
    let wake = state
        .queue
        .next_due()
        .into_iter()
        .chain(<Entries as Logic>::due(state))
        .min();
    said.push(Said::Queue(QueueEffect::WakeAt(wake)));
    said
}

#[cfg(test)]
#[path = "entries_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "window_destroyed_tests.rs"]
mod window_destroyed_tests;
