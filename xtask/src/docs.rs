//! `cargo xtask docs`: the check that the repository's Markdown documents
//! point only at things that exist, and that `docs/readme.md` indexes every
//! document under `docs/`.
//!
//! Every Markdown file of the repository is checked except those under
//! `nvda/`, `third_party/`, `.git/`, `.claude/worktrees/` (other sessions'
//! checkouts), a top-level directory whose name starts with `target`,
//! and `experiments/` (which records code as it was when each experiment
//! ran). Fenced code blocks are not read. The rules:
//!
//! 1. Every relative Markdown link, `[text](path)` or `[text](path#anchor)`,
//!    resolves to an existing file or directory, relative to the linking
//!    file (or to the repository root when the path starts with `/`). Links
//!    to `http:`, `https:` and `mailto:` addresses and bare `#anchor` links
//!    are not checked.
//! 2. Every backticked repository path exists relative to the repository
//!    root: a code span starting with one of [`REPOSITORY_ROOTS`], holding
//!    no whitespace (a span with spaces is a command, not a path) and none
//!    of `*?<>{}|` (a glob or a placeholder such as `<crate>`, not one
//!    path), with a trailing `#anchor`, `:line` or `:line-line` removed.
//!    A path that deliberately names something not there yet is followed
//!    directly by the text [`PLANNED`], as in `` `crates/verbatim-braille`
//!    (planned) ``, which exempts that one span and says so to the reader.
//! 3. `docs/readme.md` links every Markdown file under `docs/`, directly or
//!    through one of [`INDEXES`] that it links directly.
//!
//! Names are compared exactly, letter case included, as GitHub compares
//! them, although Windows' own file system would accept another case.
//!
//! Each violation is printed as `path:line: what is wrong`, and the command
//! fails if there is any.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The first components a backticked span must start with to be read as a
/// repository path by rule 2.
const REPOSITORY_ROOTS: &[&str] = &[
    "crates/",
    "docs/",
    "xtask/",
    "vm/",
    "experiments/",
    "tools/",
    "sounds/",
    "videos/",
    "extensions/",
    ".claude/",
];

/// The text that, directly after a backticked path's closing backtick,
/// marks the path as naming something deliberately absent.
const PLANNED: &str = " (planned)";

/// The documentation index every document under `docs/` is reached from.
const DOCS_INDEX: &str = "docs/readme.md";

/// The indexes `docs/readme.md` may delegate to: a document they link
/// counts as indexed when `docs/readme.md` links the index itself.
const INDEXES: &[&str] = &[
    "docs/crates/readme.md",
    "docs/parity/readme.md",
    "docs/nvda/readme.md",
    "docs/explainers/readme.md",
    "docs/tooling/readme.md",
];

/// Directories, from the repository root, whose documents are not checked.
const EXCLUDED: &[&str] = &[
    "nvda",
    "third_party",
    ".git",
    ".claude/worktrees",
    "experiments",
];

/// Runs the check on the repository this xtask was built from, printing
/// every violation.
pub fn run() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives in the workspace root");
    match check(root) {
        Ok(violations) if violations.is_empty() => {
            println!("xtask docs: no violations");
            ExitCode::SUCCESS
        }
        Ok(violations) => {
            for violation in &violations {
                println!("{violation}");
            }
            eprintln!("xtask docs: {} violations", violations.len());
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("xtask docs: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Reads every checked document under `root` and returns the violations.
fn check(root: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    collect_documents(root, "", &mut files)?;
    let mut documents = Vec::new();
    for relative in files {
        let text = fs::read_to_string(root.join(&relative))
            .map_err(|error| format!("{relative}: {error}"))?;
        documents.push((relative, text));
    }
    documents.sort();
    let mut disk = Disk {
        root: root.to_path_buf(),
        listings: HashMap::new(),
    };
    Ok(check_documents(&documents, &mut |path| disk.exists(path)))
}

/// Adds to `files` the checked Markdown files under the directory
/// `relative` of `root`, each as a path from `root` with `/` between its
/// parts.
fn collect_documents(root: &Path, relative: &str, files: &mut Vec<String>) -> Result<(), String> {
    let directory = root.join(relative);
    let entries =
        fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        let kind = entry
            .file_type()
            .map_err(|error| format!("{path}: {error}"))?;
        if kind.is_dir() {
            let excluded = EXCLUDED.contains(&path.as_str())
                || (relative.is_empty() && name.starts_with("target"));
            if !excluded {
                collect_documents(root, &path, files)?;
            }
        } else if kind.is_file()
            && Path::new(&name)
                .extension()
                .is_some_and(|extension| extension == "md")
        {
            files.push(path);
        }
    }
    Ok(())
}

/// Existence on disk, with every component's letter case compared exactly.
struct Disk {
    root: PathBuf,
    /// Each directory read so far, from the root, and the names in it; a
    /// path that is not a readable directory lists nothing.
    listings: HashMap<String, HashSet<String>>,
}

impl Disk {
    /// Whether `path`, from the root with `/` between its parts, names an
    /// existing file or directory.
    fn exists(&mut self, path: &str) -> bool {
        let mut directory = String::new();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            let names = self.listings.entry(directory.clone()).or_insert_with(|| {
                fs::read_dir(self.root.join(&directory))
                    .map(|entries| {
                        entries
                            .filter_map(Result::ok)
                            .map(|entry| entry.file_name().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default()
            });
            if !names.contains(part) {
                return false;
            }
            if !directory.is_empty() {
                directory.push('/');
            }
            directory.push_str(part);
        }
        true
    }
}

/// The three rules over `documents`, each a path from the repository root
/// with `/` between its parts and the file's text, sorted by path, with
/// `exists` saying whether a path from the root names a file or directory.
fn check_documents(
    documents: &[(String, String)],
    exists: &mut dyn FnMut(&str) -> bool,
) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, text) in documents {
        for item in scan(text) {
            match item {
                Item::Link { line, target } => {
                    let Some(relative) = link_path(&target) else {
                        continue;
                    };
                    match resolve(path, &relative) {
                        None => violations.push(format!(
                            "{path}:{line}: the link ({target}) leads outside the repository"
                        )),
                        Some(resolved) if !exists(&resolved) => violations.push(format!(
                            "{path}:{line}: the link ({target}) names {resolved}, which does not exist"
                        )),
                        Some(_) => {}
                    }
                }
                Item::Code {
                    line,
                    text: span,
                    planned,
                } => {
                    if planned {
                        continue;
                    }
                    if let Some(named) = repository_path(&span)
                        && !exists(named)
                    {
                        violations.push(format!(
                            "{path}:{line}: `{span}` names {named}, which does not exist"
                        ));
                    }
                }
            }
        }
    }
    violations.extend(unindexed(documents));
    violations
}

/// Rule 3: a violation for each document under `docs/` that
/// [`DOCS_INDEX`] does not reach.
fn unindexed(documents: &[(String, String)]) -> Vec<String> {
    let links_of = |file: &str| -> BTreeSet<String> {
        documents
            .iter()
            .find(|(path, _)| path == file)
            .map(|(path, text)| {
                scan(text)
                    .into_iter()
                    .filter_map(|item| match item {
                        Item::Link { target, .. } => link_path(&target),
                        Item::Code { .. } => None,
                    })
                    .filter_map(|relative| resolve(path, &relative))
                    .collect()
            })
            .unwrap_or_default()
    };
    let direct = links_of(DOCS_INDEX);
    let mut reached = direct.clone();
    for index in INDEXES {
        if direct.contains(*index) {
            reached.extend(links_of(index));
        }
    }
    documents
        .iter()
        .map(|(path, _)| path)
        .filter(|path| {
            path.starts_with("docs/")
                && path.as_str() != DOCS_INDEX
                && !reached.contains(path.as_str())
        })
        .map(|path| {
            format!(
                "{path}:1: {DOCS_INDEX} does not link this document, directly or through an index it links"
            )
        })
        .collect()
}

/// A link or a code span found in a document's prose.
#[derive(Debug, PartialEq)]
enum Item {
    /// A link's destination, as written between its parentheses.
    Link { line: usize, target: String },
    /// A code span's text, and whether [`PLANNED`] follows it directly.
    Code {
        line: usize,
        text: String,
        planned: bool,
    },
}

/// The links and code spans of `text`, outside fenced code blocks, each
/// with the line it starts on. A code span may continue onto later lines
/// of its paragraph; a link's destination stays on one line.
fn scan(text: &str) -> Vec<Item> {
    let prose = without_fences(text);
    let bytes = prose.as_bytes();
    let mut items = Vec::new();
    let mut line = 1;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => {
                line += 1;
                at += 1;
            }
            b'\\' => {
                // An escaped character is literal; a newline still counts.
                if bytes.get(at + 1) == Some(&b'\n') {
                    line += 1;
                }
                at += 2;
            }
            b'`' => {
                let ticks = run_length(bytes, at, b'`');
                if let Some(close) = closing_ticks(bytes, at + ticks, ticks) {
                    let span = &prose[at + ticks..close];
                    items.push(Item::Code {
                        line,
                        text: span.trim().to_owned(),
                        planned: prose[close + ticks..].starts_with(PLANNED),
                    });
                    line += span.matches('\n').count();
                    at = close + ticks;
                } else {
                    at += ticks;
                }
            }
            b']' if bytes.get(at + 1) == Some(&b'(') => {
                let start = at + 2;
                let end = prose[start..]
                    .find([')', '\n'])
                    .map(|offset| start + offset);
                match end {
                    Some(end) if bytes[end] == b')' => {
                        items.push(Item::Link {
                            line,
                            target: prose[start..end].to_owned(),
                        });
                        at = end + 1;
                    }
                    _ => at = start,
                }
            }
            _ => at += 1,
        }
    }
    items
}

/// `text` with every line of a fenced code block, fences included, left
/// empty, so line numbers are kept.
fn without_fences(text: &str) -> String {
    let mut prose = String::with_capacity(text.len());
    let mut fence: Option<(u8, usize)> = None;
    for line in text.lines() {
        let trimmed = line.trim_start().as_bytes();
        match fence {
            None => {
                fence = (*b"`~")
                    .into_iter()
                    .map(|mark| (mark, run_length(trimmed, 0, mark)))
                    .find(|&(_, length)| length >= 3);
                if fence.is_none() {
                    prose.push_str(line);
                }
            }
            Some((mark, length)) => {
                let run = run_length(trimmed, 0, mark);
                if run >= length && trimmed[run..].trim_ascii().is_empty() {
                    fence = None;
                }
            }
        }
        prose.push('\n');
    }
    prose
}

/// How many `mark` bytes in a row start at `at`.
fn run_length(bytes: &[u8], at: usize, mark: u8) -> usize {
    bytes[at..].iter().take_while(|&&byte| byte == mark).count()
}

/// Where the run of exactly `ticks` backticks that closes a code span
/// opened before `from` starts, if one comes before the paragraph ends.
fn closing_ticks(bytes: &[u8], from: usize, ticks: usize) -> Option<usize> {
    let mut at = from;
    while at < bytes.len() {
        match bytes[at] {
            b'`' => {
                let run = run_length(bytes, at, b'`');
                if run == ticks {
                    return Some(at);
                }
                at += run;
            }
            b'\n' => {
                let rest = &bytes[at + 1..];
                let next_line = rest.split(|&byte| byte == b'\n').next().unwrap_or(rest);
                if next_line.trim_ascii().is_empty() {
                    return None;
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    None
}

/// The path a link's destination names, when rule 1 checks it: without its
/// title, angle brackets, query and anchor, with `%20` read as a space.
fn link_path(target: &str) -> Option<String> {
    let target = target.trim();
    let destination = match target.strip_prefix('<') {
        Some(inside) => inside.split('>').next().unwrap_or(inside),
        None => target.split_whitespace().next().unwrap_or(""),
    };
    let lower = destination.to_ascii_lowercase();
    if destination.is_empty()
        || destination.starts_with('#')
        || ["http:", "https:", "mailto:"]
            .iter()
            .any(|scheme| lower.starts_with(scheme))
    {
        return None;
    }
    let path = destination.split(['#', '?']).next().unwrap_or(destination);
    Some(path.replace("%20", " "))
}

/// `relative`, a link's path in the document at `from`, as a path from the
/// repository root, or `None` when it climbs out of the repository.
fn resolve(from: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<&str> = if relative.starts_with('/') {
        Vec::new()
    } else {
        let mut parts: Vec<&str> = from.split('/').collect();
        parts.pop();
        parts
    };
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// The repository path a code span names, for rule 2, or `None` when the
/// span is not one.
fn repository_path(span: &str) -> Option<&str> {
    if !REPOSITORY_ROOTS.iter().any(|root| span.starts_with(root))
        || span.contains(char::is_whitespace)
        || span.contains(['*', '?', '<', '>', '{', '}', '|'])
    {
        return None;
    }
    let path = span.split('#').next().unwrap_or(span);
    Some(strip_line(path))
}

/// `path` without a trailing `:line` or `:line-line`.
fn strip_line(path: &str) -> &str {
    let Some((head, tail)) = path.rsplit_once(':') else {
        return path;
    };
    let numbers = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let is_line = match tail.split_once('-') {
        Some((first, last)) => numbers(first) && numbers(last),
        None => numbers(tail),
    };
    if is_line { head } else { path }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    /// The documents of a fixture, with `docs/readme.md` added when it is
    /// not among them, linking every document under `docs/`, so rules 1
    /// and 2 can be tested alone.
    fn fixture(documents: &[(&str, &str)]) -> Vec<(String, String)> {
        let mut owned: Vec<(String, String)> = documents
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect();
        if !owned.iter().any(|(path, _)| path == DOCS_INDEX) {
            let links: String = owned
                .iter()
                .filter_map(|(path, _)| path.strip_prefix("docs/"))
                .fold(String::new(), |mut links, path| {
                    let _ = writeln!(links, "[x]({path})");
                    links
                });
            owned.push((DOCS_INDEX.to_owned(), links));
        }
        owned.sort();
        owned
    }

    /// The check over `documents`, where the files that exist are the
    /// documents, `others`, and the directories holding any of them.
    fn run_check(documents: &[(String, String)], others: &[&str]) -> Vec<String> {
        let mut existing: HashSet<String> = HashSet::new();
        let files = documents
            .iter()
            .map(|(path, _)| path.as_str())
            .chain(others.iter().copied());
        for file in files {
            let mut prefix = String::new();
            for part in file.split('/') {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                existing.insert(prefix.clone());
            }
        }
        check_documents(documents, &mut |path| {
            path.is_empty() || existing.contains(path.trim_end_matches('/'))
        })
    }

    #[test]
    fn a_relative_link_must_resolve_from_the_linking_file() {
        let documents = fixture(&[(
            "docs/tooling/setup.md",
            "See [the guide](../vm.md#create), [missing](missing.md), [up](../../../x.md),\n\
             [web](https://example.com/a.md), [mail](mailto:a@b.c), [here](#top),\n\
             [crate](../../crates/foo/ \"title\") and [lib](/crates/foo/src/lib.rs).\n\
             ```\n[fenced](nowhere.md)\n```\n\
             and `[code](nowhere.md)`.\n",
        )]);
        assert_eq!(
            run_check(&documents, &["docs/vm.md", "crates/foo/src/lib.rs"]),
            vec![
                "docs/tooling/setup.md:1: the link (missing.md) names docs/tooling/missing.md, which does not exist"
                    .to_owned(),
                "docs/tooling/setup.md:1: the link (../../../x.md) leads outside the repository"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn a_backticked_repository_path_must_exist_unless_marked_planned() {
        let documents = fixture(&[(
            "readme.md",
            "Read `crates/foo/src/lib.rs:12`, `docs/vm.md#create`, `xtask/src/a.rs:3-9`,\n\
             `crates/gone.rs`, ``crates/also\n\
             gone.rs`` and `crates/braille` (planned); not `crates/braille`, (planned).\n\
             Not paths: `cargo test -p crates/x`, `docs/crates/<crate>.md`, `crates/*/src`,\n\
             `src/missing.rs`.\n\
             ```\n`crates/fenced.rs`\n```\n",
        )]);
        assert_eq!(
            run_check(
                &documents,
                &["crates/foo/src/lib.rs", "docs/vm.md", "xtask/src/a.rs"]
            ),
            vec![
                "readme.md:2: `crates/gone.rs` names crates/gone.rs, which does not exist"
                    .to_owned(),
                "readme.md:3: `crates/braille` names crates/braille, which does not exist"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn every_document_under_docs_must_be_reached_from_the_docs_index() {
        let documents = fixture(&[
            (
                "docs/readme.md",
                "[a](a.md), [crates](crates/readme.md), [b](b.md)\n",
            ),
            ("docs/a.md", "[deep](deep/c.md)\n"),
            ("docs/b.md", ""),
            ("docs/crates/readme.md", "[one](one.md)\n"),
            ("docs/crates/one.md", ""),
            ("docs/deep/c.md", ""),
            ("docs/parity/readme.md", "[two](two.md)\n"),
            ("docs/parity/two.md", ""),
            ("readme.md", ""),
        ]);
        assert_eq!(
            run_check(&documents, &[]),
            vec![
                "docs/deep/c.md:1: docs/readme.md does not link this document, directly or through an index it links"
                    .to_owned(),
                "docs/parity/readme.md:1: docs/readme.md does not link this document, directly or through an index it links"
                    .to_owned(),
                "docs/parity/two.md:1: docs/readme.md does not link this document, directly or through an index it links"
                    .to_owned(),
            ]
        );
    }

    #[test]
    fn the_check_on_disk_skips_excluded_directories_and_compares_letter_case() {
        let root = std::env::temp_dir().join(format!("xtask-docs-test-{}", std::process::id()));
        let write = |path: &str, text: &str| {
            let file = root.join(path);
            fs::create_dir_all(file.parent().expect("a file has a parent"))
                .expect("create the fixture's directory");
            fs::write(file, text).expect("write the fixture's file");
        };
        write("docs/readme.md", "[guide](Guide.md)\n");
        write("docs/guide.md", "");
        write("experiments/run.md", "[gone](gone.md)\n");
        write("target-x/notes.md", "[gone](gone.md)\n");
        let result = check(&root);
        fs::remove_dir_all(&root).expect("remove the fixture");
        assert_eq!(
            result,
            Ok(vec![
                "docs/readme.md:1: the link (Guide.md) names docs/Guide.md, which does not exist"
                    .to_owned(),
                "docs/guide.md:1: docs/readme.md does not link this document, directly or through an index it links"
                    .to_owned(),
            ])
        );
    }
}
