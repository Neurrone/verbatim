//! A watch for one UI Automation structure change, for
//! [`Request::WatchChildrenAdded`](crate::protocol::Request::WatchChildrenAdded)
//! and [`Request::ControlledItems`](crate::protocol::Request::ControlledItems):
//! the evidence that an application has finished filling a list, so the
//! agent reads it only once it is whole.
//!
//! Settings' search suggestions are the case it was made for. Its list
//! (`SuggestionsList`) goes from no children to all of them at once, and
//! the one event that follows the fill in every opening is a
//! `StructureChanged` event of the kind `ChildrenBulkAdded` on the list; in
//! some openings Settings replaces the items as the list opens, and the
//! list has no children for about 100 ms after its first selection, so a
//! read made before that event can find none. The list is not reachable
//! through the search box before the query is typed, so the watch is
//! registered on the whole of the foreground window and keeps only the
//! changes from the element with the identifier asked for.
//!
//! A watch belongs to the connection that made it and lives on its thread
//! (the agent serves each connection on a thread of its own), so its COM
//! objects never cross threads; [`forget`] removes it when the connection
//! ends.

use std::cell::RefCell;
use std::io;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, SAFEARRAY,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationStructureChangedEventHandler,
    StructureChangeType, StructureChangeType_ChildrenBulkAdded, TreeScope_Subtree,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

thread_local! {
    /// This connection's watch, if one is armed.
    static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
}

/// A registered structure-change handler and what it has seen.
struct Watch {
    automation: IUIAutomation,
    root: IUIAutomationElement,
    handler: IUIAutomationStructureChangedEventHandler,
    automation_id: String,
    added: Arc<Added>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        // SAFETY: the element and handler are the ones registered together
        // in `watch_children_added`; removal waits for a call in progress.
        if let Err(error) = unsafe {
            self.automation
                .RemoveStructureChangedEventHandler(&self.root, &self.handler)
        } {
            tracing::warn!(%error, automation_id = self.automation_id, "removing a structure watch failed");
        }
    }
}

/// Whether the change watched for has come, set by the handler on UI
/// Automation's thread and waited on by the connection's thread.
#[derive(Default)]
struct Added {
    seen: Mutex<bool>,
    changed: Condvar,
}

/// Starts watching the foreground window for a `ChildrenBulkAdded`
/// structure change raised by its element whose UI Automation identifier
/// is `automation_id`, replacing this connection's earlier watch.
///
/// # Errors
///
/// Returns an error if UI Automation cannot be reached, there is no
/// foreground window, or the handler cannot be registered.
pub fn watch_children_added(automation_id: &str) -> io::Result<()> {
    forget();
    // SAFETY: initializes COM for this connection's thread; a thread
    // already initialized in the same mode is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    // SAFETY: a plain COM creation.
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(io::Error::other)?;
    // SAFETY: no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return Err(io::Error::other("there is no foreground window"));
    }
    // SAFETY: plain COM calls on live interfaces.
    let root = unsafe { automation.ElementFromHandle(window) }.map_err(io::Error::other)?;
    let added = Arc::new(Added::default());
    let handler = handler::new(automation_id.to_owned(), Arc::clone(&added));
    // SAFETY: as above; the handler stays registered until the watch is
    // dropped.
    unsafe { automation.AddStructureChangedEventHandler(&root, TreeScope_Subtree, None, &handler) }
        .map_err(io::Error::other)?;
    tracing::info!(automation_id, "watching for children added");
    WATCH.with(|watch| {
        *watch.borrow_mut() = Some(Watch {
            automation,
            root,
            handler,
            automation_id: automation_id.to_owned(),
            added,
        });
    });
    Ok(())
}

/// Waits up to `timeout` for this connection's watch to see its change,
/// then removes the watch.
///
/// # Errors
///
/// Returns an error if no watch is armed or the change does not come in
/// time.
pub fn wait_children_added(timeout: Duration) -> io::Result<()> {
    let watch = WATCH
        .with(|watch| watch.borrow_mut().take())
        .ok_or_else(|| io::Error::other("no WatchChildrenAdded was armed on this connection"))?;
    let seen = watch
        .added
        .seen
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let (seen, _) = watch
        .added
        .changed
        .wait_timeout_while(seen, timeout, |seen| !*seen)
        .unwrap_or_else(PoisonError::into_inner);
    let came = *seen;
    drop(seen);
    let automation_id = watch.automation_id.clone();
    drop(watch);
    if came {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "no children were added to {automation_id:?} within {} ms",
            timeout.as_millis()
        )))
    }
}

/// Removes this connection's watch, if one is armed.
pub fn forget() {
    let watch = WATCH.with(|watch| watch.borrow_mut().take());
    drop(watch);
}

/// The structure-change handler.
mod handler {
    // The `implement` macro's expansion.
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::{Arc, PoisonError};

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationStructureChangedEventHandler,
        IUIAutomationStructureChangedEventHandler_Impl,
    };

    use super::{Added, SAFEARRAY, StructureChangeType, StructureChangeType_ChildrenBulkAdded};

    pub(super) fn new(
        automation_id: String,
        added: Arc<Added>,
    ) -> IUIAutomationStructureChangedEventHandler {
        ChildrenAdded {
            automation_id,
            added,
        }
        .into()
    }

    #[windows_core::implement(IUIAutomationStructureChangedEventHandler)]
    struct ChildrenAdded {
        automation_id: String,
        added: Arc<Added>,
    }

    impl IUIAutomationStructureChangedEventHandler_Impl for ChildrenAdded_Impl {
        fn HandleStructureChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            change: StructureChangeType,
            _runtime_id: *const SAFEARRAY,
        ) -> windows_core::Result<()> {
            if change != StructureChangeType_ChildrenBulkAdded {
                return Ok(());
            }
            let Some(sender) = sender.as_ref() else {
                return Ok(());
            };
            // Read live, only for the few bulk additions: the agent keeps
            // no cache request, which Verbatim's shapes alone make.
            // SAFETY: a plain COM call on a live element.
            let Ok(automation_id) = (unsafe { sender.CurrentAutomationId() }) else {
                return Ok(());
            };
            if automation_id != self.automation_id.as_str() {
                return Ok(());
            }
            tracing::info!(
                automation_id = self.automation_id,
                "children added, as watched for"
            );
            *self
                .added
                .seen
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = true;
            self.added.changed.notify_all();
            Ok(())
        }
    }
}
