//! The text protocol over an in-memory source: chunks, positions and
//! anchors, the wait for evidence, selection changes, and reads.

use std::sync::atomic::AtomicU64;

use verbatim_model::{CaretKey, CaretMotion};

use super::*;
use crate::outpost::text_reads::{CaretEvents, EventWait};

/// An in-memory text whose positions are UTF-16 offsets. Lines end after
/// each line feed; a word is a run of letters and digits with the spaces
/// after it, or one other character; a paragraph is a line.
struct Fake {
    text: Vec<u16>,
    caret: usize,
    selection: Option<(usize, usize)>,
    sentences: Sentences,
    /// How many times the caret was read.
    caret_reads: usize,
    /// Moves the caret to this offset on the given caret read, as an
    /// application handling a key a little later would.
    moves_on_read: Option<(usize, usize)>,
}

impl Fake {
    fn new(text: &str, caret: usize) -> Self {
        Self {
            text: text.encode_utf16().collect(),
            caret,
            selection: None,
            sentences: Sentences::ByParagraph,
            caret_reads: 0,
            moves_on_read: None,
        }
    }

    fn lines(&self) -> Vec<(usize, usize)> {
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
            TextUnit::Line | TextUnit::Paragraph => Some(self.lines()),
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

/// A clock that advances only when waited on, and caret events on demand.
struct FakeSignal {
    now: Instant,
    events: bool,
    waits: usize,
    /// How many waits had passed each time the wait said it ended.
    awaited: Vec<usize>,
}

impl FakeSignal {
    fn new() -> Self {
        Self {
            now: Instant::now(),
            events: false,
            waits: 0,
            awaited: Vec::new(),
        }
    }
}

impl CaretSignal for FakeSignal {
    fn caret_event(&mut self) -> bool {
        self.events
    }

    fn wait(&mut self, timeout: Duration) {
        self.waits += 1;
        self.now += timeout;
    }

    fn now(&mut self) -> Instant {
        self.now
    }

    fn now_ms(&mut self) -> u64 {
        WAITED_AT
    }

    fn awaited(&mut self) {
        self.awaited.push(self.waits);
    }
}

/// When [`report`] reads the caret, in milliseconds since the Unix epoch.
const REPORTED_AT: u64 = 1_000;

/// When [`watch`]'s key was pressed, after [`report`]'s reads.
const PRESSED_AT: u64 = 2_000;

/// When a [`FakeSignal`] says a wait reads the caret, after the key.
const WAITED_AT: u64 = 2_050;

fn store() -> Anchors<usize> {
    Anchors::new(Arc::new(AtomicU64::new(0)))
}

fn report(source: &mut Fake, anchors: &mut Anchors<usize>) -> CaretReport {
    report_at(source, anchors, REPORTED_AT)
}

/// A caret report from a read that finished at `read_at_ms`.
fn report_at(source: &mut Fake, anchors: &mut Anchors<usize>, read_at_ms: u64) -> CaretReport {
    caret_report(source, &mut anchors.node(1), &mut || read_at_ms, false)
        .expect("the fake answers")
        .0
}

fn watch(since: Option<TextPosition>, unit: TextUnit) -> CaretWatch {
    CaretWatch {
        since,
        pressed_at_ms: PRESSED_AT,
        unit,
        compare: None,
        previous_selection: None,
        wait: CaretWait::Standard,
    }
}

fn caret_reply(reply: TextReply) -> CaretReply {
    match reply {
        TextReply::Caret(reply) => *reply,
        other => panic!("a caret reply, not {other:?}"),
    }
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
        &mut FakeSignal::new(),
    );
    let TextReply::Read { moved: 0, chunk } = reply else {
        panic!("a read, not {reply:?}");
    };
    assert_eq!(chunk.text, "x");
    assert_eq!(chunk.offset, 0);
}

#[test]
fn an_anchor_is_forgotten_after_64_newer_unless_core_holds_it() {
    let mut source = Fake::new("one\ntwo", 0);
    let mut anchors = store();
    let first = report(&mut source, &mut anchors).line.start;
    let held = report(&mut source, &mut anchors).line.start;
    anchors.set_held([held.0]);
    for _ in 0..KEPT_ANCHORS {
        report(&mut source, &mut anchors);
    }
    let read = |anchors: &mut Anchors<usize>, source: &mut Fake, anchor| {
        perform(
            source,
            &mut anchors.node(1),
            &TextOp::Read(TextRead {
                at: TextPoint::At(TextPosition { anchor, offset: 1 }),
                movement: None,
                unit: TextUnit::Line,
            }),
            &mut FakeSignal::new(),
        )
    };
    assert_eq!(
        read(&mut anchors, &mut source, first),
        TextReply::AnchorLost
    );
    assert!(matches!(
        read(&mut anchors, &mut source, held),
        TextReply::Read { .. }
    ));
}

#[test]
fn a_caret_that_moved_from_where_core_knew_it_is_evidence_at_once() {
    let mut source = Fake::new("one two\nthree", 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.caret = 4;
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(since), TextUnit::Word)),
        &mut signal,
    ));
    assert!(reply.moved);
    assert_eq!(signal.waits, 0, "no waiting once the caret moved");
    assert_eq!(reply.unit.expect("the provider's word").text, "two");
    assert_eq!(reply.caret.line.text, "one two\n");
    assert_eq!(reply.caret.line.offset, 4);
}

#[test]
fn a_caret_moved_late_is_found_by_polling() {
    let mut source = Fake::new("one\ntwo", 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    source.caret_reads = 0;
    source.moves_on_read = Some((3, 4));
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(TextPosition::at(line.start)), TextUnit::Line)),
        &mut signal,
    ));
    assert!(reply.moved);
    assert_eq!(signal.waits, 2);
    assert_eq!(signal.awaited, [2], "the wait ends once, after its polls");
    assert_eq!(reply.caret.line.text, "two");
    assert_eq!(reply.unit, None, "a line is the caret's line");
}

#[test]
fn with_no_evidence_the_wait_runs_out_and_reports_the_caret_anyway() {
    let mut source = Fake::new("only", 4);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(since), TextUnit::Character)),
        &mut signal,
    ));
    assert!(!reply.moved);
    assert_eq!(signal.waits, 10, "100 ms in 10 ms polls");
    assert_eq!(signal.awaited, [10], "the wait ends once, at its deadline");
    assert_eq!(reply.caret.line.text, "only");
    assert_eq!(reply.unit.expect("the character").text, "", "the end");
}

#[test]
fn a_changed_line_at_the_same_position_is_evidence() {
    // The application handled the key before the wait began, and the
    // position Core knew moved with the edit: only the line's text tells.
    let mut source = Fake::new("abcx", 4);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.text = "abcy".encode_utf16().collect();
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(since), TextUnit::Character)),
        &mut signal,
    ));
    assert!(reply.moved);
    assert_eq!(signal.waits, 0);
    assert_eq!(reply.caret.line.text, "abcy");
}

#[test]
fn a_caret_reported_after_the_one_core_knew_is_the_baseline() {
    // Core asked with the caret it knew; before the key, the outpost
    // reported a newer one (an earlier key's late caret event, or a
    // paste's), which Core had not had yet, and the key has done nothing
    // yet.
    let mut source = Fake::new("one two", 0);
    let mut anchors = store();
    let line = report_at(&mut source, &mut anchors, PRESSED_AT - 500).line;
    let known_to_core = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.caret = 4;
    report_at(&mut source, &mut anchors, PRESSED_AT - 1);
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(known_to_core), TextUnit::Character)),
        &mut signal,
    ));
    assert!(
        !reply.moved,
        "the change Core had not heard of is not the key's"
    );
    assert_eq!(signal.waits, 10);
}

#[test]
fn a_caret_reported_after_the_key_but_before_the_wait_is_not_the_baseline() {
    // Backspace after typing "x": the application handled the key and its
    // caret event reached the outpost, which reported the new caret, before
    // Core's request did. The caret as it was when the key was pressed is
    // the baseline, so the move is evidence.
    let mut source = Fake::new("delta epsilonx", 14);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let known_to_core = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.text = "delta epsilon".encode_utf16().collect();
    source.caret = 13;
    report_at(&mut source, &mut anchors, PRESSED_AT + 5);
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(known_to_core), TextUnit::Character)),
        &mut signal,
    ));
    assert!(reply.moved, "the key's own caret event is not the baseline");
    assert_eq!(signal.waits, 0);
    assert_eq!(reply.caret.line.text, "delta epsilon");
    assert_eq!(reply.caret.line.offset, 13);
}

#[test]
fn a_caret_read_in_the_same_millisecond_as_the_key_is_not_the_baseline() {
    // The read may have finished after the key reached the application.
    let mut source = Fake::new("one two", 0);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let known_to_core = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.caret = 4;
    report_at(&mut source, &mut anchors, PRESSED_AT);
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(known_to_core), TextUnit::Character)),
        &mut signal,
    ));
    assert!(reply.moved);
    assert_eq!(signal.waits, 0);
}

#[test]
fn a_line_that_changed_away_from_the_caret_or_a_late_event_is_no_evidence() {
    // The line wrapped anew and the application reported an earlier caret
    // change late; the caret itself has not moved.
    let mut source = Fake::new("one two", 4);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.text = "one two three".encode_utf16().collect();
    let mut signal = FakeSignal::new();
    signal.events = true;
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(since), TextUnit::Character)),
        &mut signal,
    ));
    assert!(!reply.moved);
    assert_eq!(signal.waits, 10, "the wait ran out");
}

/// The outpost's own caret events (`EventWait`), on a clock that moves on
/// by each wait's timeout when it runs out and stands still when an event
/// ends it, counting the caret's reads. The first read meets a caret event:
/// the application's late report of something earlier.
struct EventsOnce<'a> {
    events: &'a CaretEvents,
    wait: EventWait<'a>,
    now: Instant,
    reads: usize,
}

impl CaretSignal for EventsOnce<'_> {
    fn caret_event(&mut self) -> bool {
        self.wait.caret_event()
    }

    fn wait(&mut self, timeout: Duration) {
        if !self.wait.wait(timeout) {
            self.now += timeout;
        }
    }

    fn now(&mut self) -> Instant {
        self.now
    }

    fn now_ms(&mut self) -> u64 {
        WAITED_AT
    }

    fn reading(&mut self) {
        self.reads += 1;
        assert!(self.reads <= 12, "the wait read the caret without waiting");
        self.wait.reading();
        if self.reads == 1 {
            self.events.arrived();
        }
    }
}

#[test]
fn a_caret_event_during_the_wait_ends_one_wait_only() {
    // Core knew the caret, so the event is no evidence; it ends the wait
    // after the read it arrived during, and every later wait waits its
    // time again rather than returning at once.
    let mut source = Fake::new("only", 2);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    let events = CaretEvents::default();
    let mut signal = EventsOnce {
        events: &events,
        wait: EventWait::new(&events),
        now: Instant::now(),
        reads: 0,
    };
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(Some(since), TextUnit::Character)),
        &mut signal,
    ));
    assert!(!reply.moved);
    assert_eq!(
        signal.reads, 12,
        "the read the event met, one again at once, and one every 10 ms for 100 ms"
    );
}

#[test]
fn a_caret_event_is_evidence_when_core_did_not_know_the_caret() {
    let mut source = Fake::new("only", 0);
    let mut anchors = store();
    let mut signal = FakeSignal::new();
    signal.events = true;
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch(None, TextUnit::Character)),
        &mut signal,
    ));
    assert!(reply.moved);
    assert_eq!(reply.unit.expect("the character").text, "o");
}

#[test]
fn a_delete_that_changes_the_text_at_the_caret_is_evidence() {
    let mut source = Fake::new("abc", 1);
    let mut anchors = store();
    let line = report(&mut source, &mut anchors).line;
    let since = TextPosition {
        anchor: line.start,
        offset: line.offset,
    };
    source.text = "ac".encode_utf16().collect();
    let mut watch = watch(
        Some(since),
        CaretKey {
            motion: CaretMotion::Delete,
            select: false,
        }
        .motion
        .unit(),
    );
    watch.compare = Some("b".to_owned());
    let mut signal = FakeSignal::new();
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch),
        &mut signal,
    ));
    assert!(reply.moved, "the character at the caret changed");
    assert_eq!(signal.waits, 0);
    assert_eq!(reply.unit.expect("the character").text, "c");
}

/// A selecting key's reply, from a selection of `old` (equal ends for none,
/// with the caret there) to `new`.
fn selection_after(old: (usize, usize), new: Option<(usize, usize)>) -> Vec<(bool, String, u32)> {
    let mut source = Fake::new("hello world", old.0);
    source.selection = (old.0 != old.1).then_some(old);
    let mut anchors = store();
    let report = report(&mut source, &mut anchors);
    let previous = if let Some(selection) = report.selection {
        PreviousSelection {
            start: selection.start,
            end: selection.end,
        }
    } else {
        let caret = TextPosition {
            anchor: report.line.start,
            offset: report.line.offset,
        };
        PreviousSelection {
            start: caret,
            end: caret,
        }
    };
    source.selection = new;
    source.caret = new.map_or(old.0, |(start, _)| start);
    let mut watch = watch(None, TextUnit::Character);
    watch.previous_selection = Some(previous);
    let reply = caret_reply(perform(
        &mut source,
        &mut anchors.node(1),
        &TextOp::AwaitCaret(watch),
        &mut FakeSignal::new(),
    ));
    assert!(reply.moved, "a changed selection is evidence");
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

fn read(source: &mut Fake, anchors: &mut Anchors<usize>, read: TextRead) -> TextReply {
    perform(
        source,
        &mut anchors.node(1),
        &TextOp::Read(read),
        &mut FakeSignal::new(),
    )
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
        },
        &mut FakeSignal::new(),
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
        },
        &mut FakeSignal::new(),
    );
    assert_eq!(selected, TextReply::Done);
    assert_eq!(source.selection, Some((0, 5)));
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
