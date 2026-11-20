//! A read or move in a node's text as an event-queue request
//! (`docs/design/readers-tell-core-asks-build1.md`, "Requests in Core"):
//! the shared list's text request, naming the node's window, answered with
//! the outpost's text reply. The commands that read the committed focus's
//! text resolve it here at their turn.

use verbatim_model::request::{Reply, Request as Wire};
use verbatim_model::{NodeId, Pid, Role, TextFailure, TextOp, TextReply};

use crate::event_queue::{Reader, Request, RequestWindow};
use crate::state::SrState;

/// One text request on `node`, of its application's outpost, naming the
/// node's window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextRequest {
    /// The application whose outpost holds the node.
    pub(crate) process: Pid,
    /// The node's window.
    pub(crate) window: RequestWindow,
    /// The node.
    pub(crate) node: NodeId,
    /// What is read or moved.
    pub(crate) op: TextOp,
}

impl Request for TextRequest {
    type Reply = TextReply;

    fn reader(&self) -> Reader {
        Reader::Outpost(self.process)
    }

    fn window(&self) -> RequestWindow {
        self.window
    }

    fn wire(&self) -> Wire {
        Wire::Text {
            node: self.node,
            op: self.op.clone(),
        }
    }

    fn decode(reply: Reply) -> Option<TextReply> {
        match reply {
            Reply::Text(reply) => Some(reply),
            _ => None,
        }
    }
}

/// A text request's result, parsed: the op's own reply, which `own` takes,
/// or one of the failures any text request can be answered
/// ([`TextFailure`]), so every answer a correct outpost gives decodes, and
/// the function that asked handles each. `None` only for a result that is
/// no text reply, or another op's own reply: a malformed answer.
pub(crate) fn decode_text<T>(
    reply: Reply,
    own: impl FnOnce(TextReply) -> Option<T>,
) -> Option<Result<T, TextFailure>> {
    let Reply::Text(reply) = reply else {
        return None;
    };
    match reply.into_failure() {
        Ok(failure) => Some(Err(failure)),
        Err(reply) => own(reply).map(Ok),
    }
}

/// A node with text: its application, window and role.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TextNode {
    /// The application whose outpost holds the node.
    pub(crate) process: Pid,
    /// The node's window.
    pub(crate) window: RequestWindow,
    /// The node.
    pub(crate) node: NodeId,
    /// Its role.
    pub(crate) role: Role,
}

impl TextNode {
    /// Its request of `op`.
    pub(crate) fn request(self, op: TextOp) -> TextRequest {
        TextRequest {
            process: self.process,
            window: self.window,
            node: self.node,
            op,
        }
    }
}

/// The committed focus's text, as a caret command finds it at its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaretTarget {
    /// No focus a reader holds: nothing committed, its reader ended, or
    /// Core's own focus. "No caret".
    NoCaret,
    /// A focus that cannot have text. "Not supported".
    NoText,
    /// The focus's node.
    Text(TextNode),
}

impl CaretTarget {
    /// What the committed focus of `state` gives a caret command: a focus
    /// that may have text by its role, or that Core holds a caret on, has
    /// a caret.
    pub(crate) fn of(state: &SrState) -> Self {
        let Some(focus) = state.versions.live() else {
            return Self::NoCaret;
        };
        let (node, role) = (focus.context.snapshot.id, focus.context.snapshot.role);
        if !crate::editing::may_have_text(role) && state.focus_caret(node).is_none() {
            return Self::NoText;
        }
        Self::Text(TextNode {
            process: focus.context.source,
            window: focus.kept.asked,
            node,
            role,
        })
    }
}
