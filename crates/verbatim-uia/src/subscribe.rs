//! UIA event subscriptions: property changes, automation events (an element
//! selected, a menu opened), and notifications, over a [`Scope`] that can be
//! moved later.
//!
//! Each [`Registration`] owns its own thread, apartment, client, and
//! handlers, and registers all of its subscriptions as one event handler
//! group (`IUIAutomationEventHandlerGroup`): the handlers are added to the
//! group, which is local, and the group is registered on each element of
//! the scope in one `AddEventHandlerGroup` call, as NVDA registers its
//! handlers. Moving it ([`Registration::retarget`]) removes everything its
//! client registered and registers the group again on the new scope, on
//! that thread, so the caller never waits: removing a UIA handler waits for
//! its running callbacks to finish, which is why callbacks must never wait
//! on whoever moves the subscription. Dropping a registration unregisters
//! and ends its thread. Every handler is registered with the base cache
//! request, so the element arrives with its properties prefetched and the
//! callback reads them without a cross-process call.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationActiveTextPositionChangedEventHandler, IUIAutomationCacheRequest,
    IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandlerGroup,
    IUIAutomationNotificationEventHandler, IUIAutomationPropertyChangedEventHandler,
    IUIAutomationTextRange, NotificationKind, NotificationProcessing, TreeScope, TreeScope_Element,
    TreeScope_Subtree, UIA_EVENT_ID, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_IsEnabledPropertyId, UIA_NamePropertyId, UIA_PROPERTY_ID, UIA_RangeValueValuePropertyId,
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

/// Invoked on a UIA callback thread when a text control's active position
/// changed (`IUIAutomation6`'s active text position changed event, raised
/// when the application moves the place being read without moving the
/// caret, such as scrolling to an in-page link's target), with the cached
/// element and the range that is now active, when the event carries one.
pub type ActiveTextPositionCallback =
    Arc<dyn Fn(&IUIAutomationElement, Option<&IUIAutomationTextRange>) + Send + Sync>;

/// The properties an outpost follows on the focus, and only there: name,
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
    /// Active text position changes (`IUIAutomation6`).
    ActiveTextPosition {
        /// Called for each change.
        callback: ActiveTextPositionCallback,
    },
}

/// A live subscription: one or more [`Subscription`]s registered together
/// as one event handler group. Dropping it unregisters and ends its thread.
pub struct Registration {
    retarget: Option<mpsc::Sender<Scope>>,
    join: Option<JoinHandle<()>>,
}

impl Registration {
    /// Subscribes to every one of `subscriptions` over `scope`, as one event
    /// handler group, returning once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the client, cache request, or a handler
    /// cannot be created. An element of the scope that cannot be resolved,
    /// or on which the group cannot be registered, is skipped rather than
    /// failing the registration.
    pub fn new(subscriptions: Vec<Subscription>, scope: Scope) -> windows::core::Result<Self> {
        let (retarget_tx, retarget_rx) = mpsc::channel::<Scope>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-subscription".to_owned())
            .spawn(move || run(subscriptions, &scope, &ready_tx, &retarget_rx))
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
    Notifications(IUIAutomationNotificationEventHandler),
    ActiveTextPosition(IUIAutomationActiveTextPositionChangedEventHandler),
}

impl Handler {
    fn of(subscription: Subscription) -> Self {
        match subscription {
            Subscription::Properties {
                properties,
                callback,
            } => Self::Properties(handlers::PropertyHandler { callback }.into(), properties),
            Subscription::Event { event, callback } => Self::Event(
                handlers::EventHandler {
                    callback: Arc::new(move |element, _| callback(element)),
                }
                .into(),
                event,
            ),
            Subscription::Events { events, callback } => {
                Self::Events(handlers::EventHandler { callback }.into(), events)
            }
            Subscription::Notifications { callback } => {
                Self::Notifications(handlers::NotificationHandler { callback }.into())
            }
            Subscription::ActiveTextPosition { callback } => {
                Self::ActiveTextPosition(handlers::ActiveTextPositionHandler { callback }.into())
            }
        }
    }

    /// Adds this handler to `group`, listening over `tree_scope` with
    /// `cache`. Local: nothing is registered until the group is.
    fn add_to(
        &self,
        group: &IUIAutomationEventHandlerGroup,
        tree_scope: TreeScope,
        cache: &IUIAutomationCacheRequest,
    ) -> windows::core::Result<()> {
        match self {
            // SAFETY: the group, cache, and handler are live and owned by
            // this thread; the property slice outlives the call.
            Self::Properties(handler, properties) => unsafe {
                group.AddPropertyChangedEventHandler(tree_scope, cache, handler, properties)
            },
            // SAFETY: as above.
            Self::Event(handler, event) => unsafe {
                group.AddAutomationEventHandler(*event, tree_scope, cache, handler)
            },
            Self::Events(handler, events) => events.iter().try_for_each(|event| {
                // SAFETY: as above.
                unsafe { group.AddAutomationEventHandler(*event, tree_scope, cache, handler) }
            }),
            // SAFETY: as above.
            Self::Notifications(handler) => unsafe {
                group.AddNotificationEventHandler(tree_scope, cache, handler)
            },
            // SAFETY: as above.
            Self::ActiveTextPosition(handler) => unsafe {
                group.AddActiveTextPositionChangedEventHandler(tree_scope, cache, handler)
            },
        }
    }
}

/// What a registration's thread holds: its client, also as
/// `IUIAutomation6` for the event handler groups, the cache request, and the
/// handlers.
struct Parts {
    uia: Uia,
    client: IUIAutomation6,
    cache: IUIAutomationCacheRequest,
    handlers: Vec<Handler>,
}

fn run(
    subscriptions: Vec<Subscription>,
    scope: &Scope,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    retarget: &mpsc::Receiver<Scope>,
) {
    let setup = (|| -> windows::core::Result<Parts> {
        let uia = Uia::new()?;
        Ok(Parts {
            client: uia.client().cast()?,
            cache: uia.base_cache_request()?,
            handlers: subscriptions.into_iter().map(Handler::of).collect(),
            uia,
        })
    })();
    let parts = match setup {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    register(&parts, scope);
    let _ = ready.send(Ok(()));
    while let Ok(mut scope) = retarget.recv() {
        // Only the newest scope matters: skip any that queued up behind it.
        while let Ok(newer) = retarget.try_recv() {
            scope = newer;
        }
        // SAFETY: removing this thread's own client's registrations. One on
        // an element that has gone since is forgotten by UIA itself, as NVDA
        // notes.
        unsafe {
            let _ = parts.uia.client().RemoveAllEventHandlers();
        }
        register(&parts, &scope);
    }
    // SAFETY: as above, before teardown.
    unsafe {
        let _ = parts.uia.client().RemoveAllEventHandlers();
    }
}

/// Registers the handlers, as one group, on every element of `scope`.
fn register(parts: &Parts, scope: &Scope) {
    let Parts {
        uia,
        client,
        cache,
        handlers,
    } = parts;
    let (targets, tree_scope): (Vec<IUIAutomationElement>, TreeScope) = match scope {
        Scope::Nothing => (Vec::new(), TreeScope_Element),
        Scope::Windows(hwnds) => (
            hwnds
                .iter()
                .filter_map(|&hwnd| uia.element_from_handle(hwnd, cache).ok())
                .collect(),
            TreeScope_Subtree,
        ),
        Scope::Desktop => (
            // SAFETY: `cache` is a live cache request owned by this thread.
            unsafe { uia.client().GetRootElementBuildCache(cache) }
                .ok()
                .into_iter()
                .collect(),
            TreeScope_Subtree,
        ),
        Scope::Elements(elements) => (
            elements
                .iter()
                .filter_map(|agile| agile.resolve().ok())
                .collect(),
            TreeScope_Element,
        ),
    };
    if targets.is_empty() {
        return;
    }
    let group = (|| -> windows::core::Result<IUIAutomationEventHandlerGroup> {
        // SAFETY: a local call on this thread's live client.
        let group = unsafe { client.CreateEventHandlerGroup() }?;
        for handler in handlers {
            handler.add_to(&group, tree_scope, cache)?;
        }
        Ok(group)
    })();
    let group = match group {
        Ok(group) => group,
        Err(error) => {
            tracing::warn!(%error, "a UIA event handler group could not be built");
            return;
        }
    };
    for element in targets {
        // SAFETY: the element and group are live and owned by this thread's
        // client.
        if let Err(error) = unsafe { client.AddEventHandlerGroup(&element, &group) } {
            // As NVDA logs it and goes on: the element has most likely gone,
            // and nothing is registered on it.
            tracing::debug!(%error, "a UIA event handler group could not be registered");
        }
    }
}

/// The `#[implement]`-generated COM objects live in their own module so the
/// module-level allow covers the macro's generated glue without loosening the
/// lint elsewhere.
mod handlers {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationActiveTextPositionChangedEventHandler_Impl, IUIAutomationElement,
        IUIAutomationEventHandler_Impl, IUIAutomationNotificationEventHandler_Impl,
        IUIAutomationPropertyChangedEventHandler_Impl, IUIAutomationTextRange, NotificationKind,
        NotificationProcessing, UIA_EVENT_ID, UIA_PROPERTY_ID,
    };
    use windows_core::implement;

    use super::{
        ActiveTextPositionCallback, EventCallback, NotificationCallback, PropertyCallback,
    };

    /// The active text position handler.
    #[implement(
        windows::Win32::UI::Accessibility::IUIAutomationActiveTextPositionChangedEventHandler
    )]
    pub struct ActiveTextPositionHandler {
        pub callback: ActiveTextPositionCallback,
    }

    impl IUIAutomationActiveTextPositionChangedEventHandler_Impl for ActiveTextPositionHandler_Impl {
        fn HandleActiveTextPositionChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            range: windows_core::Ref<IUIAutomationTextRange>,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                crate::com::guarded("active text position", || {
                    (self.callback)(element, range.as_ref());
                });
            }
            Ok(())
        }
    }

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
