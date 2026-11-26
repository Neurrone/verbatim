//! How an outpost's answers that carry large texts or colour runs cross
//! the pipe (`docs/design/communication.md`, section 4.2): a batch's
//! answer whose steps answered a trimmed range's text (`ReadRange`), the
//! visible text (`Visible`), a search's matches (`FindText`) or a screen's
//! colours (`ReadColours`), whichever text they read. Each such answer is
//! a frame with a raw section.
//! Every such text is in the raw section as UTF-8, its place in the JSON
//! body a byte range, `[start, length]`; and colour runs are there as
//! 16-byte records, four little-endian `u32`s (column, length, foreground,
//! background), the body giving their range and each row's count of runs.
//! Everything else in the answer, a batch's other steps included, stays in
//! the body as the model's own JSON.
//!
//! The model's types describe the content and know nothing of this: the
//! answer is converted to the wire types here as it is encoded, borrowing
//! from it, and back as it is decoded, each text validated and copied into
//! a `String` of its own. A range outside the raw section, a text that is
//! not UTF-8, colour records that do not fill their range, or ranges that
//! do not tile the raw section, each starting where the last ended and the
//! last ending at its end, is a protocol error.

use std::borrow::Cow;
use std::io;

use serde::{Deserialize, Serialize};
use verbatim_model::batch::{Done, FoundText, RangeText, StepOutcome, VisibleText};
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::request::{Answer, Outcome, Probe, Reply, RequestNumber};
use verbatim_model::{TerminalColourRun, TextAnchor};

/// A place in the raw section: where it starts and how many bytes it
/// takes, written `[start, length]`.
type Range = (usize, usize);

/// The bytes of one colour run's record.
const RUN_BYTES: usize = 16;

/// [`OutpostToCore`] as the body of a frame with a raw section gives it:
/// only the answers that carry a text or colours there.
#[derive(Serialize, Deserialize)]
enum Wire<'a> {
    Answer(WireAnswer<'a>),
}

/// An [`Answer`] with a result in the raw section.
#[derive(Serialize, Deserialize)]
struct WireAnswer<'a> {
    number: RequestNumber,
    outcome: WireOutcome<'a>,
    #[serde(default, skip_serializing_if = "<[Probe]>::is_empty")]
    probes: Cow<'a, [Probe]>,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    detail: Cow<'a, str>,
    began_us: u64,
    ended_us: u64,
}

/// [`Outcome::Result`] with a result in the raw section.
#[derive(Serialize, Deserialize)]
enum WireOutcome<'a> {
    Result(WireResult<'a>),
}

/// A [`Reply`] with texts or colours in the raw section.
#[derive(Serialize, Deserialize)]
enum WireResult<'a> {
    Steps(Vec<WireStep<'a>>),
}

/// One step's outcome of a batch's answer.
#[derive(Serialize, Deserialize)]
enum WireStep<'a> {
    /// An outcome with nothing in the raw section, as the model's own JSON.
    Body(Cow<'a, StepOutcome>),
    /// A trimmed [`RangeText`], its text in the raw section.
    ReadRange {
        text: Range,
        truncated: bool,
        padded: Cow<'a, [(u32, u32)]>,
    },
    /// A [`VisibleText`], its text in the raw section.
    Visible {
        start: TextAnchor,
        end: TextAnchor,
        at_text_start: bool,
        text: Range,
        padded: Cow<'a, [(u32, u32)]>,
    },
    /// A search's matches, each one's text in the raw section.
    FindText(Vec<Found>),
    /// A screen's colour runs, in the raw section.
    ReadColours(Colours),
}

/// A [`FoundText`], its text in the raw section.
#[derive(Serialize, Deserialize)]
struct Found {
    to_end: u32,
    text: Range,
}

/// A screen's colour runs: their records' range in the raw section, and how
/// many runs each row has, row by row.
#[derive(Serialize, Deserialize)]
struct Colours {
    raw: Range,
    rows: Vec<usize>,
}

/// What goes in the raw section, in order.
enum Piece<'a> {
    Text(&'a str),
    Colours(&'a [Vec<TerminalColourRun>]),
}

/// An answer with texts or colours for the raw section, ready to be
/// framed: its body borrowing from it, and the pieces of its raw section.
pub(super) struct WithRaw<'a> {
    wire: Wire<'a>,
    layout: Layout<'a>,
}

/// A raw section as it is laid out: its pieces, in order, and its length.
#[derive(Default)]
struct Layout<'a> {
    pieces: Vec<Piece<'a>>,
    length: usize,
}

impl<'a> WithRaw<'a> {
    /// `message` ready to be framed, when it is an answer with a text or
    /// colours for the raw section; `None` for every other message, framed
    /// as a body alone.
    pub(super) fn of(message: &'a OutpostToCore) -> Option<Self> {
        let OutpostToCore::Answer(Answer {
            number,
            outcome: Outcome::Result(reply),
            probes,
            detail,
            began_us,
            ended_us,
        }) = message
        else {
            return None;
        };
        let mut layout = Layout::default();
        let Reply::Steps(steps) = reply;
        if !steps.iter().any(in_raw) {
            return None;
        }
        let result = WireResult::Steps(steps.iter().map(|step| layout.step(step)).collect());
        Some(Self {
            wire: Wire::Answer(WireAnswer {
                number: *number,
                outcome: WireOutcome::Result(result),
                probes: Cow::Borrowed(probes),
                detail: Cow::Borrowed(detail),
                began_us: *began_us,
                ended_us: *ended_us,
            }),
            layout,
        })
    }
}

/// Whether `step`'s outcome has a text or colours for the raw section.
fn in_raw(step: &StepOutcome) -> bool {
    matches!(
        step,
        StepOutcome::Done(
            Done::ReadRange(RangeText {
                padded: Some(_),
                ..
            }) | Done::Visible(_)
                | Done::FindText(_)
                | Done::ReadColours(_)
        )
    )
}

impl<'a> Layout<'a> {
    /// `step` as the body gives it, its text or colours placed next in the
    /// raw section.
    fn step(&mut self, step: &'a StepOutcome) -> WireStep<'a> {
        match step {
            StepOutcome::Done(Done::ReadRange(RangeText {
                text,
                truncated,
                padded: Some(padded),
            })) => WireStep::ReadRange {
                text: self.text(text),
                truncated: *truncated,
                padded: Cow::Borrowed(padded),
            },
            StepOutcome::Done(Done::Visible(VisibleText {
                start,
                end,
                at_text_start,
                text,
                padded,
            })) => WireStep::Visible {
                start: *start,
                end: *end,
                at_text_start: *at_text_start,
                text: self.text(text),
                padded: Cow::Borrowed(padded),
            },
            StepOutcome::Done(Done::FindText(matches)) => WireStep::FindText(
                matches
                    .iter()
                    .map(|FoundText { to_end, text }| Found {
                        to_end: *to_end,
                        text: self.text(text),
                    })
                    .collect(),
            ),
            StepOutcome::Done(Done::ReadColours(rows)) => WireStep::ReadColours(self.colours(rows)),
            other => WireStep::Body(Cow::Borrowed(other)),
        }
    }
}

impl<'a> Layout<'a> {
    /// Places `text` next in the raw section.
    fn text(&mut self, text: &'a str) -> Range {
        let range = (self.length, text.len());
        self.length += text.len();
        if !text.is_empty() {
            self.pieces.push(Piece::Text(text));
        }
        range
    }

    /// Places `rows`' runs next in the raw section.
    fn colours(&mut self, rows: &'a [Vec<TerminalColourRun>]) -> Colours {
        let counts: Vec<usize> = rows.iter().map(Vec::len).collect();
        let length = counts.iter().sum::<usize>() * RUN_BYTES;
        let range = (self.length, length);
        self.length += length;
        self.pieces.push(Piece::Colours(rows));
        Colours {
            raw: range,
            rows: counts,
        }
    }
}

impl WithRaw<'_> {
    /// Appends the answer's frame to `buffer`: the body, then the raw
    /// section, in one buffer, so the frame is still one write.
    pub(super) fn put_frame(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
        crate::transport::put_frame_with_raw(
            buffer,
            |body| serde_json::to_writer(body, &self.wire).map_err(io::Error::other),
            |raw| {
                raw.reserve(self.layout.length);
                for piece in &self.layout.pieces {
                    match piece {
                        Piece::Text(text) => raw.extend_from_slice(text.as_bytes()),
                        Piece::Colours(rows) => {
                            for run in rows.iter().flatten() {
                                for value in
                                    [run.column, run.length, run.foreground, run.background]
                                {
                                    raw.extend_from_slice(&value.to_le_bytes());
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        )
    }
}

/// Reads a message from the body and raw section of its frame.
pub(super) fn decode(body: &[u8], raw: &[u8]) -> io::Result<OutpostToCore> {
    // A body that is not such an answer is one that may not have a raw
    // section.
    let Wire::Answer(WireAnswer {
        number,
        outcome: WireOutcome::Result(result),
        probes,
        detail,
        began_us,
        ended_us,
    }) = serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))?;
    let mut section = Section {
        bytes: raw,
        next: 0,
    };
    let WireResult::Steps(steps) = result;
    let steps: Vec<StepOutcome> = steps
        .into_iter()
        .map(|step| section.step(step))
        .collect::<io::Result<_>>()?;
    // A batch's answer with nothing for the raw section is framed as its
    // body alone.
    if !steps.iter().any(in_raw) {
        return Err(invalid(
            "a frame has a raw section its batch's answer does not take",
        ));
    }
    let reply = Reply::Steps(steps);
    section.finish()?;
    Ok(OutpostToCore::Answer(Answer {
        number,
        outcome: Outcome::Result(reply),
        probes: probes.into_owned(),
        detail: detail.into_owned(),
        began_us,
        ended_us,
    }))
}

/// A frame's raw section as its message is read from it: the bytes, and
/// where the next range must start, the end of the ranges read so far.
struct Section<'s> {
    bytes: &'s [u8],
    next: usize,
}

impl<'s> Section<'s> {
    /// A batch's step from the body's `step`, its text or colours read
    /// from the raw section.
    fn step(&mut self, step: WireStep<'_>) -> io::Result<StepOutcome> {
        Ok(match step {
            WireStep::Body(outcome) => {
                let outcome = outcome.into_owned();
                // An outcome the raw section takes is never in the body.
                if in_raw(&outcome) {
                    return Err(invalid(
                        "a step's text or colours are in the body of a frame with a raw section",
                    ));
                }
                outcome
            }
            WireStep::ReadRange {
                text,
                truncated,
                padded,
            } => StepOutcome::Done(Done::ReadRange(RangeText {
                text: self.text(text)?,
                truncated,
                padded: Some(padded.into_owned()),
            })),
            WireStep::Visible {
                start,
                end,
                at_text_start,
                text,
                padded,
            } => StepOutcome::Done(Done::Visible(VisibleText {
                start,
                end,
                at_text_start,
                text: self.text(text)?,
                padded: padded.into_owned(),
            })),
            WireStep::FindText(matches) => StepOutcome::Done(Done::FindText(
                matches
                    .into_iter()
                    .map(|Found { to_end, text }| {
                        Ok(FoundText {
                            to_end,
                            text: self.text(text)?,
                        })
                    })
                    .collect::<io::Result<_>>()?,
            )),
            WireStep::ReadColours(colours) => {
                StepOutcome::Done(Done::ReadColours(self.colours(colours)?))
            }
        })
    }

    /// The bytes at `range`, which must start where the last range ended,
    /// so the ranges tile the raw section in the order they are read.
    fn bytes(&mut self, (start, length): Range) -> io::Result<&'s [u8]> {
        let end = start
            .checked_add(length)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| invalid("a range outside the frame's raw section"))?;
        if start != self.next {
            return Err(invalid(
                "a range in the frame's raw section that does not start where the last ended",
            ));
        }
        self.next = end;
        Ok(&self.bytes[start..end])
    }

    /// The text at `range`, copied out.
    fn text(&mut self, range: Range) -> io::Result<String> {
        let bytes = self.bytes(range)?;
        simdutf8::basic::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| invalid("a text in the raw section is not UTF-8"))
    }

    /// The colour runs at `colours`' range, parted into its rows.
    fn colours(
        &mut self,
        Colours { raw, rows }: Colours,
    ) -> io::Result<Vec<Vec<TerminalColourRun>>> {
        let bytes = self.bytes(raw)?;
        let runs = rows
            .iter()
            .try_fold(0usize, |runs, &row| runs.checked_add(row))
            .and_then(|runs| runs.checked_mul(RUN_BYTES));
        if runs != Some(bytes.len()) {
            return Err(invalid(
                "colour runs do not fill their range in the raw section",
            ));
        }
        let mut records = bytes.as_chunks::<RUN_BYTES>().0.iter();
        Ok(rows
            .iter()
            .map(|&count| records.by_ref().take(count).map(run).collect())
            .collect())
    }

    /// Checks that the ranges read, each starting where the last ended,
    /// reached the raw section's end: they tile it exactly.
    fn finish(self) -> io::Result<()> {
        if self.next == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid(
                "a frame's raw section is not the length its message names",
            ))
        }
    }
}

/// One colour run from its record.
fn run(record: &[u8; RUN_BYTES]) -> TerminalColourRun {
    let (values, _) = record.as_chunks::<4>();
    let value = |at: usize| u32::from_le_bytes(values[at]);
    TerminalColourRun {
        column: value(0),
        length: value(1),
        foreground: value(2),
        background: value(3),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests;
