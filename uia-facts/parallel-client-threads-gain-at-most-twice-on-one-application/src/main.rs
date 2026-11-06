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
//!   then costly calls, K = 16 with every `GetPropertyValue` made 5 ms
//!   slow, the window pumping meanwhile (`slow 5 GetPropertyValue`), at N
//!   of 2 and 4.
//!
//! Each N's speed-up is judged against the fact's own figures: at most 2
//! times for cheap calls, at most 1.5 for costly ones.
//! Each N's bound is computed from that N's own runs: one call's time on
//! one thread divided by the provider's serial time per call under that
//! load, the provider calls' time from entry to exit per read, which no
//! other call can share; it is printed for trends. The provider's log
//! cannot see UIA's I/O thread, so for cheap calls, whose time is mostly
//! that thread's, the bound lies far above the speed-up.
//!
//! - `shared`: the cheap 8-thread bench, K = 1,000, with one client object
//!   for all threads against one per thread.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use uia_facts_support::args::Args;
use uia_facts_support::note;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::stats::{as_f64, median, us_to_ms};
use uia_facts_support::uia::{self, Shared};

/// Timed runs after the warm-up, for each figure.
const TIMED_RUNS: usize = 3;

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("parallel-client-threads-gain-at-most-twice-on-one-application");
    if args.wants("gains") {
        report.case("gains", "reading names from 1, 2 and 8 client threads with cheap provider calls, then from 1, 2 and 4 with costly ones; judging each speed-up against the fact's figure, at most 2 times for cheap calls and 1.5 for costly");
        gains(
            &args,
            &mut report,
            ("cheap", 1_000, 0),
            (&[2, 8], 2.0),
            "at most 2 times (1.74 to 1.98 on the synthetic provider)",
        );
        gains(
            &args,
            &mut report,
            ("costly", 16, 5),
            (&[2, 4], 1.5),
            "1.0 to 1.5 times (1.01 with a 5 ms provider call)",
        );
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
/// call (`slow`), at each of `threads`, each speed-up judged at most
/// `most` times.
fn gains(
    args: &Args,
    report: &mut Report,
    (name, k, slow): (&str, usize, u64),
    (threads, most): (&[usize], f64),
    recorded: &str,
) {
    let mut provider = Provider::launch(&["--windows", "uia:0"]);
    let elements = nodes(&provider);
    args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
    // Only the name read's own provider call is slowed, so a costly read
    // is one slow call and the bench stays short.
    provider.request(&format!("slow {slow} GetPropertyValue"));
    note::progress(&format!("{name} calls, 1 thread"));
    let one = timed(&provider, &elements, 1, k, true);
    let call = one / as_f64(k);
    // Each N's speed-up against the bound from the same N's runs: the
    // provider's serial time per call under that load.
    let mut rows = Vec::new();
    let mut at_most = Vec::new();
    let mut shares = Vec::new();
    for &threads in threads {
        note::progress(&format!("{name} calls, {threads} threads"));
        provider.log();
        let time = timed(&provider, &elements, threads, k, true);
        // The log covers the warm-up and the timed runs, K reads each.
        let serial = serial_time(&provider.log(), k * (TIMED_RUNS + 1));
        let (speedup, bound) = (one / time, call / serial);
        rows.push(format!("{threads} threads {speedup:.2} (bound {bound:.2})"));
        at_most.push(speedup <= most);
        shares.push(format!("{threads} threads {:.2}", serial / call));
    }
    provider.request("slow 0");
    report.claim(
        &format!("{name} calls: the speed-up of N threads over one, with each N's bound"),
        &format!("{} (a call {call:.3} ms)", rows.join(", ")),
        "",
        Verdict::Trend,
    );
    report.claim(
        &format!("{name} calls: each N's speed-up is at most {most} times"),
        &runs(&at_most),
        &format!("true ({recorded})"),
        relation(&at_most),
    );
    report.claim(
        &format!("{name} calls: the provider's serial share of one call"),
        &shares.join(", "),
        "about half",
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
