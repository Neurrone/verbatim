use crate::transport::{Parts, read_frame};

use verbatim_model::WindowHandle;

use super::super::{Message, decode_message, encode_message};
use super::*;

fn run(column: u32, length: u32, foreground: u32, background: u32) -> TerminalColourRun {
    TerminalColourRun {
        column,
        length,
        foreground,
        background,
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

/// A message with nothing for the raw section is framed exactly as its
/// JSON alone always was, with no `Raw-Length` line: a pong, and answers
/// that are not results.
#[test]
fn a_message_without_a_text_for_the_raw_section_is_framed_as_its_json_alone() {
    let messages = [
        OutpostToCore::Pong {
            parked_count: 0,
            written_at_us: 2,
        },
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

/// A batch's answer of one search with one match whose text is at
/// `range`, read from a frame with `raw` as its raw section.
fn one_match(range: &str, raw: &[u8]) -> io::Result<OutpostToCore> {
    let body = format!(
        r#"{{"Answer":{{"number":4,"outcome":{{"Result":{{"Steps":[{{"FindText":[{{"to_end":3,"text":{range}}}]}}]}}}},"began_us":7,"ended_us":9}}}}"#
    );
    OutpostToCore::from_parts(Parts {
        body: body.as_bytes(),
        raw: Some(raw),
    })
}

#[test]
fn a_range_the_raw_section_does_not_hold_is_a_protocol_error() {
    assert_eq!(
        one_match("[0,3]", b"abc").expect("the well-formed frame reads"),
        steps(vec![found(&["abc"])])
    );
    for range in ["[0,4]", "[2,2]", "[18446744073709551615,2]"] {
        let error = one_match(range, b"abc").expect_err("refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{range}");
    }
}

#[test]
fn a_raw_section_longer_or_shorter_than_its_body_names_is_a_protocol_error() {
    // Longer: bytes the body names no place for.
    let error = one_match("[0,2]", b"abc").expect_err("refused");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    // Shorter: the last colour record cut off.
    let (body, raw) = parts_of(&steps(vec![StepOutcome::Done(Done::ReadColours(vec![
        vec![run(0, 1, 2, 3)],
        vec![run(1, 1, 2, 3)],
    ]))]));
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
        // The answer's probes and detail.
        OutpostToCore::Answer(Answer {
            number: RequestNumber(5),
            outcome: Outcome::Result(Reply::Steps(vec![visible("screen")])),
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

/// Ranges that overlap or leave a gap are a protocol error, though their
/// lengths add up to the raw section's: the ranges must tile it, each
/// starting where the last ended.
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
    // Overlapping, a gap then a repeat, out of order, past the end, short
    // of the end, and not UTF-8.
    for (first, second, raw) in [
        ("[0,3]", "[0,3]", &b"abcdef"[..]),
        ("[3,3]", "[3,3]", b"abcdef"),
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
