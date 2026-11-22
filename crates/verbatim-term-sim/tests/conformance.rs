//! The simulated terminal checked against the real ones: every stream of
//! the corpus replayed and read as the real console host and Windows
//! Terminal were, their answers compared field by field with the
//! expectations `cargo xtask regenerate-terminal-conformance-expectations`
//! recorded (`books/dev/src/explanation/design/terminal-model.md`, "Testing").

#![expect(
    clippy::disallowed_methods,
    reason = "reads the conformance corpus: listed under the determinism rule's exceptions"
)]

use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::{
    Expectation, expectation_path, replay, replay_program_colours, streams,
};

#[test]
fn the_simulated_terminal_answers_as_the_real_ones() {
    let mut failures = Vec::new();
    let streams = streams();
    assert!(!streams.is_empty(), "the corpus has no streams");
    for (name, stream) in streams {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let path = expectation_path(&name, flavor);
            let text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
                panic!(
                    "no expectation at {}: run cargo xtask regenerate-terminal-conformance-expectations",
                    path.display()
                )
            });
            let expected: Expectation =
                serde_json::from_str(&text).expect("an expectation's contents");
            assert_eq!(
                expected.flavor,
                flavor,
                "{} is another terminal's",
                path.display()
            );
            let differences = replay(&stream, &expected);
            if !differences.is_empty() {
                failures.push(format!("{name} in {flavor:?}:\n{}", differences.join("\n")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The read program's way of reading colours, checking stretches as the
/// providers answer them and reading cell by cell only within a stretch of
/// several colours, gives the colours the real terminals showed cell by
/// cell, in every stream with more than one colour on a row.
#[test]
fn the_read_programs_colours_are_the_cells_colours_in_the_colour_streams() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for (name, stream) in streams() {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let text = std::fs::read_to_string(expectation_path(&name, flavor))
                .expect("the stream's expectation");
            let expected: Expectation =
                serde_json::from_str(&text).expect("an expectation's contents");
            let coloured = expected
                .answers
                .iter()
                .any(|answer| answer.colours.iter().any(|row| row.len() > 1));
            if !coloured {
                continue;
            }
            checked += 1;
            let differences = replay_program_colours(&stream, &expected);
            if !differences.is_empty() {
                failures.push(format!("{name} in {flavor:?}:\n{}", differences.join("\n")));
            }
        }
    }
    assert!(checked > 0, "the corpus has no stream of several colours");
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Where a read's screen meets the document's end, when no document row
/// lies below it: the screen's text up to the document's end, which the
/// read's text from its start to the document's end ends with, or which
/// ends with that text when the read started inside the screen. Only blank
/// rows of the screen may lie past the document's end, as before a screen
/// fills and after a clear.
fn screen_to_document_end<'a>(text: &str, screen: &'a str) -> Option<&'a str> {
    let blank_from = screen.trim_end_matches([' ', '\r', '\n']).len();
    (blank_from..=screen.len())
        .rev()
        .filter(|&end| screen.is_char_boundary(end))
        .map(|end| &screen[..end])
        .find(|within| text.ends_with(within) || within.ends_with(text))
}

/// The window read's assumption (`TerminalRead`, which reads the two
/// screens above the document's end): in every read of both terminals no
/// document row lies below the screen, so the screen's top is at most one
/// screen above the document's end and the window holds the whole screen.
#[test]
fn no_document_row_lies_below_a_reads_screen() {
    let mut failures = Vec::new();
    let mut reads = 0;
    for (name, _) in streams() {
        for flavor in [Flavor::ConsoleHost, Flavor::WindowsTerminal] {
            let text = std::fs::read_to_string(expectation_path(&name, flavor))
                .expect("the stream's expectation");
            let expected: Expectation =
                serde_json::from_str(&text).expect("an expectation's contents");
            for (index, answer) in expected.answers.iter().enumerate() {
                reads += 1;
                match screen_to_document_end(&answer.text, &answer.screen) {
                    None => failures.push(format!(
                        "{name} in {flavor:?}, read {index}: a document row lies below the screen"
                    )),
                    Some(within) => {
                        let rows = within
                            .strip_suffix("\r\n")
                            .unwrap_or(within)
                            .split("\r\n")
                            .count();
                        if rows > expected.height {
                            failures.push(format!(
                                "{name} in {flavor:?}, read {index}: the screen's top is {rows} rows above the document's end, past its height {}",
                                expected.height
                            ));
                        }
                    }
                }
            }
        }
    }
    assert!(reads > 0, "the corpus has no reads");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
