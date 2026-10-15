//! Compare mode: a scenario's own setup and body run unchanged against NVDA
//! instead of Verbatim, and a report sets what the scenario expects
//! Verbatim to say beside what NVDA said, step by step
//! (`docs/nvda-transcript.md`, "Comparing a scenario").
//!
//! It is a research instrument, not a test: it asserts nothing, never runs
//! in continuous integration, and passes unless the harness itself fails,
//! such as a window that does not come to the front. [`ENV`] set to `1`
//! selects it. The scenario's [`crate::Scenario`] then launches no
//! Verbatim and reads NVDA's speech from the transcript add-on
//! ([`crate::nvda`]), and its [`crate::SpeechCollector`] records each
//! assertion's texts against the step under way instead of checking them.
//! A step runs from one input to the next. Each assertion waits until NVDA
//! has queued nothing for the quiet period ([`QUIET_ENV`]), so the
//! scenario does not run ahead of NVDA. Before each input, and at the end,
//! the collector waits the same way unless NVDA was found quiet and
//! nothing has happened since, takes what NVDA said, and closes the step
//! as a [`CompareStep`], judged by [`CompareStep::verdict`]. The registry
//! writes the steps as `compare.txt` in the run's artifacts
//! ([`render_report`]) and prints it.
//!
//! A step a scenario declares deliberately different from NVDA
//! ([`crate::Scenario::deviates_from_nvda`]) is judged as declared, and a
//! step whose input is Verbatim's own, a control-plane request with no key
//! to press for NVDA, as Verbatim-only; a gesture is pressed as keys, the
//! Verbatim modifier as Insert, and compared like any other step. A sound
//! Verbatim is expected to play is listed but not compared, since the
//! add-on records no sounds.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::timeline::{Said, StepEntries};

/// Environment variable that, set to `1`, runs a scenario in compare mode.
pub const ENV: &str = "VERBATIM_E2E_COMPARE";

/// Environment variable setting compare mode's quiet period in
/// milliseconds; [`crate::nvda::DEFAULT_QUIET`] when unset.
pub const QUIET_ENV: &str = "VERBATIM_E2E_QUIET_MS";

/// The report's file name in the run's artifacts.
pub const REPORT_FILE_NAME: &str = "compare.txt";

/// Whether this run is in compare mode ([`ENV`] is `1`).
#[must_use]
pub fn enabled() -> bool {
    std::env::var(ENV).is_ok_and(|value| value == "1")
}

/// The quiet period [`QUIET_ENV`] sets, or the default.
///
/// # Errors
///
/// Returns an error naming the variable if it is set to anything but a
/// number of milliseconds.
pub fn quiet_period() -> Result<Duration, String> {
    match std::env::var(QUIET_ENV) {
        Err(_) => Ok(crate::nvda::DEFAULT_QUIET),
        Ok(value) => value
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| format!("{QUIET_ENV} is {value:?}, not a number of milliseconds")),
    }
}

/// One step of a compared scenario: its input, what Verbatim is expected
/// to say, what NVDA said, and what the scenario declared about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareStep {
    /// What was injected, as the timeline describes it: `start` for what
    /// comes before any input, or a note that the step had none.
    pub input: String,
    /// The texts the scenario expects Verbatim to say, in order.
    pub expected: Vec<String>,
    /// What NVDA said, in order: its speech, and its other events, such as
    /// a cancel.
    pub heard: Vec<Said>,
    /// The parity entries the step is declared to differ by, each its file
    /// and heading.
    pub deviations: Vec<(String, String)>,
    /// What makes the step Verbatim's own, if anything does.
    pub verbatim_only: Option<String>,
    /// The harness's remarks, such as NVDA's speech not settling.
    pub notes: Vec<String>,
}

/// How a step compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// NVDA said exactly what Verbatim is expected to.
    Same,
    /// NVDA said something else, and no difference was declared.
    Differs,
    /// NVDA said something else, as the declared parity entry says.
    Declared(String),
    /// A difference was declared, but NVDA said the same: worth a look.
    DeclaredButAgrees(String),
    /// The step's input is Verbatim's own, so nothing is compared.
    VerbatimOnly,
}

impl Verdict {
    /// The verdict as the report's verdict line words it.
    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Same => "same".to_owned(),
            Self::Differs => "differs".to_owned(),
            Self::Declared(entry) => format!("declared deviation: {entry}"),
            Self::DeclaredButAgrees(entry) => format!("declared, but NVDA agrees: {entry}"),
            Self::VerbatimOnly => "Verbatim-only".to_owned(),
        }
    }
}

/// What an expected text starts with when it is a sound Verbatim plays.
const SOUND_PREFIX: &str = "sound: ";

impl CompareStep {
    /// The step from what the timeline holds for it, `step`, and the texts
    /// the scenario expected in it; `first` says whether it is the run's
    /// first step, which is `start` when it has no input.
    pub(crate) fn new(step: StepEntries, expected: Vec<String>, first: bool) -> Self {
        let input = if !step.inputs.is_empty() {
            step.inputs.join("; ")
        } else if first {
            "start".to_owned()
        } else {
            "no input since the step before".to_owned()
        };
        Self {
            input,
            expected,
            heard: step.said,
            deviations: step.deviations,
            verbatim_only: step.verbatim_only,
            notes: step.notes,
        }
    }

    /// Whether the step has anything to report.
    pub(crate) fn is_empty(&self) -> bool {
        self.expected.is_empty()
            && self.heard.is_empty()
            && self.deviations.is_empty()
            && self.verbatim_only.is_none()
            && self.notes.is_empty()
    }

    /// The step's verdict: Verbatim-only when its input is Verbatim's own;
    /// otherwise whether the speech NVDA queued is exactly the texts
    /// expected, sounds left out, judged against the first declared
    /// deviation when there is one.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        if self.verbatim_only.is_some() {
            return Verdict::VerbatimOnly;
        }
        let expected: Vec<&str> = self
            .expected
            .iter()
            .map(String::as_str)
            .filter(|text| !text.starts_with(SOUND_PREFIX))
            .collect();
        let heard: Vec<&str> = self
            .heard
            .iter()
            .filter_map(|said| match said {
                Said::Speech(text) => Some(text.as_str()),
                Said::Event(_) => None,
            })
            .collect();
        let same = expected == heard;
        match (self.deviations.first(), same) {
            (None, true) => Verdict::Same,
            (None, false) => Verdict::Differs,
            (Some((_, entry)), true) => Verdict::DeclaredButAgrees(entry.clone()),
            (Some((_, entry)), false) => Verdict::Declared(entry.clone()),
        }
    }
}

/// What the report's header says about the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportHeader {
    /// The scenario's name.
    pub scenario: String,
    /// The date of the run, `2026-10-15`, in UTC.
    pub date: String,
    /// NVDA's version, as the add-on's `Hello` gives it.
    pub nvda_version: String,
    /// The `nvda` submodule's pin, as `git describe` gives it, or why it
    /// could not be read.
    pub pin: String,
    /// Whether the pin is the installed build: `Some(true)` or
    /// `Some(false)` when the version names a commit or a release tag the
    /// pin can be checked against, `None` when it cannot be told.
    pub pin_matches: Option<bool>,
    /// The quiet period each step waited for.
    pub quiet: Duration,
}

/// The `nvda` submodule's pin under `root`, as `git describe --tags
/// --always` words it, and whether it is the build `nvda_version` names: an
/// NVDA alpha's version ends with its commit after a comma
/// (`alpha-58022,732d1ce2`), and a release's is the submodule's tag
/// without its `release-` prefix.
#[must_use]
pub fn submodule_pin(root: &Path, nvda_version: &str) -> (String, Option<bool>) {
    let git = |args: &[&str]| -> Option<String> {
        use std::os::windows::process::CommandExt as _;
        let output = std::process::Command::new("git")
            .creation_flags(crate::CREATE_NO_WINDOW)
            .arg("-C")
            .arg(root.join("nvda"))
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    let Some(commit) = git(&["rev-parse", "HEAD"]) else {
        return (
            "(the nvda submodule's commit could not be read)".to_owned(),
            None,
        );
    };
    let short: String = commit.chars().take(9).collect();
    let described = git(&["describe", "--tags", "--always"]).unwrap_or_else(|| short.clone());
    let tags = git(&["tag", "--points-at", "HEAD"]).unwrap_or_default();
    let tags: Vec<&str> = tags.lines().collect();
    (
        format!("{short} ({described})"),
        pin_matches(&commit, &tags, nvda_version),
    )
}

/// Whether the submodule at `commit`, tagged `tags`, is the build
/// `nvda_version` names; `None` when the version names neither a commit
/// nor a release.
fn pin_matches(commit: &str, tags: &[&str], nvda_version: &str) -> Option<bool> {
    if let Some((_, built)) = nvda_version.rsplit_once(',') {
        let built = built.trim();
        if !built.is_empty() && built.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(commit.starts_with(&built.to_ascii_lowercase()));
        }
        return None;
    }
    let release = nvda_version
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit());
    release.then(|| tags.contains(&format!("release-{nvda_version}").as_str()))
}

/// The report: a header, one block per step (its input, Verbatim's
/// expected texts, what NVDA said, the harness's notes, and the verdict),
/// and a summary counting each verdict. Plain text, one fact to a line, so
/// it reads well with a screen reader.
#[must_use]
pub fn render_report(header: &ReportHeader, steps: &[CompareStep]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Comparing {} with NVDA", header.scenario);
    let _ = writeln!(out, "Date: {}", header.date);
    let _ = writeln!(out, "NVDA: {}", header.nvda_version);
    let _ = writeln!(out, "Submodule pin: {}", header.pin);
    let _ = writeln!(
        out,
        "{}",
        match header.pin_matches {
            Some(true) => "The pin is the installed build.",
            Some(false) => {
                "pin differs from the installed build: expectations come from captures and ported code from the pin"
            }
            None => "Whether the pin is the installed build could not be told from NVDA's version.",
        }
    );
    let _ = writeln!(out, "Quiet period: {} ms", header.quiet.as_millis());
    let mut counts = [0_usize; 5];
    for (index, step) in steps.iter().enumerate() {
        let verdict = step.verdict();
        counts[match verdict {
            Verdict::Same => 0,
            Verdict::Differs => 1,
            Verdict::Declared(_) => 2,
            Verdict::DeclaredButAgrees(_) => 3,
            Verdict::VerbatimOnly => 4,
        }] += 1;
        let _ = writeln!(out, "\nStep {}: {}", index + 1, step.input);
        if let Some(what) = &step.verbatim_only {
            let _ = writeln!(out, "Verbatim-only: {what}");
        }
        out.push_str("Verbatim:\n");
        if step.expected.is_empty() {
            out.push_str("  (nothing)\n");
        }
        for text in &step.expected {
            let note = if text.starts_with(SOUND_PREFIX) {
                " (not compared)"
            } else {
                ""
            };
            let _ = writeln!(out, "  {}{note}", readable(text));
        }
        out.push_str("NVDA:\n");
        if step.heard.is_empty() {
            out.push_str("  (nothing)\n");
        }
        for said in &step.heard {
            match said {
                Said::Speech(text) => {
                    let _ = writeln!(out, "  {}", readable(text));
                }
                Said::Event(name) => {
                    let _ = writeln!(out, "  {name}");
                }
            }
        }
        for (file, entry) in &step.deviations {
            let _ = writeln!(out, "Declared deviation: {file}, \"{entry}\"");
        }
        for note in &step.notes {
            let _ = writeln!(out, "Note: {note}");
        }
        let _ = writeln!(out, "Verdict: {}", verdict.line());
    }
    let _ = writeln!(out, "\nSummary of {} steps:", steps.len());
    for (label, count) in [
        "same",
        "differs",
        "declared deviation",
        "declared, but NVDA agrees",
        "Verbatim-only",
    ]
    .iter()
    .zip(counts)
    {
        let _ = writeln!(out, "  {label}: {count}");
    }
    out
}

/// `text` on one line of the report: an empty utterance named, and a line
/// break inside one shown as a slash between its lines.
fn readable(text: &str) -> String {
    if text.is_empty() {
        "(empty)".to_owned()
    } else {
        text.lines().collect::<Vec<_>>().join(" / ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(expected: &[&str], heard: &[Said]) -> CompareStep {
        CompareStep {
            input: "keys [tab]".to_owned(),
            expected: expected.iter().map(|text| (*text).to_owned()).collect(),
            heard: heard.to_vec(),
            deviations: Vec::new(),
            verbatim_only: None,
            notes: Vec::new(),
        }
    }

    fn speech(text: &str) -> Said {
        Said::Speech(text.to_owned())
    }

    fn declared(mut step: CompareStep) -> CompareStep {
        step.deviations.push((
            "docs/parity/review.md".to_owned(),
            "Review cursor columns".to_owned(),
        ));
        step
    }

    #[test]
    fn a_step_is_the_same_only_when_nvda_said_exactly_the_expected_texts() {
        assert_eq!(
            step(&["a", "b"], &[speech("a"), speech("b")]).verdict(),
            Verdict::Same
        );
        assert_eq!(
            step(&["a", "b"], &[speech("a")]).verdict(),
            Verdict::Differs
        );
        assert_eq!(
            step(&["a"], &[speech("b"), speech("a")]).verdict(),
            Verdict::Differs
        );
        assert_eq!(step(&[], &[]).verdict(), Verdict::Same);
    }

    #[test]
    fn sounds_and_nvda_s_cancels_are_left_out_of_the_comparison() {
        let heard = [Said::Event("cancel".to_owned()), speech("x")];
        assert_eq!(
            step(&["sound: start", "x"], &heard).verdict(),
            Verdict::Same
        );
    }

    #[test]
    fn a_declared_step_is_judged_against_its_entry() {
        assert_eq!(
            declared(step(&["3"], &[speech("A")])).verdict(),
            Verdict::Declared("Review cursor columns".to_owned())
        );
        assert_eq!(
            declared(step(&["3"], &[speech("3")])).verdict(),
            Verdict::DeclaredButAgrees("Review cursor columns".to_owned())
        );
    }

    #[test]
    fn a_verbatim_only_step_is_never_compared() {
        let mut only = step(&["Verbatim", "Context menu"], &[]);
        only.verbatim_only = Some("a control-plane request".to_owned());
        assert_eq!(only.verdict(), Verdict::VerbatimOnly);
    }

    #[test]
    fn the_pin_is_matched_by_the_commit_or_the_release_nvda_s_version_names() {
        let commit = "732d1ce2f0123456789abcdef0123456789abcde";
        assert_eq!(pin_matches(commit, &[], "alpha-58022,732d1ce2"), Some(true));
        assert_eq!(
            pin_matches(commit, &[], "alpha-58022,0123abcd"),
            Some(false)
        );
        assert_eq!(
            pin_matches(commit, &["release-2026.3"], "2026.3"),
            Some(true)
        );
        assert_eq!(pin_matches(commit, &[], "2026.3"), Some(false));
        assert_eq!(pin_matches(commit, &[], "source-master"), None);
    }

    #[test]
    fn the_report_shows_each_step_s_two_sides_and_verdict_and_counts_them() {
        let header = ReportHeader {
            scenario: "text_box_review_cursor".to_owned(),
            date: "2026-10-15".to_owned(),
            nvda_version: "alpha-58022,732d1ce2".to_owned(),
            pin: "732d1ce2f (release-2026.3beta3-44-g732d1ce2f)".to_owned(),
            pin_matches: Some(false),
            quiet: Duration::from_secs(1),
        };
        let mut start = step(&["sound: start", "Desktop list"], &[speech("Desktop list")]);
        start.input = "start".to_owned();
        let mut cut = declared(step(
            &["3"],
            &[Said::Event("cancel".to_owned()), speech("")],
        ));
        cut.notes
            .push("NVDA's speech did not settle within 15s".to_owned());
        let mut menu = step(&[], &[]);
        menu.input = "a control-plane request".to_owned();
        menu.verbatim_only = Some("a control-plane request".to_owned());
        assert_eq!(
            render_report(&header, &[start, cut, menu]),
            "Comparing text_box_review_cursor with NVDA\n\
             Date: 2026-10-15\n\
             NVDA: alpha-58022,732d1ce2\n\
             Submodule pin: 732d1ce2f (release-2026.3beta3-44-g732d1ce2f)\n\
             pin differs from the installed build: expectations come from captures and ported code from the pin\n\
             Quiet period: 1000 ms\n\
             \n\
             Step 1: start\n\
             Verbatim:\n\
             \x20 sound: start (not compared)\n\
             \x20 Desktop list\n\
             NVDA:\n\
             \x20 Desktop list\n\
             Verdict: same\n\
             \n\
             Step 2: keys [tab]\n\
             Verbatim:\n\
             \x20 3\n\
             NVDA:\n\
             \x20 cancel\n\
             \x20 (empty)\n\
             Declared deviation: docs/parity/review.md, \"Review cursor columns\"\n\
             Note: NVDA's speech did not settle within 15s\n\
             Verdict: declared deviation: Review cursor columns\n\
             \n\
             Step 3: a control-plane request\n\
             Verbatim-only: a control-plane request\n\
             Verbatim:\n\
             \x20 (nothing)\n\
             NVDA:\n\
             \x20 (nothing)\n\
             Verdict: Verbatim-only\n\
             \n\
             Summary of 3 steps:\n\
             \x20 same: 1\n\
             \x20 differs: 0\n\
             \x20 declared deviation: 1\n\
             \x20 declared, but NVDA agrees: 0\n\
             \x20 Verbatim-only: 1\n"
        );
    }
}
