//! The end-of-run summary the suite's test binary prints after every
//! scenario it ran (`docs/tooling/e2e.md`, "The end-of-run summary"): one
//! line per scenario, in the order they ran, with its name, its result and
//! its seconds, so people and tools read a run's outcome without parsing
//! timestamps.

use std::time::Duration;

/// The line before the summary's scenario lines.
pub const HEADING: &str = "end-of-run summary, one line per scenario that ran:";

/// The scenarios a run has finished, in the order they finished.
#[derive(Debug, Default)]
pub struct RunSummary {
    ran: Vec<(String, bool, Duration)>,
}

impl RunSummary {
    /// Records that the scenario `name` finished, passing or not, after
    /// `took`, from before its launch to after its artifacts were saved.
    pub fn record(&mut self, name: &str, passed: bool, took: Duration) {
        self.ran.push((name.to_owned(), passed, took));
    }

    /// The summary's lines: [`HEADING`], then one `summary: <name> <pass
    /// or fail> <seconds>s` per scenario, its seconds with one decimal
    /// place; nothing when no scenario ran.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        if self.ran.is_empty() {
            return Vec::new();
        }
        std::iter::once(HEADING.to_owned())
            .chain(self.ran.iter().map(|(name, passed, took)| {
                format!(
                    "summary: {name} {} {:.1}s",
                    if *passed { "pass" } else { "fail" },
                    took.as_secs_f64()
                )
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_scenario_that_ran_has_its_line_in_order() {
        let mut summary = RunSummary::default();
        assert_eq!(summary.lines(), Vec::<String>::new());
        summary.record("conhost_typing", true, Duration::from_millis(41_249));
        summary.record("notepad_editing", false, Duration::from_millis(7_060));
        assert_eq!(
            summary.lines(),
            [
                HEADING,
                "summary: conhost_typing pass 41.2s",
                "summary: notepad_editing fail 7.1s",
            ]
        );
    }
}
