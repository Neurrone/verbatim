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
//! ends; the language UIA reports; the caret read in one remote
//! operation agreeing with its classic reads, formatting included; and
//! Core's say-all over UIA text, which has no sentence unit, speaking it
//! by sentence.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use verbatim_core::{SrState, reduce};
use verbatim_model::{
    Backend, CaretWatch, Effect, Input, NodeDetails, NodeId, NodeSnapshot, NormalizedEvent,
    OutpostId, Pid, PreviousSelection, ReviewCommand, Role, SegmentContent, SpeechMark, StateSet,
    TextChunk, TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReadAhead, TextReply,
    TextUnit, TraceId,
};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::UiaText;
use verbatim_outpost::text::{
    Anchors, CaretSignal, NodeText, TextSource, Watched, caret_report, check_caret, perform,
};
use verbatim_uia::text::Endpoint;
use verbatim_uia::{NodeIdRegistry, Uia};
use verbatim_uia_rops::{
    Attributes, CaretAnswer, CaretQuery, FormatSpan, LocationQuery, Movement, Position,
    RangeAction, RangeEnd, RangeQuery, RunAttributes, TextAttribute, TextFrom, TextTarget,
    UnitsAnswer, UnitsQuery, caret_read_classic, caret_read_remote, text_location_classic,
    text_location_remote, text_range_classic, text_range_remote, text_units_classic,
    text_units_remote,
};
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, TextUnit_Line, TextUnit_Word};
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::core::w;

/// A caret key's watch checked once with no caret event: every test moves
/// mockapp's caret before asking, so the first read is the evidence.
struct AlreadyMoved;

impl CaretSignal for AlreadyMoved {
    fn caret_event(&mut self) -> bool {
        false
    }

    fn now_ms(&mut self) -> u64 {
        0
    }
}

/// The caret reply a check of a key's watch answered with.
fn answered(watched: Watched) -> TextReply {
    match watched {
        Watched::Answered(reply) => reply,
        Watched::Watching => panic!("the check found no evidence"),
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
    let last: Vec<bool> = chunks.iter().map(|chunk| chunk.last).collect();
    assert_eq!(
        last,
        [false, false, true],
        "only the empty last line is last"
    );
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
                &*format!("gamma{break_text}")
            ]
        )
    );
    let last: Vec<bool> = chunks.iter().map(|chunk| chunk.last).collect();
    assert_eq!(last, [false, false], "more lines follow");
}

/// The first word and character, and `second_word`, the word reached
/// inside the line, as the source ends a word at a line's end.
fn words_and_characters<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    second_word: &str,
) {
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
    assert_eq!(next.text, second_word);
}

/// The word "beta" through UIA: mockapp's word is a run of letters with
/// the spaces after it, so the line feed after it is a word of its own.
const UIA_SECOND_WORD: &str = "beta";

/// The word "beta" through the edit control, whose line break is a word
/// of its own, as NVDA's plain edit word (`plain_word` in the outpost's
/// `text/edit.rs`).
const EDIT_SECOND_WORD: &str = "beta";

/// A caret key's answer: the caret moved to 6, the word there,
/// `second_word`, and then a selection of the first word reported as
/// selected.
fn a_caret_key_is_answered_with_what_it_did<S: TextSource>(
    (app, hwnd): (&mut common::MockApp, HWND),
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    second_word: &str,
) {
    common::apply(app, hwnd, "caret doc 0");
    let (before, _) = caret_report(source, anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 6");
    let reply = answered(check_caret(
        source,
        anchors,
        &CaretWatch {
            landing: false,
            pressed_at_ms: 0,
            since: Some(point_of(&before.line)),
            unit: TextUnit::Word,
            compare: None,
            previous_selection: None,
        },
        &mut AlreadyMoved,
    ));
    let TextReply::Caret(reply) = reply else {
        panic!("a caret reply, not {reply:?}");
    };
    assert!(reply.moved);
    assert_eq!(reply.caret.line.offset, 6);
    assert_eq!(reply.unit.expect("the word").text, second_word);

    common::apply(app, hwnd, "caret doc 0");
    let (collapsed, _) = caret_report(source, anchors, &mut || 0, false).expect("the caret");
    common::apply(app, hwnd, "caret doc 0 5");
    let at = point_of(&collapsed.line);
    let reply = answered(check_caret(
        source,
        anchors,
        &CaretWatch {
            landing: false,
            pressed_at_ms: 0,
            since: Some(at),
            unit: TextUnit::Character,
            compare: None,
            previous_selection: Some(PreviousSelection { start: at, end: at }),
        },
        &mut AlreadyMoved,
    ));
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
    words_and_characters(&mut source, &mut anchors, UIA_SECOND_WORD);
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
    a_caret_key_is_answered_with_what_it_did(
        (&mut app, hwnd),
        &mut source,
        &mut anchors,
        UIA_SECOND_WORD,
    );
    // And the same keys answered the classic way, with remote operations
    // off.
    let mut classic = uia_notes(hwnd, false);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    a_caret_key_is_answered_with_what_it_did(
        (&mut app, hwnd),
        &mut classic,
        &mut anchors,
        UIA_SECOND_WORD,
    );
    app.quit();
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
        // Not supported by mockapp's text without styles: strikethrough,
        // the background color, bullets, and links.
        ..RunAttributes::default()
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
    let all = Attributes::ALL;
    let query = |formats, unit| CaretQuery {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
        since: None,
        previous_selection: None,
        unit,
        formats,
        attributes: all,
        learning: Attributes::NONE,
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
    nothing.attributes = Attributes::NONE;
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
    app.quit();
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
        attributes: Attributes::ALL,
        learning: Attributes::NONE,
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
    app.quit();
}

/// A stretch of the format unit whose italics read as UIA's "mixed", since
/// mockapp's format unit does not end where italics do (as Windows
/// Terminal's and the console host's do not), is walked again by words,
/// and a mixed word by characters, the same remotely and classically; a
/// line of alternating italic characters stops at `MAX_RUNS` stretches.
fn a_mixed_stretch_is_read_by_words_then_characters() {
    common::init_com();
    let title = common::unique_title("mockapp-mixed-stretch");
    let mut app = common::spawn("italic.json", "uia", &title);
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
        formats: Some(FormatSpan::Line),
        attributes: Attributes::ALL,
        learning: Attributes::NONE,
        max_text: 1024,
        max_change_text: 1024,
    };
    let italic = |italic| RunAttributes {
        italic: Some(italic),
        ..mock_attributes(false, false)
    };
    common::apply(&mut app, hwnd, "caret doc 0");
    let line = caret_both(&query);
    assert_eq!(line.line, ("plain italic text\n".to_owned(), 0));
    let mut expected = vec![(6, italic(false))];
    expected.extend(std::iter::repeat_n((1, italic(true)), 6));
    expected.extend([(1, italic(false)), (4, italic(false)), (1, italic(false))]);
    assert_eq!(line.runs, expected);

    common::apply(&mut app, hwnd, "caret doc 18");
    let alternating = caret_both(&query);
    let expected: Vec<(usize, RunAttributes)> = (0..verbatim_uia_rops::MAX_RUNS)
        .map(|index| (1, italic(index % 2 == 1)))
        .collect();
    assert_eq!(alternating.runs, expected);
    app.quit();
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
    assert_eq!(
        units(TextFrom::Start, None, 2),
        (
            0,
            vec![
                ("alpha beta\n".to_owned(), 0, en()),
                ("gamma\n".to_owned(), 0, en()),
            ],
            false
        )
    );

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
        attributes: Attributes::NONE,
        learning: Attributes::NONE,
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
    app.quit();
}

/// A read ahead over lines in three languages, whose `Culture` over the
/// whole batch is UIA's "mixed", gives each line its own language, the same
/// remotely and classically; a batch in one language and a single line
/// read give theirs.
fn a_batch_in_several_languages_gives_each_line_its_own() {
    common::init_com();
    let title = common::unique_title("mockapp-text-languages");
    let mut app = common::spawn("languages.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
    };
    let units = |movement, count| {
        let query = UnitsQuery {
            target,
            from: TextFrom::Start,
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
    let tag = |tag: &str| Some(tag.to_owned());
    assert_eq!(
        units(None, 16),
        (
            0,
            vec![
                ("hello\n".to_owned(), 0, tag("en-US")),
                ("bonjour\n".to_owned(), 0, tag("fr-FR")),
                ("hallo\n".to_owned(), 0, tag("de-DE")),
                (String::new(), 0, tag("en-US")),
            ],
            true
        )
    );
    assert_eq!(
        units(Some(Movement::By(TextUnit_Line, 1)), 1),
        (1, vec![("bonjour\n".to_owned(), 0, tag("fr-FR"))], false)
    );
    // The French stretch now holds two whole lines.
    common::apply(&mut app, hwnd, r"set-text doc hello\nab\ncd\nef\n");
    assert_eq!(
        units(Some(Movement::By(TextUnit_Line, 1)), 2),
        (
            1,
            vec![
                ("ab\n".to_owned(), 0, tag("fr-FR")),
                ("cd\n".to_owned(), 0, tag("fr-FR")),
            ],
            false
        )
    );
    app.quit();
}

/// One part of a say-all utterance: an index mark, or text.
#[derive(Debug, PartialEq)]
enum Said {
    Mark(SpeechMark),
    Text(String),
}

/// What Core did with one input to say-all, its text requests answered
/// by mockapp through the outpost's text module and the answers given back
/// to it: the utterances it spoke, each as its marks and texts, and the
/// caret moves it asked for.
#[derive(Debug, Default)]
struct SayAllStep {
    utterances: Vec<Vec<Said>>,
    caret_moves: Vec<TextPoint>,
    display_released: bool,
}

/// Gives Core `input`, answering every text request it makes from
/// `source`, until it asks for nothing more. Any other effect fails the
/// test, so nothing is skipped unchecked.
fn say_all_step<S: TextSource>(
    state: &mut SrState,
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    input: &Input,
) -> SayAllStep {
    let mut step = SayAllStep::default();
    let mut inputs = vec![input.clone()];
    while let Some(input) = inputs.pop() {
        for effect in reduce(state, &input) {
            match effect {
                Effect::Speak(utterance) => {
                    assert!(utterance.say_all, "{utterance:?} is read by say-all");
                    step.utterances.push(
                        utterance
                            .segments
                            .iter()
                            .map(|segment| match &segment.content {
                                SegmentContent::Mark(mark) => Said::Mark(*mark),
                                SegmentContent::Text(text) => Said::Text(text.clone()),
                                _ => panic!("say-all speech is marks and text: {utterance:?}"),
                            })
                            .collect(),
                    );
                }
                Effect::Text(request) => {
                    if let TextOp::MoveCaret(point) = request.op {
                        step.caret_moves.push(point);
                    }
                    let reply = perform(source, anchors, &request.op);
                    inputs.push(Input::TextCompleted {
                        trace_id: TraceId::mint(),
                        query_id: request.query_id,
                        reply,
                    });
                }
                Effect::KeepDisplayOn(false) => step.display_released = true,
                Effect::KeepDisplayOn(true) => {}
                other => panic!("an effect say-all does not make: {other:?}"),
            }
        }
    }
    step
}

/// The marks of an utterance, in order.
fn marks_of(said: &[Said]) -> Vec<SpeechMark> {
    said.iter()
        .filter_map(|part| match part {
            Said::Mark(mark) => Some(*mark),
            Said::Text(_) => None,
        })
        .collect()
}

/// Core's state with the focus on mockapp's "Notes" document.
fn focused_notes() -> SrState {
    let mut state = SrState::new();
    let _ = reduce(
        &mut state,
        &Input::Event {
            trace_id: TraceId::mint(),
            observed_at_ms: 0,
            source: Pid(1),
            backend: Backend::Uia,
            window: None,
            event: NormalizedEvent::FocusChanged {
                node: NodeSnapshot {
                    id: NodeId::in_outpost(OutpostId(1), 1),
                    backend: Backend::Uia,
                    role: Role::Document,
                    name: Some("Notes".to_owned()),
                    value: None,
                    states: StateSet::new(),
                    details: NodeDetails::default(),
                },
                foreground: false,
                ancestors: Vec::new(),
                ancestors_unknown: false,
                selected_child: None,
            },
        },
    );
    state
}

/// Say-all over UIA text, which has no sentence unit, reads it by line and
/// speaks it by sentence without pauses: a line holding one sentence's end
/// and the next one's start is spoken in two utterances, the sentence that
/// runs on to the next line is spoken whole, and the caret moves to each
/// line's start as its text starts playing, not as its utterance does.
/// Core runs here with its text requests answered from
/// mockapp's text provider, remotely, as the outpost answers them.
fn uia_say_all_speaks_by_sentence_without_a_sentence_unit() {
    common::init_com();
    let title = common::unique_title("mockapp-text-say-all");
    let mut app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    common::apply(
        &mut app,
        hwnd,
        r"set-text doc Verbatim is written in Rust. It is informed by NVDA\nbut not constrained by it.\n",
    );
    common::apply(&mut app, hwnd, "caret doc 0");
    let mut source = uia_notes(hwnd, true);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    let mut state = focused_notes();
    let step = say_all_step(
        &mut state,
        &mut source,
        &mut anchors,
        &Input::Command {
            trace_id: TraceId::mint(),
            command: ReviewCommand::SayAllFromCaret,
            repeat: 0,
        },
    );
    // The first line up to its sentence end, then the sentence that runs
    // on to the second line, with the second line's mark where its text
    // starts. The empty line after the last line break says nothing.
    assert_eq!(step.caret_moves, []);
    assert!(!step.display_released);
    let [first, second] = step.utterances.as_slice() else {
        panic!("two utterances: {step:?}");
    };
    let first_mark = marks_of(first)[0];
    assert_eq!(
        first,
        &[
            Said::Mark(first_mark),
            Said::Text("Verbatim is written in Rust. ".to_owned())
        ]
    );
    let [opening, second_line] = marks_of(second)[..] else {
        panic!("two marks: {second:?}");
    };
    assert_eq!(
        second,
        &[
            Said::Mark(opening),
            Said::Text("It is informed by NVDA ".to_owned()),
            Said::Mark(second_line),
            Said::Text("but not constrained by it.".to_owned()),
        ]
    );
    let reached = |mark| Input::MarkReached { mark };
    let caret_line = |source: &mut UiaText, anchors: &mut NodeText<'_, _>| {
        let (_, line) = chunk(read(
            source,
            anchors,
            TextPoint::Caret,
            None,
            TextUnit::Line,
        ));
        (line.text, line.offset)
    };

    // Each mark reached: the first line's moves the caret to its start;
    // the rest of the first line leaves it there.
    let step = say_all_step(&mut state, &mut source, &mut anchors, &reached(first_mark));
    assert_eq!(step.caret_moves.len(), 1, "{step:?}");
    assert_eq!(step.utterances, Vec::<Vec<Said>>::new());
    assert!(!step.display_released);
    assert_eq!(
        caret_line(&mut source, &mut anchors),
        (
            "Verbatim is written in Rust. It is informed by NVDA\n".to_owned(),
            0
        )
    );
    let step = say_all_step(&mut state, &mut source, &mut anchors, &reached(opening));
    assert_eq!(step.caret_moves, []);
    assert_eq!(step.utterances, Vec::<Vec<Said>>::new());
    assert!(!step.display_released);
    // The second line's text starts: the caret moves to its start, and
    // say-all, with nothing more to hand on, ends with it there.
    let step = say_all_step(&mut state, &mut source, &mut anchors, &reached(second_line));
    assert_eq!(step.caret_moves.len(), 1, "{step:?}");
    assert_eq!(step.utterances, Vec::<Vec<Said>>::new());
    assert!(
        step.display_released,
        "say-all ends after its last utterance"
    );
    assert_eq!(
        caret_line(&mut source, &mut anchors),
        ("but not constrained by it.\n".to_owned(), 0)
    );
    app.quit();
}

/// What `EM_LINEINDEX` answers for the line after the last, line 3 of
/// "alpha beta", "gamma", and the empty last line: -1, which the classic
/// edit control sign-extends into the message's result.
const CLASSIC_PAST_LAST_LINE: isize = -1;

/// The same -1 from Common Controls version 6's edit control, as a Windows
/// Forms text box answers it: zero-extended into the result.
const VERSION_6_PAST_LAST_LINE: isize = 0xFFFF_FFFF;

fn edit_control_text_reads_moves_and_answers_caret_keys() {
    edit_control_text("text.json", CLASSIC_PAST_LAST_LINE);
}

/// Common Controls version 6's edit control, read as the classic one is,
/// though it answers a line past the last differently.
fn version_6_edit_control_text_reads_moves_and_answers_caret_keys() {
    edit_control_text("text_version_6.json", VERSION_6_PAST_LAST_LINE);
}

/// The edit control mockapp makes for `fixture`'s "Notes" text, whose
/// `EM_LINEINDEX` answers `past_last_line` for the line after the last,
/// read by lines, words, characters, and paragraphs, and its caret keys
/// answered.
fn edit_control_text(fixture: &str, past_last_line: isize) {
    use windows::Win32::UI::Controls::EM_LINEINDEX;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
    common::init_com();
    let title = common::unique_title("mockapp-text-edit");
    let mut app = common::spawn(fixture, "msaa", &title);
    let hwnd = common::find_window(&title);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe { FindWindowExW(Some(hwnd), None, w!("EDIT"), None) }
        .expect("mockapp's edit control");
    // SAFETY: EM_LINEINDEX takes and answers plain integers.
    let answer = unsafe { SendMessageW(edit, EM_LINEINDEX, Some(WPARAM(3)), None) }.0;
    assert_eq!(answer, past_last_line, "the line after the last");
    let mut source = edit_notes(hwnd);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    lines_stop_at_the_end(&mut source, &mut anchors, "\r\n");
    words_and_characters(&mut source, &mut anchors, EDIT_SECOND_WORD);
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
    a_caret_key_is_answered_with_what_it_did(
        (&mut app, hwnd),
        &mut source,
        &mut anchors,
        EDIT_SECOND_WORD,
    );
    app.quit();
}

/// The attributes mockapp's `formatting.json` reports for plain text, every
/// attribute supported.
fn plain_formatting() -> RunAttributes {
    RunAttributes {
        spelling_error: false,
        grammar_error: false,
        font_name: Some("Consolas".to_owned()),
        font_size: Some(11.0),
        font_weight: Some(400),
        italic: Some(false),
        underline: Some(0),
        strikethrough: Some(0),
        color: Some(0),
        background_color: Some(0x00FF_FFFF),
        bullet_style: Some(0),
        link: Some(false),
    }
}

/// Every attribute Verbatim reads, over mockapp's `formatting.json`, which
/// supports them all: a heading's font size, a background color,
/// strikethrough, a double underline, a link, and a bullet, each its own
/// stretch, the same remotely and classically; and, for an attribute being
/// learned, that the provider supports it, from a line but not from a
/// character.
#[expect(
    clippy::too_many_lines,
    reason = "each line's stretches listed in full"
)]
fn every_attribute_is_read_both_ways() {
    common::init_com();
    let title = common::unique_title("mockapp-every-attribute");
    let mut app = common::spawn("formatting.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let learning = Attributes::ALL.without(Attributes::of(&[TextAttribute::Annotations]));
    let query = |formats| CaretQuery {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
        since: None,
        previous_selection: None,
        unit: None,
        formats: Some(formats),
        attributes: Attributes::ALL,
        learning,
        max_text: 1024,
        max_change_text: 1024,
    };
    let plain = plain_formatting;
    let read = |formats| {
        let remote = caret_read_remote(&query(formats)).expect("the remote program runs");
        let classic = caret_read_classic(&query(formats)).expect("the classic reads run");
        assert_eq!(remote.unsupported, classic.unsupported);
        let unsupported = remote.unsupported;
        let remote = summary(&remote);
        assert_eq!(remote, summary(&classic));
        (remote.runs, unsupported)
    };

    common::apply(&mut app, hwnd, "caret doc 0");
    assert_eq!(
        read(FormatSpan::Line),
        (
            vec![
                (
                    5,
                    RunAttributes {
                        font_size: Some(28.0),
                        ..plain()
                    }
                ),
                (1, plain()),
            ],
            Some(Attributes::NONE)
        ),
        "the heading's size; every attribute supported"
    );
    common::apply(&mut app, hwnd, "caret doc 6");
    assert_eq!(
        read(FormatSpan::Line),
        (
            vec![
                (
                    5,
                    RunAttributes {
                        background_color: Some(0x00C0_C0C0),
                        ..plain()
                    }
                ),
                (1, plain()),
                (
                    6,
                    RunAttributes {
                        strikethrough: Some(1),
                        ..plain()
                    }
                ),
                (1, plain()),
                (
                    6,
                    RunAttributes {
                        underline: Some(3),
                        ..plain()
                    }
                ),
                (1, plain()),
                (
                    4,
                    RunAttributes {
                        link: Some(true),
                        ..plain()
                    }
                ),
                (1, plain()),
            ],
            Some(Attributes::NONE)
        )
    );
    common::apply(&mut app, hwnd, "caret doc 31");
    assert_eq!(
        read(FormatSpan::Line),
        (
            vec![(
                5,
                RunAttributes {
                    bullet_style: Some(2),
                    ..plain()
                }
            )],
            Some(Attributes::NONE)
        )
    );
    assert_eq!(
        read(FormatSpan::Character),
        (
            vec![(
                0,
                RunAttributes {
                    bullet_style: Some(2),
                    ..plain()
                }
            )],
            None
        ),
        "nothing learned from a character"
    );
    app.quit();
}

/// mockapp's text without styles (`text.json`) does not support
/// strikethrough, the background color, bullets, or links, as Windows
/// Terminal does not: UIA answers "not supported" for each, which both
/// ways read as no value and report as unsupported, from a line; the
/// annotation types are never reported, though the text between spelling
/// errors answers "not supported" for them.
fn unsupported_attributes_are_found_both_ways() {
    common::init_com();
    let title = common::unique_title("mockapp-unsupported-attributes");
    let mut app = common::spawn("text.json", "uia", &title);
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
        formats: Some(FormatSpan::Line),
        attributes: Attributes::ALL,
        learning: Attributes::ALL,
        max_text: 1024,
        max_change_text: 1024,
    };
    let unsupported = Some(Attributes::of(&[
        TextAttribute::StrikethroughStyle,
        TextAttribute::BackgroundColor,
        TextAttribute::BulletStyle,
        TextAttribute::Link,
    ]));
    // "gamma", with no spelling error: its annotation types are not
    // supported, which says nothing about the provider.
    common::apply(&mut app, hwnd, "caret doc 11");
    let remote = caret_read_remote(&query).expect("the remote program runs");
    let classic = caret_read_classic(&query).expect("the classic reads run");
    assert_eq!(summary(&remote), summary(&classic));
    assert_eq!(
        summary(&remote).runs,
        [(6, mock_attributes(false, false))],
        "the unsupported attributes read as none"
    );
    assert_eq!(remote.unsupported, unsupported);
    assert_eq!(classic.unsupported, unsupported);
    app.quit();
}

/// A provider whose backward moves answer with a positive count
/// (`backward_moves.json`), as some do: the count is corrected to a
/// negative one, as NVDA corrects it, the same remotely and classically.
fn a_backward_move_is_counted_backward_both_ways() {
    common::init_com();
    let title = common::unique_title("mockapp-backward-moves");
    let mut app = common::spawn("backward_moves.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let (pattern, pattern2) = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
        pattern2: pattern2.as_ref(),
    };
    let units = |unit, count| {
        let query = UnitsQuery {
            target,
            from: TextFrom::Caret,
            movement: Some(Movement::By(unit, count)),
            unit,
            count: 1,
            max_text: 1024,
            max_total: 1024,
            culture: false,
        };
        let remote = text_units_remote(&query).expect("the remote program runs");
        let classic = text_units_classic(&query).expect("the classic reads run");
        let remote = units_summary(&remote);
        assert_eq!(remote, units_summary(&classic));
        remote
    };
    common::apply(&mut app, hwnd, "caret doc 6");
    assert_eq!(
        units(TextUnit_Word, -1),
        (-1, vec![("alpha ".to_owned(), 0, None)], false),
        "from \"beta\" back to \"alpha\""
    );
    common::apply(&mut app, hwnd, "caret doc 13");
    assert_eq!(
        units(TextUnit_Line, -1),
        (-1, vec![("alpha beta\n".to_owned(), 0, None)], false)
    );
    assert_eq!(
        units(TextUnit_Line, 1),
        (1, vec![(String::new(), 0, None)], false),
        "forward moves are as they were"
    );
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "every_attribute_is_read_both_ways",
            every_attribute_is_read_both_ways,
        ),
        (
            "unsupported_attributes_are_found_both_ways",
            unsupported_attributes_are_found_both_ways,
        ),
        (
            "a_backward_move_is_counted_backward_both_ways",
            a_backward_move_is_counted_backward_both_ways,
        ),
        (
            "a_failing_attribute_is_not_supported",
            a_failing_attribute_is_not_supported,
        ),
        (
            "a_mixed_stretch_is_read_by_words_then_characters",
            a_mixed_stretch_is_read_by_words_then_characters,
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
            "a_batch_in_several_languages_gives_each_line_its_own",
            a_batch_in_several_languages_gives_each_line_its_own,
        ),
        (
            "uia_text_reads_moves_and_answers_caret_keys",
            uia_text_reads_moves_and_answers_caret_keys,
        ),
        (
            "edit_control_text_reads_moves_and_answers_caret_keys",
            edit_control_text_reads_moves_and_answers_caret_keys,
        ),
        (
            "version_6_edit_control_text_reads_moves_and_answers_caret_keys",
            version_6_edit_control_text_reads_moves_and_answers_caret_keys,
        ),
        (
            "uia_say_all_speaks_by_sentence_without_a_sentence_unit",
            uia_say_all_speaks_by_sentence_without_a_sentence_unit,
        ),
    ]);
}
