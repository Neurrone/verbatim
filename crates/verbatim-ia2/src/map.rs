//! Mapping from MSAA roles and state bits into the normalized model
//! (architecture section 3). Both functions are pure and unit-tested; the
//! acquisition code in [`crate::acquire`] feeds them values read from
//! `IAccessible`.

use verbatim_model::{Role, State, StateSet};
use windows::Win32::UI::Accessibility::{
    ROLE_SYSTEM_ALERT, ROLE_SYSTEM_APPLICATION, ROLE_SYSTEM_BUTTONDROPDOWN, ROLE_SYSTEM_BUTTONMENU,
    ROLE_SYSTEM_CELL, ROLE_SYSTEM_CHECKBUTTON, ROLE_SYSTEM_CLIENT, ROLE_SYSTEM_COLUMNHEADER,
    ROLE_SYSTEM_COMBOBOX, ROLE_SYSTEM_DIALOG, ROLE_SYSTEM_DOCUMENT, ROLE_SYSTEM_GRAPHIC,
    ROLE_SYSTEM_GROUPING, ROLE_SYSTEM_HOTKEYFIELD, ROLE_SYSTEM_LINK, ROLE_SYSTEM_LIST,
    ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_MENUBAR, ROLE_SYSTEM_MENUITEM, ROLE_SYSTEM_MENUPOPUP,
    ROLE_SYSTEM_OUTLINE, ROLE_SYSTEM_OUTLINEITEM, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETABLIST,
    ROLE_SYSTEM_PANE, ROLE_SYSTEM_PROGRESSBAR, ROLE_SYSTEM_PROPERTYPAGE, ROLE_SYSTEM_PUSHBUTTON,
    ROLE_SYSTEM_RADIOBUTTON, ROLE_SYSTEM_ROW, ROLE_SYSTEM_ROWHEADER, ROLE_SYSTEM_SCROLLBAR,
    ROLE_SYSTEM_SEPARATOR, ROLE_SYSTEM_SLIDER, ROLE_SYSTEM_SPINBUTTON, ROLE_SYSTEM_SPLITBUTTON,
    ROLE_SYSTEM_STATICTEXT, ROLE_SYSTEM_STATUSBAR, ROLE_SYSTEM_TABLE, ROLE_SYSTEM_TEXT,
    ROLE_SYSTEM_TITLEBAR, ROLE_SYSTEM_TOOLBAR, ROLE_SYSTEM_TOOLTIP, ROLE_SYSTEM_WINDOW,
};
// MSAA `STATE_SYSTEM_*` bit values (winuser.h). These are frozen ABI constants;
// the `windows` crate splits them across three feature-gated modules and types
// four of them as a combo-box newtype, so they are defined here as plain `u32`
// bits to keep the mapping self-contained and readable.
const STATE_SYSTEM_UNAVAILABLE: u32 = 0x0000_0001;
const STATE_SYSTEM_SELECTED: u32 = 0x0000_0002;
const STATE_SYSTEM_FOCUSED: u32 = 0x0000_0004;
const STATE_SYSTEM_PRESSED: u32 = 0x0000_0008;
const STATE_SYSTEM_CHECKED: u32 = 0x0000_0010;
const STATE_SYSTEM_MIXED: u32 = 0x0000_0020;
const STATE_SYSTEM_READONLY: u32 = 0x0000_0040;
const STATE_SYSTEM_EXPANDED: u32 = 0x0000_0200;
const STATE_SYSTEM_COLLAPSED: u32 = 0x0000_0400;
const STATE_SYSTEM_BUSY: u32 = 0x0000_0800;
const STATE_SYSTEM_OFFSCREEN: u32 = 0x0001_0000;
const STATE_SYSTEM_FOCUSABLE: u32 = 0x0010_0000;
const STATE_SYSTEM_SELECTABLE: u32 = 0x0020_0000;
const STATE_SYSTEM_PROTECTED: u32 = 0x2000_0000;
const STATE_SYSTEM_HASPOPUP: u32 = 0x4000_0000;
/// Invisible, which the model has no state for; the dialog text reads it.
pub(crate) const STATE_SYSTEM_INVISIBLE: u32 = 0x0000_8000;

/// Maps an MSAA `ROLE_SYSTEM_*` value to a normalized [`Role`], as NVDA's
/// MSAA role table does. Unmapped roles become [`Role::Unknown`].
#[must_use]
pub fn role_from_msaa(role: u32) -> Role {
    match role {
        ROLE_SYSTEM_PUSHBUTTON => Role::Button,
        ROLE_SYSTEM_CHECKBUTTON => Role::CheckBox,
        ROLE_SYSTEM_COMBOBOX => Role::ComboBox,
        ROLE_SYSTEM_SLIDER => Role::Slider,
        ROLE_SYSTEM_LISTITEM => Role::ListItem,
        ROLE_SYSTEM_LIST => Role::List,
        ROLE_SYSTEM_MENUITEM => Role::MenuItem,
        ROLE_SYSTEM_MENUPOPUP => Role::Menu,
        ROLE_SYSTEM_DIALOG => Role::Dialog,
        ROLE_SYSTEM_WINDOW => Role::Window,
        // A window's client area, as NVDA maps it: a pane, which says only
        // its name when focused.
        ROLE_SYSTEM_CLIENT | ROLE_SYSTEM_PANE => Role::Pane,
        ROLE_SYSTEM_STATICTEXT => Role::StaticText,
        ROLE_SYSTEM_TEXT => Role::EditableText,
        ROLE_SYSTEM_PROPERTYPAGE => Role::PropertyPage,
        ROLE_SYSTEM_GROUPING => Role::Group,
        ROLE_SYSTEM_SPINBUTTON => Role::SpinButton,
        ROLE_SYSTEM_RADIOBUTTON => Role::RadioButton,
        ROLE_SYSTEM_LINK => Role::Link,
        ROLE_SYSTEM_TOOLBAR => Role::ToolBar,
        ROLE_SYSTEM_STATUSBAR => Role::StatusBar,
        ROLE_SYSTEM_PAGETABLIST => Role::TabControl,
        ROLE_SYSTEM_PAGETAB => Role::Tab,
        ROLE_SYSTEM_OUTLINE => Role::Tree,
        ROLE_SYSTEM_OUTLINEITEM => Role::TreeItem,
        ROLE_SYSTEM_MENUBAR => Role::MenuBar,
        ROLE_SYSTEM_TITLEBAR => Role::TitleBar,
        ROLE_SYSTEM_GRAPHIC => Role::Graphic,
        ROLE_SYSTEM_TOOLTIP => Role::ToolTip,
        ROLE_SYSTEM_PROGRESSBAR => Role::ProgressBar,
        ROLE_SYSTEM_SCROLLBAR => Role::ScrollBar,
        ROLE_SYSTEM_TABLE => Role::Table,
        ROLE_SYSTEM_ROW => Role::Row,
        ROLE_SYSTEM_CELL => Role::Cell,
        ROLE_SYSTEM_COLUMNHEADER => Role::ColumnHeader,
        ROLE_SYSTEM_ROWHEADER => Role::RowHeader,
        ROLE_SYSTEM_SPLITBUTTON => Role::SplitButton,
        ROLE_SYSTEM_BUTTONDROPDOWN => Role::DropDownButton,
        ROLE_SYSTEM_BUTTONMENU => Role::MenuButton,
        ROLE_SYSTEM_SEPARATOR => Role::Separator,
        ROLE_SYSTEM_DOCUMENT => Role::Document,
        ROLE_SYSTEM_APPLICATION => Role::Application,
        ROLE_SYSTEM_ALERT => Role::Alert,
        ROLE_SYSTEM_HOTKEYFIELD => Role::HotkeyField,
        _ => Role::Unknown,
    }
}

/// Maps an MSAA state bitmask (`accState`) to a normalized [`StateSet`], as
/// NVDA's MSAA state table does: `STATE_SYSTEM_DEFAULT` has no counterpart
/// there, so a dialog's default button is not announced as such.
#[must_use]
pub fn states_from_msaa(state: u32) -> StateSet {
    let mut states = StateSet::new();
    let mut set = |bit: u32, mapped: State| {
        if state & bit != 0 {
            states.insert(mapped);
        }
    };
    set(STATE_SYSTEM_FOCUSED, State::Focused);
    set(STATE_SYSTEM_FOCUSABLE, State::Focusable);
    set(STATE_SYSTEM_SELECTED, State::Selected);
    set(STATE_SYSTEM_SELECTABLE, State::Selectable);
    set(STATE_SYSTEM_CHECKED, State::Checked);
    set(STATE_SYSTEM_MIXED, State::Mixed);
    set(STATE_SYSTEM_UNAVAILABLE, State::Disabled);
    set(STATE_SYSTEM_READONLY, State::ReadOnly);
    set(STATE_SYSTEM_EXPANDED, State::Expanded);
    set(STATE_SYSTEM_COLLAPSED, State::Collapsed);
    set(STATE_SYSTEM_PRESSED, State::Pressed);
    set(STATE_SYSTEM_HASPOPUP, State::HasPopup);
    set(STATE_SYSTEM_PROTECTED, State::Protected);
    set(STATE_SYSTEM_OFFSCREEN, State::Offscreen);
    set(STATE_SYSTEM_BUSY, State::Busy);
    states
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_roles_map_as_specified() {
        assert_eq!(role_from_msaa(ROLE_SYSTEM_PUSHBUTTON), Role::Button);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_CHECKBUTTON), Role::CheckBox);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_MENUPOPUP), Role::Menu);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_TEXT), Role::EditableText);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_PAGETABLIST), Role::TabControl);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_PAGETAB), Role::Tab);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_OUTLINE), Role::Tree);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_OUTLINEITEM), Role::TreeItem);
    }

    #[test]
    fn roles_nvda_maps_are_not_unknown() {
        assert_eq!(role_from_msaa(ROLE_SYSTEM_SPLITBUTTON), Role::SplitButton);
        assert_eq!(
            role_from_msaa(ROLE_SYSTEM_BUTTONDROPDOWN),
            Role::DropDownButton
        );
        assert_eq!(role_from_msaa(ROLE_SYSTEM_GRAPHIC), Role::Graphic);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_PROGRESSBAR), Role::ProgressBar);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_MENUBAR), Role::MenuBar);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_PANE), Role::Pane);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_DOCUMENT), Role::Document);
        assert_eq!(role_from_msaa(ROLE_SYSTEM_CELL), Role::Cell);
    }

    #[test]
    fn default_is_dropped_and_protected_kept() {
        let states = states_from_msaa(0x0000_0100 | STATE_SYSTEM_PROTECTED);
        assert_eq!(states, StateSet::new().with(State::Protected));
    }

    #[test]
    fn unmapped_role_is_unknown() {
        assert_eq!(role_from_msaa(0), Role::Unknown);
        assert_eq!(role_from_msaa(9999), Role::Unknown);
    }

    #[test]
    fn focused_focusable_states_combine() {
        let states = states_from_msaa(STATE_SYSTEM_FOCUSED | STATE_SYSTEM_FOCUSABLE);
        assert!(states.contains(State::Focused));
        assert!(states.contains(State::Focusable));
        assert!(!states.contains(State::Checked));
    }

    #[test]
    fn unavailable_maps_to_disabled_and_mixed_to_mixed() {
        let states = states_from_msaa(STATE_SYSTEM_UNAVAILABLE | STATE_SYSTEM_MIXED);
        assert!(states.contains(State::Disabled));
        assert!(states.contains(State::Mixed));
    }

    /// A focused, hot-tracked menu item reports `accState = 0x00100084`
    /// (FOCUSED | HOTTRACKED | FOCUSABLE), captured live from a real menu. It
    /// must map to exactly Focused + Focusable — no spurious Mixed from the
    /// adjacent bits. Hot-tracked has no normalized state and is dropped.
    #[test]
    fn focused_hot_tracked_menu_item_is_not_mixed() {
        let states = states_from_msaa(0x0010_0084);
        assert!(states.contains(State::Focused));
        assert!(states.contains(State::Focusable));
        assert!(!states.contains(State::Mixed));
        assert!(!states.contains(State::Checked));
        assert!(!states.contains(State::Pressed));
    }

    /// A selected list item reports `accState = 0x00300002`
    /// (SELECTED | FOCUSABLE | SELECTABLE), captured live from a real list. It
    /// must map to exactly Selected + Focusable + Selectable, no Mixed.
    #[test]
    fn selected_list_item_is_not_mixed() {
        let states = states_from_msaa(0x0030_0002);
        assert!(states.contains(State::Selected));
        assert!(states.contains(State::Focusable));
        assert!(states.contains(State::Selectable));
        assert!(!states.contains(State::Mixed));
        assert!(!states.contains(State::Checked));
    }

    /// A genuinely tri-state (indeterminate) check box does carry Mixed.
    #[test]
    fn indeterminate_check_box_is_mixed() {
        let states = states_from_msaa(STATE_SYSTEM_FOCUSABLE | STATE_SYSTEM_MIXED);
        assert!(states.contains(State::Mixed));
        assert!(!states.contains(State::Checked));
    }
}
