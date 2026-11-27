use verbatim_model::batch::{Descendant, Done, Missing, Op, StepOutcome, Subject};
use verbatim_model::property::{PropertyAnswer, PropertyValues};
use verbatim_model::request::NodeWindow;
use verbatim_model::{NodeId, Pid, Rect, StateSet};

use super::*;

/// A node answered with `values`.
fn read(node: u64, values: Vec<(PropertyId, PropertyValue)>) -> NodeRead {
    NodeRead {
        node: Some(NodeId::new(node)),
        window: NodeWindow {
            handle: WindowHandle(5),
            top_level: WindowHandle(5),
            root_owner: WindowHandle(5),
            process: Some(Pid(9)),
            thread: 1,
            desktop: false,
        },
        values: PropertyValues(
            values
                .into_iter()
                .map(|(id, value)| PropertyAnswer {
                    id,
                    value: Some(value),
                })
                .collect(),
        ),
    }
}

/// A walk's answer of `nodes`, each with its parent's place.
fn walked(nodes: Vec<(NodeRead, Option<u32>)>, complete: bool) -> StepOutcome {
    StepOutcome::Done(Done::Descendants(Descended {
        nodes: nodes
            .into_iter()
            .map(|(node, parent)| Descendant {
                node,
                parent,
                root: 0,
            })
            .collect(),
        complete,
    }))
}

fn named(node: u64, name: &str, runtime_id: &[i32]) -> NodeRead {
    read(
        node,
        vec![
            (PropertyId::Name, PropertyValue::Text(name.to_owned())),
            (
                PropertyId::RuntimeId,
                PropertyValue::RuntimeId(RuntimeId(runtime_id.to_vec())),
            ),
        ],
    )
}

/// The dump is one walk of the main window's own object and everything
/// below it, through UI Automation, its rectangle read only with
/// locations; its answer, a flat list with parents' places, becomes the
/// tree, a node with a runtime id read through UI Automation and one
/// without through MSAA.
#[test]
fn a_dump_walks_the_main_window_and_builds_the_tree_from_the_parents_places() {
    let sent = dump(WindowHandle(5), true);
    let [step] = sent.batch().steps.as_slice() else {
        panic!("one step: {:?}", sent.batch());
    };
    let Op::Descendants { subject, walk } = &step.op else {
        panic!("a walk: {step:?}");
    };
    assert_eq!(
        *subject,
        Subject::Window {
            which: WindowOf::Handle(WindowHandle(5)),
            backend: Backend::Uia,
            visible: false,
        }
    );
    assert_eq!(
        (walk.depth, walk.nodes, walk.include_subject, walk.view),
        (64, 4096, true, View::Raw)
    );
    assert!(walk.set.contains(&PropertyId::Rect));
    let unlocated = dump(WindowHandle(5), false);
    let Op::Descendants {
        walk: unlocated, ..
    } = &unlocated.batch().steps[0].op
    else {
        panic!("a walk");
    };
    assert!(!unlocated.set.contains(&PropertyId::Rect));
    let window = read(
        1,
        vec![(PropertyId::Role, PropertyValue::Role(Role::Window))],
    );
    let pane = named(2, "pane", &[42, 2]);
    let button = named(3, "OK", &[42, 3]);
    let text = named(4, "label", &[42, 4]);
    let reply = Reply::Steps(vec![walked(
        vec![
            (window, None),
            (pane, Some(0)),
            (button, Some(1)),
            (text, Some(0)),
        ],
        false,
    )]);
    let tree = dumped(&sent, reply, true).expect("a tree");
    assert!(tree.truncated);
    assert!(tree.locations);
    assert_eq!(tree.root.snapshot.backend, Backend::Msaa);
    let names: Vec<_> = tree
        .root
        .children
        .iter()
        .map(|child| (child.snapshot.name.as_deref(), child.children.len()))
        .collect();
    assert_eq!(names, [(Some("pane"), 1), (Some("label"), 0)]);
    assert_eq!(tree.root.children[0].snapshot.backend, Backend::Uia);
    assert_eq!(
        tree.root.children[0].children[0].snapshot.name.as_deref(),
        Some("OK")
    );
}

/// A dump whose walk did not run has no tree.
#[test]
fn a_dump_whose_walk_did_not_run_has_no_tree() {
    let sent = dump(WindowHandle(5), false);
    let reply = Reply::Steps(vec![StepOutcome::NotThere(Missing::Object)]);
    assert!(dumped(&sent, reply, false).is_err());
}

/// The tray list walks the notification area and each overflow window
/// shown, in the control view, for named buttons on the screen; the list
/// is every walk's items, a whitespace-only name left out, an overflow
/// window not shown adding nothing.
#[test]
fn the_tray_list_is_every_walks_named_buttons() {
    let sent = shell(ShellItemKind::SystemTray, None);
    let windows: Vec<_> = sent
        .batch()
        .steps
        .iter()
        .map(|step| match &step.op {
            Op::Descendants {
                subject: Subject::Window { which, visible, .. },
                walk,
            } => {
                assert_eq!(walk.view, View::Control);
                assert_eq!(walk.matcher, Some(listed()));
                (which.clone(), *visible)
            }
            other => panic!("a walk of a window: {other:?}"),
        })
        .collect();
    let path = |classes: &[&str]| {
        WindowOf::ClassPath(classes.iter().map(|&class| class.to_owned()).collect())
    };
    assert_eq!(
        windows,
        [
            (path(&["Shell_TrayWnd", "TrayNotifyWnd"]), false),
            (path(&["TopLevelWindowForOverflowXamlIsland"]), true),
            (path(&["NotifyIconOverflowWindow"]), true),
        ]
    );
    let reply = Reply::Steps(vec![
        walked(
            vec![
                (named(1, "Volume", &[42, 1]), None),
                (named(2, "   ", &[42, 2]), None),
            ],
            true,
        ),
        walked(vec![(named(3, "Clock", &[42, 3]), None)], true),
        StepOutcome::NotThere(Missing::Object),
    ]);
    let items = items(&sent, reply).expect("the list");
    assert_eq!(
        items,
        [
            ShellItem {
                name: "Volume".to_owned(),
                runtime_id: vec![42, 1],
            },
            ShellItem {
                name: "Clock".to_owned(),
                runtime_id: vec![42, 3],
            },
        ]
    );
}

/// A list is complete or nothing: a walk cut short, or a notification
/// area not there, gives no list.
#[test]
fn a_list_cut_short_or_without_its_window_is_no_list() {
    let sent = shell(ShellItemKind::SystemTray, None);
    let cut = Reply::Steps(vec![
        walked(vec![(named(1, "Volume", &[42, 1]), None)], false),
        StepOutcome::NotThere(Missing::Object),
        StepOutcome::NotThere(Missing::Object),
    ]);
    assert_eq!(items(&sent, cut), None);
    let gone = Reply::Steps(vec![
        StepOutcome::NotThere(Missing::Object),
        StepOutcome::NotThere(Missing::Object),
        StepOutcome::NotThere(Missing::Object),
    ]);
    assert_eq!(items(&sent, gone), None);
}

/// The taskbar list walks the taskbar alone, the notification area's
/// subtree left out by its class.
#[test]
fn the_taskbar_list_leaves_the_notification_area_out() {
    let sent = shell(ShellItemKind::Taskbar, None);
    let [step] = sent.batch().steps.as_slice() else {
        panic!("one walk: {:?}", sent.batch());
    };
    let Op::Descendants { walk, .. } = &step.op else {
        panic!("a walk: {step:?}");
    };
    assert_eq!(
        walk.prune,
        Some(Matcher::Is(
            PropertyId::ClassName,
            PropertyValue::Text("TrayNotifyWnd".to_owned()),
        ))
    );
    let reply = Reply::Steps(vec![walked(vec![(named(1, "Edge", &[42, 1]), None)], true)]);
    assert_eq!(
        items(&sent, reply).map(|items| items.len()),
        Some(1),
        "the one walk's items"
    );
}

/// A click's read matches the chosen item's runtime id or name among the
/// listed buttons, and answers each with whether it is off the screen and
/// where it is; a walk cut short marks the answer incomplete.
#[test]
fn a_position_read_matches_the_chosen_items_id_or_name() {
    let chosen = ShellItem {
        name: "Clock".to_owned(),
        runtime_id: vec![42, 3],
    };
    let sent = shell(ShellItemKind::SystemTray, Some(&chosen));
    let Op::Descendants { walk, .. } = &sent.batch().steps[0].op else {
        panic!("a walk");
    };
    assert_eq!(
        walk.matcher,
        Some(Matcher::All(vec![
            listed(),
            Matcher::Any(vec![
                Matcher::Is(
                    PropertyId::RuntimeId,
                    PropertyValue::RuntimeId(RuntimeId(vec![42, 3])),
                ),
                Matcher::Is(PropertyId::Name, PropertyValue::Text("Clock".to_owned())),
            ]),
        ]))
    );
    let rect = Rect {
        left: 1162,
        top: 752,
        width: 24,
        height: 48,
    };
    let mut clock = named(3, "Clock", &[42, 3]);
    clock.values.0.push(PropertyAnswer {
        id: PropertyId::States,
        value: Some(PropertyValue::States(StateSet::new())),
    });
    clock.values.0.push(PropertyAnswer {
        id: PropertyId::Rect,
        value: Some(PropertyValue::Rect(rect)),
    });
    let reply = Reply::Steps(vec![
        walked(vec![(clock, None)], false),
        StepOutcome::NotThere(Missing::Object),
        StepOutcome::NotThere(Missing::Object),
    ]);
    assert_eq!(
        candidates(&sent, reply),
        Some(ShellCandidates {
            candidates: vec![ShellCandidate {
                item: chosen,
                offscreen: false,
                rect: Some(rect),
            }],
            complete: false,
        })
    );
}

/// An item listed with no runtime id is matched by its name alone.
#[test]
fn an_item_with_no_runtime_id_is_matched_by_name_alone() {
    let chosen = ShellItem {
        name: "Volume".to_owned(),
        runtime_id: Vec::new(),
    };
    let sent = shell(ShellItemKind::Taskbar, Some(&chosen));
    let Op::Descendants { walk, .. } = &sent.batch().steps[0].op else {
        panic!("a walk");
    };
    assert_eq!(
        walk.matcher,
        Some(Matcher::All(vec![
            listed(),
            Matcher::Any(vec![Matcher::Is(
                PropertyId::Name,
                PropertyValue::Text("Volume".to_owned()),
            )]),
        ]))
    );
}
