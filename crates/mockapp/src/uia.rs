//! The scripted UIA provider (architecture section 13, layer 2).
//!
//! Every node in the tree gets its own COM object on demand: the root
//! (index 0, corresponding to the host window) is a [`RootProvider`], every
//! other node a [`ChildProvider`]. Both implement `IRawElementProviderSimple`
//! and `IRawElementProviderFragment`; only the root additionally implements
//! `IRawElementProviderFragmentRoot`, matching real hwnd-rooted providers and
//! keeping non-root elements from misreporting themselves as fragment roots.
//! [`props`] holds the shared, tree-reading logic both structs delegate to.
//!
//! Boundary navigation results (no parent, no next sibling, pattern not
//! supported, and so on) are expressed as `Err(windows_core::Error::empty())`,
//! which the generated COM glue turns into `S_OK` with a null out-parameter —
//! the documented UIA contract for "nothing here" — since the interface
//! types themselves cannot represent a null interface pointer safely.

use verbatim_model::Role;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IRawElementProviderSimple, NotificationKind_Other, NotificationProcessing_All,
    UIA_AutomationFocusChangedEventId, UIA_ButtonControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_EditControlTypeId, UIA_GroupControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_PaneControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_SelectionItem_ElementSelectedEventId, UIA_SliderControlTypeId, UIA_SpinnerControlTypeId,
    UIA_StatusBarControlTypeId, UIA_TabControlTypeId, UIA_TabItemControlTypeId,
    UIA_TextControlTypeId, UIA_ToolBarControlTypeId, UIA_TreeControlTypeId,
    UIA_TreeItemControlTypeId, UIA_ValueValuePropertyId, UIA_WindowControlTypeId,
    UiaRaiseAutomationEvent, UiaRaiseAutomationPropertyChangedEvent, UiaRaiseNotificationEvent,
};
use windows_core::Interface;

use crate::stdin::Command;
use crate::tree::SharedTree;

pub(crate) use handler::{ChildProvider, RootProvider};

/// Builds the root's provider, for answering `WM_GETOBJECT`.
pub(crate) fn root_provider(tree: SharedTree, hwnd: HWND) -> RootProvider {
    RootProvider {
        tree,
        hwnd,
        index: 0,
    }
}

/// Applies a parsed stdin [`Command`] against `tree` and raises the matching
/// UIA notification. Runs on the window thread.
pub(crate) fn apply_command(tree: &SharedTree, hwnd: HWND, command: Command) {
    match command {
        Command::Focus(id) => {
            if let Some(index) = focus_node(tree, &id) {
                raise_focus(tree, hwnd, index);
            }
        }
        Command::SetName(id, text) => {
            if let Some(index) = set_name(tree, &id, text) {
                raise_property_changed(tree, hwnd, index, UIA_NamePropertyId);
            }
        }
        Command::SetValue(id, text) => {
            if let Some(index) = set_value(tree, &id, text) {
                raise_property_changed(tree, hwnd, index, UIA_ValueValuePropertyId);
            }
        }
        Command::Select(id) => {
            if let Some(index) = select_node(tree, &id) {
                raise_selection(tree, hwnd, index);
            }
        }
        Command::Notify(text) => raise_notification(tree, hwnd, &text),
        Command::Quit => {}
    }
}

fn focus_node(tree: &SharedTree, id: &str) -> Option<usize> {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = guard.index_of(id)?;
    if let Some(previous) = guard.focused.replace(index) {
        guard.nodes[previous]
            .states
            .remove(verbatim_model::State::Focused);
    }
    guard.nodes[index]
        .states
        .insert(verbatim_model::State::Focused);
    Some(index)
}

fn set_name(tree: &SharedTree, id: &str, text: String) -> Option<usize> {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = guard.index_of(id)?;
    guard.nodes[index].name = (!text.is_empty()).then_some(text);
    Some(index)
}

fn set_value(tree: &SharedTree, id: &str, text: String) -> Option<usize> {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = guard.index_of(id)?;
    guard.nodes[index].value = (!text.is_empty()).then_some(text);
    Some(index)
}

/// Marks the node selected, moving the `Selected` state off any previously
/// selected node — the single-selection model `select` scripts.
fn select_node(tree: &SharedTree, id: &str) -> Option<usize> {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = guard.index_of(id)?;
    if let Some(previous) = guard.selected.replace(index) {
        guard.nodes[previous]
            .states
            .remove(verbatim_model::State::Selected);
    }
    guard.nodes[index]
        .states
        .insert(verbatim_model::State::Selected);
    Some(index)
}

fn raise_focus(tree: &SharedTree, hwnd: HWND, index: usize) {
    let fragment = props::provider_for(tree.clone(), hwnd, index);
    // `IRawElementProviderFragment` and `IRawElementProviderSimple` are
    // sibling interfaces on the same COM object (both derive only from
    // `IUnknown`), so getting from one to the other is a `QueryInterface`
    // (`cast`), not an upcast (`into`).
    let Ok(provider) = fragment.cast::<IRawElementProviderSimple>() else {
        return;
    };
    // SAFETY: `provider` is a live COM object for the just-updated node.
    unsafe {
        let _ = UiaRaiseAutomationEvent(&provider, UIA_AutomationFocusChangedEventId);
    }
}

fn raise_selection(tree: &SharedTree, hwnd: HWND, index: usize) {
    let fragment = props::provider_for(tree.clone(), hwnd, index);
    let Ok(provider) = fragment.cast::<IRawElementProviderSimple>() else {
        return;
    };
    // SAFETY: `provider` is a live COM object for the just-selected node.
    unsafe {
        let _ = UiaRaiseAutomationEvent(&provider, UIA_SelectionItem_ElementSelectedEventId);
    }
}

/// Raises a UIA `AutomationNotification` from the root provider, carrying
/// `text` as the display string and a fixed mockapp activity id. Kind and
/// processing are `Other` and `All` — the values a generic app-initiated
/// announcement (a snap-layout hint, say) would use.
fn raise_notification(tree: &SharedTree, hwnd: HWND, text: &str) {
    let fragment = props::provider_for(tree.clone(), hwnd, 0);
    let Ok(provider) = fragment.cast::<IRawElementProviderSimple>() else {
        return;
    };
    let display = windows_core::BSTR::from(text);
    let activity = windows_core::BSTR::from("mockapp-notify");
    // SAFETY: `provider` is a live COM object for the root; the BSTRs live
    // across the call.
    unsafe {
        let _ = UiaRaiseNotificationEvent(
            &provider,
            NotificationKind_Other,
            NotificationProcessing_All,
            &display,
            &activity,
        );
    }
}

fn raise_property_changed(tree: &SharedTree, hwnd: HWND, index: usize, property: UIA_PROPERTY_ID) {
    let fragment = props::provider_for(tree.clone(), hwnd, index);
    let Ok(provider) = fragment.cast::<IRawElementProviderSimple>() else {
        return;
    };
    let old = props::empty_variant();
    let new = props::empty_variant();
    // SAFETY: `provider` is a live COM object for the just-updated node; UIA
    // re-reads the current value via `GetPropertyValue` rather than trusting
    // the old/new payload, so empty placeholders are sufficient here.
    unsafe {
        let _ = UiaRaiseAutomationPropertyChangedEvent(&provider, property, &old, &new);
    }
}

/// Maps a normalized [`Role`] to the UIA control type mockapp serves for it
/// — the inverse of `verbatim_uia::map::role_from_control_type`, restricted
/// to the roles that map cleanly in both directions. A fixture author who
/// picks [`Role::Dialog`] or [`Role::PropertyPage`] gets [`Role::Window`]
/// back on read, because UIA has no distinct control type for either; that
/// mirrors UIA itself (and `verbatim_uia`'s own forward map), not a mockapp
/// gap.
fn role_to_control_type(role: Role) -> i32 {
    match role {
        Role::Button => UIA_ButtonControlTypeId.0,
        Role::CheckBox => UIA_CheckBoxControlTypeId.0,
        Role::ComboBox => UIA_ComboBoxControlTypeId.0,
        Role::EditableText => UIA_EditControlTypeId.0,
        Role::Slider => UIA_SliderControlTypeId.0,
        Role::SpinButton => UIA_SpinnerControlTypeId.0,
        Role::List => UIA_ListControlTypeId.0,
        Role::ListItem => UIA_ListItemControlTypeId.0,
        Role::Menu => UIA_MenuControlTypeId.0,
        Role::MenuBar => UIA_MenuBarControlTypeId.0,
        Role::MenuItem => UIA_MenuItemControlTypeId.0,
        Role::Window | Role::Dialog | Role::PropertyPage => UIA_WindowControlTypeId.0,
        Role::StaticText => UIA_TextControlTypeId.0,
        Role::TabControl => UIA_TabControlTypeId.0,
        Role::Tab => UIA_TabItemControlTypeId.0,
        Role::Link => UIA_HyperlinkControlTypeId.0,
        Role::ToolBar => UIA_ToolBarControlTypeId.0,
        Role::StatusBar => UIA_StatusBarControlTypeId.0,
        Role::Group => UIA_GroupControlTypeId.0,
        Role::RadioButton => UIA_RadioButtonControlTypeId.0,
        Role::Tree => UIA_TreeControlTypeId.0,
        Role::TreeItem => UIA_TreeItemControlTypeId.0,
        // `Role` is `#[non_exhaustive]`; Pane, Unknown, and anything added
        // later serve as an unstyled pane rather than failing to answer.
        _ => UIA_PaneControlTypeId.0,
    }
}

/// Shared, tree-reading logic used by both [`RootProvider`] and
/// [`ChildProvider`]. Kept free of the `#[implement]` macro's generated glue
/// so it reads (and lints) like ordinary code.
mod props {
    use std::mem::ManuallyDrop;

    use verbatim_model::State;
    use windows::Win32::Foundation::{HWND, VARIANT_FALSE, VARIANT_TRUE};
    use windows::Win32::System::Com::SAFEARRAY;
    use windows::Win32::System::Ole::SafeArrayCreateVector;
    use windows::Win32::System::Ole::SafeArrayPutElement;
    use windows::Win32::System::Variant::{
        VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4,
    };
    use windows::Win32::UI::Accessibility::{
        ExpandCollapseState, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded,
        IRawElementProviderFragment, IRawElementProviderSimple, NavigateDirection,
        NavigateDirection_FirstChild, NavigateDirection_LastChild, NavigateDirection_NextSibling,
        NavigateDirection_Parent, NavigateDirection_PreviousSibling, ToggleState,
        ToggleState_Indeterminate, ToggleState_On, UIA_AccessKeyPropertyId,
        UIA_ControlTypePropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
        UIA_FullDescriptionPropertyId, UIA_HasKeyboardFocusPropertyId, UIA_IsEnabledPropertyId,
        UIA_IsExpandCollapsePatternAvailablePropertyId, UIA_IsKeyboardFocusablePropertyId,
        UIA_IsOffscreenPropertyId, UIA_IsSelectionItemPatternAvailablePropertyId,
        UIA_IsTogglePatternAvailablePropertyId, UIA_LevelPropertyId, UIA_NamePropertyId,
        UIA_NativeWindowHandlePropertyId, UIA_PROPERTY_ID, UIA_PositionInSetPropertyId,
        UIA_ProcessIdPropertyId, UIA_SelectionItemIsSelectedPropertyId, UIA_SizeOfSetPropertyId,
        UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId, UiaAppendRuntimeId,
        UiaHostProviderFromHwnd,
    };
    use windows::core::{BSTR, Result as WinResult};
    use windows_core::Error;

    use super::{ChildProvider, RootProvider, role_to_control_type};
    use crate::tree::SharedTree;

    /// Builds the provider for `index`: the root gets a [`RootProvider`],
    /// every other node a [`ChildProvider`].
    pub(super) fn provider_for(
        tree: SharedTree,
        hwnd: HWND,
        index: usize,
    ) -> IRawElementProviderFragment {
        if index == 0 {
            RootProvider { tree, hwnd, index }.into()
        } else {
            ChildProvider { tree, hwnd, index }.into()
        }
    }

    pub(super) fn empty_variant() -> VARIANT {
        VARIANT::default()
    }

    fn i32_variant(value: i32) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_I4,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 { lVal: value },
                }),
            },
        }
    }

    fn bool_variant(value: bool) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_BOOL,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 {
                        boolVal: if value { VARIANT_TRUE } else { VARIANT_FALSE },
                    },
                }),
            },
        }
    }

    fn bstr_variant(text: &str) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_BSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 {
                        bstrVal: ManuallyDrop::new(BSTR::from(text)),
                    },
                }),
            },
        }
    }

    /// Whether `role` reports `TogglePattern` availability. Gated on role
    /// alone (rather than current state) so an unchecked box still reports
    /// availability, matching real toggle-capable controls.
    pub(super) fn toggle_available(role: verbatim_model::Role) -> bool {
        matches!(
            role,
            verbatim_model::Role::CheckBox | verbatim_model::Role::RadioButton
        )
    }

    /// Whether `states` reports `ExpandCollapsePattern` availability. Driven
    /// by the fixture's own `expanded`/`collapsed` states, since (unlike
    /// toggle) there is no single expand-capable role in the vocabulary.
    pub(super) fn expand_available(states: verbatim_model::StateSet) -> bool {
        states.contains(State::Expanded) || states.contains(State::Collapsed)
    }

    /// Whether `states` reports `SelectionItemPattern` availability. Driven
    /// by the fixture's `selectable` (or already-`selected`) states, like
    /// [`expand_available`].
    pub(super) fn selection_available(states: verbatim_model::StateSet) -> bool {
        states.contains(State::Selectable) || states.contains(State::Selected)
    }

    pub(super) fn get_property_value(
        tree: &SharedTree,
        hwnd: HWND,
        index: usize,
        property_id: UIA_PROPERTY_ID,
    ) -> VARIANT {
        let id = property_id.0;
        let guard = tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let node = &guard.nodes[index];
        if id == UIA_NamePropertyId.0 {
            node.name
                .as_deref()
                .map_or_else(empty_variant, bstr_variant)
        } else if id == UIA_ControlTypePropertyId.0 {
            i32_variant(role_to_control_type(node.role))
        } else if id == UIA_ValueValuePropertyId.0 {
            node.value
                .as_deref()
                .map_or_else(empty_variant, bstr_variant)
        } else if id == UIA_ProcessIdPropertyId.0 {
            i32_variant(i32::try_from(std::process::id()).unwrap_or(0))
        } else if id == UIA_NativeWindowHandlePropertyId.0 {
            if index == 0 {
                i32_variant(i32::try_from(hwnd.0 as isize).unwrap_or(0))
            } else {
                i32_variant(0)
            }
        } else if id == UIA_IsEnabledPropertyId.0 {
            bool_variant(!node.states.contains(State::Disabled))
        } else if id == UIA_HasKeyboardFocusPropertyId.0 {
            bool_variant(node.states.contains(State::Focused))
        } else if id == UIA_IsKeyboardFocusablePropertyId.0 {
            bool_variant(node.states.contains(State::Focusable))
        } else if id == UIA_IsOffscreenPropertyId.0 {
            bool_variant(node.states.contains(State::Offscreen))
        } else if id == UIA_IsTogglePatternAvailablePropertyId.0 {
            bool_variant(toggle_available(node.role))
        } else if id == UIA_ToggleToggleStatePropertyId.0 {
            i32_variant(toggle_state_value(node.states).0)
        } else if id == UIA_IsExpandCollapsePatternAvailablePropertyId.0 {
            bool_variant(expand_available(node.states))
        } else if id == UIA_ExpandCollapseExpandCollapseStatePropertyId.0 {
            i32_variant(expand_collapse_state_value(node.states).0)
        } else if id == UIA_IsSelectionItemPatternAvailablePropertyId.0 {
            bool_variant(selection_available(node.states))
        } else if id == UIA_SelectionItemIsSelectedPropertyId.0 {
            bool_variant(node.states.contains(State::Selected))
        } else if id == UIA_FullDescriptionPropertyId.0 {
            node.description
                .as_deref()
                .map_or_else(empty_variant, bstr_variant)
        } else if id == UIA_AccessKeyPropertyId.0 {
            node.keyboard_shortcut
                .as_deref()
                .map_or_else(empty_variant, bstr_variant)
        } else if id == UIA_PositionInSetPropertyId.0 {
            one_based_variant(node.position_in_set)
        } else if id == UIA_SizeOfSetPropertyId.0 {
            one_based_variant(node.set_size)
        } else if id == UIA_LevelPropertyId.0 {
            one_based_variant(node.level)
        } else {
            empty_variant()
        }
    }

    /// A one-based integer detail (`PositionInSet`, `SizeOfSet`, `Level`) as
    /// a `VT_I4` variant. `None` answers an empty variant, which UIA turns
    /// into the property's zero default on the client side — exactly the
    /// "not reported" convention `verbatim-uia`'s mapping treats as absent.
    fn one_based_variant(value: Option<u32>) -> VARIANT {
        value.map_or_else(empty_variant, |value| {
            i32_variant(i32::try_from(value).unwrap_or(0))
        })
    }

    /// The `ToggleState` for `states`, independent of whether the pattern is
    /// actually available — callers gate on [`toggle_available`] separately.
    pub(super) fn toggle_state_value(states: verbatim_model::StateSet) -> ToggleState {
        if states.contains(State::Checked) {
            ToggleState_On
        } else if states.contains(State::Mixed) {
            ToggleState_Indeterminate
        } else {
            ToggleState(0)
        }
    }

    /// The `ExpandCollapseState` for `states`, independent of whether the
    /// pattern is actually available — callers gate on [`expand_available`]
    /// separately.
    pub(super) fn expand_collapse_state_value(
        states: verbatim_model::StateSet,
    ) -> ExpandCollapseState {
        if states.contains(State::Expanded) {
            ExpandCollapseState_Expanded
        } else if states.contains(State::Collapsed) {
            ExpandCollapseState_Collapsed
        } else {
            ExpandCollapseState(0)
        }
    }

    pub(super) fn host_raw_element_provider(
        hwnd: HWND,
        index: usize,
    ) -> WinResult<IRawElementProviderSimple> {
        if index == 0 {
            // SAFETY: `hwnd` is the mockapp window's own handle, live for the
            // process's lifetime.
            unsafe { UiaHostProviderFromHwnd(hwnd) }
        } else {
            Err(Error::empty())
        }
    }

    pub(super) fn navigate(
        tree: &SharedTree,
        hwnd: HWND,
        index: usize,
        direction: NavigateDirection,
    ) -> WinResult<IRawElementProviderFragment> {
        let target = {
            let guard = tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if direction == NavigateDirection_Parent {
                guard.nodes[index].parent
            } else if direction == NavigateDirection_FirstChild {
                guard.nodes[index].children.first().copied()
            } else if direction == NavigateDirection_LastChild {
                guard.nodes[index].children.last().copied()
            } else if direction == NavigateDirection_NextSibling {
                guard.sibling(index, 1)
            } else if direction == NavigateDirection_PreviousSibling {
                guard.sibling(index, -1)
            } else {
                None
            }
        };
        target
            .map(|target_index| provider_for(tree.clone(), hwnd, target_index))
            .ok_or_else(Error::empty)
    }

    pub(super) fn get_runtime_id(index: usize) -> WinResult<*mut SAFEARRAY> {
        if index == 0 {
            // The hwnd-rooted element derives its runtime id from the window
            // handle automatically; returning null is the documented UIA
            // contract for that case.
            return Ok(std::ptr::null_mut());
        }
        // SAFETY: a freshly created two-element i32 SAFEARRAY, filled by
        // index within bounds; ownership passes to the caller, matching
        // `GetRuntimeId`'s documented contract.
        unsafe {
            let array = SafeArrayCreateVector(VT_I4, 0, 2);
            if array.is_null() {
                return Err(Error::empty());
            }
            let marker = i32::try_from(UiaAppendRuntimeId).unwrap_or(0);
            let unique = i32::try_from(index).unwrap_or(0);
            let zero: i32 = 0;
            let one: i32 = 1;
            let _ = SafeArrayPutElement(array, &raw const zero, (&raw const marker).cast());
            let _ = SafeArrayPutElement(array, &raw const one, (&raw const unique).cast());
            Ok(array)
        }
    }

    pub(super) fn get_focus(
        tree: &SharedTree,
        hwnd: HWND,
    ) -> WinResult<IRawElementProviderFragment> {
        let focused = tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .focused;
        focused
            .map(|index| provider_for(tree.clone(), hwnd, index))
            .ok_or_else(Error::empty)
    }
}

/// The `#[implement]`-generated COM objects live in their own module so the
/// module-level allow covers the macro's generated glue, matching the
/// pattern `verbatim-uia`'s `focus` and `events` modules use.
mod handler {
    #![allow(
        clippy::inline_always,
        clippy::ref_as_ptr,
        clippy::used_underscore_binding
    )]

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::SAFEARRAY;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IExpandCollapseProvider, IExpandCollapseProvider_Impl, IRawElementProviderFragment,
        IRawElementProviderFragment_Impl, IRawElementProviderFragmentRoot,
        IRawElementProviderFragmentRoot_Impl, IRawElementProviderSimple,
        IRawElementProviderSimple_Impl, ISelectionItemProvider, ISelectionItemProvider_Impl,
        IToggleProvider, IToggleProvider_Impl, NavigateDirection, ProviderOptions,
        ProviderOptions_ServerSideProvider, ProviderOptions_UseComThreading,
        UIA_ExpandCollapsePatternId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_SelectionItemPatternId,
        UIA_TogglePatternId, UIA_ValuePatternId, UiaRect,
    };
    use windows::core::Result as WinResult;
    use windows_core::{Error, IUnknown, implement};

    use super::props;
    use crate::tree::SharedTree;

    /// The provider for the root node (index 0), which also answers as the
    /// fragment root — the only node that does, matching real hwnd-rooted
    /// providers.
    #[implement(
        IRawElementProviderSimple,
        IRawElementProviderFragment,
        IRawElementProviderFragmentRoot,
        Agile = false
    )]
    pub(crate) struct RootProvider {
        pub(crate) tree: SharedTree,
        pub(crate) hwnd: HWND,
        pub(crate) index: usize,
    }

    /// The provider for every non-root node.
    #[implement(IRawElementProviderSimple, IRawElementProviderFragment, Agile = false)]
    pub(crate) struct ChildProvider {
        pub(crate) tree: SharedTree,
        pub(crate) hwnd: HWND,
        pub(crate) index: usize,
    }

    impl IRawElementProviderSimple_Impl for RootProvider_Impl {
        fn ProviderOptions(&self) -> WinResult<ProviderOptions> {
            Ok(provider_options())
        }
        fn GetPatternProvider(&self, pattern_id: UIA_PATTERN_ID) -> WinResult<IUnknown> {
            get_pattern_provider(&self.tree, self.index, pattern_id)
        }
        fn GetPropertyValue(&self, property_id: UIA_PROPERTY_ID) -> WinResult<VARIANT> {
            Ok(props::get_property_value(
                &self.tree,
                self.hwnd,
                self.index,
                property_id,
            ))
        }
        fn HostRawElementProvider(&self) -> WinResult<IRawElementProviderSimple> {
            props::host_raw_element_provider(self.hwnd, self.index)
        }
    }

    impl IRawElementProviderFragment_Impl for RootProvider_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> WinResult<IRawElementProviderFragment> {
            props::navigate(&self.tree, self.hwnd, self.index, direction)
        }
        fn GetRuntimeId(&self) -> WinResult<*mut SAFEARRAY> {
            props::get_runtime_id(self.index)
        }
        fn BoundingRectangle(&self) -> WinResult<UiaRect> {
            Ok(bounding_rectangle())
        }
        fn GetEmbeddedFragmentRoots(&self) -> WinResult<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }
        fn SetFocus(&self) -> WinResult<()> {
            Ok(())
        }
        fn FragmentRoot(&self) -> WinResult<IRawElementProviderFragmentRoot> {
            Ok(RootProvider {
                tree: self.tree.clone(),
                hwnd: self.hwnd,
                index: 0,
            }
            .into())
        }
    }

    impl IRawElementProviderFragmentRoot_Impl for RootProvider_Impl {
        fn ElementProviderFromPoint(
            &self,
            _x: f64,
            _y: f64,
        ) -> WinResult<IRawElementProviderFragment> {
            // Bounding rectangles are always zero (mockapp never shows real
            // control layout), so point-based hit testing has nothing
            // meaningful to answer.
            Err(Error::empty())
        }
        fn GetFocus(&self) -> WinResult<IRawElementProviderFragment> {
            props::get_focus(&self.tree, self.hwnd)
        }
    }

    impl IRawElementProviderSimple_Impl for ChildProvider_Impl {
        fn ProviderOptions(&self) -> WinResult<ProviderOptions> {
            Ok(provider_options())
        }
        fn GetPatternProvider(&self, pattern_id: UIA_PATTERN_ID) -> WinResult<IUnknown> {
            get_pattern_provider(&self.tree, self.index, pattern_id)
        }
        fn GetPropertyValue(&self, property_id: UIA_PROPERTY_ID) -> WinResult<VARIANT> {
            Ok(props::get_property_value(
                &self.tree,
                self.hwnd,
                self.index,
                property_id,
            ))
        }
        fn HostRawElementProvider(&self) -> WinResult<IRawElementProviderSimple> {
            props::host_raw_element_provider(self.hwnd, self.index)
        }
    }

    impl IRawElementProviderFragment_Impl for ChildProvider_Impl {
        fn Navigate(&self, direction: NavigateDirection) -> WinResult<IRawElementProviderFragment> {
            props::navigate(&self.tree, self.hwnd, self.index, direction)
        }
        fn GetRuntimeId(&self) -> WinResult<*mut SAFEARRAY> {
            props::get_runtime_id(self.index)
        }
        fn BoundingRectangle(&self) -> WinResult<UiaRect> {
            Ok(bounding_rectangle())
        }
        fn GetEmbeddedFragmentRoots(&self) -> WinResult<*mut SAFEARRAY> {
            Ok(std::ptr::null_mut())
        }
        fn SetFocus(&self) -> WinResult<()> {
            Ok(())
        }
        fn FragmentRoot(&self) -> WinResult<IRawElementProviderFragmentRoot> {
            Ok(RootProvider {
                tree: self.tree.clone(),
                hwnd: self.hwnd,
                index: 0,
            }
            .into())
        }
    }

    fn provider_options() -> ProviderOptions {
        ProviderOptions(ProviderOptions_ServerSideProvider.0 | ProviderOptions_UseComThreading.0)
    }

    /// mockapp *also* answers pattern-availability and pattern-value
    /// properties directly through `GetPropertyValue` (the documented,
    /// optional shortcut UIA offers precisely so simple providers do not
    /// need pattern objects) — but empirically, `IUIAutomationCacheRequest`
    /// building a cache still asks `GetPatternProvider` for the toggle and
    /// expand-collapse patterns before trusting a cached state, so a real
    /// pattern object is required for cached property fetches (which is what
    /// `verbatim-uia`'s base cache request always does) to see the current
    /// state at all.
    fn get_pattern_provider(
        tree: &SharedTree,
        index: usize,
        pattern_id: UIA_PATTERN_ID,
    ) -> WinResult<IUnknown> {
        let (role, states, has_value) = {
            let guard = tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let node = &guard.nodes[index];
            (node.role, node.states, node.value.is_some())
        };
        if pattern_id == UIA_TogglePatternId && props::toggle_available(role) {
            let provider: IUnknown = ToggleProvider {
                tree: tree.clone(),
                index,
            }
            .into();
            return Ok(provider);
        }
        if pattern_id == UIA_ExpandCollapsePatternId && props::expand_available(states) {
            let provider: IUnknown = ExpandCollapseProvider {
                tree: tree.clone(),
                index,
            }
            .into();
            return Ok(provider);
        }
        if pattern_id == UIA_ValuePatternId && has_value {
            let provider: IUnknown = ValueProvider {
                tree: tree.clone(),
                index,
            }
            .into();
            return Ok(provider);
        }
        if pattern_id == UIA_SelectionItemPatternId && props::selection_available(states) {
            let provider: IUnknown = SelectionItemProvider {
                tree: tree.clone(),
                index,
            }
            .into();
            return Ok(provider);
        }
        Err(Error::empty())
    }

    fn bounding_rectangle() -> UiaRect {
        UiaRect {
            left: 0.0,
            top: 0.0,
            width: 0.0,
            height: 0.0,
        }
    }

    /// The `TogglePattern` provider for a checkbox or radio button node.
    /// `Toggle` is a no-op: mockapp's scripted state changes only via
    /// stdin commands (`set-name`/`set-value`), never through pattern
    /// invocation, so there is nothing to flip.
    #[implement(IToggleProvider, Agile = false)]
    struct ToggleProvider {
        tree: SharedTree,
        index: usize,
    }

    impl IToggleProvider_Impl for ToggleProvider_Impl {
        fn Toggle(&self) -> WinResult<()> {
            Ok(())
        }
        fn ToggleState(&self) -> WinResult<windows::Win32::UI::Accessibility::ToggleState> {
            let states = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .nodes[self.index]
                .states;
            Ok(props::toggle_state_value(states))
        }
    }

    /// The `ExpandCollapsePattern` provider for a node whose fixture states
    /// include `expanded` or `collapsed`. `Expand`/`Collapse` are no-ops for
    /// the same reason `Toggle` is.
    #[implement(IExpandCollapseProvider, Agile = false)]
    struct ExpandCollapseProvider {
        tree: SharedTree,
        index: usize,
    }

    impl IExpandCollapseProvider_Impl for ExpandCollapseProvider_Impl {
        fn Expand(&self) -> WinResult<()> {
            Ok(())
        }
        fn Collapse(&self) -> WinResult<()> {
            Ok(())
        }
        fn ExpandCollapseState(
            &self,
        ) -> WinResult<windows::Win32::UI::Accessibility::ExpandCollapseState> {
            let states = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .nodes[self.index]
                .states;
            Ok(props::expand_collapse_state_value(states))
        }
    }

    /// The `ValuePattern` provider for any node whose fixture supplies a
    /// `value`. `SetValue` is a no-op for the same reason `Toggle` is.
    #[implement(windows::Win32::UI::Accessibility::IValueProvider, Agile = false)]
    struct ValueProvider {
        tree: SharedTree,
        index: usize,
    }

    impl windows::Win32::UI::Accessibility::IValueProvider_Impl for ValueProvider_Impl {
        fn SetValue(&self, _val: &windows_core::PCWSTR) -> WinResult<()> {
            Ok(())
        }
        fn Value(&self) -> WinResult<windows_core::BSTR> {
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(guard.nodes[self.index]
                .value
                .as_deref()
                .unwrap_or("")
                .into())
        }
        fn IsReadOnly(&self) -> WinResult<windows_core::BOOL> {
            let read_only = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .nodes[self.index]
                .states
                .contains(verbatim_model::State::ReadOnly);
            Ok(read_only.into())
        }
    }

    /// The `SelectionItemPattern` provider for a node whose fixture states
    /// include `selectable` or `selected`. The mutating methods are no-ops
    /// for the same reason `Toggle` is: mockapp's scripted state changes
    /// only via stdin commands (`select`), never through pattern
    /// invocation. Like toggle and expand-collapse above, a real pattern
    /// object exists because `IUIAutomationCacheRequest`'s cache building
    /// asks `GetPatternProvider` before trusting a cached
    /// `SelectionItemIsSelected` value.
    #[implement(ISelectionItemProvider, Agile = false)]
    struct SelectionItemProvider {
        tree: SharedTree,
        index: usize,
    }

    impl ISelectionItemProvider_Impl for SelectionItemProvider_Impl {
        fn Select(&self) -> WinResult<()> {
            Ok(())
        }
        fn AddToSelection(&self) -> WinResult<()> {
            Ok(())
        }
        fn RemoveFromSelection(&self) -> WinResult<()> {
            Ok(())
        }
        fn IsSelected(&self) -> WinResult<windows_core::BOOL> {
            let selected = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .nodes[self.index]
                .states
                .contains(verbatim_model::State::Selected);
            Ok(selected.into())
        }
        fn SelectionContainer(&self) -> WinResult<IRawElementProviderSimple> {
            // The containing list is reachable through ordinary navigation;
            // a null container is the UIA contract for "not exposed", same
            // as the other legitimate-null results this module documents.
            Err(Error::empty())
        }
    }
}
