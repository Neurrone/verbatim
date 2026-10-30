//! The messages between verbatim.exe and its children, the focus listener
//! and the outposts, and the types they carry (milestone M4.4,
//! `docs/design/ownership-messages.md`): one enum for each direction and
//! role ([`CoreToOutpost`], [`CoreToListener`], [`OutpostToCore`] and
//! [`ListenerToCore`]), each variant of a declared kind ([`Kind`]). The
//! framing that carries them on the pipes is the outpost crate's own;
//! these are pure data, and their methods pure functions of it.

#![deny(clippy::wildcard_enum_match_arm)]

use std::time::Duration;

mod merge;
mod responder;
pub mod samples;

pub use merge::{Barrier, EventKey, Merge, merge_into};
pub use responder::Responder;

use serde::{Deserialize, Serialize};

use crate::{
    Backend, CallCounts, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    NotificationKind, NotificationProcessing, OutpostId, Pid, QueryKind, Role, StateSet, TextOp,
    TextReply, TraceId, TreeNode, WindowFacts,
};

/// When an event reached each point on its way through the listener and
/// the outpost, for the latency log Core writes for every announcement:
/// microseconds since the Unix epoch, 0 for a point it did not pass. Also
/// the cross-process calls the outpost made for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EventTiming {
    /// How many milliseconds before it was observed Windows raised the
    /// event; only `WinEvents` carry the time they were raised.
    pub raised_ms_ago: Option<u32>,
    /// When the listener or the outpost observed it: for a fact the
    /// listener sent, the observation stamp by which the fact is ordered
    /// against other observations.
    pub observed_at_us: u64,
    /// When the focus listener finished handling this fact and queued it
    /// for its writer: for a fact a reading thread judged (a UIA focus's or
    /// menu opening's read or drop, a selection's or notification's check),
    /// when that thread finished with it; for any other (a `WinEvent`'s
    /// fact, a marker, a drop decided without a read, a menu's end), when
    /// the listener queued it. 0 for anything the listener did not send.
    pub listener_handled_at_us: u64,
    /// When the focus listener began writing the frame that carries this
    /// fact to verbatim.exe's pipe, taken just before the frame is encoded;
    /// 0 for anything the listener did not send.
    pub listener_written_at_us: u64,
    /// When the outpost received it from Core, for an event the listener
    /// observed.
    pub relayed_at_us: u64,
    /// When the outpost's worker took it from its queue.
    pub dequeued_at_us: u64,
    /// When the evidence that answered a caret key's watch, after its first
    /// check found none, was taken from the outpost's queue
    /// (`TextOp::AwaitCaret`); 0 for anything else.
    pub awaited_at_us: u64,
    /// When the outpost's worker published the resulting event or reply,
    /// queuing it for the outpost's writer.
    pub published_at_us: u64,
    /// When the outpost's writer began writing the message carrying this
    /// timing to verbatim.exe's pipe, taken just before the message is
    /// encoded; 0 until then.
    pub written_at_us: u64,
    /// The cross-process calls the outpost's worker made between taking the
    /// entry from its queue and publishing this message, by kind
    /// (`docs/performance.md`). Zero from the listener, which makes none, and
    /// from an older peer.
    pub calls: CallCounts,
    /// The part of `calls` a caret key's watch made in the checks that found
    /// no evidence, before `awaited_at_us`.
    pub awaited_calls: CallCounts,
    /// When verbatim.exe took in what it relayed to the outpost: for a
    /// listener's fact, when its relay thread took the read's completion;
    /// for a request Core made, when the request was queued for the
    /// outpost (`docs/design/communication.md`, section 10.1). 0 for an event the
    /// outpost observed itself.
    pub core_received_at_us: u64,
    /// How long verbatim.exe held it, in microseconds, measured on one
    /// clock in verbatim.exe from `core_received_at_us`'s moment to the
    /// moment its relay thread began encoding it for the write to the
    /// outpost; the time waiting in the outpost's command queue counts, and
    /// so does `core_held_us`.
    pub core_relay_us: u64,
    /// The part of `core_relay_us` a listener's fact was held while its
    /// outpost started, before the outpost said it was ready; 0 for a fact
    /// delivered at once.
    pub core_held_us: u64,
    /// `core_received_at_us` plus `core_relay_us`: when verbatim.exe
    /// relayed it, on the clock the other stamps share.
    pub core_sent_at_us: u64,
    /// When verbatim.exe read the outpost's message carrying this timing
    /// off the outpost's pipe, parsed; 0 until then.
    pub core_read_at_us: u64,
}

/// The time from the stamp `from_us` to the stamp `to_us`, in
/// microseconds, or `None` when either is 0, a point not stamped (a message
/// from an older peer).
#[must_use]
pub fn hop_us(from_us: u64, to_us: u64) -> Option<u64> {
    (from_us != 0 && to_us != 0).then(|| to_us.saturating_sub(from_us))
}

/// The identity-free contents of a UIA focus element, as the focus listener
/// captures them (decision D13): the runtime id plus role, name, value,
/// states, and details — everything a [`NodeSnapshot`](crate::NodeSnapshot)
/// carries except its [`NodeId`](crate::NodeId). The node id is
/// deliberately absent: identity is minted per application inside that
/// application's own outpost and never crosses a process boundary, so the
/// receiving outpost mints it from the runtime id when it rebuilds the
/// snapshot. Mirrors `verbatim_uia::map::CachedUiaParts`, which is what the
/// listener reads by cached property access.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiaSnapshotFact {
    /// The UIA runtime id, the key the receiving outpost mints its node id from.
    pub runtime_id: Vec<i32>,
    /// Normalized role (already refined for toggle buttons).
    pub role: Role,
    /// Accessible name, if any.
    pub name: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Current states.
    pub states: StateSet,
    /// The optional properties beyond the core four.
    pub details: NodeDetails,
}

/// One raw-view ancestor of a UIA focus, as the focus listener read it from
/// the event's own element: its cached parts, its own native window handle
/// (0 for an element that is not a window), and whether UIA counts it both
/// a control and content. The outpost filters what it reports from these
/// and finds its own elements by the runtime ids below a window
/// (`docs/design/focus-pipeline.md`, section 12.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiaAncestorFact {
    /// The ancestor's cached parts.
    pub snapshot: UiaSnapshotFact,
    /// Its own native window handle, or 0.
    pub hwnd: isize,
    /// Whether it is both a control and content element.
    pub content: bool,
}

/// Where the chain above a UIA focus read's ancestors goes on, as the
/// listener's walk ended (`docs/design/focus-pipeline-rework.md`, "A
/// window's name, and the one fallback"). The listener connects to no
/// window, so it names the window and the outpost of the focus's
/// application goes on from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AncestorsEnd {
    /// The ancestors reach the desktop root, the process boundary, or a
    /// known ancestor, and the rest as read before: the outpost goes on
    /// above the boundary, or from a window it reads through MSAA.
    #[default]
    Top,
    /// A step to a parent failed or found none below the root: the chain
    /// goes on from this window's own object, the parent window of the
    /// outermost ancestor's nearest window, as NVDA's parent read falls
    /// back to the window hierarchy (`Window._get_parent`); 0 for none. Or
    /// a step reached this window, a ghost's or a hung application's,
    /// other than the nearest window: the chain goes on from it, which the
    /// outpost reads as a plain window, as NVDA does after that one step.
    ContinueFrom(isize),
    /// The walk was cut by the read's budget or its depth limit: the chain
    /// is completed by plain windows from this window, included: the
    /// outermost ancestor's nearest window, or its parent window when that
    /// ancestor is the window's own element
    /// ([`verbatim_uia::plain_windows_start`]).
    PlainFrom(isize),
}

/// A UIA focus as the focus listener judged and read it from the event's
/// own element, on the thread for the element's application
/// (`docs/design/focus-pipeline.md`, section 12.1): the element had the
/// keyboard focus, read live, when it was read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiaFocusRead {
    /// The element's cached native window handle, or 0 when it is not a
    /// window in its own right.
    pub hwnd: isize,
    /// The focus: its name and role from the event's cache, as NVDA reads
    /// them from the event's element, and its value, states, and details
    /// read afresh.
    pub snapshot: UiaSnapshotFact,
    /// Its raw-view ancestors, nearest first, up to its process's
    /// top-level window: read from the element up to the first one the
    /// thread already knew from the focus before, and the rest as they were
    /// read then. Where the walk stopped short, [`Self::ancestors_end`]
    /// names the window the outpost completes the chain from.
    pub ancestors: Vec<UiaAncestorFact>,
    /// Where the chain goes on above them, as the walk ended.
    #[serde(default)]
    pub ancestors_end: AncestorsEnd,
    /// For a list or tab control, its first selected child; never for a
    /// focus whose value is its first selected item
    /// ([`Self::value_from_selection`]).
    pub selected_child: Option<UiaSnapshotFact>,
    /// Whether the focus's value is its first selected item's name, as
    /// NVDA reads it for a combo box without the `Value` pattern and a
    /// looping selector (`verbatim_uia::map::value_from_selection`): that
    /// item is then its value, never its selected child, and a later
    /// selection of one of its items is a change of its value.
    pub value_from_selection: bool,
    /// The element's own window or its nearest ancestor's (NVDA's
    /// `getNearestWindowHandle`), when it was found.
    pub window: Option<isize>,
    /// Whether the event's runtime id still names the element it named
    /// before: `None` when it is not that of the last focus the listener's
    /// thread read for the application, or the read was cut before that
    /// element's; else what was read live of that earlier element with this
    /// focus. Not focused or gone means the application gave the id to a
    /// new element after the one it named died: the node keeps its id, and
    /// the outpost evicts the element it kept for it, dropping its text
    /// caches and terminal entry too when it is gone; focused lets the
    /// outpost keep the element it found for the node, as it does for a
    /// repeat read while the focus before it waited for Core's judgement
    /// (`docs/design/focus-pipeline-rework.md`, "One object, one identity").
    pub same_as_last: Option<crate::EarlierElement>,
    /// The bound of the read's walk, in microseconds from the event's
    /// observation (`docs/design/focus-pipeline-rework.md`, the Terms):
    /// `MOVED_ON_GRACE` when the focus's top-level window is not that of
    /// the previous focus the listener observed, and the read's budget
    /// otherwise. The outpost's continuation of the chain ends by it too.
    pub bound_us: u64,
}

/// The thread that raised an MSAA `WinEvent` and when, as every hook that
/// receives the event is told (`idEventThread` and `dwmsEventTime`): with
/// the event's kind and address, what tells the listener's copy of an event
/// and the outpost's own hook's copy apart from other events
/// (`docs/design/focus-pipeline.md`, section 12.4, "The MSAA hand-over,
/// exactly"). The thread is also the one NVDA's limiter counts the event
/// by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WinEventStamp {
    /// The thread that raised the event.
    pub thread: u32,
    /// When Windows raised it, on the clock `verbatim_ia2::event_clock`
    /// reads, which wraps after 49.7 days.
    pub time: u32,
}

impl WinEventStamp {
    /// Whether the event was raised after the clock read `tick`.
    #[must_use]
    pub fn raised_after(self, tick: u32) -> bool {
        tick_is_after(self.time, tick)
    }

    /// Whether the event was raised before the clock read `tick`.
    #[must_use]
    pub fn raised_before(self, tick: u32) -> bool {
        tick_is_after(tick, self.time)
    }
}

/// Whether tick `a` comes after tick `b` on the event clock, which wraps:
/// two ticks are compared by their difference, so the comparison holds
/// across a wrap for times less than 24 days apart.
#[must_use]
pub fn tick_is_after(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < 1 << 31
}

/// A fact the listener forwards to Core, tagged with the pid Core routes it
/// to (decision D13). An MSAA fact forwards the raw `WinEvent` address
/// untouched, and a UIA selection, menu, or notification fact is built
/// from the element's cached properties, with hang-safe local reads only
/// (the owning pid, a cached window handle). A UIA focus is a marker sent
/// as the event arrives, then the read the listener's thread for the
/// application made from the event's own element
/// ([`DeliveredFact::UiaFocusRead`]), or its drop.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ListenerFact {
    /// The process whose outpost handles the fact: the application it
    /// concerns, or, for a console window, its console host.
    pub pid: Pid,
    /// The fact itself, as the target outpost receives it.
    pub fact: DeliveredFact,
}

impl ListenerFact {
    /// The pid the supervisor routes this fact to.
    #[must_use]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Strips the pid, yielding the [`DeliveredFact`] the supervisor hands to
    /// the target outpost once routing is done.
    #[must_use]
    pub fn into_delivered(self) -> DeliveredFact {
        self.fact
    }
}

/// What a UIA focus's outcome says of its application's gap record, the
/// outpost's wait, after a `WinEvent` focus it dropped because it judged
/// the window UIA, for a UIA focus that answers it
/// (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
/// fact", the gap's evidence). The listener computes it from each drop
/// and sends it with the drop; the outpost places a read itself. It goes
/// on the pipe as JSON; the listener, the supervisor and the outposts are
/// one build of this definition, so it needs no version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GapAnswer {
    /// It answers a copy in the top-level window given, or, with `None`,
    /// a copy in any window of the application.
    Answers(Option<isize>),
    /// It answers a copy in any window of the application, which is to be
    /// read at once in its place: the event came, and its sender has no
    /// window above it, as a sender gone before its read has, so the event
    /// for where the focus went may never come
    /// (`docs/design/listener-without-connections.md`, risk 13).
    ReadsNow,
    /// It leaves the record open.
    LeavesOpen,
}

/// A fact the supervisor delivers to a target outpost (decision D13): what
/// the listener captured, minus the pid, since routing is already done. The
/// outpost's worker reads, arbitrates, and reports it exactly as it does the
/// events it hooks itself; its own hook makes the same facts of the MSAA
/// focus, foreground, menu, alert, and tooltip events it takes in
/// (the outpost crate's `fact_of_win_event`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeliveredFact {
    /// The focus listener received a UIA focus event in this application
    /// and is reading it: entries the outpost takes in after this wait
    /// until the read arrives or is dropped, and the focus is handled in
    /// this place (`docs/design/focus-pipeline.md`, section 12.4, decision
    /// 1 of section 15), as NVDA's UIA events pass in order through the
    /// one thread that judges them.
    UiaFocusMarker {
        /// The listener's number for this focus event, which its read or
        /// drop carries.
        marker: u64,
    },
    /// The focus listener's read of the UIA focus event `marker` names.
    UiaFocusRead {
        /// The marker the read resolves.
        marker: u64,
        /// The focus as read.
        read: Box<UiaFocusRead>,
        /// Whether the event read was a focus rather than a menu opening.
        focus: bool,
    },
    /// The UIA focus event `marker` names is not reported: its element had
    /// lost the keyboard focus when it was read, as NVDA drops such an
    /// event, or its window was hung, the console window's own, or of a
    /// class on NVDA's MSAA list; or its read did not return in time.
    UiaFocusDropped {
        /// The marker the drop resolves.
        marker: u64,
        /// Whether the event dropped was a focus rather than a menu
        /// opening, a selection, or a notification.
        focus: bool,
        /// What the drop says of its application's gap record, placed in
        /// the top-level window of the window its read found.
        answer: GapAnswer,
    },
    /// The focus listener that sent the markers still waiting has ended:
    /// none of them will be resolved, so none is waited for any more. Made
    /// by the supervisor when it replaces the listener.
    UiaFocusReadsEnded,
    /// An MSAA focus change: the raw `WinEvent` address.
    MsaaFocus {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A foreground change: the new foreground window.
    Foreground {
        /// The new foreground window handle.
        hwnd: isize,
    },
    /// A popup menu opening: the raw `WinEvent` address.
    MenuPopup {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A UIA event the focus listener took to its application's reading
    /// thread for NVDA's checks of its sender (a selection's or a
    /// notification's nearest window), behind marker `marker`: what the
    /// thread found, which takes the marker's place in the outpost's
    /// intake as a focus's read does.
    UiaMarked {
        /// The marker this resolves.
        marker: u64,
        /// The event, as the reading thread passed it.
        fact: Box<DeliveredFact>,
    },
    /// A UIA element selected within its container
    /// (`SelectionItem_ElementSelected`).
    UiaSelection {
        /// The element's cached native window handle, or 0.
        hwnd: isize,
        /// For an element that is not a window: its nearest window, which
        /// the listener's reading thread walked up to as NVDA does
        /// (`getNearestWindowHandle`), else 0. The window the outpost tests
        /// the selection's window and acceptance with, as NVDA tests them
        /// with the nearest window.
        #[serde(default)]
        focus_window: isize,
        /// The selected element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
    },
    /// A UIA notification (`AutomationNotification`).
    UiaNotification {
        /// The element's cached native window handle, or 0.
        hwnd: isize,
        /// The raising element's cached snapshot parts.
        snapshot: UiaSnapshotFact,
        /// The notification.
        notification: Notification,
    },
    /// An MSAA alert (`EVENT_SYSTEM_ALERT`): the raw `WinEvent` address.
    Alert {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
    /// A tooltip window shown (`EVENT_OBJECT_SHOW` on `tooltips_class32`):
    /// the raw `WinEvent` address.
    Show {
        /// The event's window handle.
        hwnd: isize,
        /// The event's `idObject`.
        id_object: i32,
        /// The event's `idChild`.
        id_child: i32,
    },
}

impl DeliveredFact {
    /// Whether this fact is an MSAA `WinEvent`'s, which an outpost's own
    /// hook takes in once it is installed, and the listener forwards only
    /// until then (`docs/design/focus-pipeline.md`, section 12.4).
    #[must_use]
    pub fn is_win_event(&self) -> bool {
        matches!(
            self,
            DeliveredFact::MsaaFocus { .. }
                | DeliveredFact::Foreground { .. }
                | DeliveredFact::MenuPopup { .. }
                | DeliveredFact::Alert { .. }
                | DeliveredFact::Show { .. }
        )
    }

    /// Whether this fact is part of a UIA focus's read: a marker, the read,
    /// a drop, a marked selection or notification, which resolves its
    /// marker as a read does, or the end of a listener's reads. The
    /// supervisor relays these in order and never drops one, since an
    /// outpost waits on a marker until its read or drop arrives.
    #[must_use]
    pub fn is_focus_read(&self) -> bool {
        matches!(
            self,
            DeliveredFact::UiaFocusMarker { .. }
                | DeliveredFact::UiaFocusRead { .. }
                | DeliveredFact::UiaFocusDropped { .. }
                | DeliveredFact::UiaMarked { .. }
                | DeliveredFact::UiaFocusReadsEnded
        )
    }

    /// The object and kind this fact concerns, for NVDA's limiter rule (one
    /// waiting entry per object and kind, a newer one replacing it); for a
    /// notification, its element, kind, processing, and activity id, not
    /// its text, as NVDA's UIA limiter keys one (`eventRecord.h`), so a
    /// newer notification of the same activity replaces one still waiting.
    /// A `WinEvent`'s fact other than a menu opening (a foreground change,
    /// an MSAA focus, an alert, or a tooltip shown) is keyed by the thread
    /// that raised it too, from `win_event` (0 without it), as NVDA's
    /// `WinEvent` limiter keys those events (`orderedWinEventLimiter.py` 63
    /// to 75 and 79), so the facts of one address from two threads are not
    /// merged. A menu opening is keyed by its address alone: NVDA's limiter
    /// keeps only its most recent menu event, whatever its address or
    /// thread (76 to 78), which the outpost's intake follows per
    /// application, for popup starts, by handling only a batch's newest
    /// menu opening (NVDA's one slot also holds menu starts and ends and
    /// covers every application: `docs/parity/backends.md`, the limiter's
    /// entry).
    /// `None` for the parts of a UIA focus's read, which are never replaced
    /// on the way: the outpost coalesces a read only where it takes its
    /// marker's place.
    #[must_use]
    pub fn key(&self, win_event: Option<WinEventStamp>) -> Option<FactKey> {
        let thread = win_event.map_or(0, |stamp| stamp.thread);
        Some(match self {
            DeliveredFact::Foreground { hwnd } => FactKey::Foreground(*hwnd, thread),
            DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            } => FactKey::MsaaFocus(*hwnd, *id_object, *id_child, thread),
            DeliveredFact::UiaFocusMarker { .. }
            | DeliveredFact::UiaFocusRead { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded
            | DeliveredFact::UiaMarked { .. } => return None,
            DeliveredFact::UiaNotification {
                snapshot,
                notification,
                ..
            } => FactKey::UiaNotification(
                snapshot.runtime_id.clone(),
                notification.kind,
                notification.processing,
                notification.activity_id.clone(),
            ),
            DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            } => FactKey::MenuPopup(*hwnd, *id_object, *id_child),
            DeliveredFact::UiaSelection { snapshot, .. } => {
                FactKey::UiaSelection(snapshot.runtime_id.clone())
            }
            DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            } => FactKey::Alert(*hwnd, *id_object, *id_child, thread),
            DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            } => FactKey::Show(*hwnd, *id_object, *id_child, thread),
        })
    }
}

/// The object and kind a [`DeliveredFact`] concerns.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FactKey {
    /// A foreground change to this window, raised by this thread.
    Foreground(isize, u32),
    /// An MSAA focus at this address, raised by this thread.
    MsaaFocus(isize, i32, i32, u32),
    /// An MSAA menu opening at this address, whatever thread raised it.
    MenuPopup(isize, i32, i32),
    /// A UIA selection of this runtime id.
    UiaSelection(Vec<i32>),
    /// A UIA notification of this runtime id, with this kind, processing,
    /// and activity id.
    UiaNotification(
        Vec<i32>,
        NotificationKind,
        NotificationProcessing,
        Option<String>,
    ),
    /// An MSAA alert at this address, raised by this thread.
    Alert(isize, i32, i32, u32),
    /// A tooltip shown at this address, raised by this thread.
    Show(isize, i32, i32, u32),
    /// The window in the foreground, whichever it is: only the newest
    /// report waits ([`ListenerToCore::ForegroundWindow`]).
    ForegroundWindow,
}

/// Messages from verbatim.exe to an outpost.
///
/// An outpost's target application is set once (decision D9: one
/// outpost per application, for its whole life): passed on its command
/// line, or, for a spare started before it had one, by
/// [`CoreToOutpost::Assign`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a message is built once and moved, never stored in bulk"
)]
pub enum CoreToOutpost {
    /// Forces one backend for every window of the target application,
    /// overriding arbitration; used by tests. `None` restores normal
    /// arbitration. One forcing UIA is sent to the focus listener too
    /// ([`CoreToListener::SetBackendOverride`]), so that its reads skip no
    /// focus of that application by its window's class, the outpost then
    /// counting every UIA focus (`docs/design/focus-pipeline-rework.md`,
    /// the verdicts); sent as two messages, the two processes can differ
    /// while one is in flight.
    SetBackendOverride {
        /// The forced backend, or `None` for normal arbitration.
        backend_override: Option<Backend>,
    },
    /// Delivers a focus fact the focus listener captured, for this outpost's
    /// target application (decision D13), threading the listener's
    /// `trace_id` and observation (`timing`'s `observed_at_us`) through to
    /// the emitted event so the event is ordered by, and the latency
    /// timeline starts at, the real OS event rather than this delivery.
    DeliverFact {
        /// Trace ID the listener minted when it observed the OS event.
        trace_id: TraceId,
        /// When it was raised and observed: its `observed_at_us` is the
        /// listener's observation, the first point on the keypress-to-audio
        /// timeline.
        #[serde(default)]
        timing: EventTiming,
        /// The routed fact, minus the pid (routing is done).
        fact: DeliveredFact,
        /// For an MSAA `WinEvent`'s fact, the thread that raised it and
        /// when, by which the outpost matches it with its own hook's copy
        /// and counts it in NVDA's limits; `None` for a UIA fact.
        #[serde(default)]
        win_event: Option<WinEventStamp>,
    },
    /// The listener forwards none of the application's `WinEvent` facts
    /// raised after `through`, and every one it forwarded before is ahead
    /// of this message, so the listener's copies and the outpost's hook's
    /// are matched from now on only for events raised while the hook went
    /// in (`docs/design/focus-pipeline.md`, section 12.4, "The overlap's
    /// ends"). Relayed in order behind every fact the listener sent before
    /// it ([`ListenerToCore::ForwardingEnded`]).
    ForwardingEnded {
        /// The listener's clock when it stopped, on the `WinEvent` clock.
        through: u32,
    },
    /// A query, answered by exactly one [`OutpostToCore::Reply`] carrying
    /// the same `request_id`.
    Query {
        /// Trace ID of the input that caused the query.
        trace_id: TraceId,
        /// Core's id for this request, echoed in the reply.
        request_id: u64,
        /// What to do.
        query: Query,
        /// verbatim.exe's relay stamps (`EventTiming::core_received_at_us`
        /// and the two after it), filled in as the request is written, which
        /// the outpost carries into its reply's timing; default as sent.
        #[serde(default)]
        timing: EventTiming,
    },
    /// Withdraws a query that has not started: it is answered
    /// [`QueryOutcome::NotStarted`] instead of being run. A query already
    /// running is not affected.
    Cancel {
        /// The query to withdraw.
        request_id: u64,
    },
    /// Liveness probe; answered by [`OutpostToCore::Pong`].
    Ping {
        /// Echoed in the matching pong.
        seq: u64,
        /// When verbatim.exe began writing this command, in microseconds
        /// since the Unix epoch ([`CoreToOutpost::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// The details the active theme wants read for each node
    /// (`SrState::fetches` in `verbatim-core`): a detail whose indication
    /// is off is not read, which saves its cross-process call. Sent when an
    /// outpost starts and whenever the theme in use changes; everything is
    /// read until the first arrives.
    Fetches(crate::Fetches),
    /// The nodes from this outpost that Core still holds
    /// (`docs/crates/verbatim-outpost.md`, held objects), as the numbers the
    /// outpost issued, and the position of the last message from this
    /// outpost that Core has handled ([`OutpostToCore::carries_nodes`] says
    /// which messages count). The outpost releases every other node it
    /// reported at or before that position, and answers a query for a
    /// released node "gone".
    NodesHeld {
        /// The held nodes' numbers.
        nodes: Vec<u64>,
        /// The text anchors Core holds in this outpost
        /// (`SrState::held_anchors` in `verbatim-core`): the outpost keeps
        /// these, and may forget any other once it has minted newer ones
        /// (the text protocol, `docs/crates/verbatim-model.md`).
        #[serde(default)]
        anchors: Vec<u64>,
        /// The position of the last message Core has handled.
        acknowledged: u64,
    },
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): answered
    /// [`OutpostToCore::IdleAnswer`] once the outpost's worker has handled
    /// everything that reached its intake before the query, with the caret
    /// those entries noted, and no caret key's watch or owed terminal read
    /// is open; behind every message it sent before, never merged and never
    /// ahead as urgent.
    IdleQuery {
        /// The round, echoed in the answer.
        round: u64,
    },
    /// Shut down cleanly and exit: take no new work, let the call or remote
    /// operation in progress finish (UIA's own timeouts end one the
    /// application never answers), remove every UIA event handler and
    /// `WinEvent` hook, release every object held, leave COM, and exit with
    /// code 0. The supervisor kills the process through its job only if it
    /// has not exited within its time limit. Nothing is answered: the pipe
    /// closing and the process exiting are the answer.
    Shutdown,
    /// To a spare outpost (`docs/design/focus-pipeline.md`, section 19): the
    /// application it watches from now on, for the rest of its life. It
    /// installs its process-scoped `WinEvent` hook for that application,
    /// starts waiting for its exit, and sends [`OutpostToCore::Ready`]; or,
    /// when the hook cannot be installed, reports the failure
    /// ([`OutpostToCore::Fault`]) and exits without `Ready`. Sent once,
    /// before any other command but a ping; an outpost started for an
    /// application is never sent it.
    Assign {
        /// The application.
        target_pid: Pid,
    },
}

impl CoreToOutpost {
    /// Stamps a ping with `now`, read just before its write begins. Reads
    /// no clock for any other command.
    pub fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        if let CoreToOutpost::Ping { sent_at_us, .. } = self {
            *sent_at_us = now();
        }
    }

    /// When verbatim.exe began writing this command, for a ping; 0 for any
    /// other.
    #[must_use]
    pub fn sent_at_us(&self) -> u64 {
        if let CoreToOutpost::Ping { sent_at_us, .. } = self {
            *sent_at_us
        } else {
            0
        }
    }

    /// What kind of message this is.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            CoreToOutpost::Query { .. }
            | CoreToOutpost::Cancel { .. }
            | CoreToOutpost::IdleQuery { .. } => Kind::Ask,
            CoreToOutpost::NodesHeld { .. } => Kind::Release,
            CoreToOutpost::Assign { .. }
            | CoreToOutpost::Ping { .. }
            | CoreToOutpost::Shutdown
            | CoreToOutpost::ForwardingEnded { .. } => Kind::Lifecycle,
            CoreToOutpost::DeliverFact { .. } => Kind::Routed,
            CoreToOutpost::Fetches(_) | CoreToOutpost::SetBackendOverride { .. } => {
                Kind::Configuration
            }
        }
    }
}

/// Messages from verbatim.exe to the focus listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreToListener {
    /// Forces one backend for the application `pid`, as its outpost is told
    /// ([`CoreToOutpost::SetBackendOverride`]): one forcing UIA makes the
    /// listener's reads skip no focus of that application by its window's
    /// class.
    SetBackendOverride {
        /// The forced backend, or `None` for normal arbitration.
        backend_override: Option<Backend>,
        /// The application the override is for.
        pid: Pid,
    },
    /// The outpost `outpost` watching `pid` has its own `WinEvent` hook in,
    /// so the listener forwards none of `pid`'s `WinEvent` facts raised
    /// from now on, and answers [`ListenerToCore::ForwardingEnded`] behind
    /// every fact it has forwarded (`docs/design/focus-pipeline.md`,
    /// section 12.4).
    HookInstalled {
        /// The application.
        pid: Pid,
        /// Its outpost generation, echoed in the answer so a late answer
        /// never reaches a later generation.
        outpost: OutpostId,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch
        /// ([`CoreToListener::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// `pid` has no outpost with its hook in any more, so the listener
    /// forwards its `WinEvent` facts again.
    HookRemoved {
        /// The application.
        pid: Pid,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch
        /// ([`CoreToListener::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// Liveness probe; answered by [`ListenerToCore::Pong`].
    Ping {
        /// Echoed in the matching pong.
        seq: u64,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch
        /// ([`CoreToListener::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): answered
    /// [`ListenerToCore::IdleAnswer`] once the listener's `WinEvent` thread
    /// and its reading threads have nothing in progress, behind every
    /// message it sent before.
    IdleQuery {
        /// The round, echoed in the answer.
        round: u64,
    },
    /// Shut down cleanly and exit, as an outpost does
    /// ([`CoreToOutpost::Shutdown`]).
    Shutdown,
}

impl CoreToListener {
    /// Stamps a ping or a hand-over notice with `now`, read just before the
    /// command's write begins. Reads no clock for any other command.
    pub fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        match self {
            CoreToListener::Ping { sent_at_us, .. }
            | CoreToListener::HookInstalled { sent_at_us, .. }
            | CoreToListener::HookRemoved { sent_at_us, .. } => *sent_at_us = now(),
            CoreToListener::SetBackendOverride { .. }
            | CoreToListener::IdleQuery { .. }
            | CoreToListener::Shutdown => {}
        }
    }

    /// When verbatim.exe began writing this command, for one
    /// [`stamp_sent`](Self::stamp_sent) stamps; 0 for any other.
    #[must_use]
    pub fn sent_at_us(&self) -> u64 {
        match self {
            CoreToListener::Ping { sent_at_us, .. }
            | CoreToListener::HookInstalled { sent_at_us, .. }
            | CoreToListener::HookRemoved { sent_at_us, .. } => *sent_at_us,
            CoreToListener::SetBackendOverride { .. }
            | CoreToListener::IdleQuery { .. }
            | CoreToListener::Shutdown => 0,
        }
    }

    /// What kind of message this is.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            CoreToListener::IdleQuery { .. } => Kind::Ask,
            CoreToListener::HookInstalled { .. }
            | CoreToListener::HookRemoved { .. }
            | CoreToListener::Ping { .. }
            | CoreToListener::Shutdown => Kind::Lifecycle,
            CoreToListener::SetBackendOverride { .. } => Kind::Configuration,
        }
    }
}

/// What a [`CoreToOutpost::Query`] asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Query {
    /// The application's current foreground window, if it is the system's
    /// foreground window, and its focused control with that control's
    /// ancestors and selected child. Used at startup and after an outpost or
    /// the listener is replaced, in place of retrying until focus settles.
    FocusNow,
    /// One object-navigation step from `node_id` in the direction `kind`
    /// names.
    Navigate {
        /// The node to navigate from.
        node_id: NodeId,
        /// Which way: parent, next or previous sibling, or first child.
        kind: QueryKind,
    },
    /// Activate a node: UIA `Invoke`, `Toggle`, or the legacy default
    /// action; MSAA `accDoDefaultAction`.
    Activate {
        /// The node to activate.
        node_id: NodeId,
    },
    /// The node's ancestors, outermost first, capped at 64.
    Ancestors {
        /// The node whose ancestors are wanted.
        node_id: NodeId,
    },
    /// The application's tree from its top-level window, capped at a depth
    /// of 64 and 4096 nodes.
    DumpTree {
        /// Whether each element's location and whether it is off the
        /// screen are read. A read of many elements leaves both out
        /// otherwise, since for some elements, a window's title bar among
        /// them, the application's UI thread is asked for them, so each
        /// node shows them as not read ([`DumpedTree::locations`]).
        locations: bool,
    },
    /// A text request (milestone M4's text protocol, `TextOp` in
    /// `verbatim-model`): read, wait for the caret, select, or move the
    /// caret in a node's text. Answered [`QueryResult::Text`].
    Text {
        /// The node whose text to use.
        node_id: NodeId,
        /// What to do.
        op: TextOp,
    },
    /// The shell's tray icons or taskbar buttons, for Core's system tray
    /// list dialog, asked of the outpost of the process that owns the
    /// taskbar, which reads them within its own application
    /// (`docs/design/listener-without-connections.md`, decision 9).
    /// Answered [`QueryResult::ShellItems`].
    ShellItems {
        /// Which shell surface.
        kind: crate::ShellItemKind,
    },
    /// Where one item of a list [`Query::ShellItems`] answered may be on
    /// the screen now, for a click on it: the outpost walks the surface
    /// again and reads the position, which no list read fetches, of every
    /// button whose runtime id or name is the item's. Which of them Core
    /// clicks, if any, Core decides. Answered
    /// [`QueryResult::ShellItemPosition`].
    ShellItemPosition {
        /// Which shell surface the item was listed from.
        kind: crate::ShellItemKind,
        /// The item chosen.
        item: crate::ShellItem,
    },
}

/// How long an outpost's worker may take over a walk query,
/// [`Query::Ancestors`] (up to 64 hops), [`Query::DumpTree`] (up to 4096
/// nodes), [`Query::ShellItems`] or [`Query::ShellItemPosition`], from
/// when it begins it: by then it has answered, or it is abandoned and the
/// query answered [`QueryOutcome::Abandoned`]. Core waits for a shell
/// items or position answer longer than this, so that whichever comes
/// reaches it before it gives up.
pub const WALK_DEADLINE: Duration = Duration::from_secs(5);

/// The one outcome of a query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a reply is built once and moved, never stored in bulk"
)]
pub enum QueryOutcome {
    /// The query finished.
    Done(QueryResult),
    /// The node the query named is no longer reachable.
    Gone,
    /// The query failed, for the reason given.
    Failed(String),
    /// The query was withdrawn or expired before it started: it had no side
    /// effects.
    NotStarted,
    /// The query started and passed its deadline: side effects, such as an
    /// activation, may already have happened.
    Abandoned,
}

/// What a finished query found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a reply is built once and moved, never stored in bulk"
)]
pub enum QueryResult {
    /// The answer to [`Query::FocusNow`].
    Focus(FocusNow),
    /// The answer to [`Query::Navigate`]: the neighbor, or `None` for a
    /// genuine tree edge (not an error).
    Navigated(Option<NodeSnapshot>),
    /// The answer to [`Query::Activate`]: the name of the action performed,
    /// if it has one.
    Activated(Option<crate::ActionName>),
    /// The answer to [`Query::Ancestors`], outermost first.
    Ancestors(Vec<NodeSnapshot>),
    /// The answer to [`Query::DumpTree`].
    Tree(DumpedTree),
    /// The answer to [`Query::Text`]. Every answer the text protocol has,
    /// `NoText` and `Gone` among them, comes back this way, so Core hands it
    /// to the reducer unchanged.
    Text(TextReply),
    /// The answer to [`Query::ShellItems`], in the shell's order.
    ShellItems(Vec<crate::ShellItem>),
    /// The answer to [`Query::ShellItemPosition`]: the buttons that may be
    /// the item, with their positions, and whether the walk read them all.
    ShellItemPosition(crate::ShellCandidates),
}

/// The answer to [`Query::FocusNow`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusNow {
    /// The application's top-level window and its facts, when it is the
    /// system's foreground window.
    pub window: Option<(NodeSnapshot, WindowFacts)>,
    /// The focused control and its facts, when the application has one.
    pub focus: Option<FocusedControl>,
    /// When the outpost began reading the answer, in microseconds since the
    /// Unix epoch on the run's shared clock: the answer's place among the
    /// focus events, which carry the time they were observed. A focus event
    /// observed before it is older than the answer, and one observed after
    /// it newer, as NVDA queues a focus it reads in order with the events
    /// around it. 0 from an older peer.
    #[serde(default)]
    pub observed_at_us: u64,
}

/// The focused control in a [`FocusNow`] answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusedControl {
    /// The control.
    pub node: NodeSnapshot,
    /// Its ancestors, outermost first, complete to the desktop: a read cut
    /// short, by its time or a failed step, is completed by plain windows
    /// (`docs/design/focus-pipeline-rework.md`, "A window's name, and the
    /// one fallback").
    pub ancestors: Vec<NodeSnapshot>,
    /// The selected child, for a selection container.
    pub selected_child: Option<NodeSnapshot>,
    /// Its window's facts.
    pub window: Option<WindowFacts>,
    /// What the outpost's caret read found, for a control that may have
    /// text, read as a reported focus's is (`FocusChanged`'s `text`), so a
    /// text field's line is spoken and Core has its caret; `None` for any
    /// other control. The outpost's read leaves it `None` and the worker
    /// fills it.
    pub text: Option<Box<crate::FocusText>>,
}

/// Messages from an outpost to verbatim.exe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a message is built once and moved; the backlogs that hold messages are short"
)]
pub enum OutpostToCore {
    /// The outpost has its application and its `WinEvent` hook is in: the
    /// first message of an outpost started for an application, and the
    /// answer to [`CoreToOutpost::Assign`] of a spare.
    Ready {
        /// The outpost's own process id.
        outpost_pid: Pid,
        /// The application it is watching.
        target_pid: Pid,
    },
    /// A normalized accessibility event.
    Event {
        /// Trace ID minted when the OS event was first observed in the
        /// outpost.
        trace_id: TraceId,
        /// Microseconds since the Unix epoch on the run's shared clock when
        /// the OS event was first observed, which orders it against other
        /// outposts' events. A foreground change carries instead the time
        /// its window was confirmed as the foreground, since Windows raises
        /// the event before the change completes; `timing` keeps the first
        /// observation, where the latency timeline starts. Defaults to 0
        /// for messages from older peers.
        #[serde(default)]
        observed_at_us: u64,
        /// Which backend sourced the event.
        backend: Backend,
        /// Facts about the window the event concerns, read with local calls
        /// when the event was observed; `None` when it had no window.
        #[serde(default)]
        window: Option<WindowFacts>,
        /// The application the event belongs to when it is not the
        /// outpost's target: for an event in a console window, which the
        /// console host's outpost reports, the console's client, the
        /// window's owner by `GetWindowThreadProcessId`, as NVDA gives a
        /// console window and its text area the client's app module
        /// (`docs/design/focus-pipeline.md`, section 5.4). `None` for the
        /// target itself.
        #[serde(default)]
        application: Option<Pid>,
        /// When the event reached each point in the outpost, for the
        /// latency log.
        #[serde(default)]
        timing: EventTiming,
        /// The event itself.
        event: NormalizedEvent,
    },
    /// The one answer to a [`CoreToOutpost::Query`].
    Reply {
        /// Trace ID carried through from the query.
        trace_id: TraceId,
        /// The request this answers.
        request_id: u64,
        /// What became of it.
        outcome: QueryOutcome,
        /// When the query reached each point in the outpost, and the
        /// cross-process calls answering it made, for the latency log.
        /// Default for a query withdrawn or abandoned, and from an older
        /// peer.
        #[serde(default)]
        timing: EventTiming,
    },
    /// Answer to [`CoreToOutpost::Ping`].
    Pong {
        /// The probed sequence number.
        seq: u64,
        /// How many of this outpost's workers have been abandoned to calls
        /// that passed their deadline and have not yet returned; the
        /// supervisor ends an outpost that piles them up.
        parked_count: usize,
        /// When the writer began writing this pong to verbatim.exe's pipe,
        /// in microseconds since the Unix epoch; 0 from an older peer.
        #[serde(default)]
        written_at_us: u64,
    },
    /// A backend error worth reporting without dying — a failed event
    /// registration, an arbitration probe that keeps timing out.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
    /// The outpost's target application has exited. Sent once, ahead of
    /// ordinary messages; the supervisor then shuts the outpost down
    /// ([`CoreToOutpost::Shutdown`]), as it has nothing left to
    /// watch.
    TargetExited,
    /// A spare outpost has done everything of its start that needs no
    /// application, and waits for [`CoreToOutpost::Assign`]
    /// (`docs/design/focus-pipeline.md`, section 19.2). Its first message.
    SpareReady {
        /// The outpost's own process id.
        outpost_pid: Pid,
    },
    /// The answer to [`CoreToOutpost::IdleQuery`] for `round`, sent
    /// behind every message sent before it: never sent ahead as urgent,
    /// and nothing is merged across it.
    IdleAnswer {
        /// The round answered.
        round: u64,
    },
}

/// Messages from the focus listener to verbatim.exe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "a message is built once and moved; the backlogs that hold messages are short"
)]
pub enum ListenerToCore {
    /// The listener has made its desktop-wide UIA subscriptions, or failed
    /// to: its first message after its start's facts.
    Ready {
        /// The listener's own process id.
        outpost_pid: Pid,
        /// A sentinel, 0: the listener has no target application. Only
        /// logged.
        target_pid: Pid,
    },
    /// A focus fact the focus listener captured (decision D13). Sent only by
    /// the listener process, never a per-application outpost; the supervisor
    /// intercepts it, routes it to the target's own outpost, and never
    /// forwards it to the app. The listener mints the `trace_id` and stamps
    /// `timing`'s `observed_at_us` at observation, so the fact is ordered
    /// by, and the latency timeline starts at, the real OS event.
    FocusFact {
        /// Trace ID minted when the listener observed the OS event.
        trace_id: TraceId,
        /// When it was raised and observed, in microseconds on the run's
        /// shared clock.
        #[serde(default)]
        timing: EventTiming,
        /// The captured fact, tagged with the pid Core routes it to.
        fact: ListenerFact,
        /// For an MSAA `WinEvent`'s fact, the thread that raised it and
        /// when; `None` for a UIA fact.
        #[serde(default)]
        win_event: Option<WinEventStamp>,
    },
    /// The listener's answer to [`CoreToListener::HookInstalled`],
    /// sent behind every fact it forwarded before it: it forwards none of
    /// `pid`'s `WinEvent` facts raised after `through`. Sent only by the
    /// listener; the supervisor relays it to `outpost` as
    /// [`CoreToOutpost::ForwardingEnded`] if it still watches `pid`.
    ForwardingEnded {
        /// The application.
        pid: Pid,
        /// The outpost generation the hand-over was for.
        outpost: OutpostId,
        /// The listener's clock when it stopped, on the `WinEvent` clock.
        through: u32,
    },
    /// A menu closed, menu mode ended, or the Alt+Tab switcher closed,
    /// anywhere on the desktop, 50 milliseconds ago. Sent only by the
    /// listener; the supervisor passes it to the app, which reads the
    /// foreground application's focused control (NVDA's fake focus) unless a
    /// focus observed since the end has already been applied.
    MenuOrSwitchEnded {
        /// When the menu or switcher ended, in microseconds since the Unix
        /// epoch on the run's shared clock.
        ended_at_us: u64,
        /// When the listener observed the end and queued it, and when its
        /// writer began writing this message, as a fact's are stamped;
        /// verbatim.exe logs the listener's stages from them.
        #[serde(default)]
        timing: EventTiming,
    },
    /// A window came to the system's foreground, for the keyboard hook's
    /// view of it (architecture section 5). Sent only by the listener, for
    /// every foreground `WinEvent`, whatever process raised it and whether
    /// or not its fact is forwarded; once when the listener starts, for the
    /// window then in front; and again for a console window in front whose
    /// thread no event had named, once one of its events names it. Only
    /// the newest waits. The supervisor passes it to the app, which gives
    /// it to Core.
    ForegroundWindow {
        /// The window.
        window: isize,
        /// The thread that raised its event, the console host's own for a
        /// console window; 0 for a console window whose thread no event
        /// has named yet.
        thread: u32,
        /// Whether it is a console host window.
        console: bool,
        /// The integrity level of the process that owns the window, or
        /// `None` when it cannot be read.
        integrity: Option<u32>,
    },
    /// Answer to [`CoreToListener::Ping`].
    Pong {
        /// The probed sequence number.
        seq: u64,
        /// Always 0: the listener abandons no workers.
        parked_count: usize,
        /// When the writer began writing this pong to verbatim.exe's pipe,
        /// in microseconds since the Unix epoch.
        #[serde(default)]
        written_at_us: u64,
    },
    /// A fault worth reporting without dying, logged by verbatim.exe.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
    /// The answer to [`CoreToListener::IdleQuery`] for `round`, sent
    /// behind every message sent before it.
    IdleAnswer {
        /// The round answered.
        round: u64,
    },
}

impl OutpostToCore {
    /// What kind of message this is.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            OutpostToCore::Reply { .. } | OutpostToCore::IdleAnswer { .. } => Kind::Answer,
            OutpostToCore::Event { .. } | OutpostToCore::Fault { .. } => Kind::Report,
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::Pong { .. } => Kind::Lifecycle,
        }
    }

    /// Whether this message counts toward the message positions that
    /// [`CoreToOutpost::NodesHeld`] acknowledges: an event or a completed
    /// query's reply, the messages that carry node ids. The outpost and
    /// Core count the same messages, in pipe order, with this one function,
    /// so every variant is listed: one that carries nodes and is not
    /// counted would let the outpost release a node Core still reads.
    #[must_use]
    pub fn carries_nodes(&self) -> bool {
        match self {
            OutpostToCore::Event { .. } => true,
            OutpostToCore::Reply { outcome, .. } => match outcome {
                QueryOutcome::Done(_) => true,
                QueryOutcome::Gone
                | QueryOutcome::Failed(_)
                | QueryOutcome::NotStarted
                | QueryOutcome::Abandoned => false,
            },
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. } => false,
        }
    }

    /// Records, in the timing the message carries, when its writer began
    /// writing it to verbatim.exe's pipe: an event's or a reply's
    /// (`EventTiming::written_at_us`), or a pong's. Nothing for any other
    /// message.
    pub fn written(&mut self, at_us: u64) {
        match self {
            OutpostToCore::Event { timing, .. } | OutpostToCore::Reply { timing, .. } => {
                timing.written_at_us = at_us;
            }
            OutpostToCore::Pong { written_at_us, .. } => *written_at_us = at_us,
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. } => {}
        }
    }

    /// Records, in an event's or a reply's timing, when verbatim.exe read
    /// the message off the outpost's pipe (`EventTiming::core_read_at_us`).
    pub fn core_read(&mut self, at_us: u64) {
        if let OutpostToCore::Event { timing, .. } | OutpostToCore::Reply { timing, .. } = self {
            timing.core_read_at_us = at_us;
        }
    }

    /// Stamps `outpost` on every node id this message carries. Core applies
    /// it to everything arriving on an outpost's pipe, so node ids name their
    /// outpost generation and never come from the message body.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            OutpostToCore::Event { event, .. } => event.assign_outpost(outpost),
            OutpostToCore::Reply {
                outcome: QueryOutcome::Done(result),
                ..
            } => match result {
                QueryResult::Focus(focus) => {
                    if let Some((window, _)) = &mut focus.window {
                        window.assign_outpost(outpost);
                    }
                    if let Some(control) = &mut focus.focus {
                        control.node.assign_outpost(outpost);
                        for ancestor in &mut control.ancestors {
                            ancestor.assign_outpost(outpost);
                        }
                        if let Some(selected) = &mut control.selected_child {
                            selected.assign_outpost(outpost);
                        }
                    }
                }
                QueryResult::Navigated(Some(node)) => node.assign_outpost(outpost),
                QueryResult::Ancestors(chain) => {
                    for node in chain {
                        node.assign_outpost(outpost);
                    }
                }
                QueryResult::Tree(dumped) => dumped.root.assign_outpost(outpost),
                QueryResult::Navigated(None)
                | QueryResult::Activated(_)
                | QueryResult::Text(_)
                | QueryResult::ShellItems(_)
                | QueryResult::ShellItemPosition(_) => {}
            },
            OutpostToCore::Reply { .. }
            | OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. } => {}
        }
    }
}

/// The node ids and text anchors a message names
/// ([`OutpostToCore::named`]): what an outpost records as carried by the
/// message, so it keeps each until Core has seen the message and does not
/// hold it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// The nodes, in the order the message names them, repeats kept.
    pub nodes: Vec<NodeId>,
    /// The text anchors, likewise.
    pub anchors: Vec<crate::TextAnchor>,
}

impl Named {
    fn node(&mut self, node: &NodeSnapshot) {
        self.nodes.push(node.id);
    }

    fn tree(&mut self, tree: &TreeNode) {
        self.node(&tree.snapshot);
        for child in &tree.children {
            self.tree(child);
        }
    }

    fn chunk(&mut self, chunk: &crate::TextChunk) {
        self.anchors.push(chunk.start);
    }

    fn caret(&mut self, caret: &crate::CaretReport) {
        self.chunk(&caret.line);
        if let Some(selection) = &caret.selection {
            self.anchors.push(selection.start.anchor);
            self.anchors.push(selection.end.anchor);
        }
        if let Some(line) = &caret.selection_line {
            self.chunk(line);
        }
    }

    fn focus_text(&mut self, text: Option<&crate::FocusText>) {
        if let Some(crate::FocusText::Caret { caret, .. }) = text {
            self.caret(caret);
        }
    }

    fn text_reply(&mut self, reply: &TextReply) {
        match reply {
            TextReply::Caret(reply) => {
                self.caret(&reply.caret);
                if let Some(unit) = &reply.unit {
                    self.chunk(unit);
                }
            }
            TextReply::Read { chunk, .. } => self.chunk(chunk),
            TextReply::Chunks { chunks, .. } => {
                for chunk in chunks {
                    self.chunk(chunk);
                }
            }
            TextReply::TerminalSnapshot(snapshot) => {
                if let Some(caret) = &snapshot.caret {
                    self.caret(caret);
                }
            }
            TextReply::Range { .. }
            | TextReply::Done
            | TextReply::Selected(_)
            | TextReply::TerminalUnchanged
            | TextReply::TerminalHistory { .. }
            | TextReply::TerminalFound { .. }
            | TextReply::Location { .. }
            | TextReply::UnsupportedUnit(_)
            | TextReply::Unsupported
            | TextReply::NoText
            | TextReply::AnchorLost
            | TextReply::Gone
            | TextReply::Unanswered
            | TextReply::WatchEnded => {}
        }
    }

    fn event(&mut self, event: &NormalizedEvent) {
        match event {
            NormalizedEvent::FocusChanged {
                node,
                ancestors,
                selected_child,
                text,
                ..
            } => {
                self.node(node);
                for ancestor in ancestors {
                    self.node(ancestor);
                }
                if let Some(selected) = selected_child {
                    self.node(selected);
                }
                self.focus_text(text.as_deref());
            }
            NormalizedEvent::SelectionChanged { node } | NormalizedEvent::Alert { node } => {
                self.node(node);
            }
            NormalizedEvent::ProgressChanged { bar } => self.nodes.push(bar.id),
            NormalizedEvent::ControlledSelection { controller, node } => {
                self.nodes.push(*controller);
                self.node(node);
            }
            NormalizedEvent::CaretMoved { node_id, caret, .. } => {
                self.nodes.push(*node_id);
                self.caret(caret);
            }
            NormalizedEvent::TerminalSnapshot { node_id, snapshot } => {
                self.nodes.push(*node_id);
                if let Some(caret) = &snapshot.caret {
                    self.caret(caret);
                }
            }
            NormalizedEvent::ActiveTextPositionChanged { node_id, position } => {
                self.nodes.push(*node_id);
                self.anchors.push(position.anchor);
            }
            NormalizedEvent::PropertyChanged { node_id, .. }
            | NormalizedEvent::ValueChanged { node_id, .. }
            | NormalizedEvent::Notification { node_id, .. }
            | NormalizedEvent::TextChanged { node_id } => self.nodes.push(*node_id),
        }
    }
}

impl OutpostToCore {
    /// Every node id and text anchor this message names: the nodes
    /// [`assign_outpost`](Self::assign_outpost) stamps, and the anchors of
    /// its text, a caret's line and selection, a read's chunks, and an
    /// active text position. Empty for a message that names none.
    #[must_use]
    pub fn named(&self) -> Named {
        let mut named = Named::default();
        match self {
            OutpostToCore::Event { event, .. } => named.event(event),
            OutpostToCore::Reply {
                outcome: QueryOutcome::Done(result),
                ..
            } => match result {
                QueryResult::Focus(focus) => {
                    if let Some((window, _)) = &focus.window {
                        named.node(window);
                    }
                    if let Some(control) = &focus.focus {
                        named.node(&control.node);
                        for ancestor in &control.ancestors {
                            named.node(ancestor);
                        }
                        if let Some(selected) = &control.selected_child {
                            named.node(selected);
                        }
                        named.focus_text(control.text.as_deref());
                    }
                }
                QueryResult::Navigated(Some(node)) => named.node(node),
                QueryResult::Ancestors(chain) => {
                    for node in chain {
                        named.node(node);
                    }
                }
                QueryResult::Tree(dumped) => named.tree(&dumped.root),
                QueryResult::Text(reply) => named.text_reply(reply),
                QueryResult::Navigated(None)
                | QueryResult::Activated(_)
                | QueryResult::ShellItems(_)
                | QueryResult::ShellItemPosition(_) => {}
            },
            OutpostToCore::Reply { .. }
            | OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. } => {}
        }
        named
    }
}

/// The reducer's input an outpost's event is, `source` being the
/// application the outpost watches: the event's own application when it
/// names one, a console's client for an event in its console window, else
/// `source`. `None` for any other message. The app and the merge's tests
/// both reduce what this gives, so a test reduces what the app applies.
#[must_use]
pub fn event_input(source: Pid, message: OutpostToCore) -> Option<crate::Input> {
    let OutpostToCore::Event {
        trace_id,
        observed_at_us,
        backend,
        window,
        application,
        event,
        ..
    } = message
    else {
        return None;
    };
    Some(crate::Input::Event {
        trace_id,
        observed_at_us,
        source: application.unwrap_or(source),
        backend,
        window,
        event,
    })
}

impl ListenerToCore {
    /// What kind of message this is.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            ListenerToCore::IdleAnswer { .. } => Kind::Answer,
            ListenerToCore::Fault { .. }
            | ListenerToCore::MenuOrSwitchEnded { .. }
            | ListenerToCore::ForegroundWindow { .. } => Kind::Report,
            ListenerToCore::Ready { .. }
            | ListenerToCore::Pong { .. }
            | ListenerToCore::ForwardingEnded { .. } => Kind::Lifecycle,
            ListenerToCore::FocusFact { .. } => Kind::Routed,
        }
    }

    /// Records, in the timing the message carries, when its writer began
    /// writing it to verbatim.exe's pipe: a fact's or a menu end's
    /// (`EventTiming::listener_written_at_us`), or a pong's. Nothing for
    /// any other message.
    pub fn written(&mut self, at_us: u64) {
        match self {
            ListenerToCore::FocusFact { timing, .. }
            | ListenerToCore::MenuOrSwitchEnded { timing, .. } => {
                timing.listener_written_at_us = at_us;
            }
            ListenerToCore::Pong { written_at_us, .. } => *written_at_us = at_us,
            ListenerToCore::Ready { .. }
            | ListenerToCore::ForwardingEnded { .. }
            | ListenerToCore::ForegroundWindow { .. }
            | ListenerToCore::Fault { .. }
            | ListenerToCore::IdleAnswer { .. } => {}
        }
    }
}

/// What kind of message a variant of a direction enum is
/// (`docs/design/ownership.md`, "Messages are of declared kinds"), as its
/// `kind` method says, one exhaustive match per enum, so a new variant is
/// given a kind where it is added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The asking half of a query, answered once: a query, its withdrawal
    /// (answered by the query's own reply), and the idle barrier's query.
    Ask,
    /// The answering half of a query.
    Answer,
    /// Something observed, which the receiver judges.
    Report,
    /// What the receiver may let go of: the nodes Core holds.
    Release,
    /// A generation's life: its start, readiness, liveness, hand-over and
    /// end, each from one ordered path per generation, the owner thread's
    /// writer or the child's own pipe.
    Lifecycle,
    /// A fact the relay routes from the listener to an outpost.
    Routed,
    /// A setting the receiver applies.
    Configuration,
}

/// A message an outpost sends ahead of everything waiting in its outbound
/// queue: none carries a node, so none can skip the outpost's numbering of
/// the messages that do ([`OutpostToCore::carries_nodes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgent {
    /// [`OutpostToCore::Pong`].
    Pong {
        /// The probed sequence number.
        seq: u64,
        /// How many of the outpost's workers are abandoned.
        parked_count: usize,
    },
    /// [`OutpostToCore::Ready`].
    Ready {
        /// The outpost's own process id.
        outpost_pid: Pid,
        /// The application it is watching.
        target_pid: Pid,
    },
    /// [`OutpostToCore::TargetExited`].
    TargetExited,
    /// [`OutpostToCore::SpareReady`].
    SpareReady {
        /// The outpost's own process id.
        outpost_pid: Pid,
    },
}

impl From<Urgent> for OutpostToCore {
    fn from(urgent: Urgent) -> Self {
        match urgent {
            Urgent::Pong { seq, parked_count } => OutpostToCore::Pong {
                seq,
                parked_count,
                written_at_us: 0,
            },
            Urgent::Ready {
                outpost_pid,
                target_pid,
            } => OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            },
            Urgent::TargetExited => OutpostToCore::TargetExited,
            Urgent::SpareReady { outpost_pid } => OutpostToCore::SpareReady { outpost_pid },
        }
    }
}

/// One completed tree walk from a target application's top-level window
/// (architecture section 1). The walk is bounded by a depth cap of 64 and a
/// node-count cap of 4096; `truncated` notes when it is incomplete: it
/// stopped early against either cap rather than reaching every node, or an
/// element gone during the walk left the part of the tree under it unread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpedTree {
    /// The root node reached.
    pub root: TreeNode,
    /// Whether the tree is incomplete, against a cap or at a gone element.
    pub truncated: bool,
    /// Whether each node's location and whether it is off the screen were
    /// read. When not, every node's rectangle is `None` and its states
    /// never hold `Offscreen`, which then says nothing of where it is.
    pub locations: bool,
}

#[cfg(test)]
mod tests;
