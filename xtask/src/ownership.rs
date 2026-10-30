//! The `cargo xtask ci` checks that the owners keep their rules, by two
//! rules over named modules:
//!
//! - The pure rule (`docs/design/ownership-one-judge.md`, "The listener"):
//!   a module whose transitions are replayed in tests may name nothing that
//!   blocks, shares, spawns, reads a clock or calls out of the process.
//! - The hook rule (`docs/design/ownership-input.md`, "The ownership check
//!   on the hook"): the modules whose code runs on the keyboard hook's
//!   thread may name nothing that takes a lock, waits, logs, does I/O or
//!   dispatches sent messages. Their test modules, which collect reports
//!   under a lock, are skipped.
//!
//! Each checked module may `use` only the paths its allow-list names, and
//! no identifier or path on its rule's forbidden list may appear in its
//! code, imported or written out in full. The check reads text, outside
//! comments and string literals, so a re-export could hide what it looks
//! for, and a type allowed in could offer a method that locks: a review
//! rule covers that, and the modules' own tests are the real check.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::waits::{strip_comments, strip_strings};

/// A module the check reads, the paths it may `use`, what it may not name,
/// and whether its `#[cfg(test)]` module is read.
struct Checked {
    path: &'static str,
    allowed: &'static [&'static str],
    forbidden: &'static [&'static str],
    skip_tests: bool,
}

/// The modules checked: the listener's read scheduler, by the pure rule,
/// and the hook thread's modules, by the hook rule.
const CHECKED: [Checked; 4] = [
    Checked {
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
        forbidden: &FORBIDDEN,
        skip_tests: false,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/typed.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
    },
    Checked {
        path: "crates/verbatim-app/src/key_reports.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
    },
];

/// What the hook thread's modules may `use`.
const HOOK_ALLOWED: [&str; 13] = [
    "std::cell",
    "std::collections",
    "std::sync::Arc",
    "std::sync::atomic",
    "std::time::Instant",
    "windows::Win32",
    "verbatim_input",
    "verbatim_model",
    "verbatim_process::clock",
    "verbatim_process::port_queue::Poster",
    // Its cancel posts to the speech queue's completion port.
    "verbatim_speech::SpeechControl",
    "crate",
    "super",
];

/// What the hook thread's modules may not name: identifiers, matched whole,
/// and paths, matched as written.
const HOOK_FORBIDDEN: [&str; 16] = [
    "Mutex",
    "RwLock",
    "Condvar",
    "OnceLock",
    "LazyLock",
    "lock",
    "mpsc",
    "sleep",
    "std::thread",
    "std::fs",
    "std::io",
    "std::net",
    "std::process",
    "tracing",
    "SendMessageW",
    "SendMessageTimeoutW",
];

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
        for finding in findings(&source, checked) {
            let _ = writeln!(report, "  {}:{finding}", checked.path);
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a checked module names something outside its rule (the pure rule, docs/design/ownership-one-judge.md; the hook rule, docs/design/ownership-input.md):\n{report}"
        ))
    }
}

/// The findings in `source`, checked as `checked` says, each its line
/// number and what was found.
fn findings(source: &str, checked: &Checked) -> Vec<String> {
    let mut found = Vec::new();
    let mut code: Vec<String> = source
        .lines()
        .map(|line| strip_strings(&strip_comments(line)))
        .collect();
    if checked.skip_tests {
        blank_test_modules(&mut code);
    }
    let allowed = checked.allowed;
    for (index, line) in code.iter().enumerate() {
        for &forbidden in checked.forbidden {
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

/// Blanks each `#[cfg(test)]` module of `code`, from the attribute to the
/// brace that closes the `mod` after it, counting the braces of code
/// already stripped of comments and strings.
fn blank_test_modules(code: &mut [String]) {
    let mut index = 0;
    while index < code.len() {
        if code[index].trim() != "#[cfg(test)]" {
            index += 1;
            continue;
        }
        let start = index;
        // The `mod` the attribute is on, then its braces to the close.
        while index < code.len() && !code[index].split_whitespace().any(|word| word == "mod") {
            index += 1;
        }
        let mut depth = 0_i32;
        let mut opened = false;
        while index < code.len() {
            for character in code[index].chars() {
                match character {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            index += 1;
            if opened && depth <= 0 {
                break;
            }
        }
        for line in &mut code[start..index] {
            line.clear();
        }
    }
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

    /// A module checked by the pure rule, allowed [`ALLOWED`].
    const PURE: Checked = Checked {
        path: "pure.rs",
        allowed: &ALLOWED,
        forbidden: &FORBIDDEN,
        skip_tests: false,
    };

    /// A module checked by the hook rule.
    const HOOK: Checked = Checked {
        path: "hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
    };

    #[test]
    fn a_lock_in_a_hook_module_is_found() {
        let source = "fn decide(state: &Mutex<u8>) {\n    let held = state.lock();\n}\n";
        assert_eq!(
            findings(source, &HOOK),
            ["1: names `Mutex`", "2: names `lock`"]
        );
    }

    #[test]
    fn a_hook_modules_test_module_is_skipped_to_its_closing_brace() {
        let source = "fn decide() {}\n#[cfg(test)]\nmod tests {\n    use std::sync::Mutex;\n    fn sink() {\n        let _ = Mutex::new(0);\n    }\n}\nfn after() { tracing::info!(\"\"); }\n";
        assert_eq!(findings(source, &HOOK), ["9: names `tracing`"]);
    }

    #[test]
    fn a_use_of_a_lock_is_found_through_the_hook_allow_list() {
        let source = "use std::sync::{Arc, Mutex};\nuse std::collections::HashSet;\n";
        assert_eq!(
            findings(source, &HOOK),
            ["1: names `Mutex`", "1: uses `std::sync::Mutex`"]
        );
    }

    #[test]
    fn a_use_outside_the_allow_list_is_found() {
        let source =
            "use std::collections::{HashMap, VecDeque};\nuse std::time::{Duration, Instant};\n";
        assert_eq!(findings(source, &PURE), ["2: uses `std::time::Instant`"]);
    }

    #[test]
    fn a_renamed_use_is_judged_by_its_path() {
        let source = "use std::fmt::Write as _;\nuse std::collections::HashMap as Map;\n";
        assert_eq!(findings(source, &PURE), ["1: uses `std::fmt::Write`"]);
    }

    #[test]
    fn a_use_over_several_lines_is_read_whole() {
        let source = "use crate::focus_read::{\n    Judged,\n    Meta,\n};\nuse crate::outpost::{\n    alarm::Alarm,\n};\n";
        assert_eq!(
            findings(source, &PURE),
            ["5: uses `crate::outpost::alarm::Alarm`"]
        );
    }

    #[test]
    fn a_forbidden_name_is_found_imported_or_written_in_full() {
        let source = "fn a() {\n    let now = std::time::Instant::now();\n    let shared = Arc::new(0);\n    let count = AtomicU64::new(0);\n}\n";
        assert_eq!(
            findings(source, &PURE),
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
        assert_eq!(findings(source, &PURE), Vec::<String>::new());
    }
}
