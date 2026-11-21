//! `Follow` at each commit: Table 10's rows against its columns A to F.

use verbatim_model::request::{FollowGroup, Followed};
use verbatim_model::{
    Backend, NodeDetails, NodeId, NodeSnapshot, OutpostId, Pid, Role, StateSet, WindowHandle,
};

use super::*;
use crate::focus::versions::Focus;
use crate::focus::versions::tests::focus as committed;

fn snapshot(id: NodeId, role: Role, class: Option<&str>) -> NodeSnapshot {
    NodeSnapshot {
        id,
        backend: Backend::Uia,
        role,
        name: None,
        value: None,
        states: StateSet::default(),
        details: NodeDetails {
            class_name: class.map(str::to_owned),
            ..NodeDetails::default()
        },
    }
}

/// A UI Automation focus of `process`, generation 7, in window 11.
fn uia(process: Pid) -> Focus {
    let mut focus = committed(10);
    focus.kept.asked.process = process;
    focus
}

fn msaa() -> Focus {
    let mut focus = committed(10);
    focus.kept.read_by = ReadBy::Msaa {
        event: None,
        held_child: 0,
    };
    focus
}

fn naming(process: Pid, node: NodeId, group: FollowGroup) -> FollowRequest {
    FollowRequest {
        process,
        window: committed(10).kept.asked,
        follow: Some(Followed {
            node,
            group,
            extra: None,
        }),
    }
}

fn nothing(process: Pid) -> FollowRequest {
    FollowRequest {
        process,
        window: committed(10).kept.asked,
        follow: None,
    }
}

const APP: Pid = Pid(40);
const OTHER: Pid = Pid(50);

/// Commits `focus` as a button with no parent.
fn commit(follows: &mut Follows, focus: &Focus) -> Vec<FollowRequest> {
    let node = snapshot(focus.context.snapshot.id, Role::Button, None);
    follows.committed(Some((&focus.kept, &node, None)), false)
}

/// A UI Automation focus of `APP` committed: its node named.
fn named() -> Follows {
    let mut follows = Follows::default();
    let focus = uia(APP);
    assert_eq!(
        commit(&mut follows, &focus),
        [naming(APP, focus.context.snapshot.id, FollowGroup::Base)]
    );
    follows
}

#[test]
fn nothing_named_then_a_commit_of_a_uia_node_names_it() {
    let follows = named();
    assert_eq!(
        follows.named(APP).map(|named| named.followed.node),
        Some(uia(APP).context.snapshot.id)
    );
}

#[test]
fn nothing_named_then_a_commit_with_no_uia_element_sends_nothing() {
    let mut follows = Follows::default();
    assert_eq!(commit(&mut follows, &msaa()), []);
    assert_eq!(follows.committed(None, false), []);
    assert_eq!(follows, Follows::default());
}

#[test]
fn a_node_named_then_a_new_node_of_its_outpost_names_the_new_one() {
    let mut follows = named();
    let mut focus = uia(APP);
    focus.context.snapshot.id = NodeId::new(9).with_outpost(OutpostId(7));
    assert_eq!(
        commit(&mut follows, &focus),
        [naming(APP, focus.context.snapshot.id, FollowGroup::Base)]
    );
}

#[test]
fn a_node_named_then_another_applications_focus_names_nothing_to_the_old_outpost() {
    let mut follows = named();
    let focus = uia(OTHER);
    assert_eq!(
        commit(&mut follows, &focus),
        [
            nothing(APP),
            FollowRequest {
                window: focus.kept.asked,
                ..naming(OTHER, focus.context.snapshot.id, FollowGroup::Base)
            }
        ]
    );
    assert_eq!(follows.named(APP), None);
}

#[test]
fn a_node_named_then_a_focus_with_no_uia_element_or_cores_own_names_nothing() {
    let mut follows = named();
    assert_eq!(commit(&mut follows, &msaa()), [nothing(APP)]);
    assert_eq!(follows.named(APP), None);
    let mut follows = named();
    assert_eq!(follows.committed(None, false), [nothing(APP)]);
}

#[test]
fn an_unexecuted_completion_names_what_core_names_there() {
    let follows = named();
    assert_eq!(
        follows.unexecuted(APP, committed(10).kept.asked),
        naming(APP, uia(APP).context.snapshot.id, FollowGroup::Base)
    );
    assert_eq!(
        follows.unexecuted(OTHER, committed(10).kept.asked),
        nothing(OTHER)
    );
}

/// The named node's window destroyed (column E): it is named no more, and
/// its outpost is sent a `Follow` naming nothing; another window's
/// destruction changes nothing.
#[test]
fn the_named_nodes_window_destroyed_names_nothing_and_sends_nothing_named() {
    let mut follows = named();
    assert_eq!(follows.window_destroyed(WindowHandle(99)), []);
    assert!(follows.named(APP).is_some());
    assert_eq!(
        follows.window_destroyed(uia(APP).kept.place.window),
        [nothing(APP)]
    );
    assert_eq!(follows.named(APP), None);
}

#[test]
fn the_outposts_generation_ending_names_nothing() {
    let mut follows = named();
    follows.outpost_ended(APP, OutpostId(6));
    assert!(follows.named(APP).is_some());
    follows.outpost_ended(APP, OutpostId(7));
    assert_eq!(follows.named(APP), None);
}

#[test]
fn the_group_follows_the_role_the_class_and_a_console_window() {
    let focus = uia(APP);
    let cases = [
        (
            Role::Document,
            Some("RichEditD2DPT"),
            false,
            FollowGroup::Base,
        ),
        (Role::EditableText, None, false, FollowGroup::Base),
        (Role::Document, Some("_WwG"), false, FollowGroup::Extended),
        (
            Role::Terminal,
            None,
            true,
            FollowGroup::Terminal { console: true },
        ),
        (
            Role::Terminal,
            Some("TermControl"),
            false,
            FollowGroup::Terminal { console: false },
        ),
        (Role::ListItem, None, false, FollowGroup::Base),
    ];
    for (role, class, console, group) in cases {
        let mut follows = Follows::default();
        let node = snapshot(focus.context.snapshot.id, role, class);
        assert_eq!(
            follows.committed(Some((&focus.kept, &node, None)), console),
            [naming(APP, focus.context.snapshot.id, group)],
            "{role:?} of class {class:?}, a console host's: {console}"
        );
    }
}

#[test]
fn a_slider_that_focuses_its_thumb_is_followed_as_the_extra_object() {
    let focus = uia(APP);
    let thumb = snapshot(focus.context.snapshot.id, Role::Button, None);
    let slider = snapshot(
        NodeId::new(5).with_outpost(OutpostId(7)),
        Role::Slider,
        Some("SensitiveSlider"),
    );
    let mut follows = Follows::default();
    let sent = follows.committed(Some((&focus.kept, &thumb, Some(&slider))), false);
    assert_eq!(
        sent.first()
            .and_then(|request| request.follow.as_ref())
            .and_then(|followed| followed.extra),
        Some(slider.id)
    );
    let plain = snapshot(slider.id, Role::Slider, None);
    let mut follows = Follows::default();
    let sent = follows.committed(Some((&focus.kept, &thumb, Some(&plain))), false);
    assert_eq!(
        sent.first()
            .and_then(|request| request.follow.as_ref())
            .and_then(|followed| followed.extra),
        None
    );
}
