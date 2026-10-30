//! A caret key's watch for evidence in a real outpost, against mockapp's
//! UIA text provider: the worker never waits for a caret key, so a focus
//! change made while a key's watch is open is handled at once, a caret move
//! the application reports later still answers the watch, a key that
//! moves nothing is answered by nothing at all, and a text pattern missing
//! as the focus arrives is read once the application raises a caret event,
//! and the system caret's move is the caret event of a UIA focus that
//! raises none of its own, as its show is a classic edit control's; and a
//! selection no key made is reported with its change, one a key or
//! Verbatim made is not; and a selection change after a text change is
//! said in general terms, through UIA once the old selection's range no
//! longer holds its text, and in a classic edit control in either order of
//! its caret event and value change; and a deleting key's answer compares
//! with the selection the outpost kept, in both backends, so a caret
//! report that compared it first leaves the answer no change, and a
//! selection moved by a report read after the key is evidence of it; a
//! Shift key after a selection Verbatim made or a caret it moved compares
//! with it; and a selection's start is reported in the line it starts in;
//! and a text field a focus-now query answered with, and no focus event
//! reported, has a caret event no key caused reported; and a held key's
//! watch is judged from the caret read before the key.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::protocol::{DeliveredFact, OutpostToCore, Query, QueryOutcome, QueryResult};
use verbatim_model::{
    CaretReply, CaretReport, CaretWatch, FocusText, NodeId, NormalizedEvent, SelectionChange,
    TextOp, TextPoint, TextPosition, TextReply, TextUnit,
};
use verbatim_outpost::Heard;
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
) -> verbatim_model::protocol::DeliveredFact {
    app.send(&format!("set-focus {id}"));
    let element = focused(hwnd);
    outpost.listener_fact(&element)
}

/// Focuses mockapp's "Notes" document and returns its node and the caret
/// the outpost reported with the focus, which must be all it said.
fn focus_notes(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    hwnd: HWND,
) -> (NodeId, CaretReport) {
    outpost.deliver(focus_fact(app, outpost, hwnd, "doc"));
    let (node, caret) = match outpost.next() {
        OutpostToCore::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => {
            let FocusText::Caret { caret, .. } = *text else {
                unreachable!("matched by the guard");
            };
            (node, caret)
        }
        other => panic!("the outpost said {other:?}, not the document's focus with its caret"),
    };
    assert_eq!(node.name.as_deref(), Some("Notes"));
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
        deletes: false,
    })
}

/// The answer to caret key `key`, which must be the outpost's next message.
fn answer_to(outpost: &OutpostUnderTest, key: u64) -> TextReply {
    match outpost.next() {
        OutpostToCore::Reply {
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
        OutpostToCore::Event {
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

/// A held caret key's caret, read before the key is passed on
/// (`TextOp::ReadCaret`, `docs/design/key-holding.md`), is its watch's
/// baseline, though the application moved the caret there without
/// reporting it: the watch stays open until the key's own move, and is
/// answered with the caret where the key took it. Judged from the focus's
/// report, the newest the outpost had made before the read, the
/// unreported move would have answered the watch at once.
fn a_held_keys_watch_is_judged_from_the_caret_read_before_it() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-held");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    // The application moves its caret and reports nothing.
    app.send("caret doc 3");
    let read = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::ReadCaret,
    });
    let before = caret_reply(answer_to(&outpost, read));
    assert!(!before.moved);
    assert_eq!(
        (before.caret.line.text.as_str(), before.caret.line.offset),
        (
            "alpha beta
",
            3
        )
    );
    outpost.settled();
    // The key is passed on just after the read, as Core takes it.
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(TextPosition {
                anchor: before.caret.line.start,
                offset: before.caret.line.offset,
            }),
            pressed_at_ms: before.read_at_ms + 1,
            unit: TextUnit::Character,
            compare: None,
            deletes: false,
        }),
    });
    // The watch is open: the caret is where the read found it.
    outpost.settled();
    app.send("caret doc 4");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert!(reply.moved);
    assert_eq!(reply.caret.line.offset, 4);
    assert_eq!(reply.unit.expect("the character").text, "a");
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A text field the outpost was asked for with a focus-now query, as Core
/// asks at its start and after an outpost is replaced, with no focus event
/// for it, becomes the outpost's focus as a reported focus does: a caret
/// event no key caused is reported with the caret where it moved.
fn a_focus_now_text_fields_caret_event_is_reported() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-focus-now");
    app.send("set-focus doc");
    outpost.read_focus_as(focused(hwnd).live().element());
    let control = outpost
        .focus_now()
        .expect("the focus now has a focused control");
    let notes = control.node.id;
    app.send("caret doc 1");
    app.send("caret-event doc");
    match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == notes => assert_eq!(
            (caret.line.text.as_str(), caret.line.offset),
            ("alpha beta\n", 1)
        ),
        other => panic!("the outpost said {other:?}, not the document's caret"),
    }
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

/// Windows 11 Notepad, starting up, once answered that its document had
/// no text pattern (`docs/parity/text.md`, "A text pattern missing at the
/// focus"), and
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
        OutpostToCore::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::NoText) => node.id,
        other => panic!("the outpost said {other:?}, not the document's focus with no text"),
    };
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: None,
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            deletes: false,
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
        OutpostToCore::Event {
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
        OutpostToCore::Event {
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
        OutpostToCore::Event {
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
        OutpostToCore::Event {
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
        OutpostToCore::Event {
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
    outpost.deliver(verbatim_model::protocol::DeliveredFact::MsaaFocus {
        hwnd: edit.0 as isize,
        id_object: windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT.0,
        id_child: 0,
    });
    let node = match outpost.next() {
        OutpostToCore::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => {
            let FocusText::Caret { caret, .. } = *text else {
                unreachable!("matched by the guard");
            };
            assert_eq!(caret.line.offset, 0);
            node
        }
        other => panic!("the outpost said {other:?}, not the edit control's focus with its caret"),
    };
    outpost.settled();
    app.send("caret doc 6");
    // Unfocused on this desktop, the control raises nothing for the move,
    // so mockapp raises the show.
    app.send("system-caret show");
    match outpost.next() {
        OutpostToCore::Event {
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
        OutpostToCore::Event {
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

/// A selection made around the caret, as a double-click inside a word
/// makes it, is one change, the whole selection, and so is a caret left
/// inside it, the whole selection unselected, as NVDA speaks each when the
/// other side holds nothing (`docs/parity/text.md`, "TextInfo-equivalent
/// layer").
fn a_selection_around_the_caret_is_one_change_each_way() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-around");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), []);
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret doc 3 8");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), [change(true, "ha be")]);
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret doc 5");
    app.send("caret-event doc");
    assert_eq!(reported_changes(&outpost, notes), [change(false, "ha be")]);
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The caret report of `node` the outpost said next.
fn reported_caret(outpost: &OutpostUnderTest, node: NodeId) -> CaretReport {
    match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::CaretMoved { node_id, caret, .. },
            ..
        } if node_id == node => caret,
        other => panic!("the outpost said {other:?}, not the caret of {node:?}"),
    }
}

/// A selection whose caret is its end, as Windows 11 Notepad's
/// `TextPattern2` caret is after Shift+Control+Right Arrow: the report
/// gives the caret's line with the caret at that end, and the selection's
/// start as a position in the same line, so Core knows what comes before
/// the start, which a Backspace over the selection says, as NVDA reads it
/// from its caret, the selection's start (`docs/parity/text.md`,
/// "TextInfo-equivalent layer"). A selection that starts on an earlier
/// line has that line reported with it, its start a position there.
fn a_selections_start_on_the_carets_line_is_reported_in_it() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-selection-start");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 6 10 end");
    app.send("caret-event doc");
    let caret = reported_caret(&outpost, notes);
    assert_eq!(
        (
            caret.line.text.as_str(),
            caret.line.offset,
            caret.selection.map(|selection| selection.start)
        ),
        (
            "alpha beta\n",
            10,
            Some(TextPosition {
                anchor: caret.line.start,
                offset: 6,
            })
        )
    );
    heard_caret_event(&outpost);
    outpost.settled();
    app.send("caret doc 6 13 end");
    app.send("caret-event doc");
    let caret = reported_caret(&outpost, notes);
    assert_eq!(caret.line.text, "gamma\n");
    let start_line = caret
        .selection_line
        .expect("the line the selection starts in");
    assert_eq!(
        (
            start_line.text.as_str(),
            caret.selection.map(|selection| selection.start)
        ),
        (
            "alpha beta\n",
            Some(TextPosition {
                anchor: start_line.start,
                offset: 6,
            })
        )
    );
    heard_caret_event(&outpost);
    outpost.settled();
    // A selection from the text's start: its line is reported as the
    // text's first, so Core knows nothing comes before it.
    app.send("caret doc 0 5 end");
    app.send("caret-event doc");
    let caret = reported_caret(&outpost, notes);
    assert_eq!(
        (
            caret.line.first,
            caret.selection.map(|selection| selection.start)
        ),
        (
            true,
            Some(TextPosition {
                anchor: caret.line.start,
                offset: 0,
            })
        )
    );
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
            deletes: false,
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

/// A Shift key's watch from `caret`, as Core sends it: no selection of
/// its own, the answer comparing with the selection the outpost kept.
fn shift_key(caret: TextPosition) -> TextOp {
    TextOp::AwaitCaret(CaretWatch {
        landing: false,
        since: Some(caret),
        pressed_at_ms: 0,
        unit: TextUnit::Character,
        compare: None,
        deletes: false,
    })
}

/// A Shift key after a selection Verbatim made, as the review copy's
/// first press makes it, compares with that selection, which the outpost
/// keeps as it keeps a caret report's: Shift+Right Arrow after "alpha"
/// was selected says " " selected. So does one after Verbatim moved the
/// caret, from the caret where it was moved: Shift+Control+Right Arrow
/// there says "beta" selected. Core sends no selection of its own for
/// either.
fn a_shift_key_after_verbatims_selection_compares_with_it() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-after-own-selection");
    let (notes, before) = focus_notes(&mut app, &outpost, hwnd);
    let at = |offset| TextPosition {
        anchor: before.line.start,
        offset,
    };
    let select = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::Select {
            start: TextPoint::At(at(0)),
            end: TextPoint::At(at(5)),
            through: None,
        },
    });
    assert_eq!(
        answer_to(&outpost, select),
        TextReply::Selected(vec![change(true, "alpha")])
    );
    outpost.settled();
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: shift_key(at(0)),
    });
    outpost.settled();
    app.send("caret doc 0 6");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        reply.selection_changes,
        [SelectionChange {
            selected: true,
            text: " ".to_owned(),
            characters: 1,
        }]
    );
    heard_caret_event(&outpost);
    outpost.settled();

    let moved = outpost.ask(Query::Text {
        node_id: notes,
        op: TextOp::MoveCaret(TextPoint::At(at(6))),
    });
    assert_eq!(answer_to(&outpost, moved), TextReply::Done);
    outpost.settled();
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: shift_key(at(6)),
    });
    outpost.settled();
    app.send("caret doc 6 10");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        reply.selection_changes,
        [SelectionChange {
            selected: true,
            text: "beta".to_owned(),
            characters: 4,
        }]
    );
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The caret report of `node` that the outpost said next: its selection
/// changes, whether they are said in general terms, and the new selection's
/// text that comes with them.
fn reported_selection(
    outpost: &OutpostUnderTest,
    node: NodeId,
) -> (Vec<SelectionChange>, bool, Option<SelectionChange>) {
    match outpost.next() {
        OutpostToCore::Event {
            event:
                NormalizedEvent::CaretMoved {
                    node_id,
                    selection_changes,
                    content_changed,
                    new_selection,
                    ..
                },
            ..
        } if node_id == node => (selection_changes, content_changed, new_selection),
        other => panic!("the outpost said {other:?}, not the caret of {node:?}"),
    }
}

/// The text change of `node`, which must be the outpost's next message.
fn text_changed(outpost: &OutpostUnderTest, node: NodeId) {
    match outpost.next() {
        OutpostToCore::Event {
            event: NormalizedEvent::TextChanged { node_id },
            ..
        } if node_id == node => {}
        other => panic!("the outpost said {other:?}, not the text's change"),
    }
}

/// An unselection said in general terms, which carries no text.
fn unselected_in_general() -> SelectionChange {
    SelectionChange {
        selected: false,
        text: String::new(),
        characters: 0,
    }
}

/// "x" typed over the selected "a" in a document read through UIA, its
/// caret event raised before its text change: the old selection's range
/// no longer holds the "a" it held when it was reported, so the report
/// says its change in general terms, the selection removed, Dickson's
/// deliberate difference from NVDA, which handles no text change for such
/// a document and says "x unselected" in Windows 11 Notepad
/// (`docs/parity/text.md`, "TextInfo-equivalent layer";
/// `target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-notepad-1.txt`).
/// mockapp's ranges are offsets into the text as it now is, so the old
/// range holds what a range that follows the edit would: one character.
fn a_uia_selection_typed_over_is_said_in_general_terms() {
    let (mut app, hwnd, outpost) = start("mockapp-caret-watch-typed-over");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 0 1");
    app.send("caret-event doc");
    assert_eq!(
        reported_selection(&outpost, notes),
        (
            vec![SelectionChange {
                selected: true,
                text: "a".to_owned(),
                characters: 1,
            }],
            false,
            None
        )
    );
    heard_caret_event(&outpost);
    outpost.settled();
    outpost.hold_worker();
    app.send(r"set-text doc xlpha beta\ngamma\n");
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
    assert_eq!(
        reported_selection(&outpost, notes),
        (vec![unselected_in_general()], true, None)
    );
    text_changed(&outpost, notes);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// mockapp's classic edit control focused, with an outpost watching it,
/// and "alpha" selected: the app, the outpost, the edit control's node,
/// and the caret reported with the selection.
fn focus_edit(name: &str) -> (common::MockApp, OutpostUnderTest, NodeId, CaretReport) {
    common::init_com();
    let title = common::unique_title(name);
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
        OutpostToCore::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => node.id,
        other => panic!("the outpost said {other:?}, not the edit control's focus with its caret"),
    };
    outpost.settled();
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    let caret = match outpost.next() {
        OutpostToCore::Event {
            event:
                NormalizedEvent::CaretMoved {
                    node_id,
                    caret,
                    selection_changes,
                    content_changed,
                    new_selection,
                },
            ..
        } if node_id == node => {
            assert_eq!(
                (selection_changes, content_changed, new_selection),
                (vec![change(true, "alpha")], false, None)
            );
            caret
        }
        other => panic!("the outpost said {other:?}, not the caret of {node:?}"),
    };
    outpost.heard(&[Heard::Msaa(WinEventKind::TextSelectionChange)]);
    outpost.settled();
    (app, outpost, node, caret)
}

/// "x" typed over the selected "alpha" in a classic edit control, its
/// caret event raised before its value change, as the Windows Forms text
/// box raises the caret's location change before the client's value
/// change (`docs/crates/verbatim-outpost.md`, "Text and caret", checked live
/// on 2026-10-08): the
/// value change waiting at the caret event, the report says its change in
/// general terms, the selection removed, as NVDA's edit control does
/// (`target/e2e-evidence/m42-desktop-2-2026-10-22/part3b-textbox-1-type-over.txt`).
fn an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms() {
    let (mut app, outpost, node, _) = focus_edit("mockapp-caret-watch-edit-typed-over");
    outpost.hold_worker_on(common::another_applications_window().0 as isize);
    app.send(r"set-text doc x beta\ngamma\n");
    app.send("caret doc 1");
    app.send("caret-event doc");
    app.send("text-changed doc");
    // The control raises a value change of its own for the new text,
    // before the caret event. Only once the later value change is heard
    // has it replaced that one at the back of the intake, so both are
    // waited for before the worker goes on.
    outpost.heard(&[
        Heard::Msaa(WinEventKind::ValueChange),
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.release_worker();
    assert_eq!(
        reported_selection(&outpost, node),
        (vec![unselected_in_general()], true, None)
    );
    text_changed(&outpost, node);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control, its value
/// change handled before its caret event: the caret event's report says
/// its change in general terms, as when the value change waits behind it.
fn an_edit_controls_value_change_handled_first_has_the_next_change_said_in_general_terms() {
    let (mut app, outpost, node, _) = focus_edit("mockapp-caret-watch-edit-changed-first");
    app.send(r"set-text doc x beta\ngamma\n");
    outpost.heard(&[Heard::Msaa(WinEventKind::ValueChange)]);
    text_changed(&outpost, node);
    outpost.settled();
    app.send("caret doc 1");
    app.send("caret-event doc");
    outpost.heard(&[Heard::Msaa(WinEventKind::TextSelectionChange)]);
    assert_eq!(
        reported_selection(&outpost, node),
        (vec![unselected_in_general()], true, None)
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// "x" typed over the selected "alpha" in a classic edit control while the
/// caret event before it is read, as the Windows Forms text box handles a
/// typed character: the caret event is taken from the intake with no value
/// change waiting, and the control types the character and raises its
/// value change as it answers the read, which then sees the edited text.
/// The value change, checked for once the caret is read and behind the
/// `WinEvent` thread's queue, has the report say its change in general
/// terms, the selection removed, as when the value change waits behind the
/// caret event from the start.
fn an_edit_controls_value_change_raised_during_its_caret_read_is_said_in_general_terms() {
    let (mut app, outpost, node, _) = focus_edit("mockapp-caret-watch-edit-typed-during-read");
    app.send("write-on-read doc x");
    app.send("caret-event doc");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    assert_eq!(
        reported_selection(&outpost, node),
        (vec![unselected_in_general()], true, None)
    );
    text_changed(&outpost, node);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Delete's watch over the selection `before` reports, with the character
/// at its caret to compare, as Core sends it: no selection of its own, the
/// answer comparing with the selection the outpost kept.
fn delete_over(before: &CaretReport, pressed_at_ms: u64) -> TextOp {
    let at = &before.line.text[before.line.offset as usize..];
    TextOp::AwaitCaret(CaretWatch {
        landing: false,
        since: Some(TextPosition {
            anchor: before.line.start,
            offset: before.line.offset,
        }),
        pressed_at_ms,
        unit: TextUnit::Character,
        compare: Some(at.chars().next().map(String::from).unwrap_or_default()),
        deletes: true,
    })
}

/// The selected "alpha" deleted from a classic edit control with Delete,
/// its caret event raised before its value change, as the Windows Forms
/// text box raises them: the answer compares with the selection the
/// outpost kept, and with the value change waiting says its change in
/// general terms, the selection removed, as NVDA's edit control does at
/// the key's caret event
/// (`target/e2e-evidence/m42-selection-keys-2026-10-22/textbox-word-delete.txt`).
fn an_edit_controls_deleting_key_over_a_selection_is_answered_in_general_terms() {
    let (mut app, mut outpost, node, before) = focus_edit("mockapp-caret-watch-edit-delete");
    let key = outpost.ask(Query::Text {
        node_id: node,
        op: delete_over(&before, 0),
    });
    outpost.settled();
    outpost.hold_worker_on(common::another_applications_window().0 as isize);
    app.send(r"set-text doc  beta\ngamma\n");
    app.send("caret doc 0");
    app.send("caret-event doc");
    app.send("text-changed doc");
    // The control's own value change for the new text is waited for too,
    // as in `an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms`.
    outpost.heard(&[
        Heard::Msaa(WinEventKind::ValueChange),
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.release_worker();
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        (
            reply.selection_changes,
            reply.content_changed,
            reply.new_selection
        ),
        (vec![unselected_in_general()], true, None)
    );
    text_changed(&outpost, node);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The selected "alpha" deleted from a classic edit control with Delete,
/// the control's value change and then its caret event both handled
/// before the key's watch arrives: the caret event's report compares with
/// the selection kept, in general terms, the selection removed, and
/// replaces it, so the key's answer has no change left to say; Core says
/// the report's change after the key's own words.
fn a_caret_report_before_a_keys_watch_leaves_its_answer_no_selection_change() {
    let (mut app, mut outpost, node, before) =
        focus_edit("mockapp-caret-watch-edit-delete-reported");
    let pressed_at_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds that fit");
    app.send(r"set-text doc  beta\ngamma\n");
    // The control raises its value change for the new text itself.
    app.send("caret doc 0");
    app.send("caret-event doc");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::ValueChange),
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
    text_changed(&outpost, node);
    assert_eq!(
        reported_selection(&outpost, node),
        (vec![unselected_in_general()], true, None)
    );
    outpost.settled();
    let key = outpost.ask(Query::Text {
        node_id: node,
        op: delete_over(&before, pressed_at_ms),
    });
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        (
            reply.selection_changes,
            reply.content_changed,
            reply.new_selection
        ),
        (Vec::new(), false, None)
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key that changes nothing over the selected "alpha" in a classic edit
/// control, as Control+C copies it, its watch arriving only after the
/// selection was extended to "alpha be", as Shift and a click extend it:
/// that caret event's report says " be" selected and replaces the
/// selection the outpost kept, and the caret, the selection's start, stays
/// where it was. The selection in the report the watch takes its baseline
/// from, before the key, has ends other than the selection's now, which is
/// evidence: the key is answered at once, with no change left to say, so
/// Core says the report's words then, as NVDA says them at once, and not
/// when the watch reaches its bound.
fn a_selection_moved_before_a_keys_watch_is_evidence_by_its_ends() {
    let (mut app, mut outpost, node, before) = focus_edit("mockapp-caret-watch-edit-extended");
    let pressed_at_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds that fit");
    app.send("caret doc 0 8");
    app.send("caret-event doc");
    outpost.heard(&[Heard::Msaa(WinEventKind::TextSelectionChange)]);
    assert_eq!(
        reported_selection(&outpost, node),
        (
            vec![SelectionChange {
                selected: true,
                text: " be".to_owned(),
                characters: 3,
            }],
            false,
            None
        )
    );
    outpost.settled();
    let key = outpost.ask(Query::Text {
        node_id: node,
        op: TextOp::AwaitCaret(CaretWatch {
            landing: true,
            since: Some(TextPosition {
                anchor: before.line.start,
                offset: before.line.offset,
            }),
            pressed_at_ms,
            unit: TextUnit::Word,
            compare: None,
            deletes: false,
        }),
    });
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        (
            reply.moved,
            reply.selection_changes,
            reply.content_changed,
            reply.new_selection
        ),
        (true, Vec::new(), false, None)
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The selected "alpha" deleted from a document read through UIA with
/// Delete: the answer compares with the selection the outpost kept, whose
/// range no longer holds the "alpha" it held when it was reported, so the
/// answer says its change in general terms, the selection removed, as
/// Verbatim says it in Windows 11 Notepad where NVDA says nothing
/// (`docs/parity/text.md`, "TextInfo-equivalent layer";
/// `target/e2e-evidence/m42-selection-keys-2026-10-22/notepad-word-delete.txt`).
fn a_uia_deleting_key_over_a_selection_is_answered_in_general_terms() {
    let (mut app, hwnd, mut outpost) = start("mockapp-caret-watch-delete");
    let (notes, _) = focus_notes(&mut app, &outpost, hwnd);
    app.send("caret doc 0 5");
    app.send("caret-event doc");
    let before = match outpost.next() {
        OutpostToCore::Event {
            event:
                NormalizedEvent::CaretMoved {
                    node_id,
                    caret,
                    selection_changes,
                    ..
                },
            ..
        } if node_id == notes => {
            assert_eq!(selection_changes, [change(true, "alpha")]);
            caret
        }
        other => panic!("the outpost said {other:?}, not the document's caret"),
    };
    heard_caret_event(&outpost);
    outpost.settled();
    let key = outpost.ask(Query::Text {
        node_id: notes,
        op: delete_over(&before, 0),
    });
    outpost.settled();
    app.send(r"set-text doc  beta\ngamma\n");
    app.send("caret doc 0");
    app.send("caret-event doc");
    let reply = caret_reply(answer_to(&outpost, key));
    assert_eq!(
        (
            reply.selection_changes,
            reply.content_changed,
            reply.new_selection
        ),
        (vec![unselected_in_general()], true, None)
    );
    heard_caret_event(&outpost);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_focus_now_text_fields_caret_event_is_reported",
            a_focus_now_text_fields_caret_event_is_reported,
        ),
        (
            "a_focus_change_during_a_caret_watch_is_handled_at_once",
            a_focus_change_during_a_caret_watch_is_handled_at_once,
        ),
        (
            "a_caret_move_raised_later_answers_the_watch",
            a_caret_move_raised_later_answers_the_watch,
        ),
        (
            "a_held_keys_watch_is_judged_from_the_caret_read_before_it",
            a_held_keys_watch_is_judged_from_the_caret_read_before_it,
        ),
        (
            "a_key_that_moves_nothing_is_answered_with_nothing",
            a_key_that_moves_nothing_is_answered_with_nothing,
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
            "a_selection_around_the_caret_is_one_change_each_way",
            a_selection_around_the_caret_is_one_change_each_way,
        ),
        (
            "a_selections_start_on_the_carets_line_is_reported_in_it",
            a_selections_start_on_the_carets_line_is_reported_in_it,
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
            "a_shift_key_after_verbatims_selection_compares_with_it",
            a_shift_key_after_verbatims_selection_compares_with_it,
        ),
        (
            "a_uia_selection_typed_over_is_said_in_general_terms",
            a_uia_selection_typed_over_is_said_in_general_terms,
        ),
        (
            "an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms",
            an_edit_controls_caret_event_before_its_value_change_is_said_in_general_terms,
        ),
        (
            "an_edit_controls_value_change_handled_first_has_the_next_change_said_in_general_terms",
            an_edit_controls_value_change_handled_first_has_the_next_change_said_in_general_terms,
        ),
        (
            "an_edit_controls_value_change_raised_during_its_caret_read_is_said_in_general_terms",
            an_edit_controls_value_change_raised_during_its_caret_read_is_said_in_general_terms,
        ),
        (
            "an_edit_controls_deleting_key_over_a_selection_is_answered_in_general_terms",
            an_edit_controls_deleting_key_over_a_selection_is_answered_in_general_terms,
        ),
        (
            "a_caret_report_before_a_keys_watch_leaves_its_answer_no_selection_change",
            a_caret_report_before_a_keys_watch_leaves_its_answer_no_selection_change,
        ),
        (
            "a_selection_moved_before_a_keys_watch_is_evidence_by_its_ends",
            a_selection_moved_before_a_keys_watch_is_evidence_by_its_ends,
        ),
        (
            "a_uia_deleting_key_over_a_selection_is_answered_in_general_terms",
            a_uia_deleting_key_over_a_selection_is_answered_in_general_terms,
        ),
    ]);
}
