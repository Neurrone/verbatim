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
    AccessibleChildren, AccessibleObjectFromEvent, AccessibleObjectFromWindow, IAccessible,
    WindowFromAccessibleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId, OBJID_CLIENT,
};
use windows::core::Interface;

use verbatim_model::{Backend, NodeSnapshot, Role, TreeNode};

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

/// Walks the MSAA tree from `hwnd`'s client accessible object, bounded by
/// `max_depth` (the root is depth 0) and `max_nodes` (the total node count
/// across the whole walk, including the root). Unlike UIA there are no
/// cache requests on this backend, so every step is its own cross-process
/// COM round trip (architecture section 4's IA2 cost model); blocking,
/// query pool only, guarded by the caller's deadline. Returns `None` if the
/// window has no accessible client object.
#[must_use]
pub fn walk_tree(
    hwnd: isize,
    registry: &NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
) -> Option<(TreeNode, bool)> {
    // SAFETY: `hwnd` is a caller-supplied handle; `accessible_from_window`
    // fails safely on an invalid one.
    let acc = unsafe { accessible_from_window(HWND(hwnd as *mut c_void)) }?;
    let limits = WalkLimits {
        registry,
        max_depth,
        max_nodes,
    };
    let mut state = WalkState {
        visited: 1, // the root counts as one node.
        truncated: false,
    };
    let key = (hwnd, OBJID_CLIENT.0, CHILDID_SELF);
    // SAFETY: `acc` was just acquired and is live; `CHILDID_SELF` addresses
    // the object itself, a valid child variant for it.
    let root = unsafe {
        walk_recursive(
            &acc,
            &child_variant(CHILDID_SELF),
            hwnd,
            key,
            &limits,
            0,
            &mut state,
        )
    };
    Some((root, state.truncated))
}

/// The per-walk parameters threaded through every level of
/// [`walk_recursive`]: node-identity plumbing plus the walk's caps.
struct WalkLimits<'a> {
    registry: &'a NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
}

/// Mutable state accumulated across the whole walk, shared by every level
/// of recursion.
struct WalkState {
    visited: usize,
    truncated: bool,
}

/// Recursive worker for [`walk_tree`]. `state.visited` already counts the
/// node named by `acc`/`child`. A simple element (addressed purely by a
/// child id, not its own `IAccessible`) is always a leaf per the MSAA
/// model, so only `CHILDID_SELF` nodes are ever expanded.
///
/// # Safety
///
/// `acc` must be a live `IAccessible` and `child` a valid child-id `VARIANT`
/// for it.
unsafe fn walk_recursive(
    acc: &IAccessible,
    child: &VARIANT,
    hwnd: isize,
    key: MsaaKey,
    limits: &WalkLimits<'_>,
    depth: u32,
    state: &mut WalkState,
) -> TreeNode {
    // SAFETY: forwarded to this function's contract.
    let snapshot = unsafe { read_snapshot(acc, child, key, limits.registry) };

    // SAFETY: `child` is valid for `acc` per the caller's contract.
    if unsafe { child_id_of(child) } != CHILDID_SELF {
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }

    if depth >= limits.max_depth {
        // SAFETY: `acc` is live per the caller's contract; only peeks
        // whether there is a child we are declining to descend into.
        let has_children = unsafe { acc.accChildCount() }.is_ok_and(|count| count > 0);
        if has_children {
            state.truncated = true;
        }
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }

    // SAFETY: `acc` is live per the caller's contract.
    let child_count = match unsafe { acc.accChildCount() } {
        Ok(count) if count > 0 => count,
        _ => {
            return TreeNode {
                snapshot,
                children: Vec::new(),
            };
        }
    };
    let Ok(child_count) = usize::try_from(child_count) else {
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    };

    let mut buffer: Vec<VARIANT> = (0..child_count).map(|_| VARIANT::default()).collect();
    let mut obtained = 0i32;
    // SAFETY: `acc` is live; `buffer` holds exactly `child_count` freshly
    // zeroed VARIANTs, which is what AccessibleChildren fills (writing at
    // most that many entries and reporting the actual count in `obtained`).
    if unsafe { AccessibleChildren(acc, 0, &mut buffer, &raw mut obtained) }.is_err() {
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }
    let obtained = usize::try_from(obtained).unwrap_or(0).min(buffer.len());

    let mut children = Vec::new();
    for entry in &buffer[..obtained] {
        if state.visited >= limits.max_nodes {
            state.truncated = true;
            break;
        }
        // SAFETY: `entry` is one VARIANT written by AccessibleChildren above.
        let (child_acc, child_child, child_hwnd) = unsafe { resolve_child(acc, entry, hwnd) };
        state.visited += 1;
        // SAFETY: `child_child` is valid for `child_acc` per
        // `resolve_child`'s construction.
        let child_key = (child_hwnd, OBJID_CLIENT.0, unsafe {
            child_id_of(&child_child)
        });
        // SAFETY: `child_acc` is a live IAccessible and `child_child` a
        // valid child variant for it, per `resolve_child`.
        let child_node = unsafe {
            walk_recursive(
                &child_acc,
                &child_child,
                child_hwnd,
                child_key,
                limits,
                depth + 1,
                state,
            )
        };
        children.push(child_node);
    }

    TreeNode { snapshot, children }
}

/// Resolves one entry from `AccessibleChildren` into `(accessible, child
/// variant, hwnd)` to recurse into: a `VT_I4` entry is a simple element
/// addressed by child id on `parent`; a `VT_DISPATCH` entry carries its own
/// `IAccessible`, addressed by `CHILDID_SELF`, and may belong to a distinct
/// window (a nested control), resolved the same way `resolve_focus` does.
///
/// # Safety
///
/// `parent` must be a live `IAccessible`; `entry` must be one `VARIANT`
/// produced by `AccessibleChildren` on it.
unsafe fn resolve_child(
    parent: &IAccessible,
    entry: &VARIANT,
    parent_hwnd: isize,
) -> (IAccessible, VARIANT, isize) {
    // SAFETY: the variant type is checked before any union field is read.
    unsafe {
        let vt = entry.Anonymous.Anonymous.vt;
        if vt == VT_DISPATCH {
            if let Some(dispatch) = entry.Anonymous.Anonymous.Anonymous.pdispVal.as_ref()
                && let Ok(child_acc) = dispatch.cast::<IAccessible>()
            {
                let hwnd = window_of(&child_acc).unwrap_or(parent_hwnd);
                return (child_acc, child_variant(CHILDID_SELF), hwnd);
            }
            (parent.clone(), child_variant(CHILDID_SELF), parent_hwnd)
        } else if vt == VT_I4 {
            let child_id = entry.Anonymous.Anonymous.Anonymous.lVal;
            (parent.clone(), child_variant(child_id), parent_hwnd)
        } else {
            (parent.clone(), child_variant(CHILDID_SELF), parent_hwnd)
        }
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
