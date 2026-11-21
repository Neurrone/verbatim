//! The messages between verbatim.exe and its children, the focus listener
//! and the outposts, and the types they carry (milestone M4.4,
//! `books/dev/src/explanation/design/ownership.md`): one enum for each direction and
//! role ([`CoreToOutpost`], [`CoreToListener`], [`OutpostToCore`] and
//! [`ListenerToCore`]). The framing that carries them on the pipes is the outpost crate's own;
//! these are pure data, and their methods pure functions of it.

#![deny(clippy::wildcard_enum_match_arm)]

mod responder;
pub mod samples;

pub use responder::Responder;

use serde::{Deserialize, Serialize};

use crate::{NodeDetails, NodeId, OutpostId, Pid, Role, StateSet, TextReply, TreeNode};
/// When an event reached each point on its way through the listener and
/// the outpost: microseconds since the Unix epoch, 0 for a point it did not
/// pass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTiming {
    /// How many milliseconds before it was observed Windows raised the
    /// event; only `WinEvents` carry the time they were raised.
    pub raised_ms_ago: Option<u32>,
    /// When the listener or the outpost observed it: for a fact the
    /// listener sent, the observation stamp by which the fact is ordered
    /// against other observations.
    pub observed_at_us: u64,
    /// When the focus listener finished handling this fact and queued it
    /// for its writer: for a fact a reading thread judged (a UIA focus's or
    /// menu opening's read or drop, a selection's or notification's check),
    /// when that thread finished with it; for any other (a `WinEvent`'s
    /// fact, a marker, a drop decided without a read, a menu's end), when
    /// the listener queued it. 0 for anything the listener did not send.
    pub listener_handled_at_us: u64,
    /// When the focus listener began writing the frame that carries this
    /// fact to verbatim.exe's pipe, taken just before the frame is encoded;
    /// 0 for anything the listener did not send.
    pub listener_written_at_us: u64,
}

/// The time from the stamp `from_us` to the stamp `to_us`, in
/// microseconds, or `None` when either is 0, a point not stamped.
#[must_use]
pub fn hop_us(from_us: u64, to_us: u64) -> Option<u64> {
    (from_us != 0 && to_us != 0).then(|| to_us.saturating_sub(from_us))
}

/// The identity-free contents of a UIA focus element, as the focus listener
/// captures them (decision D13): the runtime id plus role, name, value,
/// states, and details — everything a [`NodeSnapshot`](crate::NodeSnapshot)
/// carries except its [`NodeId`](crate::NodeId). The node id is
/// deliberately absent: identity is minted per application inside that
/// application's own outpost and never crosses a process boundary, so the
/// receiving outpost mints it from the runtime id when it rebuilds the
/// snapshot. Mirrors `verbatim_uia::map::CachedUiaParts`, which is what the
/// listener reads by cached property access.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiaSnapshotFact {
    /// The UIA runtime id, the key the receiving outpost mints its node id from.
    pub runtime_id: Vec<i32>,
    /// Normalized role (already refined for toggle buttons).
    pub role: Role,
    /// Accessible name, if any.
    pub name: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Current states.
    pub states: StateSet,
    /// The optional properties beyond the core four.
    pub details: NodeDetails,
}

/// Messages from verbatim.exe to an outpost.
///
/// An outpost's target application is set once (decision D9: one
/// outpost per application, for its whole life): passed on its command
/// line, or, for a spare started before it had one, by
/// [`CoreToOutpost::Assign`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CoreToOutpost {
    /// Liveness probe; answered by [`OutpostToCore::Pong`].
    Ping {
        /// When verbatim.exe began writing this command, in microseconds
        /// since the Unix epoch ([`CoreToOutpost::stamp_sent`]).
        sent_at_us: u64,
    },
    /// The nodes from this outpost that Core still holds
    /// (`books/dev/src/reference/crates/verbatim-outpost.md`, held objects), as the numbers the
    /// outpost issued, and the position of the last message from this
    /// outpost that Core has handled ([`OutpostToCore::carries_nodes`] says
    /// which messages count). The outpost releases every other node it
    /// reported at or before that position, and answers a query for a
    /// released node "gone".
    NodesHeld {
        /// The held nodes' numbers.
        nodes: Vec<u64>,
        /// The text anchors Core holds in this outpost
        /// (`SrState::held_anchors` in `verbatim-core`): the outpost keeps
        /// these, and may forget any other once it has minted newer ones
        /// (the text protocol, `books/dev/src/reference/crates/verbatim-model.md`).
        anchors: Vec<u64>,
        /// The tokens of the caret tells' elements Core holds for this
        /// outpost: those its caret entries waiting or running name, for
        /// its caret read to adopt. The outpost keeps these.
        told: Vec<u64>,
        /// The position of the last message Core has handled.
        acknowledged: u64,
    },
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): answered
    /// [`OutpostToCore::IdleAnswer`] once the outpost's worker has handled
    /// everything that reached its intake before the query, with the caret
    /// those entries noted, and no caret key's watch or owed terminal read
    /// is open; behind every message it sent before, never merged and never
    /// ahead as urgent.
    IdleQuery {
        /// The round, echoed in the answer.
        round: u64,
    },
    /// Shut down cleanly and exit: take no new work, let the call or remote
    /// operation in progress finish (UIA's own timeouts end one the
    /// application never answers), remove every UIA event handler and
    /// `WinEvent` hook, release every object held, leave COM, and exit with
    /// code 0. The supervisor kills the process through its job only if it
    /// has not exited within its time limit. Nothing is answered: the pipe
    /// closing and the process exiting are the answer.
    Shutdown,
    /// To a spare outpost (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"): the
    /// application it watches from now on, for the rest of its life. It
    /// installs its process-scoped `WinEvent` hook for that application,
    /// starts waiting for its exit, and sends [`OutpostToCore::Ready`]; or,
    /// when the hook cannot be installed, reports the failure
    /// ([`OutpostToCore::Fault`]) and exits without `Ready`. Sent once,
    /// before any other command but a ping; an outpost started for an
    /// application is never sent it.
    Assign {
        /// The application.
        target_pid: Pid,
    },
    /// A request of the shared list, answered once by
    /// [`OutpostToCore::Answer`] on the outpost's one worker
    /// (`docs/design/readers-tell-core-asks-build1.md`, "The outpost in
    /// this build" and Table 14).
    Request(Box<crate::request::Ask>),
    /// Cancel a request: one queued is dropped and answered cancelled; one
    /// running is abandoned with its worker replaced when the foreground
    /// moved away, or else left alone.
    Cancel(crate::request::Cancel),
    /// A window of the outpost's application was destroyed, as the
    /// listener told Core (`docs/design/readers-tell-core-asks-build1.md`,
    /// "Changes on objects"): the outpost answers "not there" the queued
    /// reads whose MSAA ids name it and the watches waiting on it, and
    /// forgets its MSAA nodes, text facts and classic-read mark before any
    /// later work. A followed node's registration is
    /// Core's to end, with a `Follow` naming nothing.
    WindowDestroyed(crate::WindowHandle),
}

impl CoreToOutpost {
    /// Stamps a ping with `now`, read just before its write begins. Reads
    /// no clock for any other command.
    pub fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        if let CoreToOutpost::Ping { sent_at_us, .. } = self {
            *sent_at_us = now();
        }
    }

    /// When verbatim.exe began writing this command, for a ping; 0 for any
    /// other.
    #[must_use]
    pub fn sent_at_us(&self) -> u64 {
        if let CoreToOutpost::Ping { sent_at_us, .. } = self {
            *sent_at_us
        } else {
            0
        }
    }
}

/// Messages from verbatim.exe to the focus listener.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoreToListener {
    /// `pid`'s process has exited: the supervisor holds its handle until
    /// [`ListenerToCore::ExitHandled`] answers, so no new process can have
    /// its id until the listener has moved its record. Sent once per held
    /// process that exits.
    ApplicationExited {
        /// The process that exited.
        pid: Pid,
    },
    /// Liveness probe; answered by [`ListenerToCore::Pong`].
    Ping {
        /// When verbatim.exe began writing this command to the listener's
        /// pipe, in microseconds since the Unix epoch
        /// ([`CoreToListener::stamp_sent`]).
        sent_at_us: u64,
    },
    /// The harness's idle barrier, round `round`
    /// (`docs/design/communication.md`, section 6.7): answered
    /// [`ListenerToCore::IdleAnswer`] once the listener's `WinEvent` thread
    /// and its reading threads have nothing in progress, behind every
    /// message it sent before.
    IdleQuery {
        /// The round, echoed in the answer.
        round: u64,
    },
    /// Shut down cleanly and exit, as an outpost does
    /// ([`CoreToOutpost::Shutdown`]).
    Shutdown,
    /// A request of the shared list, answered once by
    /// [`ListenerToCore::Answer`] (`docs/design/readers-tell-core-asks-build1.md`,
    /// "The listener in this build" and Table 9).
    Request(Box<crate::request::Ask>),
    /// Cancel a request: one queued is dropped and answered cancelled; one
    /// running is abandoned for the foreground, or else left alone.
    Cancel(crate::request::Cancel),
    /// The elements Core holds of those the listener keeps for its tells
    /// (design of the listener group, "Told elements released by Core's
    /// held view"): the listener lets go of every kept element absent from
    /// `tokens` whose tell was written at or before `acknowledged`, the
    /// position of the last message carrying tokens Core has handled
    /// ([`ListenerToCore::carries_tokens`]). A view naming another
    /// listener generation is dropped.
    ElementsHeld {
        /// The listener generation it is for.
        generation: u64,
        /// The tokens Core holds.
        tokens: Vec<crate::tell::ElementToken>,
        /// The position of the last message carrying tokens Core handled.
        acknowledged: u64,
    },
}

impl CoreToListener {
    /// Stamps a ping or a hand-over notice with `now`, read just before the
    /// command's write begins. Reads no clock for any other command.
    pub fn stamp_sent(&mut self, now: impl FnOnce() -> u64) {
        match self {
            CoreToListener::Ping { sent_at_us, .. } => *sent_at_us = now(),
            CoreToListener::IdleQuery { .. }
            | CoreToListener::ApplicationExited { .. }
            | CoreToListener::Shutdown
            | CoreToListener::Request(_)
            | CoreToListener::Cancel(_)
            | CoreToListener::ElementsHeld { .. } => {}
        }
    }

    /// When verbatim.exe began writing this command, for one
    /// [`stamp_sent`](Self::stamp_sent) stamps; 0 for any other.
    #[must_use]
    pub fn sent_at_us(&self) -> u64 {
        match self {
            CoreToListener::Ping { sent_at_us, .. } => *sent_at_us,
            CoreToListener::IdleQuery { .. }
            | CoreToListener::ApplicationExited { .. }
            | CoreToListener::Shutdown
            | CoreToListener::Request(_)
            | CoreToListener::Cancel(_)
            | CoreToListener::ElementsHeld { .. } => 0,
        }
    }
}

/// Messages from an outpost to verbatim.exe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum OutpostToCore {
    /// The outpost has its application and its `WinEvent` hook is in: the
    /// first message of an outpost started for an application, and the
    /// answer to [`CoreToOutpost::Assign`] of a spare.
    Ready {
        /// The outpost's own process id.
        outpost_pid: Pid,
        /// The application it is watching.
        target_pid: Pid,
    },
    /// Answer to [`CoreToOutpost::Ping`].
    Pong {
        /// How many of this outpost's workers have been abandoned and have
        /// not yet returned: Core's generation record ends an outpost that
        /// piles them up.
        parked_count: usize,
        /// When the writer began writing this pong to verbatim.exe's pipe,
        /// in microseconds since the Unix epoch.
        written_at_us: u64,
    },
    /// A backend error worth reporting without dying — a failed event
    /// registration, an arbitration probe that keeps timing out.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
    /// The outpost's target application has exited. Sent once, ahead of
    /// ordinary messages; the supervisor then shuts the outpost down
    /// ([`CoreToOutpost::Shutdown`]), as it has nothing left to
    /// watch.
    TargetExited,
    /// A spare outpost has done everything of its start that needs no
    /// application, and waits for [`CoreToOutpost::Assign`]
    /// (`books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"). Its first message.
    SpareReady {
        /// The outpost's own process id.
        outpost_pid: Pid,
    },
    /// The answer to [`CoreToOutpost::IdleQuery`] for `round`, sent
    /// behind every message sent before it: never sent ahead as urgent,
    /// and nothing is merged across it.
    IdleAnswer {
        /// The round answered.
        round: u64,
    },
    /// A tell (ADR 0003, "Readers tell, Core asks"): an event the outpost's
    /// `Follow` registration delivered, with the value it carried. Sent in
    /// order with the outpost's answers.
    Tell(Box<crate::tell::Event>),
    /// The answer to a [`CoreToOutpost::Request`], one per request.
    Answer(crate::request::Answer),
    /// A window of the outpost's application whose event it heard while
    /// the window was hung, read as the event arrived with calls to
    /// Windows only: sent once per stall of the window's thread
    /// ([`crate::hung::Marks`]), before the event's own tell.
    Hung(crate::tell::Hung),
}

/// Messages from the focus listener to verbatim.exe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ListenerToCore {
    /// The listener has made its desktop-wide UIA subscriptions, or failed
    /// to: its first message after its start's facts.
    Ready {
        /// The listener's own process id.
        outpost_pid: Pid,
    },
    /// The listener's answer to [`CoreToListener::ApplicationExited`]: it
    /// has moved `pid`'s record, so the supervisor closes the process's
    /// handle.
    ExitHandled {
        /// The process that exited.
        pid: Pid,
    },
    /// A window came to the system's foreground: the only report of one,
    /// from which Core takes the keyboard hook's view of the foreground
    /// (architecture section 5), attention and its foreground record. Sent
    /// only by the listener, for every foreground `WinEvent` whose window
    /// `GetForegroundWindow` names, Program Manager's, the taskbar's and an
    /// ignored process's included, whatever process raised it and whether
    /// or not its fact is forwarded; for one whose window was not in front,
    /// once a later focus `WinEvent` finds it there, unless a
    /// newer foreground event came first; once when the listener starts,
    /// for the window then in front; and again for a console window in
    /// front whose thread no event had named, once one of its events names
    /// it. None replaces another. The supervisor passes it to the app,
    /// which gives it to Core
    /// (`books/dev/src/reference/adr/0001-focus.md`, "The foreground and the window title").
    Foreground {
        /// The window.
        window: isize,
        /// The top of its owner chain (`GetAncestor` with `GA_ROOTOWNER`).
        root_owner: isize,
        /// The process its facts go to: its owner, or for a console window
        /// whose thread an event named, the console host.
        process: u32,
        /// When the listener's callback observed the event that sent it,
        /// in microseconds on the run's shared clock.
        observed_at_us: u64,
        /// The thread that raised its event, the console host's own for a
        /// console window; 0 for a console window whose thread no event
        /// has named yet.
        thread: u32,
        /// Whether it is a console host window.
        console: bool,
        /// The integrity level of the process that owns the window, or
        /// `None` when it cannot be read.
        integrity: Option<u32>,
    },
    /// A tell (ADR 0003, "Readers tell, Core asks"): an event the
    /// listener heard, with what arrived with it and the facts it gathered
    /// with calls to Windows only, after its local drops, its merge and its
    /// foreground hold. Today's routed facts are sent beside it until the
    /// build removes them.
    Tell(Box<crate::tell::Event>),
    /// A window was destroyed: every `EVENT_OBJECT_DESTROY` of a window
    /// object the listener hears, held in the order of its tells through
    /// the foreground hold and never merged, so Core clears what it keeps
    /// of the window and its outpost forgets it, as NVDA handles a
    /// destroy in its hook's callback
    /// (`IAccessibleHandler/internalWinEventHandler.py` 101 to 103). A
    /// destroy is never dropped for its process.
    Destroyed {
        /// The window.
        window: crate::WindowHandle,
        /// The process of the thread that raised it, the outpost that
        /// forgets the window; `None` when that thread cannot be opened,
        /// as a ghost's (the desktop window manager's) or an exited
        /// thread's cannot.
        process: Option<Pid>,
        /// When the listener's callback observed it, in microseconds on
        /// the run's shared clock.
        at_us: u64,
    },
    /// The answer to a [`CoreToListener::Request`], one per request.
    Answer(crate::request::Answer),
    /// A window whose event the listener dropped as hung, a ghost reported
    /// as the window it stands for: sent once per stall of the window's
    /// thread ([`crate::hung::Marks`]), held in the order of its tells
    /// behind any pending foreground.
    Hung(crate::tell::Hung),
    /// Answer to [`CoreToListener::Ping`].
    Pong {
        /// When the writer began writing this pong to verbatim.exe's pipe,
        /// in microseconds since the Unix epoch.
        written_at_us: u64,
    },
    /// A fault worth reporting without dying, logged by verbatim.exe.
    Fault {
        /// Human-readable detail for logs and diagnostics.
        detail: String,
    },
    /// The answer to [`CoreToListener::IdleQuery`] for `round`, sent
    /// behind every message sent before it.
    IdleAnswer {
        /// The round answered.
        round: u64,
    },
}

impl OutpostToCore {
    /// Whether this message counts toward the message positions that
    /// [`CoreToOutpost::NodesHeld`] acknowledges: an answer with a result,
    /// the message that carries node ids, and a tell naming an element by
    /// its token, which the outpost keeps for Core's caret read. The
    /// outpost and Core count the same messages, in pipe order, with this
    /// one function, so every variant is listed: one that carries what the
    /// outpost keeps and is not counted would let the outpost release it
    /// while Core still names it.
    #[must_use]
    pub fn carries_nodes(&self) -> bool {
        match self {
            OutpostToCore::Answer(answer) => {
                matches!(answer.outcome, crate::request::Outcome::Result(_))
            }
            OutpostToCore::Tell(tell) => {
                matches!(tell.object, crate::tell::EventObject::Uia { .. })
            }
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. }
            | OutpostToCore::Hung(_) => false,
        }
    }

    /// Records, in a pong, when its writer began writing it to
    /// verbatim.exe's pipe. Nothing for any other message.
    pub fn written(&mut self, at_us: u64) {
        if let OutpostToCore::Pong { written_at_us, .. } = self {
            *written_at_us = at_us;
        }
    }

    /// Stamps `outpost` on every node id this message carries. Core applies
    /// it to everything arriving on an outpost's pipe, so node ids name their
    /// outpost generation and never come from the message body.
    pub fn assign_outpost(&mut self, outpost: OutpostId) {
        match self {
            OutpostToCore::Answer(answer) => answer.assign_outpost(outpost),
            OutpostToCore::Tell(tell) => tell.assign_outpost(outpost),
            OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. }
            | OutpostToCore::Hung(_) => {}
        }
    }

    /// Every node id and text anchor this message names: the nodes
    /// [`assign_outpost`](Self::assign_outpost) stamps in an answer, and
    /// the anchors of its text. Empty for a message that names none.
    #[must_use]
    pub fn named(&self) -> Named {
        let mut named = Named::default();
        match self {
            OutpostToCore::Answer(answer) => named.answer(answer),
            OutpostToCore::Tell(_)
            | OutpostToCore::Ready { .. }
            | OutpostToCore::SpareReady { .. }
            | OutpostToCore::Pong { .. }
            | OutpostToCore::Fault { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::IdleAnswer { .. }
            | OutpostToCore::Hung(_) => {}
        }
        named
    }
}

/// The node ids and text anchors a message names
/// ([`OutpostToCore::named`]): what an outpost records as carried by the
/// message, so it keeps each until Core has seen the message and does not
/// hold it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// The nodes, in the order the message names them, repeats kept.
    pub nodes: Vec<NodeId>,
    /// The text anchors, likewise.
    pub anchors: Vec<crate::TextAnchor>,
}

impl Named {
    fn answer(&mut self, answer: &crate::request::Answer) {
        let (nodes, anchors) = answer.named();
        self.nodes.extend(nodes);
        self.anchors.extend(anchors);
        if let crate::request::Outcome::Result(crate::request::Reply::Text(reply)) = &answer.outcome
        {
            self.text_reply(reply);
        }
    }

    fn chunk(&mut self, chunk: &crate::TextChunk) {
        self.anchors.push(chunk.start);
    }

    fn caret(&mut self, caret: &crate::CaretReport) {
        self.chunk(&caret.line);
        if let Some(selection) = &caret.selection {
            self.selection(selection);
        }
    }

    fn selection(&mut self, selection: &crate::CaretSelection) {
        self.anchors.push(selection.range.start.anchor);
        self.anchors.push(selection.range.end.anchor);
    }

    fn text_reply(&mut self, reply: &TextReply) {
        match reply {
            TextReply::TerminalRead(read) => {
                self.anchors.push(read.start);
                self.anchors.extend(read.point);
                if let Some(caret) = &read.snapshot.caret {
                    self.caret(caret);
                }
            }
            TextReply::TerminalHistory { .. }
            | TextReply::TerminalFound { .. }
            | TextReply::UnsupportedUnit(_)
            | TextReply::Unsupported
            | TextReply::NoText
            | TextReply::AnchorLost
            | TextReply::Gone
            | TextReply::Unanswered => {}
        }
    }
}

impl ListenerToCore {
    /// The token of the kept element this message names, when it is a
    /// listener tell naming one: the messages the listener numbers as it
    /// writes them and Core counts as it handles them, so that a view's
    /// `acknowledged` position names a point in the listener's stream
    /// ([`CoreToListener::ElementsHeld`]). Every variant is listed: one
    /// that names a kept element and is not counted would let the listener
    /// release an element Core still holds.
    #[must_use]
    pub fn carried_token(&self) -> Option<crate::tell::ElementToken> {
        match self {
            ListenerToCore::Tell(tell) => match &tell.object {
                crate::tell::EventObject::Uia { token, .. } => Some(*token),
                crate::tell::EventObject::Msaa { .. }
                | crate::tell::EventObject::Followed { .. }
                | crate::tell::EventObject::Terminal { .. } => None,
            },
            ListenerToCore::Ready { .. }
            | ListenerToCore::Pong { .. }
            | ListenerToCore::Fault { .. }
            | ListenerToCore::Foreground { .. }
            | ListenerToCore::Destroyed { .. }
            | ListenerToCore::Hung(_)
            | ListenerToCore::Answer(_)
            | ListenerToCore::IdleAnswer { .. }
            | ListenerToCore::ExitHandled { .. } => None,
        }
    }

    /// Whether this message carries a kept element's token
    /// ([`carried_token`](Self::carried_token)).
    #[must_use]
    pub fn carries_tokens(&self) -> bool {
        self.carried_token().is_some()
    }

    /// Records when its writer began writing it to verbatim.exe's pipe: a
    /// pong's. Nothing for any other message.
    pub fn written(&mut self, at_us: u64) {
        match self {
            ListenerToCore::Pong { written_at_us, .. } => *written_at_us = at_us,
            ListenerToCore::Ready { .. }
            | ListenerToCore::ExitHandled { .. }
            | ListenerToCore::Foreground { .. }
            | ListenerToCore::Destroyed { .. }
            | ListenerToCore::Hung(_)
            | ListenerToCore::Tell(_)
            | ListenerToCore::Answer(_)
            | ListenerToCore::Fault { .. }
            | ListenerToCore::IdleAnswer { .. } => {}
        }
    }
}

/// A message an outpost sends ahead of everything waiting in its outbound
/// queue: none carries a node, so none can skip the outpost's numbering of
/// the messages that do ([`OutpostToCore::carries_nodes`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Urgent {
    /// [`OutpostToCore::Pong`].
    Pong {
        /// How many of the outpost's workers are abandoned.
        parked_count: usize,
    },
    /// [`OutpostToCore::Ready`].
    Ready {
        /// The outpost's own process id.
        outpost_pid: Pid,
        /// The application it is watching.
        target_pid: Pid,
    },
    /// [`OutpostToCore::TargetExited`].
    TargetExited,
    /// [`OutpostToCore::SpareReady`].
    SpareReady {
        /// The outpost's own process id.
        outpost_pid: Pid,
    },
}

impl From<Urgent> for OutpostToCore {
    fn from(urgent: Urgent) -> Self {
        match urgent {
            Urgent::Pong { parked_count } => OutpostToCore::Pong {
                parked_count,
                written_at_us: 0,
            },
            Urgent::Ready {
                outpost_pid,
                target_pid,
            } => OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            },
            Urgent::TargetExited => OutpostToCore::TargetExited,
            Urgent::SpareReady { outpost_pid } => OutpostToCore::SpareReady { outpost_pid },
        }
    }
}

/// One completed tree walk from a target application's top-level window
/// (architecture section 1). The walk is bounded by a depth cap of 64 and a
/// node-count cap of 4096; `truncated` notes when it is incomplete: it
/// stopped early against either cap rather than reaching every node, or an
/// element gone during the walk left the part of the tree under it unread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpedTree {
    /// The root node reached.
    pub root: TreeNode,
    /// Whether the tree is incomplete, against a cap or at a gone element.
    pub truncated: bool,
    /// Whether each node's location and whether it is off the screen were
    /// read. When not, every node's rectangle is `None` and its states
    /// never hold `Offscreen`, which then says nothing of where it is.
    pub locations: bool,
}

#[cfg(test)]
mod tests;
