//! The reducer's cost of a terminal's reads once Core reads terminals
//! (`docs/design/terminal-model.md`, "Where it runs"), in the simulated
//! terminal: a measurement, not a test (`docs/performance.md`, "A
//! terminal's reads in Core"). It asserts nothing about the times, and is
//! ignored so that no test run includes it; run it in a release build:
//!
//! `cargo test --release -p verbatim-core --test terminal_cost -- --ignored --nocapture`
//!
//! Each case fills a 120 by 30 terminal of each kind with a full history of
//! distinct numbered lines, read as the outpost reads for Core
//! (`terminal_support`), writes the case's output, and lets Core take the
//! reads that follow, the whole document included when its rules ask for
//! it. Each input that carries a read (a live snapshot, or the answer to
//! Core's request for one) is then timed through `reduce` itself, from a
//! clone of the state before the case's output, its frame's JSON parse
//! timed apart, and its size given as encoded: the snapshot without its
//! padding, and, for comparison, the same read with its texts padded as
//! the plan first had them. A 2,000-line console-host flood read in 1,054
//! reads, as measured live on 2026-10-08, gives the bytes a flood moves.

mod terminal_support;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use terminal_support::Session;
use verbatim_core::reduce;
use verbatim_model::{Input, NormalizedEvent, TerminalSnapshot, TerminalText, TextReply};
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
    /// read after it reads the whole document.
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

/// The read an input carries, if any.
fn read_of(input: &Input) -> Option<&TerminalSnapshot> {
    match input {
        Input::Event {
            event: NormalizedEvent::TerminalSnapshot { snapshot, .. },
            ..
        }
        | Input::TextCompleted {
            reply: TextReply::TerminalSnapshot(snapshot),
            ..
        } => Some(snapshot),
        _ => None,
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

/// The JSON of `snapshot` as its input carries it: the event, or the
/// reply.
fn encoded(input: &Input, snapshot: TerminalSnapshot) -> String {
    match input {
        Input::Event { event, .. } => {
            let NormalizedEvent::TerminalSnapshot { node_id, .. } = event else {
                unreachable!("an input that carries a read");
            };
            serde_json::to_string(&NormalizedEvent::TerminalSnapshot {
                node_id: *node_id,
                snapshot: Box::new(snapshot),
            })
        }
        _ => serde_json::to_string(&TextReply::TerminalSnapshot(Box::new(snapshot))),
    }
    .expect("a read serializes")
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

/// Times `input`, a read, from `state`: its frame's parse, the padded
/// form's parse, the read rebuilt from it, and the reducer's step.
fn time(state: &mut verbatim_core::SrState, input: &Input, times: &mut Times) {
    let snapshot = read_of(input).expect("a read");
    let frame = serde_json::to_string(input).expect("an input serializes");
    let padded_frame = encoded(input, padded(snapshot));
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
    let effects = reduce(state, input);
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
        .filter_map(|(at, input)| read_of(input).map(|_| at))
        .collect();
    let mut times: Vec<Times> = reads.iter().map(|_| Times::default()).collect();
    for _ in 0..RUNS {
        let mut state = before.clone();
        for (at, input) in inputs.iter().enumerate() {
            match reads.iter().position(|read| *read == at) {
                Some(read) => time(&mut state, input, &mut times[read]),
                None => {
                    let _ = reduce(&mut state, input);
                }
            }
        }
    }
    let parts: Vec<String> = reads
        .iter()
        .zip(&mut times)
        .map(|(&at, times)| {
            let snapshot = read_of(&inputs[at]).expect("a read");
            let shrunk = encoded(&inputs[at], snapshot.clone());
            let full = encoded(&inputs[at], padded(snapshot));
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
    session.log.clear();
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
    for (input, _) in &session.log {
        if let Some(snapshot) = read_of(input) {
            shrunk += encoded(input, snapshot.clone()).len();
            full += encoded(input, padded(snapshot)).len();
            count += 1;
        }
    }
    format!(
        "a 2,000-line console-host flood in {reads} live reads: {count} reads in all, {shrunk} bytes ({full} padded)"
    )
}

#[test]
#[ignore = "a measurement, not a test: run in a release build with --ignored --nocapture (docs/performance.md, \"A terminal's reads in Core\")"]
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
