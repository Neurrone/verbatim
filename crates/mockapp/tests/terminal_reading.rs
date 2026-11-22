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
//! - Core's window read, the request list's generic steps
//!   (`verbatim_core::terminal_window`), shows what was told: the text
//!   written and the caret moved; it takes the colours only when Core
//!   asks; and a caret key's starting read holds the key's caret starting
//!   point with the caret, in one remote program.
//! - A terminal caret key's watch has no event shortcut: a caret event, the
//!   console's or Windows Terminal's, is told and leaves the watch polling
//!   to its bound, and a caret moved with no event is found by a check; it
//!   answers with nothing read, Core's full read following.
//!
//! Each test runs on a desktop of its own (`harness::run_isolated`), so no
//! other client sees mockapp's window or raises its events.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_core::batch::Builder;
use verbatim_core::terminal_window::{TerminalWindow, WindowAsk, add_steps, window_of};
use verbatim_ia2::WinEventKind;
use verbatim_model::batch::Waited;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyId;
use verbatim_model::request::{FollowGroup, Followed, Outcome, Reply};
use verbatim_model::tell::{EventObject, Payload};
use verbatim_model::{CallCounts, CaretReport, NodeId, TerminalText, TextAnchor};
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

/// mockapp's terminal, an outpost watching it, and the terminal's node.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    outpost: OutpostUnderTest,
    node: NodeId,
    /// Whether its window is a console host's.
    console: bool,
}

/// The brand of the window read's batches.
enum WindowRead {}

impl Terminal {
    /// Core's window read of the terminal, as Core asks it with no mark,
    /// four rows high, taking the screen's colours when `colours`: the
    /// window Core makes of the answer, and the calls the read made into
    /// mockapp.
    fn read_with_calls(&self, colours: bool) -> (TerminalWindow, CallCounts) {
        let ask = WindowAsk {
            end: None,
            aside: None,
            height: 4,
            opening: 2,
            colours,
        };
        let mut steps = Builder::<WindowRead>::named();
        add_steps(&mut steps, self.node, &ask);
        let number = self.outpost.request(self.hwnd, steps.finish().question());
        let (answer, calls) = self.outpost.answer_with_calls(number);
        let Outcome::Result(Reply::Steps(outcomes)) = answer.outcome else {
            panic!(
                "the outpost answered {:?}, not the read's steps",
                answer.outcome
            )
        };
        let window = window_of(&outcomes, &ask, 0)
            .unwrap_or_else(|unread| panic!("the window, not {unread:?} of {outcomes:#?}"));
        (window, calls)
    }

    /// Core's window read of the terminal ([`Self::read_with_calls`]),
    /// with no colours.
    fn read(&self) -> TerminalWindow {
        self.read_with_calls(false).0
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
        common::outpost::follow(Some(Followed {
            node,
            group: FollowGroup::Terminal { console },
            extra: None,
        })),
    );
    assert_eq!(
        common::outpost::only_step(outpost.answer(follow).outcome),
        verbatim_model::batch::StepOutcome::Done(verbatim_model::batch::Done::Follow)
    );
    let terminal = Terminal {
        app,
        hwnd,
        outpost,
        node,
        console,
    };
    // The baseline, its caret read with it.
    assert!(terminal.read().caret.is_some());
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
fn text(window: &TerminalWindow) -> &str {
    &window.rows.text
}

/// The caret's line a read found, without its padding, and the caret's
/// place in it.
fn caret_at(window: &TerminalWindow) -> (String, u32) {
    let line = &window.caret.as_ref().expect("a caret").report.line;
    (TerminalText::unpadded(&line.text).text, line.offset)
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
    let read = terminal.read();
    assert_eq!(text(&read), after(&["three"]));
    assert!(read.caret.is_some());
    terminal.outpost.settled();
    terminal.quit();
}

/// A focus repeated on the followed Windows Terminal: its completion keeps
/// the group Core's `Follow` named, with text changes, since Core alone
/// decides the group (`docs/parity/backends.md`, "Which focus takes
/// text-change events"), so the output written after it is still told.
fn a_completion_on_the_followed_terminal_keeps_its_text_changes() {
    let mut terminal = start("mockapp-terminal-completion-keeps");
    terminal.app.send("set-focus term");
    let again = terminal.outpost.uia_focus_answer(&focused(terminal.hwnd));
    assert_eq!(again.node(), terminal.node);
    terminal.outpost.settled();
    write(&mut terminal, &["three"]);
    let read = terminal.read();
    assert_eq!(text(&read), after(&["three"]));
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
    let read = terminal.read();
    assert_eq!(caret_at(&read), ("one\r\n".to_owned(), 1));
    terminal.quit();
}

/// A read takes the screen's colours when Core asks, and only then.
/// Converted from `a_colours_request_after_a_read_without_them_reads_at_once`:
/// whether a read takes the colours is Core's decision, sent with the read
/// (build 3, "Colours").
fn a_read_takes_the_colours_when_core_asks() {
    let mut terminal = start("mockapp-terminal-colours");
    let without = terminal.read();
    assert!(without.colours.is_none());
    write(&mut terminal, &["three"]);
    let (read, _) = terminal.read_with_calls(true);
    assert_eq!(text(&read), after(&["three"]));
    assert!(read.colours.is_some());
    terminal.outpost.settled();
    terminal.quit();
}

/// A read's colours are the screen's rows at its width as the same read
/// found them, so the first read after the screen grew takes every row of
/// the new screen: three rows of 20 cells on a screen showing three, then
/// five once it shows five.
fn the_colours_are_the_screen_this_read_found() {
    let mut terminal = start("mockapp-terminal-colours-resize");
    let cells = |read: &TerminalWindow| -> Vec<u32> {
        read.colours
            .as_ref()
            .expect("the colours")
            .iter()
            .map(|runs| runs.iter().map(|run| run.length).sum())
            .collect()
    };
    let (read, _) = terminal.read_with_calls(true);
    assert_eq!(cells(&read), [20; 3]);
    terminal.app.send("screen term 6");
    write(&mut terminal, &["four", "five"]);
    let (read, _) = terminal.read_with_calls(true);
    assert_eq!(cells(&read), [20; 5]);
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
    let (read, calls) = terminal.read_with_calls(false);
    assert_eq!(caret_at(&read), ("one\r\n".to_owned(), 1));
    assert!(read.caret.is_some());
    assert_eq!(
        calls,
        CallCounts {
            uia: 1,
            msaa: 0,
            window_messages: 1,
            connections: 0,
        },
        "the window's probe, which a probe-first request always makes, and one program's one call"
    );
    terminal.outpost.settled();
    terminal.quit();
}

/// A caret key's starting read of the terminal, the caret where mockapp
/// has it: the caret starting point it mints.
fn starting_point(terminal: &Terminal) -> (TextAnchor, CaretReport) {
    let caret = terminal
        .read()
        .caret
        .expect("a starting read holds the caret it read");
    (caret.point, caret.report)
}

/// Core's watch of a terminal's caret key from `point`, passed now, as
/// Core sends it for the terminal: no event is the key's effect, the
/// checks are every 5 ms to NVDA's 100 ms bound in the console host and
/// every 10 ms to 300 ms in Windows Terminal, and no unit is read at its
/// end (`books/dev/src/reference/crates/verbatim-core.md`, "Terminals"). Returns the request's
/// number.
fn watch(
    terminal: &Terminal,
    (point, caret): (TextAnchor, CaretReport),
) -> verbatim_model::request::RequestNumber {
    use verbatim_model::batch::{Batch, CaretWait, EffectTest, Op, Placement, Step, Subject};
    let (poll_us, bound_us) = if terminal.console {
        (5_000, 100_000)
    } else {
        (10_000, 300_000)
    };
    terminal.outpost.request(
        terminal.hwnd,
        verbatim_model::request::Question::Batch(Batch {
            steps: vec![Step {
                op: Op::WaitForCaret {
                    subject: Subject::Node(terminal.node),
                    wait: Box::new(CaretWait {
                        from: point,
                        line: caret.line.text,
                        offset: caret.line.offset,
                        word: None,
                        effect: EffectTest {
                            unit: false,
                            line: false,
                        },
                        passed_at_us: common::outpost::now_us(),
                        poll_us,
                        bound_us,
                        events: false,
                    }),
                },
                when: None,
            }],
            placement: Placement::Queue,
            spare: None,
        }),
    )
}

/// How the wait `number` ended, which must be the outpost's next message.
fn watched(terminal: &Terminal, number: verbatim_model::request::RequestNumber) -> Waited {
    use verbatim_model::batch::{Done, StepOutcome};
    match terminal.outpost.answer(number).outcome {
        Outcome::Result(Reply::Steps(steps)) => match steps[..] {
            [StepOutcome::Done(Done::WaitForCaret(waited))] => waited,
            _ => panic!("the wait's outcome, not {steps:?}"),
        },
        other => panic!("the outpost answered {other:?}, not the wait"),
    }
}

/// A terminal caret key's watch finds a caret the program moved with no
/// event by its check, and answers with no unit read, Core's full read of
/// the terminal following. Converted from
/// `a_read_terminal_message_reads_the_caret_first_under_the_keys_trace`:
/// the key's effect is the watch's, its read Core's (build 1, "A
/// terminal's caret key").
fn a_terminal_watch_finds_a_caret_moved_with_no_event() {
    let mut terminal = start("mockapp-terminal-watch-moved");
    let point = starting_point(&terminal);
    terminal.app.send("caret term 1");
    let number = watch(&terminal, point);
    assert_eq!(watched(&terminal, number), Waited::Effect);
    terminal.outpost.settled();
    terminal.quit();
}

/// Windows Terminal's caret event during a terminal key's watch, with the
/// caret where the key found it, is told to Core as the terminal's change
/// and is not the key's effect: a terminal's watch has no event shortcut,
/// so it runs to its bound and answers with nothing read. Converted from
/// `a_read_covering_a_caret_event_answers_a_watch_with_no_caret`: an event
/// no longer answers a terminal's watch (build 1, "A terminal's caret
/// key").
fn a_caret_event_is_not_a_terminal_watchs_effect() {
    let mut terminal = start("mockapp-terminal-watch-event");
    let point = starting_point(&terminal);
    let number = watch(&terminal, point);
    terminal.app.send("caret-event term");
    terminal.outpost.heard(&[Heard::UiaTextSelection]);
    terminal.told();
    assert_eq!(watched(&terminal, number), Waited::Bound);
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
    terminal
        .outpost
        .heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.read();
    assert_eq!(caret_at(&read), ("one\r\n".to_owned(), 1));
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
    terminal
        .outpost
        .heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.read();
    assert_eq!(text(&read), rows(&["one", "two", "ready>"]));
    assert_eq!(caret_at(&read), ("two\r\n".to_owned(), 2));
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
    terminal
        .outpost
        .heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    terminal.outpost.settled();
    let read = terminal.read();
    assert_eq!(caret_at(&read), ("one\r\n".to_owned(), 1));
    terminal.outpost.settled();
    terminal.quit();
}

/// The console's caret event during a caret key's watch, with no move, is
/// told as the terminal's change and leaves the watch polling, every 5 ms
/// in the console host: it runs to its 100 ms bound and answers with
/// nothing read. Converted from
/// `a_console_caret_event_without_a_move_leaves_the_poll_reading`: the
/// console poll goes into Core's watch (build 3's appendix).
fn a_console_caret_event_without_a_move_leaves_the_watch_polling() {
    let mut terminal = start_console("mockapp-console-watch-event");
    let point = starting_point(&terminal);
    let number = watch(&terminal, point);
    terminal.app.send("console-caret 0 0");
    terminal
        .outpost
        .heard(&[Heard::Msaa(WinEventKind::ConsoleCaret)]);
    terminal.told();
    assert_eq!(watched(&terminal, number), Waited::Bound);
    terminal.outpost.settled();
    terminal.quit();
}

/// The console host's caret moved with no event is the key's effect, found
/// by the watch's check, which answers with nothing read.
fn a_console_watch_finds_a_caret_moved_with_no_event() {
    let mut terminal = start_console("mockapp-console-watch-moved");
    let point = starting_point(&terminal);
    terminal.app.send("caret text 1");
    let number = watch(&terminal, point);
    assert_eq!(watched(&terminal, number), Waited::Effect);
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
            "a_completion_on_the_followed_terminal_keeps_its_text_changes",
            a_completion_on_the_followed_terminal_keeps_its_text_changes,
        ),
        (
            "a_caret_event_is_told_as_a_text_change_is",
            a_caret_event_is_told_as_a_text_change_is,
        ),
        (
            "a_read_takes_the_colours_when_core_asks",
            a_read_takes_the_colours_when_core_asks,
        ),
        (
            "the_colours_are_the_screen_this_read_found",
            the_colours_are_the_screen_this_read_found,
        ),
        (
            "a_starting_read_reads_the_caret_and_mints_the_starting_point",
            a_starting_read_reads_the_caret_and_mints_the_starting_point,
        ),
        (
            "a_terminal_watch_finds_a_caret_moved_with_no_event",
            a_terminal_watch_finds_a_caret_moved_with_no_event,
        ),
        (
            "a_caret_event_is_not_a_terminal_watchs_effect",
            a_caret_event_is_not_a_terminal_watchs_effect,
        ),
        (
            "a_console_caret_event_without_a_move_leaves_the_watch_polling",
            a_console_caret_event_without_a_move_leaves_the_watch_polling,
        ),
        (
            "a_console_watch_finds_a_caret_moved_with_no_event",
            a_console_watch_finds_a_caret_moved_with_no_event,
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
