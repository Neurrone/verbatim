//! A process with no UIA focus registration is not held by a hung window
//! (`uia-facts/a-process-with-no-uia-focus-registration-is-not-held-by-a-hung-window/readme.md`).
//!
//! The scene is support's `uia_facts_support::hung`, the hold's own
//! program's: T stalled 0.9 s, past two 250 ms waits, a focus `WinEvent`
//! naming it, and the outpost stand-in of `e_cross`, a client with no
//! focus registration: B's name and text changes 100 ms after the
//! `WinEvent`, then a name-change `WinEvent` naming M heard by its MSAA
//! hook, and a name read of B on another thread.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::hung::{NOT_HELD_MS, delay, hooked, not_held, read_end, stand_in};
use uia_facts_support::listen::{self, heard_at};
use uia_facts_support::report::{Report, relation};
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
        Report::start("a-process-with-no-uia-focus-registration-is-not-held-by-a-hung-window");
    if args.wants("outpost") {
        report.case("outpost", "T stalled 0.9 s and a focus WinEvent naming it; the outpost stand-in, with no focus registration, hearing B's name and text changes, a name WinEvent naming M through its MSAA hook, and reading B's name");
        outpost(&args, &mut report);
    }
    report.finish();
}

/// The outpost stand-in, with no focus registration, not held.
fn outpost(args: &Args, report: &mut Report) {
    let mut outpost: Vec<[f64; 4]> = Vec::new();
    for _ in 0..trials() {
        let (lines, [_, renamed, texted, m_name, _, read_at], window_m, _) =
            stand_in(args, "outpost stand-in", true);
        let o = [
            delay(renamed, heard_at(&lines, "name", "held")),
            delay(texted, heard_at(&lines, "text", "Notes")),
            delay(read_at, read_end(&lines)),
            delay(m_name, hooked(&lines, 0x800C, window_m)),
        ];
        note::evidence(&format!(
            "outpost stand-in: name, text, read, hook {o:?} ms"
        ));
        outpost.push(o);
    }
    for (i, what, recorded) in [
        (0, "B's name change", "1 ms (e_cross; P4's N_B)"),
        (1, "B's text change", "0 ms (e_cross)"),
        (2, "a name read of B on another thread", "0 ms (e_cross)"),
        (3, "its MSAA hook's name change of M", "0 ms (e_cross)"),
    ] {
        let values: Vec<f64> = outpost.iter().map(|o| o[i]).collect();
        report.claim(
            &format!("a client with no focus registration, the outpost stand-in: {what} within {NOT_HELD_MS} ms, not held"),
            &summary_ms(&values),
            recorded,
            relation(&[not_held(&values)]),
        );
    }
}
