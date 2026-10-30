//! The `cargo xtask ci` check that the pure owners stay pure
//! (`docs/design/ownership-one-judge.md`, "The listener"): a module whose
//! transitions are replayed in tests may name nothing that blocks, shares,
//! spawns, reads a clock or calls out of the process.
//!
//! Each checked module may `use` only the paths its allow-list names, and
//! no identifier or path on [`FORBIDDEN`] may appear in its code, imported
//! or written out in full. The check reads text, outside comments and
//! string literals, so a re-export could hide what it looks for: a review
//! rule covers that, and the module's replay tests are the real check.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::waits::{strip_comments, strip_strings};

/// A module the check reads, and the paths it may `use`.
struct Checked {
    path: &'static str,
    allowed: &'static [&'static str],
}

/// The modules checked: the listener's read scheduler.
const CHECKED: [Checked; 1] = [Checked {
    path: "crates/verbatim-outpost/src/read_schedule.rs",
    allowed: &[
        "std::collections",
        "std::time::Duration",
        "verbatim_model",
        // The listener's data types.
        "crate::focus_read",
        "crate::protocol",
        // The module's own tests.
        "super",
    ],
}];

/// What a pure module may not name: identifiers, matched whole, and paths,
/// matched as written.
const FORBIDDEN: [&str; 15] = [
    "thread",
    "Mutex",
    "Condvar",
    "Atomic",
    "Arc",
    "mpsc",
    "Instant::now",
    "SystemTime",
    "std::fs",
    "std::io",
    "std::env",
    "std::process",
    "std::net",
    "windows::",
    "unsafe",
];

/// Runs the check from the workspace root `root`, returning the findings
/// as the error.
pub fn check(root: &Path) -> Result<(), String> {
    let mut report = String::new();
    for checked in &CHECKED {
        let file = root.join(checked.path);
        let source =
            fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        for finding in findings(&source, checked.allowed) {
            let _ = writeln!(report, "  {}:{finding}", checked.path);
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a pure module names something outside its allow-list (docs/design/ownership-one-judge.md):\n{report}"
        ))
    }
}

/// The findings in `source`, each its line number and what was found.
fn findings(source: &str, allowed: &[&str]) -> Vec<String> {
    let mut found = Vec::new();
    let code: Vec<String> = source
        .lines()
        .map(|line| strip_strings(&strip_comments(line)))
        .collect();
    for (index, line) in code.iter().enumerate() {
        for forbidden in FORBIDDEN {
            if names(line, forbidden) {
                found.push(format!("{}: names `{forbidden}`", index + 1));
            }
        }
    }
    for (line, path) in uses(&code) {
        if !allowed
            .iter()
            .any(|allowed| path == *allowed || path.starts_with(&format!("{allowed}::")))
        {
            found.push(format!("{line}: uses `{path}`"));
        }
    }
    found
}

/// Whether `code` names `forbidden`: a path, with `::`, as written; an
/// identifier as a whole word, or for `Atomic`, as the start of one.
fn names(code: &str, forbidden: &str) -> bool {
    if forbidden.contains("::") {
        return code.contains(forbidden);
    }
    code.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .any(|word| {
            if forbidden == "Atomic" {
                word.starts_with(forbidden)
            } else {
                word == forbidden
            }
        })
}

/// Each path a `use` declaration of `code`'s lines brings in, with the
/// line it starts on: `a::{b, c::d}` is `a::b` and `a::c::d`.
fn uses(code: &[String]) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < code.len() {
        let trimmed = code[index].trim_start();
        let declaration = trimmed
            .strip_prefix("pub(crate) use ")
            .or_else(|| trimmed.strip_prefix("pub use "))
            .or_else(|| trimmed.strip_prefix("use "));
        let Some(first) = declaration else {
            index += 1;
            continue;
        };
        let start = index + 1;
        let mut text = first.to_owned();
        while !text.contains(';') && index + 1 < code.len() {
            index += 1;
            text.push(' ');
            text.push_str(code[index].trim());
        }
        let text = text.split(';').next().unwrap_or_default();
        for path in expand(text) {
            found.push((start, path));
        }
        index += 1;
    }
    found
}

/// The paths of one `use` tree, each without its rename.
fn expand(tree: &str) -> Vec<String> {
    let tree = tree.trim();
    let Some(open) = tree.find('{') else {
        let path = tree.split(" as ").next().unwrap_or(tree).trim();
        return vec![path.trim_end_matches("::*").to_owned()];
    };
    let prefix = tree[..open].trim().trim_end_matches("::");
    let inner = &tree[open + 1..tree.rfind('}').unwrap_or(tree.len())];
    let mut parts = Vec::new();
    let (mut depth, mut from) = (0, 0);
    for (at, character) in inner.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&inner[from..at]);
                from = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[from..]);
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .flat_map(|part| {
            expand(part).into_iter().map(move |path| {
                if path == "self" {
                    prefix.to_owned()
                } else {
                    format!("{prefix}::{path}")
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALLOWED: [&str; 3] = [
        "std::collections",
        "std::time::Duration",
        "crate::focus_read",
    ];

    #[test]
    fn a_use_outside_the_allow_list_is_found() {
        let source =
            "use std::collections::{HashMap, VecDeque};\nuse std::time::{Duration, Instant};\n";
        assert_eq!(findings(source, &ALLOWED), ["2: uses `std::time::Instant`"]);
    }

    #[test]
    fn a_renamed_use_is_judged_by_its_path() {
        let source = "use std::fmt::Write as _;\nuse std::collections::HashMap as Map;\n";
        assert_eq!(findings(source, &ALLOWED), ["1: uses `std::fmt::Write`"]);
    }

    #[test]
    fn a_use_over_several_lines_is_read_whole() {
        let source = "use crate::focus_read::{\n    Judged,\n    Meta,\n};\nuse crate::outpost::{\n    alarm::Alarm,\n};\n";
        assert_eq!(
            findings(source, &ALLOWED),
            ["5: uses `crate::outpost::alarm::Alarm`"]
        );
    }

    #[test]
    fn a_forbidden_name_is_found_imported_or_written_in_full() {
        let source = "fn a() {\n    let now = std::time::Instant::now();\n    let shared = Arc::new(0);\n    let count = AtomicU64::new(0);\n}\n";
        assert_eq!(
            findings(source, &ALLOWED),
            [
                "2: names `Instant::now`",
                "3: names `Arc`",
                "4: names `Atomic`"
            ]
        );
    }

    #[test]
    fn comments_and_strings_and_longer_words_are_not_findings() {
        let source = "// a reading thread waits on a Mutex\nfn a() -> &'static str {\n    let threads = 2;\n    \"the thread ended\"\n}\n";
        assert_eq!(findings(source, &ALLOWED), Vec::<String>::new());
    }
}
