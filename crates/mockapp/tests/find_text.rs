//! The request list's `FindText`, composed as a search outward from a row
//! by this suite's support (`common/window_steps.rs`, `add_search`: a
//! move to the row, the search down and up from it, each way to 20
//! matches that start a row over 100 calls), against mockapp's UIA text
//! provider, through a real outpost reading by remote operations and one
//! reading by calls, which must answer alike. Core's command search makes
//! the same two searches (`verbatim_core::terminal::reader::search_batch`);
//! these tests cover the executors' arms of the operation: a provider
//! whose search fails, one whose backward search fails, one whose misses
//! answer a range as Windows Terminal's do, the rows that start with the
//! needle, the order outward from a row, and the bound on calls.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::batch::Op;
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

use common::outpost::OutpostUnderTest;

/// The terminal's width in cells.
const WIDTH: usize = 12;

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

/// Brands the generic steps' batches.
enum Searching {}

/// The search for `needle` outward from the row `from` rows above the
/// screen's top, composed of the request list's generic steps
/// (`common::window_steps::add_search`) run through a real
/// outpost reading mockapp's terminal by remote operations when `remote`:
/// the matches, by their rows.
fn find_generic(
    app: &common::MockApp,
    hwnd: HWND,
    (needle, from): (&str, i32),
    remote: bool,
) -> Vec<common::window_steps::Match> {
    use verbatim_core::batch::{Answers, Builder};
    use verbatim_model::batch::{Batch, Done, Placement, Step, StepOutcome, Subject};
    use verbatim_model::request::{Outcome, Reply, WindowVerdict};
    let uia = Uia::new().expect("a UIA client");
    let outpost = OutpostUnderTest::with_options(
        app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost.read_focus_as(&terminal_element(&uia, hwnd));
    let steps = |ops: Vec<Op>| Batch {
        steps: ops.into_iter().map(|op| Step { op, when: None }).collect(),
        placement: Placement::Queue,
        spare: None,
    };
    let done = |outcome: &Outcome| -> Vec<StepOutcome> {
        let Outcome::Result(Reply::Steps(steps)) = outcome else {
            panic!("steps, not {outcome:?}")
        };
        steps.clone()
    };
    let (focus, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        steps(vec![Op::FocusRead {
            subject: Subject::Focus,
            known: Vec::new(),
            set: common::outpost::focus_set(),
            details: verbatim_model::TextDetails::NONE,
        }]),
    );
    let Some(StepOutcome::Done(Done::FocusRead(read))) = done(&focus.outcome).first().cloned()
    else {
        panic!("the terminal's focus read")
    };
    let node = read.focus.read.node.expect("the terminal's node");
    let (visible, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        steps(vec![Op::Visible {
            subject: Subject::Node(node),
        }]),
    );
    let Some(StepOutcome::Done(Done::Visible(visible))) = done(&visible.outcome).first().cloned()
    else {
        panic!("the screen's top")
    };
    let mut search = Builder::<Searching>::named();
    let found = common::window_steps::add_search(&mut search, node, (visible.start, from), needle)
        .expect("a row to search from");
    let sent = search.finish();
    let (answer, _) = outpost.batch(hwnd, WindowVerdict::Uia, sent.batch().clone());
    let Outcome::Result(reply) = answer.outcome else {
        panic!("the search's steps, not {:?}", answer.outcome)
    };
    let answers = Answers::parse(&sent, reply).expect("the search's answers");
    common::window_steps::matches_of(&answers, &found).expect("the search ran")
}

/// The search for `needle` from `from` by the generic steps, through an
/// outpost reading by remote operations and one reading by calls, which
/// must answer alike: the matches, the rows below first, each way nearest
/// first.
fn search_both(
    app: &common::MockApp,
    hwnd: HWND,
    needle_from: (&str, i32),
) -> Vec<common::window_steps::Match> {
    let remote = find_generic(app, hwnd, needle_from, true);
    assert_eq!(
        find_generic(app, hwnd, needle_from, false),
        remote,
        "the search by remote operations and by calls differ"
    );
    remote
}

/// mockapp's terminal of `fixture` showing `rows` on a screen of three
/// rows, focused.
fn searched_terminal(fixture: &str, rows: &[&str]) -> (common::MockApp, HWND) {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find");
    let mut app = common::spawn(fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    common::apply(&mut app, hwnd, "screen term 3");
    if !rows.is_empty() {
        show(&mut app, hwnd, rows);
    }
    app.send("set-focus term");
    (app, hwnd)
}

/// A provider whose `FindText` fails (`terminal_find_fails.json`): the
/// search finds nothing, and is no error.
fn a_failing_search_finds_nothing() {
    let (app, hwnd) = searched_terminal("terminal_find_fails.json", &[]);
    assert_eq!(search_both(&app, hwnd, ("two", 1)), []);
    app.quit();
}

/// A provider whose `FindText` fails when it searches backward
/// (`terminal_find_backward_fails.json`): the failure ends the search up
/// from the row, and the search down from it goes on.
fn a_failing_search_goes_on_in_the_other_direction() {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    let found = search_in("terminal_find_backward_fails.json", &rows, ("y", 2));
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(1, "y 4"),
            found_row(0, "y 5"),
        ]
    );
}

/// A provider whose `FindText` answers a search that finds nothing with a
/// range rather than null, as Windows Terminal's does
/// (`terminal_find_misses_answer_a_range.json`): the search takes every
/// match each way from its row, each way ending where the text is only
/// elsewhere, and finds nothing for text that is nowhere. The outpost
/// reading by remote operations makes no call the provider answers with
/// `RPC_E_SERVERFAULT`, which ends a remote run in Windows Terminal; the
/// one reading by calls makes them, each ending its way alone.
fn a_search_whose_misses_answer_a_range_finds_its_matches() {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    let (app, hwnd) = searched_terminal("terminal_find_misses_answer_a_range.json", &rows);
    common::reset_hits(hwnd);
    let found = find_generic(&app, hwnd, ("y", 2), true);
    let nowhere = find_generic(&app, hwnd, ("q", 2), true);
    let faults = common::read_hits(hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "RPC_E_SERVERFAULT").then_some(count));
    let by_calls =
        [("y", 2), ("q", 2)].map(|needle_from| find_generic(&app, hwnd, needle_from, false));
    app.quit();
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(1, "y 4"),
            found_row(0, "y 5"),
            found_row(3, "y 2"),
            found_row(4, "y 1"),
        ]
    );
    assert_eq!(nowhere, []);
    assert_eq!(faults, None, "the searches by remote operations");
    assert_eq!(by_calls, [found, nowhere], "the searches by calls");
}

/// The search for `needle` over every row of `rows`, from the row `from`
/// rows above the screen's top, on a screen of three rows.
fn search(rows: &[&str], needle_from: (&str, i32)) -> Vec<common::window_steps::Match> {
    search_in("terminal.json", rows, needle_from)
}

/// As [`search`], on the terminal of `fixture`.
fn search_in(
    fixture: &str,
    rows: &[&str],
    needle_from: (&str, i32),
) -> Vec<common::window_steps::Match> {
    let (app, hwnd) = searched_terminal(fixture, rows);
    let found = search_both(&app, hwnd, needle_from);
    app.quit();
    found
}

/// A match: its rows above the screen's top, and its row's text as read.
fn found_row(above: i32, text: &str) -> common::window_steps::Match {
    common::window_steps::Match {
        above,
        text: format!("{text:WIDTH$}\r\n"),
    }
}

/// A short needle found within rows far more often than at a row's start,
/// as a short command's text is, sought up from below the document's end:
/// only the rows that start with it are matches, nearest first, a longer
/// row starting with it among them for the caller to check by its line's text,
/// and the hits within rows do not use up the matches a search takes.
fn the_search_takes_rows_starting_with_the_needle() {
    let within = " y y y y y y";
    let mut rows = vec!["y", "yes"];
    rows.extend([within; 5]);
    rows.push("ready>");
    let found = search(&rows, ("y", -10));
    assert_eq!(found, [found_row(5, "yes"), found_row(6, "y")]);
}

/// From a row in the middle, the search takes that row's match and those
/// below it, nearest first, then those above it, nearest first.
fn the_search_works_outward_from_its_row() {
    let rows = ["y 1", "y 2", "y 3", "y 4", "y 5", "ready>"];
    // The document's seven rows end with an empty line, and the screen's
    // top is three rows from the end: "y 3", on row 2, is two rows above
    // it.
    let found = search(&rows, ("y", 2));
    assert_eq!(
        found,
        [
            found_row(2, "y 3"),
            found_row(1, "y 4"),
            found_row(0, "y 5"),
            found_row(3, "y 2"),
            found_row(4, "y 1"),
        ]
    );
}

/// A needle found within rows more often than a way of the search's
/// `FindText` calls allow ends that way before a row starting with it,
/// further away, is reached: one call down over the empty span below the
/// document's end, and a hundred up, for each of the two outposts, and
/// for the one reading by remote operations two more, outside a way's
/// calls: the search of the whole text, which the two ways share, and the
/// search for text no terminal holds.
fn the_search_ends_after_its_calls() {
    let within = " y y y y y y";
    let mut rows = vec!["y"];
    // Six hits a row, a row more than a way's calls allow.
    rows.extend(vec![within; 100 / 6 + 1]);
    rows.push("ready>");
    let (app, hwnd) = searched_terminal("terminal.json", &rows);
    common::reset_hits(hwnd);
    assert_eq!(search_both(&app, hwnd, ("y", -10)), []);
    let calls = common::read_hits(hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "FindText").then_some(count));
    app.quit();
    assert_eq!(calls, Some(2 * 101 + 2), "both outposts' searches' calls");
}

/// Runs each test on a desktop of its own (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_failing_search_finds_nothing",
            a_failing_search_finds_nothing,
        ),
        (
            "a_failing_search_goes_on_in_the_other_direction",
            a_failing_search_goes_on_in_the_other_direction,
        ),
        (
            "a_search_whose_misses_answer_a_range_finds_its_matches",
            a_search_whose_misses_answer_a_range_finds_its_matches,
        ),
        (
            "the_search_takes_rows_starting_with_the_needle",
            the_search_takes_rows_starting_with_the_needle,
        ),
        (
            "the_search_works_outward_from_its_row",
            the_search_works_outward_from_its_row,
        ),
        (
            "the_search_ends_after_its_calls",
            the_search_ends_after_its_calls,
        ),
    ]);
}
