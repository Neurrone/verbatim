//! Checks the copies uia-facts keeps of the main workspace's settings
//! (`uia-facts/readme.md`, "Reusing the main build"). Cargo cannot inherit
//! `[workspace.dependencies]`, `[workspace.lints]` or a profile across
//! workspaces, so uia-facts copies them; a copy that drifts would build a
//! second `windows` into the shared target directory, or fail to reuse the
//! main build. It fails when any of these differ:
//!
//! - the windows-family entries of `[workspace.dependencies]` (version,
//!   `default-features` and features);
//! - `[workspace.lints]`;
//! - `[profile.dev]`, less the main root's per-package overrides;
//! - a package of uia-facts' lock that the main lock holds at no such
//!   version and source, when the main lock names it or it is a
//!   third-party package;
//! - the top-level `env` blocks of `.github/workflows/ci.yml` and
//!   `.github/workflows/uia-facts.yml`, which rust-cache's key hashes.
//!
//! Run from `uia-facts/` by `build.ps1`.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use toml::{Table, Value};

/// The dependencies whose entries must match: the windows-rs family, which
/// the workspace-hack crate leaves out (`.config/hakari.toml`).
const WINDOWS_FAMILY: [&str; 4] = [
    "windows",
    "windows-core",
    "windows-collections",
    "windows-future",
];

fn main() -> ExitCode {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let main_root = here.join("..");
    let mut failures = Vec::new();
    match check(&here, &main_root, &mut failures) {
        Ok(()) if failures.is_empty() => {
            println!("check-copies: the copies match the main workspace");
            ExitCode::SUCCESS
        }
        Ok(()) => {
            for failure in &failures {
                eprintln!("check-copies: {failure}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("check-copies: {error}");
            ExitCode::FAILURE
        }
    }
}

fn check(here: &Path, main_root: &Path, failures: &mut Vec<String>) -> Result<(), String> {
    let ours = read_toml(&here.join("Cargo.toml"))?;
    let theirs = read_toml(&main_root.join("Cargo.toml"))?;

    let our_deps = table_at(&ours, &["workspace", "dependencies"])?;
    let their_deps = table_at(&theirs, &["workspace", "dependencies"])?;
    for name in WINDOWS_FAMILY {
        let ours = our_deps.get(name);
        let theirs = their_deps.get(name);
        for key in ["version", "default-features", "features"] {
            let a = ours.and_then(|v| v.get(key));
            let b = theirs.and_then(|v| v.get(key));
            if a != b {
                failures.push(format!(
                    "[workspace.dependencies] {name}.{key}: uia-facts has {a:?}, the main root {b:?}"
                ));
            }
        }
    }

    if table_at(&ours, &["workspace", "lints"])? != table_at(&theirs, &["workspace", "lints"])? {
        failures.push("[workspace.lints] differs from the main root's".to_owned());
    }

    let mut their_profile = table_at(&theirs, &["profile", "dev"])?.clone();
    their_profile.remove("package");
    if *table_at(&ours, &["profile", "dev"])? != their_profile {
        failures.push(
            "[profile.dev] differs from the main root's, its per-package overrides left out"
                .to_owned(),
        );
    }

    let our_lock = lock_packages(&here.join("Cargo.lock"))?;
    let their_lock = lock_packages(&main_root.join("Cargo.lock"))?;
    let their_names: BTreeSet<&str> = their_lock.iter().map(|p| p.0.as_str()).collect();
    // A package the main lock also names must be one of the main lock's
    // entries for it; the main lock may hold versions uia-facts' graph
    // does not use, which the derivation prunes.
    // A third-party package the main lock does not name would be a crate
    // the main build never built.
    for package in &our_lock {
        let named = their_names.contains(package.0.as_str());
        if (named || package.2 != "path") && !their_lock.contains(package) {
            failures.push(format!(
                "Cargo.lock: {} {} ({}) is not in the main lock",
                package.0, package.1, package.2
            ));
        }
    }

    let workflows = main_root.join(".github").join("workflows");
    let main_env = top_level_env(&workflows.join("ci.yml"))?;
    let our_env = top_level_env(&workflows.join("uia-facts.yml"))?;
    if main_env != our_env {
        failures.push(format!(
            "the top-level env of uia-facts.yml ({our_env:?}) differs from ci.yml's ({main_env:?})"
        ));
    }
    Ok(())
}

fn read_toml(path: &Path) -> Result<Table, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    text.parse::<Table>()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn table_at<'a>(table: &'a Table, keys: &[&str]) -> Result<&'a Table, String> {
    let mut current = table;
    for key in keys {
        current = current
            .get(*key)
            .and_then(Value::as_table)
            .ok_or_else(|| format!("no [{}] table", keys.join(".")))?;
    }
    Ok(current)
}

/// Every package of a lock file as (name, version, source), a path
/// package's source written as "path".
fn lock_packages(path: &Path) -> Result<BTreeSet<(String, String, String)>, String> {
    let lock = read_toml(path)?;
    let packages = lock
        .get("package")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: no packages", path.display()))?;
    packages
        .iter()
        .map(|package| {
            let field = |key: &str| package.get(key).and_then(Value::as_str);
            match (field("name"), field("version")) {
                (Some(name), Some(version)) => Ok((
                    name.to_owned(),
                    version.to_owned(),
                    field("source").unwrap_or("path").to_owned(),
                )),
                _ => Err(format!(
                    "{}: a package without a name or version",
                    path.display()
                )),
            }
        })
        .collect()
}

/// The lines of a workflow's top-level `env:` block, trimmed, or none when
/// it has no such block.
fn top_level_env(path: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("env:") {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with(|c: char| !c.is_whitespace() && c != '#') {
                break;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                lines.push(trimmed.to_owned());
            }
        }
    }
    Ok(lines)
}
