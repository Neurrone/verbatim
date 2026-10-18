//! Experiment e57: does a registration that timed out against a stalled
//! window deliver events once the stall ends, and from when?
//!
//! One trial per process. Settings from the environment:
//!
//! - `E57_CTO`, `E57_TTO`: the connection and transaction timeouts, in ms,
//!   set in the registering process.
//! - `E57_WHERE`: `in` registers on a thread of the test process; `out`
//!   registers in a second process, this binary started again.
//! - `E57_STALL`: `1` (default) registers while mockapp's window thread is
//!   stalled (`stall-until-release`); `0` registers with no stall, the
//!   control.
//! - `E57_RAISES`: the focus events to raise, as offsets in ms from the
//!   stall's end (the release's acknowledgement), comma-separated; each
//!   renames a focusable node to `evN` and raises UIA's focus event on it.
//! - `E57_OBSERVE`: how long to listen after the last raise, in ms.
//! - `E57_HOLD_AFTER`: how long after the registration returns the stall
//!   is released, in ms (200 by default).
//! - `E57_POSTED`: `0` stalls without `posted`, so that mockapp reports
//!   only messages sent to the stalled thread.
//!
//! This file was run from `crates/mockapp/tests/uia_bugs/`; the readme
//! beside it says how to put it back there.
//!
//! The handler reads the sender's cached name, filled in mockapp's
//! process as the event is raised, so a late delivery still names the
//! event it belongs to.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomation6, TreeScope_Subtree, UIA_AutomationFocusChangedEventId, UIA_NamePropertyId,
};
use windows::core::Interface;

use crate::{common, support};

/// Names mockapp's window, making this binary the registering process.
pub const REGISTRAR: &str = "VERBATIM_E57_REGISTRAR";

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The registering client: sets both timeouts, fetches the window's
/// element, says `ready`, registers a group with the focus event on the
/// window when `go` arrives, says `registered <result> <ms>`, and then
/// `heard <name> <unix us>` for each event, until `go` closes.
fn registrar(hwnd: HWND, say: Arc<dyn Fn(String) + Send + Sync>, go: &Receiver<()>) {
    let uia = support::client();
    let client2: IUIAutomation2 = uia.client().cast().expect("IUIAutomation2");
    let cto = u32::try_from(env_u64("E57_CTO", 2_000)).expect("fits");
    let tto = u32::try_from(env_u64("E57_TTO", 5_000)).expect("fits");
    unsafe { client2.SetConnectionTimeout(cto) }.expect("cto");
    unsafe { client2.SetTransactionTimeout(tto) }.expect("tto");
    let window = support::element(uia.client(), hwnd);
    say("ready".to_owned());
    if go.recv().is_err() {
        return;
    }
    let cache = support::cache(uia.client());
    unsafe { cache.AddProperty(UIA_NamePropertyId) }.expect("name cached");
    let handler = hearing::new(say.clone());
    let client6: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
    let group = unsafe { client6.CreateEventHandlerGroup() }.expect("group");
    unsafe {
        group.AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            TreeScope_Subtree,
            &cache,
            &handler,
        )
    }
    .expect("focus in group");
    let started = Instant::now();
    let result = unsafe { client6.AddEventHandlerGroup(&window, &group) };
    let took = started.elapsed().as_millis();
    say(format!(
        "registered {} {took}",
        match &result {
            Ok(()) => "ok".to_owned(),
            Err(e) => format!("{:#010x}", e.code().0),
        }
    ));
    let _ = go.recv();
    drop(group);
}

/// The registering process's body.
pub fn registrar_process(window: &str) -> ! {
    let hwnd = HWND(window.parse::<isize>().expect("a window handle") as *mut _);
    let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(|line: String| {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    });
    let (go, gone) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() || go.send(()).is_err() {
                break;
            }
        }
    });
    registrar(hwnd, say, &gone);
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::abort();
}

/// The registrar, in this process or another, as lines and a trigger.
struct Registrar {
    lines: Receiver<String>,
    go: Box<dyn FnMut()>,
    end: Box<dyn FnOnce()>,
}

fn start_registrar(hwnd: HWND, out_of_process: bool) -> Registrar {
    let (tx, lines) = mpsc::channel::<String>();
    if out_of_process {
        common::contain_children();
        let mut child = Command::new(std::env::current_exe().expect("exe"))
            .env(REGISTRAR, (hwnd.0 as isize).to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("registrar started");
        let stdin = Arc::new(Mutex::new(child.stdin.take().expect("stdin")));
        let stdout = child.stdout.take().expect("stdout");
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        let input = stdin.clone();
        Registrar {
            lines,
            go: Box::new(move || {
                let mut input = input.lock().expect("lock");
                writeln!(input, "go").expect("go");
                input.flush().expect("flush");
            }),
            end: Box::new(move || {
                drop(stdin);
                let _ = child.kill();
                let _ = child.wait();
            }),
        }
    } else {
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let hwnd = hwnd.0 as isize;
        let tx = Mutex::new(tx);
        std::thread::spawn(move || {
            let say: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |line| {
                let _ = tx.lock().expect("lock").send(line);
            });
            registrar(HWND(hwnd as *mut _), say, &go_rx);
        });
        let go_tx2 = go_tx.clone();
        Registrar {
            lines,
            go: Box::new(move || go_tx2.send(()).expect("go")),
            end: Box::new(move || drop(go_tx)),
        }
    }
}

fn ms_since(us: u64, from: u64) -> f64 {
    (us as f64 - from as f64) / 1_000.0
}

/// One trial; prints one summary line beginning `e57`.
pub fn e57_trial() {
    let cto = env_u64("E57_CTO", 2_000);
    let tto = env_u64("E57_TTO", 5_000);
    let out_of_process = std::env::var("E57_WHERE").as_deref() == Ok("out");
    let stall = env_u64("E57_STALL", 1) == 1;
    let observe = env_u64("E57_OBSERVE", 6_000);
    let raises: Vec<u64> = std::env::var("E57_RAISES")
        .unwrap_or_else(|_| "0".to_owned())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse().expect("an offset"))
        .collect();

    let (mut app, hwnd) = support::mockapp("e57", "uia");
    let mut reg = start_registrar(hwnd, out_of_process);
    let line = reg.lines.recv_timeout(common::WAIT_TIMEOUT).expect("ready");
    assert_eq!(line, "ready");

    let mut sends = 0;
    let mut early = Vec::new();
    if stall {
        app.write(if std::env::var("E57_POSTED").as_deref() == Ok("0") { "stall-until-release" } else { "stall-until-release posted" });
        app.expect_line("stall started", String::new);
    }
    let go_us = common::now_us();
    (reg.go)();
    let registered = reg
        .lines
        .recv_timeout(Duration::from_secs(60))
        .expect("registered line");
    let registered_us = common::now_us();
    let release_us;
    if stall {
        // Give UIA a moment to finish anything after the call returned, as
        // a caller would not release at once; then release.
        std::thread::sleep(Duration::from_millis(env_u64("E57_HOLD_AFTER", 200)));
        app.write("release");
        loop {
            let line = app
                .line_within(common::WAIT_TIMEOUT)
                .expect("mockapp's lines");
            match line.as_str() {
                "stall message sent" => sends += 1,
                "applied" => break,
                other => early.push(other.to_owned()),
            }
        }
        release_us = common::now_us();
    } else {
        release_us = common::now_us();
    }

    let nodes = ["btn1", "item1", "item2", "slider1"];
    let mut raised = Vec::new();
    for (i, offset) in raises.iter().enumerate() {
        let target = release_us + offset * 1_000;
        let now = common::now_us();
        if target > now {
            std::thread::sleep(Duration::from_micros(target - now));
        }
        let node = nodes[i % nodes.len()];
        app.send(&format!("set-name {node} ev{i}"));
        let at = common::now_us();
        app.send(&format!("focus {node}"));
        raised.push(at);
    }
    let deadline = Instant::now() + Duration::from_millis(observe);
    let mut heard: Vec<(String, u64)> = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match reg.lines.recv_timeout(left) {
            Ok(line) => {
                let mut parts = line.splitn(3, ' ');
                assert_eq!(parts.next(), Some("heard"), "registrar said {line:?}");
                let name = parts.next().expect("name").to_owned();
                let us: u64 = parts.next().expect("time").parse().expect("us");
                heard.push((name, us));
            }
            Err(_) => break,
        }
    }
    let mut summary = format!(
        "e57 where={} cto={cto} tto={tto} stall={} {} (returned {:.0} ms after go) sends={sends} other_lines={early:?} released {:.0} ms after return;",
        if out_of_process { "out" } else { "in" },
        u8::from(stall),
        registered,
        ms_since(registered_us, go_us),
        ms_since(release_us, registered_us),
    );
    for (i, at) in raised.iter().enumerate() {
        let name = format!("ev{i}");
        let got: Vec<&(String, u64)> = heard.iter().filter(|(n, _)| *n == name).collect();
        summary.push_str(&format!(" ev{i} raised +{:.0}", ms_since(*at, release_us)));
        if got.is_empty() {
            summary.push_str(" not heard;");
        } else {
            for (_, us) in got {
                summary.push_str(&format!(
                    " heard +{:.0} (after {:.0});",
                    ms_since(*us, release_us),
                    ms_since(*us, *at)
                ));
            }
        }
    }
    let others: Vec<&(String, u64)> = heard
        .iter()
        .filter(|(n, _)| !(0..raised.len()).any(|i| *n == format!("ev{i}")))
        .collect();
    if !others.is_empty() {
        summary.push_str(&format!(" others={others:?}"));
    }
    eprintln!("{summary}");
    (reg.end)();
    app.quit();
}

/// The handler, reading the sender's cached name.
mod hearing {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::Arc;

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        UIA_EVENT_ID,
    };

    pub(super) fn new(say: Arc<dyn Fn(String) + Send + Sync>) -> IUIAutomationEventHandler {
        Hearing { say }.into()
    }

    #[windows_core::implement(IUIAutomationEventHandler)]
    struct Hearing {
        say: Arc<dyn Fn(String) + Send + Sync>,
    }

    impl IUIAutomationEventHandler_Impl for Hearing_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            _event: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            let us = crate::common::now_us();
            let name = sender
                .as_ref()
                .and_then(|e| unsafe { e.CachedName() }.ok())
                .map_or_else(|| "?".to_owned(), |n| n.to_string());
            (self.say)(format!("heard {name} {us}"));
            Ok(())
        }
    }
}
