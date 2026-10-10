//! MSAA change events from mockapp's scripted provider, handled by a real
//! outpost in this process through its own hooks and spoken by the reducer:
//! what NVDA's base handlers speak for a description change on the focus,
//! and that a change on any other object says nothing (`docs/nvda/events.md`,
//! "The focus gate"), a tree view item's logical parent included: NVDA's
//! ancestors are those reached through `accParent`. A state change on
//! such an ancestor with no address of its own is not spoken either, as
//! NVDA does not speak it: nothing NVDA does meets that ancestor again
//! from an event.
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
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, Query};

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
    let outpost = OutpostUnderTest::new(&app);
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

/// A modal dialog's owner disabled as the dialog opens says nothing: a
/// state change on the foreground window the focus is in, an ancestor the
/// outpost knows at its own address, the window's client area, is not
/// spoken, as NVDA, checked live, never speaks it (`docs/parity.md`, "A
/// top-level window's state change"). The window is the foreground as
/// the focus enters it, and is disabled while the focus is still on its
/// Remove button; the description change on the button after it is the
/// next thing the outpost says, whatever the timing.
fn disabling_the_focus_window_is_not_spoken() {
    /// The Remove button, by its index in mockapp's tree.
    const REMOVE: usize = 1;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-modal-owner");
    let mut app = common::spawn("modal_owner.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("client-identity");
    app.send("set-focus remove");
    let reported = outpost.msaa_focus(hwnd, REMOVE);
    assert_eq!(reported.chain(), [Some("Settings"), Some("Remove")]);
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("disable-client");
    app.send("set-description remove Removes the theme");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Description(
            "Removes the theme".to_owned()
        )]],
        "the window's change was not reported"
    );
    app.quit();
}

/// A top-level window that was not the foreground as the focus entered
/// it, as a popup menu's never is, keeps its place among the focus's
/// ancestors, as in NVDA, whose foreground object is another window's: its
/// state change is spoken.
fn disabling_a_window_not_in_the_foreground_is_spoken() {
    /// The Remove button, by its index in mockapp's tree.
    const REMOVE: usize = 1;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-background-owner");
    let mut app = common::spawn("modal_owner.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();
    // mockapp's window stands for a popup menu's: topmost, and not the
    // foreground window, which here is none.
    common::make_topmost(hwnd);
    outpost.set_foreground(windows::Win32::Foundation::HWND::default());

    app.send("client-identity");
    app.send("set-focus remove");
    let reported = outpost.msaa_focus(hwnd, REMOVE);
    assert_eq!(reported.chain(), [Some("Settings"), Some("Remove")]);
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("disable-client");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::State(State::Disabled)]]
    );
    app.quit();
}

/// What Core does with an event: each utterance's content, and where it
/// stops speech.
#[derive(Debug, PartialEq)]
enum Said {
    Speech(Vec<SegmentContent>),
    Stop,
}

/// Gives Core `event` from mockapp, as [`spoken`] does, and returns what
/// it says and where it stops speech, in order.
fn said(state: &mut SrState, mut event: NormalizedEvent) -> Vec<Said> {
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
        Effect::Speak(utterance) => Some(Said::Speech(
            utterance
                .segments
                .into_iter()
                .map(|segment| segment.content)
                .collect(),
        )),
        Effect::StopSpeech => Some(Said::Stop),
        _ => None,
    })
    .collect()
}

/// How the window's state change and the focus change after it reach the
/// outpost: one handled before the other arrives, or both waiting for one
/// batch, the state change first.
#[derive(Clone, Copy, Debug)]
enum Batching {
    Separate,
    Together,
}

/// The focus on the settings window's Remove button; the window, the
/// focus's ancestor at its own address, is disabled, and then the focus
/// moves to node `to` (its fixture id and index), the two reaching the
/// outpost as `batching` says. Returns what Core does with the state
/// change and the focus change, in the order the outpost reports them.
fn disable_then_move(batching: Batching, (to_id, to_index): (&str, usize)) -> Vec<Said> {
    /// The Remove button, by its index in mockapp's tree.
    const REMOVE: usize = 1;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-before-focus");
    let mut app = common::spawn("modal_owner.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();
    // mockapp's window stands for a popup menu's: topmost, and not the
    // foreground window, which here is none, so the focus's window is one
    // of its ancestors whose state change is spoken.
    common::make_topmost(hwnd);
    outpost.set_foreground(windows::Win32::Foundation::HWND::default());

    app.send("client-identity");
    app.send("set-focus remove");
    let reported = outpost.msaa_focus(hwnd, REMOVE);
    assert_eq!(reported.chain(), [Some("Settings"), Some("Remove")]);
    let _ = said(&mut state, focus_event(&reported));
    let focus_fact = DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: i32::try_from(to_index + 1).expect("a small index"),
        id_child: 0,
    };
    let events = match batching {
        Batching::Separate => {
            app.send("disable-client");
            let changed = next_event(&outpost);
            outpost.settled();
            app.send(&format!("set-focus {to_id}"));
            outpost.deliver_observed_now(focus_fact);
            vec![changed, next_event(&outpost)]
        }
        Batching::Together => {
            // The worker reads the Remove button's next sibling slowly
            // meanwhile, so the change and the focus wait for the same
            // batch.
            app.send("slow 100");
            let request = outpost.ask(Query::Navigate {
                node_id: reported.node.id,
                kind: verbatim_model::QueryKind::NextSibling,
            });
            app.send("disable-client");
            app.send(&format!("set-focus {to_id}"));
            outpost.deliver_observed_now(focus_fact);
            app.send("slow 0");
            match outpost.next() {
                OutpostToSupervisor::Reply { request_id, .. } => assert_eq!(request_id, request),
                other => panic!("the outpost said {other:?}, not the ancestors"),
            }
            vec![next_event(&outpost), next_event(&outpost)]
        }
    };
    outpost.settled();
    app.quit();
    events
        .into_iter()
        .flat_map(|event| said(&mut state, event))
        .collect()
}

/// The window the focus is in disabled, and the focus moving within it:
/// NVDA handles the state change first, judged against the focus it was
/// observed under, and speaks "unavailable", which the focus change
/// within the window does not cut off, and then the new focus. The same
/// whether the two reach the outpost in one batch or two.
fn a_change_before_a_focus_within_the_window_is_heard_first() {
    for batching in [Batching::Separate, Batching::Together] {
        assert_eq!(
            disable_then_move(batching, ("close", 2)),
            [
                Said::Speech(vec![SegmentContent::State(State::Disabled)]),
                Said::Speech(vec![
                    SegmentContent::Label("Close".to_owned()),
                    SegmentContent::Role(verbatim_model::Role::Button),
                ]),
            ],
            "{batching:?}"
        );
    }
}

/// The window the focus is in disabled, and the focus moving into a menu:
/// "unavailable" is spoken and then cut off as the menu is entered, as
/// NVDA's entering a menu cancels speech. The same whether the two reach
/// the outpost in one batch or two.
fn a_change_before_a_focus_into_a_menu_is_cut_off() {
    for batching in [Batching::Separate, Batching::Together] {
        assert_eq!(
            disable_then_move(batching, ("copy", 4)),
            [
                Said::Speech(vec![SegmentContent::State(State::Disabled)]),
                Said::Stop,
                Said::Speech(vec![SegmentContent::Label("Copy".to_owned())]),
            ],
            "{batching:?}"
        );
    }
}

/// A state change on a windowless ancestor of the focus, one reached
/// through `accParent` with no address of its own, is not spoken, as NVDA
/// does not speak it: NVDA meets an event's object again only at an
/// address an earlier object was created for, which such an ancestor never
/// had, and its speech gate asks whether the object is the very ancestor it
/// holds, never whether it is like it (`docs/parity.md`). The focused
/// list item's list becomes unavailable and is not even reported, as the
/// description change on the focus after it is the next thing the outpost
/// says. mockapp hands out one object per node, so no change of identity on
/// mockapp's side makes this so.
fn a_state_change_on_a_windowless_ancestor_is_not_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-acc-parent-state");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("set-states list focusable disabled");
    app.send("set-description item2 Second choice");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Description(
            "Second choice".to_owned()
        )]],
        "the list's change was not reported"
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
    let outpost = OutpostUnderTest::new(&app);
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
    let outpost = OutpostUnderTest::new(&app);
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
    let outpost = OutpostUnderTest::new(&app);
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
    harness::run_isolated(&[
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
            "a_state_change_on_a_windowless_ancestor_is_not_spoken",
            a_state_change_on_a_windowless_ancestor_is_not_spoken,
        ),
        (
            "disabling_the_focus_window_is_not_spoken",
            disabling_the_focus_window_is_not_spoken,
        ),
        (
            "a_change_before_a_focus_within_the_window_is_heard_first",
            a_change_before_a_focus_within_the_window_is_heard_first,
        ),
        (
            "a_change_before_a_focus_into_a_menu_is_cut_off",
            a_change_before_a_focus_into_a_menu_is_cut_off,
        ),
        (
            "disabling_a_window_not_in_the_foreground_is_spoken",
            disabling_a_window_not_in_the_foreground_is_spoken,
        ),
        (
            "collapsing_a_tree_items_parent_says_nothing_until_its_focus",
            collapsing_a_tree_items_parent_says_nothing_until_its_focus,
        ),
    ]);
}
