//! The suite's test binary's command line (`tests/e2e.rs`), which is
//! libtest's: `libtest-mimic` parses it, but takes one filter only, where
//! libtest takes any number, so the binary takes the filters out first
//! ([`split_filters`]) and selects its tests by them itself
//! ([`selected`]), with libtest's meaning. A filter given with `--exact`
//! that names no test is a misspelt scenario ([`unmatched`]), and fails
//! the run rather than passing with nothing run.

/// The flags of libtest's command line, as `libtest-mimic` parses it,
/// that take their value as the next argument.
const TAKES_A_VALUE: &[&str] = &[
    "--test-threads",
    "--logfile",
    "--skip",
    "--color",
    "--format",
    "-Z",
];

/// `args`, the command line after the program's name, as its filters,
/// every positional argument in order, and the rest, for `libtest-mimic`
/// to parse. A flag that takes a value keeps the argument after it, unless
/// it is written `--flag=value`; after a lone `--`, every argument is a
/// filter.
#[must_use]
pub fn split_filters(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut filters = Vec::new();
    let mut rest = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            filters.extend(args.by_ref().cloned());
        } else if TAKES_A_VALUE.contains(&arg.as_str()) {
            rest.push(arg.clone());
            rest.extend(args.next().cloned());
        } else if arg.starts_with('-') {
            rest.push(arg.clone());
        } else {
            filters.push(arg.clone());
        }
    }
    (filters, rest)
}

/// Whether libtest runs the test `name` for `filters`: every test when
/// there are none, else each that any filter names, as its whole name with
/// `--exact` (`exact`), as a part of it without.
#[must_use]
pub fn selected(name: &str, filters: &[String], exact: bool) -> bool {
    filters.is_empty()
        || filters.iter().any(|filter| {
            if exact {
                name == filter
            } else {
                name.contains(filter.as_str())
            }
        })
}

/// The filters that name none of `names`, when they are exact names
/// (`exact`); without `--exact` a filter is a part of a name, as a plain
/// `cargo test` hands a unit test's name to this binary, and none is
/// unmatched.
#[must_use]
pub fn unmatched<'a>(names: &[&str], filters: &'a [String], exact: bool) -> Vec<&'a str> {
    if !exact {
        return Vec::new();
    }
    filters
        .iter()
        .filter(|filter| !names.contains(&filter.as_str()))
        .map(String::as_str)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|&arg| arg.to_owned()).collect()
    }

    /// How a grouped run names its scenarios: several exact names among
    /// the flags, the skips keeping their values.
    #[test]
    fn every_positional_argument_is_a_filter_and_each_flag_keeps_its_value() {
        let args = strings(&[
            "--ignored",
            "--exact",
            "conhost_typing",
            "--skip",
            "demo_",
            "notepad_editing",
            "--skip=measure_",
            "--test-threads",
            "1",
            "text_box_editing",
        ]);
        assert_eq!(
            split_filters(&args),
            (
                strings(&["conhost_typing", "notepad_editing", "text_box_editing"]),
                strings(&[
                    "--ignored",
                    "--exact",
                    "--skip",
                    "demo_",
                    "--skip=measure_",
                    "--test-threads",
                    "1",
                ])
            )
        );
        assert_eq!(
            split_filters(&strings(&["--list", "--", "--odd", "name"])),
            (strings(&["--odd", "name"]), strings(&["--list"]))
        );
    }

    #[test]
    fn a_test_runs_when_any_filter_names_it_whole_with_exact_or_in_part_without() {
        let filters = strings(&["conhost_typing", "settings_search_suggestions"]);
        assert!(selected("conhost_typing", &filters, true));
        assert!(selected("settings_search_suggestions", &filters, true));
        assert!(!selected(
            "settings_search_suggestions_reentered",
            &filters,
            true
        ));
        assert!(selected(
            "settings_search_suggestions_reentered",
            &filters,
            false
        ));
        assert!(!selected("notepad_typing", &filters, false));
        assert!(selected("notepad_typing", &[], true));
    }

    #[test]
    fn an_exact_filter_naming_no_test_is_unmatched() {
        let names = ["conhost_typing", "notepad_typing"];
        let filters = strings(&["conhost_typing", "conhost_typng", "notepad"]);
        assert_eq!(
            unmatched(&names, &filters, true),
            ["conhost_typng", "notepad"]
        );
        assert_eq!(unmatched(&names, &filters, false), Vec::<&str>::new());
        assert_eq!(unmatched(&names, &[], true), Vec::<&str>::new());
    }
}
