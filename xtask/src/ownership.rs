//! The `cargo xtask ci` checks that the owners keep their rules, by two
//! rules over named modules:
//!
//! - The pure rule (`docs/design/ownership.md`):
//!   a module whose transitions are replayed in tests may name nothing that
//!   blocks, shares, spawns, reads a clock or calls out of the process.
//!   The outpost's reducer is held to it as a directory, every file in it
//!   (`docs/design/ownership.md`),
//!   with an allow-list of its own: the module's own items, the model,
//!   Core's focus rules, `std`'s collections, `cmp`, `mem`, `Arc` and
//!   `Duration`; its `#[cfg(test)]` modules, inline or in files of their
//!   own, are skipped.
//! - The hook rule (`docs/design/ownership.md`): the modules whose code runs on the keyboard hook's
//!   thread may name nothing that takes a lock, waits, logs, does I/O or
//!   dispatches sent messages, and nothing that would have the thread take
//!   work from another thread as a message: the hook thread takes only
//!   keys. Their test modules, which collect reports under a lock, are
//!   skipped. The hook thread's start-up in `lib.rs`, `hook_thread`, is
//!   read alone by the same rule, but may report on a channel and log once
//!   before the loop, and may not run a message loop of its own.
//! - The audio rule (`docs/design/ownership.md`): the module of the audio thread's pass may
//!   name nothing that takes a lock, waits, allocates, formats or logs, and
//!   may use only atomics, `Arc`, time, `VecDeque`, `rtrb` and the audio
//!   crate's own device, record and store types. A method that allocates
//!   inside an allowed type is a review rule, held by the mixer's
//!   allocation test.
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

/// A module the check reads, a file or, ending in `/`, every file of a
/// directory, the paths it may `use`, what it may not name, whether its
/// `#[cfg(test)]` module is read, and the one function read, when only that
/// function's body is checked.
struct Checked {
    path: &'static str,
    allowed: &'static [&'static str],
    forbidden: &'static [&'static str],
    skip_tests: bool,
    within: Option<&'static str>,
}

/// The modules checked: the listener's read scheduler and the outpost's
/// reducer, by the pure rule, the hook thread's modules, by the hook rule,
/// the hook thread's body in `lib.rs`, by the hook thread's start-up rule,
/// and the audio thread's pass, by the audio rule.
const CHECKED: [Checked; 7] = [
    Checked {
        path: "crates/verbatim-listener-logic/src/schedule.rs",
        allowed: &[
            "std::collections",
            "std::time::Duration",
            "verbatim_model",
            "verbatim_reader_rules",
            // The listener's data types.
            "crate::focus",
            // The module's own tests.
            "super",
        ],
        forbidden: &FORBIDDEN,
        skip_tests: false,
        within: None,
    },
    Checked {
        path: "crates/verbatim-outpost-logic/src/",
        allowed: &REDUCER_ALLOWED,
        forbidden: &REDUCER_FORBIDDEN,
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/typed.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-app/src/key_reports.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
        within: None,
    },
    Checked {
        path: "crates/verbatim-input-windows/src/lib.rs",
        allowed: &[],
        forbidden: &HOOK_START_FORBIDDEN,
        skip_tests: true,
        within: Some("hook_thread"),
    },
    Checked {
        path: "crates/verbatim-audio/src/mixer/run.rs",
        allowed: &AUDIO_ALLOWED,
        forbidden: &AUDIO_FORBIDDEN,
        skip_tests: false,
        within: None,
    },
];

/// What the outpost's reducer may `use`: the module's own items, by
/// `self`, `super` from a file inside it (never `super::super`, which is
/// forbidden) or the module's full path; the model; Core's focus rules; the
/// rules both readers share; and the parts of `std` the design names.
const REDUCER_ALLOWED: [&str; 11] = [
    "self",
    "super",
    "crate",
    "verbatim_model",
    "verbatim_core::focus_rules",
    "verbatim_reader_rules",
    "std::collections",
    "std::cmp",
    "std::mem",
    "std::sync::Arc",
    "std::time::Duration",
];

/// What the outpost's reducer may not name: the pure rule's list, but
/// `Arc`, which it may share immutable snapshots by, and `thread`, a field
/// of the model's `WinEventStamp`, for which the thread module's paths and
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

/// What the hook thread's modules may not name: identifiers, matched whole,
/// and paths, matched as written. The private message ranges and the
/// loops that take or dispatch messages one at a time are forbidden
/// because the thread's one `GetMessageW` loop handles no message but
/// `WM_QUIT`: another thread hands it no work, so every number it mints is
/// a key's.
const HOOK_FORBIDDEN: [&str; 26] = [
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

/// What the hook thread's body in `lib.rs`, its start-up around the one
/// loop in `hook.rs`, may not name: the hook rule's list but `mpsc`,
/// `std::io` and `tracing`, since it reports its readiness or its error on
/// a channel and logs a failure to make the text services' profile
/// manager, each once, before the loop takes a key; and `GetMessageW`,
/// since its one loop is `hook.rs`'s.
const HOOK_START_FORBIDDEN: [&str; 24] = [
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
    "GetMessageW",
    "GetMessageA",
    "MsgWaitForMultipleObjects",
    "MsgWaitForMultipleObjectsEx",
    "WaitForSingleObject",
    "WM_APP",
    "WM_USER",
];

/// What a pure module may not name: identifiers, matched whole, and paths,
/// matched as written. A responder answers its asker when it is dropped,
/// so one held in a pure module would answer as a replay drops its state
/// (`docs/design/ownership.md`).
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
            "a checked module names something outside its rule (the pure rule, docs/design/ownership.md; the hook rule, docs/design/ownership.md):\n{report}"
        ))
    }
}

/// The sources `path` names under `root`, each with its path: the file, or,
/// for a path ending in `/`, every `.rs` file of the directory but those a
/// `#[cfg(test)] mod name;` of one of them declares, which are test modules
/// in files of their own.
fn sources(root: &Path, path: &str) -> Result<Vec<(String, String)>, String> {
    let read = |file: &Path| {
        fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))
    };
    if !path.ends_with('/') {
        return Ok(vec![(path.to_owned(), read(&root.join(path))?)]);
    }
    let directory = root.join(path);
    let mut files: Vec<(String, String)> = Vec::new();
    let entries =
        fs::read_dir(&directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", directory.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"))
        {
            files.push((name.clone(), read(&entry.path())?));
        }
    }
    let tests: Vec<String> = files
        .iter()
        .flat_map(|(_, source)| test_module_files(source))
        .collect();
    files.retain(|(name, _)| !tests.contains(name));
    files.sort();
    Ok(files
        .into_iter()
        .map(|(name, source)| (format!("{path}{name}"), source))
        .collect())
}

/// The files of the test modules `source` declares as `#[cfg(test)] mod
/// name;`, each `name.rs`.
fn test_module_files(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().map(str::trim).collect();
    lines
        .windows(2)
        .filter(|pair| pair[0] == "#[cfg(test)]")
        .filter_map(|pair| {
            let name = pair[1].strip_prefix("mod ")?.strip_suffix(';')?;
            Some(format!("{name}.rs"))
        })
        .collect()
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

/// Blanks every line of `code` but those of the function `function`, from
/// its `fn` to the brace that closes its body, counting the braces of code
/// already stripped of comments and strings. Returns whether the function
/// was found; when not, every line is blank.
fn keep_only_function(code: &mut [String], function: &str) -> bool {
    let signature = format!("fn {function}(");
    let start = code.iter().position(|line| line.contains(&signature));
    let mut end = start.unwrap_or(0);
    if let Some(start) = start {
        let mut depth = 0_i32;
        let mut opened = false;
        end = start;
        while end < code.len() {
            for character in code[end].chars() {
                match character {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            end += 1;
            if opened && depth <= 0 {
                break;
            }
        }
    }
    let kept = start.map_or(0..0, |start| start..end);
    for (index, line) in code.iter_mut().enumerate() {
        if !kept.contains(&index) {
            line.clear();
        }
    }
    start.is_some()
}

/// Whether `code` names `forbidden`: a path, with `::`, as written; a
/// macro, ending in `!`, as its name as a whole word followed by `!`; an
/// identifier as a whole word, or for `Atomic`, as the start of one.
fn names(code: &str, forbidden: &str) -> bool {
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
        within: None,
    };

    /// A module checked by the hook rule.
    const HOOK: Checked = Checked {
        path: "hook.rs",
        allowed: &HOOK_ALLOWED,
        forbidden: &HOOK_FORBIDDEN,
        skip_tests: true,
        within: None,
    };

    /// The hook thread's start-up, `start_up` in its module.
    const HOOK_START: Checked = Checked {
        path: "lib.rs",
        allowed: &[],
        forbidden: &HOOK_START_FORBIDDEN,
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
        forbidden: &AUDIO_FORBIDDEN,
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
        forbidden: &REDUCER_FORBIDDEN,
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
        let source = "use std::sync::Arc;\nuse std::time::Duration;\nuse super::entry::Entry;\nuse verbatim_model::protocol::WinEventStamp;\nfn raiser(stamp: WinEventStamp) -> u32 {\n    stamp.thread\n}\n";
        assert_eq!(findings(source, &REDUCER), Vec::<String>::new());
    }

    #[test]
    fn a_test_module_in_a_file_of_its_own_is_named_by_its_declaration() {
        let source = "mod entry;\n#[cfg(test)]\nmod tests;\n";
        assert_eq!(test_module_files(source), ["tests.rs"]);
    }

    #[test]
    fn comments_and_strings_and_longer_words_are_not_findings() {
        let source = "// a reading thread waits on a Mutex\nfn a() -> &'static str {\n    let threads = 2;\n    \"the thread ended\"\n}\n";
        assert_eq!(findings(source, &PURE), Vec::<String>::new());
    }
}
