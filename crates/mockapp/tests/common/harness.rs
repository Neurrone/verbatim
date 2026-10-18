//! The test runner for every mockapp test binary (their `Cargo.toml`
//! entries set `harness = false`). It runs each test in a process of its own
//! on a desktop of its own ([`run_isolated`]), in parallel, printing
//! libtest's lines, and then ends the process without running any DLL's
//! process-detach code.
//!
//! Why the process is not allowed to exit normally: a process that has
//! connected to UIA providers sometimes hangs at full CPU, or crashes, in
//! `UIAutomationCore.dll`'s own process-detach code as it exits normally,
//! after all its tests have passed, and nothing tried on our side avoids it
//! (`docs/uia-bugs.md`, "A process that used UIA can hang or crash as it
//! exits"). Verbatim's own processes are not affected, because outposts and
//! the listener end themselves with `TerminateProcess` once they have shut
//! down, and never run that code (`docs/architecture.md`, section 1,
//! "Process and thread model"); these test binaries are the only processes
//! that use UIA as a client and exit normally.
//!
//! So, once the results are printed and flushed, the runner ends its own
//! process with `TerminateProcess`, as outposts and the listener end
//! themselves, without process-detach code. This is a workaround for that UIA
//! fault, not cleanup: it skips every DLL's detach code, which these tests
//! do not need, since nothing they leave behind is flushed at exit. Remove
//! it if the fault is fixed or found to be ours.
//!
//! Each test runs on a desktop of its own so that nothing it does reaches
//! the desktop the run was started from, which may be in use: no mockapp
//! window, and no event, appears there, and no other client on it, a
//! screen reader or another test, calls mockapp. No mockapp test needs the
//! foreground or the keyboard focus; those that need a window to be the
//! foreground, or an element to be focused, say so to the outpost under test
//! instead (`common/outpost.rs`).

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
/// `--list`) and output, each in a process of its own on a desktop of its
/// own, which no other process uses, then ends the process with libtest's
/// exit code, as explained in the module documentation.
///
/// The desktop also keeps the counts of the calls `mockapp`'s providers
/// answer exact.
/// A provider cannot tell which client a call came from. UIA calls arrive
/// from UI Automation's own threads inside `mockapp`, carrying none of
/// the client's identity, so the counts would include every other client's
/// calls: a screen reader's or another test agent's answering `mockapp`'s
/// window being created, at times of their own, and those of the other
/// tests of the same binary, whose desktop-wide event registrations read
/// every new window. A desktop isolates `mockapp` from all of them: a
/// window, and every event about it, is seen only from its own desktop,
/// and a process started on one, `mockapp` included, starts its children
/// there. So each test runs as this binary again, started on a new desktop
/// with `--isolated-test` and the test's name, and is the only client
/// there; this process prints its result as libtest does.
#[allow(
    dead_code,
    reason = "each test binary compiles this file whole, and one with ignored tests calls run_isolated_ignoring instead"
)]
pub fn run_isolated(tests: &[(&'static str, fn())]) -> ! {
    run_isolated_ignoring(tests, &[])
}

/// A test that runs only when asked for with `--ignored` or
/// `--include-ignored`, as one marked `#[ignore = "reason"]` does under
/// libtest: its name, the recorded decision that keeps it out of a plain
/// run, and its function.
pub type Ignored = (&'static str, &'static str, fn());

/// [`run_isolated`], with the tests `ignored` besides `tests`: a plain run
/// reports each of them as ignored, with its reason, and runs none.
pub fn run_isolated_ignoring(tests: &[(&'static str, fn())], ignored: &[Ignored]) -> ! {
    let options = Options::parse();
    let all: Vec<Test> = tests
        .iter()
        .map(|&(name, run)| Test {
            name,
            run,
            ignored: None,
        })
        .chain(ignored.iter().map(|&(name, reason, run)| Test {
            name,
            run,
            ignored: Some(reason),
        }))
        .collect();
    if let Some(name) = &options.isolated {
        let Some(&Test { run: test, .. }) = all.iter().find(|test| test.name == name) else {
            eprintln!("error: no test is named {name}");
            end(FAILED);
        };
        end(if catch_unwind(AssertUnwindSafe(test)).is_ok() {
            0
        } else {
            FAILED
        });
    }
    crate::common::contain_children();
    run_with(&options, &all, |name, _| isolated::run(name))
}

/// One test the runner knows: its name, its function, and, for an ignored
/// test, why it is ignored.
#[derive(Clone, Copy)]
struct Test {
    name: &'static str,
    run: fn(),
    ignored: Option<&'static str>,
}

/// Runs the tests `options` selects, each with `execute`, which says
/// whether it passed, printing libtest's lines, and ends the process. An
/// ignored test that the names select but `options` does not ask to run
/// is reported as ignored, as libtest reports it.
fn run_with(
    options: &Options,
    tests: &[Test],
    execute: impl Fn(&'static str, fn()) -> bool + Sync,
) -> ! {
    let named: Vec<Test> = tests
        .iter()
        .copied()
        .filter(|test| options.names(test.name))
        .collect();
    if options.list {
        for test in &named {
            println!("{}: test", test.name);
        }
        end(0);
    }
    let (selected, skipped): (Vec<Test>, Vec<Test>) = named
        .iter()
        .partition(|test| options.runs(test.ignored.is_some()));
    let filtered_out = tests.len() - named.len();
    println!(
        "\nrunning {} test{}",
        named.len(),
        if named.len() == 1 { "" } else { "s" }
    );
    for test in &skipped {
        println!(
            "test {} ... ignored, {}",
            test.name,
            test.ignored.unwrap_or_default()
        );
    }
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..options.threads.min(selected.len()).max(1) {
            scope.spawn(|| {
                while let Some(&Test {
                    name, run: test, ..
                }) = selected.get(next.fetch_add(1, Ordering::Relaxed))
                {
                    let passed = execute(name, test);
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
        "\ntest result: {}. {} passed; {} failed; {} ignored; 0 measured; {filtered_out} filtered out; finished in {:.2}s\n",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len(),
        skipped.len(),
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
    /// Which of the ignored tests run: none, unless `--ignored` asks for
    /// them alone or `--include-ignored` for them as well.
    ignored: IgnoredTests,
    threads: usize,
    /// `--isolated-test <name>`: this process is the one [`run_isolated`]
    /// started to run that test alone.
    isolated: Option<String>,
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
            ignored: IgnoredTests::Skipped,
            threads: std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            isolated: None,
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
                "--ignored" => options.ignored = IgnoredTests::Only,
                "--include-ignored" => options.ignored = IgnoredTests::Included,
                // Output is never captured, so these change nothing.
                "--nocapture" | "--show-output" | "--quiet" | "-q" => {}
                "--test-threads" => {
                    let threads = value("--test-threads")?;
                    options.threads = threads
                        .parse()
                        .map_err(|_| format!("--test-threads {threads} is not a number"))?;
                }
                "--skip" => options.skips.push(value("--skip")?),
                "--isolated-test" => options.isolated = Some(value("--isolated-test")?),
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

    /// Whether the name filters and skips select the test `name`.
    fn names(&self, name: &str) -> bool {
        (self.filters.is_empty() || self.filters.iter().any(|filter| self.matches(name, filter)))
            && !self.skips.iter().any(|skip| self.matches(name, skip))
    }

    /// Whether a test the names select is run: an ignored one only with
    /// `--ignored` or `--include-ignored`, any other unless `--ignored`
    /// asks for the ignored ones alone.
    fn runs(&self, ignored: bool) -> bool {
        match self.ignored {
            IgnoredTests::Skipped => !ignored,
            IgnoredTests::Only => ignored,
            IgnoredTests::Included => true,
        }
    }
}

/// Which of the ignored tests a run includes.
#[derive(Clone, Copy)]
enum IgnoredTests {
    /// None, as by default.
    Skipped,
    /// Those alone (`--ignored`).
    Only,
    /// Those as well as the rest (`--include-ignored`).
    Included,
}

/// Running one test in a process of its own on a desktop of its own
/// ([`run_isolated`]).
mod isolated {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_ALL, HANDLE, WAIT_OBJECT_0,
    };
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, GetProcessWindowStation,
        GetUserObjectInformationW, HDESK, UOI_NAME,
    };
    use windows::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetCurrentProcess, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
    };
    use windows::core::{PCWSTR, PWSTR};

    /// Numbers this process's desktops, so each is new.
    static DESKTOPS: AtomicUsize = AtomicUsize::new(0);

    /// `text` as a NUL-terminated UTF-16 string.
    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// A desktop made for one test, closed once the test's process has
    /// ended; the system destroys it when nothing uses it any more.
    struct Desktop {
        handle: HDESK,
        /// Its name after its window station's, as a process is started on
        /// it.
        path: Vec<u16>,
    }

    impl Desktop {
        fn new() -> Self {
            let name = format!(
                "verbatim-test-{}-{}",
                std::process::id(),
                DESKTOPS.fetch_add(1, Ordering::Relaxed)
            );
            let name_wide = wide(name.as_ref());
            // SAFETY: a NUL-terminated name, no device or mode, and no
            // security attributes: the desktop gets this user's default
            // access, and its handle is not inheritable.
            let handle = unsafe {
                CreateDesktopW(
                    PCWSTR(name_wide.as_ptr()),
                    PCWSTR::null(),
                    None,
                    DESKTOP_CONTROL_FLAGS(0),
                    GENERIC_ALL.0,
                    None,
                )
            }
            .unwrap_or_else(|error| panic!("the desktop {name} could not be made: {error}"));
            let path = format!("{}\\{name}", window_station_name());
            Self {
                handle,
                path: wide(path.as_ref()),
            }
        }
    }

    impl Drop for Desktop {
        fn drop(&mut self) {
            // SAFETY: the handle `new` made, closed once.
            let closed = unsafe { CloseDesktop(self.handle) };
            if let Err(error) = closed
                && !std::thread::panicking()
            {
                panic!("a test's desktop could not be closed: {error}");
            }
        }
    }

    /// The name of this process's window station.
    fn window_station_name() -> String {
        // SAFETY: no preconditions; the handle is not to be closed.
        let station = unsafe { GetProcessWindowStation() }
            .unwrap_or_else(|error| panic!("this process has no window station: {error}"));
        let mut name = [0u16; 256];
        // SAFETY: `name` is writable for the size given, in bytes.
        unsafe {
            GetUserObjectInformationW(
                HANDLE(station.0),
                UOI_NAME,
                Some(name.as_mut_ptr().cast()),
                u32::try_from(size_of_val(&name)).expect("the buffer's size fits"),
                None,
            )
        }
        .unwrap_or_else(|error| panic!("the window station's name could not be read: {error}"));
        let length = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        String::from_utf16_lossy(&name[..length])
    }

    /// Inheritable copies of this process's standard output and error, for
    /// the test's process to write to: the only handles it inherits.
    struct Inherited(Vec<HANDLE>);

    impl Inherited {
        fn standard_output_and_error() -> Self {
            // SAFETY: the pseudo-handle of this process; no preconditions.
            let this_process = unsafe { GetCurrentProcess() };
            let mut handles = Vec::new();
            for raw in [
                std::io::stdout().as_raw_handle(),
                std::io::stderr().as_raw_handle(),
            ] {
                let handle = HANDLE(raw);
                assert!(
                    !handle.is_invalid(),
                    "this process has no standard output or error for the test's to go to"
                );
                let mut copy = HANDLE::default();
                // SAFETY: `handle` is this process's own live standard
                // handle; the inheritable copy is written to `copy`.
                unsafe {
                    DuplicateHandle(
                        this_process,
                        handle,
                        this_process,
                        &raw mut copy,
                        0,
                        true,
                        DUPLICATE_SAME_ACCESS,
                    )
                }
                .unwrap_or_else(|error| panic!("a standard handle could not be copied: {error}"));
                handles.push(copy);
            }
            Self(handles)
        }
    }

    impl Drop for Inherited {
        fn drop(&mut self) {
            for &handle in &self.0 {
                // SAFETY: a copy this value made and owns, closed once.
                let _ = unsafe { CloseHandle(handle) };
            }
        }
    }

    /// Runs the test `name` in a new process of this binary on a new
    /// desktop and returns whether it passed. The process is in this
    /// process's job ([`crate::common::contain_children`]) from its start,
    /// as every process this one starts is, so it ends with this process
    /// however that ends.
    pub(super) fn run(name: &str) -> bool {
        let desktop = Desktop::new();
        let mut desktop_path = desktop.path.clone();
        let exe = std::env::current_exe().expect("this test binary's path");
        let mut command = wide(format!("\"{}\" --isolated-test {name}", exe.display()).as_ref());
        let inherited = Inherited::standard_output_and_error();

        let mut size = 0usize;
        // SAFETY: this call only reports the size a list of one attribute
        // needs; its failure for the missing buffer is expected.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &raw mut size) };
        // Pointer-sized units align the list for its pointer-sized fields.
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        let attributes = LPPROC_THREAD_ATTRIBUTE_LIST(buffer.as_mut_ptr().cast());
        // SAFETY: `buffer` holds `size` bytes, aligned, and outlives every
        // use of `attributes`, which ends with the delete below.
        unsafe { InitializeProcThreadAttributeList(Some(attributes), 1, None, &raw mut size) }
            .unwrap_or_else(|error| panic!("an attribute list could not be made: {error}"));
        // SAFETY: `attributes` is initialized; the handles outlive the
        // process creation that reads them, and their size is given.
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(inherited.0.as_ptr().cast()),
                inherited.0.len() * size_of::<HANDLE>(),
                None,
                None,
            )
        }
        .unwrap_or_else(|error| panic!("the inherited handles could not be listed: {error}"));
        let mut startup = STARTUPINFOEXW::default();
        startup.StartupInfo.cb =
            u32::try_from(size_of::<STARTUPINFOEXW>()).expect("the structure's size fits");
        startup.StartupInfo.lpDesktop = PWSTR(desktop_path.as_mut_ptr());
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdOutput = inherited.0[0];
        startup.StartupInfo.hStdError = inherited.0[1];
        startup.lpAttributeList = attributes;
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: `command` and `desktop_path` are NUL-terminated, writable
        // UTF-16 buffers that outlive the call; the startup information is
        // sized and carries the attribute list, which limits the handles
        // inherited to the two listed.
        let created = unsafe {
            CreateProcessW(
                PCWSTR::null(),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                true,
                EXTENDED_STARTUPINFO_PRESENT,
                None,
                PCWSTR::null(),
                (&raw const startup).cast(),
                &raw mut process,
            )
        };
        // SAFETY: initialized above and not used again.
        unsafe { DeleteProcThreadAttributeList(attributes) };
        drop(inherited);
        created
            .unwrap_or_else(|error| panic!("the test {name}'s process could not start: {error}"));
        // SAFETY: the thread handle the creation returned, closed once.
        let _ = unsafe { CloseHandle(process.hThread) };
        // The test bounds its own waits; this waits for its process to end.
        // SAFETY: the process handle the creation returned, live until
        // closed below.
        let waited = unsafe { WaitForSingleObject(process.hProcess, INFINITE) };
        assert_eq!(
            waited, WAIT_OBJECT_0,
            "the test {name}'s process was waited for"
        );
        let mut code = 0u32;
        // SAFETY: as above; `code` is written.
        unsafe { GetExitCodeProcess(process.hProcess, &raw mut code) }
            .unwrap_or_else(|error| panic!("the test {name}'s exit could not be read: {error}"));
        // SAFETY: the process handle, closed once.
        let _ = unsafe { CloseHandle(process.hProcess) };
        drop(desktop);
        code == 0
    }
}
