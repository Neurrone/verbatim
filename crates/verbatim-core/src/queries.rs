//! Core's queries: every request the reducer makes for a node's text, a
//! navigation or an activation, whether an outpost answers it or Core
//! answers it from a terminal's memory, recorded in one table under one
//! numbering from its request to its one ending
//! (`docs/crates/verbatim-core.md`, "Core's queries").
//!
//! A query is asked by [`Queries::ask`], which records it and returns the
//! request's effect, or by [`Queries::ask_memory`] for one only Core
//! answers. It ends exactly once, by one of four transitions: the outpost's
//! answer ([`answered`]), its outpost's end ([`outpost_gone`]), its owner's
//! withdrawal ([`Queries::withdraw`]), or Core's answer from a terminal's
//! memory ([`answer_from_memory`]), or the removal of that terminal's model
//! for one Core answers. An answer that finds no entry, or comes from
//! another outpost than the one asked, is dropped.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::protocol::{QueryOutcome, QueryResult};
use verbatim_model::{
    Effect, FetchResult, NodeId, OutpostId, Query, QueryId, QueryKind, TextOp, TextReply,
    TextRequest, TraceId,
};

use crate::state::SrState;

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

/// What a query asked for, which decides the result its outcome becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Kind {
    /// A navigation from the navigator object.
    Navigation(QueryKind),
    /// An activation of the navigator object.
    Activation,
    /// A request about a node's text.
    Text,
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
    pub(crate) kind: Kind,
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
    /// Records the query `asking` describes and returns its number and the
    /// request's effect, which names it: a caret key's watch as a
    /// [`Asked::Watch`], anything else as a [`Asked::Read`].
    pub(crate) fn ask(&mut self, asking: Asking) -> (QueryId, Effect) {
        let query_id = QueryId(self.next);
        self.next += 1;
        let (node_id, asked, kind, effect) = match asking {
            Asking::Navigate(node_id, kind) => (
                node_id,
                Asked::Read,
                Kind::Navigation(kind),
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
                Kind::Text,
                Effect::Text(TextRequest {
                    query_id,
                    node_id,
                    op,
                }),
            ),
            Asking::Activate(node_id) => (
                node_id,
                Asked::Read,
                Kind::Activation,
                Effect::Activate { query_id, node_id },
            ),
        };
        self.open.insert(
            query_id,
            Open {
                outpost: node_id.outpost(),
                trace_id: None,
                asked,
                kind,
            },
        );
        (query_id, effect)
    }

    /// Records a query about `node`'s text that only Core answers, from a
    /// terminal's memory, and returns its number: no request is made.
    pub(crate) fn ask_memory(&mut self, node: NodeId) -> QueryId {
        let query_id = QueryId(self.next);
        self.next += 1;
        self.open.insert(
            query_id,
            Open {
                outpost: node.outpost(),
                trace_id: None,
                asked: Asked::Memory,
                kind: Kind::Text,
            },
        );
        query_id
    }

    /// Core takes query `query_id`'s request out of the effects and
    /// answers it from a terminal's memory: it is not sent, and not waited
    /// for.
    pub(crate) fn answer_from_memory_later(&mut self, query_id: QueryId) {
        if let Some(open) = self.open.get_mut(&query_id) {
            open.asked = Asked::Memory;
        }
    }

    /// Ends query `query_id` with no answer, as its owner gives up on it,
    /// returning the withdrawal's effect for its outpost; nothing for a
    /// query already ended.
    pub(crate) fn withdraw(&mut self, query_id: QueryId) -> Option<Effect> {
        let open = self.open.remove(&query_id)?;
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
    /// answers that are not a caret key's watch.
    pub(crate) fn outstanding(&self) -> usize {
        self.open
            .values()
            .filter(|open| open.asked == Asked::Read)
            .count()
    }

    /// The numbers of the open queries, in order.
    #[cfg(test)]
    pub(crate) fn numbers(&self) -> impl Iterator<Item = QueryId> + '_ {
        self.open.keys().copied()
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
}

/// What a query's ending gives its continuation: an outpost's outcome, or
/// Core's own answer from a terminal's memory.
#[expect(
    clippy::large_enum_variant,
    reason = "an ending is built once and moved into its continuation"
)]
enum Ending {
    Outcome(QueryOutcome),
    Memory(TextReply),
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
) -> Vec<verbatim_model::Effect> {
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
        open.kind,
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
    ended(state, trace_id, query_id, open.kind, Ending::Memory(reply))
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
                open.kind,
                Ending::Outcome(QueryOutcome::Gone),
            ));
        }
    }
    effects
}

/// Runs what ended query `query_id`, `open`, continues, then resumes the
/// held keys when it was the watch they wait on, however it ended.
fn ended(
    state: &mut SrState,
    trace_id: TraceId,
    query_id: QueryId,
    kind: Kind,
    ending: Ending,
) -> Vec<Effect> {
    // The watch of the held key last passed on has its answer, whatever it
    // is: the held key waiting behind it may have its caret read now.
    let resume = state.held_keys.awaiting_watch == Some(query_id);
    if resume {
        state.held_keys.awaiting_watch = None;
    }
    let mut effects = match (kind, ending) {
        (Kind::Navigation(kind), Ending::Outcome(outcome)) => {
            let result = match outcome {
                QueryOutcome::Done(QueryResult::Navigated(Some(node))) => FetchResult::Node(node),
                QueryOutcome::Done(QueryResult::Navigated(None)) => FetchResult::NoNeighbor,
                QueryOutcome::Gone => FetchResult::Gone,
                _ => FetchResult::Unanswered,
            };
            crate::reduce::navigate_completed(state, trace_id, query_id, kind, &result)
        }
        (Kind::Activation, Ending::Outcome(outcome)) => match outcome {
            QueryOutcome::Done(QueryResult::Activated(action)) => {
                crate::reduce::activation_completed(trace_id, true, action.as_ref())
            }
            // The activation started and passed its deadline: whether it
            // happened is unknown, so nothing is said either way. Whatever
            // it caused, such as a dialog taking focus, announces itself.
            QueryOutcome::Abandoned => Vec::new(),
            _ => crate::reduce::activation_completed(trace_id, false, None),
        },
        (Kind::Text, ending) => {
            let reply = match ending {
                Ending::Memory(reply)
                | Ending::Outcome(QueryOutcome::Done(QueryResult::Text(reply))) => reply,
                Ending::Outcome(QueryOutcome::Gone) => TextReply::Gone,
                Ending::Outcome(_) => TextReply::Unanswered,
            };
            crate::reduce::text_completed(state, trace_id, query_id, reply)
        }
        (Kind::Navigation(_) | Kind::Activation, Ending::Memory(_)) => Vec::new(),
    };
    if resume {
        effects.extend(crate::editing::held_watch_answered(state));
    }
    effects
}

#[cfg(test)]
mod tests;
