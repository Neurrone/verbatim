//! The test runner for the test binaries that use UIA as a client (their
//! `Cargo.toml` entries set `harness = false`). It runs the tests as
//! libtest would, in parallel, printing libtest's lines, and then ends the
//! process without running any DLL's process-detach code.
//!
//! Why the process is not allowed to exit normally: a process that has
//! connected to UIA providers sometimes hangs at full CPU, or crashes with
//! an access violation, as it exits normally, after all its tests have
//! passed. Resolved with Microsoft's public symbols, the fault is inside
//! `UIAutomationCore.dll`'s own process-detach code: destroying its global
//! telemetry object (`TelemetryUtility::m_instance`) walks a linked list of
//! per-provider connection records that is corrupt by then, and the walk
//! never ends or reads freed memory. Releasing every UIA object first does
//! not prevent it, nor does waiting seconds after every provider has gone,
//! so there is no proper shutdown on our side that avoids it; what corrupts
//! the list is not yet known. It happened in about 1 to 3 percent of runs
//! of the arbitration tests, and failed CI on GitHub's runner. Verbatim's
//! own processes are not affected, because outposts and the listener are
//! killed with their job and never run that code (`docs/architecture.md`,
//! "Process lifetime"); these test binaries are the only processes that
//! use UIA as a client and exit normally. The evidence is in
//! `handoff-2026-09-02.md` ("Open: a process that has used UIA as a
//! client").
//!
//! So, once the results are printed and flushed, the runner ends its own
//! process with `TerminateProcess`, which ends it as the job ends an
//! outpost, without process-detach code. This is a workaround for that UIA
//! fault, not cleanup: it skips every DLL's detach code, which these tests
//! do not need, since nothing they leave behind is flushed at exit. Remove
//! it if the fault is fixed or found to be ours.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

/// libtest's exit code when a test failed.
const FAILED: u32 = 101;

/// Runs `tests` (name and function) with libtest's command-line filtering
/// (name substrings, `--skip`, `--exact`, `--ignored`, `--test-threads`,
/// `--list`) and output, then
/// ends the process with libtest's exit code, as explained in the module
/// documentation.
pub fn run(tests: &[(&'static str, fn())]) -> ! {
    let options = Options::parse();
    let selected: Vec<(&'static str, fn())> = tests
        .iter()
        .copied()
        .filter(|(name, _)| options.selects(name))
        .collect();
    if options.list {
        for (name, _) in &selected {
            println!("{name}: test");
        }
        end(0);
    }
    let filtered_out = tests.len() - selected.len();
    println!(
        "\nrunning {} test{}",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" }
    );
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..options.threads.min(selected.len()).max(1) {
            scope.spawn(|| {
                while let Some(&(name, test)) = selected.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let passed = catch_unwind(AssertUnwindSafe(test)).is_ok();
                    println!("test {name} ... {}", if passed { "ok" } else { "FAILED" });
                    if !passed {
                        failed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(name);
                    }
                }
            });
        }
    });
    let failed = failed
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !failed.is_empty() {
        println!("\nfailures:");
        for name in &failed {
            println!("    {name}");
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed; 0 ignored; 0 measured; {filtered_out} filtered out; finished in {:.2}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        started.elapsed().as_secs_f64(),
    );
    end(if failed.is_empty() { 0 } else { FAILED });
}

/// Flushes the output and ends the process with `code`, without running any
/// DLL's process-detach code (see the module documentation for why).
fn end(code: u32) -> ! {
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    // SAFETY: the pseudo-handle of this process; no preconditions.
    let process = unsafe { GetCurrentProcess() };
    // SAFETY: ends this process; nothing runs after it.
    let _ = unsafe { TerminateProcess(process, code) };
    // TerminateProcess on the current process does not return.
    std::process::abort();
}

/// The libtest options this runner honors. Any other option ends the run
/// with an error rather than being ignored, so a run never reports a
/// different set of tests than was asked for.
struct Options {
    filters: Vec<String>,
    skips: Vec<String>,
    exact: bool,
    list: bool,
    /// `--ignored`: run only the ignored tests, of which these binaries
    /// have none.
    ignored_only: bool,
    threads: usize,
}

impl Options {
    fn parse() -> Self {
        match Self::parse_from(std::env::args().skip(1)) {
            Ok(options) => options,
            Err(error) => {
                eprintln!("error: {error}");
                end(FAILED);
            }
        }
    }

    fn parse_from(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            filters: Vec::new(),
            skips: Vec::new(),
            exact: false,
            list: false,
            ignored_only: false,
            threads: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) if flag.starts_with("--") => {
                    (flag.to_owned(), Some(value.to_owned()))
                }
                _ => (arg.clone(), None),
            };
            let mut value = |name: &str| {
                inline
                    .clone()
                    .or_else(|| args.next())
                    .ok_or_else(|| format!("{name} needs a value"))
            };
            match flag.as_str() {
                "--exact" => options.exact = true,
                "--list" => options.list = true,
                "--ignored" => options.ignored_only = true,
                // Every test runs anyway, since none is ignored, and output
                // is never captured, so these change nothing.
                "--include-ignored" | "--nocapture" | "--show-output" | "--quiet" | "-q" => {}
                "--test-threads" => {
                    let threads = value("--test-threads")?;
                    options.threads = threads
                        .parse()
                        .map_err(|_| format!("--test-threads {threads} is not a number"))?;
                }
                "--skip" => options.skips.push(value("--skip")?),
                "--color" | "--format" => {
                    let wanted = value(&flag)?;
                    if !matches!(wanted.as_str(), "auto" | "always" | "never" | "pretty") {
                        return Err(format!("{flag} {wanted} is not supported by this runner"));
                    }
                }
                other if other.starts_with('-') => {
                    return Err(format!("{other} is not supported by this runner"));
                }
                _ => options.filters.push(arg),
            }
        }
        Ok(options)
    }

    fn matches(&self, name: &str, pattern: &str) -> bool {
        if self.exact {
            name == pattern
        } else {
            name.contains(pattern)
        }
    }

    fn selects(&self, name: &str) -> bool {
        !self.ignored_only
            && (self.filters.is_empty()
                || self.filters.iter().any(|filter| self.matches(name, filter)))
            && !self.skips.iter().any(|skip| self.matches(name, skip))
    }
}
