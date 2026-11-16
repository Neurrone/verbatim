//! A program's commit (`books/dev/src/explanation/design/ownership.md`): what one step of a program changes, built
//! with every call it needs already made, and posted to the mailbox. The
//! reducer applies it only while the program's request is live; until
//! then the program applies its pure part to its own copies
//! ([`super::reducer::Facts`]), so what it reads after a commit is what the
//! reducer will hold.
//!
//! [`Effects`] carries, besides the reducer's [`Change`], what only the
//! shell can apply: the program's text writes since its last commit,
//! writes to the store made on the reducer's thread under the store's lock
//! alone, with no call into the application.

#![forbid(unsafe_code)]

use verbatim_model::protocol::OutpostToCore;

use super::reducer::Change;
use super::store::TextWrite;

/// What one commit does, built on the stack before the commit with every
/// call it needs already made.
#[derive(Default)]
pub(super) struct Effects {
    /// The program's message to Core: a tell.
    pub(super) result: Option<OutpostToCore>,
    /// The program's text writes since its last commit, in order: its
    /// text facts' changes, which it applied to its own copies as it made
    /// them, with the ranges they name, and its patterns and support.
    pub(super) text: Vec<TextWrite>,
}

impl Effects {
    /// The reducer's part, its text facts' changes given by the store as
    /// it makes the text writes, and the text writes only the shell makes.
    pub(super) fn split(self) -> (Change, Vec<TextWrite>) {
        let change = Change {
            result: self.result,
            text: Vec::new(),
        };
        (change, self.text)
    }
}

/// What [`Worker::commit`](super::worker) did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Committed {
    /// The effects were posted for the reducer.
    Applied,
    /// Nothing was posted: the worker was abandoned.
    Refused,
}
