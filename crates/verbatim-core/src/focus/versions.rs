//! The two versions of the focus, [`Versions`]: the committed focus, one
//! [`Focus`] record of what Core spoke of it, what it keeps of its focus
//! answer and its line, and the readers' last focus (`books/dev/src/reference/crates/verbatim-core.md`,
//! "Focus, events and objects", the committed focus; ADR 0003, "The two
//! versions of the focus").

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{KnownAncestor, MsaaIds};
use verbatim_model::tell::ElementToken;
use verbatim_model::{Issuer, NodeId, NodeSnapshot, OutpostId, Role, StateSet, WindowHandle};

use crate::keys::entry::Fence;
use crate::state::FocusContext;

/// How a reader's focus is named, which the duplicate tests compare.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ReadBy {
    /// A UI Automation element: its runtime id, and the token the listener
    /// keeps it behind, `None` once the listener no longer holds it (after
    /// a re-take, or a late answer read by an outpost).
    Uia {
        /// Its runtime id.
        runtime_id: RuntimeId,
        /// The listener's token for it.
        token: Option<ElementToken>,
    },
    /// An MSAA object: the ids it was read by and the child id the
    /// outpost holds it by. A focus read through `accFocus` carries the ids
    /// it is held by, as NVDA's object gets its ids from `IAccIdentity`
    /// (`NVDAObjects/IAccessible/__init__.py` 803 to 808); a focus read
    /// that carries none names no ids, so no later event is its duplicate.
    Msaa {
        /// The ids it was read by, `None` for a read that carries none.
        event: Option<MsaaIds>,
        /// The child id the outpost holds it by.
        held_child: i32,
    },
    /// A plain window Core's plain reader holds: a ghost window found in
    /// front by a read NVDA makes itself, which NVDA makes a plain window
    /// object of, with no ids an event can name (`crate::focus::plain`).
    /// NVDA's duplicate test applies only to `IAccessible` objects
    /// (`IAccessibleHandler/__init__.py` 772 to 778), so a plain focus is
    /// never a duplicate, and nothing is a duplicate of it.
    Plain,
}

impl ReadBy {
    /// The backend that read it: a plain window is read as a window, by
    /// Windows' own calls, which Core names as MSAA's window object.
    pub(crate) fn backend(&self) -> verbatim_model::Backend {
        match self {
            Self::Uia { .. } => verbatim_model::Backend::Uia,
            Self::Msaa { .. } | Self::Plain => verbatim_model::Backend::Msaa,
        }
    }
}

/// Where a focus is, by window: its nearest window, its top-level window
/// and the top of its owner chain, and the windows of its ancestors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Place {
    /// Its nearest window.
    pub(crate) window: WindowHandle,
    /// Its top-level window.
    pub(crate) top_level: WindowHandle,
    /// The top of its owner chain.
    pub(crate) root_owner: WindowHandle,
    /// Each ancestor's own window, nearest first, for a window object;
    /// shared, as the chain is, so a checkpoint copies a pointer.
    pub(crate) ancestor_windows: Arc<[WindowHandle]>,
}

impl Place {
    /// Whether `window` is a top-level window unrelated to this place:
    /// neither its top-level window, nor its root owner, nor an ancestor's
    /// window (Table 3, columns D and E). The last holds only for a focus
    /// in Verbatim's popup menu, which has no owner yet keeps the hidden
    /// frame as its outermost ancestor: the frame coming to the front
    /// leaves no focus in the menu (Verbatim-only).
    pub(crate) fn unrelated(&self, window: WindowHandle, root_owner: WindowHandle) -> bool {
        window != self.top_level
            && root_owner != self.root_owner
            && !self.ancestor_windows.contains(&window)
    }

    /// Whether `window` is this focus's window or one of its ancestors'.
    pub(crate) fn holds_window(&self, window: WindowHandle) -> bool {
        self.window == window || self.ancestor_windows.contains(&window)
    }
}

/// The readers' last focus (NVDA's last queued focus).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LastFocus {
    /// How it was read.
    pub(crate) read_by: ReadBy,
    /// Where it is.
    pub(crate) place: Place,
    /// Its role, which the menu-start rule judges.
    pub(crate) role: Role,
    /// The speech fence of the entry that executed it, while no key has
    /// come since and no focus read was tried and refused since: `None`
    /// once a read was tried and refused or found nothing (Dickson,
    /// 2026-11-18). A later MSAA focus of that fence is "with no key
    /// between", which the identity check in the duplicate test reads
    /// (`docs/parity/focus-events.md`, "Duplicate focus suppression").
    pub(crate) since: Option<Fence>,
}

impl LastFocus {
    /// Whether it names the committed focus `focus`, whatever its fence.
    pub(crate) fn names(&self, focus: &Focus) -> bool {
        (&self.read_by, &self.place, self.role)
            == (
                &focus.kept.read_by,
                &focus.kept.place,
                focus.context.snapshot.role,
            )
    }
}

/// What Core keeps with a committed focus a reader read, each from its
/// focus answer (design, "Focus and foreground"), beside what it spoke.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// How it was read.
    pub(crate) read_by: ReadBy,
    /// Where it is.
    pub(crate) place: Place,
    /// The window the read that found it named, which a re-take of its
    /// application names.
    pub(crate) asked: crate::event_queue::RequestWindow,
    /// The foreground window it entered under, when it could be read.
    pub(crate) foreground: Option<NodeId>,
    /// Whether its value comes from its selection.
    pub(crate) value_from_selection: bool,
    /// Its selection baseline and text-changed flag (Table 17).
    pub(crate) selection: crate::text_facts::SelectionFacts,
    /// Whether it is a console host's terminal: a terminal whose tell's
    /// window, or, for a focus Core read itself, whose nearest ancestor
    /// with a window of its own, is a console window. The console's caret
    /// event is its caret, so the system caret's tells are dropped while
    /// it is committed (`books/dev/src/reference/crates/verbatim-core.md`, "Terminals").
    pub(crate) console_terminal: bool,
}

/// One entry of a committed focus's line: an ancestor as Core spoke from
/// it, and the two facts its snapshot lacks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LineEntry {
    /// The ancestor: its node, and its own window for a window object, by
    /// which the next focus read's walk meets it.
    pub(crate) snapshot: NodeSnapshot,
    /// Its runtime id, for a UI Automation node.
    pub(crate) runtime_id: Option<RuntimeId>,
    /// Whether it is a simple child, never spoken as a state change's
    /// ancestor.
    pub(crate) simple_child: bool,
}

impl crate::focus_rules::EnteredKey for LineEntry {
    fn object_key(&self) -> verbatim_model::ObjectKey {
        verbatim_model::ObjectKey::of(&self.snapshot)
    }
}

/// A reader's committed focus, one record (ADR 0003, "The two versions of
/// the focus"): what Core spoke of it, what it keeps of its focus answer,
/// and its line, against which the next focus is joined and spoken and
/// from which the next focus request's known ancestors come.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Focus {
    /// What Core spoke of it.
    pub(crate) context: FocusContext,
    /// What Core keeps of its focus answer.
    pub(crate) kept: Kept,
    /// Its ancestors, outermost first. The part that grows with the
    /// application, so shared: a checkpoint, which every entry that asks
    /// takes, copies a pointer, not the line.
    pub(crate) line: Arc<[LineEntry]>,
}

impl Focus {
    /// The focus itself as the next focus read's walk meets it, an
    /// ancestor of a focus inside it: NVDA's walk meets the old focus as it
    /// meets the old focus's ancestors (`docs/parity/focus-context.md`).
    pub(crate) fn as_entry(&self) -> LineEntry {
        LineEntry {
            snapshot: self.context.snapshot.clone(),
            runtime_id: match &self.kept.read_by {
                ReadBy::Uia { runtime_id, .. } => Some(runtime_id.clone()),
                ReadBy::Msaa { .. } | ReadBy::Plain => None,
            },
            simple_child: matches!(self.kept.read_by, ReadBy::Msaa { held_child, .. } if held_child != 0),
        }
    }

    /// The readers' last focus this focus becomes when it executes for an
    /// entry of speech fence `since`.
    pub(crate) fn last(&self, since: Fence) -> LastFocus {
        LastFocus {
            read_by: self.kept.read_by.clone(),
            place: self.kept.place.clone(),
            role: self.context.snapshot.role,
            since: Some(since),
        }
    }

    /// The states last spoken of `node`, the focus or one of its ancestors,
    /// or its reading's when nothing was recorded for an ancestor; `None`
    /// for a node not in the focus line.
    pub(crate) fn spoken_states(&self, node: NodeId) -> Option<StateSet> {
        let context = &self.context;
        if node == context.snapshot.id {
            return Some(context.spoken.states);
        }
        context
            .spoken
            .ancestors
            .iter()
            .find(|(id, _)| *id == node)
            .map(|(_, states)| *states)
            .or_else(|| {
                self.line
                    .iter()
                    .find(|entry| entry.snapshot.id == node)
                    .map(|entry| entry.snapshot.states)
            })
    }
}

/// The committed focus, as Table 3's rows tell it apart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Committed {
    /// Nothing committed yet: startup.
    #[default]
    Nothing,
    /// A reader's focus whose reader lives.
    Read(Box<Focus>),
    /// A reader's focus whose reader ended: its data kept for the silent
    /// re-take, its ids dead.
    Ended(Box<Focus>),
    /// The sleeping focus Core committed itself while the secure desktop is
    /// up, which no reader read: commands find no focus object, and the
    /// next focus is spoken from the top, its window's title included.
    Own,
}

/// The two versions of the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Versions {
    /// The committed focus.
    pub(crate) committed: Committed,
    /// The readers' last focus; empty at startup and after the foreground
    /// left for an unrelated top-level window.
    pub(crate) last: Option<LastFocus>,
    /// The postponed dialog, read again at its show (`super::shown`);
    /// every commit ends it.
    pub(crate) postponed: Option<super::shown::Postponed>,
}

/// What the committed focus's reader ending does (Table 3, column F).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderEnd {
    /// Nothing: the ended reader held no committed node.
    Nothing,
    /// The committed focus is marked ended; Core appends the re-take
    /// entry when the rule asks for one.
    Retake,
}

impl Versions {
    /// A reader's focus executes (column B), for an entry of speech fence
    /// `since`: both versions become it.
    pub(crate) fn execute(&mut self, focus: Focus, since: Fence) {
        self.last = Some(focus.last(since));
        self.committed = Committed::Read(Box::new(focus));
        self.postponed = None;
    }

    /// The live committed focus spoken again, `focus` with what Core spoke
    /// of it replaced: it is the committed focus again, a live reader's,
    /// and the readers' last focus and the postponed dialog are left
    /// ([`super::execute::Commit::InPlace`]).
    pub(crate) fn respoken(&mut self, focus: Box<Focus>) {
        self.committed = Committed::Read(focus);
    }

    /// Core commits its own focus (column C): the committed focus becomes
    /// it; the readers' last focus is unchanged.
    pub(crate) fn commit_own(&mut self) {
        self.committed = Committed::Own;
        self.postponed = None;
    }

    /// A focus read was tried at its own turn and answered refused or not
    /// there, or its reader ended: the readers' last focus's fence ends, so
    /// no later focus is "with no key between" it (Dickson, 2026-11-18).
    pub(crate) fn tried_and_refused(&mut self) {
        if let Some(last) = self.last.as_mut() {
            last.since = None;
        }
    }

    /// A foreground marker for `window`, of root owner `root_owner`,
    /// reaches the front (columns D and E): the readers' last focus is
    /// emptied when the window is a top-level window unrelated to the
    /// committed focus then. With nothing committed, or Core's own focus,
    /// nothing is related, so a set last focus is emptied.
    pub(crate) fn foreground_marker(&mut self, window: WindowHandle, root_owner: WindowHandle) {
        let unrelated = self
            .focus()
            .is_none_or(|focus| focus.kept.place.unrelated(window, root_owner));
        if unrelated {
            self.last = None;
        }
    }

    /// The reader holding the committed focus's node, generation
    /// `outpost`, ended (column F).
    pub(crate) fn outpost_ended(&mut self, outpost: OutpostId) -> ReaderEnd {
        match std::mem::take(&mut self.committed) {
            Committed::Read(mut focus) | Committed::Ended(mut focus)
                if focus.context.snapshot.id.issuer() == Issuer::Outpost(outpost) =>
            {
                crate::text_facts::outpost_ended(&mut focus, outpost);
                self.committed = Committed::Ended(focus);
                ReaderEnd::Retake
            }
            other => {
                self.committed = other;
                ReaderEnd::Nothing
            }
        }
    }

    /// The committed focus when it is a reader's focus, whose reader lives
    /// or ended.
    pub(crate) fn focus(&self) -> Option<&Focus> {
        match &self.committed {
            Committed::Read(focus) | Committed::Ended(focus) => Some(focus),
            Committed::Own | Committed::Nothing => None,
        }
    }

    /// [`Versions::focus`], to change in place.
    pub(crate) fn focus_mut(&mut self) -> Option<&mut Focus> {
        match &mut self.committed {
            Committed::Read(focus) | Committed::Ended(focus) => Some(focus),
            Committed::Own | Committed::Nothing => None,
        }
    }

    /// The committed focus when it is a reader's focus whose reader lives.
    pub(crate) fn live(&self) -> Option<&Focus> {
        match &self.committed {
            Committed::Read(focus) => Some(focus),
            Committed::Ended(_) | Committed::Own | Committed::Nothing => None,
        }
    }

    /// [`Versions::live`], to change in place.
    pub(crate) fn live_mut(&mut self) -> Option<&mut Focus> {
        match &mut self.committed {
            Committed::Read(focus) => Some(focus),
            Committed::Ended(_) | Committed::Own | Committed::Nothing => None,
        }
    }

    /// The committed focus's node, whose reader lives or ended; `None`
    /// with nothing committed or an own focus.
    pub(crate) fn committed_node(&self) -> Option<NodeId> {
        self.focus().map(|focus| focus.context.snapshot.id)
    }

    /// The ancestors Core already knows, sent with a focus request to the
    /// outpost generation `asked` (column H): a reader's committed focus
    /// itself and its ancestors, only while the readers' last focus is set,
    /// a node only when the answering generation minted it; nothing of an
    /// ended generation, whose ids name nothing, but the ancestors another
    /// generation minted stay known, as NVDA's old focus line keeps every
    /// object when the old focus's application exits
    /// (`docs/parity/focus-context.md`); none for an own focus. The listener mints no node, so `asked` is
    /// `None` for it.
    #[cfg(test)]
    pub(crate) fn known_ancestors(&self, asked: Option<OutpostId>) -> Vec<KnownAncestor> {
        self.known_with_places(asked)
            .into_iter()
            .map(|(_, known)| known)
            .collect()
    }

    /// [`Versions::known_ancestors`], each with the place of the ancestor
    /// it names in the committed focus's line, the focus itself at 0 and
    /// its ancestors after it nearest first, by which an answer's meeting
    /// point is found again.
    pub(crate) fn known_with_places(
        &self,
        asked: Option<OutpostId>,
    ) -> Vec<(usize, KnownAncestor)> {
        if self.last.is_none() {
            return Vec::new();
        }
        // A reader's focus is met itself; of a focus whose generation
        // ended, only what another generation minted is met.
        let (focus, ended) = match &self.committed {
            Committed::Read(focus) => (focus, None),
            Committed::Ended(focus) => (focus, Some(focus.context.snapshot.id.issuer())),
            Committed::Own | Committed::Nothing => return Vec::new(),
        };
        let mut known = Vec::new();
        let itself = focus.as_entry();
        let line = std::iter::once(&itself).chain(focus.line.iter().rev());
        for (place, entry) in line.enumerate() {
            let minted_by = entry.snapshot.id.issuer();
            if Some(minted_by) == ended {
                continue;
            }
            if asked.map(Issuer::Outpost) == Some(minted_by) {
                known.push((place, KnownAncestor::Node(entry.snapshot.id)));
            }
            if let Some(runtime_id) = &entry.runtime_id {
                known.push((place, KnownAncestor::Uia(runtime_id.clone())));
            }
            if let Some(window) = entry.snapshot.details.window {
                known.push((place, KnownAncestor::Window(window)));
            }
        }
        known
    }

    /// Whether a system caret tell has a target (column G): the readers'
    /// last focus is set and is the committed focus, a reader's focus
    /// whose reader lives. The console terminal's drop is the caret rule's.
    pub(crate) fn caret_target(&self) -> Option<&Focus> {
        let last = self.last.as_ref()?;
        self.live().filter(|focus| last.names(focus))
    }
}

#[cfg(test)]
#[path = "versions_tests.rs"]
pub(crate) mod tests;
