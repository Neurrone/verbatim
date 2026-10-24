//! A terminal's reading as the outpost does it, on the simulated terminal
//! (`verbatim-term-sim`) in place of UI Automation: what the reading rules
//! say becomes Core's [`TerminalSpeech`], a message too large is split, and
//! the review cursor's requests are answered from the memory, a place
//! moving with its row, rows above the memory read from the history.

use std::collections::HashSet;

use verbatim_model::{
    Skipped, TerminalItem, TerminalKey, TerminalKeyKind, TerminalSettings, TerminalSpeech,
    TerminalUnit, TextAnchor, TextMovement, TextOp, TextPoint, TextPosition, TextRead, TextReply,
    TextUnit,
};
use verbatim_term_sim::{Flavor, Query};
use verbatim_text::terminal_reading::{Counted, Speech, Step};
use verbatim_text::terminal_rows::{Checked, Read};

use super::review::{self, CaretAt, History};
use super::{Terminal, rules_key, speech_of, split};
use crate::text::TextError;

/// A simulated terminal read as the worker reads it: each read starts where
/// the last said, as many rows above as the memory asks, and a read the
/// rules ask for again is taken at once.
struct Watch {
    sim: verbatim_term_sim::Terminal,
    terminal: Terminal,
    query: Query,
    at_ms: u64,
}

impl Watch {
    fn new(width: usize, height: usize, history: usize) -> Self {
        let terminal = Terminal::new(TerminalSettings::default());
        let above = u32::try_from(terminal.reader.memory().above()).expect("a few rows");
        let mut watch = Self {
            sim: verbatim_term_sim::Terminal::new(Flavor::ConsoleHost, width, height, history),
            terminal,
            query: Query {
                above,
                ..Query::default()
            },
            at_ms: 0,
        };
        let _ = watch.read();
        watch
    }

    /// Writes `bytes` and reads.
    fn write(&mut self, bytes: &str) -> TerminalSpeech {
        self.sim.write(bytes.as_bytes());
        self.read()
    }

    /// Reads, as [`super::read`] does.
    fn read(&mut self) -> TerminalSpeech {
        self.at_ms += 100;
        let mut query = self.query;
        loop {
            let snapshot = self.sim.snapshot(&query).expect("a simulated read");
            let mut read = Read {
                text: snapshot.text,
                screen: snapshot.screen,
                from_start: snapshot.from_start,
                shrank: snapshot.shrank,
                at_document_start: snapshot.at_document_start,
                alternate: snapshot.alternate,
                first_row: snapshot.first_row,
                caret_rows_to_end: snapshot.caret_rows_to_end,
                caret_line: snapshot.caret_line,
                caret_column: snapshot.caret_column,
                at_ms: self.at_ms,
                above: usize::try_from(snapshot.above).expect("a few rows"),
                checked: None,
            };
            let mut step = self.terminal.reader.read(&read, false, None);
            while !step.check_rows.is_empty() {
                let runs: Vec<(u32, u32)> = step
                    .check_rows
                    .iter()
                    .map(|run| {
                        (
                            u32::try_from(run.above).expect("a small buffer"),
                            u32::try_from(run.rows).expect("a small buffer"),
                        )
                    })
                    .collect();
                let rows = self
                    .sim
                    .rows_above(query.start.expect("rows are read from a start"), &runs)
                    .expect("a simulated read");
                read.checked = Some(Checked {
                    runs: rows.runs,
                    first_row: rows.first_row,
                });
                step = self.terminal.reader.read(&read, false, None);
            }
            let screen_rows = step.outcome.as_ref().map_or(0, |outcome| {
                u32::try_from(outcome.height).expect("a small screen")
            });
            if step.whole_document {
                query = Query {
                    start: None,
                    screen_rows,
                    whole: true,
                    above: 0,
                };
                continue;
            }
            if let Some(above) = step.read_above {
                query.above = u32::try_from(above).expect("a small buffer");
                continue;
            }
            self.terminal.handled(&read, &step);
            self.query = Query {
                start: Some(snapshot.next_start),
                screen_rows,
                whole: false,
                above: u32::try_from(self.terminal.reader.memory().above())
                    .expect("a small screen"),
            };
            return speech_of(&step);
        }
    }

    /// Answers `op` from the memory, as the worker does after its read.
    fn answer(&mut self, op: &TextOp, minted: &mut u64) -> TextReply {
        let mut history = SimHistory {
            sim: &self.sim,
            query: self.query,
            reads: 0,
        };
        let caret = self.terminal.caret;
        let mut mint = || {
            *minted += 1;
            *minted
        };
        review::answer(
            &mut self.terminal,
            op,
            review::Context {
                history: &mut history,
                caret,
                mint: &mut mint,
                held: &HashSet::new(),
            },
        )
        .expect("the simulated history answers")
    }
}

/// The simulated terminal's history, read from where the next read starts.
struct SimHistory<'a> {
    sim: &'a verbatim_term_sim::Terminal,
    query: Query,
    reads: usize,
}

impl History for SimHistory<'_> {
    fn read(&mut self, above: u32, whole: bool) -> Result<(String, usize), TextError> {
        self.reads += 1;
        let query = Query {
            start: if whole { None } else { self.query.start },
            whole,
            above: if whole { 0 } else { above },
            ..self.query
        };
        let snapshot = self
            .sim
            .snapshot(&query)
            .map_err(|refused| TextError::Failed(format!("{refused:?}")))?;
        Ok((
            snapshot.text,
            usize::try_from(snapshot.above).expect("rows"),
        ))
    }
}

/// A read of the line at `at`, after `movement` lines.
fn line_at(at: TextPoint, movement: i32) -> TextOp {
    TextOp::Read(TextRead {
        at,
        movement: (movement != 0).then_some(TextMovement {
            unit: TextUnit::Line,
            count: movement,
        }),
        unit: TextUnit::Line,
    })
}

/// The text, offset and anchor of a read's answer, and how far it moved.
fn landed(reply: &TextReply) -> (String, u32, TextAnchor, i32) {
    match reply {
        TextReply::Read { moved, chunk } => (chunk.text.clone(), chunk.offset, chunk.start, *moved),
        other => panic!("a read's answer, not {other:?}"),
    }
}

#[test]
fn what_the_rules_say_is_cores_speech_in_order() {
    let step = Step {
        speech: vec![
            Speech::Caret {
                row: 2,
                unit: verbatim_text::terminal_reading::Unit::Word,
                text: "word".to_owned(),
            },
            Speech::Line {
                row: 3,
                text: "first".to_owned(),
            },
            Speech::Skipped {
                rows: vec![4, 5],
                count: Counted::MoreThan(9000),
            },
            Speech::Growth {
                row: 6,
                text: " grown".to_owned(),
            },
            Speech::Echo {
                row: 2,
                text: "x".to_owned(),
            },
        ],
        followed: true,
        ..Step::default()
    };
    assert_eq!(
        speech_of(&step),
        TerminalSpeech {
            items: vec![
                TerminalItem::Caret {
                    unit: TerminalUnit::Word,
                    text: "word".to_owned(),
                },
                TerminalItem::Output("first".to_owned()),
                TerminalItem::Skipped(Skipped::MoreThan(9000)),
                TerminalItem::Output("grown".to_owned()),
                TerminalItem::Echo("x".to_owned()),
            ],
            followed: true,
        }
    );
}

#[test]
fn speech_too_large_for_a_message_is_split_in_order() {
    let items: Vec<TerminalItem> = ["one", "two", "three"]
        .map(|line| TerminalItem::Output(line.repeat(10)))
        .to_vec();
    let speech = TerminalSpeech {
        items: items.clone(),
        followed: true,
    };
    // Each item costs its text and 16: the first two, 92, fit in 100.
    let parts = split(speech.clone(), 100);
    assert_eq!(
        parts,
        vec![
            TerminalSpeech {
                items: items[..2].to_vec(),
                followed: false,
            },
            TerminalSpeech {
                items: items[2..].to_vec(),
                followed: true,
            },
        ]
    );
    assert_eq!(split(speech.clone(), 10_000), vec![speech]);
}

#[test]
fn a_key_from_core_is_the_rules_key() {
    let key = rules_key(&TerminalKey {
        kind: TerminalKeyKind::Caret(TerminalUnit::Line),
        at_ms: 42,
    });
    assert_eq!(
        key,
        verbatim_text::terminal_reading::Key {
            kind: verbatim_text::terminal_reading::KeyKind::Caret(
                verbatim_text::terminal_reading::Unit::Line
            ),
            at_ms: 42,
        }
    );
}

#[test]
fn new_output_is_spoken_line_by_line() {
    let mut watch = Watch::new(20, 4, 100);
    let speech = watch.write("alpha\r\nbeta\r\n");
    assert_eq!(
        speech.items,
        vec![
            TerminalItem::Output("alpha".to_owned()),
            TerminalItem::Output("beta".to_owned()),
        ]
    );
}

#[test]
fn the_review_position_moves_with_its_row_as_output_scrolls_it() {
    let mut watch = Watch::new(20, 4, 100);
    let _ = watch.write("one\r\ntwo\r\nthree\r\n");
    let mut minted = 0;
    // Up from the caret's line, the blank row below the output, to "three"
    // and "two".
    let (text, _, anchor, moved) =
        landed(&watch.answer(&line_at(TextPoint::Caret, -2), &mut minted));
    assert_eq!((text.as_str(), moved), ("two", -2));
    // Output scrolls "two" up the screen and into the history above the
    // rows the memory keeps.
    let _ = watch.write("four\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\n");
    let at = TextPoint::At(TextPosition { anchor, offset: 1 });
    let (text, offset, _, moved) = landed(&watch.answer(&line_at(at, 0), &mut minted));
    assert_eq!((text.as_str(), offset, moved), ("two", 1, 0));
    // Down from it, one row at a time, through the history.
    let (text, offset, _, moved) = landed(&watch.answer(&line_at(at, 1), &mut minted));
    assert_eq!((text.as_str(), offset, moved), ("three", 0, 1));
}

#[test]
fn moving_above_the_memorys_rows_reads_the_history_up_to_its_start() {
    let mut watch = Watch::new(20, 3, 100);
    let lines = (1..=12)
        .map(|line| format!("line {line}\r\n"))
        .collect::<Vec<_>>()
        .concat();
    let _ = watch.write(&lines);
    let mut minted = 0;
    let (text, _, first, moved) = landed(&watch.answer(&line_at(TextPoint::Start, 0), &mut minted));
    assert_eq!((text.as_str(), moved), ("line 1", 0));
    let reply = watch.answer(&line_at(TextPoint::Start, 0), &mut minted);
    let TextReply::Read { chunk, .. } = &reply else {
        panic!("a read's answer, not {reply:?}");
    };
    assert!(chunk.first, "the document's first row is known to be first");
    // Up from the first row goes nowhere.
    let at = TextPoint::At(TextPosition::at(first));
    let (text, _, _, moved) = landed(&watch.answer(&line_at(at, -1), &mut minted));
    assert_eq!((text.as_str(), moved), ("line 1", 0));
    let (text, _, _, moved) = landed(&watch.answer(&line_at(at, 3), &mut minted));
    assert_eq!((text.as_str(), moved), ("line 4", 3));
}

#[test]
fn a_row_ends_at_its_text_and_a_range_joins_lines() {
    let mut watch = Watch::new(20, 4, 100);
    let _ = watch.write("ab\r\ncd\r\n");
    let mut minted = 0;
    let (text, _, ab, _) = landed(&watch.answer(&line_at(TextPoint::Caret, -2), &mut minted));
    // No padding: the row's text is all of it.
    assert_eq!(text, "ab");
    let (_, _, cd, _) = landed(&watch.answer(&line_at(TextPoint::Caret, -1), &mut minted));
    let range = watch.answer(
        &TextOp::ReadRange {
            start: TextPoint::At(TextPosition {
                anchor: ab,
                offset: 1,
            }),
            end: TextPoint::At(TextPosition {
                anchor: cd,
                offset: 1,
            }),
            through: None,
        },
        &mut minted,
    );
    assert_eq!(
        range,
        TextReply::Range {
            text: "b\r\nc".to_owned(),
            truncated: false,
        }
    );
}

#[test]
fn the_review_reads_where_the_caret_is() {
    let mut watch = Watch::new(20, 4, 100);
    let _ = watch.write("ready> abc");
    assert_eq!(watch.terminal.caret, Some(CaretAt { row: 0, column: 10 }));
    let mut minted = 0;
    let (text, offset, _, _) = landed(&watch.answer(&line_at(TextPoint::Caret, 0), &mut minted));
    assert_eq!((text.as_str(), offset), ("ready> abc", 10));
}
