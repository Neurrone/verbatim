//! How an outpost's answers that carry a terminal's text cross the pipe
//! (`docs/design/communication.md`, section 4.2): the answer to a terminal
//! read, to a history read, and to a search. Each such answer is a frame
//! with a raw section. Every text it carries is in the raw section as
//! UTF-8, its place in the JSON body a byte range, `[start, length]`; and a
//! read's colour runs are there as 16-byte records, four little-endian
//! `u32`s (column, length, foreground, background), the body giving their
//! range and each row's count of runs. Everything else in the answer stays
//! in the body as the model's own JSON.
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
use verbatim_model::protocol::OutpostToCore;
use verbatim_model::request::{Answer, Outcome, Probe, Reply, RequestNumber};
use verbatim_model::{
    CaretReport, TerminalColourRun, TerminalMatch, TerminalReadReply, TerminalSnapshot,
    TerminalText, TextAnchor, TextReply,
};

/// A place in the raw section: where it starts and how many bytes it
/// takes, written `[start, length]`.
type Range = (usize, usize);

/// The bytes of one colour run's record.
const RUN_BYTES: usize = 16;

/// [`OutpostToCore`] as the body of a frame with a raw section gives it:
/// only the answers that carry a terminal's text.
#[derive(Serialize, Deserialize)]
enum Wire<'a> {
    Answer(WireAnswer<'a>),
}

/// An [`Answer`] with a terminal's reply.
#[derive(Serialize, Deserialize)]
struct WireAnswer<'a> {
    number: RequestNumber,
    outcome: WireOutcome<'a>,
    #[serde(default, skip_serializing_if = "<[Probe]>::is_empty")]
    probes: Cow<'a, [Probe]>,
    #[serde(default, skip_serializing_if = "str::is_empty")]
    detail: Cow<'a, str>,
}

/// [`Outcome::Result`] with a terminal's reply.
#[derive(Serialize, Deserialize)]
enum WireOutcome<'a> {
    Result(WireResult<'a>),
}

/// [`Reply::Text`] with a terminal's reply.
#[derive(Serialize, Deserialize)]
enum WireResult<'a> {
    Text(WireReply<'a>),
}

/// The [`TextReply`]s that carry a terminal's text.
#[derive(Serialize, Deserialize)]
#[expect(
    clippy::enum_variant_names,
    reason = "the variants' names are the model's, which the body carries"
)]
enum WireReply<'a> {
    TerminalRead(Box<WireRead<'a>>),
    TerminalHistory { text: Range, above: u32, read: u64 },
    TerminalFound { matches: Vec<Match>, read: u64 },
}

/// A [`TerminalReadReply`], its snapshot's texts and colours in the raw
/// section.
#[derive(Serialize, Deserialize)]
struct WireRead<'a> {
    snapshot: Snapshot<'a>,
    start: TextAnchor,
    started_us: u64,
    point: Option<TextAnchor>,
}

/// A [`TerminalMatch`], its text in the raw section.
#[derive(Serialize, Deserialize)]
struct Match {
    above: i32,
    text: Range,
}

/// A [`TerminalSnapshot`], its texts and colours in the raw section.
#[derive(Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the snapshot's own answers, as the model gives them"
)]
struct Snapshot<'a> {
    text: Text<'a>,
    screen: Text<'a>,
    from_start: bool,
    shrank: bool,
    at_document_start: bool,
    alternate: bool,
    first_row: Text<'a>,
    caret_rows_to_end: u32,
    caret_line: Text<'a>,
    caret_column: u32,
    above: u32,
    started_ms: u64,
    ended_ms: u64,
    waited: bool,
    read: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    colours: Option<Colours>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    caret: Option<Cow<'a, CaretReport>>,
}

/// A [`TerminalText`], its text in the raw section and its line lengths
/// in the body.
#[derive(Serialize, Deserialize)]
struct Text<'a> {
    raw: Range,
    lengths: Cow<'a, [(u32, u32)]>,
}

/// A read's colour runs: their records' range in the raw section, and how
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

/// An answer that carries a terminal's text, ready to be framed: its body
/// borrowing from it, and the pieces of its raw section.
pub(super) struct Terminal<'a> {
    wire: Wire<'a>,
    layout: Layout<'a>,
}

/// A raw section as it is laid out: its pieces, in order, and its length.
#[derive(Default)]
struct Layout<'a> {
    pieces: Vec<Piece<'a>>,
    length: usize,
}

impl<'a> Terminal<'a> {
    /// `message` ready to be framed, when it is an answer that carries a
    /// terminal's text; `None` for every other message, framed as a body
    /// alone.
    pub(super) fn of(message: &'a OutpostToCore) -> Option<Self> {
        let OutpostToCore::Answer(Answer {
            number,
            outcome: Outcome::Result(Reply::Text(reply)),
            probes,
            detail,
        }) = message
        else {
            return None;
        };
        let mut layout = Layout::default();
        let reply = match reply {
            TextReply::TerminalRead(read) => {
                let TerminalReadReply {
                    snapshot,
                    start,
                    started_us,
                    point,
                } = &**read;
                WireReply::TerminalRead(Box::new(WireRead {
                    snapshot: layout.snapshot(snapshot),
                    start: *start,
                    started_us: *started_us,
                    point: *point,
                }))
            }
            TextReply::TerminalHistory { text, above, read } => WireReply::TerminalHistory {
                text: layout.text(text),
                above: *above,
                read: *read,
            },
            TextReply::TerminalFound { matches, read } => WireReply::TerminalFound {
                matches: matches
                    .iter()
                    .map(|TerminalMatch { above, text }| Match {
                        above: *above,
                        text: layout.text(text),
                    })
                    .collect(),
                read: *read,
            },
            _ => return None,
        };
        Some(Self {
            wire: Wire::Answer(WireAnswer {
                number: *number,
                outcome: WireOutcome::Result(WireResult::Text(reply)),
                probes: Cow::Borrowed(probes),
                detail: Cow::Borrowed(detail),
            }),
            layout,
        })
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

    fn terminal_text(&mut self, text: &'a TerminalText) -> Text<'a> {
        let TerminalText { text, lengths } = text;
        Text {
            raw: self.text(text),
            lengths: Cow::Borrowed(lengths),
        }
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

    fn snapshot(&mut self, snapshot: &'a TerminalSnapshot) -> Snapshot<'a> {
        let TerminalSnapshot {
            text,
            screen,
            from_start,
            shrank,
            at_document_start,
            alternate,
            first_row,
            caret_rows_to_end,
            caret_line,
            caret_column,
            above,
            started_ms,
            ended_ms,
            waited,
            read,
            colours,
            caret,
        } = snapshot;
        Snapshot {
            text: self.terminal_text(text),
            screen: self.terminal_text(screen),
            from_start: *from_start,
            shrank: *shrank,
            at_document_start: *at_document_start,
            alternate: *alternate,
            first_row: self.terminal_text(first_row),
            caret_rows_to_end: *caret_rows_to_end,
            caret_line: self.terminal_text(caret_line),
            caret_column: *caret_column,
            above: *above,
            started_ms: *started_ms,
            ended_ms: *ended_ms,
            waited: *waited,
            read: *read,
            colours: colours.as_deref().map(|rows| self.colours(rows)),
            caret: caret.as_ref().map(Cow::Borrowed),
        }
    }
}

impl Terminal<'_> {
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
    // A body that is not a terminal's answer is one that may not have a
    // raw section.
    let Wire::Answer(WireAnswer {
        number,
        outcome: WireOutcome::Result(WireResult::Text(reply)),
        probes,
        detail,
    }) = serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))?;
    let mut section = Section {
        bytes: raw,
        next: 0,
    };
    let reply = match reply {
        WireReply::TerminalRead(read) => {
            let WireRead {
                snapshot,
                start,
                started_us,
                point,
            } = *read;
            TextReply::TerminalRead(Box::new(TerminalReadReply {
                snapshot: section.snapshot(snapshot)?,
                start,
                started_us,
                point,
            }))
        }
        WireReply::TerminalHistory { text, above, read } => TextReply::TerminalHistory {
            text: section.text(text)?,
            above,
            read,
        },
        WireReply::TerminalFound { matches, read } => TextReply::TerminalFound {
            matches: matches
                .into_iter()
                .map(|Match { above, text }| {
                    Ok(TerminalMatch {
                        above,
                        text: section.text(text)?,
                    })
                })
                .collect::<io::Result<_>>()?,
            read,
        },
    };
    section.finish()?;
    Ok(OutpostToCore::Answer(Answer {
        number,
        outcome: Outcome::Result(Reply::Text(reply)),
        probes: probes.into_owned(),
        detail: detail.into_owned(),
    }))
}

/// A frame's raw section as its message is read from it: the bytes, and
/// where the next range must start, the end of the ranges read so far.
struct Section<'s> {
    bytes: &'s [u8],
    next: usize,
}

impl<'s> Section<'s> {
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
            .map_err(|_| invalid("a terminal's text in the raw section is not UTF-8"))
    }

    fn terminal_text(&mut self, Text { raw, lengths }: Text<'_>) -> io::Result<TerminalText> {
        Ok(TerminalText {
            text: self.text(raw)?,
            lengths: lengths.into_owned(),
        })
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
                "a read's colour runs do not fill their range in the raw section",
            ));
        }
        let mut records = bytes.as_chunks::<RUN_BYTES>().0.iter();
        Ok(rows
            .iter()
            .map(|&count| records.by_ref().take(count).map(run).collect())
            .collect())
    }

    fn snapshot(&mut self, snapshot: Snapshot<'_>) -> io::Result<TerminalSnapshot> {
        let Snapshot {
            text,
            screen,
            from_start,
            shrank,
            at_document_start,
            alternate,
            first_row,
            caret_rows_to_end,
            caret_line,
            caret_column,
            above,
            started_ms,
            ended_ms,
            waited,
            read,
            colours,
            caret,
        } = snapshot;
        Ok(TerminalSnapshot {
            text: self.terminal_text(text)?,
            screen: self.terminal_text(screen)?,
            from_start,
            shrank,
            at_document_start,
            alternate,
            first_row: self.terminal_text(first_row)?,
            caret_rows_to_end,
            caret_line: self.terminal_text(caret_line)?,
            caret_column,
            above,
            started_ms,
            ended_ms,
            waited,
            read,
            colours: colours.map(|colours| self.colours(colours)).transpose()?,
            caret: caret.map(Cow::into_owned),
        })
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
