//! `cargo xtask e2e-plan`: decides how many groups GitHub's `e2e` job runs
//! the end-to-end suite in, and prints the job's matrix as JSON,
//! `{"group":[1,2,...]}` (`books/dev/src/how-to/e2e.md`, "How CI runs the suite").
//! It runs on every host, since the `plan` job runs it on Linux, where
//! xtask builds neither the registry crate nor the test binary.
//!
//! The suite is read from the source of the registry,
//! `crates/verbatim-e2e/src/registry.rs`, whose entries the test binary
//! builds its tests from: each `suite!` entry not marked `.local_only`, and
//! the agent's session check ([`SESSION_CHECK`]), the one test the binary
//! adds outside the registry. Each Windows group checks this reading
//! against the test binary's own listing and the real registry
//! (`xtask/src/group.rs`), so a scenario is never dropped silently.
//!
//! Each scenario's estimate is its median in `xtask/e2e-durations.txt`,
//! or [`DEFAULT_SECONDS`] when the table lacks it; a table entry naming no
//! scenario is ignored. The groups are balanced greedily, longest first,
//! each to the group with the least time so far (the lowest index on a
//! tie), with names breaking ties of time, so the assignment depends only
//! on the set of names and the table. The count is the smallest, from the
//! total over [`BUDGET_SECONDS`] rounded up, whose every group's estimate
//! is within that budget: the step running a group times out after 11
//! minutes, and the budget leaves a minute's margin and a minute for a
//! failure's evidence, such as a minidump the agent may wait a minute
//! for. The one failure is a
//! single scenario estimated over the budget, which no count can place.
//!
//! With `--coverage`, for a coverage run's groups, which run an instrumented
//! build (`books/dev/src/how-to/coverage.md`), every estimate is multiplied by
//! [`COVERAGE_SLOWDOWN`] before it is held against the budget, so the run
//! gets more groups rather than longer ones. Only the count changes: the
//! split for a count is the same, so `cargo xtask e2e-group` needs no flag.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The checked-in table of median durations.
const DURATIONS: &str = include_str!("../e2e-durations.txt");

/// The estimate for a scenario the table lacks: longer than most
/// scenarios take, so a new one is not packed in as if it were free.
pub const DEFAULT_SECONDS: f64 = 60.0;

/// The most a group's estimate may total: the 11-minute timeout of the
/// step running it, less a minute's margin and a minute for a failure's
/// evidence.
pub const BUDGET_SECONDS: f64 = 540.0;

/// How much longer a scenario takes against the instrumented build of a
/// coverage run than the table says: the instrumented unit and mockapp
/// tests' measured slowdown, at most 1.17, with a margin
/// (`books/dev/src/how-to/e2e.md`, "How CI runs the suite").
pub const COVERAGE_SLOWDOWN: f64 = 1.25;

/// One registry entry as its source states it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The scenario's name.
    pub name: String,
    /// Its group's variant name: `Suite`, `Demo` or `Measure`.
    pub group: String,
    /// Whether it is marked `.local_only`.
    pub local_only: bool,
}

/// The test the binary adds outside the registry: the agent's session
/// check, which is in the suite.
pub const SESSION_CHECK: &str = "agent_reports_an_interactive_window_station";

/// Runs the command with its arguments after `e2e-plan`.
pub fn run(args: &[String]) -> ExitCode {
    let slowdown = match args {
        [] => 1.0,
        [flag] if flag == "--coverage" => COVERAGE_SLOWDOWN,
        _ => {
            eprintln!("usage: cargo xtask e2e-plan [--coverage]");
            return ExitCode::from(2);
        }
    };
    let planned = table().and_then(|table| {
        let suite = read_suite()?;
        let count = group_count(&suite, &table, slowdown)?;
        Ok((suite, table, count))
    });
    let (suite, table, count) = match planned {
        Ok(planned) => planned,
        Err(error) => {
            eprintln!("e2e-plan: {error}");
            return ExitCode::FAILURE;
        }
    };
    let groups = assign(&suite, &table, count);
    let longest = groups
        .iter()
        .map(|group| total(&table, group))
        .fold(0.0, f64::max);
    eprintln!(
        "e2e-plan: {} scenarios, about {:.1} minutes by the table, in {count} groups of at most {:.1} minutes",
        suite.len(),
        total(&table, &suite) * slowdown / 60.0,
        longest * slowdown / 60.0
    );
    let indices: Vec<String> = (1..=count).map(|index| index.to_string()).collect();
    println!("{{\"group\":[{}]}}", indices.join(","));
    ExitCode::SUCCESS
}

/// The workspace's root.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the workspace")
        .to_path_buf()
}

/// The checked-in table, parsed.
///
/// # Errors
///
/// Returns the first malformed line.
pub fn table() -> Result<HashMap<String, f64>, String> {
    parse_durations(DURATIONS).map_err(|error| format!("xtask/e2e-durations.txt: {error}"))
}

/// The suite as the source states it, in the tests file's order.
///
/// # Errors
///
/// Returns an error when a source file cannot be read or is not laid out
/// as [`suite_from_source`] expects.
pub fn read_suite() -> Result<Vec<String>, String> {
    let root = workspace();
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).map_err(|error| format!("{path}: {error}"))
    };
    suite_from_source(&read("crates/verbatim-e2e/src/registry.rs")?)
}

/// Parses the table: `#` comments, blank lines, and lines of seconds then
/// a name.
///
/// # Errors
///
/// Returns the first line that is not seconds then a name, or that names
/// a scenario named before.
pub fn parse_durations(text: &str) -> Result<HashMap<String, f64>, String> {
    let mut table = HashMap::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let (Some(seconds), Some(name), None) = (words.next(), words.next(), words.next()) else {
            return Err(format!(
                "line {}: not <seconds> <name>: {line:?}",
                number + 1
            ));
        };
        let seconds: f64 = seconds
            .parse()
            .map_err(|_| format!("line {}: {seconds:?} is not a number", number + 1))?;
        if table.insert(name.to_string(), seconds).is_some() {
            return Err(format!("line {}: {name} is named twice", number + 1));
        }
    }
    Ok(table)
}

/// The entries of the registry's `SCENARIOS` array, read from its source:
/// each entry starts a line with its constructor, `suite!(`, `demo!(` or
/// `measure!(`, which gives its group, followed by its name, a module's
/// name or a string literal; it is local-only when its text, up to the
/// next entry, calls `.local_only(`. Comment lines are skipped.
///
/// # Errors
///
/// Returns an error naming the line when an entry's name cannot be read,
/// or when the array is not found or holds no entry.
pub fn registry_entries(source: &str) -> Result<Vec<Entry>, String> {
    const GROUPS: [(&str, &str); 3] = [
        ("suite!(", "Suite"),
        ("demo!(", "Demo"),
        ("measure!(", "Measure"),
    ];
    let start = source
        .find("pub const SCENARIOS: &[ScenarioDef] = &[")
        .ok_or("registry.rs: no SCENARIOS array")?;
    let mut entries: Vec<Entry> = Vec::new();
    for line in source[start..].lines().skip(1) {
        let line = line.trim();
        if line == "];" {
            break;
        }
        if line.starts_with("//") {
            continue;
        }
        let constructor = GROUPS
            .iter()
            .find_map(|(prefix, group)| line.strip_prefix(prefix).map(|rest| (rest, *group)));
        if let Some((rest, group)) = constructor {
            let name = rest
                .split([',', ')'])
                .next()
                .map(|name| name.trim().trim_matches('"'))
                .filter(|name| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .ok_or_else(|| {
                    format!("registry.rs: no scenario name this reading understands in {line:?}")
                })?;
            entries.push(Entry {
                name: name.to_string(),
                group: group.to_string(),
                local_only: false,
            });
        }
        if line.contains(".local_only(")
            && let Some(entry) = entries.last_mut()
        {
            entry.local_only = true;
        }
    }
    if entries.is_empty() {
        return Err(
            "registry.rs: the SCENARIOS array holds no entry this reading understands".into(),
        );
    }
    Ok(entries)
}

/// The suite from the registry's source: every entry of the `Suite` group
/// not marked local-only, and [`SESSION_CHECK`].
///
/// # Errors
///
/// Returns [`registry_entries`]'s error.
pub fn suite_from_source(registry: &str) -> Result<Vec<String>, String> {
    let mut suite: Vec<String> = registry_entries(registry)?
        .into_iter()
        .filter(|entry| !entry.local_only && entry.group == "Suite")
        .map(|entry| entry.name)
        .collect();
    suite.push(SESSION_CHECK.to_string());
    Ok(suite)
}

/// The table's estimate for `name`, or [`DEFAULT_SECONDS`].
pub fn estimate(table: &HashMap<String, f64>, name: &str) -> f64 {
    table.get(name).copied().unwrap_or(DEFAULT_SECONDS)
}

/// The table's estimate for some scenarios together.
pub fn total(table: &HashMap<String, f64>, names: &[String]) -> f64 {
    names.iter().map(|name| estimate(table, name)).sum()
}

/// The number of groups: the smallest, from the total over
/// [`BUDGET_SECONDS`] rounded up, whose every group is within the budget,
/// each estimate multiplied by `slowdown` (1 for an ordinary run).
///
/// # Errors
///
/// Returns an error naming each scenario estimated over the budget, since
/// no count places one.
pub fn group_count(
    suite: &[String],
    table: &HashMap<String, f64>,
    slowdown: f64,
) -> Result<usize, String> {
    let over: Vec<String> = suite
        .iter()
        .filter(|name| estimate(table, name) * slowdown > BUDGET_SECONDS)
        .map(|name| {
            format!(
                "{name} ({:.1} minutes)",
                estimate(table, name) * slowdown / 60.0
            )
        })
        .collect();
    if !over.is_empty() {
        return Err(format!(
            "{} estimated over a group's budget of {:.0} minutes (the step's 11-minute timeout less a minute's margin and a minute for a failure's evidence), so no group can hold it: make it faster",
            over.join(", "),
            BUDGET_SECONDS / 60.0
        ));
    }
    let mut count = 1;
    let mut capacity = BUDGET_SECONDS;
    while total(table, suite) * slowdown > capacity {
        count += 1;
        capacity += BUDGET_SECONDS;
    }
    while assign(suite, table, count)
        .iter()
        .any(|group| total(table, group) * slowdown > BUDGET_SECONDS)
    {
        count += 1;
    }
    Ok(count)
}

/// Splits `scenarios` into `count` groups, greedily: the longest first,
/// each to the group with the least time so far (the lowest index on a
/// tie), with names breaking ties of time. Each group is in name order.
pub fn assign(
    scenarios: &[String],
    table: &HashMap<String, f64>,
    count: usize,
) -> Vec<Vec<String>> {
    let mut order: Vec<(f64, &String)> = scenarios
        .iter()
        .map(|name| (estimate(table, name), name))
        .collect();
    order.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut groups: Vec<(f64, Vec<String>)> = vec![(0.0, Vec::new()); count];
    for (seconds, name) in order {
        let lightest = groups
            .iter_mut()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .expect("at least one group");
        lightest.0 += seconds;
        lightest.1.push(name.clone());
    }
    groups
        .into_iter()
        .map(|(_, mut names)| {
            names.sort();
            names
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table's names and one it lacks.
    fn listing() -> Vec<String> {
        let mut names: Vec<String> = table().expect("the table parses").into_keys().collect();
        names.push("a_scenario_the_table_lacks".to_string());
        names.sort();
        names
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn every_scenario_lands_in_exactly_one_group_for_one_to_six_groups() {
        let table = table().expect("the table parses");
        let listed = listing();
        for count in 1..=6 {
            let groups = assign(&listed, &table, count);
            assert_eq!(groups.len(), count, "{count} groups");
            let mut assigned: Vec<String> = groups.into_iter().flatten().collect();
            assigned.sort();
            assert_eq!(assigned, listed, "{count} groups");
        }
    }

    #[test]
    fn the_assignment_does_not_depend_on_the_listing_order() {
        let table = table().expect("the table parses");
        let listed = listing();
        let mut reversed = listed.clone();
        reversed.reverse();
        for count in 1..=6 {
            assert_eq!(
                assign(&listed, &table, count),
                assign(&reversed, &table, count),
                "{count} groups"
            );
        }
    }

    #[test]
    fn a_scenario_the_table_lacks_counts_as_the_default_estimate() {
        let table = parse_durations("59 short\n61 long\n").expect("parses");
        // long (61) to the first group, new (60) to the second, short (59)
        // to the second as well, now the lighter.
        assert_eq!(
            assign(&names(&["long", "new", "short"]), &table, 2),
            vec![names(&["long"]), names(&["new", "short"])]
        );
    }

    #[test]
    fn the_count_grows_past_the_rounded_up_total_until_every_group_fits() {
        // 1500 seconds is three budgets rounded up, but five scenarios of
        // 300 seconds fit only one to a group.
        let table = parse_durations("300 a\n300 b\n300 c\n300 d\n300 e\n").expect("parses");
        assert_eq!(
            group_count(&names(&["a", "b", "c", "d", "e"]), &table, 1.0),
            Ok(5)
        );
    }

    #[test]
    fn the_count_is_the_rounded_up_total_when_its_groups_fit() {
        let table = parse_durations("200 a\n200 b\n200 c\n200 d\n").expect("parses");
        assert_eq!(
            group_count(&names(&["a", "b", "c", "d"]), &table, 1.0),
            Ok(2)
        );
    }

    #[test]
    fn a_slowdown_multiplies_every_estimate_against_the_budget() {
        // Unscaled, 800 seconds fit two groups. Doubled they are 1600,
        // three budgets rounded up, but three groups leave one holding
        // 400 seconds, 800 doubled, so each scenario takes a group.
        let table = parse_durations(
            "200 a
200 b
200 c
200 d
",
        )
        .expect("parses");
        assert_eq!(
            group_count(&names(&["a", "b", "c", "d"]), &table, 2.0),
            Ok(4)
        );
    }

    #[test]
    fn a_scenario_estimated_over_the_budget_fails() {
        let table = parse_durations("541 slow\n10 quick\n").expect("parses");
        assert_eq!(
            group_count(&names(&["quick", "slow"]), &table, 1.0),
            Err("slow (9.0 minutes) estimated over a group's budget of 9 minutes (the step's 11-minute timeout less a minute's margin and a minute for a failure's evidence), so no group can hold it: make it faster".to_string())
        );
    }

    #[test]
    fn the_suite_is_read_from_the_registry() {
        let registry = r#"
pub const SCENARIOS: &[ScenarioDef] = &[
    suite!(kept, Refused("a reason")),
    suite!("named", module::{setup, body, teardown}, Runs),
    // suite!(commented_out, Runs),
    suite!(only_here, Runs)
        .document(only_here::document)
        .local_only("a reason"),
    demo!(demo_kind),
    measure!("measure_kind", measure::{setup, body, teardown}),
];
"#;
        assert_eq!(
            suite_from_source(registry),
            Ok(names(&["kept", "named", SESSION_CHECK]))
        );
        assert_eq!(
            registry_entries(registry).map(|entries| entries
                .into_iter()
                .map(|entry| (entry.name, entry.group, entry.local_only))
                .collect::<Vec<_>>()),
            Ok(vec![
                ("kept".to_string(), "Suite".to_string(), false),
                ("named".to_string(), "Suite".to_string(), false),
                ("only_here".to_string(), "Suite".to_string(), true),
                ("demo_kind".to_string(), "Demo".to_string(), false),
                ("measure_kind".to_string(), "Measure".to_string(), false),
            ])
        );
        assert!(
            registry_entries(
                "pub const SCENARIOS: &[ScenarioDef] = &[\n    suite!(Not a name, Runs),\n];"
            )
            .is_err()
        );
    }
}
