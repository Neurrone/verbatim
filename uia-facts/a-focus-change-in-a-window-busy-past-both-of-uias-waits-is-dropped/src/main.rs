//! A focus change in a window busy past both of UIA's waits is dropped
//! (`uia-facts/a-focus-change-in-a-window-busy-past-both-of-uias-waits-is-dropped/readme.md`).
//!
//! On a desktop of its own, as `h6` ran: M, the provider program's MSAA
//! window, moves its focus while it answers, then again at the start of a
//! 1.5 s stall of its UI thread, from inside it. A client with a desktop
//! focus handler caching `h6`'s properties, at connection timeouts of 10 s,
//! 1 s and 250 ms, each in a trial of its own, listens 3 s from the stall's
//! start.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::hung::{DESKTOP, NOT_HELD_MS, Scene, as_ms, delay, spec};
use uia_facts_support::listen::{self, Listener, heard_at};
use uia_facts_support::report::{Report, TICK_MS, relation};
use uia_facts_support::stats::summary_ms;
use uia_facts_support::{note, uia};

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report =
        Report::start("a-focus-change-in-a-window-busy-past-both-of-uias-waits-is-dropped");
    if args.wants("dropped") {
        report.case("dropped", "on a desktop of its own, M moving its focus while it answers, then again at the start of a 1.5 s stall of its UI thread; a client with a desktop focus handler at a connection timeout of 10 s, then 1 s, then 0.25 s, each in a trial of its own");
        dropped(&args, &mut report);
    }
    report.finish();
}

fn dropped(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 1_500;
    let timeouts = [10_000u32, 1_000, 250];
    let mut control = vec![Vec::new(); 3];
    let mut heard = vec![Vec::new(); 3];
    for _ in 0..trials() {
        // Each timeout in a trial of its own, as h6 ran each in a run of
        // its own.
        for (i, t) in timeouts.iter().enumerate() {
            let mut scene = Scene::on(Some(DESKTOP), &["--windows", "plain:0,uia:1,msaa:2"]);
            let client = Listener::start_on(
                Some(DESKTOP),
                &format!("{t} ms"),
                &spec(&["desk", "cache=min", &format!("cto={t}")]),
            );
            args.pause_for_debugger(&[("P1 (M)", scene.p1.pid), ("client", client.pid())]);
            // As h6: M moves its focus to its first item while it answers,
            // then to its second at the start of a 1.5 s stall, from inside
            // it, 1.5 waits at 1 s as h6's 3 s stall was at 2 s.
            let raised = scene
                .p1
                .request("raise focus 2 1")
                .parse::<u64>()
                .expect("a time");
            let lines = client.lines_until(raised + 500_000);
            control[i].push(delay(raised, heard_at(&lines, "focus", "First")));
            note::waiting(&format!(
                "3.0 s, M's 1.5 s stall and its focus after, at {t} ms"
            ));
            let stalled: u64 = scene
                .p1
                .request(&format!("busy 2 {STALL_MS} focus-at 0 2"))
                .parse()
                .expect("a time");
            let lines = client.lines_until(stalled + 3_000_000);
            heard[i].push(delay(stalled, heard_at(&lines, "focus", "Second")));
        }
    }
    for (i, t) in timeouts.iter().enumerate() {
        report.claim(
            &format!("connection timeout {t} ms: M's focus raised while it answers is heard within {NOT_HELD_MS} ms"),
            &summary_ms(&control[i]),
            "10 to 12 ms (h6)",
            relation(&control[i].iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
        );
    }
    report.claim(
        "connection timeout 10,000 ms: M's focus from inside its 1.5 s stall is heard once the stall ends",
        &summary_ms(&heard[0]),
        "3,003 ms into a 3 s stall (h6_cto10000)",
        relation(&heard[0].iter().map(|v| v.is_finite() && *v >= as_ms(STALL_MS) - TICK_MS).collect::<Vec<_>>()),
    );
    report.claim(
        "connection timeout 1,000 ms, the 1.5 s stall 1.5 waits long, as h6 stalled 3 s at 2,000 ms, shorter than both: M's focus from inside it is heard once the stall ends",
        &summary_ms(&heard[1]),
        "mockapp's MSAA window: never within 8 s (h6_default); the provider program's: at the stall's end",
        relation(&heard[1].iter().map(|v| v.is_finite() && *v >= as_ms(STALL_MS) - TICK_MS).collect::<Vec<_>>()),
    );
    report.claim(
        "connection timeout 250 ms, the 1.5 s stall longer than both waits: M's focus from inside it is not heard within 3 s of the stall's start",
        &summary_ms(&heard[2]),
        "never delivered within 8 s at 500 ms in a 3 s stall (h6_cto500)",
        relation(&heard[2].iter().map(|v| v.is_infinite()).collect::<Vec<_>>()),
    );
}
