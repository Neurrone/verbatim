//! Mapping from UIA control types and cached properties into the normalized
//! model (architecture section 3). The role table is pure and unit-tested;
//! [`snapshot_from_cached_element`] reads only cached values, so it never makes
//! a cross-process call and is safe to run on a UIA event-callback thread.

use verbatim_model::{Backend, NodeDetails, NodeSnapshot, Rect, Role, State, StateSet};
use windows::Win32::UI::Accessibility::{
    ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded, IUIAutomationElement,
    NotificationKind_ActionAborted, NotificationKind_ActionCompleted, NotificationKind_ItemAdded,
    NotificationKind_ItemRemoved, NotificationProcessing_All,
    NotificationProcessing_CurrentThenMostRecent, NotificationProcessing_ImportantMostRecent,
    NotificationProcessing_MostRecent, ToggleState_Indeterminate, ToggleState_On,
    UIA_AcceleratorKeyPropertyId,
    UIA_AccessKeyPropertyId, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_FullDescriptionPropertyId, UIA_GroupControlTypeId, UIA_HasKeyboardFocusPropertyId,
    UIA_HelpTextPropertyId, UIA_HyperlinkControlTypeId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsTogglePatternAvailablePropertyId, UIA_LevelPropertyId,
    UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId,
    UIA_MenuControlTypeId, UIA_MenuItemControlTypeId, UIA_NamePropertyId,
    UIA_NativeWindowHandlePropertyId, UIA_PaneControlTypeId, UIA_PositionInSetPropertyId,
    UIA_ProcessIdPropertyId, UIA_RadioButtonControlTypeId, UIA_SizeOfSetPropertyId,
    UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_StatusBarControlTypeId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TextControlTypeId,
    UIA_ToggleToggleStatePropertyId, UIA_ToolBarControlTypeId, UIA_ValueValuePropertyId,
    UIA_WindowControlTypeId,
};

use crate::com::{variant_bool, variant_i32, variant_string};
use crate::registry::NodeIdRegistry;

/// Maps a UIA control-type id to a normalized [`Role`]. Unmapped types become
/// [`Role::Unknown`] so new UIA controls degrade rather than mislead.
#[must_use]
pub fn role_from_control_type(control_type: i32) -> Role {
    match control_type {
        t if t == UIA_ButtonControlTypeId.0 => Role::Button,
        t if t == UIA_CheckBoxControlTypeId.0 => Role::CheckBox,
        t if t == UIA_ComboBoxControlTypeId.0 => Role::ComboBox,
        t if t == UIA_EditControlTypeId.0 || t == UIA_DocumentControlTypeId.0 => Role::EditableText,
        t if t == UIA_SliderControlTypeId.0 => Role::Slider,
        t if t == UIA_SpinnerControlTypeId.0 => Role::SpinButton,
        t if t == UIA_ListControlTypeId.0 => Role::List,
        t if t == UIA_ListItemControlTypeId.0 => Role::ListItem,
        t if t == UIA_MenuControlTypeId.0 => Role::Menu,
        t if t == UIA_MenuBarControlTypeId.0 => Role::MenuBar,
        t if t == UIA_MenuItemControlTypeId.0 => Role::MenuItem,
        t if t == UIA_WindowControlTypeId.0 => Role::Window,
        t if t == UIA_TextControlTypeId.0 => Role::StaticText,
        t if t == UIA_TabControlTypeId.0 => Role::TabControl,
        t if t == UIA_TabItemControlTypeId.0 => Role::Tab,
        t if t == UIA_HyperlinkControlTypeId.0 => Role::Link,
        t if t == UIA_ToolBarControlTypeId.0 => Role::ToolBar,
        t if t == UIA_StatusBarControlTypeId.0 => Role::StatusBar,
        t if t == UIA_GroupControlTypeId.0 => Role::Group,
        t if t == UIA_PaneControlTypeId.0 => Role::Pane,
        t if t == UIA_RadioButtonControlTypeId.0 => Role::RadioButton,
        _ => Role::Unknown,
    }
}

/// Reads a cached property as a `VARIANT`. Returns `None` when the property was
/// not cached or is unsupported (UIA returns a reserved sentinel value).
///
/// # Safety
///
/// `element` must be a live element built with a cache request that included
/// `property`.
unsafe fn cached_i32(element: &IUIAutomationElement, property: i32) -> Option<i32> {
    use windows::Win32::UI::Accessibility::UIA_PROPERTY_ID;
    // SAFETY: forwarded to the caller's contract; the returned VARIANT is
    // borrowed only for the extraction call.
    unsafe {
        let value = element
            .GetCachedPropertyValue(UIA_PROPERTY_ID(property))
            .ok()?;
        variant_i32(&value)
    }
}

/// Reads a cached boolean property, defaulting to `false`.
///
/// # Safety
///
/// `element` must be a live element built with a cache request that included
/// `property`.
unsafe fn cached_bool(element: &IUIAutomationElement, property: i32) -> bool {
    use windows::Win32::UI::Accessibility::UIA_PROPERTY_ID;
    // SAFETY: forwarded to the caller's contract.
    unsafe {
        element
            .GetCachedPropertyValue(UIA_PROPERTY_ID(property))
            .ok()
            .is_some_and(|value| variant_bool(&value))
    }
}

/// Reads a cached string property, `None` when empty or absent.
///
/// # Safety
///
/// `element` must be a live element built with a cache request that included
/// `property`.
unsafe fn cached_string(element: &IUIAutomationElement, property: i32) -> Option<String> {
    use windows::Win32::UI::Accessibility::UIA_PROPERTY_ID;
    // SAFETY: forwarded to the caller's contract.
    unsafe {
        let value = element
            .GetCachedPropertyValue(UIA_PROPERTY_ID(property))
            .ok()?;
        variant_string(&value)
    }
}

/// The raw cached inputs to the UIA state mapping, separated from the element
/// so the mapping logic ([`states_from_uia`]) is pure and unit-testable. The
/// several booleans are the whole point — each is one cached UIA flag — so the
/// "too many bools" lint does not apply.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy)]
struct RawUiaStates {
    has_focus: bool,
    focusable: bool,
    enabled: bool,
    offscreen: bool,
    /// Whether the element actually exposes `TogglePattern`. UIA returns a
    /// default `ToggleState` (`Indeterminate`) for controls that do not, so the
    /// state value must be ignored unless the pattern is available.
    toggle_available: bool,
    toggle_state: Option<i32>,
    /// Whether the element actually exposes `ExpandCollapsePattern`.
    expand_available: bool,
    expand_state: Option<i32>,
}

/// Pure mapping from raw cached UIA state inputs to a normalized [`StateSet`].
/// Toggle and expand values are honored only when their pattern is available,
/// so a non-toggle control's default `ToggleState_Indeterminate` never becomes
/// a spurious [`State::Mixed`].
fn states_from_uia(raw: &RawUiaStates) -> StateSet {
    let mut states = StateSet::new();
    if raw.has_focus {
        states.insert(State::Focused);
    }
    if raw.focusable {
        states.insert(State::Focusable);
    }
    if !raw.enabled {
        states.insert(State::Disabled);
    }
    if raw.offscreen {
        states.insert(State::Offscreen);
    }
    if raw.toggle_available {
        if raw.toggle_state == Some(ToggleState_On.0) {
            states.insert(State::Checked);
        } else if raw.toggle_state == Some(ToggleState_Indeterminate.0) {
            states.insert(State::Mixed);
        }
    }
    if raw.expand_available {
        if raw.expand_state == Some(ExpandCollapseState_Expanded.0) {
            states.insert(State::Expanded);
        } else if raw.expand_state == Some(ExpandCollapseState_Collapsed.0) {
            states.insert(State::Collapsed);
        }
    }
    states
}

/// Derives the normalized [`StateSet`] from an element's cached properties.
///
/// # Safety
///
/// `element` must be a live element built with the base cache request.
unsafe fn states_from_cached(element: &IUIAutomationElement) -> StateSet {
    // SAFETY: every property below is in the base cache request; each read is
    // forwarded to the cached_* helpers' contract.
    let raw = unsafe {
        RawUiaStates {
            has_focus: cached_bool(element, UIA_HasKeyboardFocusPropertyId.0),
            focusable: cached_bool(element, UIA_IsKeyboardFocusablePropertyId.0),
            enabled: cached_bool(element, UIA_IsEnabledPropertyId.0),
            offscreen: cached_bool(element, UIA_IsOffscreenPropertyId.0),
            toggle_available: cached_bool(element, UIA_IsTogglePatternAvailablePropertyId.0),
            toggle_state: cached_i32(element, UIA_ToggleToggleStatePropertyId.0),
            expand_available: cached_bool(
                element,
                UIA_IsExpandCollapsePatternAvailablePropertyId.0,
            ),
            expand_state: cached_i32(element, UIA_ExpandCollapseExpandCollapseStatePropertyId.0),
        }
    };
    states_from_uia(&raw)
}

/// Reads a cached one-based property (`PositionInSet`, `SizeOfSet`, `Level`)
/// as `None` when UIA reports its "not supported" default of zero or
/// negative — the same trap [`cached_bool`]'s doc comment on pattern
/// availability describes, applied here to plain integer properties instead
/// of pattern-gated ones: UIA returns a default value for a property an
/// element does not support rather than an error, and every one of these
/// properties is documented as one-based when it is genuinely reported.
///
/// # Safety
///
/// `element` must be a live element built with a cache request that included
/// `property`.
unsafe fn cached_one_based(element: &IUIAutomationElement, property: i32) -> Option<u32> {
    // SAFETY: forwarded to the caller's contract.
    unsafe { cached_i32(element, property) }.and_then(|value| u32::try_from(value).ok())
}

/// Reads the cached `BoundingRectangle` as a [`Rect`], `None` when UIA
/// reports its "not supported" default of an all-zero rectangle (the same
/// trap as every other cached property here) or the read fails outright —
/// an element with a genuine zero-area rectangle is not a case Verbatim's
/// positional-audio consumer (milestone M11) needs to distinguish from
/// "unreported".
///
/// # Safety
///
/// `element` must be a live element built with a cache request that included
/// [`windows::Win32::UI::Accessibility::UIA_BoundingRectanglePropertyId`].
unsafe fn cached_rect(element: &IUIAutomationElement) -> Option<Rect> {
    // SAFETY: forwarded to the caller's contract; `CachedBoundingRectangle`
    // reads the same cached property `UIA_BoundingRectanglePropertyId` names,
    // through UIA's dedicated typed accessor rather than a generic VARIANT.
    let rect = unsafe { element.CachedBoundingRectangle() }.ok()?;
    if rect.left == 0 && rect.top == 0 && rect.right == 0 && rect.bottom == 0 {
        return None;
    }
    Some(Rect {
        left: rect.left,
        top: rect.top,
        width: rect.right - rect.left,
        height: rect.bottom - rect.top,
    })
}

/// Builds a [`NodeDetails`] from an element's cached properties: description
/// (`FullDescription`, falling back to `HelpText`), keyboard shortcut
/// (`AccessKey`, falling back to `AcceleratorKey`), `PositionInSet`,
/// `SizeOfSet`, `Level`, and `BoundingRectangle`. Every field maps UIA's
/// "not supported" default (an empty string or zero) to `None`.
///
/// # Safety
///
/// `element` must be a live element built with the base cache request.
unsafe fn details_from_cached(element: &IUIAutomationElement) -> NodeDetails {
    // SAFETY: every property below is in the base cache request; each read is
    // forwarded to the cached_* helpers' contract.
    unsafe {
        let description = cached_string(element, UIA_FullDescriptionPropertyId.0)
            .or_else(|| cached_string(element, UIA_HelpTextPropertyId.0));
        let keyboard_shortcut = cached_string(element, UIA_AccessKeyPropertyId.0)
            .or_else(|| cached_string(element, UIA_AcceleratorKeyPropertyId.0));
        NodeDetails {
            description,
            keyboard_shortcut,
            position_in_set: cached_one_based(element, UIA_PositionInSetPropertyId.0),
            set_size: cached_one_based(element, UIA_SizeOfSetPropertyId.0),
            level: cached_one_based(element, UIA_LevelPropertyId.0),
            rect: cached_rect(element),
        }
    }
}

/// Reads the cached process id, so callers can filter events by target pid
/// without a cross-process call.
///
/// # Safety
///
/// `element` must be a live element built with the base cache request.
#[must_use]
pub unsafe fn cached_process_id(element: &IUIAutomationElement) -> Option<u32> {
    // SAFETY: forwarded to `cached_i32`'s contract.
    unsafe { cached_i32(element, UIA_ProcessIdPropertyId.0).map(i32::cast_unsigned) }
}

/// Reads the cached native window handle (0 when the element is not itself a
/// window), used by the outpost arbitration cross-filter.
///
/// # Safety
///
/// `element` must be a live element built with the base cache request.
#[must_use]
pub unsafe fn cached_native_window_handle(element: &IUIAutomationElement) -> isize {
    // SAFETY: forwarded to `cached_i32`'s contract; the handle is stored as an
    // integer property.
    unsafe { cached_i32(element, UIA_NativeWindowHandlePropertyId.0).unwrap_or(0) as isize }
}

/// Builds a [`NodeSnapshot`] from a cached UIA element, minting or reusing its
/// [`NodeId`](verbatim_model::NodeId) via `registry`. Reads only cached values,
/// so it is safe on an event-callback thread.
///
/// # Safety
///
/// `element` must be a live element built with [`crate::cache::base_cache_request`].
#[must_use]
pub unsafe fn snapshot_from_cached_element(
    element: &IUIAutomationElement,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    // SAFETY: `element` was built with the base cache request per the contract,
    // so GetRuntimeId and every cached read below are satisfied. The runtime-id
    // SAFEARRAY is consumed by `take_i32_safearray`.
    unsafe {
        let runtime_id = element
            .GetRuntimeId()
            .map(|array| crate::com::take_i32_safearray(array))
            .unwrap_or_default();
        let control_type = cached_i32(element, UIA_ControlTypePropertyId.0).unwrap_or(0);
        NodeSnapshot {
            id: registry.id_for(&runtime_id),
            backend: Backend::Uia,
            role: role_from_control_type(control_type),
            name: cached_string(element, UIA_NamePropertyId.0),
            value: cached_string(element, UIA_ValueValuePropertyId.0),
            states: states_from_cached(element),
            details: details_from_cached(element),
        }
    }
}

/// Maps a UIA `NotificationKind` to the normalized
/// [`verbatim_model::NotificationKind`]. UIA's enum has no "unknown" value —
/// every one of its five members maps directly — so this is total, unlike
/// the role and state tables above.
///
/// Compared by equality rather than matched by pattern, like every other
/// UIA constant-as-enum value in this module (`ToggleState_On` and friends
/// above): these are plain `const`s of a tuple-struct type, not real enum
/// variants, and matching them by pattern name trips rustc's
/// `non_upper_case_globals` lint.
#[must_use]
pub fn notification_kind_from_uia(
    kind: windows::Win32::UI::Accessibility::NotificationKind,
) -> verbatim_model::NotificationKind {
    if kind == NotificationKind_ItemAdded {
        verbatim_model::NotificationKind::ItemAdded
    } else if kind == NotificationKind_ItemRemoved {
        verbatim_model::NotificationKind::ItemRemoved
    } else if kind == NotificationKind_ActionCompleted {
        verbatim_model::NotificationKind::ActionCompleted
    } else if kind == NotificationKind_ActionAborted {
        verbatim_model::NotificationKind::ActionAborted
    } else {
        verbatim_model::NotificationKind::Other
    }
}

/// Maps a UIA `NotificationProcessing` to the normalized
/// [`verbatim_model::NotificationProcessing`]. Defaults to `ImportantAll`
/// (the most conservative choice — process everything) for
/// `NotificationProcessing_ImportantAll` itself and for any value outside
/// UIA's documented five, which is not expected in practice. See
/// [`notification_kind_from_uia`] for why this compares by equality rather
/// than matching by pattern.
#[must_use]
pub fn notification_processing_from_uia(
    processing: windows::Win32::UI::Accessibility::NotificationProcessing,
) -> verbatim_model::NotificationProcessing {
    if processing == NotificationProcessing_ImportantMostRecent {
        verbatim_model::NotificationProcessing::ImportantMostRecent
    } else if processing == NotificationProcessing_All {
        verbatim_model::NotificationProcessing::All
    } else if processing == NotificationProcessing_MostRecent {
        verbatim_model::NotificationProcessing::MostRecent
    } else if processing == NotificationProcessing_CurrentThenMostRecent {
        verbatim_model::NotificationProcessing::CurrentThenMostRecent
    } else {
        verbatim_model::NotificationProcessing::ImportantAll
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_control_types_map_to_expected_roles() {
        assert_eq!(
            role_from_control_type(UIA_ButtonControlTypeId.0),
            Role::Button
        );
        assert_eq!(
            role_from_control_type(UIA_CheckBoxControlTypeId.0),
            Role::CheckBox
        );
        assert_eq!(
            role_from_control_type(UIA_EditControlTypeId.0),
            Role::EditableText
        );
        assert_eq!(
            role_from_control_type(UIA_MenuItemControlTypeId.0),
            Role::MenuItem
        );
        assert_eq!(
            role_from_control_type(UIA_TabItemControlTypeId.0),
            Role::Tab
        );
        assert_eq!(
            role_from_control_type(UIA_TabControlTypeId.0),
            Role::TabControl
        );
    }

    #[test]
    fn unmapped_control_type_is_unknown() {
        assert_eq!(role_from_control_type(-1), Role::Unknown);
        assert_eq!(role_from_control_type(999_999), Role::Unknown);
    }

    /// A focused, non-toggle element (e.g. a Pane or menu/list item) reports a
    /// default `ToggleState_Indeterminate` even though it has no `TogglePattern`.
    /// Gating on availability keeps that from becoming a spurious `Mixed`.
    #[test]
    fn unavailable_toggle_pattern_never_yields_mixed() {
        let raw = RawUiaStates {
            has_focus: true,
            focusable: true,
            enabled: true,
            offscreen: false,
            toggle_available: false,
            toggle_state: Some(ToggleState_Indeterminate.0),
            expand_available: false,
            expand_state: Some(3), // LeafNode default for non-expandable elements.
        };
        let states = states_from_uia(&raw);
        assert!(states.contains(State::Focused));
        assert!(states.contains(State::Focusable));
        assert!(
            !states.contains(State::Mixed),
            "a non-toggle element must not be reported as half-checked"
        );
        assert!(!states.contains(State::Expanded));
        assert!(!states.contains(State::Collapsed));
    }

    /// When the pattern is available, the toggle value is honored.
    #[test]
    fn available_toggle_pattern_maps_toggle_state() {
        let base = RawUiaStates {
            has_focus: false,
            focusable: true,
            enabled: true,
            offscreen: false,
            toggle_available: true,
            toggle_state: Some(ToggleState_On.0),
            expand_available: false,
            expand_state: None,
        };
        assert!(states_from_uia(&base).contains(State::Checked));

        let indeterminate = RawUiaStates {
            toggle_state: Some(ToggleState_Indeterminate.0),
            ..base
        };
        assert!(states_from_uia(&indeterminate).contains(State::Mixed));

        let off = RawUiaStates {
            toggle_state: Some(0),
            ..base
        };
        let off_states = states_from_uia(&off);
        assert!(!off_states.contains(State::Checked));
        assert!(!off_states.contains(State::Mixed));
    }

    #[test]
    fn available_expand_pattern_maps_expand_state() {
        let expanded = RawUiaStates {
            has_focus: false,
            focusable: true,
            enabled: true,
            offscreen: false,
            toggle_available: false,
            toggle_state: None,
            expand_available: true,
            expand_state: Some(ExpandCollapseState_Expanded.0),
        };
        assert!(states_from_uia(&expanded).contains(State::Expanded));
        let collapsed = RawUiaStates {
            expand_state: Some(ExpandCollapseState_Collapsed.0),
            ..expanded
        };
        assert!(states_from_uia(&collapsed).contains(State::Collapsed));
    }

    #[test]
    fn disabled_when_not_enabled() {
        let raw = RawUiaStates {
            has_focus: false,
            focusable: false,
            enabled: false,
            offscreen: true,
            toggle_available: false,
            toggle_state: None,
            expand_available: false,
            expand_state: None,
        };
        let states = states_from_uia(&raw);
        assert!(states.contains(State::Disabled));
        assert!(states.contains(State::Offscreen));
    }

    #[test]
    fn notification_kinds_map_one_to_one() {
        use windows::Win32::UI::Accessibility::{
            NotificationKind_ActionAborted, NotificationKind_ActionCompleted,
            NotificationKind_ItemAdded, NotificationKind_ItemRemoved, NotificationKind_Other,
        };
        assert_eq!(
            notification_kind_from_uia(NotificationKind_ItemAdded),
            verbatim_model::NotificationKind::ItemAdded
        );
        assert_eq!(
            notification_kind_from_uia(NotificationKind_ItemRemoved),
            verbatim_model::NotificationKind::ItemRemoved
        );
        assert_eq!(
            notification_kind_from_uia(NotificationKind_ActionCompleted),
            verbatim_model::NotificationKind::ActionCompleted
        );
        assert_eq!(
            notification_kind_from_uia(NotificationKind_ActionAborted),
            verbatim_model::NotificationKind::ActionAborted
        );
        assert_eq!(
            notification_kind_from_uia(NotificationKind_Other),
            verbatim_model::NotificationKind::Other
        );
    }

    #[test]
    fn notification_processing_maps_every_documented_value() {
        use windows::Win32::UI::Accessibility::{
            NotificationProcessing_All, NotificationProcessing_CurrentThenMostRecent,
            NotificationProcessing_ImportantAll, NotificationProcessing_ImportantMostRecent,
            NotificationProcessing_MostRecent,
        };
        assert_eq!(
            notification_processing_from_uia(NotificationProcessing_ImportantAll),
            verbatim_model::NotificationProcessing::ImportantAll
        );
        assert_eq!(
            notification_processing_from_uia(NotificationProcessing_ImportantMostRecent),
            verbatim_model::NotificationProcessing::ImportantMostRecent
        );
        assert_eq!(
            notification_processing_from_uia(NotificationProcessing_All),
            verbatim_model::NotificationProcessing::All
        );
        assert_eq!(
            notification_processing_from_uia(NotificationProcessing_MostRecent),
            verbatim_model::NotificationProcessing::MostRecent
        );
        assert_eq!(
            notification_processing_from_uia(NotificationProcessing_CurrentThenMostRecent),
            verbatim_model::NotificationProcessing::CurrentThenMostRecent
        );
    }
}
