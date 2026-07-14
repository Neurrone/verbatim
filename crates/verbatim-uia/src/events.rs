//! UIA name-, value-, and state-change registration, scoped to the target
//! app's top-level windows.
//!
//! Focus changes come through [`crate::focus`]; this module covers the
//! property changes the reducer consumes in M1 — name, value, and the
//! state-bearing toggle and enabled properties — registered with the base
//! cache request so the changed element arrives with its M1 properties
//! prefetched (the state read on delivery is therefore a cached, non-blocking
//! read). Like the focus module it owns a thread, apartment, client, and
//! handler, and unregisters on drop.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationPropertyChangedEventHandler, TreeScope_Subtree,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_IsEnabledPropertyId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};

use crate::client::Uia;

pub use handler::PropertyHandler;

/// Invoked on the UIA callback thread for each watched property change.
/// Receives the cached element and the changed property id (one of
/// [`UIA_NamePropertyId`], [`UIA_ValueValuePropertyId`], or a state-bearing
/// property such as [`UIA_ToggleToggleStatePropertyId`]).
pub type PropertyCallback = Arc<dyn Fn(&IUIAutomationElement, i32) + Send + Sync>;

/// The properties whose changes this registration subscribes to. The
/// state-bearing toggle, enabled, and expand/collapse properties are here so a
/// check box toggling or a control becoming unavailable reaches the reducer;
/// they are already in the base cache request, so the handler reads the rebuilt
/// state set from the cache without a fresh cross-process call.
const WATCHED_PROPERTIES: [UIA_PROPERTY_ID; 5] = [
    UIA_NamePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
];

/// The `#[implement]`-generated COM object lives in its own module so the
/// module-level allow covers the macro's generated glue (see [`crate::focus`]
/// for the same pattern and rationale) without loosening the lint elsewhere.
mod handler {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationPropertyChangedEventHandler_Impl, UIA_PROPERTY_ID,
    };
    use windows_core::implement;

    use super::PropertyCallback;

    /// The COM object implementing the property-change handler.
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
                (self.callback)(element, propertyid.0);
            }
            Ok(())
        }
    }
}

/// A live name/value change registration over a set of top-level windows.
/// Dropping it unregisters and tears down its thread.
pub struct PropertyRegistration {
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl PropertyRegistration {
    /// Registers name/value change handlers over the subtree of each window in
    /// `hwnds`. Returns once registered.
    ///
    /// # Errors
    ///
    /// Returns the COM error if setup or registration fails. Windows whose
    /// element cannot be resolved are skipped rather than failing the whole
    /// registration, so a transient bad handle does not silence the rest.
    pub fn new(hwnds: Vec<isize>, callback: PropertyCallback) -> windows::core::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-props".to_owned())
            .spawn(move || run(hwnds, callback, &ready_tx, &stop_rx))
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
                    "property registration thread ended before signalling readiness",
                ))
            }
        }
    }
}

impl Drop for PropertyRegistration {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn run(
    hwnds: Vec<isize>,
    callback: PropertyCallback,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    stop: &mpsc::Receiver<()>,
) {
    let setup = (|| -> windows::core::Result<(Uia, IUIAutomationPropertyChangedEventHandler)> {
        let uia = Uia::new()?;
        let cache = uia.base_cache_request()?;
        let handler: IUIAutomationPropertyChangedEventHandler = PropertyHandler { callback }.into();
        let properties = WATCHED_PROPERTIES;
        for hwnd in hwnds {
            if let Ok(element) = uia.element_from_handle(hwnd, &cache) {
                // SAFETY: `element`, `cache`, and `handler` are all live and
                // owned by this thread's client; the property slice outlives
                // the call.
                unsafe {
                    let _ = uia.client().AddPropertyChangedEventHandlerNativeArray(
                        &element,
                        TreeScope_Subtree,
                        &cache,
                        &handler,
                        &properties,
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
