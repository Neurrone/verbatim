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
//! A foreground event whose window is not yet in front is dropped and
//! never kept, as NVDA drops it: the window is announced from the focus's
//! ancestry instead (`docs/design/focus-pipeline.md`, section 16.4). One
//! whose window is in front is reported at once, whether or not the window
//! has been shown yet.
//!
//! mockapp's focus moves with `set-focus`, which raises no event; the
//! test hands the outpost the focus itself, as `call_counts.rs` does.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex, PoisonError};

use verbatim_core::SrState;
use verbatim_ia2::{WinEvent, WinEventHook, WinEventKind};
use verbatim_model::{
    Backend, CallCounts, Earcon, Effect, FocusGroup, Input, NormalizedEvent, OutpostId, Pid,
    SegmentContent, State, TraceId, WindowFacts,
};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, Query, WinEventStamp};

use common::outpost::{OutpostUnderTest, Reported};
use windows::Win32::Foundation::HWND;

/// The counts fixture's second list item, by its index in mockapp's tree.
const ITEM_TWO: usize = 6;

/// What reporting mockapp's window as the foreground costs at the
/// outpost's first look at the window: its name and states, eight MSAA
/// calls, and the window's probe, one window message.
const FIRST_FOREGROUND_REPORT_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 8,
    window_messages: 1,
    connections: 0,
};

/// What reporting mockapp's window as the foreground a second time costs,
/// measured: its client area read again, eleven MSAA calls.
const REPEATED_FOREGROUND_REPORT_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 11,
    window_messages: 0,
    connections: 0,
};

/// What reporting mockapp's window as the foreground costs once the
/// window's backend is known: its name and states, eight MSAA calls.
const FOREGROUND_REPORT_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 8,
    window_messages: 0,
    connections: 0,
};

/// What reporting a list item costs at the outpost's first look at its
/// window, a foreground change of the window dropped before it: the item
/// and its ancestors, whose locations are not read, 28 MSAA calls, and the
/// window's probe.
const DROPPED_FOREGROUND_FOCUS_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 28,
    window_messages: 1,
    connections: 0,
};

/// What reporting a list item costs once its window's backend is known, a
/// foreground change of the window dropped before it with no call: its
/// list, the container of the item reported before it, is found equal to
/// that item's by NVDA's equality and the walk stops there.
const FOCUS_BESIDE_DROPPED_FOREGROUND_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 20,
    window_messages: 0,
    connections: 0,
};

/// Gives Core `event` from mockapp with no window facts, as
/// [`spoken_in`] does.
fn spoken(state: &mut SrState, event: NormalizedEvent) -> Vec<Vec<SegmentContent>> {
    spoken_in(state, event, None)
}

/// Gives Core `event` from mockapp, in the window `window` describes, its
/// node ids stamped with outpost 1, and returns the content of each
/// utterance spoken, in order. A focus's drop of expired speech is the
/// only other effect allowed: speech is never cut.
fn spoken_in(
    state: &mut SrState,
    mut event: NormalizedEvent,
    window: Option<WindowFacts>,
) -> Vec<Vec<SegmentContent>> {
    event.assign_outpost(OutpostId(1));
    verbatim_core::reduce(
        state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Msaa,
            window,
            event,
        },
    )
    .into_iter()
    .filter_map(|effect| match effect {
        Effect::Speak(utterance, _) => Some(
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
        selected_child: reported.selected_child.clone(),
        text: None,
        earlier: None,
        group: None,
    }
}

/// The outpost's next message, which must be an event, as its event.
fn next_event(outpost: &OutpostUnderTest) -> NormalizedEvent {
    next_event_in(outpost).0
}

/// The outpost's next message, which must be an event, as its event and
/// its window facts.
fn next_event_in(outpost: &OutpostUnderTest) -> (NormalizedEvent, Option<WindowFacts>) {
    match outpost.next() {
        OutpostToSupervisor::Event { event, window, .. } => (event, window),
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
/// spoken, as NVDA, checked live, never speaks it (`docs/parity/focus-announcement.md`, "A
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

/// The same owner disabled after a foreground report of its window, which
/// Core refuses since its focus is already inside: the outpost reads the
/// window's change, the report's focus being unreleased, and Core, whose
/// foreground object the window is, says nothing, as NVDA's event on the
/// window resolves to its foreground object, never to the ancestor.
fn disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken() {
    /// The Remove button, by its index in mockapp's tree.
    const REMOVE: usize = 1;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-modal-owner-refused");
    let mut app = common::spawn("modal_owner.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("client-identity");
    app.send("set-focus remove");
    let reported = outpost.msaa_focus(hwnd, REMOVE);
    assert_eq!(reported.chain(), [Some("Settings"), Some("Remove")]);
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (change, facts) = next_event_in(&outpost);
    assert_eq!(
        spoken_in(&mut state, change, facts),
        [] as [Vec<SegmentContent>; 0],
        "the focus is already in the window"
    );
    app.send("disable-client");
    let (event, facts) = next_event_in(&outpost);
    outpost.settled();
    assert_eq!(
        spoken_in(&mut state, event, facts),
        [] as [Vec<SegmentContent>; 0]
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
            observed_at_us: 0,
            source: Pid(1),
            backend: Backend::Msaa,
            window: None,
            event,
        },
    )
    .into_iter()
    .filter_map(|effect| match effect {
        Effect::Speak(utterance, _) => Some(Said::Speech(
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
    use verbatim_outpost::Heard;
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
            // batch, in the order they reached the outpost: the change,
            // taken in by the outpost's own hook, before the focus is
            // handed over.
            app.send("slow 100");
            let request = outpost.ask(Query::Navigate {
                node_id: reported.node.id,
                kind: verbatim_model::QueryKind::NextSibling,
            });
            app.send("disable-client");
            outpost.heard(&[Heard::Msaa(WinEventKind::StateChange)]);
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
/// holds, never whether it is like it (`docs/parity/focus-announcement.md`,
/// "Focus announcement content and property order"). The focused
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
/// item's is the control, so the control moving its selection to the
/// collapsed item says nothing. The control's focus on it, which the
/// outpost's own hook takes in, says its announcement, collapsed; the
/// item's state change, raised after it, is then the focus's own and says
/// nothing new, the collapsed state having been spoken with the focus.
fn collapsing_a_tree_items_parent_says_nothing_until_its_focus() {
    use windows::Win32::UI::Controls::{TVE_COLLAPSE, TVM_EXPAND};
    common::init_com();
    let title = common::unique_title("mockapp-msaa-ancestor-state");
    let app = common::spawn("tree_view.json", "msaa", &title);
    let tree = common::tree_view::tree_view(common::find_window(&title));
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    let reported = common::tree_view::focus_item(&outpost, tree, "Disks");
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    let hardware = common::tree_view::item(tree, "Hardware");
    common::tree_view::send(tree, TVM_EXPAND, TVE_COLLAPSE.0 as usize, hardware);
    // The control's focus on the parent is the first thing the outpost
    // says, and the parent's state change the last; the focus, in the
    // same top-level window, does not cut speech.
    let (focus, facts) = next_event_in(&outpost);
    let change = next_event(&outpost);
    outpost.settled();
    assert!(
        matches!(change, NormalizedEvent::PropertyChanged { .. }),
        "{change:?}"
    );
    assert_eq!(
        spoken_in(&mut state, focus, facts),
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
    assert_eq!(
        spoken(&mut state, change),
        Vec::<Vec<SegmentContent>>::new()
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
                observed_at_us: 0,
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

/// A selection is read only as far as it can be spoken. While the focus is
/// a button, an item selected in the list beside it is neither the focus
/// nor an ancestor, nor inside a focused list, so it costs its acquisition
/// alone and is not reported; once the focus is on the list, an item
/// selected in it is read in full and spoken, as before.
fn a_selection_is_read_in_full_only_inside_a_focused_list() {
    /// The counts fixture's First button and Options list, by their index
    /// in mockapp's tree.
    const FIRST: usize = 2;
    const LIST: usize = 4;
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-selection-read");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus first");
    let reported = outpost.msaa_focus(hwnd, FIRST);
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    common::reset_hits(hwnd);
    app.send("select item1");
    outpost.heard(&[Heard::Msaa(WinEventKind::Selection)]);
    outpost.settled();
    assert_eq!(
        common::read_hits(hwnd),
        [("WM_GETOBJECT", 1), ("get_accChild", 1)],
        "the selection was acquired and nothing of it read"
    );

    app.send("set-focus list");
    let reported = outpost.msaa_focus(hwnd, LIST);
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    app.send("select item2");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Label("Two".to_owned())]]
    );
    app.quit();
}

/// A change is read for the nodes the outpost has not released: once Core
/// holds another application's focus and releases the outpost's line, a
/// description change on the list item this outpost reported last is not
/// read, and costs its acquisition alone; once the item is reported again,
/// under a new node, its description change is reported.
fn a_change_of_a_focus_core_no_longer_holds_is_not_read() {
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-elsewhere");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    outpost.release_all();
    common::reset_hits(hwnd);
    app.send("set-description item2 Second choice");
    outpost.heard(&[Heard::Msaa(WinEventKind::DescriptionChange)]);
    outpost.settled();
    assert_eq!(
        common::read_hits(hwnd),
        [("WM_GETOBJECT", 1), ("get_accChild", 1)],
        "the change was acquired and nothing of it read"
    );

    let again = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_ne!(again.node.id, reported.node.id, "the released node is gone");
    app.send("set-description item2 Third choice");
    assert_eq!(
        next_event(&outpost),
        NormalizedEvent::PropertyChanged {
            node_id: again.node.id,
            change: verbatim_model::PropertyChange::Description(Some("Third choice".to_owned())),
            child_count: None,
        }
    );
    outpost.settled();
    app.quit();
}

/// A change raised after a focus this outpost reported, and handled before
/// Core has judged that focus, is judged against it as well as against the
/// focus Core holds: Core may yet take it, and speaks the change if it does
/// (decision D14, "Where the rules run").
fn a_change_of_a_focus_core_has_not_judged_yet_is_reported() {
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-unjudged");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item2");
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: i32::try_from(ITEM_TWO + 1).expect("a small index"),
        id_child: 0,
    });
    outpost.settle();
    app.send("set-description item2 Second choice");
    outpost.heard(&[Heard::Msaa(WinEventKind::DescriptionChange)]);
    outpost.settle();
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost did not report the focus first");
    };
    assert_eq!(
        next_event(&outpost),
        NormalizedEvent::PropertyChanged {
            node_id: node.id,
            change: verbatim_model::PropertyChange::Description(Some("Second choice".to_owned())),
            child_count: None,
        }
    );
    outpost.settled();
    app.quit();
}

/// A tooltip window's show event (which the outpost's own hook and the
/// listener take only from `tooltips_class32` windows) is spoken as NVDA's notification behavior
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

/// Has mockapp, running the small fixture with its MSAA provider, raise a
/// focus event on `btn1` (`focus`), and returns the fact the focus
/// listener forwards for it with the thread that raised it and when, as a
/// hook on this thread took it in.
fn raise_focus(app: &mut common::MockApp) -> (DeliveredFact, WinEventStamp) {
    raise_focus_on(app, "btn1")
}

/// [`raise_focus`], on the node `id`.
fn raise_focus_on(app: &mut common::MockApp, id: &str) -> (DeliveredFact, WinEventStamp) {
    let seen = Arc::new(Mutex::new(None::<WinEvent>));
    let sink = Arc::clone(&seen);
    let hook = WinEventHook::install(
        app.pid(),
        &[WinEventKind::Focus],
        Box::new(move |event: WinEvent| {
            *sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(event);
        }),
    )
    .expect("WinEventHook::install");
    app.send(&format!("focus {id}"));
    let event = common::pump_until(|| *seen.lock().unwrap_or_else(PoisonError::into_inner));
    drop(hook);
    let fact =
        DeliveredFact::from_win_event(event.kind, (event.hwnd, event.id_object, event.id_child))
            .expect("a focus in mockapp's window");
    let stamp = WinEventStamp {
        thread: event.thread,
        time: event.time,
    };
    (fact, stamp)
}

/// An MSAA focus raised once an outpost's own hook is in reaches it by
/// that hook and is reported; the focus listener's copy of the same event,
/// forwarded before the listener heard the hook was in, is the twin of the
/// hook's copy and is dropped (`docs/design/focus-pipeline.md`, section
/// 12.4, "The MSAA hand-over, exactly").
fn a_focus_the_outposts_own_hook_took_in_is_reported_once() {
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-hook-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let outpost = OutpostUnderTest::new(&app);

    let (fact, stamp) = raise_focus(&mut app);
    outpost.heard(&[Heard::Msaa(WinEventKind::Focus)]);
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost reported something other than the focus");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    outpost.deliver_win_event(fact, stamp);
    outpost.settled();
    app.quit();
}

/// An MSAA focus raised before an outpost's own hook is in reaches it only
/// from the focus listener, with the thread that raised it and when: it has
/// no twin, and is reported.
fn a_focus_raised_before_the_outposts_hook_is_reported_from_the_listener() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-listener-focus");
    let mut app = common::spawn("small.json", "msaa", &title);
    let (fact, stamp) = raise_focus(&mut app);
    let outpost = OutpostUnderTest::new(&app);

    outpost.deliver_win_event(fact, stamp);
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost reported something other than the focus");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    outpost.settled();
    app.quit();
}

/// A spare given to mockapp (`docs/design/focus-pipeline.md`, section 19)
/// reports the focus raised before it was given from the listener's copy,
/// which the supervisor held for it; during the hand-over's overlap it
/// reports a focus its own hook took in once, the listener's twin dropped;
/// and once the listener's forwarding has ended, a focus from its own hook,
/// once.
fn a_spare_given_to_an_application_reports_each_focus_once() {
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-spare");
    let mut app = common::spawn("small.json", "msaa", &title);
    let outpost = OutpostUnderTest::spare(&app);

    let (held, held_stamp) = raise_focus(&mut app);
    outpost.assign(&app);
    outpost.deliver_win_event(held, held_stamp);
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost reported something other than the held focus");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    outpost.settled();

    let (twin, twin_stamp) = raise_focus_on(&mut app, "slider1");
    outpost.heard(&[Heard::Msaa(WinEventKind::Focus)]);
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost reported something other than its own hook's focus");
    };
    assert_eq!(node.name.as_deref(), Some("Level"));
    outpost.deliver_win_event(twin, twin_stamp);
    outpost.settled();

    outpost.forwarding_ended(twin_stamp.time);
    raise_focus(&mut app);
    outpost.heard(&[Heard::Msaa(WinEventKind::Focus)]);
    let NormalizedEvent::FocusChanged { node, .. } = next_event(&outpost) else {
        panic!("the outpost reported something other than its own hook's focus");
    };
    assert_eq!(node.name.as_deref(), Some("Original Name"));
    outpost.settled();
    app.quit();
}

/// A foreground event whose window is not the foreground window when the
/// outpost handles it is dropped, not kept for a focus inside the window:
/// once the window is in front, its focus is reported alone, with no
/// foreground report before it, as NVDA, having dropped the event, meets
/// the window only in the focus's ancestry.
fn a_foreground_event_before_its_window_is_in_front_is_dropped() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-early-foreground");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    outpost.set_foreground(common::another_applications_window());
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.settled();

    outpost.set_foreground(hwnd);
    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    app.quit();
}

/// A foreground event whose window is in front is reported at once, as a
/// focus on the window, though the window has not been shown: NVDA tests
/// only that the window is the foreground window. mockapp's window is never
/// shown in these tests.
fn a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-hidden-foreground");
    let app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // SAFETY: IsWindowVisible tolerates any handle.
    let shown = unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) };
    assert!(!shown.as_bool(), "mockapp's window is hidden");
    let outpost = OutpostUnderTest::new(&app);

    outpost.set_foreground(hwnd);
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let NormalizedEvent::FocusChanged {
        node, foreground, ..
    } = next_event(&outpost)
    else {
        panic!("the foreground report is a focus");
    };
    assert_eq!(
        (foreground, node.name.as_deref()),
        (true, Some("Mockapp Counts Fixture"))
    );
    outpost.settled();
    app.quit();
}

/// The counts fixture's third list item, by its index in mockapp's tree.
const ITEM_THREE: usize = 7;

/// The MSAA focus fact for mockapp's node `index` in window `hwnd`.
fn msaa_focus_fact(hwnd: HWND, index: usize) -> DeliveredFact {
    DeliveredFact::MsaaFocus {
        hwnd: hwnd.0 as isize,
        id_object: i32::try_from(index + 1).expect("a small index"),
        id_child: 0,
    }
}

/// The outpost's next message, which must be a foreground report: the
/// window's name, and the calls the outpost made for it.
fn foreground_report(outpost: &OutpostUnderTest) -> (Option<String>, CallCounts) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    foreground: true,
                    ..
                },
            timing,
            ..
        } => (node.name, timing.calls),
        other => panic!("the outpost said {other:?}, not a foreground report"),
    }
}

/// Hands the outpost, held, a focus on mockapp's node `index` and then a
/// foreground change of its window, so that the two are planned as one
/// batch, one group tried newest first, and lets the worker go on. The
/// worker is held on a foreground change of `held_on`, which is not the
/// foreground window and says nothing.
fn focus_then_foreground(outpost: &OutpostUnderTest, hwnd: HWND, index: usize, held_on: HWND) {
    outpost.hold_worker_on(held_on.0 as isize);
    outpost.deliver(msaa_focus_fact(hwnd, index));
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.release_worker();
}

/// A focus and a foreground change of its window in one batch, the change
/// newest, with no focus reported before: the change is tried first, is
/// reported, as the window, and ends the group's tries, so the focus is
/// not tried, as NVDA's `pumpAll` stops at the first event processed.
fn a_foreground_change_newest_beside_a_focus_is_reported_and_ends_its_group() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-foreground-group");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item2");
    focus_then_foreground(
        &outpost,
        hwnd,
        ITEM_TWO,
        common::another_applications_window(),
    );
    let (name, calls) = foreground_report(&outpost);
    assert_eq!(name.as_deref(), Some("Mockapp Counts Fixture"));
    assert_eq!(calls, FIRST_FOREGROUND_REPORT_CALLS);
    outpost.settled();
    app.quit();
}

/// A focus and a foreground change of its window in one batch, the change
/// newest but its window not in front: the change is dropped without
/// counting as reported, so the focus beside it is tried next and
/// reported. mockapp's window is topmost, so the focus passes NVDA's
/// acceptance test though another window is the foreground.
fn a_dropped_foreground_change_newest_beside_a_focus_lets_the_focus_be_tried() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-dropped-foreground");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    common::make_topmost(hwnd);
    let outpost = OutpostUnderTest::new(&app);

    outpost.set_foreground(common::another_applications_window());
    app.send("set-focus item2");
    focus_then_foreground(&outpost, hwnd, ITEM_TWO, hwnd);
    let reported = outpost.reported_focus();
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    assert_eq!(reported.calls, DROPPED_FOREGROUND_FOCUS_CALLS);
    app.quit();
}

/// The sequence `text_box_editing` meets in a Windows Forms form: the
/// form's foreground report, which Core takes, then a focus in a control's
/// child window; Core's focus leaves for another application and comes
/// back with a foreground change and the focus in one batch, the focus
/// newest, so the focus is tried first and ends the group's tries and the
/// foreground change is never read. The window's name is said before the
/// focus each time the focus enters the window, as NVDA's `doPreGainFocus`
/// speaks the foreground object on entering a new top-level window. The
/// form's client area, reached through `accParent` from the child window's
/// window object with no identity string, above which the form's window
/// object offers none either (`form-client`), is read at its own address, the
/// same node as the foreground report's, marked as a top-level window's
/// client (`docs/parity/focus-context.md`, "Top-level windows in the
/// ancestry").
fn a_window_entered_by_a_focus_beside_its_unread_foreground_change_is_said() {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-form-entered");
    let mut app = common::spawn("form.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    app.send("child-window");
    app.send("child-client-identity");
    app.send("form-client");
    // SAFETY: a local search of the form's children.
    let child = unsafe { FindWindowExW(Some(hwnd), None, None, None) }
        .expect("mockapp made the child window");
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();
    let title_said = vec![SegmentContent::Label("Mockapp Form Fixture".to_owned())];
    let ok_said = vec![
        SegmentContent::Label("OK".to_owned()),
        SegmentContent::Role(verbatim_model::Role::Button),
    ];

    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (change, facts) = next_event_in(&outpost);
    assert_eq!(
        spoken_in(&mut state, change, in_front(facts)),
        std::slice::from_ref(&title_said)
    );
    outpost.settled();
    let reported = outpost.msaa_focus(child, FORM_BUTTON);
    assert_eq!(
        spoken_in(
            &mut state,
            focus_event(&reported),
            in_front(reported.facts.clone())
        ),
        std::slice::from_ref(&ok_said),
        "the focus moves inside the window"
    );

    outpost.release_all();
    state = SrState::new();
    outpost.hold_worker_on(common::another_applications_window().0 as isize);
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    outpost.deliver(msaa_focus_fact(child, FORM_BUTTON));
    outpost.release_worker();
    let reported = outpost.reported_focus();
    outpost.settled();
    assert_eq!(
        spoken_in(
            &mut state,
            focus_event(&reported),
            in_front(reported.facts.clone())
        ),
        [title_said, ok_said],
        "the focus enters the window again"
    );
    app.quit();
}

/// The form fixture's button, by its index in mockapp's tree.
const FORM_BUTTON: usize = 1;

/// The group a focus report carries, with the report.
fn group_of(event: &NormalizedEvent) -> Option<FocusGroup> {
    match event {
        NormalizedEvent::FocusChanged { group, .. } => *group,
        other => panic!("{other:?} is not a focus report"),
    }
}

/// With the focus already in mockapp's window, a focus on another item and
/// a foreground change of that window in one batch, the change newest:
/// whether the change is processed is Core's verdict, so both are
/// reported, ranked in their group, and Core refuses the change, its focus
/// being in the window, and takes the older entry, "Three", as NVDA's
/// `processForegroundWinEvent` returns false and its `pumpAll` tries the
/// older focus event.
fn a_foreground_change_of_the_focus_s_window_and_the_focus_beside_it_are_reported_ranked() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-foreground-focus-window");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    app.send("set-focus item3");
    focus_then_foreground(
        &outpost,
        hwnd,
        ITEM_THREE,
        common::another_applications_window(),
    );
    let (change, change_facts, change_calls) = next_event_with_calls(&outpost);
    let (beside, beside_facts, beside_calls) = next_event_with_calls(&outpost);
    outpost.settled();
    let (change_group, beside_group) = (group_of(&change), group_of(&beside));
    assert_eq!(
        (
            change_group.map(|group| group.rank),
            beside_group.map(|group| group.rank),
            change_group.map(|group| group.number) == beside_group.map(|group| group.number),
        ),
        (Some(0), Some(1), true),
        "one group, the change newest"
    );
    assert_eq!(
        (change_calls, beside_calls),
        (
            FOREGROUND_REPORT_CALLS,
            FOCUS_BESIDE_DROPPED_FOREGROUND_CALLS
        )
    );
    assert_eq!(
        spoken_in(&mut state, change, change_facts),
        [] as [Vec<SegmentContent>; 0],
        "the focus is already in the window"
    );
    assert_eq!(
        spoken_in(&mut state, beside, in_front(beside_facts)),
        [vec![
            SegmentContent::Label("Three".to_owned()),
            SegmentContent::NegatedState(State::Selected),
        ]]
    );
    app.quit();
}

/// A second foreground change of a window the outpost has just reported as
/// the foreground is reported too, and Core, whose focus is that window,
/// says nothing of it.
fn a_second_foreground_change_of_a_reported_window_is_reported_and_silent() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-second-foreground");
    let app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    let change = DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    };
    outpost.deliver(change.clone());
    let (first, first_facts, first_calls) = next_event_with_calls(&outpost);
    assert_eq!(first_calls, FIRST_FOREGROUND_REPORT_CALLS);
    assert_eq!(
        spoken_in(&mut state, first, first_facts),
        [vec![
            SegmentContent::Label("Mockapp Counts Fixture".to_owned()),
            SegmentContent::Role(verbatim_model::Role::Window),
        ]]
    );
    outpost.settled();
    outpost.deliver(change);
    let (second, second_facts, second_calls) = next_event_with_calls(&outpost);
    assert_eq!(second_calls, REPEATED_FOREGROUND_REPORT_CALLS);
    assert_eq!(
        spoken_in(&mut state, second, second_facts),
        [] as [Vec<SegmentContent>; 0]
    );
    outpost.settled();
    app.quit();
}

/// With Core's focus a list item, its window's foreground change is
/// reported and Core refuses it, keeping its focus: the item, which the
/// outpost has not released, still has its state change read, and Core
/// says "checked".
fn a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-change-after-refused");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    let _ = spoken_in(&mut state, focus_event(&reported), reported.facts.clone());
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (change, facts) = next_event_in(&outpost);
    assert_eq!(
        spoken_in(&mut state, change, facts),
        [] as [Vec<SegmentContent>; 0],
        "the focus is already in the window"
    );
    app.send("set-states item2 focusable focused checked");
    let (event, facts) = next_event_in(&outpost);
    outpost.settled();
    assert_eq!(
        spoken_in(&mut state, event, facts),
        [vec![SegmentContent::State(State::Checked)]]
    );
    app.quit();
}

/// With Core's focus mockapp's window, taken from its foreground report,
/// the window's client area renamed is that report's focus, which the
/// outpost has not released: its name change is read, and Core says the
/// new name.
fn a_name_change_of_the_window_core_took_is_spoken() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-window-renamed");
    let mut app = common::spawn("modal_owner.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    let mut state = SrState::new();

    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (change, facts) = next_event_in(&outpost);
    assert_eq!(
        spoken_in(&mut state, change, facts),
        [vec![
            SegmentContent::Label("Settings".to_owned()),
            SegmentContent::Role(verbatim_model::Role::Dialog),
        ]]
    );
    app.send("client-name Options");
    let (event, facts) = next_event_in(&outpost);
    outpost.settled();
    assert_eq!(
        spoken_in(&mut state, event, facts),
        [vec![SegmentContent::Label("Options".to_owned())]]
    );
    app.quit();
}

/// `facts` as they read with mockapp's window in front, which the
/// outpost's own foreground reader says it is, though the system's
/// foreground window is another: Core takes a focus in a window unrelated
/// to the foreground only before any foreground report.
fn in_front(facts: Option<WindowFacts>) -> Option<WindowFacts> {
    facts.map(|facts| WindowFacts {
        in_foreground: true,
        ..facts
    })
}

/// The outpost's next message, which must be an event, as its event, its
/// window facts and the calls it made.
fn next_event_with_calls(
    outpost: &OutpostUnderTest,
) -> (NormalizedEvent, Option<WindowFacts>, CallCounts) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event,
            window,
            timing,
            ..
        } => (event, window, timing.calls),
        other => panic!("the outpost said {other:?}, not an event"),
    }
}

/// A focus Core took from another application replaces the focus the
/// outpost judges against, as NVDA's `lastQueuedFocusObject` is replaced by
/// any application's focus: a foreground change of the window the
/// outpost's focus was in is then reported.
fn a_foreground_change_after_another_application_s_focus_is_reported() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-foreground-after-elsewhere");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    // Observed now, after the outpost's focus.
    outpost.release_all();
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (name, calls) = foreground_report(&outpost);
    assert_eq!(name.as_deref(), Some("Mockapp Counts Fixture"));
    assert_eq!(calls, FOREGROUND_REPORT_CALLS);
    outpost.settled();
    app.quit();
}

/// The stamp of the outpost's next message, which must be a focus change,
/// and when its event was first observed, both in microseconds.
fn focus_stamp(outpost: &OutpostUnderTest) -> (u64, u64) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            observed_at_us,
            timing,
            event: NormalizedEvent::FocusChanged { .. },
            ..
        } => (observed_at_us, timing.observed_at_us),
        other => panic!("the outpost said {other:?}, not a focus change"),
    }
}

/// A focus the outpost's own hook took in is stamped with its hook's
/// observation to the microsecond, so Core orders it against another
/// application's focus in the same millisecond
/// (`docs/parity/focus-events.md`, "Stale focus events").
fn a_hooked_focus_is_stamped_with_its_observation_to_the_microsecond() {
    use verbatim_outpost::Heard;
    common::init_com();
    let title = common::unique_title("mockapp-msaa-focus-stamp");
    let mut app = common::spawn("small.json", "msaa", &title);
    let outpost = OutpostUnderTest::new(&app);

    let (fact, stamp) = raise_focus(&mut app);
    outpost.heard(&[Heard::Msaa(WinEventKind::Focus)]);
    let (observed, first_observed) = focus_stamp(&outpost);
    assert_ne!(first_observed, 0, "the hook observed the focus");
    assert_eq!(observed, first_observed);
    outpost.deliver_win_event(fact, stamp);
    outpost.settled();
    app.quit();
}

/// A foreground change is stamped when its window was confirmed as the
/// foreground, to the microsecond: the fact it came from carried no
/// observation.
fn a_foreground_report_is_stamped_at_its_confirmation_to_the_microsecond() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-foreground-stamp");
    let app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    outpost.set_foreground(hwnd);
    outpost.deliver_stamped(
        DeliveredFact::Foreground {
            hwnd: hwnd.0 as isize,
        },
        0,
    );
    let (observed, first_observed) = focus_stamp(&outpost);
    assert_eq!(first_observed, 0, "the fact carried no observation");
    assert_ne!(observed, 0, "the confirmation is stamped");
    outpost.settled();
    app.quit();
}

#[expect(clippy::too_many_lines, reason = "the binary's tests, listed in full")]
fn main() {
    harness::run_isolated(&[
        (
            "a_change_of_a_focus_core_no_longer_holds_is_not_read",
            a_change_of_a_focus_core_no_longer_holds_is_not_read,
        ),
        (
            "a_change_of_a_focus_core_has_not_judged_yet_is_reported",
            a_change_of_a_focus_core_has_not_judged_yet_is_reported,
        ),
        (
            "a_hooked_focus_is_stamped_with_its_observation_to_the_microsecond",
            a_hooked_focus_is_stamped_with_its_observation_to_the_microsecond,
        ),
        (
            "a_foreground_report_is_stamped_at_its_confirmation_to_the_microsecond",
            a_foreground_report_is_stamped_at_its_confirmation_to_the_microsecond,
        ),
        (
            "a_foreground_change_newest_beside_a_focus_is_reported_and_ends_its_group",
            a_foreground_change_newest_beside_a_focus_is_reported_and_ends_its_group,
        ),
        (
            "a_dropped_foreground_change_newest_beside_a_focus_lets_the_focus_be_tried",
            a_dropped_foreground_change_newest_beside_a_focus_lets_the_focus_be_tried,
        ),
        (
            "a_window_entered_by_a_focus_beside_its_unread_foreground_change_is_said",
            a_window_entered_by_a_focus_beside_its_unread_foreground_change_is_said,
        ),
        (
            "a_foreground_change_of_the_focus_s_window_and_the_focus_beside_it_are_reported_ranked",
            a_foreground_change_of_the_focus_s_window_and_the_focus_beside_it_are_reported_ranked,
        ),
        (
            "a_second_foreground_change_of_a_reported_window_is_reported_and_silent",
            a_second_foreground_change_of_a_reported_window_is_reported_and_silent,
        ),
        (
            "a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken",
            a_change_of_the_focus_after_its_window_s_refused_foreground_report_is_spoken,
        ),
        (
            "a_name_change_of_the_window_core_took_is_spoken",
            a_name_change_of_the_window_core_took_is_spoken,
        ),
        (
            "a_foreground_change_after_another_application_s_focus_is_reported",
            a_foreground_change_after_another_application_s_focus_is_reported,
        ),
        (
            "a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported",
            a_foreground_event_for_a_window_in_front_not_yet_shown_is_reported,
        ),
        (
            "a_spare_given_to_an_application_reports_each_focus_once",
            a_spare_given_to_an_application_reports_each_focus_once,
        ),
        (
            "a_focus_the_outposts_own_hook_took_in_is_reported_once",
            a_focus_the_outposts_own_hook_took_in_is_reported_once,
        ),
        (
            "a_focus_raised_before_the_outposts_hook_is_reported_from_the_listener",
            a_focus_raised_before_the_outposts_hook_is_reported_from_the_listener,
        ),
        (
            "a_selection_is_read_in_full_only_inside_a_focused_list",
            a_selection_is_read_in_full_only_inside_a_focused_list,
        ),
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
            "disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken",
            disabling_the_focus_window_after_its_refused_foreground_report_is_not_spoken,
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
        (
            "a_foreground_event_before_its_window_is_in_front_is_dropped",
            a_foreground_event_before_its_window_is_in_front_is_dropped,
        ),
    ]);
}
