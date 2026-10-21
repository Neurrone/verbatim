//! A caret key's watch for evidence in a real outpost, against mockapp's
//! UIA text provider: the worker never waits for a caret key, so a focus
//! change made while a key's watch is open is handled at once, a caret move
//! the application reports later still answers the watch, a key that
//! moves nothing is answered by nothing at all, and a text pattern missing
//! as the focus arrives is read once the application raises a caret event,
//! and the system caret's move is the caret event of a UIA focus that
//! raises none of its own, as its show is a classic edit control's; and a
//! watch's end at its bound reads no focus for the wait for a UIA focus
//! after a `WinEvent` focus while a UIA focus's marker waits for its read;
//! and a selection no key made is reported with its change, one a key or
//! Verbatim made is not.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{
    CaretReply, CaretReport, CaretWatch, NodeId, NormalizedEvent, PreviousSelection,
    SelectionChange, TextOp, TextPoint, TextPosition, TextReply, TextUnit,
};
use verbatim_outpost::Heard;
use verbatim_outpost::protocol::{
    DeliveredFact, DropEvidence, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
    WinEventStamp,
};
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;

use common::outpost::OutpostUnderTest;

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// Moves mockapp's focus to `id` and returns the fact the focus listener
/// sends for it once it has read it.
fn focus_fact(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
    id: &str,
) -> verbatim_outpost::protocol::DeliveredFact {
    app.send(&format!("set-focus {id}"));
    let element = focused(hwnd);
    outpost.listener_fact(&element)
}

/// Focuses mockapp's "Notes" document and returns its node and the caret
/// the outpost reported after the focus, which must be all it said.
fn focus_notes(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
) -> (NodeId, CaretReport) {
    outpost.deliver(focus_fact(app, outpost, hwnd, "doc"));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node,
        other => panic!("the outpost said {other:?}, not the document's focus"),
    };
    assert_eq!(node.name.as_deref(), Some("Notes"));
    let caret = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == node.id => caret,
        other => panic!("the outpost said {other:?}, not the document's caret"),
    };
    outpost.settled();
    (node.id, caret)
}

/// Right Arrow's watch, with the caret Core knew before the key.
fn right_arrow(before: &CaretReport) -> TextOp {
    TextOp::AwaitCaret(CaretWatch {
        landing: false,
        since: Some(TextPosition {
            anchor: before.line.start,
            offset: before.line.offset,
        }),
        pressed_at_ms: 0,
        unit: TextUnit::Character,
        compare: None,
        previous_selection: None,
    })
}

/// The answer to caret key `key`, which must be the outpost's next message.
fn answer_to(outpost: &OutpostUnderTest, key: u64) -> TextReply {
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(reply)),
            ..
        } if request_id == key => reply,
        other => panic!("the outpost said {other:?}, not the answer to the key"),
    }
}

/// The caret a key's answer reports, with what the key did.
fn caret_reply(reply: TextReply) -> CaretReply {
    match reply {
        TextReply::Caret(reply) => *reply,
        other => panic!("a caret reply, not {other:?}"),
    }
}

/// Waits for the outpost to hear the caret event mockapp raised last, by
/// both routes it takes: UIA's own event, and the `WinEvent` UIA raises for
/// MSAA clients alongside it. Until then the event may still be on its way,
/// and a step that moved on would have it answer, or be reported after,
/// the next step's key (`docs/crates/mockapp.md`).
fn heard_caret_event(outpost: &OutpostUnderTest) {
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
}

/// mockapp's "caret watch" fixture in its UIA backend, with an outpost
/// watching it.
fn start(name: &str) -> (common::MockApp, HWND, OutpostUnderTest) {
    common::init_com();
    let title = common::unique_title(name);
    let app = common::spawn("caret_watch.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::new(&app);
    (app, hwnd, outpost)
}

/// A focus change that arrives while a caret key's watch is open is
/// reported first: the worker does not wait for the key's evidence before
/// handling it. The watch then ends with the focus, answered with nothing.
fn a_focus_change_during_a_caret_watch_is_handled_at_once() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-focus");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let to_ok = focus_fact(&mut app, &outpost, hwnd, "ok");
    // Right Arrow, which the application ignores, then the focus moves.
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    outpost.deliver(to_ok);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => assert_eq!(node.name.as_deref(), Some("OK")),
        other => panic!("the outpost said {other:?} before the focus on OK"),
    }
    assert_eq!(answer_to(&outpost, key), TextReply::WatchEnded);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key the application handles after the watch was opened, reporting its
/// caret with an event as it does: the event answers the watch with the
/// caret where the key took it.
fn a_caret_move_raised_later_answers_the_watch() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-later");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    // The watch is open: nothing is said, and the worker has finished.
    outpost.settled();
    app.send("caret doc 1");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert!(reply.moved);
    assert_eq!(reply.caret.line.text, "alpha beta\n");
    assert_eq!(reply.caret.line.offset, 1);
    assert_eq!(reply.unit.expect("the character").text, "l");
    assert_eq!(reply.selection_changes, []);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key that moves nothing is answered with nothing: its watch stays open
/// without the worker waiting, a caret event that shows no change leaves it
/// open, and the next key's watch replaces it, ending it with nothing to
/// say.
fn a_key_that_moves_nothing_is_answered_with_nothing() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-nothing");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let first = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    outpost.settled();
    // The application's late report of a caret that has not moved, handled
    // before the next key is asked: the outpost has heard it and settled.
    app.send("caret-event doc");
    heard_caret_event(&outpost);
    outpost.settled();
    let second = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    assert_eq!(answer_to(&outpost, first), TextReply::WatchEnded);
    outpost.settled();
    // The second key's watch is open in its turn.
    app.send("caret doc 1");
    app.send("caret-event doc");
    assert!(caret_reply(answer_to(&outpost, second)).moved);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A caret key's watch that reaches its bound, ten seconds by the system's
/// clock, while the outpost waits for a UIA focus after a `WinEvent` focus
/// in mockapp's window (`docs/design/listener-without-connections.md`,
/// decision 11) whose bound has passed by the outpost's, and a UIA focus's
/// marker waits for its read: the worker's wake for the watch reads no
/// focus, since the marker's resolution decides first, and the watch ends
/// with nothing to say. The marker's drop by a verdict of NVDA's checks
/// then answers the wait, and nothing is read.
fn a_watchs_end_reads_no_focus_while_a_marker_waits() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-gap");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: right_arrow(&before),
    });
    outpost.settled();
    // The focus a read would find, which no UIA focus has reported.
    app.send("set-focus ok");
    outpost.read_focus_as(focused(hwnd).live().element());
    // The copy, observed now, after the document's focus, which it follows.
    outpost.deliver_win_event_observed(
        DeliveredFact::MsaaFocus {
            hwnd: hwnd.0 as isize,
            id_object: -4,
            id_child: 0,
        },
        WinEventStamp { thread: 1, time: 1 },
        common::outpost::now_ms(),
    );
    outpost.settled();
    // The marker arrives, and the bound passes, while the worker is held,
    // so no wake of its own judges the gap before the marker is in.
    outpost.hold_worker();
    outpost.deliver(DeliveredFact::UiaFocusMarker { marker: 8 });
    outpost.pass_time(std::time::Duration::from_secs(1));
    outpost.release_worker();
    assert_eq!(answer_to(&outpost, key), TextReply::WatchEnded);
    outpost.deliver(DeliveredFact::UiaFocusDropped {
        marker: 8,
        evidence: DropEvidence::Focus,
    });
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Windows 11 Notepad, starting up, once answered that its document had
/// no text pattern (`docs/design/phase6-design.md`, "Live caret event checks"), and
/// every caret key in it was silent from then on. A provider that fails the
/// request reaches the client the same way, as no pattern, so the focus is
/// reported as having no text, and Core speaks its value, as NVDA does for
/// an object without text. That answer is not kept past the evidence that
/// it is out of date: a caret key's watch on the document stays open, and
/// once the provider answers, the application's caret event, which only an
/// element with text raises, has the text read again and answers the key.
fn a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-not-ready");
    app.send("refuse-text on");
    let fact = focus_fact(&mut app, &outpost, hwnd, "doc");
    outpost.deliver(fact);
    let notes = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node.id,
        other => panic!("the outpost said {other:?}, not the document's focus"),
    };
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::NoText { node_id },
            ..
        } if node_id == notes => {}
        other => panic!("the outpost said {other:?}, not that the document has no text"),
    }
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: None,
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            previous_selection: None,
        }),
    });
    // The watch is open: nothing is said, and the worker has finished.
    outpost.settled();

    app.send("refuse-text off");
    app.send("caret doc 1");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert!(reply.moved);
    assert_eq!(
        (reply.caret.line.text.as_str(), reply.caret.line.offset),
        ("alpha beta\n", 1)
    );
    assert_eq!(reply.unit.expect("the character").text, "l");
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A UIA text control whose application raises no UIA caret event, as
/// some UIA documents do not, while Windows reports its system caret
/// moving: the caret's `WinEvent` from the focus's window is the focus's
/// caret event, and its caret is reported where it moved.
fn the_system_carets_move_is_the_uia_focuss_caret_event() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-system-caret");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 6");
    app.send("system-caret");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == notes => {
            assert_eq!(
                (caret.line.text.as_str(), caret.line.offset),
                ("alpha beta\n", 6)
            );
        }
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
    outpost.heard(&[Heard::Msaa(WinEventKind::Caret)]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Once the focus has raised a UIA caret event of its own, the system
/// caret's events are not used for it: they would only read the caret
/// again for a move its UIA caret event reports.
fn a_focus_with_uia_caret_events_has_no_system_caret_events() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-both-carets");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 1");
    app.send("caret-event doc");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == notes => assert_eq!(caret.line.offset, 1),
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret doc 6");
    app.send("system-caret");
    outpost.heard(&[Heard::Msaa(WinEventKind::Caret)]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A new focus has the system caret's events used again until it raises
/// a UIA caret event of its own: the focus's mark is the focus's alone.
fn a_new_focus_uses_the_system_caret_again() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-new-focus");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 1");
    app.send("caret-event doc");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, .. },
            ..
        } if node_id == notes => {}
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
    heard_caret_event(&outpost);
    outpost.settled();
    // The focus leaves the document and comes back.
    outpost.deliver(focus_fact(&mut app, &outpost, hwnd, "ok"));
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => assert_eq!(node.name.as_deref(), Some("OK")),
        other => panic!("the outpost said {other:?}, not the focus on OK"),
    }
    outpost.settled();
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 6");
    app.send("system-caret");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == notes => assert_eq!(caret.line.offset, 6),
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
    outpost.heard(&[Heard::Msaa(WinEventKind::Caret)]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A system caret's show from the focused classic edit control, read
/// through MSAA, is its caret event, and its caret is reported where it
/// moved. The show is raised by mockapp: on a desktop of its own the
/// control cannot take the keyboard focus, so it raises nothing for the
/// move, and what a focused one raises is not checked here.
fn a_classic_edit_controls_caret_show_is_its_caret_event() {
    common::init_com();
    let title = common::unique_title("mockapp-caret-watch-edit-show");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The keyboard focus cannot be given to the control on a desktop of
    // its own, so its host has the focused state, which an MSAA focus
    // needs on the object or an ancestor.
    app.send("set-focus root");
    let outpost = OutpostUnderTest::new(&app);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    outpost.deliver(verbatim_outpost::protocol::DeliveredFact::MsaaFocus {
        hwnd: edit.0 as isize,
        id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        id_child: 0,
    });
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node,
        other => panic!("the outpost said {other:?}, not the edit control's focus"),
    };
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == node.id => assert_eq!(caret.line.offset, 0),
        other => panic!("the outpost said {other:?}, not the edit control's caret"),
    }
    outpost.settled();
    app.send("caret doc 6");
    // Unfocused on this desktop, the control raises nothing for the move,
    // so mockapp raises the show.
    app.send("system-caret show");
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == node.id => {
            assert_eq!(
                (caret.line.text.as_str(), caret.line.offset),
                ("alpha beta\r\n", 6)
            );
        }
        other => panic!("the outpost said {other:?}, not the edit control's caret"),
    }
    outpost.heard(&[Heard::Msaa(WinEventKind::Caret)]);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The selection changes of the outpost's next message, which must be the
/// caret report of `node`.
fn reported_changes(outpost: &OutpostUnderTest, node: NodeId) -> Vec<SelectionChange> {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::CaretMoved {
                    node_id,
                    selection_changes,
                    ..
                },
            ..
        } if node_id == node => selection_changes,
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
}

/// A change of `text`, five characters, selected or not.
fn change(selected: bool, text: &str) -> SelectionChange {
    SelectionChange {
        selected,
        text: text.to_owned(),
        characters: 5,
    }
}

/// A selection no key made, as the mouse or the application makes it, is
/// reported with the caret event that follows it, as how it changed from
/// the selection in the outpost's last report: "alpha" selected from the
/// caret at the start, then unselected as the caret collapses at its end.
fn a_selection_no_key_made_is_reported_with_its_change() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-selection");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), [change(true, "alpha")]);
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret doc 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), [change(false, "alpha")]);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A selection a caret key made is answered to the key's watch, which takes
/// the key's caret event, so no caret report carries it; nor does the
/// report of a later caret event, the key's answer being the last report.
fn a_selection_a_key_made_is_reported_to_the_key_alone() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-key-selection");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let caret = TextPosition {
        anchor: before.line.start,
        offset: before.line.offset,
    };
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(caret),
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            previous_selection: Some(PreviousSelection {
                start: caret,
                end: caret,
            }),
        }),
    });
    outpost.settled();
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(reply.selection_changes, [change(true, "alpha")]);
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), []);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A selection Verbatim made, as the review copy's first press makes it, is
/// answered to Verbatim, and the caret event that follows carries no change.
fn a_selection_verbatim_made_is_not_reported_again() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-own-selection");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let at = |offset| {
        TextPoint::At(TextPosition {
            anchor: before.line.start,
            offset,
        })
    };
    let select = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::Select {
            start: at(0),
            end: at(5),
            through: None,
        },
    });
    assert_eq!(
        answer_to(&outpost, select),
        TextReply::Selected(vec![change(true, "alpha")])
    );
    outpost.settled();
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), []);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The caret's report and the text change of `node` that the outpost said
/// next: a caret event raised before its text's change, both waiting when
/// the outpost took the caret event, as a key typed over a selection
/// raises them. The text changed since the selection was last reported, so
/// the report carries no change, as it carries none when the text change
/// comes first (NVDA's `hasContentChangedSinceLastSelection`, set when a
/// value change waits at the caret event, words Verbatim does not yet say).
fn typed_over_the_selection(outpost: &OutpostUnderTest, node: NodeId) {
    assert_eq!(reported_changes(outpost, node), []);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::TextChanged { node_id },
            ..
        } if node_id == node => {}
        other => panic!("the outpost said {other:?}, not the text's change"),
    }
    outpost.settled();
}

/// "x" typed over the selected "alpha" in a document read through UIA, its
/// caret event raised before its text change.
fn a_caret_event_before_its_text_change_carries_no_selection_change() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-typed-over");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), [change(true, "alpha")]);
    heard_caret_event(&outpost);
    outpost.settled();
    outpost.hold_worker();
    app.send(r"set-text doc x beta\ngamma\n");
    app.send("caret doc 1");
    app.send("caret-event doc");
    app.send("text-changed doc");
    // UIA raises a value change `WinEvent` alongside the text change.
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.release_worker();
    typed_over_the_selection(&outpost, notes);
    drop(outpost);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control, its
/// caret event raised before its value change, as the Windows Forms text
/// box raises the caret's location change before the client's value
/// change (`docs/design/phase6-design.md`, "Live caret event checks").
fn an_edit_controls_caret_event_before_its_value_change_carries_no_selection_change() {
    common::init_com();
    let title = common::unique_title("mockapp-caret-watch-edit-typed-over");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // The host has the focused state, as in
    // `a_classic_edit_controls_caret_show_is_its_caret_event`.
    app.send("set-focus root");
    let outpost = OutpostUnderTest::new(&app);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(hwnd),
            None,
            windows::core::w!("EDIT"),
            None,
        )
    }
    .expect("mockapp's edit control");
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: edit.0 as isize,
        id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        id_child: 0,
    });
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::FocusChanged { node, .. },
            ..
        } => node.id,
        other => panic!("the outpost said {other:?}, not the edit control's focus"),
    };
    assert_eq!(reported_changes(&outpost, node), []);
    outpost.settled();
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, node), [change(true, "alpha")]);
    outpost.heard(&[Heard::Msaa(WinEventKind::TextSelectionChange)]);
    outpost.settled();
    outpost.hold_worker_on(common::another_applications_window().0 as isize);
    app.send(r"set-text doc x beta\ngamma\n");
    app.send("caret doc 1");
    app.send("caret-event doc");
    app.send("text-changed doc");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.release_worker();
    typed_over_the_selection(&outpost, node);
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_focus_change_during_a_caret_watch_is_handled_at_once",
            a_focus_change_during_a_caret_watch_is_handled_at_once,
        ),
        (
            "a_caret_move_raised_later_answers_the_watch",
            a_caret_move_raised_later_answers_the_watch,
        ),
        (
            "a_key_that_moves_nothing_is_answered_with_nothing",
            a_key_that_moves_nothing_is_answered_with_nothing,
        ),
        (
            "a_watchs_end_reads_no_focus_while_a_marker_waits",
            a_watchs_end_reads_no_focus_while_a_marker_waits,
        ),
        (
            "a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event",
            a_text_pattern_missing_at_the_focus_is_read_at_the_next_caret_event,
        ),
        (
            "the_system_carets_move_is_the_uia_focuss_caret_event",
            the_system_carets_move_is_the_uia_focuss_caret_event,
        ),
        (
            "a_focus_with_uia_caret_events_has_no_system_caret_events",
            a_focus_with_uia_caret_events_has_no_system_caret_events,
        ),
        (
            "a_new_focus_uses_the_system_caret_again",
            a_new_focus_uses_the_system_caret_again,
        ),
        (
            "a_classic_edit_controls_caret_show_is_its_caret_event",
            a_classic_edit_controls_caret_show_is_its_caret_event,
        ),
        (
            "a_selection_no_key_made_is_reported_with_its_change",
            a_selection_no_key_made_is_reported_with_its_change,
        ),
        (
            "a_selection_a_key_made_is_reported_to_the_key_alone",
            a_selection_a_key_made_is_reported_to_the_key_alone,
        ),
        (
            "a_selection_verbatim_made_is_not_reported_again",
            a_selection_verbatim_made_is_not_reported_again,
        ),
        (
            "a_caret_event_before_its_text_change_carries_no_selection_change",
            a_caret_event_before_its_text_change_carries_no_selection_change,
        ),
        (
            "an_edit_controls_caret_event_before_its_value_change_carries_no_selection_change",
            an_edit_controls_caret_event_before_its_value_change_carries_no_selection_change,
        ),
    ]);
}
