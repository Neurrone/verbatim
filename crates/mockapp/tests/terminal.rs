//! A terminal's reading (`books/dev/src/explanation/design/terminal-model.md`, "The rules")
//! against mockapp's UIA text provider, whose `set-text` command rewrites a
//! node's text the way a terminal's buffer changes, here as a terminal
//! gives it, every row padded to the width and ended by a carriage return
//! and line feed, and whose `screen` command puts only the text's last
//! rows on screen, as a terminal shows the end of its buffer.
//!
//! The reading tests drive Core's reducer and a real outpost watching
//! mockapp's Windows Terminal control (UIA class `TermControl`), through
//! the request driver (`common::core_driver`): the terminal is focused and
//! followed, each text change mockapp raises is told to Core, and Core
//! reads the terminal when it chooses
//! (`docs/design/readers-tell-core-asks-build3.md`), so a test asserts
//! what Core says. The search tests drive `verbatim-outpost`'s terminal
//! module on this thread, as the outpost's worker does.
//!
//! What is checked: Core says the new output a read shows, line by line,
//! and the read costs one call remotely; a terminal's typing is echoed
//! character by character and Backspace says what it deleted, the keys
//! reaching the rules through Core; "Report status bar" reads the rows
//! below the caret after a fresh read; and a command's line, marked when it
//! was entered, is found by the outpost's search, and the history read
//! counted from the start Core names, while output scrolls.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::cell::RefCell;
use std::rc::Rc;

use verbatim_model::request::TerminalAsk;
use verbatim_model::{
    CallCounts, CaretKey, CaretMotion, ReviewCommand, SegmentContent, TerminalScope,
    TerminalSnapshot, TextOp,
};
use verbatim_outpost::terminal::{ReadAsk, read};
use verbatim_outpost::text::uia::UiaText;
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use verbatim_uia_rops::CaretLineQuery;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextRange};

use common::core_driver::{CoreDriver, CoreTime, Utterance};
use common::outpost::OutpostUnderTest;
use verbatim_outpost::Heard;

/// The terminal's width in cells.
const WIDTH: usize = 12;

/// What a step said that is nothing.
const NOTHING: [Utterance; 0] = [];

/// mockapp's terminal's element, found by its name
/// (`common::named_element`), which passes the window's title bar by.
fn terminal_element(uia: &Uia, hwnd: HWND) -> IUIAutomationElement {
    common::named_element(uia, hwnd, "Terminal")
}

/// `rows` as a terminal's text: each padded to [`WIDTH`] and ended by a
/// carriage return and line feed, escaped for `set-text`; and the UTF-16
/// offset of the end of the last row's text, where the caret is. The text's
/// last line break gives mockapp an empty last line, so that moving by
/// lines from the caret to the document's end counts the caret's row as
/// the terminals' providers do (`verbatim_uia_rops::Snapshot::caret_rows_to_end`).
fn terminal_text(rows: &[&str]) -> (String, usize) {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:WIDTH$}\r\n")).collect();
    let text = padded.concat();
    let before: usize = padded[..rows.len() - 1]
        .iter()
        .map(|row| row.encode_utf16().count())
        .sum();
    let caret = before + rows.last().map_or(0, |row| row.encode_utf16().count());
    (text.replace('\r', "\\r").replace('\n', "\\n"), caret)
}

/// Shows `rows` in mockapp's terminal, the caret at the end of the last
/// row's text, raising no event.
fn show(app: &mut common::MockApp, hwnd: HWND, rows: &[&str]) {
    let (text, caret) = terminal_text(rows);
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, &format!("caret term {caret} {caret}"));
}

/// Shows `rows` in mockapp's terminal with the caret at the end of row
/// `caret`'s text, raising no event.
fn show_caret_on(app: &mut common::MockApp, hwnd: HWND, rows: &[&str], caret: usize) {
    let (text, _) = terminal_text(rows);
    let row_units = WIDTH + 2;
    let at = caret * row_units + rows[caret].encode_utf16().count();
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, &format!("caret term {at} {at}"));
}

/// What Core names in a terminal's reads: the start the last read minted,
/// and the screen's shape from its model, 0 before it has one
/// (`docs/design/readers-tell-core-asks-build3.md`, "The full read request
/// and its starts").
#[derive(Default)]
struct Terminal {
    start: Option<IUIAutomationTextRange>,
    height: u32,
    width: u32,
}

impl Terminal {
    fn new() -> Self {
        Self::default()
    }

    /// The start the last read minted, which a history read or search
    /// counts from.
    fn start(&self) -> &IUIAutomationTextRange {
        self.start.as_ref().expect("a read before")
    }
}

/// One fresh read of the terminal from the start the last read minted, as
/// the worker makes it, and the snapshot it answers.
fn read_terminal(
    uia: &Uia,
    source: &UiaText,
    terminal: &mut Terminal,
    remote: bool,
) -> TerminalSnapshot {
    let caret = CaretLineQuery {
        element: source.element(),
        pattern: source.pattern(),
        max_text: 4096,
        since: None,
    };
    let ask = ReadAsk {
        start: terminal.start.as_ref(),
        scope: TerminalScope::Fresh,
        height: terminal.height,
        width: terminal.width,
        colours: false,
    };
    let mut answer = read(
        uia,
        (source.element(), source.pattern()),
        caret,
        &ask,
        remote,
    )
    .expect("the terminal reads");
    terminal.start = answer.next_start.take();
    answer.snapshot
}

/// The outpost's source for mockapp's terminal.
fn source(uia: &Uia, hwnd: HWND, remote: bool) -> UiaText {
    let element = terminal_element(uia, hwnd);
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    UiaText::new(element, pattern, true).remote(remote)
}

fn uia_calls(uia: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa: 0,
        window_messages: 0,
        connections: 0,
    }
}

/// An utterance of `text` alone.
fn text(text: &str) -> Utterance {
    vec![SegmentContent::Text(text.to_owned())]
}

/// An utterance of the character `text` alone.
fn character(text: &str) -> Utterance {
    vec![SegmentContent::Character(text.to_owned())]
}

/// mockapp's Windows Terminal control, its process shared with what a test
/// does as Core sends a request ([`CoreDriver::observe_sends`]), and Core
/// driving an outpost watching it, the terminal focused, followed and read
/// for its baseline.
struct Driven {
    app: Rc<RefCell<common::MockApp>>,
    hwnd: HWND,
    core: CoreDriver,
}

impl Driven {
    /// mockapp's terminal showing `rows` on a screen of `height` rows, the
    /// caret at the end of the last, focused with Core driving its outpost,
    /// which reads by remote operations when `remote`. Core's time is the
    /// run's shared clock, on which the outpost stamps each read's start, so
    /// a key's time and the reads after it compare as in a run.
    fn start(name: &str, (rows, height): (&[&str], u32), remote: bool) -> Self {
        common::init_com();
        let title = common::unique_title(name);
        let mut app = common::spawn("terminal_reading.json", "uia", &title);
        let hwnd = common::find_window(&title);
        common::apply(&mut app, hwnd, &format!("screen term {height}"));
        show(&mut app, hwnd, rows);
        let outpost = OutpostUnderTest::with_options(
            &app,
            OutpostOptions {
                remote_operations: remote,
            },
        );
        let mut core = CoreDriver::on_time(&app, outpost, CoreTime::Real);
        app.send("set-focus term");
        let uia = Uia::new().expect("a UIA client");
        let _ = core.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
        Self {
            app: Rc::new(RefCell::new(app)),
            hwnd,
            core,
        }
    }

    /// Shows `rows`, the caret at the end of the last, raising no event.
    fn show(&self, rows: &[&str]) {
        show(&mut self.app.borrow_mut(), self.hwnd, rows);
    }

    /// Raises the terminal's text change, which the outpost tells Core of;
    /// what Core said once its read was answered.
    fn changed(&mut self) -> Vec<Utterance> {
        self.app.borrow_mut().send("text-changed term");
        self.core.told(1).spoken()
    }

    /// Shows `rows` and raises the terminal's text change
    /// ([`Self::changed`]).
    fn write(&mut self, rows: &[&str]) -> Vec<Utterance> {
        self.show(rows);
        self.changed()
    }

    /// Has `act` run with mockapp, and the terminal's text change raised
    /// after it, the first time Core sends the outpost a request that `of`
    /// picks, before the outpost is handed it, which waits until the
    /// outpost has heard the change: its tell then reaches Core while the
    /// request is out. Counts in the counter returned each request `of`
    /// picks.
    fn before_first(
        &mut self,
        of: fn(&TextOp) -> bool,
        act: impl FnOnce(&mut common::MockApp, HWND) + 'static,
    ) -> Rc<RefCell<u32>> {
        let app = Rc::clone(&self.app);
        let hwnd = self.hwnd;
        let count = Rc::new(RefCell::new(0));
        let counted = Rc::clone(&count);
        let mut act = Some(act);
        self.core.observe_sends(Some(Box::new(move |ask, outpost| {
            let Some(TerminalAsk { op, .. }) = ask.request.terminal() else {
                return;
            };
            if !of(op) {
                return;
            }
            *counted.borrow_mut() += 1;
            if let Some(act) = act.take() {
                act(&mut app.borrow_mut(), hwnd);
                app.borrow_mut().send("text-changed term");
                outpost.heard(&[Heard::UiaTextChanged]);
            }
        })));
        count
    }

    fn quit(mut self) {
        self.core.observe_sends(None);
        let Self { app, core, .. } = self;
        drop(core);
        Rc::try_unwrap(app)
            .unwrap_or_else(|_| panic!("mockapp is still shared"))
            .into_inner()
            .quit();
    }
}

/// Output written below the rows read is said line by line, and Core's read
/// of it is one remote program, its own call.
fn new_output_is_said_line_by_line(remote: bool) {
    // The screen: three rows, and the empty last line.
    let mut driven = Driven::start(
        "mockapp-terminal-output",
        (&["one", "two", "ready>"], 4),
        remote,
    );
    driven.show(&["one", "two", "three", "four", "ready>"]);
    driven.app.borrow_mut().send("text-changed term");
    let step = driven.core.told(1);
    if remote {
        assert_eq!(step.calls, uia_calls(1), "one program, one call");
    }
    assert_eq!(step.spoken(), [text("three"), text("four")]);
    driven.quit();
}

fn new_output_is_said_line_by_line_remote() {
    new_output_is_said_line_by_line(true);
}

fn new_output_is_said_line_by_line_classic() {
    new_output_is_said_line_by_line(false);
}

/// "Report status bar" reads the rows below the caret, after the fresh read
/// Core asks the outpost for, and nothing else.
fn the_status_bar_is_read_below_the_caret(remote: bool) {
    let rows = ["one", "ready>", "status: busy"];
    let mut driven = Driven::start("mockapp-terminal-status-bar", (&rows, 5), remote);
    show_caret_on(&mut driven.app.borrow_mut(), driven.hwnd, &rows, 1);
    assert_eq!(driven.changed(), NOTHING, "the caret moved to the prompt");
    show_caret_on(
        &mut driven.app.borrow_mut(),
        driven.hwnd,
        &["one", "ready>", "status: done"],
        1,
    );
    let step = driven.core.command(ReviewCommand::ReportStatusBar);
    assert_eq!(step.spoken(), [text("done"), text("status: done")]);
    driven.quit();
}

fn the_status_bar_is_read_below_the_caret_remote() {
    the_status_bar_is_read_below_the_caret(true);
}

fn the_status_bar_is_read_below_the_caret_classic() {
    the_status_bar_is_read_below_the_caret(false);
}

/// A command is typed at the prompt and entered, so Core marks its line;
/// output carries it above the rows Core keeps; Verbatim+Alt+Up Arrow then
/// finds it with the outpost's search, moves the review cursor there and
/// speaks it, and a second press, with no mark above, says nothing.
fn command_marks_move_the_review_cursor(remote: bool) {
    let mut driven = with_a_command("mockapp-terminal-marks", remote);
    let step = driven.core.command(ReviewCommand::PreviousCommandMark);
    assert_eq!(step.spoken(), [text("ready>ls")]);
    let step = driven.core.command(ReviewCommand::PreviousCommandMark);
    assert_eq!(step.spoken(), NOTHING);
    driven.quit();
}

fn command_marks_move_the_review_cursor_remote() {
    command_marks_move_the_review_cursor(true);
}

fn command_marks_move_the_review_cursor_classic() {
    command_marks_move_the_review_cursor(false);
}

/// The rows of [`with_a_command`]'s terminal: `ls` entered, and `lines`
/// lines of output after its answer.
fn rows_after_ls(lines: u32) -> Vec<String> {
    let mut rows: Vec<String> = ["one", "ready>ls", "a.txt"].map(str::to_owned).into();
    rows.extend((1..=lines).map(|line| format!("line {line}")));
    rows.push("ready>".to_owned());
    rows
}

/// Shows [`rows_after_ls`]`(lines)`.
fn show_after_ls(app: &mut common::MockApp, hwnd: HWND, lines: u32) {
    let rows = rows_after_ls(lines);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    show(app, hwnd, &rows);
}

/// As [`command_marks_move_the_review_cursor`], but the program prints
/// three more lines as the search is on its way, told to Core while it is
/// out: the search counts from the start Core named, and its answer is
/// placed from the anchor Core holds at that start, which the read taken
/// meanwhile moves; the move lands on the command's line, and the next line
/// is the command's output. Converted from the test of a search counted
/// from a newer read: every search names its start (build 3, "Searches and
/// history reads count from a start Core names").
fn command_marks_are_found_while_output_scrolls(remote: bool) {
    let mut driven = with_a_command("mockapp-terminal-marks-scrolling", remote);
    let finds = driven.before_first(
        |op| matches!(op, TextOp::TerminalFind { .. }),
        |app, hwnd| show_after_ls(app, hwnd, 13),
    );
    let step = driven.core.command(ReviewCommand::PreviousCommandMark);
    assert_eq!(*finds.borrow(), 1);
    // The new output waits in the flood policy's queue behind the earlier
    // output, which this test never plays.
    assert_eq!(step.spoken(), [text("ready>ls")]);
    let step = driven.core.command(ReviewCommand::ReviewNextLine);
    assert_eq!(step.spoken(), [text("a.txt")]);
    driven.quit();
}

fn command_marks_are_found_while_output_scrolls_remote() {
    command_marks_are_found_while_output_scrolls(true);
}

fn command_marks_are_found_while_output_scrolls_classic() {
    command_marks_are_found_while_output_scrolls(false);
}

/// The review cursor on the command's line above the rows Core keeps, as
/// [`command_marks_move_the_review_cursor`] puts it there; the program
/// prints three more lines as the history read the next line needs is on
/// its way, told to Core while it is out: the history's rows are placed
/// from the anchor Core holds at the start it named, the screen more Core
/// asked for still holds the command's output, and the review cursor moves
/// to it, with one history read on the remote path.
fn the_history_is_placed_while_output_scrolls_remote() {
    let mut driven = with_a_command("mockapp-terminal-history-scrolling", true);
    let step = driven.core.command(ReviewCommand::PreviousCommandMark);
    assert_eq!(step.spoken(), [text("ready>ls")]);
    let histories = driven.before_first(
        |op| matches!(op, TextOp::TerminalHistory { .. }),
        |app, hwnd| show_after_ls(app, hwnd, 13),
    );
    let step = driven.core.command(ReviewCommand::ReviewNextLine);
    assert_eq!(*histories.borrow(), 1);
    // The new output waits in the flood policy's queue, which this test
    // never plays.
    assert_eq!(step.spoken(), [text("a.txt")]);
    driven.quit();
}

/// A terminal of four rows where `ls` was typed at the prompt and entered,
/// so Core marked its line, and ten lines of output then carried it above
/// the rows Core keeps.
fn with_a_command(name: &str, remote: bool) -> Driven {
    let mut driven = Driven::start(name, (&["one", "ready>"], 4), remote);
    let mut prompt = "ready>".to_owned();
    for typed in ["l", "s", "\r"] {
        let _ = driven.core.typed(typed);
        if typed == "\r" {
            let _ = driven.write(&["one", &prompt, "a.txt", "ready>"]);
        } else {
            prompt.push_str(typed);
            let _ = driven.write(&["one", &prompt]);
        }
    }
    let rows = rows_after_ls(10);
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    let _ = driven.write(&rows);
    driven
}

/// Typing "helo" after the prompt is echoed character by character as the
/// terminal shows it, each read for the terminal's text change, and
/// Backspace then says exactly "o", from the key's final read, which shows
/// its effect with no event; the text change raised after it is read
/// again and says nothing more.
fn backspace_says_what_it_deleted(remote: bool) {
    // The screen: three rows, and the empty last line.
    let mut driven = Driven::start(
        "mockapp-terminal-backspace",
        (&["one", "two", "ready>"], 4),
        remote,
    );
    let mut prompt = "ready>".to_owned();
    for typed in ["h", "e", "l", "o"] {
        let _ = driven.core.typed(typed);
        prompt.push_str(typed);
        assert_eq!(driven.write(&["one", "two", &prompt]), [character(typed)]);
    }
    let backspace = CaretKey {
        motion: CaretMotion::Backspace,
        select: false,
    };
    prompt.pop();
    let app = Rc::clone(&driven.app);
    let hwnd = driven.hwnd;
    let step = driven.core.caret_key(backspace, || {
        show(&mut app.borrow_mut(), hwnd, &["one", "two", &prompt]);
    });
    drop(app);
    assert_eq!(step.spoken(), [character("o")]);
    assert_eq!(driven.changed(), NOTHING);
    driven.quit();
}

fn backspace_says_what_it_deleted_remote() {
    backspace_says_what_it_deleted(true);
}

fn backspace_says_what_it_deleted_classic() {
    backspace_says_what_it_deleted(false);
}

/// The outpost's search for `needle` in `terminal`'s text, outward from the
/// row `from` rows above where its next read starts, on the path under
/// test alone: the remote program, never falling back, or the classic one.
fn find_on_path(
    uia: &Uia,
    source: &UiaText,
    terminal: &Terminal,
    (needle, from): (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    let start = terminal.start().clone();
    let query = verbatim_uia_rops::FindQuery {
        element: source.element(),
        pattern: source.pattern(),
        start: &start,
        needle,
        from,
        rows: 1,
    };
    let found = if remote {
        verbatim_uia_rops::terminal_find_remote(uia, &query)
    } else {
        verbatim_uia_rops::terminal_find_classic(uia, &query)
    };
    found
        .expect("the search runs on its path")
        .into_iter()
        .map(|found| verbatim_model::TerminalMatch {
            above: found.above,
            text: found.text,
        })
        .collect()
}

/// A provider whose `FindText` fails (`terminal_find_fails.json`): the
/// search finds nothing, and is no error.
fn a_failing_search_finds_nothing(remote: bool) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-fails");
    let mut app = common::spawn("terminal_find_fails.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, remote);
    let found = find_on_path(&uia, &source, &terminal, ("two", 1), remote);
    assert_eq!(found, []);
    app.quit();
}

fn a_failing_search_finds_nothing_remote() {
    a_failing_search_finds_nothing(true);
}

fn a_failing_search_finds_nothing_classic() {
    a_failing_search_finds_nothing(false);
}

/// A provider whose `FindText` fails when it searches backward
/// (`terminal_find_backward_fails.json`): the failure closes the search's
/// upward direction alone, and the search goes on downward from its row.
fn a_failing_search_goes_on_in_the_other_direction(remote: bool) {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    let found = search_in("terminal_find_backward_fails.json", &rows, ("y", 2), remote);
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(1, "y 4"),
            found_row(0, "y 5"),
        ]
    );
}

fn a_failing_search_goes_on_in_the_other_direction_remote() {
    a_failing_search_goes_on_in_the_other_direction(true);
}

fn a_failing_search_goes_on_in_the_other_direction_classic() {
    a_failing_search_goes_on_in_the_other_direction(false);
}

/// A provider whose `FindText` answers a search that finds nothing with a
/// range rather than null, as Windows Terminal's does
/// (`terminal_find_misses_answer_a_range.json`): the search takes every
/// match outward from its row, each direction ending where the text is
/// only elsewhere, and finds nothing for text that is nowhere. The remote
/// program makes no call that Windows Terminal answers with
/// `RPC_E_SERVERFAULT`, which would end it there; the classic
/// implementation's failed calls only end a direction.
fn a_search_whose_misses_answer_a_range(remote: bool) -> u32 {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-misses");
    let mut app = common::spawn("terminal_find_misses_answer_a_range.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    show(
        &mut app,
        hwnd,
        &["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"],
    );
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, remote);
    common::reset_hits(hwnd);
    assert_eq!(
        find_on_path(&uia, &source, &terminal, ("y", 2), remote),
        [
            found_row(2, "y 3"),
            found_row(3, "y 2"),
            found_row(1, "y 4"),
            found_row(4, "y 1"),
            found_row(0, "y 5"),
        ]
    );
    assert_eq!(find_on_path(&uia, &source, &terminal, ("q", 2), remote), []);
    let faults = common::read_hits(hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "RPC_E_SERVERFAULT").then_some(count))
        .unwrap_or(0);
    app.quit();
    faults
}

fn a_search_whose_misses_answer_a_range_finds_its_matches_remote() {
    assert_eq!(a_search_whose_misses_answer_a_range(true), 0);
}

fn a_search_whose_misses_answer_a_range_finds_its_matches_classic() {
    let _ = a_search_whose_misses_answer_a_range(false);
}

/// The outpost's search for `needle` over every row of `rows`, outward
/// from the row `from` rows above where the next read starts, on a screen
/// of three rows.
fn search(
    rows: &[&str],
    needle_from: (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    search_in("terminal.json", rows, needle_from, remote)
}

/// As [`search`], on the terminal of `fixture`.
fn search_in(
    fixture: &str,
    rows: &[&str],
    (needle, from): (&str, i32),
    remote: bool,
) -> Vec<verbatim_model::TerminalMatch> {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find");
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    show(&mut app, hwnd, rows);
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, remote);
    let found = find_on_path(&uia, &source, &terminal, (needle, from), remote);
    app.quit();
    found
}

/// A match: its rows above the start, and its row's text as read.
fn found_row(above: i32, text: &str) -> verbatim_model::TerminalMatch {
    verbatim_model::TerminalMatch {
        above,
        text: format!("{text:WIDTH$}\r\n"),
    }
}

/// A short needle found within rows far more often than at a row's start,
/// as a short command's text is, sought up from below the document's end:
/// only the rows that start with it are matches, nearest first, a longer
/// row starting with it among them for Core to check by its line's hash,
/// and the hits within rows do not use up the matches a search takes.
fn the_search_takes_rows_starting_with_the_needle(remote: bool) {
    let within = " y y y y y y";
    let mut rows = vec!["y", "yes"];
    rows.extend([within; 5]);
    rows.push("ready>");
    let found = search(&rows, ("y", -10), remote);
    assert_eq!(found, [found_row(5, "yes"), found_row(6, "y")]);
}

fn the_search_takes_rows_starting_with_the_needle_remote() {
    the_search_takes_rows_starting_with_the_needle(true);
}

fn the_search_takes_rows_starting_with_the_needle_classic() {
    the_search_takes_rows_starting_with_the_needle(false);
}

/// From a row in the middle, the search takes that row's match first, then
/// alternately the next above and the next below.
fn the_search_works_outward_from_its_row(remote: bool) {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    // The document's seven rows end with an empty line, and the start is
    // the screen's top, three rows from the end: "y 3", on row 2, is two
    // rows above it.
    let found = search(&rows, ("y", 2), remote);
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(3, "y 2"),
            found_row(1, "y 4"),
            found_row(4, "y 1"),
            found_row(0, "y 5"),
        ]
    );
}

fn the_search_works_outward_from_its_row_remote() {
    the_search_works_outward_from_its_row(true);
}

fn the_search_works_outward_from_its_row_classic() {
    the_search_works_outward_from_its_row(false);
}

/// A needle found within rows more often than the search's `FindText`
/// calls allow ends the search before a row starting with it, further
/// away, is reached.
fn the_search_ends_after_its_calls(remote: bool) {
    let within = " y y y y y y";
    let mut rows = vec!["y"];
    // Six hits a row, a row more than the calls allow.
    rows.extend(vec![within; verbatim_uia_rops::FIND_CALLS / 6 + 1]);
    rows.push("ready>");
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-calls");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    common::apply(&mut app, hwnd, "screen term 3");
    show(&mut app, hwnd, &rows);
    let source = source(&uia, hwnd, remote);
    let mut terminal = Terminal::new();
    let _ = read_terminal(&uia, &source, &mut terminal, remote);
    common::reset_hits(hwnd);
    assert_eq!(
        find_on_path(&uia, &source, &terminal, ("y", -10), remote),
        []
    );
    // Every `FindText` call the search made, the remote program's two over
    // the whole document among them.
    let calls = common::read_hits(hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "FindText").then_some(count));
    app.quit();
    assert_eq!(
        calls,
        Some(u32::try_from(verbatim_uia_rops::FIND_CALLS).expect("a small bound"))
    );
}

fn the_search_ends_after_its_calls_remote() {
    the_search_ends_after_its_calls(true);
}

fn the_search_ends_after_its_calls_classic() {
    the_search_ends_after_its_calls(false);
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "new_output_is_said_line_by_line_remote",
            new_output_is_said_line_by_line_remote,
        ),
        (
            "new_output_is_said_line_by_line_classic",
            new_output_is_said_line_by_line_classic,
        ),
        (
            "backspace_says_what_it_deleted_remote",
            backspace_says_what_it_deleted_remote,
        ),
        (
            "backspace_says_what_it_deleted_classic",
            backspace_says_what_it_deleted_classic,
        ),
        (
            "the_status_bar_is_read_below_the_caret_remote",
            the_status_bar_is_read_below_the_caret_remote,
        ),
        (
            "the_status_bar_is_read_below_the_caret_classic",
            the_status_bar_is_read_below_the_caret_classic,
        ),
        (
            "command_marks_move_the_review_cursor_remote",
            command_marks_move_the_review_cursor_remote,
        ),
        (
            "command_marks_move_the_review_cursor_classic",
            command_marks_move_the_review_cursor_classic,
        ),
        (
            "command_marks_are_found_while_output_scrolls_remote",
            command_marks_are_found_while_output_scrolls_remote,
        ),
        (
            "command_marks_are_found_while_output_scrolls_classic",
            command_marks_are_found_while_output_scrolls_classic,
        ),
        (
            "the_history_is_placed_while_output_scrolls_remote",
            the_history_is_placed_while_output_scrolls_remote,
        ),
        (
            "the_search_works_outward_from_its_row_remote",
            the_search_works_outward_from_its_row_remote,
        ),
        (
            "the_search_works_outward_from_its_row_classic",
            the_search_works_outward_from_its_row_classic,
        ),
        (
            "a_failing_search_finds_nothing_remote",
            a_failing_search_finds_nothing_remote,
        ),
        (
            "a_failing_search_finds_nothing_classic",
            a_failing_search_finds_nothing_classic,
        ),
        (
            "a_search_whose_misses_answer_a_range_finds_its_matches_remote",
            a_search_whose_misses_answer_a_range_finds_its_matches_remote,
        ),
        (
            "a_search_whose_misses_answer_a_range_finds_its_matches_classic",
            a_search_whose_misses_answer_a_range_finds_its_matches_classic,
        ),
        (
            "a_failing_search_goes_on_in_the_other_direction_remote",
            a_failing_search_goes_on_in_the_other_direction_remote,
        ),
        (
            "a_failing_search_goes_on_in_the_other_direction_classic",
            a_failing_search_goes_on_in_the_other_direction_classic,
        ),
        (
            "the_search_takes_rows_starting_with_the_needle_remote",
            the_search_takes_rows_starting_with_the_needle_remote,
        ),
        (
            "the_search_takes_rows_starting_with_the_needle_classic",
            the_search_takes_rows_starting_with_the_needle_classic,
        ),
        (
            "the_search_ends_after_its_calls_remote",
            the_search_ends_after_its_calls_remote,
        ),
        (
            "the_search_ends_after_its_calls_classic",
            the_search_ends_after_its_calls_classic,
        ),
    ]);
}
