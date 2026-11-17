//! Caret tells as event-queue entries and Core's text facts (Table 17):
//! the caret read naming the selection baseline, the difference said in
//! specific or general terms, the edit control's repeated offsets, the
//! text-changed flag set by a value change and cleared by a comparison,
//! and the baseline minted, kept and dropped.

use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::{
    FocusCaret, FocusCaretRead, Outcome as WireOutcome, Refusal, Reply, Request as Wire, RunRead,
    Tried,
};
use verbatim_model::tell::{ElementToken, EventObject, Payload};
use verbatim_model::{
    Backend, CaretReport, CaretSelection, EditSelection, Effect as Said, NormalizedEvent, Observed,
    Phrase, Role, SegmentContent, Selection, SelectionChange, SelectionDifference, SelectionRead,
    SelectionReadReply, SelectionText, TextAnchor, TextChunk, TextOp, TextPosition, TextProvider,
    TextReply, TextUnit, WindowHandle,
};

use crate::entries::tests::{
    APP, Held, LISTENER, OUTPOST, answer, answer_outcome, entry, feed_at, focus_read, held,
    msaa_tell, node, ready, uia_tell, values,
};
use crate::entries::{Entry, feed};
use crate::event_queue::{Effect, Foreground, Input, Timed};

/// The application's window 4 in front.
const IN_FRONT: Foreground = Foreground {
    window: WindowHandle(4),
    root_owner: WindowHandle(4),
    process: APP,
    thread: 41,
    by_class: None,
    shell: false,
};

/// The edit field "Body" committed through UI Automation, node 2 of
/// runtime id 42.1 in window 5; the next request is 3.
fn uia_field() -> Held {
    let mut held = ready();
    let _ = feed_at(&mut held, 500, Input::Foreground(IN_FRONT));
    let mut read = focus_read(None);
    read.focus.read.values = values("Body", Role::EditableText);
    let _ = feed_at(
        &mut held,
        600,
        answer(OUTPOST, 1, Reply::Focus(Box::new(read))),
    );
    held
}

/// The edit control "Name" committed through MSAA, node 2 in window 5,
/// read through its messages; the next request is 2.
fn edit_control() -> Held {
    let mut state = ready();
    let _ = feed_at(&mut state, 500, entry(msaa_tell(-4)));
    let mut read = focus_read(Some(held(-4)));
    read.focus.read.values = values("Name", Role::EditableText);
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    let _ = feed_at(&mut state, 600, answer(OUTPOST, 1, Reply::Run(tried)));
    state
}

/// A UI Automation text-selection change of the element of runtime id
/// 42.`runtime`, told by the outpost following it.
fn uia_caret(runtime: i32) -> Input<Entry> {
    let mut tell = uia_tell(runtime, Vec::new());
    tell.kind = EventKind::UiaTextSelectionChanged;
    tell.checked = None;
    Input::Entry(Entry::of_tell(OUTPOST, tell).expect("a caret tell"))
}

/// The system caret's location change in window 5, told by the listener.
fn system_caret() -> Input<Entry> {
    let mut tell = msaa_tell(-8);
    tell.kind = EventKind::SystemCaret;
    Input::Entry(Entry::of_tell(LISTENER, tell).expect("a caret tell"))
}

/// An MSAA value change of window 5's client object.
fn value_change() -> Input<Entry> {
    let mut tell = msaa_tell(-4);
    tell.kind = EventKind::ValueChange;
    entry(tell)
}

/// What one step did: each text request by its number and what it asks,
/// each other request, each utterance by its segments, and each event the
/// control plane's stream carries.
#[derive(Debug, Default, PartialEq)]
struct Step {
    text: Vec<(u64, TextOp)>,
    other: usize,
    said: Vec<Vec<String>>,
    observed: Vec<Observed>,
}

fn step(held: &mut Held, at_us: u64, input: Input<Entry>) -> Step {
    let effects = feed(&mut held.state, &mut held.runtime, Timed { at_us, input }).unwrap();
    let mut step = Step::default();
    for effect in effects {
        match effect {
            Effect::Send { ask, .. } => match ask.request {
                Wire::Text { op, .. } => step.text.push((ask.number.0, op)),
                _ => step.other += 1,
            },
            Effect::Said(Said::Speak(utterance, _)) => step.said.push(
                utterance
                    .segments
                    .iter()
                    .map(|segment| format!("{:?}", segment.content))
                    .collect(),
            ),
            Effect::Said(Said::Observed(observed)) => step.observed.push(*observed),
            _ => {}
        }
    }
    step
}

/// A caret tell's read through `provider` naming `baseline`, adopting the
/// UI Automation tell's element (token 9) or, for the system caret's,
/// none.
fn read_naming(provider: TextProvider, baseline: Option<u64>) -> TextOp {
    read_adopting(provider, baseline, Some(ElementToken(9)))
}

fn read_adopting(
    provider: TextProvider,
    baseline: Option<u64>,
    adopt: Option<ElementToken>,
) -> TextOp {
    TextOp::ReadSelection(SelectionRead {
        provider,
        baseline: baseline.map(TextAnchor),
        adopt,
    })
}

fn line(text: &str, offset: u32) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text: text.to_owned(),
        start: TextAnchor(12),
        offset,
        languages: Vec::new(),
        first: true,
        last: true,
        truncated: false,
        formats: Vec::new(),
    }
}

/// The caret on "alpha beta" at byte `offset`, with `selected` bytes
/// selected from it.
fn caret(offset: u32, selected: u32) -> CaretReport {
    CaretReport {
        line: line("alpha beta", offset),
        selection: (selected > 0).then(|| CaretSelection {
            range: Selection {
                start: TextPosition {
                    anchor: TextAnchor(12),
                    offset,
                },
                end: TextPosition {
                    anchor: TextAnchor(12),
                    offset: offset + selected,
                },
            },
        }),
        read_at_us: 0,
    }
}

fn change(selected: bool, text: &str) -> SelectionChange {
    SelectionChange {
        selected,
        text: text.to_owned(),
        characters: u32::try_from(text.chars().count()).unwrap(),
    }
}

/// The answer to request `number`: `caret`, baseline `baseline` minted,
/// the difference from the named baseline when there was one, and the
/// edit control's offsets.
fn selection_read(
    number: u64,
    (caret, baseline): (CaretReport, u64),
    difference: Option<SelectionDifference>,
    edit_selection: Option<EditSelection>,
) -> Input<Entry> {
    answer(
        OUTPOST,
        number,
        Reply::Text(TextReply::SelectionRead(Box::new(SelectionReadReply {
            caret,
            baseline: TextAnchor(baseline),
            difference,
            edit_selection,
        }))),
    )
}

fn differing(changes: Vec<SelectionChange>, new_selection: Option<&str>) -> SelectionDifference {
    SelectionDifference {
        changes,
        new_selection: new_selection.map(|text| change(true, text)),
    }
}

fn phrase(phrase: Phrase) -> Vec<String> {
    vec![format!("{:?}", SegmentContent::Phrase(phrase))]
}

fn character(text: &str) -> SelectionText {
    SelectionText::Character(text.to_owned())
}

fn text(text: &str) -> SelectionText {
    SelectionText::Text(text.to_owned())
}

const SELECTION_REMOVED: &str = "Message(SelectionRemoved)";

/// The baseline Core holds for the committed focus.
fn baseline(held: &Held) -> Option<TextAnchor> {
    crate::text_facts::held(&held.state).map(|(_, baseline)| baseline)
}

/// The committed focus's text-changed flag.
fn text_changed(held: &Held) -> bool {
    held.state
        .versions
        .committed_read()
        .expect("a focus is committed")
        .selection
        .text_changed
}

/// `held`'s committed field with baseline 70 minted by a first caret
/// tell's read (request `number`), named by none.
fn with_baseline(held: &mut Held, tell: Input<Entry>, provider: TextProvider, number: u64) {
    let adopt = match &tell {
        Input::Entry(Entry::Caret(caret)) => caret.adopted(),
        _ => None,
    };
    let asked = step(held, 1_000, tell);
    assert_eq!(asked.text, [(number, read_adopting(provider, None, adopt))]);
    let answered = step(
        held,
        1_100,
        selection_read(number, (caret(1, 0), 70), None, None),
    );
    assert_eq!(answered.said, Vec::<Vec<String>>::new());
    assert_eq!(baseline(held), Some(TextAnchor(70)));
}

/// The edit control's value change as the control plane's stream carries
/// it, as the outpost reported it before Core took its tells: a text change
/// of the committed field, node 2, read through MSAA.
fn text_change() -> Observed {
    Observed {
        trace_id: verbatim_model::TraceId::from_raw(7),
        source: APP,
        backend: Backend::Msaa,
        window: None,
        event: NormalizedEvent::TextChanged { node_id: node(2) },
    }
}

/// The edit control's value change is the control plane's text change
/// whether or not the field detects selection, which the harness waits on
/// after a paste (`text_box.rs`, `paste`); a caret tell's read is its caret
/// move.
#[test]
fn an_edit_controls_value_change_and_a_caret_read_are_the_control_planes_events() {
    let mut held = edit_control();
    let changed = step(&mut held, 1_500, value_change());
    assert_eq!(changed.observed, [text_change()]);
    let _ = step(&mut held, 2_000, system_caret());
    let read = step(
        &mut held,
        2_100,
        selection_read(
            2,
            (caret(1, 3), 70),
            Some(differing(vec![change(true, "lph")], Some("lph"))),
            Some(EditSelection { start: 1, end: 4 }),
        ),
    );
    assert_eq!(
        read.observed,
        [Observed {
            trace_id: verbatim_model::TraceId::from_raw(7),
            source: APP,
            backend: Backend::Msaa,
            window: None,
            event: NormalizedEvent::CaretMoved {
                node_id: node(2),
                caret: caret(1, 3),
                selection_changes: vec![change(true, "lph")],
                content_changed: false,
                new_selection: None,
                facts: None,
            },
        }]
    );
}

#[test]
fn a_caret_tell_with_no_baseline_mints_one_says_nothing_and_takes_the_caret() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let caret = held
        .state
        .focus_caret(node(2))
        .expect("the caret is Core's");
    assert_eq!(
        (caret.line.text.as_str(), caret.line.offset),
        ("alpha beta", 1)
    );
    assert!(!text_changed(&held));
}

#[test]
fn a_caret_tell_says_what_changed_from_the_baseline_in_specific_terms() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let asked = step(&mut held, 2_000, uia_caret(1));
    assert_eq!(asked.text, [(4, read_naming(TextProvider::Uia, Some(70)))]);
    let answered = step(
        &mut held,
        2_100,
        selection_read(
            4,
            (caret(1, 3), 71),
            Some(differing(vec![change(true, "lph")], Some("lph"))),
            None,
        ),
    );
    assert_eq!(answered.said, [phrase(Phrase::Selected(text("lph")))]);
    assert_eq!(baseline(&held), Some(TextAnchor(71)));
}

/// `notepad_selection_keys`, Backspace over the second word: the old range
/// now empty and nothing selected, nothing is said for the selection
/// (`notepad-second-word-backspace.txt`).
#[test]
fn nothing_is_said_when_the_old_range_and_the_new_selection_are_both_empty() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let _ = step(&mut held, 2_000, uia_caret(1));
    let answered = step(
        &mut held,
        2_100,
        selection_read(
            4,
            (caret(6, 0), 71),
            Some(differing(Vec::new(), None)),
            None,
        ),
    );
    assert_eq!(answered.said, Vec::<Vec<String>>::new());
    assert_eq!(baseline(&held), Some(TextAnchor(71)));
}

/// `notepad_selection_keys`, Enter over the first word: the old range now
/// holds the line break, said unselected (`notepad-word-enter.txt`).
#[test]
fn an_old_range_now_holding_other_text_is_said_as_it_now_is() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let _ = step(&mut held, 2_000, uia_caret(1));
    let answered = step(
        &mut held,
        2_100,
        selection_read(
            4,
            (caret(0, 0), 71),
            Some(differing(vec![change(false, "\r")], None)),
            None,
        ),
    );
    assert_eq!(answered.said, [phrase(Phrase::Unselected(character("\r")))]);
}

#[test]
fn a_caret_tell_on_another_element_asks_nothing() {
    let mut held = uia_field();
    let asked = step(&mut held, 1_000, uia_caret(7));
    assert_eq!(asked, Step::default());
    assert_eq!(baseline(&held), None);
}

/// A selection change no key made, its caret tell waiting while a focus
/// becomes pending behind it: unsaid, as NVDA's caret event speaks only
/// with no focus pending. Needs capture: written to the design's derived
/// speech ("What users hear"), awaiting NVDA's capture.
#[test]
fn a_caret_tell_with_a_focus_pending_asks_nothing() {
    let mut held = uia_field();
    // The first caret tell's read holds the front; the system caret's
    // tell waits, then a focus counting as pending joins behind it.
    let _ = step(&mut held, 1_000, uia_caret(1));
    let _ = step(&mut held, 1_010, system_caret());
    let _ = step(&mut held, 1_020, entry(uia_tell(2, Vec::new())));
    let answered = step(
        &mut held,
        1_100,
        selection_read(3, (caret(1, 0), 70), None, None),
    );
    // The waiting caret tell ends with no read; the focus asks its read.
    assert_eq!(answered.text, Vec::new());
    assert_eq!(answered.other, 1);
}

#[test]
fn a_failed_caret_read_keeps_the_baseline_and_the_flag() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let _ = step(&mut held, 1_500, value_change());
    assert!(text_changed(&held));
    let _ = step(&mut held, 2_000, system_caret());
    let failed = step(
        &mut held,
        2_100,
        answer_outcome(OUTPOST, 3, WireOutcome::Refused(Refusal::NotFocused)),
    );
    assert_eq!(failed.said, Vec::<Vec<String>>::new());
    assert_eq!(baseline(&held), Some(TextAnchor(70)));
    assert!(text_changed(&held));
}

/// Table 17, columns G then D: an edit control's value change sets the
/// flag, saying nothing; the next comparison speaks in general terms and
/// clears it, so the one after speaks in specific terms again.
#[test]
fn a_value_change_makes_the_next_difference_general_and_is_cleared_by_it() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let changed = step(&mut held, 1_500, value_change());
    assert_eq!(
        changed,
        Step {
            observed: vec![text_change()],
            ..Step::default()
        }
    );
    assert!(text_changed(&held));

    let asked = step(&mut held, 2_000, system_caret());
    assert_eq!(
        asked.text,
        [(3, read_adopting(TextProvider::EditMessages, Some(70), None))]
    );
    let general = step(
        &mut held,
        2_100,
        selection_read(
            3,
            (caret(1, 0), 71),
            Some(differing(vec![change(false, "lph")], None)),
            Some(EditSelection { start: 1, end: 1 }),
        ),
    );
    assert_eq!(general.said, [vec![SELECTION_REMOVED.to_owned()]]);
    assert!(!text_changed(&held));

    let _ = step(&mut held, 3_000, system_caret());
    let specific = step(
        &mut held,
        3_100,
        selection_read(
            4,
            (caret(1, 1), 72),
            Some(differing(vec![change(true, "l")], Some("l"))),
            Some(EditSelection { start: 1, end: 2 }),
        ),
    );
    assert_eq!(specific.said, [phrase(Phrase::Selected(character("l")))]);
}

/// `text_box_selection_keys`, Delete over the first word: the value
/// change ordered behind the caret tell still waits at the caret read's
/// answer, so the terms are general, "selection removed", and the value
/// change, counted, does not set the flag again.
#[test]
fn a_value_change_waiting_at_the_answer_makes_the_terms_general_and_is_counted() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let _ = step(&mut held, 2_000, system_caret());
    let _ = step(&mut held, 2_010, value_change());
    let answered = step(
        &mut held,
        2_100,
        selection_read(
            3,
            (caret(0, 0), 71),
            Some(differing(vec![change(false, "alpha")], None)),
            Some(EditSelection { start: 0, end: 0 }),
        ),
    );
    assert_eq!(answered.said, [vec![SELECTION_REMOVED.to_owned()]]);
    assert!(!text_changed(&held));
    assert_eq!(held.state.queue.waiting_for_tests(), 0);
}

/// `text_box_review_cursor`'s paste: the value change waiting at the caret
/// read's answer is taken as counted, and is still the control plane's
/// text change, which the harness waits on after a paste.
#[test]
fn a_value_change_a_caret_tell_takes_as_counted_is_still_the_control_planes_text_change() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let _ = step(&mut held, 2_000, system_caret());
    let _ = step(&mut held, 2_010, value_change());
    let answered = step(
        &mut held,
        2_100,
        selection_read(
            3,
            (caret(4, 0), 71),
            Some(differing(Vec::new(), None)),
            Some(EditSelection { start: 4, end: 4 }),
        ),
    );
    assert_eq!(held.state.queue.waiting_for_tests(), 0);
    assert_eq!(
        answered
            .observed
            .iter()
            .filter(|observed| matches!(observed.event, NormalizedEvent::TextChanged { .. }))
            .collect::<Vec<_>>(),
        [&text_change()]
    );
}

/// NVDA's `Edit.event_caret` returns before comparing when the selection's
/// offsets are those of the last caret event: nothing said, the baseline
/// and flag kept.
#[test]
fn an_edit_controls_caret_tell_with_the_last_offsets_says_nothing() {
    let mut held = edit_control();
    let _ = step(&mut held, 1_000, system_caret());
    let _ = step(
        &mut held,
        1_100,
        selection_read(
            2,
            (caret(1, 0), 70),
            None,
            Some(EditSelection { start: 1, end: 1 }),
        ),
    );
    let _ = step(&mut held, 1_500, value_change());
    let _ = step(&mut held, 2_000, system_caret());
    let repeated = step(
        &mut held,
        2_100,
        selection_read(
            3,
            (caret(1, 0), 71),
            Some(differing(vec![change(true, "a")], Some("a"))),
            Some(EditSelection { start: 1, end: 1 }),
        ),
    );
    assert_eq!(repeated.said, Vec::<Vec<String>>::new());
    assert_eq!(baseline(&held), Some(TextAnchor(70)));
    assert!(text_changed(&held));
}

/// Table 17, column H: the baseline's anchor ends with its generation;
/// the flag is the focus's and stays.
#[test]
fn a_generation_end_drops_the_baseline_and_keeps_the_flag() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let _ = step(&mut held, 1_500, value_change());
    let _ = step(&mut held, 2_000, Input::ReaderEnded(OUTPOST));
    let kept = held
        .state
        .versions
        .committed_kept()
        .expect("the ended focus is kept");
    assert_eq!(kept.selection.baseline, None);
    assert!(kept.selection.text_changed);
}

/// Table 17, column C: a failed focus read clears the baseline and the
/// flag. NVDA: `editableText.py` 421 to 424.
#[test]
fn a_failed_focus_read_clears_the_baseline_and_the_flag() {
    let mut held = edit_control();
    with_baseline(&mut held, system_caret(), TextProvider::EditMessages, 2);
    let _ = step(&mut held, 1_500, value_change());
    let _ = step(&mut held, 2_000, entry(msaa_tell(-5)));
    let _ = step(
        &mut held,
        2_100,
        answer_outcome(OUTPOST, 3, WireOutcome::NotThere),
    );
    assert_eq!(baseline(&held), None);
    assert!(!text_changed(&held));
}

/// A UI Automation value change of an element with a text pattern says
/// nothing (NVDA: `NVDAObjects/UIA/__init__.py` 2514 to 2516).
#[test]
fn a_uia_value_change_of_an_element_with_a_text_pattern_says_nothing() {
    let mut held = uia_field();
    let mut tell = uia_tell(1, Vec::new());
    tell.kind = EventKind::UiaPropertyChanged;
    tell.checked = None;
    tell.cached.0.push(PropertyAnswer {
        id: PropertyId::HasTextPattern,
        value: Some(PropertyValue::Flag(true)),
    });
    tell.payload = Payload::PropertyChanged {
        property: PropertyId::Value,
        value: Some(PropertyValue::Text("alpha beta".to_owned())),
    };
    let said = step(
        &mut held,
        1_000,
        Input::Entry(Entry::of_tell(OUTPOST, tell).expect("a change")),
    );
    assert_eq!(said, Step::default());
}

#[test]
fn the_baseline_and_every_anchor_a_queued_request_names_are_held() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let _ = step(&mut held, 2_000, uia_caret(1));
    let anchors = held.state.held_anchors();
    let outpost = anchors
        .get(&verbatim_model::OutpostId(
            crate::entries::tests::GENERATION,
        ))
        .expect("the outpost's anchors");
    // The caret's line, and the baseline, held and named by the read out.
    assert_eq!(
        outpost.iter().copied().collect::<Vec<_>>(),
        [TextAnchor(12), TextAnchor(70)]
    );
}

/// The system caret's tell is for the readers' last focus, whatever its
/// window and backend, as NVDA's MSAA caret event is
/// (`IAccessibleHandler/__init__.py` 634 to 666).
#[test]
fn the_system_carets_tell_reads_the_committed_focus_through_its_provider() {
    let mut held = uia_field();
    let asked = step(&mut held, 1_000, system_caret());
    assert_eq!(
        asked.text,
        [(3, read_adopting(TextProvider::Uia, None, None))]
    );
}

/// A UI Automation caret tell whose element the outpost could not keep
/// names it by its runtime id alone: its read is the same, adopting
/// nothing.
#[test]
fn a_uia_caret_tell_with_nothing_kept_reads_adopting_nothing() {
    let mut held = uia_field();
    let mut tell = uia_tell(1, Vec::new());
    tell.kind = EventKind::UiaTextSelectionChanged;
    tell.checked = None;
    tell.object = EventObject::Followed {
        runtime_id: verbatim_model::property::RuntimeId(vec![42, 1]),
    };
    let entry = Input::Entry(Entry::of_tell(OUTPOST, tell).expect("a caret tell"));
    let asked = step(&mut held, 1_000, entry);
    assert_eq!(
        asked.text,
        [(3, read_adopting(TextProvider::Uia, None, None))]
    );
}

/// Several caret tells waiting for one object merge into one entry, so
/// one comparison says what changed since the baseline (NVDA:
/// `IAccessibleHandler/orderedWinEventLimiter.py` 79). Needs capture for
/// Shift+Right three times faster than the application reports: written
/// to the design's derived speech, awaiting NVDA's capture.
#[test]
fn caret_tells_waiting_for_one_element_merge_into_one_read() {
    let mut held = uia_field();
    with_baseline(&mut held, uia_caret(1), TextProvider::Uia, 3);
    let _ = step(&mut held, 2_000, uia_caret(1));
    let _ = step(&mut held, 2_010, uia_caret(1));
    let _ = step(&mut held, 2_020, uia_caret(1));
    let first = step(
        &mut held,
        2_100,
        selection_read(
            4,
            (caret(0, 1), 71),
            Some(differing(vec![change(true, "a")], Some("a"))),
            None,
        ),
    );
    assert_eq!(first.said, [phrase(Phrase::Selected(character("a")))]);
    assert_eq!(first.text, [(5, read_naming(TextProvider::Uia, Some(71)))]);
    let merged = step(
        &mut held,
        2_200,
        selection_read(
            5,
            (caret(0, 3), 72),
            Some(differing(vec![change(true, "lp")], Some("alp"))),
            None,
        ),
    );
    assert_eq!(merged.said, [phrase(Phrase::Selected(text("lp")))]);
    assert_eq!(merged.text, Vec::new());
    assert_eq!(held.state.queue.waiting_for_tests(), 0);
}

/// A focus answer's caret read: `caret`, baseline `baseline` minted, and
/// the difference from the baseline the request named.
fn focus_caret(
    (caret, baseline): (CaretReport, u64),
    difference: Option<SelectionDifference>,
) -> FocusCaret {
    FocusCaret::Read(Box::new(FocusCaretRead {
        caret,
        selected_text: None,
        baseline: TextAnchor(baseline),
        difference,
    }))
}

/// An MSAA focus run in window 5 whose candidate is object `object`,
/// answered as request `number` by the edit control "Name", node 2,
/// carrying `caret`; what the step and the answer said.
fn edit_focus(held: &mut Held, (object, number): (i32, u64), caret: FocusCaret) -> Vec<String> {
    let mut sent = feed_at(held, 500 * number, entry(msaa_tell(object)));
    let mut read = focus_read(Some(crate::entries::tests::held(object)));
    read.focus.read.values = values("Name", Role::EditableText);
    read.caret = caret;
    let tried = RunRead {
        tried: vec![Tried::Focus(Box::new(read))],
    };
    sent.extend(feed_at(
        held,
        500 * number + 100,
        answer(OUTPOST, number, Reply::Run(tried)),
    ));
    sent
}

/// The baseline the step's focus request names.
fn named_baseline(held: &mut Held, object: i32) -> Option<TextAnchor> {
    let effects = feed(
        &mut held.state,
        &mut held.runtime,
        Timed {
            at_us: 10_000,
            input: entry(msaa_tell(object)),
        },
    )
    .unwrap();
    effects.into_iter().find_map(|effect| match effect {
        Effect::Send { ask, .. } => match ask.request {
            Wire::MsaaRun(run) => Some(run.baseline),
            _ => None,
        },
        _ => None,
    })?
}

/// Table 17, column A: a new focus's caret becomes Core's, its line said
/// with the focus, and its baseline minted with no change said, whatever
/// the difference the answer carries.
#[test]
fn a_new_focus_says_its_line_and_mints_the_baseline_with_no_change() {
    let mut held = ready();
    let said = edit_focus(
        &mut held,
        (-4, 1),
        focus_caret(
            (caret(1, 0), 80),
            Some(differing(vec![change(true, "a")], Some("a"))),
        ),
    );
    assert_eq!(
        said,
        [
            "send Outpost(Pid(40)) MsaaRun",
            "Label(\"Editor\") Role(Window)",
            "Label(\"Name\") Role(EditableText)",
            "Text(\"alpha beta\")",
        ]
    );
    assert_eq!(baseline(&held), Some(TextAnchor(80)));
    assert!(!text_changed(&held));
    let caret = held
        .state
        .focus_caret(node(2))
        .expect("the caret is Core's");
    assert_eq!(caret.line.offset, 1);
}

/// Table 17, column B in T3: a repeated focus's request names the
/// baseline, and its answer's change from it is said in specific terms;
/// the next baseline replaces it.
#[test]
fn a_repeated_focus_says_the_change_from_the_named_baseline() {
    let mut held = ready();
    let _ = edit_focus(&mut held, (-4, 1), focus_caret((caret(1, 0), 80), None));
    let mut probe = held.clone();
    assert_eq!(named_baseline(&mut probe, -8), Some(TextAnchor(80)));
    let said = edit_focus(
        &mut held,
        (-8, 2),
        focus_caret(
            (caret(0, 1), 81),
            Some(differing(vec![change(true, "a")], Some("a"))),
        ),
    );
    assert_eq!(
        said,
        [
            "send Outpost(Pid(40)) MsaaRun",
            "Label(\"Name\") Role(EditableText)",
            "Text(\"alpha beta\")",
            "Phrase(Selected(Character(\"a\")))",
        ]
    );
    assert_eq!(baseline(&held), Some(TextAnchor(81)));
}

/// Table 17, column B in T4: with the text-changed flag set, a repeated
/// focus says the change in general terms and clears the flag.
#[test]
fn a_repeated_focus_after_a_value_change_says_the_change_in_general_terms() {
    let mut held = ready();
    let _ = edit_focus(&mut held, (-4, 1), focus_caret((caret(1, 0), 80), None));
    let _ = step(&mut held, 800, value_change());
    assert!(text_changed(&held));
    let said = edit_focus(
        &mut held,
        (-8, 2),
        focus_caret(
            (caret(0, 3), 81),
            Some(differing(vec![change(true, "alp")], Some("alp"))),
        ),
    );
    assert_eq!(
        said,
        [
            "send Outpost(Pid(40)) MsaaRun",
            "Label(\"Name\") Role(EditableText)",
            "Text(\"alpha beta\")",
            "Phrase(Selected(Text(\"alp\")))",
        ]
    );
    assert!(!text_changed(&held));
    assert_eq!(baseline(&held), Some(TextAnchor(81)));
}

/// Table 17, column C: a focus answer with no caret lets the baseline go
/// and clears the flag, and Core holds no caret.
#[test]
fn a_focus_answer_with_no_caret_clears_the_baseline_and_the_flag() {
    let mut held = ready();
    let _ = edit_focus(&mut held, (-4, 1), focus_caret((caret(1, 0), 80), None));
    let _ = step(&mut held, 800, value_change());
    let _ = edit_focus(&mut held, (-8, 2), FocusCaret::NotRead);
    assert_eq!(baseline(&held), None);
    assert!(!text_changed(&held));
    assert_eq!(held.state.focus_caret(node(2)), None);
}
