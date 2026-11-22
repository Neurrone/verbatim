//! A terminal's window read, the request list's generic steps
//! (`verbatim_core::terminal_window`), against mockapp's UIA text provider,
//! whose `set-text` command rewrites the terminal's text as a terminal's
//! buffer changes, every row padded to the width and ended by a carriage
//! return and line feed, and whose `screen` command puts only the text's
//! last rows on screen. Each read runs through a real outpost with remote
//! operations on and one with them off, which must answer alike, the
//! remote read one program and the colours one more, and the window Core
//! makes of the outcomes is pinned whole.
//!
//! What is checked: a first read from the screen's top; a read after
//! output, from two screens above the document's end, counting the rows
//! written past the end marks it names and giving the opening rows after
//! them, with the colours of the screen rows whose text changed; a read
//! after a clear, whose marks lie past the document's end and are refused;
//! the search for a mark's line, by classic calls, against both of Windows
//! Terminal's kinds of miss (`terminal_find_misses_answer_a_range.json`):
//! the line above the window found, a line in the window not sought, and a
//! line nowhere missed; and the search run twice with the history trimmed
//! between, which leaves its second search Windows Terminal's nowhere
//! range, missed by the search's calls.

mod common;
#[path = "common/harness.rs"]
mod harness;

use common::outpost::OutpostUnderTest;

use verbatim_core::batch::Builder;
use verbatim_core::terminal_window::{
    CurrentMark, EndMark, MarkPlace, MarkSearch, TerminalWindow, WindowAsk, add_steps, window_of,
};
use verbatim_model::batch::{Done, Op, Placement, Step, StepOutcome, Subject};
use verbatim_model::request::{Outcome, Question, Reply, WindowVerdict};
use verbatim_model::{NodeId, TerminalColourRun, TerminalText, TextAnchor, TextDetails};
use verbatim_process::arguments::OutpostOptions;
use verbatim_uia::Uia;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::IUIAutomationElement;

/// The terminal's width in cells.
const WIDTH: usize = 12;

/// `rows` as a terminal's text, each padded to [`WIDTH`] and ended by a
/// carriage return and line feed, escaped for `set-text`; and the UTF-16
/// offset of the end of the last row's text, where the caret is.
fn terminal_text(rows: &[&str]) -> (String, usize) {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:WIDTH$}\r\n")).collect();
    let before: usize = padded[..rows.len() - 1]
        .iter()
        .map(|row| row.encode_utf16().count())
        .sum();
    let caret = before + rows.last().map_or(0, |row| row.encode_utf16().count());
    (
        padded.concat().replace('\r', "\\r").replace('\n', "\\n"),
        caret,
    )
}

/// Shows `rows` in the terminal, the caret at the end of the last row's
/// text.
fn show(app: &mut common::MockApp, hwnd: HWND, rows: &[&str]) {
    let (text, caret) = terminal_text(rows);
    common::apply(app, hwnd, &format!("set-text term {text}"));
    common::apply(app, hwnd, &format!("caret term {caret} {caret}"));
}

/// Rows `prefix 1` to `prefix last`.
fn numbered(prefix: &str, range: std::ops::RangeInclusive<u32>) -> Vec<String> {
    range.map(|number| format!("{prefix} {number}")).collect()
}

/// `rows` as the window read answers them, padding removed by the model's
/// own conversion.
fn unpadded(rows: &[&str]) -> TerminalText {
    let padded: Vec<String> = rows.iter().map(|row| format!("{row:WIDTH$}\r\n")).collect();
    TerminalText::unpadded(&padded.concat())
}

/// A terminal under test, with an outpost reading it by remote
/// operations and one reading it by calls, each holding the terminal's
/// node.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    outposts: Vec<(OutpostUnderTest, NodeId)>,
}

/// The brand of the generic steps' batches.
enum WindowSteps {}

/// The end a read found: the anchor each outpost holds for it.
struct Ends {
    anchors: Vec<TextAnchor>,
}

/// What a test asks a read: its marks by the ends earlier reads found.
struct Asked<'a> {
    end: Option<(&'a Ends, &'a str, bool)>,
    aside: Option<&'a Ends>,
    height: u32,
    colours: bool,
}

/// An outpost reading mockapp's terminal by remote operations when
/// `remote`, and the terminal's node, minted by its read of the focus.
fn generic_outpost(
    app: &common::MockApp,
    element: &IUIAutomationElement,
    hwnd: HWND,
    remote: bool,
) -> (OutpostUnderTest, NodeId) {
    let outpost = OutpostUnderTest::with_options(
        app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    outpost.read_focus_as(element);
    let (focus, _) = outpost.batch(
        hwnd,
        WindowVerdict::Uia,
        verbatim_model::batch::Batch {
            steps: vec![Step {
                op: Op::FocusRead {
                    subject: Subject::Focus,
                    known: Vec::new(),
                    set: common::outpost::focus_set(),
                    details: TextDetails::NONE,
                },
                when: None,
            }],
            placement: Placement::Queue,
            spare: None,
        },
    );
    let Outcome::Result(Reply::Steps(steps)) = focus.outcome else {
        panic!("the focus read's steps, not {:?}", focus.outcome)
    };
    let Some(StepOutcome::Done(Done::FocusRead(read))) = steps.first() else {
        panic!("the focus read, not {steps:?}")
    };
    let node = read.focus.read.node.expect("the terminal's node");
    (outpost, node)
}

impl Terminal {
    /// mockapp's terminal of `fixture`, its screen `screen` rows high.
    fn start(fixture: &str, name: &str, screen: u32) -> Self {
        common::init_com();
        let title = common::unique_title(name);
        let mut app = common::spawn(fixture, "uia", &title);
        let hwnd = common::find_window(&title);
        let uia = Uia::new().expect("a UIA client");
        let element = common::named_element(&uia, hwnd, "Terminal");
        common::apply(&mut app, hwnd, &format!("screen term {screen}"));
        app.send("set-focus term");
        let outposts = [true, false]
            .into_iter()
            .map(|remote| generic_outpost(&app, &element, hwnd, remote))
            .collect();
        Self {
            app,
            hwnd,
            outposts,
        }
    }

    fn show(&mut self, rows: &[String]) {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        show(&mut self.app, self.hwnd, &rows);
    }

    /// The read `asked` names, by the generic steps through outpost
    /// `which`: the window Core makes of their outcomes.
    fn generic(&self, which: usize, asked: &Asked<'_>) -> TerminalWindow {
        let (outpost, node) = &self.outposts[which];
        let ask = WindowAsk {
            end: asked.end.map(|(ends, line, find)| CurrentMark {
                mark: EndMark {
                    anchor: ends.anchors[which],
                    line: line.to_owned(),
                    line_above: 0,
                },
                find,
            }),
            aside: asked.aside.map(|ends| EndMark {
                anchor: ends.anchors[which],
                line: String::new(),
                line_above: 0,
            }),
            height: asked.height,
            opening: 2,
            colours: asked.colours,
        };
        let mut steps = Builder::<WindowSteps>::named();
        let colours = asked.colours.then(|| {
            (
                asked.height,
                u32::try_from(WIDTH).expect("a narrow terminal"),
            )
        });
        add_steps(&mut steps, *node, &ask, colours);
        let Question::Batch(batch) = steps.finish().question();
        let (answer, calls) = outpost.batch(self.hwnd, WindowVerdict::Uia, batch);
        // By remote operations, a read with no search is one program, and
        // the colours one more: no step fell back to calls.
        if which == 0 && !asked.end.is_some_and(|(_, _, find)| find) {
            assert_eq!(
                calls.uia,
                1 + u32::from(asked.colours),
                "the generic read's programs"
            );
        }
        let Outcome::Result(Reply::Steps(outcomes)) = answer.outcome else {
            panic!("the window read's steps, not {:?}", answer.outcome)
        };
        window_of(&outcomes, &ask, 0)
            .unwrap_or_else(|unread| panic!("the window, not {unread:?} of {outcomes:#?}"))
    }

    /// The read `asked` names through both outposts, which must answer
    /// alike: the fields of the window Core makes, and the ends found.
    fn both(&self, asked: &Asked<'_>) -> (Fields, Ends) {
        let remote = self.generic(0, asked);
        let calls = self.generic(1, asked);
        let fields = Fields::of_window(&remote);
        assert_eq!(fields, Fields::of_window(&calls), "remote and calls differ");
        (
            fields,
            Ends {
                anchors: vec![remote.end, calls.end],
            },
        )
    }
}

/// The fields a test checks of a window, both outposts' alike: all but its
/// anchors and its times, the caret's line, without its padding, and the
/// caret's place in it kept.
#[derive(Debug, PartialEq, Eq)]
struct Fields {
    rows: TerminalText,
    above: u32,
    screen_top: u32,
    screen_rows: u32,
    width: u32,
    screen_at_start: bool,
    caret_rows_to_end: Option<u32>,
    first_row: TerminalText,
    document_rows: u32,
    end_place: Option<MarkPlace>,
    aside_place: Option<MarkPlace>,
    found: MarkSearch,
    colours: Option<Vec<Vec<TerminalColourRun>>>,
    caret: Option<(String, usize)>,
}

impl Fields {
    /// The fields of the window Core made of the generic steps' outcomes.
    fn of_window(window: &TerminalWindow) -> Self {
        Self {
            rows: window.rows.clone(),
            above: window.above,
            screen_top: window.screen_top,
            screen_rows: window.screen_rows,
            width: window.width,
            screen_at_start: window.screen_at_start,
            caret_rows_to_end: window.caret_rows_to_end,
            first_row: window.first_row.clone(),
            document_rows: window.document_rows,
            end_place: window.end_place.clone(),
            aside_place: window.aside_place.clone(),
            found: window.found,
            colours: window.colours.clone(),
            caret: window.caret.as_ref().map(|caret| {
                let line = &caret.report.line;
                (
                    line.text.clone(),
                    usize::try_from(line.offset).expect("an offset"),
                )
            }),
        }
    }
}

/// A read naming no mark, of a screen `height` rows high as last read.
fn plain(height: u32) -> Asked<'static> {
    Asked {
        end: None,
        aside: None,
        height,
        colours: false,
    }
}

/// One row's colours where the colour fixture paints none: black on white.
fn plain_row() -> Vec<TerminalColourRun> {
    vec![TerminalColourRun {
        column: 0,
        length: 12,
        foreground: 0,
        background: 0x00FF_FFFF,
    }]
}

/// The caret's line, the last row's, and the caret at its text's end.
fn caret_on(row: &str) -> (String, usize) {
    (format!("{row:WIDTH$}\r\n"), row.len())
}

/// A first read starts at the screen's top; after output, a read from two
/// screens above the document's end counts the rows written past each
/// mark it names and gives the two opening rows after them, and reads the
/// colours of every screen row; after
/// a clear, both marks lie past the document's end and are refused.
fn a_read_counts_the_rows_past_its_marks_and_reads_the_screens_colours_both_ways() {
    let mut terminal = Terminal::start("terminal-colours.json", "mockapp-window-marks", 4);
    let mut rows = numbered("line", 1..=8);
    rows.push("ready>".to_owned());
    terminal.show(&rows);
    let (first, first_end) = terminal.both(&plain(0));
    assert_eq!(
        first,
        Fields {
            rows: unpadded(&["line 7", "line 8", "ready>"]),
            above: 3,
            screen_top: 3,
            screen_rows: 3,
            width: 12,
            screen_at_start: false,
            caret_rows_to_end: Some(1),
            first_row: unpadded(&["line 1"]),
            document_rows: 9,
            end_place: None,
            aside_place: None,
            found: MarkSearch::NotAsked,
            colours: None,
            caret: Some(caret_on("ready>")),
        }
    );

    let mut rows = numbered("line", 1..=28);
    rows.push("ready>".to_owned());
    terminal.show(&rows);
    let ask = Asked {
        end: Some((&first_end, "ready>", false)),
        aside: Some(&first_end),
        height: first.screen_rows,
        colours: true,
    };
    let (second, second_end) = terminal.both(&ask);
    let past = MarkPlace::Rows {
        written: 20,
        opening: Some(unpadded(&["line 10", "line 11"])),
    };
    assert_eq!(
        second,
        Fields {
            rows: unpadded(&[
                "line 24", "line 25", "line 26", "line 27", "line 28", "ready>"
            ]),
            above: 6,
            screen_top: 3,
            screen_rows: 3,
            width: 12,
            screen_at_start: false,
            caret_rows_to_end: Some(1),
            first_row: unpadded(&["line 1"]),
            document_rows: 29,
            end_place: Some(past.clone()),
            aside_place: Some(past),
            found: MarkSearch::NotAsked,
            colours: Some(vec![plain_row(), plain_row(), plain_row()]),
            caret: Some(caret_on("ready>")),
        }
    );

    terminal.show(&["ready>".to_owned()]);
    let ask = Asked {
        end: Some((&second_end, "ready>", false)),
        aside: Some(&first_end),
        height: second.screen_rows,
        colours: false,
    };
    let (cleared, _) = terminal.both(&ask);
    assert_eq!(
        cleared,
        Fields {
            rows: unpadded(&["ready>"]),
            above: 1,
            screen_top: 1,
            screen_rows: 1,
            width: 12,
            screen_at_start: true,
            caret_rows_to_end: Some(1),
            first_row: unpadded(&["ready>"]),
            document_rows: 1,
            end_place: Some(MarkPlace::Refused),
            aside_place: Some(MarkPlace::Refused),
            found: MarkSearch::NotAsked,
            colours: None,
            caret: Some(caret_on("ready>")),
        }
    );
    terminal.app.quit();
}

/// A line longer than the width wraps over two rows, which the provider's
/// line unit moves over one at a time, as a terminal's does: the window,
/// the screen's top, the caret's rows to the end and the document's rows
/// count rows, the width is a row's and not the wrapped line's, and the
/// colours are read for each screen row, the wrapped line's two rows each
/// its own. The screen of four rows shows three, as mockapp counts the
/// empty end after the last line break as a row.
fn a_wrapped_lines_rows_are_counted_and_coloured_each_its_own_both_ways() {
    let mut terminal = Terminal::start("terminal-colours.json", "mockapp-window-wrap", 4);
    common::apply(
        &mut terminal.app,
        terminal.hwnd,
        &format!("wrap term {WIDTH}"),
    );
    let wrapped = "abcdefghijklmnopqrstuvwx";
    let text = format!(
        "{:WIDTH$}\r\n{wrapped}\r\n{:WIDTH$}\r\n",
        "line 1", "ready>"
    );
    let escaped = text.replace('\r', "\\r").replace('\n', "\\n");
    common::apply(
        &mut terminal.app,
        terminal.hwnd,
        &format!("set-text term {escaped}"),
    );
    let caret = WIDTH + 2 + wrapped.len() + 2 + "ready>".len();
    common::apply(
        &mut terminal.app,
        terminal.hwnd,
        &format!("caret term {caret} {caret}"),
    );
    let ask = Asked {
        colours: true,
        ..plain(3)
    };
    let (read, _) = terminal.both(&ask);
    // The fixture's red, at the text's units 21 and 22, falls on the
    // wrapped line's first row, columns 7 and 8, and not on its second.
    let run = |column, length, foreground| TerminalColourRun {
        column,
        length,
        foreground,
        background: 0x00FF_FFFF,
    };
    let red_at_seven = vec![run(0, 7, 0), run(7, 2, 255), run(9, 3, 0)];
    assert_eq!(
        read,
        Fields {
            rows: TerminalText::unpadded(&text),
            above: 4,
            screen_top: 3,
            screen_rows: 3,
            width: 12,
            screen_at_start: false,
            caret_rows_to_end: Some(1),
            first_row: unpadded(&["line 1"]),
            document_rows: 4,
            end_place: None,
            aside_place: None,
            found: MarkSearch::NotAsked,
            colours: Some(vec![red_at_seven, plain_row(), plain_row()]),
            caret: Some(caret_on("ready>")),
        }
    );
    terminal.app.quit();
}

/// The rows of the search tests: a command's line, nine rows of output and
/// the prompt.
fn command_rows(first: u32) -> Vec<String> {
    let mut rows = vec!["cmd 1".to_owned()];
    rows.extend(numbered("a", first..=9));
    rows.push("ready>".to_owned());
    rows
}

/// What the search for `line`, from a read naming `end` as its mark, found.
fn sought(terminal: &Terminal, end: &Ends, line: &str) -> MarkSearch {
    let ask = Asked {
        end: Some((end, line, true)),
        aside: None,
        height: 3,
        colours: false,
    };
    terminal.both(&ask).0.found
}

/// In a provider whose `FindText` answers a miss with a range, as Windows
/// Terminal's does, the search for a mark's line meets both kinds of miss
/// and answers by classic calls: the line above the window is found, with
/// the empty span below the document's end answering the elsewhere range;
/// a line the window holds is not sought; and a line nowhere answers the
/// nowhere range and is missed.
fn the_search_for_a_marks_line_answers_both_kinds_of_miss_both_ways() {
    let mut terminal = Terminal::start(
        "terminal_find_misses_answer_a_range.json",
        "mockapp-window-find",
        3,
    );
    terminal.show(&command_rows(1));
    let (_, end) = terminal.both(&plain(3));
    assert_eq!(sought(&terminal, &end, "cmd 1"), MarkSearch::Found(11));
    assert_eq!(sought(&terminal, &end, "a 8"), MarkSearch::InWindow);
    assert_eq!(sought(&terminal, &end, "zzz"), MarkSearch::Missed);
    terminal.app.quit();
}

/// The search for a mark's line run twice, the history trimmed between so
/// that the command's line is gone: the second search answers Windows
/// Terminal's nowhere range, whose calls would end a remote operation, and
/// is missed by the search's calls, which `FindText` alone makes.
fn a_search_after_the_history_is_trimmed_is_missed_by_calls() {
    let mut terminal = Terminal::start(
        "terminal_find_misses_answer_a_range.json",
        "mockapp-window-trimmed",
        3,
    );
    terminal.show(&command_rows(1));
    let (_, end) = terminal.both(&plain(3));
    let search = |terminal: &Terminal, end: &Ends| {
        terminal.both(&Asked {
            end: Some((end, "cmd 1", true)),
            aside: None,
            height: 3,
            colours: false,
        })
    };
    let (first, first_end) = search(&terminal, &end);
    assert_eq!(first.found, MarkSearch::Found(11));
    let mut trimmed = command_rows(3);
    trimmed.remove(0);
    terminal.show(&trimmed);
    common::reset_hits(terminal.hwnd);
    let (second, _) = search(&terminal, &first_end);
    assert_eq!(second.found, MarkSearch::Missed);
    let faults = common::read_hits(terminal.hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "RPC_E_SERVERFAULT").then_some(count))
        .unwrap_or(0);
    terminal.app.quit();
    assert!(faults > 0, "the second search met the nowhere range");
}

/// Runs this file's tests through the UIA test runner (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "a_read_counts_the_rows_past_its_marks_and_reads_the_screens_colours_both_ways",
            a_read_counts_the_rows_past_its_marks_and_reads_the_screens_colours_both_ways,
        ),
        (
            "a_wrapped_lines_rows_are_counted_and_coloured_each_its_own_both_ways",
            a_wrapped_lines_rows_are_counted_and_coloured_each_its_own_both_ways,
        ),
        (
            "the_search_for_a_marks_line_answers_both_kinds_of_miss_both_ways",
            the_search_for_a_marks_line_answers_both_kinds_of_miss_both_ways,
        ),
        (
            "a_search_after_the_history_is_trimmed_is_missed_by_calls",
            a_search_after_the_history_is_trimmed_is_missed_by_calls,
        ),
    ]);
}
