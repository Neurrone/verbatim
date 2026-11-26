use crate::transport::{Parts, read_frame};

use verbatim_model::{TextChunk, TextUnit, WindowHandle};

use super::super::{Message, decode_message, encode_message};
use super::*;

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
    }
}

/// The answer to request 4 with `reply`.
fn answer(reply: TextReply) -> OutpostToCore {
    OutpostToCore::Answer(Answer {
        number: RequestNumber(4),
        outcome: Outcome::Result(Reply::Text(reply)),
        probes: Vec::new(),
        detail: String::new(),
        began_us: 7,
        ended_us: 9,
    })
}

/// The answer to a terminal read of `snapshot`.
fn read(snapshot: TerminalSnapshot) -> OutpostToCore {
    answer(TextReply::TerminalRead(Box::new(TerminalReadReply {
        snapshot,
        start: TextAnchor(11),
        started_us: 1_752_000_000_000_000,
        point: Some(TextAnchor(12)),
    })))
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
fn a_terminals_answers_round_trip_through_the_raw_section() {
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
        read(snapshot(&latin, "screen   ", Some(colours.clone()))),
        // Cyrillic text, no colours.
        read(snapshot(&cyrillic, &cyrillic, None)),
        // Empty texts, and colours taken but with no rows.
        read(snapshot("", "", Some(Vec::new()))),
        // The read's caret given, no point minted, and the answer's probes
        // and detail.
        OutpostToCore::Answer(Answer {
            number: RequestNumber(5),
            outcome: Outcome::Result(Reply::Text(TextReply::TerminalRead(Box::new(
                TerminalReadReply {
                    snapshot: TerminalSnapshot {
                        caret: Some(caret()),
                        ..snapshot(&latin, "", None)
                    },
                    start: TextAnchor(2),
                    started_us: 3,
                    point: None,
                },
            )))),
            probes: vec![Probe {
                window: WindowHandle(0x10),
                thread: 7,
                provider: true,
                began_us: 8,
            }],
            detail: "slow".to_owned(),
            began_us: 20,
            ended_us: 21,
        }),
        read(snapshot(&cyrillic, "", Some(colours))),
        answer(TextReply::TerminalHistory {
            text: cyrillic.clone(),
            above: 2,
        }),
        answer(TextReply::TerminalHistory {
            text: String::new(),
            above: 0,
        }),
        answer(TextReply::TerminalFound {
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
        }),
        answer(TextReply::TerminalFound {
            matches: Vec::new(),
        }),
    ];
    for message in &messages {
        let (body, raw) = parts_of(message);
        let raw = raw.expect("a terminal's answer has a raw section");
        let read: OutpostToCore = decode_message(Parts {
            body: &body,
            raw: Some(&raw),
        })
        .expect("decoded");
        assert_eq!(&read, message);
    }
}

/// The frame's exact form: a terminal read's text is not in the JSON body,
/// which names each text's range and the colours' range and counts; the
/// raw section holds the texts, then the runs' records.
#[test]
fn a_terminal_reads_text_and_colours_are_the_raw_section_and_the_body_names_them() {
    let message = read(TerminalSnapshot {
        text: text("ab"),
        screen: text("пр"),
        first_row: TerminalText::default(),
        colours: Some(vec![vec![run(0, 2, 7, 0)], Vec::new()]),
        ..snapshot("", "", None)
    });
    let (body, raw) = parts_of(&message);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        body,
        serde_json::json!({"Answer": {
            "number": 4,
            "outcome": {"Result": {"Text": {"TerminalRead": {
                "snapshot": {
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
                    "colours": {"raw": [6, 16], "rows": [1, 0]},
                },
                "start": 11,
                "started_us": 1_752_000_000_000_000_u64,
                "point": 12,
            }}}},
            "began_us": 7,
            "ended_us": 9,
        }})
    );
    let mut expected = "abпр".as_bytes().to_vec();
    for value in [0u32, 2, 7, 0] {
        expected.extend(value.to_le_bytes());
    }
    assert_eq!(raw, Some(expected));
}

/// A history's and a search's texts are the raw section too, the body
/// naming their ranges.
#[test]
fn a_history_and_a_searchs_texts_are_the_raw_section() {
    let (body, raw) = parts_of(&answer(TextReply::TerminalHistory {
        text: "abc".to_owned(),
        above: 1,
    }));
    assert_eq!(
        String::from_utf8(body).expect("UTF-8"),
        r#"{"Answer":{"number":4,"outcome":{"Result":{"Text":{"TerminalHistory":{"text":[0,3],"above":1}}}},"began_us":7,"ended_us":9}}"#
    );
    assert_eq!(raw, Some(b"abc".to_vec()));
    let (body, raw) = parts_of(&answer(TextReply::TerminalFound {
        matches: vec![
            TerminalMatch {
                above: 0,
                text: "de".to_owned(),
            },
            TerminalMatch {
                above: -1,
                text: "f".to_owned(),
            },
        ],
    }));
    assert_eq!(
        String::from_utf8(body).expect("UTF-8"),
        r#"{"Answer":{"number":4,"outcome":{"Result":{"Text":{"TerminalFound":{"matches":[{"above":0,"text":[0,2]},{"above":-1,"text":[2,1]}]}}}},"began_us":7,"ended_us":9}}"#
    );
    assert_eq!(raw, Some(b"def".to_vec()));
}

/// A message with no terminal text is framed exactly as its JSON alone
/// always was, with no `Raw-Length` line: a pong, and answers of other
/// kinds, a terminal's refusal among them.
#[test]
fn a_message_without_a_terminals_text_is_framed_as_its_json_alone() {
    let messages = [
        OutpostToCore::Pong {
            parked_count: 0,
            written_at_us: 2,
        },
        answer(TextReply::NoText),
        OutpostToCore::Answer(Answer {
            number: RequestNumber(4),
            outcome: Outcome::NotThere,
            probes: Vec::new(),
            detail: String::new(),
            began_us: 0,
            ended_us: 0,
        }),
    ];
    for message in &messages {
        let json = serde_json::to_vec(message).expect("JSON");
        let mut expected = format!("Content-Length: {:010}\r\n\r\n", json.len()).into_bytes();
        expected.extend(json);
        assert_eq!(encode_message(message).expect("encoded"), expected);
    }
    // Such a message read with a raw section is refused: it never has one.
    for message in &messages {
        let json = serde_json::to_vec(message).expect("JSON");
        let error = OutpostToCore::from_parts(Parts {
            body: &json,
            raw: Some(b""),
        })
        .expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

/// A terminal history whose text is at `range`, read from a frame with
/// `raw` as its raw section.
fn history(range: &str, raw: &[u8]) -> io::Result<OutpostToCore> {
    let body = format!(
        r#"{{"Answer":{{"number":4,"outcome":{{"Result":{{"Text":{{"TerminalHistory":{{"text":{range},"above":0}}}}}}}},"began_us":7,"ended_us":9}}}}"#
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
        answer(TextReply::TerminalHistory {
            text: "abc".to_owned(),
            above: 0,
        })
    );
    for range in ["[0,4]", "[2,2]", "[18446744073709551615,2]"] {
        let error = history(range, b"abc").expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{range}");
    }
}

/// Ranges that overlap or leave a gap are a protocol error, though their
/// lengths add up to the raw section's: the ranges must tile it, each
/// starting where the last ended.
#[test]
fn ranges_that_do_not_tile_the_raw_section_are_a_protocol_error() {
    let found = |first: &str, second: &str| {
        let body = format!(
            r#"{{"Answer":{{"number":4,"outcome":{{"Result":{{"Text":{{"TerminalFound":{{"matches":[{{"above":0,"text":{first}}},{{"above":1,"text":{second}}}]}}}}}}}},"began_us":7,"ended_us":9}}}}"#
        );
        OutpostToCore::from_parts(Parts {
            body: body.as_bytes(),
            raw: Some(b"abcdef"),
        })
    };
    let matches = |first: &str, second: &str| {
        answer(TextReply::TerminalFound {
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
    let (body, raw) = parts_of(&read(snapshot(
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

/// The answer to request 4 with a batch's `steps`.
fn steps(steps: Vec<StepOutcome>) -> OutpostToCore {
    OutpostToCore::Answer(Answer {
        number: RequestNumber(4),
        outcome: Outcome::Result(Reply::Steps(steps)),
        probes: Vec::new(),
        detail: String::new(),
        began_us: 7,
        ended_us: 9,
    })
}

fn moved() -> StepOutcome {
    StepOutcome::Done(Done::Move(verbatim_model::batch::Moved {
        moved: -3,
        from: TextAnchor(1),
        to: TextAnchor(2),
    }))
}

fn trimmed(text: &str) -> StepOutcome {
    StepOutcome::Done(Done::ReadRange(RangeText {
        text: text.to_owned(),
        truncated: false,
        padded: Some(vec![(2, 12)]),
    }))
}

fn visible(text: &str) -> StepOutcome {
    StepOutcome::Done(Done::Visible(VisibleText {
        start: TextAnchor(5),
        end: TextAnchor(6),
        at_text_start: true,
        text: text.to_owned(),
        padded: vec![(1, 80)],
    }))
}

fn found(texts: &[&str]) -> StepOutcome {
    StepOutcome::Done(Done::FindText(
        texts
            .iter()
            .zip(3..)
            .map(|(text, to_end)| FoundText {
                to_end,
                text: (*text).to_owned(),
            })
            .collect(),
    ))
}

/// A batch's answer whose steps answered a trimmed range, the visible
/// text, matches or colours comes back equal through the raw section,
/// its other steps, skipped and failed ones among them, with it.
#[test]
fn a_batchs_answers_round_trip_through_the_raw_section() {
    let cyrillic = "привет мир\r\nэто тест\r\n".repeat(3);
    let colours = vec![
        vec![run(0, 4, 0x00FF_FFFF, 0), run(4, 116, 0x0000_FF00, 5)],
        Vec::new(),
    ];
    let messages = [
        // Every kind the raw section takes, between steps it does not.
        steps(vec![
            moved(),
            visible("screen"),
            StepOutcome::Skipped,
            trimmed(&cyrillic),
            StepOutcome::Failed,
            found(&["$ ls", "", &cyrillic]),
            StepOutcome::Done(Done::ReadColours(colours)),
            StepOutcome::NotThere(verbatim_model::batch::Missing::Anchor(TextAnchor(3))),
        ]),
        // One of each alone, with nothing in it.
        steps(vec![trimmed("")]),
        steps(vec![visible("")]),
        steps(vec![found(&[])]),
        steps(vec![StepOutcome::Done(Done::ReadColours(Vec::new()))]),
    ];
    for message in &messages {
        let (body, raw) = parts_of(message);
        let raw = raw.expect("such an answer has a raw section");
        let read: OutpostToCore = decode_message(Parts {
            body: &body,
            raw: Some(&raw),
        })
        .expect("decoded");
        assert_eq!(&read, message);
    }
}

/// The frame's exact form for a batch's answer: the texts and the colours
/// are not in the body, which names their ranges in the steps' order, and
/// a step with nothing in the raw section is the model's JSON.
#[test]
fn a_batchs_texts_and_colours_are_the_raw_section_and_the_body_names_them() {
    let message = steps(vec![
        visible("ab"),
        moved(),
        trimmed("пр"),
        found(&["c", "de"]),
        StepOutcome::Done(Done::ReadColours(vec![vec![run(0, 2, 7, 0)], Vec::new()])),
    ]);
    let (body, raw) = parts_of(&message);
    let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        body,
        serde_json::json!({"Answer": {
            "number": 4,
            "outcome": {"Result": {"Steps": [
                {"Visible": {
                    "start": 5,
                    "end": 6,
                    "at_text_start": true,
                    "text": [0, 2],
                    "padded": [[1, 80]],
                }},
                {"Body": {"Done": {"Move": {"moved": -3, "from": 1, "to": 2}}}},
                {"ReadRange": {"text": [2, 4], "truncated": false, "padded": [[2, 12]]}},
                {"FindText": [
                    {"to_end": 3, "text": [6, 1]},
                    {"to_end": 4, "text": [7, 2]},
                ]},
                {"ReadColours": {"raw": [9, 16], "rows": [1, 0]}},
            ]}},
            "began_us": 7,
            "ended_us": 9,
        }})
    );
    let mut expected = "abпрcde".as_bytes().to_vec();
    for value in [0u32, 2, 7, 0] {
        expected.extend(value.to_le_bytes());
    }
    assert_eq!(raw, Some(expected));
}

/// A batch's answer with no trimmed range, visible text, match list or
/// colours is its JSON alone, an untrimmed range's text in it; and read
/// with a raw section it is refused, as is one whose body holds a text the
/// raw section takes.
#[test]
fn a_batchs_answer_with_nothing_for_the_raw_section_is_its_json_alone() {
    let untrimmed = StepOutcome::Done(Done::ReadRange(RangeText {
        text: "abc".to_owned(),
        truncated: false,
        padded: None,
    }));
    let message = steps(vec![moved(), untrimmed, StepOutcome::Skipped]);
    let json = serde_json::to_vec(&message).expect("JSON");
    let mut expected = format!("Content-Length: {:010}\r\n\r\n", json.len()).into_bytes();
    expected.extend(&json);
    assert_eq!(encode_message(&message).expect("encoded"), expected);
    let refused = |body: &str| {
        OutpostToCore::from_parts(Parts {
            body: body.as_bytes(),
            raw: Some(b""),
        })
        .expect_err("refused")
        .kind()
    };
    // Its steps in a raw frame's form, none of them taking the section.
    assert_eq!(
        refused(
            r#"{"Answer":{"number":4,"outcome":{"Result":{"Steps":[{"Body":"Skipped"}]}},"began_us":7,"ended_us":9}}"#
        ),
        io::ErrorKind::InvalidData
    );
    // The visible text in the body, where the raw section takes it.
    assert_eq!(
        refused(
            r#"{"Answer":{"number":4,"outcome":{"Result":{"Steps":[{"Body":{"Done":{"Visible":{"start":5,"end":6,"at_text_start":true,"text":"","padded":[]}}}}]}},"began_us":7,"ended_us":9}}"#
        ),
        io::ErrorKind::InvalidData
    );
    // The same step in its raw form reads.
    assert_eq!(
        OutpostToCore::from_parts(Parts {
            body: br#"{"Answer":{"number":4,"outcome":{"Result":{"Steps":[{"Visible":{"start":5,"end":6,"at_text_start":true,"text":[0,0],"padded":[[1,80]]}}]}},"began_us":7,"ended_us":9}}"#,
            raw: Some(b""),
        })
        .expect("read"),
        steps(vec![visible("")])
    );
}

/// A batch's ranges must tile the raw section as a terminal's do.
#[test]
fn a_batchs_ranges_that_do_not_tile_the_raw_section_are_a_protocol_error() {
    let read = |first: &str, second: &str, raw: &[u8]| {
        let body = format!(
            r#"{{"Answer":{{"number":4,"outcome":{{"Result":{{"Steps":[{{"ReadRange":{{"text":{first},"truncated":false,"padded":[[2,12]]}}}},{{"FindText":[{{"to_end":3,"text":{second}}}]}}]}}}},"began_us":7,"ended_us":9}}}}"#
        );
        OutpostToCore::from_parts(Parts {
            body: body.as_bytes(),
            raw: Some(raw),
        })
    };
    assert_eq!(
        read("[0,3]", "[3,3]", b"abcdef").expect("the well-formed frame reads"),
        steps(vec![trimmed("abc"), found(&["def"])])
    );
    // Overlapping, out of order, past the end, short of the end, and not
    // UTF-8.
    for (first, second, raw) in [
        ("[0,3]", "[0,3]", &b"abcdef"[..]),
        ("[3,3]", "[0,3]", b"abcdef"),
        ("[0,3]", "[3,4]", b"abcdef"),
        ("[0,3]", "[3,2]", b"abcdef"),
        ("[0,3]", "[3,2]", &[b'a', b'b', b'c', 0xC3, 0x28]),
    ] {
        let error = read(first, second, raw).expect_err("refused");
        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidData,
            "{first} then {second}"
        );
    }
}
