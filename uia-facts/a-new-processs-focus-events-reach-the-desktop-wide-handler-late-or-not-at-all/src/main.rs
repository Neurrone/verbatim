//! A new process's focus events reach the desktop-wide handler late or not
//! at all
//! (`uia-facts/a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md`).
//!
//! On the unlocked desktop, this process holds one desktop-wide focus
//! handler, caching the sender's process id, and an out-of-context hook
//! for focus `WinEvent`s from every process, as Verbatim's listener holds
//! both. It starts the provider program, a tree it controls, 11 times,
//! each time bringing its window to the foreground and moving its focus to
//! a node. For each start it takes the first focus `WinEvent` from the new
//! process and waits up to 5 s for a UIA focus from it. The UIA focus is
//! late when it comes more than 1 s after the `WinEvent`, the second
//! Verbatim's outpost waits before reading the focus itself, and missing
//! when none comes within 5 s.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::events::{self, Handler, OnEvent};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict};
use uia_facts_support::stats::us_to_ms;
use uia_facts_support::winevents::{Hook, WinEvent};
use uia_facts_support::{apps, note, now_us, precondition, uia};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_NamePropertyId, UIA_ProcessIdPropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS;

/// Starts of the provider program. The end-to-end runs that met the fault
/// recorded no rate; 11 starts would show one met in 1 start of 4, an
/// assumed rate, with probability 0.95 ((3/4) to the 11th power is below
/// 0.05).
const STARTS: usize = 11;
/// A UIA focus later than this after the focus `WinEvent` is late.
const LATE_MS: f64 = 1_000.0;
/// How long a start's own focus `WinEvent` may take: the program brings
/// the window to the front itself, so it comes at once.
const WINEVENT_LIMIT: Duration = Duration::from_secs(2);

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start(
        "a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all",
    );
    if let Err(reason) = precondition::unlocked_desktop() {
        report.unmet("the late or missing focus of new processes", &reason);
        report.finish();
    }
    let focus: Arc<Mutex<Vec<(u32, u64)>>> = Arc::new(Mutex::new(Vec::new()));
    let client = uia::client();
    let keep = focus.clone();
    let on_event: OnEvent = Box::new(move |sender: &IUIAutomationElement| {
        // SAFETY: a live element UIA passed to the callback.
        if let Ok(pid) = unsafe { sender.CachedProcessId() } {
            keep.lock()
                .expect("a lock no one poisons")
                .push((pid.cast_unsigned(), now_us()));
        }
    });
    let (handler, _reports) = Handler::calling(Some(on_event));
    let cache = uia::cache_request(&client, &[UIA_ProcessIdPropertyId, UIA_NamePropertyId])
        .expect("a cache request");
    // SAFETY: live client, cache request and handler.
    unsafe { client.AddFocusChangedEventHandler(&cache, &events::as_focus_handler(&handler)) }
        .expect("the desktop focus handler");
    let (_hook, winevents) = Hook::start(EVENT_OBJECT_FOCUS, EVENT_OBJECT_FOCUS, 0);
    if args.wants("provider") {
        report.case(
            "provider",
            "starting the provider program 11 times, bringing its window to the front and moving its focus",
        );
        provider(&focus, &winevents, &mut report);
    }
    report.finish();
}

fn provider(
    focus: &Arc<Mutex<Vec<(u32, u64)>>>,
    winevents: &std::sync::mpsc::Receiver<WinEvent>,
    report: &mut Report,
) {
    let mut late = 0;
    let mut missing = 0;
    let mut delays = Vec::new();
    for start in 0..STARTS {
        while winevents.try_recv().is_ok() {}
        focus.lock().expect("a lock no one poisons").clear();
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        let front = provider.request("foreground 0");
        provider.request("focus 0 5");
        provider.request("raise focus 0 5");
        // A start with no focus WinEvent from the new process has nothing
        // to measure from: the case stops there.
        let Some(at) = first_winevent(winevents, provider.pid, WINEVENT_LIMIT) else {
            report.unmet(
                "provider: the late or missing focus",
                &format!(
                    "start {} had no focus WinEvent from the new process within 2 s (in front: {front}; window {} in front)",
                    start + 1,
                    apps::foreground_window()
                ),
            );
            return;
        };
        let deadline = at + 5_000_000;
        let heard = loop {
            let heard = focus
                .lock()
                .expect("a lock no one poisons")
                .iter()
                .find(|(p, t)| *p == provider.pid && *t >= at)
                .map(|(_, t)| *t);
            if heard.is_some() || now_us() >= deadline {
                break heard.map(|h| us_to_ms(h - at));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        note::evidence(&format!(
            "start {}: UIA focus {heard:?} ms after the WinEvent",
            start + 1
        ));
        match heard {
            Some(ms) => {
                delays.push(ms);
                if ms > LATE_MS {
                    late += 1;
                }
            }
            None => missing += 1,
        }
        drop(provider);
        uia::pause(Duration::from_millis(500), "before the next start");
    }
    report.claim(
        "provider: starts whose UIA focus came more than 1 s after its focus WinEvent, or not within 5 s",
        &format!("{late} late and {missing} missing of {STARTS} (UIA focus {delays:.0?} ms after the WinEvent)"),
        "",
        Verdict::New,
    );
}

/// The first focus `WinEvent` within `limit` from a window of process
/// `pid`: its time.
fn first_winevent(
    winevents: &std::sync::mpsc::Receiver<WinEvent>,
    pid: u32,
    limit: Duration,
) -> Option<u64> {
    let end = now_us() + u64::try_from(limit.as_micros()).expect("a short limit");
    loop {
        let left = end.saturating_sub(now_us());
        let event = winevents
            .recv_timeout(Duration::from_micros(left.max(1)))
            .ok()?;
        if apps::window_pid(event.hwnd) == pid {
            return Some(event.at);
        }
    }
}
