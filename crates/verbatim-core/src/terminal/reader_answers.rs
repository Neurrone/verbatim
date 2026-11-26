//! For the tests of the rules above the reader, which state a read as the
//! rules take it: the outcomes of a window read's steps that read back as
//! that read ([`outcomes`]), so that such a test answers Core's batch and
//! its read reaches the rules through the reader, as an outpost's does.
//! Every answer made here is read back by [`window_of`] and compared with
//! the read it was made from before it is given.

use verbatim_model::batch::{CaretAt, Chunks, Done, Moved, RangeText, StepOutcome, VisibleText};
use verbatim_model::{TextChunk, TextUnit};

use super::*;

/// The anchors an answer holds that no test names.
struct Anchors(u64);

impl Anchors {
    fn next(&mut self) -> TextAnchor {
        self.0 += 1;
        TextAnchor(self.0)
    }
}

/// A move that went `moved` lines and landed at `to`.
fn moved(anchors: &mut Anchors, moved: i64, to: Option<TextAnchor>) -> StepOutcome {
    StepOutcome::Done(Done::Move(Moved {
        moved: i32::try_from(moved).expect("rows"),
        from: anchors.next(),
        to: to.unwrap_or_else(|| anchors.next()),
    }))
}

/// A line, `text`, the point `offset` bytes into it.
fn chunk(anchors: &mut Anchors, text: String, offset: usize) -> TextChunk {
    TextChunk {
        unit: TextUnit::Line,
        text,
        start: anchors.next(),
        offset: u32::try_from(offset).expect("a short line"),
        languages: Vec::new(),
        first: false,
        last: false,
        truncated: false,
        formats: Vec::new(),
    }
}

/// One line read, `chunk`, with its padded lengths when it was trimmed.
fn line(chunk: TextChunk, trimmed: Option<Vec<(u32, u32)>>) -> StepOutcome {
    StepOutcome::Done(Done::ReadText(Chunks {
        moved: 0,
        chunks: vec![chunk],
        padded: trimmed,
    }))
}

/// The caret a read of `ask` that reads back as `reply` found, its line,
/// and the starting point a starting read holds: the report's line when
/// `reply` states one, and otherwise the caret's line with the caret at
/// its column.
fn caret_found(
    anchors: &mut Anchors,
    ask: &WindowAsk,
    reply: &WindowReply,
) -> (CaretAt, TextChunk, Option<TextAnchor>) {
    let point = ask
        .starting
        .then(|| reply.point.unwrap_or_else(|| anchors.next()));
    let caret = CaretAt {
        caret: anchors.next(),
        selection: None,
        held: point.unwrap_or_else(|| anchors.next()),
        offsets: None,
        reported: true,
    };
    let line = if let Some(report) = &reply.snapshot.caret {
        report.line.clone()
    } else {
        let text = reply.snapshot.caret_line.padded();
        let offset = text
            .char_indices()
            .nth(reply.snapshot.caret_column as usize)
            .map_or(text.len(), |(offset, _)| offset);
        chunk(anchors, text, offset)
    };
    (caret, line, point)
}

/// A trimmed range read as `text`.
fn range(text: &TerminalText) -> StepOutcome {
    StepOutcome::Done(Done::ReadRange(RangeText {
        text: text.text.clone(),
        truncated: false,
        padded: Some(text.lengths.clone()),
    }))
}

/// The outcomes of one start's steps, `above` rows asked above it: read
/// as `snapshot` when `ran`, skipped otherwise.
fn set(
    anchors: &mut Anchors,
    (above, ran): (u32, bool),
    snapshot: &TerminalSnapshot,
) -> Vec<StepOutcome> {
    let mut outcomes = Vec::new();
    if above > 0 {
        outcomes.push(moved(anchors, -i64::from(snapshot.above), None));
    }
    outcomes.push(range(&snapshot.text));
    outcomes.push(moved(
        anchors,
        if snapshot.at_document_start { 0 } else { -1 },
        None,
    ));
    if !ran {
        outcomes.fill(StepOutcome::Skipped);
    }
    outcomes
}

/// The outcomes of the window read `ask` names of terminal `node` that
/// read back as `reply`, with when the read ran, as its answer stamps it.
/// The read back differs from `reply` only where the reader decides: a
/// caret report where `reply` has none, a starting point for a starting
/// read, the colours for a read that asks them, and a named start not used
/// read as refused.
///
/// # Panics
///
/// When `reply` is no read the batch can give: the reader does not read
/// the outcomes back as it.
pub(crate) fn outcomes(
    node: NodeId,
    ask: &WindowAsk,
    reply: &WindowReply,
) -> (Vec<StepOutcome>, (u64, u64)) {
    let snapshot = &reply.snapshot;
    let mut anchors = Anchors(1_000_000);
    let (height, above) = match ask.read {
        WindowRead::First => (0, 0),
        WindowRead::From { height, .. } => (height, height),
        WindowRead::Whole { height } => (height, 0),
        WindowRead::HistoryBlock { height } => (height.get(), height.get()),
    };
    let (caret, chunk, point) = caret_found(&mut anchors, ask, reply);
    let caret_line = line(chunk.clone(), None);
    let caret_steps = [StepOutcome::Done(Done::Caret(caret)), caret_line];
    let mut outcomes = Vec::new();
    if ask.starting {
        outcomes.extend(caret_steps.clone());
    }
    outcomes.push(StepOutcome::Done(Done::Visible(VisibleText {
        start: if height > 0 {
            anchors.next()
        } else {
            reply.start
        },
        end: anchors.next(),
        at_text_start: snapshot.alternate,
        text: snapshot.screen.text.clone(),
        padded: snapshot.screen.lengths.clone(),
    })));
    if ask.colours {
        let top = self::chunk(&mut anchors, String::new(), 0);
        outcomes.push(line(top, Some(Vec::new())));
    }
    outcomes.push(moved(&mut anchors, 0, None));
    if height > 0 {
        outcomes.push(moved(&mut anchors, -i64::from(height), Some(reply.start)));
    }
    match ask.read {
        WindowRead::Whole { .. } => outcomes.push(range(&snapshot.text)),
        WindowRead::First | WindowRead::HistoryBlock { .. } => {
            outcomes.extend(set(&mut anchors, (above, true), snapshot));
        }
        WindowRead::From { .. } => {
            outcomes.extend(set(&mut anchors, (above, snapshot.from_start), snapshot));
            outcomes.extend(set(&mut anchors, (above, !snapshot.from_start), snapshot));
        }
    }
    let first = self::chunk(&mut anchors, snapshot.first_row.text.clone(), 0);
    outcomes.push(line(first, Some(snapshot.first_row.lengths.clone())));
    if !ask.starting {
        outcomes.extend(caret_steps);
    }
    outcomes.push(moved(
        &mut anchors,
        i64::from(snapshot.caret_rows_to_end),
        None,
    ));
    let colours = ask
        .colours
        .then(|| snapshot.colours.clone().unwrap_or_default());
    if let Some(colours) = &colours {
        outcomes.push(StepOutcome::Done(Done::ReadColours(colours.clone())));
    }
    let ran = (reply.started_us, snapshot.ended_ms * 1_000);
    // What the reader reads back, checked against the read stated.
    let expected = WindowReply {
        snapshot: TerminalSnapshot {
            shrank: ask.start().is_some() && !snapshot.from_start,
            colours,
            caret: Some(crate::text_facts::caret_report(&caret, chunk)),
            ..snapshot.clone()
        },
        point,
        ..reply.clone()
    };
    let (sent, steps) = window_batch(node, ask);
    let answers = Answers::parse(
        &sent,
        verbatim_model::request::Reply::Steps(outcomes.clone()),
    )
    .expect("an outcome of each step's kind");
    assert_eq!(
        window_of(&answers, &steps, ask, ran),
        Ok(expected),
        "the read stated is not one {ask:?} can give"
    );
    (outcomes, ran)
}
