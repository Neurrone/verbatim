//! What the UI Automation examples share: finding the target and printing
//! a sample's percentiles.

#![allow(dead_code, reason = "each example uses part of it")]

use std::time::Duration;

use verbatim_win32::window::{top_level_windows, window_is_visible};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

/// Prints `message` with the example's name and ends with status 2.
pub fn fail(example: &str, message: &str) -> ! {
    eprintln!("{example}: {message}");
    std::process::exit(2);
}

/// The one process named `name`, with or without `.exe`, in any case; a
/// failure naming each when there is none or several.
pub fn process_named(example: &str, name: &str) -> u32 {
    let stem = process_stem(name);
    // SAFETY: a snapshot of the system's processes; closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .unwrap_or_else(|error| fail(example, &format!("could not list the processes: {error}")));
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut pids = Vec::new();
    // SAFETY: `snapshot` is the live snapshot and `entry` has its size set.
    let mut more = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(entry.szExeFile.len());
        if process_stem(&String::from_utf16_lossy(&entry.szExeFile[..length])) == stem {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the handle the snapshot call returned, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    match pids.as_slice() {
        [] => fail(example, &format!("no process is named {name}")),
        [pid] => *pid,
        several => fail(
            example,
            &format!("several processes are named {name}: {several:?}; use --hwnd"),
        ),
    }
}

/// `name` as process names are compared: lower case, without `.exe`.
fn process_stem(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.strip_suffix(".exe") {
        Some(stem) => stem.to_owned(),
        None => lower,
    }
}

/// The first visible top-level window `pid` serves, in the system's
/// order (topmost first).
pub fn first_visible_window(example: &str, pid: u32) -> isize {
    top_level_windows(pid)
        .into_iter()
        .find(|&window| window_is_visible(window))
        .unwrap_or_else(|| fail(example, &format!("process {pid} has no visible window")))
}

/// The value at `fraction` of `sorted`, which is not empty.
pub fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an index within a sample of thousands"
    )]
    let at = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[at]
}

/// Prints the header of a table of [`print_row`]s, its first column
/// headed `what`.
pub fn print_header(what: &str) {
    println!(
        "{what:<46} {:>9} {:>9} {:>9} {:>9} {:>9} {:>7}",
        "min", "median", "p90", "p99", "max", "count"
    );
}

/// Prints `sample`'s minimum, median, 90th and 99th percentiles and
/// maximum on one row named `name`, or that it is empty.
pub fn print_row(name: &str, sample: &[f64]) {
    if sample.is_empty() {
        println!("{name:<46} no sample");
        return;
    }
    let mut sorted = sample.to_vec();
    sorted.sort_by(f64::total_cmp);
    println!(
        "{name:<46} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>7}",
        sorted[0],
        percentile(&sorted, 0.5),
        percentile(&sorted, 0.9),
        percentile(&sorted, 0.99),
        sorted[sorted.len() - 1],
        sorted.len(),
    );
}

/// `duration` in microseconds.
pub fn micros(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000.0
}

/// The value after the option at `at`, or a failure saying it needs one.
pub fn value<'a>(example: &str, arguments: &'a [String], at: usize) -> &'a str {
    arguments.get(at + 1).map_or_else(
        || fail(example, &format!("{} needs a value", arguments[at])),
        String::as_str,
    )
}

/// The number after the option at `at`, or a failure saying it needs one.
pub fn number(example: &str, arguments: &[String], at: usize) -> u64 {
    value(example, arguments, at)
        .parse()
        .unwrap_or_else(|_| fail(example, &format!("{} needs a number", arguments[at])))
}
