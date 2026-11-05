//! Focus events cannot be limited to one application: what one focus event
//! costs the application per registered process
//! (`uia-facts/focus-events-cannot-be-limited-to-one-application/readme.md`).
//!
//! A is the provider program's UIA window. Each registered process is a
//! listening process (`uia_facts_support::listen`) holding the focus event
//! in an event handler group on A's window, subtree scope, with one of
//! P2's cache requests. A raises 10 focus events, alternating between nodes
//! 5 and 6, each once every registered process has heard the one before.
//! The provider calls and `WM_GETOBJECT`s A's log records from the first
//! raise to the last delivery, over 10, are the calls one focus event
//! costs; the cost per registered process is the rise from 1 process to 4,
//! over 3.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::listen::{self, Line, Listener};
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::stats::{as_f64, median, number};
use uia_facts_support::{note, uia};

/// Focus events each trial raises, as P2 raised.
const EVENTS: usize = 10;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report = Report::start("focus-events-cannot-be-limited-to-one-application");
    if args.wants("cost") {
        report.case("cost", "1 and then 4 processes, each with the focus event in a group on A's window, with no cache, the process id, and NVDA's base cache; A's focus moved 10 times; counting A's provider calls");
        cost(&args, &mut report);
    }
    report.finish();
}

/// The provider calls and `WM_GETOBJECT`s in `log`.
fn calls(log: &[Entry]) -> usize {
    log.iter()
        .filter(|e| e.kind == "call" || (e.kind == "msg" && e.name == "WM_GETOBJECT"))
        .count()
}

/// The calls one focus event costs A with `k` processes registered with
/// cache `cache`.
fn per_event(args: &Args, k: usize, cache: &str) -> f64 {
    let mut a = Provider::launch(&["--windows", "uia:0"]);
    let clients: Vec<Listener> = (0..k)
        .map(|i| {
            Listener::start(
                &format!("process {}", i + 1),
                &[format!("group={}", a.hwnd(0)), format!("cache={cache}")],
            )
        })
        .collect();
    let mut pids = vec![("A", a.pid)];
    pids.extend(clients.iter().map(|c| (c.label.as_str(), c.pid())));
    args.pause_for_debugger(&pids);
    let _ = a.log();
    for i in 0..EVENTS {
        let node = 5 + i % 2;
        a.request(&format!("focus 0 {node}"));
        a.request(&format!("raise focus 0 {node}"));
        for client in &clients {
            let heard = client.next(
                Duration::from_secs(5),
                |l| matches!(l, Line::Heard { kind, .. } if kind == "focus"),
            );
            assert!(heard.is_some(), "{} hears focus event {i}", client.label);
        }
    }
    // The last delivery's own calls are made before its handler runs, so
    // nothing more is awaited.
    let total = calls(&a.log());
    as_f64(total) / as_f64(EVENTS)
}

fn cost(args: &Args, report: &mut Report) {
    for cache in ["none", "pid", "base"] {
        let mut slopes = Vec::new();
        for _ in 0..trials() {
            let one = per_event(args, 1, cache);
            let four = per_event(args, 4, cache);
            note::evidence(&format!(
                "cache {cache}: {one:.1} calls an event with 1 process, {four:.1} with 4"
            ));
            slopes.push((four - one) / 3.0);
        }
        let inside: Vec<bool> = slopes.iter().map(|s| (40.0..=60.0).contains(s)).collect();
        let observed = format!("{} ({} calls)", runs(&inside), number(median(&slopes), 1));
        if cache == "base" {
            report.claim(
                "NVDA's base cache: the provider calls one focus event costs per registered process",
                &observed,
                "about 63, the 47 to 49 and about 16 GetPropertyValue for the cache (P2 on mockapp)",
                Verdict::Trend,
            );
        } else {
            report.claim(
                &format!("cache {cache}: the provider calls one focus event costs per registered process, 40 to 60"),
                &observed,
                "47 to 49 (P2 and E-cost on mockapp)",
                relation(&inside),
            );
        }
    }
}
