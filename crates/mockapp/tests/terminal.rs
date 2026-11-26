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
//! what Core says.
//!
//! What is checked: Core says the new output a read shows, line by line,
//! and the read costs one call remotely; a terminal's typing is echoed
//! character by character and Backspace says what it deleted, the keys
//! reaching the rules through Core; "Report status bar" reads the rows
//! below the caret after a fresh read; and a command's line, marked when it
//! was entered, is found by Core's search batch, and the history read
//! counted from the start Core names, while output scrolls.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::cell::RefCell;
use std::rc::Rc;

use verbatim_core::terminal::reader;
use verbatim_model::request::Question;
use verbatim_model::{CallCounts, CaretKey, CaretMotion, ReviewCommand, SegmentContent};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;

use common::core_driver::{CoreDriver, CoreTime, Utterance};
use common::outpost::OutpostUnderTest;
use verbatim_outpost::Heard;

/// The terminal's width in cells.
const WIDTH: usize = 12;

/// What a step said that is nothing.
const NOTHING: [Utterance; 0] = [];

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
        of: fn(&Question) -> bool,
        act: impl FnOnce(&mut common::MockApp, HWND) + 'static,
    ) -> Rc<RefCell<u32>> {
        let app = Rc::clone(&self.app);
        let hwnd = self.hwnd;
        let count = Rc::new(RefCell::new(0));
        let counted = Rc::clone(&count);
        let mut act = Some(act);
        self.core.observe_sends(Some(Box::new(move |ask, outpost| {
            if !of(&ask.request) {
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
        |request| {
            matches!(request, Question::Batch(batch)
                if matches!(reader::asked(batch), Some(reader::Asked::Search(_))))
        },
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
        |request| {
            matches!(request, Question::Batch(batch)
                if matches!(reader::asked(batch), Some(reader::Asked::History(_))))
        },
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
    ]);
}
