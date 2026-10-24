//! The private Core-outpost protocol (architecture section 1).
//!
//! Transport: two pipes with no name, made by the supervisor, whose child
//! ends the child inherits, their handle values passed on its command line,
//! so there is nothing to discover or secure. Each message is one frame of
//! `verbatim_process::transport`: a `Content-Length` header, then the
//! message as compact JSON, readable by eye and in captures, and isolated
//! behind [`write_message`], [`encode_message`] and [`read_message`] so
//! neither end's logic sees the framing.

#![forbid(unsafe_code)]

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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
    /// When the listener or the outpost observed it.
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

/// Microseconds since the Unix epoch on the run's shared clock
/// ([`verbatim_process::clock`]), which [`EventTiming`], the latency log,
/// and every time compared across processes share.
#[must_use]
pub fn now_us() -> u64 {
    verbatim_process::clock::now_us()
}
use verbatim_model::{
    Backend, CallCounts, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, Notification,
    NotificationKind, NotificationProcessing, OutpostId, Pid, QueryKind, Role, StateSet, TextOp,
    TextReply, TraceId, TreeNode, WindowFacts,
};

/// The identity-free contents of a UIA focus element, as the focus listener
/// captures them (decision D13): the runtime id plus role, name, value,
/// states, and details — everything a [`NodeSnapshot`](verbatim_model::NodeSnapshot)
/// carries except its [`NodeId`](verbatim_model::NodeId). The node id is
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
    /// read then. `None` when they could not be read.
    pub ancestors: Option<Vec<UiaAncestorFact>>,
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
    /// thread read for the application; else whether that earlier element
    /// still had the keyboard focus, read live with this focus.
    /// `Some(false)`, the earlier element without the focus or gone, means
    /// the application gave the id to a new element after the one it named
    /// died, and the outpost gives the id a new node; `Some(true)` lets the
    /// outpost keep the element it found for the node. A plain repeat of the
    /// focus never carries it, as the listener drops a repeat first
    /// (`focus_read::LastAccepted`).
    pub same_as_last: Option<bool>,
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
pub(crate) fn tick_is_after(a: u32, b: u32) -> bool {
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

/// What the drop of a UIA focus event's read says to an outpost waiting
/// for a UIA focus after a `WinEvent` focus in one of its UIA windows
/// (`docs/design/listener-without-connections.md`, decision 11; Dickson,
/// 2026-10-21): only a focus event's
/// resolution answers that wait, in one of three ways. It goes on the pipe
/// as JSON, by its variant's name; the listener, the supervisor and the
/// outposts are one build of this definition, so it needs no version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DropEvidence {
    /// A focus event whose drop ends the wait: a verdict of NVDA's checks
    /// on it, as its read would have been (its window was hung, not
    /// related to the foreground window, the console window's own, or a
    /// shell window that only stages a switch, or the event repeats the
    /// focus, which still has the keyboard focus), or the listener giving
    /// up on its application, which did not answer in time (the read did
    /// not return in time, waited too long behind abandoned reads, ran out
    /// of time before its window was looked up or judged, or its event
    /// said it lacked the keyboard focus when the read's time was up), so
    /// that the outpost does not read an application that has just failed
    /// to answer.
    Focus,
    /// A focus event whose drop could not establish where the focus is:
    /// its element no longer had the keyboard focus, its window does not
    /// use UIA, so it is another window than the one the outpost waits on,
    /// its read was abandoned when the user moved on or never started, the
    /// listener was shutting down, its element could not be kept or
    /// resolved for the read, its reading thread had no UIA client, or a
    /// newer event of the same element replaced it. The wait goes on to
    /// its bound, and nothing is read at once: such drops come in ordinary
    /// fast navigation, where the next key's focus event follows, and in a
    /// new File Explorer window, whose frame's focus is dropped beside its
    /// items view's.
    Inconclusive,
    /// A focus event whose sender has no window above it, which a sender
    /// gone before its read has, as in a new window's first burst of focus
    /// events: the event came and cannot be read, and the event for where
    /// the focus went may never come, so the outpost reads its focus at
    /// once.
    NoWindow,
    /// Not a focus event: a selection, a notification, or a menu opening,
    /// which says nothing of the focus.
    NotFocus,
}

/// A fact the supervisor delivers to a target outpost (decision D13): what
/// the listener captured, minus the pid, since routing is already done. The
/// outpost's worker reads, arbitrates, and reports it exactly as it does the
/// events it hooks itself; its own hook makes the same facts of the MSAA
/// focus, foreground, menu, alert, and tooltip events it takes in
/// ([`DeliveredFact::from_win_event`]).
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
        /// Whether the event read was a focus rather than a menu opening:
        /// only a focus's read, in the copy's top-level window, answers an
        /// outpost's wait for a UIA focus after a `WinEvent` focus in a UIA
        /// window (`docs/design/listener-without-connections.md`, decision
        /// 11).
        focus: bool,
    },
    /// The UIA focus event `marker` names is not reported: its element had
    /// lost the keyboard focus when it was read, as NVDA drops such an
    /// event, or its window was hung, a shell window that only stages a
    /// switch, or the console window's own; or its read did not return in
    /// time.
    UiaFocusDropped {
        /// The marker the drop resolves.
        marker: u64,
        /// What the drop says to an outpost waiting for a UIA focus after a
        /// `WinEvent` focus in a UIA window.
        evidence: DropEvidence,
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
    /// The fact an MSAA `WinEvent` of `kind` at the address `(hwnd,
    /// id_object, id_child)` makes, whether the listener's hook or an
    /// outpost's own took it in: a focus, a foreground change, a menu
    /// opening, an alert, or a tooltip shown. `None` for any other kind,
    /// and for an event with no window, which neither routes nor reads.
    #[must_use]
    pub fn from_win_event(
        kind: verbatim_ia2::WinEventKind,
        (hwnd, id_object, id_child): (isize, i32, i32),
    ) -> Option<Self> {
        use verbatim_ia2::WinEventKind;
        if hwnd == 0 {
            return None;
        }
        Some(match kind {
            WinEventKind::Focus => DeliveredFact::MsaaFocus {
                hwnd,
                id_object,
                id_child,
            },
            // Taken unchecked: whether the window is still the foreground
            // window is checked when the fact is handled, as NVDA checks it
            // when it processes the event rather than in its event callback.
            WinEventKind::Foreground => DeliveredFact::Foreground { hwnd },
            WinEventKind::MenuPopupStart => DeliveredFact::MenuPopup {
                hwnd,
                id_object,
                id_child,
            },
            WinEventKind::Alert => DeliveredFact::Alert {
                hwnd,
                id_object,
                id_child,
            },
            WinEventKind::Show => DeliveredFact::Show {
                hwnd,
                id_object,
                id_child,
            },
            _ => return None,
        })
    }

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
    /// marker as a read does, or the end of a listener's reads. The supervisor relays
    /// these in order and never drops one, since an outpost waits on a
    /// marker until its read or drop arrives.
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

    /// Whether this fact may start an outpost for an application that has
    /// none (outpost redesign, "The focus listener"): focus, foreground,
    /// menu, and alert facts may, a UIA focus's marker and read among them,
    /// and the shell's window-snap results notification, the one Core
    /// speaks from any application. A selection without an outpost is
    /// dropped, and so is any other notification: Core speaks a
    /// notification only from the focus's application, and an application
    /// with no outpost cannot hold the focus, as NVDA drops a background
    /// application's notification. A focus's drop and the end of a
    /// listener's reads resolve only markers an outpost already holds.
    #[must_use]
    pub fn may_start_outpost(&self) -> bool {
        match self {
            DeliveredFact::UiaSelection { .. }
            | DeliveredFact::UiaFocusDropped { .. }
            | DeliveredFact::UiaFocusReadsEnded => false,
            DeliveredFact::UiaNotification { notification, .. } => {
                notification.activity_id.as_deref() == Some(verbatim_core::SNAP_RESULTS_ACTIVITY)
            }
            _ => true,
        }
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
    /// report waits ([`OutpostToSupervisor::ForegroundWindow`]).
    ForegroundWindow,
}

/// Messages from the Core-side supervisor to an outpost.
///
/// An outpost's target application is set once (decision D9: one
/// outpost per application, for its whole life): passed on its command
/// line, or, for a spare started before it had one, by
/// [`SupervisorToOutpost::Assign`]. Core ends an outpost by closing its job
/// handle; there is no shutdown message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SupervisorToOutpost {
    /// Forces one backend for every window of the target application,
    /// overriding arbitration; used by tests and per-app config overrides.
    /// `None` restores normal arbitration.
    SetBackendOverride {
        /// The forced backend, or `None` for normal arbitration.
        backend_override: Option<Backend>,
    },
    /// Delivers a focus fact the focus listener captured, for this outpost's
    /// target application (decision D13), threading the listener's
    /// `trace_id` and `observed_at_ms` through to the emitted event so the
    /// latency timeline starts at the real OS event rather than this
    /// delivery.
    DeliverFact {
        /// Trace ID the listener minted when it observed the OS event.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the listener observed the
        /// OS event — the first point on the keypress-to-audio timeline.
        observed_at_ms: u64,
        /// When it was raised and observed, for the latency log.
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
    /// To the focus listener: the outpost `outpost` watching `pid` has its
    /// own `WinEvent` hook in, so the listener forwards none of `pid`'s
    /// `WinEvent` facts raised from now on, and answers
    /// [`OutpostToSupervisor::ForwardingEnded`] behind every fact it has
    /// forwarded (`docs/design/focus-pipeline.md`, section 12.4).
    HookInstalled {
        /// The application.
        pid: Pid,
        /// Its outpost incarnation, echoed in the answer so a late answer
        /// never reaches a later incarnation.
        outpost: OutpostId,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch; 0 from an older peer
        /// ([`SupervisorToOutpost::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// To the focus listener: `pid` has no outpost with its hook in any
    /// more, so the listener forwards its `WinEvent` facts again.
    HookRemoved {
        /// The application.
        pid: Pid,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch; 0 from an older peer
        /// ([`SupervisorToOutpost::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// To the focus listener, after every focus Core queues, and to the
    /// outpost that reported the focus Core queued before, when the new
    /// one is another application's: Core has taken as the focus the event
    /// traced `trace_id`, observed at `observed_at_ms` (0 for a focus Core
    /// asked for). NVDA's duplicate rules compare a focus event with the
    /// last focus queued, whatever reported it, so the listener forgets the
    /// last focus it accepted when a newer, other focus was queued
    /// (`focus_read::LastAccepted`), and the outpost's MSAA duplicate test
    /// stops comparing with its last focus when that was observed no later
    /// than this one.
    FocusQueued {
        /// The trace of the event Core took.
        trace_id: TraceId,
        /// When that event was observed, in milliseconds since the Unix
        /// epoch.
        observed_at_ms: u64,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch; 0 from an older peer
        /// ([`SupervisorToOutpost::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// To an outpost: the listener forwards none of the application's
    /// `WinEvent` facts raised after `through`, and every one it forwarded
    /// before is ahead of this message, so the listener's copies and the
    /// outpost's hook's are matched from now on only for events raised
    /// while the hook went in (section 12.4, "The overlap's ends"). Relayed
    /// in order behind every fact the listener sent before it.
    ForwardingEnded {
        /// The listener's clock when it stopped, on the `WinEvent` clock.
        through: u32,
    },
    /// A query, answered by exactly one [`OutpostToSupervisor::Reply`]
    /// carrying the same `request_id`.
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
    /// Liveness probe; answered by [`OutpostToSupervisor::Pong`].
    Ping {
        /// Echoed in the matching pong.
        seq: u64,
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch; 0 from an older peer
        /// ([`SupervisorToOutpost::stamp_sent`]).
        #[serde(default)]
        sent_at_us: u64,
    },
    /// The details the active theme wants read for each node
    /// (`SrState::fetches` in `verbatim-core`): a detail whose indication
    /// is off is not read, which saves its cross-process call. Sent when an
    /// outpost starts and whenever the theme in use changes; everything is
    /// read until the first arrives.
    Fetches(verbatim_model::Fetches),
    /// A key reached the focused application, sent by the keyboard hook's
    /// thread as the hook reports it, before the key is passed on, so it
    /// reaches the outpost ahead of what the application shows of it: the
    /// focused terminal's reading rules take it
    /// (`docs/design/terminal-model.md`, section 5.5). Ignored when the
    /// outpost's focus is not a terminal.
    TerminalKey(verbatim_model::TerminalKey),
    /// The terminal reading rules' settings ("report new output", "review
    /// follows caret", and the flood policy's limits), from Core's. Sent
    /// when an outpost starts and whenever they change; the defaults until
    /// the first arrives.
    TerminalSettings(verbatim_model::TerminalSettings),
    /// The nodes from this outpost that Core still holds (outpost redesign,
    /// "Held objects"), as the numbers the outpost issued, and the position
    /// of the last message from this outpost that Core has handled
    /// ([`OutpostToSupervisor::carries_nodes`] says which messages count).
    /// The outpost releases every other node it reported at or before that
    /// position, and answers a query for a released node "gone".
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
    /// [`OutpostToSupervisor::IdleAnswer`] once nothing that came before
    /// this query is still in progress. The listener answers once its
    /// `WinEvent` thread and its reading threads have nothing in progress;
    /// an outpost once its worker has handled everything that reached its
    /// intake before the query, with the caret those entries noted, and no
    /// caret key's watch or owed terminal read is open. Either
    /// answers behind every message it sent before, never merged and never
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
    /// starts waiting for its exit, and sends
    /// [`OutpostToSupervisor::Ready`]; or, when the hook cannot be
    /// installed, reports the failure ([`OutpostToSupervisor::Fault`]) and
    /// exits without `Ready`. Sent once, before any other command but a
    /// ping; an outpost started for an application is never sent it.
    Assign {
        /// The application.
        target_pid: Pid,
    },
}

impl SupervisorToOutpost {
    /// Stamps a command to the focus listener (a ping, a hand-over notice,
    /// or a focus queued) with `now`, read just before the command's write
    /// begins. Reads no clock for any other command.
    pub fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        match self {
            SupervisorToOutpost::Ping { sent_at_us, .. }
            | SupervisorToOutpost::HookInstalled { sent_at_us, .. }
            | SupervisorToOutpost::HookRemoved { sent_at_us, .. }
            | SupervisorToOutpost::FocusQueued { sent_at_us, .. } => *sent_at_us = now(),
            _ => {}
        }
    }

    /// When verbatim.exe began writing this command, for one
    /// [`stamp_sent`](Self::stamp_sent) stamps; 0 for any other.
    #[must_use]
    pub fn sent_at_us(&self) -> u64 {
        match self {
            SupervisorToOutpost::Ping { sent_at_us, .. }
            | SupervisorToOutpost::HookInstalled { sent_at_us, .. }
            | SupervisorToOutpost::HookRemoved { sent_at_us, .. }
            | SupervisorToOutpost::FocusQueued { sent_at_us, .. } => *sent_at_us,
            _ => 0,
        }
    }
}

/// What a [`SupervisorToOutpost::Query`] asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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
    DumpTree,
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
        kind: verbatim_model::ShellItemKind,
    },
}

/// How long an outpost's worker may take over a walk query,
/// [`Query::Ancestors`] (up to 64 hops), [`Query::DumpTree`] (up to 4096
/// nodes) or [`Query::ShellItems`], from when it begins it: by then it has
/// answered, or it is abandoned and the query answered
/// [`QueryOutcome::Abandoned`]. Core waits for a shell items answer longer
/// than this, so that whichever comes reaches it before it gives up.
pub const WALK_DEADLINE: Duration = Duration::from_secs(5);

/// The one outcome of a query.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
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
#[non_exhaustive]
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
    Activated(Option<verbatim_model::ActionName>),
    /// The answer to [`Query::Ancestors`], outermost first.
    Ancestors(Vec<NodeSnapshot>),
    /// The answer to [`Query::DumpTree`].
    Tree(DumpedTree),
    /// The answer to [`Query::Text`]. Every answer the text protocol has,
    /// `NoText` and `Gone` among them, comes back this way, so Core hands it
    /// to the reducer unchanged.
    Text(TextReply),
    /// The answer to [`Query::ShellItems`], in the shell's order.
    ShellItems(Vec<verbatim_model::ShellItem>),
}

/// The answer to [`Query::FocusNow`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusNow {
    /// The application's top-level window and its facts, when it is the
    /// system's foreground window.
    pub window: Option<(NodeSnapshot, WindowFacts)>,
    /// The focused control and its facts, when the application has one.
    pub focus: Option<FocusedControl>,
    /// When the outpost began reading the answer, in milliseconds since the
    /// Unix epoch: the answer's place among the focus events, which carry
    /// the time they were observed. A focus event observed before it is
    /// older than the answer, and one observed after it newer, as NVDA
    /// queues a focus it reads in order with the events around it.
    pub observed_at_ms: u64,
}

/// The focused control in a [`FocusNow`] answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FocusedControl {
    /// The control.
    pub node: NodeSnapshot,
    /// Its ancestors, outermost first, or `None` when their read left them
    /// unknown (it ran out of time, or a step to a parent failed or found
    /// none below the desktop root), so Core keeps the previous focus's
    /// chain rather than taking the control as having no containers.
    pub ancestors: Option<Vec<NodeSnapshot>>,
    /// The selected child, for a selection container.
    pub selected_child: Option<NodeSnapshot>,
    /// Its window's facts.
    pub window: Option<WindowFacts>,
    /// What the outpost's caret read found, for a control that may have
    /// text, read as a reported focus's is (`FocusChanged`'s `text`), so a
    /// text field's line is spoken and Core has its caret; `None` for any
    /// other control. The outpost's read leaves it `None` and the worker
    /// fills it.
    pub text: Option<Box<verbatim_model::FocusText>>,
}

/// Messages from an outpost to the Core-side supervisor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[expect(
    clippy::large_enum_variant,
    reason = "a message is built once, written, read and moved, never stored in bulk"
)]
pub enum OutpostToSupervisor {
    /// The outpost has its application and its `WinEvent` hook is in: the
    /// first message of an outpost started for an application, and the
    /// answer to [`SupervisorToOutpost::Assign`] of a spare.
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
        /// Milliseconds since the Unix epoch when the OS event was first
        /// observed, which orders it against other outposts' events. A
        /// foreground change carries instead the time its window was
        /// confirmed as the foreground, since Windows raises the event
        /// before the change completes; `timing` keeps the first
        /// observation, where the latency timeline starts. Defaults to 0
        /// for messages from older peers.
        #[serde(default)]
        observed_at_ms: u64,
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
    /// The one answer to a [`SupervisorToOutpost::Query`].
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
    /// Answer to [`SupervisorToOutpost::Ping`].
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
    /// A focus fact the focus listener captured (decision D13). Sent only by
    /// the listener process, never a per-application outpost; the supervisor
    /// intercepts it, routes it to the target's own outpost, and never
    /// forwards it to the app. The listener mints the `trace_id` and stamps
    /// `observed_at_ms` at observation, so the latency timeline starts at the
    /// real OS event.
    FocusFact {
        /// Trace ID minted when the listener observed the OS event.
        trace_id: TraceId,
        /// Milliseconds since the Unix epoch when the OS event was observed.
        observed_at_ms: u64,
        /// When it was raised and observed, for the latency log.
        #[serde(default)]
        timing: EventTiming,
        /// The captured fact, tagged with the pid Core routes it to.
        fact: ListenerFact,
        /// For an MSAA `WinEvent`'s fact, the thread that raised it and
        /// when; `None` for a UIA fact.
        #[serde(default)]
        win_event: Option<WinEventStamp>,
    },
    /// The listener's answer to [`SupervisorToOutpost::HookInstalled`],
    /// sent behind every fact it forwarded before it: it forwards none of
    /// `pid`'s `WinEvent` facts raised after `through`. Sent only by the
    /// listener; the supervisor relays it to `outpost` as
    /// [`SupervisorToOutpost::ForwardingEnded`] if it still watches `pid`.
    ForwardingEnded {
        /// The application.
        pid: Pid,
        /// The outpost incarnation the hand-over was for.
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
        /// When the menu or switcher ended, in milliseconds since the Unix
        /// epoch.
        ended_at_ms: u64,
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
    /// The outpost's target application has exited. Sent once, ahead of
    /// ordinary messages; the supervisor then shuts the outpost down
    /// ([`SupervisorToOutpost::Shutdown`]), as it has nothing left to
    /// watch.
    TargetExited,
    /// A spare outpost has done everything of its start that needs no
    /// application, and waits for [`SupervisorToOutpost::Assign`]
    /// (`docs/design/focus-pipeline.md`, section 19.2). Its first message.
    SpareReady {
        /// The outpost's own process id.
        outpost_pid: Pid,
    },
    /// The answer to [`SupervisorToOutpost::IdleQuery`] for `round`, sent
    /// behind every message sent before it: never sent ahead as urgent,
    /// and nothing is merged across it.
    IdleAnswer {
        /// The round answered.
        round: u64,
    },
}

impl OutpostToSupervisor {
    /// Whether this is an [`OutpostToSupervisor::IdleAnswer`]: no message
    /// sent after it is merged into one sent before it.
    #[must_use]
    pub fn is_idle_answer(&self) -> bool {
        matches!(self, OutpostToSupervisor::IdleAnswer { .. })
    }
}

impl OutpostToSupervisor {
    /// Whether this message counts toward the message positions that
    /// [`SupervisorToOutpost::NodesHeld`] acknowledges: an event or a
    /// completed query's reply, the messages that carry node ids. The
    /// outpost and Core count the same messages, in pipe order.
    #[must_use]
    pub fn carries_nodes(&self) -> bool {
        matches!(
            self,
            OutpostToSupervisor::Event { .. }
                | OutpostToSupervisor::Reply {
                    outcome: QueryOutcome::Done(_),
                    ..
                }
        )
    }

    /// Records, in the timing the message carries, when its writer began
    /// writing it to verbatim.exe's pipe: the focus listener's fact's or
    /// menu end's (`EventTiming::listener_written_at_us`), an outpost's
    /// event's or reply's (`EventTiming::written_at_us`), or a pong's.
    /// Nothing for any other message.
    pub fn written(&mut self, at_us: u64) {
        match self {
            OutpostToSupervisor::FocusFact { timing, .. }
            | OutpostToSupervisor::MenuOrSwitchEnded { timing, .. } => {
                timing.listener_written_at_us = at_us;
            }
            OutpostToSupervisor::Event { timing, .. }
            | OutpostToSupervisor::Reply { timing, .. } => {
                timing.written_at_us = at_us;
            }
            OutpostToSupervisor::Pong { written_at_us, .. } => *written_at_us = at_us,
            _ => {}
        }
    }

    /// Records, in an event's or a reply's timing, when verbatim.exe read
    /// the message off the outpost's pipe (`EventTiming::core_read_at_us`).
    pub fn core_read(&mut self, at_us: u64) {
        if let OutpostToSupervisor::Event { timing, .. }
        | OutpostToSupervisor::Reply { timing, .. } = self
        {
            timing.core_read_at_us = at_us;
        }
    }

    /// Stamps `outpost` on every node id this message carries. Core applies
    /// it to everything arriving on an outpost's pipe, so node ids name their
    /// outpost incarnation and never come from the message body.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            OutpostToSupervisor::Event { event, .. } => event.assign_outpost(outpost),
            OutpostToSupervisor::Reply {
                outcome: QueryOutcome::Done(result),
                ..
            } => match result {
                QueryResult::Focus(focus) => {
                    if let Some((window, _)) = &mut focus.window {
                        window.assign_outpost(outpost);
                    }
                    if let Some(control) = &mut focus.focus {
                        control.node.assign_outpost(outpost);
                        for ancestor in control.ancestors.iter_mut().flatten() {
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
                _ => {}
            },
            _ => {}
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
}

/// The most bytes one message may take, its frame's header aside, in either
/// direction between Core and a child (`docs/design/phase6-design.md`, "Terminal risks
/// found by studying NVDA", the limit on outpost messages): room for 10 MB
/// of terminal output and JSON's escaping. A writer never sends more, and a
/// reader that is told of more treats the other end as failed rather than
/// reading on, so an enormous message cannot make it allocate without
/// bound.
pub const MAX_MESSAGE_BYTES: usize = verbatim_process::transport::MAX_FRAME_BYTES;

/// Writes one message as one frame and flushes.
///
/// The frame goes to `writer` in one `write_all`, so on a bare pipe, as the
/// listener and every outpost write to, a message is one `WriteFile`
/// (`docs/design/communication.md`, section 4.3).
///
/// # Errors
///
/// Returns any I/O error from the underlying writer, and
/// [`io::ErrorKind::InvalidInput`] for a message larger than
/// [`MAX_MESSAGE_BYTES`], which is not sent; serialization of these
/// message types cannot fail.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    let frame = encode_message(message)?;
    writer.write_all(&frame)?;
    writer.flush()
}

/// One message as the frame [`write_message`] writes.
///
/// # Errors
///
/// As [`write_message`], without the I/O.
pub fn encode_message<T: Serialize>(message: &T) -> io::Result<Vec<u8>> {
    let mut frame = Vec::new();
    put_message(&mut frame, message)?;
    Ok(frame)
}

/// Puts one message's frame in `buffer`, which is cleared first, so a
/// sender encodes straight into the buffer it writes from.
///
/// # Errors
///
/// As [`write_message`], without the I/O.
pub fn put_message<T: Serialize>(buffer: &mut Vec<u8>, message: &T) -> io::Result<()> {
    verbatim_process::transport::put_frame(buffer, |body| {
        serde_json::to_writer(body, message).map_err(io::Error::other)
    })
}

/// Reads one message's frame. Returns `Ok(None)` on clean end of stream:
/// the peer closed its pipe end between messages.
///
/// # Errors
///
/// Returns an error for I/O failures, for the stream ending inside a
/// frame, for a frame larger than [`MAX_MESSAGE_BYTES`]
/// ([`io::ErrorKind::InvalidData`], read no further than its header), and
/// for a frame that is not a valid message (a protocol violation, not a
/// recoverable condition).
pub fn read_message<R: BufRead, T: DeserializeOwned>(reader: &mut R) -> io::Result<Option<T>> {
    let Some(body) = verbatim_process::transport::read_frame(reader)? else {
        return Ok(None);
    };
    decode_message(&body).map(Some)
}

/// The kind of message a frame's body holds, read from the body's first
/// bytes without parsing the rest: the name of its variant, which serde's
/// external tagging writes first, as `{"Pong":...}` for a variant with
/// fields and `"TargetExited"` for one without. `None` for a body that
/// starts neither way. verbatim.exe's relay reads it on every frame from
/// an outpost, to handle pongs, `Ready` and `TargetExited` itself and pass
/// every other frame on whole (`docs/design/communication.md`, section
/// 6.2).
#[must_use]
pub fn message_kind(body: &[u8]) -> Option<&str> {
    let name = body
        .strip_prefix(b"{\"")
        .or_else(|| body.strip_prefix(b"\""))?;
    let end = name.iter().position(|&byte| byte == b'"')?;
    std::str::from_utf8(&name[..end]).ok()
}

/// Parses one frame's body as a message.
///
/// # Errors
///
/// Returns an error for a body that is not a valid message.
pub fn decode_message<T: DeserializeOwned>(body: &[u8]) -> io::Result<T> {
    serde_json::from_slice(body).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{
        NodeDetails, NodeId, NodeSnapshot, NotificationKind, NotificationProcessing, Role, State,
        StateSet,
    };

    /// The kind read from a body's first bytes is the variant's name, for
    /// a variant with fields and for one without.
    #[test]
    fn a_frames_kind_is_its_variants_name() {
        let kind_of = |message: &OutpostToSupervisor| {
            let frame = encode_message(message).expect("encoded");
            let body = verbatim_process::transport::read_frame(&mut frame.as_slice())
                .expect("a frame")
                .expect("not the end");
            message_kind(&body).map(str::to_owned)
        };
        let pong = OutpostToSupervisor::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        };
        assert_eq!(kind_of(&pong).as_deref(), Some("Pong"));
        assert_eq!(
            kind_of(&OutpostToSupervisor::TargetExited).as_deref(),
            Some("TargetExited")
        );
        assert_eq!(
            kind_of(&OutpostToSupervisor::Ready {
                outpost_pid: Pid(1),
                target_pid: Pid(2),
            })
            .as_deref(),
            Some("Ready")
        );
        assert_eq!(message_kind(b"[]"), None);
    }

    #[test]
    fn a_message_larger_than_the_limit_is_neither_sent_nor_read() {
        let mut written = Vec::new();
        let large = "x".repeat(MAX_MESSAGE_BYTES);
        let error = write_message(&mut written, &large).expect_err("too large to send");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(written, Vec::<u8>::new());
        // A peer that sends one anyway: the reader stops at its header.
        let header = format!("Content-Length: {}\r\n\r\n", MAX_MESSAGE_BYTES + 2);
        let mut stream = std::io::Cursor::new(format!("{header}\"{large}\"").into_bytes());
        let error = read_message::<_, String>(&mut stream).expect_err("too large to read");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            usize::try_from(stream.position()).expect("a position"),
            header.len()
        );
        // One at the limit is read.
        let fits = "y".repeat(MAX_MESSAGE_BYTES - 2);
        let mut written = Vec::new();
        write_message(&mut written, &fits).expect("fits");
        let read: Option<String> = read_message(&mut std::io::Cursor::new(written)).expect("reads");
        assert_eq!(read.as_deref(), Some(fits.as_str()));
    }

    #[test]
    fn a_message_and_its_header_are_one_write() {
        /// Records each write it is given.
        #[derive(Default)]
        struct Writes(Vec<Vec<u8>>);
        impl Write for Writes {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.push(bytes.to_vec());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut writes = Writes::default();
        write_message(&mut writes, &"one").expect("written");
        assert_eq!(
            writes.0,
            vec![b"Content-Length: 0000000005\r\n\r\n\"one\"".to_vec()]
        );
    }

    #[test]
    fn messages_round_trip_over_a_byte_stream() {
        let event = OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_000,
            backend: Backend::Msaa,
            window: None,
            application: Some(Pid(20)),
            event: NormalizedEvent::FocusChanged {
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
                node: NodeSnapshot {
                    id: NodeId::new(1),
                    backend: Backend::Msaa,
                    role: Role::MenuItem,
                    name: Some("Settings...".into()),
                    value: None,
                    states: StateSet::new().with(State::Focused),
                    details: NodeDetails::default(),
                },
                text: None,
            },
        };
        let ready = OutpostToSupervisor::Ready {
            outpost_pid: Pid(4242),
            target_pid: Pid(1234),
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &event).expect("writes");
        write_message(&mut buffer, &ready).expect("writes");

        let mut reader = buffer.as_slice();
        let first: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let second: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(first, event);
        assert_eq!(second, ready);
        let end: Option<OutpostToSupervisor> = read_message(&mut reader).expect("reads");
        assert!(end.is_none(), "end of stream reads as None");
    }

    #[test]
    fn stamping_names_every_node_id_in_a_message_with_its_outpost() {
        let snapshot = |number| NodeSnapshot {
            id: NodeId::new(number),
            backend: Backend::Uia,
            role: Role::Button,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let mut event = OutpostToSupervisor::Event {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 0,
            backend: Backend::Uia,
            window: None,
            application: None,
            event: NormalizedEvent::FocusChanged {
                node: snapshot(1),
                foreground: false,
                ancestors: vec![snapshot(2)],
                ancestors_unknown: false,
                selected_child: Some(snapshot(3)),
                text: None,
            },
        };
        let mut chain = OutpostToSupervisor::Reply {
            trace_id: TraceId::mint(),
            request_id: 1,
            outcome: QueryOutcome::Done(QueryResult::Ancestors(vec![snapshot(4)])),
            timing: EventTiming::default(),
        };
        event.assign_outpost(OutpostId(7));
        chain.assign_outpost(OutpostId(7));

        let OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    ancestors,
                    ancestors_unknown: false,
                    selected_child,
                    ..
                },
            ..
        } = event
        else {
            panic!("still a focus event");
        };
        let OutpostToSupervisor::Reply {
            outcome: QueryOutcome::Done(QueryResult::Ancestors(chain)),
            ..
        } = chain
        else {
            panic!("still an ancestor chain");
        };
        for (id, number) in [
            (node.id, 1),
            (ancestors[0].id, 2),
            (selected_child.expect("kept").id, 3),
            (chain[0].id, 4),
        ] {
            assert_eq!(id, NodeId::in_outpost(OutpostId(7), number));
        }
    }

    #[test]
    fn ping_and_pong_round_trip() {
        let ping = SupervisorToOutpost::Ping {
            seq: 5,
            sent_at_us: 4_000,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &ping).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, ping);

        let expected_pong = OutpostToSupervisor::Pong {
            seq: 5,
            parked_count: 3,
            written_at_us: 4_500,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &expected_pong).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, expected_pong);
    }

    #[test]
    fn a_reply_or_timing_from_an_older_peer_reads_with_no_calls() {
        let reply: OutpostToSupervisor =
            serde_json::from_str(r#"{"Reply":{"trace_id":7,"request_id":3,"outcome":"Gone"}}"#)
                .expect("an older reply still reads");
        let OutpostToSupervisor::Reply { timing, .. } = reply else {
            panic!("still a reply");
        };
        assert_eq!(timing, EventTiming::default());
        let timing: EventTiming =
            serde_json::from_str(r#"{"observed_at_us":5}"#).expect("older timing still reads");
        assert!(timing.calls.is_empty());
    }

    #[test]
    fn garbage_is_a_protocol_error() {
        let mut reader: &[u8] = b"not json\n";
        let result: io::Result<Option<SupervisorToOutpost>> = read_message(&mut reader);
        assert!(result.is_err());
    }

    #[test]
    fn focus_fact_and_deliver_fact_round_trip() {
        let snapshot = UiaSnapshotFact {
            runtime_id: vec![42, 7],
            role: Role::MenuItem,
            name: Some("Settings...".into()),
            value: None,
            states: StateSet::new().with(State::Focused),
            details: NodeDetails::default(),
        };
        let read = UiaFocusRead {
            hwnd: 0,
            snapshot: snapshot.clone(),
            ancestors: Some(vec![UiaAncestorFact {
                snapshot: snapshot.clone(),
                hwnd: 0x40,
                content: true,
            }]),
            selected_child: None,
            value_from_selection: false,
            window: Some(0x40),
            same_as_last: Some(false),
        };
        let fact = ListenerFact {
            pid: Pid(1234),
            fact: DeliveredFact::UiaFocusRead {
                marker: 9,
                read: Box::new(read.clone()),
                focus: true,
            },
        };
        let focus_fact = OutpostToSupervisor::FocusFact {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_000,
            fact: fact.clone(),
            win_event: None,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &focus_fact).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: OutpostToSupervisor = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, focus_fact);

        // The delivered fact is the same address minus the pid.
        assert_eq!(fact.pid(), Pid(1234));
        assert_eq!(
            fact.into_delivered(),
            DeliveredFact::UiaFocusRead {
                marker: 9,
                read: Box::new(read),
                focus: true,
            }
        );
        for fact in [
            DeliveredFact::UiaFocusMarker { marker: 9 },
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                evidence: DropEvidence::NoWindow,
            },
            DeliveredFact::UiaFocusReadsEnded,
        ] {
            let deliver = SupervisorToOutpost::DeliverFact {
                trace_id: TraceId::mint(),
                timing: EventTiming::default(),
                observed_at_ms: 1,
                fact,
                win_event: None,
            };
            let mut buffer = Vec::new();
            write_message(&mut buffer, &deliver).expect("writes");
            let read_back: Option<SupervisorToOutpost> =
                read_message(&mut buffer.as_slice()).expect("reads");
            assert_eq!(read_back, Some(deliver));
        }

        let deliver = SupervisorToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
            timing: EventTiming::default(),
            observed_at_ms: 1_752_000_000_001,
            fact: DeliveredFact::MsaaFocus {
                hwnd: 0x1234,
                id_object: -4,
                id_child: 0,
            },
            win_event: Some(WinEventStamp {
                thread: 77,
                time: 1000,
            }),
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &deliver).expect("writes");
        let mut reader = buffer.as_slice();
        let read_back: SupervisorToOutpost = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_back, deliver);
    }

    #[test]
    fn the_hand_over_messages_round_trip() {
        let pid = Pid(1234);
        let outpost = OutpostId(5);
        for command in [
            SupervisorToOutpost::HookInstalled {
                pid,
                outpost,
                sent_at_us: 4_000,
            },
            SupervisorToOutpost::HookRemoved {
                pid,
                sent_at_us: 4_000,
            },
            SupervisorToOutpost::ForwardingEnded { through: 99 },
        ] {
            let mut buffer = Vec::new();
            write_message(&mut buffer, &command).expect("writes");
            let read_back: Option<SupervisorToOutpost> =
                read_message(&mut buffer.as_slice()).expect("reads");
            assert_eq!(read_back, Some(command));
        }
        let ended = OutpostToSupervisor::ForwardingEnded {
            pid,
            outpost,
            through: 99,
        };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &ended).expect("writes");
        let read_back: Option<OutpostToSupervisor> =
            read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(ended));
    }

    #[test]
    fn the_idle_barriers_query_and_answer_round_trip() {
        let query = SupervisorToOutpost::IdleQuery { round: 12 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &query).expect("writes");
        let read_back: Option<SupervisorToOutpost> =
            read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(query));
        let answer = OutpostToSupervisor::IdleAnswer { round: 12 };
        let mut buffer = Vec::new();
        write_message(&mut buffer, &answer).expect("writes");
        let read_back: Option<OutpostToSupervisor> =
            read_message(&mut buffer.as_slice()).expect("reads");
        assert_eq!(read_back, Some(answer));
        // It carries no node, so Core and the outpost do not count it.
        assert!(!OutpostToSupervisor::IdleAnswer { round: 12 }.carries_nodes());
    }

    #[test]
    fn a_win_event_is_raised_before_or_after_a_tick_across_the_clocks_wrap() {
        let at = |time| WinEventStamp { thread: 1, time };
        assert!(at(20).raised_after(10));
        assert!(at(5).raised_before(10));
        assert!(!at(10).raised_after(10) && !at(10).raised_before(10));
        // Just after the clock wrapped is after just before it.
        assert!(at(3).raised_after(u32::MAX - 3));
        assert!(at(u32::MAX - 3).raised_before(3));
    }

    #[test]
    fn only_the_five_win_event_kinds_with_a_window_make_a_fact() {
        use verbatim_ia2::WinEventKind;
        for kind in [
            WinEventKind::Focus,
            WinEventKind::Foreground,
            WinEventKind::MenuPopupStart,
            WinEventKind::Alert,
            WinEventKind::Show,
        ] {
            let fact = DeliveredFact::from_win_event(kind, (7, -4, 0))
                .unwrap_or_else(|| panic!("{kind:?} makes a fact"));
            assert!(fact.is_win_event(), "{kind:?}");
            assert_eq!(DeliveredFact::from_win_event(kind, (0, -4, 0)), None);
        }
        assert_eq!(
            DeliveredFact::from_win_event(WinEventKind::NameChange, (7, -4, 0)),
            None
        );
    }

    #[test]
    fn a_selection_or_a_background_notification_may_not_start_an_outpost() {
        let snapshot = UiaSnapshotFact {
            runtime_id: vec![1],
            role: Role::ListItem,
            name: None,
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let selection = DeliveredFact::UiaSelection {
            hwnd: 0,
            focus_window: 0,
            snapshot: snapshot.clone(),
        };
        assert!(!selection.may_start_outpost());
        let notification = |activity_id: &str| DeliveredFact::UiaNotification {
            hwnd: 0,
            snapshot: snapshot.clone(),
            notification: Notification {
                kind: NotificationKind::ActionCompleted,
                processing: NotificationProcessing::ImportantAll,
                display_string: Some("Copied".into()),
                activity_id: Some(activity_id.into()),
            },
        };
        assert!(!notification("Clipboard").may_start_outpost());
        // A focus's drop, and the end of a listener's reads, only resolve
        // markers an outpost holds.
        assert!(
            !DeliveredFact::UiaFocusDropped {
                marker: 1,
                evidence: DropEvidence::Focus,
            }
            .may_start_outpost()
        );
        assert!(!DeliveredFact::UiaFocusReadsEnded.may_start_outpost());
        for fact in [
            notification(verbatim_core::SNAP_RESULTS_ACTIVITY),
            DeliveredFact::Foreground { hwnd: 1 },
            DeliveredFact::UiaFocusMarker { marker: 1 },
            DeliveredFact::Alert {
                hwnd: 1,
                id_object: -4,
                id_child: 0,
            },
        ] {
            assert!(fact.may_start_outpost(), "{fact:?}");
        }
    }

    #[test]
    fn queries_and_every_outcome_round_trip() {
        let node = NodeSnapshot {
            id: NodeId::new(9),
            backend: Backend::Uia,
            role: Role::Window,
            name: Some("Verbatim".into()),
            value: None,
            states: StateSet::new(),
            details: NodeDetails::default(),
        };
        let mut buffer = Vec::new();
        let queries = [
            Query::FocusNow,
            Query::Navigate {
                node_id: NodeId::new(9),
                kind: QueryKind::NextSibling,
            },
            Query::Activate {
                node_id: NodeId::new(9),
            },
            Query::Ancestors {
                node_id: NodeId::new(9),
            },
            Query::DumpTree,
        ];
        let commands: Vec<SupervisorToOutpost> = queries
            .into_iter()
            .map(|query| SupervisorToOutpost::Query {
                trace_id: TraceId::mint(),
                request_id: 3,
                query,
                timing: EventTiming::default(),
            })
            .chain([SupervisorToOutpost::Cancel { request_id: 3 }])
            .collect();
        for command in &commands {
            write_message(&mut buffer, command).expect("writes");
        }
        let outcomes = [
            QueryOutcome::Done(QueryResult::Focus(FocusNow {
                window: None,
                focus: Some(FocusedControl {
                    node: node.clone(),
                    ancestors: None,
                    selected_child: None,
                    window: None,
                    text: None,
                }),
                observed_at_ms: 42,
            })),
            QueryOutcome::Done(QueryResult::Navigated(None)),
            QueryOutcome::Done(QueryResult::Tree(DumpedTree {
                root: TreeNode {
                    snapshot: node,
                    children: Vec::new(),
                },
                truncated: true,
            })),
            QueryOutcome::Gone,
            QueryOutcome::Failed("no activation pattern".to_owned()),
            QueryOutcome::NotStarted,
            QueryOutcome::Abandoned,
        ];
        let replies: Vec<OutpostToSupervisor> = outcomes
            .into_iter()
            .map(|outcome| OutpostToSupervisor::Reply {
                trace_id: TraceId::mint(),
                request_id: 3,
                outcome,
                timing: EventTiming {
                    dequeued_at_us: 10,
                    published_at_us: 20,
                    calls: CallCounts {
                        uia: 3,
                        msaa: 0,
                        window_messages: 1,
                        connections: 0,
                    },
                    ..EventTiming::default()
                },
            })
            .collect();
        for reply in &replies {
            write_message(&mut buffer, reply).expect("writes");
        }

        let mut reader = buffer.as_slice();
        for command in &commands {
            let read: SupervisorToOutpost = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, command);
        }
        for reply in &replies {
            let read: OutpostToSupervisor = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, reply);
        }
    }

    #[test]
    fn a_pongs_trip_back_runs_from_its_write_to_verbatim_exes_read() {
        let mut pong = OutpostToSupervisor::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        };
        pong.written(7_000);
        let OutpostToSupervisor::Pong { written_at_us, .. } = pong else {
            unreachable!("a pong stays a pong");
        };
        assert_eq!(hop_us(written_at_us, 7_250), Some(250));
    }

    #[test]
    fn a_menu_ends_trip_runs_from_the_listeners_write_to_verbatim_exes_read() {
        let mut ended = OutpostToSupervisor::MenuOrSwitchEnded {
            ended_at_ms: 1,
            timing: EventTiming::default(),
        };
        ended.written(7_000);
        let OutpostToSupervisor::MenuOrSwitchEnded { timing, .. } = ended else {
            unreachable!("a menu end stays a menu end");
        };
        assert_eq!(hop_us(timing.listener_written_at_us, 7_250), Some(250));
    }
}
