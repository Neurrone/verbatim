//! Table 3's cells that the two versions decide on their own: each row
//! against columns B to H.

use verbatim_model::property::RuntimeId;
use verbatim_model::request::{KnownAncestor, ObjectClass};
use verbatim_model::tell::ElementToken;
use verbatim_model::{NodeId, OutpostId, Pid, WindowHandle};

use super::*;

const OUTPOST: OutpostId = OutpostId(7);

pub(crate) fn kept(top_level: u64) -> Kept {
    Kept {
        source: Pid(40),
        outpost: OUTPOST,
        node: NodeId::new(1).with_outpost(OUTPOST),
        read_by: ReadBy::Uia {
            runtime_id: RuntimeId(vec![42, 1]),
            token: Some(ElementToken(3)),
        },
        place: Place {
            window: WindowHandle(top_level + 1),
            top_level: WindowHandle(top_level),
            root_owner: WindowHandle(top_level),
            ancestor_windows: vec![WindowHandle(top_level)],
        },
        window: crate::event_queue::RequestWindow {
            handle: WindowHandle(top_level + 1),
            top_level: WindowHandle(top_level),
            root_owner: WindowHandle(top_level),
            process: Pid(40),
            thread: 41,
            by_class: None,
        },
        ancestors: vec![
            KeptAncestor {
                node: NodeId::new(2).with_outpost(OUTPOST),
                runtime_id: Some(RuntimeId(vec![42, 2])),
                window: None,
                simple_child: false,
                dialog_text: None,
            },
            KeptAncestor {
                node: NodeId::new(3).with_outpost(OUTPOST),
                runtime_id: Some(RuntimeId(vec![42, 3])),
                window: Some(WindowHandle(top_level)),
                simple_child: false,
                dialog_text: None,
            },
        ],
        redirected_to: None,
        foreground: None,
        value_from_selection: false,
        class: ObjectClass::Plain,
    }
}

fn normal() -> Versions {
    let mut versions = Versions::default();
    versions.execute(kept(10));
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
    versions.commit_own(true);
    assert_eq!(versions.committed, Committed::Own { source: None });
    assert_eq!(versions.last, None);
}

#[test]
fn a_reader_focus_executing_becomes_both_versions() {
    let versions = normal();
    assert_eq!(versions.last, Some(kept(10).last()));
    assert_eq!(versions.committed, Committed::Read(Box::new(kept(10))));
}

#[test]
fn the_known_ancestors_are_the_committed_focuss_with_nodes_of_its_own_generation() {
    let versions = normal();
    assert_eq!(
        versions.known_ancestors(Some(OUTPOST)),
        vec![
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
            KnownAncestor::Uia(RuntimeId(vec![42, 2])),
            KnownAncestor::Uia(RuntimeId(vec![42, 3])),
            KnownAncestor::Window(WindowHandle(10)),
        ]
    );
    assert_eq!(versions.known_ancestors(None).len(), 3);
}

#[test]
fn a_marker_for_an_unrelated_window_empties_the_last_and_keeps_the_committed() {
    let mut versions = normal();
    versions.foreground_marker(WindowHandle(20), WindowHandle(20));
    assert_eq!(versions.last, None);
    assert_eq!(versions.committed, Committed::Read(Box::new(kept(10))));
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
    versions.execute(kept(20));
    assert_eq!(versions.last, Some(kept(20).last()));
}

#[test]
fn the_committed_readers_end_marks_it_ended_and_asks_a_retake() {
    let mut versions = normal();
    assert_eq!(versions.outpost_ended(OutpostId(8)), ReaderEnd::Nothing);
    assert_eq!(versions, normal());
    assert_eq!(versions.outpost_ended(OUTPOST), ReaderEnd::Retake);
    assert_eq!(versions.committed, Committed::Ended(Box::new(kept(10))));
    assert_eq!(versions.last, Some(kept(10).last()));
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
fn core_commits_its_own_focus_over_a_reader_focus() {
    let mut versions = normal();
    versions.commit_own(false);
    assert_eq!(
        versions.committed,
        Committed::Own {
            source: Some(Box::new(kept(10)))
        }
    );
    assert_eq!(versions.last, Some(kept(10).last()));
    // Its source's ancestors are known; the caret has no target.
    assert_eq!(versions.known_ancestors(None).len(), 3);
    assert!(versions.caret_target().is_none());
    // A newer own focus replaces it, keeping the source.
    versions.commit_own(false);
    assert_eq!(
        versions.committed,
        Committed::Own {
            source: Some(Box::new(kept(10)))
        }
    );
    // Its reader ending does nothing.
    assert_eq!(versions.outpost_ended(OUTPOST), ReaderEnd::Nothing);
}

#[test]
fn the_sleeping_focus_has_no_ancestors_and_relates_to_nothing() {
    let mut versions = normal();
    versions.commit_own(true);
    assert_eq!(versions.committed, Committed::Own { source: None });
    assert_eq!(versions.known_ancestors(Some(OUTPOST)), Vec::new());
    versions.foreground_marker(WindowHandle(10), WindowHandle(10));
    assert_eq!(versions.last, None);
    // A reader focus after it becomes both versions.
    versions.execute(kept(10));
    assert_eq!(versions, normal());
}

#[test]
fn the_caret_targets_the_last_focus_only_while_it_is_the_committed() {
    let versions = normal();
    assert_eq!(versions.caret_target(), Some(&kept(10)));
}

#[test]
fn an_msaa_focus_read_through_acc_focus_is_kept_with_no_event_ids() {
    let mut focus = kept(10);
    focus.read_by = ReadBy::Msaa {
        event: None,
        held_child: 3,
    };
    let mut versions = Versions::default();
    versions.execute(focus);
    assert!(matches!(
        versions.last.as_ref().map(|last| &last.read_by),
        Some(ReadBy::Msaa { event: None, .. })
    ));
}
