//! The release rule, on a store and an outbound queue writing to a pipe:
//! what a message names, and a caret tell's element; and a commit's text
//! writes.

use std::fs::File;

use verbatim_model::property::{NodeDetailSet, PropertyValues};
use verbatim_model::request::{Answer, NodeRead, NodeWindow, Outcome, Reply, RequestNumber};
use verbatim_model::{Pid, WindowHandle};
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

/// An answer naming `nodes`, the children Core asked for.
fn focus(nodes: &[NodeId]) -> OutpostToCore {
    let window = WindowHandle(0);
    let read = |id: NodeId| NodeRead {
        node: Some(id),
        window: NodeWindow {
            handle: window,
            top_level: window,
            root_owner: window,
            process: Pid(0),
            thread: 0,
            desktop: false,
        },
        values: PropertyValues(Vec::new()),
    };
    OutpostToCore::Answer(Answer {
        number: RequestNumber(1),
        outcome: Outcome::Result(Reply::Steps(vec![
            verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Related(
                nodes.iter().copied().map(read).collect(),
            )),
        ])),
        probes: Vec::new(),
        detail: String::new(),
    })
}

/// Sends `message` and waits until it is written, so it has its position.
fn sent(store: &Store, outbound: &Outbound, message: OutpostToCore) {
    outbound.publish(store.record(message));
    outbound.flush();
}

/// The node of runtime id `runtime_id`, issued or looked up.
fn node(store: &Store, runtime_id: &[i32]) -> NodeId {
    store.nodes_for(NodeDetailSet::ALL).id_for(runtime_id)
}

/// Releases for a held list that holds nothing and acknowledges
/// `acknowledged`: the nodes freed.
fn release(store: &Store, outbound: &Outbound, acknowledged: u64) -> Vec<NodeId> {
    let (_, released) = store.release(outbound, &HashSet::new(), acknowledged);
    let mut freed: Vec<NodeId> = released
        .map(|released| released.nodes.into_iter().map(NodeId::new).collect())
        .unwrap_or_default();
    freed.sort_by_key(|node| node.number());
    freed
}

/// A node a message names without having issued it, an entry spliced from
/// an earlier chain or one looked up, is recorded as carried by it: kept
/// while Core has not acknowledged the message or holds the node, and
/// freed once Core's list acknowledges the message and omits the node.
#[test]
fn a_spliced_or_looked_up_node_is_recorded_as_carried() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let spliced = node(&store, &[1]);
    let focused = node(&store, &[2]);
    assert_eq!(
        store.record(focus(&[focused, spliced])).into_parts().1,
        vec![
            Carried::Node(focused.number()),
            Carried::Node(spliced.number())
        ]
    );
    sent(&store, &outbound, focus(&[focused, spliced]));
    assert_eq!(release(&store, &outbound, 0), Vec::<NodeId>::new());
    let held = HashSet::from([Carried::Node(spliced.number())]);
    let (_, released) = store.release(&outbound, &held, 1);
    assert_eq!(
        released.map(|released| released.nodes),
        Some(std::collections::BTreeSet::from([focused.number()]))
    );
    assert_eq!(release(&store, &outbound, 1), vec![spliced]);
}

/// A commit's text write for a node the store released meanwhile is not
/// made: its anchors are not kept and its change is not in the store's
/// text facts, while the write for a node still held is made.
#[test]
fn a_text_write_for_a_released_node_is_not_made() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    let node_held = node(&store, &[1]);
    let gone = node(&store, &[2]);
    let held = HashSet::from([Carried::Node(node_held.number())]);
    let (_, released) = store.release(&outbound, &held, 0);
    assert_eq!(
        released.map(|released| released.nodes),
        Some(std::collections::BTreeSet::from([gone.number()]))
    );
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
    drop(store.write_text(vec![write(node_held, 5), write(gone, 6)]));
    let text = store.text_facts();
    assert!(text.edit(node_held.number()).is_some());
    assert_eq!(text.edit(gone.number()), None);
    assert!(store.keeps_anchor(5));
    assert!(!store.keeps_anchor(6));
}

/// A caret tell naming the element parked under `key` by its token.
fn caret_tell(key: u64) -> OutpostToCore {
    OutpostToCore::Tell(Box::new(verbatim_model::tell::Event {
        trace_id: verbatim_model::TraceId::mint(),
        kind: verbatim_model::event_kind::EventKind::UiaTextSelectionChanged,
        process: Pid(0),
        application: String::new(),
        thread: 0,
        at_us: 0,
        object: verbatim_model::tell::EventObject::Uia {
            token: verbatim_model::tell::ElementToken(key),
            runtime_id: verbatim_model::property::RuntimeId(vec![42, 1]),
        },
        cached: PropertyValues(Vec::new()),
        payload: verbatim_model::tell::Payload::None,
        window: verbatim_model::tell::TellWindow::NoWindow,
        checked: None,
    }))
}

/// Keeps a caret tell's element under the next key, as the worker keeps
/// the element its event came with, and sends the tell: the key.
fn told(store: &Store, outbound: &Outbound, key: u64) -> u64 {
    store.keep_told(key);
    assert_eq!(
        store.record(caret_tell(key)).into_parts().1,
        vec![Carried::Told(key)]
    );
    sent(store, outbound, caret_tell(key));
    key
}

/// The outpost sends caret tell T carrying key k; before Core handles T,
/// Core writes a held view that names no caret tell and acknowledges only
/// the messages before T: k stays, since T carried it after that position
/// and counts toward it (`OutpostToCore::carries_nodes`). Once a view
/// acknowledges T and leaves k out, k goes.
#[test]
fn a_caret_tells_element_stays_until_core_has_handled_its_tell() {
    let (outbound, _pipe) = outbound();
    let store = Store::new();
    sent(&store, &outbound, focus(&[]));
    let key = told(&store, &outbound, 7);
    let (position, released) = store.release(&outbound, &HashSet::new(), 1);
    assert_eq!(position, 2, "the tell counts");
    assert!(released.is_none());
    assert!(store.keeps_told(key), "k is kept");
    let (_, released) = store.release(&outbound, &HashSet::new(), 2);
    assert!(released.is_some());
    assert!(!store.keeps_told(key), "k is let go");
}

/// A caret tell's element Core's held view names is kept however many
/// tells follow it, where the store once kept only the newest 64. The
/// 65-tell case's speech is source-derived, approved by Dickson on
/// 2026-11-21: NVDA handles a text selection change as a caret event on
/// the event's own object (`nvda/source/UIAHandler/__init__.py` 280;
/// `nvda/source/NVDAObjects/behaviors.py` 346 to 349), so it is the
/// speech of one tell.
#[test]
fn a_held_caret_tells_element_is_kept_past_64_tells() {
    let (outbound, mut pipe) = outbound();
    // Sixty-five tells pass the pipe's buffer: Core's end is read.
    std::thread::spawn(move || std::io::copy(&mut pipe, &mut std::io::sink()));
    let store = Store::new();
    let first = told(&store, &outbound, 1);
    let later: Vec<u64> = (2..=65).map(|key| told(&store, &outbound, key)).collect();
    assert!(store.keeps_told(first), "kept past 64 tells");
    let held = HashSet::from([Carried::Told(first)]);
    let (position, released) = store.release(&outbound, &held, 65);
    assert_eq!(position, 65);
    assert!(released.is_some());
    assert!(store.keeps_told(first), "the held element is kept");
    assert!(
        later.iter().all(|&key| !store.keeps_told(key)),
        "every element Core let go of is released"
    );
}
