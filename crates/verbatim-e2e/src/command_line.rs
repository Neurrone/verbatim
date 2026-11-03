//! The suite's test binary's command line (`tests/e2e.rs`), which is
//! libtest's: `libtest-mimic` parses it, but takes one filter only, where
//! libtest takes any number, so the binary takes the filters out first
//! ([`split_filters`]) and selects its tests by them itself
//! ([`selected`]), with libtest's meaning.

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
}
