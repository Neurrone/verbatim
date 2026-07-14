//! Self-contained UIA focus-change registration.
//!
//! This module owns its own thread, apartment, client, and the global
//! `IUIAutomationFocusChangedEventHandler`. Its entire public surface is
//! [`FocusRegistration::new`] — construct with a target-pid filter and a
//! callback — and `Drop`, which unregisters. Keeping it this self-contained is
//! deliberate: the M3 sentinel split moves focus watching into a separate
//! process, and a narrow seam makes that a relocation rather than a rewrite.

use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationFocusChangedEventHandler,
};

use crate::client::Uia;

pub use handler::FocusHandler;

/// Invoked on the UIA callback thread for each focus change whose element
/// belongs to the target process. Receives the cached element; the outpost
/// maps it to a snapshot and applies its arbitration cross-filter. Must be
/// `Send + Sync` because UIA delivers on library-managed apartment threads.
pub type FocusCallback = Arc<dyn Fn(&IUIAutomationElement) + Send + Sync>;

/// The `#[implement]`-generated COM object lives in its own module so the
/// module-level allow covers the macro's generated glue (which uses
/// `#[inline(always)]` and reference-to-raw-pointer casts the pedantic group
/// flags) without loosening the lint for hand-written code.
mod handler {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationFocusChangedEventHandler_Impl,
    };
    use windows_core::implement;

    use crate::map::cached_process_id;

    use super::FocusCallback;

    /// The COM object implementing the focus handler, filtering to a target pid.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationFocusChangedEventHandler)]
    pub struct FocusHandler {
        pub target_pid: u32,
        pub callback: FocusCallback,
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for FocusHandler_Impl {
        fn HandleFocusChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref()
                // SAFETY: the sender element was built with this handler's cache
                // request, so its process id is cached and this read never blocks.
                && unsafe { cached_process_id(element) } == Some(self.target_pid)
            {
                (self.callback)(element);
            }
            Ok(())
        }
    }
}

/// A live global focus-change registration. Dropping it unregisters the
/// handler and tears down its thread and apartment.
pub struct FocusRegistration {
    // Dropping the sender wakes the worker thread out of its `recv`, which is
    // its signal to unregister and exit.
    stop: Option<mpsc::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl FocusRegistration {
    /// Registers a global focus-change handler filtered to `target_pid`,
    /// spawning the thread that owns the apartment and client. Returns once the
    /// handler is registered (or with the error that prevented it).
    ///
    /// # Errors
    ///
    /// Returns the COM error if the apartment, client, cache request, or
    /// handler registration could not be set up.
    pub fn new(target_pid: u32, callback: FocusCallback) -> windows::core::Result<Self> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<windows::core::Result<()>>();
        let join = thread::Builder::new()
            .name("verbatim-uia-focus".to_owned())
            .spawn(move || run(target_pid, callback, &ready_tx, &stop_rx))
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
                    "focus registration thread ended before signalling readiness",
                ))
            }
        }
    }
}

impl Drop for FocusRegistration {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The worker-thread body: set up the apartment, client, and handler, report
/// readiness, then block until asked to stop and unregister.
fn run(
    target_pid: u32,
    callback: FocusCallback,
    ready: &mpsc::Sender<windows::core::Result<()>>,
    stop: &mpsc::Receiver<()>,
) {
    let setup = (|| -> windows::core::Result<(Uia, IUIAutomationFocusChangedEventHandler)> {
        let uia = Uia::new()?;
        let cache = uia.base_cache_request()?;
        let handler: IUIAutomationFocusChangedEventHandler = FocusHandler {
            target_pid,
            callback,
        }
        .into();
        // SAFETY: `cache` and `handler` are live; the client is this thread's
        // own. The handler AddRefs internally, so dropping our `cache` handle
        // after registration is safe.
        unsafe {
            uia.client().AddFocusChangedEventHandler(&cache, &handler)?;
        }
        Ok((uia, handler))
    })();

    match setup {
        Err(error) => {
            let _ = ready.send(Err(error));
        }
        Ok((uia, handler)) => {
            let _ = ready.send(Ok(()));
            // Block until the registration is dropped.
            let _ = stop.recv();
            // SAFETY: unregistering the same handler on the same client thread.
            unsafe {
                let _ = uia.client().RemoveFocusChangedEventHandler(&handler);
            }
            drop(handler);
            drop(uia);
        }
    }
}
