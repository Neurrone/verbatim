//! MSAA change events from mockapp's scripted provider, handled by a real
//! outpost in this process through its own hooks and spoken by the reducer:
//! what NVDA's base handlers speak for a description change on the focus,
//! and that a change on any other object says nothing (`docs/nvda/events.md`,
//! "The focus gate"), a tree view item's logical parent included: NVDA's
//! ancestors are those reached through `accParent`. A state change on
//! such an ancestor is spoken: having no address of its own and no COM
//! identity two sightings share (oleacc wraps every object anew), it is
//! recognized by NVDA's comparison of window, role, name, and location,
//! and an object that differs in location alone is not it.
//!
//! mockapp's focus moves with `set-focus`, which raises no event; the
//! test hands the outpost the focus itself, as `call_counts.rs` does.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::SrState;
use verbatim_model::{
    Backend, Earcon, Effect, Input, NormalizedEvent, OutpostId, Pid, SegmentContent, State, TraceId,
};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor};

use common::outpost::{OutpostUnderTest, Reported};

/// The counts fixture's second list item, by its index in mockapp's tree.
const ITEM_TWO: usize = 6;

/// Gives Core `event` from mockapp, its node ids stamped with outpost 1,
/// and returns the content of each utterance spoken, in order. A focus's
/// drop of expired speech is the only other effect allowed.
fn spoken(state: &mut SrState, mut event: NormalizedEvent) -> Vec<Vec<SegmentContent>> {
    event.assign_outpost(OutpostId(1));
    verbatim_core::reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            source: Pid(1),
            backend: Backend::Msaa,
            window: None,
            event,
        },
    )
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
        other => panic!("an event makes no {other:?}"),
    })
    .collect()
}

/// The focus `reported` as the event it came in.
fn focus_event(reported: &Reported) -> NormalizedEvent {
    NormalizedEvent::FocusChanged {
        node: reported.node.clone(),
        foreground: false,
        ancestors: reported.ancestors.clone(),
        ancestors_unknown: false,
        selected_child: reported.selected_child.clone(),
    }
}

/// The outpost's next message, which must be an event, as its event.
fn next_event(outpost: &OutpostUnderTest) -> NormalizedEvent {
    match outpost.next() {
        OutpostToSupervisor::Event { event, .. } => event,
        other => panic!("the outpost said {other:?}, not an event"),
    }
}

/// A description change on the focus speaks the new description; a state
/// change on the group beside the focus's list, neither the focus nor an
/// ancestor, is not even reported, as the description change after it is
/// the next thing the outpost says.
fn a_description_change_on_the_focus_is_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-events");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut state = SrState::new();

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    assert_eq!(
        spoken(&mut state, focus_event(&reported)),
        [
            vec![
                SegmentContent::Label("Options".to_owned()),
                SegmentContent::Role(verbatim_model::Role::List)
            ],
            vec![
                SegmentContent::Label("Two".to_owned()),
                SegmentContent::NegatedState(State::Selected)
            ],
        ]
    );

    app.send("set-description item2 Second choice");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Description(
            "Second choice".to_owned()
        )]]
    );

    app.send("set-states group expanded");
    app.send("set-description item2 Third choice");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Description("Third choice".to_owned())]],
        "the group's change was not reported"
    );
    app.quit();
}

/// A state change on an ancestor of the focus reached through `accParent`,
/// NVDA's ancestry, is spoken: the focused list item's list becomes
/// unavailable, and says so. The list has no address of its own the
/// outpost reached it at, and oleacc wraps every object it hands out anew,
/// so neither address nor identity can tell the event's object is that
/// ancestor: they are compared as NVDA compares them, by window, role,
/// name, and location.
fn a_state_change_on_an_acc_parent_ancestor_is_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-acc-parent-state");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut state = SrState::new();

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("set-states list focusable disabled");
    let changed = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, changed),
        [vec![SegmentContent::State(State::Disabled)]]
    );
    app.quit();
}

/// An object like the focus's windowless ancestor in all but its location
/// is not that ancestor: two lists named "Options" side by side, and the
/// focus in the left one. The right list's state change is not reported;
/// the left list's, raised after it from the same hook, is the first thing
/// the outpost says.
fn the_same_name_in_another_location_is_not_the_ancestor() {
    /// The left list's item, by its index in mockapp's tree.
    const LEFT_ONE: usize = 2;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-twin-lists");
    let mut app = common::spawn("twin_lists.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut state = SrState::new();

    app.send("set-focus left_one");
    let reported = outpost.msaa_focus(hwnd, LEFT_ONE);
    assert_eq!(reported.chain(), [Some("Options"), Some("One")]);
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("set-states right focusable disabled");
    app.send("set-states left focusable disabled");
    let changed = next_event(&outpost);
    let NormalizedEvent::PropertyChanged { node_id, .. } = &changed else {
        panic!("the outpost said {changed:?}, not a state change");
    };
    assert_eq!(
        Some(*node_id),
        reported.ancestors.last().map(|list| list.id),
        "the left list's change, not the right one's"
    );
    outpost.settled();
    assert_eq!(
        spoken(&mut state, changed),
        [vec![SegmentContent::State(State::Disabled)]]
    );
    app.quit();
}

/// Collapsing the item the focused tree item is in says nothing of the
/// collapsed item until it has the focus. It is the focused item's logical
/// parent, read through the control's messages, not an ancestor NVDA has:
/// NVDA's ancestors are those reached through `accParent`, and a tree view
/// item's is the control, so neither the control moving its selection to
/// the collapsed item nor the item's state change is spoken. The control's
/// focus on it then says its announcement, collapsed.
fn collapsing_a_tree_items_parent_says_nothing_until_its_focus() {
    use windows::Win32::UI::Controls::{TVE_COLLAPSE, TVM_EXPAND};
    common::init_com();
    let title = common::unique_title("mockapp-msaa-ancestor-state");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::new(app.pid());
    let mut state = SrState::new();

    let reported = common::tree_view::focus_item(&outpost, tree, "Disks");
    let _ = spoken(&mut state, focus_event(&reported));
    let hardware = common::tree_view::item(tree, "Hardware");
    common::tree_view::send(tree, TVM_EXPAND, TVE_COLLAPSE.0 as usize, hardware);
    // The selection and the state change say nothing: the parent's focus
    // is the first thing the outpost says.
    let focus = common::tree_view::focus_item(&outpost, tree, "Hardware");
    assert_eq!(
        spoken(&mut state, focus_event(&focus)),
        [vec![
            SegmentContent::Level(0),
            SegmentContent::Label("Hardware".to_owned()),
            SegmentContent::State(State::Mixed),
            SegmentContent::State(State::Collapsed),
            SegmentContent::Position {
                position: 1,
                set_size: Some(3)
            },
        ]]
    );
    app.quit();
}

/// A progress bar that is not the focus indicates each change of its
/// percentage by a tone at once, as NVDA's progress bar behavior beeps,
/// when it moves by at least one percent; a value that is not a number is
/// an ordinary value change, silent off the focus.
fn a_progress_bar_off_the_focus_indicates_its_percentage() {
    /// mockapp's scripted Inbox item, by its index in its tree.
    const INBOX: usize = 2;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-progress");
    let mut app = common::spawn("tree_view.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(app.pid());
    let mut state = SrState::new();
    let reported = outpost.msaa_focus(hwnd, INBOX);
    let _ = spoken(&mut state, focus_event(&reported));

    let mut indicated = |value: &str| {
        app.send(&format!("set-value copying {value}"));
        let mut event = next_event(&outpost);
        outpost.settled();
        assert!(
            matches!(event, NormalizedEvent::ProgressChanged { .. }),
            "{event:?} reports a progress bar"
        );
        event.assign_outpost(OutpostId(1));
        verbatim_core::reduce(
            &mut state,
            &Input::Event {
                trace_id: TraceId::mint(),
                observed_at_ms: 0,
                source: Pid(1),
                backend: Backend::Msaa,
                window: None,
                event,
            },
        )
    };
    assert_eq!(indicated("40"), [Effect::PlayEarcon(Earcon::Progress(40))]);
    assert_eq!(indicated("40.5"), [], "less than a percent from the last");
    assert_eq!(indicated("55%"), [Effect::PlayEarcon(Earcon::Progress(55))]);
    assert_eq!(indicated("Paused"), [], "not a number, and not the focus");
    app.quit();
}

/// A tooltip window's show event (which the listener forwards only from
/// `tooltips_class32` windows) is spoken as NVDA's notification behavior
/// speaks a help balloon, which NVDA reports by default; an ordinary
/// tooltip is not, as NVDA does not report tooltips by default.
fn a_help_balloon_shown_is_spoken() {
    /// mockapp's scripted help balloon and tooltip, by their index in its
    /// tree.
    const BALLOON: usize = 5;
    const TIP: usize = 6;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-help-balloon");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let outpost = OutpostUnderTest::new(app.pid());
    let show = |index: usize| DeliveredFact::Show {
        hwnd,
        id_object: i32::try_from(index + 1).expect("a small index"),
        id_child: 0,
    };

    outpost.deliver(show(TIP));
    outpost.settled();
    outpost.deliver(show(BALLOON));
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut SrState::new(), event),
        [vec![
            SegmentContent::Label("Updates are ready".to_owned()),
            SegmentContent::Role(verbatim_model::Role::HelpBalloon)
        ]]
    );
    app.quit();
}

fn main() {
    harness::run(&[
        (
            "a_help_balloon_shown_is_spoken",
            a_help_balloon_shown_is_spoken,
        ),
        (
            "a_progress_bar_off_the_focus_indicates_its_percentage",
            a_progress_bar_off_the_focus_indicates_its_percentage,
        ),
        (
            "a_description_change_on_the_focus_is_spoken",
            a_description_change_on_the_focus_is_spoken,
        ),
        (
            "a_state_change_on_an_acc_parent_ancestor_is_spoken",
            a_state_change_on_an_acc_parent_ancestor_is_spoken,
        ),
        (
            "the_same_name_in_another_location_is_not_the_ancestor",
            the_same_name_in_another_location_is_not_the_ancestor,
        ),
        (
            "collapsing_a_tree_items_parent_says_nothing_until_its_focus",
            collapsing_a_tree_items_parent_says_nothing_until_its_focus,
        ),
    ]);
}
