//! UIA selection-item and notification event registration, scoped to the
//! target app's top-level windows (roadmap M3's selection-events and
//! generic-notification bullets).
//!
//! Two independent registrations, following [`crate::events`]'s
//! `PropertyRegistration` pattern exactly: each owns its own thread,
//! apartment, client, and handler, and unregisters on drop.
//!
//! - [`SelectionRegistration`] — `SelectionItem_ElementSelected`, UIA's
//!   answer to "an item became selected within its container" (list boxes,
//!   tab controls, and similar). Feeds
//!   [`NormalizedEvent::SelectionChanged`](verbatim_model::NormalizedEvent::SelectionChanged).
//! - [`NotificationRegistration`] — `AutomationNotification`
//!   (`IUIAutomation5::AddNotificationEventHandler`), the generic
//!   app-initiated announcement channel Windows 11 shell surfaces use for
//!   things like snap-layout hints. Feeds
//!   [`NormalizedEvent::Notification`](verbatim_model::NormalizedEvent::Notification).

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomation5, IUIAutomationElement, IUIAutomationEventHandler,
    IUIAutomationNotificationEventHandler, NotificationKind, NotificationProcessing,
    TreeScope_Subtree, UIA_SelectionItem_ElementSelectedEventId,
};
use windows_core::Interface;

use crate::client::Uia;

pub use selection_handler::SelectionHandler;

/// Invoked on the UIA callback thread for each selection event whose
/// element belongs to a watched window's subtree. Receives the cached
/// selected element; the outpost maps it to a snapshot and applies its
/// arbitration cross-filter, matching every other UIA callback in this
/// crate.
pub type SelectionCallback = Arc<dyn Fn(&IUIAutomationElement) + Send + Sync>;

/// The `#[implement]`-generated COM object lives in its own module so the
/// module-level allow covers the macro's generated glue, matching
/// [`crate::events`] and [`crate::focus`]'s modules.
mod selection_handler {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler_Impl, UIA_EVENT_ID,
    };
    use windows_core::implement;

    use super::SelectionCallback;

    /// The COM object implementing the selection-item event handler.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationEventHandler)]
    pub struct SelectionHandler {
        pub callback: SelectionCallback,
    }

    impl IUIAutomationEventHandler_Impl for SelectionHandler_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            _eventid: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                (self.callback)(element);
            }
            Ok(())
        }
    }
}

/// A live `SelectionItem_ElementSelected` registration over a set of
/// top-level windows. Dropping it unregisters and tears down its thread.
pub struct SelectionRegistration {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl SelectionRegistration {
    /// Registers a `SelectionItem_ElementSelected` handler over the subtree
    /// of each window in `hwnds`. Returns once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if setup or registration fails. Windows whose
    /// element cannot be resolved are skipped rather than failing the whole
    /// registration, matching `PropertyRegistration::new`.
    pub fn new(hwnds: Vec<isize>, callback: SelectionCallback) -> windows::core::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-selection".to_owned())
            .spawn(move || run_selection(hwnds, callback, &ready_tx, &stop_rx))
            .map_err(|e| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop: Some(stop_tx),
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
                    "selection registration thread ended before signalling readiness",
                ))
            }
        }
    }
}

impl Drop for SelectionRegistration {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_selection(
    hwnds: Vec<isize>,
    callback: SelectionCallback,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    stop: &mpsc::Receiver<()>,
) {
    let setup = (|| -> windows::core::Result<(Uia, IUIAutomationEventHandler)> {
        let uia = Uia::new()?;
        let cache = uia.base_cache_request()?;
        let handler: IUIAutomationEventHandler = SelectionHandler { callback }.into();
        for hwnd in hwnds {
            if let Ok(element) = uia.element_from_handle(hwnd, &cache) {
                // SAFETY: `element`, `cache`, and `handler` are all live and
                // owned by this thread's client.
                unsafe {
                    let _ = uia.client().AddAutomationEventHandler(
                        UIA_SelectionItem_ElementSelectedEventId,
                        &element,
                        TreeScope_Subtree,
                        &cache,
                        &handler,
                    );
                }
            }
        }
        Ok((uia, handler))
    })();

    match setup {
        Err(error) => {
            let _ = ready.send(Err(error));
        }
        Ok((uia, _handler)) => {
            let _ = ready.send(Ok(()));
            let _ = stop.recv();
            // SAFETY: removing this thread's own registrations before teardown.
            unsafe {
                let _ = uia.client().RemoveAllEventHandlers();
            }
            drop(uia);
        }
    }
}

pub use notification_handler::NotificationHandler;

/// Invoked on the UIA callback thread for each notification whose element
/// belongs to a watched window's subtree: the raising element, the
/// notification kind and processing hint, and the optional display string
/// and activity id UIA carries alongside them.
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

/// The `#[implement]`-generated COM object lives in its own module, matching
/// [`selection_handler`].
mod notification_handler {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationNotificationEventHandler_Impl, NotificationKind,
        NotificationProcessing,
    };
    use windows_core::implement;

    use super::NotificationCallback;

    /// The COM object implementing the notification event handler.
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
                let display = (!displaystring.is_empty()).then(|| displaystring.to_string());
                let activity = (!activityid.is_empty()).then(|| activityid.to_string());
                (self.callback)(
                    element,
                    notificationkind,
                    notificationprocessing,
                    display,
                    activity,
                );
            }
            Ok(())
        }
    }
}

/// A live `AutomationNotification` registration over a set of top-level
/// windows. Dropping it unregisters and tears down its thread.
pub struct NotificationRegistration {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl NotificationRegistration {
    /// Registers an `AutomationNotification` handler over the subtree of
    /// each window in `hwnds`, via `IUIAutomation5` (queried from the base
    /// `IUIAutomation` client). Returns once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if setup, the `IUIAutomation5` query, or
    /// registration fails. Windows whose element cannot be resolved are
    /// skipped rather than failing the whole registration, matching
    /// `PropertyRegistration::new`.
    pub fn new(hwnds: Vec<isize>, callback: NotificationCallback) -> windows::core::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-notify".to_owned())
            .spawn(move || run_notification(hwnds, callback, &ready_tx, &stop_rx))
            .map_err(|e| {
                windows::core::Error::new(windows::Win32::Foundation::E_FAIL, e.to_string())
            })?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                stop: Some(stop_tx),
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
                    "notification registration thread ended before signalling readiness",
                ))
            }
        }
    }
}

impl Drop for NotificationRegistration {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run_notification(
    hwnds: Vec<isize>,
    callback: NotificationCallback,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    stop: &mpsc::Receiver<()>,
) {
    let setup = (|| -> windows::core::Result<(Uia, IUIAutomationNotificationEventHandler)> {
        let uia = Uia::new()?;
        let client5: IUIAutomation5 = uia.client().cast()?;
        let cache = uia.base_cache_request()?;
        let handler: IUIAutomationNotificationEventHandler =
            NotificationHandler { callback }.into();
        for hwnd in hwnds {
            if let Ok(element) = uia.element_from_handle(hwnd, &cache) {
                // SAFETY: `element`, `cache`, and `handler` are all live and
                // owned by this thread's client.
                unsafe {
                    let _ = client5.AddNotificationEventHandler(
                        &element,
                        TreeScope_Subtree,
                        &cache,
                        &handler,
                    );
                }
            }
        }
        Ok((uia, handler))
    })();

    match setup {
        Err(error) => {
            let _ = ready.send(Err(error));
        }
        Ok((uia, _handler)) => {
            let _ = ready.send(Ok(()));
            let _ = stop.recv();
            // SAFETY: removing this thread's own registrations before teardown.
            unsafe {
                let _ = uia.client().RemoveAllEventHandlers();
            }
            drop(uia);
        }
    }
}
