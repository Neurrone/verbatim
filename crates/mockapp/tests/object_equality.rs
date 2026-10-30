//! NVDA's equality of MSAA objects (`IAccessible._isEqual`) as the outpost
//! applies it to the ancestors its focus walks read
//! (`docs/design/focus-pipeline-rework.md`, "MSAA object equality"), heard
//! through Core: mockapp's MSAA mode over `object_equality.json`, two
//! focuses reported one after the other, the first waiting for Core's
//! judgement when the second is read, so its line is the one compared
//! with, and both reports reduced. What is heard for the second focus is
//! its entered containers, each asserted whole, and then the focus.
//!
//! NVDA's speech for each case is read from its source; the capture session
//! against this fixture is still to be made.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::SrState;
use verbatim_model::{
    Backend, Effect, Input, NormalizedEvent, OutpostId, Pid, Role, SegmentContent, TraceId,
};
use verbatim_outpost::protocol::DeliveredFact;

use common::outpost::{OutpostUnderTest, Reported};

/// The fixture's nodes, by their index in mockapp's tree.
const A: usize = 3;
const B: usize = 4;
const C: usize = 6;
const OK: usize = 8;
const APPLY: usize = 9;
const APPLES: usize = 12;
const PEARS: usize = 13;
/// The child id the root's object answers the Options group at: its third
/// child.
const OPTIONS_CHILD: i32 = 3;

/// The focus `reported` as the event it came in.
fn focus_event(reported: &Reported) -> NormalizedEvent {
    NormalizedEvent::FocusChanged {
        node: reported.node.clone(),
        foreground: false,
        ancestors: reported.ancestors.clone(),
        selected_child: reported.selected_child.clone(),
        text: None,
        earlier: None,
        group: None,
    }
}

/// What Core speaks for `reported`, given what it took before, each
/// utterance's contents in order.
fn spoken(state: &mut SrState, reported: &Reported) -> Vec<Vec<SegmentContent>> {
    let mut event = focus_event(reported);
    event.assign_outpost(OutpostId(1));
    verbatim_core::reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Msaa,
            window: reported.facts.clone(),
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
        _ => None,
    })
    .collect()
}

/// The containers entered with the second of two focuses, each an
/// utterance of its contents, after asserting that the last utterance
/// names `focus`: mockapp is focused on `first` and then on `second`, the
/// focus facts for them `facts` gives, with `between` sent to mockapp
/// after the first focus is reported.
fn entered_with(
    (first, second): (&str, &str),
    facts: impl Fn(isize, &str) -> DeliveredFact,
    between: &[&str],
    focus: &str,
) -> Vec<Vec<SegmentContent>> {
    let title = common::unique_title("mockapp-object-equality");
    let mut app = common::spawn("object_equality.json", "msaa", &title);
    let hwnd = common::find_window(&title).0 as isize;
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();
    app.send(&format!("set-focus {first}"));
    let reported = outpost.focus(facts(hwnd, first));
    let _ = spoken(&mut state, &reported);
    for command in between {
        app.send(command);
    }
    app.send(&format!("set-focus {second}"));
    let reported = outpost.focus(facts(hwnd, second));
    let mut said = spoken(&mut state, &reported);
    let last = said.pop().expect("the focus is spoken");
    assert_eq!(
        last.first(),
        Some(&SegmentContent::Label(focus.into())),
        "the focus comes last"
    );
    app.quit();
    said
}

/// An MSAA focus fact for the node of `id` at its own object id.
fn at_object(hwnd: isize, id: &str) -> DeliveredFact {
    let index = match id {
        "a" => A,
        "b" => B,
        "c" => C,
        "ok" => OK,
        "apply" => APPLY,
        "apples" => APPLES,
        "pears" => PEARS,
        _ => unreachable!("a node the tests focus"),
    };
    DeliveredFact::MsaaFocus {
        hwnd,
        id_object: i32::try_from(index + 1).expect("a small index"),
        id_child: 0,
    }
}

/// The group "Options" said as an entered container.
fn options_grouping() -> Vec<SegmentContent> {
    vec![
        SegmentContent::Label("Options".into()),
        SegmentContent::Role(Role::Group),
    ]
}

/// A container reached by another path, a new sighting of the Options
/// group from its second button, is found equal to the one the first
/// button's line holds, so it is not entered again.
fn a_container_reached_by_another_path_keeps_its_id() {
    let entered = entered_with(("ok", "apply"), at_object, &[], "Apply");
    assert_eq!(entered, Vec::<Vec<SegmentContent>>::new());
}

/// A group renamed around the focused list: the list is found equal to its
/// line entry and the walk stops there, so only the item is said.
fn a_renamed_msaa_group_around_the_focused_list_says_only_the_item() {
    let entered = entered_with(
        ("apples", "pears"),
        at_object,
        &["set-name cart Cart (2)"],
        "Pears",
    );
    assert_eq!(entered, Vec::<Vec<SegmentContent>>::new());
}

/// A renamed group where the lines meet matches by its name read live.
fn a_renamed_msaa_group_where_the_lines_meet_is_not_said() {
    let entered = entered_with(
        ("ok", "apply"),
        at_object,
        &["set-name options Options 2"],
        "Apply",
    );
    assert_eq!(entered, Vec::<Vec<SegmentContent>>::new());
}

/// A second unnamed list, alike in role and name but at another place, is
/// another object, and is entered.
fn a_second_unnamed_msaa_container_is_announced() {
    let entered = entered_with(("a", "c"), at_object, &[], "c");
    assert_eq!(entered, vec![vec![SegmentContent::Role(Role::List)]]);
}

/// A group scrolled between two focuses matches its line entry by its
/// location read live.
fn a_scrolled_msaa_group_is_matched_live() {
    let entered = entered_with(
        ("ok", "apply"),
        at_object,
        &["move options 10 350 300 100"],
        "Apply",
    );
    assert_eq!(entered, Vec::<Vec<SegmentContent>>::new());
}

/// A group destroyed and made again in its place is another object: the
/// kept object's live location fails as gone.
fn a_recreated_msaa_group_is_announced() {
    let entered = entered_with(("ok", "apply"), at_object, &["recreate options"], "Apply");
    assert_eq!(entered, vec![options_grouping()]);
}

/// Groups whose locations cannot be read, on either side, meet by their
/// other facts, as NVDA's two `None` locations are equal.
fn msaa_groups_whose_locations_fail_meet_by_their_other_facts() {
    let entered = entered_with(
        ("ok", "apply"),
        at_object,
        &["fail-acc-location options"],
        "Apply",
    );
    assert_eq!(entered, Vec::<Vec<SegmentContent>>::new());
}

/// A group focused through an event at its child id on the root's object,
/// which answers it as a full object, has that event child id; reached
/// later from its button through `accParent`, at child id 0, it is another
/// object, and is entered again.
fn a_container_focused_at_an_event_child_id_is_entered_from_its_child() {
    let facts = |hwnd, id: &str| match id {
        "options" => DeliveredFact::MsaaFocus {
            hwnd,
            id_object: 1,
            id_child: OPTIONS_CHILD,
        },
        other => at_object(hwnd, other),
    };
    let entered = entered_with(("options", "ok"), facts, &[], "OK");
    assert_eq!(entered, vec![options_grouping()]);
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_container_reached_by_another_path_keeps_its_id",
            a_container_reached_by_another_path_keeps_its_id,
        ),
        (
            "a_renamed_msaa_group_around_the_focused_list_says_only_the_item",
            a_renamed_msaa_group_around_the_focused_list_says_only_the_item,
        ),
        (
            "a_renamed_msaa_group_where_the_lines_meet_is_not_said",
            a_renamed_msaa_group_where_the_lines_meet_is_not_said,
        ),
        (
            "a_second_unnamed_msaa_container_is_announced",
            a_second_unnamed_msaa_container_is_announced,
        ),
        (
            "a_scrolled_msaa_group_is_matched_live",
            a_scrolled_msaa_group_is_matched_live,
        ),
        (
            "a_recreated_msaa_group_is_announced",
            a_recreated_msaa_group_is_announced,
        ),
        (
            "msaa_groups_whose_locations_fail_meet_by_their_other_facts",
            msaa_groups_whose_locations_fail_meet_by_their_other_facts,
        ),
        (
            "a_container_focused_at_an_event_child_id_is_entered_from_its_child",
            a_container_focused_at_an_event_child_id_is_entered_from_its_child,
        ),
    ]);
}
