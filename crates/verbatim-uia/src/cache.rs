//! Building a cache request from a list of properties, for the shape
//! module's requests ([`crate::Request`]) and a registration's parts
//! (architecture section 4: "cache requests everywhere"). The lists
//! themselves come from the shapes (`docs/design/uia-shapes.md`), so a
//! request and the read of the elements it fills cannot disagree.

use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationCacheRequest, UIA_PROPERTY_ID,
};

/// Builds a cache request for exactly `properties`. Local.
///
/// # Errors
///
/// Returns the COM error if the client cannot create or populate the cache
/// request.
pub(crate) fn cache_request(
    client: &IUIAutomation,
    properties: &[UIA_PROPERTY_ID],
) -> windows::core::Result<IUIAutomationCacheRequest> {
    // SAFETY: `client` is a live IUIAutomation; CreateCacheRequest takes no
    // arguments.
    let request = unsafe { client.CreateCacheRequest() }?;
    for &property in properties {
        // SAFETY: `request` was just created; a property id is a plain value.
        unsafe { request.AddProperty(property) }?;
    }
    Ok(request)
}
