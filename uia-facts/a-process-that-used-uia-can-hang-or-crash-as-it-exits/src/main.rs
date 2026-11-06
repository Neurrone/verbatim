//! A process that used UIA can hang or crash as it exits
//! (`uia-facts/a-process-that-used-uia-can-hang-or-crash-as-it-exits/readme.md`).
//!
//! 150 child processes (the `exiter` role), one after another, each start six
//! provider programs of their own and, from a thread each, connect as UIA
//! clients to each of a provider's three windows (one with a UIA provider,
//! one with its own `IAccessible`, one plain), read names, search a subtree
//! and register an event handler. Each child then ends its providers, as the mockapp
//! tests' providers ended before their test process, and ends normally with
//! `std::process::exit`, which runs every DLL's process-detach code, rather
//! than with `TerminateProcess` as support's processes end. This program
//! leaves that out on purpose. A child that has not ended 10 s after its
//! work is a hang, and one that ends with an access violation a crash; a
//! child that fails otherwise makes the claim inconclusive.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, rate};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{note, uia};
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

/// Children a run, kept short: one hang among them passes against the
/// recorded 1 in 100, and none is inconclusive. `--trials 2` runs 300, the
/// fewest at which none differs, since 0.99 to the 300th power is below
/// 0.05.
const CHILDREN: usize = 150;
/// Threads in each child, each with a provider program of its own, as six
/// threads each connecting to a mockapp of their own hung 1 in 40.
const THREADS: usize = 6;
/// How long a child may take to end after its work.
const HANG_LIMIT: Duration = Duration::from_secs(10);

fn main() {
    let args = Args::parse();
    if let Some((name, _)) = &args.role {
        assert_eq!(name, "exiter", "unknown role {name}");
        exiter();
    }
    let mut report = Report::start("a-process-that-used-uia-can-hang-or-crash-as-it-exits");
    if args.wants("exit") {
        report.case("exit", "150 child processes, one after another, each using UIA from six threads against six provider programs of its own, ending them, and then ending normally, running UIA's detach code; counting those that hang or crash");
        judge(&mut report);
    }
    report.finish();
}

/// The `exiter` role: uses UIA against six provider programs of its own,
/// one thread each, ends them, reports `done`, then ends normally.
fn exiter() -> ! {
    uia::mta();
    let providers: Vec<Provider> = (0..THREADS)
        .map(|_| Provider::launch(&["--windows", "uia:0,msaa:1,plain:2"]))
        .collect();
    // UIA's first-time setup, before the threads, which would otherwise
    // meet "A cache request fails while UIA's first-time setup runs on
    // another thread".
    let first = uia::client();
    uia::cache_request(&first, &[]).expect("UIA's first-time setup");
    let threads: Vec<_> = providers
        .iter()
        .map(|p| {
            let windows = [p.hwnd(0), p.hwnd(1), p.hwnd(2)];
            std::thread::spawn(move || {
                uia::mta();
                for hwnd in windows {
                    use_uia(hwnd);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("a UIA thread");
    }
    drop(providers);
    role::report("done");
    // An ordinary exit, running every DLL's detach code.
    std::process::exit(0)
}

/// Connects to window `hwnd`, reads its name, searches its subtree and
/// registers a name-change handler, which stays registered.
fn use_uia(hwnd: isize) {
    let client = uia::client();
    let mut kept = Vec::new();
    {
        let Ok(root) = uia::element(&client, hwnd) else {
            return;
        };
        let _ = uia::name(&root);
        // SAFETY: a live client object.
        let condition = unsafe { client.CreateTrueCondition() }.expect("a condition");
        // SAFETY: live element and condition.
        let _ = unsafe { root.FindAll(TreeScope_Descendants, &condition) };
        let (handler, _reports) = Handler::make();
        let cache = events::name_cache(&client).expect("a cache request");
        // SAFETY: live client, element, cache request and handler.
        let _ = unsafe {
            client.AddPropertyChangedEventHandlerNativeArray(
                &root,
                windows::Win32::UI::Accessibility::TreeScope_Subtree,
                &cache,
                &events::as_property_handler(&handler),
                &[UIA_NamePropertyId],
            )
        };
        kept.push(root);
    }
    // The handler stays registered and the elements are kept: the process
    // ends with its connections in place, as a test process did.
    std::mem::forget(kept);
    std::mem::forget(client);
}

/// `STATUS_ACCESS_VIOLATION`, the crash the fault ends a process with.
const ACCESS_VIOLATION: u32 = 0xC000_0005;

fn judge(report: &mut Report) {
    let mut hangs = 0;
    let mut crashes = 0;
    let mut done = 0;
    let mut other = Vec::new();
    let children = CHILDREN * trials();
    note::progress(&format!(
        "{children} children, one after another, up to 10 s each to end"
    ));
    for _ in 0..children {
        let mut child = Role::start("exiter", &[]);
        // Only a child that has done its UIA work counts: anything else is
        // another failure, kept as evidence.
        let line = child.line(Duration::from_secs(30));
        if line.as_deref() != Some("done") {
            other.push(format!("process {} reported {line:?}", child.pid));
            continue;
        }
        done += 1;
        match child.wait(HANG_LIMIT) {
            None => {
                hangs += 1;
                note::evidence(&format!("process {} did not end within 10 s", child.pid));
            }
            Some(0) => {}
            Some(code) if code.cast_unsigned() == ACCESS_VIOLATION => {
                crashes += 1;
                note::evidence(&format!(
                    "process {} ended with an access violation",
                    child.pid
                ));
            }
            Some(code) => other.push(format!(
                "process {} ended with code 0x{:08X}",
                child.pid,
                code.cast_unsigned()
            )),
        }
    }
    for line in &other {
        note::evidence(line);
    }
    let faults = hangs + crashes;
    let verdict = if other.is_empty() {
        rate(faults, done, 1, 100)
    } else {
        Verdict::Inconclusive(format!("{} children failed otherwise", other.len()))
    };
    report.claim(
        "children that hung or ended with an access violation",
        &format!("{faults} of {done} ({hangs} hung, {crashes} access violations)"),
        "about 1 to 3 percent of runs; the arbitration tests 1 run in 100 after the first-time setup fix",
        verdict,
    );
}
