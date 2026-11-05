//! A provider process serves one UIA request at a time
//! (`uia-facts/a-provider-process-serves-one-uia-request-at-a-time/readme.md`).
//!
//! The provider program has windows 0 and 1 on UI thread 0 and window 2
//! on UI thread 1, with `UseComThreading` unless an arm says otherwise.
//! Every element is fetched before a hold or a busy thread.
//!
//! - `no-overlap`: 8 client threads make 50 name reads each across the
//!   three windows; the provider's peak of calls in progress at once. Both
//!   threading modes, 3 runs each.
//! - `held`: node 1's name read in window 0 held for 1,000 ms, while
//!   probes run: node 2 of window 0 from another thread, window 2 (the
//!   other UI thread), a remote operation on window 0's node 3, a read
//!   from a second client process into window 1, and a read into a second
//!   provider process. 3 trials.
//! - `fifo`: UI thread 0 busy; a read queued into window 0, then 200 ms
//!   later a read into window 2, on the idle thread; released at 1,000 ms.
//!   3 trials.
//! - `busy-only`: UI thread 0 busy with nothing queued; a read into window
//!   2. 3 trials.
//! - `pumping`: node 1's name read in window 0 waiting in
//!   `CoWaitForMultipleHandles` until released; a read into window 2.
//!   3 trials.
//! - `nocom`: the provider without `UseComThreading`, UI thread 0 busy: a
//!   read of window 0's node 1, a new connection to window 0, then an
//!   `Invoke` of node 1. 3 trials.
//! - `recovery`: `held`, `fifo`, `busy-only`, `pumping` and `nocom` again
//!   with connection recovery on in every client object, their outcomes
//!   printed as new (Not known).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, Shared, Timed};
use uia_facts_support::{exit_now, now_us, rop};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationInvokePattern, UIA_InvokePatternId,
};

/// Whether every client object of this run has connection recovery on.
static RECOVERY: AtomicBool = AtomicBool::new(false);
/// How long a held call is held.
const HOLD: Duration = Duration::from_millis(1_000);
const LAYOUT: &str = "uia:0,uia:0,uia:1";

fn client() -> IUIAutomation6 {
    let client = uia::client();
    uia::set_timeouts(&client, 2_000, 30_000);
    uia::set_recovery(&client, RECOVERY.load(Ordering::SeqCst));
    client
}

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "read", "the one role is read");
        read_role(rest[0].parse().expect("a window handle"));
    }
    let mut report = Report::start("a-provider-process-serves-one-uia-request-at-a-time");
    if args.wants("no-overlap") {
        no_overlap(&mut report);
    }
    let mut runs_of = vec![false];
    if args.wants("recovery") {
        runs_of.push(true);
    }
    for recovery in runs_of {
        RECOVERY.store(recovery, Ordering::SeqCst);
        let mut judge = Judge {
            report: &mut report,
            recovery,
        };
        if args.wants("held") || recovery {
            held(&args, &mut judge);
        }
        if args.wants("fifo") || recovery {
            fifo(&mut judge);
        }
        if args.wants("busy-only") || recovery {
            busy_only(&mut judge);
        }
        if args.wants("pumping") || recovery {
            pumping(&mut judge);
        }
        if args.wants("nocom") || recovery {
            nocom(&mut judge);
        }
    }
    report.finish();
}

/// Claims judged by their record, or, with connection recovery on, printed
/// as new.
struct Judge<'a> {
    report: &'a mut Report,
    recovery: bool,
}

impl Judge<'_> {
    fn claim(&mut self, claim: &str, observed: &str, recorded: &str, verdict: Verdict) {
        if self.recovery {
            self.report.claim(
                &format!("with connection recovery on, {claim}"),
                observed,
                "",
                Verdict::New,
            );
        } else {
            self.report.claim(claim, observed, recorded, verdict);
        }
    }
}

/// The second client process: reads node 1 of window `hwnd`, fetched at
/// start, when told `go`, and reports `done <outcome> <end>`.
fn read_role(hwnd: isize) -> ! {
    let client = client();
    let root = uia::element(&client, hwnd).expect("the window's element");
    let first = uia::node(&client, &root, 1).expect("node 1");
    role::report("ready");
    for command in role::commands() {
        if command == "go" {
            let read = Timed::of(|| uia::name(&first));
            role::report(&format!("done {} {}", read.outcome, read.end));
        }
    }
    exit_now(0)
}

/// Node `node` of window `window`, fetched now, to read on another thread.
fn node(client: &IUIAutomation6, provider: &Provider, window: usize, node: usize) -> Shared {
    let root = uia::element(client, provider.hwnd(window)).expect("the window's element");
    uia::share(&uia::node(client, &root, node).expect("the node"))
}

fn read_on_thread(element: Shared) -> std::thread::JoinHandle<Timed> {
    uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&element))))
}

fn no_overlap(report: &mut Report) {
    for threading in ["com", "nocom"] {
        let mut peaks = Vec::new();
        for _ in 0..3 {
            let mut provider = Provider::launch(&["--windows", LAYOUT, "--threading", threading]);
            let client = client();
            let elements: Vec<Shared> = (0..3)
                .flat_map(|w| {
                    [
                        node(&client, &provider, w, 1),
                        node(&client, &provider, w, 2),
                    ]
                })
                .collect();
            provider.request("peak");
            let threads: Vec<_> = (0..8)
                .map(|t| {
                    let mine = elements[t % elements.len()].clone();
                    uia::spawn(move || {
                        let element = uia::resolve(&mine);
                        (0..50).all(|_| uia::name(&element).is_ok())
                    })
                })
                .collect();
            let all = threads.into_iter().all(|t| t.join().expect("a reader"));
            assert!(all, "every read succeeds");
            peaks.push(provider.request("peak"));
        }
        report.claim(
            &format!(
                "8 client threads' reads ({threading}): the most provider calls in progress at once"
            ),
            &runs(&peaks),
            "1",
            deterministic(&peaks, &"1".to_owned()),
        );
    }
}

fn held(args: &Args, judge: &mut Judge) {
    let mut same_window = Vec::new();
    let mut other_thread = Vec::new();
    let mut remote = Vec::new();
    let mut other_client = Vec::new();
    let mut other_process = Vec::new();
    let mut behind = Vec::new();
    for _ in 0..3 {
        let mut provider = Provider::launch(&["--windows", LAYOUT]);
        let other = Provider::launch(&["--windows", "uia:0"]);
        let client = client();
        let first = node(&client, &provider, 0, 1);
        let second = node(&client, &provider, 0, 2);
        let far = node(&client, &provider, 2, 1);
        let notes = node(&client, &provider, 0, 3);
        let elsewhere = node(&client, &other, 0, 1);
        let mut role = Role::start("read", &[provider.hwnd(1).to_string()]);
        assert_eq!(role.expect_line(), "ready", "the second client is ready");
        args.pause_for_debugger(&[
            ("provider", provider.pid),
            ("client", std::process::id()),
            ("second client", role.pid),
        ]);
        provider.log();
        provider.request("hold GetPropertyValue w=0 n=1");
        let stuck = read_on_thread(first);
        provider
            .held(Duration::from_secs(30))
            .expect("the read is held");
        let probes = [read_on_thread(second), read_on_thread(far)];
        let notes_probe = uia::spawn(move || Timed::of(|| rop::name(&uia::resolve(&notes))));
        let elsewhere_probe = read_on_thread(elsewhere);
        role.send("go");
        let elsewhere = elsewhere_probe.join().expect("the other process's read");
        uia::pause(HOLD);
        let released = now_us();
        provider.request("release");
        let [second, far] = probes.map(|p| p.join().expect("a probe"));
        let notes = notes_probe.join().expect("the remote operation");
        let words: Vec<String> = role.expect_line().split(' ').map(str::to_owned).collect();
        let role_end: u64 = words[2].parse().expect("a time");
        let _ = stuck.join();
        same_window.push(second.end >= released && second.outcome == "ok");
        other_thread.push(far.end >= released && far.outcome == "ok");
        remote.push(notes.end >= released && notes.outcome == "ok");
        other_client.push(role_end >= released && words[1] == "ok");
        other_process.push(elsewhere.end < released && elsewhere.outcome == "ok");
        let log = provider.log();
        let held_end = log
            .iter()
            .find(|e| e.is_call("GetPropertyValue") && e.window == Some(0) && e.node == Some(1))
            .map_or(u64::MAX, |e| e.end);
        behind.push(
            log.iter()
                .filter(|e| {
                    e.is_call("GetPropertyValue") && e.window == Some(0) && e.node == Some(2)
                })
                .all(|e| e.start >= held_end),
        );
    }
    judge.claim(
        "a read of another element of the held window waits for the release",
        &runs(&same_window),
        "true",
        relation(&same_window),
    );
    judge.claim(
        "a read into the other UI thread's window waits for the release",
        &runs(&other_thread),
        "true",
        relation(&other_thread),
    );
    judge.claim(
        "a remote operation into the held window waits for the release",
        &runs(&remote),
        "true",
        relation(&remote),
    );
    judge.claim(
        "a read from another client process waits for the release",
        &runs(&other_client),
        "true",
        relation(&other_client),
    );
    judge.claim(
        "a read into another provider process answers during the hold",
        &runs(&other_process),
        "true",
        relation(&other_process),
    );
    judge.claim(
        "the provider starts the waiting read only once the held call returns",
        &runs(&behind),
        "true",
        relation(&behind),
    );
}

fn fifo(judge: &mut Judge) {
    let mut orders = Vec::new();
    let mut waited = Vec::new();
    for _ in 0..3 {
        let mut provider = Provider::launch(&["--windows", LAYOUT]);
        let client = client();
        let first = node(&client, &provider, 0, 1);
        let far = node(&client, &provider, 2, 1);
        provider.log();
        provider.request("busy 0");
        let queued = read_on_thread(first);
        // The offset that orders the two requests: nothing marks a request
        // queued on a busy thread, so the second is sent 200 ms after.
        uia::pause(Duration::from_millis(200));
        let idle = read_on_thread(far);
        uia::pause(HOLD.saturating_sub(Duration::from_millis(200)));
        let released = now_us();
        provider.request("release");
        let idle = idle.join().expect("the idle thread's read");
        let _ = queued.join();
        let log = provider.log();
        let order: Vec<usize> = log
            .iter()
            .filter(|e| e.is_call("GetPropertyValue"))
            .filter_map(|e| e.window)
            .fold(Vec::new(), |mut seen, w| {
                if !seen.contains(&w) {
                    seen.push(w);
                }
                seen
            });
        orders.push(order);
        waited.push(idle.end >= released);
    }
    judge.claim(
        "the provider serves the busy thread's earlier request before the idle thread's later one",
        &runs(&orders),
        "window 0, then window 2",
        deterministic(&orders, &vec![0, 2]),
    );
    judge.claim(
        "the idle thread's read waits for the release",
        &runs(&waited),
        "true",
        relation(&waited),
    );
}

fn busy_only(judge: &mut Judge) {
    let mut answered = Vec::new();
    let mut times = Vec::new();
    for _ in 0..3 {
        let mut provider = Provider::launch(&["--windows", LAYOUT]);
        let client = client();
        let far = node(&client, &provider, 2, 1);
        provider.request("busy 0");
        let read = read_on_thread(far).join().expect("the read");
        let released = now_us();
        provider.request("release");
        answered.push(read.end < released && read.outcome == "ok");
        times.push(read.ms());
    }
    judge.claim(
        "with UI thread 0 busy and nothing queued on it, a read into the other UI thread answers",
        &runs(&answered),
        "true",
        relation(&answered),
    );
    judge.claim(
        "that read's time",
        &summary_ms(&times),
        "0.4 to 1.2 ms",
        Verdict::Trend,
    );
}

fn pumping(judge: &mut Judge) {
    let mut waited = Vec::new();
    for _ in 0..3 {
        let mut provider = Provider::launch(&["--windows", LAYOUT]);
        let client = client();
        let first = node(&client, &provider, 0, 1);
        let far = node(&client, &provider, 2, 1);
        provider.request("hold GetPropertyValue w=0 n=1 pump");
        let stuck = read_on_thread(first);
        provider
            .held(Duration::from_secs(30))
            .expect("the read is held");
        let probe = read_on_thread(far);
        uia::pause(HOLD);
        let released = now_us();
        provider.request("release");
        let probe = probe.join().expect("the probe");
        let _ = stuck.join();
        waited.push(probe.end >= released && probe.outcome == "ok");
    }
    judge.claim(
        "a provider call waiting in CoWaitForMultipleHandles holds a read into the other UI thread",
        &runs(&waited),
        "true",
        relation(&waited),
    );
}

fn nocom(judge: &mut Judge) {
    let mut read_answers = Vec::new();
    let mut connection_outcomes = Vec::new();
    let mut connection_times = Vec::new();
    let mut invoke_waits = Vec::new();
    for _ in 0..3 {
        let mut provider = Provider::launch(&["--windows", "uia:0,uia:1", "--threading", "nocom"]);
        let client = client();
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::node(&client, &root, 1).expect("node 1");
        // SAFETY: a live element; the invoke pattern's interface.
        let invoke: IUIAutomationInvokePattern =
            unsafe { first.GetCurrentPatternAs(UIA_InvokePatternId) }.expect("the invoke pattern");
        let first = uia::share(&first);
        let invoke = windows::core::AgileReference::new(&invoke).expect("an agile reference");
        provider.request("busy 0");
        let read = read_on_thread(first).join().expect("the read");
        let fresh = client.clone();
        let hwnd = provider.hwnd(0);
        let connection = Timed::of(|| uia::element(&fresh, hwnd));
        let invoked = uia::spawn(move || {
            // SAFETY: a live pattern, resolved on this thread.
            Timed::of(|| unsafe { invoke.resolve().expect("the pattern").Invoke() })
        });
        uia::pause(Duration::from_millis(300));
        let released = now_us();
        provider.request("release");
        let invoked = invoked.join().expect("the invoke");
        read_answers.push(read.end < released && read.outcome == "ok");
        connection_outcomes.push(connection.outcome.clone());
        connection_times.push(connection.ms());
        invoke_waits.push(invoked.end >= released && invoked.outcome == "ok");
    }
    judge.claim(
        "without UseComThreading, a read answers while the UI thread is busy",
        &runs(&read_answers),
        "true",
        relation(&read_answers),
    );
    judge.claim(
        "without UseComThreading, a new connection to the busy window fails with UIA_E_TIMEOUT",
        &runs(&connection_outcomes),
        "UIA_E_TIMEOUT",
        deterministic(&connection_outcomes, &"UIA_E_TIMEOUT".to_owned()),
    );
    let window = band(&[2_000.0]);
    judge.claim(
        "it fails at the connection timeout",
        &summary_ms(&connection_times),
        &band_text(window),
        in_band(&connection_times, window),
    );
    judge.claim(
        "without UseComThreading, an Invoke waits for the UI thread",
        &runs(&invoke_waits),
        "true",
        relation(&invoke_waits),
    );
}
