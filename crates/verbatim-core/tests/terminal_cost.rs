//! The reducer's cost of a terminal's window reads
//! (`books/dev/src/explanation/design/terminal-model.md`, "Where it runs"),
//! in the simulated terminal: a measurement, not a test
//! (`books/dev/src/reference/performance.md`, "A terminal's reads in
//! Core"). It asserts nothing about the times, and is ignored so that no
//! test run includes it; run it in a release build:
//!
//! `cargo test --release -p verbatim-core --test terminal_cost -- --ignored --nocapture`
//!
//! Each case fills a 120 by 30 terminal of each kind with a full history of
//! distinct numbered lines, focuses it through the public reducer as the
//! listener and the outpost report a terminal's focus (`desk`), answers the
//! watch's baseline read with the window read of the full terminal, writes
//! the case's output, and answers the window read Core then asks for. That
//! answer's step is timed through `reduce` itself, from a clone of the
//! state before it, its frame's JSON parse timed apart, and its size given
//! as encoded. A window read is the document's last two screens' rows,
//! made here from the simulated terminal's whole document as the outpost's
//! read takes them, the end mark's rows written counted from the
//! simulator's rows; the provider reports no caret.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a measurement, timed by the monotonic clock: listed under the determinism rule's exceptions"
)]

mod desk;
#[path = "support/readers.rs"]
mod readers;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use desk::{Desk, ONE, focus_read_of};
use verbatim_core::reduce;
use verbatim_model::batch::{Done, Op, StepOutcome};
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::PropertyValues;
use verbatim_model::request::{Answer, Outcome, Question, QueueInput, Reply, RequestNumber};
use verbatim_model::tell::{Event, EventObject, Payload, TellWindow};
use verbatim_model::{
    Backend, Input, MarkPlace, MarkSearch, NodeDetails, NodeSnapshot, Role, StateSet, TerminalText,
    TerminalWindow, TextAnchor, TraceId, WindowAsk,
};
use verbatim_term_sim::conformance::history;
use verbatim_term_sim::{Flavor, Query, Terminal};

const WIDTH: usize = 120;
const HEIGHT: usize = 30;

/// How many times each case is run.
const RUNS: usize = 31;

/// The focused terminal's node.
const TERMINAL: u64 = 31;

/// What a case writes after the history is full and read.
enum Output {
    /// This many new lines.
    Lines(usize),
    /// A clear of the screen and the history, then this many lines.
    ClearAndReprint(usize),
    /// A full-screen program's screen drawn over the main one.
    FullScreen,
}

struct Case {
    name: &'static str,
    output: Output,
}

const CASES: &[Case] = &[
    Case {
        name: "an ordinary read, one new line",
        output: Output::Lines(1),
    },
    Case {
        name: "a rotation of 61 rows",
        output: Output::Lines(61),
    },
    Case {
        name: "a lost read keeping nothing, 10,000 lines",
        output: Output::Lines(10_000),
    },
    Case {
        name: "a full-screen program's screen",
        output: Output::FullScreen,
    },
    Case {
        name: "a clear and a reprint of 8,905 rows",
        output: Output::ClearAndReprint(8905),
    },
    Case {
        name: "clear; dir, 100 lines",
        output: Output::ClearAndReprint(100),
    },
];

/// The simulated terminal, with what the window reads have counted so far.
struct Simulated {
    terminal: Terminal,
    /// The rows the document held at the last read, and how many lines
    /// had been written by then.
    last: Option<(usize, usize)>,
    /// How many lines have been written.
    written: usize,
    /// The next end mark's anchor.
    next_anchor: u64,
}

impl Simulated {
    fn new(flavor: Flavor) -> Self {
        Self {
            terminal: Terminal::new(flavor, WIDTH, HEIGHT, history(flavor)),
            last: None,
            written: 0,
            next_anchor: 1_000,
        }
    }

    /// Writes `count` numbered lines, numbered on from those written.
    fn lines(&mut self, count: usize) {
        let mut bytes = String::new();
        for _ in 0..count {
            self.written += 1;
            let _ = write!(bytes, "{}\r\n", self.written);
        }
        self.terminal.write(bytes.as_bytes());
    }

    /// The window read `ask` names, as the outpost answers it.
    fn read(&mut self, ask: &WindowAsk) -> TerminalWindow {
        let whole = self
            .terminal
            .snapshot(&Query {
                whole: true,
                ..Query::default()
            })
            .expect("the simulated terminal's whole text");
        let rows: Vec<&str> = whole
            .text
            .strip_suffix("\r\n")
            .unwrap_or(&whole.text)
            .split("\r\n")
            .collect();
        let document_rows = rows.len();
        let height = if ask.height == 0 {
            HEIGHT
        } else {
            usize::try_from(ask.height).unwrap_or(HEIGHT)
        };
        let above = (2 * height).min(document_rows);
        let window = rows[document_rows - above..].join("\r\n");
        let screen_top = HEIGHT.min(document_rows);
        // The rows written past the current mark: those the document grew
        // by, and those the history rotated out, counted by the lines
        // written; a clear refuses the mark.
        let end_place = ask.end.as_ref().map(|_| match self.last {
            Some((rows_then, written_then))
                if document_rows >= rows_then || self.written > written_then =>
            {
                MarkPlace::Rows {
                    written: u32::try_from(
                        (self.written - written_then).max(document_rows.saturating_sub(rows_then)),
                    )
                    .unwrap_or(u32::MAX),
                    opening: None,
                }
            }
            _ => MarkPlace::Refused,
        });
        self.last = Some((document_rows, self.written));
        self.next_anchor += 1;
        let count = |rows: usize| u32::try_from(rows).unwrap_or(u32::MAX);
        TerminalWindow {
            rows: TerminalText::unpadded(&window),
            above: count(above),
            screen_top: count(screen_top),
            screen_rows: count(HEIGHT),
            width: count(WIDTH),
            screen_at_start: whole.alternate || document_rows <= HEIGHT,
            caret: None,
            caret_rows_to_end: None,
            end: TextAnchor(self.next_anchor),
            first_row: TerminalText::unpadded(&whole.first_row),
            document_rows: count(document_rows),
            end_place,
            aside_place: ask.aside.as_ref().map(|_| MarkPlace::Refused),
            found: match &ask.end {
                Some(end) if end.find => MarkSearch::Missed,
                _ => MarkSearch::NotAsked,
            },
            colours: ask.colours.then(|| vec![Vec::new(); HEIGHT]),
            began_us: 0,
            ended_us: 0,
        }
    }
}

/// The window read Core asked for last, by its request's number.
fn window_ask(desk: &Desk) -> Option<(u64, WindowAsk)> {
    desk.sent
        .iter()
        .rev()
        .find_map(|(number, _, question)| match question {
            Question::Batch(batch) => batch.steps.iter().find_map(|step| match &step.op {
                Op::TerminalRead { ask, .. } => Some((*number, (**ask).clone())),
                _ => None,
            }),
            Question::Terminal(_) => None,
        })
}

/// The outpost's tell that the focused terminal changed, at `at_us`.
fn changed(at_us: u64) -> Event {
    Event {
        trace_id: TraceId::from_raw(8),
        kind: EventKind::TerminalChanged,
        process: ONE.pid,
        application: "app".to_owned(),
        thread: 41,
        at_us,
        object: EventObject::Terminal {
            node: ONE.node(TERMINAL),
            window: ONE.window(),
        },
        cached: PropertyValues::default(),
        payload: Payload::None,
        window: TellWindow::NoWindow,
        checked: None,
    }
}

/// The input answering request `number` with `window`.
fn answer_of(number: u64, window: TerminalWindow) -> QueueInput {
    QueueInput::Answer {
        from: ONE.instance(),
        answer: Answer {
            number: RequestNumber(number),
            outcome: Outcome::Result(Reply::Steps(vec![StepOutcome::Done(Done::TerminalRead(
                Box::new(window),
            ))])),
            probes: Vec::new(),
            detail: String::new(),
        },
    }
}

/// What one case measured.
struct Measured {
    bytes: usize,
    reducer: Duration,
    parse: Duration,
}

/// Runs `case` in a terminal of `flavor`.
fn measure(flavor: Flavor, case: &Case) -> Measured {
    let mut simulated = Simulated::new(flavor);
    simulated.lines(history(flavor) + HEIGHT);
    let mut desk = Desk::new();
    let terminal = NodeSnapshot {
        id: ONE.node(TERMINAL),
        backend: Backend::Uia,
        role: Role::Terminal,
        name: Some("Terminal".to_owned()),
        value: None,
        states: StateSet::new(),
        details: NodeDetails::default(),
    };
    let _ = desk.focus_read(ONE, focus_read_of(ONE, &terminal, &[]));
    let (number, ask) = window_ask(&desk)
        .unwrap_or_else(|| panic!("Core asks for the terminal's baseline: {:?}", desk.asked));
    let baseline = simulated.read(&ask);
    let _ = desk.step(answer_of(number, baseline));
    match case.output {
        Output::Lines(count) => simulated.lines(count),
        Output::ClearAndReprint(count) => {
            simulated.terminal.write(b"\x1b[2J\x1b[3J\x1b[H");
            simulated.lines(count);
        }
        Output::FullScreen => {
            let mut screen = String::from("\x1b[?1049h\x1b[H");
            for row in 0..HEIGHT {
                let _ = write!(screen, "program row {row}\r\n");
            }
            simulated.terminal.write(screen.as_bytes());
        }
    }
    let _ = desk.tell(ONE.instance(), changed(desk.at_us + 1_000));
    let (number, ask) = window_ask(&desk)
        .unwrap_or_else(|| panic!("Core asks for the output's read: {:?}", desk.sent));
    let input = Input::Queue {
        trace_id: TraceId::from_raw(9),
        at_us: desk.at_us + 1_000,
        input: answer_of(number, simulated.read(&ask)),
    };
    let frame = serde_json::to_string(&input).expect("an input encodes");
    let mut reducer = Vec::with_capacity(RUNS);
    let mut parse = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let parsed: Input = serde_json::from_str(&frame).expect("an input decodes");
        parse.push(started.elapsed());
        let mut state = desk.state.clone();
        let mut runtime = verbatim_core::Runtime::default();
        let started = Instant::now();
        let effects = reduce(&mut state, &mut runtime, &parsed);
        reducer.push(started.elapsed());
        drop(effects);
    }
    reducer.sort();
    parse.sort();
    Measured {
        bytes: frame.len(),
        reducer: reducer[RUNS / 2],
        parse: parse[RUNS / 2],
    }
}

#[test]
#[ignore = "a measurement, run in a release build with --ignored --nocapture"]
fn a_terminals_window_reads_cost_the_reducer() {
    let mut report = String::new();
    for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
        for case in CASES {
            let measured = measure(flavor, case);
            let _ = writeln!(
                report,
                "{flavor:?}, {}: {} bytes; reducer {} µs; parse {} µs",
                case.name,
                measured.bytes,
                measured.reducer.as_micros(),
                measured.parse.as_micros(),
            );
        }
    }
    println!("{report}");
}
