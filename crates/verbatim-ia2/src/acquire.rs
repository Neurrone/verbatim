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
//!
//! `SysTreeView32` seam: a Win32 common-control tree view (comctl32) exposes
//! every visible item to MSAA as a flat sibling list directly under the tree
//! control, not nested under its logical parent — confirmed live against
//! msinfo32. [`navigate`] and [`ancestor_chain`] detect this window class and
//! route a tree item's navigation through the control's own `TVM_*` window
//! messages instead of `accNavigate`/`accParent`, mirroring NVDA's
//! `sysTreeView32.py`. Those messages are sent with plain `SendMessageW`,
//! which can block if the owning application is wedged — acceptable only
//! because every caller of this module already runs on a deadline-guarded
//! query-pool thread (never the event thread), the same bound every other
//! blocking call in this module already relies on.

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleChildren, AccessibleObjectFromEvent, AccessibleObjectFromWindow, IAccessible,
    NAVDIR_FIRSTCHILD, NAVDIR_NEXT, NAVDIR_PREVIOUS, WindowFromAccessibleObject,
};
use windows::Win32::UI::Controls::{
    TVGN_CHILD, TVGN_NEXT, TVGN_PARENT, TVGN_PREVIOUS, TVM_GETNEXTITEM, TVM_MAPACCIDTOHTREEITEM,
    TVM_MAPHTREEITEMTOACCID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_PARENT, GUITHREADINFO, GW_HWNDNEXT, GW_HWNDPREV, GetAncestor, GetClassNameW,
    GetDesktopWindow, GetGUIThreadInfo, GetTopWindow, GetWindow, GetWindowThreadProcessId,
    IsWindowVisible, OBJID_CLIENT, OBJID_WINDOW, SendMessageW,
};
use windows::core::Interface;

use verbatim_model::{Backend, NodeDetails, NodeSnapshot, Rect, Role, TreeNode};

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
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let (acc, child) = unsafe { accessible_and_child(hwnd, id_object, id_child) }?;
    // SAFETY: `acc` and `child` were just acquired together and are valid
    // for each other.
    Some(unsafe { read_snapshot(&acc, &child, (hwnd, id_object, id_child), registry) })
}

/// Acquires the `IAccessible` and child variant named by a `WinEvent`
/// address, the shared first step behind [`snapshot_from_event`] and
/// [`ancestor_chain`]. Returns `None` if the object cannot be acquired.
///
/// # Safety
///
/// `hwnd` may be any handle value (an invalid one fails safely, per
/// `AccessibleObjectFromEvent`'s own contract).
unsafe fn accessible_and_child(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
) -> Option<(IAccessible, VARIANT)> {
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
        Some((acc?, child))
    }
}

/// Walks the chain of ancestors of the node named by `key`, nearest first,
/// as [`NodeSnapshot`]s. Every hop is its own cross-process round trip
/// (architecture section 4's IA2 cost model: no cache requests on this
/// backend), via `IAccessible::accParent` — except the first hop for a node
/// addressed as a "simple child" (a bare child id, not its own `IDispatch`),
/// whose immediate parent is the object it is a child of, since plain MSAA
/// has no `accParent` for a child id, only for a full object. Capped at
/// `max_hops` ancestors; stops early, without error, once a hop finds no
/// further parent or fails. Blocking; query pool only.
///
/// For a `SysTreeView32` item addressed as a simple child (see this module's
/// top doc comment), the walk first follows the item's own logical
/// ancestors through `TVGN_PARENT` (the same relation [`navigate`] uses for
/// `Parent`), then continues with the tree control itself and its own
/// window ancestry through the normal `accParent` walk below — so a
/// focus-ancestry announcement for a deeply nested tree item reads its full
/// logical path, not the flat MSAA sibling list the control otherwise
/// exposes. Both stages share the one `max_hops` budget.
///
/// This is deliberately the simplest correct implementation, behind this
/// function as a seam: milestone M4's remote-operations work replaces
/// UIA's equivalent per-hop walk with a single batched round trip, and MSAA
/// has no remote-operations analog to migrate to, so this stays the
/// permanent MSAA implementation — but callers should still depend only on
/// the result, never on how many round trips producing it took.
#[must_use]
pub fn ancestor_chain(key: MsaaKey, registry: &NodeIdRegistry, max_hops: u32) -> Vec<NodeSnapshot> {
    let (hwnd, id_object, id_child) = key;
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let Some((acc, child)) = (unsafe { accessible_and_child(hwnd, id_object, id_child) }) else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    // SAFETY: `child` is valid for `acc`, just acquired together above.
    let is_simple_child = unsafe { child_id_of(&child) } != CHILDID_SELF;
    let mut hops_used = 0u32;

    if is_simple_child && is_systreeview32(hwnd) {
        // SAFETY: `child` is valid for `acc`.
        let mut current_acc_id = unsafe { child_id_of(&child) };
        while hops_used < max_hops {
            let Some(parent_acc_id) = tree_view_relation_acc_id(hwnd, current_acc_id, TVGN_PARENT)
            else {
                // The item is a root: fall through to the tree control
                // itself, via the ordinary simple-child handling below.
                break;
            };
            let parent_key = (hwnd, id_object, parent_acc_id);
            // SAFETY: `acc` is the tree control's own live IAccessible;
            // `parent_acc_id` addresses one of its simple children.
            let snapshot =
                unsafe { read_snapshot(&acc, &child_variant(parent_acc_id), parent_key, registry) };
            chain.push(snapshot);
            hops_used += 1;
            current_acc_id = parent_acc_id;
        }
    }

    let mut current = acc;
    let mut at_self = !is_simple_child;
    while hops_used < max_hops {
        if !at_self {
            // SAFETY: `current` is a live IAccessible from a prior successful
            // acquisition.
            let self_hwnd = unsafe { window_of(&current) }.unwrap_or(hwnd);
            let self_key = (self_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            // SAFETY: `current` is live; CHILDID_SELF addresses it directly.
            let snapshot = unsafe {
                read_snapshot(&current, &child_variant(CHILDID_SELF), self_key, registry)
            };
            chain.push(snapshot);
            at_self = true;
            hops_used += 1;
            continue;
        }
        // SAFETY: `current` is a live IAccessible.
        let Ok(parent_dispatch) = (unsafe { current.accParent() }) else {
            break;
        };
        let Ok(parent_acc) = parent_dispatch.cast::<IAccessible>() else {
            break;
        };
        // SAFETY: `parent_acc` was just acquired above.
        let parent_hwnd = unsafe { window_of(&parent_acc) }.unwrap_or(hwnd);
        let parent_key = (parent_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        // SAFETY: `parent_acc` is live; CHILDID_SELF addresses it directly.
        let snapshot = unsafe {
            read_snapshot(
                &parent_acc,
                &child_variant(CHILDID_SELF),
                parent_key,
                registry,
            )
        };
        chain.push(snapshot);
        hops_used += 1;
        current = parent_acc;
    }
    chain.reverse();
    chain
}

/// Reads a window's class name via `GetClassNameW` — a local, non-blocking
/// call safe on any thread even against a hung window, unlike the
/// `SendMessageW` calls the `SysTreeView32` helpers below make.
fn window_class_name(hwnd: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: GetClassNameW writes at most buffer.len()-1 code units plus a
    // NUL and returns the count written; an invalid handle yields 0, and
    // `buffer` is a local, fully owned array.
    let len = unsafe { GetClassNameW(HWND(hwnd as *mut c_void), &mut buffer) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    String::from_utf16_lossy(&buffer[..len.min(buffer.len())])
}

/// Returns whether `hwnd` is a `SysTreeView32` common control (comctl32's
/// tree view) — see this module's top doc comment for why its items need
/// the `TVM_*`-based navigation below instead of `accNavigate`/`accParent`.
fn is_systreeview32(hwnd: isize) -> bool {
    window_class_name(hwnd) == "SysTreeView32"
}

/// Sends a `TVM_*` message to a tree-view control and returns the result as
/// an `isize`, the common shape every `HTREEITEM`/relation-code message
/// below uses. A thin wrapper so every call site reads the same way.
///
/// # Safety
///
/// `hwnd` may be any handle value (an invalid one fails safely, returning
/// 0, since `SendMessageW` itself tolerates a bad handle). See this
/// module's top doc comment for why a plain, non-timeout `SendMessageW` is
/// acceptable here.
unsafe fn send_tvm(hwnd: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: forwarded to the caller's contract; the TVM_* messages used by
    // this module's callers take plain integers (an acc id, an `HTREEITEM`
    // value, or a `TVGN_*` relation code) in `wparam`/`lparam`, never a
    // pointer that would need to stay valid beyond the call.
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0 }
}

/// Maps an MSAA child id to its `HTREEITEM`, via `TVM_MAPACCIDTOHTREEITEM`.
/// Falls back to using the child id as the `HTREEITEM` value directly when
/// the message returns 0: comctl32 versions before v6 have no accid/htreeitem
/// mapping and use the hItem as the child id outright, exactly as NVDA's
/// `sysTreeView32.py` (`treeview_hItem`) does.
fn htreeitem_for_acc_id(hwnd: HWND, acc_id: i32) -> isize {
    let wparam = usize::try_from(acc_id.cast_unsigned()).unwrap_or(0);
    // SAFETY: `hwnd` may be any handle; `send_tvm` fails safely per its own
    // contract.
    let mapped = unsafe { send_tvm(hwnd, TVM_MAPACCIDTOHTREEITEM, wparam, 0) };
    if mapped == 0 {
        isize::try_from(acc_id).unwrap_or(0)
    } else {
        mapped
    }
}

/// Maps an `HTREEITEM` back to its MSAA child id, via
/// `TVM_MAPHTREEITEMTOACCID`. Falls back to using the `HTREEITEM` value
/// itself as the child id when the message returns 0, the same comctl32 <
/// v6 fallback [`htreeitem_for_acc_id`] documents.
fn acc_id_for_htreeitem(hwnd: HWND, hitem: isize) -> i32 {
    let wparam = usize::try_from(hitem).unwrap_or(0);
    // SAFETY: `hwnd` may be any handle; `send_tvm` fails safely per its own
    // contract.
    let mapped = unsafe { send_tvm(hwnd, TVM_MAPHTREEITEMTOACCID, wparam, 0) };
    if mapped == 0 {
        i32::try_from(hitem).unwrap_or(0)
    } else {
        i32::try_from(mapped).unwrap_or(0)
    }
}

/// The shared `TVM_*` plumbing behind a `SysTreeView32` item's logical
/// parent, next sibling, previous sibling, and first child: maps `acc_id`
/// to its `HTREEITEM` ([`htreeitem_for_acc_id`]), walks `relation`
/// (`TVGN_PARENT`, `TVGN_NEXT`, `TVGN_PREVIOUS`, or `TVGN_CHILD`) via
/// `TVM_GETNEXTITEM`, and maps the result back to an acc id
/// ([`acc_id_for_htreeitem`]) — mirroring NVDA's `sysTreeView32.py`
/// relation properties. `None` for a 0 result at either step: a 0
/// `HTREEITEM` lookup means `acc_id` no longer resolves; a 0 relation
/// result means a genuine "no such neighbor" (or, for `TVGN_PARENT`, that
/// the item is a root — see [`navigate`] and [`ancestor_chain`] for how
/// each caller handles that case).
fn tree_view_relation_acc_id(hwnd: isize, acc_id: i32, relation: u32) -> Option<i32> {
    let hwnd = HWND(hwnd as *mut c_void);
    let hitem = htreeitem_for_acc_id(hwnd, acc_id);
    if hitem == 0 {
        return None;
    }
    // SAFETY: `hwnd` may be any handle; `send_tvm` fails safely per its own
    // contract.
    let neighbor_hitem = unsafe { send_tvm(hwnd, TVM_GETNEXTITEM, relation as usize, hitem) };
    if neighbor_hitem == 0 {
        return None;
    }
    Some(acc_id_for_htreeitem(hwnd, neighbor_hitem))
}

/// A direction to navigate from a node with [`navigate`], mirroring
/// [`verbatim_uia`]'s equivalent (the crates do not depend on each other, so
/// each carries its own copy) and the object-navigation commands milestone
/// M3 adds (roadmap: parent, next and previous sibling, first child).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavigateDirection {
    /// The node's parent.
    Parent,
    /// The next sibling in tree order.
    NextSibling,
    /// The previous sibling in tree order.
    PreviousSibling,
    /// The first child.
    FirstChild,
}

/// Navigates one step from the node named by `key` in `direction`.
///
/// For a `SysTreeView32` item addressed as a simple child (see this
/// module's top doc comment), every direction routes through the tree
/// control's own `TVGN_*` relations ([`tree_view_relation_acc_id`]) instead
/// of `accNavigate`/`accParent`, mirroring NVDA's `sysTreeView32.py`: MSAA
/// over this control exposes every visible item as a flat sibling list
/// directly under the tree, not nested under its logical parent, so
/// `accNavigate`'s next/previous/first-child answers and `accParent`'s
/// answer are all wrong for it (confirmed live against msinfo32). A
/// `Parent` query whose item is a root (`TVGN_PARENT` returns nothing)
/// falls through to the ordinary simple-child handling below, which
/// correctly lands on the tree control itself.
///
/// Every other node goes through `IAccessible::accParent` for `Parent`
/// (with the same "simple child's immediate parent is the object it is a
/// child of" handling [`ancestor_chain`] uses, since plain MSAA has no
/// `accParent` for a child id) and `IAccessible::accNavigate` for the other
/// three directions.
///
/// Blocking; query pool only.
///
/// # Errors
///
/// Returns `Err` only when the source node itself — the one named by `key`
/// — can no longer be re-acquired; this is the "the node is gone" case.
/// `Ok(None)` is a genuine edge: the source node is fine, but there is no
/// neighbor in that direction (a root's parent, a last child's next
/// sibling, a leaf's first child). The two are never conflated, so callers
/// can distinguish "this node vanished" from "this is the end of the tree".
pub fn navigate(
    key: MsaaKey,
    registry: &NodeIdRegistry,
    direction: NavigateDirection,
) -> Result<Option<NodeSnapshot>, String> {
    let (hwnd, id_object, id_child) = key;
    // A window-root object (a windowed control's window face, keyed under
    // OBJID_WINDOW — see `read_snapshot`) navigates the Win32 window
    // hierarchy, not `accNavigate`/`accParent`, mirroring NVDA's Window and
    // WindowRoot classes. MSAA over a plain windowed control answers sibling
    // and child navigation with the control's own scroll-bar and client
    // pieces (confirmed live: "next" from a settings slider landed on its
    // "Page left" scroll button), while the sibling *controls* a user
    // navigates between are sibling windows the window hierarchy walks
    // directly.
    if id_object == OBJID_WINDOW.0 && id_child == CHILDID_SELF {
        return Ok(window_navigate(hwnd, direction, registry));
    }
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let (acc, child) = unsafe { accessible_and_child(hwnd, id_object, id_child) }
        .ok_or_else(|| "could not acquire the node".to_owned())?;
    // SAFETY: `child` is valid for `acc`, just acquired together above.
    let is_simple_child = unsafe { child_id_of(&child) } != CHILDID_SELF;
    let tree_view = is_simple_child && is_systreeview32(hwnd);

    if direction == NavigateDirection::Parent {
        if tree_view {
            // SAFETY: `child` is valid for `acc`.
            let acc_id = unsafe { child_id_of(&child) };
            if let Some(parent_acc_id) = tree_view_relation_acc_id(hwnd, acc_id, TVGN_PARENT) {
                let parent_key = (hwnd, id_object, parent_acc_id);
                // SAFETY: `acc` is the tree control's own live IAccessible;
                // `parent_acc_id` addresses one of its simple children.
                return Ok(Some(unsafe {
                    read_snapshot(&acc, &child_variant(parent_acc_id), parent_key, registry)
                }));
            }
            // The item is a root: fall through to the ordinary simple-child
            // handling below, which lands on the tree control itself.
        }
        if is_simple_child {
            // The immediate parent of a simple child (addressed only by a
            // child id, not its own IDispatch) is the object it is a child
            // of; see this module's `ancestor_chain` doc for the same case.
            // SAFETY: `acc` is live.
            let self_hwnd = unsafe { window_of(&acc) }.unwrap_or(hwnd);
            let self_key = (self_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            // SAFETY: `acc` is live; CHILDID_SELF addresses it directly.
            return Ok(Some(unsafe {
                read_snapshot(&acc, &child_variant(CHILDID_SELF), self_key, registry)
            }));
        }
        // SAFETY: `acc` is live.
        let Ok(parent_dispatch) = (unsafe { acc.accParent() }) else {
            return Ok(None);
        };
        let Ok(parent_acc) = parent_dispatch.cast::<IAccessible>() else {
            return Ok(None);
        };
        // SAFETY: `parent_acc` was just acquired above.
        let parent_hwnd = unsafe { window_of(&parent_acc) }.unwrap_or(hwnd);
        let parent_key = (parent_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        // SAFETY: `parent_acc` is live; CHILDID_SELF addresses it directly.
        return Ok(Some(unsafe {
            read_snapshot(
                &parent_acc,
                &child_variant(CHILDID_SELF),
                parent_key,
                registry,
            )
        }));
    }

    if tree_view {
        // SAFETY: `child` is valid for `acc`.
        let acc_id = unsafe { child_id_of(&child) };
        let relation = match direction {
            NavigateDirection::NextSibling => TVGN_NEXT,
            NavigateDirection::PreviousSibling => TVGN_PREVIOUS,
            NavigateDirection::FirstChild => TVGN_CHILD,
            NavigateDirection::Parent => unreachable!("handled above"),
        };
        return Ok(
            tree_view_relation_acc_id(hwnd, acc_id, relation).map(|neighbor_acc_id| {
                let neighbor_key = (hwnd, id_object, neighbor_acc_id);
                // SAFETY: `acc` is the tree control's own live IAccessible;
                // `neighbor_acc_id` addresses one of its simple children.
                unsafe {
                    read_snapshot(
                        &acc,
                        &child_variant(neighbor_acc_id),
                        neighbor_key,
                        registry,
                    )
                }
            }),
        );
    }

    let navdir = match direction {
        NavigateDirection::NextSibling => NAVDIR_NEXT,
        NavigateDirection::PreviousSibling => NAVDIR_PREVIOUS,
        NavigateDirection::FirstChild => NAVDIR_FIRSTCHILD,
        NavigateDirection::Parent => unreachable!("handled above"),
    };
    // SAFETY: `acc` is live; `child` is valid for it.
    let Ok(result) = (unsafe { acc.accNavigate(navdir.cast_signed(), &child) }) else {
        return Ok(None);
    };
    // SAFETY: `result` is the VARIANT `accNavigate` just returned; `acc` is
    // live, matching `resolve_child`'s contract even though this result did
    // not come from `AccessibleChildren` — both follow the same MSAA
    // VT_DISPATCH/VT_I4 convention for naming a related object.
    let (target_acc, target_child, target_hwnd) = unsafe { resolve_child(&acc, &result, hwnd) };
    // SAFETY: `target_child` is valid for `target_acc`, per `resolve_child`.
    let target_key = (target_hwnd, OBJID_CLIENT.0, unsafe {
        child_id_of(&target_child)
    });
    // SAFETY: `target_acc` is live and `target_child` valid for it, per
    // `resolve_child`.
    Ok(Some(unsafe {
        read_snapshot(&target_acc, &target_child, target_key, registry)
    }))
}

/// Activates the node named by `key`: `IAccessible::accDoDefaultAction`, the
/// only activation MSAA offers (UIA's richer `Invoke`/`Toggle` ladder has no
/// MSAA equivalent). Blocking; query pool only.
///
/// # Errors
///
/// Returns a human-readable reason if the node cannot be acquired or the
/// call fails (including "not implemented", MSAA's answer for a node with no
/// default action).
pub fn activate(key: MsaaKey) -> Result<(), String> {
    let (hwnd, id_object, id_child) = key;
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let (acc, child) = unsafe { accessible_and_child(hwnd, id_object, id_child) }
        .ok_or_else(|| "could not acquire the node".to_owned())?;
    // SAFETY: `acc` is live; `child` is valid for it.
    unsafe { acc.accDoDefaultAction(&child) }
        .map_err(|error| format!("accDoDefaultAction failed: {error}"))
}

/// Re-reads a node previously seen at `key`. Returns `None` if it can no longer
/// be acquired. Blocking; query pool only.
#[must_use]
pub fn resnapshot(key: MsaaKey, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let (hwnd, id_object, id_child) = key;
    snapshot_from_event(hwnd, id_object, id_child, registry)
}

/// Reads the selected child of a selection container via `accSelection`: a
/// `VT_I4` result names a child by id on the container itself, a
/// `VT_DISPATCH` carries the child's own `IAccessible`. A multi-selection
/// (`VT_UNKNOWN` carrying an `IEnumVARIANT`) and an empty selection both
/// report `None` — the reducer speaks one item, and richer multi-selection
/// reporting is deliberately out of M3's scope. Only meaningful for a key
/// addressing a full object (`CHILDID_SELF`); a child-id key reports `None`
/// since a simple child cannot contain anything. Blocking; query pool only.
#[must_use]
pub fn selected_child(key: MsaaKey, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let (hwnd, id_object, id_child) = key;
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let (acc, child) = unsafe { accessible_and_child(hwnd, id_object, id_child) }?;
    // SAFETY: `child` was just acquired together with `acc`.
    if unsafe { child_id_of(&child) } != CHILDID_SELF {
        return None;
    }
    // SAFETY: `acc` is a live IAccessible.
    let selection = unsafe { acc.accSelection() }.ok()?;
    // SAFETY: the variant type is checked before any union field is read.
    unsafe {
        let vt = selection.Anonymous.Anonymous.vt;
        if vt == VT_I4 {
            let child_id = selection.Anonymous.Anonymous.Anonymous.lVal;
            let child_key = (hwnd, id_object, child_id);
            return Some(read_snapshot(
                &acc,
                &child_variant(child_id),
                child_key,
                registry,
            ));
        }
        if vt == VT_DISPATCH {
            let dispatch = selection.Anonymous.Anonymous.Anonymous.pdispVal.as_ref()?;
            let child_acc = dispatch.cast::<IAccessible>().ok()?;
            let child_hwnd = window_of(&child_acc).unwrap_or(hwnd);
            let child_key = (child_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            return Some(read_snapshot(
                &child_acc,
                &child_variant(CHILDID_SELF),
                child_key,
                registry,
            ));
        }
    }
    None
}

/// Reads the currently focused object of `target_pid` for the synthetic focus
/// event an `AnnounceFocus` triggers. Uses `GetGUIThreadInfo` then `accFocus`, with
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

/// Whether `hwnd` is a window a user would navigate onto — NVDA's
/// `isUsableWindow`, reduced to its load-bearing check: it must be visible.
/// (NVDA also rejects hung and DWM-ghost windows; those are a
/// responsiveness guard, not a correctness one, and the query pool's
/// deadline already bounds a hung provider here.)
fn is_usable_window(hwnd: isize) -> bool {
    // SAFETY: IsWindowVisible tolerates any handle, returning false for an
    // invalid one.
    hwnd != 0 && unsafe { IsWindowVisible(HWND(hwnd as *mut c_void)) }.as_bool()
}

/// The window-object snapshot for `hwnd` (`OBJID_WINDOW`, `CHILDID_SELF`): the
/// window face `read_snapshot` keys under `OBJID_WINDOW`. `None` if the
/// window cannot be acquired.
fn window_object_snapshot(hwnd: isize, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    // SAFETY: forwarded to `accessible_and_child`'s contract.
    let (acc, child) = unsafe { accessible_and_child(hwnd, OBJID_WINDOW.0, CHILDID_SELF) }?;
    // SAFETY: `acc`/`child` were just acquired together; the key names the
    // same window object `read_snapshot` will re-key under OBJID_WINDOW.
    Some(unsafe { read_snapshot(&acc, &child, (hwnd, OBJID_WINDOW.0, CHILDID_SELF), registry) })
}

/// Navigates the Win32 window hierarchy from a window-root object, NVDA's
/// Window/WindowRoot navigation: parent is the parent window (`GA_PARENT`),
/// siblings are the next and previous usable top-level child windows
/// (`GW_HWNDNEXT`/`GW_HWNDPREV`, skipping invisible ones), and the first
/// child is the first usable child window, or — for a leaf control with no
/// child windows — the control's own client object, the content inside the
/// frame (a settings slider's actual slider). `None` at each edge.
fn window_navigate(
    hwnd: isize,
    direction: NavigateDirection,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    let handle = HWND(hwnd as *mut c_void);
    match direction {
        NavigateDirection::Parent => {
            // SAFETY: GetAncestor/GetDesktopWindow tolerate any handle.
            let parent = unsafe { GetAncestor(handle, GA_PARENT) };
            let desktop = unsafe { GetDesktopWindow() };
            if parent.0.is_null() || parent.0 == desktop.0 {
                return None;
            }
            window_object_snapshot(parent.0 as isize, registry)
        }
        NavigateDirection::NextSibling => window_sibling(hwnd, GW_HWNDNEXT, registry),
        NavigateDirection::PreviousSibling => window_sibling(hwnd, GW_HWNDPREV, registry),
        NavigateDirection::FirstChild => {
            // SAFETY: GetTopWindow tolerates any handle.
            let mut child = unsafe { GetTopWindow(Some(handle)) }.unwrap_or_default();
            while !child.0.is_null() && !is_usable_window(child.0 as isize) {
                // SAFETY: `child` is a live handle from the walk above.
                child = unsafe { GetWindow(child, GW_HWNDNEXT) }.unwrap_or_default();
            }
            if child.0.is_null() {
                // A leaf control: its "child" is the content inside its own
                // frame, the client object of the same window.
                // SAFETY: forwarded to `accessible_and_child`'s contract.
                let (acc, ch) =
                    unsafe { accessible_and_child(hwnd, OBJID_CLIENT.0, CHILDID_SELF) }?;
                let key = (hwnd, OBJID_CLIENT.0, CHILDID_SELF);
                // SAFETY: `acc`/`ch` just acquired together.
                return Some(unsafe { read_snapshot(&acc, &ch, key, registry) });
            }
            window_object_snapshot(child.0 as isize, registry)
        }
    }
}

/// The next or previous usable sibling window of `hwnd` in the given
/// `GetWindow` direction, skipping invisible windows. `None` at the edge.
fn window_sibling(
    hwnd: isize,
    direction: windows::Win32::UI::WindowsAndMessaging::GET_WINDOW_CMD,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    let mut current = HWND(hwnd as *mut c_void);
    loop {
        // SAFETY: `current` is a live handle from the caller or the previous
        // hop; GetWindow returns an error (mapped to null) at the edge.
        current = unsafe { GetWindow(current, direction) }.ok()?;
        if current.0.is_null() {
            return None;
        }
        if is_usable_window(current.0 as isize) {
            return window_object_snapshot(current.0 as isize, registry);
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

/// Reads name, role, value, state, and the M3 [`NodeDetails`] properties
/// plain MSAA offers (`accDescription`, `accKeyboardShortcut`, `accLocation`)
/// from an accessible and its child id. Position-in-set stays `None` on this
/// backend until IA2 lands in M6 (architecture section 4;
/// `IServiceProvider::QueryService` is the seam, not touched here).
///
/// Level is the one exception: a `SysTreeView32` item ([`Role::TreeItem`])
/// overloads `accValue` to report its 0-based indent depth as a numeric
/// string rather than a real value — NVDA's `sysTreeView32.py` overrides
/// `TreeViewItem.value` to `None` for exactly this reason. This reads that
/// same string into [`NodeDetails::level`] instead, one-based to match
/// NVDA's spoken level (confirmed live: the root item's raw `accValue` is
/// `"0"`, which read as `value` used to make Verbatim announce the root's
/// value as "0" instead of its level), and leaves `value` itself `None` for
/// a tree item, matching NVDA.
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
        let raw_value = acc
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
        let description = acc
            .get_accDescription(child)
            .ok()
            .and_then(|b| bstr_to_option(&b));
        let keyboard_shortcut = acc
            .get_accKeyboardShortcut(child)
            .ok()
            .and_then(|b| bstr_to_option(&b));
        let rect = location_of(acc, child);
        // See this function's doc comment: a tree item's accValue is really
        // its 0-based level, not a value.
        let (value, level) = if role == Role::TreeItem {
            let level = raw_value
                .as_deref()
                .and_then(|v| v.parse::<u32>().ok())
                .map(|v| v + 1);
            (None, level)
        } else {
            (raw_value, None)
        };
        // A window object — MSAA's second face of every windowed control,
        // role ROLE_SYSTEM_WINDOW alongside the client object's real role —
        // gets its identity keyed under OBJID_WINDOW, never OBJID_CLIENT.
        // Callers key by acquisition path and mostly assume OBJID_CLIENT,
        // which collided the window object onto the client object's node:
        // re-acquiring that key yielded the client again, so navigating to
        // a parent window object announced it once and then went nowhere —
        // observed live as "parent is stuck" from any hwnd-backed control.
        let key = if role == Role::Window && key.2 == CHILDID_SELF && key.1 == OBJID_CLIENT.0 {
            (key.0, OBJID_WINDOW.0, key.2)
        } else {
            key
        };
        NodeSnapshot {
            id: registry.id_for(key),
            backend: Backend::Msaa,
            role,
            name,
            value,
            states,
            details: NodeDetails {
                description,
                keyboard_shortcut,
                position_in_set: None,
                set_size: None,
                level,
                rect,
            },
        }
    }
}

/// Reads `accLocation` (screen coordinates, already left/top/width/height —
/// no conversion needed, unlike UIA's `BoundingRectangle`). `None` when the
/// call fails, which is how MSAA reports "not supported" here (unlike UIA,
/// plain MSAA has no documented default-value trap for this property).
///
/// # Safety
///
/// `acc` must be a live `IAccessible` and `child` a valid child-id `VARIANT`
/// for it.
unsafe fn location_of(acc: &IAccessible, child: &VARIANT) -> Option<Rect> {
    let mut left = 0i32;
    let mut top = 0i32;
    let mut width = 0i32;
    let mut height = 0i32;
    // SAFETY: forwarded to the caller's contract; the four out-parameters are
    // local, fully owned `i32`s written by `accLocation` on success.
    unsafe {
        acc.accLocation(
            &raw mut left,
            &raw mut top,
            &raw mut width,
            &raw mut height,
            child,
        )
        .ok()?;
    }
    Some(Rect {
        left,
        top,
        width,
        height,
    })
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
