//! The `cargo xtask ci` check that every cached read of a UIA element goes
//! through a shape (`docs/design/uia-shapes.md`, section 3.5, item 7).
//!
//! Inside `verbatim-uia` the compiler confines Verbatim's own cached
//! readers to the shape module, but `Live` hands out the raw element, whose
//! own cached getters the `windows` crate lets anyone call. So this check
//! fails on, outside comments and strings:
//!
//! - a raw cached read, `GetCached` followed by anything (such as
//!   `GetCachedPropertyValue` or `GetCachedPatternAs`) or a `.Cached` getter
//!   (such as `.CachedName(`), anywhere but the shape module
//!   (`crates/verbatim-uia/src/shape/`), and `GetCachedChildren` in
//!   `crates/verbatim-uia/src/client.rs`, where `Uia::children` reads a
//!   parent's cached children;
//! - a raw cache refill, `build_updated_cache` or `BuildUpdatedCache`,
//!   outside `verbatim-uia`: a refilled raw element can no longer become a
//!   `Cached<S>`, but invites a raw read, and `Live::refresh` is the way to
//!   refill into a shape;
//! - `.claim(`, the call that wraps a remote program's element as a
//!   `Cached<S>` (`Fill::claim`), outside `verbatim-uia-rops`, whose
//!   programs fill caches from the token's list.
//!
//! Every Rust file under `crates` and `xtask` is checked, tests included.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crate::waits::{collect_rust_files, strip_comments, strip_strings};

/// Where raw cached reads may be made.
const SHAPE_MODULE: &str = "crates/verbatim-uia/src/shape/";

/// The file whose children read may read a parent's cached children.
const CHILDREN_READ: &str = "crates/verbatim-uia/src/client.rs";

/// Where a raw cache refill may be made.
const UIA_CRATE: &str = "crates/verbatim-uia/";

/// Where a remote program's element may be claimed as a shape.
const ROPS_CRATE: &str = "crates/verbatim-uia-rops/";

/// One finding: where, and what.
struct Finding {
    file: String,
    line: usize,
    text: String,
}

/// Runs the check from the workspace root `root`. Returns the findings as
/// the error.
pub fn check(root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("crates"), &mut files)?;
    collect_rust_files(&root.join("xtask"), &mut files)?;
    let mut sources = Vec::new();
    for file in files {
        // This check names what it looks for, in its own tests too.
        if file.ends_with(Path::new("xtask").join("src").join("shapes.rs")) {
            continue;
        }
        let source =
            fs::read_to_string(&file).map_err(|error| format!("{}: {error}", file.display()))?;
        let relative: PathBuf = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
        sources.push((relative.to_string_lossy().replace('\\', "/"), source));
    }
    check_sources(&sources)
}

/// The check over `sources`, each a path from the workspace root, with `/`
/// between its parts, and the file's text.
fn check_sources(sources: &[(String, String)]) -> Result<(), String> {
    let mut findings = Vec::new();
    for (path, source) in sources {
        for (index, line) in source.lines().enumerate() {
            let code = strip_strings(&strip_comments(line));
            if let Some(rule) = broken_rule(path, &code) {
                findings.push((
                    rule,
                    Finding {
                        file: path.clone(),
                        line: index + 1,
                        text: line.trim().to_owned(),
                    },
                ));
            }
        }
    }
    if findings.is_empty() {
        return Ok(());
    }
    let mut report = String::from(
        "UIA caches are read through shapes alone (docs/design/uia-shapes.md, section 3.5):\n",
    );
    for (rule, finding) in &findings {
        let _ = writeln!(
            report,
            "  {}:{}: {rule}: {}",
            finding.file, finding.line, finding.text
        );
    }
    Err(report)
}

/// The rule `code`, a line of the file at `path` without its comments and
/// strings, breaks, if any.
fn broken_rule(path: &str, code: &str) -> Option<&'static str> {
    let in_shape_module = path.starts_with(SHAPE_MODULE);
    let reads = cached_reads(code);
    let allowed =
        |read: &str| in_shape_module || (path == CHILDREN_READ && read == "GetCachedChildren");
    if reads.iter().any(|read| !allowed(read)) {
        return Some("a cached read outside the shape module");
    }
    if !path.starts_with(UIA_CRATE)
        && (code.contains("build_updated_cache") || code.contains("BuildUpdatedCache"))
    {
        return Some("a raw cache refill outside verbatim-uia");
    }
    if !path.starts_with(ROPS_CRATE) && code.contains(".claim(") {
        return Some("a remote program's element claimed outside verbatim-uia-rops");
    }
    None
}

/// The raw cached reads in `code`: each identifier that starts with
/// `GetCached`, and each method called after a dot whose name is `Cached`
/// followed by a capital letter.
fn cached_reads(code: &str) -> Vec<String> {
    let characters: Vec<char> = code.chars().collect();
    let is_ident = |character: char| character.is_ascii_alphanumeric() || character == '_';
    let mut reads = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        if !is_ident(characters[index]) || (index > 0 && is_ident(characters[index - 1])) {
            index += 1;
            continue;
        }
        let start = index;
        while index < characters.len() && is_ident(characters[index]) {
            index += 1;
        }
        let word: String = characters[start..index].iter().collect();
        let after_dot = start > 0 && characters[start - 1] == '.';
        let called = characters.get(index) == Some(&'(');
        let cached_getter = after_dot
            && called
            && word
                .strip_prefix("Cached")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|first| first.is_ascii_uppercase());
        if word.starts_with("GetCached") || cached_getter {
            reads.push(word);
        }
    }
    reads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cached_read_outside_the_shape_module_fails_the_check() {
        let read =
            "    let name = unsafe { element.GetCachedPropertyValue(UIA_NamePropertyId) };\n";
        let getter = "fn f() {\n    let name = unsafe { element.CachedName() };\n}\n";
        let outside = [
            (
                "crates/verbatim-outpost/src/outpost/read.rs".to_owned(),
                read.to_owned(),
            ),
            (
                "crates/mockapp/tests/events.rs".to_owned(),
                getter.to_owned(),
            ),
        ];
        assert_eq!(
            check_sources(&outside),
            Err(format!(
                "UIA caches are read through shapes alone (docs/design/uia-shapes.md, section 3.5):\n  \
                 crates/verbatim-outpost/src/outpost/read.rs:1: a cached read outside the shape module: {}\n  \
                 crates/mockapp/tests/events.rs:2: a cached read outside the shape module: {}\n",
                read.trim(),
                getter.lines().nth(1).expect("the getter's line").trim(),
            ))
        );
        let inside = [(
            "crates/verbatim-uia/src/shape/mod.rs".to_owned(),
            read.to_owned(),
        )];
        assert_eq!(check_sources(&inside), Ok(()));
    }
}
