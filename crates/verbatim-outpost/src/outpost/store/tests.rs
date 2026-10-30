//! The release rule, on a store and an outbound queue writing to a pipe:
//! a request's tags, the lines a program copies, a dead generation's
//! nodes, and what a message names; and a commit's text writes.

use std::fs::File;

use verbatim_model::protocol::EventTiming;
use verbatim_model::{Backend, NodeDetails, NodeSnapshot, NormalizedEvent, StateSet, TraceId};
use verbatim_process::overlapped::{End, Flow, pipe};
use verbatim_process::transport::Sender;
use verbatim_uia::Interning as _;

use super::*;

/// An outbound queue writing to a pipe whose 4 KiB buffer takes every
/// message a test sends, and the pipe's other end, kept open.
fn outbound() -> (Outbound, File) {
    let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
    let (sender, _finishing) =
        Sender::start(ours, Outbound::backlog(), "store-test", || {}).expect("a sender");
    (Outbound::new(sender), File::from(theirs))
}

/// A focus event naming `nodes`, the first the focus and the rest its
/// ancestors.
fn focus(nodes: &[NodeId]) -> OutpostToCore {
    let snapshot = |id: NodeId| NodeSnapshot {
        id,
        backend: Backend::Uia,
        role: Role::Button,
        name: None,
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    OutpostToCore::Event {
        trace_id: TraceId::mint(),
        observed_at_us: 0,
        backend: Backend::Uia,
        window: None,
        application: None,
        timing: EventTiming::default(),
        event: NormalizedEvent::FocusChanged {
            node: snapshot(nodes[0]),
            foreground: false,
            ancestors: nodes[1..].iter().copied().map(snapshot).collect(),
            selected_child: None,
            text: None,
            earlier: None,
            group: None,
        },
    }
}

/// Sends `message` and waits until it is written, so it has its position.
fn sent(store: &Store, outbound: &Outbound, message: OutpostToCore) {
    outbound.publish(store.record(message));
    outbound.flush();
}

/// The node of runtime id `runtime_id`, issued or looked up under the
/// request `tag`.
fn node_under(store: &Store, tag: Tag, runtime_id: &[i32]) -> NodeId {
    store.nodes_for(tag).id_for(runtime_id)
}

/// Releases for a held list that holds nothing and acknowledges
/// `acknowledged`, with `live` the live requests: the nodes freed.
fn release(store: &Store, outbound: &Outbound, acknowledged: u64, live: &[Tag]) -> Vec<NodeId> {
    let (_, released) = store.release(outbound, &HashSet::new(), acknowledged, live);
    let mut freed: Vec<NodeId> = released
        .map(|released| released.nodes.into_iter().map(NodeId::new).collect())
        .unwrap_or_default();
    freed.sort_by_key(|node| node.number());
    freed
}

/// A walk in flight keeps every node it issued or looked up across a
/// release that acknowledges and omits them, whether a message carried
/// one before or never did; once its request has ended, they go.
#[test]
fn a_walk_in_flight_keeps_its_tagged_nodes_carried_before_or_not() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let carried = node_under(&store, store.next_tag(0), &[1]);
    sent(&store, &outbound, focus(&[carried]));
    let walk = store.next_tag(0);
    let looked_up = node_under(&store, walk, &[1]);
    let issued = node_under(&store, walk, &[2]);
    assert_eq!(looked_up, carried);
    assert_eq!(release(&store, &outbound, 1, &[walk]), Vec::<NodeId>::new());
    assert_eq!(release(&store, &outbound, 1, &[]), vec![carried, issued]);
}

/// A node of a line a running request copied is kept across a release
/// that omits it, though Core saw its last carrier; once the request has
/// ended, it goes.
#[test]
fn a_node_of_a_running_programs_copied_line_is_kept() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let ancestor = node_under(&store, store.next_tag(0), &[1]);
    sent(&store, &outbound, focus(&[ancestor]));
    let walk = store.next_tag(0);
    assert_eq!(store.held_prefix(walk, &[ancestor]), 1);
    assert_eq!(release(&store, &outbound, 1, &[walk]), Vec::<NodeId>::new());
    assert_eq!(release(&store, &outbound, 1, &[]), vec![ancestor]);
}

/// An abandoned generation's nodes, which no message carried, are freed
/// by the next release, though the generation's worker may still be in
/// its call.
#[test]
fn a_dead_generations_nodes_are_freed() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let abandoned = node_under(&store, store.next_tag(0), &[1]);
    let in_charge = store.next_tag(1);
    let kept = node_under(&store, in_charge, &[2]);
    assert_eq!(release(&store, &outbound, 0, &[in_charge]), vec![abandoned]);
    assert!(store.holds(kept));
}

/// A node a message names without having issued it, an entry spliced from
/// an earlier chain or one looked up, is recorded as carried by it: kept
/// while Core has not acknowledged the message or holds the node, and
/// freed once Core's list acknowledges the message and omits the node.
#[test]
fn a_spliced_or_looked_up_node_is_recorded_as_carried() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let spliced = node_under(&store, store.next_tag(0), &[1]);
    let focused = node_under(&store, store.next_tag(0), &[2]);
    assert_eq!(
        store.record(focus(&[focused, spliced])).into_parts().1,
        vec![
            Carried::Node(focused.number()),
            Carried::Node(spliced.number())
        ]
    );
    sent(&store, &outbound, focus(&[focused, spliced]));
    assert_eq!(release(&store, &outbound, 0, &[]), Vec::<NodeId>::new());
    let held = HashSet::from([Carried::Node(spliced.number())]);
    let (_, released) = store.release(&outbound, &held, 1, &[]);
    assert_eq!(
        released.map(|released| released.nodes),
        Some(HashSet::from([focused.number()]))
    );
    assert_eq!(release(&store, &outbound, 1, &[]), vec![spliced]);
}

/// A commit's text write for a node the store released meanwhile is not
/// made: its anchors are not tagged and the reducer is not given its
/// change, while the write for a node still held is made.
#[test]
fn a_text_write_for_a_released_node_is_not_made() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let reading = store.next_tag(0);
    let node = node_under(&store, reading, &[1]);
    let gone = node_under(&store, store.next_tag(0), &[2]);
    assert_eq!(release(&store, &outbound, 0, &[reading]), vec![gone]);
    let change = |node: NodeId| TextChange::Edit {
        node: node.number(),
        part: crate::outpost::NodeAnchors::default(),
        copied: std::collections::BTreeSet::new(),
    };
    let write = |node: NodeId, anchor: u64| TextWrite::Facts {
        change: change(node),
        ranges: Vec::new(),
        touched: vec![anchor],
    };
    let (changes, _) = store.write_text(reading, vec![write(node, 5), write(gone, 6)]);
    assert_eq!(changes, vec![change(node)]);
    assert!(store.tags_anchor(5));
    assert!(!store.tags_anchor(6));
}
