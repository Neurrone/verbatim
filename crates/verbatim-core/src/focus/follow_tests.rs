//! `Follow` at each commit: Table 10's rows against its columns A to F.

use verbatim_model::request::{Follow, FollowGroup, Followed};
use verbatim_model::{
    Backend, NodeDetails, NodeId, NodeSnapshot, OutpostId, Pid, Role, StateSet, WindowHandle,
};

use super::*;
use crate::focus::versions::tests::kept;

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
fn uia(process: Pid) -> Kept {
    let mut kept = kept(10);
    kept.window.process = process;
    kept
}

fn msaa() -> Kept {
    let mut kept = kept(10);
    kept.read_by = ReadBy::Msaa {
        event: None,
        held_child: 0,
    };
    kept
}

fn naming(process: Pid, node: NodeId, group: FollowGroup) -> FollowRequest {
    FollowRequest {
        process,
        window: kept(10).window,
        follow: Follow {
            node: Some(Followed {
                node,
                group,
                extra: None,
            }),
        },
    }
}

fn nothing(process: Pid) -> FollowRequest {
    FollowRequest {
        process,
        window: kept(10).window,
        follow: Follow { node: None },
    }
}

const APP: Pid = Pid(40);
const OTHER: Pid = Pid(50);

/// Commits `kept` as a button with no parent.
fn commit(follows: &mut Follows, kept: &Kept) -> Vec<FollowRequest> {
    let node = snapshot(kept.node, Role::Button, None);
    follows.committed(Some((kept, &node, None)), None)
}

/// A UI Automation focus of `APP` committed: its node named.
fn named() -> Follows {
    let mut follows = Follows::default();
    let focus = uia(APP);
    assert_eq!(
        commit(&mut follows, &focus),
        [naming(APP, focus.node, FollowGroup::Base)]
    );
    follows
}

#[test]
fn nothing_named_then_a_commit_of_a_uia_node_names_it() {
    let follows = named();
    assert_eq!(
        follows.named(APP).map(|named| named.followed.node),
        Some(uia(APP).node)
    );
}

#[test]
fn nothing_named_then_a_commit_with_no_uia_element_sends_nothing() {
    let mut follows = Follows::default();
    assert_eq!(commit(&mut follows, &msaa()), []);
    assert_eq!(follows.committed(None, None), []);
    assert_eq!(follows, Follows::default());
}

#[test]
fn a_node_named_then_a_new_node_of_its_outpost_names_the_new_one() {
    let mut follows = named();
    let mut focus = uia(APP);
    focus.node = NodeId::new(9).with_outpost(OutpostId(7));
    assert_eq!(
        commit(&mut follows, &focus),
        [naming(APP, focus.node, FollowGroup::Base)]
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
                window: focus.window,
                ..naming(OTHER, focus.node, FollowGroup::Base)
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
    assert_eq!(follows.committed(None, None), [nothing(APP)]);
}

#[test]
fn an_unexecuted_completion_names_what_core_names_there() {
    let follows = named();
    assert_eq!(
        follows.unexecuted(APP, kept(10).window),
        naming(APP, uia(APP).node, FollowGroup::Base)
    );
    assert_eq!(follows.unexecuted(OTHER, kept(10).window), nothing(OTHER));
}

#[test]
fn the_named_nodes_window_destroyed_names_nothing() {
    let mut follows = named();
    follows.window_destroyed(WindowHandle(99));
    assert!(follows.named(APP).is_some());
    follows.window_destroyed(uia(APP).place.window);
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
fn the_group_follows_the_role_and_a_console_window() {
    let focus = uia(APP);
    let cases = [
        (Role::EditableText, None, FollowGroup::Extended),
        (Role::Document, None, FollowGroup::Extended),
        (
            Role::Terminal,
            Some(CONSOLE_WINDOW_CLASS),
            FollowGroup::Terminal { console: true },
        ),
        (
            Role::Terminal,
            Some("CASCADIA_HOSTING_WINDOW_CLASS"),
            FollowGroup::Terminal { console: false },
        ),
        (Role::ListItem, None, FollowGroup::Base),
    ];
    for (role, class, group) in cases {
        let mut follows = Follows::default();
        let node = snapshot(focus.node, role, None);
        assert_eq!(
            follows.committed(Some((&focus, &node, None)), class),
            [naming(APP, focus.node, group)],
            "{role:?} in {class:?}"
        );
    }
}

#[test]
fn a_slider_that_focuses_its_thumb_is_followed_as_the_extra_object() {
    let focus = uia(APP);
    let thumb = snapshot(focus.node, Role::Button, None);
    let slider = snapshot(
        NodeId::new(5).with_outpost(OutpostId(7)),
        Role::Slider,
        Some("SensitiveSlider"),
    );
    let mut follows = Follows::default();
    let sent = follows.committed(Some((&focus, &thumb, Some(&slider))), None);
    assert_eq!(
        sent.first()
            .and_then(|request| request.follow.node.as_ref())
            .and_then(|followed| followed.extra),
        Some(slider.id)
    );
    let plain = snapshot(slider.id, Role::Slider, None);
    let mut follows = Follows::default();
    let sent = follows.committed(Some((&focus, &thumb, Some(&plain))), None);
    assert_eq!(
        sent.first()
            .and_then(|request| request.follow.node.as_ref())
            .and_then(|followed| followed.extra),
        None
    );
}
