//! Compare mode: a scenario's own setup and body run unchanged against NVDA
//! instead of Verbatim, and a report sets what the scenario expects
//! Verbatim to say beside what NVDA said, step by step
//! (`docs/nvda-transcript.md`, "Comparing a scenario").
//!
//! It is a research instrument, not a test: it asserts nothing, never runs
//! in continuous integration, and passes unless the harness itself fails,
//! such as a window that does not come to the front. [`ENV`] set to the
//! name of the scenario being run selects it ([`selects`]); set to any
//! other name, the scenario fails at once as a harness error, so a suite
//! run with the variable left set cannot pass. The scenario's
//! [`crate::Scenario`] then launches no Verbatim and reads NVDA's speech
//! from the transcript add-on ([`crate::nvda`]), and its
//! [`crate::SpeechCollector`] records each assertion's texts against the
//! step under way instead of checking them.
//!
//! A step runs from one input, or one action of the harness's that changes
//! what is in front, such as launching a window or bringing one forward,
//! to the next. Each assertion waits until NVDA has queued nothing for the
//! quiet period ([`QUIET_ENV`]), so the scenario does not run ahead of
//! NVDA. Before each input or action, and at the end, the collector waits
//! the same way unless NVDA was found quiet and nothing has happened
//! since, takes what NVDA said, and closes the step as a [`CompareStep`],
//! judged by [`CompareStep::verdict`]. A step whose speech the next input
//! interrupts ([`crate::SpeechCollector::expect_started`],
//! [`crate::SpeechCollector::expect_queued`]) waits only for NVDA's first
//! speech in it, not for the quiet period, since that input follows at
//! once, as the second press of a key pressed twice does;
//! it is compared together with the step after it, as one unit with both
//! inputs and one verdict, since NVDA's speech for a rapid double press
//! cannot be told apart press by press. The quiet period is never less
//! than [`QUIET_FLOOR`], NVDA's multi-press timeout, so NVDA never takes
//! the inputs of two distinct steps for a repeated press. The registry
//! writes the steps as `compare.txt` in the run's artifacts
//! ([`render_report`]) and prints it.
//!
//! A step a scenario declares deliberately different from NVDA
//! ([`crate::Scenario::deviates_from_nvda`]) is judged as declared; a
//! gesture is pressed as keys, the Verbatim modifier as Insert, and
//! compared like any other step. A sound Verbatim is expected to play,
//! `sound:` and its name wherever it is in an utterance's text, is left out
//! of the comparison and the rest of the utterance compared, since the
//! add-on records no sounds; an utterance that is nothing but a sound is
//! listed but not compared.
//!
//! A step is compared on normalized text ([`CompareStep::compared`]): each
//! side's utterances joined with single spaces, sounds, cancels and empty
//! entries dropped, and every run of white space one space. The add-on
//! records NVDA's speech sequence parts, joined with double spaces, not
//! where an utterance ends, so neither boundaries nor spacing are evidence
//! of a difference in what is heard.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::timeline::{Said, StepEntries};

/// Environment variable that, set to a scenario's name, runs that scenario
/// in compare mode ([`selects`]).
pub const ENV: &str = "VERBATIM_E2E_COMPARE";

/// Environment variable setting compare mode's quiet period in
/// milliseconds; [`crate::nvda::DEFAULT_QUIET`] when unset, and never less
/// than [`QUIET_FLOOR`].
pub const QUIET_ENV: &str = "VERBATIM_E2E_QUIET_MS";

/// The shortest quiet period: NVDA's multi-press timeout, 500 ms by
/// default (`multiPressTimeout` in `nvda/source/config/configSpec.py`).
/// NVDA counts a key pressed again within it as a repeat, so with a
/// shorter quiet period the inputs of two distinct steps could reach NVDA
/// as one double press.
pub const QUIET_FLOOR: Duration = Duration::from_millis(500);

/// The report's file name in the run's artifacts.
pub const REPORT_FILE_NAME: &str = "compare.txt";

/// Whether a run of `scenario` is in compare mode, [`ENV`] being `setting`:
/// `Ok(true)` when it names `scenario`, `Ok(false)` when it is unset or
/// empty.
///
/// # Errors
///
/// Returns a harness error naming both when it names another scenario, as
/// a variable left set after a comparison does in a suite run, which then
/// fails rather than running anything unasked.
pub fn selects(scenario: &str, setting: Option<&str>) -> Result<bool, String> {
    match setting {
        None | Some("") => Ok(false),
        Some(name) if name == scenario => Ok(true),
        Some(name) => Err(format!(
            "harness error: {ENV} is {name:?}, which runs only the scenario {name:?} in compare mode, but this is {scenario:?}; unset {ENV} to run {scenario:?} against Verbatim"
        )),
    }
}

/// The quiet period a comparison waits for: what [`QUIET_ENV`] sets, or the
/// default, raised to [`QUIET_FLOOR`] when it is shorter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuietPeriod {
    /// The period each step waits for.
    pub period: Duration,
    /// The period asked for, when it was shorter than [`QUIET_FLOOR`] and
    /// so raised to it.
    pub raised_from: Option<Duration>,
}

impl QuietPeriod {
    /// `asked`, raised to [`QUIET_FLOOR`] when it is shorter.
    #[must_use]
    pub fn from_asked(asked: Duration) -> Self {
        if asked < QUIET_FLOOR {
            Self {
                period: QUIET_FLOOR,
                raised_from: Some(asked),
            }
        } else {
            Self {
                period: asked,
                raised_from: None,
            }
        }
    }
}

/// The quiet period [`QUIET_ENV`] sets, or the default, raised to
/// [`QUIET_FLOOR`] when it is shorter.
///
/// # Errors
///
/// Returns an error naming the variable if it is set to anything but a
/// number of milliseconds.
pub fn quiet_period() -> Result<QuietPeriod, String> {
    let asked = match std::env::var(QUIET_ENV) {
        Err(_) => crate::nvda::DEFAULT_QUIET,
        Ok(value) => value
            .parse::<u64>()
            .map(Duration::from_millis)
            .map_err(|_| format!("{QUIET_ENV} is {value:?}, not a number of milliseconds"))?,
    };
    Ok(QuietPeriod::from_asked(asked))
}

/// One step of a compared scenario: its input, what Verbatim is expected
/// to say, what NVDA said, and what the scenario declared about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompareStep {
    /// What was injected or done, as the timeline describes it, the inputs
    /// of a unit of several separated by semicolons: `start` for what comes
    /// before any input, or a note that the step had none.
    pub input: String,
    /// The texts the scenario expects Verbatim to say, in order.
    pub expected: Vec<String>,
    /// What NVDA said, in order: its speech, and its other events, such as
    /// a cancel.
    pub heard: Vec<Said>,
    /// The parity entries the step is declared to differ by, each its file
    /// and heading.
    pub deviations: Vec<(String, String)>,
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
        }
    }
}

/// The word that starts a sound in an utterance's text, followed by the
/// sound's name, one word: `sound: spelling-error`.
const SOUND_WORD: &str = "sound:";

/// The words of `text` with every sound, [`SOUND_WORD`] and the name after
/// it, left out.
fn without_sounds(text: &str) -> impl Iterator<Item = &str> {
    let mut words = text.split_whitespace();
    std::iter::from_fn(move || {
        loop {
            let word = words.next()?;
            if word == SOUND_WORD {
                let _name = words.next();
            } else {
                return Some(word);
            }
        }
    })
}

/// Whether `text` holds a sound and nothing else.
fn only_sounds(text: &str) -> bool {
    text.split_whitespace().next() == Some(SOUND_WORD) && without_sounds(text).next().is_none()
}

impl CompareStep {
    /// The step from what the timeline holds for it, `step`, and the texts
    /// the scenario expected in it; `first` says whether it is the run's
    /// first step, which is `start` when it has no input. `None` when the
    /// step has nothing to report: no input, nothing expected or said, and
    /// no declaration or note. A step with an input always has.
    pub(crate) fn new(step: StepEntries, expected: Vec<String>, first: bool) -> Option<Self> {
        if step.inputs.is_empty()
            && expected.is_empty()
            && step.said.is_empty()
            && step.deviations.is_empty()
            && step.notes.is_empty()
        {
            return None;
        }
        let input = if !step.inputs.is_empty() {
            step.inputs.join("; ")
        } else if first {
            "start".to_owned()
        } else {
            "no input since the step before".to_owned()
        };
        Some(Self {
            input,
            expected,
            heard: step.said,
            deviations: step.deviations,
            notes: step.notes,
        })
    }

    /// The two strings the step is judged on, the expected texts' and
    /// NVDA's speech's: each side's texts, with the sounds in the expected
    /// ones and NVDA's events such as cancels left out, joined with single
    /// spaces, every run of white space made one space, and trimmed, so an
    /// empty text adds nothing.
    #[must_use]
    pub fn compared(&self) -> (String, String) {
        let expected: Vec<&str> = self
            .expected
            .iter()
            .flat_map(|text| without_sounds(text))
            .collect();
        let heard = self.heard.iter().filter_map(|said| match said {
            Said::Speech(text) => Some(text.as_str()),
            Said::Event(_) => None,
        });
        (expected.join(" "), normalized(heard))
    }

    /// The step's verdict: whether the two strings [`Self::compared`] gives
    /// are equal, judged against the first declared deviation when there
    /// is one.
    #[must_use]
    pub fn verdict(&self) -> Verdict {
        let (expected, heard) = self.compared();
        let same = expected == heard;
        match (self.deviations.first(), same) {
            (None, true) => Verdict::Same,
            (None, false) => Verdict::Differs,
            (Some((_, entry)), true) => Verdict::DeclaredButAgrees(entry.clone()),
            (Some((_, entry)), false) => Verdict::Declared(entry.clone()),
        }
    }
}

/// `texts` joined with single spaces, every run of white space made one
/// space, and trimmed.
fn normalized<'a>(texts: impl Iterator<Item = &'a str>) -> String {
    texts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
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
    pub quiet: QuietPeriod,
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
/// expected texts, what NVDA said, the harness's notes, the two strings
/// compared when the step differs, and the verdict), and a summary
/// counting each verdict. Plain text, one fact to a line, so it reads well
/// with a screen reader.
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
    let _ = writeln!(out, "Quiet period: {} ms", header.quiet.period.as_millis());
    if let Some(asked) = header.quiet.raised_from {
        let _ = writeln!(
            out,
            "Note: the quiet period asked for, {} ms, was raised to {} ms, NVDA's multi-press timeout, so that NVDA never takes the inputs of two steps for a repeated press.",
            asked.as_millis(),
            QUIET_FLOOR.as_millis()
        );
    }
    let mut counts = [0_usize; 4];
    for (index, step) in steps.iter().enumerate() {
        let verdict = step.verdict();
        counts[match verdict {
            Verdict::Same => 0,
            Verdict::Differs => 1,
            Verdict::Declared(_) => 2,
            Verdict::DeclaredButAgrees(_) => 3,
        }] += 1;
        let _ = writeln!(out, "\nStep {}: {}", index + 1, step.input);
        out.push_str("Verbatim:\n");
        if step.expected.is_empty() {
            out.push_str("  (nothing)\n");
        }
        for text in &step.expected {
            let note = if only_sounds(text) {
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
        if verdict == Verdict::Differs {
            let (expected, heard) = step.compared();
            for (side, text) in [("Verbatim", expected), ("NVDA", heard)] {
                let text = if text.is_empty() {
                    "(nothing)".to_owned()
                } else {
                    text
                };
                let _ = writeln!(out, "{side} compared: {text}");
            }
        }
        let _ = writeln!(out, "Verdict: {}", verdict.line());
    }
    let _ = writeln!(out, "\nSummary of {} steps:", steps.len());
    for (label, count) in [
        "same",
        "differs",
        "declared deviation",
        "declared, but NVDA agrees",
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

    fn cancel() -> Said {
        Said::Event("cancel".to_owned())
    }

    fn header() -> ReportHeader {
        ReportHeader {
            scenario: "text_box_review_cursor".to_owned(),
            date: "2026-10-15".to_owned(),
            nvda_version: "alpha-58022,732d1ce2".to_owned(),
            pin: "732d1ce2f (release-2026.3beta3-44-g732d1ce2f)".to_owned(),
            pin_matches: Some(false),
            quiet: QuietPeriod::from_asked(Duration::from_secs(1)),
        }
    }

    fn compared(expected: &str, heard: &str) -> (String, String) {
        (expected.to_owned(), heard.to_owned())
    }

    #[test]
    fn the_variable_selects_only_the_scenario_it_names_and_fails_any_other() {
        assert_eq!(selects("spelling_errors", None), Ok(false));
        assert_eq!(selects("spelling_errors", Some("")), Ok(false));
        assert_eq!(
            selects("spelling_errors", Some("spelling_errors")),
            Ok(true)
        );
        assert_eq!(
            selects("notepad_review_cursor", Some("text_box_review_cursor")),
            Err(
                "harness error: VERBATIM_E2E_COMPARE is \"text_box_review_cursor\", which runs only the scenario \"text_box_review_cursor\" in compare mode, but this is \"notepad_review_cursor\"; unset VERBATIM_E2E_COMPARE to run \"notepad_review_cursor\" against Verbatim"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_quiet_period_below_nvda_s_multi_press_timeout_is_raised_to_it() {
        assert_eq!(
            QuietPeriod::from_asked(Duration::from_millis(200)),
            QuietPeriod {
                period: QUIET_FLOOR,
                raised_from: Some(Duration::from_millis(200)),
            }
        );
        assert_eq!(
            QuietPeriod::from_asked(QUIET_FLOOR),
            QuietPeriod {
                period: QUIET_FLOOR,
                raised_from: None,
            }
        );
        let mut raised = header();
        raised.quiet = QuietPeriod::from_asked(Duration::from_millis(200));
        let report = render_report(&raised, &[]);
        assert!(
            report.contains(
                "Quiet period: 500 ms\nNote: the quiet period asked for, 200 ms, was raised to 500 ms, NVDA's multi-press timeout, so that NVDA never takes the inputs of two steps for a repeated press.\n"
            ),
            "{report}"
        );
    }

    #[test]
    fn a_step_is_the_same_only_when_nvda_said_the_expected_words_in_order() {
        let both = step(&["a", "b"], &[speech("a"), speech("b")]);
        assert_eq!(both.compared(), compared("a b", "a b"));
        assert_eq!(both.verdict(), Verdict::Same);
        let short = step(&["a", "b"], &[speech("a")]);
        assert_eq!(short.compared(), compared("a b", "a"));
        assert_eq!(short.verdict(), Verdict::Differs);
        let extra = step(&["a"], &[speech("b"), speech("a")]);
        assert_eq!(extra.compared(), compared("a", "b a"));
        assert_eq!(extra.verdict(), Verdict::Differs);
        let silent = step(&[], &[]);
        assert_eq!(silent.compared(), compared("", ""));
        assert_eq!(silent.verdict(), Verdict::Same);
    }

    #[test]
    fn a_step_with_an_input_is_reported_even_when_both_sides_are_silent() {
        let entries = |inputs: &[&str]| StepEntries {
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
            ..StepEntries::default()
        };
        let silent = CompareStep::new(entries(&["keys [shift]"]), Vec::new(), false)
            .expect("a step with an input is reported");
        assert_eq!(silent.input, "keys [shift]");
        assert_eq!(silent.verdict(), Verdict::Same);
        assert_eq!(CompareStep::new(entries(&[]), Vec::new(), false), None);
    }

    #[test]
    fn sounds_and_nvda_s_cancels_are_left_out_of_the_comparison() {
        let sounded = step(&["sound: start", "x"], &[cancel(), speech("x")]);
        assert_eq!(sounded.compared(), compared("x", "x"));
        assert_eq!(sounded.verdict(), Verdict::Same);
    }

    #[test]
    fn a_sound_inside_an_utterance_is_left_out_and_the_rest_of_it_compared() {
        // spelling_errors's word with a spelling error, read by
        // Control+Right Arrow, and its first line, whose error is mid-line.
        let word = step(
            &["sound: spelling-error spelling error h"],
            &[cancel(), speech("spelling error  h")],
        );
        assert_eq!(
            word.compared(),
            compared("spelling error h", "spelling error h")
        );
        assert_eq!(word.verdict(), Verdict::Same);
        let line = step(
            &[
                "sound: spelling-error spelling error Ths line has a sound: spelling-error spelling error tset .",
            ],
            &[speech(
                "spelling error  Ths line has a  spelling error  tset .",
            )],
        );
        assert_eq!(
            line.compared(),
            compared(
                "spelling error Ths line has a spelling error tset .",
                "spelling error Ths line has a spelling error tset ."
            )
        );
        let report = render_report(&header(), &[word]);
        assert!(
            report.contains("Verbatim:\n  sound: spelling-error spelling error h\nNVDA:\n"),
            "{report}"
        );
    }

    #[test]
    fn utterance_boundaries_spacing_and_empty_entries_are_not_differences() {
        // Each case is a step of the first compared run of
        // text_box_review_cursor as NVDA's add-on recorded it.
        let spaced = step(&["Name    Qty"], &[cancel(), speech("Name Qty")]);
        let split = step(
            &["Bottom blank"],
            &[cancel(), speech("Bottom"), speech("blank")],
        );
        let joined = step(
            &["Text edit multi line", "Name    Qty"],
            &[speech("Text  edit  multi line  Name Qty")],
        );
        let trailing = step(&["line feed"], &[cancel(), speech("line feed"), speech("")]);
        assert_eq!(spaced.compared(), compared("Name Qty", "Name Qty"));
        assert_eq!(split.compared(), compared("Bottom blank", "Bottom blank"));
        assert_eq!(
            joined.compared(),
            compared(
                "Text edit multi line Name Qty",
                "Text edit multi line Name Qty"
            )
        );
        assert_eq!(trailing.compared(), compared("line feed", "line feed"));
        for case in [spaced, split, joined, trailing] {
            assert_eq!(case.verdict(), Verdict::Same);
        }
    }

    #[test]
    fn a_word_nvda_adds_differs_and_the_report_shows_the_strings_compared() {
        let mut spelled = step(&["a"], &[cancel(), speech("a"), speech("Alfa")]);
        spelled.input = "gesture kb:numpad3".to_owned();
        assert_eq!(spelled.verdict(), Verdict::Differs);
        assert_eq!(
            render_report(&header(), &[spelled]),
            "Comparing text_box_review_cursor with NVDA\n\
             Date: 2026-10-15\n\
             NVDA: alpha-58022,732d1ce2\n\
             Submodule pin: 732d1ce2f (release-2026.3beta3-44-g732d1ce2f)\n\
             pin differs from the installed build: expectations come from captures and ported code from the pin\n\
             Quiet period: 1000 ms\n\
             \n\
             Step 1: gesture kb:numpad3\n\
             Verbatim:\n\
             \x20 a\n\
             NVDA:\n\
             \x20 cancel\n\
             \x20 a\n\
             \x20 Alfa\n\
             Verbatim compared: a\n\
             NVDA compared: a Alfa\n\
             Verdict: differs\n\
             \n\
             Summary of 1 steps:\n\
             \x20 same: 0\n\
             \x20 differs: 1\n\
             \x20 declared deviation: 0\n\
             \x20 declared, but NVDA agrees: 0\n"
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
        let mut start = step(
            &["sound: start", "Desktop list"],
            &[speech("Desktop  list")],
        );
        start.input = "start".to_owned();
        let mut cut = declared(step(
            &["3"],
            &[Said::Event("cancel".to_owned()), speech("")],
        ));
        cut.notes
            .push("NVDA's speech did not settle within 15s".to_owned());
        assert_eq!(
            render_report(&header(), &[start, cut]),
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
             \x20 Desktop  list\n\
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
             Summary of 2 steps:\n\
             \x20 same: 1\n\
             \x20 differs: 0\n\
             \x20 declared deviation: 1\n\
             \x20 declared, but NVDA agrees: 0\n"
        );
    }
}
