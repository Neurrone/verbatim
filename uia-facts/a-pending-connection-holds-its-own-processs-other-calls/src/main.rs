//! A pending connection holds its own process's other calls
//! (`uia-facts/a-pending-connection-holds-its-own-processs-other-calls/readme.md`).
//!
//! The provider program has window 0 on UI thread 0 and window 1 on UI
//! thread 1. This process, B, and a second client process, C, each hold
//! window 1's node 1. UI thread 0 is made busy until released; a client
//! connects to window 0 (`ElementFromHandle`), a connection left pending,
//! at a 500 ms connection timeout; once the provider logs its message
//! reaching the busy thread, the reads are made. The busy thread is
//! released once the reads have answered, or 5 s after the connection
//! failed. A read is judged held when it answers no sooner than the
//! connection timeout less a tick from the connection's start, and
//! answered when it answers within half of it.
//!
//! - `connected`: B, which holds window 1's node, makes the connection,
//!   with `UseComThreading`.
//! - `first`: the connection is the first of a third client process, F,
//!   which has connected to nothing; C reads meanwhile.
//! - `connected-nocom`, run only when named: as `connected`, without
//!   `UseComThreading`, the reads going to window 0's own node 1, on the
//!   busy thread, served on UIA's I/O thread.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, TICK_MS, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{exit_now, note, now_us};
use windows::Win32::UI::Accessibility::IUIAutomation6;

/// The connection timeout: short, since the claim is the hold's relation
/// to it.
const CONNECTION_MS: u32 = 500;

fn client() -> IUIAutomation6 {
    let client = uia::client();
    uia::set_timeouts(&client, CONNECTION_MS, 30_000);
    client
}

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
            "read" => read_role(rest),
            "connect" => connect_role(),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report = Report::start("a-pending-connection-holds-its-own-processs-other-calls");
    let cases = [
        (
            "connected",
            true,
            "com",
            1,
            true,
            "with COM threading; this process, connected already, leaves a connection to the busy window pending; its own and another process's reads into the idle thread's window",
        ),
        (
            "first",
            true,
            "com",
            1,
            false,
            "with COM threading; a process connected to nothing leaves its first connection to the busy window pending; another process's read into the idle thread's window",
        ),
        (
            "connected-nocom",
            false,
            "nocom",
            0,
            true,
            "without COM threading; this process leaves a connection to the busy window pending; its own and another process's reads of the busy window's node, served on UIA's I/O thread",
        ),
    ];
    for (name, default, threading, window, connected, what) in cases {
        if (default && args.wants(name)) || args.names(name) {
            report.case(name, what);
            case(&mut report, threading, window, connected);
        }
    }
    report.finish();
}

/// The second client: `<hwnd>`. Holds window `hwnd`'s node 1; reads its
/// name when told `go`, reporting `done <outcome> <end µs>`.
fn read_role(rest: &[String]) -> ! {
    let client = client();
    let root = uia::element(&client, rest[0].parse().expect("a window")).expect("the window");
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

/// The client connected to nothing: told `go <hwnd>`, reports `sent <µs>`
/// and connects to window `hwnd`, reporting `done <outcome> <end µs>`.
fn connect_role() -> ! {
    let client = client();
    role::report("ready");
    for command in role::commands() {
        if let Some(hwnd) = command.strip_prefix("go ") {
            role::report(&format!("sent {}", now_us()));
            let hwnd = hwnd.parse().expect("a window");
            let connection = Timed::of(|| uia::element(&client, hwnd));
            role::report(&format!("done {} {}", connection.outcome, connection.end));
        }
    }
    exit_now(0)
}

/// The words of `role`'s next line.
fn words(role: &Role) -> Vec<String> {
    role.expect_line().split(' ').map(str::to_owned).collect()
}

/// Milliseconds from `start` to `t`, both in microseconds.
fn since(start: u64, t: u64) -> f64 {
    uia_facts_support::stats::us_to_ms(t.saturating_sub(start))
}

#[expect(clippy::too_many_lines, reason = "one trial, read in order")]
fn case(report: &mut Report, threading: &str, window: usize, connected: bool) {
    let held_from = f64::from(CONNECTION_MS) - TICK_MS;
    let free_until = f64::from(CONNECTION_MS) / 2.0;
    let mut own_held = Vec::new();
    let mut other = Vec::new();
    let mut outcomes = Vec::new();
    let mut waits = Vec::new();
    for _ in 0..trials() {
        let mut provider =
            Provider::launch(&["--windows", "uia:0,uia:1", "--threading", threading]);
        let b = client();
        let root = uia::element(&b, provider.hwnd(window)).expect("the window");
        let first = uia::share(&uia::node(&b, &root, 1).expect("node 1"));
        let mut c = Role::start("read", &[provider.hwnd(window).to_string()]);
        assert_eq!(c.expect_line(), "ready", "the second client is ready");
        let mut f = (!connected).then(|| {
            let f = Role::start("connect", &[]);
            assert_eq!(f.expect_line(), "ready", "the third client is ready");
            f
        });
        provider.log();
        provider.request("busy 0");
        let busy = provider.hwnd(0);
        note::waiting_ms(CONNECTION_MS, "the pending connection's connection timeout");
        let (start, connection) = if let Some(f) = f.as_mut() {
            f.send(&format!("go {busy}"));
            (words(f)[1].parse().expect("a time"), None)
        } else {
            let start = now_us();
            let thread = uia::spawn(move || Timed::of(|| uia::element(&client(), busy)));
            (start, Some(thread))
        };
        let (_, reached) = provider.log_until(Duration::from_secs(10), |e| {
            e.kind == "queued" && e.name == "sent" && e.start >= start
        });
        assert!(reached, "the connection's message reaches the busy thread");
        let read = connected.then(|| {
            let (done, answer) = std::sync::mpsc::channel();
            uia::spawn(move || {
                let _ = done.send(Timed::of(|| uia::name(&uia::resolve(&first))));
            });
            answer
        });
        c.send("go");
        let theirs = words(&c);
        let (outcome, end) = if let Some(thread) = connection {
            let connection = thread.join().expect("the connection");
            (connection.outcome, connection.end)
        } else {
            let done = words(f.as_ref().expect("the third client connects"));
            (done[1].clone(), done[2].parse().expect("a time"))
        };
        // B's read answers by itself once the connection has failed, or is
        // released 5 s after.
        let early = read
            .as_ref()
            .and_then(|r| r.recv_timeout(Duration::from_secs(5)).ok());
        provider.request("release");
        let their_end: u64 = theirs[2].parse().expect("a time");
        if let Some(answer) = read {
            let read = early.unwrap_or_else(|| {
                answer
                    .recv_timeout(Duration::from_secs(30))
                    .expect("B's read answers after the release")
            });
            note::evidence(&format!(
                "this process's read {} {:.1} ms after the connection's start",
                read.outcome,
                since(start, read.end)
            ));
            own_held.push(read.outcome == "ok" && since(start, read.end) >= held_from);
        }
        note::evidence(&format!(
            "from the connection's start: its end {:.1} ms, the other process's read {} at {:.1} ms",
            since(start, end),
            theirs[1],
            since(start, their_end)
        ));
        let theirs_ok = theirs[1] == "ok";
        let after = since(start, their_end);
        other.push(if theirs_ok && after < free_until {
            "answered"
        } else if theirs_ok && after >= held_from {
            "held"
        } else {
            "neither"
        });
        outcomes.push(outcome);
        waits.push(since(start, end));
    }
    report.claim(
        "the pending connection fails with",
        &runs(&outcomes),
        "UIA_E_TIMEOUT",
        deterministic(&outcomes, &"UIA_E_TIMEOUT".to_owned()),
    );
    let window_band = band(&[f64::from(CONNECTION_MS)]);
    report.claim(
        "at the connection timeout",
        &summary_ms(&waits),
        &band_text(window_band),
        in_band(&waits, window_band),
    );
    if connected {
        report.claim(
            "this process's read waits until its pending connection has failed",
            &runs(&own_held),
            "true",
            relation(&own_held),
        );
        report.claim(
            "another process's read, beside a pending connection from a connected process",
            &runs(&other),
            "held (the first run, 3 of 3)",
            deterministic(&other, &"held"),
        );
    } else {
        report.claim(
            "another process's read, beside a process's first connection pending",
            &runs(&other),
            "answered",
            deterministic(&other, &"answered"),
        );
    }
}
