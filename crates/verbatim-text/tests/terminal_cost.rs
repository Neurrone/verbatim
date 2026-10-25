//! The cost of the terminal memory and reading rules
//! (`verbatim_text::terminal_rows` and `verbatim_text::terminal_reading`)
//! for the reads that cost them most, in the simulated terminal: a
//! measurement, not a test (`docs/performance.md`, "A terminal's rotation").
//! It asserts nothing about the times, and is ignored so that no test run
//! includes it; run it in a release build:
//!
//! `cargo test --release -p verbatim-text --test terminal_cost -- --ignored --nocapture`
//!
//! Each case fills a 120 by 30 terminal of each kind with a full history of
//! distinct lines, reads it as the outpost does (the first read, then its
//! history block, then one ordinary read), writes the case's output, and
//! times each read the rules then take until they ask for no other, the
//! whole document included when they ask for it. Only the reader's own
//! time is counted, not the simulated terminal's: the reads are taken once
//! and given again to a fresh reader for each run.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use verbatim_term_sim::conformance::history;
use verbatim_term_sim::{Flavor, Query, Snapshot, Terminal};
use verbatim_text::terminal_reading::{Settings, Step, TerminalReader};
use verbatim_text::terminal_rows::Read;

/// The screen's width and height.
const WIDTH: usize = 120;
const HEIGHT: usize = 30;

/// How many times each case is run.
const RUNS: usize = 31;

/// How the lines that fill the history read.
#[derive(Clone, Copy)]
enum Style {
    /// A line's number alone, as `1..9100 | % { $_ }` prints.
    Numbered,
    /// A line's number then text to one cell short of the screen's width,
    /// so each row holds 119 characters to split and hash.
    Wide,
    /// A line's number then Cyrillic text to one cell short of the
    /// screen's width: two bytes a character, split by their cells and
    /// hashed. (The simulated terminal gives every character one cell, so
    /// wide characters are not among the cases.)
    Cyrillic,
    /// A line's number then text to one cell short of two rows, so each
    /// line wraps onto a second row.
    Wrapped,
}

impl Style {
    fn line(self, number: usize) -> String {
        let mut line = format!("{number} ");
        match self {
            Self::Numbered => line = number.to_string(),
            Self::Wide | Self::Wrapped => {
                let length = if matches!(self, Self::Wide) {
                    WIDTH - 1
                } else {
                    2 * WIDTH - 1
                };
                while line.len() < length {
                    line.push_str("lorem ipsum ");
                }
                line.truncate(length);
            }
            Self::Cyrillic => {
                let cells = line.len();
                line.extend("привет мир ".chars().cycle().take(WIDTH - 1 - cells));
            }
        }
        line.push_str("\r\n");
        line
    }
}

/// What the history holds.
#[derive(Clone, Copy)]
enum Fill {
    /// Distinct lines, numbered.
    Distinct,
    /// One line, repeated; the output repeats it too.
    OneLine,
}

/// What a case writes after the history is full and read.
#[derive(Clone, Copy)]
enum Output {
    /// This many new lines, numbered on from the history's.
    Lines(usize),
    /// A clear of the screen and the history, then this many lines of other
    /// text.
    ClearAndReprint(usize),
    /// This many lines, each one of the last screen's texts, in an order
    /// that repeats no stretch of them.
    Shuffled(usize),
    /// This many lines, each one of the last screen's texts drawn at
    /// random, then 60 new lines: the read before the whole document keeps
    /// nothing, and in the whole document the last screen's rows are found
    /// at one place and their texts at thousands, the most rows for the
    /// subsequence to compare.
    Echoed(usize),
}

struct Case {
    name: &'static str,
    style: Style,
    fill: Fill,
    output: Output,
}

impl Case {
    /// The text of line `number` of the history or the output.
    fn text(&self, number: usize) -> String {
        match self.fill {
            Fill::Distinct => self.style.line(number),
            Fill::OneLine => self.style.line(0),
        }
    }
}

const CASES: &[Case] = &[
    Case {
        name: "one new line",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Lines(1),
    },
    Case {
        name: "a rotation of 58 rows",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Lines(58),
    },
    Case {
        name: "a rotation of 61 rows",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Lines(61),
    },
    Case {
        name: "a rotation of 1,000 rows",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Lines(1000),
    },
    Case {
        name: "a rotation of 1,000 rows, wide lines",
        style: Style::Wide,
        fill: Fill::Distinct,
        output: Output::Lines(1000),
    },
    Case {
        name: "a rotation of 1,000 rows, Cyrillic lines",
        style: Style::Cyrillic,
        fill: Fill::Distinct,
        output: Output::Lines(1000),
    },
    Case {
        name: "an overflow keeping nothing, lines of two rows",
        style: Style::Wrapped,
        fill: Fill::Distinct,
        output: Output::Lines(5000),
    },
    Case {
        name: "an overflow keeping nothing",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Lines(10_000),
    },
    Case {
        name: "an overflow keeping nothing, wide lines",
        style: Style::Wide,
        fill: Fill::Distinct,
        output: Output::Lines(10_000),
    },
    Case {
        name: "an overflow keeping nothing, Cyrillic lines",
        style: Style::Cyrillic,
        fill: Fill::Distinct,
        output: Output::Lines(10_000),
    },
    Case {
        name: "a clear and a reprint of 8,905 rows",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::ClearAndReprint(8905),
    },
    Case {
        name: "1,000 lines of a history of one repeated line",
        style: Style::Numbered,
        fill: Fill::OneLine,
        output: Output::Lines(1000),
    },
    Case {
        name: "1,000 lines shuffled from the last screen's",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Shuffled(1000),
    },
    Case {
        name: "8,000 lines drawn from the last screen's, then 60 new",
        style: Style::Numbered,
        fill: Fill::Distinct,
        output: Output::Echoed(8000),
    },
];

/// The reads of one case, taken once from the simulated terminal: those
/// before its output, then those after.
struct Reads {
    before: Vec<Read>,
    after: Vec<Read>,
    /// The UTF-8 length of each read after the output.
    sizes: Vec<usize>,
}

fn read_of(snapshot: &Snapshot, at_ms: u64) -> Read {
    Read {
        text: snapshot.text.clone(),
        screen: snapshot.screen.clone(),
        from_start: snapshot.from_start,
        shrank: snapshot.shrank,
        at_document_start: snapshot.at_document_start,
        alternate: snapshot.alternate,
        first_row: snapshot.first_row.clone(),
        caret_rows_to_end: snapshot.caret_rows_to_end,
        caret_line: snapshot.caret_line.clone(),
        caret_column: snapshot.caret_column,
        at_ms,
        above: usize::try_from(snapshot.above).unwrap_or(usize::MAX),
    }
}

/// Reads `terminal` as the outpost does until the reader asks for no other
/// read, returning the reads; `query` is where the read starts, and is left
/// where the next starts.
fn settle(
    terminal: &Terminal,
    reader: &mut TerminalReader,
    query: &mut Query,
    at_ms: &mut u64,
) -> Vec<Read> {
    let mut reads = Vec::new();
    loop {
        *at_ms += 100;
        let snapshot = terminal
            .snapshot(query)
            .expect("the simulated terminal answers every read");
        let read = read_of(&snapshot, *at_ms);
        let step = reader.read(&read, false, None);
        reads.push(read);
        let height = step.outcome.as_ref().map_or(0, |outcome| outcome.height);
        let screen_rows = u32::try_from(height).unwrap_or(u32::MAX);
        if step.whole_document || step.history_block {
            *query = Query {
                start: None,
                screen_rows,
                whole: step.whole_document,
                above: if step.history_block { screen_rows } else { 0 },
            };
            continue;
        }
        *query = Query {
            start: Some(snapshot.next_start),
            screen_rows,
            whole: false,
            above: u32::try_from(reader.memory().above()).unwrap_or(u32::MAX),
        };
        return reads;
    }
}

fn take_reads(case: &Case, flavor: Flavor) -> Reads {
    let mut terminal = Terminal::new(flavor, WIDTH, HEIGHT, history(flavor));
    let mut reader = TerminalReader::new(Settings::default());
    let mut query = Query::default();
    let mut at_ms = 0;
    let filled = history(flavor) + HEIGHT + 100;
    let text = |number: usize| case.text(number);
    let mut fill = String::new();
    for number in 1..=filled {
        fill.push_str(&text(number));
    }
    terminal.write(fill.as_bytes());
    let mut before = settle(&terminal, &mut reader, &mut query, &mut at_ms);
    terminal.write(text(filled + 1).as_bytes());
    before.extend(settle(&terminal, &mut reader, &mut query, &mut at_ms));
    let mut output = String::new();
    match case.output {
        Output::Lines(count) => {
            for number in filled + 2..filled + 2 + count {
                output.push_str(&text(number));
            }
        }
        Output::ClearAndReprint(count) => {
            output.push_str("\x1b[H\x1b[2J\x1b[3J");
            for number in 1..=count {
                let _ = write!(output, "reprinted {number}\r\n");
            }
        }
        Output::Shuffled(count) => {
            // The last screen's lines but its caret's, which is blank: a
            // walk through them by a step coprime with their count.
            let screen: Vec<usize> = (filled + 1 - (HEIGHT - 2)..=filled + 1).collect();
            for at in 0..count {
                output.push_str(&text(screen[(at * 7) % screen.len()]));
            }
        }
        Output::Echoed(count) => {
            let screen: Vec<usize> = (filled + 1 - (HEIGHT - 2)..=filled + 1).collect();
            // A linear congruential generator, so every run draws alike.
            let mut state: u64 = 1;
            for _ in 0..count {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let at = usize::try_from(state >> 33).unwrap_or(0) % screen.len();
                output.push_str(&text(screen[at]));
            }
            for number in filled + 2..filled + 62 {
                output.push_str(&text(number));
            }
        }
    }
    terminal.write(output.as_bytes());
    let after = settle(&terminal, &mut reader, &mut query, &mut at_ms);
    let sizes = after.iter().map(|read| read.text.len()).collect();
    Reads {
        before,
        after,
        sizes,
    }
}

/// One run of a case: the time of each read after its output, the steps
/// they gave, and a fresh reader that took the reads before it.
fn run(reads: &Reads) -> (Vec<Duration>, Vec<Step>) {
    let mut reader = TerminalReader::new(Settings::default());
    for read in &reads.before {
        reader.read(read, false, None);
    }
    let mut times = Vec::new();
    let mut steps = Vec::new();
    for read in &reads.after {
        let started = Instant::now();
        let step = reader.read(read, false, None);
        times.push(started.elapsed());
        steps.push(step);
    }
    (times, steps)
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

#[test]
#[ignore = "a measurement, not a test: run in a release build with --ignored --nocapture (docs/performance.md, \"A terminal's rotation\")"]
fn the_reading_rules_cost_of_the_costliest_reads() {
    let mut report = String::new();
    for case in CASES {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let reads = take_reads(case, flavor);
            let (_, expected) = run(&reads);
            let mut totals = Vec::new();
            let mut drops = Vec::new();
            let mut parts: Vec<Vec<Duration>> = vec![Vec::new(); reads.after.len()];
            for _ in 0..RUNS {
                let (times, steps) = run(&reads);
                assert_eq!(steps, expected, "every run reads alike");
                // What the caller frees once it has taken the steps: their
                // lines and speech.
                let started = Instant::now();
                drop(steps);
                drops.push(started.elapsed());
                totals.push(times.iter().sum());
                for (part, time) in parts.iter_mut().zip(times) {
                    part.push(time);
                }
            }
            let lines = expected
                .last()
                .map_or(0, |step| step.speech.len() + step.withheld.len());
            let parts: Vec<String> = parts
                .iter_mut()
                .zip(&reads.sizes)
                .map(|(part, size)| format!("{size} bytes {}", spread(part)))
                .collect();
            let _ = writeln!(
                report,
                "{} ({flavor:?}): {} reads, least, median and greatest {} microseconds ({}); its steps freed in {}; {lines} lines spoken or withheld",
                case.name,
                reads.after.len(),
                spread(&mut totals),
                parts.join("; "),
                spread(&mut drops),
            );
        }
    }
    eprint!("{report}");
}
