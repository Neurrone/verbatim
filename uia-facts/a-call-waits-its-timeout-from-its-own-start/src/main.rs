//! A call waits its timeout from its own start
//! (`uia-facts/a-call-waits-its-timeout-from-its-own-start/readme.md`).
//!
//! Every arm runs against the provider program with one UIA window, the
//! elements fetched before the provider holds node 1's
//! `GetPropertyValue`, and the connection timeout at 2,000 ms.
//!
//! - `waits`: `CurrentName` of node 1, held, at transaction timeouts of
//!   2,000, 5,000 and 20,000 ms; 5 trials each.
//! - `single-reads`: E-TT's reads at a 1,000 ms transaction timeout, both
//!   timeouts 1,000 ms: `CurrentName` and `CurrentHasKeyboardFocus` of node
//!   1, held; 5 trials each.
//! - `own-start`: at 2,000 ms, node 1's name read held on one thread, and
//!   200 ms later node 2's name read on another; 5 trials. Each wait is
//!   timed from its own call's start, and after the release the provider
//!   log shows node 2's read served.
//! - `timer`: the `waits` arm at 2,000 ms with the system timer at 1 ms
//!   (`timeBeginPeriod(1)`); 5 trials.

use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, Timed};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::UI::Accessibility::{IUIAutomation6, IUIAutomationElement};

const TIMEOUT: &str = "UIA_E_TIMEOUT";
/// One read of an element.
type Read = fn(&IUIAutomationElement) -> windows::core::Result<()>;
/// How long the second read starts after the first: a deliberate offset,
/// the original runs' 200 ms, so the two waits can be told apart.
const OFFSET: Duration = Duration::from_millis(200);

/// The provider program with its root and nodes 1 and 2 fetched.
struct Setup {
    provider: Provider,
    client: IUIAutomation6,
    first: IUIAutomationElement,
    second: IUIAutomationElement,
}

impl Setup {
    fn new(connection_ms: u32, transaction_ms: u32) -> Self {
        let provider = Provider::launch(&["--windows", "uia:0"]);
        let client = uia::client();
        uia::set_timeouts(&client, connection_ms, transaction_ms);
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::node(&client, &root, 1).expect("node 1");
        let second = uia::node(&client, &root, 2).expect("node 2");
        Self {
            provider,
            client,
            first,
            second,
        }
    }

    /// Holds node 1's `GetPropertyValue`, times `read` of node 1, and
    /// releases.
    fn held_read(
        &mut self,
        read: impl Fn(&IUIAutomationElement) -> windows::core::Result<()>,
    ) -> Timed {
        self.provider.request("hold GetPropertyValue w=0 n=1");
        let timed = Timed::of(|| read(&self.first));
        self.provider.request("release");
        self.provider.clear_held();
        timed
    }
}

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("a-call-waits-its-timeout-from-its-own-start");
    if args.wants("waits") {
        for (timeout, recorded) in [(2_000, "2.2 s"), (5_000, "5.5 s"), (20_000, "22.1 s")] {
            waits(&args, &mut report, timeout, recorded, "");
        }
    }
    if args.wants("single-reads") {
        single_reads(&args, &mut report);
    }
    if args.wants("own-start") {
        own_start(&args, &mut report);
    }
    if args.wants("timer") {
        // SAFETY: no precondition; undone below.
        let set = unsafe { timeBeginPeriod(1) };
        assert_eq!(set, 0, "the timer resolution is set");
        waits(
            &args,
            &mut report,
            2_000,
            "",
            " with the system timer at 1 ms",
        );
        // SAFETY: undoes the call above.
        let _ = unsafe { timeEndPeriod(1) };
    }
    report.finish();
}

fn waits(args: &Args, report: &mut Report, timeout: u32, recorded: &str, condition: &str) {
    let mut setup = Setup::new(2_000, timeout);
    args.pause_for_debugger(&[
        ("provider", setup.provider.pid),
        ("client", std::process::id()),
    ]);
    let trials: Vec<Timed> = (0..5)
        .map(|_| setup.held_read(|e| uia::name(e).map(|_| ())))
        .collect();
    let outcomes: Vec<String> = trials.iter().map(|t| t.outcome.clone()).collect();
    let times: Vec<f64> = trials.iter().map(Timed::ms).collect();
    report.claim(
        &format!("a held name read at a {timeout} ms transaction timeout{condition} fails with UIA_E_TIMEOUT"),
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    if condition.is_empty() {
        let window = band(&[f64::from(timeout)]);
        report.claim(
            &format!("it fails after about 1.1 times {timeout} ms"),
            &summary_ms(&times),
            &format!("{recorded}, {}", band_text(window)),
            in_band(&times, window),
        );
    } else {
        report.claim(
            &format!("its wait{condition}, against the 100 ms slices inferred"),
            &summary_ms(&times),
            "",
            Verdict::New,
        );
    }
}

fn single_reads(args: &Args, report: &mut Report) {
    let mut setup = Setup::new(1_000, 1_000);
    args.pause_for_debugger(&[
        ("provider", setup.provider.pid),
        ("client", std::process::id()),
    ]);
    let window = band(&[1_000.0]);
    let reads: [(&str, Read); 2] = [
        ("CurrentName", |e| uia::name(e).map(|_| ())),
        ("CurrentHasKeyboardFocus", |e| {
            // SAFETY: a live element.
            unsafe { e.CurrentHasKeyboardFocus() }.map(|_| ())
        }),
    ];
    for (name, read) in reads {
        let trials: Vec<Timed> = (0..5).map(|_| setup.held_read(read)).collect();
        let outcomes: Vec<String> = trials.iter().map(|t| t.outcome.clone()).collect();
        let times: Vec<f64> = trials.iter().map(Timed::ms).collect();
        report.claim(
            &format!("E-TT's held {name} at a 1,000 ms timeout fails with UIA_E_TIMEOUT"),
            &runs(&outcomes),
            TIMEOUT,
            deterministic(&outcomes, &TIMEOUT.to_owned()),
        );
        report.claim(
            &format!("E-TT's held {name} fails after one timeout"),
            &summary_ms(&times),
            &format!("1,082 to 1,095 ms, {}", band_text(window)),
            in_band(&times, window),
        );
    }
}

fn own_start(args: &Args, report: &mut Report) {
    let mut setup = Setup::new(2_000, 2_000);
    args.pause_for_debugger(&[
        ("provider", setup.provider.pid),
        ("client", std::process::id()),
    ]);
    let mut seconds = Vec::new();
    let mut outcomes = Vec::new();
    let mut served = Vec::new();
    for _ in 0..5 {
        setup.provider.log();
        setup.provider.request("hold GetPropertyValue w=0 n=1");
        let first = uia::share(&setup.first);
        let second = uia::share(&setup.second);
        let a = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&first))));
        setup
            .provider
            .held(Duration::from_secs(30))
            .expect("the first read reaches the provider");
        uia::pause(OFFSET);
        let b = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&second))));
        let a = a.join().expect("the first read returns");
        let b = b.join().expect("the second read returns");
        let released = uia_facts_support::now_us();
        setup.provider.request("release");
        seconds.push(b.ms());
        outcomes.push((a.outcome.clone(), b.outcome.clone()));
        served.push(served_after(&mut setup.provider, released));
        setup.provider.clear_held();
    }
    let _ = &setup.client;
    let pair = (TIMEOUT.to_owned(), TIMEOUT.to_owned());
    report.claim(
        "both reads, the second queued behind the first, fail with UIA_E_TIMEOUT",
        &runs(&outcomes),
        "both UIA_E_TIMEOUT",
        deterministic(&outcomes, &pair),
    );
    let window = band(&[2_000.0]);
    report.claim(
        "the queued read fails one timeout after its own start",
        &summary_ms(&seconds),
        &format!("2,198 ms, {}", band_text(window)),
        in_band(&seconds, window),
    );
    report.claim(
        "after the release, the provider serves the queued read that already failed",
        &format!("{served:?}"),
        "node 2's read served after the release",
        relation(&served),
    );
}

/// Whether the provider log shows node 2's `GetPropertyValue` starting
/// after `released`, watched for up to 5 seconds.
fn served_after(provider: &mut Provider, released: u64) -> bool {
    provider
        .log_until(Duration::from_secs(5), |e| {
            e.is_call("GetPropertyValue") && e.node == Some(2) && e.start >= released
        })
        .1
}
