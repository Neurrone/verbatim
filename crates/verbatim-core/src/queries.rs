//! Core's queries: every request the reducer makes for a node's text, a
//! navigation or an activation, whether an outpost answers it or Core
//! answers it from a terminal's memory, recorded in one table under one
//! numbering from its request to its one ending, with what its answer
//! continues (`books/dev/src/reference/crates/verbatim-core.md`, "Core's queries").
//!
//! A query is asked by [`Queries::ask`], which records it and returns the
//! request's effect, or by [`Queries::ask_memory`] for one only Core
//! answers. It ends exactly once, by one of four transitions: the outpost's
//! answer ([`answered`]), its outpost's end ([`outpost_gone`]), its owner's
//! withdrawal ([`Queries::withdraw`]), or Core's answer from a terminal's
//! memory ([`answer_from_memory`]), or the removal of that terminal's model
//! for one Core answers. Its continuation runs then, once, routed by the
//! entry. A query superseded or cleared is turned to [`Then::Nothing`]
//! ([`Queries::forget`]), so its answer still ends it and runs nothing. An
//! answer that finds no entry, or comes from another outpost than the one
//! asked, is dropped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::protocol::{QueryOutcome, QueryResult};
use verbatim_model::{
    Effect, FetchResult, NodeId, OutpostId, Query, QueryId, QueryKind, TextOp, TextReply,
    TextRequest, TraceId,
};

use crate::state::{HeldRead, PendingCaret, PendingText, SrState};

/// Who answers a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Asked {
    /// The outpost, to a read the idle barrier waits for.
    Read,
    /// The outpost, to a caret key's watch, which waits for the
    /// application and which the idle barrier does not wait for.
    Watch,
    /// Core, from a terminal's memory: never sent, and never waited for.
    Memory,
}

/// What the answer to a query continues.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Then {
    /// A navigation of the navigator object from `from`.
    Navigation { from: NodeId, kind: QueryKind },
    /// An activation of the navigator object, whose outcome is spoken.
    Activation,
    /// A caret key's watch, waiting for its evidence.
    Caret(PendingCaret),
    /// A caret key's watch a later key on the same node superseded, whose
    /// answer may still carry a change of the selection.
    SupersededCaret { node: NodeId },
    /// The read of the caret before a held key.
    HeldRead(HeldRead),
    /// A review or text command's request.
    Text(PendingText),
    /// Say-all's read.
    SayAll,
    /// The flood policy's request of the focused terminal.
    Terminal(TerminalAsk),
    /// A terminal model's request, for the model of `node` made as
    /// `generation`.
    TerminalModel {
        node: NodeId,
        generation: u64,
        ask: ModelAsk,
    },
    /// Nothing: superseded, cleared, or never awaited.
    Nothing,
}

/// Which of the flood policy's requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum TerminalAsk {
    /// What is new.
    Read,
    /// The cancel sent to the outpost.
    Cancel,
    /// What is new, asked before a cancel overtook it.
    Overtaken,
}

/// What a terminal model asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ModelAsk {
    /// The whole document, after a lost read, which the model is owed.
    Whole,
    /// A first read again with its history block, which the model is owed.
    HistoryBlock,
    /// The fresh read asked for before a review answer.
    Fresh,
    /// The history read asked for by the review answer in progress.
    History(crate::terminal_model::HistoryAsked),
    /// The search asked for by the command mark move in progress, for the
    /// line whose hash this is.
    Find { hash: u64 },
}

impl ModelAsk {
    /// Whether it is a read the model is owed.
    pub(crate) fn owed(self) -> bool {
        matches!(self, Self::Whole | Self::HistoryBlock)
    }
}

/// One query not yet ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Open {
    /// The outpost asked: the node's.
    pub(crate) outpost: OutpostId,
    /// The trace the request runs under, set when the reducer's transition
    /// ends ([`Queries::set_traces`]); `None` for one asked under an input
    /// with no trace.
    pub(crate) trace_id: Option<TraceId>,
    pub(crate) asked: Asked,
    pub(crate) then: Then,
}

/// What a query asks of its node.
pub(crate) enum Asking {
    /// A navigation step from the node.
    Navigate(NodeId, QueryKind),
    /// A request about the node's text.
    Text(NodeId, TextOp),
    /// An activation of the node.
    Activate(NodeId),
}

/// Core's open queries, by number.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Queries {
    next: u64,
    open: BTreeMap<QueryId, Open>,
}

impl Queries {
    /// Records the query `asking` describes, whose answer continues
    /// `then`, and returns its number and the request's effect, which
    /// names it: a caret key's watch as a [`Asked::Watch`], anything else
    /// as a [`Asked::Read`].
    pub(crate) fn ask(&mut self, asking: Asking, then: Then) -> (QueryId, Effect) {
        let query_id = QueryId(self.next);
        self.next += 1;
        let (node_id, asked, effect) = match asking {
            Asking::Navigate(node_id, kind) => (
                node_id,
                Asked::Read,
                Effect::Fetch(Query {
                    query_id,
                    node_id,
                    kind,
                }),
            ),
            Asking::Text(node_id, op) => (
                node_id,
                if matches!(op, TextOp::AwaitCaret(_)) {
                    Asked::Watch
                } else {
                    Asked::Read
                },
                Effect::Text(TextRequest {
                    query_id,
                    node_id,
                    op,
                }),
            ),
            Asking::Activate(node_id) => {
                (node_id, Asked::Read, Effect::Activate { query_id, node_id })
            }
        };
        self.open.insert(
            query_id,
            Open {
                outpost: node_id.outpost(),
                trace_id: None,
                asked,
                then,
            },
        );
        (query_id, effect)
    }

    /// Records a query about `node`'s text that only Core answers, from a
    /// terminal's memory, whose answer continues `then`, and returns its
    /// number: no request is made.
    pub(crate) fn ask_memory(&mut self, node: NodeId, then: Then) -> QueryId {
        let query_id = QueryId(self.next);
        self.next += 1;
        self.open.insert(
            query_id,
            Open {
                outpost: node.outpost(),
                trace_id: None,
                asked: Asked::Memory,
                then,
            },
        );
        query_id
    }

    /// Core takes query `query_id`'s request out of the effects and
    /// answers it from a terminal's memory: it is not sent, and not waited
    /// for. It keeps `trace`, the trace its request would have been sent
    /// under, since no effect is left to give it one.
    pub(crate) fn answer_from_memory_later(&mut self, query_id: QueryId, trace: Option<TraceId>) {
        if let Some(open) = self.open.get_mut(&query_id) {
            open.asked = Asked::Memory;
            open.trace_id = trace;
        }
    }

    /// The first open query, in number order, whose continuation `which`
    /// picks: its number and continuation. A query turned to
    /// [`Then::Nothing`] is never picked.
    pub(crate) fn find(&self, which: impl Fn(&Then) -> bool) -> Option<(QueryId, &Then)> {
        self.open
            .iter()
            .find(|(_, open)| open.then != Then::Nothing && which(&open.then))
            .map(|(query_id, open)| (*query_id, &open.then))
    }

    /// Whether an open query's continuation is one `which` picks.
    pub(crate) fn has(&self, which: impl Fn(&Then) -> bool) -> bool {
        self.find(which).is_some()
    }

    /// The continuation of the first open query `which` picks, to change.
    pub(crate) fn find_mut(&mut self, which: impl Fn(&Then) -> bool) -> Option<&mut Then> {
        self.open
            .values_mut()
            .find(|open| open.then != Then::Nothing && which(&open.then))
            .map(|open| &mut open.then)
    }

    /// Gives open query `query_id` the continuation `then`, returning the
    /// one it had; nothing for a query not open.
    pub(crate) fn replace(&mut self, query_id: QueryId, then: Then) -> Option<Then> {
        self.open
            .get_mut(&query_id)
            .map(|open| std::mem::replace(&mut open.then, then))
    }

    /// Turns every open query whose continuation `which` picks to
    /// [`Then::Nothing`], superseded or cleared: one an outpost answers
    /// keeps its entry until its answer, which runs nothing; one Core
    /// answers from memory, which nothing else would end, is removed.
    pub(crate) fn forget(&mut self, which: impl Fn(&Then) -> bool) {
        self.open.retain(|_, open| {
            if open.then == Then::Nothing || !which(&open.then) {
                return true;
            }
            open.then = Then::Nothing;
            open.asked != Asked::Memory
        });
    }

    /// Ends query `query_id` with no answer, as its owner gives up on it,
    /// returning its continuation and the withdrawal's effect for its
    /// outpost; nothing for a query already ended.
    pub(crate) fn withdraw(&mut self, query_id: QueryId) -> Option<(Then, Effect)> {
        let open = self.open.remove(&query_id)?;
        Some((
            open.then,
            Effect::Withdraw {
                outpost: open.outpost,
                query_id,
            },
        ))
    }

    /// Ends the queries Core answers from memory among `query_ids`, whose
    /// terminal's model is gone, with no continuation.
    pub(crate) fn forget_memory(&mut self, query_ids: impl IntoIterator<Item = QueryId>) {
        for query_id in query_ids {
            if self
                .open
                .get(&query_id)
                .is_some_and(|open| open.asked == Asked::Memory)
            {
                self.open.remove(&query_id);
            }
        }
    }

    /// How many queries the idle barrier waits for: those an outpost
    /// answers that are not a caret key's watch.
    pub(crate) fn outstanding(&self) -> usize {
        self.open
            .values()
            .filter(|open| open.asked == Asked::Read)
            .count()
    }

    /// The continuations of the open queries, in number order, for the
    /// nodes they hold.
    pub(crate) fn continuations(&self) -> impl Iterator<Item = &Then> {
        self.open.values().map(|open| &open.then)
    }

    /// The next number to be given, which [`set_traces`](Self::set_traces)
    /// takes as the first of a transition's queries.
    pub(crate) fn next(&self) -> u64 {
        self.next
    }

    /// Sets the trace of each query numbered `first` or later among
    /// `effects`, the requests a transition made, to the trace the shell
    /// runs it under: the key's inside [`Effect::ForKey`], else the
    /// input's, `input`.
    pub(crate) fn set_traces(&mut self, first: u64, effects: &[Effect], input: Option<TraceId>) {
        if self.next == first {
            return;
        }
        for effect in effects {
            self.set_trace(first, effect, input);
        }
    }

    fn set_trace(&mut self, first: u64, effect: &Effect, trace: Option<TraceId>) {
        let query_id = match effect {
            Effect::ForKey(key, inner) => return self.set_trace(first, inner, Some(*key)),
            Effect::Fetch(Query { query_id, .. })
            | Effect::Text(TextRequest { query_id, .. })
            | Effect::Activate { query_id, .. } => *query_id,
            _ => return,
        };
        if query_id.0 >= first
            && let Some(open) = self.open.get_mut(&query_id)
        {
            open.trace_id = trace;
        }
    }

    /// Ends query `query_id` with no continuation run, for a test that
    /// runs it itself, returning it.
    #[cfg(test)]
    pub(crate) fn end(&mut self, query_id: QueryId) -> Option<Then> {
        self.open.remove(&query_id).map(|open| open.then)
    }

    /// The numbers of the open queries, in order.
    #[cfg(test)]
    pub(crate) fn numbers(&self) -> impl Iterator<Item = QueryId> + '_ {
        self.open.keys().copied()
    }

    /// The open queries, in number order.
    #[cfg(test)]
    pub(crate) fn entries(&self) -> impl Iterator<Item = (QueryId, &Open)> {
        self.open.iter().map(|(query_id, open)| (*query_id, open))
    }
}

/// What a query's ending gives its continuation: an outpost's outcome, or
/// Core's own answer.
#[expect(
    clippy::large_enum_variant,
    reason = "an ending is built once and moved into its continuation"
)]
pub(crate) enum Ending {
    Outcome(QueryOutcome),
    Reply(TextReply),
}

impl Ending {
    /// The text answer this ending is, a failure of any kind as
    /// "unanswered" and a gone node as gone.
    fn reply(self) -> TextReply {
        match self {
            Self::Reply(reply) | Self::Outcome(QueryOutcome::Done(QueryResult::Text(reply))) => {
                reply
            }
            Self::Outcome(QueryOutcome::Gone) => TextReply::Gone,
            Self::Outcome(_) => TextReply::Unanswered,
        }
    }
}

/// The outpost's answer to query `query_id`, from `outpost`, under the
/// reply's trace: ends the query and runs what it continues. An answer to
/// no open query, from another outpost than the one asked, or to a query
/// only Core answers, is dropped.
pub(crate) fn answered(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    outpost: OutpostId,
    outcome: &QueryOutcome,
) -> Vec<Effect> {
    if state
        .queries
        .open
        .get(&query_id)
        .is_none_or(|open| open.outpost != outpost || open.asked == Asked::Memory)
    {
        return Vec::new();
    }
    let Some(open) = state.queries.open.remove(&query_id) else {
        return Vec::new();
    };
    ended(
        state,
        trace_id,
        query_id,
        open.then,
        Ending::Outcome(outcome.clone()),
    )
}

/// Core's answer `reply` from a terminal's memory to query `query_id`,
/// under `trace_id`: ends the query and runs what it continues.
pub(crate) fn answer_from_memory(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    reply: TextReply,
) -> Vec<Effect> {
    let Some(open) = state.queries.open.remove(&query_id) else {
        return Vec::new();
    };
    ended(state, trace_id, query_id, open.then, Ending::Reply(reply))
}

/// Ends every query still open at `outpost`, whose end Core has handled,
/// "gone", in number order, each under its own trace or else `trace_id`,
/// the end's.
pub(crate) fn outpost_gone(
    state: &mut SrState,
    trace_id: TraceId,
    outpost: OutpostId,
) -> Vec<Effect> {
    let gone: Vec<QueryId> = state
        .queries
        .open
        .iter()
        .filter(|(_, open)| open.outpost == outpost && open.asked != Asked::Memory)
        .map(|(query_id, _)| *query_id)
        .collect();
    let mut effects = Vec::new();
    for query_id in gone {
        if let Some(open) = state.queries.open.remove(&query_id) {
            let trace = open.trace_id.unwrap_or(trace_id);
            effects.extend(ended(
                state,
                trace,
                query_id,
                open.then,
                Ending::Outcome(QueryOutcome::Gone),
            ));
        }
    }
    effects
}

/// Runs `then`, what ended query `query_id` continues, then resumes the
/// held keys when it was the watch they wait on, however it ended.
pub(crate) fn ended(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    then: Then,
    ending: Ending,
) -> Vec<Effect> {
    // The watch of the held key last passed on has its answer, whatever it
    // is: the held key waiting behind it may have its caret read now.
    let resume = state.held_keys.awaiting_watch == Some(query_id);
    if resume {
        state.held_keys.awaiting_watch = None;
    }
    let mut effects = continued(state, trace_id, then, ending);
    if resume {
        effects.extend(crate::editing::held_watch_answered(state));
    }
    effects
}

/// What `then` does with `ending`, its query's answer.
fn continued(state: &mut SrState, trace_id: TraceId, then: Then, ending: Ending) -> Vec<Effect> {
    match then {
        Then::Navigation { kind, .. } => {
            let result = match ending {
                Ending::Outcome(QueryOutcome::Done(QueryResult::Navigated(Some(node)))) => {
                    FetchResult::Node(node)
                }
                Ending::Outcome(QueryOutcome::Done(QueryResult::Navigated(None))) => {
                    FetchResult::NoNeighbor
                }
                Ending::Outcome(QueryOutcome::Gone) => FetchResult::Gone,
                _ => FetchResult::Unanswered,
            };
            crate::reduce::navigate_completed(state, trace_id, kind, &result)
        }
        Then::Activation => match ending {
            Ending::Outcome(QueryOutcome::Done(QueryResult::Activated(action))) => {
                crate::reduce::activation_completed(trace_id, true, action.as_ref())
            }
            // The activation started and passed its deadline: whether it
            // happened is unknown, so nothing is said either way. Whatever
            // it caused, such as a dialog taking focus, announces itself.
            Ending::Outcome(QueryOutcome::Abandoned) => Vec::new(),
            _ => crate::reduce::activation_completed(trace_id, false, None),
        },
        Then::HeldRead(reading) => {
            let reply = ending.reply();
            // In a terminal Core reads, the read before the key is a read
            // of the terminal, aligned first, whose caret answers it.
            if let TextReply::TerminalSnapshot(snapshot) = &reply {
                let (mut effects, caret) =
                    crate::terminal_model::before_key(state, trace_id, reading.node, snapshot);
                effects.extend(crate::editing::held_read(state, reading, caret));
                return effects;
            }
            crate::editing::held_read(state, reading, reply)
        }
        Then::Caret(pending) => {
            crate::editing::caret_reply(state, trace_id, &pending, ending.reply())
        }
        Then::SupersededCaret { node } => {
            crate::editing::superseded_reply(state, trace_id, node, &ending.reply())
        }
        Then::Text(pending) => {
            let reply = ending.reply();
            if matches!(
                pending.then,
                crate::state::TextFollowUp::NavigatorSelection(_)
                    | crate::state::TextFollowUp::NavigatorLine(_)
            ) {
                return crate::editing::navigator_text_reply(state, trace_id, &pending, reply);
            }
            crate::review_text::reply(state, trace_id, pending, reply)
        }
        Then::SayAll => crate::say_all::reply(state, trace_id, ending.reply()),
        Then::Terminal(ask) => crate::terminal::reply(state, trace_id, ask, &ending.reply()),
        Then::TerminalModel {
            node,
            generation,
            ask,
        } => crate::terminal_model::reply(state, trace_id, (node, generation, ask), ending.reply()),
        Then::Nothing => Vec::new(),
    }
}

#[cfg(test)]
mod ended;
#[cfg(test)]
mod explore;
#[cfg(test)]
mod generated;
#[cfg(test)]
mod tests;
