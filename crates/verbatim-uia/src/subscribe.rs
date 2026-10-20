//! UIA event subscriptions: property changes, automation events (an element
//! selected, a menu opened), notifications, and active text position
//! changes, over a [`Scope`] that can be moved later.
//!
//! Every [`Registration`] in a process is made by one thread, the
//! registering thread, with one client: the thread, its client, and its
//! place in COM's multithreaded apartment are created on the process's
//! first registration, and every registration's adds, removes, moves, and
//! closes are made there, in the order they are asked for. Microsoft's
//! guidance for UIA clients asks for this: a client should not add or
//! remove event handlers from several threads at once, since unexpected
//! behavior can result
//! (<https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-threading>).
//! What a registration registers, its handlers, groups, and the elements
//! they are on, is its own. A registration registers its subscriptions as
//! one event handler group (`IUIAutomationEventHandlerGroup`): the handlers
//! are added to the group, which is local, and the group is registered on
//! each element of the scope in one `AddEventHandlerGroup` call, as NVDA
//! registers its handlers. A registration has two such groups, as NVDA has
//! two local groups (`UIAHandler/__init__.py`,
//! `_createLocalEventHandlerGroup`): the base group, and the extended
//! group, which also holds the parts marked [`Part::extended`]; and a
//! third, the extended group without the parts marked
//! [`Part::text_events`], for an element whose text and caret changes
//! reach the caller another way. Each group is built once per
//! registration, on first need, and every handler in it is added with its
//! own part's cache request, so the element each event brings arrives with
//! that part's properties prefetched and the callback reads them without a
//! cross-process call. UIA calls the handlers on threads of its own.
//!
//! Moving a registration ([`Registration::retarget`],
//! [`Registration::retarget_to`]) is made on the registering thread, so the
//! caller never waits: removing a UIA handler waits for its running
//! callbacks to finish, which is why callbacks must never wait on any
//! registration's moves. A move adds the chosen group to each element of
//! the new scope that does not already have it, then removes from each
//! element that is no longer in the scope the group added there
//! (`RemoveEventHandlerGroup`), as NVDA adds its group to a new focus and
//! removes it from the old one (`UIAHandler/__init__.py`,
//! `addLocalEventHandlerGroupToElement` and
//! `removeLocalEventHandlerGroupFromElement`). An element that already has
//! the chosen group, the same element by `CompareElements`, keeps it, and no
//! call is made for it. Every move is logged at debug level, with the
//! elements it moves from and to, by runtime id, or, when a newer move of
//! the same registration queued behind it makes it unneeded, that it was
//! skipped and for which move; and each call a move or a close makes with
//! its result and the time it took, at warn level when that passed UIA's
//! connection timeout ([`crate::CONNECTION_TIMEOUT`]) or when a removal
//! failed, whatever its time, since what it leaves live goes on
//! delivering.
//! [`Registration::settle`] waits for every move asked for so far to be
//! made, for a caller that measures what the moves cost the application.
//! Closing or dropping a registration removes what it registered, each
//! removal tried once: a group or focus handler whose removal fails, in a
//! move or in the close, stays recorded as live, and a group a move
//! failed to remove is tried again at the close, but one whose removal
//! fails at the close stays live on the shared client, still calling the
//! closed registration's callbacks, until the registering thread ends, as
//! does a focus handler of a registration whose setting up failed after
//! it was added, if its removal fails. Whether the close's second try at a
//! group can succeed is not tested (`docs/uia-bugs.md`, "A registration
//! that times out reports success").
//! `RemoveAllEventHandlers` is called only then, once the process has
//! finished with UIA ([`crate::release_mta_usage`]), and removes it.
//!
//! No call is given a deadline or abandoned: UIA ends a registration call
//! into an application that does not answer at its connection timeout
//! (`docs/uia-bugs.md`, "A registration that times out reports success"),
//! and the next move waits for it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::E_FAIL;

use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationActiveTextPositionChangedEventHandler, IUIAutomationCacheRequest,
    IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandlerGroup,
    IUIAutomationFocusChangedEventHandler, IUIAutomationNotificationEventHandler,
    IUIAutomationPropertyChangedEventHandler, IUIAutomationTextRange, NotificationKind,
    NotificationProcessing, TreeScope, TreeScope_Element, TreeScope_Subtree,
    UIA_ControllerForPropertyId, UIA_EVENT_ID, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HelpTextPropertyId, UIA_IsEnabledPropertyId, UIA_NamePropertyId, UIA_PATTERN_ID,
    UIA_PROPERTY_ID, UIA_RangeValueValuePropertyId, UIA_ToggleToggleStatePropertyId,
    UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;
use windows_core::Interface;

use crate::client::Uia;
use crate::focus::{FocusCallback, FocusHandler};
use crate::shape::{Cached, Ungated, delivered};

/// Invoked on a UIA callback thread for a property change, with the cached
/// element and the changed property id.
pub type PropertyCallback<E = IUIAutomationElement> = Arc<dyn Fn(&E, i32) + Send + Sync>;

/// Invoked on a UIA callback thread for an automation event, with the cached
/// element that raised it.
pub type ElementCallback<E = IUIAutomationElement> = Arc<dyn Fn(&E) + Send + Sync>;

/// Invoked on a UIA callback thread for one of several automation events,
/// with the cached element that raised it and the event's id.
pub type EventCallback<E = IUIAutomationElement> = Arc<dyn Fn(&E, i32) + Send + Sync>;

/// Invoked on a UIA callback thread for a notification: the raising element,
/// the kind and processing hint, and the optional display string and
/// activity id.
pub type NotificationCallback<E = IUIAutomationElement> = Arc<
    dyn Fn(&E, NotificationKind, NotificationProcessing, Option<String>, Option<String>)
        + Send
        + Sync,
>;

/// Invoked on a UIA callback thread when a text control's active position
/// changed (`IUIAutomation6`'s active text position changed event, raised
/// when the application moves the place being read without moving the
/// caret, such as scrolling to an in-page link's target), with the cached
/// element and the range that is now active, when the event carries one.
pub type ActiveTextPositionCallback<E = IUIAutomationElement> =
    Arc<dyn Fn(&E, Option<&IUIAutomationTextRange>) + Send + Sync>;

/// The properties an outpost follows on the focus, and only there: name,
/// value (of the `Value` or the `RangeValue` pattern), the state-bearing
/// toggle, enabled, and expand/collapse properties, the help text (a
/// description change), and the `ControllerFor` relation (suggestions
/// opened or closed), as NVDA's local group follows them
/// (`localEventHandlerGroupUIAPropertyIds`, `UIAHandler/__init__.py` 247).
/// NVDA follows the range value desktop-wide instead, for progress bars
/// anywhere, and the item status too, which only its app modules act on.
pub const FOCUS_PROPERTIES: [UIA_PROPERTY_ID; 8] = [
    UIA_NamePropertyId,
    UIA_ValueValuePropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HelpTextPropertyId,
    UIA_ControllerForPropertyId,
];

/// Where a registration listens.
#[derive(Clone)]
pub enum Scope {
    /// Nowhere, until retargeted.
    Nothing,
    /// The subtree of each of these elements, such as windows' elements.
    /// The caller looks a window's element up itself, on a thread that
    /// serves the window's application, so the registering thread makes
    /// no connection by handle.
    Subtrees(Vec<AgileReference<IUIAutomationElement>>),
    /// The whole desktop: the subtree of the root element.
    Desktop,
    /// Exactly these elements, not their descendants.
    Elements(Vec<AgileReference<IUIAutomationElement>>),
}

/// Which of a registration's event handler groups the elements of its
/// scope are given, as NVDA gives a focus one of its two local groups
/// (`UIAHandler/__init__.py`, `addLocalEventHandlerGroupToElement`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Group {
    /// Every part not marked [`Part::extended`].
    #[default]
    Base,
    /// Every part.
    Extended,
    /// Every part but those marked [`Part::text_events`]: for an element
    /// whose text and caret changes the caller learns of another way, so
    /// UIA is not asked to deliver them.
    ExtendedWithoutTextEvents,
}

impl Group {
    /// Whether this group holds a part marked as `extended_only` and as
    /// `text_events`.
    fn holds(self, extended_only: bool, text_events: bool) -> bool {
        match self {
            Group::Base => !extended_only,
            Group::Extended => true,
            Group::ExtendedWithoutTextEvents => !text_events,
        }
    }
}

/// What a registration listens for. Its callbacks receive each event's
/// element as `E`: the raw element, or for a part made with
/// [`Part::shaped`] the element as its shape.
#[derive(Clone)]
pub enum Subscription<E = IUIAutomationElement> {
    /// Changes to these properties.
    Properties {
        /// The properties to watch.
        properties: Vec<UIA_PROPERTY_ID>,
        /// Called for each change.
        callback: PropertyCallback<E>,
    },
    /// An automation event, such as `SelectionItem_ElementSelected` or
    /// `MenuOpened`.
    Event {
        /// The event to watch.
        event: UIA_EVENT_ID,
        /// Called for each event.
        callback: ElementCallback<E>,
    },
    /// `AutomationNotification` events (`IUIAutomation5`).
    Notifications {
        /// Called for each notification.
        callback: NotificationCallback<E>,
    },
    /// Several automation events through one handler, such as a text
    /// control's caret and text changes.
    Events {
        /// The events to watch.
        events: Vec<UIA_EVENT_ID>,
        /// Called for each event, with its id.
        callback: EventCallback<E>,
    },
    /// Active text position changes (`IUIAutomation6`).
    ActiveTextPosition {
        /// Called for each change.
        callback: ActiveTextPositionCallback<E>,
    },
    /// Focus changes anywhere on the desktop. Not part of the event handler
    /// groups: the handler is registered on the registering thread's client
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
        callback: FocusCallback<E>,
    },
}

impl<S: Ungated> Subscription<Cached<S>> {
    /// The subscription with callbacks that receive the raw element, each
    /// wrapping it as the [`Cached<S>`] UIA filled it as.
    fn erase(self) -> Subscription {
        match self {
            Self::Properties {
                properties,
                callback,
            } => Subscription::Properties {
                properties,
                callback: Arc::new(move |element, property| {
                    callback(&delivered::<S>(element), property);
                }),
            },
            Self::Event { event, callback } => Subscription::Event {
                event,
                callback: Arc::new(move |element| callback(&delivered::<S>(element))),
            },
            Self::Notifications { callback } => Subscription::Notifications {
                callback: Arc::new(move |element, kind, processing, display, activity| {
                    callback(
                        &delivered::<S>(element),
                        kind,
                        processing,
                        display,
                        activity,
                    );
                }),
            },
            Self::Events { events, callback } => Subscription::Events {
                events,
                callback: Arc::new(move |element, event| {
                    callback(&delivered::<S>(element), event);
                }),
            },
            Self::ActiveTextPosition { callback } => Subscription::ActiveTextPosition {
                callback: Arc::new(move |element, range| {
                    callback(&delivered::<S>(element), range);
                }),
            },
            Self::Focus { callback } => Subscription::Focus {
                callback: Arc::new(move |element| callback(&delivered::<S>(element))),
            },
        }
    }
}

/// One [`Subscription`] of a registration, with the properties each of its
/// events' elements arrives with, and which of the registration's groups
/// hold it.
#[derive(Clone)]
pub struct Part {
    /// What the part listens for.
    subscription: Subscription,
    /// The properties each event's element arrives with: the part's handler
    /// is added to its group with a cache request of exactly these.
    properties: &'static [UIA_PROPERTY_ID],
    /// Whether only the extended groups ([`Group::Extended`] and
    /// [`Group::ExtendedWithoutTextEvents`]) hold the part; otherwise every
    /// group does.
    extended_only: bool,
    /// Whether the part delivers text or caret changes, which
    /// [`Group::ExtendedWithoutTextEvents`] leaves out.
    text_events: bool,
    /// The patterns the cache request also holds, such as the Text
    /// pattern [`crate::ListenerEvent`] prefetches, as NVDA's base cache
    /// request holds it.
    patterns: &'static [UIA_PATTERN_ID],
}

impl Part {
    /// A part every group holds for shape `S`: each event's element arrives
    /// with `S`'s request filled, and the subscription's callbacks receive
    /// it as a [`Cached<S>`] (`docs/design/uia-shapes.md`).
    #[must_use]
    pub fn shaped<S: Ungated>(subscription: Subscription<Cached<S>>) -> Self {
        Self {
            subscription: subscription.erase(),
            properties: S::PROPERTIES,
            extended_only: false,
            text_events: false,
            patterns: S::PATTERNS,
        }
    }

    /// The part, held only by the extended groups.
    #[must_use]
    pub fn extended(self) -> Self {
        Self {
            extended_only: true,
            ..self
        }
    }

    /// The part, marked as one that delivers text or caret changes, which
    /// [`Group::ExtendedWithoutTextEvents`] leaves out.
    #[must_use]
    pub fn text_events(self) -> Self {
        Self {
            text_events: true,
            ..self
        }
    }
}

/// A live subscription: one or more [`Part`]s registered together as one
/// event handler group, by the process's registering thread. Dropping it
/// unregisters it.
pub struct Registration {
    /// Which of the registering thread's registrations this is.
    id: u64,
    /// The way to the registering thread, until [`Registration::close`]
    /// takes it.
    commands: Mutex<Option<mpsc::Sender<Command>>>,
    /// The failed cached reads its callbacks have made, on UIA's threads
    /// ([`crate::calls`]).
    failed_reads: Arc<AtomicU32>,
}

/// Where a registration is asked to listen: a scope, and the group its
/// elements are given.
#[derive(Clone)]
struct Target {
    scope: Scope,
    group: Group,
}

/// The id the next registration is given.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The process's registering thread, from its first use until
/// [`end_registering_thread`].
static REGISTRAR: Mutex<Option<Registrar>> = Mutex::new(None);

/// The process's registering thread, as the registrations reach it.
struct Registrar {
    commands: mpsc::Sender<Command>,
    join: JoinHandle<()>,
}

/// The way to the process's registering thread, which is started on first
/// use: it joins COM's multithreaded apartment and creates its client
/// before this returns, local calls only.
fn registrar() -> windows::core::Result<mpsc::Sender<Command>> {
    let mut registrar = REGISTRAR.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(running) = registrar.as_ref()
        && !running.join.is_finished()
    {
        return Ok(running.commands.clone());
    }
    let (commands, received) = mpsc::channel::<Command>();
    let (ready_tx, ready) = mpsc::channel::<windows::core::Result<()>>();
    let join = thread::Builder::new()
        .name("verbatim-uia-registrations".to_owned())
        .spawn(move || serve(&ready_tx, &received))
        .map_err(|e| windows::core::Error::new(E_FAIL, e.to_string()))?;
    match ready.recv() {
        Ok(Ok(())) => {
            *registrar = Some(Registrar {
                commands: commands.clone(),
                join,
            });
            Ok(commands)
        }
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            let _ = join.join();
            Err(ended())
        }
    }
}

/// Ends the process's registering thread, if it runs, and returns once it
/// has ended: every move and close asked for before is made, then
/// everything its client registered is removed (`RemoveAllEventHandlers`,
/// which waits for any callback in progress to return, and the only place
/// that call is made), its objects are released, and it leaves COM's
/// multithreaded apartment. For a process that has finished with UIA for
/// good, which [`crate::release_mta_usage`] says: a registration still
/// open then hears nothing more, and moving, settling, or closing it does
/// nothing. A later registration starts the thread again, but only once
/// this one has ended: the registering thread's slot stays locked until
/// it has, so a registration asked for meanwhile waits and then starts a
/// fresh thread, and two threads never add and remove handlers at once.
pub(crate) fn end_registering_thread() {
    let mut registrar = REGISTRAR.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(Registrar { commands, join }) = registrar.take() {
        let _ = commands.send(Command::End);
        drop(commands);
        let _ = join.join();
    }
}

thread_local! {
    /// Whether this thread is the registering thread, where a
    /// registration's close cannot wait for the thread's answer.
    static REGISTERING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The error for a registering thread that ended before it answered.
fn ended() -> windows::core::Error {
    windows::core::Error::new(
        E_FAIL,
        "the UIA registering thread ended before it answered",
    )
}

impl Registration {
    /// Subscribes to every one of `parts` over `scope`, each with its own
    /// cache request, its elements given the base group ([`Group::Base`]),
    /// returning once registered. A later move can give a scope's elements
    /// the extended group instead ([`retarget_to`](Self::retarget_to)). The
    /// desktop's root is fetched with the first part's cache request.
    ///
    /// The registration is made on the process's registering thread, after
    /// every move and close asked for before it, and this waits for it
    /// however long it takes, with no deadline of its own: UIA ends a
    /// registration on an application that does not answer at its
    /// connection timeout ([`crate::CONNECTION_TIMEOUT`]; `docs/uia-bugs.md`,
    /// "A registration that times out reports success"), so a desktop-wide
    /// registration that advises several such applications waits that
    /// long for each. The time it took is logged at debug level, and at
    /// warn level when it passed the connection timeout.
    ///
    /// # Errors
    ///
    /// Returns the COM error if the registering thread's client, a cache
    /// request, or a handler cannot be created. An element of the scope
    /// that cannot be resolved, or on which the group cannot be
    /// registered, is skipped rather than failing the registration.
    pub fn with_parts(parts: Vec<Part>, scope: Scope) -> windows::core::Result<Self> {
        let target = Target {
            scope,
            group: Group::Base,
        };
        let to = describe(&target.scope);
        let failed_reads = Arc::new(AtomicU32::new(0));
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let commands = registrar()?;
        let (ready_tx, ready) = mpsc::channel();
        let started = Instant::now();
        commands
            .send(Command::Start {
                id,
                parts,
                target,
                failed_reads: Arc::clone(&failed_reads),
                ready: ready_tx,
            })
            .map_err(|_| ended())?;
        let result = ready.recv().unwrap_or_else(|_| Err(ended()));
        let elapsed = started.elapsed();
        let elapsed_ms = elapsed.as_millis();
        if elapsed > crate::CONNECTION_TIMEOUT {
            tracing::warn!(
                registration = id,
                to,
                result = ?result,
                elapsed_ms,
                "a UIA subscription's first registration took longer than UIA's connection timeout"
            );
        } else {
            tracing::debug!(
                registration = id,
                to,
                result = ?result,
                elapsed_ms,
                "a UIA subscription's first registration"
            );
        }
        result.map(|()| Self {
            id,
            commands: Mutex::new(Some(commands)),
            failed_reads,
        })
    }

    /// Moves the subscription to `scope`, its elements given the base group,
    /// without waiting ([`retarget_to`](Self::retarget_to)).
    pub fn retarget(&self, scope: Scope) {
        self.retarget_to(scope, Group::Base);
    }

    /// Moves the subscription to `scope`, its elements given `group`,
    /// without waiting: the registering thread adds the group to each
    /// element of the scope that does not already have it, then removes
    /// from every other element the group added there (module doc), once
    /// it has made everything asked of it before.
    pub fn retarget_to(&self, scope: Scope, group: Group) {
        if let Some(commands) = self.lock().as_ref() {
            let _ = commands.send(Command::Retarget {
                id: self.id,
                target: Target { scope, group },
            });
        }
    }

    /// Waits until every move [`retarget`](Self::retarget) was asked for
    /// before this call has been made: the new handlers registered and the
    /// old ones removed, so every call those make into an application has
    /// returned. It waits as long as those calls take, and as long as the
    /// registering thread takes over what other registrations asked of it
    /// before. Returns at once if the registration is closed.
    pub fn settle(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        {
            let commands = self.lock();
            let Some(commands) = commands.as_ref() else {
                return;
            };
            let sent = commands.send(Command::Settle {
                id: self.id,
                done: done_tx,
            });
            if sent.is_err() {
                return;
            }
        }
        // A disconnection means the thread ended, with nothing left to
        // move.
        let _ = done_rx.recv();
    }

    /// The failed cached reads its callbacks have made so far, on every
    /// thread UIA called them on ([`crate::calls`]).
    #[must_use]
    pub fn failed_reads(&self) -> u32 {
        self.failed_reads.load(Ordering::Relaxed)
    }

    /// Ends the subscription: every move already asked for is made, then
    /// the registering thread removes every group it registered for this
    /// registration (`RemoveEventHandlerGroup`) and its focus handler
    /// (`RemoveFocusChangedEventHandler`), each of which waits for any of
    /// its callbacks in progress to return, and releases its objects.
    /// Returns once all of that is done, except on the registering thread
    /// itself, as when the last reference to a registration's owner is
    /// dropped there with another registration's callbacks: the close is
    /// then queued and made after what the thread is doing, without
    /// waiting, since the thread cannot answer itself. Each removal is
    /// tried once; one that fails leaves its group or handler live on the
    /// registering thread's client until the thread ends (module doc).
    /// Moving or settling a closed registration does nothing; closing it
    /// again returns at once. Dropping a registration closes it.
    pub fn close(&self) {
        let Some(commands) = self.lock().take() else {
            return;
        };
        let (done_tx, done_rx) = mpsc::channel();
        let sent = commands.send(Command::Close {
            id: self.id,
            done: done_tx,
        });
        drop(commands);
        if sent.is_ok() && !REGISTERING.with(std::cell::Cell::get) {
            // A disconnection means the thread ended, removing everything.
            let _ = done_rx.recv();
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<mpsc::Sender<Command>>> {
        self.commands.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What the registering thread is asked to do, each for one registration
/// but [`Command::End`].
enum Command {
    /// Set registration `id` up and make its first registration, replying
    /// once it is made.
    Start {
        id: u64,
        parts: Vec<Part>,
        target: Target,
        failed_reads: Arc<AtomicU32>,
        ready: mpsc::Sender<windows::core::Result<()>>,
    },
    /// Move registration `id` to this target.
    Retarget { id: u64, target: Target },
    /// Reply once every move asked for before has been made.
    Settle { id: u64, done: mpsc::Sender<()> },
    /// Remove what registration `id` registered, and reply.
    Close { id: u64, done: mpsc::Sender<()> },
    /// End the thread ([`end_registering_thread`]).
    End,
}

impl Command {
    /// The registration the command is for.
    fn registration(&self) -> Option<u64> {
        match self {
            Self::Start { id, .. }
            | Self::Retarget { id, .. }
            | Self::Settle { id, .. }
            | Self::Close { id, .. } => Some(*id),
            Self::End => None,
        }
    }
}

/// The later move of registration `id` that lets a move of it be skipped,
/// if `queued`, the commands behind it, holds one before anything else of
/// that registration's that waits for its moves (a settle or a close):
/// only the newest target matters.
fn superseded<'a>(queued: impl IntoIterator<Item = &'a Command>, id: u64) -> Option<&'a Target> {
    for command in queued {
        if command.registration() == Some(id) {
            return match command {
                Command::Retarget { target, .. } => Some(target),
                _ => None,
            };
        }
    }
    None
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.close();
    }
}

/// Runs `callback`, adding the failed cached reads it makes to
/// `failed_reads`, by the difference between two looks at the thread's
/// count: UIA's callback threads run nothing else of this crate's that
/// takes it.
fn counted(failed_reads: &AtomicU32, callback: impl FnOnce()) {
    let before = crate::calls::peek_failed_reads();
    callback();
    let failed = crate::calls::peek_failed_reads().wrapping_sub(before);
    if failed != 0 {
        failed_reads.fetch_add(failed, Ordering::Relaxed);
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
    /// The handler for `subscription`. The failed cached reads each
    /// callback makes on UIA's thread are added to `failed_reads`.
    fn of(subscription: Subscription, failed_reads: &Arc<AtomicU32>) -> Self {
        let failed_reads = Arc::clone(failed_reads);
        match subscription {
            Subscription::Properties {
                properties,
                callback,
            } => Self::Properties(
                handlers::PropertyHandler {
                    callback: Arc::new(move |element, property| {
                        counted(&failed_reads, || callback(element, property));
                    }),
                }
                .into(),
                properties,
            ),
            Subscription::Event { event, callback } => Self::Event(
                handlers::EventHandler {
                    callback: Arc::new(move |element, _| {
                        counted(&failed_reads, || callback(element));
                    }),
                }
                .into(),
                event,
            ),
            Subscription::Events { events, callback } => Self::Events(
                handlers::EventHandler {
                    callback: Arc::new(move |element, event| {
                        counted(&failed_reads, || callback(element, event));
                    }),
                }
                .into(),
                events,
            ),
            Subscription::Notifications { callback } => Self::Notifications(
                handlers::NotificationHandler {
                    callback: Arc::new(move |element, kind, processing, display, activity| {
                        counted(&failed_reads, || {
                            callback(element, kind, processing, display, activity);
                        });
                    }),
                }
                .into(),
            ),
            Subscription::ActiveTextPosition { callback } => Self::ActiveTextPosition(
                handlers::ActiveTextPositionHandler {
                    callback: Arc::new(move |element, range| {
                        counted(&failed_reads, || callback(element, range));
                    }),
                }
                .into(),
            ),
            Subscription::Focus { callback } => Self::Focus(
                FocusHandler {
                    callback: Arc::new(move |element| {
                        counted(&failed_reads, || callback(element));
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

/// A part's handler as the registering thread holds it: with its cache
/// request, and whether only the extended group holds it.
struct Built {
    handler: Handler,
    cache: IUIAutomationCacheRequest,
    extended_only: bool,
    text_events: bool,
}

/// A group built on the registering thread for one registration, for one
/// [`Group`] and one tree scope, kept for the registration's life.
struct BuiltGroup {
    group: Group,
    tree_scope: TreeScope,
    handlers: IUIAutomationEventHandlerGroup,
}

/// An element the registering thread has added a registration's group to,
/// with the group added there, which a later move removes from it.
struct Registered {
    element: IUIAutomationElement,
    group: Group,
    tree_scope: TreeScope,
    handlers: IUIAutomationEventHandlerGroup,
}

/// The registering thread's client, also as `IUIAutomation6` for the event
/// handler groups: one for the process, which every registration's
/// handlers are added to and removed from.
struct Client {
    uia: Uia,
    client: IUIAutomation6,
}

/// What the registering thread holds for one registration: the handlers
/// with their cache requests, the groups built so far, the elements the
/// groups are registered on, and the focus handlers registered on the
/// client.
struct Follower {
    /// Which registration this is, for the log.
    id: u64,
    built: Vec<Built>,
    /// The desktop's root element, fetched before a focus handler was
    /// registered, for the group to be registered on, as NVDA registers its
    /// global group on the root it fetched first.
    desktop_root: Option<IUIAutomationElement>,
    groups: Vec<BuiltGroup>,
    registered: Vec<Registered>,
    /// The groups a move no longer wanted whose removal failed, still live
    /// on the client: [`Follower::close`] tries them again.
    unremoved: Vec<Registered>,
    /// The focus handlers registered on the client, which
    /// [`Follower::close`] removes.
    focus_handlers: Vec<IUIAutomationFocusChangedEventHandler>,
}

/// The registering thread: creates the process's client, says whether it
/// could, then makes what it is asked, in order, until it is ended
/// ([`end_registering_thread`]). It waits on its commands and wakes for
/// nothing else.
fn serve(ready: &mpsc::Sender<windows::core::Result<()>>, received: &mpsc::Receiver<Command>) {
    let client = match Uia::new().and_then(|uia| {
        Ok(Client {
            client: uia.client().cast()?,
            uia,
        })
    }) {
        Ok(client) => client,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    REGISTERING.with(|registering| registering.set(true));
    let mut followers: Vec<Follower> = Vec::new();
    // Closed registrations with a group or focus handler whose removal
    // failed, still live on the client until `RemoveAllEventHandlers`.
    let mut unremoved: Vec<Follower> = Vec::new();
    let mut queue: VecDeque<Command> = VecDeque::new();
    'serving: while let Ok(command) = received.recv() {
        queue.push_back(command);
        loop {
            queue.extend(received.try_iter());
            let Some(command) = queue.pop_front() else {
                break;
            };
            match command {
                Command::Start {
                    id,
                    parts,
                    target,
                    failed_reads,
                    ready,
                } => match Follower::set_up(&client, id, parts, &target, &failed_reads) {
                    Ok(mut follower) => {
                        follower.move_to(&client, &target);
                        followers.push(follower);
                        let _ = ready.send(Ok(()));
                    }
                    Err((error, live)) => {
                        unremoved.extend(live.map(|live| *live));
                        let _ = ready.send(Err(error));
                    }
                },
                Command::Retarget { id, target } => {
                    // Only the newest target matters: one queued behind
                    // it, before a settle or a close that waits for it,
                    // is made instead.
                    if let Some(later) = superseded(&queue, id) {
                        tracing::debug!(
                            registration = id,
                            to = describe(&target.scope),
                            later = describe(&later.scope),
                            later_group = ?later.group,
                            "a UIA subscription's move is skipped: a later move replaces it"
                        );
                        continue;
                    }
                    if let Some(follower) = followers.iter_mut().find(|follower| follower.id == id)
                    {
                        follower.move_to(&client, &target);
                    }
                }
                Command::Settle { done, .. } => {
                    let _ = done.send(());
                }
                Command::Close { id, done } => {
                    if let Some(at) = followers.iter().position(|follower| follower.id == id)
                        && let Some(live) = followers.swap_remove(at).close(&client)
                    {
                        unremoved.push(live);
                    }
                    let _ = done.send(());
                }
                Command::End => break 'serving,
            }
        }
    }
    // SAFETY: removing the registering client's own registrations, before
    // teardown.
    let removed = unsafe { client.uia.client().RemoveAllEventHandlers() };
    tracing::debug!(
        result = ?removed,
        open = followers.len(),
        closed_with_live_handlers = unremoved.len(),
        "the UIA registering thread ends: RemoveAllEventHandlers"
    );
    // The commands not made, the handlers, the cache requests, the groups,
    // the elements, and the client are released before the thread leaves
    // the apartment `Uia::new` joined it to.
    drop(queue);
    drop(followers);
    drop(unremoved);
    drop(client);
    crate::com::leave_mta();
}

/// Logs one registration call into an application, `call` on `element`,
/// with its result and how long it took: at debug level, or at warn level
/// when it took longer than UIA's connection timeout, or when it is a
/// removal (`removal`) that failed, whatever its time, since the group it
/// leaves live goes on delivering. The element is named by its runtime id,
/// which is local; its window would take a cached read the caller's
/// element is not known to hold, or a call into the application that has
/// just failed to answer.
fn log_call(
    (call, removal): (&str, bool),
    registration: u64,
    element: &IUIAutomationElement,
    result: &windows::core::Result<()>,
    elapsed: Duration,
) {
    // The runtime id is read only for a line that is written.
    let elapsed_ms = elapsed.as_millis();
    if elapsed > crate::CONNECTION_TIMEOUT {
        let runtime_id = crate::com::runtime_id(element);
        tracing::warn!(
            registration,
            ?runtime_id,
            ?result,
            elapsed_ms,
            "{call} took longer than UIA's connection timeout"
        );
    } else if removal && result.is_err() {
        let runtime_id = crate::com::runtime_id(element);
        tracing::warn!(
            registration,
            ?runtime_id,
            ?result,
            elapsed_ms,
            "{call} failed; the group stays live"
        );
    } else if tracing::enabled!(tracing::Level::DEBUG) {
        let runtime_id = crate::com::runtime_id(element);
        tracing::debug!(registration, ?runtime_id, ?result, elapsed_ms, "{call}");
    }
}

impl Follower {
    /// Sets registration `id` up on the registering thread: each part's
    /// handler with its cache request, and any focus handler registered on
    /// the client in NVDA's order (the desktop's root fetched first, for a
    /// desktop registration), before any group is. On an error, what it
    /// had registered is removed, and the registration is answered with
    /// the error when a removal failed, holding only what is still live,
    /// for the registering thread to keep until it ends.
    fn set_up(
        client: &Client,
        id: u64,
        parts: Vec<Part>,
        target: &Target,
        failed_reads: &Arc<AtomicU32>,
    ) -> Result<Self, (windows::core::Error, Option<Box<Self>>)> {
        let uia = &client.uia;
        // Parts asking for the same properties share one cache request, as
        // every one of NVDA's global handlers carries its one base cache
        // request.
        let mut caches: Vec<(
            &'static [UIA_PROPERTY_ID],
            &'static [UIA_PATTERN_ID],
            IUIAutomationCacheRequest,
        )> = Vec::new();
        let built = parts
            .into_iter()
            .map(|part| {
                let shared = caches.iter().find(|(properties, patterns, _)| {
                    *properties == part.properties && *patterns == part.patterns
                });
                let cache = if let Some((_, _, cache)) = shared {
                    cache.clone()
                } else {
                    let cache = crate::cache::cache_request(uia.client(), part.properties)?;
                    for &pattern in part.patterns {
                        // SAFETY: `cache` is a live cache request; a pattern id
                        // is a plain value.
                        unsafe { cache.AddPattern(pattern) }?;
                    }
                    caches.push((part.properties, part.patterns, cache.clone()));
                    cache
                };
                Ok(Built {
                    cache,
                    handler: Handler::of(part.subscription, failed_reads),
                    extended_only: part.extended_only,
                    text_events: part.text_events,
                })
            })
            .collect::<windows::core::Result<Vec<_>>>()
            .map_err(|error| (error, None))?;
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
                Some(
                    unsafe { uia.client().GetRootElementBuildCache(&first.cache) }
                        .map_err(|error| (error, None))?,
                )
            }
            _ => None,
        };
        let mut follower = Self {
            id,
            built,
            desktop_root,
            groups: Vec::new(),
            registered: Vec::new(),
            unremoved: Vec::new(),
            focus_handlers: Vec::new(),
        };
        let focus: Vec<_> = follower
            .built
            .iter()
            .filter_map(|built| match &built.handler {
                Handler::Focus(handler) => Some((handler.clone(), built.cache.clone())),
                _ => None,
            })
            .collect();
        for (handler, cache) in focus {
            let started = Instant::now();
            // SAFETY: the cache and handler are live and owned by this
            // thread's client, which keeps its own references to both.
            let added = unsafe { uia.client().AddFocusChangedEventHandler(&cache, &handler) };
            let elapsed_ms = started.elapsed().as_millis();
            if started.elapsed() > crate::CONNECTION_TIMEOUT {
                tracing::warn!(
                    registration = id,
                    result = ?added,
                    elapsed_ms,
                    "AddFocusChangedEventHandler took longer than UIA's connection timeout"
                );
            } else {
                tracing::debug!(
                    registration = id,
                    result = ?added,
                    elapsed_ms,
                    "AddFocusChangedEventHandler"
                );
            }
            if let Err(error) = added {
                // What this registration added before is removed; it has
                // no groups yet. A focus handler whose removal failed is
                // kept, as a closed registration's is.
                return Err((error, follower.close(client).map(Box::new)));
            }
            follower.focus_handlers.push(handler);
        }
        Ok(follower)
    }

    /// Moves the registration to `target`: adds the target's group to each
    /// of its elements that does not already have it, then removes from
    /// every element that is no longer in the target the group added there
    /// (module doc).
    fn move_to(&mut self, client: &Client, target: &Target) {
        let (elements, tree_scope) = self.resolve(client, &target.scope);
        tracing::debug!(
            registration = self.id,
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
            match self.group(client, target.group, tree_scope) {
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
                    wanted(registered) && same(client, &registered.element, element)
                });
                if let Some(at) = kept {
                    tracing::debug!(
                        runtime_id = ?crate::com::runtime_id(&element),
                        "the element already has the group; no call is made"
                    );
                    now.push(previous.swap_remove(at));
                    continue;
                }
                let started = Instant::now();
                // SAFETY: the element and group are live and owned by this
                // thread's client.
                let added = unsafe { client.client.AddEventHandlerGroup(&element, &handlers) };
                log_call(
                    ("AddEventHandlerGroup", false),
                    self.id,
                    &element,
                    &added,
                    started.elapsed(),
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
        let unremoved = self.remove(client, previous);
        self.unremoved.extend(unremoved);
        self.registered = now;
    }

    /// Removes from each of `registered` the group added there, once each,
    /// and answers those whose removal failed, which stay live on the
    /// client (a removal made in the gap after a registration that timed
    /// out fails and leaves the group live; `docs/uia-bugs.md`, "A
    /// registration that times out reports success").
    fn remove(&self, client: &Client, registered: Vec<Registered>) -> Vec<Registered> {
        let mut failed = Vec::new();
        for registered in registered {
            let started = Instant::now();
            // SAFETY: the element and the group added there are live and
            // owned by this thread's client.
            let removed = unsafe {
                client
                    .client
                    .RemoveEventHandlerGroup(&registered.element, &registered.handlers)
            };
            log_call(
                ("RemoveEventHandlerGroup", true),
                self.id,
                &registered.element,
                &removed,
                started.elapsed(),
            );
            if removed.is_err() {
                failed.push(registered);
            }
        }
        failed
    }

    /// Ends the registration: removes every group it registered, those a
    /// move failed to remove included, and its focus handlers, each removal
    /// tried once and waiting for that handler's callbacks in progress,
    /// then releases its objects. Answers the registration again, holding
    /// only what is still live, when a removal failed, so that the
    /// registering thread keeps it until it ends.
    fn close(mut self, client: &Client) -> Option<Self> {
        let mut registered = std::mem::take(&mut self.registered);
        registered.append(&mut self.unremoved);
        self.unremoved = self.remove(client, registered);
        for handler in std::mem::take(&mut self.focus_handlers) {
            let started = Instant::now();
            // SAFETY: the handler is live and was registered on this
            // client.
            let removed = unsafe { client.uia.client().RemoveFocusChangedEventHandler(&handler) };
            let elapsed = started.elapsed();
            let elapsed_ms = elapsed.as_millis();
            if elapsed > crate::CONNECTION_TIMEOUT {
                tracing::warn!(
                    registration = self.id,
                    result = ?removed,
                    elapsed_ms,
                    "RemoveFocusChangedEventHandler took longer than UIA's connection timeout"
                );
            } else if removed.is_err() {
                tracing::warn!(
                    registration = self.id,
                    result = ?removed,
                    elapsed_ms,
                    "RemoveFocusChangedEventHandler failed; the handler stays live"
                );
            } else {
                tracing::debug!(
                    registration = self.id,
                    result = ?removed,
                    elapsed_ms,
                    "RemoveFocusChangedEventHandler"
                );
            }
            if removed.is_err() {
                self.focus_handlers.push(handler);
            }
        }
        if self.unremoved.is_empty() && self.focus_handlers.is_empty() {
            return None;
        }
        tracing::warn!(
            registration = self.id,
            groups = self.unremoved.len(),
            focus_handlers = self.focus_handlers.len(),
            "a closed UIA subscription keeps handlers whose removal failed until the registering thread ends"
        );
        Some(self)
    }

    /// The elements of `scope`, and the tree scope a group registered on
    /// them listens over. The elements of a scope of elements or of their
    /// subtrees are the caller's, resolved here, a local call. The
    /// desktop's root is fetched with the first part's cache request, and
    /// UIA serves it in this process.
    fn resolve(&self, client: &Client, scope: &Scope) -> (Vec<IUIAutomationElement>, TreeScope) {
        let Some(cache) = self.built.first().map(|built| &built.cache) else {
            return (Vec::new(), TreeScope_Element);
        };
        match scope {
            Scope::Nothing => (Vec::new(), TreeScope_Element),
            Scope::Subtrees(elements) => (
                elements
                    .iter()
                    .filter_map(|agile| agile.resolve().ok())
                    .collect(),
                TreeScope_Subtree,
            ),
            Scope::Desktop => (
                match &self.desktop_root {
                    Some(root) => vec![root.clone()],
                    // SAFETY: `cache` is a live cache request owned by this
                    // thread.
                    None => unsafe { client.uia.client().GetRootElementBuildCache(cache) }
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
        client: &Client,
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
        let handlers = unsafe { client.client.CreateEventHandlerGroup() }?;
        for built in &self.built {
            if group.holds(built.extended_only, built.text_events) {
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
}

/// Whether `a` and `b` are the same element, by UIA's own comparison,
/// which compares their runtime ids, kept with each element
/// (`docs/performance.md`, "What counts as a call"). NVDA tests whether
/// an element has its group by membership of a set of the element
/// pointers it added the group to, not by `CompareElements`, so it
/// removes and adds the group again for a new pointer to the same
/// element, where Verbatim keeps it; what is registered is the same.
fn same(client: &Client, a: &IUIAutomationElement, b: &IUIAutomationElement) -> bool {
    // SAFETY: both elements are live, owned by this thread.
    unsafe { client.uia.client().CompareElements(a, b) }.is_ok_and(windows_core::BOOL::as_bool)
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
        Scope::Subtrees(elements) => format!("the subtrees of {} elements", elements.len()),
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
                crate::com::log_arrival("active text position", 0, element);
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
                crate::com::log_arrival("property", propertyid.0, element);
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
                crate::com::log_arrival("event", eventid.0, element);
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
                crate::com::log_arrival("notification", notificationkind.0, element);
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
    use super::{Command, Group, Scope, Target, plan_keep, superseded};

    /// Each group holds the parts it should, as (extended only, text
    /// events): the group without text events holds the base parts and the
    /// extended ones but for the text and caret events, base or extended.
    #[test]
    fn the_group_without_text_events_holds_every_part_but_the_text_events() {
        let parts = [(false, false), (false, true), (true, false), (true, true)];
        let held = |group: Group| -> Vec<bool> {
            parts
                .iter()
                .map(|&(extended_only, text_events)| group.holds(extended_only, text_events))
                .collect()
        };
        assert_eq!(held(Group::Base), [true, true, false, false]);
        assert_eq!(held(Group::Extended), [true, true, true, true]);
        assert_eq!(
            held(Group::ExtendedWithoutTextEvents),
            [true, false, true, false]
        );
    }

    /// An element the registering thread has registered on, by name, with
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

    fn retarget(id: u64) -> Command {
        Command::Retarget {
            id,
            target: Target {
                scope: Scope::Nothing,
                group: Group::Base,
            },
        }
    }

    /// A move queued on the registering thread is skipped for a later move
    /// of the same registration, past other registrations' commands, but
    /// not past a settle or close of its own, which waits for it.
    #[test]
    fn a_move_is_skipped_only_for_a_later_move_of_its_own_registration() {
        let (done, _) = std::sync::mpsc::channel();
        let settle = |id| Command::Settle {
            id,
            done: done.clone(),
        };
        let close = |id| Command::Close {
            id,
            done: done.clone(),
        };
        assert!(superseded(&[retarget(2), settle(2), retarget(1)], 1).is_some());
        assert!(superseded(&[retarget(2)], 1).is_none());
        assert!(superseded(&[settle(1), retarget(1)], 1).is_none());
        assert!(superseded(&[close(1), retarget(1)], 1).is_none());
        assert!(superseded(&[], 1).is_none());
    }
}
