//! The shapes, and the mapping each group does from what it reads.

use verbatim_model::{NodeDetails, Role, State, StateSet};
use windows::Win32::UI::Accessibility::{
    UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId, UIA_ControlTypePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_FrameworkIdPropertyId,
    UIA_HasKeyboardFocusPropertyId, UIA_IsContentElementPropertyId, UIA_IsControlElementPropertyId,
    UIA_IsDataValidForFormPropertyId, UIA_IsDialogPropertyId, UIA_IsEnabledPropertyId,
    UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId, UIA_IsRequiredForFormPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTextPatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId,
    UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId,
    UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_ProcessIdPropertyId, UIA_ProviderDescriptionPropertyId,
    UIA_RangeValueValuePropertyId, UIA_SelectionItemIsSelectedPropertyId, UIA_TextPatternId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValueValuePropertyId,
};

use super::{Covers, Reader, Shape, Ungated, uia_shape};
use crate::map::{
    CachedUiaParts, RawUiaStates, role_of, states_from_uia, strip_bidi_controls, value_from,
};

uia_shape! {
    /// What an element's role is mapped from.
    pub struct RoleCore {
        gates: none;
        groups {}
        properties {
            /// The control type id.
            control_type: UIA_ControlTypePropertyId => i32,
            /// The UIA class name.
            class_name: UIA_ClassNamePropertyId => string,
            /// Whether the element says it is a dialog.
            is_dialog: UIA_IsDialogPropertyId => bool,
            /// Whether the element has the Toggle pattern.
            toggle_available: UIA_IsTogglePatternAvailablePropertyId => bool,
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
        }
        prefetch {}
        covers {}
    }
}

impl RoleCore {
    /// The role, as NVDA's UIA objects decide it: refined for toggle
    /// buttons, terminals, and dialogs.
    #[must_use]
    pub fn role(&self) -> Role {
        role_of(
            self.control_type,
            self.toggle_available,
            self.class_name.as_deref(),
            self.is_dialog,
            self.hwnd,
        )
    }
}

uia_shape! {
    /// What an element's value is read from.
    pub struct ValueSet {
        gates: none;
        groups {}
        properties {
            /// The `Value` pattern's value.
            value: UIA_ValueValuePropertyId => string,
            /// Whether the element has the `RangeValue` pattern.
            range_available: UIA_IsRangeValuePatternAvailablePropertyId => bool,
            /// The `RangeValue` pattern's value, which means something only
            /// when the pattern is available: a remotely filled cache
            /// stores the default of zero.
            range: UIA_RangeValueValuePropertyId => f64,
        }
        prefetch {}
        covers {}
    }
}

impl ValueSet {
    /// The value, as NVDA reads it, for an element of class `class_name`:
    /// none for a class that reports none.
    #[must_use]
    pub fn value(&self, class_name: Option<&str>) -> Option<String> {
        value_from(
            self.value.clone(),
            self.range_available,
            self.range,
            class_name,
        )
    }
}

uia_shape! {
    /// What an element's states are mapped from.
    #[allow(clippy::struct_excessive_bools, reason = "one field per cached UIA flag")]
    pub struct States {
        gates: none;
        groups {}
        properties {
            /// Whether it has the keyboard focus.
            has_focus: UIA_HasKeyboardFocusPropertyId => bool,
            /// Whether it can take the keyboard focus.
            focusable: UIA_IsKeyboardFocusablePropertyId => bool,
            /// Whether it is enabled.
            enabled: UIA_IsEnabledPropertyId => bool,
            /// Whether it is off the screen.
            offscreen: UIA_IsOffscreenPropertyId => bool,
            /// Whether it has the Toggle pattern.
            toggle_available: UIA_IsTogglePatternAvailablePropertyId => bool,
            /// The toggle state, meaningful only with the pattern.
            toggle_state: UIA_ToggleToggleStatePropertyId => i32,
            /// Whether it has the `ExpandCollapse` pattern.
            expand_available: UIA_IsExpandCollapsePatternAvailablePropertyId => bool,
            /// The expand state, meaningful only with the pattern.
            expand_state: UIA_ExpandCollapseExpandCollapseStatePropertyId => i32,
            /// Whether it has the `SelectionItem` pattern.
            selection_available: UIA_IsSelectionItemPatternAvailablePropertyId => bool,
            /// Whether it is selected.
            selected: UIA_SelectionItemIsSelectedPropertyId => bool,
            /// Whether it holds a password.
            password: UIA_IsPasswordPropertyId => bool,
            /// Whether a form requires it.
            required: UIA_IsRequiredForFormPropertyId => bool,
            /// Whether its entry is valid, `None` when not supported.
            data_valid: UIA_IsDataValidForFormPropertyId => optional_bool,
            /// Whether it has the `Value` pattern.
            value_available: UIA_IsValuePatternAvailablePropertyId => bool,
            /// Whether its value is read-only, meaningful only with the
            /// pattern: a remotely filled cache stores the default of true.
            value_read_only: UIA_ValueIsReadOnlyPropertyId => optional_bool,
        }
        prefetch {}
        covers {}
    }
}

impl States {
    /// The states, for an element whose role is `role`.
    #[must_use]
    pub fn states(&self, role: Role) -> StateSet {
        let raw = RawUiaStates {
            has_focus: self.has_focus,
            focusable: self.focusable,
            enabled: self.enabled,
            offscreen: self.offscreen,
            toggle_available: self.toggle_available,
            toggle_state: self.toggle_state,
            expand_available: self.expand_available,
            expand_state: self.expand_state,
            selection_available: self.selection_available,
            selected: self.selected,
            password: self.password,
            required: self.required,
            data_valid: self.data_valid,
            value_read_only: self.value_available && self.value_read_only == Some(true),
        };
        states_from_uia(&raw, role)
    }
}

uia_shape! {
    /// What every reader of an element the node registry keeps reads of it:
    /// every shape whose elements are kept covers it.
    pub struct Kept {
        gates: none;
        groups {}
        properties {
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
            /// The UIA class name.
            class_name: UIA_ClassNamePropertyId => string,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// A desktop-wide event's element, as the focus listener's
    /// registration delivers it: NVDA's base cache request
    /// (`UIAHandler/__init__.py` 61 to 74 and 547 to 555), with the Text
    /// pattern, and the two properties that refine a role for dialogs and
    /// toggle buttons (`docs/design/uia-shapes.md`, decision 6).
    pub struct ListenerEvent {
        gates: none;
        groups {
            /// What its role is mapped from.
            role: RoleCore,
        }
        properties {
            /// The UIA framework, such as `XAML` or `WinForm`.
            framework_id: UIA_FrameworkIdPropertyId => string,
            /// The automation id.
            automation_id: UIA_AutomationIdPropertyId => string,
            /// The provider's description, as NVDA's debug strings read it.
            provider_description: UIA_ProviderDescriptionPropertyId => string,
            /// The owning process.
            process_id: UIA_ProcessIdPropertyId => process_id,
            /// Whether it has the Text pattern.
            text_available: UIA_IsTextPatternAvailablePropertyId => bool,
            /// Whether UIA counts it as content.
            content: UIA_IsContentElementPropertyId => bool,
            /// Whether UIA counts it as a control.
            control: UIA_IsControlElementPropertyId => bool,
            /// The name.
            name: UIA_NamePropertyId => string,
            /// The localized control type.
            localized_control_type: UIA_LocalizedControlTypePropertyId => string,
            /// Whether it has the keyboard focus.
            has_focus: UIA_HasKeyboardFocusPropertyId => bool,
        }
        prefetch { UIA_TextPatternId }
        covers { Kept }
    }
}

impl ListenerEvent {
    /// The event's element as the focus listener forwards it: its runtime
    /// id `runtime_id`, role, name, and, as its only state, whether it has
    /// the keyboard focus. Nothing else is in the cache, so the value,
    /// other states, and details are left empty, not read as defaults;
    /// what Verbatim needs of them is read afresh.
    #[must_use]
    pub fn parts(&self, runtime_id: Vec<i32>) -> CachedUiaParts {
        let mut name = self.name.clone();
        strip_bidi_controls([&mut name]);
        let mut states = StateSet::new();
        if self.has_focus {
            states.insert(State::Focused);
        }
        CachedUiaParts {
            runtime_id,
            role: self.role.role(),
            name,
            value: None,
            states,
            details: NodeDetails::default(),
        }
    }
}

uia_shape! {
    /// A property change's element, as the outpost's focus-following
    /// registration delivers it: everything a role, name, value, or state
    /// set is mapped from, and no detail, since a change of states carries
    /// the node's whole state set and no property change reports a detail.
    pub struct PropertyEvent {
        gates: none;
        groups {
            /// What its role is mapped from.
            role: RoleCore,
            /// What its value is read from.
            value: ValueSet,
            /// What its states are mapped from.
            states: States,
        }
        properties {
            /// The name.
            name: UIA_NamePropertyId => string,
            /// The owning process.
            process_id: UIA_ProcessIdPropertyId => process_id,
        }
        prefetch {}
        covers { Kept }
    }
}

impl PropertyEvent {
    /// The event's element as a snapshot's parts, with runtime id
    /// `runtime_id` and no details.
    #[must_use]
    pub fn parts(&self, runtime_id: Vec<i32>) -> CachedUiaParts {
        let role = self.role.role();
        let mut name = self.name.clone();
        let mut value = self.value.value(self.role.class_name.as_deref());
        strip_bidi_controls([&mut name, &mut value]);
        CachedUiaParts {
            runtime_id,
            role,
            name,
            value,
            states: self.states.states(role),
            details: NodeDetails::default(),
        }
    }
}

uia_shape! {
    /// A text focus's caret, text, or active text position change's
    /// element, from which nothing is spoken: its process and window, and
    /// its class name, since the element can become the node's kept
    /// element, whose class name says whether its text is a terminal's.
    pub struct TextEvent {
        gates: none;
        groups {}
        properties {
            /// The owning process.
            process_id: UIA_ProcessIdPropertyId => process_id,
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
            /// The UIA class name.
            class_name: UIA_ClassNamePropertyId => string,
        }
        prefetch {}
        covers { Kept }
    }
}
