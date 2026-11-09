//! The text protocol over an in-memory source: chunks, positions and
//! anchors, a key's reads compared with the last read, selection changes, and
//! reads.

use std::sync::atomic::AtomicU64;

use verbatim_model::{
    CaretMotion, CaretRead, CaretReply, Change, KeyFetch, NodeKind, ReadFacts, ReadPurpose,
};

use super::*;

/// An in-memory text whose positions are UTF-16 offsets. Paragraphs end
/// after each line feed, and a line is a paragraph, or, with `wrap`, each
/// run of that many units of one; a word is a run of letters and digits
/// with the spaces after it, or one other character.
struct Fake {
    text: Vec<u16>,
    caret: usize,
    /// The width at which lines wrap, if any.
    wrap: Option<usize>,
    selection: Option<(usize, usize)>,
    sentences: Sentences,
    /// How many times the caret was read.
    caret_reads: usize,
    /// Moves the caret to this offset on the given caret read, collapsing
    /// any selection, as an application handling a key a little later
    /// would.
    moves_on_read: Option<(usize, usize)>,
}

impl Fake {
    fn new(text: &str, caret: usize) -> Self {
        Self {
            text: text.encode_utf16().collect(),
            caret,
            wrap: None,
            selection: None,
            sentences: Sentences::ByParagraph,
            caret_reads: 0,
            moves_on_read: None,
        }
    }

    fn lines(&self) -> Vec<(usize, usize)> {
        let Some(width) = self.wrap else {
            return self.paragraphs();
        };
        self.paragraphs()
            .into_iter()
            .flat_map(|(start, end)| {
                (start..end.max(start + 1))
                    .step_by(width)
                    .map(move |from| (from, end.min(from + width)))
            })
            .collect()
    }

    fn paragraphs(&self) -> Vec<(usize, usize)> {
        let mut lines = Vec::new();
        let mut start = 0;
        for (index, &unit) in self.text.iter().enumerate() {
            if unit == u16::from(b'\n') {
                lines.push((start, index + 1));
                start = index + 1;
            }
        }
        lines.push((start, self.text.len()));
        lines
    }

    fn words(&self) -> Vec<(usize, usize)> {
        let word = |unit: u16| char::from_u32(u32::from(unit)).is_some_and(char::is_alphanumeric);
        let mut words = Vec::new();
        let mut index = 0;
        while index < self.text.len() {
            let start = index;
            if word(self.text[index]) {
                while index < self.text.len() && word(self.text[index]) {
                    index += 1;
                }
                while index < self.text.len() && self.text[index] == u16::from(b' ') {
                    index += 1;
                }
            } else {
                index += 1;
            }
            words.push((start, index));
        }
        words
    }

    fn units(&self, unit: TextUnit) -> Option<Vec<(usize, usize)>> {
        match unit {
            TextUnit::Line => Some(self.lines()),
            TextUnit::Paragraph => Some(self.paragraphs()),
            TextUnit::Word => Some(self.words()),
            TextUnit::Character => Some((0..self.text.len()).map(|i| (i, i + 1)).collect()),
            _ => None,
        }
    }

    fn containing(&self, at: usize, unit: TextUnit) -> Option<(usize, usize, usize)> {
        let units = self.units(unit)?;
        let index = units
            .iter()
            .position(|&(_, end)| at < end || end == self.text.len() && at == end)
            .unwrap_or(units.len().saturating_sub(1));
        let (start, end) = units.get(index).copied().unwrap_or((at, at));
        Some((index, start, end))
    }
}

impl TextSource for Fake {
    type Pos = usize;

    fn caret(&mut self) -> TextResult<CaretState<usize>> {
        self.caret_reads += 1;
        if let Some((read, to)) = self.moves_on_read
            && read == self.caret_reads
        {
            self.caret = to;
            self.selection = None;
        }
        Ok(CaretState {
            caret: self.caret,
            selection: self.selection,
        })
    }

    fn start(&mut self) -> TextResult<usize> {
        Ok(0)
    }

    fn end(&mut self) -> TextResult<usize> {
        Ok(self.text.len())
    }

    fn unit_at(
        &mut self,
        at: &usize,
        unit: TextUnit,
        max_units: usize,
    ) -> TextResult<Option<Unit<usize>>> {
        Ok(self.containing(*at, unit).map(|(_, start, end)| {
            let cut = end.min(start + max_units);
            Unit {
                start,
                end,
                text: self.text[start..cut].to_vec(),
                truncated: cut < end,
            }
        }))
    }

    fn move_by(
        &mut self,
        at: &usize,
        unit: TextUnit,
        count: i32,
    ) -> TextResult<Option<(usize, i32)>> {
        let Some((index, _, _)) = self.containing(*at, unit) else {
            return Ok(None);
        };
        let units = self.units(unit).expect("the unit exists");
        let target = (i64::try_from(index).unwrap() + i64::from(count))
            .clamp(0, i64::try_from(units.len()).unwrap() - 1);
        let moved = i32::try_from(target - i64::try_from(index).unwrap()).unwrap();
        Ok(Some((units[usize::try_from(target).unwrap()].0, moved)))
    }

    fn text(
        &mut self,
        start: &usize,
        end: &usize,
        max_units: usize,
    ) -> TextResult<(Vec<u16>, bool)> {
        let cut = (*end).min(start + max_units);
        Ok((self.text[*start..cut].to_vec(), cut < *end))
    }

    fn offset_in(&mut self, unit: &Unit<usize>, at: &usize) -> TextResult<usize> {
        Ok(at.saturating_sub(unit.start).min(unit.text.len()))
    }

    fn advance(&mut self, from: &usize, prefix: &[u16]) -> TextResult<usize> {
        Ok(from + prefix.len())
    }

    fn compare(&mut self, a: &usize, b: &usize) -> TextResult<Ordering> {
        Ok(a.cmp(b))
    }

    fn select(&mut self, start: &usize, end: &usize) -> TextResult<bool> {
        self.selection = (start != end).then_some((*start, *end));
        self.caret = *start;
        Ok(true)
    }

    fn location(&mut self, at: &usize) -> TextResult<Option<(i32, i32)>> {
        Ok(Some((i32::try_from(*at).unwrap() * 10, 5)))
    }

    fn languages(&mut self, _unit: &Unit<usize>) -> Vec<(usize, usize, String)> {
        Vec::new()
    }

    fn edges(&mut self, unit: &Unit<usize>, _kind: TextUnit) -> (bool, bool) {
        (unit.start == 0, unit.end == self.text.len())
    }

    fn sentences(&self) -> Sentences {
        self.sentences
    }
}

/// When [`report`] reads the caret, in microseconds.
const REPORTED_AT: u64 = 1_000_000;

fn store() -> Anchors<usize> {
    Anchors::new(Arc::new(AtomicU64::new(0)))
}

fn report(source: &mut Fake, anchors: &mut Anchors<usize>) -> CaretReport {
    caret_report(source, &mut anchors.node(1), REPORTED_AT, false)
        .expect("the fake answers")
        .0
}

/// A report that compares its selection with the last read.
fn compared(source: &mut Fake, anchors: &mut Anchors<usize>) -> ReportedCaret<usize> {
    caret_report_since(
        source,
        &mut anchors.node(1),
        REPORTED_AT + 1,
        &ReportAsk {
            formats: false,
            since: None,
            compare: true,
            fetch: None,
        },
    )
    .expect("the fake answers")
}

/// The ask of a caret read that compares its selection.
fn comparing<'a>() -> ReportAsk<'a, usize> {
    ReportAsk {
        formats: false,
        since: None,
        compare: true,
        fetch: None,
    }
}

/// Pins `anchors`' last read for a key.
fn pinned(anchors: &mut Anchors<usize>) -> LastRead<usize> {
    anchors
        .node(1)
        .last_read()
        .expect("a read made the baseline")
}

/// One check of a key of `motion` judged from `baseline`, on node 1.
fn check(
    source: &mut Fake,
    anchors: &mut Anchors<usize>,
    _baseline: &LastRead<usize>,
    motion: CaretMotion,
) -> KeyChecked {
    check_key(
        source,
        &mut anchors.node(1),
        &CheckKey {
            note: NoteFetch {
                unit: motion.unit(),
                motion,
            },
            read_at_us: REPORTED_AT + 2,
            final_read: false,
            ended: || REPORTED_AT + 3,
        },
    )
}

/// A read for a key, its report and what it compared with apart.
struct Evidence {
    caret: verbatim_model::CaretReport,
    selection_changes: Vec<SelectionChange>,
    content_changed: bool,
    new_selection: Option<SelectionChange>,
    unit: verbatim_model::TextChunk,
    before_line: verbatim_model::TextChunk,
    on_baseline_line: bool,
}

impl Evidence {
    fn of(read: Box<CaretReply>) -> Self {
        let CaretReply {
            caret,
            selection_changes,
            content_changed,
            new_selection,
            facts: ReadFacts { unit, change, .. },
        } = *read;
        let Change::Compared {
            previous_line,
            on_previous_line,
            ..
        } = change
        else {
            panic!("no comparison: {change:?}");
        };
        Evidence {
            caret,
            selection_changes,
            content_changed,
            new_selection,
            unit,
            before_line: previous_line,
            on_baseline_line: on_previous_line,
        }
    }
}

/// The read of a check that found evidence.
fn answered(checked: KeyChecked) -> Evidence {
    match checked {
        KeyChecked::Read { read, effect: true } => Evidence::of(read),
        other => panic!("no evidence: {other:?}"),
    }
}

/// Whether a check found nothing.
fn nothing(checked: &KeyChecked) -> bool {
    matches!(checked, KeyChecked::Read { effect: false, .. })
}

#[test]
fn a_caret_report_is_the_line_with_the_caret_as_a_byte_offset() {
    // "é" is two bytes and one unit; the emoji two units and four bytes.
    let mut source = Fake::new("first\ncafé 😀 x\nlast", 13);
    let mut anchors = store();
    let report = report(&mut source, &mut anchors);
    assert_eq!(report.line.text, "café 😀 x\n");
    assert_eq!(report.line.unit, TextUnit::Line);
    assert_eq!(
        &report.line.text[report.line.offset as usize..],
        " x\n",
        "the caret sits after the emoji"
    );
    assert!(!report.line.first && !report.line.last);
    assert_eq!(report.selection, None);
}

#[test]
fn a_position_inside_a_chunk_resolves_through_its_text() {
    let mut source = Fake::new("first\ncafé 😀 x\nlast", 6);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    // The review cursor, at the "x", which Core found by slicing the text.
    let x = u32::try_from(line.text.find('x').unwrap()).unwrap();
    let reply = perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::Read(TextRead {
            at: TextPoint::At(TextPosition {
                anchor: line.start,
                offset: x,
            }),
            movement: None,
            unit: TextUnit::Word,
        }),
    );
    let TextReply::Read { moved: 0, chunk } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(chunk.text, "x");
    assert_eq!(chunk.offset, 0);
}

#[test]
fn an_anchor_is_kept_however_many_are_minted_until_it_is_forgotten() {
    let mut source = Fake::new("one\ntwo", 0);
    let mut anchors = store();
    let first = report(&mut source, &mut anchors).line.start;
    let kept = report(&mut source, &mut anchors).line.start;
    for _ in 0..100 {
        report(&mut source, &mut anchors);
    }
    drop(anchors.forget_anchors(&std::collections::HashSet::from([first.0])));
    let read = |anchors: &mut Anchors<usize>, source: &mut Fake, anchor| {
        perform(
            source,
            &mut anchors.node(1),
            &TextOp::Read(TextRead {
                at: TextPoint::At(TextPosition { anchor, offset: 1 }),
                movement: None,
                unit: TextUnit::Line,
            }),
        )
    };
    assert_eq!(
        read(&mut anchors, &mut source, first),
        TextReply::AnchorLost
    );
    assert!(matches!(
        read(&mut anchors, &mut source, kept),
        TextReply::Read { .. }
    ));
}

#[test]
fn a_keys_request_is_not_carried_out_by_perform() {
    // A key's check is made by `check_key`.
    let mut source = Fake::new("one", 0);
    let request = CaretRead {
        fetch: KeyFetch {
            unit: TextUnit::Character,
            motion: CaretMotion::NextCharacter,
            kind: NodeKind::TextField,
        },
        purpose: ReadPurpose::BeforeKey,
    };
    let reply = perform(
        &mut source,
        &mut store().node(1),
        &TextOp::ReadCaret(request),
    );
    assert_eq!(reply, TextReply::Unsupported);
    assert_eq!(source.caret_reads, 0);
}

#[test]
fn a_caret_that_left_where_the_baseline_had_it_is_evidence_at_once() {
    let mut source = Fake::new("one two\nthree", 0);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.caret = 4;
    source.caret_reads = 0;
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextWord,
    ));
    assert_eq!(source.caret_reads, 2, "the read and its recheck");
    assert_eq!(read.unit.text, "two");
    assert_eq!(read.caret.line.text, "one two\n");
    assert_eq!(read.caret.line.offset, 4);
    assert_eq!(read.before_line.text, "one two\n");
    assert!(read.on_baseline_line);
}

#[test]
fn with_no_evidence_the_check_reads_the_caret_once() {
    let mut source = Fake::new("only", 2);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.caret_reads = 0;
    let checked = check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    );
    assert!(nothing(&checked), "{checked:?}");
    assert_eq!(source.caret_reads, 1);
}

#[test]
fn a_caret_moved_later_answers_a_later_check() {
    let mut source = Fake::new("one\ntwo", 0);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.caret_reads = 0;
    source.moves_on_read = Some((2, 4));
    let checked = check(&mut source, &mut anchors, &baseline, CaretMotion::NextLine);
    assert!(nothing(&checked), "{checked:?}");
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextLine,
    ));
    assert_eq!(read.caret.line.text, "two");
    assert_eq!(read.unit.unit, TextUnit::Line, "a line is the caret's line");
}

#[test]
fn a_changed_character_beside_the_caret_is_evidence() {
    // The position moved with the edit: only the characters beside it tell.
    let mut source = Fake::new("abcx", 4);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = "abcy".encode_utf16().collect();
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    ));
    assert_eq!(read.caret.line.text, "abcy");
}

#[test]
fn a_line_that_changed_away_from_the_caret_is_no_evidence() {
    let mut source = Fake::new("one two", 4);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = "one two three".encode_utf16().collect();
    let checked = check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    );
    assert!(nothing(&checked), "{checked:?}");
}

#[test]
fn a_delete_that_changes_the_text_at_the_caret_is_evidence() {
    let mut source = Fake::new("abc", 1);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = "ac".encode_utf16().collect();
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::Delete,
    ));
    assert_eq!(read.unit.text, "c");
}

#[test]
fn a_deleting_key_in_a_run_of_one_character_is_evidence_by_its_line() {
    // A password field's mask: deleting one star leaves the caret, the
    // characters beside it, and the character at it as they were, and only
    // the line, one star shorter, shows the key did something.
    let mut source = Fake::new("******", 2);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = "*****".encode_utf16().collect();
    let checked = check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    );
    assert!(
        nothing(&checked),
        "no evidence but a deleting key's: {checked:?}"
    );
    // The check replaced the last read; the key's read compares with the
    // field before it, as it would have with a key of its own.
    source.text = "******".encode_utf16().collect();
    report(&mut source, &mut anchors);
    source.text = "*****".encode_utf16().collect();
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::Delete,
    ));
    assert_eq!(
        (read.caret.line.text.as_str(), read.caret.line.offset),
        ("*****", 2)
    );
}

/// A caret event's report compared with the last after the text changed:
/// the change in general terms, with no text, and the note cleared, so
/// the next report compares as always.
#[test]
fn a_comparison_after_the_text_changed_is_in_general_terms_once() {
    let mut source = Fake::new("hello world", 0);
    source.selection = Some((0, 5));
    let mut anchors = store();
    report(&mut source, &mut anchors);
    anchors.note_content_changed(1);
    source.selection = None;
    source.caret = 1;
    let compare = |source: &mut Fake, anchors: &mut Anchors<usize>| {
        let read = compared(source, anchors);
        (
            read.selection_changes,
            read.content_changed,
            read.new_selection,
        )
    };
    assert_eq!(
        compare(&mut source, &mut anchors),
        (
            vec![SelectionChange {
                selected: false,
                text: String::new(),
                characters: 0,
            }],
            true,
            None
        )
    );
    source.selection = Some((1, 3));
    assert_eq!(
        compare(&mut source, &mut anchors),
        (
            vec![SelectionChange {
                selected: true,
                text: "el".to_owned(),
                characters: 2,
            }],
            false,
            None
        )
    );
}

/// A selecting key's answer after the text changed: the change in general
/// terms, with the whole new selection's text.
#[test]
fn a_selecting_keys_answer_after_the_text_changed_is_in_general_terms() {
    let mut source = Fake::new("hello world", 0);
    source.selection = Some((0, 2));
    let mut anchors = store();
    let before = report(&mut source, &mut anchors);
    assert!(before.selection.is_some());
    anchors.note_content_changed(1);
    source.selection = Some((0, 5));
    let baseline = pinned(&mut anchors);
    let reply = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    ));
    assert_eq!(
        (
            reply.selection_changes,
            reply.content_changed,
            reply.new_selection
        ),
        (
            vec![SelectionChange {
                selected: true,
                text: String::new(),
                characters: 0,
            }],
            true,
            Some(SelectionChange {
                selected: true,
                text: "hello".to_owned(),
                characters: 5,
            })
        )
    );
}

/// A selecting key's reply, from a selection of `old` (equal ends for none,
/// with the caret there), which the outpost reported and kept, to `new`.
fn selection_after(old: (usize, usize), new: Option<(usize, usize)>) -> Vec<(bool, String, u32)> {
    let mut source = Fake::new("hello world", old.0);
    source.selection = (old.0 != old.1).then_some(old);
    let mut anchors = store();
    let _ = report(&mut source, &mut anchors);
    let pinned_read = pinned(&mut anchors);
    source.selection = new;
    source.caret = new.map_or(old.0, |(start, _)| start);
    let baseline = pinned_read;
    let reply = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextCharacter,
    ));
    reply
        .selection_changes
        .into_iter()
        .map(|change| (change.selected, change.text, change.characters))
        .collect()
}

#[test]
fn selecting_and_unselecting_report_what_changed_on_each_side() {
    assert_eq!(
        selection_after((0, 0), Some((0, 1))),
        [(true, "h".to_owned(), 1)],
        "Shift+Right Arrow from no selection"
    );
    assert_eq!(
        selection_after((0, 2), Some((0, 5))),
        [(true, "llo".to_owned(), 3)],
        "the end moved forward"
    );
    assert_eq!(
        selection_after((0, 5), Some((0, 3))),
        [(false, "lo".to_owned(), 2)],
        "the end moved back"
    );
    assert_eq!(
        selection_after((3, 5), Some((1, 4))),
        [(true, "el".to_owned(), 2), (false, "o".to_owned(), 1)],
        "the start side first, then the end side"
    );
    assert_eq!(
        selection_after((0, 2), Some((6, 11))),
        [(false, "he".to_owned(), 2), (true, "world".to_owned(), 5)],
        "apart: the old unselected, then the new selected"
    );
    assert_eq!(
        selection_after((0, 5), None),
        [(false, "hello".to_owned(), 5)],
        "collapsed at the old start"
    );
}

/// A selection made around a collapsed caret, as a double-click inside a
/// word makes it, is one change, the whole selection selected, and the
/// caret left inside it again is one change, the whole selection
/// unselected, on the generic path classic edit controls take.
#[test]
fn a_selection_around_the_caret_is_one_change_each_way() {
    let mut source = Fake::new("hello world", 5);
    let mut anchors = store();
    let _ = report(&mut source, &mut anchors);
    let answer = |source: &mut Fake, anchors: &mut Anchors<usize>, baseline: &LastRead<usize>| {
        answered(check(source, anchors, baseline, CaretMotion::NextCharacter))
            .selection_changes
            .into_iter()
            .map(|change| (change.selected, change.text, change.characters))
            .collect::<Vec<_>>()
    };
    let baseline = pinned(&mut anchors);
    source.selection = Some((3, 8));
    source.caret = 8;
    assert_eq!(
        answer(&mut source, &mut anchors, &baseline),
        [(true, "lo wo".to_owned(), 5)]
    );
    let baseline = pinned(&mut anchors);
    source.selection = None;
    source.caret = 5;
    assert_eq!(
        answer(&mut source, &mut anchors, &baseline),
        [(false, "lo wo".to_owned(), 5)]
    );
}

fn read(source: &mut Fake, anchors: &mut Anchors<usize>, read: TextRead) -> TextReply {
    perform(source, &mut anchors.node(1), &TextOp::Read(read))
}

#[test]
fn movement_stops_at_the_ends_and_reports_how_far_it_went() {
    let mut source = Fake::new("one\ntwo\nthree", 5);
    let mut anchors = store();
    let line = |count| TextRead {
        at: TextPoint::Caret,
        movement: Some(TextMovement {
            unit: TextUnit::Line,
            count,
        }),
        unit: TextUnit::Line,
    };
    let TextReply::Read { moved, chunk } = read(&mut source, &mut anchors, line(1)) else {
        panic!("a read");
    };
    assert_eq!((moved, chunk.text.as_str(), chunk.last), (1, "three", true));
    let TextReply::Read { moved, chunk } = read(&mut source, &mut anchors, line(5)) else {
        panic!("a read");
    };
    assert_eq!(
        (moved, chunk.text.as_str()),
        (1, "three"),
        "never past the end"
    );
    let TextReply::Read { moved, chunk } = read(&mut source, &mut anchors, line(-3)) else {
        panic!("a read");
    };
    assert_eq!(
        (moved, chunk.text.as_str(), chunk.first),
        (-1, "one\n", true)
    );
}

#[test]
fn the_ends_of_the_document_are_reached_by_document_movement() {
    let mut source = Fake::new("one\ntwo", 1);
    let mut anchors = store();
    let to_end = TextRead {
        at: TextPoint::Caret,
        movement: Some(TextMovement {
            unit: TextUnit::Document,
            count: 1,
        }),
        unit: TextUnit::Line,
    };
    let TextReply::Read { moved, chunk } = read(&mut source, &mut anchors, to_end) else {
        panic!("a read");
    };
    assert_eq!((moved, chunk.text.as_str()), (1, "two"));
    assert_eq!(chunk.offset, 3, "the point reached is the end");
    let whole = TextRead {
        at: TextPoint::Start,
        movement: None,
        unit: TextUnit::Document,
    };
    assert_eq!(
        read(&mut source, &mut anchors, whole),
        TextReply::UnsupportedUnit(TextUnit::Document)
    );
}

#[test]
fn sentences_are_unsupported_or_read_as_the_paragraph() {
    let sentence = TextRead {
        at: TextPoint::Caret,
        movement: None,
        unit: TextUnit::Sentence,
    };
    let mut source = Fake::new("One. Two.\nNext", 0);
    let mut anchors = store();
    let TextReply::Read { chunk, .. } = read(&mut source, &mut anchors, sentence) else {
        panic!("a read");
    };
    assert_eq!(chunk.unit, TextUnit::Paragraph);
    assert_eq!(chunk.text, "One. Two.\n");
    source.sentences = Sentences::Unsupported;
    assert_eq!(
        read(&mut source, &mut anchors, sentence),
        TextReply::UnsupportedUnit(TextUnit::Sentence)
    );
    let page = TextRead {
        unit: TextUnit::Page,
        ..sentence
    };
    assert_eq!(
        read(&mut source, &mut anchors, page),
        TextReply::UnsupportedUnit(TextUnit::Page)
    );
}

#[test]
fn a_long_line_is_cut_at_a_character_boundary() {
    let long = "é".repeat(MAX_CHUNK_BYTES);
    let mut source = Fake::new(&long, 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    assert!(line.truncated);
    assert_eq!(line.text.len(), MAX_CHUNK_BYTES);
    assert!(line.text.chars().all(|character| character == 'é'));
}

#[test]
fn a_range_reads_in_document_order_and_selecting_moves_the_caret() {
    let mut source = Fake::new("hello world", 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let at = |offset| {
        TextPoint::At(TextPosition {
            anchor: line.start,
            offset,
        })
    };
    let range = perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::ReadRange {
            start: at(11),
            end: at(6),
            through: None,
        },
    );
    assert_eq!(
        range,
        TextReply::Range {
            text: "world".to_owned(),
            truncated: false
        }
    );
    let selected = perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::Select {
            start: at(5),
            end: at(0),
            through: None,
        },
    );
    assert_eq!(
        selected,
        TextReply::Selected(vec![SelectionChange {
            selected: true,
            text: "hello".to_owned(),
            characters: 5,
        }])
    );
    assert_eq!(source.selection, Some((0, 5)));
}

#[test]
fn a_range_through_its_ends_holds_both_ends_characters_whichever_comes_first() {
    // The review copy's range: from the earlier end through the character
    // at the later, the marker's ("w", at 6) or the review cursor's ("h",
    // at 0), whichever way round they are.
    let mut source = Fake::new("hello world", 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let at = |offset| {
        TextPoint::At(TextPosition {
            anchor: line.start,
            offset,
        })
    };
    let through = |start, end| {
        Some(TextThrough {
            start: at(start),
            end: at(end),
        })
    };
    for (start, end, past) in [(6, 0, through(7, 1)), (0, 6, through(1, 7))] {
        let range = perform(
            &mut source,
            &mut anchors.node(1),
            &TextOp::ReadRange {
                start: at(start),
                end: at(end),
                through: past,
            },
        );
        assert_eq!(
            range,
            TextReply::Range {
                text: "hello w".to_owned(),
                truncated: false
            },
            "from {start} to {end}"
        );
    }
    let selected = perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::Select {
            start: at(6),
            end: at(0),
            through: through(7, 1),
        },
    );
    assert_eq!(
        selected,
        TextReply::Selected(vec![SelectionChange {
            selected: true,
            text: "hello w".to_owned(),
            characters: 7,
        }])
    );
    assert_eq!(source.selection, Some((0, 7)));
}

#[test]
fn utf16_converts_to_utf8_with_offsets_at_character_boundaries() {
    let units: Vec<u16> = "a😀b".encode_utf16().collect();
    let (text, offsets, cut) = to_utf8(&units, 100, &[0, 1, 2, 3, 4]);
    assert_eq!(text, "a😀b");
    assert!(!cut);
    // Inside the surrogate pair maps past the emoji, a boundary.
    assert_eq!(offsets, [0, 1, 5, 5, 6]);
    let (text, _, cut) = to_utf8(&units, 3, &[]);
    assert_eq!((text.as_str(), cut), ("a", true));
}

#[test]
fn text_cut_short_ends_at_a_whole_character() {
    // Read up to the first half of an emoji's surrogate pair.
    let units: Vec<u16> = "ab😀".encode_utf16().take(3).collect();
    let (mut text, _, _) = to_utf8(&units, 100, &[]);
    keep_whole_characters(&mut text);
    assert_eq!(text, "ab");
    // Read up to a letter whose combining accent the limit left out.
    let mut text = "ae".to_owned();
    keep_whole_characters(&mut text);
    assert_eq!(text, "a");
    // A limit inside a letter and its accent cuts before the letter.
    assert_eq!(grapheme_floor("ae\u{301}b", 3), 1);
    assert_eq!(grapheme_floor("ae\u{301}b", 4), 4);
    // So does a byte limit met between them, and one met before a new
    // character keeps everything before it.
    let units: Vec<u16> = "ae\u{301}b".encode_utf16().collect();
    let (text, offsets, cut) = to_utf8(&units, 2, &[3]);
    assert_eq!(
        (text.as_str(), offsets.as_slice(), cut),
        ("a", &[1][..], true)
    );
    let (text, _, cut) = to_utf8(&units, 4, &[]);
    assert_eq!((text.as_str(), cut), ("ae\u{301}", true));
}

#[test]
fn a_keys_read_says_whether_the_caret_stayed_on_its_line() {
    for (moved_to, same_line) in [(2, true), (3, true), (6, false)] {
        // The caret at "one"'s start; a key the caret table does not know
        // moves it within the line, to its end, or onto the next.
        let mut source = Fake::new("one\ntwo", 0);
        let mut anchors = store();
        report(&mut source, &mut anchors);
        let baseline = pinned(&mut anchors);
        source.caret = moved_to;
        let read = answered(check(
            &mut source,
            &mut anchors,
            &baseline,
            CaretMotion::Other,
        ));
        assert_eq!(read.on_baseline_line, same_line, "moved to {moved_to}");
    }
}

#[test]
fn a_caret_read_before_the_key_with_its_line_after_is_no_answer() {
    // A Backspace handled between the caret's read and its line's: the line
    // has lost "y", the caret read first has not moved yet, and moves by
    // the time it is read again. Nothing is answered until the next check.
    let mut source = Fake::new("abcxy", 5);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = "abcx".encode_utf16().collect();
    source.moves_on_read = Some((source.caret_reads + 2, 4));
    let first = check(&mut source, &mut anchors, &baseline, CaretMotion::Backspace);
    assert!(matches!(first, KeyChecked::Moving), "{first:?}");
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::Backspace,
    ));
    assert_eq!(read.caret.line.text, "abcx");
    assert_eq!(read.caret.line.offset, 4);
}

#[test]
fn a_selection_read_before_a_cut_with_its_line_after_is_no_answer() {
    // Control+X handled between the selection's read and its line's, as
    // the Windows Forms text box can be: the line has lost the selected
    // "abc", the selection read first is still there, and is gone by the
    // time it is read again, its start, the caret, unmoved. Nothing is
    // answered until the next check.
    let mut source = Fake::new("abc def", 0);
    source.selection = Some((0, 3));
    let mut anchors = store();
    report(&mut source, &mut anchors);
    let baseline = pinned(&mut anchors);
    source.text = " def".encode_utf16().collect();
    source.moves_on_read = Some((source.caret_reads + 2, 0));
    let first = check(&mut source, &mut anchors, &baseline, CaretMotion::NextWord);
    assert!(matches!(first, KeyChecked::Moving), "{first:?}");
    let read = answered(check(
        &mut source,
        &mut anchors,
        &baseline,
        CaretMotion::NextWord,
    ));
    assert_eq!(read.caret.line.text, " def");
    assert_eq!(read.caret.selection, None);
}

/// A caret event's report with nothing selected before or after has no
/// change to tell and nothing to ask about the text: it is not cut, so the
/// caret move stays one program.
#[test]
fn a_caret_move_with_no_selection_is_not_cut() {
    let mut source = Fake::new("hello world", 0);
    let mut anchors = store();
    report(&mut source, &mut anchors);
    source.caret = 3;
    let checked = caret_report_checking(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT,
        &comparing(),
        true,
    )
    .expect("the fake answers");
    let Checked::Done(read) = checked else {
        panic!("the report is made whole");
    };
    assert_eq!(
        (
            read.selection_changes,
            read.content_changed,
            read.state.caret
        ),
        (Vec::new(), false, 3)
    );
}

/// A caret event's report over a selection typed over is cut where it
/// would ask whether the text's change waits, with the caret it read and
/// the selection it compares with; finished with the change waiting, it
/// says the change in general terms, as after a noted change.
#[test]
fn a_selection_typed_over_is_cut_then_finished_in_general_terms() {
    let mut source = Fake::new("hello world", 0);
    source.selection = Some((0, 5));
    let mut anchors = store();
    report(&mut source, &mut anchors);
    source.selection = None;
    source.caret = 1;
    let checked = caret_report_checking(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT,
        &comparing(),
        true,
    )
    .expect("the fake answers");
    let Checked::Cut(cut) = checked else {
        panic!("the report is cut");
    };
    assert_eq!(
        (cut.caret, cut.selection, cut.last.ends, cut.last.collapsed),
        (1, None, (0, 5), false)
    );
    let read = finish_report(&mut source, &mut anchors.node(1), cut, true, || 0)
        .expect("the fake answers");
    assert_eq!(
        (
            read.selection_changes,
            read.content_changed,
            read.new_selection
        ),
        (
            vec![SelectionChange {
                selected: false,
                text: String::new(),
                characters: 0,
            }],
            true,
            None
        )
    );
}
