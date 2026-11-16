//! What the mailbox carries to the reducer: an entry, with no application
//! object in it. A UIA event's element and an active text position's range
//! are parked in the store as the entry is pushed, and the entry carries
//! the key they are parked under (`books/dev/src/explanation/design/ownership.md`).

use verbatim_model::protocol::{EventTiming, Query, RequestId, UiaSnapshotFact};
use verbatim_model::{Notification, TraceId};
/// The `WinEvent` kinds an outpost still takes in
/// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in this
/// build"): a console host's console events, which its own hook delivers,
/// and a window Core names destroyed. The mailbox turns a hooked event into
/// one of these at its arrival.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WinKind {
    /// `EVENT_CONSOLE_UPDATE_REGION`, `EVENT_CONSOLE_UPDATE_SIMPLE`,
    /// `EVENT_CONSOLE_UPDATE_SCROLL` or `EVENT_CONSOLE_LAYOUT`: the
    /// console's text changed.
    ConsoleUpdate,
    /// `EVENT_CONSOLE_CARET`: the console's cursor moved.
    ConsoleCaret,
    /// `EVENT_OBJECT_DESTROY` of a window, which Core names.
    Destroy,
}

/// The UIA property ids the reducer tells apart, as `UIAutomationClient.h`
/// numbers them: the reducer names no Windows binding.
pub mod property {
    /// `UIA_NamePropertyId`.
    pub const NAME: i32 = 30005;
    /// `UIA_HelpTextPropertyId`.
    pub const HELP_TEXT: i32 = 30013;
    /// `UIA_ValueValuePropertyId`.
    pub const VALUE: i32 = 30045;
    /// `UIA_RangeValueValuePropertyId`.
    pub const RANGE_VALUE: i32 = 30047;
    /// `UIA_ControllerForPropertyId`.
    pub const CONTROLLER_FOR: i32 = 30104;
}

/// Whether a change of UIA property `id` is reported as a change of the
/// element's states: every property but the name, the value, the range
/// value, the help text and `ControllerFor`, such as the expand state and
/// the toggle state.
#[must_use]
pub fn is_state_property(id: i32) -> bool {
    ![
        property::NAME,
        property::VALUE,
        property::RANGE_VALUE,
        property::HELP_TEXT,
        property::CONTROLLER_FOR,
    ]
    .contains(&id)
}

/// What a UIA event reports.
#[derive(Clone, Debug)]
pub enum UiaKind {
    /// A property changed: name, value, or a state-bearing property.
    Property {
        /// The UI Automation property id.
        id: i32,
        /// Whether the element has the Text pattern, from the event's
        /// cache, which Core's rule for a value change reads.
        text_pattern: bool,
        /// The facts of the followed node's known window, gathered by the
        /// callback with calls to Windows only, which Core's acceptance
        /// reads; no window when no node is followed.
        window: Box<verbatim_model::tell::TellWindow>,
    },
    /// An element was selected within its container.
    Selection,
    /// An application-initiated notification.
    Notification(Notification),
    /// A Windows toast opened (`UIA_Window_WindowOpenedEventId`).
    Toast,
    /// A text focus's caret or selection changed (`Text_TextSelectionChanged`).
    TextSelection,
    /// A text focus's text changed (`Text_TextChanged`).
    TextChanged,
    /// The active position in a text focus's text changed, to the start of
    /// the range parked under this key when the event carried one.
    ActiveTextPosition(Option<u64>),
}

/// A UIA event as its callback captured it: cached properties only, and
/// the key its element is parked under in the store, for anything a
/// program must ask it.
#[derive(Clone, Debug)]
pub struct UiaEvent {
    /// What kind of event it is.
    pub kind: UiaKind,
    /// The element's cached properties.
    pub parts: UiaSnapshotFact,
    /// The element's cached window handle, 0 when it is not a window itself.
    pub hwnd: isize,
    /// For a listener's selection on an element that is not a window
    /// itself, the nearest window the listener's reading thread found, else
    /// 0: the window its acceptance is tested with.
    pub focus_window: isize,
    /// The key its element is parked under, if it came with one.
    pub element: Option<u64>,
}

impl UiaEvent {
    /// The keys of the objects parked for this event.
    pub fn parked(&self) -> impl Iterator<Item = u64> {
        let range = match self.kind {
            UiaKind::ActiveTextPosition(range) => range,
            _ => None,
        };
        self.element.into_iter().chain(range)
    }
}

/// Who asked for an idle barrier ([`Item::Barrier`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asker {
    /// Core's idle query, of this round.
    Round(u64),
    /// A test's `Outpost::settle`, numbered so the shell finds who waits
    /// on it.
    Settle(u64),
}

/// One unit of work: what a program handles, or what the reducer does
/// itself.
#[derive(Clone, Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry moves by value from the mailbox to its program, as it always has"
)]
pub enum Item {
    /// An MSAA `WinEvent` from this outpost's own hooks, with the thread
    /// that raised it, which the batch limits count it by.
    Msaa {
        /// The event's kind.
        kind: WinKind,
        /// The event's window.
        hwnd: isize,
        /// The event's object id.
        id_object: i32,
        /// The event's child id.
        id_child: i32,
        /// The thread that raised it.
        raiser: u32,
    },
    /// A UIA event from this outpost's own subscriptions.
    Uia(UiaEvent),
    /// A query from Core.
    Query {
        /// The query's id, which its reply answers.
        request_id: RequestId,
        /// The query.
        query: Query,
    },
    /// The nodes and text anchors Core still holds, and the position of
    /// the last message it has handled: release the rest.
    NodesHeld {
        /// The nodes Core still holds, by number.
        nodes: Vec<u64>,
        /// The text anchors Core still holds, by number.
        anchors: Vec<u64>,
        /// The position of the last message Core has handled.
        acknowledged: u64,
    },
    /// The idle barrier (`docs/design/communication.md`, section 6.7):
    /// it ends its batch, and is reached once everything that arrived
    /// before it has been handled. Core's round is answered then once no
    /// deferred work is pending, a test's settle at once.
    Barrier {
        /// Who asked for it.
        asker: Asker,
    },
}

impl Item {
    /// The keys of the objects parked for this item.
    #[must_use]
    pub fn parked(&self) -> Vec<u64> {
        match self {
            Item::Uia(event) => event.parked().collect(),
            _ => Vec::new(),
        }
    }
}

/// An item with its trace and observation time, in microseconds on the
/// run's shared clock.
#[derive(Clone, Debug)]
pub struct Entry {
    /// What is to be done.
    pub item: Item,
    /// The trace minted for it.
    pub trace: TraceId,
    /// When it was observed, in microseconds on the run's shared clock.
    pub observed_at_us: u64,
    /// When it was raised, observed, and relayed, for the latency log.
    pub timing: EventTiming,
}

impl Entry {
    /// An entry the reducer makes itself, with a new trace.
    #[must_use]
    pub fn own(item: Item, trace: TraceId, observed_at_us: u64) -> Self {
        Self {
            item,
            trace,
            observed_at_us,
            timing: EventTiming::default(),
        }
    }
}
