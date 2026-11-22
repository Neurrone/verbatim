//! A terminal's window read (`verbatim_model::batch::Op::TerminalRead`,
//! `verbatim-outpost`'s `terminal::window` on `verbatim-uia-rops`'s
//! `terminal_read`) against mockapp's UIA text provider, whose `set-text`
//! command rewrites the terminal's text as a terminal's buffer changes,
//! every row padded to the width and ended by a carriage return and line
//! feed, and whose `screen` command puts only the text's last rows on
//! screen. Each read is taken by the remote program and by the classic
//! calls, which must answer alike, and each answer is pinned whole.
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
//! range, answered with no remote operation falling back.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::{Arc, Mutex};

use verbatim_model::{MarkPlace, MarkSearch, TerminalColourRun, TerminalText};
use verbatim_outpost::terminal::{WindowAnswer, WindowRead, window};
use verbatim_uia::Uia;
use verbatim_uia_rops::CaretLineQuery;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};

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

/// A terminal under test.
struct Terminal {
    app: common::MockApp,
    hwnd: HWND,
    uia: Uia,
    element: IUIAutomationElement,
    pattern: IUIAutomationTextPattern,
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
        let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
        common::apply(&mut app, hwnd, &format!("screen term {screen}"));
        Self {
            app,
            hwnd,
            uia,
            element,
            pattern,
        }
    }

    fn show(&mut self, rows: &[String]) {
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        show(&mut self.app, self.hwnd, &rows);
    }

    /// The read `ask` names, by the remote program when `remote`.
    fn read(&self, ask: &WindowRead<'_>, remote: bool) -> WindowAnswer {
        let caret = CaretLineQuery {
            element: &self.element,
            pattern: &self.pattern,
            max_text: 4096,
            since: None,
        };
        window(
            &self.uia,
            (&self.element, &self.pattern),
            caret,
            ask,
            (remote, None),
        )
        .expect("the window is read")
    }

    /// The read `ask` names both ways, which must answer alike: the
    /// answer's fields, and the end the remote read found.
    fn both(&self, ask: &WindowRead<'_>) -> (Fields, IUIAutomationTextRange) {
        let remote = self.read(ask, true);
        let classic = self.read(ask, false);
        let fields = Fields::from(&remote);
        assert_eq!(fields, Fields::from(&classic), "remote and classic differ");
        (fields, remote.end)
    }
}

/// The fields a test checks of a read, both paths' alike: all but its
/// ranges, its times and the caret's range, of which the caret's line and
/// the caret's place in it are kept.
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

impl From<&WindowAnswer> for Fields {
    fn from(answer: &WindowAnswer) -> Self {
        assert!(
            answer.began_us <= answer.ended_us,
            "the read ends after it begins"
        );
        Self {
            rows: answer.rows.clone(),
            above: answer.above,
            screen_top: answer.screen_top,
            screen_rows: answer.screen_rows,
            width: answer.width,
            screen_at_start: answer.screen_at_start,
            caret_rows_to_end: answer.caret_rows_to_end,
            first_row: answer.first_row.clone(),
            document_rows: answer.document_rows,
            end_place: answer.end_place.clone(),
            aside_place: answer.aside_place.clone(),
            found: answer.found,
            colours: answer.colours.clone(),
            caret: answer.caret.as_ref().map(|caret| {
                (
                    String::from_utf16_lossy(&caret.line.text),
                    caret.line.offset,
                )
            }),
        }
    }
}

/// A read naming no mark, of a screen `height` rows high as last read.
fn plain(height: u32) -> WindowRead<'static> {
    WindowRead {
        end: None,
        aside: None,
        height,
        opening: 2,
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
    let ask = WindowRead {
        end: Some((&first_end, "ready>", false)),
        aside: Some(&first_end),
        height: first.screen_rows,
        opening: 2,
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
    let ask = WindowRead {
        end: Some((&second_end, "ready>", false)),
        aside: Some(&first_end),
        height: second.screen_rows,
        opening: 2,
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
    let ask = WindowRead {
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
fn sought(terminal: &Terminal, end: &IUIAutomationTextRange, line: &str) -> MarkSearch {
    let ask = WindowRead {
        end: Some((end, line, true)),
        aside: None,
        height: 3,
        opening: 2,
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

/// The targets of the log lines a run wrote.
#[derive(Default)]
struct Targets(Mutex<Vec<String>>);

impl tracing::Subscriber for Targets {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let target = event.metadata().target();
        if target.starts_with("verbatim::") {
            self.0.lock().expect("the log").push(target.to_owned());
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// The search run by the remote read's caller twice, the history trimmed
/// between so that the command's line is gone: the second search answers
/// Windows Terminal's nowhere range, whose calls would end a remote
/// operation, and is missed by the classic calls, with no fallback logged.
fn a_search_after_the_history_is_trimmed_falls_back_nowhere() {
    let mut terminal = Terminal::start(
        "terminal_find_misses_answer_a_range.json",
        "mockapp-window-trimmed",
        3,
    );
    terminal.show(&command_rows(1));
    let end = terminal.read(&plain(3), true).end;
    let targets = Arc::new(Targets::default());
    let search = |terminal: &Terminal, end: &IUIAutomationTextRange| {
        let ask = WindowRead {
            end: Some((end, "cmd 1", true)),
            aside: None,
            height: 3,
            opening: 2,
            colours: false,
        };
        tracing::subscriber::with_default(Arc::clone(&targets), || terminal.read(&ask, true))
    };
    let first = search(&terminal, &end);
    assert_eq!(first.found, MarkSearch::Found(11));
    let mut trimmed = command_rows(3);
    trimmed.remove(0);
    terminal.show(&trimmed);
    common::reset_hits(terminal.hwnd);
    let second = search(&terminal, &first.end);
    assert_eq!(second.found, MarkSearch::Missed);
    let faults = common::read_hits(terminal.hwnd)
        .into_iter()
        .find_map(|(name, count)| (name == "RPC_E_SERVERFAULT").then_some(count))
        .unwrap_or(0);
    terminal.app.quit();
    assert!(faults > 0, "the second search met the nowhere range");
    assert_eq!(
        *targets.0.lock().expect("the log"),
        Vec::<String>::new(),
        "nothing fell back"
    );
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
            "a_search_after_the_history_is_trimmed_falls_back_nowhere",
            a_search_after_the_history_is_trimmed_falls_back_nowhere,
        ),
    ]);
}
