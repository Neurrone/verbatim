//! The `cargo xtask ci` ratchet on shared state (CLAUDE.md, "Design
//! principles": one owner and one representation per fact): every
//! declaration of product code whose type names an atomic, a cell, a lock
//! or a once-initialized value is counted per file, and the counts must
//! equal the committed baseline, `xtask/ownership-baseline/shared-state.txt`.
//!
//! A count above the baseline, or a file the baseline does not list, fails
//! the run, because a flag or a lock beside the record that owns a fact is
//! how a copy of that fact is usually kept. A count below it fails too,
//! until the baseline is lowered, so the ratchet only tightens. `cargo
//! xtask ownership --update-baseline` rewrites the counts, keeping each
//! entry's `#` lines.
//!
//! Product code is every crate's `src` folder but [`EXCLUDED`]'s, its
//! `#[cfg(test)]` items and test modules in files of their own left out;
//! `tests`, `benches` and `examples` folders are outside `src`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::items::{Declaration, crate_sources, items, product_code, statics, words};

/// The baseline, from the workspace root.
pub(crate) const BASELINE: &str = "xtask/ownership-baseline/shared-state.txt";

/// The workspace's crates whose code is not product code: the test
/// harness and its agent, the stand-ins tests drive (`mockapp`, the
/// simulated terminal, the test console, the capture synthesizer), the
/// fault points compiled only into tests, the developer tools (the
/// inspection CLI, the event recorder, the trace reader), and the
/// generated workspace-hack crate. `uia-facts` is a workspace of its own.
pub(crate) const EXCLUDED: [&str; 11] = [
    "mockapp",
    "verbatim-agent",
    "verbatim-e2e",
    "verbatim-term-sim",
    "verbatim-test-console",
    "verbatim-synth-capture",
    "verbatim-faults",
    "verbatim-inspect",
    "verbatim-event-record",
    "verbatim-trace-waits",
    "workspace-hack",
];

/// The type names counted as shared state, matched as whole identifiers;
/// any identifier starting `Atomic` is counted as well. `Lock` is the
/// outpost's lock (`crates/verbatim-outpost/src/outpost/lock.rs`).
const SHARED: [&str; 8] = [
    "Cell", "RefCell", "Mutex", "RwLock", "OnceLock", "OnceCell", "LazyLock", "Lock",
];

/// Whether the type `ty` names shared state.
pub(crate) fn is_shared(ty: &str) -> bool {
    words(ty).any(|word| word.starts_with("Atomic") || SHARED.contains(&word))
}

/// The declarations of shared state in `source`: fields, variants' fields
/// and statics, in line order.
pub(crate) fn shared_declarations(source: &str) -> Vec<Declaration> {
    let code = product_code(source);
    let mut found: Vec<Declaration> = items(&code)
        .into_iter()
        .flat_map(|item| item.fields)
        .chain(statics(&code))
        .filter(|declaration| is_shared(&declaration.ty))
        .collect();
    found.sort_by_key(|declaration| declaration.line);
    found
}

/// Every product file with shared state under `root`, by path, with its
/// declarations.
fn current(root: &Path) -> Result<BTreeMap<String, Vec<Declaration>>, String> {
    let mut found = BTreeMap::new();
    for name in crate_names(root)? {
        if EXCLUDED.contains(&name.as_str()) {
            continue;
        }
        for (path, source) in crate_sources(root, &name)? {
            let declarations = shared_declarations(&source);
            if !declarations.is_empty() {
                found.insert(path, declarations);
            }
        }
    }
    Ok(found)
}

/// The names of the folders under `crates`.
pub(crate) fn crate_names(root: &Path) -> Result<Vec<String>, String> {
    let folder = root.join("crates");
    let mut names = Vec::new();
    for entry in fs::read_dir(&folder).map_err(|error| format!("{}: {error}", folder.display()))? {
        let entry = entry.map_err(|error| format!("{}: {error}", folder.display()))?;
        if entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// A parsed baseline: its header, the `#` lines before the first entry and
/// the blank line after them, and each file's count with the `#` lines
/// just above it.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Baseline {
    header: Vec<String>,
    entries: BTreeMap<String, (usize, Vec<String>)>,
}

/// Parses the baseline's text; a line that is neither a comment, blank,
/// nor `path count` is an error.
pub(crate) fn parse_baseline(text: &str) -> Result<Baseline, String> {
    let mut baseline = Baseline::default();
    let mut comments = Vec::new();
    let mut in_header = true;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim_end();
        if line.starts_with('#') {
            comments.push(line.to_owned());
            continue;
        }
        if line.is_empty() {
            if in_header {
                baseline.header.append(&mut comments);
                in_header = false;
            }
            continue;
        }
        in_header = false;
        let parsed = line
            .rsplit_once(' ')
            .and_then(|(path, count)| Some((path.trim(), count.parse::<usize>().ok()?)));
        let Some((path, count)) = parsed else {
            return Err(format!(
                "{BASELINE}:{}: `{line}` is not `path count`",
                index + 1
            ));
        };
        if baseline
            .entries
            .insert(path.to_owned(), (count, std::mem::take(&mut comments)))
            .is_some()
        {
            return Err(format!(
                "{BASELINE}:{}: `{path}` is listed twice",
                index + 1
            ));
        }
    }
    if in_header {
        baseline.header.append(&mut comments);
    }
    Ok(baseline)
}

/// The baseline rewritten to `current`'s counts, each entry keeping its
/// `#` lines and the header kept.
pub(crate) fn rewrite(baseline: &Baseline, current: &BTreeMap<String, Vec<Declaration>>) -> String {
    let mut text = String::new();
    for line in &baseline.header {
        let _ = writeln!(text, "{line}");
    }
    if !baseline.header.is_empty() {
        text.push('\n');
    }
    for (path, declarations) in current {
        if let Some((_, comments)) = baseline.entries.get(path) {
            for comment in comments {
                let _ = writeln!(text, "{comment}");
            }
        }
        let _ = writeln!(text, "{path} {}", declarations.len());
    }
    text
}

/// The findings of comparing `current` with `baseline`, each written as
/// instructions to whoever reads the failed run.
pub(crate) fn findings(
    baseline: &Baseline,
    current: &BTreeMap<String, Vec<Declaration>>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (path, declarations) in current {
        let allowed = baseline.entries.get(path).map_or(0, |(count, _)| *count);
        let count = declarations.len();
        if count > allowed {
            let mut finding = format!(
                "{path} declares {count} pieces of shared state where {BASELINE} allows {allowed}; the new one is among these:\n"
            );
            for declaration in declarations {
                let _ = writeln!(
                    finding,
                    "      line {}: {}: {}",
                    declaration.line,
                    declaration.key(),
                    declaration.ty
                );
            }
            finding.push_str(
                "    A fact has one owner and one representation (CLAUDE.md, \"Design principles\"). Do not keep a flag, cell, lock or once-value beside the record that owns the fact: read the fact from its owner, pass it in the call or message that needs it, or make it a variant of the owner's one state value. A genuinely new handle, one that is itself the hand-off between threads and copies no fact, is added by raising this file's count in the baseline in the same commit, with a `# reason` line above the entry saying why the hand-off cannot be removed.",
            );
            found.push(finding);
        } else if count < allowed {
            found.push(format!(
                "{path} declares {count} pieces of shared state, fewer than the {allowed} {BASELINE} allows: lower the baseline in the same commit, so the ratchet keeps the gain (`cargo xtask ownership --update-baseline` rewrites it)."
            ));
        }
    }
    for (path, (count, _)) in &baseline.entries {
        if !current.contains_key(path) {
            found.push(format!(
                "{path} declares no shared state, or is gone, but {BASELINE} allows it {count}: remove its entry in the same commit (`cargo xtask ownership --update-baseline` rewrites it)."
            ));
        }
    }
    found
}

/// Runs the ratchet from the workspace root `root`, returning its findings
/// as the error.
pub fn check(root: &Path) -> Result<(), String> {
    let text =
        fs::read_to_string(root.join(BASELINE)).map_err(|error| format!("{BASELINE}: {error}"))?;
    let found = findings(&parse_baseline(&text)?, &current(root)?);
    if found.is_empty() {
        return Ok(());
    }
    let mut report =
        String::from("shared state changed against its baseline (xtask/src/shared_state.rs):\n");
    for finding in found {
        let _ = writeln!(report, "  {finding}");
    }
    Err(report)
}

/// Rewrites the baseline from the code under `root`.
pub fn update(root: &Path) -> Result<(), String> {
    let path = root.join(BASELINE);
    let baseline = match fs::read_to_string(&path) {
        Ok(text) => parse_baseline(&text)?,
        Err(_) => Baseline::default(),
    };
    let text = rewrite(&baseline, &current(root)?);
    fs::write(&path, text).map_err(|error| format!("{BASELINE}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_of(files: &[(&str, &str)]) -> BTreeMap<String, Vec<Declaration>> {
        files
            .iter()
            .map(|(path, source)| ((*path).to_owned(), shared_declarations(source)))
            .filter(|(_, declarations)| !declarations.is_empty())
            .collect()
    }

    #[test]
    fn atomics_cells_locks_and_once_values_are_counted_and_other_types_are_not() {
        let source = "struct Notices {\n    property_changed: AtomicBool,\n    shared: Arc<Mutex<Vec<u8>>>,\n    cell: OnceLock<u8>,\n    held: Lock<Mailbox>,\n    reads: Arc<Reads>,\n    locked: Locked,\n}\nstatic STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);\nstatic NAMES: [&str; 1] = [\"a\"];\n";
        let keys: Vec<String> = shared_declarations(source)
            .iter()
            .map(Declaration::key)
            .collect();
        assert_eq!(
            keys,
            [
                "Notices.property_changed",
                "Notices.shared",
                "Notices.cell",
                "Notices.held",
                "static STARTED"
            ]
        );
    }

    #[test]
    fn a_rise_names_the_files_declarations_and_a_fall_or_a_new_file_fails_too() {
        let baseline = parse_baseline(
            "# The counts.\n\n# The notices' hand-off.\na.rs 1\nb.rs 2\ngone.rs 1\n",
        )
        .unwrap();
        let current = current_of(&[
            (
                "a.rs",
                "struct A {\n    one: AtomicBool,\n    two: Cell<u8>,\n}\n",
            ),
            ("b.rs", "struct B {\n    one: AtomicBool,\n}\n"),
            ("new.rs", "static S: Mutex<u8> = Mutex::new(0);\n"),
        ]);
        let found = findings(&baseline, &current);
        assert_eq!(found.len(), 4, "{found:#?}");
        assert!(found[0].starts_with(
            "a.rs declares 2 pieces of shared state where xtask/ownership-baseline/shared-state.txt allows 1; the new one is among these:\n      line 2: A.one: AtomicBool\n      line 3: A.two: Cell<u8>\n    A fact has one owner"
        ));
        assert!(found[1].starts_with("b.rs declares 1 pieces of shared state, fewer than the 2"));
        assert!(found[2].starts_with(
            "new.rs declares 1 pieces of shared state where xtask/ownership-baseline/shared-state.txt allows 0"
        ));
        assert!(found[3].starts_with("gone.rs declares no shared state, or is gone"));
    }

    #[test]
    fn the_rewritten_baseline_keeps_its_header_and_each_entrys_reason() {
        let baseline =
            parse_baseline("# The counts.\n\n# The notices' hand-off.\na.rs 1\ngone.rs 1\n")
                .unwrap();
        let current = current_of(&[
            (
                "a.rs",
                "struct A {\n    one: AtomicBool,\n    two: Cell<u8>,\n}\n",
            ),
            ("c.rs", "struct C(RwLock<u8>);\n"),
        ]);
        let text = rewrite(&baseline, &current);
        assert_eq!(
            text,
            "# The counts.\n\n# The notices' hand-off.\na.rs 2\nc.rs 1\n"
        );
        assert_eq!(
            findings(&parse_baseline(&text).unwrap(), &current),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_malformed_baseline_line_is_an_error() {
        assert_eq!(
            parse_baseline("a.rs many\n"),
            Err(format!("{BASELINE}:1: `a.rs many` is not `path count`"))
        );
    }
}
