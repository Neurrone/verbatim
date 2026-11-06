//! The hung-window programs' scene
//! (`uia-facts/a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md`,
//! "Method"): provider program P1 holds T, a plain Win32 window on UI
//! thread 0, the window made to hang; A, a UIA window on thread 1; and M, a
//! window with its own `IAccessible` on thread 2. Provider program P2 holds
//! B, the UIA window whose name changes are listened for. A stall is a UI
//! thread busy, taking no message. The focus `WinEvent` naming T is raised
//! from P1's control thread. Each client is a listening process of its own
//! ([`crate::listen`]), so its UIA has seen nothing before the trial, and
//! each trial has new providers and new clients. A hold is the time from
//! the focus `WinEvent` to a client hearing B's name change, raised 200 ms
//! after the `WinEvent`.

use std::time::Duration;

use crate::args::Args;
use crate::listen::{Line, Listener, heard_at};
use crate::provider::Provider;
use crate::stats::us_to_ms;
use crate::{note, now_us, uia};

/// B's name change follows the focus `WinEvent` by this much, as in every
/// original experiment.
pub const B_AFTER_US: u64 = 200_000;
/// A delivery counts as not held when it comes within this long of its
/// raise: the originals measured 0 to 3 ms, and a hold is at least one
/// connection timeout, 250 ms or more here.
pub const NOT_HELD_MS: f64 = 100.0;
/// The desktop of its own the cases that ran on one run on.
pub const DESKTOP: &str = "uia-facts";
/// The connection timeout every client has unless its spec says otherwise,
/// UIA's default.
pub const CTO_MS: f64 = 2_000.0;
/// The connection timeout the cases set where their claim is the hold's
/// relation to the timeout rather than its length at the default, so that
/// their stalls stay short.
pub const LOW_MS: f64 = 250.0;

/// The windows of one trial.
pub struct Scene {
    /// P1: T, A and M.
    pub p1: Provider,
    /// P2: B.
    pub p2: Provider,
}

impl Default for Scene {
    fn default() -> Self {
        Self::new()
    }
}

impl Scene {
    /// The scene on the program's desktop of its own.
    #[must_use]
    pub fn new() -> Self {
        Self::with(&["--windows", "plain:0,uia:1,msaa:2"])
    }

    /// The scene on the program's desktop of its own, as the originals
    /// ran on one: no input or real focus there reaches its clients.
    #[must_use]
    pub fn with(p1: &[&str]) -> Self {
        Self::on(Some(DESKTOP), p1)
    }

    /// The scene on the user's desktop, for the cases that need it.
    #[must_use]
    pub fn screen() -> Self {
        Self::on(None, &["--windows", "plain:0,uia:1,msaa:2"])
    }

    /// The scene on desktop `desktop`, when given.
    #[must_use]
    pub fn on(desktop: Option<&str>, p1: &[&str]) -> Self {
        Self {
            p1: Provider::launch_on(desktop, p1),
            p2: Provider::launch_on(desktop, &["--windows", "uia:0"]),
        }
    }

    /// B's window.
    #[must_use]
    pub fn b(&self) -> isize {
        self.p2.hwnd(0)
    }

    /// The listener spec of B's name changes.
    #[must_use]
    pub fn b_spec(&self) -> String {
        format!("name={}", self.b())
    }

    /// Makes P1's window `w` UI thread busy for `ms`; answers when it
    /// started.
    ///
    /// # Panics
    ///
    /// When the provider does not answer the request.
    pub fn stall(&mut self, w: usize, ms: u64) -> u64 {
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
    ///
    /// # Panics
    ///
    /// When the provider does not answer the request.
    pub fn hold(&mut self, w: usize) -> u64 {
        self.p1
            .request(&format!("busy {w} 0"))
            .parse()
            .expect("the time the stall started")
    }

    /// Ends the stall [`Scene::hold`] began.
    pub fn release(&mut self) {
        self.p1.request("release");
    }

    /// Raises `WinEvent` `kind` naming P1's window `w`; answers when.
    ///
    /// # Panics
    ///
    /// When the provider does not answer the request.
    pub fn winevent(&mut self, kind: &str, w: usize) -> u64 {
        self.p1
            .request(&format!("winevent {kind} {w}"))
            .parse()
            .expect("the time it was raised")
    }

    /// Renames B's node 1 `name` and raises its name change; answers when.
    ///
    /// # Panics
    ///
    /// When the provider does not answer the request.
    pub fn rename_b(&mut self, name: &str) -> u64 {
        self.p2.request(&format!("rename 0 1 {name}"));
        self.p2
            .request("raise name 0 1")
            .parse()
            .expect("the time it was raised")
    }

    /// Raises a text change on B's node 3; answers when.
    ///
    /// # Panics
    ///
    /// When the provider does not answer the request.
    pub fn text_b(&mut self) -> u64 {
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
/// Milliseconds as a float.
#[must_use]
pub fn as_ms(ms: u64) -> f64 {
    ms as f64
}

/// The lines `client` sends until it has heard a name change of a sender
/// named `name` or until `limit` on [`now_us`]'s clock.
#[must_use]
pub fn until_heard(client: &Listener, name: &str, limit: u64) -> Vec<Line> {
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
pub const PAST_TOP_US: u64 = 300_000;

/// A listening client on the program's desktop of its own.
#[must_use]
pub fn listener(label: &str, spec: &[String]) -> Listener {
    Listener::start_on(Some(DESKTOP), label, spec)
}

/// Waits until `at` on [`now_us`]'s clock: an offset in the scene's
/// timeline, a deliberate wait.
pub fn until(at: u64, why: &str) {
    uia::pause(Duration::from_micros(at.saturating_sub(now_us())), why);
}

/// Milliseconds from `from` to `to`; infinite when `to` never came.
#[must_use]
pub fn delay(from: u64, to: Option<u64>) -> f64 {
    to.map_or(f64::INFINITY, |to| us_to_ms(to.saturating_sub(from)))
}

/// Milliseconds as a duration on [`now_us`]'s clock.
///
/// # Panics
///
/// Never for a fact's durations, which fit in a `u64` of microseconds.
#[must_use]
pub fn us(ms: f64) -> u64 {
    Duration::from_secs_f64(ms / 1_000.0)
        .as_micros()
        .try_into()
        .expect("a short time")
}

/// A listener's spec from its words.
#[must_use]
pub fn spec(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| (*w).to_owned()).collect()
}

/// B's events follow the focus `WinEvent` by this much in `who`, inside
/// its shortest hold, one wait of 250 ms.
pub const WHO_B_AFTER_US: u64 = 100_000;

/// One `who` trial of a stand-in: T stalled 0.9 s past two 250 ms waits,
/// the focus `WinEvent`, B's name and text changes 100 ms later, a name and
/// a foreground `WinEvent` naming M 50 ms after those, and a name read of
/// B; answers the stand-in's lines and the times of those.
#[must_use]
pub fn stand_in(args: &Args, label: &str, outpost: bool) -> (Vec<Line>, [u64; 6], isize, isize) {
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

/// The time of a stand-in's first successful read in `lines`.
#[must_use]
pub fn read_end(lines: &[Line]) -> Option<u64> {
    lines.iter().find_map(|l| match l {
        Line::Read { outcome, end, .. } if outcome == "ok" => Some(*end),
        _ => None,
    })
}

/// The time a stand-in's hook heard `WinEvent` `event` naming `hwnd`.
#[must_use]
pub fn hooked(lines: &[Line], event: u32, hwnd: isize) -> Option<u64> {
    lines.iter().find_map(|l| match l {
        Line::Hook {
            event: e,
            hwnd: h,
            at,
        } if *e == event && *h == hwnd => Some(*at),
        _ => None,
    })
}

/// Whether every delivery came within [`NOT_HELD_MS`] of its raise.
#[must_use]
pub fn not_held(values: &[f64]) -> bool {
    values.iter().all(|v| *v <= NOT_HELD_MS)
}
