//! A focused terminal's on-demand reading (`docs/design/terminal-model.md`,
//! section 5.1) through a real outpost's worker, against mockapp's UIA text
//! provider presenting a Windows Terminal control (UIA class
//! `TermControl`), its text given as a terminal gives it, every row padded
//! to the width and ended by a carriage return and line feed: each
//! transition of `verbatim_outpost::terminal::reading` as the worker drives
//! it, by text change events and Core's requests.
//!
//! - Live, a text change is read at once and the read sent to Core as a
//!   snapshot, with the caret read with it; a read identical to the last
//!   one sent is not sent again.
//! - Held (`TerminalHold`), a change is only noted: nothing is sent.
//! - `TerminalRead` answers with a read when the text changed since the
//!   last read, and reading is live again; asked with nothing changed, it
//!   answers that nothing changed without reading, and asked to hold, the
//!   terminal stays held.
//! - `TerminalCancel` answers with a read of what changed while held, and
//!   reading is live again.
//! - Core's requests for a read at once (`TerminalReadNow`), for rows of
//!   the history (`TerminalHistory`), for the colours after a key
//!   (`TerminalColours`) and the screen's shape (`TerminalShape`) are
//!   answered, a colours request with a read with colours at once when a
//!   read began after the key without them.
//! - The read before a held key (`ReadCaret`) is a snapshot with the caret.
//! - A caret event waiting ahead of a text change in one batch is planned
//!   after it, so the terminal is read once, the caret with the text.
//! - Held with its text changed, a caret event is only noted, and the read
//!   Core asks for reads the caret with the text.
//! - The console host's text area, in a window of the console's class,
//!   takes its caret event from its console (`EVENT_CONSOLE_CARET`), and
//!   UI Automation's caret event is not subscribed for it, and the system
//!   caret's events are not used for it.
//! - A caret key's watch that knows no caret before the key is answered by
//!   a read that covers a caret event waiting behind it.
//! - A caret key's watch whose read finds the caret moved while the
//!   terminal writes between the read's calls, so that the line shows a
//!   character the caret was read before, answers nothing; the text change
//!   that follows answers it, whether the read is one remote operation or
//!   the classic read's separate calls, and its read says what the line
//!   shows the line with the caret past what it gained.
//! - While the console's window is the application's focus window and its
//!   foreground window, both read through UI Automation, the outpost's
//!   hook leaves out the change events of windows read through MSAA, UI
//!   Automation's `WinEvent` copies of its events among them, and hooks
//!   them again once the focus is in a Win32 button read through MSAA.
//! - A terminal that a focus-now query answered with, and no focus event
//!   reported, is the outpost's focus: its text change is read and sent;
//!   so is a focused terminal's text change after a
//!   foreground report of another window, which names a window and not the
//!   control focus is in, both before and after the same query answers with
//!   the terminal.
//!
//! While the terminal is held, the evidence that the outpost has taken a
//! text change is the report of a caret event raised after it: UI
//! Automation delivers one client's events from a provider in the order
//! they were raised, and the outpost reads the caret of a caret event noted
//! while held once it has nothing else to do. The two tests of one batch
//! hold the outpost's worker ([`OutpostUnderTest::hold_worker`]) while
//! mockapp raises the batch's events.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::{
    CaretWatch, FocusText, NodeId, NormalizedEvent, PropertyChange, Role, TerminalScope,
    TerminalSnapshot, TextOp, TextPosition, TextReply, TextUnit,
};
use verbatim_outpost::protocol::DeliveredFact;
use verbatim_outpost::protocol::{
    FocusedControl, OutpostToSupervisor, Query, QueryOutcome, QueryResult,
};
use verbatim_outpost::{Heard, OutpostOptions};
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UIA_HelpTextPropertyId;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, OBJID_CLIENT};
use windows::core::w;

use common::outpost::OutpostUnderTest;

/// The terminal's rows before each test writes to it.
const START: [&str; 3] = ["one", "two", "ready>"];

/// The terminal's width in cells.
const WIDTH: usize = 20;

/// `rows` as the terminal's text, each padded to [`WIDTH`] and ended by a
/// carriage return and line feed, escaped for `set-text`. The last line
/// break gives mockapp an empty last line, so that moving by lines from
/// the caret to the document's end counts the caret's row, as the
/// terminals' providers do.
fn terminal_text(rows: &[&str]) -> String {
    rows.iter()
        .map(|row| format!(r"{row:WIDTH$}\r\n"))
        .collect::<Vec<_>>()
        .concat()
}

/// The element mockapp reports as having the keyboard focus, built with the
/// listener's cache request, as a focus event's element arrives.
fn focused(hwnd: HWND) -> Cached<ListenerEvent> {
    let uia = Uia::new().expect("a UIA client");
    common::listener_focus(&uia, hwnd.0 as isize)
}

/// The caret the outpost reports, which must be its next message.
fn caret_read(outpost: &OutpostUnderTest, node: NodeId) {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::CaretMoved { node_id, .. },
            ..
        } if node_id == node => {}
        other => panic!("the outpost said {other:?}, not the terminal's caret"),
    }
}

/// mockapp's terminal fixture in its UIA backend, with an outpost watching
/// it and the terminal focused: its focus said with its caret, and nothing
/// else.
fn start(name: &str) -> (common::MockApp, OutpostUnderTest, NodeId) {
    start_with(
        name,
        "terminal_reading.json",
        "term",
        OutpostOptions::default(),
    )
}

/// [`start`] for the terminal `id` of fixture `fixture`, the outpost
/// reading as `options` say.
fn start_with(
    name: &str,
    fixture: &str,
    id: &str,
    options: OutpostOptions,
) -> (common::MockApp, OutpostUnderTest, NodeId) {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::with_options(&app, options);
    // The screen: the text's last three rows, and the empty last line.
    app.send(&format!("screen {id} 4"));
    app.send(&format!("set-focus {id}"));
    let element = focused(hwnd);
    outpost.deliver(outpost.listener_fact(&element));
    let node = match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    node,
                    text: Some(text),
                    ..
                },
            ..
        } if matches!(*text, FocusText::Caret { .. }) => node,
        other => panic!("the outpost said {other:?}, not the terminal's focus with its caret"),
    };
    assert_eq!(node.role, Role::Terminal);
    // The terminal's first read, with its caret.
    let first = reported(&outpost, node.id);
    assert!(first.caret.is_some());
    outpost.settled();
    (app, outpost, node.id)
}

/// Writes `lines` after [`START`] as the terminal's whole text and raises
/// its text changed event, with the outpost's worker held until it has
/// taken in both the event and the `WinEvent` UI Automation raises for MSAA
/// clients beside it, so that one read covers both.
fn write(app: &mut common::MockApp, outpost: &OutpostUnderTest, lines: &[&str]) {
    outpost.hold_worker();
    write_text(app, lines);
    outpost.heard(&[
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
    outpost.release_worker();
}

/// Writes `lines` after [`START`] as the terminal's whole text and raises
/// its text changed event.
fn write_text(app: &mut common::MockApp, lines: &[&str]) {
    let rows: Vec<&str> = START.iter().chain(lines).copied().collect();
    app.send(&format!("set-text term {}", terminal_text(&rows)));
    app.send("text-changed term");
}

/// Writes `lines` as [`write`] does while the terminal is held, then moves
/// the caret to `caret` and raises its caret event, and takes its report:
/// the evidence that the outpost has taken the text change.
fn write_held(
    app: &mut common::MockApp,
    outpost: &OutpostUnderTest,
    node: NodeId,
    lines: &[&str],
    caret: usize,
) {
    write_text(app, lines);
    app.send(&format!("caret term {caret}"));
    app.send("caret-event term");
    outpost.heard(&[
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
    caret_read(outpost, node);
    outpost.settled();
}

/// The read of the terminal the outpost sends live, which must be its
/// next message.
fn reported(outpost: &OutpostUnderTest, node: NodeId) -> TerminalSnapshot {
    match outpost.next() {
        OutpostToSupervisor::Event {
            event: NormalizedEvent::TerminalSnapshot { node_id, snapshot },
            ..
        } if node_id == node => *snapshot,
        other => panic!("the outpost said {other:?}, not the terminal's read"),
    }
}

/// The answer to request `request`, which must be the outpost's next
/// message.
fn answer_to(outpost: &OutpostUnderTest, request: u64) -> TextReply {
    match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Text(reply)),
            ..
        } if request_id == request => reply,
        other => panic!("the outpost said {other:?}, not the answer to {request}"),
    }
}

/// The read answering request `request`, which must be the outpost's next
/// message.
fn read_answer(outpost: &OutpostUnderTest, request: u64) -> TerminalSnapshot {
    match answer_to(outpost, request) {
        TextReply::TerminalSnapshot(snapshot) => *snapshot,
        other => panic!("the outpost answered {other:?}, not with a read"),
    }
}

/// A read's text, its lines without their padding.
fn text(snapshot: &TerminalSnapshot) -> &str {
    &snapshot.text.text
}

/// The caret's line and column a read found.
fn caret_at(snapshot: &TerminalSnapshot) -> (&str, u32) {
    (&snapshot.caret_line.text, snapshot.caret_column)
}

/// The text of a read of the whole document, [`START`] and then `lines`,
/// as [`write`] writes them.
fn after(lines: &[&str]) -> String {
    let all: Vec<&str> = START.iter().chain(lines).copied().collect();
    rows(&all)
}

/// The text of a read: each of `rows` without its padding and ended by a
/// line break, then the empty last line.
fn rows(rows: &[&str]) -> String {
    rows.iter().map(|row| [row, "\r\n"].concat()).collect()
}

/// The focused control the outpost answers a focus-now query with, a
/// terminal, which it then reads first, as it reads a terminal that gains
/// the focus.
fn focus_now(outpost: &mut OutpostUnderTest) -> FocusedControl {
    let request = outpost.ask(Query::FocusNow);
    let control = match outpost.next() {
        OutpostToSupervisor::Reply {
            request_id,
            outcome: QueryOutcome::Done(QueryResult::Focus(now)),
            ..
        } if request_id == request => now.focus.expect("the focus now has a focused control"),
        other => panic!("the outpost said {other:?}, not the focus now"),
    };
    let first = reported(outpost, control.node.id);
    assert!(first.caret.is_some());
    outpost.settled();
    control
}

/// Asks the outpost `op` about `node`.
fn ask(outpost: &mut OutpostUnderTest, node: NodeId, op: TextOp) -> u64 {
    outpost.ask(Query::Text { node_id: node, op })
}

/// Holds the terminal, which is answered at once.
fn hold(outpost: &mut OutpostUnderTest, node: NodeId) {
    let hold = ask(outpost, node, TextOp::TerminalHold);
    assert_eq!(answer_to(outpost, hold), TextReply::Done);
    outpost.settled();
}

/// A terminal the outpost was asked for with a focus-now query, as Core
/// asks at its start and after an outpost is replaced, with no focus event
/// for it, becomes the outpost's focus as a reported focus does: its new
/// line is reported as the terminal's speech.
fn a_focus_now_terminal_reports_its_output() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-focus-now");
    let mut app = common::spawn("terminal_reading.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut outpost = OutpostUnderTest::new(&app);
    app.send("screen term 4");
    app.send("set-focus term");
    outpost.read_focus_as(focused(hwnd).live().element());
    let control = focus_now(&mut outpost);
    assert_eq!(control.node.role, Role::Terminal);
    let term = control.node.id;
    write(&mut app, &outpost, &["three"]);
    assert_eq!(text(&reported(&outpost, term)), after(&["three"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A focused terminal stays the outpost's terminal through a foreground
/// report that no focus event follows, which names a window, here the
/// window's Win32 button, and not the control focus is in: its text change
/// after the report is terminal output, and a focus-now answer for the
/// terminal after it, as Core asks for one after Alt+Tab with no focus
/// event, leaves the terminal reporting its output.
fn a_terminal_stays_followed_through_a_foreground_report() {
    let (mut app, mut outpost, term) = start_with(
        "mockapp-terminal-foreground",
        "terminal_reading_button.json",
        "term",
        OutpostOptions::default(),
    );
    let window = common::find_window(&app.title);
    // SAFETY: a local search of the window's children.
    let find = unsafe { FindWindowExW(Some(window), None, w!("Button"), w!("Find")) }
        .expect("mockapp made the button");
    outpost.set_foreground(find);
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: find.0 as isize,
    });
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::FocusChanged {
                    foreground: true, ..
                },
            ..
        } => {}
        other => panic!("the outpost said {other:?}, not the button's window as the foreground"),
    }
    outpost.settled();
    write_text(&mut app, &["three"]);
    assert_eq!(text(&reported(&outpost, term)), after(&["three"]));
    outpost.settled();
    outpost.set_foreground(window);
    outpost.read_focus_as(focused(window).live().element());
    let control = focus_now(&mut outpost);
    assert_eq!(control.node.id, term);
    write_text(&mut app, &["three", "four"]);
    assert_eq!(
        text(&reported(&outpost, term)),
        rows(&["two", "ready>", "three", "four"])
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Live, then held, then read: a change while held says nothing, and the
/// read answers with it; reading is then live again.
fn a_held_terminal_notes_changes_and_a_read_answers_them() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-held");
    write(&mut app, &outpost, &["three"]);
    assert_eq!(text(&reported(&outpost, term)), after(&["three"]));
    outpost.settled();

    hold(&mut outpost, term);
    write_held(&mut app, &outpost, term, &["three", "four"], 1);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    let answer = read_answer(&outpost, read);
    assert_eq!(text(&answer), rows(&["two", "ready>", "three", "four"]));
    // It waited for Core's speech queue.
    assert!(answer.waited);
    outpost.settled();

    // Live again.
    write(&mut app, &outpost, &["three", "four", "five"]);
    assert_eq!(
        text(&reported(&outpost, term)),
        rows(&["ready>", "three", "four", "five"])
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A read asked with nothing changed since the last answers that nothing
/// changed, and one asked to hold again leaves the terminal held.
fn a_read_with_nothing_new_answers_empty_and_may_hold_again() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-empty");
    hold(&mut outpost, term);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: true });
    assert_eq!(answer_to(&outpost, read), TextReply::TerminalUnchanged);
    outpost.settled();
    // Still held: a change is only noted.
    write_held(&mut app, &outpost, term, &["six"], 1);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    assert_eq!(text(&read_answer(&outpost, read)), after(&["six"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A cancel answers with what changed while held, and reading is live
/// again.
fn a_cancel_answers_what_is_new_and_reading_is_live_again() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-cancel");
    hold(&mut outpost, term);
    write_held(&mut app, &outpost, term, &["seven"], 1);
    let cancel = ask(&mut outpost, term, TextOp::TerminalCancel);
    let answer = read_answer(&outpost, cancel);
    assert_eq!(text(&answer), after(&["seven"]));
    assert!(!answer.waited);
    outpost.settled();
    write(&mut app, &outpost, &["seven", "eight"]);
    assert_eq!(
        text(&reported(&outpost, term)),
        rows(&["two", "ready>", "seven", "eight"])
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Waits for the outpost to take in the caret event and the text change
/// mockapp raised, each by both routes: UIA's own event, and the
/// `WinEvent` UIA raises for MSAA clients alongside it.
fn heard_caret_and_text(outpost: &OutpostUnderTest) {
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::UiaTextChanged,
        Heard::Msaa(WinEventKind::ValueChange),
    ]);
}

/// A caret event waiting ahead of a text change in one batch is planned
/// after it: the text change's read reads the caret with the text, and the
/// caret event, which that read saw, reads nothing more: one read, with
/// the caret moved.
fn a_caret_event_ahead_of_a_text_change_is_read_with_it() {
    let (mut app, outpost, term) = start("mockapp-terminal-caret-first");
    outpost.hold_worker();
    app.send("caret term 1");
    app.send("caret-event term");
    write_text(&mut app, &["three"]);
    heard_caret_and_text(&outpost);
    outpost.release_worker();
    let read = reported(&outpost, term);
    assert_eq!(text(&read), after(&["three"]));
    assert_eq!(caret_at(&read), ("one\r\n", 1));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Held, a caret event after a text change is only noted: the read Core
/// asks for in the same batch reads the caret with the text, and the noted
/// caret, which that read saw, reads nothing more: the answer, with the
/// caret moved, and nothing else.
fn a_held_terminals_caret_event_is_read_with_the_text() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-held-caret");
    hold(&mut outpost, term);
    outpost.hold_worker();
    write_text(&mut app, &["three"]);
    app.send("caret term 1");
    app.send("caret-event term");
    heard_caret_and_text(&outpost);
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    outpost.release_worker();
    let answer = read_answer(&outpost, read);
    assert_eq!(text(&answer), after(&["three"]));
    assert_eq!(caret_at(&answer), ("one\r\n", 1));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The console host's text area, in a window of the console's class,
/// takes its caret event from its console: UI Automation's caret event
/// reaches the outpost only as the `WinEvent` UIA raises for MSAA clients,
/// which the outpost drops, as the subscription leaves UIA's out for it,
/// and a description change UIA delivers after it is reported alone; the
/// console's own caret event (`EVENT_CONSOLE_CARET`) reads the terminal,
/// and the read is sent with the caret.
fn the_console_hosts_caret_comes_from_its_console() {
    let (mut app, outpost, text) = start_with(
        "mockapp-console-caret",
        "console_terminal.json",
        "text",
        OutpostOptions::default(),
    );
    app.send("caret text 1");
    app.send("caret-event text");
    app.send("set-description text moved");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::UiaProperty(UIA_HelpTextPropertyId.0),
    ]);
    match outpost.next() {
        OutpostToSupervisor::Event {
            event:
                NormalizedEvent::PropertyChanged {
                    node_id,
                    change: PropertyChange::Description(description),
                    child_count: None,
                },
            ..
        } => assert_eq!((node_id, description.as_deref()), (text, Some("moved"))),
        other => panic!("the outpost said {other:?}, not the text area's description"),
    }
    outpost.settled();
    app.send("console-caret 1 0");
    outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    assert_eq!(caret_at(&reported(&outpost, text)), ("one\r\n", 1));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A console host's caret move with no text change reaches Core: its caret
/// event reads the terminal, as Windows Terminal's caret events do, and the
/// read, its text as before, carries the moved caret, so Core's reading
/// rules judge the next key from it (`docs/design/terminal-model.md`,
/// "Where it runs": one caret).
fn a_console_caret_move_with_no_text_change_is_read() {
    let (mut app, outpost, area) = start_with(
        "mockapp-console-caret-move",
        "console_terminal.json",
        "text",
        OutpostOptions::default(),
    );
    app.send("caret text 24");
    app.send("console-caret 2 1");
    outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    let read = reported(&outpost, area);
    assert_eq!(text(&read), rows(&["one", "two", "ready>"]));
    assert_eq!(caret_at(&read), ("two\r\n", 2));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// In the focused console host's text area the system caret's events tell
/// of the caret its console caret event tells of, at the same tick, and are
/// not used: the console's caret event alone has the caret read.
fn the_console_hosts_system_caret_is_not_its_caret_event() {
    let (mut app, outpost, text) = start_with(
        "mockapp-console-system-caret",
        "console_terminal.json",
        "text",
        OutpostOptions::default(),
    );
    app.send("caret text 1");
    app.send("system-caret");
    outpost.heard(&[Heard::Msaa(WinEventKind::Caret)]);
    outpost.settled();
    app.send("console-caret 1 0");
    outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    assert_eq!(caret_at(&reported(&outpost, text)), ("one\r\n", 1));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A caret key's watch that knows no caret before the key (no `since`,
/// and no press time to find the outpost's own report before it by)
/// takes a caret event as its evidence. One waiting behind a text change in one batch is
/// planned after it, and the change's read reads the caret, covering the
/// event, which then checks nothing: the watch, checked after that read,
/// is answered by it as the event would have answered it. The answer,
/// then the read.
fn a_read_covering_a_caret_event_answers_a_watch_with_no_caret() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-covered-caret");
    let key = ask(
        &mut outpost,
        term,
        TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: None,
            pressed_at_ms: 0,
            unit: TextUnit::Character,
            compare: None,
            deletes: false,
        }),
    );
    // The watch is open: nothing is said.
    outpost.settled();
    outpost.hold_worker();
    app.send("caret term 1");
    app.send("caret-event term");
    write_text(&mut app, &["three"]);
    heard_caret_and_text(&outpost);
    outpost.release_worker();
    let TextReply::Caret(reply) = answer_to(&outpost, key) else {
        panic!("the watch was not answered with the caret");
    };
    assert!(reply.moved);
    assert_eq!(
        (reply.caret.line.text.as_str(), reply.caret.line.offset),
        ("one                 \r\n", 1)
    );
    assert_eq!(reply.unit.expect("the character").text, "n");
    assert_eq!(text(&reported(&outpost, term)), after(&["three"]));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A read identical to the last one sent, its caret included, is not sent
/// again (`verbatim_outpost::terminal::Terminal::sends`): not for a caret
/// event that moved nothing; a read finding the caret moved is sent.
fn an_identical_read_is_not_sent_again() {
    let (mut app, outpost, term) = start("mockapp-terminal-identical-read");
    write(&mut app, &outpost, &["three"]);
    assert_eq!(text(&reported(&outpost, term)), after(&["three"]));
    outpost.settled();
    // Windows Terminal's caret event reads the terminal: from where the
    // last read said to start, the screen's top, which moved down a row.
    caret_event(&mut app, &outpost);
    assert_eq!(
        text(&reported(&outpost, term)),
        rows(&["two", "ready>", "three"])
    );
    outpost.settled();
    // The same read again is not sent.
    caret_event(&mut app, &outpost);
    outpost.settled();
    // Moved, the caret is a change of its own.
    app.send("caret term 1");
    caret_event(&mut app, &outpost);
    let read = reported(&outpost, term);
    assert_eq!(text(&read), rows(&["two", "ready>", "three"]));
    assert_eq!(caret_at(&read), ("one\r\n", 1));
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Raises the terminal's caret event and waits for the outpost to take it
/// in by both routes.
fn caret_event(app: &mut common::MockApp, outpost: &OutpostUnderTest) {
    outpost.hold_worker();
    app.send("caret-event term");
    outpost.heard(&[
        Heard::UiaTextSelection,
        Heard::Msaa(WinEventKind::TextSelectionChange),
    ]);
    outpost.release_worker();
}

/// While the console's window is both the focus window and the
/// foreground window, as the listener routes them for the console host,
/// and is read through UI Automation, UI Automation's `WinEvent` copy of
/// the text area's caret event does not reach the outpost: the hook has
/// left those events out, and the console's caret event, raised after it
/// and always hooked, is the first event heard. Once a Win32 button in the
/// window, read through MSAA, has the focus, the copy is heard again,
/// before the console's caret event.
fn the_hooks_change_events_wait_for_a_window_read_through_msaa() {
    let (mut app, outpost, area) = start_with(
        "mockapp-console-narrowed",
        "console_terminal_button.json",
        "text",
        OutpostOptions::default(),
    );
    let console = common::find_window(&app.title);
    // UI Automation owns the window: neither is reported.
    outpost.deliver(DeliveredFact::Foreground {
        hwnd: console.0 as isize,
    });
    outpost.deliver(DeliveredFact::MsaaFocus {
        hwnd: console.0 as isize,
        id_object: OBJID_CLIENT.0,
        id_child: 0,
    });
    outpost.settled();
    app.send("caret-event text");
    app.send("console-caret 0 0");
    outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    // The console's caret event reads the terminal: the caret has not
    // moved, and the read, from where the first said to start, is sent
    // once.
    assert_eq!(caret_at(&reported(&outpost, area)), ("one\r\n", 0));
    outpost.settled();

    // The window has the focused state, which an MSAA focus in it needs,
    // as when a dialog's button takes the focus.
    app.send("set-focus root");
    // SAFETY: a local search of the console window's children.
    let find = unsafe { FindWindowExW(Some(console), None, w!("Button"), w!("Find")) }
        .expect("mockapp made the button");
    let focus = outpost.focus(DeliveredFact::MsaaFocus {
        hwnd: find.0 as isize,
        id_object: OBJID_CLIENT.0,
        id_child: 0,
    });
    assert_eq!(
        focus.chain(),
        [Some("Mockapp Console Terminal Fixture"), Some("Find")]
    );
    app.send("caret-event text");
    app.send("console-caret 0 0");
    outpost.heard(&[
        Heard::Msaa(WinEventKind::TextSelectionChange),
        Heard::Msaa(WinEventKind::ConsoleCaret),
    ]);
    // The text area is no longer the focus.
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Up Arrow and "x" pressed at once in the console host
/// (`conhost_up_typing`): the shell has drawn the recalled line and put
/// the caret after it, and writes the "x" between the calls of the caret
/// key's read, after its caret and before its line. The line read shows
/// "x" with the caret before it, a state the terminal never had; read again
/// at the read's end, the caret is past the "x", so the read answers
/// nothing. The text change that follows reads again, and the watch is
/// answered with the caret past the "x"; the caret read with the text is
/// that answer's, so it is not reported again, and the output follows.
/// The read is one remote operation.
fn a_watch_read_while_the_terminal_writes_waits_for_its_change() {
    watch_read_while_the_terminal_writes(
        "mockapp-terminal-written-while-read",
        OutpostOptions::default(),
    );
}

/// [`a_watch_read_while_the_terminal_writes_waits_for_its_change`] with
/// remote operations off, so the read is the classic one, its caret and
/// its line separate cross-process calls, and the "x" is written between
/// them as the line's text is read.
fn a_classic_watch_read_while_the_terminal_writes_waits_for_its_change() {
    watch_read_while_the_terminal_writes(
        "mockapp-terminal-written-while-read-classic",
        OutpostOptions {
            remote_operations: false,
        },
    );
}

/// The body of the two tests of a watch read while the terminal writes,
/// the outpost reading as `options` say.
fn watch_read_while_the_terminal_writes(name: &str, options: OutpostOptions) {
    let (mut app, mut outpost, term) = start_with(name, "terminal_reading.json", "term", options);
    // The prompt's trailing space, which the padding hides, and the caret
    // after it: a caret event's read finds the text as it was and the
    // caret moved.
    app.send(&format!(
        "set-text term {}",
        terminal_text(&["one", "two", "ready> "])
    ));
    let row = WIDTH + 2;
    app.send(&format!("caret term {}", 2 * row + 7));
    app.send("caret-event term");
    let caret = reported(&outpost, term).caret.expect("the read's caret");
    let since = TextPosition {
        anchor: caret.line.start,
        offset: caret.line.offset,
    };
    assert_eq!(since.offset, 7);
    outpost.settled();
    app.send(&format!(
        "set-text term {}",
        terminal_text(&["one", "two", "ready> echo one"])
    ));
    app.send(&format!("caret term {}", 2 * row + 15));
    app.send("write-on-read term x");
    let key = ask(
        &mut outpost,
        term,
        TextOp::AwaitCaret(CaretWatch {
            landing: false,
            since: Some(since),
            pressed_at_ms: 0,
            unit: TextUnit::Line,
            compare: None,
            deletes: false,
        }),
    );
    // The read saw the caret move while it read: nothing is said.
    outpost.settled();
    app.send("text-changed term");
    let TextReply::Caret(reply) = answer_to(&outpost, key) else {
        panic!("the watch was not answered with the caret");
    };
    assert!(reply.moved);
    assert_eq!(
        (reply.caret.line.text.as_str(), reply.caret.line.offset),
        ("ready> echo onex    \r\n", 16)
    );
    // The read that answered the watch is sent, the caret past the "x".
    assert_eq!(
        caret_at(&reported(&outpost, term)),
        ("ready> echo onex\r\n", 16)
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Core's read at once before a review command
/// (`TerminalReadNow(Fresh)`) reads whatever the on-demand state, and a
/// held terminal's change it reads is no longer waiting: the read Core asks
/// for after it finds nothing changed.
fn a_fresh_read_is_taken_while_held_and_reads_what_was_held() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-fresh");
    hold(&mut outpost, term);
    write_held(&mut app, &outpost, term, &["nine"], 1);
    let fresh = ask(
        &mut outpost,
        term,
        TextOp::TerminalReadNow(TerminalScope::Fresh),
    );
    assert_eq!(text(&read_answer(&outpost, fresh)), after(&["nine"]));
    outpost.settled();
    let read = ask(&mut outpost, term, TextOp::TerminalRead { hold: false });
    assert_eq!(answer_to(&outpost, read), TextReply::TerminalUnchanged);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// With the screen's shape told (`TerminalShape`), the whole document is
/// read from its start when Core asks, and a history block counted up from
/// the document's end by the height asked for with as many rows above it.
fn the_whole_document_and_a_history_block_are_read_when_asked() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-whole");
    write(&mut app, &outpost, &["three", "four", "five"]);
    let _ = reported(&outpost, term);
    outpost.settled();
    let shape = ask(
        &mut outpost,
        term,
        TextOp::TerminalShape {
            height: 2,
            width: 20,
        },
    );
    assert_eq!(answer_to(&outpost, shape), TextReply::Done);
    let whole = ask(
        &mut outpost,
        term,
        TextOp::TerminalReadNow(TerminalScope::Whole),
    );
    let answer = read_answer(&outpost, whole);
    assert_eq!(text(&answer), after(&["three", "four", "five"]));
    assert!(answer.at_document_start);
    let block = ask(
        &mut outpost,
        term,
        TextOp::TerminalReadNow(TerminalScope::HistoryBlock { height: 2 }),
    );
    let answer = read_answer(&outpost, block);
    assert_eq!(text(&answer), rows(&["ready>", "three", "four", "five"]));
    assert_eq!(answer.above, 2);
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Rows of the history above where the next read starts are read by place
/// when Core asks (`TerminalHistory`), padding kept, with how many rows
/// above that start the read holds.
fn history_above_the_next_reads_start_is_read_by_place() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-history");
    write(&mut app, &outpost, &["three", "four", "five"]);
    let _ = reported(&outpost, term);
    outpost.settled();
    let history = ask(
        &mut outpost,
        term,
        TextOp::TerminalHistory {
            above: 2,
            whole: false,
        },
    );
    assert_eq!(
        answer_to(&outpost, history),
        TextReply::TerminalHistory {
            text: [
                "two                 \r\n",
                "ready>              \r\n",
                "three               \r\n",
                "four                \r\n",
                "five                \r\n",
            ]
            .concat(),
            above: 2,
        }
    );
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// A key's colours request (`TerminalColours`) when a read began after the
/// key without the colours is answered at once with a read that has them;
/// one for a key after the last read is answered done, and the next read
/// takes them.
fn a_colours_request_after_a_read_without_them_reads_at_once() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-colours");
    let colours = ask(&mut outpost, term, TextOp::TerminalColours { since_ms: 0 });
    let answer = read_answer(&outpost, colours);
    assert!(answer.colours.is_some());
    outpost.settled();
    let later = verbatim_outpost::protocol::now_us() / 1_000 + 1;
    let colours = ask(
        &mut outpost,
        term,
        TextOp::TerminalColours { since_ms: later },
    );
    assert_eq!(answer_to(&outpost, colours), TextReply::Done);
    write(&mut app, &outpost, &["three"]);
    let read = reported(&outpost, term);
    assert_eq!(text(&read), after(&["three"]));
    assert!(read.colours.is_some());
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// The read before a held key (`ReadCaret`) in a terminal is a read of it,
/// whose caret answers the request.
fn the_read_before_a_held_key_is_a_read_with_its_caret() {
    let (mut app, mut outpost, term) = start("mockapp-terminal-before-key");
    app.send("caret term 1");
    let before = ask(&mut outpost, term, TextOp::ReadCaret);
    let answer = read_answer(&outpost, before);
    assert_eq!(caret_at(&answer), ("one\r\n", 1));
    assert!(answer.caret.is_some());
    outpost.settled();
    drop(outpost);
    app.quit();
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_fresh_read_is_taken_while_held_and_reads_what_was_held",
            a_fresh_read_is_taken_while_held_and_reads_what_was_held,
        ),
        (
            "the_whole_document_and_a_history_block_are_read_when_asked",
            the_whole_document_and_a_history_block_are_read_when_asked,
        ),
        (
            "history_above_the_next_reads_start_is_read_by_place",
            history_above_the_next_reads_start_is_read_by_place,
        ),
        (
            "a_colours_request_after_a_read_without_them_reads_at_once",
            a_colours_request_after_a_read_without_them_reads_at_once,
        ),
        (
            "the_read_before_a_held_key_is_a_read_with_its_caret",
            the_read_before_a_held_key_is_a_read_with_its_caret,
        ),
        (
            "a_focus_now_terminal_reports_its_output",
            a_focus_now_terminal_reports_its_output,
        ),
        (
            "a_terminal_stays_followed_through_a_foreground_report",
            a_terminal_stays_followed_through_a_foreground_report,
        ),
        (
            "a_held_terminal_notes_changes_and_a_read_answers_them",
            a_held_terminal_notes_changes_and_a_read_answers_them,
        ),
        (
            "a_read_with_nothing_new_answers_empty_and_may_hold_again",
            a_read_with_nothing_new_answers_empty_and_may_hold_again,
        ),
        (
            "a_cancel_answers_what_is_new_and_reading_is_live_again",
            a_cancel_answers_what_is_new_and_reading_is_live_again,
        ),
        (
            "a_caret_event_ahead_of_a_text_change_is_read_with_it",
            a_caret_event_ahead_of_a_text_change_is_read_with_it,
        ),
        (
            "a_held_terminals_caret_event_is_read_with_the_text",
            a_held_terminals_caret_event_is_read_with_the_text,
        ),
        (
            "the_console_hosts_caret_comes_from_its_console",
            the_console_hosts_caret_comes_from_its_console,
        ),
        (
            "a_console_caret_move_with_no_text_change_is_read",
            a_console_caret_move_with_no_text_change_is_read,
        ),
        (
            "the_console_hosts_system_caret_is_not_its_caret_event",
            the_console_hosts_system_caret_is_not_its_caret_event,
        ),
        (
            "a_read_covering_a_caret_event_answers_a_watch_with_no_caret",
            a_read_covering_a_caret_event_answers_a_watch_with_no_caret,
        ),
        (
            "a_watch_read_while_the_terminal_writes_waits_for_its_change",
            a_watch_read_while_the_terminal_writes_waits_for_its_change,
        ),
        (
            "a_classic_watch_read_while_the_terminal_writes_waits_for_its_change",
            a_classic_watch_read_while_the_terminal_writes_waits_for_its_change,
        ),
        (
            "an_identical_read_is_not_sent_again",
            an_identical_read_is_not_sent_again,
        ),
        (
            "the_hooks_change_events_wait_for_a_window_read_through_msaa",
            the_hooks_change_events_wait_for_a_window_read_through_msaa,
        ),
    ]);
}
