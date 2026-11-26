//! What the two versions of the focus decide on their own
//! (`books/dev/src/reference/crates/verbatim-core.md`, "Focus, events and objects", the committed focus).

use verbatim_model::property::RuntimeId;
use verbatim_model::request::KnownAncestor;
use verbatim_model::tell::ElementToken;
use verbatim_model::{Backend, NodeDetails, NodeId, OutpostId, Pid, WindowHandle};

use super::*;

const OUTPOST: OutpostId = OutpostId(7);

/// A UI Automation button of generation 7 committed in the top-level
/// window `top_level`, under a pane and the window: the one way the tests
/// build a committed focus.
pub(crate) fn focus(top_level: u64) -> Focus {
    let snapshot = |number, role, window: Option<u64>| NodeSnapshot {
        id: NodeId::new(number).with_outpost(OUTPOST),
        backend: Backend::Uia,
        role,
        name: None,
        value: None,
        states: StateSet::default(),
        details: NodeDetails {
            window: window.map(WindowHandle),
            ..NodeDetails::default()
        },
    };
    let entry = |number: u64, role, window| LineEntry {
        snapshot: snapshot(number, role, window),
        runtime_id: Some(RuntimeId(vec![42, i32::try_from(number).unwrap()])),
        simple_child: false,
    };
    let button = snapshot(1, Role::Button, None);
    Focus {
        context: FocusContext {
            source: Pid(40),
            window: None,
            spoken: crate::state::SpokenRecord::of(&button, Vec::new().into()),
            snapshot: button,
            reported_focused: false,
            trace_id: None,
            observed_at_us: 0,
            reported_format: verbatim_model::TextAttributes::default(),
            caret: None,
        },
        kept: Kept {
            read_by: ReadBy::Uia {
                runtime_id: RuntimeId(vec![42, 1]),
                token: Some(ElementToken(3)),
            },
            place: Place {
                window: WindowHandle(top_level + 1),
                top_level: WindowHandle(top_level),
                root_owner: WindowHandle(top_level),
                ancestor_windows: vec![WindowHandle(top_level)].into(),
            },
            asked: crate::event_queue::RequestWindow {
                handle: WindowHandle(top_level + 1),
                top_level: WindowHandle(top_level),
                root_owner: WindowHandle(top_level),
                process: Pid(40),
                thread: 41,
                by_class: None,
            },
            foreground: None,
            value_from_selection: false,
            selection: crate::text_facts::SelectionFacts::default(),
            console_terminal: false,
        },
        line: vec![
            entry(3, Role::Window, Some(top_level)),
            entry(2, Role::Pane, None),
        ]
        .into(),
    }
}

fn normal() -> Versions {
    let mut versions = Versions::default();
    versions.execute(focus(10), Fence::default());
    versions
}

#[test]
fn at_startup_nothing_is_known_and_nothing_is_related() {
    let mut versions = Versions::default();
    assert_eq!(versions.known_ancestors(Some(OUTPOST)), Vec::new());
    assert!(versions.caret_target().is_none());
    versions.foreground_marker(WindowHandle(10), WindowHandle(10));
    assert_eq!(versions, Versions::default());
}

#[test]
fn at_startup_core_commits_its_own_focus_and_the_last_stays_empty() {
    let mut versions = Versions::default();
    versions.commit_own();
    assert_eq!(versions.committed, Committed::Own);
    assert_eq!(versions.last, None);
}

#[test]
fn a_reader_focus_executing_becomes_both_versions() {
    let versions = normal();
    assert_eq!(versions.last, Some(focus(10).last(Fence::default())));
    assert_eq!(versions.committed, Committed::Read(Box::new(focus(10))));
}

#[test]
fn the_known_ancestors_are_the_committed_focus_and_its_with_nodes_of_its_own_generation() {
    let versions = normal();
    assert_eq!(
        versions.known_ancestors(Some(OUTPOST)),
        vec![
            KnownAncestor::Node(NodeId::new(1).with_outpost(OUTPOST)),
            KnownAncestor::Uia(RuntimeId(vec![42, 1])),
            KnownAncestor::Node(NodeId::new(2).with_outpost(OUTPOST)),
            KnownAncestor::Uia(RuntimeId(vec![42, 2])),
            KnownAncestor::Node(NodeId::new(3).with_outpost(OUTPOST)),
            KnownAncestor::Uia(RuntimeId(vec![42, 3])),
            KnownAncestor::Window(WindowHandle(10)),
        ]
    );
    assert_eq!(
        versions.known_ancestors(Some(OutpostId(8))),
        vec![
            KnownAncestor::Uia(RuntimeId(vec![42, 1])),
            KnownAncestor::Uia(RuntimeId(vec![42, 2])),
            KnownAncestor::Uia(RuntimeId(vec![42, 3])),
            KnownAncestor::Window(WindowHandle(10)),
        ]
    );
    assert_eq!(versions.known_ancestors(None).len(), 4);
}

#[test]
fn a_marker_for_an_unrelated_window_empties_the_last_and_keeps_the_committed() {
    let mut versions = normal();
    versions.foreground_marker(WindowHandle(20), WindowHandle(20));
    assert_eq!(versions.last, None);
    assert_eq!(versions.committed, Committed::Read(Box::new(focus(10))));
    assert_eq!(versions.known_ancestors(Some(OUTPOST)), Vec::new());
    assert!(versions.caret_target().is_none());
}

#[test]
fn a_marker_for_a_related_window_changes_nothing() {
    let mut versions = normal();
    versions.foreground_marker(WindowHandle(10), WindowHandle(10));
    versions.foreground_marker(WindowHandle(30), WindowHandle(10));
    assert_eq!(versions, normal());
}

#[test]
fn an_empty_last_is_not_restored_by_a_related_marker() {
    let mut versions = normal();
    versions.foreground_marker(WindowHandle(20), WindowHandle(20));
    versions.foreground_marker(WindowHandle(10), WindowHandle(10));
    assert_eq!(versions.last, None);
}

#[test]
fn with_the_last_empty_a_reader_focus_becomes_both_again() {
    let mut versions = normal();
    versions.foreground_marker(WindowHandle(20), WindowHandle(20));
    versions.execute(focus(20), Fence::default());
    assert_eq!(versions.last, Some(focus(20).last(Fence::default())));
}

#[test]
fn the_committed_readers_end_marks_it_ended_and_asks_a_retake() {
    let mut versions = normal();
    assert_eq!(versions.outpost_ended(OutpostId(8)), ReaderEnd::Nothing);
    assert_eq!(versions, normal());
    assert_eq!(versions.outpost_ended(OUTPOST), ReaderEnd::Retake);
    assert_eq!(versions.committed, Committed::Ended(Box::new(focus(10))));
    assert_eq!(versions.last, Some(focus(10).last(Fence::default())));
    // Nothing of an ended generation is known, and its caret target is
    // dead.
    assert_eq!(versions.known_ancestors(Some(OUTPOST)), Vec::new());
    assert!(versions.caret_target().is_none());
    // A replacement that also ends asks another re-take.
    assert_eq!(versions.outpost_ended(OUTPOST), ReaderEnd::Retake);
}

#[test]
fn a_marker_with_the_reader_ended_empties_the_last() {
    let mut versions = normal();
    versions.outpost_ended(OUTPOST);
    versions.foreground_marker(WindowHandle(20), WindowHandle(20));
    assert_eq!(versions.last, None);
}

#[test]
fn the_sleeping_focus_has_no_ancestors_and_relates_to_nothing() {
    let mut versions = normal();
    versions.commit_own();
    assert_eq!(versions.committed, Committed::Own);
    assert_eq!(versions.known_ancestors(Some(OUTPOST)), Vec::new());
    versions.foreground_marker(WindowHandle(10), WindowHandle(10));
    assert_eq!(versions.last, None);
    // A reader focus after it becomes both versions.
    versions.execute(focus(10), Fence::default());
    assert_eq!(versions, normal());
}

#[test]
fn the_caret_targets_the_last_focus_only_while_it_is_the_committed() {
    let versions = normal();
    assert_eq!(versions.caret_target(), Some(&focus(10)));
}

#[test]
fn an_msaa_focus_read_through_acc_focus_is_kept_with_no_event_ids() {
    let mut focus = focus(10);
    focus.kept.read_by = ReadBy::Msaa {
        event: None,
        held_child: 3,
    };
    let mut versions = Versions::default();
    versions.execute(focus, Fence::default());
    assert!(matches!(
        versions.last.as_ref().map(|last| &last.read_by),
        Some(ReadBy::Msaa { event: None, .. })
    ));
}

/// A dialog's focus whose line holds its owner's window, which the owner's
/// generation minted, and whose own generation ends as the dialog's
/// application exits: the owner's window stays known, by node only to the
/// generation that minted it, as NVDA's old focus line keeps its objects
/// (`owned_dialog_across_processes`).
#[test]
fn an_ended_generations_focus_keeps_known_what_another_generation_minted() {
    const OWNER: OutpostId = OutpostId(4);
    let mut dialog = focus(10);
    let mut line = dialog.line.to_vec();
    let mut owner = line[0].clone();
    owner.snapshot.id = NodeId::new(2).with_outpost(OWNER);
    owner.snapshot.details.window = Some(WindowHandle(99));
    owner.runtime_id = Some(RuntimeId(vec![42, 99]));
    line.insert(0, owner);
    dialog.line = line.into();
    let mut versions = Versions::default();
    versions.execute(dialog, Fence::default());
    // While the dialog's generation lives, the owner's node goes only to
    // the owner's generation.
    assert_eq!(
        versions.known_ancestors(Some(OWNER)),
        vec![
            KnownAncestor::Uia(RuntimeId(vec![42, 1])),
            KnownAncestor::Uia(RuntimeId(vec![42, 2])),
            KnownAncestor::Uia(RuntimeId(vec![42, 3])),
            KnownAncestor::Window(WindowHandle(10)),
            KnownAncestor::Node(NodeId::new(2).with_outpost(OWNER)),
            KnownAncestor::Uia(RuntimeId(vec![42, 99])),
            KnownAncestor::Window(WindowHandle(99)),
        ]
    );
    assert_eq!(versions.outpost_ended(OUTPOST), ReaderEnd::Retake);
    let owner = vec![
        KnownAncestor::Node(NodeId::new(2).with_outpost(OWNER)),
        KnownAncestor::Uia(RuntimeId(vec![42, 99])),
        KnownAncestor::Window(WindowHandle(99)),
    ];
    assert_eq!(versions.known_ancestors(Some(OWNER)), owner);
    assert_eq!(versions.known_ancestors(None), owner[1..]);
    // Its place in the committed line is kept, for the meeting point.
    assert_eq!(
        versions
            .known_with_places(None)
            .into_iter()
            .map(|(place, _)| place)
            .collect::<Vec<_>>(),
        [3, 3]
    );
}
