//! Focus for a newly opened or shown window is not held
//! (`uia-facts/focus-for-a-newly-opened-or-shown-window-is-not-held/readme.md`).
//!
//! On the unlocked desktop, run only when named: the hung-window scene of
//! support's `uia_facts_support::hung` on the user's desktop and a third
//! provider program, P3, with a hidden UIA window O; T stalled 9 s and a
//! focus `WinEvent` naming it, B renamed 200 ms later, and O shown and
//! brought to the front 0.6 s after that: O's focus reaches Verbatim's
//! stand-in while B's name change is still held.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::hung::{B_AFTER_US, CTO_MS, Scene, as_ms, delay, until, us};
use uia_facts_support::listen::{self, Line, Listener, heard_at};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, band, band_text, in_band, relation, runs};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::{note, now_us, precondition, uia};

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report = Report::start("focus-for-a-newly-opened-or-shown-window-is-not-held");
    // It takes over the desktop, so it runs only when named.
    if args.names("open-show") {
        report.case("open-show", "on the unlocked desktop: T stalled 9 s; a focus WinEvent naming T; B renamed 200 ms later; a third provider's hidden window shown and brought to the front 0.6 s after that");
        open_show(&args, &mut report);
    }
    report.finish();
}

fn open_show(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 9_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet(
            "open-show: a newly shown window's focus during a hold",
            &reason,
        );
        return;
    }
    let mut escaped = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut opened = Provider::launch(&["--windows", "uia:0"]);
        let mut scene = Scene::screen();
        let d = Listener::start(
            "Verbatim's stand-in",
            &[
                "desk".to_owned(),
                "vbmap".to_owned(),
                "tto=5000".to_owned(),
                scene.b_spec(),
            ],
        );
        args.pause_for_debugger(&[
            ("P1 (T)", scene.p1.pid),
            ("P3 (O)", opened.pid),
            ("client", d.pid()),
        ]);
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        until(raised + 800_000, "O shown, 0.6 s after B's name change");
        let pressed = now_us();
        let front = opened.request("foreground 0");
        note::evidence(&format!("O in front: {front}"));
        let lines = d.lines_until(stalled + us(as_ms(STALL_MS)));
        let b_at = heard_at(&lines, "name", "held");
        holds.push(delay(raised, b_at));
        let focus: Vec<(String, u64)> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heard { kind, name, at, .. } if kind == "focus" && *at >= pressed => {
                    Some((name.clone(), *at))
                }
                _ => None,
            })
            .collect();
        note::evidence(&format!(
            "focus heard after O was shown, ms after it: {:?}",
            focus
                .iter()
                .map(|(n, at)| (n.clone(), us_to_ms(at - pressed)))
                .collect::<Vec<_>>()
        ));
        escaped.push(focus.iter().any(|(_, at)| b_at.is_none_or(|b| *at < b)));
    }
    report.claim(
        "a newly shown window's focus reaches the stand-in while B's name change is still held",
        &runs(&escaped),
        "Notepad's menu focus 26 to 112 ms after Alt+F, the notification held 3,815 to 3,829 ms (both; route1, route2)",
        relation(&escaped),
    );
    let two = band(&[CTO_MS, CTO_MS]);
    report.claim(
        "B's name change in the same hold",
        &summary_ms(&holds),
        &format!(
            "3,815 to 3,829 ms after B's raise (both); {}",
            band_text(two)
        ),
        in_band(&holds, two),
    );
}
