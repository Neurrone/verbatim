//! A process that used UIA can hang or crash as it exits
//! (`uia-facts/a-process-that-used-uia-can-hang-or-crash-as-it-exits/readme.md`).
//!
//! 150 child processes (the `exiter` role), one after another, each start
//! four provider programs of their own and, from a thread each, connect as
//! UIA clients to each one's UIA window and read its name, leaving the
//! clients in place. Each child then ends its providers, as the mockapp
//! tests' providers ended before their test process, and ends normally
//! with `std::process::exit`, which runs every DLL's process-detach code,
//! rather than with `TerminateProcess` as support's processes end. A child
//! that has not ended 3 s after its work is a hang, and one that ends with
//! an access violation a crash; a child that fails otherwise makes the
//! claim inconclusive. `candidates` measures smaller and larger setups,
//! and `clean` the same setup torn down before the exit.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, rate};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{note, uia};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationCacheRequest, IUIAutomationElement, TreeScope_Descendants,
    UIA_NamePropertyId,
};

/// Children a run of `exit` (Dickson, 2026-11-06): at the setup's recorded
/// 8 in 230, 90 would already make none differ (0.965 to the 90th power
/// is below 0.05).
const CHILDREN: usize = 150;
/// How long a child may take to end after its work: a child that has not
/// ended is spinning in the detach code, which ends no hang it starts.
const HANG_LIMIT: Duration = Duration::from_secs(3);
/// The faults of [`SETUP`] recorded, and the children they were met in.
const RECORDED: (usize, usize) = (8, 230);

/// The first build's setup: six provider programs of three windows each,
/// a thread each, every connection reading, searching and registering.
const FIRST: Setup = Setup {
    providers: 6,
    windows: 3,
    uia_only: false,
    threads: 6,
    every: false,
    search: true,
    register: true,
    end: End::Dirty,
};

/// The setup `exit` and `clean` use, the one of the candidates with the
/// most faults a minute: four provider programs of a UIA window each, a
/// thread each, every connection only reading the window's name.
const SETUP: Setup = Setup {
    providers: 4,
    windows: 1,
    threads: 4,
    search: false,
    register: false,
    ..FIRST
};

fn main() {
    let args = Args::parse();
    if let Some((name, words)) = &args.role {
        assert_eq!(name, "exiter", "unknown role {name}");
        exiter(Setup::parse(words));
    }
    let mut report = Report::start("a-process-that-used-uia-can-hang-or-crash-as-it-exits");
    if args.wants("exit") {
        report.case("exit", "150 child processes, one after another, each connecting from four threads to the UIA windows of four provider programs of its own and reading their names, leaving its clients in place, ending the providers, and then ending normally, running UIA's detach code; counting those that hang or crash");
        let outcome = run_children(SETUP, CHILDREN * trials());
        let verdict = if outcome.other.is_empty() {
            rate(outcome.faults(), outcome.done, RECORDED.0, RECORDED.1)
        } else {
            Verdict::Inconclusive(format!("{} children failed otherwise", outcome.other.len()))
        };
        report.claim(
            "children that hung or ended with an access violation",
            &outcome.text(),
            "8 of 230 (the candidates case, 2026-11-06); the arbitration tests about 1 to 3 percent of runs",
            verdict,
        );
    }
    // Runs past the time limit: only when named.
    if args.names("candidates") {
        report.case("candidates", "smaller setups, each in children one after another, to see which steps the fault needs and which setup gives the most faults a second");
        candidates(&mut report);
    }
    if args.names("clean") {
        report.case("clean", "the exit setup, each child tearing UIA down before it ends: every handler removed, every element, cache request, handler and client released, and COM left on each thread; the providers ended after the teardown, and in a second run before it");
        clean(&mut report);
    }
    report.finish();
}

/// Children of each candidate: at the exit setup's rate of about 1 in 75,
/// none in this many would be below it (0.987 to the 230th power is
/// about 0.05).
const CANDIDATE_CHILDREN: usize = 230;

#[expect(
    clippy::too_many_lines,
    reason = "the candidates listed, then one run each"
)]
fn candidates(report: &mut Report) {
    let base = FIRST;
    let read_only = Setup {
        search: false,
        register: false,
        ..base
    };
    let uia_read = Setup {
        windows: 1,
        ..read_only
    };
    for (label, setup) in [
        ("the first build's setup", base),
        (
            "no search",
            Setup {
                search: false,
                ..base
            },
        ),
        (
            "no registration",
            Setup {
                register: false,
                ..base
            },
        ),
        ("connect and read only", read_only),
        ("one thread", Setup { threads: 1, ..base }),
        ("the UIA window alone", Setup { windows: 1, ..base }),
        (
            "one provider, one thread",
            Setup {
                providers: 1,
                threads: 1,
                ..base
            },
        ),
        ("connect and read the UIA window alone", uia_read),
        (
            "connect and read the UIA window alone, two providers and threads",
            Setup {
                providers: 2,
                threads: 2,
                ..uia_read
            },
        ),
        (
            "connect and read, one provider with six UIA windows, six threads",
            Setup {
                providers: 1,
                windows: 6,
                uia_only: true,
                ..uia_read
            },
        ),
        (
            "connect and read, one provider's UIA window, six threads each to it",
            Setup {
                providers: 1,
                every: true,
                ..uia_read
            },
        ),
        (
            "connect and read the UIA window alone, four providers and threads",
            Setup {
                providers: 4,
                threads: 4,
                ..uia_read
            },
        ),
        (
            "connect and read the UIA window alone, six providers, three threads",
            Setup {
                threads: 3,
                ..uia_read
            },
        ),
        (
            "connect and read the UIA window alone, twelve providers and threads",
            Setup {
                providers: 12,
                threads: 12,
                ..uia_read
            },
        ),
    ] {
        let outcome = run_children(setup, CANDIDATE_CHILDREN);
        report.claim(
            &format!("{label}: children that hung or ended with an access violation"),
            &format!(
                "{}; {:.2} faults a minute",
                outcome.text(),
                as_f64(outcome.faults()) * 60.0 / outcome.seconds
            ),
            "",
            if outcome.other.is_empty() {
                Verdict::New
            } else {
                Verdict::Inconclusive(format!("{} children failed otherwise", outcome.other.len()))
            },
        );
    }
}

/// Children of each clean run: at the dirty rate, 8 in 230, none in this
/// many separates clean from dirty (0.965 to the 90th power is below
/// 0.05).
const CLEAN_CHILDREN: usize = 90;

fn clean(report: &mut Report) {
    for (label, end) in [
        (
            "the providers ended after the teardown",
            End::CleanThenProviders,
        ),
        (
            "the providers ended before the teardown",
            End::ProvidersThenClean,
        ),
    ] {
        let outcome = run_children(Setup { end, ..SETUP }, CLEAN_CHILDREN);
        // Against the dirty rate: differ is a clean rate apart from it,
        // none in 90 included; pass, no difference shown.
        let verdict = if outcome.other.is_empty() {
            rate(outcome.faults(), outcome.done, RECORDED.0, RECORDED.1)
        } else {
            Verdict::Inconclusive(format!("{} children failed otherwise", outcome.other.len()))
        };
        report.claim(
            &format!("a clean teardown, {label}: children that hung or ended with an access violation, against the dirty rate"),
            &outcome.text(),
            "8 of 230 with no teardown (the candidates case, 2026-11-06)",
            verdict,
        );
    }
}

fn as_f64(n: usize) -> f64 {
    f64::from(u32::try_from(n).expect("a small count"))
}

/// One child's setup: the `exiter` role's arguments.
#[derive(Clone, Copy, Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each a step of the setup, removed one at a time"
)]
struct Setup {
    /// Provider programs the child starts.
    providers: usize,
    /// Each provider's windows: a UIA window, then one with its own
    /// `IAccessible`, then a plain one, the first `windows` of them; or,
    /// with `uia_only`, that many UIA windows.
    windows: usize,
    /// Whether every window is a UIA window.
    uia_only: bool,
    /// Threads using UIA; the windows are shared among them in turn.
    threads: usize,
    /// Whether every thread connects to every window, rather than to its
    /// share.
    every: bool,
    /// Whether each connection searches the window's subtree.
    search: bool,
    /// Whether each connection registers a name-change handler.
    register: bool,
    /// How the child ends.
    end: End,
}

/// How a child ends its use of UIA before it exits normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Leaves its clients, elements and handlers in place, the providers
    /// ended first, as a test process did.
    Dirty,
    /// Removes its handlers and releases everything, each thread leaving
    /// COM, then ends the providers.
    CleanThenProviders,
    /// Ends the providers, then tears UIA down as `CleanThenProviders`
    /// does.
    ProvidersThenClean,
}

impl Setup {
    fn words(self) -> Vec<String> {
        vec![
            self.providers.to_string(),
            self.windows.to_string(),
            u8::from(self.uia_only).to_string(),
            self.threads.to_string(),
            u8::from(self.every).to_string(),
            u8::from(self.search).to_string(),
            u8::from(self.register).to_string(),
            match self.end {
                End::Dirty => "dirty",
                End::CleanThenProviders => "clean-after",
                End::ProvidersThenClean => "clean-before",
            }
            .to_owned(),
        ]
    }

    fn parse(words: &[String]) -> Self {
        let n = |i: usize| -> usize { words[i].parse().expect("a count") };
        Self {
            providers: n(0),
            windows: n(1),
            uia_only: n(2) == 1,
            threads: n(3),
            every: n(4) == 1,
            search: n(5) == 1,
            register: n(6) == 1,
            end: match words[7].as_str() {
                "dirty" => End::Dirty,
                "clean-after" => End::CleanThenProviders,
                "clean-before" => End::ProvidersThenClean,
                other => panic!("unknown end {other}"),
            },
        }
    }
}

/// The `exiter` role: uses UIA as `setup` says, ends, reports `done`,
/// then ends normally.
fn exiter(setup: Setup) -> ! {
    uia::mta();
    let layout = if setup.uia_only {
        (0..setup.windows)
            .map(|w| format!("uia:{w}"))
            .collect::<Vec<_>>()
            .join(",")
    } else {
        ["uia:0", "msaa:1", "plain:2"][..setup.windows].join(",")
    };
    let providers: Vec<Provider> = (0..setup.providers)
        .map(|_| Provider::launch(&["--windows", &layout]))
        .collect();
    // UIA's first-time setup, before the threads, which would otherwise
    // meet "A cache request fails while UIA's first-time setup runs on
    // another thread".
    let first = uia::client();
    let first_cache = uia::cache_request(&first, &[]).expect("UIA's first-time setup");
    let all: Vec<isize> = providers
        .iter()
        .flat_map(|p| (0..setup.windows).map(|w| p.hwnd(w)))
        .collect();
    let mut windows: Vec<Vec<isize>> = vec![Vec::new(); setup.threads];
    if setup.every {
        for share in &mut windows {
            share.clone_from(&all);
        }
    } else {
        // Each provider's windows to one thread, the providers in turn,
        // as each of the exit setup's threads had a provider of its own.
        for (i, p) in providers.iter().enumerate() {
            windows[i % setup.threads].extend((0..setup.windows).map(|w| p.hwnd(w)));
        }
    }
    drop(all);
    let (ready_tx, ready) = std::sync::mpsc::channel::<()>();
    let mut gos = Vec::new();
    let threads: Vec<_> = windows
        .into_iter()
        .map(|windows| {
            let ready_tx = ready_tx.clone();
            let (go_tx, go) = std::sync::mpsc::channel::<()>();
            gos.push(go_tx);
            std::thread::spawn(move || {
                uia::mta();
                let client = uia::client();
                let kept: Vec<Kept> = windows
                    .iter()
                    .filter_map(|hwnd| use_uia(&client, *hwnd, setup))
                    .collect();
                if setup.end == End::Dirty {
                    // The process ends with its connections in place, as
                    // a test process did.
                    std::mem::forget(kept);
                    std::mem::forget(client);
                    return;
                }
                ready_tx.send(()).expect("the main thread waits");
                go.recv().expect("the main thread says when");
                // SAFETY: a live client.
                unsafe { client.RemoveAllEventHandlers() }.expect("the handlers removed");
                drop(kept);
                drop(client);
                // SAFETY: this thread joined the MTA above, and released
                // every COM object it held.
                unsafe { windows::Win32::System::Com::CoUninitialize() };
            })
        })
        .collect();
    drop(ready_tx);
    let mut providers = Some(providers);
    if setup.end != End::Dirty {
        for _ in 0..setup.threads {
            ready.recv().expect("each thread is ready");
        }
        if setup.end == End::ProvidersThenClean {
            drop(providers.take());
        }
        for go in &gos {
            go.send(()).expect("the thread waits");
        }
    }
    for thread in threads {
        thread.join().expect("a UIA thread");
    }
    drop(providers.take());
    if setup.end == End::Dirty {
        std::mem::forget(first_cache);
        std::mem::forget(first);
    } else {
        drop(first_cache);
        drop(first);
        // SAFETY: the main thread joined the MTA above and released every
        // COM object it held.
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
    role::report("done");
    // An ordinary exit, running every DLL's detach code.
    std::process::exit(0)
}

/// What one connection keeps until the child tears UIA down.
struct Kept {
    _root: IUIAutomationElement,
    _cache: Option<IUIAutomationCacheRequest>,
    _handler: Option<windows::Win32::UI::Accessibility::IUIAutomationEventHandler>,
}

/// Connects to window `hwnd`, reads its name, and as `setup` says searches
/// its subtree and registers a name-change handler.
fn use_uia(client: &IUIAutomation6, hwnd: isize, setup: Setup) -> Option<Kept> {
    let root = uia::element(client, hwnd).ok()?;
    let _ = uia::name(&root);
    if setup.search {
        // SAFETY: a live client object.
        let condition = unsafe { client.CreateTrueCondition() }.expect("a condition");
        // SAFETY: live element and condition.
        let _ = unsafe { root.FindAll(TreeScope_Descendants, &condition) };
    }
    let (cache, handler) = if setup.register {
        let (handler, _reports) = Handler::make();
        let cache = events::name_cache(client).expect("a cache request");
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
        (Some(cache), Some(handler))
    } else {
        (None, None)
    };
    Some(Kept {
        _root: root,
        _cache: cache,
        _handler: handler,
    })
}

/// `STATUS_ACCESS_VIOLATION`, the crash the fault ends a process with.
const ACCESS_VIOLATION: u32 = 0xC000_0005;

/// What `children` children of `setup`, one after another, did as they
/// ended.
struct Outcome {
    done: usize,
    hangs: usize,
    crashes: usize,
    other: Vec<String>,
    seconds: f64,
}

impl Outcome {
    fn faults(&self) -> usize {
        self.hangs + self.crashes
    }

    fn text(&self) -> String {
        format!(
            "{} of {} ({} hung, {} access violations), {:.0} s",
            self.faults(),
            self.done,
            self.hangs,
            self.crashes,
            self.seconds
        )
    }
}

fn run_children(setup: Setup, children: usize) -> Outcome {
    let started = std::time::Instant::now();
    let mut outcome = Outcome {
        done: 0,
        hangs: 0,
        crashes: 0,
        other: Vec::new(),
        seconds: 0.0,
    };
    note::progress(&format!(
        "{children} children of {setup:?}, one after another, up to 3 s each to end"
    ));
    for _ in 0..children {
        let mut child = Role::start("exiter", &setup.words());
        // Only a child that has done its UIA work counts: anything else is
        // another failure, kept as evidence.
        let line = child.line(Duration::from_secs(30));
        if line.as_deref() != Some("done") {
            outcome
                .other
                .push(format!("process {} reported {line:?}", child.pid));
            continue;
        }
        outcome.done += 1;
        match child.wait(HANG_LIMIT) {
            None => {
                outcome.hangs += 1;
                note::evidence(&format!("process {} did not end within 3 s", child.pid));
            }
            Some(0) => {}
            Some(code) if code.cast_unsigned() == ACCESS_VIOLATION => {
                outcome.crashes += 1;
                note::evidence(&format!(
                    "process {} ended with an access violation",
                    child.pid
                ));
            }
            Some(code) => outcome.other.push(format!(
                "process {} ended with code 0x{:08X}",
                child.pid,
                code.cast_unsigned()
            )),
        }
    }
    for line in &outcome.other {
        note::evidence(line);
    }
    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}
