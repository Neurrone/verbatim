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
    UIA_AcceleratorKeyPropertyId, UIA_AccessKeyPropertyId, UIA_AppBarControlTypeId,
    UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_CalendarControlTypeId,
    UIA_CheckBoxControlTypeId, UIA_ClassNamePropertyId, UIA_ComboBoxControlTypeId,
    UIA_ControlTypePropertyId, UIA_CustomControlTypeId, UIA_DataGridControlTypeId,
    UIA_DataItemControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_FullDescriptionPropertyId,
    UIA_GroupControlTypeId, UIA_HasKeyboardFocusPropertyId, UIA_HeaderControlTypeId,
    UIA_HeaderItemControlTypeId, UIA_HelpTextPropertyId, UIA_HyperlinkControlTypeId,
    UIA_ImageControlTypeId, UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsDataValidForFormPropertyId, UIA_IsDialogPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId, UIA_IsRequiredForFormPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTogglePatternAvailablePropertyId,
    UIA_IsValuePatternAvailablePropertyId, UIA_LegacyIAccessibleStatePropertyId,
    UIA_LevelPropertyId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId, UIA_NamePropertyId,
    UIA_NativeWindowHandlePropertyId, UIA_PROPERTY_ID, UIA_PaneControlTypeId,
    UIA_PositionInSetPropertyId, UIA_ProcessIdPropertyId, UIA_ProgressBarControlTypeId,
    UIA_RadioButtonControlTypeId, UIA_RangeValueValuePropertyId, UIA_ScrollBarControlTypeId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SeparatorControlTypeId, UIA_SizeOfSetPropertyId,
    UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_SplitButtonControlTypeId,
    UIA_StatusBarControlTypeId, UIA_TabControlTypeId, UIA_TabItemControlTypeId,
    UIA_TableControlTypeId, UIA_TextControlTypeId, UIA_ThumbControlTypeId,
    UIA_TitleBarControlTypeId, UIA_ToggleToggleStatePropertyId, UIA_ToolBarControlTypeId,
    UIA_ToolTipControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId,
    UIA_ValueIsReadOnlyPropertyId, UIA_ValueValuePropertyId, UIA_WindowControlTypeId,
};

use crate::element::ElementExt;
use crate::registry::NodeIdRegistry;

/// Maps a UIA control-type id to a normalized [`Role`], as NVDA's UIA
/// control type table does. Unmapped types become [`Role::Unknown`] so new
/// UIA controls degrade rather than mislead.
#[must_use]
pub fn role_from_control_type(control_type: i32) -> Role {
    const ROLES: [(UIA_CONTROLTYPE_ID, Role); 40] = [
        (UIA_ButtonControlTypeId, Role::Button),
        (UIA_CalendarControlTypeId, Role::Calendar),
        (UIA_CheckBoxControlTypeId, Role::CheckBox),
        (UIA_ComboBoxControlTypeId, Role::ComboBox),
        (UIA_EditControlTypeId, Role::EditableText),
        (UIA_HyperlinkControlTypeId, Role::Link),
        (UIA_ImageControlTypeId, Role::Graphic),
        (UIA_ListItemControlTypeId, Role::ListItem),
        (UIA_ListControlTypeId, Role::List),
        (UIA_MenuControlTypeId, Role::Menu),
        (UIA_MenuBarControlTypeId, Role::MenuBar),
        (UIA_MenuItemControlTypeId, Role::MenuItem),
        (UIA_ProgressBarControlTypeId, Role::ProgressBar),
        (UIA_RadioButtonControlTypeId, Role::RadioButton),
        (UIA_ScrollBarControlTypeId, Role::ScrollBar),
        (UIA_SliderControlTypeId, Role::Slider),
        (UIA_SpinnerControlTypeId, Role::SpinButton),
        (UIA_StatusBarControlTypeId, Role::StatusBar),
        (UIA_TabControlTypeId, Role::TabControl),
        (UIA_TabItemControlTypeId, Role::Tab),
        (UIA_TextControlTypeId, Role::StaticText),
        (UIA_ToolBarControlTypeId, Role::ToolBar),
        (UIA_ToolTipControlTypeId, Role::ToolTip),
        (UIA_TreeControlTypeId, Role::Tree),
        (UIA_TreeItemControlTypeId, Role::TreeItem),
        (UIA_CustomControlTypeId, Role::Unknown),
        (UIA_GroupControlTypeId, Role::Group),
        (UIA_ThumbControlTypeId, Role::Thumb),
        (UIA_DataGridControlTypeId, Role::DataGrid),
        (UIA_DataItemControlTypeId, Role::DataItem),
        (UIA_DocumentControlTypeId, Role::Document),
        (UIA_SplitButtonControlTypeId, Role::SplitButton),
        (UIA_WindowControlTypeId, Role::Window),
        (UIA_PaneControlTypeId, Role::Pane),
        (UIA_HeaderControlTypeId, Role::Header),
        (UIA_HeaderItemControlTypeId, Role::HeaderItem),
        (UIA_TableControlTypeId, Role::Table),
        (UIA_TitleBarControlTypeId, Role::TitleBar),
        (UIA_SeparatorControlTypeId, Role::Separator),
        (UIA_AppBarControlTypeId, Role::Unknown),
    ];
    ROLES
        .iter()
        .find(|(id, _)| id.0 == control_type)
        .map_or(Role::Unknown, |(_, role)| *role)
}

/// The UIA class names of windows NVDA treats as dialogs even when the
/// element does not say `IsDialog`.
const DIALOG_CLASS_NAMES: [&str; 6] = [
    "#32770",
    "NUIDialog",
    "Credential Dialog Xaml Host",
    "Shell_Dialog",
    "Shell_Flyout",
    "Shell_SystemDialog",
];

/// Whether an element is a dialog, as NVDA decides: it says so through
/// `IsDialog`, or it is a window element whose class is one of
/// [`DIALOG_CLASS_NAMES`].
fn is_dialog(is_dialog: bool, is_window_element: bool, class_name: Option<&str>) -> bool {
    is_dialog
        || (is_window_element
            && class_name.is_some_and(|class| DIALOG_CLASS_NAMES.contains(&class)))
}

/// Refines a Button control type's role using `TogglePattern` availability;
/// every other role passes through unchanged. Mirrors NVDA's `_get_role`: a
/// Button element that supports the Toggle pattern is a toggle button (NVDA:
/// role BUTTON plus a supported `UIA_ToggleToggleStatePropertyId` becomes
/// TOGGLEBUTTON).
#[must_use]
pub fn refine_button_role(role: Role, toggle_available: bool) -> Role {
    if role == Role::Button && toggle_available {
        Role::ToggleButton
    } else {
        role
    }
}

/// Whether UIA considers an element both a control and content, which NVDA
/// requires of a UIA element before it counts as content, and so as focus
/// context. Reads the base cache request's properties.
#[must_use]
pub fn cached_is_control_and_content(element: &IUIAutomationElement) -> bool {
    element.cached_bool(UIA_IsContentElementPropertyId)
        && element.cached_bool(UIA_IsControlElementPropertyId)
}

/// The raw cached inputs to the UIA state mapping, separated from the element
/// so the mapping logic ([`states_from_uia`]) is pure and unit-testable. The
/// several booleans are the whole point — each is one cached UIA flag — so the
/// "too many bools" lint does not apply.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Default)]
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
    /// Whether the element actually exposes `SelectionItemPattern` — which
    /// is also what makes it [`State::Selectable`], mirroring how MSAA's
    /// `STATE_SYSTEM_SELECTABLE` bit reads.
    selection_available: bool,
    selected: bool,
    password: bool,
    required: bool,
    /// `IsDataValidForForm`, `None` when unsupported, which counts as valid.
    data_valid: Option<bool>,
    value_read_only: bool,
    /// The element's MSAA state bits through UIA's `LegacyIAccessible`
    /// pattern, `None` when unsupported or only UIA's default.
    legacy_state: Option<i32>,
}

/// MSAA's `STATE_SYSTEM_CHECKED` bit, as `LegacyIAccessibleState` reports it.
const LEGACY_STATE_CHECKED: i32 = 0x10;

/// Pure mapping from raw cached UIA state inputs to a normalized [`StateSet`].
/// Toggle and expand values are honored only when their pattern is available,
/// so a non-toggle control's default `ToggleState_Indeterminate` never becomes
/// a spurious [`State::Mixed`].
///
/// `role` is the already-resolved (see [`refine_button_role`]) role of the
/// node the states belong to: `ToggleState_On` becomes [`State::Pressed`]
/// for a [`Role::ToggleButton`] and [`State::Checked`] for everything else,
/// mirroring NVDA's toggle-state branch. A toggleable element that is not a
/// check box or toggle button (a list item or menu item with a check box)
/// is [`State::Checkable`], and a radio button's selection is its checked
/// state, as in NVDA.
fn states_from_uia(raw: &RawUiaStates, role: Role) -> StateSet {
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
        if !matches!(role, Role::ToggleButton | Role::CheckBox) {
            states.insert(State::Checkable);
        }
        if raw.toggle_state == Some(ToggleState_On.0) {
            let on_state = if role == Role::ToggleButton {
                State::Pressed
            } else {
                State::Checked
            };
            states.insert(on_state);
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
    if raw.selection_available {
        let (can, is) = if role == Role::RadioButton {
            (State::Checkable, State::Checked)
        } else {
            (State::Selectable, State::Selected)
        };
        states.insert(can);
        if raw.selected {
            states.insert(is);
        }
    }
    if raw.password {
        states.insert(State::Protected);
    }
    if raw.required {
        states.insert(State::Required);
    }
    if raw.data_valid == Some(false) {
        states.insert(State::InvalidEntry);
    }
    if raw.value_read_only {
        states.insert(State::ReadOnly);
    }
    // A menu item that UIA's patterns do not make checkable can still say
    // it is checked through its legacy MSAA state, as Windows Forms menu
    // items do; NVDA 2027.1 reads it there for a menu item.
    if role == Role::MenuItem
        && !states.contains(State::Checkable)
        && raw
            .legacy_state
            .is_some_and(|bits| bits & LEGACY_STATE_CHECKED != 0)
    {
        states.insert(State::Checkable);
        states.insert(State::Checked);
    }
    states
}

/// Derives the normalized [`StateSet`] from an element's cached properties,
/// those of the base cache request. `role` is the element's
/// already-resolved role (see [`refine_button_role`]), which the
/// toggle-state mapping needs to pick between [`State::Pressed`] and
/// [`State::Checked`].
fn states_from_cached(element: &IUIAutomationElement, role: Role) -> StateSet {
    let raw = RawUiaStates {
        has_focus: element.cached_bool(UIA_HasKeyboardFocusPropertyId),
        focusable: element.cached_bool(UIA_IsKeyboardFocusablePropertyId),
        enabled: element.cached_bool(UIA_IsEnabledPropertyId),
        offscreen: element.cached_bool(UIA_IsOffscreenPropertyId),
        toggle_available: element.cached_bool(UIA_IsTogglePatternAvailablePropertyId),
        toggle_state: element.cached_i32(UIA_ToggleToggleStatePropertyId),
        expand_available: element.cached_bool(UIA_IsExpandCollapsePatternAvailablePropertyId),
        expand_state: element.cached_i32(UIA_ExpandCollapseExpandCollapseStatePropertyId),
        selection_available: element.cached_bool(UIA_IsSelectionItemPatternAvailablePropertyId),
        selected: element.cached_bool(UIA_SelectionItemIsSelectedPropertyId),
        password: element.cached_bool(UIA_IsPasswordPropertyId),
        required: element.cached_bool(UIA_IsRequiredForFormPropertyId),
        data_valid: element.cached_optional_bool(UIA_IsDataValidForFormPropertyId),
        // Gated on the pattern, as a remotely filled cache stores the
        // property's default of true where the pattern is missing.
        value_read_only: element.cached_bool(UIA_IsValuePatternAvailablePropertyId)
            && element.cached_optional_bool(UIA_ValueIsReadOnlyPropertyId) == Some(true),
        legacy_state: element.cached_i32_ignoring_default(UIA_LegacyIAccessibleStatePropertyId),
    };
    states_from_uia(&raw, role)
}

/// Reads a cached one-based property (`PositionInSet`, `SizeOfSet`, `Level`)
/// as `None` when UIA reports its "not supported" default of zero or
/// negative — the same trap the pattern-availability flags guard against
/// (see [`crate::CACHED_PROPERTIES`]), applied here to plain integer
/// properties instead of pattern-gated ones: UIA returns a default value for
/// a property an element does not support rather than an error, and every
/// one of these properties is documented as one-based when it is genuinely
/// reported.
fn cached_one_based(element: &IUIAutomationElement, property: UIA_PROPERTY_ID) -> Option<u32> {
    element
        .cached_i32(property)
        .and_then(|value| u32::try_from(value).ok())
        // Zero is UIA's "not supported" default for these one-based
        // properties, observed live on an hwnd-hosted root element, where
        // the host provider answers 0 rather than leaving the variant empty.
        .filter(|&value| value > 0)
}

/// Reads the cached `BoundingRectangle` as a [`Rect`], `None` when UIA
/// reports its "not supported" default of an all-zero rectangle (the same
/// trap as every other cached property here) or the read fails outright —
/// an element with a genuine zero-area rectangle is not a case Verbatim's
/// positional-audio consumer (milestone M11) needs to distinguish from
/// "unreported".
fn cached_rect(element: &IUIAutomationElement) -> Option<Rect> {
    let rect = element.cached_bounding_rectangle()?;
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
/// (`AccessKey` and `AcceleratorKey`, joined as NVDA joins them), `PositionInSet`,
/// `SizeOfSet`, `Level`, and `BoundingRectangle`. Every field maps UIA's
/// "not supported" default (an empty string or zero) to `None`.
fn details_from_cached(element: &IUIAutomationElement) -> NodeDetails {
    let description = element
        .cached_string(UIA_FullDescriptionPropertyId)
        .or_else(|| element.cached_string(UIA_HelpTextPropertyId));
    let keyboard_shortcut = keyboard_shortcut(
        element.cached_string(UIA_AccessKeyPropertyId),
        element.cached_string(UIA_AcceleratorKeyPropertyId),
    );
    NodeDetails {
        description,
        keyboard_shortcut,
        position_in_set: cached_one_based(element, UIA_PositionInSetPropertyId),
        set_size: cached_one_based(element, UIA_SizeOfSetPropertyId),
        level: cached_one_based(element, UIA_LevelPropertyId),
        rect: cached_rect(element),
    }
}

/// An element's keyboard shortcut: its access key and its accelerator key,
/// each when present, joined by two spaces as NVDA joins them.
fn keyboard_shortcut(
    access_key: Option<String>,
    accelerator_key: Option<String>,
) -> Option<String> {
    match (access_key, accelerator_key) {
        (Some(access), Some(accelerator)) => Some(format!("{access}  {accelerator}")),
        (access, accelerator) => access.or(accelerator),
    }
}

/// An element's value: its `Value` pattern's value, or failing that its
/// `RangeValue` pattern's value rounded to a whole number, as NVDA reads it.
fn value_of(value: Option<String>, range_value: Option<f64>) -> Option<String> {
    value.or_else(|| range_value.map(|range| format!("{}", range.round())))
}

/// Reads the cached process id, so callers can filter events by target pid
/// without a cross-process call.
#[must_use]
pub fn cached_process_id(element: &IUIAutomationElement) -> Option<u32> {
    element
        .cached_i32(UIA_ProcessIdPropertyId)
        .map(i32::cast_unsigned)
}

/// Reads the cached native window handle (0 when the element is not itself a
/// window), used by the outpost arbitration cross-filter. The handle is
/// stored as an integer property.
#[must_use]
pub fn cached_native_window_handle(element: &IUIAutomationElement) -> isize {
    element
        .cached_i32(UIA_NativeWindowHandlePropertyId)
        .unwrap_or(0) as isize
}

/// The identity-free contents of a cached UIA element: its runtime id plus
/// the role, name, value, states, and details a [`NodeSnapshot`] carries —
/// everything except the outpost-minted [`NodeId`](verbatim_model::NodeId).
///
/// This is what the focus listener (decision D13) forwards for a UIA focus
/// event. Node identity is minted per application in that app's own outpost
/// and must never cross a process boundary, so the listener — which holds no
/// per-application state and never touches a registry — captures exactly
/// these parts by cached reads and hands them on; the receiving app outpost
/// mints the id from the runtime id when it rebuilds the snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedUiaParts {
    /// The UIA runtime id, stable for the element's lifetime and the key the
    /// receiving outpost mints its [`NodeId`](verbatim_model::NodeId) from.
    pub runtime_id: Vec<i32>,
    /// Normalized role (already refined for toggle buttons).
    pub role: Role,
    /// Accessible name, if any.
    pub name: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Current states.
    pub states: StateSet,
    /// The optional properties beyond the core four.
    pub details: NodeDetails,
}

/// Reads a cached UIA element into its identity-free [`CachedUiaParts`],
/// without minting a [`NodeId`](verbatim_model::NodeId) or touching any
/// registry. Reads only cached values (plus `GetRuntimeId`, itself a local
/// read on a cached element), so it is safe on an event-callback thread and
/// makes no cross-process call — the listener's hard rule (decision D13).
///
/// [`snapshot_from_cached_element`] is this plus the registry step that mints
/// the id and caches the live element; the two share this one reading path so
/// the role and state mapping is never duplicated. `element` should be
/// built with [`crate::cache::base_cache_request`]; a property missing from
/// its cache reads as unsupported.
#[must_use]
pub fn snapshot_parts_from_cached_element(element: &IUIAutomationElement) -> CachedUiaParts {
    let runtime_id = crate::com::runtime_id(element);
    let control_type = element.cached_i32(UIA_ControlTypePropertyId).unwrap_or(0);
    let toggle_available = element.cached_bool(UIA_IsTogglePatternAvailablePropertyId);
    let class_name = element.cached_string(UIA_ClassNamePropertyId);
    let mut role = refine_button_role(role_from_control_type(control_type), toggle_available);
    if is_dialog(
        element.cached_bool(UIA_IsDialogPropertyId),
        cached_native_window_handle(element) != 0,
        class_name.as_deref(),
    ) {
        role = Role::Dialog;
    }
    CachedUiaParts {
        runtime_id,
        role,
        name: element.cached_string(UIA_NamePropertyId),
        value: value_of(
            element.cached_string(UIA_ValueValuePropertyId),
            // Gated on the pattern, as `ValueIsReadOnly` is: a remotely
            // filled cache stores the default of zero.
            element
                .cached_bool(UIA_IsRangeValuePatternAvailablePropertyId)
                .then(|| element.cached_f64(UIA_RangeValueValuePropertyId))
                .flatten(),
        )
        .filter(|_| !reports_no_value(class_name.as_deref())),
        states: states_from_cached(element, role),
        details: details_from_cached(element),
    }
}

/// Whether an element of UIA class `class_name` reports no value. The
/// Windows shell's file and folder items (UIA class `UIItem`, in File
/// Explorer and the file dialogs) expose their name again as their value;
/// NVDA's `UIItem` class reports none, so the name is spoken once.
fn reports_no_value(class_name: Option<&str>) -> bool {
    class_name == Some("UIItem")
}

/// Builds a [`NodeSnapshot`] from a cached UIA element, minting or reusing its
/// [`NodeId`](verbatim_model::NodeId) via `registry`. Reads only cached values,
/// so it is safe on an event-callback thread. `element` should be built with
/// [`crate::cache::base_cache_request`].
#[must_use]
pub fn snapshot_from_cached_element(
    element: &IUIAutomationElement,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    let parts = snapshot_parts_from_cached_element(element);
    NodeSnapshot {
        // Caches `element` as the node's live element while minting its
        // id, so navigation and re-reads resolve it directly instead of
        // re-finding it by runtime id (see the registry's module doc).
        id: registry.id_for_element(&parts.runtime_id, element),
        backend: Backend::Uia,
        role: parts.role,
        name: parts.name,
        value: parts.value,
        states: parts.states,
        details: parts.details,
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
        assert_eq!(role_from_control_type(UIA_TreeControlTypeId.0), Role::Tree);
        assert_eq!(
            role_from_control_type(UIA_TreeItemControlTypeId.0),
            Role::TreeItem
        );
    }

    #[test]
    fn unmapped_control_type_is_unknown() {
        assert_eq!(role_from_control_type(-1), Role::Unknown);
        assert_eq!(role_from_control_type(999_999), Role::Unknown);
    }

    /// A Button that supports the Toggle pattern is a toggle button (NVDA:
    /// role BUTTON plus a supported `UIA_ToggleToggleStatePropertyId`
    /// becomes TOGGLEBUTTON).
    #[test]
    fn button_with_toggle_pattern_is_toggle_button() {
        assert_eq!(refine_button_role(Role::Button, true), Role::ToggleButton);
    }

    /// A Button with no Toggle pattern support stays a plain button; a role
    /// other than Button passes through unchanged regardless of toggle
    /// availability.
    #[test]
    fn plain_button_and_other_roles_pass_through() {
        assert_eq!(refine_button_role(Role::Button, false), Role::Button);
        assert_eq!(refine_button_role(Role::CheckBox, true), Role::CheckBox);
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
            selection_available: false,
            selected: false,
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&raw, Role::Pane);
        assert!(states.contains(State::Focused));
        assert!(states.contains(State::Focusable));
        assert!(
            !states.contains(State::Mixed),
            "a non-toggle element must not be reported as half-checked"
        );
        assert!(!states.contains(State::Expanded));
        assert!(!states.contains(State::Collapsed));
    }

    /// When the pattern is available, the toggle value is honored. A role
    /// other than `ToggleButton` maps `ToggleState_On` to `Checked`, the
    /// pre-M3 default.
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
            selection_available: false,
            selected: false,
            ..RawUiaStates::default()
        };
        assert!(states_from_uia(&base, Role::CheckBox).contains(State::Checked));

        let indeterminate = RawUiaStates {
            toggle_state: Some(ToggleState_Indeterminate.0),
            ..base
        };
        assert!(states_from_uia(&indeterminate, Role::CheckBox).contains(State::Mixed));

        let off = RawUiaStates {
            toggle_state: Some(0),
            ..base
        };
        let off_states = states_from_uia(&off, Role::CheckBox);
        assert!(!off_states.contains(State::Checked));
        assert!(!off_states.contains(State::Mixed));
    }

    /// `ToggleState_On` maps to `State::Pressed` for a `ToggleButton`,
    /// rather than `State::Checked`.
    #[test]
    fn toggle_state_on_honors_toggle_button_role() {
        let base = RawUiaStates {
            has_focus: false,
            focusable: true,
            enabled: true,
            offscreen: false,
            toggle_available: true,
            toggle_state: Some(ToggleState_On.0),
            expand_available: false,
            expand_state: None,
            selection_available: false,
            selected: false,
            ..RawUiaStates::default()
        };

        let toggle_button_states = states_from_uia(&base, Role::ToggleButton);
        assert!(toggle_button_states.contains(State::Pressed));
        assert!(!toggle_button_states.contains(State::Checked));
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
            selection_available: false,
            selected: false,
            ..RawUiaStates::default()
        };
        assert!(states_from_uia(&expanded, Role::Pane).contains(State::Expanded));
        let collapsed = RawUiaStates {
            expand_state: Some(ExpandCollapseState_Collapsed.0),
            ..expanded
        };
        assert!(states_from_uia(&collapsed, Role::Pane).contains(State::Collapsed));
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
            selection_available: false,
            selected: false,
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&raw, Role::Pane);
        assert!(states.contains(State::Disabled));
        assert!(states.contains(State::Offscreen));
    }

    /// `SelectionItemIsSelected` reads as a default `false` on elements
    /// without `SelectionItemPattern`, and (like toggle and expand above)
    /// must be honored only when the pattern is available. Availability
    /// itself maps to `Selectable`, mirroring MSAA's
    /// `STATE_SYSTEM_SELECTABLE` bit.
    #[test]
    fn selection_states_gate_on_pattern_availability() {
        let base = RawUiaStates {
            has_focus: false,
            focusable: true,
            enabled: true,
            offscreen: false,
            toggle_available: false,
            toggle_state: None,
            expand_available: false,
            expand_state: None,
            selection_available: true,
            selected: true,
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&base, Role::Pane);
        assert!(states.contains(State::Selectable));
        assert!(states.contains(State::Selected));

        let unselected = RawUiaStates {
            selected: false,
            ..base
        };
        let states = states_from_uia(&unselected, Role::Pane);
        assert!(states.contains(State::Selectable));
        assert!(!states.contains(State::Selected));

        let unavailable = RawUiaStates {
            selection_available: false,
            selected: true,
            ..base
        };
        let states = states_from_uia(&unavailable, Role::Pane);
        assert!(!states.contains(State::Selectable));
        assert!(
            !states.contains(State::Selected),
            "a stray selected value without the pattern must be ignored"
        );
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

    #[test]
    fn roles_nvda_maps_are_not_unknown() {
        assert_eq!(
            role_from_control_type(UIA_SplitButtonControlTypeId.0),
            Role::SplitButton
        );
        assert_eq!(
            role_from_control_type(UIA_ImageControlTypeId.0),
            Role::Graphic
        );
        assert_eq!(
            role_from_control_type(UIA_DocumentControlTypeId.0),
            Role::Document
        );
        assert_eq!(
            role_from_control_type(UIA_HeaderItemControlTypeId.0),
            Role::HeaderItem
        );
        assert_eq!(
            role_from_control_type(UIA_ProgressBarControlTypeId.0),
            Role::ProgressBar
        );
    }

    #[test]
    fn a_window_of_a_dialog_class_is_a_dialog() {
        assert!(is_dialog(true, false, None));
        assert!(is_dialog(false, true, Some("#32770")));
        assert!(!is_dialog(false, false, Some("#32770")));
        assert!(!is_dialog(false, true, Some("Notepad")));
    }

    #[test]
    fn a_selected_radio_button_is_checked() {
        let raw = RawUiaStates {
            enabled: true,
            selection_available: true,
            selected: true,
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&raw, Role::RadioButton);
        assert!(states.contains(State::Checked));
        assert!(states.contains(State::Checkable));
        assert!(!states.contains(State::Selected));
        assert!(!states.contains(State::Selectable));
    }

    #[test]
    fn a_toggleable_list_item_is_checkable() {
        let raw = RawUiaStates {
            enabled: true,
            toggle_available: true,
            toggle_state: Some(0),
            ..RawUiaStates::default()
        };
        assert!(states_from_uia(&raw, Role::ListItem).contains(State::Checkable));
        assert!(!states_from_uia(&raw, Role::CheckBox).contains(State::Checkable));
    }

    #[test]
    fn form_states_map_and_unsupported_validity_is_valid() {
        let raw = RawUiaStates {
            enabled: true,
            password: true,
            required: true,
            value_read_only: true,
            data_valid: Some(false),
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&raw, Role::EditableText);
        for state in [
            State::Protected,
            State::Required,
            State::ReadOnly,
            State::InvalidEntry,
        ] {
            assert!(states.contains(state), "{state:?}");
        }
        let unsupported = RawUiaStates {
            enabled: true,
            data_valid: None,
            ..RawUiaStates::default()
        };
        assert!(!states_from_uia(&unsupported, Role::EditableText).contains(State::InvalidEntry));
    }

    #[test]
    fn a_menu_item_checked_only_in_its_legacy_state_is_checked() {
        let legacy_checked = RawUiaStates {
            enabled: true,
            legacy_state: Some(LEGACY_STATE_CHECKED),
            ..RawUiaStates::default()
        };
        let states = states_from_uia(&legacy_checked, Role::MenuItem);
        assert!(states.contains(State::Checkable));
        assert!(states.contains(State::Checked));

        // Only a menu item reads its legacy state.
        assert!(!states_from_uia(&legacy_checked, Role::Button).contains(State::Checked));
        // An unchecked legacy state adds nothing.
        let unchecked = RawUiaStates {
            legacy_state: Some(0),
            ..legacy_checked
        };
        assert!(!states_from_uia(&unchecked, Role::MenuItem).contains(State::Checkable));
        // A menu item with the Toggle pattern is checkable through it, and
        // its legacy state is not consulted.
        let toggled_off = RawUiaStates {
            toggle_available: true,
            toggle_state: Some(0),
            ..legacy_checked
        };
        let states = states_from_uia(&toggled_off, Role::MenuItem);
        assert!(states.contains(State::Checkable));
        assert!(!states.contains(State::Checked));
    }

    #[test]
    fn the_shortcut_joins_access_and_accelerator_keys() {
        assert_eq!(
            keyboard_shortcut(Some("Alt+F".into()), Some("Ctrl+O".into())).as_deref(),
            Some("Alt+F  Ctrl+O")
        );
        assert_eq!(
            keyboard_shortcut(None, Some("Ctrl+O".into())).as_deref(),
            Some("Ctrl+O")
        );
        assert_eq!(keyboard_shortcut(None, None), None);
    }

    #[test]
    fn a_range_value_stands_in_for_a_missing_value() {
        assert_eq!(value_of(None, Some(42.6)).as_deref(), Some("43"));
        assert_eq!(
            value_of(Some("Medium".into()), Some(50.0)).as_deref(),
            Some("Medium")
        );
        assert_eq!(value_of(None, None), None);
    }
}
