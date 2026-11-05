//! Another client's stuck call holds an event's cache fetch
//! (`uia-facts/another-clients-stuck-call-holds-an-events-cache-fetch/readme.md`).
//!
//! Provider program P holds two UIA windows on UI threads of their own;
//! provider program Q one. Client X, a listening process with no
//! registration, reads the name of P's window 0, and P holds that read in
//! its provider until released, so X's call is stuck in P. Client D, a
//! listening process of its own, has name-change registrations on P's
//! window 1 and Q's window, each caching the sender's name, which UIA's
//! worker fetches from the application before calling the handler. While
//! X's call is stuck, P's window 1 raises a name change, then Q's window
//! raises one 100 ms later.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::listen::{self, Listener, heard_at};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, runs};
use uia_facts_support::stats::us_to_ms;
use uia_facts_support::{note, now_us, uia};

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report = Report::start("another-clients-stuck-call-holds-an-events-cache-fetch");
    if args.wants("holds") {
        report.case("holds", "client X's name read held in P for 1 s; meanwhile a name change on P's other window, then one on Q's, both registered by client D at UIA's default 20 s transaction timeout");
        let outcomes: Vec<Outcome> = (0..trials()).map(|_| trial(&args, 20_000, 1_000)).collect();
        report.claim(
            "D hears P's name change only after X's call is released",
            &runs(&outcomes.iter().map(|o| o.p_after).collect::<Vec<_>>()),
            "",
            Verdict::New,
        );
        report.claim(
            "D hears Q's name change, raised 100 ms after P's, only after X's call is released",
            &runs(&outcomes.iter().map(|o| o.q_after).collect::<Vec<_>>()),
            "",
            Verdict::New,
        );
    }
    if args.wants("timeout") {
        report.case(
            "timeout",
            "the same with X's read held 6 s, D's transaction timeout at 1 s, then at 5 s",
        );
        for tto in [1_000u32, 5_000] {
            let outcomes: Vec<Outcome> = (0..trials()).map(|_| trial(&args, tto, 6_000)).collect();
            let times = |f: fn(&Outcome) -> Option<f64>| {
                outcomes
                    .iter()
                    .map(|o| {
                        f(o).map_or_else(|| "not heard".to_owned(), |ms| format!("{ms:.0} ms"))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            report.claim(
                &format!("transaction timeout {tto} ms, X held 6 s: D hears P's name change"),
                &runs(
                    &outcomes
                        .iter()
                        .map(|o| o.p_ms.is_some())
                        .collect::<Vec<_>>(),
                ),
                "",
                Verdict::New,
            );
            report.claim(
                &format!("transaction timeout {tto} ms, X held 6 s: D hears Q's name change before X's call is released"),
                &format!(
                    "{} (after P's raise: {})",
                    runs(&outcomes.iter().map(|o| !o.q_after && o.q_ms.is_some()).collect::<Vec<_>>()),
                    times(|o| o.q_ms)
                ),
                "",
                Verdict::New,
            );
        }
    }
    report.finish();
}

/// One trial's outcome.
struct Outcome {
    p_after: bool,
    q_after: bool,
    p_ms: Option<f64>,
    q_ms: Option<f64>,
}

fn trial(args: &Args, tto: u32, hold_ms: u64) -> Outcome {
    let mut p = Provider::launch(&["--windows", "uia:0,uia:1"]);
    let mut q = Provider::launch(&["--windows", "uia:0"]);
    let mut x = Listener::start("X", &[]);
    let d = Listener::start(
        "D",
        &[
            format!("tto={tto}"),
            format!("name={}", p.hwnd(1)),
            format!("name={}", q.hwnd(0)),
        ],
    );
    args.pause_for_debugger(&[
        ("P", p.pid),
        ("Q", q.pid),
        ("client X", x.pid()),
        ("client D", d.pid()),
    ]);
    p.request("hold GetPropertyValue w=0");
    x.send(&format!("read {}", p.hwnd(0)));
    p.held(Duration::from_secs(10))
        .expect("X's read is held in P");
    p.request("rename 1 1 p");
    let raised: u64 = p.request("raise name 1 1").parse().expect("a time");
    uia::pause(Duration::from_millis(100), "Q's name change after P's");
    q.request("rename 0 1 q");
    q.request("raise name 0 1");
    let release_at = raised + hold_ms * 1_000;
    uia::pause(
        Duration::from_micros(release_at.saturating_sub(now_us())),
        "X's read held in P",
    );
    let released = now_us();
    p.request("release");
    let lines = d.lines_until(released + 1_500_000);
    let p_at = heard_at(&lines, "name", "p");
    let q_at = heard_at(&lines, "name", "q");
    let ms = |at: Option<u64>| at.map(|at| us_to_ms(at.saturating_sub(raised)));
    note::evidence(&format!(
        "transaction timeout {tto} ms: P's change heard {:?} ms and Q's {:?} ms after P's raise; X released {:.1} ms after it",
        ms(p_at),
        ms(q_at),
        us_to_ms(released - raised)
    ));
    Outcome {
        p_after: p_at.is_some_and(|at| at >= released),
        q_after: q_at.is_some_and(|at| at >= released),
        p_ms: ms(p_at),
        q_ms: ms(q_at),
    }
}
