//! The focus listener's vocabulary for a focus event, what its reading
//! threads and its read scheduler ([`crate::schedule`]) are written in: the
//! event's kind ([`FocusEvent`]), what the callback knew of it ([`Meta`])
//! and where it was queued ([`Arrival`]), what became of it ([`Judged`]),
//! the reasons an event is dropped, and what each outcome says of its
//! application's gap record ([`gap_answer`]).

use std::time::Duration;

use verbatim_model::TraceId;
use verbatim_model::protocol::{DeliveredFact, EventTiming, GapAnswer, UiaFocusRead};

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
/// window and NVDA drops the focus, and nothing more is read into an
/// application that has just failed to answer, so its drop answers the gap
/// record ([`GAVE_UP`]; `docs/design/focus-pipeline-rework.md`, the audit's
/// difference 14).
pub const NEAREST_TIMED_OUT: &str = "its nearest window's lookup timed out, so NVDA drops it";

/// The reason a focus is dropped when the lookup of its sender's nearest
/// window failed with `UIA_E_TIMEOUT` in under [`NEAREST_FAILED_FAST_BOUND`]:
/// NVDA drops the focus as for [`NEAREST_TIMED_OUT`], but the application
/// did not keep the lookup waiting, as Settings' lookup failed after 1.1 ms
/// (`settings_combo_box_expanded`, 2026-10-30), so the drop leaves the gap
/// record open and the outpost's read at the record's bound reads the
/// focus ([`gap_answer`]; Dickson, 2026-10-30). A limit: in an application
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

/// The drops of a focus that are NVDA's checks' verdict on its event, as
/// its read would have been, which answer its application's gap record
/// ([`gap_answer`]): the class-list skip among them, since the outpost
/// judges that window MSAA by the same function.
pub const VERDICTS: [&str; 6] = [
    MSAA_CLASS,
    HUNG,
    COMPARISON_FAILED,
    NOT_RELATED,
    CONSOLE_OWN,
    HIDDEN_FRAME,
];

/// The drops of a focus because the listener gave up on its application,
/// which did not answer in time, which answer the application's gap
/// record wherever its copy is ([`gap_answer`]): a read into that
/// application by the outpost would call into one that has just failed to
/// answer, so the wait ends without one (Dickson, 2026-10-21). The drop
/// whose event said it lacked the keyboard focus when the read's time was
/// up is among them, a judgement call of its own (Dickson, 2026-10-21):
/// its time ran out as the others' did; so is a nearest window's lookup
/// that timed out after [`NEAREST_FAILED_FAST_BOUND`] or more
/// ([`NEAREST_TIMED_OUT`]; `docs/design/focus-pipeline-rework.md`, the
/// audit's difference 14). One that failed fast ([`NEAREST_FAILED_FAST`])
/// is not among them (Dickson, 2026-10-30).
pub const GAVE_UP: [&str; 5] = [
    NOT_IN_TIME,
    WAITED_TOO_LONG,
    OUT_OF_TIME_BEFORE_LOOKUP,
    UNFOCUSED_OUT_OF_TIME,
    NEAREST_TIMED_OUT,
];

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
    /// `kind`, with this outcome; a drop carries its answer to the
    /// application's gap record ([`gap_answer`]), placed in the top-level
    /// window `placed` when its read found one.
    #[must_use]
    pub fn into_fact(self, marker: u64, kind: FocusEvent, placed: Option<isize>) -> DeliveredFact {
        let answer = gap_answer(kind, &self, placed);
        match self {
            Judged::Read(read) => DeliveredFact::UiaFocusRead {
                marker,
                read,
                focus: kind == FocusEvent::Focus,
            },
            Judged::Dropped(_) => DeliveredFact::UiaFocusDropped {
                marker,
                focus: kind == FocusEvent::Focus,
                answer,
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

/// What the outcome `judged` of an event of kind `kind`, placed in the
/// top-level window `placed` when its read found one, says of its
/// application's gap record (`docs/design/focus-pipeline-rework.md`, "Who
/// owns each duplicated fact", the gap's evidence), which the drop carries
/// to the outpost, whose intake keeps the record. Only a focus's outcome
/// answers it. A read and a verdict of NVDA's checks ([`VERDICTS`]), the
/// class-list skip among them, answer a copy in their top-level window, or
/// in any window when the read found none. The
/// listener giving up on the application ([`GAVE_UP`]), a nearest
/// window's lookup that timed out among them ([`NEAREST_TIMED_OUT`]),
/// answers a copy in any window. A drop for no window above the sender ([`NO_WINDOW`])
/// answers a copy in any window too, which its outpost is to read at once
/// in its place: the focus came and its sender, gone, cannot be read, and
/// the event for where the focus
/// went may never come (`docs/design/listener-without-connections.md`,
/// risk 13). Any other drop leaves the record open: one no longer focused
/// ([`NOT_FOCUSED`]) or replaced by a newer event of its element comes in
/// fast navigation, where it says nothing of the copy's focus (Dickson,
/// 2026-10-21); so does a nearest window's lookup that failed fast
/// ([`NEAREST_FAILED_FAST`]), which leaves the focus to the outpost's read
/// at the record's bound (Dickson, 2026-10-30); and so do the rest, which
/// could not establish the focus.
#[must_use]
pub fn gap_answer(kind: FocusEvent, judged: &Judged, placed: Option<isize>) -> GapAnswer {
    if kind != FocusEvent::Focus {
        return GapAnswer::LeavesOpen;
    }
    match judged {
        Judged::Read(_) => GapAnswer::Answers(placed),
        Judged::Dropped(reason) if VERDICTS.contains(reason) => GapAnswer::Answers(placed),
        Judged::Dropped(reason) if GAVE_UP.contains(reason) => GapAnswer::Answers(None),
        Judged::Dropped(reason) if *reason == NO_WINDOW => GapAnswer::ReadsNow,
        Judged::Dropped(_) | Judged::Passed(_) => GapAnswer::LeavesOpen,
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

    /// What each outcome of a focus event says of its application's gap
    /// record: a read or a verdict of NVDA's checks, the class-list skip
    /// among them, answers a copy in the top-level window its read found;
    /// the listener giving up on the application answers a copy in any
    /// window, and a drop for no window answers it to be read at once; and
    /// every other drop of a focus leaves the record open, a nearest
    /// window's lookup that failed fast among them, as does any outcome of a
    /// selection, a notification, or a menu opening. Every reason the
    /// listener drops a focus for is listed.
    #[test]
    fn a_focus_events_drop_answers_its_gap_record_or_leaves_it_open() {
        let placed = Some(70);
        let dropped = |reason| gap_answer(FocusEvent::Focus, &Judged::Dropped(reason), placed);
        assert_eq!(
            gap_answer(FocusEvent::Focus, &read_focus(), placed),
            GapAnswer::Answers(placed)
        );
        for verdict in [
            "its window's class is on NVDA's MSAA list, so its MSAA events speak for it",
            "its window is hung",
            "its comparison with the current focus failed",
            "its window is not related to the foreground window",
            "the console window's own focus",
            "Core's hidden frame",
        ] {
            assert_eq!(dropped(verdict), GapAnswer::Answers(placed), "{verdict}");
        }
        assert_eq!(
            dropped("it has no window above it, so NVDA makes no object for it"),
            GapAnswer::ReadsNow
        );
        for anywhere in [
            "its read did not return in time",
            "it waited too long behind its application's abandoned reads",
            "its read ran out of time before its window was looked up",
            "its event says it did not have the keyboard focus, and its read's time was up",
        ] {
            assert_eq!(dropped(anywhere), GapAnswer::Answers(None), "{anywhere}");
        }
        for open in [
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
            assert_eq!(dropped(open), GapAnswer::LeavesOpen, "{open}");
        }
        for kind in [
            FocusEvent::Selection,
            FocusEvent::Notification,
            FocusEvent::MenuOpened,
        ] {
            assert_eq!(
                gap_answer(kind, &read_focus(), placed),
                GapAnswer::LeavesOpen
            );
        }
    }

    /// A focus whose nearest window's lookup failed with `UIA_E_TIMEOUT`
    /// in under a second, as Settings' did after 1.1 ms, is dropped for
    /// failing fast, and its drop leaves the gap record open.
    #[test]
    fn a_nearest_window_lookup_that_times_out_fast_leaves_the_gap_record_open() {
        let reason = nearest_timeout_reason(Duration::from_micros(999_999));
        assert_eq!(
            reason,
            "its nearest window's lookup failed with a timeout at once, so NVDA drops it"
        );
        assert_eq!(
            Judged::Dropped(reason).into_fact(9, FocusEvent::Focus, Some(70)),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                answer: GapAnswer::LeavesOpen,
            }
        );
    }

    /// A focus whose nearest window's lookup failed with `UIA_E_TIMEOUT`
    /// after a second or more is dropped for timing out, and its drop
    /// answers the gap record in any window.
    #[test]
    fn a_nearest_window_lookup_that_times_out_slowly_answers_the_gap_record() {
        let reason = nearest_timeout_reason(Duration::from_secs(1));
        assert_eq!(
            reason,
            "its nearest window's lookup timed out, so NVDA drops it"
        );
        assert_eq!(
            Judged::Dropped(reason).into_fact(9, FocusEvent::Focus, Some(70)),
            DeliveredFact::UiaFocusDropped {
                marker: 9,
                focus: true,
                answer: GapAnswer::Answers(None),
            }
        );
    }
}
