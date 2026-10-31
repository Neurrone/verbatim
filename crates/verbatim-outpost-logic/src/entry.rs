//! What the mailbox carries to the reducer: an entry, with no application
//! object in it. A UIA event's element and an active text position's range
//! are parked in the store as the entry is pushed, and the entry carries
//! the key they are parked under (`docs/design/ownership.md`).

use verbatim_model::protocol::{DeliveredFact, EventTiming, Query, UiaSnapshotFact};
use verbatim_model::{NodeId, Notification, TraceId};

/// The kind of an MSAA `WinEvent`, as the outpost's hook normalized it: the
/// reducer's own copy of `verbatim_ia2::WinEventKind`, variant for variant,
/// since the reducer names no backend crate. The mailbox converts one into
/// the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WinKind {
    /// `EVENT_OBJECT_FOCUS`.
    Focus,
    /// `EVENT_SYSTEM_FOREGROUND`: a window came to the foreground.
    Foreground,
    /// `EVENT_OBJECT_VALUECHANGE`.
    ValueChange,
    /// `EVENT_OBJECT_STATECHANGE`, and the selection events NVDA handles as one.
    StateChange,
    /// `EVENT_OBJECT_NAMECHANGE`.
    NameChange,
    /// `EVENT_OBJECT_DESCRIPTIONCHANGE`.
    DescriptionChange,
    /// `EVENT_OBJECT_SELECTION`: an item became the selection.
    Selection,
    /// `EVENT_SYSTEM_MENUPOPUPSTART`: a popup menu opened.
    MenuPopupStart,
    /// A popup menu closed, or menu mode ended.
    MenuEnd,
    /// The Alt+Tab switcher closed.
    SwitchEnd,
    /// `EVENT_OBJECT_DESTROY`: an object, possibly a window, was destroyed.
    Destroy,
    /// `EVENT_SYSTEM_ALERT`: an alert, as toast notifications arrive.
    Alert,
    /// The system caret moved, or was shown again after a move.
    Caret,
    /// `EVENT_OBJECT_TEXTSELECTIONCHANGED`: a text control's selection or caret changed.
    TextSelectionChange,
    /// `EVENT_OBJECT_SHOW` from a tooltip window.
    Show,
    /// The console host's text changed, scrolled, or was laid out anew.
    ConsoleUpdate,
    /// A foreground event on Program Manager's or the taskbar's window.
    ShellForeground,
    /// The console host's caret moved.
    ConsoleCaret,
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
    Property(i32),
    /// An element was selected within its container.
    Selection,
    /// An application-initiated notification.
    Notification(Notification),
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

/// One unit of work: what a program handles, or what the reducer does
/// itself.
#[derive(Clone, Debug)]
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
    /// A fact routed from the listener, or an MSAA focus, foreground
    /// change, menu opening, alert, or tooltip the outpost's own hook took
    /// in as the same fact, with the thread that raised an MSAA event and
    /// when (`None` for a UIA fact).
    Fact(
        DeliveredFact,
        Option<verbatim_model::protocol::WinEventStamp>,
    ),
    /// A query from Core.
    Query {
        /// The query's id, which its reply answers.
        request_id: u64,
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
    /// A caret event of the focused terminal `node_id`, noted while its
    /// reading was held and its text had changed unread: its caret is read
    /// once nothing else is waiting, unless a read since saw it, so Core's
    /// copy of the caret catches up when a burst of output ends rather
    /// than at the next read Core asks for.
    NotedCaret {
        /// The terminal's node.
        node_id: NodeId,
    },
    /// `Outpost::settle`, numbered so the shell finds who waits on it:
    /// answered once nothing else is waiting, the focus-following
    /// subscriptions have made every move asked of them, and every message
    /// published has been written.
    Settle(u64),
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): reached once
    /// everything that arrived before it has been handled, and answered
    /// once a noted caret is read and no deferred work is pending.
    /// `behind_noted` once the reducer has put it directly behind the
    /// noted caret, so that reaching it again waits for nothing more.
    IdleQuery {
        /// The round.
        round: u64,
        /// Whether the reducer has put it directly behind the noted caret.
        behind_noted: bool,
    },
    /// The open caret key's watch's wake: the next read of its console
    /// poll, for a watch that polls, and its bound otherwise.
    Wake,
    /// The gap record's bound has passed: the record, still open, is read.
    GapBound,
    /// A drop for no window above its sender answered the open gap record,
    /// whose read takes the drop's marker's place, stamped with the
    /// record's stamp, the entry's observation; `window` is the record's
    /// window.
    GapRead {
        /// The record's window.
        window: isize,
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

/// Where an entry came from, which the hand-over's hold judges it by
/// ([`super::hold`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The outpost's own `WinEvent` hook.
    Hook,
    /// The focus listener, forwarding a copy of a `WinEvent` the outpost's
    /// hook may also take.
    Listener,
    /// Anything else: the outpost's own UIA subscriptions, the listener's
    /// other facts, Core's commands and the outpost's own entries.
    Other,
}
