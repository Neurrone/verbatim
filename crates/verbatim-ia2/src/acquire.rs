//! Query-pool-side MSAA acquisition.
//!
//! Everything here makes blocking cross-process COM calls and must run only on
//! a deadline-guarded query-pool thread, never on the event thread. Starting
//! from a `WinEvent` address, or from the focused window, it acquires an
//! `IAccessible`, reads name/role/value/state, and maps to a [`NodeSnapshot`].
//!
//! IA2 seam: in M3 the richer `IAccessible2` interfaces (text, hypertext,
//! relations) are acquired here by calling `IServiceProvider::QueryService` on
//! the `IAccessible` obtained below. That is deliberately left as a module
//! boundary for now — see [`acquire_ia2`].

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromEvent, AccessibleObjectFromWindow, IAccessible, WindowFromAccessibleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId, OBJID_CLIENT,
};
use windows::core::Interface;

use verbatim_model::{Backend, NodeSnapshot, Role};

use crate::com::{CHILDID_SELF, bstr_to_option, child_variant, variant_i32};
use crate::map::{role_from_msaa, states_from_msaa};
use crate::registry::{MsaaKey, NodeIdRegistry};

/// Acquires the object named by a `WinEvent` and maps it to a [`NodeSnapshot`].
/// Returns `None` if the object cannot be acquired. Blocking; query pool only.
#[must_use]
pub fn snapshot_from_event(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    // SAFETY: AccessibleObjectFromEvent tolerates a stale address by failing;
    // `acc` and `child` are initialized by the call before use.
    unsafe {
        let mut acc: Option<IAccessible> = None;
        let mut child = VARIANT::default();
        AccessibleObjectFromEvent(
            HWND(hwnd as *mut c_void),
            id_object.cast_unsigned(),
            id_child.cast_unsigned(),
            &raw mut acc,
            &raw mut child,
        )
        .ok()?;
        let acc = acc?;
        Some(read_snapshot(
            &acc,
            &child,
            (hwnd, id_object, id_child),
            registry,
        ))
    }
}

/// Re-reads a node previously seen at `key`. Returns `None` if it can no longer
/// be acquired. Blocking; query pool only.
#[must_use]
pub fn resnapshot(key: MsaaKey, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let (hwnd, id_object, id_child) = key;
    snapshot_from_event(hwnd, id_object, id_child, registry)
}

/// Reads the currently focused object of `target_pid` for the synthetic focus
/// event a `Configure` triggers. Uses `GetGUIThreadInfo` then `accFocus`, with
/// a fallback to the focused window itself. Blocking; query pool only.
///
/// M1 keys the focused node by its window and child id; a focused child exposed
/// only as a distinct `IDispatch` is keyed to its own window, which is adequate
/// for the M1 targets and refined when IA2 lands.
#[must_use]
pub fn focused_snapshot(target_pid: u32, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    // SAFETY: each call below fails safely on a bad handle; `info` is fully
    // initialized (cbSize set) before GetGUIThreadInfo.
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        GetGUIThreadInfo(0, &raw mut info).ok()?;
        let hwnd = if info.hwndFocus.0.is_null() {
            info.hwndActive
        } else {
            info.hwndFocus
        };
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        if pid != target_pid {
            return None;
        }
        let client = accessible_from_window(hwnd)?;
        let (acc, child) = resolve_focus(&client);
        let node_hwnd = window_of(&acc).unwrap_or(hwnd.0 as isize);
        let key = (node_hwnd, OBJID_CLIENT.0, child_id_of(&child));
        Some(read_snapshot(&acc, &child, key, registry))
    }
}

/// The IA2 acquisition seam (architecture section 4, roadmap M3). From the
/// `IAccessible` acquired above, `IServiceProvider::QueryService` yields
/// `IAccessible2` and the text, hypertext, and relation interfaces. Not
/// implemented in M1; present so the boundary is explicit.
#[allow(dead_code)]
fn acquire_ia2() {
    // Intentionally empty: the IA2 QueryService path lands in M3.
}

/// Reads name, role, value, and state from an accessible and its child id.
///
/// # Safety
///
/// `acc` must be a live `IAccessible` and `child` a valid child-id `VARIANT`
/// for it.
unsafe fn read_snapshot(
    acc: &IAccessible,
    child: &VARIANT,
    key: MsaaKey,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    // SAFETY: forwarded to the caller's contract; each accessor tolerates an
    // unsupported property by returning an error, mapped to a neutral default.
    unsafe {
        let name = acc.get_accName(child).ok().and_then(|b| bstr_to_option(&b));
        let value = acc
            .get_accValue(child)
            .ok()
            .and_then(|b| bstr_to_option(&b));
        let role = acc
            .get_accRole(child)
            .ok()
            .and_then(|v| variant_i32(&v))
            .map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()));
        let states = acc
            .get_accState(child)
            .ok()
            .and_then(|v| variant_i32(&v))
            .map(|s| states_from_msaa(s.cast_unsigned()))
            .unwrap_or_default();
        NodeSnapshot {
            id: registry.id_for(key),
            backend: Backend::Msaa,
            role,
            name,
            value,
            states,
        }
    }
}

/// Obtains the client `IAccessible` for a window.
///
/// # Safety
///
/// `hwnd` must be a window handle (an invalid one fails safely).
unsafe fn accessible_from_window(hwnd: HWND) -> Option<IAccessible> {
    // SAFETY: the out pointer receives an IAccessible or leaves `acc` null on
    // failure; Option<IAccessible> is null-pointer-optimized.
    unsafe {
        let mut acc: Option<IAccessible> = None;
        AccessibleObjectFromWindow(
            hwnd,
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            (&raw mut acc).cast::<*mut c_void>(),
        )
        .ok()?;
        acc
    }
}

/// Resolves `accFocus` on a client accessible into the focused accessible and
/// its child id, handling both the child-id (`VT_I4`) and child-object
/// (`VT_DISPATCH`) forms, falling back to the client itself.
///
/// # Safety
///
/// `client` must be a live `IAccessible`.
unsafe fn resolve_focus(client: &IAccessible) -> (IAccessible, VARIANT) {
    // SAFETY: accFocus returns an owned VARIANT; its variant type is inspected
    // before any union field is read.
    unsafe {
        let Ok(focus) = client.accFocus() else {
            return (client.clone(), child_variant(CHILDID_SELF));
        };
        let vt = focus.Anonymous.Anonymous.vt;
        if vt == VT_DISPATCH {
            if let Some(dispatch) = focus.Anonymous.Anonymous.Anonymous.pdispVal.as_ref()
                && let Ok(child_acc) = dispatch.cast::<IAccessible>()
            {
                return (child_acc, child_variant(CHILDID_SELF));
            }
            (client.clone(), child_variant(CHILDID_SELF))
        } else if vt == VT_I4 {
            let child_id = focus.Anonymous.Anonymous.Anonymous.lVal;
            (client.clone(), child_variant(child_id))
        } else {
            (client.clone(), child_variant(CHILDID_SELF))
        }
    }
}

/// Returns the child id encoded in a `VARIANT` (`VT_I4`), else `CHILDID_SELF`.
///
/// # Safety
///
/// `child` must be a valid `VARIANT`.
unsafe fn child_id_of(child: &VARIANT) -> i32 {
    // SAFETY: the variant type is checked before reading the integer field.
    unsafe {
        if child.Anonymous.Anonymous.vt == VT_I4 {
            child.Anonymous.Anonymous.Anonymous.lVal
        } else {
            CHILDID_SELF
        }
    }
}

/// Returns the window handle owning an accessible, if any.
///
/// # Safety
///
/// `acc` must be a live `IAccessible`.
unsafe fn window_of(acc: &IAccessible) -> Option<isize> {
    // SAFETY: WindowFromAccessibleObject writes `hwnd` or fails; the handle is
    // read only on success.
    unsafe {
        let mut hwnd = HWND::default();
        WindowFromAccessibleObject(acc, Some(&raw mut hwnd)).ok()?;
        Some(hwnd.0 as isize)
    }
}
