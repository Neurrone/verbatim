//! The scripted MSAA provider (architecture section 13, layer 2).
//!
//! Every node gets its own `IAccessible` COM object on demand, addressed
//! two ways: the window's default client object (`OBJID_CLIENT`) is always
//! the root, and every node additionally answers `WM_GETOBJECT` under a
//! custom, positive object id (`index + 1`, since positive ids are reserved
//! for application use per the MSAA object-identifier convention) so
//! `NotifyWinEvent` can address any node directly without needing per-node
//! `HWND`s. Deliberately does not answer `WM_GETOBJECT` for the UIA root
//! object id, so [`verbatim_uia::has_server_side_provider`] finds nothing
//! and the window arbitrates to MSAA.
//!
//! Roles and states are the inverse of the tables in `verbatim_ia2::map`.

use verbatim_model::{Role, State, StateSet};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::NotifyWinEvent;
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_SELECTION, EVENT_OBJECT_VALUECHANGE,
};

use crate::stdin::Command;
use crate::tree::SharedTree;

pub(crate) use handler::NodeAccessible;

/// The `idChild` MSAA always uses for these objects: every node is its own
/// full object (never addressed as a numbered simple child of another), so
/// self-reference is the only child id ever needed at the `WinEvent` boundary.
const CHILDID_SELF: i32 = 0;

/// Builds the accessible object for `index`.
pub(crate) fn node_accessible(tree: SharedTree, hwnd: HWND, index: usize) -> NodeAccessible {
    NodeAccessible { tree, hwnd, index }
}

/// The custom `idObject` mockapp answers `WM_GETOBJECT` with for `index`,
/// letting events address any node directly. Always positive, since
/// negative values are reserved for the standard object identifiers
/// (`OBJID_CLIENT` and friends).
pub(crate) fn objid_for(index: usize) -> i32 {
    i32::try_from(index + 1).unwrap_or(i32::MAX)
}

/// The inverse of [`objid_for`], for `WM_GETOBJECT` dispatch.
pub(crate) fn index_from_objid(objid: i32) -> Option<usize> {
    (objid > 0)
        .then(|| usize::try_from(objid - 1).ok())
        .flatten()
}

/// Applies a parsed stdin [`Command`] against `tree` and raises the matching
/// `WinEvent`. Runs on the window thread.
pub(crate) fn apply_command(tree: &SharedTree, hwnd: HWND, command: Command) {
    match command {
        Command::Focus(id) => {
            if let Some(index) = focus_node(tree, &id) {
                notify(hwnd, EVENT_OBJECT_FOCUS, index);
            }
        }
        Command::SetName(id, text) => {
            if let Some(index) = set_name(tree, &id, text) {
                notify(hwnd, EVENT_OBJECT_NAMECHANGE, index);
            }
        }
        Command::SetValue(id, text) => {
            if let Some(index) = set_value(tree, &id, text) {
                notify(hwnd, EVENT_OBJECT_VALUECHANGE, index);
            }
        }
        Command::Select(id) => {
            if let Some(index) = select_node(tree, &id) {
                notify(hwnd, EVENT_OBJECT_SELECTION, index);
            }
        }
        Command::Notify(_) => {
            // MSAA has no notification event; `notify` is a UIA-backend
            // command (see crate::stdin::Command::Notify).
            eprintln!("mockapp: notify is not supported on the msaa backend");
        }
        Command::Quit => {}
    }
}

fn focus_node(tree: &SharedTree, id: &str) -> Option<usize> {
    let mut guard = tree
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = guard.index_of(id)?;
    if let Some(previous) = guard.focused.replace(index) {
        guard.nodes[previous].states.remove(State::Focused);
    }
    guard.nodes[index].states.insert(State::Focused);
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
        guard.nodes[previous].states.remove(State::Selected);
    }
    guard.nodes[index].states.insert(State::Selected);
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

fn notify(hwnd: HWND, event: u32, index: usize) {
    // SAFETY: `hwnd` is the mockapp window's own live handle; `objid_for`
    // names an object this window's `WM_GETOBJECT` handler answers.
    unsafe {
        NotifyWinEvent(event, hwnd, objid_for(index), CHILDID_SELF);
    }
}

/// The inverse of `verbatim_ia2::map::role_from_msaa`, restricted to roles
/// that map cleanly in both directions. A fixture author who picks
/// [`Role::Pane`] or [`Role::MenuBar`] gets [`Role::Unknown`] back on read,
/// because `verbatim_ia2`'s forward table does not cover those MSAA roles
/// yet (M1 scope); that mirrors the client stack, not a mockapp gap.
fn role_to_msaa(role: Role) -> u32 {
    use windows::Win32::UI::Accessibility::{
        ROLE_SYSTEM_CHECKBUTTON, ROLE_SYSTEM_COMBOBOX, ROLE_SYSTEM_DIALOG, ROLE_SYSTEM_GROUPING,
        ROLE_SYSTEM_LINK, ROLE_SYSTEM_LIST, ROLE_SYSTEM_LISTITEM, ROLE_SYSTEM_MENUITEM,
        ROLE_SYSTEM_MENUPOPUP, ROLE_SYSTEM_PAGETAB, ROLE_SYSTEM_PAGETABLIST,
        ROLE_SYSTEM_PROPERTYPAGE, ROLE_SYSTEM_PUSHBUTTON, ROLE_SYSTEM_RADIOBUTTON,
        ROLE_SYSTEM_SLIDER, ROLE_SYSTEM_SPINBUTTON, ROLE_SYSTEM_STATICTEXT, ROLE_SYSTEM_STATUSBAR,
        ROLE_SYSTEM_TEXT, ROLE_SYSTEM_TOOLBAR, ROLE_SYSTEM_WINDOW,
    };
    match role {
        Role::Button => ROLE_SYSTEM_PUSHBUTTON,
        Role::CheckBox => ROLE_SYSTEM_CHECKBUTTON,
        Role::ComboBox => ROLE_SYSTEM_COMBOBOX,
        Role::Slider => ROLE_SYSTEM_SLIDER,
        Role::ListItem => ROLE_SYSTEM_LISTITEM,
        Role::List => ROLE_SYSTEM_LIST,
        Role::MenuItem => ROLE_SYSTEM_MENUITEM,
        Role::Menu => ROLE_SYSTEM_MENUPOPUP,
        Role::Dialog => ROLE_SYSTEM_DIALOG,
        Role::Window => ROLE_SYSTEM_WINDOW,
        Role::StaticText => ROLE_SYSTEM_STATICTEXT,
        Role::EditableText => ROLE_SYSTEM_TEXT,
        Role::PropertyPage => ROLE_SYSTEM_PROPERTYPAGE,
        Role::Group => ROLE_SYSTEM_GROUPING,
        Role::SpinButton => ROLE_SYSTEM_SPINBUTTON,
        Role::RadioButton => ROLE_SYSTEM_RADIOBUTTON,
        Role::Link => ROLE_SYSTEM_LINK,
        Role::ToolBar => ROLE_SYSTEM_TOOLBAR,
        Role::StatusBar => ROLE_SYSTEM_STATUSBAR,
        Role::TabControl => ROLE_SYSTEM_PAGETABLIST,
        Role::Tab => ROLE_SYSTEM_PAGETAB,
        // `Role` is `#[non_exhaustive]`; Pane, MenuBar, Unknown, and
        // anything added later have no MSAA counterpart in the current
        // `verbatim_ia2` forward map.
        _ => 0,
    }
}

// MSAA `STATE_SYSTEM_*` bit values (winuser.h), mirroring the private
// constants in `verbatim_ia2::map`; redefined here since that module is not
// public and mockapp intentionally does not depend on `verbatim-ia2` from
// its shipped binary (only from its tests).
const STATE_SYSTEM_UNAVAILABLE: u32 = 0x0000_0001;
const STATE_SYSTEM_SELECTED: u32 = 0x0000_0002;
const STATE_SYSTEM_FOCUSED: u32 = 0x0000_0004;
const STATE_SYSTEM_PRESSED: u32 = 0x0000_0008;
const STATE_SYSTEM_CHECKED: u32 = 0x0000_0010;
const STATE_SYSTEM_MIXED: u32 = 0x0000_0020;
const STATE_SYSTEM_READONLY: u32 = 0x0000_0040;
const STATE_SYSTEM_DEFAULT: u32 = 0x0000_0100;
const STATE_SYSTEM_EXPANDED: u32 = 0x0000_0200;
const STATE_SYSTEM_COLLAPSED: u32 = 0x0000_0400;
const STATE_SYSTEM_BUSY: u32 = 0x0000_0800;
const STATE_SYSTEM_OFFSCREEN: u32 = 0x0001_0000;
const STATE_SYSTEM_FOCUSABLE: u32 = 0x0010_0000;
const STATE_SYSTEM_SELECTABLE: u32 = 0x0020_0000;
const STATE_SYSTEM_HASPOPUP: u32 = 0x4000_0000;

/// The inverse of `verbatim_ia2::map::states_from_msaa`: every [`State`]
/// that table maps has a bit here, so MSAA state fidelity round-trips
/// completely (unlike roles, MSAA state bits cover the whole vocabulary).
fn states_to_msaa(states: StateSet) -> u32 {
    let mut bits = 0;
    let mut set = |state: State, bit: u32| {
        if states.contains(state) {
            bits |= bit;
        }
    };
    set(State::Focused, STATE_SYSTEM_FOCUSED);
    set(State::Focusable, STATE_SYSTEM_FOCUSABLE);
    set(State::Selected, STATE_SYSTEM_SELECTED);
    set(State::Selectable, STATE_SYSTEM_SELECTABLE);
    set(State::Checked, STATE_SYSTEM_CHECKED);
    set(State::Mixed, STATE_SYSTEM_MIXED);
    set(State::Disabled, STATE_SYSTEM_UNAVAILABLE);
    set(State::ReadOnly, STATE_SYSTEM_READONLY);
    set(State::Expanded, STATE_SYSTEM_EXPANDED);
    set(State::Collapsed, STATE_SYSTEM_COLLAPSED);
    set(State::Pressed, STATE_SYSTEM_PRESSED);
    set(State::HasPopup, STATE_SYSTEM_HASPOPUP);
    set(State::DefaultControl, STATE_SYSTEM_DEFAULT);
    set(State::Offscreen, STATE_SYSTEM_OFFSCREEN);
    set(State::Busy, STATE_SYSTEM_BUSY);
    bits
}

/// The `#[implement]`-generated COM object lives in its own module so the
/// module-level allow covers the macro's generated glue, matching the
/// pattern `verbatim-uia`'s `focus` and `events` modules use.
mod handler {
    #![allow(
        clippy::inline_always,
        clippy::ref_as_ptr,
        clippy::used_underscore_binding,
        clippy::too_many_arguments
    )]

    use std::mem::ManuallyDrop;
    use windows::Win32::Foundation::{HWND, S_FALSE};

    use windows::Win32::System::Com::{
        DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, ITypeInfo,
    };
    use windows::Win32::System::Variant::{
        VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_DISPATCH, VT_I4,
    };
    use windows::Win32::UI::Accessibility::{IAccessible, IAccessible_Impl};
    use windows::core::{BSTR, Result as WinResult};
    use windows_core::{Error, implement};

    use super::{role_to_msaa, states_to_msaa};
    use crate::tree::SharedTree;

    /// The accessible object for one node. Every node in the tree is its own
    /// full `IAccessible`/`IDispatch` object — never a numbered "simple
    /// child" of a parent — which keeps navigation, event addressing, and
    /// property queries uniform regardless of tree depth.
    #[implement(IAccessible, Agile = false)]
    pub(crate) struct NodeAccessible {
        pub(crate) tree: SharedTree,
        pub(crate) hwnd: HWND,
        pub(crate) index: usize,
    }

    /// Resolves an MSAA `varChild` argument against `self.index`: either
    /// `CHILDID_SELF` (this node) or the 1-based position of one of this
    /// node's direct children (the "ask the parent about child N" shortcut
    /// MSAA allows without a separate `get_accChild` round trip).
    fn resolve_child(tree: &SharedTree, index: usize, child: &VARIANT) -> Option<usize> {
        // SAFETY: `child` is a caller-supplied VARIANT, as every IAccessible
        // accessor receives; only its type and, if VT_I4, integer field are
        // read.
        let child_id = unsafe {
            if child.Anonymous.Anonymous.vt == VT_I4 {
                child.Anonymous.Anonymous.Anonymous.lVal
            } else {
                0
            }
        };
        if child_id == 0 {
            return Some(index);
        }
        let guard = tree
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let position = usize::try_from(child_id).ok()?.checked_sub(1)?;
        guard.nodes[index].children.get(position).copied()
    }

    fn dispatch_variant(disp: IDispatch) -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_DISPATCH,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 {
                        pdispVal: ManuallyDrop::new(Some(disp)),
                    },
                }),
            },
        }
    }

    fn self_variant() -> VARIANT {
        VARIANT {
            Anonymous: VARIANT_0 {
                Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                    vt: VT_I4,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: VARIANT_0_0_0 { lVal: 0 },
                }),
            },
        }
    }

    impl IAccessible_Impl for NodeAccessible_Impl {
        fn accParent(&self) -> WinResult<IDispatch> {
            let parent = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .nodes[self.index]
                .parent;
            match parent {
                Some(parent_index) => {
                    let accessible: IAccessible = NodeAccessible {
                        tree: self.tree.clone(),
                        hwnd: self.hwnd,
                        index: parent_index,
                    }
                    .into();
                    Ok(accessible.into())
                }
                None => Err(Error::from_hresult(S_FALSE)),
            }
        }

        fn accChildCount(&self) -> WinResult<i32> {
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            i32::try_from(guard.nodes[self.index].children.len())
                .map_err(|_| Error::from_hresult(windows::Win32::Foundation::E_FAIL))
        }

        fn get_accChild(&self, varchild: &VARIANT) -> WinResult<IDispatch> {
            // SAFETY: `varchild` is caller-supplied, as always; only its type
            // and, if VT_I4, integer field are read.
            let child_id = unsafe {
                if varchild.Anonymous.Anonymous.vt == VT_I4 {
                    varchild.Anonymous.Anonymous.Anonymous.lVal
                } else {
                    return Err(Error::from_hresult(
                        windows::Win32::Foundation::E_INVALIDARG,
                    ));
                }
            };
            if child_id == 0 {
                return Err(Error::from_hresult(
                    windows::Win32::Foundation::E_INVALIDARG,
                ));
            }
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(windows::Win32::Foundation::E_INVALIDARG))?;
            let accessible: IAccessible = NodeAccessible {
                tree: self.tree.clone(),
                hwnd: self.hwnd,
                index: target,
            }
            .into();
            Ok(accessible.into())
        }

        fn get_accName(&self, varchild: &VARIANT) -> WinResult<BSTR> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(guard.nodes[target].name.as_deref().unwrap_or("").into())
        }

        fn get_accValue(&self, varchild: &VARIANT) -> WinResult<BSTR> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Ok(guard.nodes[target].value.as_deref().unwrap_or("").into())
        }

        fn get_accDescription(&self, varchild: &VARIANT) -> WinResult<BSTR> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.nodes[target].description.as_deref() {
                Some(description) => Ok(description.into()),
                // S_FALSE is MSAA's "this object has no description",
                // distinct from an empty string.
                None => Err(Error::from_hresult(S_FALSE)),
            }
        }

        fn get_accRole(&self, varchild: &VARIANT) -> WinResult<VARIANT> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let role = role_to_msaa(guard.nodes[target].role);
            Ok(VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_I4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 {
                            lVal: role.cast_signed(),
                        },
                    }),
                },
            })
        }

        fn get_accState(&self, varchild: &VARIANT) -> WinResult<VARIANT> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let states = states_to_msaa(guard.nodes[target].states);
            Ok(VARIANT {
                Anonymous: VARIANT_0 {
                    Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                        vt: VT_I4,
                        wReserved1: 0,
                        wReserved2: 0,
                        wReserved3: 0,
                        Anonymous: VARIANT_0_0_0 {
                            lVal: states.cast_signed(),
                        },
                    }),
                },
            })
        }

        fn get_accHelp(&self, _varchild: &VARIANT) -> WinResult<BSTR> {
            Err(Error::from_hresult(S_FALSE))
        }

        fn get_accHelpTopic(&self, _pszhelpfile: *mut BSTR, _varchild: &VARIANT) -> WinResult<i32> {
            Err(Error::from_hresult(S_FALSE))
        }

        fn get_accKeyboardShortcut(&self, varchild: &VARIANT) -> WinResult<BSTR> {
            let target = resolve_child(&self.tree, self.index, varchild)
                .ok_or_else(|| Error::from_hresult(S_FALSE))?;
            let guard = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match guard.nodes[target].keyboard_shortcut.as_deref() {
                Some(shortcut) => Ok(shortcut.into()),
                // S_FALSE is MSAA's "this object has no shortcut".
                None => Err(Error::from_hresult(S_FALSE)),
            }
        }

        fn accFocus(&self) -> WinResult<VARIANT> {
            let focused = self
                .tree
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .focused;
            // No node focused yet defaults to the root, matching MSAA
            // convention: a container with no focused descendant reports
            // itself.
            let target = focused.unwrap_or(0);
            if target == self.index {
                return Ok(self_variant());
            }
            let accessible: IAccessible = NodeAccessible {
                tree: self.tree.clone(),
                hwnd: self.hwnd,
                index: target,
            }
            .into();
            Ok(dispatch_variant(accessible.into()))
        }

        fn accSelection(&self) -> WinResult<VARIANT> {
            Err(Error::from_hresult(S_FALSE))
        }

        fn get_accDefaultAction(&self, _varchild: &VARIANT) -> WinResult<BSTR> {
            Err(Error::from_hresult(S_FALSE))
        }

        fn accSelect(&self, _flagsselect: i32, _varchild: &VARIANT) -> WinResult<()> {
            Ok(())
        }

        fn accLocation(
            &self,
            pxleft: *mut i32,
            pytop: *mut i32,
            pcxwidth: *mut i32,
            pcyheight: *mut i32,
            _varchild: &VARIANT,
        ) -> WinResult<()> {
            // SAFETY: the four pointers are caller-owned out-parameters, as
            // every `accLocation` caller supplies; mockapp never lays out
            // real control geometry, so they are always zeroed.
            unsafe {
                *pxleft = 0;
                *pytop = 0;
                *pcxwidth = 0;
                *pcyheight = 0;
            }
            Ok(())
        }

        fn accNavigate(&self, navdir: i32, varstart: &VARIANT) -> WinResult<VARIANT> {
            use windows::Win32::UI::Accessibility::{
                NAVDIR_FIRSTCHILD, NAVDIR_LASTCHILD, NAVDIR_NEXT, NAVDIR_PREVIOUS,
            };
            let Some(start) = resolve_child(&self.tree, self.index, varstart) else {
                return Err(Error::from_hresult(S_FALSE));
            };
            let target = {
                let guard = self
                    .tree
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let navdir = navdir.cast_unsigned();
                if navdir == NAVDIR_FIRSTCHILD {
                    guard.nodes[start].children.first().copied()
                } else if navdir == NAVDIR_LASTCHILD {
                    guard.nodes[start].children.last().copied()
                } else if navdir == NAVDIR_NEXT {
                    guard.sibling(start, 1)
                } else if navdir == NAVDIR_PREVIOUS {
                    guard.sibling(start, -1)
                } else {
                    None
                }
            };
            match target {
                Some(index) => {
                    let accessible: IAccessible = NodeAccessible {
                        tree: self.tree.clone(),
                        hwnd: self.hwnd,
                        index,
                    }
                    .into();
                    Ok(dispatch_variant(accessible.into()))
                }
                None => Err(Error::from_hresult(S_FALSE)),
            }
        }

        fn accHitTest(&self, _xleft: i32, _ytop: i32) -> WinResult<VARIANT> {
            Err(Error::from_hresult(S_FALSE))
        }

        fn accDoDefaultAction(&self, _varchild: &VARIANT) -> WinResult<()> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }

        fn put_accName(&self, _varchild: &VARIANT, _szname: &BSTR) -> WinResult<()> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }

        fn put_accValue(&self, _varchild: &VARIANT, _szvalue: &BSTR) -> WinResult<()> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }
    }

    impl windows::Win32::System::Com::IDispatch_Impl for NodeAccessible_Impl {
        fn GetTypeInfoCount(&self) -> WinResult<u32> {
            Ok(0)
        }

        fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> WinResult<ITypeInfo> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }

        fn GetIDsOfNames(
            &self,
            _riid: *const windows_core::GUID,
            _rgsznames: *const windows_core::PCWSTR,
            _cnames: u32,
            _lcid: u32,
            _rgdispid: *mut i32,
        ) -> WinResult<()> {
            Err(Error::from_hresult(windows::Win32::Foundation::E_NOTIMPL))
        }

        fn Invoke(
            &self,
            _dispidmember: i32,
            _riid: *const windows_core::GUID,
            _lcid: u32,
            _wflags: DISPATCH_FLAGS,
            _pdispparams: *const DISPPARAMS,
            _pvarresult: *mut VARIANT,
            _pexcepinfo: *mut EXCEPINFO,
            _puargerr: *mut u32,
        ) -> WinResult<()> {
            Err(Error::from_hresult(
                windows::Win32::Foundation::DISP_E_MEMBERNOTFOUND,
            ))
        }
    }
}
