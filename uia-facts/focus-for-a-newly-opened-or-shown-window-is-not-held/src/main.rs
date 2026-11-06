//! Focus for a newly opened or shown window is not held
//! (`uia-facts/focus-for-a-newly-opened-or-shown-window-is-not-held/readme.md`).
//!
//! On the unlocked desktop, run only when named: Notepad in front, the
//! hung-window scene of support's `uia_facts_support::hung` shown on the
//! user's desktop, T stalled 9 s and a focus `WinEvent` naming it, B
//! renamed 200 ms later, and Alt+F 0.6 s after that: Notepad's File menu's
//! focus reaches Verbatim's stand-in while B's name change is still held.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::hung::{B_AFTER_US, CTO_MS, Scene, as_ms, delay, until, us};
use uia_facts_support::listen::{self, Line, Listener, heard_at};
use uia_facts_support::report::{Report, band, band_text, in_band, relation, runs};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::{apps, note, now_us, precondition, uia};

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
        report.case("open-show", "on the unlocked desktop, Notepad in front; T stalled 9 s; a focus WinEvent naming T; B renamed 200 ms later; Alt+F 0.6 s after that");
        open_show(&args, &mut report);
    }
    report.finish();
}

fn open_show(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 9_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("open-show: Notepad's menu focus during a hold", &reason);
        return;
    }
    let notepad = apps::system_program("notepad.exe");
    println!(
        "notepad: {}",
        uia_facts_support::report::file_version(&notepad)
    );
    let mut escaped = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let before = apps::top_windows();
        let _ = std::process::Command::new(&notepad).spawn();
        note::waiting("up to 10 s, Notepad's window");
        let Some(window) =
            apps::wait_window("Notepad", "Notepad.exe", &before, Duration::from_secs(10))
        else {
            report.unmet(
                "open-show: Notepad's menu focus during a hold",
                "Notepad's window did not appear",
            );
            return;
        };
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
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client", d.pid())]);
        let front = apps::foreground(window);
        note::evidence(&format!("Notepad in front: {front}"));
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        until(raised + 800_000, "Alt+F, 0.6 s after B's name change");
        let pressed = now_us();
        apps::alt_f();
        let lines = d.lines_until(stalled + us(as_ms(STALL_MS)));
        apps::escape();
        apps::kill(apps::window_pid(window));
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
            "focus heard after Alt+F, ms after it: {:?}",
            focus
                .iter()
                .map(|(n, at)| (n.clone(), us_to_ms(at - pressed)))
                .collect::<Vec<_>>()
        ));
        escaped.push(focus.iter().any(|(_, at)| b_at.is_none_or(|b| *at < b)));
    }
    report.claim(
        "Notepad's menu focus after Alt+F reaches the stand-in while B's name change is still held",
        &runs(&escaped),
        "the menu's focus 26 to 112 ms after the key, the notification held 3,815 to 3,829 ms (both; route1, route2)",
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
