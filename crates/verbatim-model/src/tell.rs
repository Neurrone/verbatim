//! The tell: the one message a reader sends Core when an event arrives,
//! in one shape whatever its interface (ADR 0003, "Readers tell, Core
//! asks"). It carries what came with the event, reads of Windows' own
//! state that cannot hang, and, for a UI Automation event, the results of
//! the calls NVDA's handler for that event makes at delivery, made by the
//! listener's check thread before the tell is sent. Core judges every rule
//! from it; the reader judged nothing but its local drops.

use serde::{Deserialize, Serialize};

use crate::event_kind::EventKind;
use crate::property::{PropertyId, PropertyValue, PropertyValues, RuntimeId};
use crate::request::Probe;
use crate::{NotificationKind, NotificationProcessing, Pid, WindowHandle};

/// A UI Automation element a reader keeps for a tell, behind the token
/// Core names it by, until Core asks about it or it is let go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ElementToken(pub u64);

/// The object an event names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventObject {
    /// A UI Automation element the reader kept, and its cached runtime id.
    Uia {
        /// The token the reader keeps the element behind.
        token: ElementToken,
        /// The element's runtime id, from the event's cache.
        runtime_id: RuntimeId,
    },
    /// An MSAA object, by the ids the `WinEvent` named.
    Msaa {
        /// The window.
        window: WindowHandle,
        /// The object id.
        object: i32,
        /// The child id.
        child: i32,
    },
}

/// A window as the acceptance test reads it: its handle, class and
/// process.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowFacet {
    /// The window.
    pub handle: WindowHandle,
    /// Its class name.
    pub class: String,
    /// The process that serves it: its owner, or for a console window the
    /// console host.
    pub process: Pid,
    /// The thread that owns it, which keys its backend verdict with it.
    pub thread: u32,
}

/// How a window stands to another, by `IsChild`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Relation {
    /// The same window.
    Equal,
    /// A descendant of it.
    Under,
    /// Neither.
    Outside,
}

/// The facts about the window a tell is judged by, which the acceptance
/// test reads, gathered by the listener with calls that cannot hang as it
/// places the tell (`docs/parity/focus-events.md`, "Event acceptance").
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one read, each a yes or no on the wire"
)]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceFacts {
    /// The window itself.
    pub window: WindowFacet,
    /// Its parent's class name, empty when it has no parent.
    pub parent_class: String,
    /// Its top-level window.
    pub root: WindowFacet,
    /// The top of its owner chain.
    pub root_owner: WindowFacet,
    /// Whether the window is topmost.
    pub window_topmost: bool,
    /// Whether its top-level window is topmost.
    pub root_topmost: bool,
    /// The system's foreground window, live.
    pub foreground: WindowHandle,
    /// The top of the foreground window's owner chain.
    pub foreground_root_owner: WindowHandle,
    /// How the window stands to the foreground window.
    pub window_to_foreground: Relation,
    /// How its root owner stands to the foreground window.
    pub root_owner_to_foreground: Relation,
    /// Whether its root owner is the foreground's root owner.
    pub root_owner_is_foregrounds: bool,
    /// The foreground thread's active window.
    pub active_window: WindowHandle,
    /// How the window stands to the foreground thread's active window.
    pub window_to_active: Relation,
    /// Whether the window is the desktop window.
    pub desktop: bool,
    /// Whether the window is Verbatim's hidden frame.
    pub hidden_frame: bool,
    /// Whether the window is owned by Verbatim's hidden frame through its
    /// owner chain, as Verbatim's own menu is, so that a focus in it takes
    /// the frame as its foreground: read by the listener from Windows with
    /// no call into the application. False for the frame itself.
    pub hidden_frame_owned: bool,
}

/// The window a tell is judged by, parsed at the boundary: found with its
/// facts, or why there is none. For a UI Automation event it is the
/// nearest window the check thread's walk found; for a `WinEvent`, the
/// event's window, which the listener makes the desktop window when the
/// event names none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TellWindow {
    /// Found, with its facts.
    Found(Box<AcceptanceFacts>),
    /// The walk found no window.
    NoWindow,
    /// The walk failed.
    Failed,
}

/// A read whose answer can fail, as a check thread's call can.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Read<T> {
    /// The call answered.
    Answered(T),
    /// The call failed.
    Failed,
}

/// What the listener's check thread found at a UI Automation event's
/// delivery, making the calls NVDA's handler for that event makes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    /// The provider probe of the nearest window, when the check probed it.
    pub probe: Option<Probe>,
    /// Whether the element has the keyboard focus, when the event's
    /// handler reads it.
    pub keyboard_focus: Option<Read<bool>>,
    /// For each element the listener keeps with the event's runtime id,
    /// whether it still has the keyboard focus.
    pub kept: Vec<(ElementToken, Read<bool>)>,
    /// Whether the element has a previous sibling, when the event's
    /// handler reads it.
    pub previous_sibling: Option<Read<bool>>,
}

/// What a kind of event brings beyond its object and cached properties.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    /// Nothing more.
    None,
    /// A property changed, to the value it carries.
    PropertyChanged {
        /// The property.
        property: PropertyId,
        /// Its new value, `None` when it has none.
        value: Option<PropertyValue>,
    },
    /// A UI Automation notification.
    Notification {
        /// Its text.
        text: String,
        /// Its kind.
        kind: NotificationKind,
        /// How it asks to be processed.
        processing: NotificationProcessing,
        /// Its activity id.
        activity: String,
    },
    /// The input desktop changed.
    DesktopSwitch {
        /// Whether the new input desktop can be opened: the user's, not a
        /// secure one.
        input_desktop_open: bool,
    },
}

/// A tell: an event a reader heard, in one shape for both readers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// The trace minted when the reader heard it.
    pub trace_id: crate::TraceId,
    /// What happened.
    pub kind: EventKind,
    /// The application: the process that serves the object, a console
    /// window's being its console host.
    pub process: Pid,
    /// The executable name of the process that sent the event, such as
    /// `explorer`, without its folder or extension: read by the listener
    /// once per process generation with `QueryFullProcessImageNameW`, a
    /// call to Windows and not into the application, and kept on its
    /// record of the application. Core's rules for an application's
    /// elements read it, as NVDA's application modules match by it.
    pub application: String,
    /// The thread that raised the event, by which the limiter counts it.
    pub thread: u32,
    /// When the reader's callback received it, in microseconds on the
    /// run's shared clock.
    pub at_us: u64,
    /// The object it names.
    pub object: EventObject,
    /// The properties its kind caches ([`crate::event_kind::EventKindEntry::cached`]).
    pub cached: PropertyValues,
    /// What its kind brings beyond them.
    pub payload: Payload,
    /// The window it is judged by.
    pub window: TellWindow,
    /// What the check thread found at delivery; `None` for an event that
    /// passes no check thread.
    pub checked: Option<Checked>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::property::PropertyAnswer;

    fn facet(handle: u64, class: &str) -> WindowFacet {
        WindowFacet {
            handle: WindowHandle(handle),
            class: class.to_owned(),
            process: Pid(40),
            thread: 41,
        }
    }

    #[test]
    fn a_uia_focus_tell_travels_as_json_and_back() {
        let tell = Event {
            trace_id: crate::TraceId::from_raw(9),
            kind: EventKind::UiaFocus,
            process: Pid(40),
            application: "explorer".to_owned(),
            thread: 41,
            at_us: 1_000,
            object: EventObject::Uia {
                token: ElementToken(3),
                runtime_id: RuntimeId(vec![42, 7]),
            },
            cached: PropertyValues(vec![PropertyAnswer {
                id: PropertyId::Name,
                value: Some(PropertyValue::Text("Save".to_owned())),
            }]),
            payload: Payload::None,
            window: TellWindow::Found(Box::new(AcceptanceFacts {
                window: facet(5, "Button"),
                parent_class: "#32770".to_owned(),
                root: facet(4, "#32770"),
                root_owner: facet(4, "#32770"),
                window_topmost: false,
                root_topmost: false,
                foreground: WindowHandle(4),
                foreground_root_owner: WindowHandle(4),
                window_to_foreground: Relation::Under,
                root_owner_to_foreground: Relation::Equal,
                root_owner_is_foregrounds: true,
                active_window: WindowHandle(4),
                window_to_active: Relation::Under,
                desktop: false,
                hidden_frame: false,
                hidden_frame_owned: false,
            })),
            checked: Some(Checked {
                probe: Some(Probe {
                    window: WindowHandle(5),
                    thread: 41,
                    provider: true,
                    began_us: 900,
                }),
                keyboard_focus: Some(Read::Answered(true)),
                kept: vec![(ElementToken(2), Read::Failed)],
                previous_sibling: None,
            }),
        };
        let json = serde_json::to_string(&tell).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), tell);
    }
}
