//! A UIA event handler that reports each event it hears, with the sender's
//! cached name and the time it was heard, and reports when UIA frees it.

#![allow(
    clippy::inline_always,
    clippy::ref_as_ptr,
    clippy::used_underscore_binding,
    non_upper_case_globals,
    // `#[implement]` makes a public `Handler_Impl` it cannot document.
    missing_docs
)]

use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationElement, IUIAutomationEventHandler,
    IUIAutomationEventHandler_Impl, IUIAutomationEventHandlerGroup,
    IUIAutomationPropertyChangedEventHandler, IUIAutomationPropertyChangedEventHandler_Impl,
    TreeScope_Subtree, UIA_AutomationFocusChangedEventId, UIA_EVENT_ID, UIA_NamePropertyId,
    UIA_PROPERTY_ID,
};
use windows::core::{Ref, Result, implement};

/// One event heard.
#[derive(Clone, Debug)]
pub struct Heard {
    /// The sender's cached name, or empty when it has none.
    pub name: String,
    /// The event's id, or the property's for a property change.
    pub id: i32,
    /// When the handler ran, on [`crate::now_us`]'s clock.
    pub at: u64,
}

/// What a [`Handler`] reports: the events it hears, and, once, that UIA
/// has freed it.
pub enum Report {
    /// An event.
    Heard(Heard),
    /// The handler object was freed.
    Freed,
}

/// Work a handler runs on each event's sender, inside the callback.
pub type OnEvent = Box<dyn Fn(&IUIAutomationElement) + Send + Sync>;

/// A handler for automation and property-change events, which reports
/// each event and its own release on a channel.
#[implement(IUIAutomationEventHandler, IUIAutomationPropertyChangedEventHandler)]
pub struct Handler {
    reports: Sender<Report>,
    on_event: Option<OnEvent>,
}

impl Handler {
    /// A handler and the receiver of its reports.
    #[must_use]
    pub fn make() -> (IUIAutomationEventHandler, Receiver<Report>) {
        Self::calling(None)
    }

    /// A handler that also runs `on_event` on each event's sender, inside
    /// the callback, and the receiver of its reports.
    #[must_use]
    pub fn calling(on_event: Option<OnEvent>) -> (IUIAutomationEventHandler, Receiver<Report>) {
        let (reports, received) = channel();
        let handler: IUIAutomationEventHandler = Handler { reports, on_event }.into();
        (handler, received)
    }

    fn heard(&self, sender: Option<&IUIAutomationElement>, id: i32) {
        let at = crate::now_us();
        if let (Some(sender), Some(on_event)) = (sender, &self.on_event) {
            on_event(sender);
        }
        let name = sender
            // SAFETY: a live element UIA passed to the callback.
            .and_then(|s| unsafe { s.CachedName() }.ok())
            .map(|n| n.to_string())
            .unwrap_or_default();
        let _ = self.reports.send(Report::Heard(Heard { name, id, at }));
    }
}

impl Drop for Handler {
    fn drop(&mut self) {
        let _ = self.reports.send(Report::Freed);
    }
}

impl IUIAutomationEventHandler_Impl for Handler_Impl {
    fn HandleAutomationEvent(
        &self,
        sender: Ref<IUIAutomationElement>,
        event: UIA_EVENT_ID,
    ) -> Result<()> {
        self.heard(sender.as_ref(), event.0);
        Ok(())
    }
}

impl IUIAutomationPropertyChangedEventHandler_Impl for Handler_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        sender: Ref<IUIAutomationElement>,
        property: UIA_PROPERTY_ID,
        _value: &VARIANT,
    ) -> Result<()> {
        self.heard(sender.as_ref(), property.0);
        Ok(())
    }
}

/// An event handler group holding UIA's focus event, subtree scope, with
/// the sender's name cached, as e57 and e58 registered it.
///
/// # Errors
///
/// UIA's error making the group.
pub fn focus_group(
    client: &IUIAutomation6,
    handler: &IUIAutomationEventHandler,
) -> Result<IUIAutomationEventHandlerGroup> {
    let request = name_cache(client)?;
    // SAFETY: a live client object.
    let group = unsafe { client.CreateEventHandlerGroup() }?;
    // SAFETY: a live group, cache request and handler.
    unsafe {
        group.AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            TreeScope_Subtree,
            &request,
            handler,
        )
    }?;
    Ok(group)
}

/// A cache request for the name.
///
/// # Errors
///
/// UIA's error.
pub fn name_cache(client: &IUIAutomation6) -> Result<IUIAutomationCacheRequest> {
    crate::uia::cache_request(client, &[UIA_NamePropertyId])
}

/// Registers `group` on `element`.
///
/// # Errors
///
/// UIA's error.
pub fn add_group(
    client: &IUIAutomation6,
    element: &IUIAutomationElement,
    group: &IUIAutomationEventHandlerGroup,
) -> Result<()> {
    // SAFETY: live client object, element and group.
    unsafe { client.AddEventHandlerGroup(element, group) }
}

/// Removes `group` from `element`.
///
/// # Errors
///
/// UIA's error.
pub fn remove_group(
    client: &IUIAutomation6,
    element: &IUIAutomationElement,
    group: &IUIAutomationEventHandlerGroup,
) -> Result<()> {
    // SAFETY: live client object, element and group.
    unsafe { client.RemoveEventHandlerGroup(element, group) }
}

/// Every event in `reports` so far, and whether the handler was freed.
#[must_use]
pub fn drain(reports: &Receiver<Report>) -> (Vec<Heard>, bool) {
    let mut heard = Vec::new();
    let mut freed = false;
    while let Ok(report) = reports.try_recv() {
        match report {
            Report::Heard(h) => heard.push(h),
            Report::Freed => freed = true,
        }
    }
    (heard, freed)
}

/// Casts a handler to the property-change interface it also implements.
///
/// # Panics
///
/// Never for a [`Handler`].
#[must_use]
pub fn as_property_handler(
    handler: &IUIAutomationEventHandler,
) -> IUIAutomationPropertyChangedEventHandler {
    windows::core::Interface::cast(handler).expect("a Handler handles property changes too")
}
