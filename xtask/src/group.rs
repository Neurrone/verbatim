//! `cargo xtask e2e-group <index>/<count>`: prints the end-to-end
//! scenarios of one of the `e2e` job's `count` groups, one name per line,
//! for the job to run with `--exact` (`docs/tooling/e2e.md`, "How CI runs
//! the suite"). The count is the one `cargo xtask e2e-plan` chose
//! (`xtask/src/plan.rs`), which the job passes as its matrix's size.
//!
//! The scenarios are the e2e test binary's own listing (`cargo test -p
//! verbatim-e2e --test e2e -- --ignored --list`) less those the registry
//! places in the `Demo` or `Measure` group or marks `local_only`. The
//! command fails, naming the difference, when that list is not the one the
//! planner read from the source, so a scenario is never dropped silently;
//! it then assigns the list as the planner does.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use verbatim_e2e::registry::{self, Group};

use crate::plan;

/// Runs the command with its arguments after `e2e-group`.
pub fn run(args: &[String]) -> ExitCode {
    let [group] = args else {
        eprintln!("usage: cargo xtask e2e-group <index>/<count>, such as 2/4");
        return ExitCode::from(2);
    };
    let (index, count) = match parse_group(group) {
        Ok(group) => group,
        Err(error) => {
            eprintln!("e2e-group: {error}");
            return ExitCode::from(2);
        }
    };
    let checked = plan::table().and_then(|table| {
        let planned = plan::read_suite()?;
        let suite: Vec<String> = list_tests()?
            .into_iter()
            .filter(|name| in_suite(name))
            .collect();
        compare(&suite, &planned)?;
        Ok((suite, table))
    });
    let (suite, table) = match checked {
        Ok(checked) => checked,
        Err(error) => {
            eprintln!("e2e-group: {error}");
            return ExitCode::FAILURE;
        }
    };
    let groups = plan::assign(&suite, &table, count);
    let group = &groups[index - 1];
    if group.is_empty() {
        eprintln!(
            "e2e-group: group {index} of {count} holds no scenario: {count} groups is more than the suite's {} scenarios fill",
            suite.len()
        );
        return ExitCode::FAILURE;
    }
    eprintln!(
        "e2e-group: group {index} of {count}: {} of {} scenarios, about {:.1} minutes by the table",
        group.len(),
        suite.len(),
        plan::total(&table, group) / 60.0
    );
    for name in group {
        println!("{name}");
    }
    ExitCode::SUCCESS
}

/// Parses `<index>/<count>`, with `1 <= index <= count`.
fn parse_group(text: &str) -> Result<(usize, usize), String> {
    let (index, count) = text
        .split_once('/')
        .ok_or_else(|| format!("{text:?} is not <index>/<count>"))?;
    let index: usize = index
        .parse()
        .map_err(|_| format!("{text:?}: the index is not a whole number"))?;
    let count: usize = count
        .parse()
        .map_err(|_| format!("{text:?}: the count is not a whole number"))?;
    if index == 0 || index > count {
        return Err(format!("{text:?}: the index must be from 1 to the count"));
    }
    Ok((index, count))
}

/// Fails, naming the names on each side alone, when the test binary's
/// suite is not the planner's.
fn compare(suite: &[String], planned: &[String]) -> Result<(), String> {
    let only_listed: Vec<&str> = suite
        .iter()
        .filter(|name| !planned.contains(name))
        .map(String::as_str)
        .collect();
    let only_planned: Vec<&str> = planned
        .iter()
        .filter(|name| !suite.contains(name))
        .map(String::as_str)
        .collect();
    if only_listed.is_empty() && only_planned.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the test binary's suite is not the one cargo xtask e2e-plan read from the source (xtask/src/plan.rs): only the binary's has [{}], only the planner's has [{}]",
        only_listed.join(", "),
        only_planned.join(", ")
    ))
}

/// The names of the e2e test binary's ignored tests, which are all of its
/// tests, from its own `--list`. Cargo's build output goes to standard
/// error as it happens.
fn list_tests() -> Result<Vec<String>, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the workspace");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "test",
            "-p",
            "verbatim-e2e",
            "--test",
            "e2e",
            "--",
            "--ignored",
            "--list",
        ])
        .current_dir(workspace)
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| format!("could not run cargo test --list: {error}"))?;
    if !output.status.success() {
        return Err(format!("cargo test --list failed: {}", output.status));
    }
    let stdout = String::from_utf8(output.stdout)
        .map_err(|error| format!("cargo test --list printed something not UTF-8: {error}"))?;
    let names: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.strip_suffix(": test"))
        .map(str::to_string)
        .collect();
    if names.is_empty() {
        return Err("the e2e test binary lists no test".to_string());
    }
    Ok(names)
}

/// Whether the test `name` is part of the CI suite by the real registry:
/// not a demonstration, not in the measure set, and not local-only. A test
/// the registry does not hold is.
fn in_suite(name: &str) -> bool {
    registry::find(name)
        .is_none_or(|def| !def.local_only && !matches!(def.group, Group::Demo | Group::Measure))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_planners_reading_of_the_registry_is_the_registry() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../crates/verbatim-e2e/src/registry.rs"
        ))
        .expect("the registry's source");
        let planned = plan::registry_entries(&source).expect("the planner reads the registry");
        let registered: Vec<plan::Entry> = registry::SCENARIOS
            .iter()
            .map(|def| plan::Entry {
                name: def.name.to_string(),
                group: format!("{:?}", def.group),
                local_only: def.local_only,
            })
            .collect();
        assert_eq!(planned, registered);
    }

    #[test]
    fn a_suite_unlike_the_planners_fails_naming_the_difference() {
        let suite = ["a", "b"].map(String::from);
        let planned = ["b", "c"].map(String::from);
        assert_eq!(
            compare(&suite, &planned),
            Err("the test binary's suite is not the one cargo xtask e2e-plan read from the source (xtask/src/plan.rs): only the binary's has [a], only the planner's has [c]".to_string())
        );
        assert_eq!(compare(&suite, &suite), Ok(()));
    }

    #[test]
    fn the_suite_leaves_out_demonstrations_measurements_and_local_only_scenarios() {
        assert!(!in_suite("demo_say_all"));
        assert!(!in_suite("measure_focus_explorer"));
        assert!(!in_suite("notepad_editing"));
        assert!(in_suite("conhost_typing"));
        assert!(in_suite("agent_reports_an_interactive_window_station"));
    }

    #[test]
    fn a_group_is_an_index_from_one_to_the_count() {
        assert_eq!(parse_group("2/4"), Ok((2, 4)));
        assert_eq!(
            parse_group("0/4"),
            Err(r#""0/4": the index must be from 1 to the count"#.to_string())
        );
        assert_eq!(
            parse_group("5/4"),
            Err(r#""5/4": the index must be from 1 to the count"#.to_string())
        );
        assert_eq!(
            parse_group("4"),
            Err(r#""4" is not <index>/<count>"#.to_string())
        );
    }
}
