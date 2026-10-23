//! The simulated terminal checked against the real ones: every stream of
//! the corpus replayed and read as the real console host and Windows
//! Terminal were, their answers compared field by field with the
//! expectations `cargo xtask regenerate-terminal-conformance-expectations`
//! recorded (`docs/design/terminal-model.md`, section 9).

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
