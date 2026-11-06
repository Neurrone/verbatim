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
/// connection timeout, 250 ms or more here.
const NOT_HELD_MS: f64 = 100.0;
/// The desktop of its own the cases that ran on one run on.
const DESKTOP: &str = "uia-facts";
/// The connection timeout every client has unless its spec says otherwise,
/// UIA's default.
const CTO_MS: f64 = 2_000.0;
/// The connection timeout the cases set where their claim is the hold's
/// relation to the timeout rather than its length at the default, so that
/// their stalls stay short.
const LOW_MS: f64 = 250.0;

/// The windows of one trial.
struct Scene {
    p1: Provider,
    p2: Provider,
}

impl Scene {
    fn new() -> Self {
        Self::with(&["--windows", "plain:0,uia:1,msaa:2"])
    }

    /// The scene on the program's desktop of its own, as the originals
    /// ran on one: no input or real focus there reaches its clients.
    fn with(p1: &[&str]) -> Self {
        Self::on(Some(DESKTOP), p1)
    }

    /// The scene on the user's desktop, for the cases that need it.
    fn screen() -> Self {
        Self::on(None, &["--windows", "plain:0,uia:1,msaa:2"])
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

    /// Makes P1's window `w` UI thread busy until [`Scene::release`];
    /// answers when it started.
    fn hold(&mut self, w: usize) -> u64 {
        self.p1
            .request(&format!("busy {w} 0"))
            .parse()
            .expect("the time the stall started")
    }

    /// Ends the stall [`Scene::hold`] began.
    fn release(&mut self) {
        self.p1.request("release");
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

/// The lines `client` sends until it has heard a name change of a sender
/// named `name` or until `limit` on [`now_us`]'s clock.
fn until_heard(client: &Listener, name: &str, limit: u64) -> Vec<Line> {
    let mut lines = Vec::new();
    loop {
        lines.extend(client.lines_until((now_us() + 20_000).min(limit)));
        if heard_at(&lines, "name", name).is_some() || now_us() >= limit {
            return lines;
        }
    }
}

/// How far past a band's top a held stall lasts when the client has not
/// heard B's change, so that a hold to T's answer shows as out of band.
const PAST_TOP_US: u64 = 300_000;

/// A listening client on the program's desktop of its own.
fn listener(label: &str, spec: &[String]) -> Listener {
    Listener::start_on(Some(DESKTOP), label, spec)
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
            "a client with a desktop focus handler and B's name changes; T stalled 5 s at UIA's default 2 s connection timeout; a focus WinEvent naming T; B renamed 200 ms later; counting the sent messages that reach T",
            calls,
        ),
        (
            "who",
            "fourteen clients that differ in one thing each, from timeouts and apartments to client objects, mappings and registrations, at connection timeouts of 250 and 500 ms, each in a trial of its own with T stalled past its hold; a focus WinEvent naming T; B renamed 100 ms later; then an outpost and a listener stand-in, each in a trial of its own, B's text changed with its name; then a name and a foreground WinEvent naming M, and a name read of B from each of two clients",
            who,
        ),
        (
            "until-answer",
            "a client at a 1 s connection timeout; T stalled 1.5 s, shorter than two waits; a focus WinEvent naming T; B renamed 200 ms later",
            until_answer,
        ),
        (
            "repeat",
            "a client at a 250 ms connection timeout; T stalled 2.2 s; three focus WinEvents naming T at once; B renamed 200 ms later",
            repeat,
        ),
        (
            "quiet",
            "a client with a desktop focus handler; T stalled 1.2 s; B renamed with no WinEvent; then a name-change WinEvent naming T and B renamed again",
            quiet,
        ),
        (
            "mapping",
            "reading which entries of a new client object's proxy factory mapping map the focus WinEvent to UIA's focus event",
            mapping,
        ),
        (
            "slow",
            "a UIA window U whose every provider call takes 300 ms while it keeps answering messages; a focus WinEvent naming U at a 500 ms connection timeout; B renamed 200 ms later, its hold measured from its raise; a client with a focus registration and one without",
            slow,
        ),
        (
            "own-focus",
            "a window whose every provider call takes 1.5 s raising its own focus at a 1 s connection timeout, 4 times as a UIA provider and 6 times as an MSAA window; B renamed 200 ms later",
            own_focus,
        ),
        (
            "dropped",
            "on a desktop of its own, M moving its focus while it answers, then again at the start of a 1.5 s stall of its UI thread; a client with a desktop focus handler at a connection timeout of 10 s, then 1 s, then 0.25 s, each in a trial of its own",
            dropped,
        ),
        (
            "shown",
            "on the unlocked desktop, three trials, T in front and stalled in each: a focus WinEvent naming T at once, asking each 0.1 s whether T is hung; a focus WinEvent 6.5 s in, once marked; and B then T brought to the front, and Alt+Tab away from T 6.5 s in, to B; B renamed once in each",
            shown,
        ),
        (
            "alt-tab-back",
            "on the unlocked desktop, T and then B, in another process, brought to the front; T stalled 12 s; Alt+Tab back to T 2 s in, before Windows marks it hung, and again once it is marked",
            alt_tab_back,
        ),
        (
            "open-show",
            "on the unlocked desktop, Notepad in front; T stalled 9 s; a focus WinEvent naming T; B renamed 200 ms later; Alt+F 0.6 s after that",
            open_show,
        ),
        (
            "desk-wide",
            "on the unlocked desktop, T and B shown; a client with a desktop focus handler and a desktop-wide name registration, and one with the name registration alone; B renamed as a control; T stalled 15 s at a 10 s connection timeout, as E-hung-proxy ran; a focus WinEvent naming T, B renamed, then a name-change WinEvent naming T and B renamed again",
            desk_wide,
        ),
    ];
    // The cases that take over the desktop run only when named.
    let on_screen = ["shown", "alt-tab-back", "open-show", "desk-wide"];
    for (name, what, run) in cases {
        let runs = if on_screen.contains(&name) {
            args.names(name)
        } else {
            args.wants(name)
        };
        if runs {
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
        let d = listener("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[
            ("P1 (T)", scene.p1.pid),
            ("P2 (B)", scene.p2.pid),
            ("client D", d.pid()),
        ]);
        let _ = scene.p1.log();
        // T held until D hears B's change, or past the band's top, so a
        // hold to T's answer falls outside it.
        scene.hold(0);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        note::waiting("up to 5.0 s, B's name change, T held until then");
        let lines = until_heard(
            &d,
            "held",
            raised + us(band(&[CTO_MS, CTO_MS]).1) + PAST_TOP_US,
        );
        scene.release();
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

/// The clients of `who`: name, spec less B's name registration, the
/// waits it is held for from the `WinEvent`, and the original figure. Each
/// runs at a connection timeout of 250 or 500 ms where the original ran at
/// 500 to 2,000 ms: what is judged is the hold's relation to the timeout.
const WHO: [(&str, &[&str], &[f64], &str); 14] = [
    (
        "connection timeout 250 ms",
        &["desk", "cto=250"],
        &[LOW_MS, LOW_MS],
        "812 ms at 500 ms (h3 cto500)",
    ),
    (
        "connection timeout 500 ms",
        &["desk", "cto=500"],
        &[500.0, 500.0],
        "1,824 ms at 1,000 ms (repeat1_cto1000)",
    ),
    (
        "transaction timeout 100 ms",
        &["desk", "cto=250", "tto=100"],
        &[LOW_MS, LOW_MS],
        "3,810 ms at 2,000 and 500 ms (h3_tto500)",
    ),
    (
        "a single-threaded apartment",
        &["sta", "desk", "cto=250"],
        &[LOW_MS, LOW_MS],
        "3,809 ms at 2,000 ms (h3 sta)",
    ),
    (
        "a focus group on B in place of the desktop handler",
        &["group=B", "cto=250"],
        &[LOW_MS, LOW_MS],
        "3,809 ms at 2,000 ms (grouponly; P4's O_B)",
    ),
    (
        "a focus group on A, another application's window",
        &["group=A", "cto=250"],
        &[LOW_MS, LOW_MS],
        "4,054 and 4,070 ms from the stall at 2,000 ms (P4's O_A)",
    ),
    (
        "focus on a second client object on the same thread",
        &["deskobj", "cto=250"],
        &[LOW_MS, LOW_MS],
        "815 ms at 500 ms (focusnew500_namemain)",
    ),
    (
        "focus on a second client object on a thread of its own",
        &["deskthr", "cto=250"],
        &[LOW_MS, LOW_MS],
        "817 ms at 500 ms (focusthr500_namemain)",
    ),
    (
        "the name registration on a client object on a thread of its own",
        &["desk", "namethr", "cto=250"],
        &[LOW_MS, LOW_MS],
        "819 ms at 500 ms (focusmain_namethr500)",
    ),
    (
        "two client objects, each with a desktop handler",
        &["desk", "deskobj", "cto=250"],
        &[LOW_MS; 4],
        "1,820 ms at 500 ms",
    ),
    (
        "two desktop handlers on one client object",
        &["desk", "desk2", "cto=250"],
        &[LOW_MS, LOW_MS],
        "807 ms at 500 ms (twodesk_500)",
    ),
    (
        "the proxy factory mapping emptied",
        &["desk", "clearmap", "cto=250"],
        &[LOW_MS],
        "1,812 ms at 2,000 ms (clearmap)",
    ),
    (
        "the focus WinEvents removed from the mapping",
        &["desk", "nofocusmap", "cto=250"],
        &[LOW_MS, LOW_MS],
        "14,789 against 14,801 ms at 10 s (e_mapping)",
    ),
    (
        "Verbatim's mapping, against a hidden T",
        &["desk", "vbmap", "cto=250", "tto=5000"],
        &[LOW_MS, LOW_MS],
        "3,806 to 3,821 ms at 2,000 ms (states' stand-in)",
    ),
];

/// B's events follow the focus `WinEvent` by this much in `who`, inside
/// its shortest hold, one wait of 250 ms.
const WHO_B_AFTER_US: u64 = 100_000;

/// One `who` trial of client `words`: T stalled past the hold's band, the
/// focus `WinEvent`, and B's name change 100 ms later; answers the hold
/// from the `WinEvent`.
fn who_hold(args: &Args, label: &str, words: &[&str], waits: &[f64]) -> f64 {
    let mut scene = Scene::new();
    let (window_a, window_b) = (scene.p1.hwnd(1), scene.b());
    let mut words: Vec<String> = words
        .iter()
        .map(|w| match *w {
            "group=B" => format!("group={window_b}"),
            "group=A" => format!("group={window_a}"),
            "namethr" => format!("namethr={window_b}"),
            other => (*other).to_owned(),
        })
        .collect();
    if !words.iter().any(|w| w.starts_with("namethr")) {
        words.push(scene.b_spec());
    }
    let client = listener(label, &words);
    args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), (label, client.pid())]);
    scene.hold(0);
    let raised = scene.winevent("focus", 0);
    until(
        raised + WHO_B_AFTER_US,
        "B's name change after the WinEvent",
    );
    scene.rename_b("held");
    let lines = until_heard(&client, "held", raised + us(band(waits).1) + PAST_TOP_US);
    scene.release();
    let hold = delay(raised, heard_at(&lines, "name", "held"));
    note::evidence(&format!(
        "{label}: B's name change {hold:.1} ms after the WinEvent"
    ));
    hold
}

/// One `who` trial of a stand-in: T stalled 0.9 s past two 250 ms waits,
/// the focus `WinEvent`, B's name and text changes 100 ms later, a name and
/// a foreground `WinEvent` naming M 50 ms after those, and a name read of
/// B; answers the stand-in's lines and the times of those.
fn stand_in(args: &Args, label: &str, outpost: bool) -> (Vec<Line>, [u64; 6], isize, isize) {
    let mut scene = Scene::new();
    let (window_b, window_m, window_t) = (scene.b(), scene.p1.hwnd(2), scene.p1.hwnd(0));
    let words = if outpost {
        vec![
            scene.b_spec(),
            format!("text={window_b}"),
            format!("hook={}", scene.p1.pid),
        ]
    } else {
        vec![
            "desk".to_owned(),
            "cto=250".to_owned(),
            scene.b_spec(),
            "hook=0".to_owned(),
        ]
    };
    let mut client = listener(label, &words);
    args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), (label, client.pid())]);
    let stalled = scene.stall(0, 900);
    let raised = scene.winevent("focus", 0);
    until(raised + WHO_B_AFTER_US, "B's events after the WinEvent");
    let renamed = scene.rename_b("held");
    let texted = scene.text_b();
    until(raised + 150_000, "the WinEvents naming M");
    let m_name = scene.winevent("name", 2);
    let m_foreground = scene.winevent("foreground", 2);
    let read_at = now_us();
    client.send(&format!("read {window_b}"));
    let lines = client.lines_until(stalled + 1_200_000);
    (
        lines,
        [raised, renamed, texted, m_name, m_foreground, read_at],
        window_m,
        window_t,
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "a trial a client, then the stand-ins' claims in turn"
)]
fn who(args: &Args, report: &mut Report) {
    for (label, words, waits, recorded) in &WHO {
        let holds: Vec<f64> = (0..trials())
            .map(|_| who_hold(args, label, words, waits))
            .collect();
        let range = band(waits);
        report.claim(
            &format!("{label}: B's name change held from the WinEvent"),
            &summary_ms(&holds),
            &format!("{recorded}; {}", band_text(range)),
            in_band(&holds, range),
        );
    }
    let older_cast: Vec<String> = (0..trials())
        .map(|_| {
            let older: Result<windows::Win32::UI::Accessibility::IUIAutomation2, _> =
                windows::core::Interface::cast(&uia::older_client());
            older.map_or_else(|e| uia::hresult(e.code()), |_| "ok".to_owned())
        })
        .collect();
    report.claim(
        "the older CUIAutomation coclass's client object has no IUIAutomation2, so its timeouts cannot be set",
        &runs(&older_cast),
        "E_NOINTERFACE (h3 cui_cto500)",
        deterministic(&older_cast, &"0x80004002".to_owned()),
    );
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
    let mut outpost: Vec<[f64; 4]> = Vec::new();
    let mut listener_stand_in: Vec<[f64; 4]> = Vec::new();
    for _ in 0..trials() {
        let (lines, [_, renamed, texted, m_name, _, read_at], window_m, _) =
            stand_in(args, "outpost stand-in", true);
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
        let (lines, [raised, _, _, _, m_foreground, read_at], window_m, window_t) =
            stand_in(args, "listener stand-in", false);
        let l = [
            delay(raised, heard_at(&lines, "name", "held")),
            delay(raised, hooked(&lines, EVENT_OBJECT_FOCUS, window_t)),
            delay(
                m_foreground,
                hooked(&lines, EVENT_SYSTEM_FOREGROUND, window_m),
            ),
            delay(read_at, read(&lines)),
        ];
        note::evidence(&format!(
            "listener stand-in: name from the WinEvent, focus hook, foreground hook, read {l:?} ms"
        ));
        listener_stand_in.push(l);
    }
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
    let first: Vec<f64> = listener_stand_in.iter().map(|l| l[0]).collect();
    report.claim(
        "the listener stand-in, with a desktop focus handler at 250 ms: B's name change held at least one connection timeout from the WinEvent",
        &summary_ms(&first),
        "9,697 to 9,699 ms at 10 s (e_cross)",
        relation(&first.iter().map(|v| *v >= LOW_MS - TICK_MS).collect::<Vec<_>>()),
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
        let values: Vec<f64> = listener_stand_in.iter().map(|l| l[i]).collect();
        report.claim(
            &format!("the listener stand-in, while held: {what} within {NOT_HELD_MS} ms"),
            &summary_ms(&values),
            recorded,
            relation(&[not_held(&values)]),
        );
    }
}

fn until_answer(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 1_500;
    const CTO: f64 = 1_000.0;
    let mut answered = Vec::new();
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = listener(
            "D",
            &["desk".to_owned(), "cto=1000".to_owned(), scene.b_spec()],
        );
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let stalled = scene.stall(0, STALL_MS);
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        scene.rename_b("held");
        let end = stalled + us(as_ms(STALL_MS) + 500.0);
        let heard = heard_at(&d.lines_until(end), "name", "held");
        let hold = delay(raised, heard);
        let stall_end = stalled + us(as_ms(STALL_MS));
        note::evidence(&format!(
            "B's name change {hold:.1} ms after the WinEvent; the stall ended {:.1} ms after it",
            us_to_ms(stall_end - raised)
        ));
        holds.push(hold);
        answered.push(heard.is_some_and(|h| h >= stall_end && hold < band(&[CTO, CTO]).0));
    }
    report.claim(
        "B's name change, at a 1,000 ms connection timeout with T stalled 1.5 s: held until T answers, after the stall's end and before two waits end",
        &format!("{} ({})", runs(&answered), summary_ms(&holds)),
        "7,773 ms, the end of an 8 s stall, at 10 s (h3 to10000_1); 14,800 ms in a 15 s stall (E-hung-proxy)",
        relation(&answered),
    );
}

fn repeat(args: &Args, report: &mut Report) {
    let mut holds = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = listener(
            "D",
            &["desk".to_owned(), "cto=250".to_owned(), scene.b_spec()],
        );
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        scene.hold(0);
        let raised = scene.winevent("focus", 0);
        scene.winevent("focus", 0);
        scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvents");
        scene.rename_b("held");
        let lines = until_heard(&d, "held", raised + us(band(&[LOW_MS; 6]).1) + PAST_TOP_US);
        scene.release();
        holds.push(delay(raised, heard_at(&lines, "name", "held")));
    }
    let six = band(&[LOW_MS; 6]);
    report.claim(
        "B's name change after three focus WinEvents naming T at a 250 ms connection timeout: each pays both waits again",
        &summary_ms(&holds),
        &format!("5,814 ms after B's raise at 1,000 ms (repeat3_cto1000); {}", band_text(six)),
        in_band(&holds, six),
    );
}

fn quiet(args: &Args, report: &mut Report) {
    let mut none = Vec::new();
    let mut name_d = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::new();
        let d = listener("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let stalled = scene.hold(0);
        until(stalled + B_AFTER_US, "B's name change into the stall");
        let first = scene.rename_b("first");
        until(stalled + 500_000, "the name-change WinEvent naming T");
        let name_event = scene.winevent("name", 0);
        until(
            name_event + B_AFTER_US,
            "B's name change after the WinEvent",
        );
        let second = scene.rename_b("second");
        let mut lines = until_heard(&d, "first", first + us(NOT_HELD_MS) + PAST_TOP_US);
        lines.extend(until_heard(
            &d,
            "second",
            second + us(NOT_HELD_MS) + PAST_TOP_US,
        ));
        scene.release();
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
        let d = listener(
            "D",
            &["desk".to_owned(), "cto=500".to_owned(), scene.b_spec()],
        );
        let d0 = listener("D0", &["cto=500".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("U's process", scene.p1.pid), ("client D", d.pid())]);
        scene.p1.request("slow 300");
        let raised = scene.winevent("focus", 0);
        until(raised + B_AFTER_US, "B's name change after the WinEvent");
        let renamed = scene.rename_b("held");
        let heard = d.next(
            Duration::from_secs(3),
            |l| matches!(l, Line::Heard { kind, name, .. } if kind == "name" && name == "held"),
        );
        let at = match heard {
            Some(Line::Heard { at, .. }) => Some(at),
            _ => None,
        };
        held.push(delay(renamed, at));
        free.push(delay(
            renamed,
            heard_at(&d0.lines_until(now_us()), "name", "held"),
        ));
        scene.p1.request("slow 0");
    }
    report.claim(
        "a focus WinEvent naming U, whose provider takes 300 ms a call: B's name change held at least one connection timeout, 500 ms, from its raise",
        &summary_ms(&held),
        "3,800 to 3,968 ms after B's raise at 1,000 ms a call and a 2,000 ms connection timeout (q3)",
        relation(&held.iter().map(|v| *v >= 500.0 - TICK_MS && v.is_finite()).collect::<Vec<_>>()),
    );
    report.claim(
        &format!("the same in a client with no focus registration: within {NOT_HELD_MS} ms"),
        &summary_ms(&free),
        "1 to 3 ms (q3, D0)",
        relation(&free.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
}

/// `own-focus`'s provider calls take this long: 1.5 times the connection
/// timeout, as q3's 3,000 ms calls were at its 2,000 ms timeout.
const OWN_SLOW_MS: u64 = 1_500;
/// `own-focus`'s connection timeout.
const OWN_CTO_MS: f64 = 1_000.0;

fn own_focus(args: &Args, report: &mut Report) {
    for (window, label, n, recorded_k) in [
        (0usize, "a UIA provider", 4usize, 1usize),
        (1, "an MSAA window", 6, 4),
    ] {
        let mut k = 0;
        let mut delays = Vec::new();
        for _ in 0..n {
            let scene = Scene::with(&["--windows", "uia:0,msaa:1"]);
            let d = listener(
                "D",
                &["desk".to_owned(), "cto=1000".to_owned(), scene.b_spec()],
            );
            args.pause_for_debugger(&[("the slow process", scene.p1.pid), ("client D", d.pid())]);
            let Scene {
                p1: mut slow,
                p2: mut b,
            } = scene;
            slow.request(&format!("slow {OWN_SLOW_MS}"));
            let raised = now_us();
            let request = if window == 0 {
                "raise focus 0 5".to_owned()
            } else {
                "raise focus 1 0".to_owned()
            };
            // The raise's own calls into the slow provider take as long as
            // they take: the trial does not wait for them. The slow provider
            // lives until both the raise has returned and the trial is over,
            // when the thread that raised ends it.
            let (over, is_over) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                slow.request(&request);
                let _ = is_over.recv();
            });
            until(raised + B_AFTER_US, "B's name change after the raise");
            b.request("rename 0 1 held");
            let renamed: u64 = b.request("raise name 0 1").parse().expect("a time");
            let heard = d.next(
                Duration::from_secs(2),
                |l| matches!(l, Line::Heard { kind, name, .. } if kind == "name" && name == "held"),
            );
            let at = match heard {
                Some(Line::Heard { at, .. }) => Some(at),
                _ => None,
            };
            let late = delay(renamed, at);
            let _ = over.send(());
            delays.push(late);
            if late > OWN_CTO_MS / 2.0 {
                k += 1;
            }
        }
        note::evidence(&format!("{label}: B's name change after {delays:?} ms"));
        report.claim(
            &format!("{label} raising its own focus while each provider call takes 1.5 s, at a 1,000 ms connection timeout: trials whose B name change is held more than 500 ms"),
            &format!("{k} of {n} ({})", summary_ms(&delays)),
            &format!("{recorded_k} of {n}, held more than 1 s at 3,000 ms a call and a 2,000 ms timeout (q3 slow_{}_focus)", if window == 0 { "uia" } else { "msaa" }),
            rate(k, n, recorded_k, n),
        );
    }
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

/// Waits up to 1 s for `hwnd` to be the foreground window, asking each
/// 10 ms; answers whether it was.
fn wait_front(hwnd: isize) -> bool {
    let limit = now_us() + 1_000_000;
    loop {
        if apps::foreground_window() == hwnd {
            return true;
        }
        if now_us() >= limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// One `shown` trial: a scene on the user's desktop with clients D and
/// Verbatim's stand-in, and the windows `front` brought to the front in
/// turn, T last; answers `None`, noting why in `unmet`, when T did not
/// come to the front.
fn shown_scene(
    args: &Args,
    b_first: bool,
    unmet: &mut Vec<String>,
) -> Option<(Scene, Listener, Listener)> {
    let mut scene = Scene::screen();
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
    if b_first {
        let b = scene.p2.request("foreground 0");
        note::evidence(&format!("B in front: {b}"));
    }
    let front = scene.p1.request("foreground 0");
    note::evidence(&format!("T in front: {front}"));
    if !wait_front(scene.p1.hwnd(0)) {
        unmet.push(format!("T did not come to the front ({front})"));
        return None;
    }
    // As h7 did: a baseline change of B, heard by both clients, so the
    // focus events of bringing T to the front are behind them.
    let base = scene.rename_b("base");
    for client in [&d, &vb] {
        if heard_at(
            &until_heard(client, "base", base + 5_000_000),
            "name",
            "base",
        )
        .is_none()
        {
            unmet.push(format!(
                "{} did not hear B's baseline change within 5 s",
                client.label
            ));
            return None;
        }
    }
    Some((scene, d, vb))
}

#[expect(
    clippy::too_many_lines,
    reason = "three trials, as h7 ran three, then their claims"
)]
fn shown(args: &Args, report: &mut Report) {
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("shown: the holds, the marking and Alt+Tab", &reason);
        return;
    }
    let mut fresh = Vec::new();
    let mut fresh_vb = Vec::new();
    let mut marking = Vec::new();
    let mut after_mark = Vec::new();
    let mut away = Vec::new();
    let mut unmet = Vec::new();
    let two = band(&[CTO_MS, CTO_MS]);
    let one = band(&[CTO_MS]);
    for _ in 0..trials() {
        // Each measurement in a trial of its own, with one name change of
        // B, as h7 ran h7_fresh, h7_hung and h7_alttab: a sender's name is
        // read when the worker takes its event, so B's changes held
        // together would all carry the latest name.
        // A WinEvent naming T in front, not yet marked hung, and when
        // Windows marks it.
        if let Some((mut scene, d, vb)) = shown_scene(args, false, &mut unmet) {
            let t = scene.p1.hwnd(0);
            let stalled = scene.stall(0, 7_000);
            let raised = scene.winevent("focus", 0);
            until(raised + B_AFTER_US, "B's name change after the WinEvent");
            scene.rename_b("fresh");
            let mark = marked_hung(t, stalled + 6_500_000);
            marking.push(delay(stalled, mark));
            let top = raised + us(two.1);
            fresh.push(delay(
                raised,
                heard_at(&d.lines_until(top), "name", "fresh"),
            ));
            fresh_vb.push(delay(
                raised,
                heard_at(&vb.lines_until(top), "name", "fresh"),
            ));
            until(stalled + 7_000_000, "the stall's end");
            scene.p1.request("hide 0");
        }
        // A WinEvent naming T once marked hung.
        if let Some((mut scene, d, _vb)) = shown_scene(args, false, &mut unmet) {
            let t = scene.p1.hwnd(0);
            let stalled = scene.stall(0, 10_000);
            let mark = marked_hung(t, stalled + 6_500_000);
            note::evidence(&format!(
                "marked hung {:.1} ms into the stall",
                delay(stalled, mark)
            ));
            until(stalled + 6_500_000, "6.5 s into the stall, as h7_hung");
            let again = scene.winevent("focus", 0);
            until(again + B_AFTER_US, "B's name change after the WinEvent");
            scene.rename_b("marked");
            // Until heard, or past the top of two waits' band, so a hold of
            // two waits shows as out of band.
            let lines = until_heard(&d, "marked", again + us(two.1) + PAST_TOP_US);
            after_mark.push(if mark.is_some() {
                delay(again, heard_at(&lines, "name", "marked"))
            } else {
                unmet.push("T was not marked hung within 6.5 s".to_owned());
                f64::NAN
            });
            until(stalled + 10_000_000, "the stall's end");
            scene.p1.request("hide 0");
        }
        // Alt+Tab away from T marked hung, with B in front before it, as
        // h7_alttab put Notepad.
        if let Some((mut scene, d, _vb)) = shown_scene(args, true, &mut unmet) {
            let t = scene.p1.hwnd(0);
            let stalled = scene.stall(0, 8_000);
            let mark = marked_hung(t, stalled + 6_500_000);
            until(stalled + 6_500_000, "6.5 s into the stall, as h7_alttab");
            apps::alt_tab();
            let switched = now_us();
            if mark.is_none() {
                unmet.push("T was not marked hung within 6.5 s".to_owned());
            }
            until(switched + B_AFTER_US, "B's name change after Alt+Tab");
            // Alt+Tab from a hung window lands on B, on T or on T's ghost,
            // as h7_alttab's rerun at 40aba4e4 landed on each: any of
            // those; another application's window would put its own
            // focus tracking on D's worker.
            let front = apps::foreground_window();
            let class = apps::class_of(front);
            note::evidence(&format!(
                "after Alt+Tab, window {front} of class {class} in front"
            ));
            if front != scene.b() && front != t && class != "Ghost" {
                unmet.push(format!(
                    "Alt+Tab away from T brought window {front} of class {class} to the front, not B, T or T's ghost"
                ));
            }
            let renamed = scene.rename_b("away");
            away.push(delay(
                renamed,
                heard_at(&d.lines_until(renamed + 1_000_000), "name", "away"),
            ));
            until(stalled + 8_000_000, "the stall's end");
            scene.p1.request("hide 0");
            scene.p2.request("hide 0");
        }
    }
    if !unmet.is_empty() {
        report.unmet(
            "shown: the holds, the marking and Alt+Tab",
            &unmet.join("; "),
        );
        return;
    }
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
    let mut missed = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::screen();
        let d = Listener::start("D", &["desk".to_owned(), scene.b_spec()]);
        args.pause_for_debugger(&[("P1 (T)", scene.p1.pid), ("client D", d.pid())]);
        let t = scene.p1.hwnd(0);
        // T, then B in another process, in front, as h7 put its window and
        // then Notepad in front: Alt+Tab goes back to T.
        let order = [
            scene.p1.request("foreground 0"),
            scene.p2.request("foreground 0"),
        ];
        note::evidence(&format!("T and then B in front: {order:?}"));
        let stalled = scene.stall(0, STALL_MS);
        until(stalled + 2_000_000, "2 s into the stall");
        let switched = alt_tab_to(t, &mut missed, "early");
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
        let again = scene.p2.request("foreground 0");
        note::evidence(&format!("B in front: {again}"));
        let switched = alt_tab_to(t, &mut missed, "late");
        until(switched + B_AFTER_US, "B's name change after Alt+Tab");
        let renamed = scene.rename_b("late");
        late.push(delay(
            renamed,
            heard_at(&d.lines_until(renamed + 1_000_000), "name", "late"),
        ));
        until(stalled + us(as_ms(STALL_MS)), "the stall's end");
        scene.p1.request("hide 0");
        scene.p2.request("hide 0");
    }
    if !missed.is_empty() {
        report.unmet(
            "alt-tab-back: Alt+Tab back to T",
            &format!("Alt+Tab did not bring T to the front: {missed:?}"),
        );
        return;
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

/// Presses Alt+Tab and waits up to 1 s for T in front, noting
/// `which` in `missed` when not; answers when it was pressed.
fn alt_tab_to(t: isize, missed: &mut Vec<String>, which: &str) -> u64 {
    apps::alt_tab();
    let switched = now_us();
    if !wait_front(t) {
        missed.push(format!(
            "{which}: window {} in front, not T ({t})",
            apps::foreground_window()
        ));
    }
    switched
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
    let _ = Verdict::Trend;
}

/// E-hung-proxy's desktop-wide name registration and q1's name-change
/// `WinEvent`, with T and B shown, since a hidden window's events reach no
/// registration on the desktop's subtree. E-hung-proxy ran at a 10 s
/// connection timeout with a 15 s stall and watched 25 s, as this case
/// does.
fn desk_wide(args: &Args, report: &mut Report) {
    const STALL_MS: u64 = 15_000;
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("desk-wide: the desktop-wide name registration", &reason);
        return;
    }
    let mut control = Vec::new();
    let mut held = Vec::new();
    let mut prop = Vec::new();
    for _ in 0..trials() {
        let mut scene = Scene::screen();
        scene.p1.request("show 0");
        scene.p2.request("show 0");
        let d = Listener::start("D", &spec(&["desk", "deskname", "cto=10000"]));
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
        let p_lines = p.lines_until(name_event + 1_000_000);
        note::waiting("25 s from the focus WinEvent, as E-hung-proxy watched");
        held.push(heard_at(&d.lines_until(raised + 25_000_000), "name", "held").is_some());
        prop.push(delay(second, heard_at(&p_lines, "name", "named")));
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
        "the desktop-wide name registration, in a client with a desktop focus handler at a 10 s connection timeout, hears B's name change raised during a 15 s hold within 25 s of the WinEvent",
        &runs(&held),
        "delivered once the hold ended, T and B shown (de138138's run); E-hung-proxy's not within 25 s had hidden windows, whose name changes its registration did not hear even unheld (recorded/e_hung_proxy_rerun.out)",
        deterministic(&held, &true),
    );
    report.claim(
        &format!("a client with a desktop-wide name registration and no focus registration, after a name-change WinEvent naming the stalled T: B's name change within {NOT_HELD_MS} ms"),
        &summary_ms(&prop),
        "2 to 3 ms (q1 stall_tree)",
        relation(&prop.iter().map(|v| *v <= NOT_HELD_MS).collect::<Vec<_>>()),
    );
}
