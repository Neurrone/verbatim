//! Real Win32 controls inside mockapp, read through their own MSAA
//! implementations by a real outpost in this process, as Verbatim reads the
//! controls of real applications: a comctl32 tree view registered under a
//! Windows Forms class name (`tests/fixtures/tree_view.json`), with check
//! boxes.
//!
//! mockapp's scripted root, the tree view's parent, has the focused state,
//! so a focus told to Core passes NVDA's focused-state check without the
//! control taking the keyboard focus from the desktop the test runs on.
//! Core, driving the outpost (`common::core_driver`), speaks it, and the
//! speech is asserted whole; what the outpost answers for the focus is read
//! again through the request driver, with none of the ancestors Core knows,
//! for the facts the speech does not show.
//!
//! The window boundary, both ways, as NVDA switches API there
//! (`correctAPIForRelation`): a Win32 button inside a window whose UIA
//! provider is native (`tests/fixtures/boundary_uia_host.json`), whose
//! MSAA ancestors continue as that window's UIA element, presented only
//! when it is a control and content (`boundary_uia_layout_host.json` says
//! it is not); and a UIA window inside a plain Win32 window
//! (`boundary_win32_host.json`, `--backend win32`), whose UIA ancestors
//! continue as the Win32 window's MSAA client area, presented by its title
//! as a top-level window's client with a system menu.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyId, PropertyValue, PropertyValues};
use verbatim_model::request::{FocusAncestor, MsaaIds};
use verbatim_model::{Role, SegmentContent, State, StateSet, WindowHandle};
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT};
use windows::core::{PCWSTR, w};

use common::core_driver::CoreDriver;
use common::outpost::{Focused, OutpostUnderTest};
use common::tree_view::{child_id, tree_view};

/// What Core said, each utterance's contents in order.
type Said = Vec<Vec<SegmentContent>>;

/// mockapp with `fixture` on `backend`, titled from `prefix`, its window,
/// and Core driving an outpost watching it.
fn started(fixture: &str, backend: &str, prefix: &str) -> (common::MockApp, HWND, CoreDriver) {
    common::init_com();
    let title = common::unique_title(prefix);
    let app = common::spawn(fixture, backend, &title);
    let hwnd = common::find_window(&title);
    let core = CoreDriver::new(&app, OutpostUnderTest::new(&app));
    (app, hwnd, core)
}

/// What Core says of an MSAA focus mockapp raised at `object` and `child`
/// of `window`, and what the outpost answers for that focus.
fn msaa_heard(core: &mut CoreDriver, window: HWND, (object, child): (i32, i32)) -> (Said, Focused) {
    let said = core
        .msaa_event(EventKind::MsaaFocus, window, (object, child))
        .spoken();
    let focused = core.outpost.msaa_focus_answer(MsaaIds {
        window: WindowHandle(window.0 as u64),
        object,
        child,
    });
    (said, focused)
}

/// The text value `id` of `values`.
fn text(values: &PropertyValues, id: PropertyId) -> Option<&str> {
    match common::outpost::value(values, id) {
        Some(PropertyValue::Text(text)) => Some(text),
        _ => None,
    }
}

/// The count value `id` of `values`.
fn count(values: &PropertyValues, id: PropertyId) -> Option<u32> {
    match common::outpost::value(values, id) {
        Some(PropertyValue::Count(count)) => Some(*count),
        _ => None,
    }
}

/// The states of `values`.
fn states_of(values: &PropertyValues) -> StateSet {
    match common::outpost::value(values, PropertyId::States) {
        Some(PropertyValue::States(states)) => *states,
        _ => StateSet::default(),
    }
}

/// The roles of `ancestors`, outermost first.
fn roles(ancestors: &[FocusAncestor]) -> Vec<Role> {
    ancestors
        .iter()
        .rev()
        .map(|ancestor| common::outpost::role_of(&ancestor.node))
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
    let (app, host, mut core) = started("tree_view.json", "msaa", "mockapp-native-tree-focus");
    let tree = tree_view(host);
    let (said, focused) = msaa_heard(&mut core, tree, (OBJID_CLIENT.0, child_id(tree, "Disks")));
    let values = &focused.read.focus.read.values;
    assert_eq!(
        (
            focused.role(),
            focused.name(),
            text(values, PropertyId::Value)
        ),
        (Role::TreeItem, Some("Disks"), None),
        "a tree view item, whose value is its level"
    );
    assert_eq!(
        focused.states(),
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
            count(values, PropertyId::Level),
            count(values, PropertyId::PositionInSet),
            count(values, PropertyId::SetSize)
        ),
        (Some(1), Some(1), Some(2)),
        "the level and the position among the item's siblings"
    );
    // Hardware is the item's logical parent, through the control's own
    // messages; the tree view's window object is layout.
    assert_eq!(
        focused.chain(),
        [
            Some("Mockapp Tree View Fixture"),
            None,
            Some("Hardware"),
            Some("Disks")
        ]
    );
    assert_eq!(
        roles(&focused.read.ancestors),
        [Role::Unknown, Role::Tree, Role::TreeItem]
    );
    assert_eq!(
        said,
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
    drop(core);
    app.quit();
}

/// A tree view that draws its own check boxes says through each item's
/// state image whether it is checked, which NVDA's tree view item reads
/// (`TVM_GETITEMSTATE`): an item with an empty box is checkable and not
/// checked, one with a partly filled box is half checked.
fn tree_view_items_are_checked_by_their_state_images() {
    let (app, host, mut core) = started("tree_view.json", "msaa", "mockapp-native-tree-checks");
    let tree = tree_view(host);
    let (said, focused) = msaa_heard(&mut core, tree, (OBJID_CLIENT.0, child_id(tree, "Display")));
    assert_eq!(
        focused.states(),
        states(&[State::Focusable, State::Selectable, State::Checkable])
    );
    let hardware = focused.read.ancestors.first().expect("the item's parent");
    assert_eq!(common::outpost::name_of(&hardware.node), Some("Hardware"));
    assert_eq!(
        states_of(&hardware.node.read.values),
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
        said,
        [
            vec![SegmentContent::Role(Role::Tree)],
            vec![
                SegmentContent::Level(1),
                SegmentContent::Label("Display".to_owned()),
                // The focus is told without the control moving its
                // selection, so the item is not selected.
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::NegatedState(State::Checked),
                SegmentContent::Position {
                    position: 2,
                    set_size: Some(2)
                },
            ],
        ]
    );
    drop(core);
    app.quit();
}

/// A tree item outside a tree view control, one of mockapp's scripted
/// nodes, is NVDA's generic outline item: a value that is a number is
/// dropped, any other value kept, and neither gives a level. Each is heard
/// by a Core of its own, as the first focus in the tree.
fn other_tree_items_keep_a_value_that_is_not_a_number() {
    /// mockapp's scripted nodes, by their index in its tree.
    const INBOX: usize = 2;
    const DRAFTS: usize = 3;
    let heard = |index: usize| {
        let (app, host, mut core) =
            started("tree_view.json", "msaa", "mockapp-scripted-tree-items");
        let object = i32::try_from(index + 1).expect("a small index");
        let (said, focused) = msaa_heard(&mut core, host, (object, 0));
        let values = &focused.read.focus.read.values;
        let facts = (
            focused.name().map(str::to_owned),
            text(values, PropertyId::Value).map(str::to_owned),
            count(values, PropertyId::Level),
        );
        drop(core);
        app.quit();
        (facts, said)
    };
    assert_eq!(
        heard(INBOX),
        (
            (Some("Inbox".to_owned()), None, None),
            vec![
                vec![
                    SegmentContent::Label("Folders".to_owned()),
                    SegmentContent::Role(Role::Tree)
                ],
                vec![SegmentContent::Label("Inbox".to_owned())],
            ]
        )
    );
    assert_eq!(
        heard(DRAFTS),
        (
            (Some("Drafts".to_owned()), Some("Unread".to_owned()), None),
            vec![
                vec![
                    SegmentContent::Label("Folders".to_owned()),
                    SegmentContent::Role(Role::Tree)
                ],
                vec![
                    SegmentContent::Label("Drafts".to_owned()),
                    SegmentContent::Value("Unread".to_owned())
                ],
            ]
        )
    );
}

/// The button of `host` titled `text`.
fn button(host: HWND, text: PCWSTR) -> HWND {
    // SAFETY: a local search of the host window's children.
    unsafe { FindWindowExW(Some(host), None, w!("Button"), text) }.expect("mockapp made the button")
}

/// A standard group box encloses a control as its container, as NVDA finds
/// it among the windows before the control, though it is the control's
/// sibling: entering it speaks it ("Synthesizer grouping"); a button
/// outside it has no such container.
fn a_group_box_is_the_context_of_the_controls_inside_it() {
    let (app, host, mut core) = started("group_box.json", "msaa", "mockapp-group-box");
    let (said, change) = msaa_heard(&mut core, button(host, w!("Change")), (OBJID_CLIENT.0, 0));
    assert_eq!(
        change.chain(),
        [
            Some("Mockapp Group Box Fixture"),
            Some("Synthesizer"),
            Some("Change")
        ]
    );
    assert_eq!(
        said,
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
    core.outpost.forget_focus();
    let (_, apply) = msaa_heard(&mut core, button(host, w!("Apply")), (OBJID_CLIENT.0, 0));
    assert_eq!(
        apply.chain(),
        [Some("Mockapp Group Box Fixture"), Some("Apply")]
    );
    drop(core);
    app.quit();
}

/// The list view of `host`.
fn list_view(host: HWND) -> HWND {
    // SAFETY: a local search of the host window's children.
    unsafe { FindWindowExW(Some(host), None, w!("SysListView32"), PCWSTR::null()) }
        .expect("mockapp made the list view")
}

/// An item of a list view in the report view is named by its columns, in
/// the order shown, each after its header but the first, a column of zero
/// width or with no text left out, as NVDA names it; it has no value or
/// description of its own.
fn a_report_view_item_is_named_by_its_columns() {
    let (app, host, mut core) = started("list_view.json", "msaa", "mockapp-list-view");
    let list = list_view(host);
    let (said, readme) = msaa_heard(&mut core, list, (OBJID_CLIENT.0, 1));
    let values = &readme.read.focus.read.values;
    assert_eq!(
        (
            readme.name(),
            text(values, PropertyId::Value),
            text(values, PropertyId::Description)
        ),
        (
            Some("readme.txt; Size: 1 KB; Type: Text Document"),
            None,
            None
        )
    );
    assert_eq!(
        said,
        [
            vec![SegmentContent::Role(Role::List)],
            vec![
                SegmentContent::Label("readme.txt; Size: 1 KB; Type: Text Document".to_owned()),
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(2)
                },
            ],
        ]
    );
    let (_, notes) = msaa_heard(&mut core, list, (OBJID_CLIENT.0, 2));
    assert_eq!(
        notes.name(),
        Some("notes.md; Type: Markdown"),
        "an empty column is left out"
    );
    drop(core);
    app.quit();
}

/// A list view item's name with left-to-right marks between its numbers,
/// as File Explorer's dates have, is read without them, as NVDA reads it:
/// the focus speaks the date alone, and the review cursor's next character
/// from the start is its second digit.
fn bidirectional_marks_are_stripped_from_a_name() {
    let (app, host, mut core) = started("bidi_marks.json", "msaa", "mockapp-bidi-marks");
    let (said, date) = msaa_heard(&mut core, list_view(host), (OBJID_CLIENT.0, 1));
    assert_eq!(date.name(), Some("08/10/2026"));
    assert_eq!(
        said,
        [
            vec![SegmentContent::Role(Role::List)],
            vec![
                SegmentContent::Label("08/10/2026".to_owned()),
                SegmentContent::NegatedState(State::Selected),
                SegmentContent::Position {
                    position: 1,
                    set_size: Some(1)
                },
            ],
        ]
    );
    // Flat review reads the object live; here it has not changed.
    assert_eq!(
        core.command(verbatim_model::ReviewCommand::ReviewNextCharacter)
            .spoken(),
        [vec![SegmentContent::Text("8".to_owned())]]
    );
    drop(core);
    app.quit();
}

/// Left Arrow from a tree item to its parent selects the parent and moves
/// the focus to it. The parent is the item's logical ancestor, through the
/// control's own messages, but NVDA's ancestors are those reached through
/// `accParent`, of which a tree view item's is the control: the parent's
/// state change as it is selected is not spoken, and moving to it says
/// only its announcement.
fn moving_to_a_tree_items_parent_says_only_the_parent() {
    use verbatim_ia2::WinEventKind;
    use windows::Win32::UI::Controls::{TVGN_CARET, TVM_SELECTITEM};
    let (app, host, mut core) = started("tree_view.json", "msaa", "mockapp-native-tree-parent");
    let tree = tree_view(host);
    let _ = core.msaa_event(
        EventKind::MsaaFocus,
        tree,
        (OBJID_CLIENT.0, child_id(tree, "Disks")),
    );
    // The control's focus on the parent, as the selection raises it.
    let raised = common::core_driver::win_events(
        app.pid(),
        || {
            common::tree_view::send(
                tree,
                TVM_SELECTITEM,
                TVGN_CARET as usize,
                common::tree_view::item(tree, "Hardware"),
            );
        },
        &[WinEventKind::Focus],
        1,
    );
    assert_eq!(
        core.win_event(&raised[0]).spoken(),
        [vec![
            SegmentContent::Level(0),
            SegmentContent::Label("Hardware".to_owned()),
            SegmentContent::State(State::Mixed),
            SegmentContent::State(State::Expanded),
            SegmentContent::Position {
                position: 1,
                set_size: Some(3)
            },
        ]]
    );
    drop(core);
    app.quit();
}

/// A Win32 button inside a window whose UIA provider is native: its MSAA
/// ancestors cross into the window, which is read as its UIA element, a
/// named window that is a control and content, and so is spoken before the
/// button as an entered container.
fn a_win32_control_in_a_uia_window_has_the_windows_element_as_context() {
    let (app, host, mut core) =
        started("boundary_uia_host.json", "uia", "mockapp-boundary-uia-host");
    let (said, apply) = msaa_heard(&mut core, button(host, w!("Apply")), (OBJID_CLIENT.0, 0));
    assert_eq!(
        apply.chain(),
        [Some("Mockapp Boundary Fixture"), Some("Apply")]
    );
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Boundary Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                SegmentContent::Label("Apply".to_owned()),
                SegmentContent::Role(Role::Button)
            ],
        ]
    );
    drop(core);
    app.quit();
}

/// The same button, in a UIA window that UIA does not count as content:
/// the window is layout by NVDA's rule for a UIA object, so it is not among
/// the button's ancestors, and the button is spoken alone.
fn a_uia_window_that_is_not_content_is_no_context_for_a_win32_control() {
    let (app, host, mut core) = started(
        "boundary_uia_layout_host.json",
        "uia",
        "mockapp-boundary-uia-layout",
    );
    let (said, apply) = msaa_heard(&mut core, button(host, w!("Apply")), (OBJID_CLIENT.0, 0));
    assert_eq!(apply.chain(), [Some("Apply")]);
    assert_eq!(
        said,
        [vec![
            SegmentContent::Label("Apply".to_owned()),
            SegmentContent::Role(Role::Button)
        ]]
    );
    drop(core);
    app.quit();
}

/// A UIA window inside a plain Win32 window: the focus read by the
/// listener's reader goes up through the UIA window's root, a named
/// grouping, and on past it to the Win32 window, whose client area, read
/// through MSAA, is a pane that stands for a top-level window with a
/// system menu. The host window is hidden here, so its client area is
/// invisible and, as NVDA counts an invisible object unavailable, not
/// spoken (`docs/parity/focus-context.md`, "Focus-ancestry context"): the
/// grouping is spoken, then the button.
fn a_uia_window_in_a_win32_window_has_the_windows_client_as_context() {
    common::init_com();
    let title = common::unique_title("mockapp-boundary-win32-host");
    let app = common::spawn("boundary_win32_host.json", "win32", &title);
    let host = common::find_window(&title);
    let mut core = CoreDriver::new(&app, OutpostUnderTest::new(&app));

    // The host window is hidden, and UIA lists no hidden child window among
    // its children, so the focus is found from the UIA window itself; its
    // parent is still the host's element, as the listener's read finds it.
    // SAFETY: a local search of the host window's children.
    let island = unsafe { FindWindowExW(Some(host), None, w!("VerbatimMockAppUiaWindow"), None) }
        .expect("mockapp made the UIA window");
    let uia = Uia::new().expect("a UIA client");
    let ok = common::listener_focus(&uia, island.0 as isize);
    let said = core.uia_focus(&ok).spoken();
    let focused = core.outpost.uia_focus_answer(&ok);
    assert_eq!(
        focused.chain(),
        [Some(title.as_str()), Some("Embedded"), Some("OK")]
    );
    let client = focused
        .read
        .ancestors
        .last()
        .expect("the host's client area");
    assert!(
        states_of(&client.node.read.values).contains(State::Invisible),
        "the hidden window's client area, invisible"
    );
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Embedded".to_owned()),
                SegmentContent::Role(Role::Group)
            ],
            vec![
                SegmentContent::Label("OK".to_owned()),
                SegmentContent::Role(Role::Button)
            ],
        ]
    );
    drop(core);
    app.quit();
}

/// The roles and class names of `ancestors`, outermost first.
fn classes(ancestors: &[FocusAncestor]) -> Vec<(Role, Option<String>)> {
    ancestors
        .iter()
        .rev()
        .map(|ancestor| {
            (
                common::outpost::role_of(&ancestor.node),
                text(&ancestor.node.read.values, PropertyId::ClassName).map(str::to_owned),
            )
        })
        .collect()
}

/// A UI Automation ancestor carries its class name from the walk's cache,
/// so Core speaks a slider of class `SensitiveSlider` above the focus as a
/// focus is, its value included (`docs/parity/focus-context.md`, "A slider
/// that focuses its thumb"): mockapp's "Volume" slider of that class, the
/// focus on the "Position" button inside it.
fn a_uia_ancestors_class_makes_a_sensitive_slider_speak_its_value() {
    let (mut app, hwnd, mut core) =
        started("sensitive_slider.json", "uia", "mockapp-sensitive-slider");
    app.send("set-focus thumb");
    let uia = Uia::new().expect("a UIA client");
    let thumb = common::listener_focus(&uia, hwnd.0 as isize);
    let said = core.uia_focus(&thumb).spoken();
    let expected = [
        (Role::Window, Some("VerbatimMockAppWindow".to_owned())),
        (Role::Slider, Some("SensitiveSlider".to_owned())),
    ];
    let focused = core.outpost.uia_focus_answer(&thumb);
    assert_eq!(
        classes(&focused.read.ancestors),
        expected,
        "each ancestor's class, from the listener's walk"
    );
    // The outpost's own walk, for the current focus.
    core.outpost.forget_focus();
    core.outpost.read_focus_as(thumb.live().element());
    let (outcome, calls) = core.outpost.current_focus();
    let now = Focused::of(outcome, calls);
    assert_eq!(
        classes(&now.read.ancestors),
        expected,
        "each ancestor's class, from the outpost's walk"
    );
    assert_eq!(
        said,
        [
            vec![
                SegmentContent::Label("Mockapp Sensitive Slider Fixture".to_owned()),
                SegmentContent::Role(Role::Window)
            ],
            vec![
                SegmentContent::Label("Volume".to_owned()),
                SegmentContent::Role(Role::Slider),
                SegmentContent::Value("50".to_owned())
            ],
            vec![
                SegmentContent::Label("Position".to_owned()),
                SegmentContent::Role(Role::Button)
            ],
        ]
    );
    drop(core);
    app.quit();
}

/// Core speaks a `SensitiveSlider`'s value change while its thumb is the
/// focus, told by the outpost following it with the value it carries
/// (`docs/parity/focus-context.md`, "A slider that focuses its thumb"):
/// mockapp's slider changes its value with its thumb focused, and Core
/// says the value.
fn a_sensitive_slider_s_value_change_reaches_core_while_its_thumb_is_the_focus() {
    let (mut app, hwnd, mut core) = started(
        "sensitive_slider.json",
        "uia",
        "mockapp-sensitive-slider-value",
    );
    app.send("set-focus thumb");
    let uia = Uia::new().expect("a UIA client");
    let thumb = common::listener_focus(&uia, hwnd.0 as isize);
    let _ = core.uia_focus(&thumb);
    app.send("set-value slider 60");
    assert_eq!(
        core.told(1).spoken(),
        [vec![SegmentContent::Value("60".to_owned())]]
    );
    drop(core);
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_uia_ancestors_class_makes_a_sensitive_slider_speak_its_value",
            a_uia_ancestors_class_makes_a_sensitive_slider_speak_its_value,
        ),
        (
            "a_sensitive_slider_s_value_change_reaches_core_while_its_thumb_is_the_focus",
            a_sensitive_slider_s_value_change_reaches_core_while_its_thumb_is_the_focus,
        ),
        (
            "a_windows_forms_tree_item_is_read_as_a_tree_view_item",
            a_windows_forms_tree_item_is_read_as_a_tree_view_item,
        ),
        (
            "tree_view_items_are_checked_by_their_state_images",
            tree_view_items_are_checked_by_their_state_images,
        ),
        (
            "a_report_view_item_is_named_by_its_columns",
            a_report_view_item_is_named_by_its_columns,
        ),
        (
            "bidirectional_marks_are_stripped_from_a_name",
            bidirectional_marks_are_stripped_from_a_name,
        ),
        (
            "a_group_box_is_the_context_of_the_controls_inside_it",
            a_group_box_is_the_context_of_the_controls_inside_it,
        ),
        (
            "other_tree_items_keep_a_value_that_is_not_a_number",
            other_tree_items_keep_a_value_that_is_not_a_number,
        ),
        (
            "moving_to_a_tree_items_parent_says_only_the_parent",
            moving_to_a_tree_items_parent_says_only_the_parent,
        ),
        (
            "a_win32_control_in_a_uia_window_has_the_windows_element_as_context",
            a_win32_control_in_a_uia_window_has_the_windows_element_as_context,
        ),
        (
            "a_uia_window_that_is_not_content_is_no_context_for_a_win32_control",
            a_uia_window_that_is_not_content_is_no_context_for_a_win32_control,
        ),
        (
            "a_uia_window_in_a_win32_window_has_the_windows_client_as_context",
            a_uia_window_in_a_win32_window_has_the_windows_client_as_context,
        ),
    ]);
}
