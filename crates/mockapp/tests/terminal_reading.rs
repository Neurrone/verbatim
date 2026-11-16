//! A focused terminal's reading through a real outpost's worker
//! (`docs/design/readers-tell-core-asks-build3.md`), against mockapp's UIA
//! text provider presenting a Windows Terminal control (UIA class
//! `TermControl`) or a console host's text area, its text given as a
//! terminal gives it, every row padded to the width and ended by a carriage
//! return and line feed. Core's requests reach the outpost as Core sends
//! them, through the request driver the integration harness shares with
//! Core's own (`common::core_driver`).
//!
//! - Every change signal of the terminal `Follow` names is told to Core as
//!   one "terminal changed" tell, with no call: Windows Terminal's text
//!   change and caret event, and a console host's console caret event,
//!   which the outpost hooks once `Follow` names a console's terminal;
//!   UI Automation's caret event and the system caret's are not used for
//!   a console's terminal.
//! - Core's "read the terminal" request reads from a screen above the
//!   start anchor Core names and mints the start of its own screen; a
//!   history read and a search count from the start Core names; the read
//!   takes the colours only when Core asks; the whole document and a
//!   history block are read when asked; a caret key's starting read mints
//!   the key's caret starting point with the caret.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_ia2::WinEventKind;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyId;
use verbatim_model::request::{Answer, Follow, FollowGroup, Followed, Outcome, Reply, Request};
use verbatim_model::tell::{EventObject, Payload};
use verbatim_model::{
    CallCounts, NodeId, TerminalRead, TerminalReadReply, TerminalScope, TerminalSnapshot, TextAnchor, TextOp,
    TextReply, TextUnit,
};
use verbatim_outpost::Heard;
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::{Cached, ListenerEvent, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::UIA_HelpTextPropertyId;

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

/// mockapp's terminal, an outpost watching it, the terminal's node, and
/// the start anchor of its baseline read.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    outpost: OutpostUnderTest,
    node: NodeId,
    /// Whether its window is a console host's.
    console: bool,
    /// The start the baseline minted.
    start: TextAnchor,
}

impl Terminal {
    /// A full read Core asks from `start` in `scope`, the screen's shape
    /// `(height, width)` from Core's model, taking the colours when
    /// `colours`.
    fn ask(
        &self,
        start: Option<TextAnchor>,
        scope: TerminalScope,
        (height, width): (u32, u32),
        colours: bool,
    ) -> TerminalRead {
        TerminalRead {
            start,
            scope,
            height,
            width,
            colours,
            console: self.console,
            starting: None,
            output: true,
        }
    }

    /// The answer to `read`, a full read, which must be a read.
    fn read(&self, read: TerminalRead) -> TerminalReadReply {
        self.read_with_calls(read).0
    }

    /// [`Self::read`], with the calls it made into mockapp.
    fn read_with_calls(&self, read: TerminalRead) -> (TerminalReadReply, CallCounts) {
        let number = self.outpost.request(
            self.hwnd,
            Request::Text {
                node: self.node,
                op: TextOp::ReadTerminal(read),
            },
        );
        match self.outpost.answer_with_calls(number) {
            (
                Answer {
                    outcome: Outcome::Result(Reply::Text(TextReply::TerminalRead(reply))),
                    ..
                },
                calls,
            ) => (*reply, calls),
            (other, _) => panic!("the outpost answered {other:?}, not a read"),
        }
    }

    /// A fresh read from the start Core names, the screen's shape not yet
    /// known to Core.
    fn fresh_from(&self, start: TextAnchor) -> TerminalReadReply {
        self.read(self.ask(Some(start), TerminalScope::Fresh, (0, 0), false))
    }

    /// The text reply to `op` on the terminal, asked as Core asks it.
    fn text(&self, op: TextOp) -> TextReply {
        let number = self.outpost.request(
            self.hwnd,
            Request::Text {
                node: self.node,
                op,
            },
        );
        match self.outpost.answer(number).outcome {
            Outcome::Result(Reply::Text(reply)) => reply,
            other => panic!("the outpost answered {other:?}, not a text reply"),
        }
    }

    /// The tell the outpost sends next, which must be a change tell of the
    /// terminal.
    fn told(&self) {
        match self.outpost.next() {
            verbatim_model::protocol::OutpostToCore::Tell(tell) => {
                assert_eq!(tell.kind, EventKind::TerminalChanged);
                match tell.object {
                    EventObject::Terminal { node, .. } => assert_eq!(node, self.node),
                    other => panic!("the change tell named {other:?}, not the terminal"),
                }
            }
            other => panic!("the outpost said {other:?}, not the terminal's change"),
        }
    }

    fn quit(self) {
        let Self { app, outpost, .. } = self;
        drop(outpost);
        app.quit();
    }
}

/// mockapp's Windows Terminal fixture in its UIA backend, with an outpost
/// watching it, its terminal completed as Core's focus, followed and read
/// for its baseline.
fn start(name: &str) -> Terminal {
    start_with(name, "terminal_reading.json", "term", false)
}

/// [`start`] for the terminal `id` of fixture `fixture`, a console host's
/// text area when `console`.
fn start_with(name: &str, fixture: &str, id: &str, console: bool) -> Terminal {
    common::init_com();
    let title = common::unique_title(name);
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let outpost = OutpostUnderTest::with_options(&app, OutpostOptions::default());
    // The screen: the text's last three rows, and the empty last line.
    app.send(&format!("screen {id} 4"));
    app.send(&format!("set-focus {id}"));
    let node = outpost.uia_focus_answer(&focused(hwnd)).node();
    let follow = outpost.request(
        hwnd,
        Request::Follow(Follow {
            node: Some(Followed {
                node,
                group: FollowGroup::Terminal { console },
                extra: None,
            }),
        }),
    );
    assert_eq!(
        outpost.answer(follow).outcome,
        Outcome::Result(Reply::Followed)
    );
    let mut terminal = Terminal {
        app,
        hwnd,
        outpost,
        node,
        console,
        start: TextAnchor(0),
    };
    // The baseline: a first read, taking the screen alone.
    let baseline = terminal.read(TerminalRead {
        output: false,
        ..terminal.ask(None, TerminalScope::Fresh, (0, 0), false)
    });
    assert!(baseline.snapshot.caret.is_some());
    terminal.start = baseline.start;
    terminal.outpost.settled();
    terminal
}

/// Writes `lines` after [`START`] as the terminal's whole text and raises
/// its text changed event.
fn write_text(app: &mut common::MockApp, lines: &[&str]) {
    let rows: Vec<&str> = START.iter().chain(lines).copied().collect();
    app.send(&format!("set-text term {}", terminal_text(&rows)));
    app.send("text-changed term");
}

/// Writes `lines` as [`write_text`] does and takes the change tell the
/// outpost sends for it.
fn write(terminal: &mut Terminal, lines: &[&str]) {
    write_text(&mut terminal.app, lines);
    terminal.outpost.heard(&[Heard::UiaTextChanged]);
    terminal.told();
    terminal.outpost.settled();
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

/// The followed terminal's text change is told to Core as one change tell,
/// with no read; Core's read from the baseline's start then shows the new
/// output with the caret read with it. Converted from
/// `a_focus_now_terminal_reports_its_output` and
/// `a_held_terminal_notes_changes_and_a_read_answers_them`: the outpost
/// reads only when Core asks (build 3, "The outpost: change tells").
fn a_text_change_is_told_and_core_reads_it_from_its_start() {
    let mut terminal = start("mockapp-terminal-told");
    write(&mut terminal, &["three"]);
    let read = terminal.fresh_from(terminal.start);
    assert_eq!(text(&read.snapshot), after(&["three"]));
    assert!(read.snapshot.caret.is_some());
    terminal.outpost.settled();
    terminal.quit();
}

/// A caret event of the followed terminal is told, as its text change is,
/// and the tells of one terminal are each told: the caret event waiting
/// ahead of a text change is no longer planned after it, since Core's
/// coverage rule reads once for both. Converted from
/// `a_caret_event_ahead_of_a_text_change_is_read_with_it` (build 3, "Core's
/// inbox: the change tell").
fn a_caret_event_is_told_as_a_text_change_is() {
    let mut terminal = start("mockapp-terminal-caret-told");
    terminal.app.send("caret term 1");
    terminal.app.send("caret-event term");
    terminal.outpost.heard(&[Heard::UiaTextSelection]);
    terminal.told();
    terminal.outpost.settled();
    write(&mut terminal, &["three"]);
    let read = terminal.fresh_from(terminal.start);
    assert_eq!(caret_at(&read.snapshot), ("one\r\n", 1));
    terminal.quit();
}

/// Core's read from a start it names reads a screen above that start, and
/// mints the start of its own screen, which the next read names: two reads
/// from the same start read the same text, and a read from the newer
/// start begins at the newer screen. Converted from
/// `a_fresh_read_is_taken_while_held_and_reads_what_was_held` (M6.4's
/// first item: every read names its start).
fn a_read_counts_from_the_start_core_names() {
    let mut terminal = start("mockapp-terminal-named-start");
    write(&mut terminal, &["nine"]);
    let first = terminal.fresh_from(terminal.start);
    assert_eq!(text(&first.snapshot), after(&["nine"]));
    let again = terminal.fresh_from(terminal.start);
    assert_eq!(text(&again.snapshot), after(&["nine"]));
    write(&mut terminal, &["nine", "ten", "eleven"]);
    let newer = terminal.fresh_from(first.start);
    assert_eq!(
        text(&newer.snapshot),
        rows(&["two", "ready>", "nine", "ten", "eleven"])
    );
    terminal.outpost.settled();
    terminal.quit();
}

/// The whole document and a history block are read when Core asks for
/// them, in place of a fresh read.
fn the_whole_document_and_a_history_block_are_read_when_asked() {
    let mut terminal = start("mockapp-terminal-whole");
    write(&mut terminal, &["three", "four", "five"]);
    let whole =
        terminal.read(terminal.ask(Some(terminal.start), TerminalScope::Whole, (2, 20), false));
    assert_eq!(text(&whole.snapshot), after(&["three", "four", "five"]));
    assert!(whole.snapshot.at_document_start);
    let block = terminal.read(terminal.ask(
        Some(terminal.start),
        TerminalScope::HistoryBlock { height: 2 },
        (2, 20),
        false,
    ));
    assert_eq!(
        text(&block.snapshot),
        rows(&["ready>", "three", "four", "five"])
    );
    assert_eq!(block.snapshot.above, 2);
    terminal.outpost.settled();
    terminal.quit();
}

/// The history above the start Core names is read by place, counted from
/// that start.
fn history_above_the_start_core_names_is_read_by_place() {
    let mut terminal = start("mockapp-terminal-history");
    write(&mut terminal, &["three", "four", "five"]);
    let read = terminal.fresh_from(terminal.start);
    let history = terminal.text(TextOp::TerminalHistory {
        above: 2,
        whole: false,
        start: read.start,
        height: 0,
    });
    assert_eq!(
        history,
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
            read: 0,
        }
    );
    terminal.outpost.settled();
    terminal.quit();
}

/// A read takes the screen's colours when Core asks, and only then.
/// Converted from `a_colours_request_after_a_read_without_them_reads_at_once`:
/// whether a read takes the colours is Core's decision, sent with the read
/// (build 3, "Colours").
fn a_read_takes_the_colours_when_core_asks() {
    let mut terminal = start("mockapp-terminal-colours");
    let without = terminal.fresh_from(terminal.start);
    assert!(without.snapshot.colours.is_none());
    write(&mut terminal, &["three"]);
    let read =
        terminal.read(terminal.ask(Some(terminal.start), TerminalScope::Fresh, (0, 20), true));
    assert_eq!(text(&read.snapshot), after(&["three"]));
    assert!(read.snapshot.colours.is_some());
    terminal.outpost.settled();
    terminal.quit();
}

/// A caret key's starting read in a terminal is a full read with its
/// caret, and mints the key's caret starting point from the caret the read
/// found, so it is one round trip, the remote program's one call, as a
/// fresh read is. Converted from
/// `the_read_before_a_held_key_is_a_read_with_its_caret` (build 1, "A
/// terminal's caret key").
fn a_starting_read_reads_the_caret_and_mints_the_starting_point() {
    let mut terminal = start("mockapp-terminal-starting-read");
    terminal.app.send("caret term 1");
    let (read, calls) = terminal.read_with_calls(TerminalRead {
        starting: Some(TextUnit::Character),
        output: false,
        ..terminal.ask(Some(terminal.start), TerminalScope::Fresh, (0, 0), false)
    });
    assert_eq!(caret_at(&read.snapshot), ("one\r\n", 1));
    assert!(read.snapshot.caret.is_some());
    assert!(read.point.is_some());
    assert_eq!(
        calls,
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        },
        "one program, one call"
    );
    terminal.outpost.settled();
    terminal.quit();
}

/// [`start_with`] for mockapp's console host fixture: its text area in a
/// window of the console's class, which `Follow` names as a console's
/// terminal, so the outpost hooks the console's events.
fn start_console(name: &str) -> Terminal {
    start_with(name, "console_terminal.json", "text", true)
}

/// The console host's text area takes its caret event from its console:
/// UI Automation's caret event is not registered for a console's terminal
/// and no `WinEvent` but the console's is hooked, so it is not heard, and a
/// description change UI Automation delivers after it is told alone, its
/// description left for Core to read as NVDA reads it; the
/// console's own caret event (`EVENT_CONSOLE_CARET`) is told as the
/// terminal's change, and Core's read then finds the caret moved.
/// Converted from `the_console_hosts_caret_comes_from_its_console`: the
/// outpost tells the change and Core reads it (build 3, "The outpost:
/// change tells").
fn the_console_hosts_caret_comes_from_its_console() {
    let mut terminal = start_console("mockapp-console-caret");
    terminal.app.send("caret text 1");
    terminal.app.send("caret-event text");
    terminal.app.send("set-description text moved");
    terminal
        .outpost
        .heard(&[Heard::UiaProperty(UIA_HelpTextPropertyId.0)]);
    match terminal.outpost.next() {
        verbatim_model::protocol::OutpostToCore::Tell(tell) => assert_eq!(
            (tell.kind, tell.payload),
            (
                EventKind::UiaPropertyChanged,
                Payload::PropertyChanged {
                    property: PropertyId::Description,
                    value: None,
                }
            )
        ),
        other => panic!("the outpost said {other:?}, not the text area's description"),
    }
    terminal.outpost.settled();
    terminal.app.send("console-caret 1 0");
    terminal.outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.fresh_from(terminal.start);
    assert_eq!(caret_at(&read.snapshot), ("one\r\n", 1));
    terminal.outpost.settled();
    terminal.quit();
}

/// A console host's caret move with no text change reaches Core: its caret
/// event is told as the terminal's change, as Windows Terminal's caret
/// events are, and Core's read, its text as before, carries the moved
/// caret, so Core's reading rules judge the next key from it
/// (`books/dev/src/explanation/design/terminal-model.md`, "Where it runs":
/// one caret). Converted from
/// `a_console_caret_move_with_no_text_change_is_read`: the outpost tells,
/// Core reads.
fn a_console_caret_move_with_no_text_change_is_told() {
    let mut terminal = start_console("mockapp-console-caret-move");
    terminal.app.send("caret text 24");
    terminal.app.send("console-caret 2 1");
    terminal.outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.fresh_from(terminal.start);
    assert_eq!(text(&read.snapshot), rows(&["one", "two", "ready>"]));
    assert_eq!(caret_at(&read.snapshot), ("two\r\n", 2));
    terminal.outpost.settled();
    terminal.quit();
}

/// In the focused console host's text area the system caret's events tell
/// of the caret its console caret event tells of, at the same tick, and are
/// not used: the outpost does not hook them, so the console's caret event,
/// raised after, is the first event heard, and the one change told.
/// Converted from `the_console_hosts_system_caret_is_not_its_caret_event`.
fn the_console_hosts_system_caret_is_not_its_caret_event() {
    let mut terminal = start_console("mockapp-console-system-caret");
    terminal.app.send("caret text 1");
    terminal.app.send("system-caret");
    terminal.app.send("console-caret 1 0");
    terminal.outpost.heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.fresh_from(terminal.start);
    assert_eq!(caret_at(&read.snapshot), ("one\r\n", 1));
    terminal.outpost.settled();
    terminal.quit();
}

fn main() {
    harness::run_isolated(&[
        (
            "a_text_change_is_told_and_core_reads_it_from_its_start",
            a_text_change_is_told_and_core_reads_it_from_its_start,
        ),
        (
            "a_caret_event_is_told_as_a_text_change_is",
            a_caret_event_is_told_as_a_text_change_is,
        ),
        (
            "a_read_counts_from_the_start_core_names",
            a_read_counts_from_the_start_core_names,
        ),
        (
            "the_whole_document_and_a_history_block_are_read_when_asked",
            the_whole_document_and_a_history_block_are_read_when_asked,
        ),
        (
            "history_above_the_start_core_names_is_read_by_place",
            history_above_the_start_core_names_is_read_by_place,
        ),
        (
            "a_read_takes_the_colours_when_core_asks",
            a_read_takes_the_colours_when_core_asks,
        ),
        (
            "a_starting_read_reads_the_caret_and_mints_the_starting_point",
            a_starting_read_reads_the_caret_and_mints_the_starting_point,
        ),
        (
            "the_console_hosts_caret_comes_from_its_console",
            the_console_hosts_caret_comes_from_its_console,
        ),
        (
            "a_console_caret_move_with_no_text_change_is_told",
            a_console_caret_move_with_no_text_change_is_told,
        ),
        (
            "the_console_hosts_system_caret_is_not_its_caret_event",
            the_console_hosts_system_caret_is_not_its_caret_event,
        ),
    ]);
}
