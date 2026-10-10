//! UIA event subscriptions: property changes, automation events (an element
//! selected, a menu opened), notifications, and active text position
//! changes, over a [`Scope`] that can be moved later.
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
//! on whoever moves the subscription. [`Registration::settle`] waits for
//! every move asked for so far to be made, for a caller that measures what
//! the moves cost the application. Dropping a registration unregisters
//! and ends its thread. Every handler is registered with one cache request,
//! the base one unless the registration names its properties
//! ([`Registration::with_cache`]), so the element arrives with its
//! properties prefetched and the callback reads them without a
//! cross-process call.
//!
//! A registration's thread calls into applications as it moves, and a
//! caller can wait on it ([`Registration::new`] for its first registration,
//! [`Registration::settle`]), so it has a watchdog, the outpost worker's
//! rule: a move that has not finished within [`MOVE_DEADLINE`] when the
//! registration is next moved or settled has its thread abandoned and
//! replaced by a new one, with its own client, that registers on the
//! newest scope. The abandoned thread's handlers call back no more, and
//! once its call returns it removes what its client registered and ends.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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

use crate::cache::CACHED_PROPERTIES;
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

/// How long a registration's thread may take over one move (removing its
/// handlers and registering them on a new scope) before it is abandoned and
/// replaced: two of UIA's call timeouts ([`crate::CALL_TIMEOUT`]), so a move
/// whose calls each answer within the timeout is never abandoned.
pub const MOVE_DEADLINE: Duration = Duration::from_secs(10);

/// Where a registration listens.
#[derive(Clone)]
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
#[derive(Clone)]
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
    /// What each of its threads registers, for a replacement.
    subscriptions: Vec<Subscription>,
    /// The properties each event's element arrives with.
    properties: &'static [UIA_PROPERTY_ID],
    state: Mutex<State>,
}

/// A registration's threads and the scope it was last asked to move to.
struct State {
    /// The thread in charge, until [`Registration::close`] takes it.
    current: Option<Incarnation>,
    /// Threads abandoned past [`MOVE_DEADLINE`] and not yet waited for.
    abandoned: Vec<JoinHandle<()>>,
    /// The newest scope asked for, where a replacement registers.
    scope: Scope,
}

/// One of a registration's threads.
struct Incarnation {
    commands: mpsc::Sender<Command>,
    join: JoinHandle<()>,
    /// Cleared when the thread is abandoned: its handlers call back no
    /// more, and it ends once its call returns.
    live: Arc<AtomicBool>,
    /// When the move the thread is making began, while it makes one.
    moving: Arc<Mutex<Option<Instant>>>,
}

/// What [`Incarnation::start`] gives: the thread, and the way to hear that
/// its first registration is made.
type Started = (Incarnation, mpsc::Receiver<windows::core::Result<()>>);

impl Incarnation {
    /// Starts a thread that registers `subscriptions` on `scope`.
    fn start(
        subscriptions: Vec<Subscription>,
        properties: &'static [UIA_PROPERTY_ID],
        scope: Scope,
    ) -> windows::core::Result<Started> {
        let (commands, received) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let live = Arc::new(AtomicBool::new(true));
        let moving = Arc::new(Mutex::new(Some(Instant::now())));
        let thread = Thread {
            live: Arc::clone(&live),
            moving: Arc::clone(&moving),
        };
        let join = thread::Builder::new()
            .name("verbatim-uia-subscription".to_owned())
            .spawn(move || {
                run(
                    subscriptions,
                    properties,
                    &scope,
                    &thread,
                    &ready_tx,
                    &received,
                );
            })
            .map_err(|e| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
            })?;
        Ok((
            Self {
                commands,
                join,
                live,
                moving,
            },
            ready_rx,
        ))
    }

    /// Whether the move the thread is making began [`MOVE_DEADLINE`] or
    /// more ago.
    fn overdue(&self) -> bool {
        self.moving
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some_and(|began| began.elapsed() >= MOVE_DEADLINE)
    }

    /// Abandons the thread: its handlers call back no more, it is asked for
    /// nothing more, and it ends once its call returns.
    fn abandon(self) -> JoinHandle<()> {
        self.live.store(false, Ordering::Release);
        drop(self.commands);
        self.join
    }
}

impl Registration {
    /// Subscribes to every one of `subscriptions` over `scope`, as one event
    /// handler group, returning once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the client, cache request, or a handler
    /// cannot be created, or `UIA_E_TIMEOUT` if the first registration is
    /// not made within [`MOVE_DEADLINE`], when its thread is abandoned. An
    /// element of the scope that cannot be resolved, or on which the group
    /// cannot be registered, is skipped rather than failing the
    /// registration.
    pub fn new(subscriptions: Vec<Subscription>, scope: Scope) -> windows::core::Result<Self> {
        Self::with_cache(subscriptions, scope, CACHED_PROPERTIES)
    }

    /// [`new`](Self::new), each event's element arriving with exactly
    /// `properties` prefetched rather than the base cache request's
    /// ([`CACHED_PROPERTIES`]): for events whose callers read fewer, so the
    /// provider is asked for no more with each event.
    ///
    /// # Errors
    ///
    /// As for [`new`](Self::new).
    pub fn with_cache(
        subscriptions: Vec<Subscription>,
        scope: Scope,
        properties: &'static [UIA_PROPERTY_ID],
    ) -> windows::core::Result<Self> {
        let (incarnation, ready) =
            Incarnation::start(subscriptions.clone(), properties, scope.clone())?;
        match ready.recv_timeout(MOVE_DEADLINE) {
            Ok(Ok(())) => Ok(Self {
                subscriptions,
                properties,
                state: Mutex::new(State {
                    current: Some(incarnation),
                    abandoned: Vec::new(),
                    scope,
                }),
            }),
            Ok(Err(error)) => {
                let _ = incarnation.join.join();
                Err(error)
            }
            Err(RecvTimeoutError::Timeout) => {
                // Not waited for: it ends once its call returns.
                drop(incarnation.abandon());
                tracing::warn!("a UIA subscription's first registration passed its deadline");
                Err(windows::core::Error::new(
                    UIA_E_TIMEOUT,
                    "the subscription's first registration passed its deadline",
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = incarnation.join.join();
                Err(windows::core::Error::new(
                    windows::Win32::Foundation::E_FAIL,
                    "subscription thread ended before signalling readiness",
                ))
            }
        }
    }

    /// Moves the subscription to `scope`, without waiting: the registration's
    /// own thread removes the old handlers and registers the new ones. A
    /// thread whose move has passed [`MOVE_DEADLINE`] is abandoned instead,
    /// and a new one registers on `scope`.
    pub fn retarget(&self, scope: Scope) {
        let mut state = self.lock();
        state.scope = scope.clone();
        if state.current.as_ref().is_some_and(Incarnation::overdue) {
            self.replace(&mut state);
        } else if let Some(current) = &state.current {
            let _ = current.commands.send(Command::Retarget(scope));
        }
    }

    /// Waits until every move [`retarget`](Self::retarget) was asked for
    /// before this call has been made: the old handlers removed and the new
    /// ones registered, so every call those make into an application has
    /// returned. Returns at once if the registration is closed. A thread
    /// that has not made them within [`MOVE_DEADLINE`] is abandoned and
    /// replaced, and this returns: the replacement registers on the newest
    /// scope on its own.
    pub fn settle(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        {
            let state = self.lock();
            let Some(current) = &state.current else {
                return;
            };
            if current.commands.send(Command::Settle(done_tx)).is_err() {
                return;
            }
        }
        // A disconnection means the thread ended, with nothing left to
        // move.
        if let Err(RecvTimeoutError::Timeout) = done_rx.recv_timeout(MOVE_DEADLINE) {
            let mut state = self.lock();
            self.replace(&mut state);
        }
    }

    /// Ends the subscription: every move already asked for is made, then
    /// everything its client registered is removed
    /// (`RemoveAllEventHandlers`, which waits for any callback in progress
    /// to return), its objects are released, its thread leaves COM's
    /// multithreaded apartment, and the thread ends. Every thread abandoned
    /// before is waited for as well, each returning from its call however
    /// long UIA takes to end it. Returns once all of that is done. Moving
    /// or settling a closed registration does nothing; closing it again
    /// returns at once. Dropping a registration closes it.
    pub fn close(&self) {
        let (current, abandoned) = {
            let mut state = self.lock();
            (state.current.take(), std::mem::take(&mut state.abandoned))
        };
        if let Some(current) = current {
            drop(current.commands);
            let _ = current.join.join();
        }
        for thread in abandoned {
            let _ = thread.join();
        }
    }

    /// How many of this registration's threads have been abandoned and not
    /// yet waited for.
    #[must_use]
    pub fn abandoned(&self) -> usize {
        self.lock().abandoned.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Abandons the thread in charge, if the registration is not closed,
    /// and starts a replacement registering on the newest scope, without
    /// waiting for it.
    fn replace(&self, state: &mut State) {
        let Some(current) = state.current.take() else {
            return;
        };
        state.abandoned.retain(|thread| !thread.is_finished());
        state.abandoned.push(current.abandon());
        tracing::warn!(
            abandoned = state.abandoned.len(),
            "a UIA subscription's move passed its deadline; its thread is abandoned and replaced"
        );
        match Incarnation::start(
            self.subscriptions.clone(),
            self.properties,
            state.scope.clone(),
        ) {
            // Its first registration is made on its own thread.
            Ok((incarnation, _ready)) => state.current = Some(incarnation),
            Err(error) => {
                tracing::warn!(%error, "a UIA subscription could not be replaced");
            }
        }
    }
}

/// UIA's error for a provider that did not answer in time.
const UIA_E_TIMEOUT: windows::core::HRESULT = windows::core::HRESULT(0x8013_1505_u32.cast_signed());

/// What a registration's thread is asked to do.
enum Command {
    /// Move the subscription to this scope.
    Retarget(Scope),
    /// Reply once every move asked for before has been made.
    Settle(mpsc::Sender<()>),
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.close();
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
    /// The handler for `subscription`, calling back only while `live` is
    /// set: an abandoned thread's handlers stay registered until its call
    /// returns, and must not repeat what its replacement's report.
    fn of(subscription: Subscription, live: &Arc<AtomicBool>) -> Self {
        let live = Arc::clone(live);
        let is_live = move || live.load(Ordering::Acquire);
        match subscription {
            Subscription::Properties {
                properties,
                callback,
            } => Self::Properties(
                handlers::PropertyHandler {
                    callback: Arc::new(move |element, property| {
                        if is_live() {
                            callback(element, property);
                        }
                    }),
                }
                .into(),
                properties,
            ),
            Subscription::Event { event, callback } => Self::Event(
                handlers::EventHandler {
                    callback: Arc::new(move |element, _| {
                        if is_live() {
                            callback(element);
                        }
                    }),
                }
                .into(),
                event,
            ),
            Subscription::Events { events, callback } => Self::Events(
                handlers::EventHandler {
                    callback: Arc::new(move |element, event| {
                        if is_live() {
                            callback(element, event);
                        }
                    }),
                }
                .into(),
                events,
            ),
            Subscription::Notifications { callback } => Self::Notifications(
                handlers::NotificationHandler {
                    callback: Arc::new(move |element, kind, processing, display, activity| {
                        if is_live() {
                            callback(element, kind, processing, display, activity);
                        }
                    }),
                }
                .into(),
            ),
            Subscription::ActiveTextPosition { callback } => Self::ActiveTextPosition(
                handlers::ActiveTextPositionHandler {
                    callback: Arc::new(move |element, range| {
                        if is_live() {
                            callback(element, range);
                        }
                    }),
                }
                .into(),
            ),
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

/// What a registration's thread shares with the registration.
struct Thread {
    /// Whether the thread is still the one in charge.
    live: Arc<AtomicBool>,
    /// When its current move began, while it makes one.
    moving: Arc<Mutex<Option<Instant>>>,
}

impl Thread {
    fn is_live(&self) -> bool {
        self.live.load(Ordering::Acquire)
    }

    /// Notes when a move begins (`Some`) or ends (`None`).
    fn moving(&self, began: Option<Instant>) {
        *self.moving.lock().unwrap_or_else(PoisonError::into_inner) = began;
    }
}

fn run(
    subscriptions: Vec<Subscription>,
    properties: &[UIA_PROPERTY_ID],
    scope: &Scope,
    thread: &Thread,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    retarget: &mpsc::Receiver<Command>,
) {
    let setup = (|| -> windows::core::Result<Parts> {
        let uia = Uia::new()?;
        Ok(Parts {
            client: uia.client().cast()?,
            cache: uia.cache_request(properties)?,
            handlers: subscriptions
                .into_iter()
                .map(|subscription| Handler::of(subscription, &thread.live))
                .collect(),
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
    thread.moving(None);
    let _ = ready.send(Ok(()));
    while let Ok(command) = retarget.recv() {
        if !thread.is_live() {
            break;
        }
        let mut scope = match command {
            Command::Retarget(scope) => scope,
            Command::Settle(done) => {
                let _ = done.send(());
                continue;
            }
        };
        // Only the newest scope matters: skip any that queued up behind it,
        // up to a request to settle, which waits for that scope.
        let mut settled: Vec<mpsc::Sender<()>> = Vec::new();
        while let Ok(newer) = retarget.try_recv() {
            match newer {
                Command::Retarget(newer) if settled.is_empty() => scope = newer,
                Command::Retarget(newer) => {
                    thread.moving(Some(Instant::now()));
                    move_to(&parts, &scope);
                    thread.moving(None);
                    for done in settled.drain(..) {
                        let _ = done.send(());
                    }
                    scope = newer;
                }
                Command::Settle(done) => settled.push(done),
            }
        }
        if !thread.is_live() {
            break;
        }
        thread.moving(Some(Instant::now()));
        move_to(&parts, &scope);
        thread.moving(None);
        for done in settled {
            let _ = done.send(());
        }
    }
    // SAFETY: as above, before teardown.
    unsafe {
        let _ = parts.uia.client().RemoveAllEventHandlers();
    }
    // The handlers, the cache request, and the client are released before
    // the thread leaves the apartment `Uia::new` joined it to.
    drop(parts);
    crate::com::leave_mta();
}

/// Removes everything this registration's client registered and registers
/// the handlers again on `scope`.
fn move_to(parts: &Parts, scope: &Scope) {
    // SAFETY: removing this thread's own client's registrations. One on an
    // element that has gone since is forgotten by UIA itself, as NVDA notes.
    unsafe {
        let _ = parts.uia.client().RemoveAllEventHandlers();
    }
    register(parts, scope);
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
