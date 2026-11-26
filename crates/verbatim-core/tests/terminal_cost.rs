//! The reducer's cost of a terminal's reads once Core reads terminals
//! (`books/dev/src/explanation/design/terminal-model.md`, "Where it runs"), in the simulated
//! terminal: a measurement, not a test (`books/dev/src/reference/performance.md`, "A
//! terminal's reads in Core"). It asserts nothing about the times, and is
//! ignored so that no test run includes it; run it in a release build:
//!
//! `cargo test --release -p verbatim-core --test terminal_cost -- --ignored --nocapture`
//!
//! Each case fills a 120 by 30 terminal of each kind with a full history of
//! distinct numbered lines, read as the outpost reads for Core
//! (`terminal_support`), writes the case's output, and lets Core take the
//! reads that follow, a count read included when its rules ask for one.
//! Each input that carries a read (the answer to the window read of
//! Core's terminal reader, a batch) is then timed through `reduce` itself,
//! from a clone of the state before the case's output, its JSON's parse
//! timed apart, and its size given as the JSON of the read the reader
//! reads back: the snapshot without its padding, and, for comparison, the
//! same read with its texts padded as the plan first had them. A
//! 2,000-line console-host flood read in 1,054 reads, as measured live on
//! 2026-10-08, gives the size of a flood's reads by the same measure, the
//! JSON of each read the reader reads back, not the bytes of the step
//! answers that cross the pipe.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "a measurement, timed by the monotonic clock: listed under the determinism rule's exceptions"
)]

#[path = "support/readers.rs"]
mod readers;
mod terminal_support;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use terminal_support::Session;
use verbatim_core::reduce;
use verbatim_model::batch::{Done, StepOutcome};
use verbatim_model::request::{Answer, Outcome, QueueInput, Reply};
use verbatim_model::{Input, TerminalSnapshot, TerminalText};
use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::history;

const WIDTH: usize = 120;
const HEIGHT: usize = 30;

/// How many times each case is run.
const RUNS: usize = 31;

/// What a case writes after the history is full and read.
enum Output {
    /// This many new lines.
    Lines(usize),
    /// A clear of the screen and the history, then this many lines.
    ClearAndReprint(usize),
    /// A full-screen program opened, drawn and read, then closed: the
    /// read after it starts at the alternate screen's top, and so reads the
    /// whole document.
    FullScreenExit,
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
        name: "a full-screen program's exit",
        output: Output::FullScreenExit,
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

/// Whether `input` carries a full read: a batch's answer with the screen's
/// text, which only the terminal reader's window read takes.
fn carries_read(input: &Input) -> bool {
    match input {
        Input::Queue {
            input:
                QueueInput::Answer {
                    answer:
                        Answer {
                            outcome: Outcome::Result(Reply::Steps(steps)),
                            ..
                        },
                    ..
                },
            ..
        } => steps
            .iter()
            .any(|step| matches!(step, StepOutcome::Done(Done::Visible(_)))),
        _ => false,
    }
}

/// `snapshot` with its texts padded, as the plan first sent them.
fn padded(snapshot: &TerminalSnapshot) -> TerminalSnapshot {
    let whole = |text: &TerminalText| TerminalText {
        text: text.padded(),
        lengths: Vec::new(),
    };
    TerminalSnapshot {
        text: whole(&snapshot.text),
        screen: whole(&snapshot.screen),
        first_row: whole(&snapshot.first_row),
        caret_line: whole(&snapshot.caret_line),
        ..snapshot.clone()
    }
}

/// The JSON of `snapshot`.
fn encoded(snapshot: &TerminalSnapshot) -> String {
    serde_json::to_string(snapshot).expect("a read serializes")
}

/// Lines `range`, each `prefix` and its number, ended.
fn numbered(prefix: &str, range: std::ops::RangeInclusive<usize>) -> String {
    range.fold(String::new(), |mut text, line| {
        let _ = write!(text, "{prefix}{line}\r\n");
        text
    })
}

/// A session of `flavor` whose history is full of distinct numbered lines.
fn filled(flavor: Flavor) -> (Session, usize) {
    let mut session = Session::new(flavor, WIDTH, HEIGHT, history(flavor));
    let lines = history(flavor) + HEIGHT + 100;
    session.write(&numbered("", 1..=lines), 100);
    session.write(&numbered("", lines + 1..=lines + 1), 200);
    (session, lines + 1)
}

fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

/// The least, median and greatest of `times`, in microseconds.
fn spread(times: &mut [Duration]) -> String {
    times.sort();
    format!(
        "{:.0}, {:.0}, {:.0}",
        micros(times[0]),
        micros(times[times.len() / 2]),
        micros(times[times.len() - 1]),
    )
}

/// The times of one read input, each run's.
#[derive(Default)]
struct Times {
    reducer: Vec<Duration>,
    rebuild: Vec<Duration>,
    parse: Vec<Duration>,
    parse_padded: Vec<Duration>,
}

/// Times `input`, which carries the read `snapshot`, from `state`: its
/// JSON's parse, the padded read's parse, the rules' read rebuilt from the
/// read, and the reducer's step.
fn time(
    state: &mut verbatim_core::SrState,
    (input, snapshot): (&Input, &TerminalSnapshot),
    times: &mut Times,
) {
    let frame = serde_json::to_string(input).expect("an input serializes");
    let padded_frame = encoded(&padded(snapshot));
    let started = Instant::now();
    let parsed: Input = serde_json::from_str(&frame).expect("an input parses");
    times.parse.push(started.elapsed());
    drop(parsed);
    let started = Instant::now();
    let parsed: serde_json::Value =
        serde_json::from_str(&padded_frame).expect("a padded read parses");
    times.parse_padded.push(started.elapsed());
    drop(parsed);
    let started = Instant::now();
    let again = verbatim_core::read_of(snapshot);
    times.rebuild.push(started.elapsed());
    drop(again);
    let started = Instant::now();
    let effects = reduce(state, &mut verbatim_core::Runtime::default(), input);
    times.reducer.push(started.elapsed());
    drop(effects);
}

/// One case in one terminal, a line of the report.
fn measure(case: &Case, flavor: Flavor) -> String {
    let (mut session, last) = filled(flavor);
    if matches!(case.output, Output::FullScreenExit) {
        let screen = (1..=HEIGHT).fold(String::new(), |mut screen, row| {
            let _ = write!(screen, "\x1b[{row};1Hpage row {row}");
            screen
        });
        session.write(&format!("\x1b[?1049h\x1b[2J\x1b[H{screen}"), 300);
    }
    let before = session.state.clone();
    session.log.clear();
    let read_before = session.answered_reads.len();
    let output = match case.output {
        Output::Lines(count) => numbered("", last + 1..=last + count),
        Output::ClearAndReprint(count) => {
            format!("\x1b[H\x1b[2J\x1b[3J{}", numbered("reprinted ", 1..=count))
        }
        Output::FullScreenExit => "\x1b[?1049l".to_owned(),
    };
    session.write(&output, 1000);
    let inputs: Vec<Input> = session.log.drain(..).map(|(input, _)| input).collect();
    let reads: Vec<usize> = inputs
        .iter()
        .enumerate()
        .filter_map(|(at, input)| carries_read(input).then_some(at))
        .collect();
    // The reads the reader read back of those answers, in their order.
    let snapshots = &session.answered_reads[read_before..];
    assert_eq!(snapshots.len(), reads.len(), "a read for each answer");
    let mut times: Vec<Times> = reads.iter().map(|_| Times::default()).collect();
    for _ in 0..RUNS {
        let mut state = before.clone();
        for (at, input) in inputs.iter().enumerate() {
            match reads.iter().position(|read| *read == at) {
                Some(read) => time(&mut state, (input, &snapshots[read]), &mut times[read]),
                None => {
                    let _ = reduce(&mut state, &mut verbatim_core::Runtime::default(), input);
                }
            }
        }
    }
    let parts: Vec<String> = snapshots
        .iter()
        .zip(&mut times)
        .map(|(snapshot, times)| {
            let shrunk = encoded(snapshot);
            let full = encoded(&padded(snapshot));
            let screen = serde_json::to_string(&snapshot.screen)
                .expect("a text serializes")
                .len();
            format!(
                "{} bytes ({} padded, screen {}); reducer {} (of which the read rebuilt {}); parse {} (padded {})",
                shrunk.len(),
                full.len(),
                screen,
                spread(&mut times.reducer),
                spread(&mut times.rebuild),
                spread(&mut times.parse),
                spread(&mut times.parse_padded),
            )
        })
        .collect();
    format!(
        "{} ({flavor:?}): {} reads, least, median and greatest microseconds: {}",
        case.name,
        reads.len(),
        parts.join(" | "),
    )
}

/// A 2,000-line console-host flood read in 1,054 reads, as the console host
/// was read live: chunks of two lines and of one, two first as far as they
/// go; a line of the report.
fn flood() -> String {
    let mut session = Session::new(
        Flavor::ConsoleHost,
        WIDTH,
        HEIGHT,
        history(Flavor::ConsoleHost),
    );
    session.write("PS> .\\flood.ps1\r\n", 100);
    let read_before = session.answered_reads.len();
    let (reads, lines) = (1054, 2000);
    let doubles = lines - reads;
    let mut line = 0;
    for read in 0..reads {
        let count = if read < doubles { 2 } else { 1 };
        session.write(
            &numbered("flood line ", line + 1..=line + count),
            1000 + u64::try_from(read).expect("a count"),
        );
        line += count;
    }
    let (mut shrunk, mut full, mut count) = (0, 0, 0);
    for snapshot in &session.answered_reads[read_before..] {
        shrunk += encoded(snapshot).len();
        full += encoded(&padded(snapshot)).len();
        count += 1;
    }
    format!(
        "a 2,000-line console-host flood in {reads} live reads: {count} reads in all, {shrunk} bytes ({full} padded)"
    )
}

#[test]
#[ignore = "a measurement, not a test: run in a release build with --ignored --nocapture (books/dev/src/reference/performance.md, \"A terminal's reads in Core\")"]
fn the_reducers_cost_of_a_terminals_reads() {
    let mut report = String::new();
    for case in CASES {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let _ = writeln!(report, "{}", measure(case, flavor));
        }
    }
    let _ = writeln!(report, "{}", flood());
    eprint!("{report}");
}
