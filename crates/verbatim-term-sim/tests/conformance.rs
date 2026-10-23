//! The simulated terminal checked against the real ones: every stream of
//! the corpus replayed and read as the real console host and Windows
//! Terminal were, their answers compared field by field with the
//! expectations `cargo xtask regenerate-terminal-conformance-expectations`
//! recorded (`docs/design/terminal-model.md`, section 9).

use verbatim_term_sim::Flavor;
use verbatim_term_sim::conformance::{Expectation, expectation_path, replay, streams};

#[test]
fn the_simulated_terminal_answers_as_the_real_ones() {
    let mut failures = Vec::new();
    for (name, stream) in streams() {
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
            let differences = replay(&stream, &expected);
            if !differences.is_empty() {
                failures.push(format!("{name} in {flavor:?}:\n{}", differences.join("\n")));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
