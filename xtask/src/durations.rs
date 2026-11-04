//! `cargo xtask e2e-durations <runs>`: rewrites `xtask/e2e-durations.txt`,
//! the table `cargo xtask e2e-plan` sizes the CI groups by, from the `e2e`
//! jobs of the last `<runs>` completed runs of CI on `main` (pushes), read
//! with the `gh` CLI (`books/dev/src/how-to/e2e.md`, "How CI runs the suite").
//!
//! Every `e2e` job counts, one per group (`e2e (1)`, `e2e (2)`, ...) or
//! the single job of a run before the groups, when it succeeded or failed;
//! a cancelled or skipped one, or one whose log has expired, is left out.
//! A job's log ends with the suite's end-of-run summary, a heading then
//! `summary: <name> <pass|fail> <seconds>s` for each scenario that ran
//! (`books/dev/src/how-to/e2e.md`, "The end-of-run summary"), and a scenario's
//! duration is its summary line's. A log from before the summary has none,
//! and there, since the scenarios run one at a time (`--test-threads=1`),
//! a scenario's duration is the time between its `test <name> ... ok` or
//! `FAILED` line and the line before it, `running <n> tests` for the
//! first. Each scenario's figure is the median of its durations, the
//! middle two's mean for an even count.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

/// Runs the command with its arguments after `e2e-durations`.
pub fn run(args: &[String]) -> ExitCode {
    let [runs] = args else {
        eprintln!("usage: cargo xtask e2e-durations <runs>, such as 30");
        return ExitCode::from(2);
    };
    let runs = match runs.parse::<usize>() {
        Ok(runs) if runs > 0 => runs,
        _ => {
            eprintln!("e2e-durations: {runs:?} is not a whole number of at least 1");
            return ExitCode::from(2);
        }
    };
    match refresh(runs) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("e2e-durations: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Reads the runs' logs and writes the table.
fn refresh(runs: usize) -> Result<(), String> {
    let listed = gh(&[
        "run",
        "list",
        "--workflow",
        "ci.yml",
        "--branch",
        "main",
        "--event",
        "push",
        "--status",
        "completed",
        "--limit",
        &runs.to_string(),
        "--json",
        "databaseId",
        "--jq",
        ".[].databaseId",
    ])?;
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut used = 0;
    for run in listed.lines() {
        let jobs = gh(&[
            "run",
            "view",
            run,
            "--json",
            "jobs",
            "--jq",
            r#".jobs[] | select((.name == "e2e" or (.name | startswith("e2e ("))) and (.conclusion == "success" or .conclusion == "failure")) | .databaseId"#,
        ])?;
        let mut read_one = false;
        for job in jobs.lines() {
            match gh(&[
                "api",
                // Logs carry the test output's colours.
                "--allow-escape-sequences",
                &format!("repos/{{owner}}/{{repo}}/actions/jobs/{job}/logs"),
            ]) {
                Ok(log) => {
                    for (name, seconds) in durations(&log) {
                        samples.entry(name).or_default().push(seconds);
                    }
                    read_one = true;
                }
                Err(error) => eprintln!("e2e-durations: run {run}, job {job}: {error}; left out"),
            }
        }
        if read_one {
            used += 1;
        }
    }
    if samples.is_empty() {
        return Err(format!(
            "no e2e job log of the last {runs} runs named a scenario"
        ));
    }
    let table = render(&samples, used);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("e2e-durations.txt");
    std::fs::write(&path, table).map_err(|error| format!("{}: {error}", path.display()))?;
    let sum: f64 = samples.values().map(|values| median(values)).sum();
    eprintln!(
        "e2e-durations: {} scenarios from {used} runs, {:.1} minutes in all, written to xtask/e2e-durations.txt",
        samples.len(),
        sum / 60.0
    );
    Ok(())
}

/// Runs `gh` with `args` and returns its standard output.
fn gh(args: &[&str]) -> Result<String, String> {
    let output = Command::new("gh")
        .args(args)
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("could not run gh: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "gh {} failed: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("gh printed something not UTF-8: {error}"))
}

/// The table's text: a header, then each scenario's median, by name.
fn render(samples: &BTreeMap<String, Vec<f64>>, runs: usize) -> String {
    let mut text = format!(
        "# The median duration in seconds of each end-to-end scenario on GitHub's\n\
         # runner, over the e2e jobs of {runs} runs of CI on main, written by\n\
         # `cargo xtask e2e-durations`; `cargo xtask e2e-plan` sizes the CI groups\n\
         # by it (books/dev/src/how-to/e2e.md, \"How CI runs the suite\").\n\
         # One scenario per line: seconds, then the name.\n"
    );
    for (name, values) in samples {
        let _ = writeln!(text, "{:.1} {name}", median(values));
    }
    text
}

/// The median: the middle value, or the middle two's mean.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        f64::midpoint(sorted[middle - 1], sorted[middle])
    } else {
        sorted[middle]
    }
}

/// The heading the suite prints before its summary lines
/// (`verbatim_e2e::run_summary::HEADING`, which xtask cannot depend on,
/// since it builds on Linux).
const SUMMARY_HEADING: &str = "end-of-run summary, one line per scenario that ran:";

/// Each scenario's duration in one job's log: its summary line's, or, in a
/// log with no summary, [`gap_durations`]'.
fn durations(log: &str) -> Vec<(String, f64)> {
    summary_durations(log).unwrap_or_else(|| gap_durations(log))
}

/// The summary's durations: each `summary: <name> <pass|fail> <seconds>s`
/// line after the heading, or `None` when the log has no heading.
fn summary_durations(log: &str) -> Option<Vec<(String, f64)>> {
    let mut found = None;
    for line in log.lines() {
        let line = line.rsplit('\t').next().unwrap_or(line);
        // The text after the line's time, when it has one.
        let text = strip_escapes(timestamped(line).map_or(line, |(_, text)| text));
        let text = text.trim();
        if text == SUMMARY_HEADING {
            found = Some(Vec::new());
        } else if let Some(found) = found.as_mut()
            && let Some(rest) = text.strip_prefix("summary: ")
        {
            let mut words = rest.split_whitespace();
            if let (Some(name), Some("pass" | "fail"), Some(seconds), None) =
                (words.next(), words.next(), words.next(), words.next())
                && let Some(Ok(seconds)) = seconds.strip_suffix('s').map(str::parse::<f64>)
            {
                found.push((name.to_string(), seconds));
            }
        }
    }
    found
}

/// Each scenario's duration in one job's log with no summary: the time
/// from the line before its result line, `running <n> tests` or the
/// previous result.
fn gap_durations(log: &str) -> Vec<(String, f64)> {
    let mut found = Vec::new();
    let mut previous: Option<f64> = None;
    for line in log.lines() {
        let Some((time, text)) = timestamped(line) else {
            continue;
        };
        let text = strip_escapes(text);
        if text.starts_with("running ") && text.ends_with(" tests") {
            previous = Some(time);
        } else if let Some(rest) = text.strip_prefix("test ")
            && let Some((name, outcome)) = rest.split_once(" ... ")
            && matches!(outcome.trim(), "ok" | "FAILED")
        {
            if let Some(start) = previous {
                found.push((name.to_string(), time - start));
            }
            previous = Some(time);
        }
    }
    found
}

/// Splits a log line into its time, in seconds since the Unix epoch, and
/// its text. GitHub's job logs start each line with an RFC 3339 time in
/// UTC, such as `2026-11-03T01:27:03.1234567Z `; `gh run view --log`
/// puts the job's and step's names and a tab before it.
fn timestamped(line: &str) -> Option<(f64, &str)> {
    let line = line.rsplit('\t').next().unwrap_or(line);
    let (stamp, text) = line.split_once("Z ")?;
    let (date, clock) = stamp.split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (Some(Ok(year)), Some(Ok(month)), Some(Ok(day)), None) =
        (date.next(), date.next(), date.next(), date.next())
    else {
        return None;
    };
    let mut clock = clock.split(':');
    let (Some(hour), Some(minute), Some(second), None) =
        (clock.next(), clock.next(), clock.next(), clock.next())
    else {
        return None;
    };
    let hour: f64 = hour.parse().ok()?;
    let minute: f64 = minute.parse().ok()?;
    let second: f64 = second.parse().ok()?;
    // Whole days since 1970-01-01, by the proleptic Gregorian calendar
    // (Howard Hinnant's days_from_civil).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    #[expect(
        clippy::cast_precision_loss,
        reason = "days since 1970 are far below 2^52"
    )]
    let days = days as f64;
    Some((
        days * 86_400.0 + hour * 3_600.0 + minute * 60.0 + second,
        text,
    ))
}

/// The text without its ANSI colour sequences (`ESC [ ... m`).
fn strip_escapes(text: &str) -> String {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_scenario_lasts_from_the_line_before_its_result() {
        let log = "2026-11-03T01:27:00.0000000Z Compiling e2e\n\
                   2026-11-03T01:27:10.0000000Z running 3 tests\n\
                   2026-11-03T01:27:40.5000000Z test conhost_typing ... \u{1b}[32mok\u{1b}[0m\n\
                   2026-11-03T01:27:41.0000000Z a line the harness printed\n\
                   2026-11-03T01:28:40.5000000Z test text_box_editing ... FAILED\n\
                   2026-11-03T01:28:45.0000000Z test demo_say_all ... ignored\n\
                   2026-11-04T00:00:00.5000000Z test theme_panel ... ok\n";
        assert_eq!(
            durations(log),
            vec![
                ("conhost_typing".to_string(), 30.5),
                ("text_box_editing".to_string(), 60.0),
                ("theme_panel".to_string(), 81_080.0),
            ]
        );
    }

    #[test]
    fn a_log_with_the_summary_takes_each_duration_from_its_line() {
        let log = "2026-11-03T01:27:10.0000000Z running 2 tests\n\
                   2026-11-03T01:27:40.5000000Z test conhost_typing ... ok\n\
                   2026-11-03T01:28:40.5000000Z test text_box_editing ... FAILED\n\
                   2026-11-03T01:28:41.0000000Z end-of-run summary, one line per scenario that ran:\n\
                   2026-11-03T01:28:41.0000000Z summary: conhost_typing pass 41.2s\n\
                   2026-11-03T01:28:41.0000000Z summary: text_box_editing fail 7.1s\n";
        assert_eq!(
            durations(log),
            vec![
                ("conhost_typing".to_string(), 41.2),
                ("text_box_editing".to_string(), 7.1),
            ]
        );
    }

    #[test]
    fn a_line_from_gh_run_view_has_its_names_before_a_tab() {
        let line = "e2e (2)\tRun the E2E suite\t1970-01-02T00:00:01.5000000Z running 1 tests";
        assert_eq!(timestamped(line), Some((86_401.5, "running 1 tests")));
    }

    #[test]
    fn the_median_of_an_even_count_is_the_middle_twos_mean() {
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), 2.5);
        assert_eq!(median(&[5.0, 1.0, 3.0]), 3.0);
    }

    #[test]
    fn the_table_lists_each_median_by_name_under_its_header() {
        let mut samples = BTreeMap::new();
        samples.insert("b".to_string(), vec![2.0, 4.0]);
        samples.insert("a".to_string(), vec![1.3]);
        let table = render(&samples, 3);
        assert_eq!(
            table
                .lines()
                .filter(|line| !line.starts_with('#'))
                .collect::<Vec<_>>(),
            vec!["1.3 a", "3.0 b"]
        );
        assert_eq!(
            plan_table(&table),
            vec![("a".to_string(), 1.3), ("b".to_string(), 3.0)]
        );
    }

    /// The table as the planner reads it, by name.
    fn plan_table(text: &str) -> Vec<(String, f64)> {
        let mut entries: Vec<(String, f64)> = crate::plan::parse_durations(text)
            .expect("the planner reads the table")
            .into_iter()
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }
}
