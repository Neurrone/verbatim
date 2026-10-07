//! The text protocol's outpost side (milestone M4) against mockapp, on both
//! stacks: UIA's text pattern served by mockapp's text provider, and a real
//! Win32 edit control, which mockapp's MSAA backend hosts and which is read
//! through its window messages. Each test drives `verbatim-outpost`'s text
//! module on this thread exactly as the outpost's worker does once it has
//! the node's text in hand, since the worker finds a UIA focus by reading
//! the system's keyboard focus, which a test must not take from the desktop
//! it runs on.
//!
//! What is checked: lines, words, and characters read; the caret reported;
//! a caret key answered with what it did, the selection's change included;
//! an unsupported unit reported as such; movement stopping at the text's
//! ends; the language UIA reports; and the caret read in one remote
//! operation agreeing with its classic reads, formatting included.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use verbatim_model::{
    CaretWait, CaretWatch, PreviousSelection, TextChunk, TextMovement, TextOp, TextPoint,
    TextPosition, TextRead, TextReadAhead, TextReply, TextUnit,
};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::UiaText;
use verbatim_outpost::text::{Anchors, CaretSignal, NodeText, TextSource, caret_report, perform};
use verbatim_uia::text::Endpoint;
use verbatim_uia::{NodeIdRegistry, Uia};
use verbatim_uia_rops::{
    Attributes, CaretAnswer, CaretQuery, FormatSpan, LocationQuery, Movement, Position,
    RangeAction, RangeEnd, RangeQuery, RunAttributes, TextFrom, TextTarget, UnitsAnswer,
    UnitsQuery, caret_read_classic, caret_read_remote, text_location_classic, text_location_remote,
    text_range_classic, text_range_remote, text_units_classic, text_units_remote,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{IUIAutomationElement, TextUnit_Line, TextUnit_Word};
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::core::w;

/// A caret key's wait that never needs to wait: every test moves mockapp's
/// caret before asking, so the first read is the evidence.
struct AlreadyMoved;

impl CaretSignal for AlreadyMoved {
    fn caret_event(&mut self) -> bool {
        false
    }

    fn wait(&mut self, _timeout: Duration) {
        panic!("the caret had already moved, so nothing should wait");
    }

    fn now(&mut self) -> Instant {
        Instant::now()
    }

    fn now_ms(&mut self) -> u64 {
        0
    }
}

/// mockapp's "Notes" document's element.
fn notes_element(hwnd: HWND) -> IUIAutomationElement {
    let uia = Uia::new().expect("a UIA client");
    let cache = uia.base_cache_request().expect("a cache request");
    let root = uia
        .element_from_handle(hwnd.0 as isize, &cache)
        .expect("mockapp's root element");
    let registry = NodeIdRegistry::new(Arc::new(AtomicU64::new(1)));
    let (tree, _) = uia
        .walk_tree(&root, &cache, &registry, 8, 64)
        .expect("mockapp's tree");
    let notes_id = tree
        .children
        .iter()
        .find(|child| child.snapshot.name.as_deref() == Some("Notes"))
        .expect("the Notes document")
        .snapshot
        .id;
    registry
        .element_of(notes_id)
        .and_then(|agile| agile.resolve().ok())
        .expect("its element")
}

/// The text of mockapp's "Notes" document through UIA, read remotely or
/// classically.
fn uia_notes(hwnd: HWND, remote: bool) -> UiaText {
    let notes = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, pattern2, false).remote(remote)
}

/// The edit control mockapp's MSAA backend hosts for the "Notes" text.
fn edit_notes(hwnd: HWND) -> EditText {
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe { FindWindowExW(Some(hwnd), None, w!("EDIT"), None) }
        .expect("mockapp's edit control");
    EditText::new(edit.0 as isize, 0)
}

/// Where a chunk's point is, as a position.
fn point_of(chunk: &TextChunk) -> TextPosition {
    TextPosition {
        anchor: chunk.start,
        offset: chunk.offset,
    }
}

/// Reads one unit.
fn read<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: TextPoint,
    movement: Option<(TextUnit, i32)>,
    unit: TextUnit,
) -> TextReply {
    perform(
        source,
        anchors,
        &TextOp::Read(TextRead {
            at,
            movement: movement.map(|(unit, count)| TextMovement { unit, count }),
            unit,
        }),
        &mut AlreadyMoved,
    )
}

/// A read's chunk and how far it moved.
fn chunk(reply: TextReply) -> (i32, TextChunk) {
    match reply {
        TextReply::Read { moved, chunk } => (moved, chunk),
        other => panic!("a read, not {other:?}"),
    }
}

/// Reads down the text by line from the caret at the start: each line in
/// turn, the empty last line after the final line break, and no further.
/// `break_text` is the line break as the backend gives it.
fn lines_stop_at_the_end<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    break_text: &str,
) {
    let (report, _) = caret_report(source, anchors, &mut || 0, false).expect("the caret");
    assert_eq!(report.line.text, format!("alpha beta{break_text}"));
    assert_eq!(report.line.offset, 0);
    assert_eq!(report.selection, None);

    let (moved, second) = chunk(read(
        source,
        anchors,
        TextPoint::Caret,
        Some((TextUnit::Line, 1)),
        TextUnit::Line,
    ));
    assert_eq!(
        (moved, second.text.as_str()),
        (1, &*format!("gamma{break_text}"))
    );
    let (moved, last) = chunk(read(
        source,
        anchors,
        TextPoint::At(point_of(&second)),
        Some((TextUnit::Line, 1)),
        TextUnit::Line,
    ));
    assert_eq!((moved, last.text.as_str()), (1, ""), "the empty last line");
    let (moved, again) = chunk(read(
        source,
        anchors,
        TextPoint::At(point_of(&last)),
        Some((TextUnit::Line, 1)),
        TextUnit::Line,
    ));
    assert_eq!((moved, again.text.as_str()), (0, ""), "no further");

    // Say-all's read ahead: every line in one request, the last marked as
    // the text's last.
    let reply = perform(
        source,
        anchors,
        &TextOp::ReadAhead(TextReadAhead {
            at: TextPoint::Start,
            movement: None,
            unit: TextUnit::Line,
            count: 16,
        }),
        &mut AlreadyMoved,
    );
    let TextReply::Chunks { moved, chunks } = reply else {
        panic!("chunks, not {reply:?}");
    };
    let texts: Vec<&str> = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
    assert_eq!(
        (moved, texts),
        (
            0,
            vec![
                &*format!("alpha beta{break_text}"),
                &*format!("gamma{break_text}"),
                ""
            ]
        )
    );
    assert!(chunks.last().is_some_and(|chunk| chunk.last));
    // Two lines asked for: two read.
    let reply = perform(
        source,
        anchors,
        &TextOp::ReadAhead(TextReadAhead {
            at: TextPoint::Start,
            movement: None,
            unit: TextUnit::Line,
            count: 2,
        }),
        &mut AlreadyMoved,
    );
    let TextReply::Chunks { chunks, .. } = reply else {
        panic!("chunks, not {reply:?}");
    };
    assert_eq!(chunks.len(), 2);
}

/// The first word and character, and a word reached inside the line.
fn words_and_characters<S: TextSource>(source: &mut S, anchors: &mut NodeText<'_, S::Pos>) {
    let (_, word) = chunk(read(
        source,
        anchors,
        TextPoint::Start,
        None,
        TextUnit::Word,
    ));
    assert_eq!(word.text, "alpha ");
    let (_, character) = chunk(read(
        source,
        anchors,
        TextPoint::Start,
        None,
        TextUnit::Character,
    ));
    assert_eq!(character.text, "a");
    // A position Core found inside the word, resolved through its text.
    let (_, next) = chunk(read(
        source,
        anchors,
        TextPoint::At(TextPosition {
            anchor: word.start,
            offset: 3,
        }),
        Some((TextUnit::Word, 1)),
        TextUnit::Word,
    ));
    assert!(next.text.starts_with("beta"), "{next:?}");
}

/// A caret key's answer: the caret moved to 6, the word there, and then a
/// selection of the first word reported as selected.
fn a_caret_key_is_answered_with_what_it_did<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
) {
    common::apply(app, hwnd, "caret doc 0");
    let (before, _) = caret_report(source, anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 6");
    let reply = perform(
        source,
        anchors,
        &TextOp::AwaitCaret(CaretWatch {
            pressed_at_ms: 0,
            since: Some(point_of(&before.line)),
            unit: TextUnit::Word,
            compare: None,
            previous_selection: None,
            wait: CaretWait::Standard,
        }),
        &mut AlreadyMoved,
    );
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    assert!(reply.moved);
    assert_eq!(reply.caret.line.offset, 6);
    assert!(reply.unit.expect("the word").text.starts_with("beta"));

    common::apply(app, hwnd, "caret doc 0");
    let (collapsed, _) = caret_report(source, anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 0 5");
    let at = point_of(&collapsed.line);
    let reply = perform(
        source,
        anchors,
        &TextOp::AwaitCaret(CaretWatch {
            pressed_at_ms: 0,
            since: Some(at),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: Some(PreviousSelection { start: at, end: at }),
            wait: CaretWait::Standard,
        }),
        &mut AlreadyMoved,
    );
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    let changes: Vec<(bool, &str, u32)> = reply
        .selection_changes
        .iter()
        .map(|change| (change.selected, change.text.as_str(), change.characters))
        .collect();
    assert_eq!(changes, [(true, "alpha", 5)]);
}

fn uia_text_reads_moves_and_answers_caret_keys() {
    common::init_com();
    let title = common::unique_title("mockapp-text-uia");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd, true);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    lines_stop_at_the_end(&mut source, &mut anchors, "\n");
    words_and_characters(&mut source, &mut anchors);
    // UIA has no sentence: Core reads by line instead.
    assert_eq!(
        read(
            &mut source,
            &mut anchors,
            TextPoint::Start,
            None,
            TextUnit::Sentence
        ),
        TextReply::UnsupportedUnit(TextUnit::Sentence)
    );
    // A read carries UIA's language for its text.
    let (_, line) = chunk(read(
        &mut source,
        &mut anchors,
        TextPoint::Start,
        None,
        TextUnit::Line,
    ));
    assert_eq!(line.language_at(0), Some("en-US"));
    a_caret_key_is_answered_with_what_it_did(&mut app, hwnd, &mut source, &mut anchors);
    // And the same keys answered the classic way, with remote operations
    // off.
    let mut classic = uia_notes(hwnd, false);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    a_caret_key_is_answered_with_what_it_did(&mut app, hwnd, &mut classic, &mut anchors);
    app.send("quit");
}

/// Runs a caret read both ways and checks that they agree; returns the
/// remote answer's line text and offset, unit text and offset, and runs.
fn caret_both(query: &CaretQuery<'_>) -> CaretSummary {
    let remote = caret_read_remote(query).expect("the remote program runs");
    let classic = caret_read_classic(query).expect("the classic reads run");
    let remote = summary(&remote);
    assert_eq!(remote, summary(&classic));
    remote
}

/// What a caret read found, comparable across the two implementations.
#[derive(Debug, PartialEq)]
struct CaretSummary {
    moved: bool,
    selection_moved: bool,
    line: (String, usize),
    unit: Option<(String, usize)>,
    runs: Vec<(usize, RunAttributes)>,
    changes: Option<Vec<(bool, String)>>,
}

fn summary(answer: &CaretAnswer) -> CaretSummary {
    let text = |text: &[u16]| String::from_utf16_lossy(text);
    CaretSummary {
        moved: answer.moved,
        selection_moved: answer.selection_moved,
        line: (text(&answer.line.text), answer.line.offset),
        unit: answer
            .unit
            .as_ref()
            .map(|unit| (text(&unit.text), unit.offset)),
        runs: answer
            .runs
            .iter()
            .map(|run| (run.length, run.attributes.clone()))
            .collect(),
        changes: answer.changes.as_ref().map(|changes| {
            changes
                .iter()
                .map(|change| (change.selected, text(&change.text)))
                .collect()
        }),
    }
}

/// The attributes mockapp reports, with these errors and weight.
fn mock_attributes(spelling_error: bool, bold: bool) -> RunAttributes {
    RunAttributes {
        spelling_error,
        grammar_error: false,
        font_name: Some("Consolas".to_owned()),
        font_size: Some(11.0),
        font_weight: Some(if bold { 700 } else { 400 }),
        italic: Some(false),
        underline: Some(0),
        color: Some(0),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "each read checked in turn against one running mockapp"
)]
fn remote_and_classic_caret_reads_agree() {
    common::init_com();
    let title = common::unique_title("mockapp-caret-read");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let all = Attributes {
        annotations: true,
        font: true,
        font_attributes: true,
        color: true,
    };
    let query = |formats, unit| CaretQuery {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
        since: None,
        previous_selection: None,
        unit,
        formats,
        attributes: all,
        max_text: 1024,
        max_change_text: 1024,
    };

    // The caret in "beta", a spelling error after bold "alpha": the line's
    // stretches are bold "alpha", a space, the error, and the line feed.
    common::apply(&mut app, hwnd, "caret doc 7");
    let line = caret_both(&query(Some(FormatSpan::Line), None));
    assert_eq!(line.line, ("alpha beta\n".to_owned(), 7));
    assert_eq!(
        line.runs,
        [
            (5, mock_attributes(false, true)),
            (1, mock_attributes(false, false)),
            (4, mock_attributes(true, false)),
            (1, mock_attributes(false, false)),
        ]
    );
    // The word, with its formatting.
    let word = caret_both(&query(Some(FormatSpan::Unit), Some(TextUnit_Word)));
    assert_eq!(word.unit, Some(("beta".to_owned(), 1)));
    assert_eq!(word.runs, [(4, mock_attributes(true, false))]);
    // The character alone.
    let character = caret_both(&query(Some(FormatSpan::Character), None));
    assert_eq!(character.runs, [(0, mock_attributes(true, false))]);
    // Only what the theme asks for: no attributes, no formatting.
    let mut nothing = query(Some(FormatSpan::Line), None);
    nothing.attributes = Attributes::default();
    assert_eq!(caret_both(&nothing).runs, []);

    // The evidence: compared with where the caret was, and the selection.
    let known = caret_read_classic(&query(None, None)).expect("the caret");
    common::apply(&mut app, hwnd, "caret doc 8");
    let mut moved = query(None, None);
    moved.since = Some(RangeEnd {
        range: &known.caret,
        endpoint: Endpoint::Start,
    });
    moved.previous_selection = Some((
        RangeEnd {
            range: &known.caret,
            endpoint: Endpoint::Start,
        },
        RangeEnd {
            range: &known.caret,
            endpoint: Endpoint::Start,
        },
    ));
    let found = caret_both(&moved);
    assert!(found.moved && found.selection_moved);
    assert_eq!(
        found.changes,
        Some(Vec::new()),
        "nothing was or is selected"
    );
    // A selection made from the caret: its text, newly selected.
    let at_six = {
        common::apply(&mut app, hwnd, "caret doc 6");
        caret_read_classic(&query(None, None)).expect("the caret")
    };
    common::apply(&mut app, hwnd, "caret doc 6 10");
    let six = RangeEnd {
        range: &at_six.caret,
        endpoint: Endpoint::Start,
    };
    let extended = caret_both(&CaretQuery {
        previous_selection: Some((six, six)),
        ..query(None, None)
    });
    assert_eq!(
        extended.changes,
        Some(vec![(true, "beta".to_owned())]),
        "the word selected"
    );
    common::apply(&mut app, hwnd, "caret doc 6 10");
    let selected = caret_read_remote(&query(None, None)).expect("the caret");
    assert!(selected.selection.is_some());
    let again = caret_both(&CaretQuery {
        since: Some(RangeEnd {
            range: &selected.caret,
            endpoint: Endpoint::Start,
        }),
        previous_selection: Some((
            RangeEnd {
                range: selected.selection.as_ref().expect("a selection"),
                endpoint: Endpoint::Start,
            },
            RangeEnd {
                range: selected.selection.as_ref().expect("a selection"),
                endpoint: Endpoint::End,
            },
        )),
        ..query(None, None)
    });
    assert!(!again.moved && !again.selection_moved);
    app.send("quit");
}

/// A provider whose `IsItalic` read fails: the failed attribute is not
/// supported and the others are read, as NVDA treats a failed attribute
/// read, both classically, where UIA answers `GetAttributeValues` for all
/// seven in one call with the failed one not supported, and in the remote
/// program.
fn a_failing_attribute_is_not_supported() {
    common::init_com();
    let title = common::unique_title("mockapp-failing-attribute");
    let mut app = common::spawn("failing_attribute.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let query = CaretQuery {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
        since: None,
        previous_selection: None,
        unit: None,
        formats: Some(FormatSpan::Character),
        attributes: Attributes {
            annotations: true,
            font: true,
            font_attributes: true,
            color: true,
        },
        max_text: 1024,
        max_change_text: 1024,
    };
    common::apply(&mut app, hwnd, "caret doc 1");
    let classic = caret_read_classic(&query).expect("the classic reads run");
    assert_eq!(
        summary(&classic).runs,
        [(
            0,
            RunAttributes {
                italic: None,
                ..mock_attributes(false, false)
            }
        )]
    );
    let remote = caret_read_remote(&query).expect("the remote program runs");
    assert_eq!(summary(&remote).runs, summary(&classic).runs);
    app.send("quit");
}

/// What a units read found, comparable across the two implementations.
/// How far a units read moved, each unit's text, offset, and language, and
/// whether the text ended.
type UnitsSummary = (i32, Vec<(String, usize, Option<String>)>, bool);

fn units_summary(answer: &UnitsAnswer) -> UnitsSummary {
    (
        answer.moved,
        answer
            .units
            .iter()
            .map(|unit| {
                (
                    String::from_utf16_lossy(&unit.text),
                    unit.offset,
                    unit.language.clone(),
                )
            })
            .collect(),
        answer.ended,
    )
}

/// The text protocol's other reads agree remotely and classically: units
/// after a movement, read ahead, and from a position some text after a held
/// one; the text between two points and selecting it; and a point's place
/// on the screen.
#[expect(
    clippy::too_many_lines,
    reason = "each read checked in turn against one running mockapp"
)]
fn remote_and_classic_text_reads_agree() {
    common::init_com();
    let title = common::unique_title("mockapp-text-reads");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
    };
    let units = |from, movement, count| {
        let query = UnitsQuery {
            target,
            from,
            movement,
            unit: TextUnit_Line,
            count,
            max_text: 1024,
            max_total: 1024,
            culture: true,
        };
        let remote = text_units_remote(&query).expect("the remote program runs");
        let classic = text_units_classic(&query).expect("the classic reads run");
        let remote = units_summary(&remote);
        assert_eq!(remote, units_summary(&classic));
        remote
    };
    let en = || Some("en-US".to_owned());
    common::apply(&mut app, hwnd, "caret doc 7");
    assert_eq!(
        units(TextFrom::Caret, None, 1),
        (0, vec![("alpha beta\n".to_owned(), 7, en())], false)
    );
    assert_eq!(
        units(TextFrom::Caret, Some(Movement::By(TextUnit_Line, 1)), 1),
        (1, vec![("gamma\n".to_owned(), 0, en())], false)
    );
    assert_eq!(
        units(TextFrom::Start, Some(Movement::Document(1)), 1),
        (1, vec![(String::new(), 0, en())], false),
        "the end of the text, on its empty last line"
    );
    assert_eq!(
        units(TextFrom::Start, None, 16),
        (
            0,
            vec![
                ("alpha beta\n".to_owned(), 0, en()),
                ("gamma\n".to_owned(), 0, en()),
                (String::new(), 0, en()),
            ],
            true
        )
    );
    assert_eq!(units(TextFrom::Start, None, 2).1.len(), 2);

    // A position Core found inside a line: the start's range, and the
    // text before the position.
    let document = verbatim_uia::text::TextPatternExt::document_range(&pattern).expect("the text");
    let start = Position {
        range: &document,
        endpoint: Endpoint::Start,
        collapsed: false,
    };
    let prefix: Vec<u16> = "alpha ".encode_utf16().collect();
    let after = TextFrom::After {
        from: start,
        prefix: &prefix,
        counts: &[6],
    };
    assert_eq!(
        units(after, None, 1),
        (0, vec![("alpha beta\n".to_owned(), 6, en())], false)
    );

    // The text between two points, given in either order, and selected.
    let range = |start, end, action| {
        let query = RangeQuery {
            target,
            start,
            end,
            action,
        };
        let remote = text_range_remote(&query).expect("the remote program runs");
        let classic = text_range_classic(&query).expect("the classic reads run");
        assert_eq!(
            (&remote.text, remote.selected),
            (&classic.text, classic.selected)
        );
        (String::from_utf16_lossy(&remote.text), remote.selected)
    };
    assert_eq!(
        range(after, Some(TextFrom::Start), RangeAction::Text(1024)),
        ("alpha ".to_owned(), false)
    );
    assert_eq!(
        range(TextFrom::Start, Some(after), RangeAction::Select),
        (String::new(), true)
    );
    let selected = caret_read_remote(&CaretQuery {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
        since: None,
        previous_selection: None,
        unit: None,
        formats: None,
        attributes: Attributes::default(),
        max_text: 1024,
        max_change_text: 1024,
    })
    .expect("the caret");
    let selection = selected.selection.expect("the selection made");
    assert_eq!(
        verbatim_uia::text::TextRangeExt::text(&selection, 64).expect("its text"),
        prefix
    );
    assert_eq!(
        range(
            TextFrom::SelectionStart,
            Some(TextFrom::SelectionEnd),
            RangeAction::Text(1024)
        ),
        ("alpha ".to_owned(), false)
    );

    // A point's place: mockapp draws each character 8 pixels wide.
    for (at, expected) in [(TextFrom::Start, (100.0, 200.0)), (after, (148.0, 200.0))] {
        let query = LocationQuery { target, at };
        let remote = text_location_remote(&query).expect("the remote program runs");
        let classic = text_location_classic(&query).expect("the classic reads run");
        assert_eq!(remote.location, classic.location);
        assert_eq!(remote.location, Some(expected));
    }
    app.send("quit");
}

fn edit_control_text_reads_moves_and_answers_caret_keys() {
    common::init_com();
    let title = common::unique_title("mockapp-text-edit");
    let mut app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut source = edit_notes(hwnd);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    lines_stop_at_the_end(&mut source, &mut anchors, "\r\n");
    words_and_characters(&mut source, &mut anchors);
    // An edit control's sentences are Core's to split: the paragraph comes
    // back, and a paragraph is a line.
    let (_, paragraph) = chunk(read(
        &mut source,
        &mut anchors,
        TextPoint::Start,
        None,
        TextUnit::Sentence,
    ));
    assert_eq!(paragraph.unit, TextUnit::Paragraph);
    assert_eq!(paragraph.text, "alpha beta\r\n");
    assert_eq!(
        read(
            &mut source,
            &mut anchors,
            TextPoint::Start,
            None,
            TextUnit::Page
        ),
        TextReply::UnsupportedUnit(TextUnit::Page)
    );
    a_caret_key_is_answered_with_what_it_did(&mut app, hwnd, &mut source, &mut anchors);
    app.send("quit");
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run(&[
        (
            "a_failing_attribute_is_not_supported",
            a_failing_attribute_is_not_supported,
        ),
        (
            "remote_and_classic_caret_reads_agree",
            remote_and_classic_caret_reads_agree,
        ),
        (
            "remote_and_classic_text_reads_agree",
            remote_and_classic_text_reads_agree,
        ),
        (
            "uia_text_reads_moves_and_answers_caret_keys",
            uia_text_reads_moves_and_answers_caret_keys,
        ),
        (
            "edit_control_text_reads_moves_and_answers_caret_keys",
            edit_control_text_reads_moves_and_answers_caret_keys,
        ),
    ]);
}
