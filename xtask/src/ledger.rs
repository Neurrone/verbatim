//! The `cargo xtask ci` ledger of long-lived state (CLAUDE.md, "Design
//! principles": one owner and one representation per fact, and a lifecycle
//! is one state value): every field of the state each process keeps is
//! listed in `xtask/ownership-baseline/state-ledger.txt`, with its type and
//! a category saying why it is there.
//!
//! The ledger's `root <file> <Type>` lines name the roots. From each, the
//! check walks the struct and enum definitions of the root's own crate: a
//! field's type that names a struct or enum the crate defines is walked in
//! turn, the definition in the same file preferred, every one of that name
//! otherwise. The roots' fields and the fields of everything reached are
//! the state. Each is a line `Type.field: type -- category reason` under
//! a `## <file>` heading, an enum variant's field as `Enum::Variant.field`
//! and a tuple's as its index.
//!
//! The run fails on a field the ledger does not list, a field whose type
//! changed, a line for a field the roots no longer reach, and a line with
//! no category or an unknown one; [`CATEGORIES`] says what each means. A
//! `known-copy` is a field that copies a fact another record owns, the
//! debt to remove: each must be listed in
//! `xtask/ownership-baseline/known-copies.txt`, which only shrinks, so a
//! new copy fails the run, and an entry there whose field is no longer a
//! known copy fails it until the entry is deleted. `cargo xtask ownership
//! --update-baseline` rewrites the ledger from the code, keeping each
//! field's category, writing a new field with none, and deletes the known
//! copies that are gone; it never adds one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::items::{Declaration, Item, crate_sources, items, product_code, words};

/// The ledger, from the workspace root.
pub(crate) const LEDGER: &str = "xtask/ownership-baseline/state-ledger.txt";

/// The known copies allowed, from the workspace root.
pub(crate) const KNOWN_COPIES: &str = "xtask/ownership-baseline/known-copies.txt";

/// The categories a field may have, each with whether its line must give
/// a reason after it:
///
/// - `owner`: this record is the one owner of the fact the field holds.
/// - `handle`: a way to reach another owner or a resource, such as a
///   channel, a pipe, a thread's handle or a COM object, holding no fact.
/// - `cache-of-application-data`: what an application said, kept to save
///   asking it again; the application owns the fact.
/// - `request-table`: the requests in flight and who waits on each.
/// - `own-outbound-record`: what this process last sent, kept to judge
///   its next message.
/// - `configuration`: settings read at the start or pushed in, never
///   changed by this record.
/// - `ordered-snapshot`: a deliberate second version of a fact, such as the
///   committed one beside the one in flight; the reason names the other.
/// - `known-copy`: a copy of a fact another record owns; the reason names
///   the owner. Allowed only as listed in the known copies.
pub(crate) const CATEGORIES: [(&str, bool); 8] = [
    ("owner", false),
    ("handle", false),
    ("cache-of-application-data", false),
    ("request-table", false),
    ("own-outbound-record", false),
    ("configuration", false),
    ("ordered-snapshot", true),
    ("known-copy", true),
];

/// One line of the ledger for a field: its type and what comes after
/// ` -- `, the category and its reason.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    ty: String,
    category: String,
    number: usize,
}

/// A parsed ledger: the header (its lines before the first root), the
/// roots, and each file's lines by key.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    header: Vec<String>,
    roots: Vec<(String, String)>,
    lines: BTreeMap<(String, String), Line>,
}

/// Parses the ledger's text.
pub(crate) fn parse(text: &str) -> Result<Ledger, String> {
    let mut ledger = Ledger::default();
    let mut file: Option<String> = None;
    let mut in_header = true;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim_end();
        if (line.starts_with('#') && !line.starts_with("## ")) || line.is_empty() {
            if in_header {
                ledger.header.push(line.to_owned());
            }
            continue;
        }
        in_header = false;
        if let Some(rest) = line.strip_prefix("root ") {
            let Some((path, name)) = rest.trim().split_once(' ') else {
                return Err(format!(
                    "{LEDGER}:{number}: `{line}` is not `root <file> <Type>`"
                ));
            };
            ledger.roots.push((path.to_owned(), name.trim().to_owned()));
            continue;
        }
        if let Some(path) = line.strip_prefix("## ") {
            file = Some(path.trim().to_owned());
            continue;
        }
        let Some(file) = &file else {
            return Err(format!(
                "{LEDGER}:{number}: a field line before any `## <file>`"
            ));
        };
        let (left, category) = match line.split_once(" -- ") {
            Some((left, category)) => (left, category.trim().to_owned()),
            None => (line.strip_suffix(" --").unwrap_or(line), String::new()),
        };
        let Some((key, ty)) = left.split_once(": ") else {
            return Err(format!(
                "{LEDGER}:{number}: `{line}` is not `Type.field: type -- category`"
            ));
        };
        let entry = Line {
            ty: ty.trim().to_owned(),
            category,
            number,
        };
        if ledger
            .lines
            .insert((file.clone(), key.trim().to_owned()), entry)
            .is_some()
        {
            return Err(format!(
                "{LEDGER}:{number}: `{key}` is listed twice under {file}"
            ));
        }
    }
    while ledger.header.last().is_some_and(String::is_empty) {
        ledger.header.pop();
    }
    Ok(ledger)
}

/// A crate's structs and enums by name, each definition with its file.
type Definitions = BTreeMap<String, Vec<(String, Item)>>;

/// The crate a path from the root, `crates/<name>/...`, is in.
fn crate_of(path: &str) -> Option<&str> {
    path.strip_prefix("crates/")?.split('/').next()
}

/// The fields of the state the roots reach, by file and key, each with its
/// declaration, read from `sources`, every file of the roots' crates by
/// path; and a finding for each root not defined where it says.
pub(crate) fn reached(
    roots: &[(String, String)],
    sources: &BTreeMap<String, String>,
) -> (BTreeMap<(String, String), Declaration>, Vec<String>) {
    let mut definitions: BTreeMap<&str, Definitions> = BTreeMap::new();
    for (path, source) in sources {
        let Some(name) = crate_of(path) else { continue };
        for item in items(&product_code(source)) {
            definitions
                .entry(name)
                .or_default()
                .entry(item.name.clone())
                .or_default()
                .push((path.clone(), item));
        }
    }
    let mut found = BTreeMap::new();
    let mut problems = Vec::new();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::new();
    for (path, name) in roots {
        let defined = crate_of(path)
            .and_then(|krate| definitions.get(krate))
            .and_then(|crate_items| crate_items.get(name))
            .is_some_and(|candidates| candidates.iter().any(|(file, _)| file == path));
        if defined {
            queue.push_back((path.clone(), name.clone()));
        } else {
            problems.push(format!(
                "the root `{name}` is not a struct or enum of {path}: correct its `root` line in {LEDGER}"
            ));
        }
    }
    while let Some((path, name)) = queue.pop_front() {
        if !seen.insert((path.clone(), name.clone())) {
            continue;
        }
        let Some(crate_items) = crate_of(&path).and_then(|krate| definitions.get(krate)) else {
            continue;
        };
        let Some((_, item)) = crate_items
            .get(&name)
            .and_then(|candidates| candidates.iter().find(|(file, _)| *file == path))
        else {
            continue;
        };
        for field in &item.fields {
            found.insert((path.clone(), field.key()), field.clone());
            for word in words(&field.ty) {
                let Some(candidates) = crate_items.get(word) else {
                    continue;
                };
                let local: Vec<&(String, Item)> = candidates
                    .iter()
                    .filter(|(file, _)| *file == path)
                    .collect();
                let chosen = if local.is_empty() {
                    candidates.iter().collect()
                } else {
                    local
                };
                for (file, _) in chosen {
                    queue.push_back((file.clone(), word.to_owned()));
                }
            }
        }
    }
    (found, problems)
}

/// The known copies' entries, `file Type.field`, from its text.
fn parse_known(text: &str) -> BTreeSet<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
        .map(|(file, key)| (file.to_owned(), key.trim().to_owned()))
        .collect()
}

/// The findings of comparing the `ledger` and the `known` copies with the
/// `fields` the roots reach, each written as instructions to whoever reads
/// the failed run.
pub(crate) fn findings(
    ledger: &Ledger,
    known: &BTreeSet<(String, String)>,
    fields: &BTreeMap<(String, String), Declaration>,
) -> Vec<String> {
    let names: Vec<&str> = CATEGORIES.iter().map(|(name, _)| *name).collect();
    let mut found = Vec::new();
    for ((file, key), declaration) in fields {
        match ledger.lines.get(&(file.clone(), key.clone())) {
            None => found.push(format!(
                "{file}:{}: `{key}: {}` is long-lived state the ledger does not list. Before listing it, decide which record owns the fact it holds (CLAUDE.md, \"Design principles\"): if another record owns it, read it from there or pass it in where it is needed, rather than keeping a copy here; if it is a stage of a lifecycle, make it a variant of that lifecycle's one state value rather than a flag beside it. Then add the line under `## {file}` in {LEDGER} with its category ({}); `cargo xtask ownership --update-baseline` writes the line without one.",
                declaration.line,
                declaration.ty,
                names[..names.len() - 1].join(", ")
            )),
            Some(line) if line.ty != declaration.ty => found.push(format!(
                "{file}:{}: `{key}` is `{}` in the code but `{}` in {LEDGER}:{}: check that its category still holds for the new type, then correct the line.",
                declaration.line, declaration.ty, line.ty, line.number
            )),
            Some(_) => {}
        }
    }
    for ((file, key), line) in &ledger.lines {
        let at = format!("{LEDGER}:{}", line.number);
        if !fields.contains_key(&(file.clone(), key.clone())) {
            found.push(format!(
                "{at}: `{key}` is no longer a field of the state the roots reach in {file}: delete the line."
            ));
            continue;
        }
        let (category, reason) = line
            .category
            .split_once(' ')
            .map_or((line.category.as_str(), ""), |(category, reason)| {
                (category, reason.trim())
            });
        match CATEGORIES.iter().find(|(name, _)| *name == category) {
            None if category.is_empty() => found.push(format!(
                "{at}: `{key}` has no category: give it one of {}.",
                names.join(", ")
            )),
            None => found.push(format!(
                "{at}: `{key}` has the unknown category `{category}`: give it one of {}.",
                names.join(", ")
            )),
            Some((_, true)) if reason.is_empty() => found.push(format!(
                "{at}: `{key}` is `{category}` with no reason: name the {} after the category.",
                if category == "known-copy" {
                    "record that owns the fact it copies"
                } else {
                    "other version of the fact it is held beside"
                }
            )),
            Some(_) => {}
        }
        if category == "known-copy" && !known.contains(&(file.clone(), key.clone())) {
            found.push(format!(
                "{at}: `{key}` is a new known copy, and no copy may be added: a fact has one owner (CLAUDE.md, \"Design principles\"), so read it from the record that owns it or pass it in where it is needed. Only the copies {KNOWN_COPIES} lists are allowed, and that list only shrinks."
            ));
        }
    }
    for (file, key) in known {
        let still = ledger
            .lines
            .get(&(file.clone(), key.clone()))
            .is_some_and(|line| line.category.split(' ').next() == Some("known-copy"));
        if !still {
            found.push(format!(
                "{KNOWN_COPIES}: `{file} {key}` is no longer a known copy in {LEDGER}: delete the entry, since the list only shrinks."
            ));
        }
    }
    found
}

/// The ledger rewritten from `fields`: its header and roots kept, and each
/// field's line with its type from the code and its category from the
/// ledger, none for a new field.
pub(crate) fn rewrite(ledger: &Ledger, fields: &BTreeMap<(String, String), Declaration>) -> String {
    let mut text = String::new();
    for line in &ledger.header {
        let _ = writeln!(text, "{line}");
    }
    text.push('\n');
    for (path, name) in &ledger.roots {
        let _ = writeln!(text, "root {path} {name}");
    }
    let mut by_file: BTreeMap<&str, Vec<&Declaration>> = BTreeMap::new();
    for ((file, _), declaration) in fields {
        by_file.entry(file).or_default().push(declaration);
    }
    for (file, mut declarations) in by_file {
        declarations.sort_by(|a, b| (a.line, &a.owner, &a.name).cmp(&(b.line, &b.owner, &b.name)));
        let _ = write!(text, "\n## {file}\n");
        for declaration in declarations {
            let key = declaration.key();
            let category = ledger
                .lines
                .get(&(file.to_owned(), key.clone()))
                .map(|line| line.category.as_str())
                .unwrap_or_default();
            if category.is_empty() {
                let _ = writeln!(text, "{key}: {} --", declaration.ty);
            } else {
                let _ = writeln!(text, "{key}: {} -- {category}", declaration.ty);
            }
        }
    }
    text
}

/// The sources of the roots' crates under `root`.
fn root_sources(root: &Path, ledger: &Ledger) -> Result<BTreeMap<String, String>, String> {
    let crates: BTreeSet<&str> = ledger
        .roots
        .iter()
        .filter_map(|(path, _)| crate_of(path))
        .collect();
    let mut sources = BTreeMap::new();
    for name in crates {
        sources.extend(crate_sources(root, name)?);
    }
    Ok(sources)
}

/// Reads a file under `root` named by its path from it.
fn read(root: &Path, path: &str) -> Result<String, String> {
    fs::read_to_string(root.join(path)).map_err(|error| format!("{path}: {error}"))
}

/// Runs the ledger's check from the workspace root `root`, returning its
/// findings as the error.
pub fn check(root: &Path) -> Result<(), String> {
    let ledger = parse(&read(root, LEDGER)?)?;
    let known = parse_known(&read(root, KNOWN_COPIES)?);
    let (fields, mut found) = reached(&ledger.roots, &root_sources(root, &ledger)?);
    found.extend(findings(&ledger, &known, &fields));
    if found.is_empty() {
        return Ok(());
    }
    let mut report =
        String::from("the long-lived state differs from its ledger (xtask/src/ledger.rs):\n");
    for finding in found {
        let _ = writeln!(report, "  {finding}");
    }
    Err(report)
}

/// Rewrites the ledger from the code under `root`, and deletes the known
/// copies that are no longer known copies in it.
pub fn update(root: &Path) -> Result<(), String> {
    let ledger = parse(&read(root, LEDGER)?)?;
    let (fields, problems) = reached(&ledger.roots, &root_sources(root, &ledger)?);
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let text = rewrite(&ledger, &fields);
    fs::write(root.join(LEDGER), &text).map_err(|error| format!("{LEDGER}: {error}"))?;
    let rewritten = parse(&text)?;
    let known_text = read(root, KNOWN_COPIES)?;
    let mut kept = String::new();
    for line in known_text.lines() {
        let entry = line.trim().split_once(' ');
        let keep = line.trim().is_empty()
            || line.starts_with('#')
            || entry.is_some_and(|(file, key)| {
                rewritten
                    .lines
                    .get(&(file.to_owned(), key.trim().to_owned()))
                    .is_some_and(|line| line.category.split(' ').next() == Some("known-copy"))
            });
        if keep {
            let _ = writeln!(kept, "{line}");
        }
    }
    fs::write(root.join(KNOWN_COPIES), kept).map_err(|error| format!("{KNOWN_COPIES}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "pub struct State {\n    focus: Focus,\n    stage: Stage,\n    asked: bool,\n}\nstruct Focus {\n    node: Node,\n}\nenum Stage {\n    Idle,\n    Waiting { since: u64 },\n}\nstruct Unreached {\n    a: u8,\n}\n";

    const OTHER: &str = "struct Focus {\n    elsewhere: u8,\n}\nstruct Node(u32);\n";

    fn sources() -> BTreeMap<String, String> {
        [
            ("crates/core/src/state.rs", STATE),
            ("crates/core/src/other.rs", OTHER),
        ]
        .into_iter()
        .map(|(path, text)| (path.to_owned(), text.to_owned()))
        .collect()
    }

    fn roots() -> Vec<(String, String)> {
        vec![("crates/core/src/state.rs".to_owned(), "State".to_owned())]
    }

    const LEDGER_TEXT: &str = "# The ledger.\n\nroot crates/core/src/state.rs State\n\n## crates/core/src/other.rs\nNode.0: u32 -- owner\n\n## crates/core/src/state.rs\nState.focus: Focus -- owner\nState.stage: Stage -- owner\nState.asked: bool -- known-copy the stage owns it\nFocus.node: Node -- cache-of-application-data\nStage::Waiting.since: u64 -- owner\n";

    #[test]
    fn the_walk_reaches_the_roots_fields_and_what_their_types_name_in_the_same_file_first() {
        let (fields, problems) = reached(&roots(), &sources());
        assert_eq!(problems, Vec::<String>::new());
        let keys: Vec<String> = fields
            .keys()
            .map(|(file, key)| format!("{file} {key}"))
            .collect();
        assert_eq!(
            keys,
            [
                "crates/core/src/other.rs Node.0",
                "crates/core/src/state.rs Focus.node",
                "crates/core/src/state.rs Stage::Waiting.since",
                "crates/core/src/state.rs State.asked",
                "crates/core/src/state.rs State.focus",
                "crates/core/src/state.rs State.stage",
            ]
        );
    }

    #[test]
    fn a_ledger_matching_the_code_with_its_known_copy_listed_passes() {
        let (fields, _) = reached(&roots(), &sources());
        let ledger = parse(LEDGER_TEXT).unwrap();
        let known = parse_known("# Only shrinks.\ncrates/core/src/state.rs State.asked\n");
        assert_eq!(findings(&ledger, &known, &fields), Vec::<String>::new());
        assert_eq!(rewrite(&ledger, &fields), LEDGER_TEXT);
    }

    #[test]
    fn a_new_field_a_changed_type_a_gone_field_and_a_missing_category_fail() {
        let changed = STATE
            .replace(
                "    asked: bool,\n",
                "    asked: bool,\n    named: String,\n",
            )
            .replace("since: u64", "since: u32");
        let mut sources = sources();
        sources.insert("crates/core/src/state.rs".to_owned(), changed);
        let (fields, _) = reached(&roots(), &sources);
        let ledger = parse(&format!(
            "{LEDGER_TEXT}Gone.field: u8 -- owner\nState.focus2: Focus --\n"
        ))
        .unwrap();
        let known = parse_known("crates/core/src/state.rs State.asked\n");
        let found = findings(&ledger, &known, &fields);
        assert_eq!(found.len(), 4, "{found:#?}");
        assert!(found[0].starts_with(
            "crates/core/src/state.rs:12: `Stage::Waiting.since` is `u32` in the code but `u64`"
        ));
        assert!(found[1].starts_with(
            "crates/core/src/state.rs:5: `State.named: String` is long-lived state the ledger does not list."
        ));
        assert!(found[2].starts_with(
            "xtask/ownership-baseline/state-ledger.txt:14: `Gone.field` is no longer a field"
        ));
        assert!(found[3].starts_with(
            "xtask/ownership-baseline/state-ledger.txt:15: `State.focus2` is no longer a field"
        ));
        let rewritten = rewrite(&ledger, &fields);
        assert!(
            rewritten.contains("State.named: String --\n"),
            "{rewritten}"
        );
        assert!(rewritten.contains("Stage::Waiting.since: u32 -- owner\n"));
        let again = findings(&parse(&rewritten).unwrap(), &known, &fields);
        assert_eq!(
            again,
            [
                "xtask/ownership-baseline/state-ledger.txt:12: `State.named` has no category: give it one of owner, handle, cache-of-application-data, request-table, own-outbound-record, configuration, ordered-snapshot, known-copy."
            ]
        );
    }

    #[test]
    fn a_new_known_copy_fails_and_a_known_copy_that_is_gone_must_leave_the_list() {
        let (fields, _) = reached(&roots(), &sources());
        let ledger = parse(&LEDGER_TEXT.replace(
            "Focus.node: Node -- cache-of-application-data",
            "Focus.node: Node -- known-copy the listener owns it",
        ))
        .unwrap();
        let known = parse_known(
            "crates/core/src/state.rs State.asked\ncrates/core/src/state.rs State.gone\n",
        );
        let found = findings(&ledger, &known, &fields);
        assert_eq!(found.len(), 2, "{found:#?}");
        assert!(found[0].starts_with(
            "xtask/ownership-baseline/state-ledger.txt:12: `Focus.node` is a new known copy, and no copy may be added"
        ));
        assert!(found[1].starts_with(
            "xtask/ownership-baseline/known-copies.txt: `crates/core/src/state.rs State.gone` is no longer a known copy"
        ));
    }

    #[test]
    fn a_known_copy_or_an_ordered_snapshot_needs_a_reason_and_a_category_must_be_known() {
        let (fields, _) = reached(&roots(), &sources());
        let ledger = parse(
            &LEDGER_TEXT
                .replace("known-copy the stage owns it", "known-copy")
                .replace(
                    "State.stage: Stage -- owner",
                    "State.stage: Stage -- ordered-snapshot",
                )
                .replace("Node.0: u32 -- owner", "Node.0: u32 -- mine"),
        )
        .unwrap();
        let known = parse_known("crates/core/src/state.rs State.asked\n");
        let found = findings(&ledger, &known, &fields);
        assert_eq!(
            found,
            [
                "xtask/ownership-baseline/state-ledger.txt:6: `Node.0` has the unknown category `mine`: give it one of owner, handle, cache-of-application-data, request-table, own-outbound-record, configuration, ordered-snapshot, known-copy.",
                "xtask/ownership-baseline/state-ledger.txt:11: `State.asked` is `known-copy` with no reason: name the record that owns the fact it copies after the category.",
                "xtask/ownership-baseline/state-ledger.txt:10: `State.stage` is `ordered-snapshot` with no reason: name the other version of the fact it is held beside after the category.",
            ]
        );
    }

    #[test]
    fn a_root_not_defined_in_its_file_is_a_finding() {
        let roots = vec![("crates/core/src/other.rs".to_owned(), "State".to_owned())];
        let (_, problems) = reached(&roots, &sources());
        assert_eq!(
            problems,
            [
                "the root `State` is not a struct or enum of crates/core/src/other.rs: correct its `root` line in xtask/ownership-baseline/state-ledger.txt"
            ]
        );
    }
}
