//! The text protocol over an in-memory source: chunks, positions and
//! anchors, selection changes, reads, and a caret key's watch.

use std::sync::atomic::AtomicU64;

use verbatim_model::batch::{Chunks, Done, Missing, RangeText, Selected, StepOutcome, StepRefusal};
use verbatim_model::{CaretMotion, SelectionDifference, SentenceMoved};

use super::*;

/// One unit read call by call at `at`, after `movement`: how far it moved
/// and its chunk, or the outcome that refuses it.
fn read_one(
    source: &mut Fake,
    anchors: &mut NodeText<'_, usize>,
    (at, movement, unit): (TextPoint, Option<TextMovement>, TextUnit),
) -> Result<(i32, TextChunk), Box<StepOutcome>> {
    let step = CallStep::Read {
        at,
        movement,
        unit,
        count: 1,
    };
    match by_calls(source, anchors, step).expect("the fake answers") {
        Ok(Done::ReadText(Chunks { moved, mut chunks })) => Ok((moved, chunks.remove(0))),
        Ok(other) => panic!("a read, not {other:?}"),
        Err(refused) => Err(Box::new(refused)),
    }
}

/// `step` run call by call: its result, or the outcome that refuses it.
fn call(
    source: &mut Fake,
    anchors: &mut NodeText<'_, usize>,
    step: CallStep,
) -> Result<Done, Box<StepOutcome>> {
    by_calls(source, anchors, step)
        .expect("the fake answers")
        .map_err(Box::new)
}

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

/// The caret's report, read as a focus's caret read reads it, holding a
/// baseline.
fn report(source: &mut Fake, anchors: &mut Anchors<usize>) -> CaretReport {
    held::read_focus(source, &mut anchors.node(1), (None, REPORTED_AT))
        .expect("the fake answers")
        .0
}

/// A batch's `Caret` of the fake's caret and selection: the read held as
/// the next baseline.
fn caret_held(source: &mut Fake, anchors: &mut Anchors<usize>) -> TextAnchor {
    ops::caret(source, &mut anchors.node(1), REPORTED_AT)
        .expect("the fake answers")
        .held
}

/// How the selection a `Caret` reads now differs from the one held under
/// `baseline`, as a batch's `CompareSelection` compares them, with the
/// `Caret`'s read held; `Err` naming a read no longer held.
fn compared(
    source: &mut Fake,
    anchors: &mut Anchors<usize>,
    baseline: TextAnchor,
) -> (Result<SelectionDifference, TextAnchor>, TextAnchor) {
    let after = caret_held(source, anchors);
    let compared = ops::compare_selection(source, &anchors.node(1), (baseline, after))
        .expect("the fake answers")
        .map(|compared| compared.difference);
    (compared, after)
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
    let at = TextPoint::At(TextPosition {
        anchor: line.start,
        offset: x,
    });
    let reply = read_one(
        &mut source,
        &mut anchors.node(1),
        (at, None, TextUnit::Word),
    );
    let Ok((0, chunk)) = reply else {
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
    drop(anchors.forget_anchors(&std::collections::BTreeSet::from([first.0])));
    let read = |anchors: &mut Anchors<usize>, source: &mut Fake, anchor| {
        let at = TextPoint::At(TextPosition { anchor, offset: 1 });
        read_one(source, &mut anchors.node(1), (at, None, TextUnit::Line))
    };
    assert_eq!(
        read(&mut anchors, &mut source, first),
        Err(Box::new(StepOutcome::NotThere(Missing::Anchor(first))))
    );
    assert!(read(&mut anchors, &mut source, kept).is_ok());
}

/// A selection's changes from the baseline held with `baseline`, what
/// each became selected or unselected, its text and characters, and the
/// next baseline.
fn changes_from(
    source: &mut Fake,
    anchors: &mut Anchors<usize>,
    baseline: TextAnchor,
) -> (Vec<(bool, String, u32)>, TextAnchor) {
    let (difference, after) = compared(source, anchors, baseline);
    let changes = difference
        .expect("the baseline is held")
        .changes
        .into_iter()
        .map(|change| (change.selected, change.text, change.characters))
        .collect();
    (changes, after)
}

/// A selection read's changes, from a selection of `old` (equal ends for
/// none, with the caret there), which a read held as its baseline, to
/// `new`.
fn selection_after(old: (usize, usize), new: Option<(usize, usize)>) -> Vec<(bool, String, u32)> {
    let mut source = Fake::new("hello world", old.0);
    source.selection = (old.0 != old.1).then_some(old);
    let mut anchors = store();
    let baseline = caret_held(&mut source, &mut anchors);
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
    let baseline = caret_held(&mut source, &mut anchors);
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
    let baseline = caret_held(&mut source, &mut anchors);
    source.text = "hello ".encode_utf16().collect();
    source.selection = None;
    source.caret = 6;
    assert_eq!(
        changes_from(&mut source, &mut anchors, baseline).0,
        [(false, String::new(), 0)]
    );
}

/// A read of one unit, its point, movement and unit `read`.
fn read(
    source: &mut Fake,
    anchors: &mut Anchors<usize>,
    read: (TextPoint, Option<TextMovement>, TextUnit),
) -> Result<(i32, TextChunk), Box<StepOutcome>> {
    read_one(source, &mut anchors.node(1), read)
}

#[test]
fn movement_stops_at_the_ends_and_reports_how_far_it_went() {
    let mut source = Fake::new("one\ntwo\nthree", 5);
    let mut anchors = store();
    let line = |count| {
        let movement = TextMovement {
            unit: TextUnit::Line,
            count,
        };
        (TextPoint::Caret, Some(movement), TextUnit::Line)
    };
    let Ok((moved, chunk)) = read(&mut source, &mut anchors, line(1)) else {
        panic!("a read");
    };
    assert_eq!((moved, chunk.text.as_str(), chunk.last), (1, "three", true));
    let Ok((moved, chunk)) = read(&mut source, &mut anchors, line(5)) else {
        panic!("a read");
    };
    assert_eq!(
        (moved, chunk.text.as_str()),
        (1, "three"),
        "never past the end"
    );
    let Ok((moved, chunk)) = read(&mut source, &mut anchors, line(-3)) else {
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
    let movement = TextMovement {
        unit: TextUnit::Document,
        count: 1,
    };
    let to_end = (TextPoint::Caret, Some(movement), TextUnit::Line);
    let Ok((moved, chunk)) = read(&mut source, &mut anchors, to_end) else {
        panic!("a read");
    };
    assert_eq!((moved, chunk.text.as_str()), (1, "two"));
    assert_eq!(chunk.offset, 3, "the point reached is the end");
    let whole = (TextPoint::Start, None, TextUnit::Document);
    assert_eq!(
        read(&mut source, &mut anchors, whole),
        Err(Box::new(StepOutcome::Refused(StepRefusal::NoSuchUnit)))
    );
}

#[test]
fn sentences_are_unsupported_or_read_as_the_paragraph() {
    let sentence = (TextPoint::Caret, None, TextUnit::Sentence);
    let mut source = Fake::new("One. Two.\nNext", 0);
    let mut anchors = store();
    let Ok((_, chunk)) = read(&mut source, &mut anchors, sentence) else {
        panic!("a read");
    };
    assert_eq!(chunk.unit, TextUnit::Paragraph);
    assert_eq!(chunk.text, "One. Two.\n");
    source.sentences = Sentences::Unsupported;
    assert_eq!(
        read(&mut source, &mut anchors, sentence),
        Err(Box::new(StepOutcome::Refused(StepRefusal::NoSuchUnit)))
    );
    let page = (TextPoint::Caret, None, TextUnit::Page);
    assert_eq!(
        read(&mut source, &mut anchors, page),
        Err(Box::new(StepOutcome::Refused(StepRefusal::NoSuchUnit)))
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
    let range = CallStep::Range {
        ends: (at(11), at(6)),
        through: None,
    };
    assert_eq!(
        call(&mut source, &mut anchors.node(1), range),
        Ok(Done::ReadRange(RangeText {
            text: "world".to_owned(),
            truncated: false
        }))
    );
    let select = CallStep::Select {
        ends: (at(5), at(0)),
        through: None,
        began_us: 7,
    };
    assert_eq!(
        call(&mut source, &mut anchors.node(1), select),
        Ok(Done::Select(Selected { began_us: 7 }))
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
    let through = |start, end| Some((at(start), at(end)));
    for (start, end, past) in [(6, 0, through(7, 1)), (0, 6, through(1, 7))] {
        let range = CallStep::Range {
            ends: (at(start), at(end)),
            through: past,
        };
        assert_eq!(
            call(&mut source, &mut anchors.node(1), range),
            Ok(Done::ReadRange(RangeText {
                text: "hello w".to_owned(),
                truncated: false
            })),
            "from {start} to {end}"
        );
    }
    let select = CallStep::Select {
        ends: (at(6), at(0)),
        through: through(7, 1),
        began_us: 7,
    };
    assert!(call(&mut source, &mut anchors.node(1), select).is_ok());
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

/// A focus's caret read holds the next baseline and compares nothing
/// (Table 17, column E).
#[test]
fn a_focus_caret_read_holds_a_baseline_and_compares_nothing() {
    let mut source = Fake::new(
        "line one
",
        2,
    );
    let mut anchors = store();
    let (report, selected, baseline) =
        held::read_focus(&mut source, &mut anchors.node(1), (None, REPORTED_AT))
            .expect("the fake answers");
    assert_eq!(selected, None);
    assert_eq!(
        report.line.text,
        "line one
"
    );
    assert_eq!(report.line.offset, 2);
    assert!(
        anchors.node(1).anchors.held.contains_key(&baseline.0),
        "the baseline is held"
    );
}

/// The selection is compared with the named baseline by the contract's
/// rule, in specific terms with each part's text, and the whole new
/// selection given beside it (Table 17, column D).
#[test]
fn a_selection_says_what_changed_from_the_named_baseline() {
    let mut source = Fake::new(
        "line one
",
        0,
    );
    let mut anchors = store();
    let first = caret_held(&mut source, &mut anchors);
    source.selection = Some((0, 4));
    let (difference, second) = compared(&mut source, &mut anchors, first);
    let selected = SelectionChange {
        selected: true,
        text: "line".to_owned(),
        characters: 4,
    };
    assert_eq!(
        difference,
        Ok(SelectionDifference {
            changes: vec![selected.clone()],
            new_selection: Some(selected),
        })
    );
    assert_ne!(second, first);
    source.selection = Some((0, 2));
    assert_eq!(
        compared(&mut source, &mut anchors, second)
            .0
            .map(|difference| difference.changes),
        Ok(vec![SelectionChange {
            selected: false,
            text: "ne".to_owned(),
            characters: 2,
        }])
    );
}

/// When the baseline and the new selection are both empty, the caret
/// having moved, nothing changed; a baseline the outpost no longer holds
/// is named as lost.
#[test]
fn an_empty_baseline_and_selection_differ_by_nothing_and_a_lost_one_is_named() {
    let mut source = Fake::new(
        "line one
",
        0,
    );
    let mut anchors = store();
    let first = caret_held(&mut source, &mut anchors);
    source.caret = 3;
    assert_eq!(
        compared(&mut source, &mut anchors, first).0,
        Ok(SelectionDifference {
            changes: Vec::new(),
            new_selection: None,
        })
    );
    assert_eq!(
        compared(&mut source, &mut anchors, TextAnchor(9_999)).0,
        Err(TextAnchor(9_999))
    );
}

/// A batch's `Caret` holds the caret's read under its held anchor, the
/// starting point a caret key's wait and its move by sentence name: the
/// caret, and no line, which the batch reads by a step of its own.
#[test]
fn a_caret_holds_the_read_a_wait_starts_from() {
    let mut source = Fake::new(
        "alpha beta
",
        6,
    );
    let mut anchors = store();
    let at = ops::caret(&mut source, &mut anchors.node(1), REPORTED_AT).expect("the fake answers");
    assert!(at.selection.is_none());
    let held = anchors
        .node(1)
        .anchors
        .held
        .get(&at.held.0)
        .map(|read| (read.caret.at, read.line.is_some()));
    assert_eq!(held, Some((6, false)));
}

/// The line read at the caret `at` held, as a batch's `ReadText` of it
/// reads it.
fn line_at(source: &mut Fake, anchors: &mut NodeText<'_, usize>, at: TextAnchor) -> TextChunk {
    let at = TextPoint::At(TextPosition::at(at));
    match read_one(source, anchors, (at, None, TextUnit::Line)) {
        Ok((_, chunk)) => chunk,
        Err(other) => panic!("a line, not {other:?}"),
    }
}

/// Verbatim's own caret move and selection, a `Select` then a `Caret`:
/// the `Caret` holds the next baseline where the caret or the selection
/// is after them.
#[test]
fn a_move_and_a_selection_then_the_caret_hold_the_next_baseline() {
    let mut source = Fake::new("line one\n", 0);
    let mut anchors = store();
    let at = report(&mut source, &mut anchors);
    let to = TextPoint::At(TextPosition {
        anchor: at.line.start,
        offset: 5,
    });
    let moving = CallStep::Select {
        ends: (to, to),
        through: None,
        began_us: 1,
    };
    assert!(call(&mut source, &mut anchors.node(1), moving).is_ok());
    let moved =
        ops::caret(&mut source, &mut anchors.node(1), REPORTED_AT).expect("the fake answers");
    assert_eq!(
        anchors
            .node(1)
            .anchors
            .held
            .get(&moved.held.0)
            .map(|read| read.caret.at),
        Some(5)
    );
    let end = TextPoint::At(TextPosition {
        anchor: at.line.start,
        offset: 4,
    });
    let selecting = CallStep::Select {
        ends: (TextPoint::Start, end),
        through: None,
        began_us: 2,
    };
    assert!(call(&mut source, &mut anchors.node(1), selecting).is_ok());
    let selected =
        ops::caret(&mut source, &mut anchors.node(1), REPORTED_AT).expect("the fake answers");
    assert_eq!(
        anchors
            .node(1)
            .anchors
            .held
            .get(&selected.held.0)
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
fn sentence_from(
    caret: usize,
    forward: bool,
) -> (
    Result<SentenceMoved, verbatim_model::batch::StepOutcome>,
    usize,
) {
    let mut source = Fake::new("One. Two.\nThree.", caret);
    let mut anchors = store();
    let at = ops::caret(&mut source, &mut anchors.node(1), REPORTED_AT).expect("the fake answers");
    let moved = sentence::move_by_sentence(
        &mut source,
        &mut anchors.node(1),
        (at.held, forward),
        &mut || REPORTED_AT,
    )
    .expect("the fake answers");
    (moved, source.caret)
}

/// The sentence and line a move answered, `None` for another answer.
fn moved_to(
    moved: &Result<SentenceMoved, verbatim_model::batch::StepOutcome>,
) -> Option<(String, String, u32)> {
    match moved {
        Ok(SentenceMoved::Moved { sentence, line, .. }) => {
            Some((sentence.clone(), line.text.clone(), line.offset))
        }
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
    let Ok(SentenceMoved::End { line, .. }) = reply else {
        panic!("the end, not {reply:?}");
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
    let Ok(SentenceMoved::StayedAtStart { sentence, .. }) = reply else {
        panic!("stayed at the start, not {reply:?}");
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
        sentence::move_by_sentence(
            &mut source,
            &mut anchors.node(1),
            (TextAnchor(77), true),
            &mut || REPORTED_AT
        ),
        Ok(Err(verbatim_model::batch::StepOutcome::NotThere(
            verbatim_model::batch::Missing::Anchor(TextAnchor(77))
        )))
    );
    source.sentences = Sentences::Unsupported;
    assert_eq!(
        sentence::move_by_sentence(
            &mut source,
            &mut anchors.node(1),
            (TextAnchor(77), true),
            &mut || REPORTED_AT
        ),
        Ok(Err(verbatim_model::batch::StepOutcome::Refused(
            verbatim_model::batch::StepRefusal::NoSuchUnit
        )))
    );
    assert_eq!(source.caret, 0);
}

/// A wait's check of the key of `motion` in `before`, from the `Caret`
/// read there, after `after`, at the bound when `last`; the wait's
/// starting texts the line read then and, for a key that compares it,
/// `word`.
fn watched(
    before: &mut Fake,
    after: impl FnOnce(&mut Fake),
    (motion, last, word): (CaretMotion, bool, Option<&str>),
) -> TextResult<Option<verbatim_model::batch::Waited>> {
    let mut anchors = store();
    let at = ops::caret(before, &mut anchors.node(1), REPORTED_AT).expect("the fake answers");
    let line = line_at(before, &mut anchors.node(1), at.caret.anchor);
    let start = anchors.node(1).held_read(at.held).expect("the read held");
    let wait = verbatim_model::batch::CaretWait {
        from: at.held,
        line: line.text,
        offset: line.offset,
        word: word.map(str::to_owned),
        effect: verbatim_model::batch::EffectTest {
            unit: motion.compares_unit(),
            line: motion.deletes(),
        },
        passed_at_us: REPORTED_AT,
        poll_us: 10_000,
        bound_us: 100_000,
        events: false,
    };
    after(before);
    check::check(
        before,
        &start,
        check::CheckAsk {
            wait: &wait,
            word: word.is_some(),
            last,
        },
    )
}

/// A check that finds the caret moved from the starting point ends the
/// wait with the effect.
#[test]
fn a_check_that_finds_the_caret_moved_answers_the_effect() {
    let found = watched(
        &mut Fake::new("alpha beta\n", 2),
        |fake| fake.caret = 3,
        (CaretMotion::NextCharacter, false, None),
    );
    assert_eq!(
        found.expect("read"),
        Some(verbatim_model::batch::Waited::Effect)
    );
}

/// A check that finds nothing before the bound ends nothing; at the bound
/// it ends the wait at its bound.
#[test]
fn a_check_with_no_effect_waits_until_the_bound() {
    let early = watched(
        &mut Fake::new("alpha\n", 5),
        |_| {},
        (CaretMotion::NextCharacter, false, None),
    );
    assert_eq!(early.expect("read"), None);
    let last = watched(
        &mut Fake::new("alpha\n", 0),
        |_| {},
        (CaretMotion::PreviousCharacter, true, None),
    );
    assert_eq!(
        last.expect("read"),
        Some(verbatim_model::batch::Waited::Bound)
    );
}

/// Delete's effect: the word at the caret changed, the caret not, compared
/// with the starting word the wait names.
#[test]
fn deletes_effect_is_the_word_at_the_caret_changing() {
    let found = watched(
        &mut Fake::new("alpha\n", 1),
        |fake| {
            fake.text.remove(1);
        },
        (CaretMotion::Delete, false, Some("alpha")),
    );
    assert_eq!(
        found.expect("read"),
        Some(verbatim_model::batch::Waited::Effect)
    );
}

/// A caret that moved while the check read it is read again rather than
/// ending on a torn read.
#[test]
fn a_check_whose_caret_moved_while_read_reads_again() {
    let mut source = Fake::new("alpha\n", 0);
    let found = watched(
        &mut source,
        |fake| {
            fake.caret = 1;
            fake.moves_on_read = Some((fake.caret_reads + 2, 2));
        },
        (CaretMotion::NextCharacter, false, None),
    );
    assert_eq!(
        found.expect("read"),
        Some(verbatim_model::batch::Waited::Effect)
    );
    assert_eq!(source.caret, 2, "read again where the caret came to rest");
}
