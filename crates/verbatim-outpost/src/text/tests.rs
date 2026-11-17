//! The text protocol over an in-memory source: chunks, positions and
//! anchors, selection changes, reads, and a caret key's watch.

use std::sync::atomic::AtomicU64;

use verbatim_model::{
    CaretMotion, SelectionDifference, SentenceMoved, TextDetails, WatchFinalRead, Watched,
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
        // Offsets past the text read up to its end, as an edit control's
        // text range does once its text was cut.
        let cut = (*end).min(start + max_units);
        let to = cut.min(self.text.len());
        Ok((self.text[(*start).min(to)..to].to_vec(), cut < *end))
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

/// The caret's report, read as a caret tell's read reads it, holding a
/// baseline.
fn report(source: &mut Fake, anchors: &mut Anchors<usize>) -> CaretReport {
    held::read_selection(source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers")
        .report
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

/// A selection read's changes from the baseline held with `baseline`,
/// what each became selected or unselected, its text and characters.
fn changes_from(
    source: &mut Fake,
    anchors: &mut Anchors<usize>,
    baseline: TextAnchor,
) -> (Vec<(bool, String, u32)>, TextAnchor) {
    let found = held::read_selection(source, &mut anchors.node(1), REPORTED_AT, Some(baseline))
        .expect("the fake answers");
    let changes = found
        .difference
        .expect("the baseline is held")
        .changes
        .into_iter()
        .map(|change| (change.selected, change.text, change.characters))
        .collect();
    (changes, found.baseline)
}

/// A selection read's changes, from a selection of `old` (equal ends for
/// none, with the caret there), which a read held as its baseline, to
/// `new`.
fn selection_after(old: (usize, usize), new: Option<(usize, usize)>) -> Vec<(bool, String, u32)> {
    let mut source = Fake::new("hello world", old.0);
    source.selection = (old.0 != old.1).then_some(old);
    let mut anchors = store();
    let baseline = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers")
        .baseline;
    source.selection = new;
    source.caret = new.map_or(old.0, |(start, _)| start);
    changes_from(&mut source, &mut anchors, baseline).0
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
    let baseline = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers")
        .baseline;
    source.selection = Some((3, 8));
    source.caret = 8;
    let (changes, baseline) = changes_from(&mut source, &mut anchors, baseline);
    assert_eq!(changes, [(true, "lo wo".to_owned(), 5)]);
    source.selection = None;
    source.caret = 5;
    assert_eq!(
        changes_from(&mut source, &mut anchors, baseline).0,
        [(false, "lo wo".to_owned(), 5)]
    );
}

/// A selection whose text the application deleted, as Backspace over
/// a selection to the text's end deletes it, is still a change: the old
/// selection unselected with no text, which Core says as "selection
/// removed" once the text changed, as NVDA lists the old selection
/// whatever it now holds (`speech/speech.py` 1364 to 1365).
#[test]
fn a_selection_whose_text_was_deleted_is_unselected_with_no_text() {
    let mut source = Fake::new("hello world", 6);
    source.selection = Some((6, 11));
    source.caret = 11;
    let mut anchors = store();
    let baseline = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers")
        .baseline;
    source.text = "hello ".encode_utf16().collect();
    source.selection = None;
    source.caret = 6;
    assert_eq!(
        changes_from(&mut source, &mut anchors, baseline).0,
        [(false, String::new(), 0)]
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

/// A caret tell's read with no baseline named holds the next and compares
/// nothing (Table 17, column E).
#[test]
fn a_selection_read_naming_no_baseline_holds_one_and_compares_nothing() {
    let mut source = Fake::new("line one\n", 2);
    let mut anchors = store();
    let found = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers");
    assert_eq!(found.difference, None);
    assert_eq!(found.selected, None);
    assert_eq!(found.report.line.text, "line one\n");
    assert_eq!(found.report.line.offset, 2);
    assert!(
        anchors.node(1).anchors.held.contains_key(&found.baseline.0),
        "the baseline is held"
    );
}

/// The selection is compared with the named baseline by the contract's
/// rule, in specific terms with each part's text, and the whole new
/// selection given beside it (Table 17, column D).
#[test]
fn a_selection_read_says_what_changed_from_the_named_baseline() {
    let mut source = Fake::new("line one\n", 0);
    let mut anchors = store();
    let first = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers");
    source.selection = Some((0, 4));
    let second = held::read_selection(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT + 1,
        Some(first.baseline),
    )
    .expect("the fake answers");
    let selected = SelectionChange {
        selected: true,
        text: "line".to_owned(),
        characters: 4,
    };
    assert_eq!(
        second.difference,
        Some(SelectionDifference {
            changes: vec![selected.clone()],
            new_selection: Some(selected),
        })
    );
    assert_ne!(second.baseline, first.baseline);
    source.selection = Some((0, 2));
    let third = held::read_selection(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT + 2,
        Some(second.baseline),
    )
    .expect("the fake answers");
    assert_eq!(
        third.difference.map(|difference| difference.changes),
        Some(vec![SelectionChange {
            selected: false,
            text: "ne".to_owned(),
            characters: 2,
        }])
    );
}

/// When the baseline and the new selection are both empty, the caret
/// having moved, nothing changed; a baseline the outpost no longer holds
/// is no baseline.
#[test]
fn an_empty_baseline_and_selection_differ_by_nothing_and_a_lost_one_by_none() {
    let mut source = Fake::new("line one\n", 0);
    let mut anchors = store();
    let first = held::read_selection(&mut source, &mut anchors.node(1), REPORTED_AT, None)
        .expect("the fake answers");
    source.caret = 3;
    let moved = held::read_selection(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT + 1,
        Some(first.baseline),
    )
    .expect("the fake answers");
    assert_eq!(
        moved.difference,
        Some(SelectionDifference {
            changes: Vec::new(),
            new_selection: None,
        })
    );
    let lost = held::read_selection(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT + 2,
        Some(TextAnchor(9_999)),
    )
    .expect("the fake answers");
    assert_eq!(lost.difference, None);
}

/// A caret key's starting read holds its starting point with, for
/// Delete, the word at the caret, which its check compares.
#[test]
fn a_starting_read_holds_the_point_with_the_keys_unit_and_word() {
    let mut source = Fake::new("alpha beta\n", 6);
    let mut anchors = store();
    let started = held::starting_read(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT,
        (TextUnit::Character, true),
    )
    .expect("the fake answers");
    assert_eq!(started.unit.text, "b");
    assert_eq!(started.word.map(|word| word.text), Some("beta".to_owned()));
    assert_eq!(started.caret.line.offset, 6);
    let held = anchors
        .node(1)
        .anchors
        .held
        .get(&started.point.0)
        .map(|read| {
            (
                read.caret.at,
                read.unit.as_ref().map(|unit| unit.text.clone()),
            )
        });
    assert_eq!(held, Some((6, Some("beta".to_owned()))));
}

/// Verbatim's own caret move and selection answer with the next baseline,
/// held where the caret or the selection is after them.
#[test]
fn a_move_and_a_selection_for_an_entry_hold_the_next_baseline() {
    let mut source = Fake::new("line one\n", 0);
    let mut anchors = store();
    let at = report(&mut source, &mut anchors);
    let moved = held::perform_placed(
        &mut source,
        &mut anchors.node(1),
        &TextOp::MoveCaret(TextPoint::At(TextPosition {
            anchor: at.line.start,
            offset: 5,
        })),
    );
    let TextReply::CaretPlaced { baseline, .. } = moved else {
        panic!("a caret placed, not {moved:?}");
    };
    assert_eq!(
        anchors
            .node(1)
            .anchors
            .held
            .get(&baseline.0)
            .map(|read| read.caret.at),
        Some(5)
    );
    let selected = held::perform_placed(
        &mut source,
        &mut anchors.node(1),
        &TextOp::Select {
            start: TextPoint::Start,
            end: TextPoint::At(TextPosition {
                anchor: at.line.start,
                offset: 4,
            }),
            through: None,
        },
    );
    let TextReply::SelectionPlaced { changes, baseline } = selected else {
        panic!("a selection placed, not {selected:?}");
    };
    assert_eq!(changes.len(), 1);
    assert_eq!(
        anchors
            .node(1)
            .anchors
            .held
            .get(&baseline.0)
            .and_then(|read| read
                .selection
                .as_ref()
                .map(|selection| (selection.start.at, selection.end.at))),
        Some((0, 4))
    );
}

/// Moves by sentence from a starting point held at `caret` in
/// "One. Two.\nThree.", `forward` or back: what it answered and where the
/// caret is after.
fn sentence_from(caret: usize, forward: bool) -> (TextReply, usize) {
    let mut source = Fake::new("One. Two.\nThree.", caret);
    let mut anchors = store();
    let started = held::starting_read(
        &mut source,
        &mut anchors.node(1),
        REPORTED_AT,
        (TextUnit::Sentence, false),
    )
    .expect("the fake answers");
    let reply =
        sentence::move_by_sentence(&mut source, &mut anchors.node(1), (started.point, forward))
            .expect("the fake answers");
    (reply, source.caret)
}

/// The sentence and line a move answered, `None` for another answer.
fn moved_to(reply: &TextReply) -> Option<(String, String, u32)> {
    match reply {
        TextReply::Sentence(moved) => match &**moved {
            SentenceMoved::Moved { sentence, line, .. } => {
                Some((sentence.clone(), line.text.clone(), line.offset))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The next sentence in the caret's paragraph, then the next paragraph's
/// first, then the text's end past the last.
#[test]
fn a_sentence_move_forward_finds_the_next_sentence_then_paragraph_then_the_end() {
    let (reply, caret) = sentence_from(0, true);
    assert_eq!(
        moved_to(&reply),
        Some(("Two.".to_owned(), "One. Two.\n".to_owned(), 5))
    );
    assert_eq!(caret, 5);
    let (reply, caret) = sentence_from(5, true);
    assert_eq!(
        moved_to(&reply),
        Some(("Three.".to_owned(), "Three.".to_owned(), 0))
    );
    assert_eq!(caret, 10);
    let (reply, caret) = sentence_from(12, true);
    let TextReply::Sentence(moved) = reply else {
        panic!("a sentence move, not {reply:?}");
    };
    let SentenceMoved::End { line, .. } = *moved else {
        panic!("the end, not {moved:?}");
    };
    assert_eq!((line.text.as_str(), line.offset), ("Three.", 6));
    assert_eq!(caret, 16);
}

/// The sentence before the caret, then the previous paragraph's last, and
/// at the text's start the caret stays with the first sentence.
#[test]
fn a_sentence_move_back_finds_the_one_before_then_stays_at_the_start() {
    let (reply, caret) = sentence_from(7, false);
    assert_eq!(
        moved_to(&reply),
        Some(("Two.".to_owned(), "One. Two.\n".to_owned(), 5))
    );
    assert_eq!(caret, 5);
    let (reply, caret) = sentence_from(10, false);
    assert_eq!(
        moved_to(&reply),
        Some(("Two.".to_owned(), "One. Two.\n".to_owned(), 5))
    );
    assert_eq!(caret, 5);
    let (reply, caret) = sentence_from(0, false);
    let TextReply::Sentence(moved) = reply else {
        panic!("a sentence move, not {reply:?}");
    };
    let SentenceMoved::StayedAtStart { sentence, .. } = *moved else {
        panic!("stayed at the start, not {moved:?}");
    };
    assert_eq!(sentence, "One. ");
    assert_eq!(caret, 0);
}

/// Text with no sentence unit is not moved in, and a starting point no
/// longer held is lost.
#[test]
fn a_sentence_move_without_sentences_or_its_point_moves_nothing() {
    let mut source = Fake::new("One. Two.", 0);
    let mut anchors = store();
    assert_eq!(
        sentence::move_by_sentence(&mut source, &mut anchors.node(1), (TextAnchor(77), true)),
        Ok(TextReply::AnchorLost)
    );
    source.sentences = Sentences::Unsupported;
    assert_eq!(
        sentence::move_by_sentence(&mut source, &mut anchors.node(1), (TextAnchor(77), true)),
        Ok(TextReply::UnsupportedUnit(TextUnit::Sentence))
    );
    assert_eq!(source.caret, 0);
}

/// A watch's check of the key in `source` from its starting read made on
/// `before`, at the bound when `last`.
fn watched(
    before: &mut Fake,
    after: impl FnOnce(&mut Fake),
    (motion, last, final_read): (CaretMotion, bool, WatchFinalRead),
) -> TextResult<Option<Watched>> {
    let mut anchors = store();
    let started = held::starting_read(
        before,
        &mut anchors.node(1),
        REPORTED_AT,
        (motion.unit(), motion.compares_unit()),
    )
    .expect("the fake answers");
    after(before);
    check::check(
        before,
        &mut anchors.node(1),
        check::CheckAsk {
            point: started.point,
            motion,
            word: motion.compares_unit(),
            last,
            final_read,
        },
        REPORTED_AT + 1,
    )
}

const CHARACTER: WatchFinalRead = WatchFinalRead::Unit {
    unit: TextUnit::Character,
    details: TextDetails::ALL,
};

/// The caret and the unit a watch answered.
fn said(watched: Option<Watched>) -> Option<(&'static str, u32, String)> {
    match watched? {
        Watched::Effect(read) => Some(("effect", read.caret.line.offset, read.unit.text)),
        Watched::Bound(read) => Some(("bound", read.caret.line.offset, read.unit.text)),
        Watched::BoundUnread => Some(("bound unread", 0, String::new())),
        Watched::EffectUnread => Some(("effect unread", 0, String::new())),
    }
}

/// A check that finds the caret moved from the starting point answers the
/// effect with the final read made in the same program.
#[test]
fn a_check_that_finds_the_caret_moved_answers_the_effect_with_its_final_read() {
    let found = watched(
        &mut Fake::new("alpha beta\n", 2),
        |fake| fake.caret = 3,
        (CaretMotion::NextCharacter, false, CHARACTER),
    );
    assert_eq!(
        said(found.expect("read")),
        Some(("effect", 3, "h".to_owned()))
    );
}

/// A check that finds nothing before the bound answers nothing; at the
/// bound it answers with its final read, or nothing read for a final
/// read only at the effect.
#[test]
fn a_check_with_no_effect_waits_until_the_bound() {
    let early = watched(
        &mut Fake::new("alpha\n", 5),
        |_| {},
        (CaretMotion::NextCharacter, false, CHARACTER),
    );
    assert_eq!(said(early.expect("read")), None);
    let last = watched(
        &mut Fake::new("alpha\n", 0),
        |_| {},
        (CaretMotion::PreviousCharacter, true, CHARACTER),
    );
    assert_eq!(
        said(last.expect("read")),
        Some(("bound", 0, "a".to_owned()))
    );
    let unread = watched(
        &mut Fake::new("alpha\n", 0),
        |_| {},
        (
            CaretMotion::NextSentence,
            true,
            WatchFinalRead::UnitAtEffect {
                unit: TextUnit::Line,
                details: TextDetails::ALL,
            },
        ),
    );
    assert_eq!(unread.expect("read"), Some(Watched::BoundUnread));
}

/// Delete's effect: the character at the caret changed, the caret not.
#[test]
fn deletes_effect_is_the_character_at_the_caret_changing() {
    let found = watched(
        &mut Fake::new("alpha\n", 1),
        |fake| {
            fake.text.remove(1);
        },
        (CaretMotion::Delete, false, CHARACTER),
    );
    assert_eq!(
        said(found.expect("read")),
        Some(("effect", 1, "p".to_owned()))
    );
}

/// A caret that moved while the check read it is read again rather than
/// answered torn.
#[test]
fn a_check_whose_caret_moved_while_read_reads_again() {
    let mut source = Fake::new("alpha\n", 0);
    let found = watched(
        &mut source,
        |fake| {
            fake.caret = 1;
            fake.moves_on_read = Some((fake.caret_reads + 2, 2));
        },
        (CaretMotion::NextCharacter, false, CHARACTER),
    );
    assert_eq!(
        said(found.expect("read")),
        Some(("effect", 2, "p".to_owned()))
    );
}

/// A starting point the outpost no longer holds answers the node gone.
#[test]
fn a_check_of_a_point_no_longer_held_is_gone() {
    let mut source = Fake::new("alpha\n", 0);
    let mut anchors = store();
    let checked = check::check(
        &mut source,
        &mut anchors.node(1),
        check::CheckAsk {
            point: TextAnchor(9_999),
            motion: CaretMotion::NextCharacter,
            word: false,
            last: false,
            final_read: CHARACTER,
        },
        REPORTED_AT,
    );
    assert!(matches!(checked, Err(TextError::Gone)), "{checked:?}");
}

/// The final read after an event reads the caret and the key's unit.
#[test]
fn a_final_read_reads_the_caret_and_the_keys_unit() {
    let mut source = Fake::new("alpha beta\n", 6);
    let mut anchors = store();
    let read = check::final_read(
        &mut source,
        &mut anchors.node(1),
        TextUnit::Word,
        REPORTED_AT,
    )
    .expect("the fake answers");
    assert_eq!(
        (read.caret.line.offset, read.unit.text),
        (6, "beta".to_owned())
    );
}
