//! Parallel client threads gain at most twice on one application
//! (`uia-facts/parallel-client-threads-gain-at-most-twice-on-one-application/readme.md`).
//!
//! The target is the provider program with one UIA window, served on its
//! UI thread (`UseComThreading`). A bench makes K name reads of nodes 1
//! to 16 on one thread, then the same K split evenly over N threads that
//! start together, N of 2, 4 and 8; each N has a warm-up run, then 7
//! timed runs, and its time is the median until the last thread ends.
//!
//! - `cheap`: K = 4,000 plain name reads.
//! - `costly`: K = 100, every provider call made 5 ms slow, the window
//!   pumping meanwhile (`slow 5`).
//!
//! Each arm's bound is the serial-share bound of the same run: one call's
//! time on one thread divided by the provider's serial time per call, the
//! interval between provider call entries in the 8-thread runs, which is
//! the time the provider's I/O and UI threads spend per call in turn.
//!
//! - `processes`: the cheap bench as N threads of this process against N
//!   client processes of one thread each.
//! - `shared`: the cheap 8-thread bench with one client object for all
//!   threads against one per thread.

use std::sync::{Arc, Barrier};
use std::time::Instant;

use uia_facts_support::args::Args;
use uia_facts_support::exit_now;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, relation};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::{as_f64, median, us_to_ms};
use uia_facts_support::uia::{self, Shared};

const THREADS: [usize; 3] = [2, 4, 8];
const TIMED_RUNS: usize = 7;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "bench", "the one role is bench");
        bench_role(rest);
    }
    let mut report = Report::start("parallel-client-threads-gain-at-most-twice-on-one-application");
    if args.wants("cheap") {
        gains(
            &args,
            &mut report,
            "cheap",
            4_000,
            0,
            "about 2 times (1.74 to 1.98 on the synthetic provider)",
        );
    }
    if args.wants("costly") {
        gains(
            &args,
            &mut report,
            "costly",
            100,
            5,
            "1.0 to 1.5 times (1.01 with a 5 ms provider call)",
        );
    }
    if args.wants("processes") {
        processes(&mut report);
    }
    if args.wants("shared") {
        shared(&mut report);
    }
    report.finish();
}

/// Nodes 1 to 16 of the window, fetched by `client`.
fn nodes(provider: &Provider) -> Vec<Shared> {
    let client = uia::client();
    let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    (1..=16)
        .map(|n| uia::share(&uia::node(&client, &root, n).expect("a node")))
        .collect()
}

/// K reads split over `threads` threads starting together; the
/// milliseconds until the last ends. With `shared`, every thread reads
/// through elements one client object fetched; otherwise each thread
/// fetches its own through a client object of its own first.
fn bench(provider: &Provider, elements: &[Shared], threads: usize, k: usize, shared: bool) -> f64 {
    let barrier = Arc::new(Barrier::new(threads + 1));
    let hwnd = provider.hwnd(0);
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let barrier = Arc::clone(&barrier);
            let mine: Vec<Shared> = elements.to_vec();
            uia::spawn(move || {
                let elements: Vec<_> = if shared {
                    mine.iter().map(uia::resolve).collect()
                } else {
                    let client = uia::client();
                    let root = uia::element(&client, hwnd).expect("the window's element");
                    (1..=16)
                        .map(|n| uia::node(&client, &root, n).expect("a node"))
                        .collect()
                };
                barrier.wait();
                for i in 0..k / threads {
                    let element = &elements[(t + i) % elements.len()];
                    assert!(uia::name(element).is_ok(), "a read succeeds");
                }
            })
        })
        .collect();
    barrier.wait();
    let start = Instant::now();
    for handle in handles {
        handle.join().expect("a reader");
    }
    uia::ms(start.elapsed())
}

/// The median of `TIMED_RUNS` runs after a warm-up.
fn timed(provider: &Provider, elements: &[Shared], threads: usize, k: usize, shared: bool) -> f64 {
    bench(provider, elements, threads, k, shared);
    let runs: Vec<f64> = (0..TIMED_RUNS)
        .map(|_| bench(provider, elements, threads, k, shared))
        .collect();
    median(&runs)
}

/// The provider's serial time per call: the median interval between the
/// starts of consecutive name reads in `log`, in milliseconds.
fn serial_interval(log: &[Entry]) -> f64 {
    let mut starts: Vec<u64> = log
        .iter()
        .filter(|e| e.is_call("GetPropertyValue"))
        .map(|e| e.start)
        .collect();
    starts.sort_unstable();
    let gaps: Vec<f64> = starts.windows(2).map(|w| us_to_ms(w[1] - w[0])).collect();
    median(&gaps)
}

fn gains(args: &Args, report: &mut Report, name: &str, k: usize, slow: u64, recorded: &str) {
    let mut provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    provider.request(&format!("slow {slow}"));
    let one = timed(&provider, &elements, 1, k, true);
    let mut speedups = Vec::new();
    let mut serial = 0.0;
    for threads in THREADS {
        provider.log();
        let time = timed(&provider, &elements, threads, k, true);
        speedups.push((threads, one / time));
        if threads == 8 {
            serial = serial_interval(&provider.log());
        }
    }
    provider.request("slow 0");
    let call = one / as_f64(k);
    let bound = call / serial;
    let best = speedups.iter().map(|s| s.1).fold(0.0, f64::max);
    let text: Vec<String> = speedups
        .iter()
        .map(|(n, s)| format!("{n} threads {s:.2}"))
        .collect();
    report.claim(
        &format!("{name} calls: the speed-up of N threads over one"),
        &text.join(", "),
        recorded,
        Verdict::Trend,
    );
    report.claim(
        &format!("{name} calls: the best speed-up stays below the serial-share bound"),
        &format!("best {best:.2}, bound {bound:.2} (a call {call:.3} ms, serial {serial:.3} ms)"),
        "below the bound (the bound predicted 5 to 14 percent high)",
        relation(&[best <= bound]),
    );
    report.claim(
        &format!("{name} calls: the provider's serial share of one call"),
        &format!("{:.2}", serial / call),
        "about half",
        Verdict::Trend,
    );
}

/// A bench client process: fetches nodes 1 to 16 of window `hwnd`, then
/// on each `go` makes `k` reads and reports `done <ms>`.
fn bench_role(rest: &[String]) -> ! {
    let hwnd: isize = rest[0].parse().expect("a window handle");
    let k: usize = rest[1].parse().expect("a count");
    let client = uia::client();
    let root = uia::element(&client, hwnd).expect("the window's element");
    let elements: Vec<_> = (1..=16)
        .map(|n| uia::node(&client, &root, n).expect("a node"))
        .collect();
    role::report("ready");
    for command in role::commands() {
        if command == "go" {
            let start = Instant::now();
            for i in 0..k {
                assert!(
                    uia::name(&elements[i % elements.len()]).is_ok(),
                    "a read succeeds"
                );
            }
            role::report(&format!("done {}", uia::ms(start.elapsed())));
        }
    }
    exit_now(0)
}

fn processes(report: &mut Report) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    let k = 4_000;
    let mut rows = Vec::new();
    let mut eight = (0.0, 0.0);
    for threads in THREADS {
        let in_threads = timed(&provider, &elements, threads, k, false);
        let mut clients: Vec<Role> = (0..threads)
            .map(|_| {
                Role::start(
                    "bench",
                    &[provider.hwnd(0).to_string(), (k / threads).to_string()],
                )
            })
            .collect();
        for client in &clients {
            assert_eq!(client.expect_line(), "ready", "the client is ready");
        }
        let mut times = Vec::new();
        for _ in 0..=TIMED_RUNS {
            let start = Instant::now();
            for client in &mut clients {
                client.send("go");
            }
            for client in &clients {
                let _ = client.expect_line();
            }
            times.push(uia::ms(start.elapsed()));
        }
        let in_processes = median(&times[1..]);
        rows.push(format!(
            "{threads}: threads {in_threads:.0} ms, processes {in_processes:.0} ms"
        ));
        if threads == 8 {
            eight = (in_threads, in_processes);
        }
    }
    report.claim(
        "N threads against N client processes",
        &rows.join("; "),
        "",
        Verdict::Trend,
    );
    report.claim(
        "8 threads of one client process are as fast as 8 client processes or faster",
        &format!("{:.0} ms against {:.0} ms", eight.0, eight.1),
        "8 processes slower than 8 threads",
        relation(&[eight.0 <= eight.1]),
    );
}

fn shared(report: &mut Report) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    let one = timed(&provider, &elements, 8, 4_000, true);
    let each = timed(&provider, &elements, 8, 4_000, false);
    report.claim(
        "8 threads through one client object's elements, against a client object each",
        &format!("{one:.0} ms against {each:.0} ms"),
        "no difference",
        Verdict::Trend,
    );
}
