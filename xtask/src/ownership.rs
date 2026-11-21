//! The `cargo xtask ci` checks that the owners keep their rules, by two
//! rules over named modules:
//!
//! - The pure rule (`books/dev/src/explanation/design/ownership.md`):
//!   a module whose transitions are replayed in tests may name nothing that
//!   blocks, shares, spawns, reads a clock or calls out of the process.
//!   The outpost's reducer is held to it as a directory, every file in it
//!   (`books/dev/src/explanation/design/ownership.md`),
//!   with an allow-list of its own: the module's own items, the model,
//!   Core's focus rules, `std`'s ordered collections and `VecDeque`,
//!   `cmp`, `mem`, `Arc` and `Duration`; its `#[cfg(test)]` modules, inline or in files of their
//!   own, are skipped.
//! - The hook rule (`books/dev/src/explanation/design/ownership.md`): the modules whose code runs on the keyboard hook's
//!   thread may name nothing that takes a lock, waits, logs, does I/O or
//!   dispatches sent messages, and nothing that would have the thread take
//!   work from another thread as a message: the hook thread takes only
//!   keys. Their test modules, which collect reports under a lock, are
//!   skipped. The hook thread's start-up in `lib.rs`, `hook_thread`, is
//!   read alone by the same rule, but may report on a channel and log once
//!   before the loop, and may not run a message loop of its own.
//! - The audio rule (`books/dev/src/explanation/design/ownership.md`): the module of the audio thread's pass may
//!   name nothing that takes a lock, waits, allocates, formats or logs, and
//!   may use only atomics, `Arc`, time, `VecDeque`, `rtrb` and the audio
//!   crate's own device, record and store types. A method that allocates
//!   inside an allowed type is a review rule, held by the mixer's
//!   allocation test.
//! - The rule of reads: the four functions of the listener's event path
//!   after `on_win_event` may read Windows' state only through the event's
//!   `EventWindows`, so its call-count test counts every read.
//!
//! - The determinism rule (`books/dev/src/reference/testing.md`, "The crates on a reducer's path"), over every crate on a reducer's path, which the tier
//!   check derives from the dependency graph ([`determinism`]): each keeps
//!   a `clippy.toml` identical to `verbatim-core`'s, with which Clippy
//!   rejects hash collections, clocks, files, processes, the network, threads, the environment, a
//!   minted trace id and the platform's floating-point functions; nothing
//!   silences those lints but an `expect` in a listed exception; and the
//!   crates name no path in a module of `std` Clippy cannot close function
//!   by function, nor `tracing`'s and `proptest`'s calls that read hidden
//!   state.
//!
//! Each checked module may `use` only the paths its allow-list names, and
//! no identifier or path on its rule's forbidden list may appear in its
//! code, imported or written out in full. The check reads text, outside
//! comments and string literals, so a re-export could hide what it looks
//! for, and a type allowed in could offer a method that locks: a review
//! rule covers that, and the modules' own tests are the real check.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use crate::waits::{cfg_test_marks, collect_rust_files, item_end, strip_comments, strip_strings};

/// A module the check reads, a file or, ending in `/`, every file of a
/// directory, the paths it may `use`, what it may not name, whether its
/// `#[cfg(test)]` module is read, and the one function read, when only that
/// function's body is checked.
struct Checked {
    path: &'static str,
    allowed: &'static [&'static str],
    forbidden: &'static [&'static [&'static str]],
    skip_tests: bool,
    within: Option<&'static str>,
}

/// The modules checked: the listener's read scheduler and the outpost's
/// reducer, by the pure rule, the hook thread's modules, by the hook rule,
/// the hook thread's body in `lib.rs`, by the hook thread's start-up rule,
/// the audio thread's pass, by the audio rule, and the listener's event
/// path, by its rule of reads.
const CHECKED: [Checked; 9] = [
    Checked {
        path: "crates/verbatim-listener-logic/src/schedule.rs",
        allowed: &[
            "std::collections::BTreeMap",
            "std::collections::BTreeSet",
            "std::collections::VecDeque",
            "std::num::NonZeroUsize",
            "std::time::Duration",
            "verbatim_model",
            "verbatim_reader_rules",
            // The listener's data types, and the idle time the check
            // threads share.
            "crate::focus",
            "crate::check_threads",
            // The module's own tests.
            "super",
        ],
        forbidden: &[&FORBIDDEN],
        skip_tests: false,
        within: None,
    },
    Checked {
        path: "crates/verbatim-outpost-logic/src/",
        allowed: &REDUCER_ALLOWED,
        forbidden: &[&REDUCER_FORBIDDEN],
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &[&HOOK_FORBIDDEN, &HOOK_LOOP_FORBIDDEN],
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/typed.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &[&HOOK_FORBIDDEN, &HOOK_LOOP_FORBIDDEN],
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-app/src/key_reports.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &[&HOOK_FORBIDDEN, &HOOK_LOOP_FORBIDDEN],
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/lib.rs",
        allowed: &[],
        forbidden: &[&HOOK_FORBIDDEN, &["GetMessageW"]],
        skip_tests: true,
        within: Some("hook_thread"),
    },
    Checked {
        path: "crates/verbatim-listener/src/listener.rs",
        allowed: &[],
        forbidden: &[&EVENT_PATH_FORBIDDEN],
        skip_tests: true,
        within: Some("watch_win_event"),
    },
    Checked {
        path: "crates/verbatim-listener/src/listener/tells.rs",
        allowed: &[],
        forbidden: &[&EVENT_PATH_FORBIDDEN],
        skip_tests: true,
        within: Some("tell_win_event"),
    },
    Checked {
        path: "crates/verbatim-audio/src/mixer/run.rs",
        allowed: &AUDIO_ALLOWED,
        forbidden: &[&AUDIO_FORBIDDEN],
        skip_tests: false,
        within: None,
    },
];

/// What the outpost's reducer may `use`: the module's own items, by
/// `self`, `super` from a file inside it (never `super::super`, which is
/// forbidden) or the module's full path; the model; Core's focus rules; the
/// rules both readers share; and the parts of `std` the design names.
const REDUCER_ALLOWED: [&str; 13] = [
    "self",
    "super",
    "crate",
    "verbatim_model",
    "verbatim_core::focus_rules",
    "verbatim_reader_rules",
    "std::collections::BTreeMap",
    "std::collections::BTreeSet",
    "std::collections::VecDeque",
    "std::cmp",
    "std::mem",
    "std::sync::Arc",
    "std::time::Duration",
];

/// What the outpost's reducer may not name: the pure rule's list, but
/// `Arc`, which it may share immutable snapshots by, and `thread`, a field
/// of the model's `NodeWindow`, for which the thread module's paths and
/// its spawn and sleep stand; with the clock read as `Instant` at all, a
/// lock by its method, a log, and a way out of the module.
const REDUCER_FORBIDDEN: [&str; 21] = [
    "thread::",
    "spawn",
    "sleep",
    "Mutex",
    "RwLock",
    "Condvar",
    "lock",
    "Atomic",
    "mpsc",
    "Instant",
    "SystemTime",
    "std::fs",
    "std::io",
    "std::env",
    "std::process",
    "std::net",
    "windows::",
    "unsafe",
    "Responder",
    "tracing",
    "super::super",
];

/// What the audio thread's pass may `use`.
const AUDIO_ALLOWED: [&str; 15] = [
    "std::collections::VecDeque",
    "std::sync::Arc",
    "std::sync::atomic",
    "std::time",
    "rtrb",
    // The mixer's records and store, and its published state.
    "super::records",
    "super::store",
    "super::Shared",
    "super::SourceShared",
    "super::source_capacity",
    // The device seam.
    "crate::AudioDevice",
    "crate::AudioError",
    "crate::AudioTap",
    "crate::DeviceFormat",
    "crate::Signal",
];

/// What the audio thread's pass may not name: identifiers, matched whole,
/// macros, matched as the word followed by `!`, and paths, matched as
/// written.
const AUDIO_FORBIDDEN: [&str; 26] = [
    "Mutex",
    "RwLock",
    "Condvar",
    "lock",
    "mpsc",
    "crossbeam",
    "crossbeam_channel",
    "park",
    "sleep",
    "recv",
    "wait",
    "Vec::new",
    "vec!",
    "Box::new",
    "to_vec",
    "to_owned",
    "collect",
    "format!",
    "String",
    "tracing",
    "trace!",
    "debug!",
    "info!",
    "warn!",
    "error!",
    "std::fs",
];

/// What the hook thread's modules may `use`.
const HOOK_ALLOWED: [&str; 14] = [
    "std::cell",
    "std::collections",
    "std::sync::Arc",
    "std::sync::atomic",
    "std::time::Instant",
    "windows::Win32",
    "verbatim_input",
    // The hook's reports, a plain enum, for the app's callbacks.
    "verbatim_input_windows::KeyReport",
    "verbatim_model",
    "verbatim_process::clock",
    "verbatim_process::port_queue::Poster",
    // Its cancel posts to the speech queue's completion port.
    "verbatim_speech::SpeechControl",
    "crate",
    "super",
];

/// What the hook thread's modules and its start-up may not name:
/// identifiers, matched whole, and paths, matched as written. The private
/// message ranges and the loops that take or dispatch messages one at a
/// time are forbidden because the thread's one `GetMessageW` loop handles
/// no message but `WM_QUIT`: another thread hands it no work, so every
/// number it mints is a key's. The start-up in `lib.rs`, around the one
/// loop in `hook.rs`, may also not name `GetMessageW`, since its one loop
/// is `hook.rs`'s.
const HOOK_FORBIDDEN: [&str; 23] = [
    "Mutex",
    "RwLock",
    "Condvar",
    "OnceLock",
    "LazyLock",
    "lock",
    "sleep",
    "std::thread",
    "std::fs",
    "std::net",
    "std::process",
    "SendMessageW",
    "SendMessageTimeoutW",
    "PeekMessageW",
    "PeekMessageA",
    "DispatchMessageW",
    "DispatchMessageA",
    "GetMessageA",
    "MsgWaitForMultipleObjects",
    "MsgWaitForMultipleObjectsEx",
    "WaitForSingleObject",
    "WM_APP",
    "WM_USER",
];

/// What the hook thread's modules may not name beside [`HOOK_FORBIDDEN`],
/// though its start-up may: it reports its readiness or its error on a
/// channel and logs a failure to make the text services' profile manager,
/// each once, before the loop takes a key.
const HOOK_LOOP_FORBIDDEN: [&str; 3] = ["mpsc", "std::io", "tracing"];

/// What the listener's event path may not name, after `on_win_event` makes
/// the event's windows: every read of Windows' state goes through the
/// event's `EventWindows`, so that its call-count test,
/// `a_flood_events_window_facts_are_read_once`, counts every read.
const EVENT_PATH_FORBIDDEN: [&str; 11] = [
    "verbatim_win32::",
    "verbatim_ia2::window",
    "window_class_name(",
    "window_pid(",
    "window_hung(",
    "front_window(",
    "in_foreground(",
    "readers.reading()",
    "SystemReads",
    " process_of_thread(",
    "(process_of_thread(",
];

/// What a pure module may not name: identifiers, matched whole, and paths,
/// matched as written. A responder answers its asker when it is dropped,
/// so one held in a pure module would answer as a replay drops its state
/// (`books/dev/src/explanation/design/ownership.md`).
const FORBIDDEN: [&str; 17] = [
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
    "Responder",
    "MarkerOwed",
];

/// Runs the check from the workspace root `root`, returning the findings
/// as the error.
pub fn check(root: &Path) -> Result<(), String> {
    let mut report = String::new();
    for checked in &CHECKED {
        for (path, source) in sources(root, checked.path)? {
            for finding in findings(&source, checked) {
                let _ = writeln!(report, "  {path}:{finding}");
            }
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a checked module names something outside its rule (the pure rule, books/dev/src/explanation/design/ownership.md; the hook rule, books/dev/src/explanation/design/ownership.md):\n{report}"
        ))
    }
}

/// The sources `path` names under `root`, each with its path: the file, or,
/// for a path ending in `/`, every `.rs` file of the directory but the test
/// modules in files of their own its files declare
/// ([`crate::coverage::test_module_files`]).
fn sources(root: &Path, path: &str) -> Result<Vec<(String, String)>, String> {
    let read = |file: &Path| {
        fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))
    };
    if !path.ends_with('/') {
        return Ok(vec![(path.to_owned(), read(&root.join(path))?)]);
    }
    let directory = root.join(path);
    let mut files = BTreeMap::new();
    let entries =
        fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
        {
            files.insert(format!("{path}{name}"), read(&entry.path())?);
        }
    }
    let tests = crate::coverage::test_module_files(&files);
    files.retain(|file, _| !tests.contains(file));
    Ok(files.into_iter().collect())
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
    if let Some(function) = checked.within
        && !keep_only_function(&mut code, function)
    {
        found.push(format!("0: has no function `{function}`"));
    }
    let allowed = checked.allowed;
    for (index, line) in code.iter().enumerate() {
        for &forbidden in checked.forbidden.iter().copied().flatten() {
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

/// Blanks each item of `code` marked `#[cfg(test)]`, a test module inline
/// or declared in a file of its own, or a test-only function.
fn blank_test_modules(code: &mut [String]) {
    let marked = cfg_test_marks(code);
    for (line, marked) in code.iter_mut().zip(marked) {
        if marked {
            line.clear();
        }
    }
}

/// Blanks every line of `code` but those of the function `function`, from
/// its `fn` to the item's end ([`item_end`]). Returns whether the function
/// was found; when not, every line is blank.
fn keep_only_function(code: &mut [String], function: &str) -> bool {
    let signature = format!("fn {function}(");
    let start = code.iter().position(|line| line.contains(&signature));
    let kept = start.map_or(0..0, |start| start..item_end(code, start));
    for (index, line) in code.iter_mut().enumerate() {
        if !kept.contains(&index) {
            line.clear();
        }
    }
    start.is_some()
}

/// Whether `code` names `forbidden`: a path, with `::`, as written; a
/// macro, ending in `!`, as its name as a whole word followed by `!`; an
/// identifier as a whole word, or for `Atomic`, as the start of one; and
/// any other text, such as a call `name(` or a field read `a.b()`, as
/// written.
fn names(code: &str, forbidden: &str) -> bool {
    let in_word = |character: char| character.is_ascii_alphanumeric() || character == '_';
    if forbidden.contains("::") {
        return code.contains(forbidden);
    }
    if let Some(name) = forbidden.strip_suffix('!') {
        return code.match_indices(name).any(|(at, _)| {
            let before = code[..at].chars().next_back();
            let after = code[at + name.len()..].chars().next();
            !before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
                && after == Some('!')
        });
    }
    if !forbidden.chars().all(in_word) {
        return code.contains(forbidden);
    }
    code.split(|character: char| !in_word(character))
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

/// The files that may `expect` `clippy::disallowed_types` and
/// `clippy::disallowed_methods`, with a reason, and no other file of a
/// covered crate may (`books/dev/src/reference/testing.md`, "The crates on a reducer's path"): the
/// two cost measurements, which time with `Instant`;
/// the replay fixtures' tests, which read their files; the mint's own test;
/// and the conformance corpus's reader and its test. The rule of whole
/// modules excepts them too, but for the mint's test, which names no module.
const EXCEPTIONS: [&str; 8] = [
    "crates/verbatim-text/tests/terminal_cost.rs",
    "crates/verbatim-core/tests/terminal_cost.rs",
    "crates/verbatim-core/tests/replay_fixture.rs",
    "crates/verbatim-core/tests/replay_validity.rs",
    "crates/verbatim-core/tests/replay_announcements.rs",
    MINT_TEST,
    "crates/verbatim-term-sim/src/conformance.rs",
    "crates/verbatim-term-sim/tests/conformance.rs",
];

/// The mint's own test, in [`EXCEPTIONS`] but held to the rule of whole
/// modules.
const MINT_TEST: &str = "crates/verbatim-model/src/lib.rs";

/// The lint names a covered crate may not `allow` or `expect`, since each
/// silences the two disallowed lints.
const SILENCING: [&str; 5] = [
    "warnings",
    "clippy::all",
    "clippy::style",
    "clippy::disallowed_types",
    "clippy::disallowed_methods",
];

/// The two lints the listed exceptions may `expect`.
const DISALLOWED: [&str; 2] = ["clippy::disallowed_types", "clippy::disallowed_methods"];

/// The modules of `std` a covered crate may name no path in, beside
/// `std::time`, of which only `Duration` is allowed: Clippy names
/// functions one by one, so a function the standard library adds later
/// would pass it.
const FORBIDDEN_MODULES: [&str; 5] = [
    "std::fs",
    "std::net",
    "std::process",
    "std::env",
    "std::thread",
];

/// What a covered crate may not name: `tracing`'s questions to its
/// dispatcher, whose answer is the subscriber's state, and `proptest`'s
/// macro and default configuration, which read `PROPTEST_` environment
/// variables, seed at random and persist failures to files; the prelude's
/// `ProptestConfig` is listed by name, since a match after a letter is not
/// a match
/// (`books/dev/src/reference/testing.md`, "The crates on a reducer's path").
const FORBIDDEN_CALLS: [&str; 9] = [
    "enabled!",
    "event_enabled!",
    "span_enabled!",
    "proptest!",
    "Config::default",
    "ProptestConfig::default",
    "Config::with_cases",
    "ProptestConfig::with_cases",
    "TestRunner::default",
];

/// The lint flags `.cargo/config.toml` may not pass.
const LINT_FLAGS: [&str; 8] = [
    "\"-A",
    "\"-W",
    "\"-D",
    "--allow",
    "--warn",
    "--deny",
    "--force-warn",
    "--cap-lints",
];

/// Runs the determinism rule over the `covered` crates, each in
/// `crates/<name>`, from the workspace root `root`
/// (`books/dev/src/reference/testing.md`, "The crates on a reducer's path"), returning the
/// findings as the error.
pub fn determinism(root: &Path, covered: &BTreeSet<String>) -> Result<(), String> {
    let mut report = String::new();
    let mut note = |finding: String| {
        let _ = writeln!(report, "  {finding}");
    };
    let read = |path: &str| {
        fs::read_to_string(root.join(path)).map_err(|error| format!("{path}: {error}"))
    };
    let canonical_path = "crates/verbatim-core/clippy.toml";
    let canonical = read(canonical_path)?;
    for name in covered {
        let directory = format!("crates/{name}");
        let clippy = format!("{directory}/clippy.toml");
        match read(&clippy) {
            Ok(text) if text == canonical => {}
            Ok(_) => note(format!("{clippy}: differs from {canonical_path}")),
            Err(error) => note(format!(
                "{error}: every covered crate keeps a copy of {canonical_path}"
            )),
        }
        let manifest = format!("{directory}/Cargo.toml");
        for finding in manifest_findings(&read(&manifest)?) {
            note(format!("{manifest}: {finding}"));
        }
        for path in rust_files(root, &directory)? {
            let source = read(&path)?;
            for finding in determinism_findings(&path, &source) {
                note(format!("{path}:{finding}"));
            }
        }
    }
    for finding in workspace_lint_findings(&read("Cargo.toml")?) {
        note(format!("Cargo.toml: {finding}"));
    }
    if let Ok(config) = read(".cargo/config.toml") {
        for flag in LINT_FLAGS {
            if config.contains(flag) {
                note(format!(".cargo/config.toml: passes the lint flag {flag}"));
            }
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "a crate on a reducer's path breaks the determinism rule (books/dev/src/reference/testing.md, \"The crates on a reducer's path\"):\n{report}"
        ))
    }
}

/// What is wrong with a covered crate's manifest `manifest`: its `[lints]`
/// table must be `workspace = true` alone, and it may have no other lint
/// table.
fn manifest_findings(manifest: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_lints = false;
    let mut lints: Vec<&str> = Vec::new();
    let mut has_lints = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_lints = line == "[lints]";
            has_lints |= in_lints;
            if line.starts_with("[lints.") {
                found.push(format!("has a lint table of its own, `{line}`"));
            }
            continue;
        }
        if in_lints && !line.is_empty() && !line.starts_with('#') {
            lints.push(line);
        }
    }
    if !has_lints || lints != ["workspace = true"] {
        found.push("its `[lints]` table must be `workspace = true` alone".to_owned());
    }
    found
}

/// Each key of the workspace's lint tables in `manifest` that names a lint
/// group or lint that would silence the disallowed lints.
fn workspace_lint_findings(manifest: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut in_lints = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_lints = line.starts_with("[workspace.lints");
            continue;
        }
        if !in_lints || line.starts_with('#') {
            continue;
        }
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"').replace('-', "_");
        if [
            "all",
            "style",
            "warnings",
            "disallowed_types",
            "disallowed_methods",
        ]
        .contains(&key.as_str())
        {
            found.push(format!("the workspace's lints name `{key}`"));
        }
    }
    found
}

/// Every `.rs` file under `directory` of `root`, by its path from `root`
/// with `/` between names, a `target` directory left out.
fn rust_files(root: &Path, directory: &str) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    collect_rust_files(&root.join(directory), &mut files)?;
    let mut paths = files
        .iter()
        .map(|file| {
            file.strip_prefix(root)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .map_err(|error| format!("{}: {error}", file.display()))
        })
        .collect::<Result<Vec<String>, String>>()?;
    paths.sort();
    Ok(paths)
}

/// The determinism rule's findings in the source `source` of the file at
/// `path`, each its line number and what was found: an attribute that
/// silences the disallowed lints, a path in a forbidden module of `std`,
/// and a forbidden macro or call.
fn determinism_findings(path: &str, source: &str) -> Vec<String> {
    let code: Vec<String> = source
        .lines()
        .map(|line| strip_strings(&strip_comments(line)))
        .collect();
    let excepted = EXCEPTIONS.contains(&path);
    let mut found = silencing_findings(&code, excepted);
    if !excepted || path == MINT_TEST {
        for (line, used) in uses(&code) {
            if forbidden_std(&used) {
                found.push(format!("{line}: uses `{used}`"));
            }
        }
        for (index, line) in code.iter().enumerate() {
            for written in std_paths(line) {
                if forbidden_std(&written) {
                    found.push(format!("{}: names `{written}`", index + 1));
                }
            }
        }
    }
    for (index, line) in code.iter().enumerate() {
        for forbidden in FORBIDDEN_CALLS {
            if names_call(line, forbidden) {
                found.push(format!("{}: names `{forbidden}`", index + 1));
            }
        }
    }
    found
}

/// Each attribute of `code` that `allow`s or `expect`s a lint of
/// [`SILENCING`], unless `excepted` and it is an `expect` of the
/// disallowed lints alone with a reason, each its first line.
fn silencing_findings(code: &[String], excepted: bool) -> Vec<String> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < code.len() {
        let trimmed = code[index].trim_start();
        if !(trimmed.starts_with("#[") || trimmed.starts_with("#![")) {
            index += 1;
            continue;
        }
        let start = index;
        let mut attribute = String::new();
        let mut depth = 0_i32;
        while index < code.len() {
            for character in code[index].chars() {
                match character {
                    '[' => depth += 1,
                    ']' => depth -= 1,
                    _ => {}
                }
            }
            attribute.push_str(&code[index]);
            attribute.push(' ');
            index += 1;
            if depth <= 0 {
                break;
            }
        }
        if !(attribute.contains("allow(") || attribute.contains("expect(")) {
            continue;
        }
        let silences: Vec<&str> = SILENCING
            .iter()
            .copied()
            .filter(|lint| names(&attribute, lint))
            .collect();
        if silences.is_empty() {
            continue;
        }
        let expected = attribute.trim_start().starts_with("#[expect(")
            || attribute.trim_start().starts_with("#![expect(");
        let allowed = excepted
            && expected
            && attribute.contains("reason")
            && silences.iter().all(|lint| DISALLOWED.contains(lint));
        if !allowed {
            found.push(format!(
                "{}: silences `{}`",
                start + 1,
                silences.join("`, `")
            ));
        }
    }
    found
}

/// Whether `path` names a forbidden module of `std`, or a path in it.
fn forbidden_std(path: &str) -> bool {
    let within = |module: &str| path == module || path.starts_with(&format!("{module}::"));
    FORBIDDEN_MODULES.iter().any(|module| within(module))
        || (within("std::time") && !within("std::time::Duration"))
}

/// The paths beginning `std::` written out in `line`, each as far as it
/// goes.
fn std_paths(line: &str) -> Vec<String> {
    line.match_indices("std::")
        .filter(|(at, _)| {
            !line[..*at]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
        })
        .map(|(at, _)| {
            line[at..]
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || *character == '_' || *character == ':'
                })
                .collect::<String>()
                .trim_end_matches(':')
                .to_owned()
        })
        .collect()
}

/// Whether `code` names the macro or call `forbidden`: a macro as
/// [`names`] finds it, a path as written with no identifier character
/// before it.
fn names_call(code: &str, forbidden: &str) -> bool {
    if !forbidden.contains("::") {
        return names(code, forbidden);
    }
    code.match_indices(forbidden).any(|(at, _)| {
        !code[..at]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
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
        forbidden: &[&FORBIDDEN],
        skip_tests: false,
        within: None,
    };

    /// A module checked by the hook rule.
    const HOOK: Checked = Checked {
        path: "hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &[&HOOK_FORBIDDEN, &HOOK_LOOP_FORBIDDEN],
        skip_tests: true,
        within: None,
    };

    /// The hook thread's start-up, `start_up` in its module.
    const HOOK_START: Checked = Checked {
        path: "lib.rs",
        allowed: &[],
        forbidden: &[&HOOK_FORBIDDEN, &["GetMessageW"]],
        skip_tests: true,
        within: Some("start_up"),
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
    fn a_private_message_in_a_hook_module_is_found() {
        let source = "fn pump(msg: &MSG) {\n    if msg.message == WM_APP {}\n}\n";
        assert_eq!(findings(source, &HOOK), ["2: names `WM_APP`"]);
    }

    #[test]
    fn only_the_hook_threads_start_up_is_read_in_its_module() {
        let source = "fn other() { let _ = state.lock(); }
fn start_up() {
    if ready {
        WaitForSingleObject(event, 0);
    }
}
fn after() { GetMessageW(); }
";
        assert_eq!(
            findings(source, &HOOK_START),
            ["4: names `WaitForSingleObject`"]
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

    /// The audio thread's pass.
    const AUDIO: Checked = Checked {
        path: "run.rs",
        allowed: &AUDIO_ALLOWED,
        forbidden: &[&AUDIO_FORBIDDEN],
        skip_tests: false,
        within: None,
    };

    #[test]
    fn a_lock_an_allocation_and_a_format_in_the_audio_pass_are_found_and_a_field_is_not() {
        let source = "fn pass(state: &Mutex<u8>) {\n    let mix = vec![0.0; 8];\n    let line = format!(\"{}\", 1);\n    let rate = state.format.sample_rate;\n}\n";
        assert_eq!(
            findings(source, &AUDIO),
            ["1: names `Mutex`", "2: names `vec!`", "3: names `format!`"]
        );
    }

    /// The outpost's reducer, by its own allow-list.
    const REDUCER: Checked = Checked {
        path: "reducer/",
        allowed: &REDUCER_ALLOWED,
        forbidden: &[&REDUCER_FORBIDDEN],
        skip_tests: true,
        within: None,
    };

    #[test]
    fn a_lock_a_clock_and_a_way_out_of_the_reducer_are_found() {
        let source = "use std::sync::Mutex;\nuse super::super::shell::Shell;\nfn step(at: Instant) {\n    let held = state.lock();\n}\n";
        assert_eq!(
            findings(source, &REDUCER),
            [
                "1: names `Mutex`",
                "2: names `super::super`",
                "3: names `Instant`",
                "4: names `lock`",
                "1: uses `std::sync::Mutex`",
            ]
        );
    }

    #[test]
    fn the_reducers_own_items_and_the_model_are_allowed() {
        let source = "use std::sync::Arc;\nuse std::time::Duration;\nuse super::entry::Entry;\nuse verbatim_model::request::NodeWindow;\nfn raiser(window: NodeWindow) -> u32 {\n    window.thread\n}\n";
        assert_eq!(findings(source, &REDUCER), Vec::<String>::new());
    }

    /// A test module in a file of its own ends at its declaration, and a
    /// test-only function at its closing brace, so the code after either
    /// is still read.
    #[test]
    fn a_test_module_in_its_own_file_blanks_only_its_declaration() {
        let source = "#[cfg(test)]\nmod tests;\nuse self::charge::{Abandoned, InCharge};\n#[cfg(test)]\nfn helper() {\n}\nfn after() {}\nmod later {}\n";
        let mut code: Vec<String> = source.lines().map(str::to_owned).collect();
        blank_test_modules(&mut code);
        assert_eq!(
            code,
            [
                "",
                "",
                "use self::charge::{Abandoned, InCharge};",
                "",
                "",
                "",
                "fn after() {}",
                "mod later {}",
            ]
        );
    }

    #[test]
    fn comments_and_strings_and_longer_words_are_not_findings() {
        let source = "// a reading thread waits on a Mutex\nfn a() -> &'static str {\n    let threads = 2;\n    \"the thread ended\"\n}\n";
        assert_eq!(findings(source, &PURE), Vec::<String>::new());
    }

    /// A covered crate's code: a silencing attribute is found, by group or
    /// by lint, unless it is a listed exception's `expect` of the two
    /// disallowed lints with a reason.
    #[test]
    fn an_attribute_that_silences_the_disallowed_lints_is_found_outside_the_exceptions() {
        let code = |source: &str| -> Vec<String> {
            source
                .lines()
                .map(|line| strip_strings(&strip_comments(line)))
                .collect()
        };
        let style = code("#[allow(clippy::style)]\nfn a() {}\n");
        assert_eq!(
            silencing_findings(&style, false),
            ["1: silences `clippy::style`"]
        );
        let warnings = code("#![allow(warnings)]\n");
        assert_eq!(
            silencing_findings(&warnings, true),
            ["1: silences `warnings`"]
        );
        let expected = code(
            "#![expect(\n    clippy::disallowed_types,\n    clippy::disallowed_methods,\n    reason = \"a measurement\"\n)]\n",
        );
        assert_eq!(silencing_findings(&expected, true), Vec::<String>::new());
        assert_eq!(
            silencing_findings(&expected, false),
            ["1: silences `clippy::disallowed_types`, `clippy::disallowed_methods`"]
        );
        let allowed = code("#[allow(clippy::disallowed_methods)]\n");
        assert_eq!(
            silencing_findings(&allowed, true),
            ["1: silences `clippy::disallowed_methods`"]
        );
        let other = code("#[expect(clippy::cast_possible_truncation, reason = \"fits\")]\n");
        assert_eq!(silencing_findings(&other, false), Vec::<String>::new());
    }

    /// A path in a forbidden module of `std` is found in a `use` tree or
    /// written out, `std::time::Duration` alone allowed; a listed
    /// exception is not read for it.
    #[test]
    fn a_path_in_a_forbidden_module_of_std_is_found() {
        let source = "use std::{fs, time::Duration};\nfn a() {\n    let at = std::time::Instant::now();\n    let zero = std::time::Duration::ZERO;\n    let me = ::std::process::id();\n}\n";
        assert_eq!(
            determinism_findings("crates/verbatim-core/src/a.rs", source),
            [
                "1: uses `std::fs`",
                "3: names `std::time::Instant::now`",
                "5: names `std::process::id`"
            ]
        );
        assert_eq!(
            determinism_findings("crates/verbatim-core/tests/terminal_cost.rs", source),
            Vec::<String>::new()
        );
    }

    /// `proptest!`, `tracing`'s `enabled!` and a default `proptest`
    /// configuration are found; a longer name ending the same is not.
    #[test]
    fn the_calls_that_read_hidden_state_are_found() {
        let source = "proptest! {}\nfn a() {\n    let on = tracing::enabled!(Level::INFO);\n    let config = Config::default();\n    let theme = ThemeConfig::default();\n    let runner = TestRunner::new(ProptestConfig::with_cases(64));\n    let other = TestRunner::default();\n}\n";
        assert_eq!(
            determinism_findings("crates/verbatim-core/src/a.rs", source),
            [
                "1: names `proptest!`",
                "3: names `enabled!`",
                "4: names `Config::default`",
                "6: names `ProptestConfig::with_cases`",
                "7: names `TestRunner::default`"
            ]
        );
    }

    /// A covered crate's manifest takes the workspace's lints alone, and
    /// the workspace names no lint that silences the disallowed lints.
    #[test]
    fn the_lint_tables_are_the_workspaces_and_silence_nothing() {
        assert_eq!(
            manifest_findings("[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            manifest_findings("[lints]\nworkspace = true\n\n[lints.clippy]\nstyle = \"allow\"\n"),
            ["has a lint table of its own, `[lints.clippy]`"]
        );
        assert_eq!(
            manifest_findings("[package]\nname = \"a\"\n"),
            ["its `[lints]` table must be `workspace = true` alone"]
        );
        assert_eq!(
            workspace_lint_findings(
                "[workspace.lints.clippy]\npedantic = { level = \"warn\", priority = -1 }\ndisallowed-methods = \"allow\"\n"
            ),
            ["the workspace's lints name `disallowed_methods`"]
        );
    }
}
