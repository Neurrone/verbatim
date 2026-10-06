//! The base cache request attached to every event registration and query, so
//! events and fetches arrive with the M1 [`NodeSnapshot`](verbatim_model::NodeSnapshot)
//! properties prefetched in one cross-process round trip (architecture
//! section 4: "cache requests everywhere").

use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationCacheRequest, UIA_AcceleratorKeyPropertyId,
    UIA_AccessKeyPropertyId, UIA_BoundingRectanglePropertyId, UIA_ClassNamePropertyId,
    UIA_ControlTypePropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_FullDescriptionPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId,
    UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsDataValidForFormPropertyId, UIA_IsDialogPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId, UIA_IsRequiredForFormPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_LevelPropertyId, UIA_NamePropertyId,
    UIA_NativeWindowHandlePropertyId, UIA_PositionInSetPropertyId, UIA_ProcessIdPropertyId,
    UIA_RangeValueValuePropertyId, UIA_SelectionItemIsSelectedPropertyId, UIA_SizeOfSetPropertyId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValueValuePropertyId,
};

/// The properties prefetched for every event and query. Kept in one place
/// so the focus handler, property-change handler, and worker all cache the
/// same set and mapping never faces an unexpectedly absent property.
///
/// The `IsTogglePatternAvailable`, `IsExpandCollapsePatternAvailable`, and
/// `IsSelectionItemPatternAvailable` flags are cached alongside the state
/// values they gate: an element that lacks a pattern still returns a value
/// for its state property (UIA reports a default, e.g.
/// `ToggleState_Indeterminate` for a non-toggle control, or `false` for
/// `SelectionItemIsSelected`), so the value is only meaningful when the
/// pattern is actually available.
///
/// `FullDescription`, `HelpText`, `AccessKey`, `AcceleratorKey`,
/// `PositionInSet`, `SizeOfSet`, `Level`, and `BoundingRectangle` feed
/// [`NodeDetails`](verbatim_model::NodeDetails) (architecture section 3,
/// milestone M3's object-navigation and backend-parity work). The same trap
/// documented above for pattern-gated properties applies to these: UIA
/// reports a default value (an empty string, or zero) for a property an
/// element does not support, so [`crate::map::snapshot_from_cached_element`]
/// maps every default value to `None` rather than trusting it as real data.
///
/// `ClassName` lets the mapping apply rules NVDA keys on a UIA class name,
/// such as the shell's `UIItem` file items reporting no value, and with
/// `IsDialog` decide which windows are dialogs. `IsPassword`,
/// `IsRequiredForForm`, `IsDataValidForForm`, and `ValueIsReadOnly` feed the
/// protected, required, invalid entry, and read-only states;
/// `RangeValueValue` is the value of a control that has no `Value` pattern;
/// `IsContentElement` and `IsControlElement` decide whether an ancestor is
/// content, all as NVDA reads them.
///
/// `IsValuePatternAvailable` and `IsRangeValuePatternAvailable` gate
/// `ValueIsReadOnly` and `RangeValueValue`, like the flags above: a cache
/// filled by a remote operation (`verbatim-uia-rops`) stores a property's
/// default where a locally built cache stores UIA's "not supported" value,
/// so reading those two while ignoring defaults is not enough. Unsupported,
/// their defaults (read-only, and a value of zero) would make every
/// container read-only with a value of "0".
pub const CACHED_PROPERTIES: &[windows::Win32::UI::Accessibility::UIA_PROPERTY_ID] = &[
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
    UIA_IsSelectionItemPatternAvailablePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_FullDescriptionPropertyId,
    UIA_HelpTextPropertyId,
    UIA_AccessKeyPropertyId,
    UIA_AcceleratorKeyPropertyId,
    UIA_PositionInSetPropertyId,
    UIA_SizeOfSetPropertyId,
    UIA_LevelPropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId,
    UIA_IsDialogPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_IsRequiredForFormPropertyId,
    UIA_IsDataValidForFormPropertyId,
    UIA_IsValuePatternAvailablePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_IsContentElementPropertyId,
    UIA_IsControlElementPropertyId,
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
