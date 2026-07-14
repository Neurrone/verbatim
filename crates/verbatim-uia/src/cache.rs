//! The base cache request attached to every event registration and query, so
//! events and fetches arrive with the M1 [`NodeSnapshot`](verbatim_model::NodeSnapshot)
//! properties prefetched in one cross-process round trip (architecture
//! section 4: "cache requests everywhere").

use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationCacheRequest, UIA_ControlTypePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_HasKeyboardFocusPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsKeyboardFocusablePropertyId, UIA_IsOffscreenPropertyId,
    UIA_IsTogglePatternAvailablePropertyId, UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId,
    UIA_ProcessIdPropertyId, UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};

/// The properties prefetched for every M1 event and query. Kept in one place
/// so the focus handler, property-change handler, and query pool all cache the
/// same set and mapping never faces an unexpectedly absent property.
///
/// The `IsTogglePatternAvailable` and `IsExpandCollapsePatternAvailable` flags
/// are cached alongside the state values they gate: an element that lacks a
/// pattern still returns a value for its state property (UIA reports a default,
/// e.g. `ToggleState_Indeterminate` for a non-toggle control), so the value is
/// only meaningful when the pattern is actually available.
pub(crate) const CACHED_PROPERTIES: &[windows::Win32::UI::Accessibility::UIA_PROPERTY_ID] = &[
    UIA_NamePropertyId,
    UIA_ControlTypePropertyId,
    UIA_ValueValuePropertyId,
    UIA_ProcessIdPropertyId,
    UIA_NativeWindowHandlePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsTogglePatternAvailablePropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
];

/// Builds the base cache request: every [`CACHED_PROPERTIES`] entry, prefetched
/// so the mapping in [`crate::map`] reads only cached values and never blocks.
///
/// # Errors
///
/// Returns the COM error if the client cannot create or populate the cache
/// request.
pub fn base_cache_request(
    client: &IUIAutomation,
) -> windows::core::Result<IUIAutomationCacheRequest> {
    // SAFETY: `client` is a live IUIAutomation; CreateCacheRequest and
    // AddProperty take only a valid property id and cannot alias.
    unsafe {
        let request = client.CreateCacheRequest()?;
        for &property in CACHED_PROPERTIES {
            request.AddProperty(property)?;
        }
        Ok(request)
    }
}
