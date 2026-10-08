//! A terminal's new output (milestone M4 item 9; `phase6-design.md`,
//! "Terminal reading by diffing the screen") against mockapp's UIA text
//! provider, whose `set-text` command rewrites a node's text the way a
//! terminal's buffer changes (lines written at the end, the oldest lines
//! discarded, the screen cleared) and whose `screen` command puts only the
//! text's last lines on screen, as a terminal shows the end of its buffer.
//! Each test drives `verbatim-uia-rops`'s `terminal_screen` and
//! `verbatim-outpost`'s terminal module on this thread exactly as the
//! outpost's worker does once it has the focused terminal's text pattern,
//! since the worker finds a UIA focus by reading the system's keyboard
//! focus, which a test must not take from the desktop it runs on.
//!
//! What is checked: the remote program and the classic implementation
//! agree on every kind of read; the anchor is found again by its text,
//! past rows that only contain it and rows not paired as it was, and a
//! provider whose `FindText` fails finds it nowhere; the outpost reports a
//! line that grew, new lines, a flood's first lines and the count of the
//! rest, a history that overflowed past the anchor, a redraw with the same
//! text, and a cleared screen; a terminal read costs exactly what
//! `docs/performance.md` records, by client calls and provider hits,
//! remotely and classically; and a terminal whose text changes with no
//! caret event, as the console host's does while typing, still has Core's
//! Backspace say exactly what it deleted, since the caret is read with the
//! text.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CallCounts, CaretKey, CaretMotion, CaretReport, Effect, Input, LineChange,
    NodeDetails, NodeId, NodeSnapshot, NormalizedEvent, OutpostId, Pid, Role, SegmentContent,
    Skipped, StateSet, TerminalOutput, TextOp, TraceId,
};
use verbatim_outpost::terminal::{Found, ReadMode, ScreenText, Terminal, read};
use verbatim_outpost::text::uia::UiaText;
use verbatim_outpost::text::{
    Anchors, CaretSignal, NodeText, Watched, caret_report_from, check_caret, perform,
};
use verbatim_uia::{NodeIdRegistry, Uia};
use verbatim_uia_rops::{
    CaretLineQuery, ScreenAnchor, ScreenQuery, terminal_screen_classic, terminal_screen_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationTextPattern};

/// How many of the lines that went by unread a read takes from their
/// first.
const WANTED: u32 = 3;

/// mockapp's terminal's text pattern.
fn terminal_text(uia: &Uia, hwnd: HWND) -> (IUIAutomationElement, IUIAutomationTextPattern) {
    let cache = uia.base_cache_request().expect("a cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let (tree, _) = uia
        .walk_tree(&root, &cache, &registry, 8, 64)
        .expect("mockapp's tree");
    let terminal = tree
        .children
        .iter()
        .find(|child| child.snapshot.name.as_deref() == Some("Terminal"))
        .expect("the terminal")
        .snapshot
        .id;
    let element = registry
        .element_of(terminal)
        .and_then(|agile| agile.resolve().ok())
        .expect("its element");
    let pattern = verbatim_uia::text::text_pattern(&element)
        .expect("a text pattern")
        .0;
    (element, pattern)
}

/// Reads the screen both ways and checks they agree; returns the answer.
fn both(uia: &Uia, query: &ScreenQuery<'_>) -> ScreenText {
    let remote = terminal_screen_remote(uia, query).expect("the remote program runs");
    let classic = terminal_screen_classic(uia, query).expect("the classic reads run");
    let remote = ScreenText::from(&remote);
    assert_eq!(remote, ScreenText::from(&classic));
    remote
}

/// A query with `anchor`, the old screen's top two rows.
fn query<'a>(
    text: (&'a IUIAutomationElement, &'a IUIAutomationTextPattern),
    anchor: Option<(&'a str, &'a str)>,
    seen_rows: u32,
) -> ScreenQuery<'a> {
    ScreenQuery {
        element: text.0,
        pattern: text.1,
        anchor: anchor.map(|(top, next)| ScreenAnchor {
            top,
            next,
            range: None,
            first: "",
            no_history: false,
        }),
        matches_padding: false,
        seen_rows,
        head_wanted: WANTED,
        caret: None,
    }
}

fn texts(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|&line| line.to_owned()).collect()
}

/// `count` numbered lines from `first`, `prefix 000` on, as mockapp's
/// `set-text` takes them, each line feed escaped.
fn numbered(prefix: &str, numbers: std::ops::Range<u32>) -> String {
    use std::fmt::Write as _;
    numbers.fold(String::new(), |mut text, number| {
        let _ = write!(text, r"{prefix} {number:03}\n");
        text
    })
}

fn remote_and_classic_screen_reads_agree() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-agree");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, pattern) = terminal_text(&uia, hwnd);
    let text = (&element, &pattern);
    common::apply(&mut app, hwnd, "screen term 4");

    // A first read: the last four lines, with history above them.
    let first = both(&uia, &query(text, None, 0));
    assert_eq!(first.text, "three\nfour\nfive\nready>");
    assert_eq!(
        (first.top_row.as_str(), first.next_row.as_str()),
        ("three\n", "four\n")
    );
    assert!(!first.alternate);
    assert_eq!((first.shift, first.document_rows), (None, None));
    assert!(first.settled);

    // Ten lines later: the old top row lies ten rows above the screen, and
    // the first three of the six that went by unread are read.
    common::apply(
        &mut app,
        hwnd,
        &format!(
            r"set-text term one\ntwo\nthree\nfour\nfive\nready>\n{}ready>",
            numbered("line", 0..9)
        ),
    );
    let later = both(&uia, &query(text, Some(("three\n", "four\n")), 4));
    assert_eq!(later.shift, Some(10));
    assert_eq!(later.head_rows, 3);
    assert_eq!(later.head, "line 000\nline 001\nline 002\n");
    assert_eq!(later.text, "line 006\nline 007\nline 008\nready>");
    assert_eq!(later.document_rows, None);

    // The anchor gone from the text: the rows of the whole text are
    // counted.
    common::apply(
        &mut app,
        hwnd,
        &format!(r"set-text term {}ready>", numbered("x", 0..7)),
    );
    let gone = both(&uia, &query(text, Some(("three\n", "four\n")), 4));
    assert_eq!(gone.shift, None);
    assert_eq!(gone.document_rows, Some(8));

    // A screen that holds the whole text has no history above it.
    common::apply(&mut app, hwnd, "screen term 0");
    let whole = both(&uia, &query(text, None, 0));
    assert!(whole.alternate);
    app.quit();
}

/// The anchor's row found past rows that only contain its text and a row
/// that holds it with another row beside it than the anchor's partner.
fn the_anchor_is_found_by_its_text_and_its_partner() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-partner");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, pattern) = terminal_text(&uia, hwnd);
    let text = (&element, &pattern);
    common::apply(&mut app, hwnd, "screen term 2");
    common::apply(
        &mut app,
        hwnd,
        r"set-text term build done\nok\nbuild done!\nbuild done\nnope\nlast\nready>",
    );
    // "build done" over "ok" is the pair; the nearer "build done" lies over
    // "nope", and "build done!" only contains the text.
    let found = both(&uia, &query(text, Some(("build done\n", "ok\n")), 2));
    assert_eq!(found.shift, Some(5));
    app.quit();
}

/// A provider whose `FindText` fails (`terminal_find_fails.json`), as
/// Windows Terminal's has: the anchor is not found, and the rows are
/// counted, both ways.
fn a_failing_find_text_finds_nothing() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-fails");
    let mut app = common::spawn("terminal_find_fails.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, pattern) = terminal_text(&uia, hwnd);
    let text = (&element, &pattern);
    common::apply(&mut app, hwnd, "screen term 2");
    let read = both(&uia, &query(text, Some(("two\n", "three\n")), 2));
    assert_eq!((read.shift, read.document_rows), (None, Some(6)));
    app.quit();
}

/// Calls by kind, in the order `CallCounts` lists them.
fn uia_calls(uia: u32) -> CallCounts {
    CallCounts {
        uia,
        msaa: 0,
        window_messages: 0,
    }
}

/// One terminal read through the outpost's terminal module, with its client
/// calls and mockapp's provider hits.
fn measured_read(
    uia: &Uia,
    hwnd: HWND,
    text: (&IUIAutomationElement, &IUIAutomationTextPattern),
    terminal: &mut Terminal,
    (remote, mode): (bool, ReadMode),
) -> (TerminalOutput, CallCounts, Vec<(&'static str, u32)>) {
    common::reset_hits(hwnd);
    let _ = verbatim_uia::calls::take();
    let answer =
        read(uia, text, None, terminal, (WANTED, remote, mode)).expect("the terminal reads");
    let calls = verbatim_uia::calls::take();
    let Found::Output(output, _) = answer.found else {
        panic!("the read settles");
    };
    (output, calls, common::read_hits(hwnd))
}

/// The output of a terminal's reads, and the cost of each read, pinned
/// exactly.
#[expect(
    clippy::too_many_lines,
    reason = "each read and its pinned cost checked in turn against one running mockapp"
)]
fn terminal_reads_report_new_output_and_cost_exactly(remote: bool) {
    common::init_com();
    let title = common::unique_title(if remote {
        "mockapp-terminal-remote"
    } else {
        "mockapp-terminal-classic"
    });
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, pattern) = terminal_text(&uia, hwnd);
    let text = (&element, &pattern);
    let mut terminal = Terminal::default();
    common::apply(&mut app, hwnd, "screen term 4");
    let measure =
        |terminal: &mut Terminal, mode| measured_read(&uia, hwnd, text, terminal, (remote, mode));

    // The baseline, when the terminal gains the focus: nothing is new.
    let (output, baseline_calls, baseline_hits) = measure(&mut terminal, ReadMode::Baseline);
    assert_eq!(output, TerminalOutput::default());

    // The prompt grows as the user types.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls",
    );
    let (output, typed_calls, typed_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(
        output,
        TerminalOutput {
            changed: Some(LineChange {
                text: " ls".to_owned(),
                line: "ready> ls".to_owned(),
                appended: true,
                uncertain: 1,
                inserted: " ls".to_owned(),
            }),
            ..TerminalOutput::default()
        }
    );

    // A command's output: one line and the prompt, scrolling the screen by
    // two rows.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\nnotes.txt\nready>",
    );
    let (output, line_calls, line_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(
        output,
        TerminalOutput {
            lines: texts(&["notes.txt", "ready>"]),
            ..TerminalOutput::default()
        }
    );

    // More than the screen holds: the first of the lines that went by
    // unread are read, the rest counted, and the screen's lines are new.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\nnotes.txt\nready> dir\n1\n2\n3\n4\n5\n6\n7\n8\nready>",
    );
    let (output, flood_calls, flood_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(
        output,
        TerminalOutput {
            changed: Some(LineChange {
                text: " dir".to_owned(),
                line: "ready> dir".to_owned(),
                appended: true,
                uncertain: 1,
                inserted: " dir".to_owned(),
            }),
            head: texts(&["1", "2", "3"]),
            skipped: Some(Skipped::Count(2)),
            lines: texts(&["6", "7", "8", "ready>"]),
            ..TerminalOutput::default()
        }
    );

    // A redraw with the same text: nothing.
    let (output, redraw_calls, redraw_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(output, TerminalOutput::default());

    // So much output that the anchor left the history: more than the
    // history's lines not spoken went by.
    common::apply(
        &mut app,
        hwnd,
        &format!(r"set-text term {}ready>", numbered("flood", 0..11)),
    );
    let (output, overflow_calls, overflow_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(
        output,
        TerminalOutput {
            skipped: Some(Skipped::MoreThan(8)),
            lines: texts(&["flood 008", "flood 009", "flood 010", "ready>"]),
            ..TerminalOutput::default()
        }
    );

    // The screen cleared and written to: no history above it, compared
    // line by line.
    common::apply(&mut app, hwnd, r"set-text term hello\nready>");
    let (output, cleared_calls, cleared_hits) = measure(&mut terminal, ReadMode::Change);
    assert_eq!(
        output,
        TerminalOutput {
            above: texts(&["hello"]),
            ..TerminalOutput::default()
        }
    );
    app.quit();

    let costs = [
        ("baseline", baseline_calls, baseline_hits),
        ("typed", typed_calls, typed_hits),
        ("output line", line_calls, line_hits),
        ("flood", flood_calls, flood_hits),
        ("redraw", redraw_calls, redraw_hits),
        ("overflow", overflow_calls, overflow_hits),
        ("cleared", cleared_calls, cleared_hits),
    ];
    // The client's calls: remotely, one program for each read; classically,
    // one call per provider method. The provider does the same work either
    // way, the remote program's import of the element and its text pattern
    // aside.
    let expected: [(&str, CallCounts, Hits); 7] = if remote {
        [
            ("baseline", uia_calls(1), REMOTE_BASELINE_HITS),
            ("typed", uia_calls(1), REMOTE_FOUND_HITS),
            ("output line", uia_calls(1), REMOTE_FOUND_HITS),
            ("flood", uia_calls(1), REMOTE_FLOOD_HITS),
            ("redraw", uia_calls(1), REMOTE_FOUND_HITS),
            ("overflow", uia_calls(1), REMOTE_OVERFLOW_HITS),
            ("cleared", uia_calls(1), REMOTE_CLEARED_HITS),
        ]
    } else {
        [
            ("baseline", uia_calls(BASELINE_CALLS), BASELINE_HITS),
            ("typed", uia_calls(FOUND_CALLS), FOUND_HITS),
            ("output line", uia_calls(FOUND_CALLS), FOUND_HITS),
            ("flood", uia_calls(FLOOD_CALLS), FLOOD_HITS),
            ("redraw", uia_calls(FOUND_CALLS), FOUND_HITS),
            ("overflow", uia_calls(OVERFLOW_CALLS), OVERFLOW_HITS),
            ("cleared", uia_calls(CLEARED_CALLS), CLEARED_HITS),
        ]
    };
    let moved: Vec<String> = costs
        .iter()
        .zip(expected)
        .filter(|((_, calls, hits), (_, expected_calls, expected_hits))| {
            (*calls, hits.as_slice()) != (*expected_calls, *expected_hits)
        })
        .map(|((name, calls, hits), _)| format!("{name}: {} calls, {hits:?}", calls.uia))
        .collect();
    assert!(
        moved.is_empty(),
        "reads whose cost moved (when that is deliberate, update this test and \
         docs/performance.md together):\n{}",
        moved.join("\n")
    );
}

/// Provider hits by method.
type Hits = &'static [(&'static str, u32)];

fn terminal_reads_cost_exactly_remote() {
    terminal_reads_report_new_output_and_cost_exactly(true);
}

fn terminal_reads_cost_exactly_classic() {
    terminal_reads_report_new_output_and_cost_exactly(false);
}

/// A caret key's watch checked once with no caret event: the test moves
/// mockapp's caret before Core's request is answered, so the first read is
/// the evidence.
struct AlreadyMoved;

impl CaretSignal for AlreadyMoved {
    fn caret_event(&mut self) -> bool {
        false
    }

    fn now_ms(&mut self) -> u64 {
        0
    }
}

/// The node Core knows mockapp's terminal by.
fn terminal_node() -> NodeId {
    NodeId::in_outpost(OutpostId(1), 1)
}

/// Sets mockapp's terminal's text to `text`, its caret at the end, raising
/// no event: the console host's pattern while typing.
fn type_silently(app: &mut common::MockApp, hwnd: HWND, text: &str) {
    common::apply(
        app,
        hwnd,
        &format!("set-text term {}", text.replace('\n', "\\n")),
    );
    let end = text.encode_utf16().count();
    common::apply(app, hwnd, &format!("caret term {end} {end}"));
}

/// One read of the terminal's new output with its caret, as the outpost's
/// worker makes it on a change of the terminal's text, and the caret as a
/// report, as the worker sends it to Core.
fn read_output_and_caret(
    uia: &Uia,
    source: &mut UiaText,
    anchors: &mut NodeText<'_, verbatim_outpost::text::uia::UiaPos>,
    terminal: &mut Terminal,
    remote: bool,
) -> (TerminalOutput, CaretReport) {
    let caret = CaretLineQuery {
        element: source.element(),
        pattern: source.pattern(),
        pattern2: source.pattern2(),
        max_text: 4096,
    };
    let answer = read(
        uia,
        (source.element(), source.pattern()),
        Some(caret),
        terminal,
        (WANTED, remote, ReadMode::Change),
    )
    .expect("the terminal reads");
    let Found::Output(output, _) = answer.found else {
        panic!("the read settles");
    };
    let caret = answer.caret.expect("the caret is read with the text");
    let read = source.caret_read_from(caret).expect("the caret read");
    let report = caret_report_from(source, anchors, read, 0).expect("the caret report");
    (output, report)
}
/// The classic baseline's calls: the document and visible ranges, the
/// screen's text, its top two rows, whether history lies above it, and the
/// top row again at the end (the guard against text that moved).
const BASELINE_CALLS: u32 = 23;

/// The classic calls of a read that finds the anchor where its range is:
/// the baseline's, the rows at the range checked, the rows to the end
/// counted from the anchor and from the screen, and the old screen's last
/// row read where it is now.
const FOUND_CALLS: u32 = 41;

/// [`FOUND_CALLS`] and the first rows that went by unread.
const FLOOD_CALLS: u32 = 48;

/// The classic calls of a read whose anchor is gone: its range's rows read
/// otherwise, the search finds it nowhere, and the rows of the whole text
/// are counted.
const OVERFLOW_CALLS: u32 = 41;

/// The classic calls of a read of a cleared screen: the search finds
/// nothing, and the rows are counted.
const CLEARED_CALLS: u32 = 36;

const BASELINE_HITS: Hits = &[
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 7),
    ("CompareEndpoints", 1),
    ("ExpandToEnclosingUnit", 4),
    ("GetText", 5),
    ("Move", 1),
    ("MoveEndpointByRange", 3),
];

/// [`BASELINE_HITS`] remotely: the program also gets the text pattern from
/// the element, which the provider answers through the element's own
/// calls, and copies ranges it collapses and expands.
const REMOTE_BASELINE_HITS: Hits = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 9),
    ("CompareEndpoints", 1),
    ("ExpandToEnclosingUnit", 4),
    ("GetText", 5),
    ("Move", 1),
    ("MoveEndpointByRange", 4),
];

const FOUND_HITS: Hits = &[
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 13),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 6),
    ("Move", 5),
    ("MoveEndpointByRange", 8),
];

const REMOTE_FOUND_HITS: Hits = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 17),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 6),
    ("Move", 5),
    ("MoveEndpointByRange", 10),
];

const FLOOD_HITS: Hits = &[
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 15),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 7),
    ("Move", 7),
    ("MoveEndpointByRange", 10),
];

const REMOTE_FLOOD_HITS: Hits = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 19),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 7),
    ("Move", 7),
    ("MoveEndpointByRange", 12),
];

const OVERFLOW_HITS: Hits = &[
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 12),
    ("CompareEndpoints", 5),
    ("ExpandToEnclosingUnit", 6),
    ("FindText", 2),
    ("GetText", 5),
    ("Move", 2),
    ("MoveEndpointByRange", 7),
];

const REMOTE_OVERFLOW_HITS: Hits = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 15),
    ("CompareEndpoints", 5),
    ("ExpandToEnclosingUnit", 6),
    ("FindText", 2),
    ("GetText", 6),
    ("Move", 2),
    ("MoveEndpointByRange", 8),
];

const CLEARED_HITS: Hits = &[
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 11),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 5),
    ("FindText", 1),
    ("GetText", 5),
    ("Move", 2),
    ("MoveEndpointByRange", 6),
];

const REMOTE_CLEARED_HITS: Hits = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 14),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 5),
    ("FindText", 1),
    ("GetText", 5),
    ("Move", 2),
    ("MoveEndpointByRange", 7),
];

/// The classic calls of the typed read with the caret and its line read
/// too.
const TYPED_WITH_CARET_CALLS: u32 = 49;

const TYPED_WITH_CARET_HITS: Hits = &[
    ("ITextProvider::GetSelection", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 15),
    ("CompareEndpoints", 3),
    ("ExpandToEnclosingUnit", 6),
    ("GetText", 8),
    ("Move", 5),
    ("MoveEndpointByRange", 9),
];

/// The same remotely: the caret's own reads in the program get the text
/// pattern from the element again.
const REMOTE_TYPED_WITH_CARET_HITS: Hits = &[
    ("ProviderOptions", 4),
    ("GetPatternProvider", 2),
    ("GetPropertyValue", 2),
    ("HostRawElementProvider", 2),
    ("Navigate", 2),
    ("ITextProvider::GetSelection", 1),
    ("GetVisibleRanges", 1),
    ("DocumentRange", 1),
    ("Clone", 20),
    ("CompareEndpoints", 3),
    ("ExpandToEnclosingUnit", 6),
    ("GetText", 8),
    ("Move", 5),
    ("MoveEndpointByRange", 12),
];

/// The console host raises no caret event for every character typed, only
/// its text's change; Core's Backspace works out what it deleted from the
/// caret it last heard of before the key. The caret read with each change
/// of the text keeps that current: typing "helo" after the prompt and
/// pressing Backspace says exactly "o".
#[expect(
    clippy::too_many_lines,
    reason = "one typing session, read top to bottom as it happens"
)]
fn backspace_says_what_it_deleted_without_caret_events(remote: bool) {
    let title = common::unique_title("mockapp-terminal-backspace");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, _) = terminal_text(&uia, hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let mut source = UiaText::new(element, pattern, pattern2, true).remote(remote);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    let mut terminal = Terminal::default();
    let event = |observed_at_ms, event| Input::Event {
        trace_id: TraceId::mint(),
        observed_at_ms,
        source: Pid(1),
        backend: Backend::Uia,
        window: None,
        event,
    };

    // The prompt, its text read where it ends now, as a focus does.
    let mut text = "one\ntwo\nthree\nfour\nfive\nready>".to_owned();
    type_silently(&mut app, hwnd, &text);
    let _ = read(
        &uia,
        (source.element(), source.pattern()),
        None,
        &mut terminal,
        (WANTED, remote, ReadMode::Baseline),
    )
    .expect("the baseline read");
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &event(
            0,
            NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: terminal_node(),
                    backend: Backend::Uia,
                    role: Role::Terminal,
                    name: Some("Terminal".to_owned()),
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
            },
        ),
    );

    // Typing: each character changes the text, and nothing else is raised.
    let mut observed_at_ms = 1_000;
    for character in ["h", "e", "l", "o"] {
        text.push_str(character);
        type_silently(&mut app, hwnd, &text);
        let (output, caret) =
            read_output_and_caret(&uia, &mut source, &mut anchors, &mut terminal, remote);
        observed_at_ms += 100;
        if !output.is_empty() {
            let _ = reduce(
                &mut state,
                &event(
                    observed_at_ms,
                    NormalizedEvent::TerminalOutput {
                        node_id: terminal_node(),
                        output,
                    },
                ),
            );
        }
        let _ = reduce(
            &mut state,
            &event(
                observed_at_ms,
                NormalizedEvent::CaretMoved {
                    node_id: terminal_node(),
                    caret,
                },
            ),
        );
    }

    // Backspace, and the terminal's text and caret after it.
    let effects = reduce(
        &mut state,
        &Input::CaretKey {
            trace_id: TraceId::mint(),
            key: CaretKey {
                motion: CaretMotion::Backspace,
                select: false,
            },
            pressed_at_ms: observed_at_ms + 50,
        },
    );
    text.pop();
    type_silently(&mut app, hwnd, &text);
    let mut spoken = Vec::new();
    let mut inputs: Vec<Input> = Vec::new();
    let mut pending = effects;
    loop {
        for effect in pending {
            match effect {
                Effect::Text(request) => {
                    let reply = match &request.op {
                        TextOp::AwaitCaret(watch) => {
                            match check_caret(&mut source, &mut anchors, watch, &mut AlreadyMoved) {
                                Watched::Answered(reply) => reply,
                                Watched::Watching => panic!("the check found no evidence"),
                            }
                        }
                        op => perform(&mut source, &mut anchors, op),
                    };
                    inputs.push(Input::TextCompleted {
                        trace_id: TraceId::mint(),
                        query_id: request.query_id,
                        reply,
                    });
                }
                Effect::Speak(utterance) => spoken.push(
                    utterance
                        .segments
                        .iter()
                        .map(|segment| segment.content.clone())
                        .collect::<Vec<_>>(),
                ),
                _ => {}
            }
        }
        let Some(input) = inputs.pop() else {
            break;
        };
        pending = reduce(&mut state, &input);
    }
    assert_eq!(spoken, [[SegmentContent::Character("o".to_owned())]]);
    app.quit();
}

/// What a read of a typed character's echo costs with the caret read in
/// it (`docs/performance.md`, "A terminal's caret"): remotely, still the
/// program's one call; classically, the screen read's calls and the
/// caret's.
fn a_read_with_the_caret_costs_exactly(remote: bool) {
    let title = common::unique_title("mockapp-terminal-caret-cost");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (element, _) = terminal_text(&uia, hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let source = UiaText::new(element, pattern, pattern2, true).remote(remote);
    let mut terminal = Terminal::default();
    let text = "one\ntwo\nthree\nfour\nfive\nready>";
    type_silently(&mut app, hwnd, text);
    let _ = read(
        &uia,
        (source.element(), source.pattern()),
        None,
        &mut terminal,
        (WANTED, remote, ReadMode::Baseline),
    )
    .expect("the baseline read");
    type_silently(&mut app, hwnd, &format!("{text}h"));
    common::reset_hits(hwnd);
    let _ = verbatim_uia::calls::take();
    let caret = CaretLineQuery {
        element: source.element(),
        pattern: source.pattern(),
        pattern2: source.pattern2(),
        max_text: 4096,
    };
    let answer = read(
        &uia,
        (source.element(), source.pattern()),
        Some(caret),
        &mut terminal,
        (WANTED, remote, ReadMode::Change),
    )
    .expect("the terminal reads");
    let calls = verbatim_uia::calls::take();
    let hits = common::read_hits(hwnd);
    let Found::Output(output, _) = answer.found else {
        panic!("the read settles");
    };
    assert!(!output.is_empty());
    assert!(answer.caret.is_some());
    let expected = if remote {
        (uia_calls(1), REMOTE_TYPED_WITH_CARET_HITS)
    } else {
        (uia_calls(TYPED_WITH_CARET_CALLS), TYPED_WITH_CARET_HITS)
    };
    assert_eq!((calls, hits.as_slice()), expected);
    app.quit();
}

fn a_read_with_the_caret_costs_exactly_remote() {
    a_read_with_the_caret_costs_exactly(true);
}

fn a_read_with_the_caret_costs_exactly_classic() {
    a_read_with_the_caret_costs_exactly(false);
}

fn backspace_says_what_it_deleted_without_caret_events_remote() {
    backspace_says_what_it_deleted_without_caret_events(true);
}

fn backspace_says_what_it_deleted_without_caret_events_classic() {
    backspace_says_what_it_deleted_without_caret_events(false);
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "remote_and_classic_screen_reads_agree",
            remote_and_classic_screen_reads_agree,
        ),
        (
            "the_anchor_is_found_by_its_text_and_its_partner",
            the_anchor_is_found_by_its_text_and_its_partner,
        ),
        (
            "a_failing_find_text_finds_nothing",
            a_failing_find_text_finds_nothing,
        ),
        (
            "terminal_reads_cost_exactly_remote",
            terminal_reads_cost_exactly_remote,
        ),
        (
            "terminal_reads_cost_exactly_classic",
            terminal_reads_cost_exactly_classic,
        ),
        (
            "backspace_says_what_it_deleted_without_caret_events_remote",
            backspace_says_what_it_deleted_without_caret_events_remote,
        ),
        (
            "backspace_says_what_it_deleted_without_caret_events_classic",
            backspace_says_what_it_deleted_without_caret_events_classic,
        ),
        (
            "a_read_with_the_caret_costs_exactly_remote",
            a_read_with_the_caret_costs_exactly_remote,
        ),
        (
            "a_read_with_the_caret_costs_exactly_classic",
            a_read_with_the_caret_costs_exactly_classic,
        ),
    ]);
}
