//! The two versions of the focus (design Table 3; ADR 0003, "The two
//! versions of the focus"): the committed focus, the focus Core has
//! executed, which commands, the narrow rule and typed echo judge; and the
//! readers' last focus, the newest focus a reader read that Core executed,
//! which the duplicate tests, the whole-window refusal, a run's refusals
//! and the system caret's target judge. Core can also commit a focus
//! itself, which no reader read.
//!
//! The committed focus's node and its spoken record stay in the reducer's
//! focus context, shared with today's announcement code; what Core keeps
//! with it from the focus answer is [`Kept`], beside it here.

use serde::{Deserialize, Serialize};
use verbatim_model::property::RuntimeId;
use verbatim_model::request::{KnownAncestor, MsaaIds, ObjectClass};
use verbatim_model::tell::ElementToken;
use verbatim_model::{NodeId, OutpostId, Pid, Role, WindowHandle};

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
    /// An MSAA object: the ids its event named and the child id the
    /// outpost holds it by. A focus read through `accFocus` names no ids
    /// of an event, so no later event is its duplicate.
    Msaa {
        /// The ids its event named, `None` for a focus read through
        /// `accFocus`.
        event: Option<MsaaIds>,
        /// The child id the outpost holds it by.
        held_child: i32,
    },
}

impl ReadBy {
    /// The backend that read it.
    pub(crate) fn backend(&self) -> verbatim_model::Backend {
        match self {
            Self::Uia { .. } => verbatim_model::Backend::Uia,
            Self::Msaa { .. } => verbatim_model::Backend::Msaa,
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
    /// Each ancestor's own window, nearest first, for a window object.
    pub(crate) ancestor_windows: Vec<WindowHandle>,
}

impl Place {
    /// Whether `window` is a top-level window unrelated to this place:
    /// neither its top-level window nor its root owner (Table 3, columns D
    /// and E).
    pub(crate) fn unrelated(&self, window: WindowHandle, root_owner: WindowHandle) -> bool {
        window != self.top_level && root_owner != self.root_owner
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
    /// Its class verdict, which may change the duplicate test.
    pub(crate) class: ObjectClass,
    /// Its role, which the menu-start rule judges.
    pub(crate) role: Role,
}

/// What Core keeps with a committed focus a reader read, each from its
/// focus answer (design, "Focus and foreground").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Kept {
    /// The application it belongs to.
    pub(crate) source: Pid,
    /// The outpost generation that holds its node.
    pub(crate) outpost: OutpostId,
    /// Its node.
    pub(crate) node: NodeId,
    /// How it was read.
    pub(crate) read_by: ReadBy,
    /// Where it is.
    pub(crate) place: Place,
    /// The window the read that found it named, which a re-take of its
    /// application names.
    pub(crate) window: crate::event_queue::RequestWindow,
    /// Its ancestors as the next focus read's walk meets them, nearest
    /// first: each node, its runtime id for a UI Automation node, and its
    /// own window for a window object.
    pub(crate) ancestors: Vec<KeptAncestor>,
    /// The focused child's ids, when a redirect from a list reached it.
    pub(crate) redirected_to: Option<MsaaIds>,
    /// The foreground window it entered under, when it could be read.
    pub(crate) foreground: Option<NodeId>,
    /// Whether its value comes from its selection.
    pub(crate) value_from_selection: bool,
    /// Its class verdict.
    pub(crate) class: ObjectClass,
    /// Its role, as the answer read it.
    pub(crate) role: Role,
    /// Its own window, for a window object: by it the next focus read's
    /// walk meets the focus itself, as an ancestor of a focus inside it.
    pub(crate) own_window: Option<WindowHandle>,
    /// Its selection baseline and text-changed flag (Table 17).
    #[serde(default)]
    pub(crate) selection: crate::text_facts::SelectionFacts,
    /// Whether it is a console host's terminal: a terminal whose tell's
    /// window, or, for a focus Core read itself, whose nearest ancestor
    /// with a window of its own, is a console window. The console's caret
    /// event is its caret, so the system caret's tells are dropped while
    /// it is committed (`docs/design/readers-tell-core-asks-build1.md`,
    /// "Terminals in this build").
    #[serde(default)]
    pub(crate) console_terminal: bool,
}

/// One ancestor of a committed focus, as Core keeps it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct KeptAncestor {
    /// The node.
    pub(crate) node: NodeId,
    /// Its runtime id, for a UI Automation node.
    pub(crate) runtime_id: Option<RuntimeId>,
    /// Its own window, for a window object.
    pub(crate) window: Option<WindowHandle>,
    /// Whether it is a simple child, never spoken as a state change's
    /// ancestor.
    pub(crate) simple_child: bool,
    /// Its dialog text, when it is a dialog the walk read.
    pub(crate) dialog_text: Option<String>,
}

impl Kept {
    /// The focus itself as the next focus read's walk meets it, an
    /// ancestor of a focus inside it, with `dialog_text`, its dialog text
    /// when it is a dialog: NVDA's walk meets the old focus as it meets
    /// the old focus's ancestors (`docs/parity/focus-context.md`).
    pub(crate) fn as_ancestor(&self, dialog_text: Option<String>) -> KeptAncestor {
        KeptAncestor {
            node: self.node,
            runtime_id: match &self.read_by {
                ReadBy::Uia { runtime_id, .. } => Some(runtime_id.clone()),
                ReadBy::Msaa { .. } => None,
            },
            window: self.own_window,
            simple_child: matches!(self.read_by, ReadBy::Msaa { held_child, .. } if held_child != 0),
            dialog_text,
        }
    }

    /// The readers' last focus this focus becomes when it executes.
    pub(crate) fn last(&self) -> LastFocus {
        LastFocus {
            read_by: self.read_by.clone(),
            place: self.place.clone(),
            class: self.class,
            role: self.role,
        }
    }
}

/// The committed focus, as Table 3's rows tell it apart.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Committed {
    /// Nothing committed yet: startup.
    #[default]
    Nothing,
    /// A reader's focus, with what Core keeps of it.
    Read(Box<Kept>),
    /// A reader's focus whose reader ended: its data kept for the silent
    /// re-take, its ids dead.
    Ended(Box<Kept>),
    /// A focus Core committed itself, such as the sleeping focus while
    /// the secure desktop is up, with the reader's focus it stands over,
    /// if any.
    Own {
        /// The reader's focus it was committed over, whose ancestors stand
        /// for its own; `None` for a sourceless focus.
        source: Option<Box<Kept>>,
    },
}

/// The two versions of the focus.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Versions {
    /// The committed focus.
    pub(crate) committed: Committed,
    /// The readers' last focus; empty at startup and after the foreground
    /// left for an unrelated top-level window.
    pub(crate) last: Option<LastFocus>,
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
    /// A reader's focus executes (column B): both versions become it.
    pub(crate) fn execute(&mut self, kept: Kept) {
        self.last = Some(kept.last());
        self.committed = Committed::Read(Box::new(kept));
    }

    /// Core commits its own focus (column C): the committed focus becomes
    /// it, standing over the reader's focus committed until now, if any;
    /// the readers' last focus is unchanged. A newer own focus keeps the
    /// source of the one it replaces.
    pub(crate) fn commit_own(&mut self, sourceless: bool) {
        let source = match std::mem::take(&mut self.committed) {
            Committed::Read(kept) | Committed::Ended(kept) => Some(kept),
            Committed::Own { source } => source,
            Committed::Nothing => None,
        };
        self.committed = Committed::Own {
            source: source.filter(|_| !sourceless),
        };
    }

    /// A foreground marker for `window`, of root owner `root_owner`,
    /// reaches the front (columns D and E): the readers' last focus is
    /// emptied when the window is a top-level window unrelated to the
    /// committed focus then. With nothing committed, or Core's sourceless
    /// focus, nothing is related, so a set last focus is emptied.
    pub(crate) fn foreground_marker(&mut self, window: WindowHandle, root_owner: WindowHandle) {
        let unrelated = self
            .committed_place()
            .is_none_or(|place| place.unrelated(window, root_owner));
        if unrelated {
            self.last = None;
        }
    }

    /// The reader holding the committed focus's node, generation
    /// `outpost`, ended (column F).
    pub(crate) fn outpost_ended(&mut self, outpost: OutpostId) -> ReaderEnd {
        match std::mem::take(&mut self.committed) {
            Committed::Read(mut kept) | Committed::Ended(mut kept) if kept.outpost == outpost => {
                crate::text_facts::outpost_ended(&mut kept, outpost);
                self.committed = Committed::Ended(kept);
                ReaderEnd::Retake
            }
            other => {
                self.committed = other;
                ReaderEnd::Nothing
            }
        }
    }

    /// What Core keeps of the committed focus when it is a reader's focus
    /// whose reader lives.
    pub(crate) fn committed_read(&self) -> Option<&Kept> {
        match &self.committed {
            Committed::Read(kept) => Some(kept),
            Committed::Ended(_) | Committed::Own { .. } | Committed::Nothing => None,
        }
    }

    /// The committed focus's place, `None` with nothing committed or a
    /// sourceless own focus.
    pub(crate) fn committed_place(&self) -> Option<&Place> {
        self.committed_kept().map(|kept| &kept.place)
    }

    /// What Core keeps of the committed focus, or of the reader's focus an
    /// own focus stands over.
    pub(crate) fn committed_kept(&self) -> Option<&Kept> {
        match &self.committed {
            Committed::Read(kept) | Committed::Ended(kept) => Some(kept),
            Committed::Own { source } => source.as_deref(),
            Committed::Nothing => None,
        }
    }

    /// The ancestors Core already knows, sent with a focus request to the
    /// outpost generation `asked` (column H): a reader's committed focus
    /// itself and its ancestors, only while the readers' last focus is set, a node only
    /// when the answering generation minted it; none from an ended
    /// generation; for an own focus, its source's; none for a sourceless
    /// one. The listener mints no node, so `asked` is `None` for it.
    pub(crate) fn known_ancestors(&self, asked: Option<OutpostId>) -> Vec<KnownAncestor> {
        self.known_with_places(asked)
            .into_iter()
            .map(|(_, known)| known)
            .collect()
    }

    /// [`Versions::known_ancestors`], each with the place of the ancestor
    /// it names in the committed focus's line, the focus itself at 0 and
    /// its kept ancestors after it nearest first, by which an answer's
    /// meeting point is found again.
    pub(crate) fn known_with_places(
        &self,
        asked: Option<OutpostId>,
    ) -> Vec<(usize, KnownAncestor)> {
        if self.last.is_none() {
            return Vec::new();
        }
        // A reader's focus is met itself; an own focus stands over its
        // source, which no walk meets as the focus.
        let (kept, itself) = match &self.committed {
            Committed::Read(kept) => (kept, true),
            Committed::Own {
                source: Some(source),
            } => (source, false),
            Committed::Ended(_) | Committed::Own { source: None } | Committed::Nothing => {
                return Vec::new();
            }
        };
        let mine = asked == Some(kept.outpost);
        let mut known = Vec::new();
        let focus = kept.as_ancestor(None);
        let line = std::iter::once(&focus).chain(&kept.ancestors);
        for (place, ancestor) in line.enumerate().skip(usize::from(!itself)) {
            if mine {
                known.push((place, KnownAncestor::Node(ancestor.node)));
            }
            if let Some(runtime_id) = &ancestor.runtime_id {
                known.push((place, KnownAncestor::Uia(runtime_id.clone())));
            }
            if let Some(window) = ancestor.window {
                known.push((place, KnownAncestor::Window(window)));
            }
        }
        known
    }

    /// Whether a system caret tell has a target (column G): the readers'
    /// last focus is set and is the committed focus, a reader's focus
    /// whose reader lives. The console terminal's drop is the caret rule's.
    pub(crate) fn caret_target(&self) -> Option<&Kept> {
        let last = self.last.as_ref()?;
        match &self.committed {
            Committed::Read(kept) if kept.last() == *last => Some(kept),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "versions_tests.rs"]
pub(crate) mod tests;
