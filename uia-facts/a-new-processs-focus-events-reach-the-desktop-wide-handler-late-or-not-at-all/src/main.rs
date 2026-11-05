//! A new process's focus events reach the desktop-wide handler late or not
//! at all
//! (`uia-facts/a-new-processs-focus-events-reach-the-desktop-wide-handler-late-or-not-at-all/readme.md`).
//!
//! On the unlocked desktop, this process holds one desktop-wide focus
//! handler, caching the sender's process id, and an out-of-context hook
//! for focus `WinEvent`s from every process, as Verbatim's listener holds
//! both. It starts a fresh application 20 times each: the provider program
//! brought to the foreground with its focus moved to a node, Notepad, and
//! the Settings app. For each start it takes the first focus `WinEvent`
//! from the new process and waits up to 5 s for a UIA focus from it. The
//! UIA focus is late when it comes more than 1 s after the `WinEvent`, the
//! second Verbatim's outpost waits before reading the focus itself, and
//! missing when none comes within 5 s.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::events::{self, Handler, OnEvent};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, file_version};
use uia_facts_support::stats::us_to_ms;
use uia_facts_support::winevents::{Hook, WinEvent};
use uia_facts_support::{apps, note, now_us, precondition, uia};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_NamePropertyId, UIA_ProcessIdPropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS;

/// Starts of each application.
const STARTS: usize = 20;
/// A UIA focus later than this after the focus `WinEvent` is late.
const LATE_MS: f64 = 1_000.0;

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
    for (name, what) in [
        (
            "provider",
            "starting the provider program 20 times, bringing its window to the front and moving its focus",
        ),
        ("notepad", "starting Notepad 20 times"),
        ("settings", "starting the Settings app 20 times"),
    ] {
        if !args.wants(name) {
            continue;
        }
        report.case(name, what);
        let mut late = 0;
        let mut missing = 0;
        let mut delays = Vec::new();
        for _ in 0..STARTS {
            let before: BTreeSet<u32> = apps::top_windows()
                .iter()
                .map(|w| apps::window_pid(*w))
                .collect();
            while winevents.try_recv().is_ok() {}
            focus.lock().expect("a lock no one poisons").clear();
            let started = start(name);
            let Some(started) = started else {
                report.unmet(
                    &format!("{name}: the late or missing focus"),
                    "the application did not start",
                );
                break;
            };
            let first = first_winevent(&winevents, &before, started.exe, Duration::from_secs(10));
            let outcome = first.map(|(pid, at)| {
                let deadline = at + 5_000_000;
                loop {
                    let heard = focus
                        .lock()
                        .expect("a lock no one poisons")
                        .iter()
                        .find(|(p, t)| *p == pid && *t >= at)
                        .map(|(_, t)| *t);
                    if heard.is_some() || now_us() >= deadline {
                        break (pid, heard.map(|h| us_to_ms(h - at)));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            });
            match outcome {
                Some((_, Some(ms))) => {
                    delays.push(ms);
                    if ms > LATE_MS {
                        late += 1;
                    }
                }
                Some((_, None)) => missing += 1,
                None => note::evidence(&format!(
                    "{name}: no focus WinEvent from the new process within 10 s"
                )),
            }
            note::evidence(&format!("{name}: {outcome:?}"));
            started.end();
            uia::pause(Duration::from_millis(500), "before the next start");
        }
        report.claim(
            &format!("{name}: starts whose UIA focus came more than 1 s after its focus WinEvent, or not within 5 s"),
            &format!("{late} late and {missing} missing of {STARTS} (UIA focus {delays:.0?} ms after the WinEvent)"),
            "",
            Verdict::New,
        );
    }
    report.finish();
}

/// A started application: the executable its focus comes from, and how it
/// is ended.
struct Started {
    exe: &'static str,
    provider: Option<Provider>,
}

impl Started {
    fn end(self) {
        if let Some(provider) = self.provider {
            drop(provider);
            return;
        }
        for hwnd in apps::top_windows() {
            let pid = apps::window_pid(hwnd);
            if apps::image_of(pid)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .is_some_and(|n| n.eq_ignore_ascii_case(self.exe))
            {
                apps::kill(pid);
            }
        }
    }
}

fn start(name: &str) -> Option<Started> {
    match name {
        "provider" => {
            let mut provider = Provider::launch(&["--windows", "uia:0"]);
            provider.request("foreground 0");
            provider.request("focus 0 5");
            provider.request("raise focus 0 5");
            Some(Started {
                exe: "uia-facts-provider.exe",
                provider: Some(provider),
            })
        }
        "notepad" => {
            let path = apps::system_program("notepad.exe");
            note::evidence(&format!("notepad.exe {}", file_version(&path)));
            std::process::Command::new(path).spawn().ok()?;
            Some(Started {
                exe: "Notepad.exe",
                provider: None,
            })
        }
        _ => {
            std::process::Command::new("explorer.exe")
                .arg("ms-settings:")
                .spawn()
                .ok()?;
            Some(Started {
                exe: "SystemSettings.exe",
                provider: None,
            })
        }
    }
}

/// The first focus `WinEvent` within `limit` from a process that had no
/// window before and whose executable is `exe`: its process id and time.
fn first_winevent(
    winevents: &std::sync::mpsc::Receiver<WinEvent>,
    before: &BTreeSet<u32>,
    exe: &str,
    limit: Duration,
) -> Option<(u32, u64)> {
    let end = now_us() + u64::try_from(limit.as_micros()).expect("a short limit");
    loop {
        let left = end.saturating_sub(now_us());
        let event = winevents
            .recv_timeout(Duration::from_micros(left.max(1)))
            .ok()?;
        let pid = apps::window_pid(event.hwnd);
        let named = apps::image_of(pid)
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .is_some_and(|n| n.eq_ignore_ascii_case(exe));
        if named && !before.contains(&pid) {
            return Some((pid, event.at));
        }
    }
}
