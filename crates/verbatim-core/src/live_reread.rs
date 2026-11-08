//! Core's live re-reads (`books/dev/src/reference/crates/verbatim-core.md`,
//! "The ancestry cache"): a focus report that would speak an ancestor its
//! focus reader took from its ancestry cache, as entered or as the title,
//! waits in the focus queue (`crate::focus_queue`), and the focus reader
//! that sent it is asked to read the same element again live, with no
//! ancestry cache. Its answer carries no cached ancestors, so it never
//! needs another. The reports Core takes after it wait behind it.
//!
//! A waiting report ends on exactly one path: its answer, executed in its
//! turn, or refused by the foreground test; its answer of gone; the end of
//! its reader's outpost generation; or, for the listener's, the listener's
//! replacement. A report that ends unanswered is never spoken, and stays
//! the last queued focus until another is queued. An answer to a report no
//! longer waiting is ignored, so a late answer is set aside by Core's own
//! state, whichever ending came first.

use serde::{Deserialize, Serialize};
use verbatim_model::FocusReaderKind;

/// A live re-read's answer of gone, or the listener's replacement: the
/// waiting reports they name end unanswered, and the queue runs on. Any
/// other input does nothing here.
pub(crate) fn ended(
    state: &mut crate::SrState,
    input: &verbatim_model::Input,
) -> Vec<verbatim_model::Effect> {
    match input {
        verbatim_model::Input::LiveRereadGone { outpost, request } => {
            state.focus.unanswered(*outpost, *request);
        }
        verbatim_model::Input::ListenerReplaced => state
            .focus
            .readers_ended(|(_, kind)| kind == FocusReaderKind::Listener),
        _ => return Vec::new(),
    }
    crate::reduce::run_queue(state)
}

/// The live re-reads' request numbers, and how many Core has asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveRereads {
    /// The next request number.
    next: u64,
    /// How many live re-reads Core has asked for.
    count: u64,
}

impl LiveRereads {
    /// Counts a live re-read and returns its request number.
    pub(crate) fn ask(&mut self) -> u64 {
        let request = self.next;
        self.next = self.next.saturating_add(1);
        self.count = self.count.saturating_add(1);
        request
    }

    /// How many live re-reads Core has asked for.
    pub(crate) fn count(&self) -> u64 {
        self.count
    }
}

#[cfg(test)]
mod generated;
#[cfg(test)]
mod tests;
