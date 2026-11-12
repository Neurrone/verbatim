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
//! withdrawal ([`Queries::withdraw_kept`]), or Core's answer from a terminal's
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
    Effect, FetchResult, NodeId, OutpostId, Query, QueryId, QueryKind, TextChunk, TextOp,
    TextReply, TextRequest, TraceId,
};

use crate::driver::Core;
use crate::state::PendingText;

/// Who answers a query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Asked {
    /// The outpost, to a read the idle barrier waits for.
    Read,
    /// Core, from a terminal's memory: never sent, and never waited for.
    Memory,
}

/// What the answer to a query continues.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Then {
    /// An activation of the navigator object, whose outcome is spoken.
    Activation,
    /// A caret key's one request (`crate::key_line`), on `node`.
    Key { node: NodeId },
    /// A move of the caret Core made, whose answer gives the stamp of the
    /// caret it leaves.
    MovedCaret { node: NodeId, line: TextChunk },
    /// A text request no press waits for: the caret's location, the
    /// navigator's text for an announcement, Verbatim+Tab's line.
    Text(PendingText),
    /// A read a paused function waits for (`crate::driver`): its ending
    /// wakes the function, which holds what the answer continues.
    Task,
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
    /// A live read of the node.
    ReadNode(NodeId),
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
    /// names it, as an [`Asked::Read`].
    pub(crate) fn ask(&mut self, asking: Asking, then: Then) -> (QueryId, Effect) {
        let query_id = QueryId(self.next);
        self.next = self.next.saturating_add(1);
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
                Asked::Read,
                Effect::Text(TextRequest {
                    query_id,
                    node_id,
                    op,
                }),
            ),
            Asking::Activate(node_id) => {
                (node_id, Asked::Read, Effect::Activate { query_id, node_id })
            }
            Asking::ReadNode(node_id) => {
                (node_id, Asked::Read, Effect::ReadNode { query_id, node_id })
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
        self.next = self.next.saturating_add(1);
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

    /// Turns query `query_id` to [`Then::Nothing`] when it was asked of
    /// `outpost`, whose end is no answer to what waits for it.
    pub(crate) fn forget_at(&mut self, query_id: QueryId, outpost: OutpostId) {
        if let Some(open) = self.open.get_mut(&query_id)
            && open.outpost == outpost
            && open.then == Then::Task
        {
            open.then = Then::Nothing;
        }
    }

    /// Whether query `query_id` is still open.
    pub(crate) fn has_open(&self, query_id: QueryId) -> bool {
        self.open.contains_key(&query_id)
    }

    /// Turns query `query_id` to [`Then::Nothing`], its answer ending it
    /// and running nothing.
    pub(crate) fn forget_query(&mut self, query_id: QueryId) {
        if let Some(open) = self.open.get_mut(&query_id) {
            open.then = Then::Nothing;
        }
    }

    /// Withdraws query `query_id` from its outpost, its entry kept so that
    /// its one answer still runs its continuation: not started it is
    /// answered "not started", and already running it answers with what it
    /// read. Returns the withdrawal's effect.
    pub(crate) fn withdraw_kept(&self, query_id: QueryId) -> Option<Effect> {
        let open = self.open.get(&query_id)?;
        Some(Effect::Withdraw {
            outpost: open.outpost,
            query_id,
        })
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
    /// answers.
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
            | Effect::Activate { query_id, .. }
            | Effect::ReadNode { query_id, .. } => *query_id,
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
    /// The navigation result this ending is.
    pub(crate) fn navigated(self) -> FetchResult {
        match self {
            Self::Outcome(QueryOutcome::Done(QueryResult::Navigated(Some(node)))) => {
                FetchResult::Node(node)
            }
            Self::Outcome(QueryOutcome::Done(QueryResult::Navigated(None))) => {
                FetchResult::NoNeighbor
            }
            Self::Outcome(QueryOutcome::Gone) => FetchResult::Gone,
            _ => FetchResult::Unanswered,
        }
    }

    /// The text answer this ending is, a failure of any kind as
    /// "unanswered" and a gone node as gone.
    pub(crate) fn reply(self) -> TextReply {
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
    state: &mut Core,
    trace_id: TraceId,
    query_id: QueryId,
    outpost: OutpostId,
    outcome: &QueryOutcome,
    at_us: u64,
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
        at_us,
    )
}

/// Core's answer `reply` from a terminal's memory to query `query_id`,
/// under `trace_id`: ends the query and runs what it continues.
pub(crate) fn answer_from_memory(
    state: &mut Core,
    trace_id: TraceId,
    query_id: QueryId,
    reply: TextReply,
) -> Vec<Effect> {
    let Some(open) = state.queries.open.remove(&query_id) else {
        return Vec::new();
    };
    // No key's read is answered from memory, so no pass is made from this.
    ended(
        state,
        trace_id,
        query_id,
        open.then,
        Ending::Reply(reply),
        0,
    )
}

/// Ends every query still open at `outpost`, whose end Core has handled,
/// "gone", in number order, each under its own trace or else `trace_id`,
/// the end's.
pub(crate) fn outpost_gone(state: &mut Core, trace_id: TraceId, outpost: OutpostId) -> Vec<Effect> {
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
                // A gone read passes on no key with a time of its own.
                0,
            ));
        }
    }
    effects
}

/// Runs `then`, what ended query `query_id` continues.
pub(crate) fn ended(
    state: &mut Core,
    trace_id: TraceId,
    query_id: QueryId,
    then: Then,
    ending: Ending,
    at_us: u64,
) -> Vec<Effect> {
    match then {
        Then::Task => crate::driver::answered(state, query_id, ending, trace_id, at_us),
        Then::Activation => match ending {
            Ending::Outcome(QueryOutcome::Done(QueryResult::Activated(action))) => {
                crate::reduce::activation_completed(trace_id, true, action.as_ref())
            }
            // The activation was abandoned, at its deadline or with its
            // window judged hung, or refused because its window is hung:
            // whether it happened is unknown, so nothing is said either
            // way. Whatever it caused, such as a dialog taking focus,
            // announces itself.
            Ending::Outcome(QueryOutcome::Abandoned) => Vec::new(),
            _ => crate::reduce::activation_completed(trace_id, false, None),
        },
        Then::Key { node } => {
            let reply = ending.reply();
            // In a terminal Core reads, the read before a key is a read of
            // the terminal, aligned first, whose caret answers it.
            if let TextReply::TerminalSnapshot(snapshot) = &reply {
                let (mut effects, answer) =
                    crate::terminal_model::before_key(state, trace_id, node, snapshot);
                effects.extend(crate::key_line::request_answered(
                    state,
                    trace_id,
                    (query_id, node),
                    answer,
                    at_us,
                ));
                return effects;
            }
            crate::key_line::request_answered(state, trace_id, (query_id, node), reply, at_us)
        }
        Then::MovedCaret { node, line } => {
            if let TextReply::Moved { read_at_us } = ending.reply() {
                crate::editing::caret_moved_by_core(state, node, line, read_at_us);
            }
            Vec::new()
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
            crate::review_text::location(trace_id, &reply)
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
pub(crate) use explore::config as explore_config;
#[cfg(test)]
mod generated;
#[cfg(test)]
mod tests;
