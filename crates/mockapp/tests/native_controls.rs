//! Real Win32 controls inside mockapp, read through their own MSAA
//! implementations by a real outpost in this process, as Verbatim reads the
//! controls of real applications: a comctl32 tree view registered under a
//! Windows Forms class name (`tests/fixtures/tree_view.json`), with check
//! boxes.
//!
//! mockapp's scripted root, the tree view's parent, has the focused state,
//! so a focus handed to the outpost passes NVDA's focused-state check
//! without the control taking the keyboard focus from the desktop the test
//! runs on. What the outpost reports is then given to the reducer, and the
//! speech it makes is asserted whole.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::SrState;
use verbatim_model::{
    Backend, Effect, Input, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role, SegmentContent,
    State, StateSet, TraceId,
};

use common::outpost::{OutpostUnderTest, Reported};
use common::tree_view::{focus_item, tree_view};

/// The speech the reducer makes for `reported`, a focus from a fresh
/// state, as the content of each utterance in order. Every effect but
/// the speech-validity one a focus always makes is spoken speech.
fn spoken(reported: &Reported) -> Vec<Vec<SegmentContent>> {
    let mut event = NormalizedEvent::FocusChanged {
        node: reported.node.clone(),
        foreground: false,
        ancestors: reported.ancestors.clone(),
        ancestors_unknown: false,
        selected_child: reported.selected_child.clone(),
    };
    event.assign_outpost(OutpostId(1));
    let effects = verbatim_core::reduce(
        &mut SrState::new(),
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            source: Pid(1),
            backend: Backend::Msaa,
            window: None,
            event,
        },
    );
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Speak(utterance) => Some(
                utterance
                    .segments
                    .into_iter()
                    .map(|segment| segment.content)
                    .collect(),
            ),
            Effect::DropExpiredSpeech(_) => None,
            other => panic!("a focus makes no {other:?}"),
        })
        .collect()
}

/// The states `states` names.
fn states(states: &[State]) -> StateSet {
    states.iter().copied().collect()
}

/// A focus on an item of a Windows Forms tree view gets the native tree
/// view's handling, its class name normalized as NVDA normalizes it: the
/// item's logical parent among its ancestors, its level from its value,
/// and its position among its siblings.
fn a_windows_forms_tree_item_is_read_as_a_tree_view_item() {
    common::init_com();
    let title = common::unique_title("mockapp-native-tree-focus");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());

    let reported = focus_item(&outpost, tree, "Disks");
    let node = &reported.node;
    assert_eq!(
        (node.role, node.name.as_deref(), node.value.as_deref()),
        (Role::TreeItem, Some("Disks"), None),
        "a tree view item, whose value is its level"
    );
    assert_eq!(
        node.states,
        states(&[
            State::Focusable,
            State::Selected,
            State::Selectable,
            State::Checkable,
            State::Checked
        ]),
        "checked, by its state image"
    );
    assert_eq!(
        (
            node.details.level,
            node.details.position_in_set,
            node.details.set_size
        ),
        (Some(1), Some(1), Some(2)),
        "the level and the position among the item's siblings"
    );
    // Hardware is the item's logical parent, through the control's own
    // messages; the tree view's window object is layout.
    assert_eq!(
        reported.chain(),
        [
            Some("Mockapp Tree View Fixture"),
            None,
            Some("Hardware"),
            Some("Disks")
        ]
    );
    assert_eq!(
        roles(&reported.ancestors),
        [Role::Unknown, Role::Tree, Role::TreeItem]
    );
    assert_eq!(
        spoken(&reported),
        [
            vec![SegmentContent::Role(Role::Tree)],
            vec![
                SegmentContent::Level(1),
                SegmentContent::Label("Disks".to_owned()),
                SegmentContent::State(State::Checked),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(2)
                },
            ],
        ]
    );
    app.quit();
}

/// A tree view that draws its own check boxes says through each item's
/// state image whether it is checked, which NVDA's tree view item reads
/// (`TVM_GETITEMSTATE`): an item with an empty box is checkable and not
/// checked, one with a partly filled box is half checked.
fn tree_view_items_are_checked_by_their_state_images() {
    common::init_com();
    let title = common::unique_title("mockapp-native-tree-checks");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());

    let reported = focus_item(&outpost, tree, "Display");
    assert_eq!(
        reported.node.states,
        states(&[State::Focusable, State::Selectable, State::Checkable])
    );
    let hardware = reported.ancestors.last().expect("the item's parent");
    assert_eq!(hardware.name.as_deref(), Some("Hardware"));
    assert_eq!(
        hardware.states,
        states(&[
            State::Focusable,
            State::Selectable,
            State::Expanded,
            State::Checkable,
            State::Mixed
        ]),
        "half checked"
    );
    assert_eq!(
        spoken(&reported),
        [
            vec![SegmentContent::Role(Role::Tree)],
            vec![
                SegmentContent::Level(1),
                SegmentContent::Label("Display".to_owned()),
                // The focus is handed to the outpost without the control
                // moving its selection, so the item is not selected.
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::NegatedState(State::Checked),
                SegmentContent::Position {
                    position: 2,
                    set_size: Some(2)
                },
            ],
        ]
    );
    app.quit();
}

/// A tree item outside a tree view control, one of mockapp's scripted
/// nodes, is NVDA's generic outline item: a value that is a number is
/// dropped, any other value kept, and neither gives a level.
fn other_tree_items_keep_a_value_that_is_not_a_number() {
    /// mockapp's scripted nodes, by their index in its tree.
    const INBOX: usize = 2;
    const DRAFTS: usize = 3;
    common::init_com();
    let title = common::unique_title("mockapp-scripted-tree-items");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let host = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());

    let inbox = outpost.msaa_focus(host, INBOX);
    assert_eq!(
        (
            inbox.node.name.as_deref(),
            inbox.node.value.as_deref(),
            inbox.node.details.level
        ),
        (Some("Inbox"), None, None)
    );
    assert_eq!(
        spoken(&inbox),
        [
            vec![
                SegmentContent::Label("Folders".to_owned()),
                SegmentContent::Role(Role::Tree)
            ],
            vec![SegmentContent::Label("Inbox".to_owned())],
        ]
    );

    let drafts = outpost.msaa_focus(host, DRAFTS);
    assert_eq!(
        (
            drafts.node.name.as_deref(),
            drafts.node.value.as_deref(),
            drafts.node.details.level
        ),
        (Some("Drafts"), Some("Unread"), None)
    );
    assert_eq!(
        spoken(&drafts),
        [
            vec![
                SegmentContent::Label("Folders".to_owned()),
                SegmentContent::Role(Role::Tree)
            ],
            vec![
                SegmentContent::Label("Drafts".to_owned()),
                SegmentContent::Value("Unread".to_owned())
            ],
        ]
    );
    app.quit();
}

/// A standard group box encloses a control as its container, as NVDA finds
/// it among the windows before the control, though it is the control's
/// sibling: entering it speaks it ("Synthesizer grouping"); a button
/// outside it has no such container.
fn a_group_box_is_the_context_of_the_controls_inside_it() {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT};
    use windows::core::w;
    common::init_com();
    let title = common::unique_title("mockapp-group-box");
    let app = common::spawn("group_box.json", "msaa", &title);
    let host = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let button = |text: windows::core::PCWSTR| {
        // SAFETY: a local search of the host window's children.
        unsafe { FindWindowExW(Some(host), None, w!("Button"), text) }
            .expect("mockapp made the button")
    };
    let focus = |hwnd: windows::Win32::Foundation::HWND| {
        outpost.focus(verbatim_outpost::protocol::DeliveredFact::MsaaFocus {
            hwnd: hwnd.0 as isize,
            id_object: OBJID_CLIENT.0,
            id_child: 0,
        })
    };

    let change = focus(button(w!("Change")));
    assert_eq!(
        change.chain(),
        [
            Some("Mockapp Group Box Fixture"),
            Some("Synthesizer"),
            Some("Change")
        ]
    );
    assert_eq!(
        spoken(&change),
        [
            vec![
                SegmentContent::Label("Synthesizer".to_owned()),
                SegmentContent::Role(Role::Group)
            ],
            vec![
                SegmentContent::Label("Change".to_owned()),
                SegmentContent::Role(Role::Button)
            ],
        ]
    );

    let apply = focus(button(w!("Apply")));
    assert_eq!(
        apply.chain(),
        [Some("Mockapp Group Box Fixture"), Some("Apply")]
    );
    app.quit();
}

/// The roles of `nodes`, in order.
fn roles(nodes: &[NodeSnapshot]) -> Vec<Role> {
    nodes.iter().map(|node| node.role).collect()
}

fn main() {
    harness::run(&[
        (
            "a_windows_forms_tree_item_is_read_as_a_tree_view_item",
            a_windows_forms_tree_item_is_read_as_a_tree_view_item,
        ),
        (
            "tree_view_items_are_checked_by_their_state_images",
            tree_view_items_are_checked_by_their_state_images,
        ),
        (
            "a_group_box_is_the_context_of_the_controls_inside_it",
            a_group_box_is_the_context_of_the_controls_inside_it,
        ),
        (
            "other_tree_items_keep_a_value_that_is_not_a_number",
            other_tree_items_keep_a_value_that_is_not_a_number,
        ),
    ]);
}
