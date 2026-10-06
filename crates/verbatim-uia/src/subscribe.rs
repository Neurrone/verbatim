//! UIA event subscriptions: property changes, automation events (an element
//! selected, a menu opened), and notifications, each over a [`Scope`] that
//! can be moved later.
//!
//! Each [`Registration`] owns its own thread, apartment, client, and handler.
//! Moving it ([`Registration::retarget`]) removes everything its client
//! registered and registers again on the new scope, on that thread, so the
//! caller never waits: removing a UIA handler waits for its running
//! callbacks to finish, which is why callbacks must never wait on whoever
//! moves the subscription. Dropping a registration unregisters and ends its
//! thread. Every handler is registered with the base cache request, so the
//! element arrives with its properties prefetched and the callback reads
//! them without a cross-process call.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomation5, IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationEventHandler,
    IUIAutomationNotificationEventHandler, IUIAutomationPropertyChangedEventHandler,
    NotificationKind, NotificationProcessing, TreeScope, TreeScope_Element, TreeScope_Subtree,
    UIA_EVENT_ID, UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_IsEnabledPropertyId,
    UIA_NamePropertyId, UIA_PROPERTY_ID, UIA_RangeValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;
use windows_core::Interface;

use crate::client::Uia;

/// Invoked on a UIA callback thread for a property change, with the cached
/// element and the changed property id.
pub type PropertyCallback = Arc<dyn Fn(&IUIAutomationElement, i32) + Send + Sync>;

/// Invoked on a UIA callback thread for an automation event, with the cached
/// element that raised it.
pub type ElementCallback = Arc<dyn Fn(&IUIAutomationElement) + Send + Sync>;

/// Invoked on a UIA callback thread for one of several automation events,
/// with the cached element that raised it and the event's id.
pub type EventCallback = Arc<dyn Fn(&IUIAutomationElement, i32) + Send + Sync>;

/// Invoked on a UIA callback thread for a notification: the raising element,
/// the kind and processing hint, and the optional display string and
/// activity id.
pub type NotificationCallback = Arc<
    dyn Fn(
            &IUIAutomationElement,
            NotificationKind,
            NotificationProcessing,
            Option<String>,
            Option<String>,
        ) + Send
        + Sync,
>;

/// The properties an outpost follows on the focus and its ancestors: name,
/// value (of the `Value` or the `RangeValue` pattern, as NVDA follows both),
/// and the state-bearing toggle, enabled, and expand/collapse properties,
/// all in the base cache request, so the state set is rebuilt from the
/// cache.
pub const FOCUS_PROPERTIES: [UIA_PROPERTY_ID; 6] = [
    UIA_NamePropertyId,
    UIA_ValueValuePropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
];

/// Where a registration listens.
pub enum Scope {
    /// Nowhere, until retargeted.
    Nothing,
    /// The subtree of each of these top-level windows.
    Windows(Vec<isize>),
    /// The whole desktop: the subtree of the root element.
    Desktop,
    /// Exactly these elements, not their descendants.
    Elements(Vec<AgileReference<IUIAutomationElement>>),
}

/// What a registration listens for.
pub enum Subscription {
    /// Changes to these properties.
    Properties {
        /// The properties to watch.
        properties: Vec<UIA_PROPERTY_ID>,
        /// Called for each change.
        callback: PropertyCallback,
    },
    /// An automation event, such as `SelectionItem_ElementSelected` or
    /// `MenuOpened`.
    Event {
        /// The event to watch.
        event: UIA_EVENT_ID,
        /// Called for each event.
        callback: ElementCallback,
    },
    /// `AutomationNotification` events (`IUIAutomation5`).
    Notifications {
        /// Called for each notification.
        callback: NotificationCallback,
    },
    /// Several automation events through one handler, such as a text
    /// control's caret and text changes.
    Events {
        /// The events to watch.
        events: Vec<UIA_EVENT_ID>,
        /// Called for each event, with its id.
        callback: EventCallback,
    },
}

/// A live subscription. Dropping it unregisters and ends its thread.
pub struct Registration {
    retarget: Option<mpsc::Sender<Scope>>,
    join: Option<JoinHandle<()>>,
}

impl Registration {
    /// Subscribes to `subscription` over `scope`, returning once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the client, cache request, or handler cannot
    /// be created. An element of the scope that cannot be resolved is skipped
    /// rather than failing the registration.
    pub fn new(subscription: Subscription, scope: Scope) -> windows::core::Result<Self> {
        let (retarget_tx, retarget_rx) = mpsc::channel::<Scope>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-subscription".to_owned())
            .spawn(move || run(subscription, &scope, &ready_tx, &retarget_rx))
            .map_err(|e| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                retarget: Some(retarget_tx),
                join: Some(join),
            }),
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "subscription thread ended before signalling readiness",
                ))
            }
        }
    }

    /// Moves the subscription to `scope`, without waiting: the registration's
    /// own thread removes the old handlers and registers the new ones.
    pub fn retarget(&self, scope: Scope) {
        if let Some(sender) = &self.retarget {
            let _ = sender.send(scope);
        }
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        drop(self.retarget.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The handler object for a subscription.
enum Handler {
    Properties(
        IUIAutomationPropertyChangedEventHandler,
        Vec<UIA_PROPERTY_ID>,
    ),
    Event(IUIAutomationEventHandler, UIA_EVENT_ID),
    Events(IUIAutomationEventHandler, Vec<UIA_EVENT_ID>),
    Notifications(IUIAutomationNotificationEventHandler, IUIAutomation5),
}

fn run(
    subscription: Subscription,
    scope: &Scope,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    retarget: &mpsc::Receiver<Scope>,
) {
    let setup = (|| -> windows::core::Result<(Uia, IUIAutomationCacheRequest, Handler)> {
        let uia = Uia::new()?;
        let cache = uia.base_cache_request()?;
        let handler = match subscription {
            Subscription::Properties {
                properties,
                callback,
            } => Handler::Properties(handlers::PropertyHandler { callback }.into(), properties),
            Subscription::Event { event, callback } => Handler::Event(
                handlers::EventHandler {
                    callback: Arc::new(move |element, _| callback(element)),
                }
                .into(),
                event,
            ),
            Subscription::Events { events, callback } => {
                Handler::Events(handlers::EventHandler { callback }.into(), events)
            }
            Subscription::Notifications { callback } => Handler::Notifications(
                handlers::NotificationHandler { callback }.into(),
                uia.client().cast()?,
            ),
        };
        Ok((uia, cache, handler))
    })();
    let (uia, cache, handler) = match setup {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    register(&uia, &cache, &handler, scope);
    let _ = ready.send(Ok(()));
    while let Ok(mut scope) = retarget.recv() {
        // Only the newest scope matters: skip any that queued up behind it.
        while let Ok(newer) = retarget.try_recv() {
            scope = newer;
        }
        // SAFETY: removing this thread's own client's registrations.
        unsafe {
            let _ = uia.client().RemoveAllEventHandlers();
        }
        register(&uia, &cache, &handler, &scope);
    }
    // SAFETY: as above, before teardown.
    unsafe {
        let _ = uia.client().RemoveAllEventHandlers();
    }
}

/// Registers `handler` on every element of `scope`.
fn register(uia: &Uia, cache: &IUIAutomationCacheRequest, handler: &Handler, scope: &Scope) {
    let targets: Vec<(IUIAutomationElement, TreeScope)> = match scope {
        Scope::Nothing => Vec::new(),
        Scope::Windows(hwnds) => hwnds
            .iter()
            .filter_map(|&hwnd| uia.element_from_handle(hwnd, cache).ok())
            .map(|element| (element, TreeScope_Subtree))
            .collect(),
        // SAFETY: `cache` is a live cache request owned by this thread.
        Scope::Desktop => unsafe { uia.client().GetRootElementBuildCache(cache) }
            .ok()
            .map(|root| (root, TreeScope_Subtree))
            .into_iter()
            .collect(),
        Scope::Elements(elements) => elements
            .iter()
            .filter_map(|agile| agile.resolve().ok())
            .map(|element| (element, TreeScope_Element))
            .collect(),
    };
    for (element, tree_scope) in targets {
        // Each call takes the element, cache, and handler, live and owned by
        // this thread's client.
        let _ = match handler {
            // SAFETY: as above; the property slice outlives the call.
            Handler::Properties(handler, properties) => unsafe {
                uia.client().AddPropertyChangedEventHandlerNativeArray(
                    &element, tree_scope, cache, handler, properties,
                )
            },
            // SAFETY: as above.
            Handler::Event(handler, event) => unsafe {
                uia.client()
                    .AddAutomationEventHandler(*event, &element, tree_scope, cache, handler)
            },
            Handler::Events(handler, events) => events.iter().try_for_each(|event| {
                // SAFETY: as above.
                unsafe {
                    uia.client()
                        .AddAutomationEventHandler(*event, &element, tree_scope, cache, handler)
                }
            }),
            // SAFETY: as above.
            Handler::Notifications(handler, client5) => unsafe {
                client5.AddNotificationEventHandler(&element, tree_scope, cache, handler)
            },
        };
    }
}

/// The `#[implement]`-generated COM objects live in their own module so the
/// module-level allow covers the macro's generated glue without loosening the
/// lint elsewhere.
mod handlers {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler_Impl,
        IUIAutomationNotificationEventHandler_Impl, IUIAutomationPropertyChangedEventHandler_Impl,
        NotificationKind, NotificationProcessing, UIA_EVENT_ID, UIA_PROPERTY_ID,
    };
    use windows_core::implement;

    use super::{EventCallback, NotificationCallback, PropertyCallback};

    /// The property-change handler.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationPropertyChangedEventHandler)]
    pub struct PropertyHandler {
        pub callback: PropertyCallback,
    }

    impl IUIAutomationPropertyChangedEventHandler_Impl for PropertyHandler_Impl {
        fn HandlePropertyChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            propertyid: UIA_PROPERTY_ID,
            _newvalue: &VARIANT,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                crate::com::guarded("property", || (self.callback)(element, propertyid.0));
            }
            Ok(())
        }
    }

    /// The automation-event handler.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationEventHandler)]
    pub struct EventHandler {
        pub callback: EventCallback,
    }

    impl IUIAutomationEventHandler_Impl for EventHandler_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            eventid: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                crate::com::guarded("event", || (self.callback)(element, eventid.0));
            }
            Ok(())
        }
    }

    /// The notification handler.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationNotificationEventHandler)]
    pub struct NotificationHandler {
        pub callback: NotificationCallback,
    }

    impl IUIAutomationNotificationEventHandler_Impl for NotificationHandler_Impl {
        fn HandleNotificationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            notificationkind: NotificationKind,
            notificationprocessing: NotificationProcessing,
            displaystring: &windows_core::BSTR,
            activityid: &windows_core::BSTR,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                crate::com::guarded("notification", || {
                    let display = (!displaystring.is_empty()).then(|| displaystring.to_string());
                    let activity = (!activityid.is_empty()).then(|| activityid.to_string());
                    (self.callback)(
                        element,
                        notificationkind,
                        notificationprocessing,
                        display,
                        activity,
                    );
                });
            }
            Ok(())
        }
    }
}
