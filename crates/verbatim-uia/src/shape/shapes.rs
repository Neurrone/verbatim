//! The shapes, and the mapping each group does from what it reads.

use verbatim_model::{Backend, NodeDetails, NodeSnapshot, Role, State, StateSet};
use windows::Win32::UI::Accessibility::{
    IUIAutomationSelectionPattern, IUIAutomationTextPattern, UIA_AcceleratorKeyPropertyId,
    UIA_AccessKeyPropertyId, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ClassNamePropertyId, UIA_ControlTypePropertyId, UIA_ControllerForPropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_FrameworkIdPropertyId,
    UIA_FullDescriptionPropertyId, UIA_GridItemColumnPropertyId, UIA_GridItemRowPropertyId,
    UIA_HasKeyboardFocusPropertyId, UIA_HelpTextPropertyId, UIA_IsContentElementPropertyId,
    UIA_IsControlElementPropertyId, UIA_IsDataValidForFormPropertyId, UIA_IsDialogPropertyId,
    UIA_IsEnabledPropertyId, UIA_IsExpandCollapsePatternAvailablePropertyId,
    UIA_IsGridItemPatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
    UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_IsRangeValuePatternAvailablePropertyId, UIA_IsRequiredForFormPropertyId,
    UIA_IsSelectionItemPatternAvailablePropertyId, UIA_IsTextPatternAvailablePropertyId,
    UIA_IsTogglePatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId,
    UIA_LevelPropertyId, UIA_LocalizedControlTypePropertyId, UIA_NamePropertyId,
    UIA_NativeWindowHandlePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_PositionInSetPropertyId,
    UIA_ProcessIdPropertyId, UIA_ProviderDescriptionPropertyId, UIA_RangeValueValuePropertyId,
    UIA_Selection2FirstSelectedItemPropertyId, UIA_SelectionItemIsSelectedPropertyId,
    UIA_SelectionPatternId, UIA_SizeOfSetPropertyId, UIA_TextPatternId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueIsReadOnlyPropertyId, UIA_ValueValuePropertyId,
};

use super::{Cached, Covers, Reader, Shape, Ungated, uia_shape};
use crate::map::{
    CachedUiaParts, RawUiaStates, keyboard_shortcut, role_of, states_from_uia, strip_bidi_controls,
    value_from,
};
use crate::registry::NodeIdRegistry;

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
    /// What an element's states are mapped from, but whether it is off the
    /// screen, which is its [`Place`]'s: a read of many elements, such as
    /// a walk's or a container's children's, never asks for that
    /// (`docs/design/uia-shapes.md`, decision 9).
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
    /// The states, for an element whose role is `role`, off the screen
    /// when `offscreen` says so: the element's [`Place`] read it, or
    /// `false` when nothing did.
    #[must_use]
    pub fn states(&self, role: Role, offscreen: bool) -> StateSet {
        let raw = RawUiaStates {
            has_focus: self.has_focus,
            focusable: self.focusable,
            enabled: self.enabled,
            offscreen,
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
    /// Where an element is: its bounding rectangle and whether it is off
    /// the screen. Read only for the one element that needs them, never by
    /// a read of many: for some elements UIA answers either only by asking
    /// the application's UI thread, a top-level window's title bar and its
    /// buttons among them, for which it sends `WM_GETTITLEBARINFOEX`, which
    /// Windows answers for one window at a time across the desktop, about
    /// 6 ms alone and seconds under load, while that thread waits. NVDA's
    /// base cache, on every element it walks to or is sent with an event,
    /// holds neither, and it reads both per object when it needs them
    /// (`UIAHandler/__init__.py`, `baseCachePropertyIDs`;
    /// `docs/design/uia-shapes.md`, decision 9).
    pub struct Place {
        gates: none;
        groups {}
        properties {
            /// Where it is on the screen.
            rect: UIA_BoundingRectanglePropertyId => rect,
            /// Whether it is off the screen.
            offscreen: UIA_IsOffscreenPropertyId => bool,
        }
        prefetch {}
        covers {}
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
            /// Whether it had the keyboard focus when the event was
            /// raised: UIA's default, `false`, when its provider answered
            /// nothing or failed, which the cache does not tell apart.
            has_focus: UIA_HasKeyboardFocusPropertyId => bool,
        }
        prefetch { UIA_TextPatternId }
        covers { Kept, FocusInput, TextAreaPattern }
    }
}

uia_shape! {
    /// The Text pattern a focus event's sender carries in its cache
    /// ([`ListenerEvent`] prefetches it), read only by the focus listener's
    /// console check of a console's text area, so the pattern costs no
    /// call (`docs/design/listener-without-connections.md`, section 3,
    /// step 5).
    pub struct TextAreaPattern {
        gates: none;
        groups {}
        properties {}
        patterns {
            /// The Text pattern, `None` when the element has none.
            text: UIA_TextPatternId => IUIAutomationTextPattern,
        }
        prefetch {}
        covers {}
    }
}

impl ListenerEvent {
    /// Whether the event's cache says its element did not have the
    /// keyboard focus: anything but a cached `true`. A provider that
    /// answered nothing for the property, or failed to, leaves UIA's
    /// default, `false`, in the cache, as NVDA's live read
    /// (`currentHasKeyboardFocus`) of such a provider answers `false`.
    #[must_use]
    pub fn lacked_keyboard_focus(&self) -> bool {
        !self.has_focus
    }

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
    /// An event registration's cache never holds the element's [`Place`],
    /// so a state set it carries is never off the screen.
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
            states: self.states.states(role, false),
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

uia_shape! {
    /// What the remote focus query reads of its input element
    /// (`verbatim-uia-rops`): whether it is a list or tab control, whose
    /// selected child is read with it, and its own window. Never requested:
    /// every shape the query is given covers it.
    pub struct FocusInput {
        gates: none;
        groups {}
        properties {
            /// The control type id.
            control_type: UIA_ControlTypePropertyId => i32,
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// What an element's description is read from.
    pub struct Description {
        gates: none;
        groups {}
        properties {
            /// The full description.
            full_description: UIA_FullDescriptionPropertyId => string,
            /// The help text.
            help_text: UIA_HelpTextPropertyId => string,
        }
        prefetch {}
        covers {}
    }
}

impl Description {
    /// The description: the full description, else the help text, as
    /// NVDA's UIA objects describe themselves.
    #[must_use]
    pub fn description(&self) -> Option<String> {
        self.full_description
            .clone()
            .or_else(|| self.help_text.clone())
    }
}

uia_shape! {
    /// What an element's keyboard shortcut is read from.
    pub struct Shortcut {
        gates: none;
        groups {}
        properties {
            /// The access key.
            access_key: UIA_AccessKeyPropertyId => string,
            /// The accelerator key.
            accelerator_key: UIA_AcceleratorKeyPropertyId => string,
        }
        prefetch {}
        covers {}
    }
}

impl Shortcut {
    /// The shortcut: the access key and the accelerator key, each when
    /// present, joined by two spaces as NVDA joins them.
    #[must_use]
    pub fn shortcut(&self) -> Option<String> {
        keyboard_shortcut(self.access_key.clone(), self.accelerator_key.clone())
    }
}

uia_shape! {
    /// An element's position in its set.
    pub struct Position {
        gates: none;
        groups {}
        properties {
            /// Its one-based position.
            position_in_set: UIA_PositionInSetPropertyId => one_based,
            /// How many the set holds.
            size_of_set: UIA_SizeOfSetPropertyId => one_based,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// Where an element is in a grid, when it is a grid's item: read only
    /// for the one element that is spoken alone, such as a focus, never by
    /// a read of many, as NVDA's base cache holds none of it and it reads
    /// these per object when it speaks one (`NVDAObjects/UIA/__init__.py`,
    /// `_get_rowNumber` and `_get_columnNumber`).
    pub struct Cell {
        gates: none;
        groups {}
        properties {
            /// Whether it has the `GridItem` pattern.
            grid_item: UIA_IsGridItemPatternAvailablePropertyId => bool,
            /// Its zero-based row, meaningful only with the pattern: a
            /// remotely filled cache stores the default of zero.
            row: UIA_GridItemRowPropertyId => i32,
            /// Its zero-based column, meaningful only with the pattern.
            column: UIA_GridItemColumnPropertyId => i32,
        }
        prefetch {}
        covers {}
    }
}

impl Cell {
    /// Its one-based row and column, as NVDA numbers them for speech (the
    /// pattern's zero-based values plus one), `None` for an element
    /// without the `GridItem` pattern or with a negative row or column.
    #[must_use]
    pub fn coordinates(&self) -> Option<(u32, u32)> {
        if !self.grid_item {
            return None;
        }
        let one_based = |value: Option<i32>| {
            u32::try_from(value?)
                .ok()
                .and_then(|value| value.checked_add(1))
        };
        Some((one_based(self.row)?, one_based(self.column)?))
    }
}

uia_shape! {
    /// A node as Core is told of it: a focus, a selection, a neighbour, a
    /// window, a tree dump's node. Its four details are fetched only when
    /// the active theme reports them, and its [`Place`] only for a read of
    /// the one element that needs it, never for a read of many, such as an
    /// ancestor walk or a tree dump ([`NodeGates`]).
    pub struct Node {
        gates: NodeGates;
        groups {
            /// What its role is mapped from.
            role: RoleCore,
            /// What its value is read from.
            value: ValueSet,
            /// What its states are mapped from.
            states: States,
        }
        gated_groups {
            /// Its description.
            description: Description,
            /// Its keyboard shortcut.
            shortcut: Shortcut,
            /// Its position in its set.
            position: Position,
            /// Where it is: its rectangle and whether it is off the screen.
            place: Place,
            /// Where it is in a grid, when it is a grid's item.
            cell: Cell,
        }
        properties {
            /// The name.
            name: UIA_NamePropertyId => string,
        }
        gated_properties {
            /// Its one-based level, in a tree or among headings.
            level: UIA_LevelPropertyId => one_based,
        }
        covers { Kept, FocusInput }
    }
}

impl Node {
    /// The node's parts, with runtime id `runtime_id`: a detail its gate
    /// left unfetched is `None`, as one the element does not report, and a
    /// node whose place was not fetched has no rectangle and is not off
    /// the screen.
    #[must_use]
    pub fn parts(&self, runtime_id: Vec<i32>) -> CachedUiaParts {
        let role = self.role.role();
        let mut name = self.name.clone();
        let mut value = self.value.value(self.role.class_name.as_deref());
        let mut details = NodeDetails {
            description: self.description.as_ref().and_then(Description::description),
            keyboard_shortcut: self.shortcut.as_ref().and_then(Shortcut::shortcut),
            position_in_set: self
                .position
                .as_ref()
                .and_then(|position| position.position_in_set),
            set_size: self
                .position
                .as_ref()
                .and_then(|position| position.size_of_set),
            level: self.level.flatten(),
            rect: self.place.as_ref().and_then(|place| place.rect),
            top_level_client: false,
            class_name: None,
            row: None,
            column: None,
            parent_automation_id: None,
        };
        if let Some((row, column)) = self.cell.as_ref().and_then(Cell::coordinates) {
            details.row = Some(row);
            details.column = Some(column);
        }
        strip_bidi_controls([&mut name, &mut value, &mut details.description]);
        let offscreen = self.place.as_ref().is_some_and(|place| place.offscreen);
        CachedUiaParts {
            runtime_id,
            role,
            name,
            value,
            states: self.states.states(role, offscreen),
            details,
        }
    }

    /// Whether the node's value is its first selected item's name
    /// ([`crate::map::value_from_selection`]): a combo box without the
    /// `Value` pattern, or a looping selector.
    #[must_use]
    pub fn value_from_selection(&self) -> bool {
        crate::map::value_from_selection(
            self.role.role(),
            self.role.class_name.as_deref(),
            self.states.value_available,
        )
    }
}

/// `node`, read from `element`, as a snapshot, its id minted by `registry`,
/// which keeps the element so later reads resolve it without a search.
fn snapshot_of(node: &Node, element: &Cached<Kept>, registry: &NodeIdRegistry) -> NodeSnapshot {
    let parts = node.parts(element.runtime_id());
    NodeSnapshot {
        id: registry.id_for_element(&parts.runtime_id, element),
        backend: Backend::Uia,
        role: parts.role,
        name: parts.name,
        value: parts.value,
        states: parts.states,
        details: parts.details,
    }
}

impl Cached<Node> {
    /// The node as a snapshot, its id minted by `registry`, which keeps the
    /// element. Local.
    #[must_use]
    pub fn snapshot(&self, registry: &NodeIdRegistry) -> NodeSnapshot {
        snapshot_of(&self.read(), &self.narrow(), registry)
    }
}

uia_shape! {
    /// An ancestor of a focus, or a window an MSAA walk crosses into: a
    /// [`Node`], with its description whatever the theme, which NVDA's
    /// layout judgment reads whatever its setting (`presentationType`;
    /// Dickson, 2026-10-13, decision 4 of `docs/design/uia-shapes.md`), and
    /// whether UIA counts it a control and content.
    pub struct Ancestor {
        gates: AncestorGates;
        groups {
            /// The node it is reported as.
            node: Node,
            /// Its description, for the layout judgment.
            description: Description,
        }
        gated_groups {}
        properties {
            /// Whether UIA counts it as content.
            content: UIA_IsContentElementPropertyId => bool,
            /// Whether UIA counts it as a control.
            control: UIA_IsControlElementPropertyId => bool,
            /// Its automation id, which a focus's snapshot carries of its
            /// nearest ancestor, its parent, for Core's rules for an
            /// application's elements (decision D6), as NVDA's app modules
            /// read `UIAAutomationId` of an object's parent from NVDA's base
            /// cache.
            automation_id: UIA_AutomationIdPropertyId => string,
        }
        gated_properties {}
        covers { Kept }
    }
}

impl Ancestor {
    /// Whether UIA counts it both a control and content, which NVDA
    /// requires of a UIA element before it counts as content.
    #[must_use]
    pub fn is_control_and_content(&self) -> bool {
        self.content && self.control
    }

    /// Its description whatever the theme, as the layout judgment reads it.
    #[must_use]
    pub fn layout_description(&self) -> Option<String> {
        let mut description = self.description.description();
        strip_bidi_controls([&mut description]);
        description
    }
}

impl Cached<Ancestor> {
    /// The ancestor as a snapshot, its id minted by `registry`, which keeps
    /// the element. Local.
    #[must_use]
    pub fn snapshot(&self, registry: &NodeIdRegistry) -> NodeSnapshot {
        snapshot_of(&self.read().node, &self.narrow(), registry)
    }
}

uia_shape! {
    /// A child of a dialog, or of a container in it, whose text the dialog's
    /// own text is gathered from: its role, value, states, which never hold
    /// whether it is off the screen ([`Place`]), and name, and its description
    /// whatever the theme, as NVDA's `basicText` includes it (decision 5 of
    /// `docs/design/uia-shapes.md`).
    pub struct DialogText {
        gates: none;
        groups {
            /// What its role is mapped from.
            role: RoleCore,
            /// What its value is read from.
            value: ValueSet,
            /// What its states are mapped from.
            states: States,
            /// Its description.
            description: Description,
        }
        properties {
            /// The name.
            name: UIA_NamePropertyId => string,
        }
        prefetch {}
        covers {}
    }
}

impl DialogText {
    /// The child's parts, with runtime id `runtime_id`: its description is
    /// its one detail.
    #[must_use]
    pub fn parts(&self, runtime_id: Vec<i32>) -> CachedUiaParts {
        let role = self.role.role();
        let mut name = self.name.clone();
        let mut value = self.value.value(self.role.class_name.as_deref());
        let mut description = self.description.description();
        strip_bidi_controls([&mut name, &mut value, &mut description]);
        CachedUiaParts {
            runtime_id,
            role,
            name,
            value,
            states: self.states.states(role, false),
            details: NodeDetails {
                description,
                ..NodeDetails::default()
            },
        }
    }
}

uia_shape! {
    /// A selection container, refreshed to find its first selected item:
    /// `SelectionPattern2`'s first selected item, read ignoring its
    /// default, as NVDA reads the newer pattern where the provider has it,
    /// and the `Selection` pattern for a provider without it.
    pub struct SelectionContainer {
        gates: none;
        groups {}
        properties {
            /// The first selected item.
            first_selected: UIA_Selection2FirstSelectedItemPropertyId => element,
        }
        patterns {
            /// The `Selection` pattern.
            selection: UIA_SelectionPatternId => IUIAutomationSelectionPattern,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// An item found among a focused combo box's children as it is
    /// selected (`verbatim_uia::map::value_from_selection`): its name,
    /// the combo box's new value, and whether it has the keyboard focus,
    /// as NVDA reads both live when it handles the item's selection
    /// (`docs/parity/changes-and-selection.md`).
    pub struct SelectedItem {
        gates: none;
        groups {}
        properties {
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
            /// The UIA class name.
            class_name: UIA_ClassNamePropertyId => string,
            /// The name.
            name: UIA_NamePropertyId => string,
            /// Whether it has the keyboard focus.
            has_focus: UIA_HasKeyboardFocusPropertyId => bool,
        }
        prefetch {}
        covers { Kept }
    }
}

impl SelectedItem {
    /// The item's name, as a node's name is given: without bidirectional
    /// control characters.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        let mut name = self.name.clone();
        strip_bidi_controls([&mut name]);
        name
    }
}

uia_shape! {
    /// An element refreshed for a help text change, which NVDA reports as a
    /// description change.
    pub struct HelpTextRefresh {
        gates: none;
        groups {
            /// Its description.
            description: Description,
        }
        properties {}
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// An element refreshed for a `ControllerFor` change: whether it is
    /// editable text or a search field, which report suggestions, and
    /// whether the relation names any element, read live by the same
    /// refresh, as NVDA reads `controllerFor` when it handles the change.
    pub struct SuggestionsRefresh {
        gates: none;
        groups {}
        properties {
            /// Whether it has the Text pattern.
            text_available: UIA_IsTextPatternAvailablePropertyId => bool,
            /// The automation id.
            automation_id: UIA_AutomationIdPropertyId => string,
            /// Whether its `ControllerFor` relation names any element.
            controls_any: UIA_ControllerForPropertyId => names_any,
        }
        prefetch {}
        covers {}
    }
}

impl SuggestionsRefresh {
    /// Whether the element reports suggestions opening and closing when its
    /// `ControllerFor` relation changes: editable text, which NVDA's UIA
    /// objects with a text pattern become, and the search fields NVDA
    /// knows by automation id `SearchTextBox` or `TextBox` (`SearchField`,
    /// `NVDAObjects/UIA/__init__.py` 1418 to 1421). NVDA's own condition
    /// also asks the text for navigable text or a selection; a text pattern
    /// is taken here as enough.
    #[must_use]
    pub fn reports_suggestions(&self) -> bool {
        self.text_available
            || matches!(
                self.automation_id.as_deref(),
                Some("SearchTextBox" | "TextBox")
            )
    }
}

uia_shape! {
    /// Which providers serve an element, cached with an outpost's lookup by
    /// handle or of the focused element and read only by the proxy rule
    /// (`docs/design/listener-without-connections.md`, section 5).
    pub struct ProviderOrigin {
        gates: none;
        groups {}
        properties {
            /// The provider's description, naming each provider's module.
            description: UIA_ProviderDescriptionPropertyId => string,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// An element normalized to itself or its nearest ancestor that is a
    /// window (NVDA's `getNearestWindowHandle`).
    pub struct WindowLookup {
        gates: none;
        groups {}
        properties {
            /// The window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
        }
        prefetch {}
        covers {}
    }
}

uia_shape! {
    /// An element of the shell's notification area or taskbar, as File
    /// Explorer's outpost reads it for Core's list of the shell's buttons.
    /// It holds whether the element is off the screen, which the list
    /// reads for every button (Dickson, 2026-10-24), but not where it is:
    /// a click reads the one chosen button's [`Place`].
    pub struct ShellButton {
        gates: none;
        groups {}
        properties {
            /// The element's own window, 0 for an element that is not one.
            hwnd: UIA_NativeWindowHandlePropertyId => window_handle,
            /// The control type id.
            control_type: UIA_ControlTypePropertyId => i32,
            /// The name.
            name: UIA_NamePropertyId => string,
            /// Whether it is off the screen.
            offscreen: UIA_IsOffscreenPropertyId => bool,
        }
        prefetch {}
        covers {}
    }
}
