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
    Backend, CallCounts, Earcon, Effect, Input, NormalizedEvent, OutpostId, Pid, SegmentContent,
    State, TraceId,
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
/// foreground change of the window dropped before it with no call.
const FOCUS_BESIDE_DROPPED_FOREGROUND_CALLS: CallCounts = CallCounts {
    uia: 0,
    msaa: 28,
    window_messages: 0,
    connections: 0,
};

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
        text: None,
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
    let _ = spoken(&mut state, focus_event(&reported));
    let hardware = common::tree_view::item(tree, "Hardware");
    common::tree_view::send(tree, TVM_EXPAND, TVE_COLLAPSE.0 as usize, hardware);
    // The control's focus on the parent is the first thing the outpost
    // says, and the parent's state change the last.
    let focus = next_event(&outpost);
    let change = next_event(&outpost);
    outpost.settled();
    assert!(
        matches!(change, NormalizedEvent::PropertyChanged { .. }),
        "{change:?}"
    );
    assert_eq!(
        spoken(&mut state, focus),
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
    let _ = spoken(&mut state, focus_event(&reported));
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
    let _ = spoken(&mut state, focus_event(&reported));
    app.send("select item2");
    let event = next_event(&outpost);
    outpost.settled();
    assert_eq!(
        spoken(&mut state, event),
        [vec![SegmentContent::Label("Two".to_owned())]]
    );
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

/// NVDA's first foreground test: with the focus already in mockapp's
/// window, a focus on another item and a foreground change of that window
/// in one batch, the change newest, have the change dropped, since the
/// last focus queued is in its window, and the focus beside it tried and
/// reported, as NVDA's `processForegroundWinEvent` returns false and its
/// `pumpAll` tries the older focus event.
fn a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-foreground-focus-window");
    let mut app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    app.send("set-focus item2");
    let reported = outpost.msaa_focus(hwnd, ITEM_TWO);
    assert_eq!(reported.chain(), [Some("Options"), Some("Two")]);
    app.send("set-focus item3");
    focus_then_foreground(
        &outpost,
        hwnd,
        ITEM_THREE,
        common::another_applications_window(),
    );
    let reported = outpost.reported_focus();
    assert_eq!(reported.chain(), [Some("Options"), Some("Three")]);
    assert_eq!(reported.calls, FOCUS_BESIDE_DROPPED_FOREGROUND_CALLS);
    app.quit();
}

/// NVDA's first foreground test, by a foreground report: a second
/// foreground change of a window the outpost has just reported as the
/// foreground is dropped, since that window is now the last focus queued,
/// and nothing is said.
fn a_second_foreground_change_of_a_reported_window_is_dropped() {
    common::init_com();
    let title = common::unique_title("mockapp-msaa-second-foreground");
    let app = common::spawn("counts.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);

    let change = DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    };
    outpost.deliver(change.clone());
    let (name, calls) = foreground_report(&outpost);
    assert_eq!(name.as_deref(), Some("Mockapp Counts Fixture"));
    assert_eq!(calls, FIRST_FOREGROUND_REPORT_CALLS);
    outpost.settled();
    outpost.deliver(change);
    outpost.settled();
    app.quit();
}

/// A focus Core queued from another application replaces the outpost's
/// last focus as the last focus queued, as NVDA's `lastQueuedFocusObject`
/// is replaced by any application's focus: a foreground change of the
/// window that focus was in is then reported.
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
    outpost.focus_queued_elsewhere(0);
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: hwnd.0 as isize,
    });
    let (name, calls) = foreground_report(&outpost);
    assert_eq!(name.as_deref(), Some("Mockapp Counts Fixture"));
    assert_eq!(calls, FOREGROUND_REPORT_CALLS);
    outpost.settled();
    app.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_foreground_change_newest_beside_a_focus_is_reported_and_ends_its_group",
            a_foreground_change_newest_beside_a_focus_is_reported_and_ends_its_group,
        ),
        (
            "a_dropped_foreground_change_newest_beside_a_focus_lets_the_focus_be_tried",
            a_dropped_foreground_change_newest_beside_a_focus_lets_the_focus_be_tried,
        ),
        (
            "a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it",
            a_foreground_change_of_the_focus_s_window_is_dropped_for_the_focus_beside_it,
        ),
        (
            "a_second_foreground_change_of_a_reported_window_is_dropped",
            a_second_foreground_change_of_a_reported_window_is_dropped,
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
