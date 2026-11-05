//! MSAA calls do not pass through UIA's request reader
//! (`uia-facts/msaa-calls-do-not-pass-through-uias-request-reader/readme.md`).
//!
//! - `stuck`: the provider program without `UseComThreading`, with
//!   `--bridge`: window 0 a UIA window, window 1 one with its own
//!   `IAccessible`, window 2 a plain window, all on UI thread 0, and
//!   window 3 a UIA window on UI thread 1. Node 1's name read in window 0
//!   is held, on UIA's I/O thread, for 1,000 ms; meanwhile `accName` on
//!   each window's `OBJID_CLIENT` object (UIA's MSAA bridge, the
//!   window's own `IAccessible`, oleacc's window proxy), each object got
//!   before the hold, and a UIA read into window 3.
//! - `messages`: oleacc's `accName` on the plain window, the window's
//!   messages counted.
//! - `busy-sweep`: oleacc's `accName` on the plain window while its UI
//!   thread is busy for 1, 5, 15 and 60 s (Not known: how long an MSAA call
//!   into a busy thread waits). One run each.
//! - `sent-rate`: 20,000 `WM_GETTEXT`s sent by 1, 4 and 8 threads of this
//!   process, each thread to a plain window on a UI thread of its own;
//!   the rate against one thread's.
//! - `processes`: the 8-thread case again as two processes of 4 threads,
//!   toward the inferred contention in the window manager.

use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use uia_facts_support::args::{Args, trials};
use uia_facts_support::note;
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, TICK_MS, Verdict, deterministic, relation, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::{median, summary_ms};
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{exit_now, now_us};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
use windows::Win32::UI::WindowsAndMessaging::{OBJID_CLIENT, SendMessageW, WM_GETTEXT};
use windows::core::Interface;

/// The messages each `sent-rate` run sends in all.
const MESSAGES: usize = 20_000;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "send", "the one role is send");
        send_role(rest);
    }
    let mut report = Report::start("msaa-calls-do-not-pass-through-uias-request-reader");
    if args.wants("stuck") {
        report.case("stuck", "holding a UIA read on UI thread 0 of a provider without COM threading; reading three windows through MSAA and one through UIA meanwhile");
        stuck(&args, &mut report);
    }
    if args.wants("messages") {
        report.case(
            "messages",
            "reading a plain window's name through oleacc; counting the messages it sent",
        );
        messages(&mut report);
    }
    if args.wants("busy-sweep") {
        report.case("busy-sweep", "making a plain window's UI thread busy for 1, 5, 15 and 60 s; reading its name through oleacc each time");
        busy_sweep(&mut report);
    }
    if args.wants("sent-rate") {
        report.case("sent-rate", "sending 20,000 WM_GETTEXT messages from 1, 4 and 8 threads, each to its own window; timing them");
        sent_rate(&mut report);
    }
    if args.wants("processes") {
        report.case("processes", "sending the 8-thread case from one process and from two processes of 4 threads; timing them");
        processes(&mut report);
    }
    report.finish();
}

/// The window's `OBJID_CLIENT` accessible object.
fn accessible(hwnd: isize) -> IAccessible {
    let mut object: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: any window handle; `object` receives an interface pointer of
    // the IID asked for.
    unsafe {
        AccessibleObjectFromWindow(
            HWND(hwnd as *mut _),
            OBJID_CLIENT.0.cast_unsigned(),
            &IAccessible::IID,
            &raw mut object,
        )
    }
    .expect("the window's accessible object");
    // SAFETY: AccessibleObjectFromWindow returned an IAccessible with a
    // reference this takes over.
    unsafe { IAccessible::from_raw(object) }
}

fn acc_name(object: &IAccessible) -> windows::core::Result<String> {
    // SAFETY: a live object; CHILDID_SELF, an I4 of 0.
    unsafe { object.get_accName(&VARIANT::from(0_i32)) }.map(|n| n.to_string())
}

fn stuck(args: &Args, report: &mut Report) {
    let names = [
        "UIA's MSAA bridge",
        "the window's own IAccessible",
        "oleacc's window proxy",
    ];
    let mut answered = [Vec::new(), Vec::new(), Vec::new()];
    let mut times = [Vec::new(), Vec::new(), Vec::new()];
    let mut uia_waits = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&[
            "--windows",
            "uia:0,msaa:0,plain:0,uia:1",
            "--threading",
            "nocom",
            "--bridge",
        ]);
        let client = uia::client();
        uia::set_timeouts(&client, 2_000, 30_000);
        let root = uia::element(&client, provider.hwnd(0)).expect("window 0");
        let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
        let far_root = uia::element(&client, provider.hwnd(3)).expect("window 3");
        let far = uia::share(&uia::node(&client, &far_root, 1).expect("window 3's node 1"));
        let objects: Vec<IAccessible> = (0..3).map(|w| accessible(provider.hwnd(w))).collect();
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        provider.request("hold GetPropertyValue w=0 n=1");
        let held = uia::spawn(move || uia::name(&uia::resolve(&first)).is_ok());
        provider
            .held(Duration::from_secs(30))
            .expect("the UIA read is held");
        let uia_probe = uia::spawn(move || Timed::of(|| uia::name(&uia::resolve(&far))));
        let reads: Vec<Timed> = objects.iter().map(|o| Timed::of(|| acc_name(o))).collect();
        uia::pause(
            Duration::from_millis(200),
            "with the UIA read held, so the UIA probe is still waiting at the release",
        );
        let released = now_us();
        provider.request("release");
        let probe = uia_probe.join().expect("the UIA probe");
        let _ = held.join();
        for (i, read) in reads.iter().enumerate() {
            answered[i].push(read.end < released && read.outcome == "ok");
            times[i].push(read.ms());
        }
        uia_waits.push(probe.end >= released && probe.outcome == "ok");
    }
    for i in 0..3 {
        report.claim(
            &format!(
                "with a UIA call stuck off the UI thread, accName through {} answers",
                names[i]
            ),
            &runs(&answered[i]),
            "true",
            relation(&answered[i]),
        );
        report.claim(
            &format!("its time through {}", names[i]),
            &summary_ms(&times[i]),
            "1.2 to 2.6 ms",
            Verdict::Trend,
        );
    }
    report.claim(
        "meanwhile a UIA read into the process's other UI thread waits for the release",
        &runs(&uia_waits),
        "true",
        relation(&uia_waits),
    );
}

fn messages(report: &mut Report) {
    let mut counts = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "plain:0"]);
        let object = accessible(provider.hwnd(0));
        provider.log();
        let _ = acc_name(&object).expect("the name");
        let log = provider.log();
        let count = |name: &str| {
            log.iter()
                .filter(|e| e.kind == "msg" && e.name == name)
                .count()
        };
        counts.push((count("WM_GETTEXTLENGTH"), count("WM_GETTEXT")));
        note::evidence(&format!("messages: {log:?}"));
    }
    report.claim(
        "oleacc's accName on a plain window sends (WM_GETTEXTLENGTH, WM_GETTEXT)",
        &runs(&counts),
        "(1, 1)",
        deterministic(&counts, &(1, 1)),
    );
}

/// How long `busy-sweep` keeps the UI thread busy.
const BUSY_MS: u64 = 20_000;

fn busy_sweep(report: &mut Report) {
    let mut waited = Vec::new();
    let mut rows = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "plain:0"]);
        let object = accessible(provider.hwnd(0));
        provider.request(&format!("busy 0 {BUSY_MS}"));
        note::waiting(&format!(
            "{} s, the UI thread busy under the accName read",
            BUSY_MS / 1_000
        ));
        let read = Timed::of(|| acc_name(&object));
        // A read that waited the whole busy time, to within a tick, and
        // then answered: no bound cut it short.
        #[expect(clippy::cast_precision_loss, reason = "a short busy time")]
        let busy = BUSY_MS as f64;
        waited.push(read.outcome == "ok" && read.ms() >= busy - TICK_MS);
        rows.push(format!("{} after {:.0} ms", read.outcome, read.ms()));
    }
    report.claim(
        "oleacc's accName into a UI thread busy for 20 s waits the whole busy time, then answers",
        &format!("{} ({})", runs(&waited), rows.join("; ")),
        "true (answered after 1,001, 5,001, 15,000 and 60,000 ms at 1, 5, 15 and 60 s)",
        relation(&waited),
    );
}

/// Sends `per_window` `WM_GETTEXT`s to each window, one thread a window,
/// all starting together; answers the milliseconds until the last ends.
fn send_all(hwnds: &[isize], per_window: usize) -> f64 {
    let barrier = Arc::new(Barrier::new(hwnds.len() + 1));
    let threads: Vec<_> = hwnds
        .iter()
        .map(|&hwnd| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut buffer = [0u16; 64];
                barrier.wait();
                for _ in 0..per_window {
                    // SAFETY: a window of the provider program and a live
                    // buffer of the length given.
                    let _ = unsafe {
                        SendMessageW(
                            HWND(hwnd as *mut _),
                            WM_GETTEXT,
                            Some(WPARAM(buffer.len())),
                            Some(LPARAM(buffer.as_mut_ptr() as isize)),
                        )
                    };
                }
            })
        })
        .collect();
    barrier.wait();
    let start = Instant::now();
    for thread in threads {
        thread.join().expect("a sender");
    }
    let ms = uia::ms(start.elapsed());
    note::evidence(&format!(
        "{} senders, {per_window} messages each: {ms:.1} ms",
        hwnds.len()
    ));
    ms
}

fn sent_rate(report: &mut Report) {
    let layout: Vec<String> = (0..8).map(|t| format!("plain:{t}")).collect();
    let mut ratios = [Vec::new(), Vec::new()];
    for _ in 0..trials() {
        let provider = Provider::launch(&["--windows", &layout.join(",")]);
        let hwnds: Vec<isize> = (0..8).map(|w| provider.hwnd(w)).collect();
        let one = send_all(&hwnds[..1], MESSAGES);
        for (i, threads) in [4, 8].into_iter().enumerate() {
            let many = send_all(&hwnds[..threads], MESSAGES / threads);
            ratios[i].push(one / many);
        }
    }
    for (i, threads) in [4, 8].into_iter().enumerate() {
        let slower: Vec<bool> = ratios[i].iter().map(|r| *r < 1.0).collect();
        report.claim(
            &format!("{threads} sending threads, each to its own UI thread, send fewer messages a second than one"),
            &runs(&slower),
            "true",
            relation(&slower),
        );
        report.claim(
            &format!("the rate of {threads} threads against one"),
            &format!(
                "{:.2} (runs {:?})",
                median(&ratios[i]),
                ratios[i]
                    .iter()
                    .map(|r| format!("{r:.2}"))
                    .collect::<Vec<_>>()
            ),
            "0.52 to 0.78",
            Verdict::Trend,
        );
    }
}

/// The second process of `processes`: sends `per_window` messages to each
/// window named, one thread each, on `go`, and reports `done <ms>`.
fn send_role(rest: &[String]) -> ! {
    let per_window: usize = rest[0].parse().expect("a count");
    let hwnds: Vec<isize> = rest[1..]
        .iter()
        .map(|h| h.parse().expect("a window handle"))
        .collect();
    role::report("ready");
    for command in role::commands() {
        if command == "go" {
            role::report(&format!("done {}", send_all(&hwnds, per_window)));
        }
    }
    exit_now(0)
}

fn processes(report: &mut Report) {
    let layout: Vec<String> = (0..8).map(|t| format!("plain:{t}")).collect();
    let mut rows = Vec::new();
    let mut no_faster = Vec::new();
    for _ in 0..trials() {
        let provider = Provider::launch(&["--windows", &layout.join(",")]);
        let hwnds: Vec<isize> = (0..8).map(|w| provider.hwnd(w)).collect();
        let per_window = MESSAGES / 8;
        let threads = send_all(&hwnds, per_window);
        let mut halves: Vec<Role> = [&hwnds[..4], &hwnds[4..]]
            .iter()
            .map(|half| {
                let mut words = vec![per_window.to_string()];
                words.extend(half.iter().map(ToString::to_string));
                Role::start("send", &words)
            })
            .collect();
        for half in &halves {
            assert_eq!(half.expect_line(), "ready", "the sender is ready");
        }
        let start = Instant::now();
        for half in &mut halves {
            half.send("go");
        }
        for half in &halves {
            let _ = half.expect_line();
        }
        let two = uia::ms(start.elapsed());
        no_faster.push(two >= threads);
        rows.push(format!(
            "8 threads of one process {threads:.0} ms, of two processes {two:.0} ms"
        ));
    }
    report.claim(
        "20,000 messages by 8 threads take two processes no less time than one",
        &format!("{} ({})", runs(&no_faster), rows.join("; ")),
        "true (one process 802 to 810 ms, two 810 to 822 ms)",
        relation(&no_faster),
    );
}
