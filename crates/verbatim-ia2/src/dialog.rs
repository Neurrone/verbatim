//! The objects a dialog's own text is gathered from, through MSAA
//! (`docs/nvda/object-model.md`, "A dialog's own text").
//!
//! The gathering itself is the outpost's; this module only reads. A
//! [`DialogObject`] reads each property the first time it is asked for and
//! keeps it, since the gathering looks at a child's neighbors as well as
//! the child, and reads nothing it is not asked for: most children are
//! passed over on their role and states alone. Every read is a blocking
//! cross-process call, worker only.
//!
//! A child that is a window object stands for the window's client area, as
//! NVDA reads it, so a dialog's controls, each a window of its own, are
//! seen with their own roles. A window that is hidden or disabled is known
//! to be so from local window reads, without acquiring its client area.

use std::cell::OnceCell;

use windows::Win32::UI::Accessibility::ROLE_SYSTEM_WINDOW;
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;

use verbatim_model::{NodeId, Role, State, StateSet};

use crate::accessible::{Accessible, Related};
use crate::com::{CHILDID_SELF, non_empty, visible_text};
use crate::map::{STATE_SYSTEM_INVISIBLE, role_from_msaa, states_from_msaa};
use crate::registry::NodeIdRegistry;
use crate::window;

/// The states of an object: the model's, and whether it is invisible, which
/// the model has no state for.
#[derive(Clone, Copy)]
struct Seen {
    states: StateSet,
    invisible: bool,
}

/// One object a dialog's text may be gathered from: the dialog itself or a
/// descendant, with what has been read of it so far.
pub struct DialogObject {
    /// The object and the child id it is read at.
    acc: Accessible,
    /// The window it is in.
    hwnd: isize,
    /// Whether it is that window's client area, whose window style says
    /// whether an edit control is multi-line.
    client: bool,
    role: OnceCell<Role>,
    seen: OnceCell<Seen>,
    name: OnceCell<Option<String>>,
}

impl DialogObject {
    /// An object read at `acc`, in the window `hwnd`.
    fn new(acc: Accessible, hwnd: isize, client: bool) -> Self {
        Self {
            acc,
            hwnd,
            client,
            role: OnceCell::new(),
            seen: OnceCell::new(),
            name: OnceCell::new(),
        }
    }

    /// The object this outpost reported as `node`, to gather its text from;
    /// `None` when the node is gone or was not issued here.
    #[must_use]
    pub fn of_node(node: NodeId, registry: &NodeIdRegistry) -> Option<Self> {
        let (acc, (hwnd, id_object, child), _) = crate::acquire::locate(node, registry).ok()?;
        let client = id_object == OBJID_CLIENT.0 && child == CHILDID_SELF;
        Some(Self::new(acc, hwnd, client))
    }

    /// The object's children, in order, each a window's client area where
    /// the child is a window object; none when they cannot be read. Two
    /// calls (`accChildCount` and `AccessibleChildren`), then for each
    /// child object its role, and for the window object of a visible,
    /// enabled window its window and client area too.
    #[must_use]
    pub fn children(&self) -> Vec<Self> {
        if self.acc.child() != CHILDID_SELF {
            return Vec::new();
        }
        let count = match self.acc.child_count() {
            Ok(count) if count > 0 => usize::try_from(count).unwrap_or(0),
            _ => return Vec::new(),
        };
        let Some(entries) = self.acc.children(count) else {
            return Vec::new();
        };
        entries
            .iter()
            .filter_map(|entry| match entry {
                Related::Child(child) => {
                    Some(Self::new(self.acc.with_child(*child), self.hwnd, false))
                }
                Related::Object(_) => entry.object().map(|acc| self.child_object(acc)),
                Related::Nothing | Related::Other => None,
            })
            .collect()
    }

    /// A child that is an object of its own: when it is the window object
    /// of another window, that window's client area.
    fn child_object(&self, acc: Accessible) -> Self {
        let raw_role = acc.role();
        if raw_role == Some(ROLE_SYSTEM_WINDOW.cast_signed())
            && let Some(hwnd) = acc.window()
            && hwnd != self.hwnd
        {
            let hidden = !window::is_visible(hwnd);
            let disabled = !window::is_enabled(hwnd);
            if hidden || disabled {
                // Passed over on these alone; nothing more is read.
                let child = Self::new(acc, hwnd, false);
                let mut states = StateSet::new();
                if disabled {
                    states.insert(State::Disabled);
                }
                let _ = child.seen.set(Seen {
                    states,
                    invisible: hidden,
                });
                let _ = child.role.set(Role::Window);
                return child;
            }
            if let Some(client) = Accessible::client_of_window(hwnd) {
                return Self::new(client, hwnd, true);
            }
        }
        let child = Self::new(acc, self.hwnd, false);
        let _ = child
            .role
            .set(raw_role.map_or(Role::Unknown, |role| role_from_msaa(role.cast_unsigned())));
        child
    }

    /// The object's role (`accRole`), unknown when it cannot be read.
    #[must_use]
    pub fn role(&self) -> Role {
        *self.role.get_or_init(|| {
            self.acc
                .role()
                .map_or(Role::Unknown, |role| role_from_msaa(role.cast_unsigned()))
        })
    }

    /// The object's states and visibility, read once (`accState`).
    fn seen(&self) -> Seen {
        *self.seen.get_or_init(|| {
            let raw = self.acc.state().map_or(0, i32::cast_unsigned);
            let mut states = states_from_msaa(raw);
            // An edit control says whether it is multi-line by its window
            // style, as NVDA's edit control class reads it.
            if self.client
                && self.role() == Role::EditableText
                && window::is_multiline_edit(self.hwnd)
            {
                states.insert(State::Multiline);
            }
            Seen {
                states,
                invisible: raw & STATE_SYSTEM_INVISIBLE != 0,
            }
        })
    }

    /// The object's states (`accState`).
    #[must_use]
    pub fn states(&self) -> StateSet {
        self.seen().states
    }

    /// Whether the object is invisible (`accState`).
    #[must_use]
    pub fn invisible(&self) -> bool {
        self.seen().invisible
    }

    /// The object's name (`accName`), `None` when it is empty or blank.
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.name
            .get_or_init(|| visible_text(self.acc.name()))
            .clone()
    }

    /// The object's value (`accValue`), `None` when it is empty or blank.
    /// An edit control's value is its text.
    #[must_use]
    pub fn value(&self) -> Option<String> {
        visible_text(self.acc.value())
    }

    /// The object's description (`accDescription`), `None` when it is
    /// empty.
    #[must_use]
    pub fn description(&self) -> Option<String> {
        non_empty(self.acc.description())
    }
}
