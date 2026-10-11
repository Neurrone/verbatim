//! UIA event subscriptions: property changes, automation events (an element
//! selected, a menu opened), notifications, and active text position
//! changes, over a [`Scope`] that can be moved later.
//!
//! Each [`Registration`] owns its own thread, apartment, client, and
//! handlers, and registers its subscriptions as one event handler group
//! (`IUIAutomationEventHandlerGroup`): the handlers are added to the group,
//! which is local, and the group is registered on each element of the scope
//! in one `AddEventHandlerGroup` call, as NVDA registers its handlers. A
//! registration has two such groups, as NVDA has two local groups
//! (`UIAHandler/__init__.py`, `_createLocalEventHandlerGroup`): the base
//! group, and the extended group, which also holds the parts marked
//! [`Part::extended_only`]. Each group is built once per thread and client,
//! on first need, and every handler in it is added with its own part's
//! cache request, so the element each event brings arrives with that part's
//! properties prefetched and the callback reads them without a
//! cross-process call.
//!
//! Moving a registration ([`Registration::retarget`],
//! [`Registration::retarget_to`]) is made on that thread, so the caller never
//! waits: removing a UIA handler waits for its running callbacks to finish,
//! which is why callbacks must never wait on whoever moves the
//! subscription. A move adds the chosen group to each element of the new
//! scope that does not already have it, then removes from each element that
//! is no longer in the scope the group added there
//! (`RemoveEventHandlerGroup`), as NVDA adds its group to a new focus and
//! removes it from the old one (`UIAHandler/__init__.py`,
//! `addLocalEventHandlerGroupToElement` and
//! `removeLocalEventHandlerGroupFromElement`). An element that already has
//! the chosen group, the same element by `CompareElements`, keeps it, and no
//! call is made for it. `RemoveAllEventHandlers` is called only as the
//! thread ends. Every move is logged at debug level, with the elements it
//! moves from and to, by runtime id, and each call it makes with its result.
//! [`Registration::settle`] waits for every move asked for so far to be
//! made, for a caller that measures what the moves cost the application.
//! Dropping a registration unregisters and ends its thread.
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
    IUIAutomationFocusChangedEventHandler, IUIAutomationNotificationEventHandler,
    IUIAutomationPropertyChangedEventHandler, IUIAutomationTextRange, NotificationKind,
    NotificationProcessing, TreeScope, TreeScope_Element, TreeScope_Subtree, UIA_EVENT_ID,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_IsEnabledPropertyId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_RangeValueValuePropertyId, UIA_ToggleToggleStatePropertyId,
    UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;
use windows_core::Interface;

use crate::cache::CACHED_PROPERTIES;
use crate::client::Uia;
use crate::focus::{FocusCallback, FocusHandler};

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

/// How long a registration's thread may take over one move (registering on
/// a new scope and removing what is no longer in it) before it is abandoned
/// and replaced: two of UIA's call timeouts ([`crate::CALL_TIMEOUT`]), so a
/// move whose calls each answer within the timeout is never abandoned.
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

/// Which of a registration's two event handler groups the elements of its
/// scope are given, as NVDA gives a focus one of its two local groups
/// (`UIAHandler/__init__.py`, `addLocalEventHandlerGroupToElement`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Group {
    /// Every part not marked [`Part::extended_only`].
    #[default]
    Base,
    /// Every part.
    Extended,
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
    /// Focus changes anywhere on the desktop. Not part of the event handler
    /// groups: the handler is registered on the registration's client
    /// itself, as NVDA registers its focus handler on the client that holds
    /// its global group (`_registerGlobalEventHandlers`), and in NVDA's
    /// order: over the desktop, its root element fetched first, then the
    /// focus handler, then the group on that root. Windows Terminal's XAML
    /// raises some focus events from a second identity of the focused
    /// control (runtime ids starting 14, not 42), and UIA delivers those to
    /// a client whose focus handler is alone, or registered after its
    /// group, but not to one whose focus handler came first (measured
    /// 2026-10-12, `docs/design/focus-pipeline.md`, section 16.8).
    Focus {
        /// Called for each focus change.
        callback: FocusCallback,
    },
}

/// One [`Subscription`] of a registration, with the properties each of its
/// events' elements arrives with, and which of the registration's groups
/// hold it.
#[derive(Clone)]
pub struct Part {
    /// What the part listens for.
    pub subscription: Subscription,
    /// The properties each event's element arrives with: the part's handler
    /// is added to its group with a cache request of exactly these.
    pub properties: &'static [UIA_PROPERTY_ID],
    /// Whether only the extended group ([`Group::Extended`]) holds the part;
    /// otherwise both groups do.
    pub extended_only: bool,
    /// Whether the cache request also holds the Text pattern, as NVDA's
    /// base cache request does ([`Part::with_text_pattern`]).
    pub text_pattern: bool,
}

impl Part {
    /// A part both groups hold.
    #[must_use]
    pub fn new(subscription: Subscription, properties: &'static [UIA_PROPERTY_ID]) -> Self {
        Self {
            subscription,
            properties,
            extended_only: false,
            text_pattern: false,
        }
    }

    /// The part, its cache request also holding the Text pattern: with
    /// [`crate::NVDA_BASE_PROPERTIES`], NVDA's base cache request.
    #[must_use]
    pub fn with_text_pattern(self) -> Self {
        Self {
            text_pattern: true,
            ..self
        }
    }

    /// A part only the extended group holds.
    #[must_use]
    pub fn extended(subscription: Subscription, properties: &'static [UIA_PROPERTY_ID]) -> Self {
        Self {
            subscription,
            properties,
            extended_only: true,
            text_pattern: false,
        }
    }
}

/// A live subscription: one or more [`Part`]s registered together as one
/// event handler group. Dropping it unregisters and ends its thread.
pub struct Registration {
    /// What each of its threads registers, for a replacement.
    parts: Vec<Part>,
    state: Mutex<State>,
}

/// Where a registration is asked to listen: a scope, and the group its
/// elements are given.
#[derive(Clone)]
struct Target {
    scope: Scope,
    group: Group,
}

/// A registration's threads and the target it was last asked to move to.
struct State {
    /// The thread in charge, until [`Registration::close`] takes it.
    current: Option<Incarnation>,
    /// Threads abandoned past [`MOVE_DEADLINE`] and not yet waited for.
    abandoned: Vec<JoinHandle<()>>,
    /// The newest target asked for, where a replacement registers.
    target: Target,
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
    /// Starts a thread that registers `parts` on `target`.
    fn start(parts: Vec<Part>, target: Target) -> windows::core::Result<Started> {
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
                run(parts, &target, &thread, &ready_tx, &received);
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
        let parts = subscriptions
            .into_iter()
            .map(|subscription| Part::new(subscription, properties))
            .collect();
        Self::with_parts(parts, scope)
    }

    /// Subscribes to every one of `parts` over `scope`, each with its own
    /// cache request, its elements given the base group ([`Group::Base`]),
    /// returning once registered. A later move can give a scope's elements
    /// the extended group instead ([`retarget_to`](Self::retarget_to)). The
    /// elements of a scope of windows or the desktop are fetched with the
    /// first part's cache request.
    ///
    /// # Errors
    ///
    /// As for [`new`](Self::new).
    pub fn with_parts(parts: Vec<Part>, scope: Scope) -> windows::core::Result<Self> {
        let target = Target {
            scope,
            group: Group::Base,
        };
        let (incarnation, ready) = Incarnation::start(parts.clone(), target.clone())?;
        match ready.recv_timeout(MOVE_DEADLINE) {
            Ok(Ok(())) => Ok(Self {
                parts,
                state: Mutex::new(State {
                    current: Some(incarnation),
                    abandoned: Vec::new(),
                    target,
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

    /// Moves the subscription to `scope`, its elements given the base group,
    /// without waiting ([`retarget_to`](Self::retarget_to)).
    pub fn retarget(&self, scope: Scope) {
        self.retarget_to(scope, Group::Base);
    }

    /// Moves the subscription to `scope`, its elements given `group`,
    /// without waiting: the registration's own thread adds the group to
    /// each element of the scope that does not already have it, then
    /// removes from every other element the group added there (module
    /// doc). A thread whose move has passed [`MOVE_DEADLINE`] is abandoned
    /// instead, and a new one registers on `scope`.
    pub fn retarget_to(&self, scope: Scope, group: Group) {
        let target = Target { scope, group };
        let mut state = self.lock();
        state.target = target.clone();
        if state.current.as_ref().is_some_and(Incarnation::overdue) {
            self.replace(&mut state);
        } else if let Some(current) = &state.current {
            let _ = current.commands.send(Command::Retarget(target));
        }
    }

    /// Waits until every move [`retarget`](Self::retarget) was asked for
    /// before this call has been made: the new handlers registered and the
    /// old ones removed, so every call those make into an application has
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
    /// and starts a replacement registering on the newest target, without
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
        match Incarnation::start(self.parts.clone(), state.target.clone()) {
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
    /// Move the subscription to this target.
    Retarget(Target),
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
    Focus(IUIAutomationFocusChangedEventHandler),
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
            Subscription::Focus { callback } => Self::Focus(
                FocusHandler {
                    callback: Arc::new(move |element| {
                        if is_live() {
                            callback(element);
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
            // Registered on the client, not in a group.
            Self::Focus(_) => Ok(()),
        }
    }
}

/// A part's handler as a registration's thread holds it: with its cache
/// request, and whether only the extended group holds it.
struct Built {
    handler: Handler,
    cache: IUIAutomationCacheRequest,
    extended_only: bool,
}

/// A group built on a registration's thread, for one [`Group`] and one tree
/// scope, kept for the thread's life.
struct BuiltGroup {
    group: Group,
    tree_scope: TreeScope,
    handlers: IUIAutomationEventHandlerGroup,
}

/// An element a registration's thread has added a group to, with the group
/// added there, which a later move removes from it.
struct Registered {
    element: IUIAutomationElement,
    group: Group,
    tree_scope: TreeScope,
    handlers: IUIAutomationEventHandlerGroup,
}

/// What a registration's thread holds: its client, also as
/// `IUIAutomation6` for the event handler groups, the handlers with their
/// cache requests, the groups built so far, and the elements the groups
/// are registered on.
struct Follower {
    uia: Uia,
    client: IUIAutomation6,
    built: Vec<Built>,
    /// The desktop's root element, fetched before a focus handler was
    /// registered, for the group to be registered on, as NVDA registers its
    /// global group on the root it fetched first.
    desktop_root: Option<IUIAutomationElement>,
    groups: Vec<BuiltGroup>,
    registered: Vec<Registered>,
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

/// Sets a registration's thread up: its client, each part's handler with
/// its cache request, and any focus handler registered on the client
/// in NVDA's order (the desktop's root fetched first, for a desktop
/// registration), before any group is.
fn set_up(parts: Vec<Part>, target: &Target, thread: &Thread) -> windows::core::Result<Follower> {
    let uia = Uia::new()?;
    // Parts asking for the same properties share one cache request, as
    // every one of NVDA's global handlers carries its one base cache
    // request.
    let mut caches: Vec<(&'static [UIA_PROPERTY_ID], bool, IUIAutomationCacheRequest)> = Vec::new();
    let built = parts
        .into_iter()
        .map(|part| {
            let shared = caches.iter().find(|(properties, text_pattern, _)| {
                *properties == part.properties && *text_pattern == part.text_pattern
            });
            let cache = if let Some((_, _, cache)) = shared {
                cache.clone()
            } else {
                let cache = uia.cache_request(part.properties)?;
                if part.text_pattern {
                    crate::cache::add_text_pattern(&cache)?;
                }
                caches.push((part.properties, part.text_pattern, cache.clone()));
                cache
            };
            Ok(Built {
                cache,
                handler: Handler::of(part.subscription, &thread.live),
                extended_only: part.extended_only,
            })
        })
        .collect::<windows::core::Result<Vec<_>>>()?;
    // NVDA's order (`UIAHandler/__init__.py` 547 to 593): the desktop's
    // root element is fetched first, then the focus handler goes on the
    // client, and only then is the group registered, on that root. A
    // focus handler registered after the group receives the second
    // identity Windows Terminal raises focus from (NVDA itself does,
    // its handler registered again after its group: 55 times in 40
    // openings; `docs/design/focus-pipeline.md`, section 16.8).
    let has_focus = built
        .iter()
        .any(|built| matches!(built.handler, Handler::Focus(_)));
    let desktop_root = match (&target.scope, built.first()) {
        (Scope::Desktop, Some(first)) if has_focus => {
            // SAFETY: the cache request is live and owned by this
            // thread's client.
            Some(unsafe { uia.client().GetRootElementBuildCache(&first.cache) }?)
        }
        _ => None,
    };
    for built in &built {
        if let Handler::Focus(handler) = &built.handler {
            // SAFETY: the cache and handler are live and owned by this
            // thread's client, which keeps its own references to both.
            unsafe {
                uia.client()
                    .AddFocusChangedEventHandler(&built.cache, handler)
            }?;
        }
    }
    Ok(Follower {
        client: uia.client().cast()?,
        uia,
        built,
        desktop_root,
        groups: Vec::new(),
        registered: Vec::new(),
    })
}

fn run(
    parts: Vec<Part>,
    target: &Target,
    thread: &Thread,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    retarget: &mpsc::Receiver<Command>,
) {
    let setup = set_up(parts, target, thread);
    let mut follower = match setup {
        Ok(follower) => follower,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    follower.move_to(target);
    thread.moving(None);
    let _ = ready.send(Ok(()));
    while let Ok(command) = retarget.recv() {
        if !thread.is_live() {
            break;
        }
        let mut target = match command {
            Command::Retarget(target) => target,
            Command::Settle(done) => {
                let _ = done.send(());
                continue;
            }
        };
        // Only the newest target matters: skip any that queued up behind
        // it, up to a request to settle, which waits for that target.
        let mut settled: Vec<mpsc::Sender<()>> = Vec::new();
        while let Ok(newer) = retarget.try_recv() {
            match newer {
                Command::Retarget(newer) if settled.is_empty() => target = newer,
                Command::Retarget(newer) => {
                    thread.moving(Some(Instant::now()));
                    follower.move_to(&target);
                    thread.moving(None);
                    for done in settled.drain(..) {
                        let _ = done.send(());
                    }
                    target = newer;
                }
                Command::Settle(done) => settled.push(done),
            }
        }
        if !thread.is_live() {
            break;
        }
        thread.moving(Some(Instant::now()));
        follower.move_to(&target);
        thread.moving(None);
        for done in settled {
            let _ = done.send(());
        }
    }
    // SAFETY: removing this thread's own client's registrations, before
    // teardown.
    let removed = unsafe { follower.uia.client().RemoveAllEventHandlers() };
    tracing::debug!(
        result = ?removed,
        "a UIA subscription ends: RemoveAllEventHandlers"
    );
    // The handlers, the cache requests, the groups, the elements, and the
    // client are released before the thread leaves the apartment
    // `Uia::new` joined it to.
    drop(follower);
    crate::com::leave_mta();
}

impl Follower {
    /// Moves the registration to `target`: adds the target's group to each
    /// of its elements that does not already have it, then removes from
    /// every element that is no longer in the target the group added there
    /// (module doc).
    fn move_to(&mut self, target: &Target) {
        let (elements, tree_scope) = self.resolve(&target.scope);
        tracing::debug!(
            from = ?self
                .registered
                .iter()
                .map(|registered| (crate::com::runtime_id(&registered.element), registered.group))
                .collect::<Vec<_>>(),
            to = describe(&target.scope),
            elements = ?elements.iter().map(crate::com::runtime_id).collect::<Vec<_>>(),
            group = ?target.group,
            "a UIA subscription moves"
        );
        let handlers = if elements.is_empty() {
            None
        } else {
            match self.group(target.group, tree_scope) {
                Ok(handlers) => Some(handlers),
                Err(error) => {
                    tracing::warn!(%error, "a UIA event handler group could not be built");
                    None
                }
            }
        };
        let mut previous = std::mem::take(&mut self.registered);
        let mut now = Vec::new();
        if let Some(handlers) = handlers {
            let wanted = |registered: &Registered| {
                registered.group == target.group && registered.tree_scope == tree_scope
            };
            for element in elements {
                let kept = plan_keep(&previous, &element, |registered, element| {
                    wanted(registered) && self.same(&registered.element, element)
                });
                if let Some(at) = kept {
                    tracing::debug!(
                        runtime_id = ?crate::com::runtime_id(&element),
                        "the element already has the group; no call is made"
                    );
                    now.push(previous.swap_remove(at));
                    continue;
                }
                // SAFETY: the element and group are live and owned by this
                // thread's client.
                let added = unsafe { self.client.AddEventHandlerGroup(&element, &handlers) };
                tracing::debug!(
                    runtime_id = ?crate::com::runtime_id(&element),
                    result = ?added,
                    "AddEventHandlerGroup"
                );
                // On an error, as NVDA logs it and goes on: the element has
                // most likely gone, and nothing is registered on it.
                if added.is_ok() {
                    now.push(Registered {
                        element,
                        group: target.group,
                        tree_scope,
                        handlers: handlers.clone(),
                    });
                }
            }
        }
        for registered in previous {
            // SAFETY: the element and the group added there are live and
            // owned by this thread's client.
            let removed = unsafe {
                self.client
                    .RemoveEventHandlerGroup(&registered.element, &registered.handlers)
            };
            // Forgotten whatever the result, as NVDA forgets it: a group on
            // an element that has gone since is forgotten by UIA itself.
            tracing::debug!(
                runtime_id = ?crate::com::runtime_id(&registered.element),
                result = ?removed,
                "RemoveEventHandlerGroup"
            );
        }
        self.registered = now;
    }

    /// The elements of `scope`, fetched with the first part's cache
    /// request where they must be fetched, and the tree scope a group
    /// registered on them listens over.
    fn resolve(&self, scope: &Scope) -> (Vec<IUIAutomationElement>, TreeScope) {
        let Some(cache) = self.built.first().map(|built| &built.cache) else {
            return (Vec::new(), TreeScope_Element);
        };
        match scope {
            Scope::Nothing => (Vec::new(), TreeScope_Element),
            Scope::Windows(hwnds) => (
                hwnds
                    .iter()
                    .filter_map(|&hwnd| self.uia.element_from_handle(hwnd, cache).ok())
                    .collect(),
                TreeScope_Subtree,
            ),
            Scope::Desktop => (
                match &self.desktop_root {
                    Some(root) => vec![root.clone()],
                    // SAFETY: `cache` is a live cache request owned by this
                    // thread.
                    None => unsafe { self.uia.client().GetRootElementBuildCache(cache) }
                        .ok()
                        .into_iter()
                        .collect(),
                },
                TreeScope_Subtree,
            ),
            Scope::Elements(elements) => (
                elements
                    .iter()
                    .filter_map(|agile| agile.resolve().ok())
                    .collect(),
                TreeScope_Element,
            ),
        }
    }

    /// The event handler group for `group` over `tree_scope`, built on its
    /// first use and kept: each part the group holds, with the part's own
    /// cache request. Local: nothing is registered until the group is.
    fn group(
        &mut self,
        group: Group,
        tree_scope: TreeScope,
    ) -> windows::core::Result<IUIAutomationEventHandlerGroup> {
        if let Some(built) = self
            .groups
            .iter()
            .find(|built| built.group == group && built.tree_scope == tree_scope)
        {
            return Ok(built.handlers.clone());
        }
        // SAFETY: a local call on this thread's live client.
        let handlers = unsafe { self.client.CreateEventHandlerGroup() }?;
        for built in &self.built {
            if group == Group::Extended || !built.extended_only {
                built.handler.add_to(&handlers, tree_scope, &built.cache)?;
            }
        }
        self.groups.push(BuiltGroup {
            group,
            tree_scope,
            handlers: handlers.clone(),
        });
        Ok(handlers)
    }

    /// Whether `a` and `b` are the same element, by UIA's own comparison,
    /// which compares their runtime ids, kept with each element
    /// (`docs/performance.md`, "What counts as a call"), as NVDA compares a
    /// focus with the elements that have its group.
    fn same(&self, a: &IUIAutomationElement, b: &IUIAutomationElement) -> bool {
        // SAFETY: both elements are live, owned by this thread.
        unsafe { self.uia.client().CompareElements(a, b) }.is_ok_and(windows_core::BOOL::as_bool)
    }
}

/// Which of `previous` to keep for `element`, the first that `same` says is
/// that element with the group wanted, if one does: a move keeps it, and
/// makes no call for the element.
fn plan_keep<P, E>(previous: &[P], element: &E, same: impl Fn(&P, &E) -> bool) -> Option<usize> {
    previous
        .iter()
        .position(|registered| same(registered, element))
}

/// A scope, for the debug log of a move.
fn describe(scope: &Scope) -> String {
    match scope {
        Scope::Nothing => "nothing".to_owned(),
        Scope::Windows(hwnds) => format!("the subtrees of windows {hwnds:?}"),
        Scope::Desktop => "the desktop".to_owned(),
        Scope::Elements(elements) => format!("{} elements", elements.len()),
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

#[cfg(test)]
mod tests {
    use super::plan_keep;

    /// An element a registration's thread has registered on, by name, with
    /// its group, for the plan's tests.
    #[derive(Debug)]
    struct Registered(&'static str, bool);

    /// Replays a move's plan as [`super::Follower::move_to`] makes it: for
    /// each element of the new scope, keeps the first previous registration
    /// on the same element with the wanted group, or adds the group; then
    /// removes every previous registration not kept. Returns the elements
    /// added to and removed from.
    fn plan(
        previous: Vec<Registered>,
        elements: &[&'static str],
        extended: bool,
    ) -> (Vec<&'static str>, Vec<&'static str>) {
        let mut previous = previous;
        let mut added = Vec::new();
        for &element in elements {
            match plan_keep(&previous, &element, |registered, element| {
                registered.0 == *element && registered.1 == extended
            }) {
                Some(at) => drop(previous.swap_remove(at)),
                None => added.push(element),
            }
        }
        let removed = previous
            .into_iter()
            .map(|registered| registered.0)
            .collect();
        (added, removed)
    }

    #[test]
    fn a_move_to_the_element_already_followed_makes_no_call() {
        assert_eq!(
            plan(vec![Registered("doc", true)], &["doc"], true),
            (vec![], vec![])
        );
    }

    #[test]
    fn a_move_to_a_new_element_adds_there_and_removes_from_the_old_one() {
        assert_eq!(
            plan(vec![Registered("doc", true)], &["ok"], false),
            (vec!["ok"], vec!["doc"])
        );
    }

    #[test]
    fn a_move_to_the_same_element_with_the_other_group_replaces_the_group() {
        assert_eq!(
            plan(vec![Registered("doc", false)], &["doc"], true),
            (vec!["doc"], vec!["doc"])
        );
    }

    #[test]
    fn a_move_to_nothing_removes_every_group() {
        assert_eq!(
            plan(
                vec![Registered("doc", false), Registered("ok", false)],
                &[],
                false
            ),
            (vec![], vec!["doc", "ok"])
        );
    }

    #[test]
    fn each_previous_registration_is_kept_for_one_element_only() {
        assert_eq!(
            plan(
                vec![Registered("window", false)],
                &["window", "window"],
                false
            ),
            (vec!["window"], vec![])
        );
    }
}
