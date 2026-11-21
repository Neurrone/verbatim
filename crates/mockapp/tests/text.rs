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
//! a caret key's final read, and a selection's change from its baseline;
//! an unsupported unit reported as such; movement stopping at the text's
//! ends; the language UIA reports; the caret read in one remote
//! operation agreeing with its classic reads, formatting included; and
//! Core's say-all over UIA text, which has no sentence unit, speaking it
//! by sentence.

#[path = "common/batch.rs"]
mod batch;
mod common;
#[path = "common/harness.rs"]
mod harness;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use common::core_driver::CoreDriver;
use common::outpost::OutpostUnderTest;
use verbatim_model::batch::{Chunks, Done, Located, Op, TextOffset};
use verbatim_model::request::Question;
use verbatim_model::{
    CaretReport, ReviewCommand, SegmentContent, SpeechMark, TextChunk, TextMovement, TextPoint,
    TextPosition, TextReply, TextUnit,
};
use verbatim_outpost::text::edit::EditText;
use verbatim_outpost::text::uia::UiaText;
use verbatim_outpost::text::{Anchors, CallStep, NodeText, TextSource, by_calls, held, ops};
use verbatim_uia::Uia;
use verbatim_uia::text::Endpoint;
use verbatim_uia_rops::batch::{
    Answer, Ends, Found, Input, Point, Ran, Segment, UnitsRead, run_batch, run_calls,
};
use verbatim_uia_rops::{
    Attributes, CaretAnswer, CaretQuery, Movement, Position, RangeEnd, RunAttributes,
    TextAttribute, TextFrom, TextTarget, caret_read_classic, caret_read_remote,
};
use windows::Win32::Foundation::{HWND, WPARAM};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, TextUnit_Line, TextUnit_Word};
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::core::w;

/// The caret's report, read as a focus's caret read reads it, at
/// `read_at_us`.
fn caret_report<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    read_at_us: u64,
) -> CaretReport {
    held::read_focus(source, anchors, read_at_us)
        .expect("the caret")
        .0
}

/// mockapp's "Notes" document's element, found by its name
/// (`common::named_element`), which passes the window's title bar by.
fn notes_element(hwnd: HWND) -> IUIAutomationElement {
    let uia = Uia::new().expect("a UIA client");
    common::named_element(&uia, hwnd, "Notes")
}

/// The text of mockapp's "Notes" document through UIA, read remotely or
/// classically.
fn uia_notes(hwnd: HWND, remote: bool) -> UiaText {
    let notes = notes_element(hwnd);
    let pattern = verbatim_uia::text::text_pattern(&notes).expect("a text pattern");
    UiaText::new(notes, pattern, false).remote(remote)
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

/// Reads `count` units call by call, as the outpost reads a batch's
/// `ReadText` of an edit control: how far it moved and the chunks, or the
/// reply that refuses it.
fn read_units<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (at, movement, unit): (TextPoint, Option<(TextUnit, i32)>, TextUnit),
    count: u8,
) -> Result<(i32, Vec<TextChunk>), TextReply> {
    let step = CallStep::Read {
        at,
        movement: movement.map(|(unit, count)| TextMovement { unit, count }),
        unit,
        count,
    };
    match by_calls(source, anchors, step).expect("the text answers") {
        Ok(Done::ReadText(Chunks { moved, chunks })) => Ok((moved, chunks)),
        Ok(other) => panic!("a read, not {other:?}"),
        Err(reply) => Err(reply),
    }
}

/// Reads one unit.
fn read<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: TextPoint,
    movement: Option<(TextUnit, i32)>,
    unit: TextUnit,
) -> Result<(i32, TextChunk), TextReply> {
    read_units(source, anchors, (at, movement, unit), 1)
        .map(|(moved, mut chunks)| (moved, chunks.remove(0)))
}

/// A read's chunk and how far it moved.
fn chunk(reply: Result<(i32, TextChunk), TextReply>) -> (i32, TextChunk) {
    reply.unwrap_or_else(|other| panic!("a read, not {other:?}"))
}

/// Where `at` is, call by call.
fn locate<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: TextPoint,
) -> Located {
    match by_calls(source, anchors, CallStep::Locate(at)).expect("the text answers") {
        Ok(Done::Locate(located)) => located,
        other => panic!("a location, not {other:?}"),
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
    let report = caret_report(source, anchors, 0);
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
    let reply = read_units(
        source,
        anchors,
        (TextPoint::Start, None, TextUnit::Line),
        16,
    );
    let Ok((moved, chunks)) = reply else {
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
    let reply = read_units(source, anchors, (TextPoint::Start, None, TextUnit::Line), 2);
    let Ok((moved, chunks)) = reply else {
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

/// The word "beta" through the edit control, with the line break after it
/// as the white space after it, as Uniscribe gives NVDA's plain edit word
/// (`plain_word` in the outpost's `text/edit.rs`).
const EDIT_SECOND_WORD: &str = "beta\r\n";

/// A caret key's end read: the caret moved to 6 and the word read there,
/// `second_word`; and then a selection of the first word read as selected
/// from the baseline held with the caret at its start.
fn a_caret_key_is_answered_with_what_it_did<S: TextSource>(
    (app, hwnd): (&mut common::MockApp, HWND),
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    second_word: &str,
) {
    common::apply(app, hwnd, "caret doc 0");
    caret_report(source, anchors, 1);
    common::apply(app, hwnd, "caret doc 6");
    let (_, word) = chunk(read(
        source,
        anchors,
        TextPoint::Caret,
        None,
        TextUnit::Word,
    ));
    assert_eq!(word.text, second_word);

    common::apply(app, hwnd, "caret doc 0");
    let baseline = ops::caret(source, anchors, 1).expect("the caret").held;
    common::apply(app, hwnd, "caret doc 0 5");
    let after = ops::caret(source, anchors, 2).expect("the caret").held;
    let changes: Vec<(bool, String, u32)> =
        ops::compare_selection(source, anchors, (baseline, after))
            .expect("the selection read")
            .expect("the baseline is held")
            .difference
            .changes
            .into_iter()
            .map(|change| (change.selected, change.text, change.characters))
            .collect();
    assert_eq!(changes, [(true, "alpha".to_owned(), 5)]);
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
        Err(TextReply::UnsupportedUnit(TextUnit::Sentence))
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
    // Control+Backspace with the caret inside "beta" reads it whole.
    backspace_reads(&mut app, hwnd, &mut source, &mut anchors, (8, "e", "beta"));
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
    backspace_reads(&mut app, hwnd, &mut classic, &mut anchors, (8, "e", "beta"));
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
    runs: Vec<(usize, RunAttributes)>,
    changes: Option<Vec<(bool, String)>>,
    texts: (Option<String>, Option<String>),
    selection_start: Option<usize>,
    start_first: bool,
}

fn summary(answer: &CaretAnswer) -> CaretSummary {
    let text = |text: &[u16]| String::from_utf16_lossy(text);
    CaretSummary {
        moved: answer.moved,
        selection_moved: answer.selection_moved,
        line: (text(&answer.line.text), answer.line.offset),
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
        texts: (
            answer.selection_text.as_deref().map(text),
            answer.previous_text.as_deref().map(text),
        ),
        selection_start: answer.selection_start,
        start_first: answer.start_first,
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let all = Attributes::ALL;
    let query = |formats| CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats,
        attributes: all,
        learning: Attributes::NONE,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
    };

    // The caret in "beta", a spelling error after bold "alpha": the line's
    // stretches are bold "alpha", a space, the error, and the line feed.
    common::apply(&mut app, hwnd, "caret doc 7");
    let line = caret_both(&query(true));
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
    // Only what the theme asks for: no attributes, no formatting.
    let mut nothing = query(true);
    nothing.attributes = Attributes::NONE;
    assert_eq!(caret_both(&nothing).runs, []);

    // The evidence: compared with where the caret was, and the selection.
    let known = caret_read_classic(&query(false)).expect("the caret");
    common::apply(&mut app, hwnd, "caret doc 8");
    let mut moved = query(false);
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
        caret_read_classic(&query(false)).expect("the caret")
    };
    common::apply(&mut app, hwnd, "caret doc 6 10");
    let six = RangeEnd {
        range: &at_six.caret,
        endpoint: Endpoint::Start,
    };
    let extended = caret_both(&CaretQuery {
        previous_selection: Some((six, six)),
        read_selection_text: true,
        ..query(false)
    });
    assert_eq!(
        extended.changes,
        Some(vec![(true, "beta".to_owned())]),
        "the word selected"
    );
    // Its ends moved, so its text is read to keep.
    assert_eq!(extended.texts, (Some("beta".to_owned()), None));
    common::apply(&mut app, hwnd, "caret doc 6 10");
    let selected = caret_read_remote(&query(false)).expect("the caret");
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
        ..query(false)
    });
    assert!(!again.moved && !again.selection_moved);
    // With the texts asked for: what the previous selection's ends hold
    // now, and the selection's text only when its ends are not those, here
    // not read, the ends being the same; with no previous selection, the
    // selection's text.
    let texts = caret_both(&CaretQuery {
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
        read_selection_text: true,
        read_previous_text: true,
        ..query(false)
    });
    assert_eq!(texts.texts, (None, Some("beta".to_owned())));
    let alone = caret_both(&CaretQuery {
        read_selection_text: true,
        ..query(false)
    });
    assert_eq!(alone.texts, (Some("beta".to_owned()), None));
    // The caret is the selection's start, as NVDA's is, whichever end
    // `TextPattern2`'s caret is at (Windows 11 Notepad's is the end after
    // Shift+Control+Right Arrow): the selection starts at the caret's own
    // offset in its line.
    assert_eq!((alone.line.1, alone.selection_start), (6, Some(6)));
    common::apply(&mut app, hwnd, "caret doc 6 10 end");
    let at_end = caret_both(&query(false));
    assert_eq!((at_end.line.1, at_end.selection_start), (6, Some(6)));
    common::apply(&mut app, hwnd, "caret doc 6 13 end");
    let below = caret_both(&query(false));
    assert_eq!(
        (below.line, below.selection_start),
        (("alpha beta\n".to_owned(), 6), Some(6))
    );
    // A selection from the text's start: the line it starts in is known
    // to be the first, so nothing comes before it, whichever line the
    // provider's caret is on; one from the start of another line is not.
    assert!(!at_end.start_first);
    common::apply(&mut app, hwnd, "caret doc 0 5 end");
    let first = caret_both(&query(false));
    assert_eq!((first.selection_start, first.start_first), (Some(0), true));
    common::apply(&mut app, hwnd, "caret doc 0 13 end");
    let first_below = caret_both(&query(false));
    assert_eq!(
        (first_below.selection_start, first_below.start_first),
        (Some(0), true)
    );

    common::apply(&mut app, hwnd, "caret doc 11 13 end");
    let second = caret_both(&query(false));
    assert_eq!(
        (second.selection_start, second.start_first),
        (Some(0), false)
    );
    // A selection longer than the cap for compared text: its first 513
    // units are read, and so are those of what the old ends hold.
    common::apply(
        &mut app,
        hwnd,
        &format!(r"set-text doc {}\n", "a".repeat(600)),
    );
    common::apply(&mut app, hwnd, "caret doc 0 600");
    let long = caret_read_remote(&query(false)).expect("the caret");
    let long_selection = long.selection.as_ref().expect("a selection");
    let capped = caret_both(&CaretQuery {
        previous_selection: Some((
            RangeEnd {
                range: long_selection,
                endpoint: Endpoint::Start,
            },
            RangeEnd {
                range: long_selection,
                endpoint: Endpoint::End,
            },
        )),
        read_previous_text: true,
        ..query(false)
    });
    assert_eq!(capped.texts, (None, Some("a".repeat(513))));
    let capped = caret_both(&CaretQuery {
        read_selection_text: true,
        ..query(false)
    });
    assert_eq!(capped.texts, (Some("a".repeat(513)), None));
    app.quit();
}

/// A provider whose `IsItalic` read fails: the caret line's one stretch
/// has the failed attribute not supported and the others read, as NVDA
/// treats a failed attribute
/// read, both classically, where UIA answers `GetAttributeValues` for all
/// seven in one call with the failed one not supported, and in the remote
/// program.
fn a_failing_attribute_is_not_supported() {
    common::init_com();
    let title = common::unique_title("mockapp-failing-attribute");
    let mut app = common::spawn("failing_attribute.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let query = CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats: true,
        attributes: Attributes::ALL,
        learning: Attributes::NONE,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
    };
    common::apply(&mut app, hwnd, "caret doc 1");
    let classic = caret_read_classic(&query).expect("the classic reads run");
    assert_eq!(
        summary(&classic).runs,
        [(
            11,
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let query = CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats: true,
        attributes: Attributes::ALL,
        learning: Attributes::NONE,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
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

fn units_summary(answer: &Answer) -> UnitsSummary {
    let Some(Ran::Done(Found::ReadText(read))) = answer.steps.first() else {
        panic!("units read, not {answer:?}");
    };
    (
        read.moved,
        read.units
            .iter()
            .map(|unit| {
                (
                    String::from_utf16_lossy(&unit.text),
                    unit.offset,
                    unit.language.clone(),
                )
            })
            .collect(),
        read.ended,
    )
}

/// `input` run as a batch's one step by both executors, a program and its
/// calls (the request list's design, section 5): the two answers, each
/// described, agree; the program's answer.
fn both<'a>(input: &dyn Fn() -> Input<'a>) -> Answer {
    let uia = Uia::new().expect("a UIA client");
    let segment = || Segment {
        uia: &uia,
        steps: vec![batch::step(0, input())],
        remote: true,
        window: None,
    };
    let remote = run_batch(&segment());
    let classic = run_calls(&segment());
    assert_eq!(
        batch::describe(None, &remote),
        batch::describe(None, &classic),
        "the two executors differ"
    );
    remote
}

/// `count` units of `unit` from `from`, after `movement`, read by both
/// executors, which agree, with the language when `culture`.
fn units_both(
    target: TextTarget<'_>,
    from: TextFrom<'_>,
    (movement, unit, count): (
        Option<Movement>,
        windows::Win32::UI::Accessibility::TextUnit,
        u32,
    ),
    culture: bool,
) -> UnitsSummary {
    let answer = both(&|| Input::ReadText {
        text: target,
        at: Point::From(from),
        read: UnitsRead {
            movement,
            unit,
            count,
            max_text: 1024,
            max_total: 1024,
            culture,
            formats: Attributes::NONE,
            learning: Attributes::NONE,
        },
    });
    units_summary(&answer)
}

/// The text protocol's other reads agree remotely and classically: units
/// after a movement, read ahead, and from a position some text after a held
/// one; the text between two points, selecting it, and selecting it with
/// the selection's change reported; and a point's place on the screen.
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
    };
    let units =
        |from, movement, count| units_both(target, from, (movement, TextUnit_Line, count), true);
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
    let ends = |start, end| Ends {
        start: Point::From(start),
        end: Point::From(end),
        through: None,
    };
    let range = |start, end| {
        let answer = both(&|| Input::ReadRange {
            text: target,
            ends: ends(start, end),
            max_text: 1024,
        });
        let Some(Ran::Done(Found::ReadRange(text))) = answer.steps.first() else {
            panic!("a range read, not {answer:?}");
        };
        String::from_utf16_lossy(text)
    };
    assert_eq!(range(after, TextFrom::Start), "alpha ");
    let selected = both(&|| Input::Select {
        text: target,
        ends: ends(TextFrom::Start, after),
    });
    assert!(matches!(
        selected.steps.first(),
        Some(Ran::Done(Found::Select))
    ));
    let selected = caret_read_remote(&CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats: false,
        attributes: Attributes::NONE,
        learning: Attributes::NONE,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
    })
    .expect("the caret");
    let selection = selected.selection.expect("the selection made");
    assert_eq!(
        verbatim_uia::text::TextRangeExt::text(&selection, 64).expect("its text"),
        prefix
    );
    assert_eq!(
        range(TextFrom::SelectionStart, TextFrom::SelectionEnd),
        "alpha "
    );

    // Review select from a caret at the start, the caret, the selection,
    // the caret again and the two compared, by each executor: the change
    // is the text selected.
    let text = batch::Text::of(element.clone());
    let uia = Uia::new().expect("a UIA client");
    for run in [run_batch, run_calls] {
        common::apply(&mut app, hwnd, "caret doc 0");
        let answer = run(&Segment {
            uia: &uia,
            steps: batch::review_select(&text, ends(TextFrom::Start, after)),
            remote: true,
            window: None,
        });
        let Some(Ran::Done(Found::CompareSelection(compared))) = answer.steps.get(3) else {
            panic!("the selections compared, not {answer:?}");
        };
        let changes: Vec<(bool, String)> = compared
            .changes
            .iter()
            .map(|change| (change.selected, String::from_utf16_lossy(&change.text)))
            .collect();
        assert_eq!(changes, [(true, "alpha ".to_owned())]);
    }

    // A point's place: mockapp draws each character 8 pixels wide.
    for (at, expected) in [(TextFrom::Start, (100.0, 200.0)), (after, (148.0, 200.0))] {
        let answer = both(&|| Input::Locate {
            text: target,
            at: Point::From(at),
        });
        let Some(Ran::Done(Found::Locate(location))) = answer.steps.first() else {
            panic!("a location, not {answer:?}");
        };
        assert_eq!(*location, Some(expected));
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
    };
    let units = |movement, count| {
        units_both(
            target,
            TextFrom::Start,
            (movement, TextUnit_Line, count),
            true,
        )
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

/// What one step of say-all did: its utterances, each as its marks and
/// texts, how many caret moves Core asked the outpost for, and whether
/// Core let the display go.
#[derive(Debug)]
struct SayAllStep {
    utterances: Vec<Vec<Said>>,
    caret_moves: usize,
    display_released: bool,
}

/// `step` as say-all's step, its caret moves counted by `moves`, which the
/// driver's send observer adds to. Any speech outside say-all fails the
/// test, so nothing is skipped unchecked.
fn say_all_step(step: &common::core_driver::Step, moves: &Rc<Cell<usize>>) -> SayAllStep {
    assert_eq!(
        step.said.len(),
        step.say_all.len(),
        "say-all speaks only its own utterances: {:?}",
        step.said
    );
    SayAllStep {
        utterances: step
            .say_all
            .iter()
            .map(|utterance| {
                utterance
                    .iter()
                    .map(|content| match content {
                        SegmentContent::Mark(mark) => Said::Mark(*mark),
                        SegmentContent::Text(text) => Said::Text(text.clone()),
                        other => panic!("say-all speech is marks and text: {other:?}"),
                    })
                    .collect()
            })
            .collect(),
        caret_moves: moves.replace(0),
        display_released: step.display_released,
    }
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

/// Say-all over UIA text, which has no sentence unit, reads it by line and
/// speaks it by sentence without pauses: a line holding one sentence's end
/// and the next one's start is spoken in two utterances, the sentence that
/// runs on to the next line is spoken whole, and the caret moves to each
/// line's start as its text starts playing, not as its utterance does.
/// Core runs here with its requests answered by a real outpost watching
/// mockapp.
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
    common::apply(&mut app, hwnd, "focus doc");
    let mut source = uia_notes(hwnd, true);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    let mut core = CoreDriver::switched_to(&app, OutpostUnderTest::new(&app));
    let uia = Uia::new().expect("a UIA client");
    let _ = core.uia_focus(&common::listener_focus(&uia, hwnd.0 as isize));
    let moves = Rc::new(Cell::new(0));
    let counted = Rc::clone(&moves);
    core.observe_sends(Some(Box::new(move |ask, _| {
        if let Question::Batch(sent) = &ask.request
            && matches!(
                sent.steps.first().map(|step| &step.op),
                Some(Op::Select { .. })
            )
        {
            counted.set(counted.get() + 1);
        }
    })));
    let step = say_all_step(&core.command(ReviewCommand::SayAllFromCaret), &moves);
    // The first line up to its sentence end, then the sentence that runs
    // on to the second line, with the second line's mark where its text
    // starts. The empty line after the last line break says nothing.
    assert_eq!(step.caret_moves, 0);
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
    let step = say_all_step(&core.mark_reached(first_mark), &moves);
    assert_eq!(step.caret_moves, 1, "{step:?}");
    assert_eq!(step.utterances, Vec::<Vec<Said>>::new());
    assert!(!step.display_released);
    assert_eq!(
        caret_line(&mut source, &mut anchors),
        (
            "Verbatim is written in Rust. It is informed by NVDA\n".to_owned(),
            0
        )
    );
    let step = say_all_step(&core.mark_reached(opening), &moves);
    assert_eq!(step.caret_moves, 0);
    assert_eq!(step.utterances, Vec::<Vec<Said>>::new());
    assert!(!step.display_released);
    // The second line's text starts: the caret moves to its start, and
    // say-all, with nothing more to hand on, ends with it there.
    let step = say_all_step(&core.mark_reached(second_line), &moves);
    assert_eq!(step.caret_moves, 1, "{step:?}");
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
        Err(TextReply::UnsupportedUnit(TextUnit::Page))
    );
    a_caret_key_is_answered_with_what_it_did(
        (&mut app, hwnd),
        &mut source,
        &mut anchors,
        EDIT_SECOND_WORD,
    );
    runs_of_one_mark_are_words(&mut app, hwnd, &mut source, &mut anchors);
    the_word_at_an_edits_end_is_empty(&mut app, hwnd, &mut source, &mut anchors);
    app.quit();
}

/// The edit control's word at the caret at its text's end, past a
/// password field's six stars, is empty, as NVDA expands a collapsed
/// position there to no word; Core says it as "blank"
/// (`verbatim-core`'s
/// `the_current_word_at_a_password_boxs_end_is_blank_as_nvdas_capture`).
fn the_word_at_an_edits_end_is_empty(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut EditText,
    anchors: &mut NodeText<'_, u32>,
) {
    common::apply(app, hwnd, "set-text doc ******");
    common::apply(app, hwnd, "caret doc 6");
    let (moved, word) = chunk(read(
        source,
        anchors,
        TextPoint::Caret,
        None,
        TextUnit::Word,
    ));
    assert_eq!((moved, word.text.as_str(), word.offset), (0, "", 0));
}

/// A run of one punctuation mark is one word of the edit control, with the
/// white space after it, as Uniscribe breaks NVDA's plain edit words: a
/// password field's stars are one word, and so are the dots of an
/// ellipsis.
fn runs_of_one_mark_are_words(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut EditText,
    anchors: &mut NodeText<'_, u32>,
) {
    common::apply(app, hwnd, "set-text doc ****  ... **");
    let (_, first) = chunk(read(
        source,
        anchors,
        TextPoint::Start,
        None,
        TextUnit::Word,
    ));
    let next = Some((TextUnit::Word, 1));
    let (_, second) = chunk(read(
        source,
        anchors,
        TextPoint::At(point_of(&first)),
        next,
        TextUnit::Word,
    ));
    let (_, third) = chunk(read(
        source,
        anchors,
        TextPoint::At(point_of(&second)),
        next,
        TextUnit::Word,
    ));
    assert_eq!(
        [first.text, second.text, third.text],
        ["****  ", "... ", "**"]
    );
    // Control+Backspace inside the stars reads them whole.
    backspace_reads(app, hwnd, source, anchors, (2, "*", "****  "));
}

/// What the starting batches of Backspace and Control+Backspace read with
/// the caret at `caret` (a `ReadText` from the caret moved back one
/// character, expanded to the unit), as NVDA's Backspace scripts read what
/// they delete: the `character` before the caret and the provider's whole
/// `word` around that character, the caret at the word's end or inside
/// it; and at the text's start no move, so nothing before the caret.
fn backspace_reads<S: TextSource>(
    app: &mut common::MockApp,
    hwnd: HWND,
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    (caret, character, word): (usize, &str, &str),
) {
    let before = |source: &mut S, anchors: &mut NodeText<'_, S::Pos>, unit| {
        let (moved, chunk) = chunk(read(
            source,
            anchors,
            TextPoint::Caret,
            Some((TextUnit::Character, -1)),
            unit,
        ));
        if moved == 0 {
            String::new()
        } else {
            chunk.text
        }
    };
    common::apply(app, hwnd, &format!("caret doc {caret}"));
    assert_eq!(
        [
            before(source, anchors, TextUnit::Character),
            before(source, anchors, TextUnit::Word),
        ],
        [character, word],
        "the caret at {caret}"
    );
    common::apply(app, hwnd, "caret doc 0");
    assert_eq!(
        [
            before(source, anchors, TextUnit::Character),
            before(source, anchors, TextUnit::Word),
        ],
        ["", ""],
        "the caret at the text's start"
    );
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let learning = Attributes::ALL.without(Attributes::of(&[TextAttribute::Annotations]));
    let query = |formats| CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats,
        attributes: Attributes::ALL,
        learning,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
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
        read(true),
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
        read(true),
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
        read(true),
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let query = CaretQuery {
        element: &element,
        pattern: &pattern,
        since: None,
        previous_selection: None,
        previous_collapsed: false,
        formats: true,
        attributes: Attributes::ALL,
        learning: Attributes::ALL,
        max_text: 1024,
        max_change_text: 1024,
        recheck: false,
        read_selection_text: false,
        read_previous_text: false,
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
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
    };
    let units = |unit, count| {
        units_both(
            target,
            TextFrom::Caret,
            (Some(Movement::By(unit, count)), unit, 1),
            false,
        )
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

/// A provider whose expansion to a line below its last line with text
/// gives that line (`lines_clamp.json`), as the console host's does: reading
/// ahead ends at the last line with text, rather than reading it again for
/// every row below it, the same remotely and classically.
fn reading_ahead_ends_where_lines_clamp_back_both_ways() {
    common::init_com();
    let title = common::unique_title("mockapp-lines-clamp");
    let mut app = common::spawn("lines_clamp.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let element = notes_element(hwnd);
    let pattern = verbatim_uia::text::text_pattern(&element).expect("a text pattern");
    let target = TextTarget {
        element: &element,
        pattern: &pattern,
    };
    common::apply(&mut app, hwnd, "caret doc 0");
    let remote = units_both(target, TextFrom::Caret, (None, TextUnit_Line, 6), false);
    assert_eq!(
        remote,
        (
            0,
            vec![
                ("one\n".to_owned(), 0, None),
                ("two\n".to_owned(), 0, None),
                ("three\n".to_owned(), 0, None),
            ],
            true
        )
    );
    app.quit();
}

/// The line `read` reaches from `at` by `pages` pages: how far it moved and
/// its text.
fn by_page<S: TextSource>(
    source: &mut S,
    anchors: &mut NodeText<'_, S::Pos>,
    at: TextPoint,
    pages: i32,
) -> (i32, TextChunk) {
    chunk(read(
        source,
        anchors,
        at,
        Some((TextUnit::Page, pages)),
        TextUnit::Line,
    ))
}

/// Review's page movement over UIA text whose provider has pages
/// (`pages.json`, two lines a page) lands on the first line of the next or
/// previous page and moves nothing past the last; over an edit control,
/// which has no page unit, it is not supported.
fn page_movement_answers_from_the_providers_pages() {
    common::init_com();
    let title = common::unique_title("mockapp-text-pages");
    let app = common::spawn("pages.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd, true);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);

    let (moved, third) = by_page(&mut source, &mut anchors, TextPoint::Start, 1);
    assert_eq!((moved, third.text.as_str()), (1, "three\n"));
    let (moved, fifth) = by_page(
        &mut source,
        &mut anchors,
        TextPoint::At(point_of(&third)),
        1,
    );
    assert_eq!((moved, fifth.text.as_str()), (1, "five\n"));
    let (moved, last) = by_page(
        &mut source,
        &mut anchors,
        TextPoint::At(point_of(&fifth)),
        1,
    );
    assert_eq!((moved, last.text.as_str()), (0, "five\n"), "the last page");
    let (moved, back) = by_page(
        &mut source,
        &mut anchors,
        TextPoint::At(point_of(&fifth)),
        -1,
    );
    assert_eq!((moved, back.text.as_str()), (-1, "three\n"));
    app.quit();

    let title = common::unique_title("mockapp-text-pages-edit");
    let app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    let mut source = edit_notes(hwnd);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    assert_eq!(
        read(
            &mut source,
            &mut anchors,
            TextPoint::Start,
            Some((TextUnit::Page, 1)),
            TextUnit::Line,
        ),
        Err(TextReply::UnsupportedUnit(TextUnit::Page))
    );
    app.quit();
}

/// A location is answered with where the provider places the point: over
/// UIA, the top left of the character's bounding rectangle, which mockapp
/// draws 8 pixels wide and 16 high from (100, 200), so "beta", from the
/// first line's seventh character, is at (148, 200), where a location
/// taken from the whole line would be at (100, 200); over an edit control,
/// the character's offset, the text's length, and its place as
/// `EM_POSFROMCHAR` gives it in the control, on the screen.
fn a_location_is_answered_from_where_the_provider_places_the_text() {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Controls::EM_POSFROMCHAR;
    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
    common::init_com();
    let title = common::unique_title("mockapp-text-location");
    let app = common::spawn("text.json", "uia", &title);
    let hwnd = common::find_window(&title);
    let mut source = uia_notes(hwnd, true);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    let (_, first) = chunk(read(
        &mut source,
        &mut anchors,
        TextPoint::Start,
        None,
        TextUnit::Line,
    ));
    assert_eq!(
        first.text,
        "alpha beta
"
    );
    let beta = TextPosition {
        offset: 6,
        ..point_of(&first)
    };
    assert_eq!(
        locate(&mut source, &mut anchors, TextPoint::At(beta)),
        Located {
            screen: Some((148, 200)),
            offset: None,
        }
    );
    app.quit();

    let title = common::unique_title("mockapp-text-location-edit");
    let app = common::spawn("text.json", "msaa", &title);
    let hwnd = common::find_window(&title);
    // SAFETY: a local search of mockapp's window's children by class.
    let edit = unsafe { FindWindowExW(Some(hwnd), None, w!("EDIT"), None) }
        .expect("mockapp's edit control");
    // Where the control places "gamma", at character 12 after
    // "alpha beta\r\n", read before the outpost is asked: the control's
    // client coordinates in the answer's two low words, then on the screen.
    // SAFETY: EM_POSFROMCHAR takes and answers plain integers.
    let packed = unsafe { SendMessageW(edit, EM_POSFROMCHAR, Some(WPARAM(12)), None) }.0;
    let word = |shift: isize| u16::try_from((packed >> shift) & 0xFFFF).expect("a 16-bit word");
    let mut expected = POINT {
        x: i32::from(word(0).cast_signed()),
        y: i32::from(word(16).cast_signed()),
    };
    // SAFETY: converts a local point; the control is alive.
    assert!(unsafe { ClientToScreen(edit, &raw mut expected) }.as_bool());
    let mut source = edit_notes(hwnd);
    let mut store = Anchors::new(Arc::default());
    let mut anchors = store.node(1);
    let (_, second) = chunk(read(
        &mut source,
        &mut anchors,
        TextPoint::Start,
        Some((TextUnit::Line, 1)),
        TextUnit::Line,
    ));
    assert_eq!(second.text, "gamma\r\n");
    assert_eq!(
        locate(&mut source, &mut anchors, TextPoint::At(point_of(&second))),
        Located {
            screen: Some((expected.x, expected.y)),
            offset: Some(TextOffset {
                offset: 12,
                length: 19,
            }),
        }
    );
    app.quit();
}

/// Runs this file's tests through the UIA test runner, which explains why
/// these binaries do not exit normally (`common/harness.rs`).
fn main() {
    harness::run_isolated(&[
        (
            "page_movement_answers_from_the_providers_pages",
            page_movement_answers_from_the_providers_pages,
        ),
        (
            "a_location_is_answered_from_where_the_provider_places_the_text",
            a_location_is_answered_from_where_the_provider_places_the_text,
        ),
        (
            "reading_ahead_ends_where_lines_clamp_back_both_ways",
            reading_ahead_ends_where_lines_clamp_back_both_ways,
        ),
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
