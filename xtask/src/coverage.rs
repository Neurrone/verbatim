//! `cargo xtask coverage`: line coverage per test level, measured with
//! cargo-llvm-cov (`docs/tooling/coverage.md`). It runs on every host.
//!
//! With no arguments it runs the instrumented tests level by level, the
//! unit tests and then, on Windows, mockapp's tests, each through
//! cargo-nextest with the filters and one-at-a-time runs `cargo xtask ci`
//! uses, then the fault tests as `ci`'s fault step runs them (on another
//! host, the unit tests of [`LINUX_CRATES`] through `cargo test`, as `ci`
//! runs them there). Each level's profiles are
//! reported on their own: an HTML report in `target/coverage/<level>/html`
//! and an lcov file, `target/coverage/<level>.lcov`, whose paths are
//! relative to the workspace, and its line coverage is printed. A failing
//! step stops nothing: every level runs and is reported, and the command
//! fails at the end, naming the steps that failed.
//!
//! `coverage report <name> <profiles>` reports the profiles in the folder
//! `<profiles>` as level `<name>`, the same two files, for an end-to-end
//! group of a coverage run: the build is instrumented by the environment
//! `cargo llvm-cov show-env` prints, and every process writes its profile
//! into that folder. The harness's own crates are left out of the report.
//!
//! `coverage gate` is the unit level's coverage gate, which `cargo xtask
//! ci` runs on a host other than Windows (GitHub's `ci-linux` job) in place
//! of a plain test run: it runs the unit tests of [`LINUX_CRATES`]
//! instrumented, prints each platform-neutral and logic crate's line
//! coverage with test code left out, and fails when their total is below
//! [`GATE_PERCENT`].
//!
//! `coverage merge <dir>` reads the lcov files of a coverage run's levels
//! from `<dir>`, `unit.lcov`, `mockapp.lcov`, `faults.lcov` and every
//! end-to-end group's `e2e-<group>.lcov`, and writes the end-to-end level's `e2e.lcov`, all
//! levels' `merged.lcov`, `summary.txt` with each level's line coverage,
//! `e2e-only.txt`, the files with lines only the end-to-end level reaches,
//! and `unreached.txt`, the files whose lines no level reaches. A level
//! whose file is missing is named so in the summary, and the rest is made
//! from the levels that are there.
//!
//! The profile runtime writes a process's profile as it exits; the
//! outposts, the listener and mockapp's test runner end with
//! `TerminateProcess`, and write theirs first through
//! `verbatim_process::child::write_coverage_profile`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::LINUX_CRATES;
use crate::tiers::{TIERS, Tier, WORKSPACE_HACK};
#[cfg(windows)]
use crate::{ALONE_TESTS, FAULT_TESTS, SERIAL_TESTS, nextest_step, unit_test_filter};
use crate::{Step, args};

/// The cargo-llvm-cov release this command runs, the one GitHub Actions
/// installs (`.github/workflows/ci.yml`): another fails the command, so a
/// new release changes nothing until the pin is moved in both places.
pub const LLVM_COV_VERSION: &str = "0.9.1";

/// The files of the end-to-end harness, its agent and xtask, left out of
/// an end-to-end group's report: they drive Verbatim rather than being
/// Verbatim, and their own tests are in the unit level.
const HARNESS_FILES: &str = r"(crates[\\/]verbatim-(e2e|agent)|xtask)[\\/]";

/// The levels `coverage merge` reads, in their order in the summary.
const LEVELS: [&str; 4] = ["unit", "mockapp", "faults", "e2e"];

/// One level of tests: its name and the cargo steps that run its tests.
struct Level {
    name: &'static str,
    steps: Vec<Step>,
}

/// Runs the command with its arguments after `coverage`.
pub fn run(args: &[String]) -> ExitCode {
    let result = match args {
        [] => measure(),
        [verb, name, profiles] if verb == "report" => report(name, Path::new(profiles)),
        [verb, dir] if verb == "merge" => merge(Path::new(dir)),
        [verb] if verb == "gate" => gate(),
        _ => {
            eprintln!(
                "usage: cargo xtask coverage | cargo xtask coverage report <name> <profiles> | cargo xtask coverage merge <dir> | cargo xtask coverage gate"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask coverage: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The workspace's root.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask sits in the workspace")
        .to_path_buf()
}

/// Where the reports go.
fn output_dir() -> PathBuf {
    workspace().join("target").join("coverage")
}

/// cargo-llvm-cov's arguments for a test run of a level: no report, which
/// also means no clean, since [`measure`] cleans once and reports each
/// level itself.
#[cfg(windows)]
const COVERED_NEXTEST: &[&str] = &["llvm-cov", "nextest", "--no-report"];

/// The fault tests of [`FAULT_TESTS`], each target run by `cargo test`
/// with its package's `faults` feature, as `cargo xtask ci`'s fault step
/// runs it.
#[cfg(windows)]
fn fault_steps() -> Vec<Step> {
    FAULT_TESTS
        .iter()
        .map(|(package, test)| {
            (
                "fault tests",
                args(&[
                    "llvm-cov",
                    "test",
                    "--no-report",
                    "-p",
                    package,
                    "--features",
                    "faults",
                    "--test",
                    test,
                ]),
            )
        })
        .collect()
}

/// The levels on Windows: the unit tests, every test but mockapp's, and
/// mockapp's tests, each through the same nextest runs as `cargo xtask
/// ci`'s, limited to the level's package, then the fault tests, last
/// because their `faults` build replaces binaries, such as `mockapp.exe`,
/// whose file names do not change with a feature, and so would leave an
/// earlier level's profiles with no binary to match. The tests that run
/// with nothing else are all mockapp's.
#[cfg(windows)]
fn levels() -> Vec<Level> {
    let level = |scope: &str, filter: &str| format!("({scope}) & ({filter})");
    let unit = "not package(mockapp)";
    let mockapp = "package(mockapp)";
    vec![
        Level {
            name: "unit",
            steps: vec![
                nextest_step(
                    "unit tests",
                    COVERED_NEXTEST,
                    &level(unit, &unit_test_filter()),
                    false,
                ),
                nextest_step(
                    "unit tests that run one at a time",
                    COVERED_NEXTEST,
                    &level(unit, SERIAL_TESTS),
                    true,
                ),
            ],
        },
        Level {
            name: "mockapp",
            steps: vec![
                nextest_step(
                    "mockapp tests",
                    COVERED_NEXTEST,
                    &level(mockapp, &unit_test_filter()),
                    false,
                ),
                nextest_step(
                    "mockapp tests that run with nothing else",
                    COVERED_NEXTEST,
                    &level(mockapp, ALONE_TESTS),
                    false,
                ),
                nextest_step(
                    "mockapp tests that run one at a time",
                    COVERED_NEXTEST,
                    &level(mockapp, SERIAL_TESTS),
                    true,
                ),
            ],
        },
        Level {
            name: "faults",
            steps: fault_steps(),
        },
    ]
}

/// The level on another host: the unit tests of [`LINUX_CRATES`], through
/// `cargo test` as `ci` runs them there.
#[cfg(not(windows))]
fn levels() -> Vec<Level> {
    let mut test = args(&["llvm-cov", "test", "--no-report"]);
    for name in LINUX_CRATES {
        test.extend(["-p".to_owned(), (*name).to_owned()]);
    }
    vec![Level {
        name: "unit",
        steps: vec![("unit tests", test)],
    }]
}

/// Runs cargo with `cargo_args` in the workspace.
fn cargo(cargo_args: &[String]) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(cargo_args)
        .current_dir(workspace())
        .status()
        .map_err(|error| format!("could not run cargo {}: {error}", cargo_args.join(" ")))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed ({status})", cargo_args.join(" ")))
    }
}

/// Checks that cargo-llvm-cov is the pinned release and that the
/// toolchain has the LLVM tools it reads profiles with: without them
/// cargo-llvm-cov asks on the terminal whether to install them.
fn check_tools() -> Result<(), String> {
    let output = Command::new(env!("CARGO"))
        .args(["llvm-cov", "--version"])
        .output()
        .map_err(|error| format!("could not run cargo llvm-cov: {error}"))?;
    let version = String::from_utf8_lossy(&output.stdout);
    let expected = format!("cargo-llvm-cov {LLVM_COV_VERSION}");
    if !output.status.success() || version.trim() != expected {
        return Err(format!(
            "needs {expected}, found {:?}: cargo install cargo-llvm-cov --version {LLVM_COV_VERSION} --locked",
            version.trim()
        ));
    }
    let status = Command::new("rustup")
        .args(["component", "add", "llvm-tools-preview"])
        .current_dir(workspace())
        .status()
        .map_err(|error| format!("could not run rustup: {error}"))?;
    if !status.success() {
        return Err(format!(
            "rustup component add llvm-tools-preview failed ({status})"
        ));
    }
    Ok(())
}

/// `cargo xtask coverage`: every level's tests, instrumented, each level
/// reported on its own. A failing step does not stop the run: every
/// level's tests run and are reported, from whatever profiles they wrote,
/// and the command fails at the end, naming each step that failed.
fn measure() -> Result<(), String> {
    check_tools()?;
    // The workspace's own artifacts, every old profile and every old
    // report go, so a report reads only this run's binaries and profiles.
    cargo(&args(&["llvm-cov", "clean", "--workspace"]))?;
    let out = output_dir();
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    }
    let mut summary = String::new();
    let mut failures = Vec::new();
    for level in levels() {
        // A level whose old profiles cannot be removed would be reported
        // with the last level's counts, so it is not run.
        if let Err(error) = cargo(&args(&["llvm-cov", "clean", "--profraw-only"])) {
            failures.push(format!("{}: {error}", level.name));
            continue;
        }
        for (name, cargo_args) in &level.steps {
            println!("xtask coverage: {}: {name}", level.name);
            if let Err(error) = cargo(cargo_args) {
                failures.push(format!("{}: {name}: {error}", level.name));
            }
        }
        match write_reports(level.name, None) {
            Ok(line) => summary.push_str(&line),
            Err(error) => failures.push(format!("{}: the report: {error}", level.name)),
        }
    }
    print!("{summary}");
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("failed: {}", failures.join("; ")))
    }
}

/// `cargo xtask coverage report`: the profiles in `profiles` as level
/// `name`, in the environment `cargo llvm-cov show-env` printed for the
/// build, whose target directory cargo-llvm-cov reads profiles from.
fn report(name: &str, profiles: &Path) -> Result<(), String> {
    let target = std::env::var_os("CARGO_LLVM_COV_TARGET_DIR").ok_or(
        "CARGO_LLVM_COV_TARGET_DIR is not set: run this in the environment `cargo llvm-cov show-env` prints, as the build ran",
    )?;
    let target = PathBuf::from(target);
    for entry in read_dir(&target)? {
        if is_profile(&entry) {
            std::fs::remove_file(&entry)
                .map_err(|error| format!("{}: {error}", entry.display()))?;
        }
    }
    let mut copied = 0;
    for entry in read_dir(profiles)? {
        if is_profile(&entry) {
            let file_name = entry.file_name().expect("a file read from a folder");
            std::fs::copy(&entry, target.join(file_name))
                .map_err(|error| format!("{}: {error}", entry.display()))?;
            copied += 1;
        }
    }
    if copied == 0 {
        return Err(format!("{} holds no profile", profiles.display()));
    }
    println!("xtask coverage: {name}: {copied} profiles");
    print!("{}", write_reports(name, Some(HARNESS_FILES))?);
    Ok(())
}

/// The unit level's floor: the platform-neutral and logic crates' line
/// coverage, test code left out, over their total, in percent (Dickson,
/// 2026-11-04).
pub const GATE_PERCENT: u64 = 90;

/// The crates the gate measures: the platform-neutral and logic tiers
/// (`xtask/src/tiers.rs`), but the workspace-hack crate, which has no code.
pub fn gate_crates() -> Vec<&'static str> {
    TIERS
        .iter()
        .filter(|(tier, _)| matches!(tier, Tier::PlatformNeutral | Tier::Logic))
        .flat_map(|(_, crates)| crates.iter().copied())
        .filter(|name| *name != WORKSPACE_HACK)
        .collect()
}

/// `cargo xtask coverage gate`: the unit tests of [`LINUX_CRATES`],
/// instrumented, then the gate over [`gate_crates`].
fn gate() -> Result<(), String> {
    check_tools()?;
    let out = output_dir();
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let lcov = out.join("gate.lcov");
    let mut run = args(&["llvm-cov", "test", "--lcov", "--output-path"]);
    run.push(lcov.display().to_string());
    for name in LINUX_CRATES {
        run.extend(["-p".to_owned(), (*name).to_owned()]);
    }
    cargo(&run)?;
    let text =
        std::fs::read_to_string(&lcov).map_err(|error| format!("{}: {error}", lcov.display()))?;
    let root = workspace();
    let lines = relative(parse_lcov(&text)?, &root);
    let mut product = Lines::new();
    for name in gate_crates() {
        add(&mut product, &product_lines(&root, name, &lines)?);
    }
    let (summary, passed) = gate_summary(&gate_crates(), &product);
    print!("{summary}");
    if passed {
        Ok(())
    } else {
        Err(format!(
            "the platform-neutral and logic crates' unit-test line coverage is below {GATE_PERCENT}%"
        ))
    }
}

/// The gate's summary, a line per crate and the total, and whether the
/// total reaches [`GATE_PERCENT`]. `lines` holds paths relative to the
/// workspace.
pub fn gate_summary(crates: &[&str], lines: &Lines) -> (String, bool) {
    let mut summary = String::new();
    // A crate with no lines measured means the profile or the paths did
    // not match, never that the crate is covered, so it fails the gate.
    let mut unmeasured = Vec::new();
    for name in crates {
        let prefix = format!("crates/{name}/");
        let own: Lines = lines
            .iter()
            .filter(|(path, _)| path.starts_with(&prefix))
            .map(|(path, counts)| (path.clone(), counts.clone()))
            .collect();
        if own.values().map(tally).all(|(found, _)| found == 0) {
            unmeasured.push(*name);
        }
        summary.push_str(&summary_line(name, &own));
    }
    summary.push_str(&summary_line("total", lines));
    let (found, hit) = lines
        .values()
        .map(tally)
        .fold((0, 0), |(found, hit), (f, h)| (found + f, hit + h));
    let passed = unmeasured.is_empty() && hit * 100 >= GATE_PERCENT * found;
    if !unmeasured.is_empty() {
        let _ = writeln!(
            summary,
            "no lines were measured for {}",
            unmeasured.join(", ")
        );
    }
    let _ = writeln!(
        summary,
        "gate: {} (the total must be at least {GATE_PERCENT}%)",
        if passed { "passed" } else { "failed" }
    );
    (summary, passed)
}

/// Crate `name`'s lines of `lines` with its test code left out: only the
/// files of its `src` folder, but files of test modules, and in the rest
/// the lines of items marked `#[cfg(test)]`.
fn product_lines(root: &Path, name: &str, lines: &Lines) -> Result<Lines, String> {
    let prefix = format!("crates/{name}/src/");
    let mut files = Vec::new();
    crate::waits::collect_rust_files(&root.join("crates").join(name).join("src"), &mut files)?;
    let mut sources = BTreeMap::new();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|error| format!("{}: {error}", file.display()))?;
        let relative = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        sources.insert(relative, source);
    }
    let test_files = test_module_files(&sources);
    let mut product = Lines::new();
    for (path, counts) in lines {
        if !path.starts_with(&prefix) || test_files.iter().any(|test| path.starts_with(test)) {
            continue;
        }
        let test_lines = sources
            .get(path)
            .map(|source| crate::waits::cfg_test_lines(source))
            .unwrap_or_default();
        let kept = counts
            .iter()
            .filter(|(line, _)| {
                !line
                    .checked_sub(1)
                    .and_then(|index| test_lines.get(index as usize))
                    .copied()
                    .unwrap_or(false)
            })
            .map(|(line, count)| (*line, *count))
            .collect();
        product.insert(path.clone(), kept);
    }
    Ok(product)
}

/// The test modules in files of their own among `sources` (relative paths
/// with forward slashes, and contents): for each `#[cfg(test)] mod name;`,
/// the paths its module's file and folder start with, `<dir>/name.rs`,
/// `<dir>/name/`, where `<dir>` is the declaring file's folder for a
/// `lib.rs`, `main.rs` or `mod.rs`, and otherwise that folder and the
/// file's name.
pub fn test_module_files(sources: &BTreeMap<String, String>) -> Vec<String> {
    let mut tests = Vec::new();
    for (path, source) in sources {
        let (folder, file) = path.rsplit_once('/').unwrap_or(("", path));
        let dir = match file {
            "lib.rs" | "main.rs" | "mod.rs" => folder.to_owned(),
            _ => format!("{folder}/{}", file.trim_end_matches(".rs")),
        };
        let lines: Vec<&str> = source.lines().map(str::trim).collect();
        for pair in lines.windows(2) {
            if pair[0] != "#[cfg(test)]" {
                continue;
            }
            let declared = pair[1]
                .strip_prefix("pub(crate) mod ")
                .or_else(|| pair[1].strip_prefix("pub mod "))
                .or_else(|| pair[1].strip_prefix("mod "))
                .and_then(|rest| rest.strip_suffix(';'));
            if let Some(name) = declared {
                tests.push(format!("{dir}/{name}.rs"));
                tests.push(format!("{dir}/{name}/"));
            }
        }
    }
    tests
}

/// The paths in folder `dir`.
fn read_dir(dir: &Path) -> Result<Vec<PathBuf>, String> {
    std::fs::read_dir(dir)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        })
        .map_err(|error| format!("{}: {error}", dir.display()))
}

/// Whether `path` is a raw profile.
fn is_profile(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "profraw")
}

/// Writes level `name`'s HTML report and lcov file from the profiles in
/// cargo-llvm-cov's target directory, leaving out files matching
/// `ignore`, and returns its summary line.
fn write_reports(name: &str, ignore: Option<&str>) -> Result<String, String> {
    let out = output_dir();
    std::fs::create_dir_all(&out).map_err(|error| format!("{}: {error}", out.display()))?;
    let lcov = out.join(format!("{name}.lcov"));
    let html = out.join(name);
    let ignored: Vec<String> = ignore
        .map(|pattern| args(&["--ignore-filename-regex", pattern]))
        .unwrap_or_default();
    let mut lcov_args = args(&["llvm-cov", "report", "--lcov", "--output-path"]);
    lcov_args.push(lcov.display().to_string());
    lcov_args.extend(ignored.iter().cloned());
    cargo(&lcov_args)?;
    let mut html_args = args(&["llvm-cov", "report", "--html", "--output-dir"]);
    html_args.push(html.display().to_string());
    html_args.extend(ignored);
    cargo(&html_args)?;
    let text =
        std::fs::read_to_string(&lcov).map_err(|error| format!("{}: {error}", lcov.display()))?;
    let lines = relative(parse_lcov(&text)?, &workspace());
    write(&lcov, &to_lcov(&lines))?;
    println!(
        "xtask coverage: {name}: {} and {}",
        lcov.display(),
        html.join("html").join("index.html").display()
    );
    Ok(summary_line(name, &lines))
}

/// Writes `text` to `path`.
fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

/// `cargo xtask coverage merge`: the levels' lcov files in `dir`, merged.
/// A level with no file, because its job failed before writing it, is
/// named as missing in the summary, and everything else is made from the
/// levels that are there.
fn merge(dir: &Path) -> Result<(), String> {
    let read = |file_name: &str| {
        let path = dir.join(file_name);
        std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))
            .and_then(|text| parse_lcov(&text).map_err(|error| format!("{file_name}: {error}")))
    };
    let read_level = |file_name: &str| {
        if dir.join(file_name).exists() {
            read(file_name).map(Some)
        } else {
            Ok(None)
        }
    };
    let unit = read_level("unit.lcov")?;
    let mockapp = read_level("mockapp.lcov")?;
    let faults = read_level("faults.lcov")?;
    let mut groups: Vec<String> = read_dir(dir)?
        .iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
        .filter(|file_name| {
            file_name.starts_with("e2e-")
                && Path::new(file_name)
                    .extension()
                    .is_some_and(|extension| extension == "lcov")
        })
        .collect();
    groups.sort();
    let e2e = if groups.is_empty() {
        None
    } else {
        let mut e2e = Lines::new();
        for group in &groups {
            add(&mut e2e, &read(group)?);
        }
        Some(e2e)
    };
    let mut below = Lines::new();
    for lines in [&unit, &mockapp, &faults].into_iter().flatten() {
        add(&mut below, lines);
    }
    let mut merged = below.clone();
    let e2e_lines = e2e.clone().unwrap_or_default();
    add(&mut merged, &e2e_lines);
    write(&dir.join("e2e.lcov"), &to_lcov(&e2e_lines))?;
    write(&dir.join("merged.lcov"), &to_lcov(&merged))?;
    let summary = merge_summary(&[&unit, &mockapp, &faults, &e2e], &merged);
    write(&dir.join("summary.txt"), &summary)?;
    write(&dir.join("e2e-only.txt"), &e2e_only(&e2e_lines, &below))?;
    write(&dir.join("unreached.txt"), &unreached(&merged))?;
    print!("{summary}");
    println!(
        "xtask coverage: merged {} end-to-end groups into {}",
        groups.len(),
        dir.display()
    );
    Ok(())
}

/// The summary of a merge: a line for each of [`LEVELS`], in order, its
/// coverage or, for one that is `None`, that it is missing, then the
/// merged line.
pub fn merge_summary(levels: &[&Option<Lines>; 4], merged: &Lines) -> String {
    let mut summary = String::new();
    for (name, lines) in LEVELS.iter().zip(levels) {
        match lines {
            Some(lines) => summary.push_str(&summary_line(name, lines)),
            None => {
                let _ = writeln!(summary, "{name}: missing, so not in the merged level");
            }
        }
    }
    summary.push_str(&summary_line("merged", merged));
    summary
}

/// Line counts: each source file's instrumented lines, each with the
/// times it ran.
pub type Lines = BTreeMap<String, BTreeMap<u32, u64>>;

/// The line counts of an lcov file: each record's `SF:` path and its `DA:`
/// lines, a line named twice in one file summed. Every other record line
/// is ignored, since only line coverage is reported.
///
/// # Errors
///
/// Returns the first `DA:` line that is not a line number and a count, or
/// one before any `SF:` line.
pub fn parse_lcov(text: &str) -> Result<Lines, String> {
    let mut lines = Lines::new();
    let mut file: Option<String> = None;
    for (number, line) in text.lines().enumerate() {
        if let Some(path) = line.strip_prefix("SF:") {
            lines.entry(path.to_owned()).or_default();
            file = Some(path.to_owned());
        } else if let Some(data) = line.strip_prefix("DA:") {
            let malformed = || format!("line {}: {line:?}", number + 1);
            let mut fields = data.split(',');
            let line_number: u32 = fields
                .next()
                .and_then(|field| field.parse().ok())
                .ok_or_else(malformed)?;
            let count: u64 = fields
                .next()
                .and_then(|field| field.parse().ok())
                .ok_or_else(malformed)?;
            let path = file.as_ref().ok_or_else(malformed)?;
            let entry = lines
                .get_mut(path)
                .expect("added at its SF line")
                .entry(line_number)
                .or_default();
            *entry = entry.saturating_add(count);
        } else if line == "end_of_record" {
            file = None;
        }
    }
    Ok(lines)
}

/// `lines` with each path relative to `root`, with forward slashes, when
/// it is under it, compared without regard to case as Windows paths are.
pub fn relative(lines: Lines, root: &Path) -> Lines {
    let root = format!(
        "{}/",
        root.display()
            .to_string()
            .replace('\\', "/")
            .trim_end_matches('/')
    );
    let mut out = Lines::new();
    for (path, counts) in lines {
        let slashed = path.replace('\\', "/");
        let path = if slashed.len() > root.len()
            && slashed.is_char_boundary(root.len())
            && slashed[..root.len()].eq_ignore_ascii_case(&root)
        {
            slashed[root.len()..].to_owned()
        } else {
            path
        };
        add(&mut out, &BTreeMap::from([(path, counts)]));
    }
    out
}

/// Adds `more`'s counts to `lines`.
pub fn add(lines: &mut Lines, more: &Lines) {
    for (path, counts) in more {
        let file = lines.entry(path.clone()).or_default();
        for (line, count) in counts {
            let entry = file.entry(*line).or_default();
            *entry = entry.saturating_add(*count);
        }
    }
}

/// `lines` as an lcov file: per file its path, its lines' counts, and how
/// many lines it has and how many ran.
pub fn to_lcov(lines: &Lines) -> String {
    let mut text = String::new();
    for (path, counts) in lines {
        let _ = writeln!(text, "SF:{path}");
        for (line, count) in counts {
            let _ = writeln!(text, "DA:{line},{count}");
        }
        let (found, hit) = tally(counts);
        let _ = writeln!(text, "LF:{found}\nLH:{hit}\nend_of_record");
    }
    text
}

/// How many lines `counts` holds, and how many of them ran.
fn tally(counts: &BTreeMap<u32, u64>) -> (u64, u64) {
    let found = counts.len() as u64;
    let hit = counts.values().filter(|count| **count > 0).count() as u64;
    (found, hit)
}

/// Level `name`'s line coverage as a line of the summary, such as
/// `unit: 61.2% of lines (612 of 1000)`, the percentage rounded down.
pub fn summary_line(name: &str, lines: &Lines) -> String {
    let (found, hit) = lines
        .values()
        .map(tally)
        .fold((0, 0), |(found, hit), (f, h)| (found + f, hit + h));
    let tenths = (hit * 1000).checked_div(found).unwrap_or(0);
    format!(
        "{name}: {}.{}% of lines ({hit} of {found})\n",
        tenths / 10,
        tenths % 10
    )
}

/// The files with lines `e2e` reaches and `below` does not, one a line as
/// the number of those lines then the path, the most first: logic only an
/// end-to-end scenario tests.
pub fn e2e_only(e2e: &Lines, below: &Lines) -> String {
    let mut files: Vec<(usize, &String)> = e2e
        .iter()
        .map(|(path, counts)| {
            let only = counts
                .iter()
                .filter(|(line, count)| {
                    **count > 0
                        && below
                            .get(path)
                            .and_then(|lower| lower.get(line))
                            .is_none_or(|lower| *lower == 0)
                })
                .count();
            (only, path)
        })
        .filter(|(only, _)| *only > 0)
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    files
        .into_iter()
        .fold(String::new(), |mut text, (only, path)| {
            let _ = writeln!(text, "{only} {path}");
            text
        })
}

/// The files with instrumented lines of which none ran, one path a line.
pub fn unreached(lines: &Lines) -> String {
    lines
        .iter()
        .filter(|(_, counts)| !counts.is_empty() && counts.values().all(|count| *count == 0))
        .fold(String::new(), |mut text, (path, _)| {
            let _ = writeln!(text, "{path}");
            text
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(list: &[(&str, &[(u32, u64)])]) -> Lines {
        list.iter()
            .map(|(path, counts)| ((*path).to_owned(), counts.iter().copied().collect()))
            .collect()
    }

    #[test]
    fn an_lcov_file_is_read_as_each_files_line_counts() {
        let text = "TN:\nSF:a.rs\nFN:1,f\nDA:1,3\nDA:2,0\nDA:1,2\nLF:2\nLH:1\nend_of_record\nSF:b.rs\nDA:7,1,checksum\nend_of_record\n";
        assert_eq!(
            parse_lcov(text),
            Ok(lines(&[("a.rs", &[(1, 5), (2, 0)]), ("b.rs", &[(7, 1)])]))
        );
    }

    #[test]
    fn paths_under_the_root_become_relative_with_forward_slashes() {
        let read = lines(&[
            (r"D:\A\verbatim\crates\x\src\lib.rs", &[(1, 1)]),
            (r"d:\a\verbatim\crates\x\src\lib.rs", &[(1, 2)]),
            (r"C:\elsewhere\y.rs", &[(2, 0)]),
        ]);
        assert_eq!(
            relative(read, Path::new(r"D:\a\verbatim")),
            lines(&[
                (r"C:\elsewhere\y.rs", &[(2, 0)]),
                ("crates/x/src/lib.rs", &[(1, 3)]),
            ])
        );
    }

    #[test]
    fn merging_sums_counts_and_keeps_every_line() {
        let mut merged = lines(&[("a.rs", &[(1, 1), (2, 0)])]);
        add(
            &mut merged,
            &lines(&[("a.rs", &[(2, 4), (3, 0)]), ("b.rs", &[(1, 0)])]),
        );
        assert_eq!(
            merged,
            lines(&[("a.rs", &[(1, 1), (2, 4), (3, 0)]), ("b.rs", &[(1, 0)])])
        );
    }

    #[test]
    fn written_lcov_states_each_files_lines_and_totals() {
        assert_eq!(
            to_lcov(&lines(&[("a.rs", &[(1, 2), (2, 0)])])),
            "SF:a.rs\nDA:1,2\nDA:2,0\nLF:2\nLH:1\nend_of_record\n"
        );
    }

    #[test]
    fn the_summary_line_is_the_share_of_lines_that_ran() {
        let read = lines(&[("a.rs", &[(1, 2), (2, 0), (3, 1)]), ("b.rs", &[(1, 0)])]);
        assert_eq!(
            summary_line("unit", &read),
            "unit: 50.0% of lines (2 of 4)\n"
        );
        assert_eq!(
            summary_line("e2e", &Lines::new()),
            "e2e: 0.0% of lines (0 of 0)\n"
        );
    }

    #[test]
    fn a_missing_level_is_named_in_the_merge_summary() {
        let unit = Some(lines(&[("a.rs", &[(1, 1), (2, 0)])]));
        let merged = lines(&[("a.rs", &[(1, 1), (2, 0)])]);
        assert_eq!(
            merge_summary(&[&unit, &None, &None, &None], &merged),
            "unit: 50.0% of lines (1 of 2)\n\
             mockapp: missing, so not in the merged level\n\
             faults: missing, so not in the merged level\n\
             e2e: missing, so not in the merged level\n\
             merged: 50.0% of lines (1 of 2)\n"
        );
    }

    #[test]
    fn e2e_only_lists_files_by_their_lines_no_lower_level_ran() {
        let e2e = lines(&[
            ("a.rs", &[(1, 1), (2, 1), (3, 0)]),
            ("b.rs", &[(1, 1), (2, 1), (3, 1)]),
            ("c.rs", &[(1, 1)]),
        ]);
        let below = lines(&[("a.rs", &[(1, 0), (2, 5)]), ("c.rs", &[(1, 1)])]);
        assert_eq!(e2e_only(&e2e, &below), "3 b.rs\n1 a.rs\n");
    }

    #[test]
    fn the_gate_prints_each_crate_and_fails_a_total_below_the_floor() {
        let read = lines(&[
            ("crates/a/src/lib.rs", &[(1, 1), (2, 1), (3, 1), (4, 1)]),
            (
                "crates/b/src/lib.rs",
                &[(1, 1), (2, 1), (3, 1), (4, 1), (5, 0)],
            ),
        ]);
        assert_eq!(
            gate_summary(&["a", "b"], &read),
            (
                "a: 100.0% of lines (4 of 4)\n\
                 b: 80.0% of lines (4 of 5)\n\
                 total: 88.8% of lines (8 of 9)\n\
                 gate: failed (the total must be at least 90%)\n"
                    .to_owned(),
                false
            )
        );
        let read = lines(&[(
            "crates/a/src/lib.rs",
            &[
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 1),
                (6, 1),
                (7, 1),
                (8, 1),
                (9, 1),
                (10, 0),
            ],
        )]);
        assert!(gate_summary(&["a"], &read).1);
    }

    #[test]
    fn the_gate_fails_when_a_crate_has_no_lines_measured() {
        let read = lines(&[("crates/a/src/lib.rs", &[(1, 1)])]);
        assert_eq!(
            gate_summary(&["a", "b"], &read),
            (
                "a: 100.0% of lines (1 of 1)\n\
                 b: 0.0% of lines (0 of 0)\n\
                 total: 100.0% of lines (1 of 1)\n\
                 no lines were measured for b\n\
                 gate: failed (the total must be at least 90%)\n"
                    .to_owned(),
                false
            )
        );
    }

    #[test]
    fn test_modules_in_files_of_their_own_are_found_from_their_declarations() {
        let sources = BTreeMap::from([
            (
                "crates/a/src/lib.rs".to_owned(),
                "mod rows;\n#[cfg(test)]\nmod tests;\n".to_owned(),
            ),
            (
                "crates/a/src/rows.rs".to_owned(),
                "fn f() {}\n    #[cfg(test)]\n    mod generated;\n#[cfg(test)]\nmod inline {\n}\n"
                    .to_owned(),
            ),
        ]);
        assert_eq!(
            test_module_files(&sources),
            [
                "crates/a/src/tests.rs",
                "crates/a/src/tests/",
                "crates/a/src/rows/generated.rs",
                "crates/a/src/rows/generated/",
            ]
        );
    }

    #[test]
    fn every_gated_crate_is_tested_on_linux() {
        let missing: Vec<&str> = gate_crates()
            .into_iter()
            .filter(|name| !LINUX_CRATES.contains(name))
            .collect();
        assert_eq!(missing, Vec::<&str>::new());
        assert_eq!(gate_crates().len(), 12);
    }

    #[test]
    fn unreached_lists_files_none_of_whose_lines_ran() {
        let merged = lines(&[
            ("a.rs", &[(1, 0), (2, 0)]),
            ("b.rs", &[(1, 0), (2, 1)]),
            ("c.rs", &[]),
        ]);
        assert_eq!(unreached(&merged), "a.rs\n");
    }
}
