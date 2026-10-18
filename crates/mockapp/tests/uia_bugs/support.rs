//! Mechanics the fault tests share, not a fault of its own: starting a
//! mockapp, a UIA client's plain calls, and a handler object that hears
//! UIA's events, made with UIA's own interfaces rather than
//! `verbatim_uia`'s registrations, because what these tests pin is UIA's
//! behaviour.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomation2, IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationEventHandler, IUIAutomationEventHandlerGroup,
    IUIAutomationFocusChangedEventHandler, IUIAutomationPropertyChangedEventHandler,
    TreeScope_Subtree, UIA_AutomationFocusChangedEventId, UIA_NamePropertyId,
};
use windows::core::{AgileReference, Interface};

use crate::common;

/// A mockapp with the `small.json` fixture on `backend`, and its window.
pub fn mockapp(prefix: &str, backend: &str) -> (common::MockApp, HWND) {
    let title = common::unique_title(prefix);
    let app = common::spawn("small.json", backend, &title);
    let hwnd = common::find_window(&title);
    (app, hwnd)
}

/// A UIA client on this thread, made by `verbatim_uia`, so UIA's
/// first-time setup in this process is finished first.
pub fn client() -> Uia {
    Uia::new().expect("a UIA client")
}

/// The element of the window `hwnd`, by a new connection to it.
pub fn element(client: &IUIAutomation, hwnd: HWND) -> IUIAutomationElement {
    // SAFETY: a live client; a window handle, which UIA checks.
    unsafe { client.ElementFromHandle(hwnd) }.expect("the window's element")
}

/// A cache request with nothing added to it, for a registration: the
/// handlers here read their senders live, since a cached read is made
/// only through `verbatim_uia`'s shapes, which a raw handler's sender is
/// not.
pub fn cache(client: &IUIAutomation) -> IUIAutomationCacheRequest {
    // SAFETY: a local call on a live client.
    unsafe { client.CreateCacheRequest() }.expect("a cache request")
}

/// Sets the process's connection timeout, which every client in the
/// process shares (`docs/uia-bugs.md`, "The connection timeout applies to
/// the whole process"), through `client`; each test runs in a process of
/// its own.
pub fn set_connection_timeout(client: &IUIAutomation, timeout: Duration) {
    let client: IUIAutomation2 = client.cast().expect("an IUIAutomation2 client");
    let milliseconds = u32::try_from(timeout.as_millis()).expect("the timeout fits");
    // SAFETY: a live client; a plain integer.
    unsafe { client.SetConnectionTimeout(milliseconds) }.expect("the connection timeout set");
}

/// A registration of `handler` for the name changes in `window`'s subtree.
pub fn names_on(client: &IUIAutomation, window: &IUIAutomationElement, handler: &Handler) {
    // SAFETY: a live client, element, cache request and handler.
    unsafe {
        client.AddPropertyChangedEventHandlerNativeArray(
            window,
            TreeScope_Subtree,
            &cache(client),
            &handler.properties,
            &[UIA_NamePropertyId],
        )
    }
    .expect("the name changes registered");
}

/// A registration of `handler` for UIA's focus event, and for name changes
/// when `names` is set, in an event handler group on `window`'s subtree.
pub fn group_on(
    client: &IUIAutomation,
    window: &IUIAutomationElement,
    handler: &Handler,
    names: bool,
) -> IUIAutomationEventHandlerGroup {
    let cache = cache(client);
    let client: IUIAutomation6 = client.cast().expect("an IUIAutomation6 client");
    // SAFETY: a local call on a live client.
    let group = unsafe { client.CreateEventHandlerGroup() }.expect("an event handler group");
    // SAFETY: a live group, cache request and handler.
    unsafe {
        group.AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            TreeScope_Subtree,
            &cache,
            &handler.events,
        )
    }
    .expect("the focus event in the group");
    if names {
        // SAFETY: a live group, cache request and handler.
        unsafe {
            group.AddPropertyChangedEventHandler(
                TreeScope_Subtree,
                &cache,
                &handler.properties,
                &[UIA_NamePropertyId],
            )
        }
        .expect("name changes in the group");
    }
    // SAFETY: a live client, element and group.
    unsafe { client.AddEventHandlerGroup(window, &group) }
        .expect("the group registered on the window");
    group
}

/// An event as a handler heard it: its sender's process id and name, read
/// live as the event arrived.
#[derive(Debug, PartialEq, Eq)]
pub struct Heard {
    pub pid: u32,
    pub name: String,
}

/// An event from `app`'s element named `name`.
pub fn from(app: &common::MockApp, name: &str) -> Heard {
    Heard {
        pid: app.pid(),
        name: name.to_owned(),
    }
}

/// What a handler heard: the event, when, and its sender.
pub type Event = (Heard, Instant, AgileReference<IUIAutomationElement>);

/// One handler object as each of the three handler interfaces, and what it
/// hears, with when it heard it and the sender.
pub struct Handler {
    pub events: IUIAutomationEventHandler,
    pub focus: IUIAutomationFocusChangedEventHandler,
    pub properties: IUIAutomationPropertyChangedEventHandler,
    heard: Receiver<Event>,
}

impl Handler {
    pub fn new() -> Self {
        let (sender, heard) = mpsc::channel();
        let events = hearing::new(sender);
        Self {
            focus: events.cast().expect("the focus handler interface"),
            properties: events.cast().expect("the property handler interface"),
            events,
            heard,
        }
    }

    /// The next event heard, and when, waiting at most the shared wait.
    pub fn next(&self) -> (Heard, Instant) {
        let (heard, at, _) = self.next_with_sender();
        (heard, at)
    }

    /// The next event heard, when, and its sender, kept as the handler
    /// received it, waiting at most the shared wait.
    pub fn next_with_sender(&self) -> Event {
        self.heard
            .recv_timeout(common::WAIT_TIMEOUT)
            .unwrap_or_else(|error| {
                panic!(
                    "no event was heard within {:?}: {error}",
                    common::WAIT_TIMEOUT
                )
            })
    }

    /// Asserts that nothing more has been heard.
    pub fn heard_nothing_more(&self) {
        match self.heard.try_recv() {
            Err(TryRecvError::Empty) => {}
            Ok((heard, ..)) => panic!("an event from {heard:?} was heard as well"),
            Err(TryRecvError::Disconnected) => panic!("the handler is gone"),
        }
    }
}

/// The handler object.
mod hearing {
    // The `implement` macro's expansion.
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::Mutex;
    use std::sync::mpsc::Sender;
    use std::time::Instant;

    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
        IUIAutomationPropertyChangedEventHandler, IUIAutomationPropertyChangedEventHandler_Impl,
        UIA_EVENT_ID, UIA_PROPERTY_ID,
    };

    use windows::core::AgileReference;

    use super::{Event, Heard};

    /// A handler that sends what it hears to `sender`.
    pub(super) fn new(sender: Sender<Event>) -> IUIAutomationEventHandler {
        Hearing {
            sender: Mutex::new(sender),
        }
        .into()
    }

    #[windows_core::implement(
        IUIAutomationEventHandler,
        IUIAutomationFocusChangedEventHandler,
        IUIAutomationPropertyChangedEventHandler
    )]
    struct Hearing {
        sender: Mutex<Sender<Event>>,
    }

    impl Hearing_Impl {
        fn hear(&self, sender: &windows_core::Ref<IUIAutomationElement>) {
            let at = Instant::now();
            let element = sender.as_ref().expect("an event's sender");
            // SAFETY: a live read of the sender, which the handler holds.
            let pid = unsafe { element.CurrentProcessId() }.expect("the sender's process id");
            // SAFETY: as above.
            let name = unsafe { element.CurrentName() }.expect("the sender's name");
            let heard = Heard {
                pid: u32::try_from(pid).expect("a process id"),
                name: name.to_string(),
            };
            let kept = AgileReference::new(element).expect("the sender kept");
            let _ = self
                .sender
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send((heard, at, kept));
        }
    }

    impl IUIAutomationEventHandler_Impl for Hearing_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            _event: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            self.hear(&sender);
            Ok(())
        }
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for Hearing_Impl {
        fn HandleFocusChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            self.hear(&sender);
            Ok(())
        }
    }

    impl IUIAutomationPropertyChangedEventHandler_Impl for Hearing_Impl {
        fn HandlePropertyChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            _property: UIA_PROPERTY_ID,
            _value: &VARIANT,
        ) -> windows_core::Result<()> {
            self.hear(&sender);
            Ok(())
        }
    }
}
