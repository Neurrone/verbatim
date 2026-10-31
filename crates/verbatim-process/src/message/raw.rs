//! How an outpost's messages that carry a terminal's text cross the pipe
//! (`docs/design/communication.md`, section 4.2): an event with a
//! terminal's read, and the replies with one, with its history, or with
//! the rows a search found. Each such message is a frame with a raw
//! section. Every text it carries is in the raw section as UTF-8, its
//! place in the JSON body a byte range, `[start, length]`; and a read's
//! colour runs are there as 16-byte records, four little-endian `u32`s
//! (column, length, foreground, background), the body giving their range
//! and each row's count of runs. Everything else in the message stays in
//! the body as the model's own JSON.
//!
//! The model's types describe the content and know nothing of this: the
//! message is converted to the wire types here as it is encoded, borrowing
//! from it, and back as it is decoded, each text validated and copied into
//! a `String` of its own. A range outside the raw section, a text that is
//! not UTF-8, colour records that do not fill their range, or ranges that
//! do not tile the raw section, each starting where the last ended and the
//! last ending at its end, is a protocol error.

use std::borrow::Cow;
use std::io;

use serde::{Deserialize, Serialize};
use verbatim_model::{
    Backend, CaretReport, NodeId, NormalizedEvent, Pid, TerminalColourRun, TerminalMatch,
    TerminalSnapshot, TerminalText, TextReply, TraceId, WindowFacts,
};

use verbatim_model::protocol::{EventTiming, OutpostToCore, QueryOutcome, QueryResult};

/// A place in the raw section: where it starts and how many bytes it
/// takes, written `[start, length]`.
type Range = (usize, usize);

/// The bytes of one colour run's record.
const RUN_BYTES: usize = 16;

/// [`OutpostToCore`] as the body of a frame with a raw section
/// gives it: only the messages that carry a terminal's text.
#[derive(Serialize, Deserialize)]
enum Wire<'a> {
    Event {
        trace_id: TraceId,
        observed_at_us: u64,
        backend: Backend,
        window: Option<Cow<'a, WindowFacts>>,
        application: Option<Pid>,
        timing: EventTiming,
        event: WireEvent<'a>,
    },
    Reply {
        trace_id: TraceId,
        request_id: u64,
        outcome: WireOutcome<'a>,
        timing: EventTiming,
    },
}

/// [`NormalizedEvent::TerminalSnapshot`].
#[derive(Serialize, Deserialize)]
enum WireEvent<'a> {
    TerminalSnapshot {
        node_id: NodeId,
        snapshot: Box<Snapshot<'a>>,
    },
}

/// [`QueryOutcome::Done`] with a terminal's reply.
#[derive(Serialize, Deserialize)]
enum WireOutcome<'a> {
    Done(WireResult<'a>),
}

/// [`QueryResult::Text`] with a terminal's reply.
#[derive(Serialize, Deserialize)]
enum WireResult<'a> {
    Text(WireReply<'a>),
}

/// The [`TextReply`]s that carry a terminal's text.
#[derive(Serialize, Deserialize)]
#[expect(
    clippy::enum_variant_names,
    reason = "the model's variants' names, which the JSON carries"
)]
enum WireReply<'a> {
    TerminalSnapshot(Box<Snapshot<'a>>),
    TerminalHistory { text: Range, above: u32, read: u64 },
    TerminalFound { matches: Vec<Match>, read: u64 },
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

/// A message that carries a terminal's text, ready to be framed: its body
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
    /// `message` ready to be framed, when it carries a terminal's text;
    /// `None` for every other message, framed as a body alone.
    pub(super) fn of(message: &'a OutpostToCore) -> Option<Self> {
        let mut layout = Layout::default();
        let wire = match message {
            OutpostToCore::Event {
                trace_id,
                observed_at_us,
                backend,
                window,
                application,
                timing,
                event: NormalizedEvent::TerminalSnapshot { node_id, snapshot },
            } => Wire::Event {
                trace_id: *trace_id,
                observed_at_us: *observed_at_us,
                backend: *backend,
                window: window.as_ref().map(Cow::Borrowed),
                application: *application,
                timing: *timing,
                event: WireEvent::TerminalSnapshot {
                    node_id: *node_id,
                    snapshot: Box::new(layout.snapshot(snapshot)),
                },
            },
            OutpostToCore::Reply {
                trace_id,
                request_id,
                outcome: QueryOutcome::Done(QueryResult::Text(reply)),
                timing,
            } => {
                let reply = match reply {
                    TextReply::TerminalSnapshot(snapshot) => {
                        WireReply::TerminalSnapshot(Box::new(layout.snapshot(snapshot)))
                    }
                    TextReply::TerminalHistory { text, above, read } => {
                        WireReply::TerminalHistory {
                            text: layout.text(text),
                            above: *above,
                            read: *read,
                        }
                    }
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
                Wire::Reply {
                    trace_id: *trace_id,
                    request_id: *request_id,
                    outcome: WireOutcome::Done(WireResult::Text(reply)),
                    timing: *timing,
                }
            }
            _ => return None,
        };
        Some(Self { wire, layout })
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
    /// Appends the message's frame to `buffer`: the body, then the raw
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
    // A body that is not a terminal's message is one that may not have a
    // raw section.
    let wire: Wire<'static> =
        serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))?;
    let mut section = Section {
        bytes: raw,
        next: 0,
    };
    let message = match wire {
        Wire::Event {
            trace_id,
            observed_at_us,
            backend,
            window,
            application,
            timing,
            event: WireEvent::TerminalSnapshot { node_id, snapshot },
        } => OutpostToCore::Event {
            trace_id,
            observed_at_us,
            backend,
            window: window.map(Cow::into_owned),
            application,
            timing,
            event: NormalizedEvent::TerminalSnapshot {
                node_id,
                snapshot: Box::new(section.snapshot(*snapshot)?),
            },
        },
        Wire::Reply {
            trace_id,
            request_id,
            outcome: WireOutcome::Done(WireResult::Text(reply)),
            timing,
        } => {
            let reply = match reply {
                WireReply::TerminalSnapshot(snapshot) => {
                    TextReply::TerminalSnapshot(Box::new(section.snapshot(*snapshot)?))
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
            OutpostToCore::Reply {
                trace_id,
                request_id,
                outcome: QueryOutcome::Done(QueryResult::Text(reply)),
                timing,
            }
        }
    };
    section.finish()?;
    Ok(message)
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
mod tests {
    use crate::transport::{Parts, read_frame};

    use verbatim_model::{TextAnchor, TextChunk, TextUnit, WindowHandle};

    use super::super::{Message, decode_message, encode_message};
    use super::*;

    fn trace(number: u64) -> TraceId {
        serde_json::from_str(&number.to_string()).expect("a trace id")
    }

    fn text(text: &str) -> TerminalText {
        TerminalText::unpadded(text)
    }

    fn snapshot(
        body: &str,
        screen: &str,
        colours: Option<Vec<Vec<TerminalColourRun>>>,
    ) -> TerminalSnapshot {
        TerminalSnapshot {
            text: text(body),
            screen: text(screen),
            from_start: true,
            shrank: false,
            at_document_start: false,
            alternate: true,
            first_row: text("first  "),
            caret_rows_to_end: 2,
            caret_line: TerminalText::default(),
            caret_column: 4,
            above: 30,
            started_ms: 5,
            ended_ms: 6,
            waited: true,
            read: 9,
            colours,
            caret: None,
        }
    }

    fn run(column: u32, length: u32, foreground: u32, background: u32) -> TerminalColourRun {
        TerminalColourRun {
            column,
            length,
            foreground,
            background,
        }
    }

    fn event(snapshot: TerminalSnapshot) -> OutpostToCore {
        OutpostToCore::Event {
            trace_id: trace(7),
            observed_at_us: 1_752_000_000_000_000,
            backend: Backend::Uia,
            window: None,
            application: Some(Pid(20)),
            timing: EventTiming::default(),
            event: NormalizedEvent::TerminalSnapshot {
                node_id: NodeId::new(3),
                snapshot: Box::new(snapshot),
            },
        }
    }

    fn window() -> WindowFacts {
        WindowFacts {
            top_level: WindowHandle(0x10),
            root_owner: WindowHandle(0x20),
            topmost: false,
            under_active_window: None,
            in_foreground: true,
            class: "ConsoleWindowClass".to_owned(),
            application: "pwsh".to_owned(),
            alt_held: false,
        }
    }

    fn caret() -> CaretReport {
        CaretReport {
            line: TextChunk {
                unit: TextUnit::Line,
                text: "$ ls -l\r\n".to_owned(),
                start: TextAnchor(1),
                offset: 4,
                languages: Vec::new(),
                first: false,
                last: true,
                truncated: false,
                formats: Vec::new(),
            },
            selection: None,
            selection_line: None,
        }
    }

    fn reply(reply: TextReply) -> OutpostToCore {
        OutpostToCore::Reply {
            trace_id: trace(8),
            request_id: 4,
            outcome: QueryOutcome::Done(QueryResult::Text(reply)),
            timing: EventTiming::default(),
        }
    }

    /// The body and raw section of `message`'s frame.
    fn parts_of(message: &OutpostToCore) -> (Vec<u8>, Option<Vec<u8>>) {
        let frame = encode_message(message).expect("encoded");
        let frame = read_frame(&mut frame.as_slice())
            .expect("read")
            .expect("a frame");
        let Parts { body, raw } = frame.parts();
        (body.to_vec(), raw.map(<[u8]>::to_vec))
    }

    #[test]
    fn a_terminals_reads_and_replies_round_trip_through_the_raw_section() {
        let latin = "$ ls -l   \r\ntotal 0   \r\n".repeat(3);
        let cyrillic = "привет мир  \r\nэто тест\r\n".repeat(3);
        let colours = vec![
            vec![
                run(0, 4, 0x00FF_FFFF, 0),
                run(4, 116, 0x0000_FF00, 0x0012_3456),
            ],
            Vec::new(),
            vec![run(0, 1, 1, 2), run(1, 2, 3, 4), run(3, 117, u32::MAX, 5)],
        ];
        let messages = [
            // Latin text, with colours in rows of differing run counts.
            event(snapshot(&latin, "screen   ", Some(colours.clone()))),
            // Cyrillic text, no colours.
            event(snapshot(&cyrillic, &cyrillic, None)),
            // Empty texts, and colours taken but with no rows.
            event(snapshot("", "", Some(Vec::new()))),
            // The event's window and the read's caret, both given.
            OutpostToCore::Event {
                trace_id: trace(7),
                observed_at_us: 1_752_000_000_000_000,
                backend: Backend::Uia,
                window: Some(window()),
                application: Some(Pid(20)),
                timing: EventTiming::default(),
                event: NormalizedEvent::TerminalSnapshot {
                    node_id: NodeId::new(3),
                    snapshot: Box::new(TerminalSnapshot {
                        caret: Some(caret()),
                        ..snapshot(&latin, "", None)
                    }),
                },
            },
            reply(TextReply::TerminalSnapshot(Box::new(snapshot(
                &cyrillic,
                "",
                Some(colours),
            )))),
            reply(TextReply::TerminalHistory {
                text: cyrillic.clone(),
                above: 2,
                read: 1,
            }),
            reply(TextReply::TerminalHistory {
                text: String::new(),
                above: 0,
                read: 0,
            }),
            reply(TextReply::TerminalFound {
                matches: vec![
                    TerminalMatch {
                        above: -1,
                        text: latin,
                    },
                    TerminalMatch {
                        above: 3,
                        text: String::new(),
                    },
                    TerminalMatch {
                        above: 4,
                        text: cyrillic,
                    },
                ],
                read: 2,
            }),
            reply(TextReply::TerminalFound {
                matches: Vec::new(),
                read: 2,
            }),
        ];
        for message in &messages {
            let (body, raw) = parts_of(message);
            let raw = raw.expect("a terminal's message has a raw section");
            let read: OutpostToCore = decode_message(Parts {
                body: &body,
                raw: Some(&raw),
            })
            .expect("decoded");
            assert_eq!(&read, message);
        }
    }

    /// The frame's exact form: the body names each text's range and the
    /// colours' range and counts; the raw section holds the texts, then
    /// the runs' records.
    #[test]
    fn a_terminals_text_and_colours_are_the_raw_section_and_the_body_names_them() {
        let message = reply(TextReply::TerminalSnapshot(Box::new(TerminalSnapshot {
            text: text("ab"),
            screen: text("пр"),
            first_row: TerminalText::default(),
            colours: Some(vec![vec![run(0, 2, 7, 0)], Vec::new()]),
            ..snapshot("", "", None)
        })));
        let (body, raw) = parts_of(&message);
        let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
        assert_eq!(
            body["Reply"]["outcome"],
            serde_json::json!({"Done": {"Text": {"TerminalSnapshot": {
                "text": {"raw": [0, 2], "lengths": [[1, 2]]},
                "screen": {"raw": [2, 4], "lengths": [[1, 2]]},
                "from_start": true,
                "shrank": false,
                "at_document_start": false,
                "alternate": true,
                "first_row": {"raw": [6, 0], "lengths": []},
                "caret_rows_to_end": 2,
                "caret_line": {"raw": [6, 0], "lengths": []},
                "caret_column": 4,
                "above": 30,
                "started_ms": 5,
                "ended_ms": 6,
                "waited": true,
                "read": 9,
                "colours": {"raw": [6, 16], "rows": [1, 0]},
            }}}})
        );
        let mut expected = "abпр".as_bytes().to_vec();
        for value in [0u32, 2, 7, 0] {
            expected.extend(value.to_le_bytes());
        }
        assert_eq!(raw, Some(expected));
    }

    /// A message with no terminal text is framed exactly as its JSON alone
    /// always was, with no `Raw-Length` line: a pong, an event of another
    /// kind, and replies of other kinds, a terminal's among them.
    #[test]
    fn a_message_without_a_terminals_text_is_framed_as_its_json_alone() {
        let messages = [
            OutpostToCore::Pong {
                seq: 1,
                parked_count: 0,
                written_at_us: 2,
            },
            OutpostToCore::Event {
                trace_id: trace(7),
                observed_at_us: 0,
                backend: Backend::Uia,
                window: None,
                application: None,
                timing: EventTiming::default(),
                event: NormalizedEvent::TextChanged {
                    node_id: NodeId::new(1),
                },
            },
            reply(TextReply::Done),
            reply(TextReply::TerminalUnchanged),
        ];
        for message in &messages {
            let json = serde_json::to_vec(message).expect("JSON");
            let mut expected = format!("Content-Length: {:010}\r\n\r\n", json.len()).into_bytes();
            expected.extend(json);
            assert_eq!(encode_message(message).expect("encoded"), expected);
        }
        // Such a message read with a raw section is refused: it never has
        // one.
        let json = serde_json::to_vec(&messages[0]).expect("JSON");
        let error = OutpostToCore::from_parts(Parts {
            body: &json,
            raw: Some(b""),
        })
        .expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    /// A terminal history whose text is at `range`, read from a frame with
    /// `raw` as its raw section.
    fn history(range: &str, raw: &[u8]) -> io::Result<OutpostToCore> {
        let timing = serde_json::to_string(&EventTiming::default()).expect("JSON");
        let body = format!(
            r#"{{"Reply":{{"trace_id":8,"request_id":4,"outcome":{{"Done":{{"Text":{{"TerminalHistory":{{"text":{range},"above":0,"read":0}}}}}}}},"timing":{timing}}}}}"#
        );
        OutpostToCore::from_parts(Parts {
            body: body.as_bytes(),
            raw: Some(raw),
        })
    }

    #[test]
    fn a_range_the_raw_section_does_not_hold_is_a_protocol_error() {
        assert_eq!(
            history("[0,3]", b"abc").expect("the well-formed frame reads"),
            reply(TextReply::TerminalHistory {
                text: "abc".to_owned(),
                above: 0,
                read: 0,
            })
        );
        for range in ["[0,4]", "[2,2]", "[18446744073709551615,2]"] {
            let error = history(range, b"abc").expect_err("refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{range}");
        }
    }

    /// Ranges that overlap or leave a gap are a protocol error, though
    /// their lengths add up to the raw section's: the ranges must tile it,
    /// each starting where the last ended.
    #[test]
    fn ranges_that_do_not_tile_the_raw_section_are_a_protocol_error() {
        let timing = serde_json::to_string(&EventTiming::default()).expect("JSON");
        let found = |first: &str, second: &str| {
            let body = format!(
                r#"{{"Reply":{{"trace_id":8,"request_id":4,"outcome":{{"Done":{{"Text":{{"TerminalFound":{{"matches":[{{"above":0,"text":{first}}},{{"above":1,"text":{second}}}],"read":0}}}}}}}},"timing":{timing}}}}}"#
            );
            OutpostToCore::from_parts(Parts {
                body: body.as_bytes(),
                raw: Some(b"abcdef"),
            })
        };
        let matches = |first: &str, second: &str| {
            reply(TextReply::TerminalFound {
                matches: vec![
                    TerminalMatch {
                        above: 0,
                        text: first.to_owned(),
                    },
                    TerminalMatch {
                        above: 1,
                        text: second.to_owned(),
                    },
                ],
                read: 0,
            })
        };
        assert_eq!(
            found("[0,3]", "[3,3]").expect("the well-formed frame reads"),
            matches("abc", "def")
        );
        // Overlapping, a gap then a repeat, and out of order.
        for (first, second) in [("[0,3]", "[0,3]"), ("[3,3]", "[3,3]"), ("[3,3]", "[0,3]")] {
            let error = found(first, second).expect_err("refused");
            assert_eq!(
                error.kind(),
                io::ErrorKind::InvalidData,
                "{first} then {second}"
            );
        }
    }

    #[test]
    fn text_that_is_not_utf8_is_a_protocol_error() {
        let error = history("[0,2]", &[0xC3, 0x28]).expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_raw_section_longer_or_shorter_than_its_body_names_is_a_protocol_error() {
        // Longer: bytes the body names no place for.
        let error = history("[0,2]", b"abc").expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // Shorter: the last colour record cut off.
        let (body, raw) = parts_of(&event(snapshot(
            "ab",
            "",
            Some(vec![vec![run(0, 1, 2, 3)], vec![run(1, 1, 2, 3)]]),
        )));
        let raw = raw.expect("a raw section");
        let error = OutpostToCore::from_parts(Parts {
            body: &body,
            raw: Some(&raw[..raw.len() - RUN_BYTES]),
        })
        .expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // Row counts that name more runs than the colours' range holds.
        let body = String::from_utf8(body)
            .expect("UTF-8")
            .replace(r#""rows":[1,1]"#, r#""rows":[1,2]"#);
        let error = OutpostToCore::from_parts(Parts {
            body: body.as_bytes(),
            raw: Some(&raw),
        })
        .expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
