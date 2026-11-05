//! A process that used UIA can hang or crash as it exits
//! (`uia-facts/a-process-that-used-uia-can-hang-or-crash-as-it-exits/readme.md`).
//!
//! 150 child processes (the `exiter` role), 12 at a time, each start six
//! provider programs of their own and, from a thread each, connect as UIA
//! clients to each of a provider's three windows (one with a UIA provider,
//! one with its own `IAccessible`, one plain), read names, search a subtree
//! and register an event handler. Each child then ends its providers, as the mockapp
//! tests' providers ended before their test process, and ends normally with
//! `std::process::exit`, which runs every DLL's process-detach code, rather
//! than with `TerminateProcess` as support's processes end. This program
//! leaves that out on purpose. A child that has not ended 10 s after its
//! work is a hang, and one that ends with a code other than 0 a crash.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, rate};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{note, uia};
use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

/// Children a run: the fewest whose outcome separates a rate of 2 in 100
/// from none, since 0.98 to the 150th power is below 0.05. `--trials n`
/// runs n times as many.
const CHILDREN: usize = 150;
/// Threads in each child, each with a provider program of its own, as six
/// threads each connecting to a mockapp of their own hung 1 in 40.
const THREADS: usize = 6;
/// Children running at once.
const AT_ONCE: usize = 12;
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
        report.case("exit", "150 child processes, 12 at a time, each using UIA from six threads against six provider programs of its own, ending them, and then ending normally, running UIA's detach code; counting those that hang or crash");
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

fn judge(report: &mut Report) {
    let mut hangs = 0;
    let mut crashes = Vec::new();
    let mut started = 0;
    let children = CHILDREN * trials();
    note::progress(&format!("{children} children, up to 10 s each to end"));
    while started < children {
        let batch = AT_ONCE.min(children - started);
        let mut batch_children: Vec<Role> =
            (0..batch).map(|_| Role::start("exiter", &[])).collect();
        started += batch;
        for child in &mut batch_children {
            let done = child.line(Duration::from_secs(30));
            if done.as_deref() != Some("done") {
                note::evidence(&format!("a child reported {done:?}"));
            }
            match child.wait(HANG_LIMIT) {
                None => {
                    hangs += 1;
                    note::evidence(&format!("process {} did not end within 10 s", child.pid));
                }
                Some(0) => {}
                Some(code) => {
                    note::evidence(&format!(
                        "process {} ended with code 0x{:08X}",
                        child.pid,
                        code.cast_unsigned()
                    ));
                    crashes.push(code);
                }
            }
        }
    }
    let faults = hangs + crashes.len();
    report.claim(
        "children that hung or crashed as they ended",
        &format!(
            "{faults} of {children} ({hangs} hung, {} crashed)",
            crashes.len()
        ),
        "about 1 to 3 percent; 2 hangs in 100 runs after waiting 12 s for the providers to go",
        rate(faults, children, 2, 100),
    );
}
