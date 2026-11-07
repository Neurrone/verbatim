//! The focus listener's vocabulary for a focus event, what its reading
//! threads and its read scheduler ([`crate::schedule`]) are written in: the
//! event's kind ([`FocusEvent`]), what the callback knew of it ([`Meta`])
//! and where it was queued ([`Arrival`]), what became of it ([`Judged`]),
//! and the reasons an event is dropped.

use verbatim_model::TraceId;
use verbatim_model::protocol::{DeliveredFact, EventTiming, UiaFocusRead};

use crate::schedule::QueuePlace;

/// How many of an application's reading threads may be abandoned and still
/// in their calls before it gets no new thread: its focus events then wait
/// for the first of those threads whose call returns, within the bound
/// the listener's read deadline (`verbatim_model::FOCUS_READ_DEADLINE`) states.
pub const ABANDONED_LIMIT: usize = 2;

/// Why an event is dropped whose read the watchdog abandoned at the
/// deadline.
pub const NOT_IN_TIME: &str = "its read did not return in time";

/// Why an event is dropped whose read the watchdog abandoned because the
/// user moved on from its window, as an outpost's watchdog says it.
pub const MOVED_ON: &str = "the user moved on from a slow window";

/// Why an event is dropped that waited for a thread while its application's
/// abandoned threads were at [`ABANDONED_LIMIT`] until the bound
/// the listener's read deadline (`verbatim_model::FOCUS_READ_DEADLINE`) states, or arrived after that bound.
pub const WAITED_TOO_LONG: &str = "it waited too long behind its application's abandoned reads";

/// The kind of UIA event a reading thread takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvent {
    /// A focus change (`HandleFocusChangedEvent`).
    Focus,
    /// A menu opened (`MenuOpened`), which NVDA maps to a focus on the menu
    /// and handles in its automation event handler
    /// (`IUIAutomationEventHandler_HandleAutomationEvent`,
    /// `UIAHandler/__init__.py` 757 to 868).
    MenuOpened,
    /// An element selected, judged by its sender's nearest window
    /// (the listener's `FocusReader::check_fact`).
    Selection,
    /// Core's live re-read of the thread's last read: its held element read
    /// again with no ancestry cache (the listener's
    /// `FocusReader::live_reread`). It has no marker the outpost waits on,
    /// moves away from no application, and is no pending focus.
    LiveReread,
    /// A notification, judged by its sender's nearest window
    /// (the listener's `FocusReader::check_fact`).
    Notification,
}

/// The reason a focus or selection is dropped when its sender has no
/// window above it, which in a new File Explorer window is a sender gone
/// before its read (`books/dev/src/explanation/design/listener-without-connections.md`, risk 13).
pub const NO_WINDOW: &str = "it has no window above it, so NVDA makes no object for it";

/// The reason a focus is dropped when the lookup of its sender's nearest
/// window failed with `UIA_E_TIMEOUT` (`verbatim_uia::nearest_window_lookup`),
/// however long it took: NVDA's lookup answers no window and NVDA drops the
/// focus, and the listener reads nothing more into an application that has
/// just failed to answer (`docs/design/focus-pipeline-rework.md`, the
/// audit's difference 14).
pub const NEAREST_TIMED_OUT: &str = "its nearest window's lookup timed out, so NVDA drops it";

/// The reason a notification is dropped when its sender has no window
/// above it.
pub const NOTIFICATION_NO_WINDOW: &str = "its sender has no window above it";

/// The reason a focus is dropped whose element no longer has the keyboard
/// focus when it is read, as NVDA drops it.
pub const NOT_FOCUSED: &str = "its element no longer has the keyboard focus";

/// The reason an event is dropped that a newer event of the same kind and
/// element replaced while it waited, as NVDA's UIA limiter keeps one.
pub const REPLACED: &str = "a newer focus event of its element replaced it";

/// The reason a menu opening is dropped whose comparison with the current
/// focus failed.
pub const COMPARISON_FAILED: &str = "its comparison with the current focus failed";

/// The reason a live re-read is answered gone: the reading thread no longer
/// holds the read it names, by trace, as after a later read or none.
pub const NOT_THE_LAST_READ: &str = "its reading thread no longer holds the read it names";

/// The reason a live re-read is answered gone when its application has no
/// reading thread.
pub const NO_READING_THREAD: &str = "its application has no reading thread";

/// The reason a focus is dropped that fails NVDA's acceptance test.
pub const NOT_RELATED: &str = "its window is not related to the foreground window";

/// The reason a focus in Core's hidden frame is dropped, as its outpost
/// would drop it once read.
pub const HIDDEN_FRAME: &str = "Core's hidden frame";

/// The reason the console window's own element's focus is dropped.
pub const CONSOLE_OWN: &str = "the console window's own focus";

/// The reason a focus is skipped whose nearest window's class is on
/// NVDA's MSAA class list, by the class verdict the outposts' arbitrators
/// use (`verbatim_reader_rules::class::class_verdict`), so the outpost judges that window MSAA too.
pub const MSAA_CLASS: &str =
    "its window's class is on NVDA's MSAA list, so its MSAA events speak for it";

/// What became of one focus event.
#[derive(Debug)]
pub enum Judged {
    /// The focus, read.
    Read(Box<UiaFocusRead>),
    /// Not reported, for the reason given.
    Dropped(&'static str),
    /// A selection or notification that passed its checks, to be passed on
    /// as it now stands (the listener's `FocusReader::check_fact`).
    Passed(Box<DeliveredFact>),
}

impl Judged {
    /// The fact that resolves marker `marker`, sent for an event of kind
    /// `kind`, with this outcome.
    #[must_use]
    pub fn into_fact(self, marker: u64, kind: FocusEvent) -> DeliveredFact {
        let focus = kind == FocusEvent::Focus;
        match self {
            Judged::Read(read) => DeliveredFact::UiaFocusRead {
                marker,
                read,
                focus,
            },
            Judged::Dropped(_) => DeliveredFact::UiaFocusDropped { marker, focus },
            Judged::Passed(fact) => DeliveredFact::UiaMarked { marker, fact },
        }
    }

    /// The fact that answers Core's live re-read `request`, with this
    /// outcome: the read, carrying `request` and the marker `marker`, which
    /// no outpost waits on, so it is taken in where it arrives; or, for
    /// any other outcome, its answer of gone.
    #[must_use]
    pub fn into_reread_fact(self, marker: u64, request: u64) -> DeliveredFact {
        match self {
            Judged::Read(mut read) => {
                read.answers = Some(request);
                read.cached_from = None;
                DeliveredFact::UiaFocusRead {
                    marker,
                    read,
                    focus: true,
                }
            }
            Judged::Dropped(_) | Judged::Passed(_) => DeliveredFact::UiaLiveRereadGone { request },
        }
    }

    /// Why it was not reported, for one that was dropped.
    #[must_use]
    pub fn dropped_reason(&self) -> Option<&'static str> {
        match self {
            Judged::Dropped(reason) => Some(reason),
            Judged::Read(_) | Judged::Passed(_) => None,
        }
    }
}

/// The UIA event a focus came in: its trace, when the listener received
/// it, on the run's shared clock, and its place in the order the reading
/// threads' events were queued in, with the newest place at which the user
/// moved away from its application before it ([`QueuePlace`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Arrival {
    /// The trace minted for the event.
    pub trace: TraceId,
    /// When the event was received, in microseconds on the run's shared
    /// clock.
    pub observed_at_us: u64,
    /// The event's place in the order events were queued in, 1 for the
    /// first; 0 for an event never queued, whose read no later menu
    /// opening compares with.
    pub place: u64,
    /// The place of the newest event, before this one, that moved away
    /// from its application: another application's UIA focus or menu
    /// opening, another process's MSAA focus, or a foreground event of
    /// another top-level window than its thread's last read's.
    pub left_at: u64,
}

impl Arrival {
    /// Its place in the order events were queued in.
    #[must_use]
    pub fn queue(self) -> QueuePlace {
        QueuePlace {
            place: self.place,
            left_at: self.left_at,
        }
    }
}

/// What the listener knows of a focus event when its callback queues it:
/// the marker it sent, and the trace and times its read or drop carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    /// The marker sent for the event.
    pub marker: u64,
    /// The event's kind, which its read or drop tells the outpost.
    pub kind: FocusEvent,
    /// The trace minted when the event arrived.
    pub trace: TraceId,
    /// When it arrived, its `observed_at_us` the observation by which its
    /// fact is ordered, and the latency log's stamps.
    pub timing: EventTiming,
}

/// The [`Arrival`] of the event `meta` describes, queued at `queue`.
#[must_use]
pub fn arrival_of(meta: Meta, queue: QueuePlace) -> Arrival {
    Arrival {
        trace: meta.trace,
        observed_at_us: meta.timing.observed_at_us,
        place: queue.place,
        left_at: queue.left_at,
    }
}
