//! The focus listener's vocabulary for a focus event, what its reading
//! threads and its read scheduler ([`crate::schedule`]) are written in: the
//! event's kind ([`FocusEvent`]), what the callback knew of it ([`Meta`])
//! and where it was queued ([`Arrival`]), what became of it ([`Judged`]),
//! and the reasons an event is dropped. A drop carries one fact for its
//! application's gap record, which the outpost alone closes by its own
//! reports: whether its sender had no window above it
//! (`docs/design/focus-pipeline-rework-remaining.md`, "The gap, closed by
//! the outpost's own reports").

use std::time::Duration;

use verbatim_model::TraceId;
use verbatim_model::protocol::{DeliveredFact, EventTiming, UiaFocusRead};

use crate::schedule::QueuePlace;

/// How many of an application's reading threads may be abandoned and still
/// in their calls before it gets no new thread: its focus events then wait
/// for the first of those threads whose call returns, within the bound
/// the listener's `READ_DEADLINE` states.
pub const ABANDONED_LIMIT: usize = 2;

/// Why an event is dropped whose read the watchdog abandoned at the
/// deadline.
pub const NOT_IN_TIME: &str = "its read did not return in time";

/// Why an event is dropped whose read the watchdog abandoned because the
/// user moved on from its window, as an outpost's watchdog says it.
pub const MOVED_ON: &str = "the user moved on from a slow window";

/// Why an event is dropped that waited for a thread while its application's
/// abandoned threads were at [`ABANDONED_LIMIT`] until the bound
/// the listener's `READ_DEADLINE` states, or arrived after that bound.
pub const WAITED_TOO_LONG: &str = "it waited too long behind its application's abandoned reads";

/// Why a focus is dropped whose event said it lacked the keyboard focus
/// when its read's time was up, before the live read of its focus.
pub const UNFOCUSED_OUT_OF_TIME: &str =
    "its event says it did not have the keyboard focus, and its read's time was up";

/// Why a focus is dropped whose read's time was up before its sender's
/// nearest window was looked up.
pub const OUT_OF_TIME_BEFORE_LOOKUP: &str =
    "its read ran out of time before its window was looked up";

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
    /// A notification, judged by its sender's nearest window
    /// (the listener's `FocusReader::check_fact`).
    Notification,
}

/// The reason a focus or selection is dropped when its sender has no
/// window above it, which in a new File Explorer window is a sender gone
/// before its read (`docs/design/listener-without-connections.md`, risk 13).
pub const NO_WINDOW: &str = "it has no window above it, so NVDA makes no object for it";

/// The reason a focus is dropped when the lookup of its sender's nearest
/// window failed with `UIA_E_TIMEOUT` (`verbatim_uia::nearest_window_lookup`)
/// after [`NEAREST_FAILED_FAST_BOUND`] or more: NVDA's lookup answers no
/// window and NVDA drops the focus, and the listener reads nothing more
/// into an application that has just failed to answer; the outpost's gap
/// record, left to its bound, reads the application's focus anew
/// (`docs/design/focus-pipeline-rework.md`, the audit's difference 14).
pub const NEAREST_TIMED_OUT: &str = "its nearest window's lookup timed out, so NVDA drops it";

/// The reason a focus is dropped when the lookup of its sender's nearest
/// window failed with `UIA_E_TIMEOUT` in under [`NEAREST_FAILED_FAST_BOUND`]:
/// NVDA drops the focus as for [`NEAREST_TIMED_OUT`], but the application
/// did not keep the lookup waiting, as Settings' lookup failed after 1.1 ms
/// (`settings_combo_box_expanded`, 2026-10-30); as every drop, it leaves the
/// gap record to its bound, whose read reads the focus (Dickson,
/// 2026-10-30). A limit: in an application
/// whose lookups keep failing fast, every focus is spoken through the gap,
/// up to a second late, and the focuses between two gap reads are not
/// spoken.
pub const NEAREST_FAILED_FAST: &str =
    "its nearest window's lookup failed with a timeout at once, so NVDA drops it";

/// How long a nearest window's lookup that fails with `UIA_E_TIMEOUT` may
/// take and still count as failing fast ([`NEAREST_FAILED_FAST`]) rather
/// than as timed out ([`NEAREST_TIMED_OUT`]): a second. It is below UIA's
/// 2-second connection timeout (`verbatim_uia::CONNECTION_TIMEOUT`), the
/// shortest wait after which UIA ends a call to an application that does
/// not answer, so a lookup that fails sooner was not kept waiting by its
/// application. It is the UIA focus wait both readers share
/// ([`verbatim_reader_rules::UIA_FOCUS_WAIT`]), the gap record's bound: a
/// lookup that took that long has held its application's later events as
/// long as the record waits for a UIA focus, and is taken as the
/// application not answering in time.
pub const NEAREST_FAILED_FAST_BOUND: Duration = verbatim_reader_rules::UIA_FOCUS_WAIT;

/// The reason a focus is dropped whose nearest window's lookup failed with
/// `UIA_E_TIMEOUT` after `elapsed`: [`NEAREST_FAILED_FAST`] under
/// [`NEAREST_FAILED_FAST_BOUND`], [`NEAREST_TIMED_OUT`] otherwise.
#[must_use]
pub fn nearest_timeout_reason(elapsed: Duration) -> &'static str {
    if elapsed < NEAREST_FAILED_FAST_BOUND {
        NEAREST_FAILED_FAST
    } else {
        NEAREST_TIMED_OUT
    }
}

/// The reason a notification is dropped when its sender has no window
/// above it.
pub const NOTIFICATION_NO_WINDOW: &str = "its sender has no window above it";

/// The reason a focus is dropped whose element no longer has the keyboard
/// focus when it is read, as NVDA drops it.
pub const NOT_FOCUSED: &str = "its element no longer has the keyboard focus";

/// The reason an event is dropped that a newer event of the same kind and
/// element replaced while it waited, as NVDA's UIA limiter keeps one.
pub const REPLACED: &str = "a newer focus event of its element replaced it";

/// The reason a focus is dropped whose own window the system reports hung.
pub const HUNG: &str = "its window is hung";

/// The reason a menu opening is dropped whose comparison with the current
/// focus failed.
pub const COMPARISON_FAILED: &str = "its comparison with the current focus failed";

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
    /// `kind`, with this outcome; a focus's drop carries whether its sender
    /// had no window above it ([`NO_WINDOW`]), the one fact a drop gives its
    /// application's gap record.
    #[must_use]
    pub fn into_fact(self, marker: u64, kind: FocusEvent) -> DeliveredFact {
        let focus = kind == FocusEvent::Focus;
        match self {
            Judged::Read(read) => DeliveredFact::UiaFocusRead {
                marker,
                read,
                focus,
            },
            Judged::Dropped(reason) => DeliveredFact::UiaFocusDropped {
                marker,
                focus,
                no_window: focus && reason == NO_WINDOW,
            },
            Judged::Passed(fact) => DeliveredFact::UiaMarked { marker, fact },
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

#[cfg(test)]
mod tests {
    use verbatim_model::NodeDetails;
    use verbatim_model::protocol::UiaSnapshotFact;

    use super::*;

    /// A read of a focus, as a reading thread reads one.
    fn read_focus() -> Judged {
        Judged::Read(Box::new(UiaFocusRead {
            hwnd: 0,
            snapshot: UiaSnapshotFact {
                runtime_id: vec![42],
                role: verbatim_model::Role::Button,
                name: None,
                value: None,
                states: verbatim_model::StateSet::new(),
                details: NodeDetails::default(),
            },
            ancestors: Vec::new(),
            ancestors_end: verbatim_model::protocol::AncestorsEnd::Top,
            selected_child: None,
            value_from_selection: false,
            window: None,
            same_as_last: None,
            bound_us: 2_000_000,
        }))
    }

    /// A focus's drop carries whether its sender had no window above it,
    /// the one fact a drop gives its application's gap record; every other
    /// drop of a focus, and a drop of any other kind of event, carries none.
    /// Every reason the listener drops a focus for is listed.
    #[test]
    fn a_focus_events_drop_carries_whether_its_sender_had_no_window() {
        let dropped = |reason, kind| match Judged::Dropped(reason).into_fact(9, kind) {
            DeliveredFact::UiaFocusDropped { no_window, .. } => no_window,
            other => panic!("a drop, not {other:?}"),
        };
        assert!(dropped(
            "it has no window above it, so NVDA makes no object for it",
            FocusEvent::Focus
        ));
        for reason in [
            "its window's class is on NVDA's MSAA list, so its MSAA events speak for it",
            "its window is hung",
            "its comparison with the current focus failed",
            "its window is not related to the foreground window",
            "the console window's own focus",
            "Core's hidden frame",
            "its read did not return in time",
            "it waited too long behind its application's abandoned reads",
            "its read ran out of time before its window was looked up",
            "its event says it did not have the keyboard focus, and its read's time was up",
            "its nearest window's lookup timed out, so NVDA drops it",
            "its nearest window's lookup failed with a timeout at once, so NVDA drops it",
            "its element no longer has the keyboard focus",
            "the user moved on from a slow window",
            "a newer focus event of its element replaced it",
            "no thread to read it",
            "the listener is shutting down",
            "its element could not be kept for its read",
            "its reading thread has no UIA client",
            "its element could not be resolved",
        ] {
            assert!(!dropped(reason, FocusEvent::Focus), "{reason}");
        }
        for kind in [
            FocusEvent::Selection,
            FocusEvent::Notification,
            FocusEvent::MenuOpened,
        ] {
            assert!(!dropped(
                "it has no window above it, so NVDA makes no object for it",
                kind
            ));
        }
        assert!(matches!(
            read_focus().into_fact(9, FocusEvent::Focus),
            DeliveredFact::UiaFocusRead {
                marker: 9,
                focus: true,
                ..
            }
        ));
    }

    /// A focus whose nearest window's lookup failed with `UIA_E_TIMEOUT`
    /// in under a second, as Settings' did after 1.1 ms, is dropped for
    /// failing fast, and its drop leaves the gap record to its bound.
    #[test]
    fn a_nearest_window_lookup_that_times_out_fast_leaves_the_record_to_its_bound() {
        let reason = nearest_timeout_reason(Duration::from_micros(999_999));
        assert_eq!(
            reason,
            "its nearest window's lookup failed with a timeout at once, so NVDA drops it"
        );
        assert_eq!(
            Judged::Dropped(reason).into_fact(9, FocusEvent::Focus),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                no_window: false,
            }
        );
    }

    /// A focus whose nearest window's lookup failed with `UIA_E_TIMEOUT`
    /// after a second or more is dropped for timing out, and its drop, as
    /// every drop but one for no window, leaves the gap record to its
    /// bound, whose read reads the application's focus anew.
    #[test]
    fn a_nearest_window_lookup_that_times_out_slowly_leaves_the_record_to_its_bound() {
        let reason = nearest_timeout_reason(Duration::from_secs(1));
        assert_eq!(
            reason,
            "its nearest window's lookup timed out, so NVDA drops it"
        );
        assert_eq!(
            Judged::Dropped(reason).into_fact(9, FocusEvent::Focus),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                no_window: false,
            }
        );
    }
}
