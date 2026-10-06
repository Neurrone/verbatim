//! MSAA acquisition on the outpost's worker.
//!
//! Everything here makes blocking cross-process COM calls and must run only on
//! the outpost's deadline-guarded worker, never on the event thread. Starting
//! from a `WinEvent` address, or from the focused window, it acquires an
//! `IAccessible`, reads name/role/value/state, and maps to a [`NodeSnapshot`].
//!
//! `IAccessible2` is not implemented. Roadmap M6 adds it here, with the
//! browsers that need it, by calling `IServiceProvider::QueryService` on the
//! `IAccessible` obtained below for the IA2 text, hypertext, and relation
//! interfaces.
//!
//! `SysTreeView32` seam: a Win32 common-control tree view (comctl32) exposes
//! every visible item to MSAA as a flat sibling list directly under the tree
//! control, not nested under its logical parent — confirmed live against
//! msinfo32. [`navigate`] and [`ancestor_chain`] detect this window class and
//! route a tree item's navigation through the control's own `TVM_*` window
//! messages instead of `accNavigate`/`accParent`, mirroring NVDA's
//! `sysTreeView32.py`. Those messages are sent with plain `SendMessageW`,
//! which can block if the owning application is wedged — acceptable only
//! because every caller of this module already runs on the outpost's
//! deadline-guarded worker (never the event thread), the same bound every other
//! blocking call in this module already relies on.

use windows::Win32::Foundation::{
    CO_E_OBJNOTCONNECTED, RPC_E_DISCONNECTED, RPC_E_SERVER_DIED, RPC_E_SERVER_DIED_DNE,
};
use windows::Win32::UI::Accessibility::{NAVDIR_FIRSTCHILD, NAVDIR_NEXT, NAVDIR_PREVIOUS};
use windows::Win32::UI::Controls::{TVGN_CHILD, TVGN_NEXT, TVGN_PARENT, TVGN_PREVIOUS};
use windows::Win32::UI::WindowsAndMessaging::{
    GET_WINDOW_CMD, GW_HWNDNEXT, GW_HWNDPREV, OBJID_CLIENT, OBJID_WINDOW,
};

use verbatim_model::{Backend, NodeDetails, NodeId, NodeSnapshot, QueryKind, Role, TreeNode};

use crate::accessible::{Accessible, Related};
use crate::com::{CHILDID_SELF, non_empty, visible_text};
use crate::map::{role_from_msaa, states_from_msaa};
use crate::registry::{Found, Held, MsaaKey, NodeIdRegistry};
use crate::window;

/// Why a read of a node this outpost issued failed.
#[derive(Debug, PartialEq, Eq)]
pub enum AcquireError {
    /// The node is no longer reachable: it was released, its window was
    /// destroyed, or its object has disconnected.
    Gone,
    /// The read failed for another reason.
    Failed(String),
}

/// Whether `error` says the object's process or the object itself has gone.
fn disconnected(error: &windows::core::Error) -> bool {
    [
        RPC_E_DISCONNECTED,
        CO_E_OBJNOTCONNECTED,
        RPC_E_SERVER_DIED,
        RPC_E_SERVER_DIED_DNE,
    ]
    .contains(&error.code())
}

/// The kept object behind `node`, with its child variant and address (outpost
/// redesign, "Held objects"): the object that was announced, never whatever
/// now sits at its address. A node issued from local window data has no
/// object and is acquired at its address. `Gone` when the node is not kept,
/// its window no longer exists, or its object cannot be reached.
///
/// Also returns whether the object was acquired at its address, which a
/// neighbor addressed by child id on the same object inherits.
fn locate(
    node: NodeId,
    registry: &NodeIdRegistry,
) -> Result<(Accessible, MsaaKey, bool), AcquireError> {
    let (key, held, at_address) = registry.locate(node).ok_or(AcquireError::Gone)?;
    if !window::exists(key.0) {
        return Err(AcquireError::Gone);
    }
    match held {
        Some((object, child)) => object
            .resolve()
            .map(|acc| (Accessible::new(acc, child), key, at_address))
            .map_err(|_| AcquireError::Gone),
        None => Accessible::from_event(key.0, key.1, key.2)
            .map(|acc| (acc, key, at_address))
            .ok_or(AcquireError::Gone),
    }
}

/// Acquires the object named by a `WinEvent` and maps it to a [`NodeSnapshot`].
/// Returns `None` if the object cannot be acquired. Blocking; worker only.
#[must_use]
pub fn snapshot_from_event(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    let acc = Accessible::from_event(hwnd, id_object, id_child)?;
    Some(read_snapshot(
        &acc,
        (hwnd, id_object, id_child),
        true,
        registry,
    ))
}

/// Acquires the object named by an `EVENT_OBJECT_FOCUS` `WinEvent`, applying
/// NVDA's child-0-on-a-list redirect before mapping to a [`NodeSnapshot`].
/// Returns `None` if the object cannot be acquired. Blocking; worker only.
///
/// NVDA's `processFocusWinEvent`
/// (`nvda/source/IAccessibleHandler/__init__.py`): some controls fire
/// `EVENT_OBJECT_FOCUS` on child id 0 even when a child holds the focus — the
/// wxWidgets generic list is one, firing on the list object itself while a list
/// item is focused. When the event names a list on its own object (child id 0
/// and MSAA role `ROLE_SYSTEM_LIST`) or the client of a `SysListView32` window,
/// NVDA reads `accFocus` and redirects the focus object to the named child if
/// it is a real, different child. This mirrors that condition exactly — no
/// broader — so a focus event on such a container announces the focused item,
/// not the container, and the navigator lands on the item. Every other focus
/// event maps directly, the same as [`snapshot_from_event`], which is what the
/// menu-popup and other non-focus paths keep using.
#[must_use]
pub fn snapshot_from_focus_event(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    let acc = Accessible::from_event(hwnd, id_object, id_child)?;
    if let Some((focus_acc, key)) = redirect_focus_to_child(&acc, hwnd, id_object, id_child) {
        return Some(read_snapshot(&focus_acc, key, true, registry));
    }
    Some(read_snapshot(
        &acc,
        (hwnd, id_object, id_child),
        true,
        registry,
    ))
}

/// NVDA's `processFocusWinEvent` redirect: when a focus event names a list
/// container on child id 0 (MSAA role `ROLE_SYSTEM_LIST`) or the client of a
/// `SysListView32` window, and `accFocus` names a real, different child *by
/// id*, return that child's accessible and [`MsaaKey`]. `None`
/// when the condition does not hold or `accFocus` names no distinct child, so
/// the caller keeps the event's own object.
///
/// The child-object (`VT_DISPATCH`) form of `accFocus` deliberately does not
/// redirect: NVDA's guard is `isinstance(realChildID, int) and realChildID > 0
/// and realChildID != childID`, so a Dispatch result fails the `isinstance`
/// check and NVDA keeps the container — matched here by the catch-all `None`.
/// Redirecting on it would also be unsound, because a child object with no
/// window handle of its own falls back to the event's own `hwnd`, keying the
/// child's snapshot under the container's `(hwnd, OBJID_CLIENT, CHILDID_SELF)`
/// node identity, so a later refetch of that node id would read the container.
fn redirect_focus_to_child(
    acc: &Accessible,
    hwnd: isize,
    id_object: i32,
    id_child: i32,
) -> Option<(Accessible, MsaaKey)> {
    if !focus_event_names_list(acc, hwnd, id_object, id_child) {
        return None;
    }
    match read_acc_focus(acc) {
        // A child by id: redirect only when it is a real child (greater than
        // zero) and not the one the event already named — NVDA's exact guard.
        FocusTarget::ChildId(real_child) if real_child > 0 && real_child != id_child => {
            Some((acc.with_child(real_child), (hwnd, id_object, real_child)))
        }
        // A `VT_DISPATCH` child object, `None`, or a self/zero child id: keep
        // the container (see this function's doc for why the dispatch form is
        // deliberately not redirected).
        _ => None,
    }
}

/// Whether an `EVENT_OBJECT_FOCUS` address matches NVDA's redirect condition:
/// a list on its own object (child id 0, MSAA role `ROLE_SYSTEM_LIST`), or the
/// client of a `SysListView32` window.
fn focus_event_names_list(acc: &Accessible, hwnd: isize, id_object: i32, id_child: i32) -> bool {
    if id_child == CHILDID_SELF {
        let role = acc
            .role()
            .map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()));
        if role == Role::List {
            return true;
        }
    }
    id_object == OBJID_CLIENT.0 && window::class_name(hwnd).contains("SysListView32")
}

/// Walks the chain of ancestors of the node named by `key`, nearest first,
/// as [`NodeSnapshot`]s. Every hop is its own cross-process round trip
/// (architecture section 4's IA2 cost model: no cache requests on this
/// backend), via `IAccessible::accParent` — except the first hop for a node
/// addressed as a "simple child" (a bare child id, not its own `IDispatch`),
/// whose immediate parent is the object it is a child of, since plain MSAA
/// has no `accParent` for a child id, only for a full object. Capped at
/// `max_hops` ancestors; stops early, without error, once a hop finds no
/// further parent or fails. Blocking; worker only.
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
///
/// # Errors
///
/// [`AcquireError::Gone`] when `node` is no longer reachable; a hop that
/// fails after that only ends the chain early.
pub fn ancestor_chain(
    node: NodeId,
    registry: &NodeIdRegistry,
    max_hops: u32,
) -> Result<Vec<NodeSnapshot>, AcquireError> {
    let limits = AncestorLimits {
        max_hops,
        known: &|_| false,
        read_by_other_api: &|_| false,
        deadline: None,
    };
    ancestor_chain_until(node, registry, &limits).map(|(chain, _)| chain)
}

/// What bounds an ancestor walk besides reaching the root.
pub struct AncestorLimits<'a> {
    /// The most ancestors to read.
    pub max_hops: u32,
    /// Whether an ancestor is already known, from the previous focus's
    /// chain: the walk stops there, as NVDA's focus ancestry stops where it
    /// meets the previous focus's ancestors and reuses them.
    pub known: &'a dyn Fn(NodeId) -> bool,
    /// Whether a window is read through the other API: the walk stops
    /// before a parent in a different window for which this holds, as NVDA
    /// switches API where a parent lies in such a window.
    pub read_by_other_api: &'a dyn Fn(isize) -> bool,
    /// When to give up.
    pub deadline: Option<std::time::Instant>,
}

/// How an ancestor walk ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walked {
    /// It reached the root, or a hop found no parent.
    Complete,
    /// It reached this known ancestor, the outermost one in the chain.
    MetKnown(NodeId),
    /// It ran out of time, so the chain is incomplete.
    OutOfTime,
    /// The next parent is in this window, which is read through the other
    /// API; the chain ends below it.
    Crossed(isize),
}

/// [`ancestor_chain`] within `limits`: the chain, outermost first, and how
/// the walk ended. A walk that meets a known ancestor ends with it as the
/// chain's outermost entry.
///
/// # Errors
///
/// [`AcquireError::Gone`] when `node` is no longer reachable.
pub fn ancestor_chain_until(
    node: NodeId,
    registry: &NodeIdRegistry,
    limits: &AncestorLimits<'_>,
) -> Result<(Vec<NodeSnapshot>, Walked), AcquireError> {
    let max_hops = limits.max_hops;
    let out_of_time = || {
        limits
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    };
    let mut walked = Walked::Complete;
    let (acc, (hwnd, id_object, _), at) = locate(node, registry)?;
    let mut chain = Vec::new();
    let is_simple_child = acc.child() != CHILDID_SELF;
    let mut hops_used = 0u32;

    if is_simple_child && is_systreeview32(hwnd) {
        let mut current_acc_id = acc.child();
        while hops_used < max_hops {
            let Some(parent_acc_id) = tree_view_relation_acc_id(hwnd, current_acc_id, TVGN_PARENT)
            else {
                // The item is a root: fall through to the tree control
                // itself, via the ordinary simple-child handling below.
                break;
            };
            let parent_key = (hwnd, id_object, parent_acc_id);
            // The tree control's own object, at one of its simple children.
            let snapshot = read_snapshot(&acc.with_child(parent_acc_id), parent_key, at, registry);
            let id = snapshot.id;
            chain.push(snapshot);
            hops_used += 1;
            current_acc_id = parent_acc_id;
            if (limits.known)(id) {
                walked = Walked::MetKnown(id);
                break;
            }
            if out_of_time() {
                walked = Walked::OutOfTime;
                break;
            }
        }
    }

    let mut current = acc.with_child(CHILDID_SELF);
    let mut current_hwnd = hwnd;
    let mut at_self = !is_simple_child;
    while walked == Walked::Complete && hops_used < max_hops {
        if out_of_time() {
            walked = Walked::OutOfTime;
            break;
        }
        if !at_self {
            let self_hwnd = current.window().unwrap_or(hwnd);
            let self_key = (self_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            // The object a simple child belongs to sits at the client address
            // of the child's own window, when the child was acquired there.
            let self_at = at && id_object == OBJID_CLIENT.0 && self_hwnd == hwnd;
            let snapshot = read_snapshot(&current, self_key, self_at, registry);
            let id = snapshot.id;
            chain.push(snapshot);
            current_hwnd = self_hwnd;
            at_self = true;
            hops_used += 1;
            if (limits.known)(id) {
                walked = Walked::MetKnown(id);
            }
            continue;
        }
        let Ok(Some(parent_acc)) = current.parent() else {
            break;
        };
        let parent_hwnd = parent_acc.window().unwrap_or(hwnd);
        // The desktop is the root, never an ancestor: NVDA's focus ancestors
        // stop below it, so it is never announced as an entered container.
        if parent_hwnd == window::desktop() {
            break;
        }
        if parent_hwnd != current_hwnd && (limits.read_by_other_api)(parent_hwnd) {
            walked = Walked::Crossed(parent_hwnd);
            break;
        }
        let parent_key = (parent_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        let snapshot = read_snapshot(&parent_acc, parent_key, false, registry);
        let id = snapshot.id;
        chain.push(snapshot);
        hops_used += 1;
        current = parent_acc;
        current_hwnd = parent_hwnd;
        if (limits.known)(id) {
            walked = Walked::MetKnown(id);
        }
    }
    chain.reverse();
    Ok((chain, walked))
}

/// Returns whether `hwnd` is a `SysTreeView32` common control (comctl32's
/// tree view) — see this module's top doc comment for why its items need
/// the `TVM_*`-based navigation below instead of `accNavigate`/`accParent`.
/// Reading the class name is a local call, safe even against a hung window.
fn is_systreeview32(hwnd: isize) -> bool {
    window::class_name(hwnd) == "SysTreeView32"
}

/// Maps an MSAA child id to its `HTREEITEM`, via `TVM_MAPACCIDTOHTREEITEM`.
/// Falls back to using the child id as the `HTREEITEM` value directly when
/// the message returns 0: comctl32 versions before v6 have no accid/htreeitem
/// mapping and use the hItem as the child id outright, exactly as NVDA's
/// `sysTreeView32.py` (`treeview_hItem`) does.
fn htreeitem_for_acc_id(hwnd: isize, acc_id: i32) -> isize {
    let wparam = usize::try_from(acc_id.cast_unsigned()).unwrap_or(0);
    let mapped = window::tree_view_item_for_acc_id(hwnd, wparam);
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
fn acc_id_for_htreeitem(hwnd: isize, hitem: isize) -> i32 {
    let wparam = usize::try_from(hitem).unwrap_or(0);
    let mapped = window::tree_view_acc_id_for_item(hwnd, wparam);
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
    let hitem = htreeitem_for_acc_id(hwnd, acc_id);
    if hitem == 0 {
        return None;
    }
    let neighbor_hitem = window::tree_view_next_item(hwnd, relation, hitem);
    if neighbor_hitem == 0 {
        return None;
    }
    Some(acc_id_for_htreeitem(hwnd, neighbor_hitem))
}

/// The four navigation directions of a navigation [`QueryKind`] (parent,
/// next and previous sibling, first child), as this module matches on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NavigateDirection {
    /// The node's parent.
    Parent,
    /// The next sibling in tree order.
    NextSibling,
    /// The previous sibling in tree order.
    PreviousSibling,
    /// The first child.
    FirstChild,
}

/// Navigates one step from the node named by `key` in the direction `kind`
/// names; any other kind is an error.
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
/// Blocking; worker only.
///
/// # Errors
///
/// Returns [`AcquireError::Gone`] when the source node itself is no longer
/// reachable.
/// `Ok(None)` is a genuine edge: the source node is fine, but there is no
/// neighbor in that direction (a root's parent, a last child's next
/// sibling, a leaf's first child). The two are never conflated, so callers
/// can distinguish "this node vanished" from "this is the end of the tree".
#[expect(
    clippy::too_many_lines,
    reason = "the window-root, tree-view, simple-child, and accNavigate cases read best side by side"
)]
pub fn navigate(
    node: NodeId,
    registry: &NodeIdRegistry,
    kind: QueryKind,
) -> Result<Option<NodeSnapshot>, AcquireError> {
    let direction = match kind {
        QueryKind::Parent => NavigateDirection::Parent,
        QueryKind::NextSibling => NavigateDirection::NextSibling,
        QueryKind::PreviousSibling => NavigateDirection::PreviousSibling,
        QueryKind::FirstChild => NavigateDirection::FirstChild,
        _ => {
            return Err(AcquireError::Failed(
                "not a navigation direction".to_owned(),
            ));
        }
    };
    let (hwnd, id_object, id_child) = registry.key_of(node).ok_or(AcquireError::Gone)?;
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
        if !window::exists(hwnd) {
            return Err(AcquireError::Gone);
        }
        return Ok(window_navigate(hwnd, direction, registry));
    }
    let (acc, _, at) = locate(node, registry)?;
    let is_simple_child = acc.child() != CHILDID_SELF;
    let tree_view = is_simple_child && is_systreeview32(hwnd);

    if direction == NavigateDirection::Parent {
        // A tree item that is a root has no `TVGN_PARENT`: it falls through
        // to the ordinary simple-child handling below, which lands on the
        // tree control itself.
        if tree_view
            && let Some(parent_acc_id) = tree_view_relation_acc_id(hwnd, acc.child(), TVGN_PARENT)
        {
            let parent_key = (hwnd, id_object, parent_acc_id);
            // The tree control's own object, at one of its simple children.
            return Ok(Some(read_snapshot(
                &acc.with_child(parent_acc_id),
                parent_key,
                at,
                registry,
            )));
        }
        if is_simple_child {
            // The immediate parent of a simple child (addressed only by a
            // child id, not its own IDispatch) is the object it is a child
            // of; see this module's `ancestor_chain` doc for the same case.
            let self_hwnd = acc.window().unwrap_or(hwnd);
            let self_key = (self_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            let self_at = at && id_object == OBJID_CLIENT.0 && self_hwnd == hwnd;
            return Ok(Some(read_snapshot(
                &acc.with_child(CHILDID_SELF),
                self_key,
                self_at,
                registry,
            )));
        }
        let parent_acc = match acc.parent() {
            Ok(Some(parent)) => parent,
            Err(error) if disconnected(&error) => return Err(AcquireError::Gone),
            Ok(None) | Err(_) => return Ok(None),
        };
        let parent_hwnd = parent_acc.window().unwrap_or(hwnd);
        let parent_key = (parent_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
        return Ok(Some(read_snapshot(
            &parent_acc,
            parent_key,
            false,
            registry,
        )));
    }

    if tree_view {
        let acc_id = acc.child();
        let relation = match direction {
            NavigateDirection::NextSibling => TVGN_NEXT,
            NavigateDirection::PreviousSibling => TVGN_PREVIOUS,
            NavigateDirection::FirstChild => TVGN_CHILD,
            NavigateDirection::Parent => unreachable!("handled above"),
        };
        return Ok(
            tree_view_relation_acc_id(hwnd, acc_id, relation).map(|neighbor_acc_id| {
                let neighbor_key = (hwnd, id_object, neighbor_acc_id);
                // The tree control's own object, at one of its simple
                // children.
                read_snapshot(&acc.with_child(neighbor_acc_id), neighbor_key, at, registry)
            }),
        );
    }

    let navdir = match direction {
        NavigateDirection::NextSibling => NAVDIR_NEXT,
        NavigateDirection::PreviousSibling => NAVDIR_PREVIOUS,
        NavigateDirection::FirstChild => NAVDIR_FIRSTCHILD,
        NavigateDirection::Parent => unreachable!("handled above"),
    };
    // The child id `accNavigate` is called with, which can differ from the
    // node's address: an object acquired at a child's address may be the
    // child's own object, with child 0.
    let source_child = acc.child();
    let navigated = match acc.navigate(navdir.cast_signed()) {
        Ok(Related::Nothing) => None,
        Ok(result) => Some(result),
        Err(error) if disconnected(&error) => return Err(AcquireError::Gone),
        // Another failure: no such neighbor.
        Err(_) => None,
    };
    let result = match navigated {
        Some(result) => result,
        // NVDA's `_get_firstChild`: an object whose `accNavigate` finds no
        // first child is asked for its children instead.
        None if direction == NavigateDirection::FirstChild && source_child == CHILDID_SELF => {
            match first_accessible_child(&acc) {
                Some(first) => first,
                None => return Ok(None),
            }
        }
        None => return Ok(None),
    };
    // `accNavigate` names a related object the same way `AccessibleChildren`
    // names a child.
    let (target_acc, target_hwnd) = resolve_child(&acc, &result, hwnd);
    let target_child_id = target_acc.child();
    let target_key = (target_hwnd, OBJID_CLIENT.0, target_child_id);
    let same_object = acc.canonical().is_some() && acc.canonical() == target_acc.canonical();
    tracing::debug!(
        ?direction,
        hwnd,
        id_child,
        target_hwnd,
        target_child_id,
        same_object,
        "MSAA navigation step"
    );
    // NVDA's `accNavigate` sanity check (its `IAccessible._get_next`,
    // `_get_previous`, and `_get_firstChild`): a control whose window is its
    // whole world answers sibling navigation with itself — the settings
    // dialog's rate slider answers next and previous with its own object
    // and child 0 (confirmed live, 2026-10-05) — and reading that as a
    // neighbor re-announced the same control instead of reporting the
    // edge. A result that is the same COM object is used only when its
    // child id moves the right way; another object always is, so a
    // different windowless object in the same window is a real neighbor.
    // Verbatim compares objects by their canonical `IUnknown`, where NVDA
    // compares interface pointers, so a self-return through another
    // interface pointer is also caught here.
    // Sibling navigation between the actual controls happens at the window
    // level (see `window_navigate`), which is why a user moves up to the
    // frame first.
    // NVDA's `_get_firstChild` takes another object as the first child only
    // when its window is this object's window or one inside it, or this
    // object is the desktop's.
    let in_own_window =
        target_hwnd == hwnd || window::is_child(hwnd, target_hwnd) || window::desktop() == hwnd;
    if !navigation_result_is_usable(
        direction,
        same_object,
        in_own_window,
        source_child,
        target_child_id,
    ) {
        return Ok(None);
    }
    Ok(Some(read_snapshot(
        &target_acc,
        target_key,
        false,
        registry,
    )))
}

/// Activates `node` through its kept object: `IAccessible::accDoDefaultAction`,
/// the only activation MSAA offers (UIA's richer `Invoke`/`Toggle` ladder has
/// no MSAA equivalent). Answers the default action's name
/// (`accDefaultAction`, such as "Press"), read first as NVDA reads it, or
/// `None` when it has none. Blocking; worker only.
///
/// # Errors
///
/// [`AcquireError::Gone`] if the node is no longer reachable;
/// [`AcquireError::Failed`] with a human-readable reason if the call fails
/// (including "not implemented", MSAA's answer for a node with no default
/// action).
pub fn activate(
    node: NodeId,
    registry: &NodeIdRegistry,
) -> Result<Option<verbatim_model::ActionName>, AcquireError> {
    let (acc, _, _) = locate(node, registry)?;
    let name = visible_text(acc.default_action());
    acc.do_default_action().map_err(|error| {
        if disconnected(&error) {
            AcquireError::Gone
        } else {
            AcquireError::Failed(format!("accDoDefaultAction failed: {error}"))
        }
    })?;
    Ok(name.map(verbatim_model::ActionName::Named))
}

/// Reads the selected child of a selection container via `accSelection`: a
/// `VT_I4` result names a child by id on the container itself, a
/// `VT_DISPATCH` carries the child's own `IAccessible`. A multi-selection
/// (`VT_UNKNOWN` carrying an `IEnumVARIANT`) and an empty selection both
/// report `None` — the reducer speaks one item, and richer multi-selection
/// reporting is deliberately out of M3's scope. Only meaningful for a key
/// addressing a full object (`CHILDID_SELF`); a child-id key reports `None`
/// since a simple child cannot contain anything. Blocking; worker only.
#[must_use]
pub fn selected_child(node: NodeId, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let (acc, (hwnd, id_object, _), at) = locate(node, registry).ok()?;
    if acc.child() != CHILDID_SELF {
        return None;
    }
    match acc.selection().ok()? {
        Related::Child(child_id) => {
            let child_key = (hwnd, id_object, child_id);
            Some(read_snapshot(
                &acc.with_child(child_id),
                child_key,
                at,
                registry,
            ))
        }
        selection @ Related::Object(_) => {
            let child_acc = selection.object()?;
            let child_hwnd = child_acc.window().unwrap_or(hwnd);
            let child_key = (child_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            Some(read_snapshot(&child_acc, child_key, false, registry))
        }
        Related::Nothing | Related::Other => None,
    }
}

/// Reads the currently focused object of `target_pid`, for a focus-now query
/// (including Core's after a menu closes). Uses `GetGUIThreadInfo` then `accFocus`,
/// with a fallback to the focused window itself. Blocking; worker only.
#[must_use]
pub fn focused_snapshot(target_pid: u32, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let (hwnd, pid) = window::focused()?;
    if pid != target_pid {
        return None;
    }
    let client = Accessible::client_of_window(hwnd)?;
    // A child object reached through `accFocus` has no address of its
    // own; the client and its children by id do.
    let (acc, at_address) = match read_acc_focus(&client) {
        FocusTarget::ChildObject(child_acc) => (child_acc, false),
        FocusTarget::ChildId(child_id) => (client.with_child(child_id), true),
        FocusTarget::None => (client, true),
    };
    let node_hwnd = acc.window().unwrap_or(hwnd);
    let key = (node_hwnd, OBJID_CLIENT.0, acc.child());
    Some(read_snapshot(&acc, key, at_address, registry))
}

/// Walks the MSAA tree from `hwnd`'s client accessible object, bounded by
/// `max_depth` (the root is depth 0) and `max_nodes` (the total node count
/// across the whole walk, including the root). Unlike UIA there are no
/// cache requests on this backend, so every step is its own cross-process
/// COM round trip (architecture section 4's IA2 cost model); blocking,
/// worker only, guarded by the caller's deadline. Returns `None` if the
/// window has no accessible client object.
#[must_use]
pub fn walk_tree(
    hwnd: isize,
    registry: &NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
) -> Option<(TreeNode, bool)> {
    let acc = Accessible::client_of_window(hwnd)?;
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
    let root = walk_recursive(&acc, hwnd, (key, true), &limits, 0, &mut state);
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
/// node named by `acc`. A simple element (addressed purely by a child id,
/// not its own `IAccessible`) is always a leaf per the MSAA model, so only
/// `CHILDID_SELF` nodes are ever expanded.
fn walk_recursive(
    acc: &Accessible,
    hwnd: isize,
    (key, at_address): (MsaaKey, bool),
    limits: &WalkLimits<'_>,
    depth: u32,
    state: &mut WalkState,
) -> TreeNode {
    let snapshot = read_snapshot(acc, key, at_address, limits.registry);

    if acc.child() != CHILDID_SELF {
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }

    if depth >= limits.max_depth {
        // Only peeks whether there is a child we are declining to descend
        // into.
        let has_children = acc.child_count().is_ok_and(|children| children > 0);
        if has_children {
            state.truncated = true;
        }
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    }

    let child_count = match acc.child_count() {
        Ok(children) if children > 0 => children,
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

    let Some(entries) = acc.children(child_count) else {
        return TreeNode {
            snapshot,
            children: Vec::new(),
        };
    };

    let mut children = Vec::new();
    for entry in &entries {
        if state.visited >= limits.max_nodes {
            state.truncated = true;
            break;
        }
        let (child_acc, child_hwnd) = resolve_child(acc, entry, hwnd);
        state.visited += 1;
        let child_id = child_acc.child();
        let child_key = (child_hwnd, OBJID_CLIENT.0, child_id);
        // A child by id is addressed on this object, as this object is; a
        // child object has no address of its own.
        let child_at = at_address && child_id != CHILDID_SELF;
        let child_node = walk_recursive(
            &child_acc,
            child_hwnd,
            (child_key, child_at),
            limits,
            depth + 1,
            state,
        );
        children.push(child_node);
    }

    TreeNode { snapshot, children }
}

/// Resolves one entry from `AccessibleChildren` into `(accessible, hwnd)` to
/// recurse into: a child id is a simple element addressed by that id on
/// `parent`; an object is its own `IAccessible`, addressed by
/// `CHILDID_SELF`, and may belong to a distinct window (a nested control),
/// resolved the same way [`focused_snapshot`] resolves a focused child
/// object. Anything else, or an object that is not an `IAccessible`, is
/// `parent` itself.
fn resolve_child(parent: &Accessible, entry: &Related, parent_hwnd: isize) -> (Accessible, isize) {
    match entry {
        Related::Object(_) => {
            if let Some(child_acc) = entry.object() {
                let hwnd = child_acc.window().unwrap_or(parent_hwnd);
                return (child_acc, hwnd);
            }
            (parent.with_child(CHILDID_SELF), parent_hwnd)
        }
        Related::Child(child_id) => (parent.with_child(*child_id), parent_hwnd),
        Related::Nothing | Related::Other => (parent.with_child(CHILDID_SELF), parent_hwnd),
    }
}

/// Whether an `accNavigate` result is a neighbor in `direction`, by NVDA's
/// check: another COM object is, except that a first child must be in the
/// object's own window or one inside it (`in_own_window`); the same object
/// is only when its child id moves the right way from `source_child`, past
/// it for the next sibling, before it (and after child 1) for the previous
/// one, and to a child of the object itself for the first child.
fn navigation_result_is_usable(
    direction: NavigateDirection,
    same_object: bool,
    in_own_window: bool,
    source_child: i32,
    target_child: i32,
) -> bool {
    if !same_object {
        return direction != NavigateDirection::FirstChild || in_own_window;
    }
    match direction {
        NavigateDirection::NextSibling => source_child > 0 && target_child > source_child,
        NavigateDirection::PreviousSibling => source_child > 1 && target_child < source_child,
        NavigateDirection::FirstChild => source_child == CHILDID_SELF && target_child > 0,
        NavigateDirection::Parent => true,
    }
}

/// The first entry `AccessibleChildren` reports for `acc`, as `accNavigate`
/// would name it, or `None` when it has no children.
fn first_accessible_child(acc: &Accessible) -> Option<Related> {
    acc.children(1)?.into_iter().next()
}

/// Whether `hwnd` is a window a user would navigate onto — NVDA's
/// `isUsableWindow`, reduced to its load-bearing check: it must be visible.
/// (NVDA also rejects hung and DWM-ghost windows; those are a
/// responsiveness guard, not a correctness one, and the worker's deadline
/// already bounds a hung provider here.)
fn is_usable_window(hwnd: isize) -> bool {
    hwnd != 0 && window::is_visible(hwnd)
}

/// The window-object snapshot for `hwnd` (`OBJID_WINDOW`, `CHILDID_SELF`): the
/// window face `read_snapshot` keys under `OBJID_WINDOW`. `None` if the
/// window cannot be acquired.
fn window_object_snapshot(hwnd: isize, registry: &NodeIdRegistry) -> Option<NodeSnapshot> {
    let acc = Accessible::from_event(hwnd, OBJID_WINDOW.0, CHILDID_SELF)?;
    // The key names the same window object `read_snapshot` would re-key
    // under OBJID_WINDOW.
    Some(read_snapshot(
        &acc,
        (hwnd, OBJID_WINDOW.0, CHILDID_SELF),
        true,
        registry,
    ))
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
    match direction {
        NavigateDirection::Parent => {
            let parent = window::parent(hwnd);
            if parent == 0 || parent == window::desktop() {
                return None;
            }
            window_object_snapshot(parent, registry)
        }
        NavigateDirection::NextSibling => window_sibling(hwnd, GW_HWNDNEXT, registry),
        NavigateDirection::PreviousSibling => window_sibling(hwnd, GW_HWNDPREV, registry),
        NavigateDirection::FirstChild => {
            let mut child = window::top_child(hwnd);
            while child != 0 && !is_usable_window(child) {
                child = window::related(child, GW_HWNDNEXT);
            }
            if child == 0 {
                // A leaf control: its "child" is the content inside its own
                // frame, the client object of the same window.
                let acc = Accessible::from_event(hwnd, OBJID_CLIENT.0, CHILDID_SELF)?;
                let key = (hwnd, OBJID_CLIENT.0, CHILDID_SELF);
                return Some(read_snapshot(&acc, key, true, registry));
            }
            window_object_snapshot(child, registry)
        }
    }
}

/// The next or previous usable sibling window of `hwnd` in the given
/// `GetWindow` direction, skipping invisible windows. `None` at the edge.
fn window_sibling(
    hwnd: isize,
    direction: GET_WINDOW_CMD,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    let mut current = hwnd;
    loop {
        current = window::related(current, direction);
        if current == 0 {
            return None;
        }
        if is_usable_window(current) {
            return window_object_snapshot(current, registry);
        }
    }
}

/// Reads name, role, value, state, and the [`NodeDetails`] properties plain
/// MSAA offers (`accDescription`, `accKeyboardShortcut`, `accLocation`) from
/// an accessible and its child id. Position-in-set is computed for list-view
/// and tree-view items (`position_of`) and is otherwise `None` until IA2's
/// group position (roadmap M6).
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
fn read_snapshot(
    acc: &Accessible,
    key: MsaaKey,
    at_address: bool,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    // Each read tolerates an unsupported property by failing, mapped to a
    // neutral default.
    let name = visible_text(acc.name());
    let raw_value = visible_text(acc.value());
    let role = acc
        .role()
        .map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()));
    let states = acc
        .state()
        .map(|s| states_from_msaa(s.cast_unsigned()))
        .unwrap_or_default();
    let description = non_empty(acc.description());
    let keyboard_shortcut = non_empty(acc.keyboard_shortcut());
    let rect = acc.location();
    // See this function's doc comment: a tree item's accValue is really
    // its 0-based level, not a value.
    // The edit field of a combo box takes the combo box's label, so it
    // has none of its own when the combo box is labelled, as in NVDA.
    let name = name.filter(|_| role != Role::EditableText || !in_labelled_combo_box(acc));
    let (position_in_set, set_size) = position_of(key.0, acc.child(), role);
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
        id: node_for(registry, key, at_address, acc, role),
        backend: Backend::Msaa,
        role,
        name,
        value,
        states,
        details: NodeDetails {
            description,
            keyboard_shortcut,
            position_in_set,
            set_size,
            level,
            rect,
        },
    }
}

/// Whether the edit field `acc` sits in a combo box that has a name of its
/// own: its parent, or the parent of a window object between them, is a
/// named combo box.
fn in_labelled_combo_box(acc: &Accessible) -> bool {
    // A failed call is "no parent".
    let parent_of = |acc: &Accessible| acc.parent().ok().flatten();
    // A failed read is an unknown role.
    let role_of = |acc: &Accessible| {
        acc.role()
            .map_or(Role::Unknown, |role| role_from_msaa(role.cast_unsigned()))
    };
    // A child id's parent is the object that holds it.
    let parent = if acc.child() == CHILDID_SELF {
        parent_of(acc)
    } else {
        Some(acc.with_child(CHILDID_SELF))
    };
    let Some(mut parent) = parent else {
        return false;
    };
    if role_of(&parent) == Role::Window {
        let Some(grandparent) = parent_of(&parent) else {
            return false;
        };
        parent = grandparent;
    }
    if role_of(&parent) != Role::ComboBox {
        return false;
    }
    // A failed read is "no name".
    visible_text(parent.name()).is_some()
}

/// An item's position in its set and the set's size, for an item of a
/// comctl32 list view or tree view, which MSAA gives no way to ask for, as
/// NVDA computes them: a list view item is at its child id among
/// `LVM_GETITEMCOUNT` items; a tree view item is counted among its siblings
/// through `TVM_GETNEXTITEM`. `(None, None)` for anything else.
fn position_of(hwnd: isize, child_id: i32, role: Role) -> (Option<u32>, Option<u32>) {
    /// More siblings than any real tree view holds, so a broken control
    /// cannot keep the walk going.
    const MAX_SIBLINGS: u32 = 100_000;
    if child_id == CHILDID_SELF || hwnd == 0 {
        return (None, None);
    }
    match role {
        Role::ListItem if window::class_name(hwnd).contains("SysListView32") => {
            let items = window::list_view_item_count(hwnd);
            let items = u32::try_from(items).ok().filter(|&items| items > 0);
            (
                u32::try_from(child_id).ok().filter(|_| items.is_some()),
                items,
            )
        }
        Role::TreeItem if is_systreeview32(hwnd) => {
            let item = htreeitem_for_acc_id(hwnd, child_id);
            if item == 0 {
                return (None, None);
            }
            let walk = |relation: u32| {
                let mut count = 0u32;
                let mut current = item;
                while current != 0 && count < MAX_SIBLINGS {
                    count += 1;
                    current = window::tree_view_next_item(hwnd, relation, current);
                }
                count
            };
            // Counting the item itself both ways.
            let index = walk(TVGN_PREVIOUS);
            let after = walk(TVGN_NEXT);
            (Some(index), Some(index + after - 1))
        }
        _ => (None, None),
    }
}

/// The node for an object just read for `key`, matched against the kept
/// nodes in NVDA's comparison order for MSAA objects (`docs/parity.md`,
/// "Held objects"): a kept node that is the same COM object with the same
/// child id in the same window is that node; otherwise, if the object was
/// acquired at `key` (`at_address`), a kept node acquired at the same address
/// is that node when a fresh read of its kept object has the same role as
/// `role` and the same identity string (`IAccIdentity`), both absent
/// counting as the same. A kept object that can no longer be resolved is a
/// different object; one whose role read fails reads as an unknown role, as
/// NVDA's does, so it no longer matches a sighting that has a role. A kept
/// node with
/// no object is compared by the role it was issued with. Anything else is
/// issued a new node, which keeps `acc` as its object. An address made up
/// for an object reached through `accParent` or as a child object is never
/// compared, since other objects in the same window share it.
///
/// NVDA also compares `IAccessible2` unique ids, which Verbatim does not read
/// yet, and the location and name, which are not compared here yet either.
fn node_for(
    registry: &NodeIdRegistry,
    key: MsaaKey,
    at_address: bool,
    acc: &Accessible,
    role: Role,
) -> NodeId {
    let child = acc.child();
    let identity = acc.canonical();
    match registry.find(key, identity, child, at_address) {
        // The kept object must still have that identity: while it is kept
        // its address cannot be reused by another object.
        Found::Object(id, held)
            if held.as_ref().is_some_and(|(object, held_child)| {
                object
                    .resolve()
                    .ok()
                    .and_then(|object| Accessible::new(object, *held_child).canonical())
                    == identity
            }) =>
        {
            registry.touch(id);
            return id;
        }
        Found::Key(id, held, held_role) => {
            let same = match held {
                Some((object, held_child)) => object.resolve().is_ok_and(|object| {
                    let object = Accessible::new(object, held_child);
                    // A failed role read is an unknown role on either side,
                    // as NVDA's is.
                    object
                        .role()
                        .map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()))
                        == role
                        && object.identity_string() == acc.identity_string()
                }),
                None => held_role.is_none_or(|held_role| held_role == role),
            };
            if same {
                registry.touch(id);
                return id;
            }
        }
        Found::Object(..) | Found::Nothing => {}
    }
    let held = identity.and_then(|identity| {
        Some(Held {
            object: acc.agile()?,
            child,
            identity,
        })
    });
    registry.insert(key, held, role, at_address)
}

/// What `accFocus` named on a client accessible: nothing distinct, a child by
/// id (`VT_I4`), or a child's own object (`VT_DISPATCH`). The single place that
/// parses the `accFocus` `VARIANT`, shared by [`focused_snapshot`] and
/// [`snapshot_from_focus_event`] (which needs to distinguish the forms to
/// apply NVDA's redirect rule).
enum FocusTarget {
    /// `accFocus` failed, or named the client itself (`CHILDID_SELF`), or an
    /// unhandled variant form.
    None,
    /// A child addressed by id on the client object.
    ChildId(i32),
    /// A child exposed as its own `IAccessible`.
    ChildObject(Accessible),
}

/// Reads `accFocus` on `client` into a [`FocusTarget`].
fn read_acc_focus(client: &Accessible) -> FocusTarget {
    match client.focus() {
        Ok(focus @ Related::Object(_)) => focus
            .object()
            .map_or(FocusTarget::None, FocusTarget::ChildObject),
        Ok(Related::Child(child_id)) => FocusTarget::ChildId(child_id),
        Ok(Related::Nothing | Related::Other) | Err(_) => FocusTarget::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_object_is_a_neighbor_only_when_its_child_id_moves_the_right_way() {
        use NavigateDirection::{FirstChild, NextSibling, PreviousSibling};
        let usable = |direction, same_object, source, target| {
            navigation_result_is_usable(direction, same_object, true, source, target)
        };
        // The settings slider: next and previous answer the slider itself.
        assert!(!usable(NextSibling, true, 0, 0));
        assert!(!usable(PreviousSibling, true, 0, 0));
        // A simple child stepping along its list.
        assert!(usable(NextSibling, true, 3, 4));
        assert!(!usable(NextSibling, true, 3, 3));
        assert!(usable(PreviousSibling, true, 3, 2));
        assert!(!usable(PreviousSibling, true, 1, 0));
        // The first child is a child of the object itself.
        assert!(usable(FirstChild, true, 0, 1));
        assert!(!usable(FirstChild, true, 0, 0));
        // Another object, a windowless sibling in the same window included.
        assert!(usable(NextSibling, false, 0, 0));
    }

    #[test]
    fn another_object_is_a_first_child_only_in_the_objects_own_windows() {
        use NavigateDirection::{FirstChild, NextSibling};
        assert!(navigation_result_is_usable(FirstChild, false, true, 0, 0));
        assert!(!navigation_result_is_usable(FirstChild, false, false, 0, 0));
        // Siblings are not held to it, as NVDA's are not.
        assert!(navigation_result_is_usable(NextSibling, false, false, 0, 0));
    }
}
