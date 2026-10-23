//! The conformance corpus and its expectations (`docs/design/terminal-model.md`,
//! section 9): streams written chunk by chunk into a terminal and read
//! after each chunk, and what the real console host and Windows Terminal
//! answered. `cargo xtask regenerate-terminal-conformance-expectations`
//! writes the expectations; [`replay`] checks the simulated terminal
//! against them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::{Flavor, Query, Run, Terminal, screen_rows};

/// A stream of the corpus: what a program writes, in chunks, the terminal
/// read after each.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Stream {
    /// What the stream exercises.
    pub about: String,
    /// The chunks, each written whole before the terminal is read.
    pub chunks: Vec<String>,
}

/// What one read of a real terminal answered after a chunk.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent answers of one read, as the read program gives them"
)]
pub struct Answer {
    /// The text from the read's start to the document's end.
    pub text: String,
    /// The screen's text.
    pub screen: String,
    /// Whether the read started at the start it was given.
    pub from_start: bool,
    /// Whether that start was refused.
    pub shrank: bool,
    /// Whether the read started at the document's start.
    pub at_document_start: bool,
    /// Whether the screen starts at the document's start.
    pub alternate: bool,
    /// The document's first row.
    pub first_row: String,
    /// How many rows the caret moved to reach the document's end.
    pub caret_rows_to_end: u32,
    /// The caret's row as the provider expands the caret to a line.
    pub caret_line: String,
    /// The caret's column in that row.
    pub caret_column: usize,
    /// Each screen row's stretches of one foreground and background.
    pub colours: Vec<Vec<Run>>,
}

/// What a real terminal answered to a whole stream.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Expectation {
    /// The terminal.
    pub flavor: Flavor,
    /// Its width in cells.
    pub width: usize,
    /// Its screen's height in rows.
    pub height: usize,
    /// The answer after each chunk.
    pub answers: Vec<Answer>,
}

/// The rows of history above the screen each terminal keeps by default,
/// as the conformance runs use them.
#[must_use]
pub fn history(flavor: Flavor) -> usize {
    match flavor {
        Flavor::ConsoleHost => 9001 - 30,
        Flavor::WindowsTerminal => 9001,
    }
}

/// The folder of this crate's conformance files.
#[must_use]
pub fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("conformance")
}

/// The file a stream's expectation for `flavor` is kept in.
#[must_use]
pub fn expectation_path(stream: &str, flavor: Flavor) -> PathBuf {
    let flavor = match flavor {
        Flavor::ConsoleHost => "console-host",
        Flavor::WindowsTerminal => "windows-terminal",
    };
    folder()
        .join("expected")
        .join(format!("{stream}.{flavor}.json"))
}

/// The corpus's streams, by name, in name order.
///
/// # Panics
///
/// When the streams folder or a stream cannot be read.
#[must_use]
pub fn streams() -> Vec<(String, Stream)> {
    let mut streams: Vec<(String, Stream)> = std::fs::read_dir(folder().join("streams"))
        .expect("the streams folder")
        .map(|entry| {
            let path = entry.expect("a stream file").path();
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a stream's name")
                .to_owned();
            let text = std::fs::read_to_string(&path).expect("a stream's file");
            let stream: Stream = serde_json::from_str(&text).expect("a stream's contents");
            (name, stream)
        })
        .collect();
    streams.sort_by(|left, right| left.0.cmp(&right.0));
    streams
}

/// The simulated terminal's answers to `stream`, read as the runner reads
/// a real terminal: after each chunk, from where the read before said to
/// start.
///
/// # Panics
///
/// When a simulated read is refused, which a stream of the corpus never
/// asks for.
#[must_use]
pub fn simulate(stream: &Stream, flavor: Flavor, width: usize, height: usize) -> Vec<Answer> {
    let mut terminal = Terminal::new(flavor, width, height, history(flavor));
    let mut query = Query::default();
    let mut answers = Vec::new();
    for chunk in &stream.chunks {
        terminal.write(chunk.as_bytes());
        let snapshot = terminal.snapshot(&query).expect("a simulated read");
        query = Query {
            start: Some(snapshot.next_start),
            screen_rows: screen_rows(&snapshot.screen),
            whole: false,
        };
        answers.push(Answer {
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
            colours: terminal.colours(),
        });
    }
    answers
}

/// Where two texts first differ: their lengths, and each text's lines
/// around the first difference, padding trimmed and line breaks shown.
fn text_difference(real: &str, simulated: &str) -> String {
    let at = real
        .chars()
        .zip(simulated.chars())
        .position(|(real, simulated)| real != simulated)
        .unwrap_or_else(|| real.chars().count().min(simulated.chars().count()));
    let around = |text: &str| -> String {
        let chars: Vec<char> = text.chars().collect();
        let from = at.saturating_sub(130);
        let to = (at + 130).min(chars.len());
        chars[from.min(to)..to]
            .iter()
            .collect::<String>()
            .split("\r\n")
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join(" | ")
    };
    format!(
        "lengths real {}, simulated {}; first difference at {at}; real [{}], simulated [{}]",
        real.chars().count(),
        simulated.chars().count(),
        around(real),
        around(simulated)
    )
}

/// Where the simulated terminal's answers to `stream` differ from what the
/// real terminal answered: one line per differing field of each read,
/// empty when they agree.
#[must_use]
pub fn replay(stream: &Stream, expected: &Expectation) -> Vec<String> {
    let got = simulate(stream, expected.flavor, expected.width, expected.height);
    let mut differences = Vec::new();
    for (index, (want, got)) in expected.answers.iter().zip(&got).enumerate() {
        let mut fields = String::new();
        macro_rules! field {
            ($name:ident) => {
                if want.$name != got.$name {
                    let _ = write!(
                        fields,
                        "\n  {}: real {:?}, simulated {:?}",
                        stringify!($name),
                        want.$name,
                        got.$name
                    );
                }
            };
        }
        macro_rules! text_field {
            ($name:ident) => {
                if want.$name != got.$name {
                    let _ = write!(
                        fields,
                        "\n  {}: {}",
                        stringify!($name),
                        text_difference(&want.$name, &got.$name)
                    );
                }
            };
        }
        text_field!(text);
        text_field!(screen);
        field!(from_start);
        field!(shrank);
        field!(at_document_start);
        field!(alternate);
        text_field!(first_row);
        field!(caret_rows_to_end);
        text_field!(caret_line);
        field!(caret_column);
        if want.colours != got.colours {
            let row = want
                .colours
                .iter()
                .zip(&got.colours)
                .position(|(want, got)| want != got);
            let _ = write!(
                fields,
                "\n  colours: real {} rows, simulated {}; first differing row {row:?}: real {:?}, simulated {:?}",
                want.colours.len(),
                got.colours.len(),
                row.and_then(|row| want.colours.get(row)),
                row.and_then(|row| got.colours.get(row)),
            );
        }
        if !fields.is_empty() {
            differences.push(format!("after chunk {index}:{fields}"));
        }
    }
    if expected.answers.len() != got.len() {
        differences.push(format!(
            "{} answers expected, {} simulated",
            expected.answers.len(),
            got.len()
        ));
    }
    differences
}
