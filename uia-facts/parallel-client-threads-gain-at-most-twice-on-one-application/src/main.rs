//! Parallel client threads gain at most twice on one application
//! (`uia-facts/parallel-client-threads-gain-at-most-twice-on-one-application/readme.md`).
//!
//! The target is the provider program with one UIA window, served on its
//! UI thread (`UseComThreading`). A bench makes K name reads of nodes 1
//! to 16 on one thread, then the same K split evenly over N threads that
//! start together. The threads are made before a warm-up run and 3 timed
//! runs and live through them; an N's time is the median until the last
//! thread ends.
//!
//! - `gains`: cheap calls, K = 1,000 plain name reads at N of 2 and 8;
//!   then costly calls, K = 16 with every provider call made 5 ms slow, the
//!   window pumping meanwhile (`slow 5`), at N of 2.
//!
//! Each N's bound is computed from that N's own runs: one call's time on
//! one thread divided by the provider's serial time per call under that
//! load, the provider calls' time from entry to exit per read, which no
//! other call can share. Each N's speed-up is judged at or below the
//! larger of its bound and 1.5, the fact's figure for calls whose cost is
//! in the provider, where the bound is 1.00.
//!
//! - `processes`: the cheap bench as 8 threads of this process against 8
//!   client processes of one thread each.
//! - `shared`: the cheap 8-thread bench, K = 1,000, with one client object
//!   for all threads against one per thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use uia_facts_support::args::Args;
use uia_facts_support::exit_now;
use uia_facts_support::note;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::{as_f64, median, us_to_ms};
use uia_facts_support::uia::{self, Shared};

/// Timed runs after the warm-up, for each figure.
const TIMED_RUNS: usize = 3;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "bench", "the one role is bench");
        bench_role(rest);
    }
    let mut report = Report::start("parallel-client-threads-gain-at-most-twice-on-one-application");
    if args.wants("gains") {
        report.case("gains", "reading names from 1, 2 and 8 client threads with cheap provider calls, then from 1 and 2 with costly ones; comparing each speed-up with the bound from the provider's busy time per call");
        gains(
            &args,
            &mut report,
            ("cheap", 1_000, 0),
            &[2, 8],
            "about 2 times (1.74 to 1.98 on the synthetic provider)",
        );
        gains(
            &args,
            &mut report,
            ("costly", 16, 5),
            &[2],
            "1.0 to 1.5 times (1.01 with a 5 ms provider call)",
        );
    }
    if args.wants("processes") {
        report.case("processes", "reading names from 8 threads of one process and from 8 client processes; comparing their times");
        processes(&args, &mut report);
    }
    if args.wants("shared") {
        report.case("shared", "reading names from 8 threads through one client object's elements, and through a client object each");
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

/// Reading threads kept alive across runs, so that a run times the reads
/// alone: no thread's start, exit or COM teardown falls inside it.
struct Workers {
    go: Vec<Sender<()>>,
    done: Receiver<()>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Workers {
    /// `threads` threads, each making K / `threads` reads a run. With
    /// `shared`, every thread reads through elements one client object
    /// fetched; otherwise each thread fetches its own through a client
    /// object of its own, before the first run.
    fn start(
        provider: &Provider,
        elements: &[Shared],
        threads: usize,
        k: usize,
        shared: bool,
    ) -> Self {
        let hwnd = provider.hwnd(0);
        let (done_tx, done) = channel();
        let (ready_tx, ready) = channel();
        let mut go = Vec::new();
        let mut handles = Vec::new();
        for t in 0..threads {
            let (go_tx, go_rx) = channel::<()>();
            go.push(go_tx);
            let mine: Vec<Shared> = elements.to_vec();
            let (done_tx, ready_tx) = (done_tx.clone(), ready_tx.clone());
            handles.push(uia::spawn(move || {
                let client = (!shared).then(uia::client);
                let elements: Vec<_> = match &client {
                    None => mine.iter().map(uia::resolve).collect(),
                    Some(client) => {
                        let root = uia::element(client, hwnd).expect("the window's element");
                        (1..=16)
                            .map(|n| uia::node(client, &root, n).expect("a node"))
                            .collect()
                    }
                };
                ready_tx.send(()).expect("the bench waits");
                while go_rx.recv().is_ok() {
                    for i in 0..k / threads {
                        let element = &elements[(t + i) % elements.len()];
                        assert!(uia::name(element).is_ok(), "a read succeeds");
                    }
                    done_tx.send(()).expect("the bench waits");
                }
            }));
        }
        for _ in 0..threads {
            ready.recv().expect("a reader is ready");
        }
        Self { go, done, handles }
    }

    /// One run: every thread starts together; the milliseconds until the
    /// last ends.
    fn run(&self) -> f64 {
        let start = Instant::now();
        for go in &self.go {
            go.send(()).expect("a reader waits");
        }
        for _ in 0..self.go.len() {
            self.done.recv().expect("a reader ends its run");
        }
        uia::ms(start.elapsed())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.go.clear();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// `TIMED_RUNS` runs after a warm-up, of `run`.
fn runs_of(run: impl Fn() -> f64) -> Vec<f64> {
    run();
    let runs: Vec<f64> = (0..TIMED_RUNS).map(|_| run()).collect();
    note::evidence(&format!("timed runs, ms: {runs:.1?}"));
    runs
}

/// The median of `TIMED_RUNS` runs after a warm-up, on threads kept alive
/// across the runs.
fn timed(provider: &Provider, elements: &[Shared], threads: usize, k: usize, shared: bool) -> f64 {
    let workers = Workers::start(provider, elements, threads, k, shared);
    median(&runs_of(|| workers.run()))
}

/// The provider's serial time per read: the time from entry to exit of
/// every provider call in `log`, during which no other call into the
/// provider runs, summed and divided by the `reads` the log covers, in
/// milliseconds. A name read makes several provider calls.
fn serial_time(log: &[Entry], reads: usize) -> f64 {
    let busy: u64 = log
        .iter()
        .filter(|e| e.kind == "call")
        .map(|e| e.end.saturating_sub(e.start))
        .sum();
    us_to_ms(busy) / as_f64(reads)
}

/// One bench: `name`, its K reads, and the provider's added milliseconds a
/// call (`slow`), at each of `threads`.
fn gains(
    args: &Args,
    report: &mut Report,
    (name, k, slow): (&str, usize, u64),
    threads: &[usize],
    recorded: &str,
) {
    let mut provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    provider.request(&format!("slow {slow}"));
    note::progress(&format!("{name} calls, 1 thread"));
    let one = timed(&provider, &elements, 1, k, true);
    let call = one / as_f64(k);
    // Each N's speed-up against the bound from the same N's runs: the
    // provider's serial time per call under that load.
    let mut rows = Vec::new();
    let mut below = Vec::new();
    let mut shares = Vec::new();
    for &threads in threads {
        note::progress(&format!("{name} calls, {threads} threads"));
        provider.log();
        let time = timed(&provider, &elements, threads, k, true);
        // The log covers the warm-up and the timed runs, K reads each.
        let serial = serial_time(&provider.log(), k * (TIMED_RUNS + 1));
        let (speedup, bound) = (one / time, call / serial);
        rows.push(format!("{threads} threads {speedup:.2} (bound {bound:.2})"));
        below.push(speedup <= bound.max(1.5));
        shares.push(format!("{threads} threads {:.2}", serial / call));
    }
    provider.request("slow 0");
    report.claim(
        &format!("{name} calls: the speed-up of N threads over one, with each N's bound"),
        &format!("{} (a call {call:.3} ms)", rows.join(", ")),
        recorded,
        Verdict::Trend,
    );
    report.claim(
        &format!(
            "{name} calls: each N's speed-up stays at or below the larger of its bound and 1.5"
        ),
        &runs(&below),
        "at or below (the bound predicted 5 to 14 percent high; costly calls 1.0 to 1.5 times)",
        relation(&below),
    );
    report.claim(
        &format!("{name} calls: the provider's serial share of one call"),
        &shares.join(", "),
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

fn processes(args: &Args, report: &mut Report) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    let k = 4_000;
    let mut rows = Vec::new();
    let mut eight = Vec::new();
    for threads in [8] {
        note::progress(&format!(
            "{threads} threads, then {threads} client processes"
        ));
        let workers = Workers::start(&provider, &elements, threads, k, false);
        let thread_runs = runs_of(|| workers.run());
        drop(workers);
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
        // Timed as the threads are: from the signal to start to the last
        // one's end, on processes kept alive across the runs.
        let clients = std::cell::RefCell::new(&mut clients);
        let process_runs = runs_of(|| {
            let mut clients = clients.borrow_mut();
            let start = Instant::now();
            for client in clients.iter_mut() {
                client.send("go");
            }
            for client in clients.iter() {
                let _ = client.expect_line();
            }
            uia::ms(start.elapsed())
        });
        rows.push(format!(
            "{threads}: threads {:.0} ms, processes {:.0} ms",
            median(&thread_runs),
            median(&process_runs)
        ));
        if threads == 8 {
            eight = thread_runs
                .iter()
                .zip(&process_runs)
                .map(|(t, p)| t <= p)
                .collect();
        }
    }
    report.claim(
        "N threads against N client processes, medians",
        &rows.join("; "),
        "",
        Verdict::Trend,
    );
    report.claim(
        "whether 8 threads of one client process were as fast as 8 client processes, run by run",
        &runs(&eight),
        "",
        Verdict::Trend,
    );
}

fn shared(report: &mut Report) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    let one = timed(&provider, &elements, 8, 1_000, true);
    let each = timed(&provider, &elements, 8, 1_000, false);
    report.claim(
        "8 threads through one client object's elements, against a client object each",
        &format!("{one:.0} ms against {each:.0} ms"),
        "no difference",
        Verdict::Trend,
    );
}
