//! The wire types' kinds, the count of the messages that carry nodes, and
//! the pure methods of the messages.

use super::*;
use crate::{NodeDetails, NodeSnapshot, Role, StateSet};

/// Every sample a `next` function chains, from the first: each match of
/// `next` names every variant without a wildcard, so a new variant must
/// be given its place in the chain, and so in the list below it.
fn chain<T>(next: impl Fn(Option<&T>) -> Option<T>) -> Vec<T> {
    let mut samples: Vec<T> = Vec::new();
    while let Some(sample) = next(samples.last()) {
        samples.push(sample);
    }
    samples
}

/// The variant's name, from its debug form.
fn name(message: &impl std::fmt::Debug) -> String {
    format!("{message:?}")
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn timing() -> EventTiming {
    EventTiming::default()
}

/// One sample of each [`CoreToOutpost`] variant, after `previous`.
fn next_core_to_outpost(previous: Option<&CoreToOutpost>) -> Option<CoreToOutpost> {
    let Some(previous) = previous else {
        return Some(CoreToOutpost::SetBackendOverride {
            backend_override: None,
        });
    };
    Some(match previous {
        CoreToOutpost::SetBackendOverride { .. } => CoreToOutpost::DeliverFact {
            trace_id: TraceId::mint(),
            timing: timing(),
            fact: DeliveredFact::UiaFocusReadsEnded,
            win_event: None,
        },
        CoreToOutpost::DeliverFact { .. } => CoreToOutpost::ForwardingEnded { through: 1 },
        CoreToOutpost::ForwardingEnded { .. } => CoreToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id: 1,
            query: Query::FocusNow,
            timing: timing(),
        },
        CoreToOutpost::Query { .. } => CoreToOutpost::Cancel { request_id: 1 },
        CoreToOutpost::Cancel { .. } => CoreToOutpost::Ping {
            seq: 1,
            sent_at_us: 0,
        },
        CoreToOutpost::Ping { .. } => CoreToOutpost::Fetches(crate::Fetches::default()),
        CoreToOutpost::Fetches(_) => CoreToOutpost::NodesHeld {
            nodes: Vec::new(),
            anchors: Vec::new(),
            acknowledged: 0,
        },
        CoreToOutpost::NodesHeld { .. } => CoreToOutpost::IdleQuery { round: 1 },
        CoreToOutpost::IdleQuery { .. } => CoreToOutpost::Shutdown,
        CoreToOutpost::Shutdown => CoreToOutpost::Assign { target_pid: Pid(1) },
        CoreToOutpost::Assign { .. } => return None,
    })
}

/// One sample of each [`CoreToListener`] variant, after `previous`.
fn next_core_to_listener(previous: Option<&CoreToListener>) -> Option<CoreToListener> {
    let Some(previous) = previous else {
        return Some(CoreToListener::SetBackendOverride {
            backend_override: Some(Backend::Uia),
            pid: Pid(1),
        });
    };
    Some(match previous {
        CoreToListener::SetBackendOverride { .. } => CoreToListener::HookInstalled {
            pid: Pid(1),
            outpost: OutpostId(1),
            sent_at_us: 0,
        },
        CoreToListener::HookInstalled { .. } => CoreToListener::HookRemoved {
            pid: Pid(1),
            sent_at_us: 0,
        },
        CoreToListener::HookRemoved { .. } => CoreToListener::Ping {
            seq: 1,
            sent_at_us: 0,
        },
        CoreToListener::Ping { .. } => CoreToListener::IdleQuery { round: 1 },
        CoreToListener::IdleQuery { .. } => CoreToListener::Shutdown,
        CoreToListener::Shutdown => return None,
    })
}

/// A reply with `outcome`.
fn reply(outcome: QueryOutcome) -> OutpostToCore {
    OutpostToCore::Reply {
        trace_id: TraceId::mint(),
        request_id: 1,
        outcome,
        timing: timing(),
    }
}

/// One sample of each [`ListenerToCore`] variant, after `previous`.
fn next_listener_to_core(previous: Option<&ListenerToCore>) -> Option<ListenerToCore> {
    let Some(previous) = previous else {
        return Some(ListenerToCore::Ready {
            outpost_pid: Pid(1),
            target_pid: Pid(0),
        });
    };
    Some(match previous {
        ListenerToCore::Ready { .. } => ListenerToCore::FocusFact {
            trace_id: TraceId::mint(),
            timing: timing(),
            fact: ListenerFact {
                pid: Pid(2),
                fact: DeliveredFact::Foreground { hwnd: 1 },
            },
            win_event: None,
        },
        ListenerToCore::FocusFact { .. } => ListenerToCore::ForwardingEnded {
            pid: Pid(2),
            outpost: OutpostId(1),
            through: 1,
        },
        ListenerToCore::ForwardingEnded { .. } => ListenerToCore::MenuOrSwitchEnded {
            ended_at_us: 1,
            timing: timing(),
        },
        ListenerToCore::MenuOrSwitchEnded { .. } => ListenerToCore::ForegroundWindow {
            window: 1,
            thread: 1,
            console: false,
            integrity: None,
        },
        ListenerToCore::ForegroundWindow { .. } => ListenerToCore::Pong {
            seq: 1,
            parked_count: 0,
            written_at_us: 0,
        },
        ListenerToCore::Pong { .. } => ListenerToCore::Fault {
            detail: "a fault".to_owned(),
        },
        ListenerToCore::Fault { .. } => ListenerToCore::IdleAnswer { round: 1 },
        ListenerToCore::IdleAnswer { .. } => return None,
    })
}

/// The reviewed list of every variant's kind, one list per direction: a
/// new variant changes its enum's `next` function and its list, both
/// reviewed.
#[test]
fn every_variant_has_its_reviewed_kind() {
    let kinds = |names: Vec<(String, Kind)>| {
        names
            .into_iter()
            .map(|(name, kind)| format!("{name}: {kind:?}"))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        kinds(
            chain(next_core_to_outpost)
                .iter()
                .map(|message| (name(message), message.kind()))
                .collect()
        ),
        [
            "SetBackendOverride: Configuration",
            "DeliverFact: Routed",
            "ForwardingEnded: Lifecycle",
            "Query: Ask",
            "Cancel: Ask",
            "Ping: Lifecycle",
            "Fetches: Configuration",
            "NodesHeld: Release",
            "IdleQuery: Ask",
            "Shutdown: Lifecycle",
            "Assign: Lifecycle",
        ]
    );
    assert_eq!(
        kinds(
            chain(next_core_to_listener)
                .iter()
                .map(|message| (name(message), message.kind()))
                .collect()
        ),
        [
            "SetBackendOverride: Configuration",
            "HookInstalled: Lifecycle",
            "HookRemoved: Lifecycle",
            "Ping: Lifecycle",
            "IdleQuery: Ask",
            "Shutdown: Lifecycle",
        ]
    );
    assert_eq!(
        kinds(
            super::samples::outpost_to_core()
                .iter()
                .map(|message| (name(message), message.kind()))
                .collect()
        ),
        [
            "Ready: Lifecycle",
            "SpareReady: Lifecycle",
            "Event: Report",
            "Reply: Answer",
            "Pong: Lifecycle",
            "Fault: Report",
            "TargetExited: Lifecycle",
            "IdleAnswer: Answer",
        ]
    );
    assert_eq!(
        kinds(
            chain(next_listener_to_core)
                .iter()
                .map(|message| (name(message), message.kind()))
                .collect()
        ),
        [
            "Ready: Lifecycle",
            "FocusFact: Routed",
            "ForwardingEnded: Lifecycle",
            "MenuOrSwitchEnded: Report",
            "ForegroundWindow: Report",
            "Pong: Lifecycle",
            "Fault: Report",
            "IdleAnswer: Answer",
        ]
    );
}

/// verbatim.exe reports nothing to a child: what it observes is judged in
/// Core, and a child is only asked, told, or given what to release.
#[test]
fn core_sends_no_report() {
    let reports: Vec<String> = chain(next_core_to_outpost)
        .iter()
        .filter(|message| message.kind() == Kind::Report)
        .map(name)
        .chain(
            chain(next_core_to_listener)
                .iter()
                .filter(|message| message.kind() == Kind::Report)
                .map(name),
        )
        .collect();
    assert_eq!(reports, Vec::<String>::new());
}

/// Only an event and a completed query's reply carry nodes, as the
/// outpost and verbatim.exe count them.
#[test]
fn only_an_event_and_a_completed_reply_carry_nodes() {
    let carried: Vec<(String, bool)> = super::samples::outpost_to_core()
        .iter()
        .map(|message| (name(message), message.carries_nodes()))
        .collect();
    assert_eq!(
        carried,
        [
            ("Ready".to_owned(), false),
            ("SpareReady".to_owned(), false),
            ("Event".to_owned(), true),
            ("Reply".to_owned(), false),
            ("Pong".to_owned(), false),
            ("Fault".to_owned(), false),
            ("TargetExited".to_owned(), false),
            ("IdleAnswer".to_owned(), false),
        ]
    );
    let outcomes = [
        (QueryOutcome::Done(QueryResult::Navigated(None)), true),
        (QueryOutcome::Gone, false),
        (QueryOutcome::Failed("no".to_owned()), false),
        (QueryOutcome::NotStarted, false),
        (QueryOutcome::Abandoned, false),
    ];
    for (outcome, carries) in outcomes {
        let message = reply(outcome);
        assert_eq!(message.carries_nodes(), carries, "{message:?}");
    }
}

/// A message names the nodes stamping stamps and the anchors of its text:
/// a focus's line, its caret's line and selection, and a read's chunk.
#[test]
fn a_message_names_its_nodes_and_its_texts_anchors() {
    use crate::{CaretReport, FocusText, Selection, TextAnchor, TextChunk, TextPosition, TextUnit};
    let snapshot = |number| NodeSnapshot {
        id: NodeId::new(number),
        backend: Backend::Uia,
        role: Role::Button,
        name: None,
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let chunk = |anchor| TextChunk {
        unit: TextUnit::Line,
        text: String::new(),
        start: TextAnchor(anchor),
        offset: 0,
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    };
    let focus = OutpostToCore::Event {
        trace_id: TraceId::mint(),
        timing: EventTiming::default(),
        observed_at_us: 0,
        backend: Backend::Uia,
        window: None,
        application: None,
        event: NormalizedEvent::FocusChanged {
            node: snapshot(1),
            foreground: false,
            ancestors: vec![snapshot(2)],
            selected_child: Some(snapshot(3)),
            text: Some(Box::new(FocusText::Caret {
                caret: CaretReport {
                    line: chunk(10),
                    selection: Some(Selection {
                        start: TextPosition::at(TextAnchor(11)),
                        end: TextPosition::at(TextAnchor(12)),
                    }),
                    selection_line: Some(Box::new(chunk(13))),
                },
                selected_text: None,
            })),
            earlier: None,
            group: None,
        },
    };
    let read = reply(QueryOutcome::Done(QueryResult::Text(TextReply::Read {
        moved: 0,
        chunk: chunk(14),
    })));
    assert_eq!(
        focus.named(),
        Named {
            nodes: vec![NodeId::new(1), NodeId::new(2), NodeId::new(3)],
            anchors: [10, 11, 12, 13].map(TextAnchor).to_vec(),
        }
    );
    assert_eq!(
        read.named(),
        Named {
            nodes: Vec::new(),
            anchors: vec![TextAnchor(14)],
        }
    );
}

#[test]
fn stamping_names_every_node_id_in_a_message_with_its_outpost() {
    let snapshot = |number| NodeSnapshot {
        id: NodeId::new(number),
        backend: Backend::Uia,
        role: Role::Button,
        name: None,
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let mut event = OutpostToCore::Event {
        trace_id: TraceId::mint(),
        timing: EventTiming::default(),
        observed_at_us: 0,
        backend: Backend::Uia,
        window: None,
        application: None,
        event: NormalizedEvent::FocusChanged {
            node: snapshot(1),
            foreground: false,
            ancestors: vec![snapshot(2)],
            selected_child: Some(snapshot(3)),
            text: None,
            earlier: None,
            group: None,
        },
    };
    let mut chain = reply(QueryOutcome::Done(QueryResult::Ancestors(vec![snapshot(
        4,
    )])));
    event.assign_outpost(OutpostId(7));
    chain.assign_outpost(OutpostId(7));

    let OutpostToCore::Event {
        event:
            NormalizedEvent::FocusChanged {
                node,
                ancestors,
                selected_child,
                ..
            },
        ..
    } = event
    else {
        panic!("still a focus event");
    };
    let OutpostToCore::Reply {
        outcome: QueryOutcome::Done(QueryResult::Ancestors(chain)),
        ..
    } = chain
    else {
        panic!("still an ancestor chain");
    };
    for (id, number) in [
        (node.id, 1),
        (ancestors[0].id, 2),
        (selected_child.expect("kept").id, 3),
        (chain[0].id, 4),
    ] {
        assert_eq!(id, NodeId::in_outpost(OutpostId(7), number));
    }
}

#[test]
fn a_reply_or_timing_from_an_older_peer_reads_with_no_calls() {
    let reply: OutpostToCore =
        serde_json::from_str(r#"{"Reply":{"trace_id":7,"request_id":3,"outcome":"Gone"}}"#)
            .expect("an older reply still reads");
    let OutpostToCore::Reply { timing, .. } = reply else {
        panic!("still a reply");
    };
    assert_eq!(timing, EventTiming::default());
    let timing: EventTiming =
        serde_json::from_str(r#"{"observed_at_us":5}"#).expect("older timing still reads");
    assert!(timing.calls.is_empty());
}

#[test]
fn a_win_event_is_raised_before_or_after_a_tick_across_the_clocks_wrap() {
    let at = |time| WinEventStamp { thread: 1, time };
    assert!(at(20).raised_after(10));
    assert!(at(5).raised_before(10));
    assert!(!at(10).raised_after(10) && !at(10).raised_before(10));
    // Just after the clock wrapped is after just before it.
    assert!(at(3).raised_after(u32::MAX - 3));
    assert!(at(u32::MAX - 3).raised_before(3));
}

#[test]
fn a_pongs_trip_back_runs_from_its_write_to_verbatim_exes_read() {
    let mut pong = OutpostToCore::Pong {
        seq: 1,
        parked_count: 0,
        written_at_us: 0,
    };
    pong.written(7_000);
    let OutpostToCore::Pong { written_at_us, .. } = pong else {
        unreachable!("a pong stays a pong");
    };
    assert_eq!(hop_us(written_at_us, 7_250), Some(250));
}

#[test]
fn a_menu_ends_trip_runs_from_the_listeners_write_to_verbatim_exes_read() {
    let mut ended = ListenerToCore::MenuOrSwitchEnded {
        ended_at_us: 1,
        timing: EventTiming::default(),
    };
    ended.written(7_000);
    let ListenerToCore::MenuOrSwitchEnded { timing, .. } = ended else {
        unreachable!("a menu end stays a menu end");
    };
    assert_eq!(hop_us(timing.listener_written_at_us, 7_250), Some(250));
}
