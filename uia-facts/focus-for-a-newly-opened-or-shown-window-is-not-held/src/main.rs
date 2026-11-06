//! Focus for a newly opened or shown window is not held
//! (`uia-facts/focus-for-a-newly-opened-or-shown-window-is-not-held/readme.md`).
//!
//! On the unlocked desktop, run only when named: the hung-window scene of
//! support's `uia_facts_support::hung` on the user's desktop and a third
//! provider program, P3, with a hidden plain window O. T is held until
//! released and a focus `WinEvent` names it; B is renamed 200 ms later;
//! 0.6 s after that P3 brings O to the front and opens a popup menu of its
//! own over it, as Alt+F opens a menu. The stall ends once Verbatim's
//! stand-in has heard B's change, or past its band's top: judged, whether
//! the menu's first item's focus reaches the stand-in while B's change is
//! still held, against the first run's answer: a Win32 popup menu's focus
//! is held with B's change, where Notepad's menu's was not.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::hung::{
    B_AFTER_US, CTO_MS, PAST_TOP_US, Scene, delay, until, until_heard, us,
};
use uia_facts_support::listen::{self, Line, Listener, heard_at};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, band, band_text, deterministic, in_band, runs};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::{note, now_us, precondition, uia};

/// The name of the menu's first item, `Menu first item`, as a listening
/// process reports a name: its spaces written as underscores.
const FIRST_ITEM: &str = "Menu_first_item";

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
        report.case("open-show", "on the unlocked desktop: T held; a focus WinEvent naming T; B renamed 200 ms later; a third provider's window brought to the front with a popup menu of its own opened over it 0.6 s after that; the stall released once B's change is heard or past its band");
        open_show(&args, &mut report);
    }
    report.finish();
}

fn open_show(args: &Args, report: &mut Report) {
    let claim = "a Win32 popup menu opened during the hold: its focus reaches the stand-in while B's name change is still held";
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet(claim, &reason);
        return;
    }
    let two = band(&[CTO_MS, CTO_MS]);
    let mut escaped = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut opened = Provider::launch(&["--windows", "plain:0"]);
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
        scene.hold(0);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        until(raised + 800_000, "the menu, 0.6 s after B's name change");
        let opened_at = now_us();
        let front = opened.request("menu 0");
        if front != "true" {
            scene.release();
            opened.request("menu-close 0");
            report.unmet(
                claim,
                "the provider program's window did not come to the front",
            );
            return;
        }
        let top = raised + us(two.1) + PAST_TOP_US;
        note::waiting("until B's name change is heard, at most past its band's top");
        let lines = until_heard(&d, "held", top);
        scene.release();
        opened.request("menu-close 0");
        let b_at = heard_at(&lines, "name", "held");
        holds.push(delay(raised, b_at));
        let focus: Vec<u64> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heard { kind, name, at, .. }
                    if kind == "focus" && name == FIRST_ITEM && *at >= opened_at =>
                {
                    Some(*at)
                }
                _ => None,
            })
            .collect();
        note::evidence(&format!(
            "the menu's focus heard, ms after it opened: {:?}",
            focus
                .iter()
                .map(|at| us_to_ms(at - opened_at))
                .collect::<Vec<_>>()
        ));
        escaped.push(focus.iter().any(|at| b_at.is_none_or(|b| *at < b)));
    }
    report.claim(
        claim,
        &runs(&escaped),
        "false: held with B's change (the first run, 4 of 4)",
        deterministic(&escaped, &false),
    );
    report.claim(
        "B's name change in the same hold",
        &summary_ms(&holds),
        &band_text(two),
        in_band(&holds, two),
    );
}
