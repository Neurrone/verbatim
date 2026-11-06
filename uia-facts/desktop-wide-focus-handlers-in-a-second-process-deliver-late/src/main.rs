//! Desktop-wide focus handlers in a second process deliver late
//! (`uia-facts/desktop-wide-focus-handlers-in-a-second-process-deliver-late/readme.md`).
//!
//! Every process runs on a desktop of its own, as P2 ran, where no window
//! has the keyboard focus, so a desktop-wide focus handler receives only
//! the focus events UIA derives from a focus `WinEvent`. M is the provider
//! program's MSAA window, whose object has two children, `First` and
//! `Second`; its UI thread moves the focus between them and raises the
//! focus `WinEvent` naming each, so UIA reads it through its proxy. Each
//! client is a listening process (`uia_facts_support::listen`) holding
//! desktop-wide focus handlers, started in turn, so the first started is
//! the first registered. A delivery's lateness is the time from the latest
//! raise before it to its arrival.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::listen::{self, Line, Listener};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, deterministic, rate, relation, runs};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::{note, now_us, uia};

/// The desktop of its own.
const DESKTOP: &str = "uia-facts";
/// Focus events a trial raises; P2 raised 10, and 6 keep the run short.
const EVENTS: usize = 6;
/// A delivery counts as on time within this long of its raise; P2's first
/// process took at most 9 to 15 ms.
const ON_TIME_MS: f64 = 100.0;
/// A later process counts as late when its worst delivery is at least this
/// much later than the first process's; P2's were 670 ms or more.
const LATE_MS: f64 = 300.0;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report = Report::start("desktop-wide-focus-handlers-in-a-second-process-deliver-late");
    if args.wants("processes") {
        report.case("processes", "1, 2 and 4 client processes, each with one desktop-wide focus handler; M's focus moved every 150 ms, 6 times");
        processes(&args, &mut report);
    }
    if args.wants("one-process") {
        report.case("one-process", "4 desktop-wide focus handlers in one client process; M's focus moved every 150 ms, 6 times");
        one_process(&args, &mut report);
    }
    if args.wants("slow") {
        report.case(
            "slow",
            "4 client processes; M's focus moved every 500 ms, 6 times",
        );
        slow(&args, &mut report);
    }
    if args.wants("plain") {
        report.case("plain", "a plain Win32 window without the keyboard focus named by 6 focus WinEvents, 150 ms apart, with 1, 2 and 4 client processes and one process with 4 handlers");
        plain(&args, &mut report);
    }
    if args.wants("threading") {
        report.case("threading", "2 client processes, the second registering on a thread of a single-threaded apartment; M's focus moved every 150 ms, 6 times");
        threading(&args, &mut report);
    }
    report.finish();
}

/// One trial: the clients `specs`, started in turn, and M's focus moved
/// `EVENTS` times `pace_ms` apart; answers each client's deliveries'
/// lateness, in ms.
fn trial(args: &Args, specs: &[&[&str]], pace_ms: u64, plain: bool) -> Vec<Vec<f64>> {
    let mut m = Provider::launch_on(
        Some(DESKTOP),
        &["--windows", if plain { "plain:0" } else { "msaa:0" }],
    );
    let clients: Vec<Listener> = specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            let words: Vec<String> = spec.iter().map(|w| (*w).to_owned()).collect();
            Listener::start_on(Some(DESKTOP), &format!("process {}", i + 1), &words)
        })
        .collect();
    let mut pids = vec![("M", m.pid)];
    pids.extend(clients.iter().map(|c| (c.label.as_str(), c.pid())));
    args.pause_for_debugger(&pids);
    // As P2 did, 0.5 s between the clients' start and the first move.
    uia::pause(Duration::from_millis(500), "before the first focus move");
    let mut raised = Vec::new();
    for i in 0..EVENTS {
        let child = if plain { 0 } else { 1 + i % 2 };
        raised.push(
            m.request(&format!("raise focus 0 {child}"))
                .parse::<u64>()
                .expect("a time"),
        );
        uia::pause(Duration::from_millis(pace_ms), "between focus moves");
    }
    // Late deliveries came up to 2.7 s after their raise.
    let listen_ms: u64 = 3_000;
    note::waiting("3.0 s, for late deliveries");
    let end = now_us() + listen_ms * 1_000;
    let names: &[&str] = if plain {
        &["msaa_window_0", "-"]
    } else {
        &["First", "Second"]
    };
    clients
        .iter()
        .map(|client| {
            let late: Vec<f64> = client
                .lines_until(end)
                .iter()
                .filter_map(|l| match l {
                    Line::Heard { kind, name, at, .. }
                        if kind == "focus" && (plain || names.contains(&name.as_str())) =>
                    {
                        let from = raised.iter().rev().find(|r| **r <= *at)?;
                        Some(us_to_ms(at - from))
                    }
                    _ => None,
                })
                .collect();
            note::evidence(&format!("{}: lateness {late:.0?} ms", client.label));
            late
        })
        .collect()
}

fn worst(late: &[f64]) -> f64 {
    late.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// Whether one process, whichever, had every delivery on time and every
/// other process's worst delivery came at least [`LATE_MS`] after its.
fn others_late(late: &[Vec<f64>]) -> bool {
    let worsts: Vec<f64> = late.iter().map(|l| worst(l)).collect();
    let earliest = worsts.iter().copied().fold(f64::INFINITY, f64::min);
    earliest <= ON_TIME_MS
        && worsts.iter().filter(|w| **w >= earliest + LATE_MS).count() == worsts.len() - 1
}

fn processes(args: &Args, report: &mut Report) {
    // One process, judged once; two and four as rates over P2c's two runs.
    for (n, runs_recorded) in [(1usize, 1usize), (2, 2), (4, 2)] {
        let specs: Vec<&[&str]> = vec![&["desk"]; n];
        let mut late_runs = 0;
        let mut on_time = Vec::new();
        let mut all_heard = Vec::new();
        let mut worsts = Vec::new();
        for _ in 0..runs_recorded {
            let late = trial(args, &specs, 150, false);
            if others_late(&late) {
                late_runs += 1;
            }
            on_time.push(worst(&late[0]) <= ON_TIME_MS);
            all_heard.push(late.iter().map(Vec::len).collect::<Vec<_>>());
            worsts.extend(late.iter().map(|l| worst(l)));
        }
        if n == 1 {
            report.claim(
                "1 process, focus every 150 ms: its deliveries all within 100 ms",
                &format!("{} (worst {})", runs(&on_time), summary_ms(&worsts)),
                "at most 9 and 10 ms (p2c procs1)",
                relation(&on_time),
            );
        } else {
            report.claim(
                &format!("{n} processes, focus every 150 ms: runs in which one process is on time and every other's worst delivery comes at least {LATE_MS} ms after its"),
                &format!("{late_runs} of {runs_recorded} (worst per process {})", summary_ms(&worsts)),
                if n == 2 {
                    "2 of 2: the first at most 10 and 13 ms, the second at most 682 and 1,112 ms (p2c procs2)"
                } else {
                    "2 of 2: the first at most 15 and 10 ms, processes 2 to 4 670 to 2,677 ms (p2c procs4)"
                },
                rate(late_runs, runs_recorded, runs_recorded, runs_recorded),
            );
        }
        report.claim(
            &format!("{n} processes: the focus events each process hears in the end"),
            &runs(&all_heard),
            "every one, 10 (p2c)",
            if n == 4 {
                Verdict::Trend
            } else {
                deterministic(&all_heard, &vec![EVENTS; n])
            },
        );
    }
}

fn one_process(args: &Args, report: &mut Report) {
    for (handlers, spec) in [(4usize, &["desk", "desks=3"][..])] {
        let mut on_time = Vec::new();
        let mut heard = Vec::new();
        for _ in 0..trials() {
            let late = trial(args, &[spec], 150, false);
            on_time.push(worst(&late[0]) <= ON_TIME_MS);
            heard.push(late[0].len());
        }
        report.claim(
            &format!("{handlers} handlers in one process: every delivery within {ON_TIME_MS} ms"),
            &runs(&on_time),
            "at most 9 to 11 ms (p2c one2fast, one4fast)",
            relation(&on_time),
        );
        report.claim(
            &format!("{handlers} handlers in one process: deliveries"),
            &runs(&heard),
            &format!("{} (p2c)", EVENTS * handlers),
            deterministic(&heard, &(EVENTS * handlers)),
        );
    }
}

fn slow(args: &Args, report: &mut Report) {
    // A rate over P2b's run of four processes and five reruns of it at its
    // commit, a405b362, on 2026-11-06.
    let specs: Vec<&[&str]> = vec![&["desk"]; 4];
    let late = trial(args, &specs, 500, false);
    let waits: Vec<f64> = late.iter().map(|l| worst(l)).collect();
    let k = usize::from(others_late(&late));
    report.claim(
        &format!("4 processes, focus every 500 ms: runs in which one process is on time and every other's worst delivery comes at least {LATE_MS} ms after its"),
        &format!("{k} of 1 (worst per process {})", summary_ms(&waits)),
        "5 of 6: waits of about 1,000, 1,500 and 2,000 ms (p2b_four-slow), and 4 of 5 reruns (p2b_rerun1 to 5)",
        rate(k, 1, 5, 6),
    );
}

fn plain(args: &Args, report: &mut Report) {
    let specs: Vec<&[&str]> = vec![
        &["desk"],
        &["desk"],
        &["desk"],
        &["desk"],
        &["desk", "desks=3"],
    ];
    let mut heard = Vec::new();
    for _ in 0..trials() {
        let late = trial(args, &specs, 150, true);
        heard.push(late.iter().map(Vec::len).collect::<Vec<_>>());
    }
    report.claim(
        "a plain Win32 window without the keyboard focus: the focus events each of 4 processes, and a process with 4 handlers, hears",
        &runs(&heard),
        "none, with 1, 2 and 4 processes and 4 handlers in one (p2d)",
        deterministic(&heard, &vec![0; 5]),
    );
}

fn threading(args: &Args, report: &mut Report) {
    let specs: Vec<&[&str]> = vec![&["desk"], &["sta", "desk"]];
    let mut late = Vec::new();
    for _ in 0..trials() {
        let trial = trial(args, &specs, 150, false);
        late.push(worst(&trial[1]) >= worst(&trial[0]) + LATE_MS);
    }
    report.claim(
        &format!("2 processes, the second registering on an STA thread: its worst delivery at least {LATE_MS} ms later than the first's"),
        &runs(&late),
        "",
        Verdict::New,
    );
}
