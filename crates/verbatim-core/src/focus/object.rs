//! The focus object: the focus Core last executed, as NVDA's focus object,
//! which commands act on and against which the next focus is spoken
//! (`docs/nvda/events.md`, "What a focus event actually does"). The
//! committed node and the last queued focus are the two versions
//! ([`crate::focus::versions`]); this is what Core keeps with the committed
//! focus to speak from.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    EarlierElement, FocusText, NodeSnapshot, Pid, TraceId, WindowFacts, WindowHandle,
};

use crate::state::FocusContext;

/// Core's focus object ([module docs](self)).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FocusState {
    /// The focus last executed, none before the first.
    object: Option<FocusContext>,
}

/// A focus as Core executes it: the parts of a focus the focus handling
/// reads, owned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) trace_id: TraceId,
    pub(crate) source: Pid,
    /// When its reader observed it, in microseconds; 0 when not known.
    pub(crate) observed_at_us: u64,
    pub(crate) window: Option<WindowFacts>,
    pub(crate) node: NodeSnapshot,
    pub(crate) whole_window: bool,
    /// Its ancestors as Core takes them, outermost first.
    pub(crate) ancestors: Arc<[NodeSnapshot]>,
    pub(crate) text: Option<FocusText>,
    pub(crate) earlier: Option<EarlierElement>,
    /// The top-level window its read named, which stands for the entry
    /// just below the desktop when it carries no window facts, as a focus
    /// Core read itself does.
    pub(crate) top_level: Option<WindowHandle>,
}

impl FocusState {
    /// The focus object, which commands act on.
    pub(crate) fn object(&self) -> Option<&FocusContext> {
        self.object.as_ref()
    }

    /// The focus object, to change in place.
    pub(crate) fn object_mut(&mut self) -> Option<&mut FocusContext> {
        self.object.as_mut()
    }

    /// Core committed the sleeping focus, which no reader read and which
    /// has no ancestors: commands find no focus object, and the next focus
    /// is spoken from the top, its window's title included.
    pub(crate) fn sleep(&mut self) {
        self.object = None;
    }

    /// `focus` was executed: it is the focus object.
    pub(crate) fn execute(&mut self, focus: FocusContext) {
        self.object = Some(focus);
    }
}
