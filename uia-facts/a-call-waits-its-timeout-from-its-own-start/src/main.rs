//! A call waits its timeout from its own start
//! (`uia-facts/a-call-waits-its-timeout-from-its-own-start/readme.md`).
//!
//! Every case runs against the provider program with one UIA window, the
//! elements fetched before the provider holds node 1's
//! `GetPropertyValue`, and the connection timeout at 2,000 ms.
//!
//! - `waits`: `CurrentName` of node 1, held, at transaction timeouts of
//!   2,000 and 5,000 ms, each trial against a provider of its own (the
//!   20,000 ms default's 22.1 s is not run);
//!   then E-TT's reads at a 1,000 ms transaction timeout, both timeouts
//!   1,000 ms: `CurrentName` and `CurrentHasKeyboardFocus` of node 1, held,
//!   on one provider, so that with connection recovery on the second fails
//!   at once.
//! - `own-start`: at 1,000 ms, node 1's name read held on one thread, and
//!   200 ms later node 2's name read on another. Each wait is
//!   timed from its own call's start, and after the release the provider
//!   log shows node 2's read served.
//! - `timer`: the `waits` case at 1,000 ms with the system timer at 1 ms
//!   (`timeBeginPeriod(1)`).
//! - `mark`: with both timeouts at 1,000 ms, node 1's name read held until
//!   it times out; then, the provider still held, the same element's read
//!   from another thread, and a read through another client object of
//!   this process; then the provider released, and node 1 read every
//!   10 ms until a read succeeds. Connection recovery's mark, by which a
//!   read made right after one into the same application timed out fails
//!   at once: whether it reaches the other thread and the other client
//!   object, and how long it lasts after the provider answers again.

use std::time::{Duration, Instant};

use uia_facts_support::args::{Args, trials};
use uia_facts_support::note;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, band, band_text, deterministic, in_band, relation, runs};
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
        note::waiting_ms(
            uia::timeouts(&self.client).1,
            "the held read's transaction timeout",
        );
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
        report.case("waits", "holding node 1's name read in the provider at transaction timeouts of 2 and 5 s, then E-TT's name and keyboard-focus reads at 1 s; timing each read until it fails");
        println!(
            "system timer resolution in effect: {:.4} ms",
            timer_resolution_ms()
        );
        for (timeout, recorded) in [(2_000, "2.2 s"), (5_000, "5.5 s")] {
            waits(&args, &mut report, timeout, recorded, "", Factor::Tenth);
        }
        single_reads(&args, &mut report);
    }
    if args.wants("own-start") {
        report.case("own-start", "holding node 1's read at a 1 s timeout, then starting node 2's read on another thread 200 ms later; timing the second from its own start");
        own_start(&args, &mut report);
    }
    if args.wants("timer") {
        report.case("timer", "setting the system timer to 1 ms; holding node 1's read at a 1 s timeout and timing it");
        // SAFETY: no precondition; undone below.
        let set = unsafe { timeBeginPeriod(1) };
        assert_eq!(set, 0, "the timer resolution is set");
        // The resolution in effect, read rather than assumed: another
        // process, or Windows, may hold it elsewhere.
        let resolution = timer_resolution_ms();
        println!("system timer resolution in effect: {resolution:.4} ms");
        if resolution <= 1.0 {
            waits(
                &args,
                &mut report,
                1_000,
                "1,001.5 to 1,028.5 ms in 7 runs",
                " with the system timer at 1 ms",
                Factor::Whole,
            );
        } else {
            report.unmet(
                "a held read at a 1000 ms timeout with the system timer at 1 ms",
                &format!("the resolution in effect is {resolution:.4} ms, not 1 ms or finer"),
            );
        }
        // SAFETY: undoes the call above.
        let _ = unsafe { timeEndPeriod(1) };
    }
    if args.wants("mark") {
        report.case("mark", "timing out node 1's held read at 1 s; reading it again, the provider still held, from another thread and through another client object; then releasing the provider and reading node 1 every 10 ms until a read succeeds");
        mark(&args, &mut report);
    }
    report.finish();
}

/// How long a held read waits against its timeout T: the timeout and a
/// tenth with the system timer left alone, the timeout itself with the
/// timer at 1 ms. The two bands meet at 1.055 T, so a wait lies in one or
/// the other.
#[derive(Clone, Copy)]
enum Factor {
    /// From 1.055 T to the band rule's top, 1.15 T plus 50 ms.
    Tenth,
    /// From T less a tick to 1.055 T.
    Whole,
}

impl Factor {
    fn band(self, timeout: f64) -> (f64, f64) {
        let (low, high) = band(&[timeout]);
        match self {
            Factor::Tenth => (timeout * 1.055, high),
            Factor::Whole => (low, timeout * 1.055),
        }
    }

    fn words(self) -> &'static str {
        match self {
            Factor::Tenth => "about 1.1 times",
            Factor::Whole => "about",
        }
    }
}

/// The system timer's resolution in effect, in milliseconds, from
/// `NtQueryTimerResolution`, whose figures are in 100-nanosecond units.
fn timer_resolution_ms() -> f64 {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryTimerResolution(coarsest: *mut u32, finest: *mut u32, current: *mut u32) -> i32;
    }
    let (mut coarsest, mut finest, mut current) = (0u32, 0u32, 0u32);
    // SAFETY: three live values to fill.
    let status =
        unsafe { NtQueryTimerResolution(&raw mut coarsest, &raw mut finest, &raw mut current) };
    assert_eq!(status, 0, "NtQueryTimerResolution answers");
    f64::from(current) / 10_000.0
}

fn waits(
    args: &Args,
    report: &mut Report,
    timeout: u32,
    recorded: &str,
    condition: &str,
    factor: Factor,
) {
    // Each trial has its own provider and client object, so under
    // connection recovery no trial's timed-out read fails the next at once.
    let trials: Vec<Timed> = (0..trials())
        .map(|_| {
            let mut setup = Setup::new(2_000, timeout);
            args.pause_for_debugger(&[
                ("provider", setup.provider.pid),
                ("client", std::process::id()),
            ]);
            setup.held_read(|e| uia::name(e).map(|_| ()))
        })
        .collect();
    let outcomes: Vec<String> = trials.iter().map(|t| t.outcome.clone()).collect();
    let times: Vec<f64> = trials.iter().map(Timed::ms).collect();
    report.claim(
        &format!("a held name read at a {timeout} ms transaction timeout{condition} fails with UIA_E_TIMEOUT"),
        &runs(&outcomes),
        TIMEOUT,
        deterministic(&outcomes, &TIMEOUT.to_owned()),
    );
    let window = factor.band(f64::from(timeout));
    report.claim(
        &format!("it fails after {} {timeout} ms{condition}", factor.words()),
        &summary_ms(&times),
        &format!("{recorded}, {}", band_text(window)),
        in_band(&times, window),
    );
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
        let trials: Vec<Timed> = (0..trials()).map(|_| setup.held_read(read)).collect();
        let outcomes: Vec<String> = trials.iter().map(|t| t.outcome.clone()).collect();
        let times: Vec<f64> = trials.iter().map(Timed::ms).collect();
        report.claim(
            &format!("E-TT's held {name} at a 1,000 ms timeout fails with UIA_E_TIMEOUT"),
            &runs(&outcomes),
            TIMEOUT,
            deterministic(&outcomes, &TIMEOUT.to_owned()),
        );
        if uia::settings().recovery {
            // With connection recovery on, a read made after one into the
            // same process timed out fails at once; only the first waits.
            let (first, later) = if name == "CurrentName" {
                (&times[..1], &times[1..])
            } else {
                (&times[..0], &times[..])
            };
            if !first.is_empty() {
                report.claim(
                    &format!("E-TT's first held {name} fails after one timeout"),
                    &summary_ms(first),
                    &format!("1,082 to 1,095 ms, {}", band_text(window)),
                    in_band(first, window),
                );
            }
            if later.is_empty() {
                continue;
            }
            let at_once: Vec<bool> = later.iter().map(|ms| *ms < 1_000.0).collect();
            report.claim(
                &format!(
                    "with connection recovery on, E-TT's held {name}, made after a read timed out, fails before the timeout"
                ),
                &format!("{} ({})", runs(&at_once), summary_ms(later)),
                "true, 0.1 ms",
                relation(&at_once),
            );
        } else {
            report.claim(
                &format!("E-TT's held {name} fails after one timeout"),
                &summary_ms(&times),
                &format!("1,082 to 1,095 ms, {}", band_text(window)),
                in_band(&times, window),
            );
        }
    }
}

fn own_start(args: &Args, report: &mut Report) {
    let mut seconds = Vec::new();
    let mut outcomes = Vec::new();
    let mut served = Vec::new();
    for _ in 0..trials() {
        // A provider and client object of its own each trial, so that
        // under connection recovery no trial's timed-out reads fail the
        // next at once.
        let mut setup = Setup::new(1_000, 1_000);
        args.pause_for_debugger(&[
            ("provider", setup.provider.pid),
            ("client", std::process::id()),
        ]);
        setup.provider.log();
        setup.provider.request("hold GetPropertyValue w=0 n=1");
        let first = uia::share(&setup.first);
        let second = uia::share(&setup.second);
        let a = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&first))));
        setup
            .provider
            .held(Duration::from_secs(30))
            .expect("the first read reaches the provider");
        uia::pause(OFFSET, "the deliberate 200 ms before the second read");
        let b = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&second))));
        let a = a.join().expect("the first read returns");
        let b = b.join().expect("the second read returns");
        let released = uia_facts_support::now_us();
        setup.provider.request("release");
        seconds.push(b.ms());
        outcomes.push((a.outcome.clone(), b.outcome.clone()));
        served.push(served_after(&mut setup.provider, released));
        let _ = &setup.client;
    }
    let pair = (TIMEOUT.to_owned(), TIMEOUT.to_owned());
    report.claim(
        "both reads, the second queued behind the first, fail with UIA_E_TIMEOUT",
        &runs(&outcomes),
        "both UIA_E_TIMEOUT",
        deterministic(&outcomes, &pair),
    );
    let window = band(&[1_000.0]);
    report.claim(
        "the queued read fails one timeout after its own start",
        &summary_ms(&seconds),
        &format!("2,198 ms at 2,000 ms, {}", band_text(window)),
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

/// How long `mark` reads node 1 after the release before it gives up
/// waiting for a read to succeed.
const MARK_WATCH: Duration = Duration::from_secs(10);

/// One `mark` trial: the first held read, the two reads made while the
/// provider was still held, and, after the release, how long until a read
/// succeeded and how many failed first.
struct Mark {
    first: Timed,
    other_thread: Timed,
    other_client: Timed,
    succeeded_after: Option<f64>,
    failed_first: usize,
}

/// Whether `read` failed at once: `UIA_E_TIMEOUT` before half its 1,000 ms
/// timeout, so not after a wait of its own.
fn at_once(read: &Timed) -> bool {
    read.outcome == TIMEOUT && read.ms() < 500.0
}

fn mark_trial(args: &Args) -> Mark {
    let mut setup = Setup::new(1_000, 1_000);
    let other = uia::client();
    let other_root = uia::element(&other, setup.provider.hwnd(0)).expect("the window's element");
    let other_first = uia::share(&uia::node(&other, &other_root, 1).expect("node 1"));
    args.pause_for_debugger(&[
        ("provider", setup.provider.pid),
        ("client", std::process::id()),
    ]);
    setup.provider.request("hold GetPropertyValue w=0 n=1");
    note::waiting_ms(1_000, "the held read's transaction timeout");
    let first = Timed::of(|| uia::name(&setup.first));
    // The provider still held: the same element from another thread, then
    // the other client object's element, in turn.
    let shared = uia::share(&setup.first);
    note::waiting_ms(
        1_000,
        "the read from another thread, unless it fails at once",
    );
    let other_thread = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&shared))))
        .join()
        .expect("the read returns");
    note::waiting_ms(
        1_000,
        "the other client object's read, unless it fails at once",
    );
    let other_client = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&other_first))))
        .join()
        .expect("the read returns");
    let released = Instant::now();
    setup.provider.request("release");
    setup.provider.clear_held();
    // Read every 10 ms until one succeeds: the mark's length after the
    // provider answers again.
    let mut failed_first = 0;
    let mut succeeded_after = None;
    while released.elapsed() < MARK_WATCH {
        if uia::name(&setup.first).is_ok() {
            succeeded_after = Some(uia::ms(released.elapsed()));
            break;
        }
        failed_first += 1;
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(other);
    Mark {
        first,
        other_thread,
        other_client,
        succeeded_after,
        failed_first,
    }
}

/// One of a `mark` trial's reads.
type Pick = fn(&Mark) -> &Timed;

fn mark(args: &Args, report: &mut Report) {
    let trials: Vec<Mark> = (0..trials()).map(|_| mark_trial(args)).collect();
    let firsts: Vec<f64> = trials.iter().map(|t| t.first.ms()).collect();
    let window = band(&[1_000.0]);
    report.claim(
        "the first held read fails after one timeout",
        &summary_ms(&firsts),
        &format!("1,082 to 1,095 ms, {}", band_text(window)),
        in_band(&firsts, window),
    );
    let picks: [(&str, Pick); 2] = [
        ("from another thread", |t| &t.other_thread),
        ("through another client object of this process", |t| {
            &t.other_client
        }),
    ];
    // The first runs' answers (2026-11-06): with connection recovery on,
    // both fail at once, in 0.1 to 0.2 ms; with it off, each waits its
    // own timeout.
    let recovery = uia::settings().recovery;
    for (what, pick) in picks {
        let reads: Vec<&Timed> = trials.iter().map(pick).collect();
        let fast: Vec<bool> = reads.iter().map(|r| at_once(r)).collect();
        let times: Vec<f64> = reads.iter().map(|r| r.ms()).collect();
        let outcomes: Vec<String> = reads.iter().map(|r| r.outcome.clone()).collect();
        report.claim(
            &format!(
                "the provider still held, a read {what}, made right after one timed out, fails at once"
            ),
            &format!(
                "{} ({}; {})",
                runs(&fast),
                runs(&outcomes),
                summary_ms(&times)
            ),
            if recovery {
                "true, UIA_E_TIMEOUT in 0.1 to 0.2 ms (the first runs)"
            } else {
                "false, UIA_E_TIMEOUT after one timeout (the first run)"
            },
            deterministic(&fast, &recovery),
        );
    }
    let after: Vec<String> = trials
        .iter()
        .map(|t| {
            t.succeeded_after.map_or_else(
                || format!("no read succeeded in {} s", MARK_WATCH.as_secs()),
                |ms| format!("{ms:.0} ms, {} reads failed first", t.failed_first),
            )
        })
        .collect();
    if recovery {
        // The first runs' 2,002 to 2,005 ms, judged by the band rule from
        // the 2,004 ms of the first: the length looks set by UIA, not by
        // the machine.
        let lengths: Vec<f64> = trials
            .iter()
            .map(|t| t.succeeded_after.unwrap_or(f64::INFINITY))
            .collect();
        let window = band(&[2_004.0]);
        report.claim(
            "with connection recovery on, after the release, reading every 10 ms: the first read that succeeds",
            &after.join("; "),
            &format!("2,002 to 2,005 ms (the first runs), {}", band_text(window)),
            in_band(&lengths, window),
        );
    } else {
        let at_once: Vec<bool> = trials.iter().map(|t| t.failed_first == 0).collect();
        report.claim(
            "with connection recovery off, after the release, the first read succeeds",
            &format!("{} ({})", runs(&at_once), after.join("; ")),
            "true (the first run)",
            relation(&at_once),
        );
    }
}
