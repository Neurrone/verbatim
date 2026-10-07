//! A terminal's new output (milestone M4 item 9) against mockapp's UIA text
//! provider, whose `set-text` command rewrites a node's text the way a
//! terminal's buffer changes: lines written at the end, the oldest lines
//! discarded, the screen cleared. Each test drives `verbatim-uia-rops`'s
//! `terminal_tail` and `verbatim-outpost`'s terminal module on this thread
//! exactly as the outpost's worker does once it has the focused terminal's
//! text pattern, since the worker finds a UIA focus by reading the system's
//! keyboard focus, which a test must not take from the desktop it runs on.
//!
//! What is checked: the remote program and the classic implementation
//! agree on every kind of read; the outpost reports appended lines, a line
//! that grew, a scrollback that shifted beneath the anchor, and a cleared
//! screen; and a terminal read costs exactly what `docs/performance.md`
//! records, by client calls and provider hits, remotely and classically.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use verbatim_model::{CallCounts, LineChange, Skipped, TerminalOutput};
use verbatim_outpost::terminal::{TailText, Terminal, read};
use verbatim_uia::{NodeIdRegistry, Uia};
use verbatim_uia_rops::{
    Fingerprint, Found, Path, TailQuery, TailStart, terminal_tail, terminal_tail_classic,
    terminal_tail_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextRange,
};

/// How many of the newest lines a read takes.
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

/// Runs `query` both ways and checks they agree; returns the remote
/// answer's text and last line.
fn both(uia: &Uia, query: &TailQuery<'_>) -> (TailText, IUIAutomationTextRange) {
    let remote = terminal_tail_remote(uia, query).expect("the remote program runs");
    let classic = terminal_tail_classic(uia, query).expect("the classic reads run");
    let remote_text = TailText::from(&remote);
    assert_eq!(remote_text, TailText::from(&classic));
    (remote_text, remote.last)
}

/// An anchored query from `anchor` with the fingerprint `line` and
/// `previous`.
fn anchored<'a>(
    anchor: &'a IUIAutomationTextRange,
    line: &'a str,
    previous: &'a str,
) -> TailQuery<'a> {
    TailQuery {
        start: TailStart::Anchor {
            range: anchor,
            fingerprint: Fingerprint { line, previous },
        },
        lines_wanted: WANTED,
    }
}

fn texts(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|&line| line.to_owned()).collect()
}

fn remote_and_classic_terminal_tails_agree() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-agree");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");

    // Afresh: every line counted, the last three read.
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (tail, last) = both(&uia, &fresh);
    assert_eq!(tail.found, Found::Afresh);
    assert_eq!(tail.count, 6);
    assert_eq!(tail.rows, 3);
    assert_eq!(tail.lines, texts(&["four", "five", "ready>"]));
    assert_eq!(tail.last_line, "ready>");
    assert_eq!(tail.before_last, "five\n");

    // Lines written after the anchor: counted and read from it.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\na\nb\nc\nd\nready>",
    );
    let (tail, next) = both(&uia, &anchored(&last, "ready>", "five\n"));
    assert_eq!(tail.found, Found::AtAnchor);
    assert_eq!(tail.line, "ready> ls\n");
    assert_eq!(tail.count, 5);
    assert_eq!(tail.lines, texts(&["c", "d", "ready>"]));

    // The oldest lines discarded as more are written: the text moved up
    // beneath the anchor, and its fingerprint is found above it, the
    // anchor's line now ending in the line break it gained.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term three\nfour\nfive\nready> ls\na\nb\nc\nd\nready>\nx\ny\nz",
    );
    let (tail, _) = both(&uia, &anchored(&next, "ready>", "d\n"));
    assert_eq!(tail.found, Found::Moved(2));
    assert_eq!(tail.count, 3);
    assert_eq!(tail.lines, texts(&["x", "y", "z"]));

    // The screen cleared: the fingerprint is nowhere.
    common::apply(&mut app, hwnd, r"set-text term hello\nready>");
    let (tail, _) = both(&uia, &anchored(&next, "ready>", "d\n"));
    assert_eq!(tail.found, Found::NotFound);
    app.quit();
}

/// Both ways find the fingerprint by its text (`FindText`), at the same
/// line and the same distance up, past lines holding the line before as
/// part of their text or starting with it.
fn a_fingerprint_is_found_by_its_text_past_partial_matches() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);

    // The anchor on "last", under "ready>".
    common::apply(
        &mut app,
        hwnd,
        r"set-text term aaaaaaaaa\naaaaaaaa\naaaaaaaa\nready>\nlast",
    );
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (tail, last) = both(&uia, &fresh);
    assert_eq!(tail.last_line, "last");

    // The first line discarded and lines written: "ready>" now also ends
    // one line and starts two others above where the anchor's range lies.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term ready>\nlast\nx ready>\nready>x\nready> extra\nmore\nmore2",
    );
    let (tail, _) = both(
        &uia,
        &anchored(
            &last, "last", "ready>
",
        ),
    );
    assert_eq!(tail.found, Found::Moved(4));
    assert_eq!(
        tail.found_line,
        "last
"
    );
    assert_eq!(tail.count, 5);
    app.quit();
}

/// mockapp's text as `set-text` takes it, its line feeds escaped.
fn escaped(text: &str) -> String {
    text.replace('\n', "\\n")
}

/// Lines of eight characters and a line feed each: `prefix` and a
/// three-digit number, for each number in `numbers`.
fn numbered(prefix: &str, numbers: std::ops::Range<u32>) -> String {
    use std::fmt::Write as _;
    numbers.fold(String::new(), |mut lines, n| {
        let _ = writeln!(lines, "{prefix} {n:03}");
        lines
    })
}

/// A fingerprint 300 lines above the anchor, beyond the 256 lines the
/// search once covered, is found by its text, the same both ways, in one
/// round trip remotely; both ways' cost is pinned.
fn a_fingerprint_far_up_is_found_and_costs_exactly() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-far");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);

    // Four hundred lines, a prompt, and the last line, the anchor's.
    let before = numbered("line", 0..400) + "ready>  \nlastline";
    common::apply(
        &mut app,
        hwnd,
        &format!("set-text term {}", escaped(&before)),
    );
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (tail, last) = both(&uia, &fresh);
    assert_eq!(tail.last_line, "lastline");

    // The oldest 300 lines discarded and 300 written: the anchor's range,
    // which keeps its place, is now 300 lines below its line.
    let after =
        numbered("line", 300..400) + "ready>  \nlastline\n" + &numbered("more", 0..300) + "end";
    common::apply(
        &mut app,
        hwnd,
        &format!("set-text term {}", escaped(&after)),
    );
    let query = anchored(&last, "lastline", "ready>  \n");
    let (tail, _) = both(&uia, &query);
    assert_eq!(tail.found, Found::Moved(300));
    assert_eq!(tail.found_line, "lastline\n");
    assert_eq!(tail.count, 301);
    assert_eq!(tail.lines, texts(&["more 298", "more 299", "end"]));

    // The cost: the anchor's two lines, one `FindText` that finds the
    // prompt, its line checked and the line under it read, the walk to the
    // end and the last lines read, and the distance found by walking from
    // the anchor too.
    let mut costs = Vec::new();
    for remote in [true, false] {
        common::reset_hits(hwnd);
        let _ = verbatim_uia::calls::take();
        let tail = if remote {
            terminal_tail_remote(&uia, &query)
        } else {
            terminal_tail_classic(&uia, &query)
        }
        .expect("the tail");
        assert_eq!(tail.found, Found::Moved(300));
        costs.push((verbatim_uia::calls::take(), common::read_hits(hwnd)));
    }
    let hits: Hits = &[
        ("Clone", 19),
        ("CompareEndpoints", 4),
        ("ExpandToEnclosingUnit", 9),
        ("FindText", 1),
        ("GetText", 8),
        ("Move", 7),
        ("MoveEndpointByUnit", 1),
        ("MoveEndpointByRange", 9),
    ];
    let expected: [(CallCounts, Hits); 2] = [(uia_calls(1), hits), (uia_calls(58), hits)];
    for ((calls, hits), (expected_calls, expected_hits)) in costs.iter().zip(expected) {
        assert_eq!(
            (*calls, hits.as_slice()),
            (expected_calls, expected_hits),
            "the far search's cost moved; when that is deliberate, update this test and \
             docs/performance.md together"
        );
    }
    app.quit();
}

/// A fingerprint whose line before is blank is found by its own line, under
/// a blank line, the same both ways.
fn a_fingerprint_under_a_blank_line_is_found_by_its_own_line() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-blank");
    let mut app = common::spawn("terminal.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);
    common::apply(
        &mut app,
        hwnd,
        r"set-text term aaaaaaaa\nbbbbbbbb\n\nready> x",
    );
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (tail, last) = both(&uia, &fresh);
    assert_eq!(
        (tail.before_last.as_str(), tail.last_line.as_str()),
        ("\n", "ready> x")
    );

    // Two lines discarded and four written: the nearest "ready> x" is under
    // another, not a blank line, and is passed over.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term \nready> x\nready> x\nm1\nm2\nm3",
    );
    let (tail, _) = both(&uia, &anchored(&last, "ready> x", "\n"));
    assert_eq!(tail.found, Found::Moved(2));
    assert_eq!(tail.found_line, "ready> x\n");
    assert_eq!(tail.lines, texts(&["m1", "m2", "m3"]));

    // Two blank lines cannot be searched for by text: not found.
    common::apply(&mut app, hwnd, r"set-text term a\n\n\nb\nc\nd\ne\nf");
    let (tail, _) = both(&uia, &anchored(&last, "\n", "\n"));
    assert_eq!(tail.found, Found::NotFound);
    app.quit();
}

/// A provider whose `FindText` fails: the remote program fails, and the
/// classic read, answering instead, walks up line by line and finds what
/// the search by text finds where `FindText` works.
fn a_failing_find_text_falls_back_to_the_walk() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-find-fails");
    let mut app = common::spawn("terminal_find_fails.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\na\nb\nc\nd\nready>",
    );
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (_, last) = both(&uia, &fresh);
    common::apply(
        &mut app,
        hwnd,
        r"set-text term three\nfour\nfive\nready> ls\na\nb\nc\nd\nready>\nx\ny\nz",
    );
    let query = anchored(&last, "ready>", "d\n");
    let (tail, path) = terminal_tail(&uia, &query, true).expect("the tail");
    assert!(matches!(path, Path::Fallback(_)), "{path:?}");
    assert_eq!(tail.found, Found::Moved(2));
    assert_eq!(tail.lines, texts(&["x", "y", "z"]));
    app.quit();
}

/// A terminal whose backward moves answer with a positive count
/// (`terminal_backward_moves.json`), as some providers' do: the tail's
/// rows, counted by moving up from its last line, are the same as from a
/// provider that counts them backward, both ways.
fn a_backward_move_counted_forward_still_counts_the_rows() {
    common::init_com();
    let title = common::unique_title("mockapp-terminal-backward-moves");
    let app = common::spawn("terminal_backward_moves.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    let (_, pattern) = terminal_text(&uia, hwnd);
    let document =
        verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the document range");
    let fresh = TailQuery {
        start: TailStart::Document(&document),
        lines_wanted: WANTED,
    };
    let (tail, _) = both(&uia, &fresh);
    assert_eq!(tail.found, Found::Afresh);
    assert_eq!(tail.count, 6);
    assert_eq!(tail.rows, 3);
    assert_eq!(tail.lines, texts(&["four", "five", "ready>"]));
    assert_eq!(tail.before_last, "five\n");
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
    remote: bool,
    baseline: bool,
) -> (TerminalOutput, CallCounts, Vec<(&'static str, u32)>) {
    common::reset_hits(hwnd);
    let _ = verbatim_uia::calls::take();
    let (output, _) =
        read(uia, text, terminal, WANTED, remote, baseline).expect("the terminal reads");
    let calls = verbatim_uia::calls::take();
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

    // The baseline, when the terminal gains the focus: nothing is new.
    let (output, baseline_calls, baseline_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, true);
    assert_eq!(output, TerminalOutput::default());

    // The prompt grows as the user types.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls",
    );
    let (output, typed_calls, typed_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, false);
    assert_eq!(
        output,
        TerminalOutput {
            changed: Some(LineChange {
                text: " ls".to_owned(),
                line: "ready> ls".to_owned(),
                appended: true,
                uncertain: 0,
            }),
            skipped: None,
            lines: Vec::new(),
        }
    );

    // A command's output: one line and the prompt.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\nnotes.txt\nready>",
    );
    let (output, line_calls, line_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, false);
    assert_eq!(
        output,
        TerminalOutput {
            changed: None,
            skipped: None,
            lines: texts(&["notes.txt", "ready>"]),
        }
    );

    // More than a read takes: the rest is counted as skipped.
    common::apply(
        &mut app,
        hwnd,
        r"set-text term one\ntwo\nthree\nfour\nfive\nready> ls\nnotes.txt\nready> dir\n1\n2\n3\n4\nready>",
    );
    let (output, overflow_calls, overflow_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, false);
    assert_eq!(
        output,
        TerminalOutput {
            changed: Some(LineChange {
                text: " dir".to_owned(),
                line: "ready> dir".to_owned(),
                appended: true,
                uncertain: 0,
            }),
            skipped: Some(Skipped::Count(2)),
            lines: texts(&["3", "4", "ready>"]),
        }
    );

    // A redraw with the same text: nothing.
    let (output, redraw_calls, redraw_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, false);
    assert_eq!(output, TerminalOutput::default());

    // The screen cleared and written to: compared line by line.
    common::apply(&mut app, hwnd, r"set-text term hello\nready>");
    let (output, cleared_calls, cleared_hits) =
        measured_read(&uia, hwnd, text, &mut terminal, remote, false);
    assert_eq!(
        output,
        TerminalOutput {
            changed: None,
            skipped: None,
            lines: texts(&["hello", "ready>"]),
        }
    );
    app.quit();

    let costs = [
        ("baseline", baseline_calls, baseline_hits),
        ("typed", typed_calls, typed_hits),
        ("output line", line_calls, line_hits),
        ("overflow", overflow_calls, overflow_hits),
        ("redraw", redraw_calls, redraw_hits),
        ("cleared", cleared_calls, cleared_hits),
    ];
    // The client's calls: remotely, one program for each read, the
    // baseline's getting the document range itself, which costs the
    // provider the import of the element and its text pattern;
    // classically, one call per provider method. The provider does the
    // same work otherwise.
    //
    // A read that finds no new line after a skip, and one that finds the
    // screen cleared, read twice: the anchor is not where it was, so the
    // read starts again from the document. The classic read of a cleared
    // screen then finds the last line by its text.
    let expected: [(&str, CallCounts, Hits); 6] = if remote {
        [
            ("baseline", uia_calls(1), REMOTE_BASELINE_HITS),
            ("typed", uia_calls(1), TYPED_HITS),
            ("output line", uia_calls(1), LINE_HITS),
            ("overflow", uia_calls(1), LINE_HITS),
            ("redraw", uia_calls(2), REMOTE_REDRAW_HITS),
            ("cleared", uia_calls(2), REMOTE_CLEARED_HITS),
        ]
    } else {
        [
            ("baseline", uia_calls(31), BASELINE_HITS),
            ("typed", uia_calls(27), TYPED_HITS),
            ("output line", uia_calls(39), LINE_HITS),
            ("overflow", uia_calls(39), LINE_HITS),
            ("redraw", uia_calls(58), REDRAW_HITS),
            ("cleared", uia_calls(61), CLEARED_HITS),
        ]
    };
    for ((name, calls, hits), (_, expected_calls, expected_hits)) in costs.iter().zip(expected) {
        assert_eq!(
            (*calls, hits.as_slice()),
            (expected_calls, expected_hits),
            "the {name} read's cost moved; when that is deliberate, update this test and \
             docs/performance.md together"
        );
    }
}

/// Provider hits by method.
type Hits = &'static [(&'static str, u32)];

/// The provider hits of the baseline read: the document range, then the
/// last line found, all six lines counted, the last three read in one
/// call, the last line and the one before it read as lines, and the first
/// line read before and after (the guard against text that moved during
/// the read).
const BASELINE_HITS: &[(&str, u32)] = &[
    ("DocumentRange", 1),
    ("Clone", 9),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 5),
    ("Move", 4),
    ("MoveEndpointByRange", 5),
];

/// The provider hits of the baseline read remotely: the classic read's,
/// and the import of the element and its text pattern.
const REMOTE_BASELINE_HITS: &[(&str, u32)] = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("DocumentRange", 1),
    ("Clone", 9),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 5),
    ("Move", 4),
    ("MoveEndpointByRange", 5),
];

/// The provider hits of a read that finds the prompt grown: the anchor's
/// line and the one before it checked, no line after it, and the line
/// before it read again at the end, the guard.
const TYPED_HITS: &[(&str, u32)] = &[
    ("Clone", 10),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 5),
    ("GetText", 3),
    ("Move", 3),
    ("MoveEndpointByRange", 4),
];

/// The provider hits of a read that finds an output line and a new prompt:
/// as for the grown prompt, and the two lines read in one call, and the
/// last line and the one before it read as lines.
const LINE_HITS: &[(&str, u32)] = &[
    ("Clone", 13),
    ("CompareEndpoints", 2),
    ("ExpandToEnclosingUnit", 6),
    ("GetText", 6),
    ("Move", 5),
    ("MoveEndpointByRange", 7),
];

/// The provider hits of a read that found nothing new after a skip,
/// classically: the anchor's check, then a read from the document.
const REDRAW_HITS: &[(&str, u32)] = &[
    ("DocumentRange", 1),
    ("Clone", 19),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 10),
    ("GetText", 8),
    ("Move", 7),
    ("MoveEndpointByRange", 9),
];

/// [`REDRAW_HITS`] remotely, with the import of the element and its text
/// pattern.
const REMOTE_REDRAW_HITS: &[(&str, u32)] = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("DocumentRange", 1),
    ("Clone", 19),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 10),
    ("GetText", 8),
    ("Move", 7),
    ("MoveEndpointByRange", 9),
];

/// The provider hits of a read that finds the screen cleared, classically:
/// the anchor's check, the last line found by its text, and a read from the
/// document.
const CLEARED_HITS: &[(&str, u32)] = &[
    ("DocumentRange", 1),
    ("Clone", 20),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 10),
    ("FindText", 1),
    ("GetText", 8),
    ("Move", 7),
    ("MoveEndpointByUnit", 1),
    ("MoveEndpointByRange", 9),
];

/// The provider hits of a read that finds the screen cleared, remotely: the
/// classic read's, the fingerprint searched for by its text the same way,
/// and the import of the element and its text pattern for the read from the
/// document.
const REMOTE_CLEARED_HITS: &[(&str, u32)] = &[
    ("ProviderOptions", 2),
    ("GetPatternProvider", 1),
    ("GetPropertyValue", 1),
    ("HostRawElementProvider", 1),
    ("Navigate", 1),
    ("DocumentRange", 1),
    ("Clone", 20),
    ("CompareEndpoints", 4),
    ("ExpandToEnclosingUnit", 10),
    ("FindText", 1),
    ("GetText", 8),
    ("Move", 7),
    ("MoveEndpointByUnit", 1),
    ("MoveEndpointByRange", 9),
];

fn terminal_reads_cost_exactly_remote() {
    terminal_reads_report_new_output_and_cost_exactly(true);
}

fn terminal_reads_cost_exactly_classic() {
    terminal_reads_report_new_output_and_cost_exactly(false);
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
        (
            "a_backward_move_counted_forward_still_counts_the_rows",
            a_backward_move_counted_forward_still_counts_the_rows,
        ),
        (
            "remote_and_classic_terminal_tails_agree",
            remote_and_classic_terminal_tails_agree,
        ),
        (
            "a_fingerprint_is_found_by_its_text_past_partial_matches",
            a_fingerprint_is_found_by_its_text_past_partial_matches,
        ),
        (
            "a_fingerprint_far_up_is_found_and_costs_exactly",
            a_fingerprint_far_up_is_found_and_costs_exactly,
        ),
        (
            "a_fingerprint_under_a_blank_line_is_found_by_its_own_line",
            a_fingerprint_under_a_blank_line_is_found_by_its_own_line,
        ),
        (
            "a_failing_find_text_falls_back_to_the_walk",
            a_failing_find_text_falls_back_to_the_walk,
        ),
        (
            "terminal_reads_cost_exactly_remote",
            terminal_reads_cost_exactly_remote,
        ),
        (
            "terminal_reads_cost_exactly_classic",
            terminal_reads_cost_exactly_classic,
        ),
    ]);
}
