//! MSAA acquisition on the outpost's worker.
//!
//! Everything here makes blocking cross-process COM calls and must run only on
//! the outpost's deadline-guarded worker, never on the event thread. Starting
//! from a `WinEvent` address, or from the focused window, it acquires an
//! `IAccessible`, reads name/role/value/state, and maps to a [`NodeSnapshot`].
//!
//! `IAccessible2` is not implemented. Roadmap M7 adds it here, with the
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
//! `sysTreeView32.py`. Those messages are sent with `SendMessageTimeoutW`
//! ([`crate::window`]), so a wedged application fails them after half a
//! second, or at once when Windows judges it hung. The `IAccessible` calls
//! here have no bound of their own: every caller of this module runs on
//! the outpost's deadline-guarded worker (never the event thread), whose
//! watchdog abandons a call that passes the entry's deadline.

use windows::Win32::Foundation::{
    CO_E_OBJNOTCONNECTED, RPC_E_DISCONNECTED, RPC_E_SERVER_DIED, RPC_E_SERVER_DIED_DNE,
};
use windows::Win32::UI::Accessibility::{
    NAVDIR_FIRSTCHILD, NAVDIR_NEXT, NAVDIR_PREVIOUS, ROLE_SYSTEM_CLIENT, ROLE_SYSTEM_GROUPING,
    ROLE_SYSTEM_WINDOW,
};
use windows::Win32::UI::Controls::{TVGN_CHILD, TVGN_NEXT, TVGN_PARENT, TVGN_PREVIOUS};
use windows::Win32::UI::WindowsAndMessaging::{
    GET_WINDOW_CMD, GW_HWNDNEXT, GW_HWNDPREV, OBJID_CLIENT, OBJID_WINDOW,
};

use verbatim_model::{
    Backend, NodeDetails, NodeId, NodeSnapshot, ProgressReading, QueryKind, Role, State, TreeNode,
    WindowHandle,
};

use crate::accessible::{Accessible, Related};
use crate::class::normalized_class_of;
use crate::com::{CHILDID_SELF, non_empty, visible_text};
use crate::map::{adjust_role_and_states, role_from_msaa, states_from_msaa};
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
pub(crate) fn locate(
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

/// Why an object is read, which decides whether the details that cost
/// more calls are read too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Purpose {
    /// The object is spoken: a focus, a navigation step's neighbor, a
    /// selected item. Its position in its set is counted.
    #[default]
    Announce,
    /// The object is read for what it is, not to be spoken in full: an
    /// ancestor of the focus, or the object of a name, value, or state
    /// change. A list view or tree view item's position, which costs a
    /// window message per sibling, is not counted, as NVDA counts it only
    /// when it speaks the object, and its location is not read: an
    /// ancestor is read among many, which never read a location, and a
    /// change reports none.
    Context,
}

/// How [`read_snapshot_with`] reads an object: whether it was acquired at
/// its address, why it is read, and the role and state words already read
/// for it, which are not read again.
#[derive(Clone, Copy, Debug, Default)]
struct Reading {
    at_address: bool,
    purpose: Purpose,
    role: Prefetched,
    state: Prefetched,
    /// Whether its location (`accLocation`, a cross-process call) is left
    /// unread, as it is for an object read among many, an ancestor or a
    /// tree dump's node, and for one read for [`Purpose::Context`], which
    /// never uses it.
    without_location: bool,
}

/// A role or state word that may have been read already.
#[derive(Clone, Copy, Debug, Default)]
enum Prefetched {
    /// Not read yet.
    #[default]
    Unread,
    /// Read, with what the read answered.
    Read(Option<i32>),
}

impl Prefetched {
    /// The word, read by `read` if it was not read yet.
    fn or_read(self, read: impl FnOnce() -> Option<i32>) -> Option<i32> {
        match self {
            Self::Unread => read(),
            Self::Read(word) => word,
        }
    }
}

/// Acquires the object named by a `WinEvent` and maps it to a [`NodeSnapshot`]
/// read for `purpose`. Returns `None` if the object cannot be acquired.
/// Blocking; worker only.
#[must_use]
pub fn snapshot_from_event(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    registry: &NodeIdRegistry,
    purpose: Purpose,
) -> Option<NodeSnapshot> {
    let acc = Accessible::from_event(hwnd, id_object, id_child)?;
    Some(read_snapshot_with(
        &acc,
        (hwnd, id_object, id_child),
        Reading {
            at_address: true,
            purpose,
            ..Reading::default()
        },
        registry,
    ))
}

/// The object a `WinEvent` names, acquired but not yet read, so the
/// outpost can find out cheaply whether it is an object the event matters
/// for before reading it in full.
pub struct EventObject {
    acc: Accessible,
    key: MsaaKey,
    role: Prefetched,
}

/// Acquires the object a `WinEvent` names, reading nothing of it. `None`
/// if it cannot be acquired. Blocking; worker only.
#[must_use]
pub fn event_object(hwnd: isize, id_object: i32, id_child: i32) -> Option<EventObject> {
    Some(EventObject {
        acc: Accessible::from_event(hwnd, id_object, id_child)?,
        key: (hwnd, id_object, id_child),
        role: Prefetched::Unread,
    })
}

impl EventObject {
    /// Which of `nodes` the object is, by its identity, reading none of its
    /// properties: the node issued for the address the event names, else
    /// the node kept for the same COM object with the same child id in the
    /// same window. `None` when it is none of them.
    #[must_use]
    pub fn which_of(&self, nodes: &[NodeId], registry: &NodeIdRegistry) -> Option<NodeId> {
        let found = if let Some(&node) = nodes
            .iter()
            .find(|&&node| registry.key_of(node) == Some(self.key))
        {
            Some(node)
        } else {
            let identity = self.acc.canonical()?;
            match registry.find(self.key, Some(identity), self.acc.child(), false) {
                Found::Object(node, _) if nodes.contains(&node) => Some(node),
                _ => None,
            }
        };
        // Looked up, as a full read would record it.
        if let Some(node) = found {
            registry.touch(node);
        }
        found
    }

    /// The object's role, one call, kept for the read that may follow.
    pub fn role(&mut self) -> Role {
        let role = self.role.or_read(|| self.acc.role());
        self.role = Prefetched::Read(role);
        role.map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()))
    }

    /// The object's name, read alone, as a full read of an object of
    /// `role` gives it: a list view item named by its columns, an edit field
    /// in a labelled combo box with none of its own, and bidirectional
    /// formatting characters stripped. `role` is the role the object was
    /// last read with, which decides how it is named, as the class NVDA
    /// chose for its object when it made it does.
    #[must_use]
    pub fn name(&self, role: Role, registry: &NodeIdRegistry) -> Option<String> {
        let fetches = registry.fetches();
        let columns = is_list_view_item(&self.acc, self.key, role)
            .then(|| {
                crate::list_view::column_name(self.key.0, self.acc.child(), || {
                    fetches
                        .description
                        .then(|| non_empty(self.acc.description()))
                        .flatten()
                })
                .ok()
                .flatten()
            })
            .flatten();
        let mut name = columns.or_else(|| {
            visible_text(self.acc.name())
                .filter(|_| role != Role::EditableText || !in_labelled_combo_box(&self.acc))
        });
        if let Some(name) = &mut name {
            verbatim_text::strip_bidi_controls(name);
        }
        name
    }

    /// The object's description, read alone, as a full read of an object of
    /// `role` gives it: none when the active theme does not report
    /// descriptions or the object is a list view item, and bidirectional
    /// formatting characters stripped.
    #[must_use]
    pub fn description(&self, role: Role, registry: &NodeIdRegistry) -> Option<String> {
        if !registry.fetches().description || is_list_view_item(&self.acc, self.key, role) {
            return None;
        }
        let mut description = non_empty(self.acc.description());
        if let Some(description) = &mut description {
            verbatim_text::strip_bidi_controls(description);
        }
        description
    }

    /// The object's states, read alone, as a full read of an object of
    /// `role` gives them.
    #[must_use]
    pub fn states(&self, role: Role) -> verbatim_model::StateSet {
        role_and_states(&self.acc, self.key, role, self.acc.state()).1
    }

    /// The object's value, read alone, as a full read of an object of
    /// `role` gives it; not read at all for an object whose value is never
    /// reported, a `SysTreeView32` item or a list view item.
    #[must_use]
    pub fn value(&self, role: Role) -> Option<String> {
        let reported = match role {
            Role::TreeItem => !is_systreeview32(self.key.0),
            Role::ListItem => !is_list_view_item(&self.acc, self.key, role),
            _ => true,
        };
        let mut value = reported
            .then(|| value_for(&self.acc, self.key, role, visible_text(self.acc.value())))
            .flatten();
        if let Some(value) = &mut value {
            verbatim_text::strip_bidi_controls(value);
        }
        value
    }

    /// The object, a progress bar whose value changed, read for what its
    /// report uses, as NVDA's progress bar behavior reads it: its states,
    /// then its value and location. `known` is its node and the role it was
    /// last read with when it is already known, as the focus is; else its
    /// role is read and its node issued or looked up as a full read would.
    /// `None`, with nothing more read, when its states make it a busy
    /// indicator or say it is invisible: neither reports progress.
    pub fn progress(
        &mut self,
        known: Option<(NodeId, Role)>,
        registry: &NodeIdRegistry,
    ) -> Option<ProgressReading> {
        let role = known.map_or_else(|| self.role(), |(_, role)| role);
        let state = self.acc.state();
        let invisible = state
            .is_some_and(|state| state.cast_unsigned() & crate::map::STATE_SYSTEM_INVISIBLE != 0);
        let (role, states, _) = role_and_states(&self.acc, self.key, role, state);
        if role != Role::ProgressBar || invisible {
            return None;
        }
        let mut value = visible_text(self.acc.value());
        if let Some(value) = &mut value {
            verbatim_text::strip_bidi_controls(value);
        }
        let rect = self.acc.location();
        let id = known.map_or_else(
            || node_for(registry, self.key, true, &self.acc, role),
            |(node, _)| node,
        );
        Some(ProgressReading {
            id,
            value,
            offscreen: states.contains(State::Offscreen),
            rect,
        })
    }

    /// Reads the object for `purpose`, as [`snapshot_from_event`] does.
    #[must_use]
    pub fn read(self, registry: &NodeIdRegistry, purpose: Purpose) -> NodeSnapshot {
        read_snapshot_with(
            &self.acc,
            self.key,
            Reading {
                at_address: true,
                purpose,
                role: self.role,
                state: Prefetched::Unread,
                without_location: false,
            },
            registry,
        )
    }
}

/// Acquires the object named by an `EVENT_OBJECT_FOCUS` `WinEvent`, applying
/// NVDA's child-0-on-a-list redirect before mapping to a [`NodeSnapshot`].
/// Returns `None` if the object cannot be acquired. Blocking; worker only.
/// The same as [`focus_candidate`] read at once, without NVDA's checks
/// before the read.
#[must_use]
pub fn snapshot_from_focus_event(
    hwnd: isize,
    id_object: i32,
    id_child: i32,
    registry: &NodeIdRegistry,
) -> Option<NodeSnapshot> {
    Some(focus_candidate(hwnd, id_object, id_child)?.read(registry))
}

/// The object an `EVENT_OBJECT_FOCUS` names, acquired and redirected but
/// not yet read, so the outpost can make NVDA's checks on it first: whether
/// it is the focus already, and whether it or an ancestor has the focused
/// state. The role and state words read for the checks are kept and not
/// read again.
pub struct FocusCandidate {
    acc: Accessible,
    key: MsaaKey,
    role: Prefetched,
    state: Prefetched,
}

/// Acquires the object named by an `EVENT_OBJECT_FOCUS` `WinEvent` and
/// applies NVDA's child-0-on-a-list redirect, reading no more than the
/// redirect needs: the role of an object named by child id 0, and
/// `accFocus` of a list. `None` if the object cannot be acquired.
/// Blocking; worker only.
///
/// NVDA's `processFocusWinEvent`
/// (`nvda/source/IAccessibleHandler/__init__.py`): some controls fire
/// `EVENT_OBJECT_FOCUS` on child id 0 even when a child holds the focus — the
/// wxWidgets generic list is one, firing on the list object itself while a list
/// item is focused. When the event names a list on its own object (child id 0
/// and MSAA role `ROLE_SYSTEM_LIST`) or the client of a `SysListView32` window,
/// NVDA reads `accFocus` and redirects the focus object to the named child if
/// it is a real, different child. This mirrors that condition, so a focus
/// event on such a container announces the focused item, not the container,
/// and the navigator lands on the item. Beyond it, any focus on an object's
/// own child id 0 is redirected to the child `accFocus` names when that
/// child has the focused state ([`redirect_focus_to_child`]).
#[must_use]
pub fn focus_candidate(hwnd: isize, id_object: i32, id_child: i32) -> Option<FocusCandidate> {
    let acc = Accessible::from_event(hwnd, id_object, id_child)?;
    let role = if id_child == CHILDID_SELF {
        Prefetched::Read(acc.role())
    } else {
        Prefetched::Unread
    };
    let read_role = match role {
        Prefetched::Read(role) => role,
        Prefetched::Unread => None,
    };
    if let Some((focus_acc, key, state)) =
        redirect_focus_to_child(&acc, (hwnd, id_object, id_child), read_role)
    {
        // A redirect for a focused child read its state word already.
        return Some(FocusCandidate {
            acc: focus_acc,
            key,
            role: Prefetched::Unread,
            state,
        });
    }
    Some(FocusCandidate {
        acc,
        key: (hwnd, id_object, id_child),
        role,
        state: Prefetched::Unread,
    })
}

impl FocusCandidate {
    /// The address the focus is at, after the redirect.
    #[must_use]
    pub fn key(&self) -> MsaaKey {
        self.key
    }

    /// The child id of the object acquired, as `AccessibleObjectFromEvent`
    /// returned it: greater than 0 for a simple child of its parent, 0 for
    /// a full object, even one an event named by a negative unique id.
    #[must_use]
    pub fn resolved_child(&self) -> i32 {
        self.acc.child()
    }

    /// Whether the object or one of its ancestors has the focused state,
    /// as NVDA's `shouldAllowIAccessibleFocusEvent` asks, which weeds out
    /// stale and spurious focus events: the object's own state first, then
    /// each ancestor's, read live through raw `accParent`, up to `max_hops`
    /// of them. NVDA walks its own parents instead, with no limit, which
    /// repair a server that skips the window object and can cross into a
    /// UIA parent.
    /// A simple child's first ancestor is the object it is a child of. The
    /// object's own state word is kept for the read that follows.
    ///
    /// Where an `accParent` fails or names nothing, the test goes on from
    /// the parent window of the object's window as NVDA's parent does
    /// (`Window._get_parent`), by that window's own object, as
    /// `parent_window` says the outpost chooses it ([`ParentWindow`]): its
    /// MSAA client, whose state is read and from which the test goes on by
    /// `accParent`; its UIA element's focused state, read by the caller; or,
    /// for a plain window, which is never focused (NVDA's `Window` states
    /// hold only invisible and unavailable), nothing, the test going on from
    /// the window's parent window, or ending, above the process boundary,
    /// where every window to the desktop is plain. `plain` says which
    /// windows are read as plain windows, with no call: another process's,
    /// a ghost or a hung one. The test follows which window it is in by
    /// each object's role: a window's own window object, whose parent is
    /// its parent window's client, is the one step that leaves a window. A
    /// parent window that is plain is not stepped into, and the test goes
    /// on from it by the window hierarchy; since an `accParent` names
    /// whatever it names when it is called, the parent window is checked
    /// again after the step, and one reparented under a plain window
    /// meanwhile has its parent not read
    /// (`docs/design/cross-process-calls-study.md`, instance 6).
    pub fn has_focused_state(
        &mut self,
        max_hops: u32,
        plain: &dyn Fn(isize) -> bool,
        parent_window: &mut dyn FnMut(isize) -> ParentWindow,
    ) -> bool {
        let focused = |state: Option<i32>| {
            state.is_some_and(|state| {
                states_from_msaa(state.cast_unsigned()).contains(verbatim_model::State::Focused)
            })
        };
        let state = self.state.or_read(|| self.acc.state());
        self.state = Prefetched::Read(state);
        if focused(state) {
            return true;
        }
        let desktop = window::desktop();
        let mut current_hwnd = self.key.0;
        // The object's own state is read; a simple child's next is that of
        // the object it is a child of, a full object's that of its parent.
        let (mut current, mut read_state) = if self.acc.child() == CHILDID_SELF {
            (self.acc.clone(), false)
        } else {
            (self.acc.with_child(CHILDID_SELF), true)
        };
        let mut hops = 0u32;
        while hops <= max_hops {
            hops += 1;
            if read_state && focused(current.state()) {
                return true;
            }
            read_state = true;
            // A window's own window object's parent is its parent window's
            // client, the one step that leaves a window; every other step
            // stays in it.
            let leaves = current.role() == Some(ROLE_SYSTEM_WINDOW.cast_signed());
            let above = window::parent(current_hwnd);
            // The window the test goes on from by the window hierarchy,
            // when the object's parent is not read or not found.
            let onward = if leaves && (above == 0 || above == desktop) {
                return false;
            } else if leaves && plain(above) {
                above
            } else if let Ok(Some(parent)) = current.parent() {
                if leaves {
                    // The window may have been reparented since the check:
                    // the parent is then in the window it has now.
                    let now = window::parent(current_hwnd);
                    if now != above && now != 0 && now != desktop && plain(now) {
                        now
                    } else {
                        current_hwnd = now;
                        current = parent;
                        continue;
                    }
                } else {
                    current = parent;
                    continue;
                }
            } else {
                above
            };
            match onward_from_window(onward, (&mut hops, max_hops), parent_window) {
                Onward::Answered(answer) => return answer,
                Onward::Client(client, hwnd) => {
                    current = client;
                    current_hwnd = hwnd;
                }
            }
        }
        false
    }

    /// Reads the object, as a focus is read to be announced.
    #[must_use]
    pub fn read(self, registry: &NodeIdRegistry) -> NodeSnapshot {
        read_snapshot_with(
            &self.acc,
            self.key,
            Reading {
                at_address: true,
                purpose: Purpose::Announce,
                role: self.role,
                state: self.state,
                without_location: false,
            },
            registry,
        )
    }
}

/// How the focused-state test reads a parent window it goes on to past a
/// failed `accParent` ([`FocusCandidate::has_focused_state`]), by the
/// object NVDA's `Window._get_parent` chooses for it, as the outpost's
/// verdict and its readability judgement say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParentWindow {
    /// Above the process boundary: it and every window above it to the
    /// desktop are plain windows, none focused, so the test ends.
    AboveBoundary,
    /// A plain window, a ghost's or a hung application's: never focused,
    /// the test going on from its parent window.
    Plain,
    /// Read through MSAA: its client area, as NVDA's parent object for a
    /// window, whose state is read and from which the test goes on.
    Msaa,
    /// Read through UIA by the caller: whether the window's own element has
    /// the focused state. From an element without it the test goes on from
    /// the window's parent window, as NVDA's UIA parent of a window's
    /// element is its parent window's element, read through that window's
    /// own API again (`correctAPIForRelation`). `None` when the element
    /// cannot be read: the window's MSAA client is read instead, as NVDA
    /// falls back to its next API class.
    Uia(Option<bool>),
}

/// Where the focused-state test goes on after a window
/// ([`onward_from_window`]).
enum Onward {
    /// The test's answer: a window's UIA element has the focused state, or
    /// the desktop, the process boundary, or the hop limit was reached
    /// with nothing focused.
    Answered(bool),
    /// From this client object of this window, whose state is read next.
    Client(Accessible, isize),
}

/// The focused-state test from window `hwnd` up by the window hierarchy:
/// each window read by its own object, as `parent_window` chooses it
/// ([`ParentWindow`]), a plain window never focused, until a window's MSAA
/// client is acquired, from which the test goes on by `accParent`, or an
/// answer. Each window counts as a hop of `hops`, up to `max_hops`.
fn onward_from_window(
    mut hwnd: isize,
    (hops, max_hops): (&mut u32, u32),
    parent_window: &mut dyn FnMut(isize) -> ParentWindow,
) -> Onward {
    let desktop = window::desktop();
    while *hops <= max_hops && hwnd != 0 && hwnd != desktop {
        *hops += 1;
        match parent_window(hwnd) {
            ParentWindow::AboveBoundary => return Onward::Answered(false),
            ParentWindow::Uia(Some(true)) => return Onward::Answered(true),
            ParentWindow::Plain | ParentWindow::Uia(Some(false)) => {}
            ParentWindow::Msaa | ParentWindow::Uia(None) => {
                if let Some(client) = Accessible::from_event(hwnd, OBJID_CLIENT.0, CHILDID_SELF) {
                    return Onward::Client(client, hwnd);
                }
            }
        }
        hwnd = window::parent(hwnd);
    }
    Onward::Answered(false)
}

/// NVDA's `processFocusWinEvent` redirect: when a focus event names a list
/// container on child id 0 (MSAA role `ROLE_SYSTEM_LIST`, from `role`, read
/// for a child id 0) or the client of a `SysListView32` window, and
/// `accFocus` names a real, different child *by id*, return that child's
/// accessible and [`MsaaKey`], and the child's state word when it was read.
/// `None` when the condition does not hold or
/// `accFocus` names no distinct child, so the caller keeps the event's own
/// object.
///
/// Any other focus on an object's own child id 0 is redirected the same way
/// when the child `accFocus` names has the focused state. A control taking
/// the focus, such as a Win32 tree view or tab control, raises focus on
/// itself and then on its focused child within the one `SetFocus` call;
/// NVDA handles both together and tries the newest first, so it announces
/// the child, with the control as its new ancestor. The outpost can read
/// the control before the child's event reaches it, so it asks the control
/// directly and reaches NVDA's result by another route (`docs/parity/windows-and-processes.md`,
/// "A control's own focus with a focused child").
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
    (hwnd, id_object, id_child): MsaaKey,
    role: Option<i32>,
) -> Option<(Accessible, MsaaKey, Prefetched)> {
    let list = focus_event_names_list((hwnd, id_object, id_child), role);
    if !list && id_child != CHILDID_SELF {
        return None;
    }
    match read_acc_focus(acc) {
        FocusTarget::ChildId(real_child) if !list && real_child > 0 => {
            let child = acc.with_child(real_child);
            let state = child.state();
            state
                .is_some_and(|state| {
                    states_from_msaa(state.cast_unsigned()).contains(State::Focused)
                })
                .then_some((
                    child,
                    (hwnd, id_object, real_child),
                    Prefetched::Read(state),
                ))
        }
        // A child by id: redirect only when it is a real child (greater than
        // zero) and not the one the event already named — NVDA's exact guard.
        FocusTarget::ChildId(real_child) if real_child > 0 && real_child != id_child => Some((
            acc.with_child(real_child),
            (hwnd, id_object, real_child),
            Prefetched::Unread,
        )),
        // A `VT_DISPATCH` child object, `None`, or a self/zero child id: keep
        // the container (see this function's doc for why the dispatch form is
        // deliberately not redirected).
        _ => None,
    }
}

/// Whether an `EVENT_OBJECT_FOCUS` address matches NVDA's redirect condition:
/// a list on its own object (child id 0, MSAA role `ROLE_SYSTEM_LIST`, from
/// `role`), or the client of a `SysListView32` window.
fn focus_event_names_list((hwnd, id_object, id_child): MsaaKey, role: Option<i32>) -> bool {
    if id_child == CHILDID_SELF
        && role.map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned())) == Role::List
    {
        return true;
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
        plain: &|_| false,
        out_of_time: None,
    };
    ancestor_chain_until(node, registry, &limits).map(|(chain, _)| chain)
}

/// What bounds an ancestor walk besides reaching the root.
pub struct AncestorLimits<'a> {
    /// The most ancestors to read: the walk is then cut
    /// ([`Walked::Cut`]).
    pub max_hops: u32,
    /// Whether an ancestor is already known, from the previous focus's
    /// chain: the walk stops there, as NVDA's focus ancestry stops where it
    /// meets the previous focus's ancestors and reuses them.
    pub known: &'a dyn Fn(NodeId) -> bool,
    /// Whether a window is read through the other API: the walk stops
    /// before a parent in a different window for which this holds, as NVDA
    /// switches API where a parent lies in such a window
    /// ([`Walked::Onward`]).
    pub read_by_other_api: &'a dyn Fn(isize) -> bool,
    /// Whether a window is read as a plain window, with no call: another
    /// process's, a ghost or a hung one. The walk does not step from a
    /// window's own window object into such a parent window, and stops
    /// before a parent an `accParent` named in such a window, unread
    /// ([`Walked::Onward`]).
    pub plain: &'a dyn Fn(isize) -> bool,
    /// Whether the walk's time is up, checked between its calls: it is
    /// then cut ([`Walked::Cut`]).
    pub out_of_time: Option<&'a dyn Fn() -> bool>,
}

/// How an ancestor walk ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walked {
    /// It reached the root.
    Complete,
    /// It reached this known ancestor, the outermost one in the chain.
    MetKnown(NodeId),
    /// It ran out of time or reached its hop limit with ancestors left: the
    /// chain is completed by plain windows from this window, the outermost
    /// ancestor's own, included (`docs/design/focus-pipeline-rework.md`,
    /// "A window's name, and the one fallback").
    Cut(isize),
    /// The chain goes on from this window's own object, chosen by the
    /// caller as NVDA chooses it (`Window._get_parent`), the chain ending
    /// below it: the next parent is in a window read through the other API
    /// or as a plain window, or a hop's `accParent` failed or named
    /// nothing, when the window is the parent window of the outermost
    /// ancestor's own, as NVDA's parent falls back to.
    Onward(isize),
}

/// [`ancestor_chain`] within `limits`: the chain, outermost first, and how
/// the walk ended. A walk that meets a known ancestor ends with it as the
/// chain's outermost entry.
///
/// # Errors
///
/// [`AcquireError::Gone`] when `node` is no longer reachable.
#[expect(
    clippy::too_many_lines,
    reason = "the tree view, simple-child, parent, and group box hops read best side by side"
)]
pub fn ancestor_chain_until(
    node: NodeId,
    registry: &NodeIdRegistry,
    limits: &AncestorLimits<'_>,
) -> Result<(Vec<NodeSnapshot>, Walked), AcquireError> {
    let max_hops = limits.max_hops;
    // An ancestor is read for what it is, not spoken in full.
    let context = |at_address| Reading {
        at_address,
        purpose: Purpose::Context,
        ..Reading::default()
    };
    let out_of_time = || limits.out_of_time.is_some_and(|out_of_time| out_of_time());
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
            let snapshot = read_snapshot_with(
                &acc.with_child(parent_acc_id),
                parent_key,
                context(at),
                registry,
            );
            let id = snapshot.id;
            chain.push(snapshot);
            hops_used += 1;
            current_acc_id = parent_acc_id;
            if (limits.known)(id) {
                walked = Walked::MetKnown(id);
                break;
            }
            if out_of_time() {
                walked = Walked::Cut(hwnd);
                break;
            }
        }
    }

    let desktop = window::desktop();
    let mut current = acc.with_child(CHILDID_SELF);
    let mut current_hwnd = hwnd;
    let mut at_self = !is_simple_child;
    // Whether `current` is a window's own window object, whose parent is
    // its parent window's client.
    let mut at_window_object = false;
    while walked == Walked::Complete {
        if out_of_time() {
            walked = Walked::Cut(current_hwnd);
            break;
        }
        // The hop limit cuts the chain only where an ancestor is left: the
        // object a simple child belongs to, or a parent found.
        if !at_self {
            if hops_used >= max_hops {
                walked = Walked::Cut(current_hwnd);
                break;
            }
            let self_hwnd = current.window().unwrap_or(hwnd);
            let self_key = (self_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            // The object a simple child belongs to sits at the client address
            // of the child's own window, when the child was acquired there.
            let self_at = at && id_object == OBJID_CLIENT.0 && self_hwnd == hwnd;
            let snapshot = read_snapshot_with(&current, self_key, context(self_at), registry);
            let id = snapshot.id;
            at_window_object = snapshot.role == Role::Window;
            chain.push(snapshot);
            current_hwnd = self_hwnd;
            at_self = true;
            hops_used += 1;
            if (limits.known)(id) {
                walked = Walked::MetKnown(id);
            }
            continue;
        }
        // A window's own window object's parent is its parent window's
        // client: a parent window read as a plain window is not stepped
        // into, as another process's is never called. Local calls.
        let above = window::parent(current_hwnd);
        if at_window_object && above != 0 && above != desktop && (limits.plain)(above) {
            walked = Walked::Onward(above);
            break;
        }
        let Ok(Some(parent_acc)) = current.parent() else {
            // A failed or empty `accParent` below the desktop: the chain
            // goes on from the parent window's object, as NVDA's parent
            // falls back to the window hierarchy (`Window._get_parent`,
            // `NVDAObjects/window/__init__.py` 287 to 293). The desktop
            // ends it.
            if above != 0 && above != desktop {
                walked = Walked::Onward(above);
            }
            break;
        };
        let parent_window = parent_acc.window();
        // A parent that names no window is in the window the walk is in,
        // or, above a window's own window object, in its parent window,
        // where it is now: the window may have been reparented since the
        // check above.
        let parent_hwnd = parent_window.unwrap_or_else(|| {
            if at_window_object {
                window::parent(current_hwnd)
            } else {
                current_hwnd
            }
        });
        // The desktop is the root, never an ancestor: NVDA's focus ancestors
        // stop below it, so it is never announced as an entered container.
        if parent_hwnd == desktop {
            break;
        }
        // The `accParent` named another window's object: one read through
        // the other API, or as a plain window, which is not read further,
        // since a window can be reparented after the check above.
        if parent_hwnd != current_hwnd
            && ((limits.read_by_other_api)(parent_hwnd) || (limits.plain)(parent_hwnd))
        {
            walked = Walked::Onward(parent_hwnd);
            break;
        }
        if hops_used >= max_hops {
            walked = Walked::Cut(current_hwnd);
            break;
        }
        // A parent that is one of a window's own standard objects names its
        // address in its identity string, so it is found again at that
        // address however it was reached: one window keeps one node, even
        // after its title changes, as NVDA, re-reading the old object live,
        // finds it equal. Any other parent gets an address made up from its
        // window, which only finds it again as the same COM object. A parent
        // `WindowFromAccessibleObject` places in no window is no window's
        // own object, so its identity string is not asked for.
        let address = parent_window.and_then(|_| parent_acc.address());
        let (parent_key, parent_at) = match address {
            Some(address) if address.0 == parent_hwnd => (address, true),
            _ => ((parent_hwnd, OBJID_CLIENT.0, CHILDID_SELF), false),
        };
        let snapshot = read_snapshot_with(&parent_acc, parent_key, context(parent_at), registry);
        let id = snapshot.id;
        let is_window_object = snapshot.role == Role::Window;
        chain.push(snapshot);
        hops_used += 1;
        current = parent_acc;
        current_hwnd = parent_hwnd;
        at_window_object = is_window_object;
        if (limits.known)(id) {
            walked = Walked::MetKnown(id);
            continue;
        }
        // A control's window object has a group box as its container when
        // one encloses it, as NVDA's window root finds it: the group box
        // is the next ancestor, and the walk goes on from it.
        if is_window_object
            && hops_used < max_hops
            && let Some((group, group_hwnd)) = group_box_of(parent_hwnd)
        {
            let group_key = (group_hwnd, OBJID_CLIENT.0, CHILDID_SELF);
            let snapshot = read_snapshot_with(&group, group_key, context(true), registry);
            let id = snapshot.id;
            chain.push(snapshot);
            hops_used += 1;
            current = group;
            current_hwnd = group_hwnd;
            at_window_object = false;
            if (limits.known)(id) {
                walked = Walked::MetKnown(id);
            }
        }
    }
    chain.reverse();
    Ok((chain, walked))
}

/// The group box enclosing the window `hwnd`, and its window, as NVDA's
/// `findGroupboxObject` finds it: among the windows before `hwnd` in
/// z-order, the first visible standard button with the group box style
/// (`BS_GROUPBOX`, compared by the raw class name, as NVDA compares it)
/// whose client object is a grouping and whose rectangle holds `hwnd`'s
/// whole rectangle. A Win32 group box is the sibling of the controls inside
/// it, not their parent, so the walk up from them never meets it
/// otherwise. Only local window calls, but for the candidate's role.
fn group_box_of(hwnd: isize) -> Option<(Accessible, isize)> {
    const BS_GROUPBOX: isize = 0x7;
    const BS_TYPEMASK: isize = 0xF;
    // The group box's own visible style: the focus's dialog is shown, so
    // this is NVDA's visibility check, which also asks of every parent.
    const WS_VISIBLE_STYLE: isize = 0x1000_0000;
    let inner = window::rect(hwnd)?;
    let mut previous = window::related(hwnd, GW_HWNDPREV);
    while previous != 0 {
        if window::class_name(previous) == "Button"
            && window::style(previous) & BS_TYPEMASK == BS_GROUPBOX
            && window::style(previous) & WS_VISIBLE_STYLE != 0
        {
            let group = Accessible::from_event(previous, OBJID_CLIENT.0, CHILDID_SELF)?;
            let outer = window::rect(previous)?;
            let encloses = inner.left >= outer.left
                && inner.right <= outer.right
                && inner.top >= outer.top
                && inner.bottom <= outer.bottom;
            if encloses && group.role() == Some(ROLE_SYSTEM_GROUPING.cast_signed()) {
                return Some((group, previous));
            }
        }
        let next = window::related(previous, GW_HWNDPREV);
        // A window can be its own previous window (NVDA's guard).
        if next == previous {
            break;
        }
        previous = next;
    }
    None
}

/// Returns whether `hwnd` is a `SysTreeView32` common control (comctl32's
/// tree view), by its normalized class name, so a Windows Forms tree view
/// counts, as NVDA's class map matches it — see this module's top doc
/// comment for why its items need the `TVM_*`-based navigation below
/// instead of `accNavigate`/`accParent`. Reading the class name is a local
/// call, safe even against a hung window.
fn is_systreeview32(hwnd: isize) -> bool {
    normalized_class_of(hwnd) == "SysTreeView32"
}

/// Maps an MSAA child id to its `HTREEITEM`, via `TVM_MAPACCIDTOHTREEITEM`,
/// 0 when it maps to none. When the message returns 0 from a control older
/// than comctl32 version 6, which has no such mapping and uses the item
/// handle as the child id outright, the child id itself is the item, as
/// NVDA's `sysTreeView32.py` (`treeview_hItem`) takes it. A version 6
/// control answering 0 has no such item (it may just have been deleted),
/// and its answer is kept: the control would treat the child id as a
/// pointer into its own memory.
fn htreeitem_for_acc_id(hwnd: isize, acc_id: i32) -> isize {
    let wparam = usize::try_from(acc_id.cast_unsigned()).unwrap_or(0);
    let mapped = window::tree_view_item_for_acc_id(hwnd, wparam);
    if mapped == 0 && !window::is_common_control_6(hwnd) {
        isize::try_from(acc_id).unwrap_or(0)
    } else {
        mapped
    }
}

/// Maps an `HTREEITEM` back to its MSAA child id, via
/// `TVM_MAPHTREEITEMTOACCID`, `None` when it maps to none. A control older
/// than comctl32 version 6 answers 0 and uses the item handle as the child
/// id, the fallback [`htreeitem_for_acc_id`] documents.
fn acc_id_for_htreeitem(hwnd: isize, hitem: isize) -> Option<i32> {
    let wparam = usize::try_from(hitem).unwrap_or(0);
    let mapped = window::tree_view_acc_id_for_item(hwnd, wparam);
    if mapped == 0 && !window::is_common_control_6(hwnd) {
        i32::try_from(hitem).ok()
    } else {
        i32::try_from(mapped).ok().filter(|&id| id != 0)
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
    acc_id_for_htreeitem(hwnd, neighbor_hitem)
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

/// Reads the focused object in `hwnd`, the window with the keyboard focus,
/// for a focus-now query (including Core's after a menu closes): its
/// client's `accFocus`, with a fallback to the client itself. `accFocus`
/// names whatever it names when it is called, so the window of the object
/// it names is checked with `serves` before anything else is read of it:
/// an object of a window the caller's application does not serve answers
/// `None`, no focused control, and is read no further
/// (`docs/design/cross-process-calls-study.md`, instance 8). Blocking;
/// worker only.
#[must_use]
pub fn focused_snapshot(
    hwnd: isize,
    registry: &NodeIdRegistry,
    serves: &dyn Fn(isize) -> bool,
) -> Option<NodeSnapshot> {
    let client = Accessible::client_of_window(hwnd)?;
    // A child object reached through `accFocus` has no address of its
    // own; the client and its children by id do.
    let (acc, at_address) = match read_acc_focus(&client) {
        FocusTarget::ChildObject(child_acc) => (child_acc, false),
        FocusTarget::ChildId(child_id) => (client.with_child(child_id), true),
        FocusTarget::None => (client, true),
    };
    // A child object `accFocus` named is in whatever window it is in, and
    // one placed in no window is not known to be served by the
    // application, so it is not read; the client and its children by id
    // are in `hwnd`.
    let node_hwnd = match acc.window() {
        Some(window) => window,
        None if at_address => hwnd,
        None => {
            tracing::debug!(
                hwnd,
                "accFocus named an object in no window; nothing more is read"
            );
            return None;
        }
    };
    if node_hwnd != hwnd && !serves(node_hwnd) {
        tracing::debug!(
            hwnd,
            named = node_hwnd,
            "accFocus named an object of a window the application does not serve; nothing more is read"
        );
        return None;
    }
    let key = (node_hwnd, OBJID_CLIENT.0, acc.child());
    Some(read_snapshot(&acc, key, at_address, registry))
}

/// Walks the MSAA tree from `hwnd`'s client accessible object, bounded by
/// `max_depth` (the root is depth 0) and `max_nodes` (the total node count
/// across the whole walk, including the root). Unlike UIA there are no
/// cache requests on this backend, so every step is its own cross-process
/// COM round trip (architecture section 4's IA2 cost model); blocking,
/// worker only, guarded by the caller's deadline. Returns the walked tree
/// and whether it is incomplete: either cap was hit before the walk reached
/// every node, or an object's children could not be read because the
/// object was gone ([`object_gone`]), which ends that object's children
/// there and lets the rest of the walk go on, as when a menu closes during
/// the walk.
///
/// Each object's location (`accLocation`) and whether it is off the screen
/// are read only with `locations`, as a read of many objects never reads a
/// location; without it every node has no rectangle and no `Offscreen`
/// state, though the state word holds the bit, so a dump shows both as not
/// read, never as a value.
///
/// # Errors
///
/// [`AcquireError::Failed`] when the window has no accessible client
/// object, or when reading an object's children (`accChildCount` or
/// `AccessibleChildren`) fails for any reason other than the object being
/// gone, so an object whose children could not be read is never reported
/// as having none.
pub fn walk_tree(
    hwnd: isize,
    registry: &NodeIdRegistry,
    (max_depth, max_nodes): (u32, usize),
    locations: bool,
) -> Result<(TreeNode, bool), AcquireError> {
    let acc = Accessible::client_of_window(hwnd).ok_or_else(|| {
        AcquireError::Failed("could not acquire the top-level MSAA object".to_owned())
    })?;
    let limits = WalkLimits {
        registry,
        max_depth,
        max_nodes,
        locations,
    };
    let mut state = WalkState {
        visited: 1, // the root counts as one node.
        truncated: false,
    };
    let key = (hwnd, OBJID_CLIENT.0, CHILDID_SELF);
    let root = walk_recursive(&acc, hwnd, (key, true), &limits, 0, &mut state)
        .map_err(|error| AcquireError::Failed(format!("MSAA tree walk failed: {error}")))?;
    Ok((root, state.truncated))
}

/// The per-walk parameters threaded through every level of
/// [`walk_recursive`]: node-identity plumbing plus the walk's caps.
struct WalkLimits<'a> {
    registry: &'a NodeIdRegistry,
    max_depth: u32,
    max_nodes: usize,
    /// Whether each object's location and off-screen state are read.
    locations: bool,
}

/// Mutable state accumulated across the whole walk, shared by every level
/// of recursion: the nodes accepted so far, and whether the walk is
/// incomplete, against a cap or at a gone object ([`walk_tree`]).
struct WalkState {
    visited: usize,
    truncated: bool,
}

/// Whether a failed call says the object it was made on is gone: its
/// server disconnected it (`CoDisconnectObject`), so COM answers
/// `RPC_E_DISCONNECTED`, or its stub no longer exists
/// (`CO_E_OBJNOTCONNECTED`). These are COM's object-disconnected errors,
/// chosen by Verbatim as the ones that say the object is gone. A process
/// that has died answers other errors, and `E_FAIL`, which a server may
/// answer for any failure, says nothing about the object.
fn object_gone(error: &windows::core::Error) -> bool {
    [RPC_E_DISCONNECTED, CO_E_OBJNOTCONNECTED].contains(&error.code())
}

/// A read of an object's children for the walk, with a failure because the
/// object is gone ([`object_gone`]) read as no children (`Ok(None)`) and
/// marking the walk incomplete, since its children are unknown; any other
/// failure stays an error.
fn walk_read<T>(
    read: windows::core::Result<T>,
    state: &mut WalkState,
) -> windows::core::Result<Option<T>> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(error) if object_gone(&error) => {
            state.truncated = true;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Recursive worker for [`walk_tree`]. `state.visited` already counts the
/// node named by `acc`. A simple element (addressed purely by a child id,
/// not its own `IAccessible`) is always a leaf per the MSAA model, so only
/// `CHILDID_SELF` nodes are ever expanded. A failed read of an object's
/// children fails the walk with its error, unless the object is gone
/// ([`walk_read`]).
fn walk_recursive(
    acc: &Accessible,
    hwnd: isize,
    (key, at_address): (MsaaKey, bool),
    limits: &WalkLimits<'_>,
    depth: u32,
    state: &mut WalkState,
) -> windows::core::Result<TreeNode> {
    let mut snapshot = read_snapshot_with(
        acc,
        key,
        Reading {
            at_address,
            without_location: !limits.locations,
            ..Reading::default()
        },
        limits.registry,
    );
    if !limits.locations {
        snapshot.states.remove(State::Offscreen);
    }
    let leaf = |snapshot| {
        Ok(TreeNode {
            snapshot,
            children: Vec::new(),
        })
    };

    if acc.child() != CHILDID_SELF {
        return leaf(snapshot);
    }

    if depth >= limits.max_depth {
        // Only peeks whether there is a child we are declining to descend
        // into.
        if walk_read(acc.child_count(), state)?.is_some_and(|children| children > 0) {
            state.truncated = true;
        }
        return leaf(snapshot);
    }

    let child_count = match walk_read(acc.child_count(), state)? {
        Some(children) if children > 0 => children,
        _ => return leaf(snapshot),
    };
    let Ok(child_count) = usize::try_from(child_count) else {
        return leaf(snapshot);
    };

    // The application's count, bounded by the walk's remaining budget:
    // children past it would only be truncated below.
    let budget = limits.max_nodes.saturating_sub(state.visited);
    if child_count > budget {
        state.truncated = true;
    }
    let Some(entries) = walk_read(acc.children(child_count.min(budget)), state)? else {
        return leaf(snapshot);
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
        )?;
        children.push(child_node);
    }

    Ok(TreeNode { snapshot, children })
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
    acc.children(1).ok()?.into_iter().next()
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
/// MSAA offers (`accDescription`, `accKeyboardShortcut`, and, for an object
/// read alone to be announced, `accLocation`) from an accessible and its
/// child id. Position-in-set is computed for list-view
/// and tree-view items (`position_of`) and is otherwise `None` until IA2's
/// group position (roadmap M7).
///
/// Level is the one exception: a `SysTreeView32` item ([`Role::TreeItem`])
/// overloads `accValue` to report its 0-based indent depth as a numeric
/// string rather than a real value — NVDA's `sysTreeView32.py` overrides
/// `TreeViewItem.value` to `None` for exactly this reason. This reads that
/// same string into [`NodeDetails::level`] instead, as it is: NVDA's tree
/// view level is the raw `accValue`, so a root item is "level 0"
/// (confirmed by an NVDA transcript in msinfo32 on 2026-10-07; Verbatim had
/// added one). It leaves `value` itself `None` for such an item, matching
/// NVDA. A tree item outside a `SysTreeView32` keeps its value unless it is
/// a number, and has no level, as NVDA's generic outline item reads it.
fn read_snapshot(
    acc: &Accessible,
    key: MsaaKey,
    at_address: bool,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    read_snapshot_with(
        acc,
        key,
        Reading {
            at_address,
            ..Reading::default()
        },
        registry,
    )
}

/// [`read_snapshot`] as `reading` says: for its purpose, and without
/// reading again the role and state words already read.
fn read_snapshot_with(
    acc: &Accessible,
    key: MsaaKey,
    reading: Reading,
    registry: &NodeIdRegistry,
) -> NodeSnapshot {
    let at_address = reading.at_address;
    // Each read tolerates an unsupported property by failing, mapped to a
    // neutral default.
    let name = visible_text(acc.name());
    let raw_value = visible_text(acc.value());
    let raw_role = reading.role.or_read(|| acc.role());
    let top_level_client = is_top_level_client(key, raw_role);
    let role = raw_role.map_or(Role::Unknown, |r| role_from_msaa(r.cast_unsigned()));
    let (role, states, tree_item) =
        role_and_states(acc, key, role, reading.state.or_read(|| acc.state()));
    // A detail the active theme reports as off is not read at all, saving
    // its cross-process call (`NodeIdRegistry::fetches`).
    let fetches = registry.fetches();
    let description = fetches
        .description
        .then(|| non_empty(acc.description()))
        .flatten();
    let keyboard_shortcut = fetches
        .shortcut
        .then(|| non_empty(acc.keyboard_shortcut()))
        .flatten();
    let rect = (reading.purpose == Purpose::Announce && !reading.without_location)
        .then(|| acc.location())
        .flatten();
    // See this function's doc comment: a tree item's accValue is really
    // its 0-based level, not a value.
    // The edit field of a combo box takes the combo box's label, so it
    // has none of its own when the combo box is labelled, as in NVDA.
    let name = name.filter(|_| role != Role::EditableText || !in_labelled_combo_box(acc));
    let (position_in_set, set_size) = if fetches.position && reading.purpose == Purpose::Announce {
        position_of(key.0, acc.child(), role, tree_item)
    } else {
        (None, None)
    };
    let list_view_item = is_list_view_item(acc, key, role);
    let (value, level) = match role {
        Role::TreeItem if is_systreeview32(key.0) => {
            let level = raw_value
                .as_deref()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|_| fetches.level);
            (None, level)
        }
        _ => (value_for(acc, key, role, raw_value), None),
    };
    // A list view item has no description either, as NVDA's has none, and
    // one that shows columns is named by them ("content; Header:
    // content"), as NVDA names it.
    let (mut name, mut description) = if list_view_item {
        let columns = crate::list_view::column_name(key.0, acc.child(), || description.clone())
            .ok()
            .flatten();
        (columns.or(name), None)
    } else {
        (name, description)
    };
    // A list view's dates, as File Explorer's, carry left-to-right marks
    // between their numbers, which NVDA strips; so are they here, and every
    // other bidirectional formatting character with them.
    let mut value = value;
    for text in [&mut name, &mut value, &mut description]
        .into_iter()
        .flatten()
    {
        verbatim_text::strip_bidi_controls(text);
    }
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
    // A window's own window or client object, acquired at its address,
    // carries the window: an object at a made-up address may be any object
    // inside it.
    let window = (at_address
        && key.2 == CHILDID_SELF
        && (key.1 == OBJID_CLIENT.0 || key.1 == OBJID_WINDOW.0))
        .then(|| WindowHandle(u64::from_ne_bytes(key.0.to_ne_bytes())));
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
            top_level_client,
            class_name: None,
            row: None,
            column: None,
            parent_automation_id: None,
            window,
        },
    }
}

/// Whether the object `raw_role` names, at `key`, is the client area of a
/// window with a system menu, a top-level window's: NVDA presents it as a
/// focus ancestor although it reads as a pane, so a window read through
/// MSAA is spoken by its title as the focus enters it
/// (`_get_isPresentableFocusAncestor`, `NVDAObjects/IAccessible/__init__.py`
/// 2094 to 2106): a client object, child id 0, of a window with the
/// `WS_SYSMENU` style. One local read of the window's style, made only for
/// a client object.
fn is_top_level_client(key: MsaaKey, raw_role: Option<i32>) -> bool {
    const WS_SYSMENU_STYLE: isize = 0x0008_0000;
    raw_role == Some(ROLE_SYSTEM_CLIENT.cast_signed())
        && key.1 == OBJID_CLIENT.0
        && key.2 == CHILDID_SELF
        && window::style(key.0) & WS_SYSMENU_STYLE != 0
}

/// The role and states `acc`, addressed at `key`, is read with, from its
/// role and its state word: a progress bar in the mixed state is a busy
/// indicator; an edit control's client object says whether it edits more
/// than one line, as NVDA's edit control class does, from the window's
/// style; and a `SysTreeView32` item has its check state added from its
/// state image. With the item's own handle, for its position, when it is
/// such an item.
fn role_and_states(
    acc: &Accessible,
    key: MsaaKey,
    role: Role,
    state: Option<i32>,
) -> (Role, verbatim_model::StateSet, Option<isize>) {
    let states = state
        .map(|s| states_from_msaa(s.cast_unsigned()))
        .unwrap_or_default();
    let (role, mut states) = adjust_role_and_states(role, states);
    if role == Role::EditableText
        && key.1 == OBJID_CLIENT.0
        && key.2 == CHILDID_SELF
        && window::is_multiline_edit(key.0)
    {
        states.insert(State::Multiline);
    }
    let tree_item =
        (role == Role::TreeItem && acc.child() != CHILDID_SELF && is_systreeview32(key.0))
            .then(|| htreeitem_for_acc_id(key.0, acc.child()))
            .filter(|&item| item != 0);
    if let Some(item) = tree_item {
        add_tree_view_check_states(&mut states, key.0, item);
    }
    (role, states, tree_item)
}

/// Whether `acc`, of `role`, addressed at `key`, is an item of a comctl32
/// list view, which NVDA names by its columns and gives no value or
/// description.
fn is_list_view_item(acc: &Accessible, key: MsaaKey, role: Role) -> bool {
    role == Role::ListItem
        && acc.child() != CHILDID_SELF
        && normalized_class_of(key.0) == "SysListView32"
}

/// The value of `acc`, of `role`, addressed at `key`, from `raw_value`, its
/// `accValue`, as it is reported: a `SysTreeView32` item and a list view
/// item have none, the first's being its level and the second having none
/// in NVDA; any other tree item's is dropped only when it is a number,
/// which some providers report as the item's depth, as NVDA's outline item
/// reads it.
fn value_for(
    acc: &Accessible,
    key: MsaaKey,
    role: Role,
    raw_value: Option<String>,
) -> Option<String> {
    match role {
        Role::TreeItem if is_systreeview32(key.0) => None,
        Role::TreeItem => raw_value.filter(|value| value.trim().parse::<i64>().is_err()),
        Role::ListItem if is_list_view_item(acc, key, role) => None,
        _ => raw_value,
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

/// Adds a `SysTreeView32` item's check state, from its state image
/// (`TVM_GETITEMSTATE`), as NVDA's tree view item reads it: any state image
/// makes the item checkable, the second checked, and the third partly
/// checked. A tree view that draws its own check boxes (msconfig's, say)
/// says so only through its items' state images.
fn add_tree_view_check_states(states: &mut verbatim_model::StateSet, hwnd: isize, item: isize) {
    use verbatim_model::State;
    match window::tree_view_state_image(hwnd, item) {
        0 => {}
        image => {
            states.insert(State::Checkable);
            match image {
                2 => states.insert(State::Checked),
                3 => states.insert(State::Mixed),
                _ => {}
            }
        }
    }
}

/// More siblings than any real tree view holds, so a broken control cannot
/// keep a walk through a `SysTreeView32`'s items going.
const MAX_TREE_VIEW_SIBLINGS: u32 = 100_000;

/// The number of children of the `SysTreeView32` item `child_id` names in
/// `hwnd`, counted as NVDA counts them: its first child (`TVGN_CHILD`),
/// then each next sibling (`TVGN_NEXT`). `None` when `hwnd` is not a
/// `SysTreeView32`, `child_id` names the control itself, or the item no
/// longer resolves. Only window messages to the control, no COM call.
#[must_use]
pub fn tree_view_child_count(hwnd: isize, child_id: i32) -> Option<u32> {
    if child_id == CHILDID_SELF || hwnd == 0 || !is_systreeview32(hwnd) {
        return None;
    }
    let item = htreeitem_for_acc_id(hwnd, child_id);
    if item == 0 {
        return None;
    }
    let mut count = 0u32;
    let mut current = window::tree_view_next_item(hwnd, TVGN_CHILD, item);
    while current != 0 && count < MAX_TREE_VIEW_SIBLINGS {
        count += 1;
        current = window::tree_view_next_item(hwnd, TVGN_NEXT, current);
    }
    Some(count)
}

/// An item's position in its set and the set's size, for an item of a
/// comctl32 list view or tree view, which MSAA gives no way to ask for, as
/// NVDA computes them: a list view item is at its child id among
/// `LVM_GETITEMCOUNT` items; a tree view item, `tree_item` its handle, is
/// counted among its siblings through `TVM_GETNEXTITEM`. `(None, None)` for
/// anything else.
fn position_of(
    hwnd: isize,
    child_id: i32,
    role: Role,
    tree_item: Option<isize>,
) -> (Option<u32>, Option<u32>) {
    if child_id == CHILDID_SELF || hwnd == 0 {
        return (None, None);
    }
    match (role, tree_item) {
        (Role::ListItem, _) if normalized_class_of(hwnd) == "SysListView32" => {
            let items = window::list_view_item_count(hwnd);
            let items = u32::try_from(items).ok().filter(|&items| items > 0);
            (
                u32::try_from(child_id).ok().filter(|_| items.is_some()),
                items,
            )
        }
        (Role::TreeItem, Some(item)) => {
            let walk = |relation: u32| {
                let mut count = 0u32;
                let mut current = item;
                while current != 0 && count < MAX_TREE_VIEW_SIBLINGS {
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
/// nodes in NVDA's comparison order for MSAA objects (`docs/parity/focus-events.md`,
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
