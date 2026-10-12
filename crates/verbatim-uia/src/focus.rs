//! The desktop-wide UIA focus-change handler.
//!
//! The handler is registered by a [`crate::Registration`] holding a
//! [`crate::Subscription::Focus`], on the same client as that
//! registration's event handler group, as NVDA registers its focus handler
//! on the client that holds its global group. Under decision D13 the one
//! process that holds it is the focus listener, which watches every
//! application at once and forwards each focus fact to the target's own
//! outpost.

use std::sync::Arc;

use windows::Win32::UI::Accessibility::IUIAutomationElement;

pub(crate) use handler::FocusHandler;

/// Invoked on the UIA callback thread for each desktop-wide focus change.
/// Receives the cached element; the listener reads its cached process id,
/// window handle, and snapshot parts and forwards a focus fact. Must be
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

    use super::FocusCallback;

    /// The COM object implementing the desktop-wide focus handler.
    #[implement(windows::Win32::UI::Accessibility::IUIAutomationFocusChangedEventHandler)]
    pub struct FocusHandler {
        pub callback: FocusCallback,
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for FocusHandler_Impl {
        fn HandleFocusChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            if let Some(element) = sender.as_ref() {
                crate::com::log_arrival("focus", 0, element);
                crate::com::guarded("focus", || (self.callback)(element));
            }
            Ok(())
        }
    }
}
