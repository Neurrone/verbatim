//! A cache request fails while UIA's first-time setup runs on another
//! thread
//! (`uia-facts/a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread/readme.md`).
//!
//! A run starts 4 fresh client processes at once (the `race` role), each
//! of which has not used UIA. In each, six threads wait at a barrier, then
//! each makes a client object through support's `uia::client()` and builds
//! a cache request (`CreateCacheRequest`), with no wait for UIA's
//! first-time setup such as Verbatim's `ensure_ready`. A thread whose cache
//! request failed tries once more 50 ms later. A run fails when any thread
//! of its 4 processes failed, as the CI test does.

use std::sync::{Arc, Barrier};
use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::report::{Report, Verdict, deterministic, rate, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{exit_now, note, uia};

/// Runs, as the test without the workaround failed 50 of 50.
const RUNS: usize = 50;
/// Fresh processes a run, as the test races.
const PROCESSES: usize = 4;
/// Threads a process.
const THREADS: usize = 6;

fn main() {
    let args = Args::parse();
    if let Some((name, _)) = &args.role {
        assert_eq!(name, "race", "unknown role {name}");
        race();
    }
    let mut report =
        Report::start("a-cache-request-fails-while-uias-first-time-setup-runs-on-another-thread");
    if args.wants("race") {
        report.case("race", "50 runs, each of 4 fresh processes at once, in each six threads making a client object and building a cache request at once; a failed one tried again 50 ms later");
        judge(&mut report);
    }
    report.finish();
}

/// The `race` role: answers `race <outcome of each thread> retry <outcome
/// of each retry>`.
fn race() -> ! {
    let barrier = Arc::new(Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                uia::mta();
                barrier.wait();
                let client = uia::client();
                // SAFETY: a live client object.
                let first = uia::outcome(&unsafe { client.CreateCacheRequest() });
                let retry = (first != "ok").then(|| {
                    std::thread::sleep(Duration::from_millis(50));
                    // SAFETY: as above.
                    uia::outcome(&unsafe { client.CreateCacheRequest() })
                });
                (first, retry)
            })
        })
        .collect();
    let outcomes: Vec<(String, Option<String>)> = threads
        .into_iter()
        .map(|t| t.join().expect("a racing thread"))
        .collect();
    let firsts: Vec<&str> = outcomes.iter().map(|o| o.0.as_str()).collect();
    let retries: Vec<&str> = outcomes.iter().filter_map(|o| o.1.as_deref()).collect();
    role::report(&format!(
        "race {} retry {}",
        firsts.join(","),
        retries.join(",")
    ));
    exit_now(0)
}

fn judge(report: &mut Report) {
    let mut failed_runs = 0;
    let mut failing_threads = Vec::new();
    let mut retries: Vec<String> = Vec::new();
    let mut codes = std::collections::BTreeSet::new();
    for run in 0..RUNS {
        let processes: Vec<Role> = (0..PROCESSES).map(|_| Role::start("race", &[])).collect();
        let mut failed = false;
        for process in &processes {
            let line = process.expect_line();
            let words: Vec<&str> = line.split(' ').collect();
            let [_, firsts, _, again] = words.as_slice() else {
                panic!("a race line: {line}");
            };
            let bad: Vec<&str> = firsts.split(',').filter(|o| *o != "ok").collect();
            failing_threads.push(bad.len());
            codes.extend(bad.iter().map(|b| (*b).to_owned()));
            failed |= !bad.is_empty();
            retries.extend(
                again
                    .split(',')
                    .filter(|r| !r.is_empty())
                    .map(str::to_owned),
            );
        }
        if failed {
            failed_runs += 1;
        }
        note::evidence(&format!(
            "run {run}: {}",
            if failed { "failed" } else { "no failure" }
        ));
    }
    report.claim(
        "runs of 4 fresh processes in which a cache request failed",
        &format!("{failed_runs} of {RUNS}"),
        "50 of 50 with the workaround removed (2026-10-18)",
        rate(failed_runs, RUNS, 50, 50),
    );
    report.claim(
        "the failure's HRESULT",
        &format!("{codes:?}"),
        "E_FAIL (0x80004005)",
        deterministic(
            &[codes.into_iter().collect::<Vec<_>>()],
            &vec!["E_FAIL".to_owned()],
        ),
    );
    let max = failing_threads.iter().copied().max().unwrap_or(0);
    report.claim(
        "the threads of 6 that failed, in the process with the most",
        &max.to_string(),
        "five nearly every time (7c402534)",
        Verdict::Trend,
    );
    let all_ok: Vec<bool> = vec![retries.iter().all(|r| r == "ok") && !retries.is_empty()];
    report.claim(
        "a failed cache request tried again 50 ms later",
        &format!("{} ({} retries)", runs(&all_ok), retries.len()),
        "succeeded (7c402534)",
        deterministic(&all_ok, &true),
    );
}
