//! A hung window named in a system focus event holds a process's UIA
//! events
//! (`uia-facts/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md`).
//!
//! The scene: provider program P1 holds T, a plain Win32 window on UI
//! thread 0, the window made to hang; A, a UIA window on thread 1; and M, a
//! window with its own `IAccessible` on thread 2. Provider program P2 holds
//! B, the UIA window whose name changes are listened for. A stall is a UI
//! thread busy, taking no message. The focus `WinEvent` naming T is raised
//! from P1's control thread. Each client is a listening process of its own
//! (`uia_facts_support::listen`), so its UIA has seen nothing before the
//! trial, and each trial has new providers and new clients. A hold is the
//! time from the focus `WinEvent` to a client hearing B's name change,
//! raised 200 ms after the `WinEvent`.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::listen::{self, Line, Listener, heard_at};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, TICK_MS, Verdict, band, band_text, deterministic, in_band, rate, relation, runs,
};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::{apps, note, now_us, precondition, uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation, UIA_AutomationFocusChangedEventId, UIA_PROPERTY_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, IsHungAppWindow,
};

/// B's name change follows the focus `WinEvent` by this much, as in every
/// original experiment.
const B_AFTER_US: u64 = 200_000;
/// A delivery counts as not held when it comes within this long of its
/// raise: the originals measured 0 to 3 ms, and a hold is at least one
/// connection timeout, 500 ms or more here.
const NOT_HELD_MS: f64 = 300.0;
/// The desktop of its own the cases that ran on one run on.
const DESKTOP: &str = "uia-facts";
/// The connection timeout every client has unless its spec says otherwise.
const CTO_MS: f64 = 2_000.0;

/// The windows of one trial.
struct Scene {
    p1: Provider,
    p2: Provider,
}

impl Scene {
    fn new() -> Self {
        Self::with(&["--windows", "plain:0,uia:1,msaa:2"])
    }

    fn with(p1: &[&str]) -> Self {
        Self::on(None, p1)
    }

    /// The scene on desktop `desktop`, when given.
    fn on(desktop: Option<&str>, p1: &[&str]) -> Self {
        Self {
            p1: Provider::launch_on(desktop, p1),
            p2: Provider::launch_on(desktop, &["--windows", "uia:0"]),
        }
    }

    fn b(&self) -> isize {
        self.p2.hwnd(0)
    }

    fn b_spec(&self) -> String {
        format!("name={}", self.b())
    }

    /// Makes P1's window `w` UI thread busy for `ms`; answers when it
    /// started.
    fn stall(&mut self, w: usize, ms: u64) -> u64 {
        note::waiting(&format!(
            "{:.1} s, the stall of P1's window {w}",
            as_ms(ms) / 1_000.0
        ));
        self.p1
            .request(&format!("busy {w} {ms}"))
            .parse()
            .expect("the time the stall started")
    }

    /// Raises `WinEvent` `kind` naming P1's window `w`; answers when.
    fn winevent(&mut self, kind: &str, w: usize) -> u64 {
        self.p1
            .request(&format!("winevent {kind} {w}"))
            .parse()
            .expect("the time it was raised")
    }

    /// Renames B's node 1 `name` and raises its name change; answers when.
    fn rename_b(&mut self, name: &str) -> u64 {
        self.p2.request(&format!("rename 0 1 {name}"));
        self.p2
            .request("raise name 0 1")
            .parse()
            .expect("the time it was raised")
    }

    /// Raises a text change on B's node 3; answers when.
    fn text_b(&mut self) -> u64 {
        self.p2
            .request("raise text 0 3")
            .parse()
            .expect("the time it was raised")
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "the facts' durations are far below 2 to the 52nd"
)]
fn as_ms(ms: u64) -> f64 {
    ms as f64
}

/// Waits until `at` on [`now_us`]'s clock: an offset in the scene's
/// timeline, a deliberate wait.
fn until(at: u64, why: &str) {
    uia::pause(Duration::from_micros(at.saturating_sub(now_us())), why);
}

/// Milliseconds from `from` to `to`; infinite when `to` never came.
fn delay(from: u64, to: Option<u64>) -> f64 {
    to.map_or(f64::INFINITY, |to| us_to_ms(to.saturating_sub(from)))
}

/// Milliseconds as a duration on [`now_us`]'s clock.
fn us(ms: f64) -> u64 {
    Duration::from_secs_f64(ms / 1_000.0)
        .as_micros()
        .try_into()
        .expect("a short time")
}

fn spec(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// A case: its name, what it does, and its function.
type Case = (&'static str, &'static str, fn(&Args, &mut Report));

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report =
        Report::start("a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events");
    let cases: [Case; 13] = [
        (
            "calls",
            "a client with a desktop focus handler and B's name changes; T stalled 5 s; a focus WinEvent naming T; B renamed 200 ms later; counting the sent messages that reach T",
            calls,
        ),
        (
            "who",
            "seventeen clients that differ in one thing each, from timeouts and apartments to client objects, mappings and registrations; T stalled 8 s; a focus WinEvent naming T; B renamed and its text changed 200 ms later; then a name and a foreground WinEvent naming M, and a name read of B from each of two clients",
            who,
        ),
        (
            "until-answer",
            "a client at a 2 s connection timeout; T stalled 3 s, shorter than two waits; a focus WinEvent naming T; B renamed 200 ms later",
            until_answer,
        ),
        (
            "repeat",
            "a client at a 1 s connection timeout; T stalled 8 s; three focus WinEvents naming T at once; B renamed 200 ms later",
            repeat,
        ),
        (
            "quiet",
            "a client with a desktop focus handler; T stalled 3 s; B renamed with no WinEvent; then a name-change WinEvent naming T and B renamed again",
            quiet,
        ),
        (
            "mapping",
            "reading which entries of a new client object's proxy factory mapping map the focus WinEvent to UIA's focus event",
            mapping,
        ),
        (
            "slow",
            "a UIA window U whose every provider call takes 1 s while it keeps answering messages; a focus WinEvent naming U; B renamed 200 ms later; a client with a focus registration and one without",
            slow,
        ),
        (
            "own-focus",
            "a window whose every provider call takes 3 s raising its own focus, 4 times as a UIA provider and 6 times as an MSAA window; B renamed 200 ms later",
            own_focus,
        ),
        (
            "dropped",
            "on a desktop of its own, M moving its focus while it answers, then again at the start of a 3 s stall of its UI thread; a client with a desktop focus handler at a connection timeout of 10 s, then 2 s, then 0.5 s, each in a trial of its own",
            dropped,
        ),
        (
            "shown",
            "on the unlocked desktop, T shown in front; T stalled 13 s; a focus WinEvent naming T at once; asking each 0.1 s whether T is hung; once it is, another WinEvent; then Alt+Tab away from it",
            shown,
        ),
        (
            "alt-tab-back",
            "on the unlocked desktop, T and another window shown, the other in front; T stalled 12 s; Alt+Tab back to T 2 s in, before Windows marks it hung, and again once it is marked",
            alt_tab_back,
        ),
        (
            "open-show",
            "on the unlocked desktop, Notepad in front; T stalled 9 s; a focus WinEvent naming T; B renamed 200 ms later; Alt+F 0.6 s after that",
            open_show,
        ),
        (
            "desk-wide",
            "on the unlocked desktop, T and B shown; a client with a desktop focus handler and a desktop-wide name registration, and one with the name registration alone; B renamed as a control; T stalled 3 s; a focus WinEvent naming T, B renamed, then a name-change WinEvent naming T and B renamed again",
            desk_wide,
        ),
    ];
    for (name, what, run) in cases {
        if args.wants(name) {
            report.case(name, what);
            run(&args, &mut report);
        }
    }
    report.finish();
}

fn calls(args: &Args, report: &mut Report) {
    let mut holds = Vec::new();
    let mut counts = Vec::new();
    let mut gaps = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[
            ("P1 (T)", scene.p1.pid),
            ("P2 (B)", scene.p2.pid),
            ("client D", d.pid()),
        ]);
        let _ = scene.p1.log();
        let stalled = scene.stall(0, 5_000);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        let lines = d.lines_until(stalled + 5_500_000);
        let hold = delay(raised, heard_at(&lines, "name", "held"));
        note::evidence(&format!(
            "B's name change heard {hold:.1} ms after the WinEvent"
        ));
        holds.push(hold);
        let log = scene.p1.log();
        let thread = scene.p1.windows[0].thread;
        let arrivals: Vec<u64> = log
            .iter()
            .filter(|e| {
                e.kind == "queued" && e.name == "sent" && e.thread == thread && e.start >= raised
            })
            .map(|e| e.start)
            .collect();
        note::evidence(&format!(
            "sent messages reached T {:?} ms after the WinEvent",
            arrivals
                .iter()
                .map(|a| us_to_ms(a - raised))
                .collect::<Vec<_>>()
        ));
        counts.push(arrivals.len());
        gaps.push(match arrivals.as_slice() {
            [first, second, ..] => us_to_ms(second - first),
            _ => f64::INFINITY,
        });
    }
    let two = band(&[CTO_MS, CTO_MS]);
    report.claim(
        "B's name change, held from the focus WinEvent naming the stalled T, at a 2,000 ms connection timeout",
        &summary_ms(&holds),
        &format!(
            "3,806 to 3,821 ms after B's raise, 200 ms after the WinEvent; {}",
            band_text(two)
        ),
        in_band(&holds, two),
    );
    report.claim(
        "sent messages that reach the stalled T after the focus WinEvent",
        &runs(&counts),
        "2 (WM_GETOBJECT, then UiaServerConnection_HookMessage)",
        deterministic(&counts, &2),
    );
    let one = band(&[CTO_MS]);
    report.claim(
        "the second message reaches T one connection timeout after the first",
        &summary_ms(&gaps),
        &format!("2.01 s (h2_base); {}", band_text(one)),
        in_band(&gaps, one),
    );
}

/// How a client of `who` is judged.
enum Held {
    /// Held for the sum of these waits, from the `WinEvent`.
    Waits(&'static [f64]),
    /// Held until T answers, at the stall's end.
    ToStallEnd,
}

/// The clients of `who`: name, spec less B's name registration, how it is
/// held, and the original figure.
const WHO: [(&str, &[&str], Held, &str); 15] = [
    (
        "connection timeout 500 ms",
        &["desk", "cto=500"],
        Held::Waits(&[500.0, 500.0]),
        "812 ms (h3 cto500)",
    ),
    (
        "connection timeout 1,000 ms",
        &["desk", "cto=1000"],
        Held::Waits(&[1_000.0, 1_000.0]),
        "1,824 ms (repeat1_cto1000)",
    ),
    (
        "transaction timeout 500 ms",
        &["desk", "tto=500"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "3,810 ms (h3_tto500)",
    ),
    (
        "a single-threaded apartment",
        &["sta", "desk"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "3,809 ms (h3 sta)",
    ),
    (
        "a focus group on B in place of the desktop handler",
        &["group=B"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "3,809 ms (grouponly; P4's O_B)",
    ),
    (
        "a focus group on A, another application's window",
        &["group=A"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "4,054 and 4,070 ms from the stall (P4's O_A)",
    ),
    (
        "focus on a second client object on the same thread, 500 ms",
        &["deskobj", "cto=500"],
        Held::Waits(&[500.0, 500.0]),
        "815 ms (focusnew500_namemain)",
    ),
    (
        "focus on a second client object on a thread of its own, 500 ms",
        &["deskthr", "cto=500"],
        Held::Waits(&[500.0, 500.0]),
        "817 ms (focusthr500_namemain)",
    ),
    (
        "the name registration on a client object on a thread of its own, 500 ms",
        &["desk", "namethr", "cto=500"],
        Held::Waits(&[500.0, 500.0]),
        "819 ms (focusmain_namethr500)",
    ),
    (
        "two client objects, each with a desktop handler, 500 ms",
        &["desk", "deskobj", "cto=500"],
        Held::Waits(&[500.0, 500.0, 500.0, 500.0]),
        "1,820 ms",
    ),
    (
        "two desktop handlers on one client object, 500 ms",
        &["desk", "desk2", "cto=500"],
        Held::Waits(&[500.0, 500.0]),
        "807 ms (twodesk_500)",
    ),
    (
        "the proxy factory mapping emptied",
        &["desk", "clearmap"],
        Held::Waits(&[CTO_MS]),
        "1,812 ms (clearmap)",
    ),
    (
        "the focus WinEvents removed from the mapping",
        &["desk", "nofocusmap"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "14,789 against 14,801 ms at 10 s (e_mapping)",
    ),
    (
        "Verbatim's mapping and timeouts, against a hidden T",
        &["desk", "vbmap", "tto=5000"],
        Held::Waits(&[CTO_MS, CTO_MS]),
        "3,806 to 3,821 ms (states' stand-in)",
    ),
    (
        "the older CUIAutomation coclass",
        &["older", "desk"],
        Held::ToStallEnd,
        "until the window answered (h2_cui)",
    ),
];

/// The stall of `who`: longer than the older coclass's first wait, 5 s,
/// and a default second wait, 2 s, together.
const WHO_STALL_MS: u64 = 8_000;

#[expect(
    clippy::too_many_lines,
    reason = "one trial, its clients judged in turn"
)]
fn who(args: &Args, report: &mut Report) {
    let mut holds: Vec<Vec<f64>> = vec![Vec::new(); WHO.len()];
    let mut stall_ends = Vec::new();
    let mut outpost: Vec<[f64; 4]> = Vec::new();
    let mut listener: Vec<[f64; 4]> = Vec::new();
    let mut older_cast = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let (window_a, window_b, window_m) = (scene.p1.hwnd(1), scene.b(), scene.p1.hwnd(2));
        let clients: Vec<Listener> = WHO
            .iter()
            .map(|(label, words, _, _)| {
                let mut words: Vec<String> = words
                    .iter()
                    .map(|w| match *w {
                        "group=B" => format!("group={window_b}"),
                        "group=A" => format!("group={window_a}"),
                        "namethr" => format!("namethr={window_b}"),
                        other => other.to_owned(),
                    })
                    .collect();
                if !words.iter().any(|w| w.starts_with("namethr")) {
                    words.push(scene.b_spec());
                }
                Listener::start(label, &words)
            })
            .collect();
        let mut outpost_client = Listener::start(
            "outpost stand-in",
            &[
                scene.b_spec(),
                format!("text={window_b}"),
                format!("hook={}", scene.p1.pid),
            ],
        );
        let mut listener_client = Listener::start(
            "listener stand-in",
            &["desk".to_owned(), scene.b_spec(), "hook=0".to_owned()],
        );
        let mut pids = vec![("P1 (T)", scene.p1.pid), ("P2 (B)", scene.p2.pid)];
        pids.extend(clients.iter().map(|c| (c.label.as_str(), c.pid())));
        args.pause_for_debugger(&pids);
        let stalled = scene.stall(0, WHO_STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's events after the WinEvent");
        let renamed = scene.rename_b("held");
        let texted = scene.text_b();
        until(raised + 300_000, "the WinEvents naming M");
        let m_name = scene.winevent("name", 2);
        let m_foreground = scene.winevent("foreground", 2);
        let read_at = now_us();
        outpost_client.send(&format!("read {window_b}"));
        listener_client.send(&format!("read {window_b}"));
        let end = stalled + us(as_ms(WHO_STALL_MS) + 1_000.0);
        stall_ends.push(stalled + us(as_ms(WHO_STALL_MS)));
        for (i, client) in clients.iter().enumerate() {
            let lines = client.lines_until(end);
            let hold = delay(raised, heard_at(&lines, "name", "held"));
            note::evidence(&format!(
                "{}: B's name change {hold:.1} ms after the WinEvent",
                client.label
            ));
            holds[i].push(hold);
        }
        let read = |lines: &[Line]| {
            lines.iter().find_map(|l| match l {
                Line::Read { outcome, end, .. } if outcome == "ok" => Some(*end),
                _ => None,
            })
        };
        let hooked = |lines: &[Line], event: u32, hwnd: isize| {
            lines.iter().find_map(|l| match l {
                Line::Hook {
                    event: e,
                    hwnd: h,
                    at,
                } if *e == event && *h == hwnd => Some(*at),
                _ => None,
            })
        };
        let lines = outpost_client.lines_until(end);
        let o = [
            delay(renamed, heard_at(&lines, "name", "held")),
            delay(texted, heard_at(&lines, "text", "Notes")),
            delay(read_at, read(&lines)),
            delay(m_name, hooked(&lines, 0x800C, window_m)),
        ];
        note::evidence(&format!(
            "outpost stand-in: name, text, read, hook {o:?} ms"
        ));
        outpost.push(o);
        let lines = listener_client.lines_until(end);
        let l = [
            delay(raised, heard_at(&lines, "name", "held")),
            delay(raised, hooked(&lines, EVENT_OBJECT_FOCUS, scene.p1.hwnd(0))),
            delay(
                m_foreground,
                hooked(&lines, EVENT_SYSTEM_FOREGROUND, window_m),
            ),
            delay(read_at, read(&lines)),
        ];
        note::evidence(&format!(
            "listener stand-in: name from the WinEvent, focus hook, foreground hook, read {l:?} ms"
        ));
        listener.push(l);
        let older: Result<windows::Win32::UI::Accessibility::IUIAutomation2, _> =
            windows::core::Interface::cast(&uia::older_client());
        older_cast.push(older.map_or_else(|e| uia::hresult(e.code()), |_| "ok".to_owned()));
    }
    for (i, (label, _, held, recorded)) in WHO.iter().enumerate() {
        match held {
            Held::Waits(waits) => {
                let range = band(waits);
                report.claim(
                    &format!("{label}: B's name change held from the WinEvent"),
                    &summary_ms(&holds[i]),
                    &format!("{recorded}; {}", band_text(range)),
                    in_band(&holds[i], range),
                );
            }
            Held::ToStallEnd => {
                let after: Vec<bool> = holds[i]
                    .iter()
                    .zip(&stall_ends)
                    .map(|(hold, _)| *hold >= as_ms(WHO_STALL_MS) - TICK_MS)
                    .collect();
                report.claim(
                    &format!("{label}: B's name change held until T answers, at the stall's end, past a 5 s and a 2 s wait"),
                    &format!("{} ({})", runs(&after), summary_ms(&holds[i])),
                    recorded,
                    relation(&after),
                );
            }
        }
    }
    report.claim(
        "the older CUIAutomation coclass's client object has no IUIAutomation2, so its timeouts cannot be set",
        &runs(&older_cast),
        "E_NOINTERFACE (h3 cui_cto500)",
        deterministic(&older_cast, &"0x80004002".to_owned()),
    );
    let not_held = |values: &[f64]| values.iter().all(|v| *v <= NOT_HELD_MS);
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
    let first: Vec<f64> = listener.iter().map(|l| l[0]).collect();
    report.claim(
        "the listener stand-in, with a desktop focus handler: B's name change held at least one connection timeout from the WinEvent",
        &summary_ms(&first),
        "9,697 to 9,699 ms at 10 s (e_cross)",
        relation(
            &first
                .iter()
                .map(|v| *v >= CTO_MS - TICK_MS)
                .collect::<Vec<_>>(),
        ),
    );
    for (i, what, recorded) in [
        (
            1,
            "its MSAA focus hook hears the WinEvent naming T",
            "0 ms (e_cross)",
        ),
        (
            2,
            "its MSAA foreground hook hears a foreground WinEvent",
            "0 ms (e_cross)",
        ),
        (
            3,
            "a name read of B on another thread of the held process",
            "1 ms (E-hung-proxy)",
        ),
    ] {
        let values: Vec<f64> = listener.iter().map(|l| l[i]).collect();
        report.claim(
            &format!("the listener stand-in, while held: {what} within {NOT_HELD_MS} ms"),
            &summary_ms(&values),
            recorded,
            relation(&[not_held(&values)]),
        );
    }
}

fn until_answer(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 3_000;
    let mut answered = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        let end = stalled + us(as_ms(STALL_MS) + 2_000.0);
        let heard = heard_at(&d.lines_until(end), "name", "held");
        let hold = delay(raised, heard);
        let stall_end = stalled + us(as_ms(STALL_MS));
        note::evidence(&format!(
            "B's name change {hold:.1} ms after the WinEvent; the stall ended {:.1} ms after it",
            us_to_ms(stall_end - raised)
        ));
        holds.push(hold);
        answered.push(heard.is_some_and(|h| h >= stall_end && hold < band(&[CTO_MS, CTO_MS]).0));
    }
    report.claim(
        "B's name change, at a 2,000 ms connection timeout with T stalled 3 s: held until T answers, after the stall's end and before two waits end",
        &format!("{} ({})", runs(&answered), summary_ms(&holds)),
        "7,773 ms, the end of an 8 s stall, at 10 s (h3 to10000_1); 14,800 ms in a 15 s stall (E-hung-proxy)",
        relation(&answered),
    );
}

fn repeat(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 8_000;
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start(
            "D",
            &["desk".to_owned(), "cto=1000".to_owned(), scene.b_spec()],
        );
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        scene.winevent("focus", 0);
        scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvents");
        scene.rename_b("held");
        let lines = d.lines_until(stalled + us(as_ms(STALL_MS) + 500.0));
        holds.push(delay(raised, heard_at(&lines, "name", "held")));
    }
    let six = band(&[1_000.0; 6]);
    report.claim(
        "B's name change after three focus WinEvents naming T at a 1,000 ms connection timeout: each pays both waits again",
        &summary_ms(&holds),
        &format!("5,814 ms after B's raise (repeat3_cto1000); {}", band_text(six)),
        in_band(&holds, six),
    );
}

fn quiet(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 3_000;
    let mut none = Vec::new();
    let mut name_d = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let stalled = scene.stall(0, STALL_MS);
        until(stalled + B_AFTER_US, "B's name change into the stall");
        let first = scene.rename_b("first");
        until(stalled + 1_000_000, "the name-change WinEvent naming T");
        let name_event = scene.winevent("name", 0);
        until(
            name_event + B_AFTER_US,
            "B's name change after the WinEvent",
        );
        let second = scene.rename_b("second");
        let end = stalled + us(as_ms(STALL_MS) + 500.0);
        let lines = d.lines_until(end);
        none.push(delay(first, heard_at(&lines, "name", "first")));
        name_d.push(delay(second, heard_at(&lines, "name", "second")));
    }
    let within =
        |values: &[f64]| relation(&values.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>());
    report.claim(
        &format!("a client with a desktop focus handler, T stalled and named by no WinEvent: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&none),
        "0 ms (E-hung-proxy's control)",
        within(&none),
    );
    report.claim(
        &format!("the same client after a name-change WinEvent naming the stalled T: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&name_d),
        "2 to 3 ms (e_hung_cause)",
        within(&name_d),
    );
}

fn mapping(_args: &Args, report: &mut Report) {
    let mut found = Vec::new();
    for _ in 0..trials() {
        let client: IUIAutomation = uia::as_interface(&uia::client());
        found.push(focus_entries(&client));
    }
    note::evidence(&format!(
        "classes whose entries map the focus WinEvent: {found:?}"
    ));
    report.claim(
        "the class names of the mapping's entries that map the focus WinEvent to UIA's focus event",
        &runs(&found),
        "SysListView32 in two entries (e_mapping)",
        deterministic(
            &found,
            &vec!["SysListView32".to_owned(), "SysListView32".to_owned()],
        ),
    );
}

/// The class name of each entry of `client`'s proxy factory mapping that
/// maps `EVENT_OBJECT_FOCUS` to UIA's focus event.
fn focus_entries(client: &IUIAutomation) -> Vec<String> {
    // SAFETY: local calls on a live client and its mapping.
    let mapping = unsafe { client.ProxyFactoryMapping() }.expect("the mapping");
    // SAFETY: as above.
    let count = unsafe { mapping.Count() }.expect("the entry count");
    let mut classes = Vec::new();
    for index in 0..count {
        // SAFETY: as above, with an index below the count.
        let entry = unsafe { mapping.GetEntry(index) }.expect("an entry");
        // SAFETY: a local call on a live entry; the array is read and
        // destroyed below.
        let array = unsafe {
            entry.GetWinEventsForAutomationEvent(
                UIA_AutomationFocusChangedEventId,
                UIA_PROPERTY_ID(0),
            )
        }
        .expect("the entry's WinEvents");
        let events = take_i32s(array);
        if events.contains(&i32::try_from(EVENT_OBJECT_FOCUS).expect("a small number")) {
            // SAFETY: a local call on a live entry.
            let class =
                unsafe { entry.ClassName() }.map_or_else(|_| "?".to_owned(), |c| c.to_string());
            classes.push(class);
        }
    }
    classes
}

/// The integers of a SAFEARRAY UIA returned, which is destroyed.
fn take_i32s(array: *mut windows::Win32::System::Com::SAFEARRAY) -> Vec<i32> {
    use windows::Win32::System::Ole::{SafeArrayDestroy, SafeArrayGetElement};
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: a live one-dimensional array UIA returned.
    let count = unsafe { (*array).rgsabound[0].cElements };
    let mut values = Vec::new();
    for i in 0..i32::try_from(count).expect("a small array") {
        let mut value = 0i32;
        // SAFETY: an index inside the array and a live i32 to fill.
        if unsafe { SafeArrayGetElement(array, &raw const i, (&raw mut value).cast()) }.is_ok() {
            values.push(value);
        }
    }
    // SAFETY: the array, owned here, destroyed once.
    let _ = unsafe { SafeArrayDestroy(array) };
    values
}

fn slow(args: &Args, report: &mut Report) {
    let mut held = Vec::new();
    let mut free = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::with(&["--windows", "uia:0"]);
        let d = Listener::start(
            "D",
            &["desk".to_owned(), "tto=5000".to_owned(), scene.b_spec()],
        );
        let d0 = Listener::start("D0", &["tto=5000".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("U's process", scene.p1.pid), ("client D", d.pid())]);
        scene.p1.request("slow 1000");
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        let renamed = scene.rename_b("held");
        note::waiting("up to 10 s, B's name change through the hold");
        let heard = d.next(
            Duration::from_secs(10),
            |l| matches!(l, Line::Heard { kind, name, .. } if kind == "name" && name == "held"),
        );
        let at = match heard {
            Some(Line::Heard { at, .. }) => Some(at),
            _ => None,
        };
        held.push(delay(raised, at));
        free.push(delay(
            renamed,
            heard_at(&d0.lines_until(now_us()), "name", "held"),
        ));
        scene.p1.request("slow 0");
    }
    report.claim(
        "a focus WinEvent naming U, whose provider is slow: B's name change held at least one connection timeout",
        &summary_ms(&held),
        "3,800 to 3,968 ms at 1,000 ms a call (q3)",
        relation(&held.iter().map(|v| *v >= CTO_MS - TICK_MS && v.is_finite()).collect::<Vec<_>>()),
    );
    report.claim(
        &format!("the same in a client with no focus registration: within {NOT_HELD_MS} ms"),
        &summary_ms(&free),
        "1 to 3 ms (q3, D0)",
        relation(&free.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
}

fn own_focus(args: &Args, report: &mut Report) {
    for (window, label, n, recorded_k) in [
        (0usize, "a UIA provider", 4usize, 1usize),
        (1, "an MSAA window", 6, 4),
    ] {
        let mut k = 0;
        let mut delays = Vec::new();
        for _ in 0..n {
            let mut scene = Scene::with(&["--windows", "uia:0,msaa:1"]);
            let d = Listener::start(
                "D",
                &["desk".to_owned(), "tto=5000".to_owned(), scene.b_spec()],
            );
            args.pause_for_debugger(&[("the slow process", scene.p1.pid), ("client D", d.pid())]);
            scene.p1.request("slow 3000");
            let raised = now_us();
            let request = if window == 0 {
                "raise focus 0 5".to_owned()
            } else {
                "raise focus 1 0".to_owned()
            };
            let held = std::thread::scope(|scope| {
                let p1 = &mut scene.p1;
                let raising = scope.spawn(move || {
                    p1.request(&request);
                });
                until(raised + B_AFTER_US, "B's name change after the raise");
                scene.p2.request("rename 0 1 held");
                let renamed: u64 = scene.p2.request("raise name 0 1").parse().expect("a time");
                note::waiting("up to 8 s, B's name change");
                let heard = d.next(Duration::from_secs(8), |l| matches!(l, Line::Heard { kind, name, .. } if kind == "name" && name == "held"));
                let at = match heard {
                    Some(Line::Heard { at, .. }) => Some(at),
                    _ => None,
                };
                let late = delay(renamed, at);
                let _ = raising.join();
                late
            });
            delays.push(held);
            if held > 1_000.0 {
                k += 1;
            }
            scene.p1.request("slow 0");
        }
        note::evidence(&format!("{label}: B's name change after {delays:?} ms"));
        report.claim(
            &format!("{label} raising its own focus while its provider is slow: trials whose B name change is held more than 1 s"),
            &format!("{k} of {n} ({})", summary_ms(&delays)),
            &format!("{recorded_k} of {n} (q3 slow_{}_focus)", if window == 0 { "uia" } else { "msaa" }),
            rate(k, n, recorded_k, n),
        );
    }
}

fn dropped(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 3_000;
    let timeouts = [10_000u32, 2_000, 500];
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
            // then to its second at the start of a 3 s stall, from inside
            // it.
            let raised = scene
                .p1
                .request("raise focus 2 1")
                .parse::<u64>()
                .expect("a time");
            let lines = client.lines_until(raised + 500_000);
            control[i].push(delay(raised, heard_at(&lines, "focus", "First")));
            note::waiting(&format!(
                "3.0 s, M's stall, then 5 s for its focus, at {t} ms"
            ));
            let stalled: u64 = scene
                .p1
                .request(&format!("busy 2 {STALL_MS} focus-at 0 2"))
                .parse()
                .expect("a time");
            let lines = client.lines_until(stalled + 8_000_000);
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
        "connection timeout 10,000 ms: M's focus from inside its 3 s stall is heard once the stall ends",
        &summary_ms(&heard[0]),
        "3,003 ms into the stall (h6_cto10000)",
        relation(&heard[0].iter().map(|v| v.is_finite() && *v >= as_ms(STALL_MS) - TICK_MS).collect::<Vec<_>>()),
    );
    report.claim(
        "connection timeout 2,000 ms, the stall shorter than both waits: M's focus from inside it is heard once the stall ends",
        &summary_ms(&heard[1]),
        "mockapp's MSAA window: never within 8 s (h6_default); the provider program's: at the stall's end",
        relation(&heard[1].iter().map(|v| v.is_finite() && *v >= as_ms(STALL_MS) - TICK_MS).collect::<Vec<_>>()),
    );
    report.claim(
        "connection timeout 500 ms, the stall longer than both waits: M's focus from inside it is not heard within 8 s of the stall's start",
        &summary_ms(&heard[2]),
        "never delivered within 8 s (h6_cto500)",
        relation(&heard[2].iter().map(|v| v.is_infinite()).collect::<Vec<_>>()),
    );
}

/// Asks Windows each 0.1 s whether `hwnd` is hung, until it is or until
/// `limit` on [`now_us`]'s clock; answers when it first was.
fn marked_hung(hwnd: isize, limit: u64) -> Option<u64> {
    while now_us() < limit {
        // SAFETY: any window handle is accepted.
        if unsafe { IsHungAppWindow(HWND(hwnd as *mut _)) }.as_bool() {
            return Some(now_us());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn shown(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 13_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("shown: the holds, the marking and Alt+Tab", &reason);
        return;
    }
    let mut fresh = Vec::new();
    let mut fresh_vb = Vec::new();
    let mut marking = Vec::new();
    let mut after_mark = Vec::new();
    let mut away = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        let vb = Listener::start(
            "Verbatim's stand-in",
            &[
                "desk".to_owned(),
                "vbmap".to_owned(),
                "tto=5000".to_owned(),
                scene.b_spec(),
            ],
        );
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let t = scene.p1.hwnd(0);
        let front = scene.p1.request("foreground 0");
        note::evidence(&format!("T in front: {front}"));
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("fresh");
        let mark = marked_hung(t, stalled + 6_500_000);
        let lines = d.lines_until(now_us());
        fresh.push(delay(raised, heard_at(&lines, "name", "fresh")));
        let vb_lines = vb.lines_until(now_us());
        fresh_vb.push(delay(raised, heard_at(&vb_lines, "name", "fresh")));
        marking.push(delay(stalled, mark));
        until(stalled + 6_500_000, "6.5 s into the stall");
        let again = scene.winevent("focus", 0);
        until(again + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("marked");
        let lines = d.lines_until(again + 3_000_000);
        after_mark.push(delay(again, heard_at(&lines, "name", "marked")));
        apps::alt_tab();
        let switched = now_us();
        until(switched + B_AFTER_US, "B's name change after Alt+Tab");
        let renamed = scene.rename_b("away");
        let lines = d.lines_until(renamed + 1_000_000);
        away.push(delay(renamed, heard_at(&lines, "name", "away")));
        until(stalled + us(as_ms(STALL_MS)), "the stall's end");
        scene.p1.request("hide 0");
    }
    let two = band(&[CTO_MS, CTO_MS]);
    report.claim(
        "T shown and not yet marked hung: B's name change held from the WinEvent",
        &summary_ms(&fresh),
        &format!(
            "3,991 and 4,166 ms after B's raise (h7_fresh); {}",
            band_text(two)
        ),
        in_band(&fresh, two),
    );
    report.claim(
        "the same in Verbatim's stand-in, Verbatim's mapping and timeouts",
        &summary_ms(&fresh_vb),
        &format!(
            "3,811 to 3,819 ms after B's raise (states, a shown window); {}",
            band_text(two)
        ),
        in_band(&fresh_vb, two),
    );
    report.claim(
        "when Windows first answers that the stalled, shown T is hung, from the stall's start",
        &summary_ms(&marking),
        "between 2 and 6.5 s (h7_alttab_to_early, ghost)",
        relation(
            &marking
                .iter()
                .map(|v| (2_000.0..=6_500.0).contains(v))
                .collect::<Vec<_>>(),
        ),
    );
    let one = band(&[CTO_MS]);
    report.claim(
        "T marked hung: B's name change held from a new WinEvent",
        &summary_ms(&after_mark),
        &format!(
            "1,803 to 1,981 ms after B's raise (h7_hung); {}",
            band_text(one)
        ),
        in_band(&after_mark, one),
    );
    report.claim(
        &format!("Alt+Tab away from the hung T: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&away),
        "1 to 2 ms (h7_alttab)",
        relation(&away.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
}

fn alt_tab_back(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 12_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("alt-tab-back: Alt+Tab back to T", &reason);
        return;
    }
    let mut early = Vec::new();
    let mut late = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let t = scene.p1.hwnd(0);
        scene.p1.request("foreground 0");
        scene.p1.request("foreground 1");
        let stalled = scene.stall(0, STALL_MS);
        until(stalled + 2_000_000, "2 s into the stall");
        apps::alt_tab();
        let switched = now_us();
        until(switched + B_AFTER_US, "B's name change after Alt+Tab");
        let renamed = scene.rename_b("early");
        early.push(delay(
            renamed,
            heard_at(&d.lines_until(renamed + 1_000_000), "name", "early"),
        ));
        let mark = marked_hung(t, stalled + 9_000_000);
        note::evidence(&format!(
            "marked hung {:.1} ms into the stall",
            delay(stalled, mark)
        ));
        scene.p1.request("foreground 1");
        apps::alt_tab();
        let switched = now_us();
        until(switched + B_AFTER_US, "B's name change after Alt+Tab");
        let renamed = scene.rename_b("late");
        late.push(delay(
            renamed,
            heard_at(&d.lines_until(renamed + 1_000_000), "name", "late"),
        ));
        until(stalled + us(as_ms(STALL_MS)), "the stall's end");
        scene.p1.request("hide 0");
        scene.p1.request("hide 1");
    }
    report.claim(
        &format!("Alt+Tab back to T, 2 s into its stall, not yet marked hung: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&early),
        "1 ms (h7_alttab_to_early)",
        relation(&early.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
    report.claim(
        &format!("Alt+Tab back to T marked hung: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&late),
        "1 to 2 ms (h7_alttab_to)",
        relation(&late.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
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
        let mut scene = Scene::new();
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
    let _ = Verdict::Trend;
}

/// E-hung-proxy's desktop-wide name registration and q1's name-change
/// `WinEvent`, with T and B shown, since a hidden window's events reach no
/// registration on the desktop's subtree. E-hung-proxy ran at a 10 s
/// connection timeout with a 15 s stall and watched 25 s; this case runs
/// at 2 s with a 3 s stall and watches 5 s after the stall's end.
fn desk_wide(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 3_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("desk-wide: the desktop-wide name registration", &reason);
        return;
    }
    let mut control = Vec::new();
    let mut held = Vec::new();
    let mut prop = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        scene.p1.request("show 0");
        scene.p2.request("show 0");
        let d = Listener::start("D", &spec(&["desk", "deskname"]));
        let p = Listener::start(
            "a desktop-wide name registration and B's",
            &["deskname".to_owned(), scene.b_spec()],
        );
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let first = scene.rename_b("control");
        control.push(delay(
            first,
            heard_at(&d.lines_until(first + 1_000_000), "name", "control"),
        ));
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        until(stalled + 1_000_000, "the name-change WinEvent naming T");
        let name_event = scene.winevent("name", 0);
        until(name_event + B_AFTER_US, "B's name change after it");
        let second = scene.rename_b("named");
        let end = stalled + us(as_ms(STALL_MS) + 5_000.0);
        held.push(heard_at(&d.lines_until(end), "name", "held").is_some());
        prop.push(delay(
            second,
            heard_at(&p.lines_until(end), "name", "named"),
        ));
        scene.p1.request("hide 0");
        scene.p2.request("hide 0");
    }
    report.claim(
        &format!("the desktop-wide name registration hears B's name change, raised with no stall, within {NOT_HELD_MS} ms"),
        &summary_ms(&control),
        "a control of this case",
        relation(&control.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
    report.claim(
        "the desktop-wide name registration, in a client with a desktop focus handler, hears B's name change raised during the hold within 5 s of the stall's end",
        &runs(&held),
        "not delivered within 25 s, at 10 s in a 15 s stall (E-hung-proxy)",
        deterministic(&held, &false),
    );
    report.claim(
        &format!("a client with a desktop-wide name registration and no focus registration, after a name-change WinEvent naming the stalled T: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&prop),
        "2 to 3 ms (q1 stall_tree)",
        relation(&prop.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
}
