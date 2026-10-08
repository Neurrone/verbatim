//! The `cargo xtask ci` check that test code waits for evidence, never for
//! time (`docs/testing.md`, "Waits are for evidence"): it fails on a fixed
//! sleep or a retry loop anywhere in test code, and lists the sleeps left in
//! production code for review.
//!
//! Test code is every file under a crate's `tests` folder, everything in the
//! end-to-end suite (`crates/verbatim-e2e`) and its agent
//! (`crates/verbatim-agent`), and every item marked `#[cfg(test)]` in any
//! crate's or the xtask's sources, found by matching its braces. A fixed
//! sleep is `thread::sleep`, `Sleep(`, PowerShell's `Start-Sleep`,
//! `park_timeout`, or `timeout /t`, wherever it appears, in strings too, so
//! a script a test writes is checked as well. A retry loop is a loop
//! variable, binding, or constant that counts retries or attempts (`for
//! attempt in`, `let mut retries`, `const MAX_ATTEMPTS`), outside comments
//! and strings.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// What a fixed sleep looks like.
const SLEEPS: [&str; 5] = [
    "thread::sleep",
    "Sleep(",
    "Start-Sleep",
    "park_timeout",
    "timeout /t",
];

/// The words a retry loop's counter is named with.
const RETRIES: [&str; 4] = ["retry", "retries", "attempt", "attempts"];

/// The keywords that bind a name: a loop variable, a binding, a constant.
const BINDERS: [&str; 4] = ["for", "let", "const", "static"];

/// Folders whose every source file is test code.
const TEST_CRATES: [&str; 2] = ["crates/verbatim-e2e", "crates/verbatim-agent"];

/// One finding: where, and what.
struct Finding {
    file: PathBuf,
    line: usize,
    text: String,
}

/// Runs the check from the workspace root `root`. Returns the findings in
/// test code as the error; prints the sleeps in production code.
pub fn check(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut files)?;
    collect_rust_files(&root.join("xtask"), &mut files)?;
    let mut in_tests = Vec::new();
    let mut in_production = Vec::new();
    for file in files {
        // This check names what it looks for, in its own tests too.
        if file.ends_with(Path::new("xtask").join("src").join("waits.rs")) {
            continue;
        }
        let source =
            fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        let relative = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        let whole_file_is_test = is_test_file(&relative);
        let test_lines = if whole_file_is_test {
            vec![true; source.lines().count()]
        } else {
            cfg_test_lines(&source)
        };
        for (index, line) in source.lines().enumerate() {
            let code = strip_comments(line);
            let is_test = test_lines.get(index).copied().unwrap_or(false);
            let finding = || Finding {
                file: relative.clone(),
                line: index + 1,
                text: line.trim().to_owned(),
            };
            let sleeps = SLEEPS.iter().any(|sleep| code.contains(sleep));
            let retries = is_test && names_a_retry(&strip_strings(&code));
            if is_test && (sleeps || retries) {
                in_tests.push(finding());
            } else if sleeps {
                in_production.push(finding());
            }
        }
    }
    if !in_production.is_empty() {
        println!("xtask ci: fixed sleeps in production code, for review:");
        for finding in &in_production {
            println!(
                "  {}:{}: {}",
                finding.file.display(),
                finding.line,
                finding.text
            );
        }
    }
    if in_tests.is_empty() {
        return Ok(());
    }
    let mut report = String::from(
        "test code must wait for evidence, not for time or by retrying (docs/testing.md):\n",
    );
    for finding in &in_tests {
        let _ = writeln!(
            report,
            "  {}:{}: {}",
            finding.file.display(),
            finding.line,
            finding.text
        );
    }
    Err(report)
}

/// Whether every line of the file at `relative` (from the workspace root)
/// is test code.
fn is_test_file(relative: &Path) -> bool {
    let path = relative.to_string_lossy().replace('\\', "/");
    TEST_CRATES
        .iter()
        .any(|folder| path.starts_with(&format!("{folder}/")))
        || path.split('/').any(|part| part == "tests")
}

/// Every `.rs` file under `folder`, skipping build output.
fn collect_rust_files(folder: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = fs::read_dir(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// For each line of `source`, whether it lies in an item marked
/// `#[cfg(test)]`: from the attribute to the brace that closes the item it
/// marks, counting braces outside comments and strings.
fn cfg_test_lines(source: &str) -> Vec<bool> {
    let lines: Vec<&str> = source.lines().collect();
    let mut marked = vec![false; lines.len()];
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim() != "#[cfg(test)]" {
            index += 1;
            continue;
        }
        let mut depth = 0i64;
        let mut opened = false;
        let mut end = index;
        while end < lines.len() {
            marked[end] = true;
            let code = strip_strings(&strip_comments(lines[end]));
            for character in code.chars() {
                match character {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            let ends_item = (opened && depth <= 0) || (!opened && code.trim_end().ends_with(';'));
            end += 1;
            if ends_item && end > index + 1 {
                break;
            }
        }
        index = end;
    }
    marked
}

/// `line` without a `//` comment, ignoring `//` inside a string.
fn strip_comments(line: &str) -> String {
    let mut in_string = false;
    let mut escaped = false;
    let characters: Vec<char> = line.chars().collect();
    for (index, &character) in characters.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }
        if character == '"' {
            in_string = true;
        } else if character == '/' && characters.get(index + 1) == Some(&'/') {
            return characters[..index].iter().collect();
        }
    }
    line.to_owned()
}

/// `code` with the contents of its string literals removed.
fn strip_strings(code: &str) -> String {
    let mut out = String::new();
    let mut in_string = false;
    let mut escaped = false;
    for character in code.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
                out.push('"');
            }
            continue;
        }
        if character == '"' {
            in_string = true;
        }
        out.push(character);
    }
    out
}

/// Whether `code` binds a name that counts retries or attempts: the name
/// after `for`, `let`, `let mut`, `const`, or `static` has a retry or an
/// attempt among its words.
fn names_a_retry(code: &str) -> bool {
    let words: Vec<&str> = code
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .filter(|word| !word.is_empty())
        .collect();
    words.iter().enumerate().any(|(index, word)| {
        if !BINDERS.contains(word) {
            return false;
        }
        let name = match words.get(index + 1) {
            Some(&"mut") => words.get(index + 2),
            other => other,
        };
        name.is_some_and(|name| {
            name.split('_')
                .flat_map(split_camel)
                .any(|part| RETRIES.contains(&part.to_ascii_lowercase().as_str()))
        })
    })
}

/// The words of an identifier written in camel case, as well as itself
/// when it has none; snake case is already split at its underscores.
fn split_camel(word: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    for character in word.chars() {
        if character.is_ascii_uppercase()
            && current
                .chars()
                .last()
                .is_some_and(|last| last.is_ascii_lowercase())
        {
            words.push(std::mem::take(&mut current));
        }
        current.push(character);
    }
    words.push(current);
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cfg_test_module_is_test_code_to_its_closing_brace() {
        let source = "fn production() {}\n#[cfg(test)]\nmod tests {\n    fn a() {\n    }\n}\nfn after() {}\n";
        assert_eq!(
            cfg_test_lines(source),
            [false, true, true, true, true, true, false]
        );
    }

    #[test]
    fn retries_are_found_in_identifiers_not_in_comments_or_strings() {
        assert!(names_a_retry(&strip_strings(&strip_comments(
            "for attempt in 0..3 {"
        ))));
        assert!(names_a_retry("let mut retries = 0;"));
        assert!(names_a_retry("const MAX_ATTEMPTS: u32 = 3;"));
        assert!(!names_a_retry(
            "fn a_failed_enumeration_presents_nothing_and_allows_a_retry() {"
        ));
        assert!(!names_a_retry(&strip_strings(&strip_comments(
            "// a retry would be wrong"
        ))));
        assert!(!names_a_retry(&strip_strings(
            "panic!(\"retry with --port\");"
        )));
    }

    #[test]
    fn a_sleep_in_a_script_a_test_writes_is_found() {
        let code = strip_comments("const SCRIPT: &str = \"Start-Sleep 1\"; // nothing");
        assert!(SLEEPS.iter().any(|sleep| code.contains(sleep)));
    }

    #[test]
    fn the_end_to_end_suite_and_its_agent_are_test_code() {
        assert!(is_test_file(Path::new("crates/verbatim-e2e/src/speech.rs")));
        assert!(is_test_file(Path::new("crates/verbatim-agent/src/wait.rs")));
        assert!(is_test_file(Path::new("crates/mockapp/tests/events.rs")));
        assert!(!is_test_file(Path::new("crates/verbatim-app/src/main.rs")));
    }
}
