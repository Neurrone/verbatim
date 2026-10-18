//! Experiment e58: what happens in mockapp, and to other registrations,
//! while a registration that timed out against a stalled window hears
//! nothing, and whether it can be removed then.
//!
//! One trial per process. `E58_CASE` picks the trial:
//!
//! - `silent`: mockapp counts registrations (`--count-registrations`) and,
//!   with `E58_LOG` set in the environment, which mockapp inherits, logs
//!   what reaches its window thread from the release on (the patch beside
//!   this file). A client on a thread of the test process
//!   registers a group with the focus event on the stalled window; the
//!   stall is released 200 ms after the call returns, and nothing more is
//!   done for `E58_OBSERVE` ms (6,000 by default).
//! - `onset`: as `silent`, then focus events are raised every 100 ms from
//!   the stall's end to 3,000 ms after it, to show when the registration
//!   starts to deliver.
//! - `scope`: no counting. Before the stall, this process registers a group
//!   with the focus event on the window (A) and one on a second mockapp's
//!   window (C), on its main thread's client, and a second process, this
//!   binary started again, registers one on the window (D). During the
//!   stall, a client on another thread of this process registers one on the
//!   window (B), which times out. After the release, focus events are raised
//!   on the window every 100 ms from 0 to 3,000 ms and on the second window
//!   every 100 ms from 50 to 2,950 ms.
//! - `remove`, `remove-late`, `remove-nostall`: a client on a thread of the
//!   test process registers a group on the window, with a stall (the first
//!   two) or without one, and removes it (`RemoveEventHandlerGroup`) 500 ms
//!   (`remove`, `remove-nostall`) or 3,000 ms (`remove-late`) after the
//!   stall's end; it then drops its own references to the group and the
//!   handler and waits up to 15 s for the handler object to be released.
//!   Focus events are raised at 1,000 and 4,000 ms after the stall's end.
//!
//! The stall is `stall-until-release quiet`; the connection timeout 2,000
//! ms and the transaction timeout 5,000 ms, set in each registering
//! process. Each handler reads the sender's cached name, which mockapp
//! sets to `ev<n>` (or `w2ev<n>` on the second window) before raising
//! event n, so a delivery names the event it belongs to.
//!
//! This file was run from `crates/mockapp/tests/uia_bugs/` in a worktree at
//! commit 64557420, put there by `apply.py` beside it with the mockapp
//! patch; the readme beside this folder says how.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomation2, IUIAutomation6, IUIAutomationEventHandlerGroup, TreeScope_Subtree, UIA_AutomationFocusChangedEventId,
    UIA_NamePropertyId,
};
use windows::core::Interface;

use crate::{common, support};

/// Names mockapp's window, making this binary the second process's
/// listener (`scope`'s D).
pub const LISTENER: &str = "VERBATIM_E58_LISTENER";

const CTO: u32 = 2_000;
const TTO: u32 = 5_000;

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn set_timeouts(client: &IUIAutomation) {
    let client2: IUIAutomation2 = client.cast().expect("IUIAutomation2");
    unsafe { client2.SetConnectionTimeout(CTO) }.expect("cto");
    unsafe { client2.SetTransactionTimeout(TTO) }.expect("tto");
}

/// A group with the focus event, the name cached, heard by `handler`.
fn focus_group(
    client: &IUIAutomation,
    handler: &windows::Win32::UI::Accessibility::IUIAutomationEventHandler,
) -> IUIAutomationEventHandlerGroup {
    let cache = support::cache(client);
    unsafe { cache.AddProperty(UIA_NamePropertyId) }.expect("name cached");
    let client6: IUIAutomation6 = client.cast().expect("IUIAutomation6");
    let group = unsafe { client6.CreateEventHandlerGroup() }.expect("group");
    unsafe {
        group.AddAutomationEventHandler(
            UIA_AutomationFocusChangedEventId,
            TreeScope_Subtree,
            &cache,
            handler,
        )
    }
    .expect("focus in group");
    group
}

/// What a heard event says: the handler's label, the sender's name, when.
type Heard = (String, String, u64);

/// The second process's listener: registers a focus group on the window,
/// says `ready`, then `heard <name> <us>` for each event, until its input
/// closes.
pub fn listener(window: &str) -> ! {
    let hwnd = HWND(window.parse::<isize>().expect("a window handle") as *mut _);
    let uia = support::client();
    set_timeouts(uia.client());
    let element = support::element(uia.client(), hwnd);
    let (tx, rx) = mpsc::channel::<Heard>();
    let handler = hearing::new("D", tx, None);
    let group = focus_group(uia.client(), &handler);
    let client6: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
    unsafe { client6.AddEventHandlerGroup(&element, &group) }.expect("D registered");
    std::thread::spawn(|| {
        for line in std::io::stdin().lock().lines() {
            if line.is_err() {
                break;
            }
        }
        unsafe {
            let _ = TerminateProcess(GetCurrentProcess(), 0);
        }
    });
    let mut out = std::io::stdout();
    writeln!(out, "ready").expect("written");
    out.flush().expect("flushed");
    for (_, name, us) in rx {
        writeln!(out, "heard {name} {us}").expect("written");
        out.flush().expect("flushed");
    }
    std::process::abort();
}

/// mockapp's lines, sorted as they are read: its log (`log ...`, from the
/// patch) kept with the time it gives, and the rest returned.
struct Lines<'a> {
    app: &'a common::MockApp,
    log: Vec<(String, u64)>,
}

impl Lines<'_> {
    /// Reads lines until `want`, keeping the log; returns when it came.
    fn until(&mut self, want: &str) -> u64 {
        loop {
            let line = self
                .app
                .line_within(Duration::from_secs(30))
                .unwrap_or_else(|| panic!("mockapp said nothing while waiting for {want:?}"));
            if let Some(rest) = line.strip_prefix("log ") {
                let (what, us) = rest.rsplit_once(' ').expect("a log line's time");
                self.log.push((what.to_owned(), us.parse().expect("us")));
                continue;
            }
            assert_eq!(line, want, "mockapp's next line");
            return common::now_us();
        }
    }

    /// Reads the log until `deadline`; any other line fails the trial.
    fn drain_until(&mut self, deadline: Instant) {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            let Some(line) = self.app.line_within(left) else {
                return;
            };
            let rest = line
                .strip_prefix("log ")
                .unwrap_or_else(|| panic!("mockapp said {line:?}"));
            let (what, us) = rest.rsplit_once(' ').expect("a log line's time");
            self.log.push((what.to_owned(), us.parse().expect("us")));
        }
    }
}

fn ms(us: u64, from: u64) -> i64 {
    (i64::try_from(us).expect("fits") - i64::try_from(from).expect("fits")) / 1_000
}

/// A client on a thread of its own: sets the timeouts, fetches the window,
/// and, on each order, registers a focus group (heard as `label`) or
/// removes it, answering how it went and how long it took.
struct Registrar {
    orders: Sender<Order>,
    answers: Receiver<String>,
}

enum Order {
    Register,
    Remove,
}

impl Registrar {
    fn start(
        hwnd: HWND,
        label: &'static str,
        heard: Sender<Heard>,
        released: Option<Sender<u64>>,
    ) -> Self {
        let (orders, taken) = mpsc::channel::<Order>();
        let (answer, answers) = mpsc::channel::<String>();
        let hwnd = hwnd.0 as isize;
        std::thread::spawn(move || {
            let uia = support::client();
            set_timeouts(uia.client());
            let window = support::element(uia.client(), HWND(hwnd as *mut _));
            let client6: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
            let mut handler = Some(hearing::new(label, heard, released));
            let mut group = None;
            answer.send("ready".to_owned()).expect("answered");
            for order in taken {
                match order {
                    Order::Register => {
                        let made = focus_group(uia.client(), handler.as_ref().expect("handler"));
                        let started = Instant::now();
                        let result = unsafe { client6.AddEventHandlerGroup(&window, &made) };
                        let took = started.elapsed().as_millis();
                        group = Some(made);
                        answer
                            .send(format!("{} {took}", outcome(&result)))
                            .expect("answered");
                    }
                    Order::Remove => {
                        let made = group.take().expect("a group to remove");
                        let started = Instant::now();
                        let result = unsafe { client6.RemoveEventHandlerGroup(&window, &made) };
                        let took = started.elapsed().as_millis();
                        drop(made);
                        handler = None;
                        answer
                            .send(format!("{} {took}", outcome(&result)))
                            .expect("answered");
                    }
                }
            }
            drop(handler);
            drop(group);
        });
        let registrar = Self { orders, answers };
        assert_eq!(registrar.answer(), "ready");
        registrar
    }

    fn order(&self, order: Order) -> String {
        self.orders.send(order).expect("ordered");
        self.answer()
    }

    fn answer(&self) -> String {
        self.answers
            .recv_timeout(Duration::from_secs(60))
            .expect("the registrar answers")
    }
}

fn outcome(result: &windows::core::Result<()>) -> String {
    match result {
        Ok(()) => "ok".to_owned(),
        Err(e) => format!("{:#010x}", e.code().0),
    }
}

/// Raises a focus event on `node`, renamed `name` first; returns when the
/// rename was written and when the focus was acknowledged.
fn raise(app: &mut common::MockApp, lines_of: &mut Vec<(String, u64)>, node: &str, name: &str) -> (u64, u64) {
    let began = common::now_us();
    app.write(&format!("set-name {node} {name}"));
    let mut lines = Lines { app, log: std::mem::take(lines_of) };
    lines.until("applied");
    *lines_of = lines.log;
    app.write(&format!("focus {node}"));
    let mut lines = Lines { app, log: std::mem::take(lines_of) };
    let ended = lines.until("applied");
    *lines_of = lines.log;
    (began, ended)
}

const NODES: [&str; 4] = ["btn1", "item1", "item2", "slider1"];

/// Summarises what `label` heard of the events `raised` (name, offset ms):
/// the names lost, and each heard's offset and delay.
fn summarise(label: &str, raised: &[(String, i64, u64)], heard: &[Heard], release: u64) -> String {
    let mut lost = Vec::new();
    let mut got = Vec::new();
    for (name, offset, at) in raised {
        let mine: Vec<&Heard> = heard.iter().filter(|(l, n, _)| l == label && n == name).collect();
        if mine.is_empty() {
            lost.push(format!("{name}@{offset}"));
        }
        for (_, _, us) in mine {
            got.push(format!("{name}@{offset}+{}", ms(*us, *at)));
        }
    }
    let others: Vec<String> = heard
        .iter()
        .filter(|(l, n, _)| l == label && !raised.iter().any(|(name, ..)| name == n))
        .map(|(_, n, us)| format!("{n}@{}", ms(*us, release)))
        .collect();
    format!("{label}: lost [{}] heard [{}] others [{}];", lost.join(" "), got.join(" "), others.join(" "))
}

/// One trial; prints one summary line beginning `e58`.
pub fn e58_trial() {
    let case = std::env::var("E58_CASE").expect("E58_CASE");
    match case.as_str() {
        "silent" | "onset" => counted(&case),
        "scope" => scope(),
        "remove" | "remove-late" | "remove-nostall" => remove(&case),
        other => panic!("no case {other}"),
    }
}

/// `silent` and `onset`.
fn counted(case: &str) {
    let title = common::unique_title("e58");
    let mut app = common::spawn_counting_registrations("small.json", &title);
    let hwnd = common::find_window(&title);
    let (tx, heard_rx) = mpsc::channel::<Heard>();
    let registrar = Registrar::start(hwnd, "B", tx, None);
    let mut log = Vec::new();

    app.write("stall-until-release quiet");
    let mut lines = Lines { app: &app, log: std::mem::take(&mut log) };
    lines.until("stall started");
    log = lines.log;
    let registered = registrar.order(Order::Register);
    let returned = common::now_us();
    std::thread::sleep(Duration::from_millis(200));
    app.write("release");
    let mut lines = Lines { app: &app, log: std::mem::take(&mut log) };
    let release = lines.until("applied");
    log = lines.log;

    let mut raised: Vec<(String, i64, u64)> = Vec::new();
    if case == "onset" {
        for (i, offset) in (0..=3_000u64).step_by(100).enumerate() {
            let target = release + offset * 1_000;
            let now = common::now_us();
            if target > now {
                std::thread::sleep(Duration::from_micros(target - now));
            }
            let name = format!("ev{i}");
            let (_, ended) = raise(&mut app, &mut log, NODES[i % NODES.len()], &name);
            raised.push((name, ms(ended, release), ended));
        }
    }
    let observe = env_u64("E58_OBSERVE", 6_000);
    let mut lines = Lines { app: &app, log: std::mem::take(&mut log) };
    lines.drain_until(Instant::now() + Duration::from_millis(observe));
    log = lines.log;
    let heard: Vec<Heard> = heard_rx.try_iter().collect();

    // The log after the release: what came within 20 ms of it (the
    // stalled thread's backlog), and what came after, up to the first
    // event the registration heard (or to the end), as distinct kinds,
    // and the first provider call, sent message or advise after the
    // backlog.
    let backlog_end = release + 20_000;
    let first_heard = heard
        .iter()
        .filter(|(label, ..)| label == "B")
        .map(|(.., us)| *us)
        .min();
    let backlog: Vec<String> = log
        .iter()
        .filter(|(_, us)| *us <= backlog_end)
        .map(|(what, us)| format!("{what}@{}", ms(*us, release)))
        .collect();
    let mut before_delivery: Vec<String> = log
        .iter()
        .filter(|(_, us)| *us > backlog_end && first_heard.is_none_or(|first| *us < first))
        .map(|(what, _)| what.clone())
        .collect();
    let gap_items = before_delivery.len();
    before_delivery.sort();
    before_delivery.dedup();
    let first_activity = log
        .iter()
        .filter(|(what, us)| {
            *us > backlog_end
                && (what.starts_with("call") || what.starts_with("sent") || what.starts_with("advise"))
        })
        .map(|(what, us)| format!("{what}@{}", ms(*us, release)))
        .next();
    let advises: Vec<String> = log
        .iter()
        .filter(|(what, _)| what.starts_with("advise"))
        .map(|(what, us)| format!("{what}@{}", ms(*us, release)))
        .collect();
    let last = log.iter().map(|(_, us)| ms(*us, release)).max();
    eprintln!(
        "e58 case={case} registered {registered} released {} ms after return; observed {} ms after the last raise; advises [{}]; backlog [{}]; after it, before the first delivery ({:?} ms): {gap_items} items of kinds [{}]; first call, sent message or advise after the backlog: {first_activity:?}; last log item at {last:?} ms; {}",
        ms(release, returned),
        observe,
        advises.join(" "),
        backlog.join(" "),
        first_heard.map(|first| ms(first, release)),
        before_delivery.join(", "),
        summarise("B", &raised, &heard, release),
    );
    drop(registrar);
    app.quit();
}

/// `scope`.
fn scope() {
    let (mut app, hwnd) = support::mockapp("e58", "uia");
    let (mut other, other_hwnd) = support::mockapp("e58-other", "uia");
    let uia = support::client();
    set_timeouts(uia.client());
    let (tx, heard_rx) = mpsc::channel::<Heard>();
    let client6: IUIAutomation6 = uia.client().cast().expect("IUIAutomation6");
    let a = hearing::new("A", tx.clone(), None);
    let a_group = focus_group(uia.client(), &a);
    unsafe { client6.AddEventHandlerGroup(&support::element(uia.client(), hwnd), &a_group) }
        .expect("A registered");
    let c = hearing::new("C", tx.clone(), None);
    let c_group = focus_group(uia.client(), &c);
    unsafe {
        client6.AddEventHandlerGroup(&support::element(uia.client(), other_hwnd), &c_group)
    }
    .expect("C registered");

    // D, in a second process.
    common::contain_children();
    let mut child = Command::new(std::env::current_exe().expect("exe"))
        .env(LISTENER, (hwnd.0 as isize).to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("listener started");
    let stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (dtx, d_lines) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if dtx.send(line).is_err() {
                return;
            }
        }
    });
    assert_eq!(d_lines.recv_timeout(Duration::from_secs(30)).expect("ready"), "ready");

    // Each hears an event before the stall.
    other.send("set-name btn1 pre");
    other.send("focus btn1");
    app.send("set-name btn1 pre");
    app.send("focus btn1");
    std::thread::sleep(Duration::from_millis(300));

    let registrar = Registrar::start(hwnd, "B", tx, None);
    app.write("stall-until-release quiet");
    app.expect_line("stall started", String::new);
    let registered = registrar.order(Order::Register);
    let returned = common::now_us();
    std::thread::sleep(Duration::from_millis(200));
    app.write("release");
    app.expect_line("applied", String::new);
    let release = common::now_us();

    let mut raised: Vec<(String, i64, u64)> = Vec::new();
    let mut raised_other: Vec<(String, i64, u64)> = Vec::new();
    let mut schedule: Vec<(u64, bool, usize)> = Vec::new();
    for (i, offset) in (0..=3_000u64).step_by(100).enumerate() {
        schedule.push((offset, false, i));
        if offset + 50 < 3_000 {
            schedule.push((offset + 50, true, i));
        }
    }
    for (offset, on_other, i) in schedule {
        let target = release + offset * 1_000;
        let now = common::now_us();
        if target > now {
            std::thread::sleep(Duration::from_micros(target - now));
        }
        let node = NODES[i % NODES.len()];
        if on_other {
            let name = format!("w2ev{i}");
            other.send(&format!("set-name {node} {name}"));
            other.send(&format!("focus {node}"));
            let at = common::now_us();
            raised_other.push((name, ms(at, release), at));
        } else {
            let name = format!("ev{i}");
            app.send(&format!("set-name {node} {name}"));
            app.send(&format!("focus {node}"));
            let at = common::now_us();
            raised.push((name, ms(at, release), at));
        }
    }
    std::thread::sleep(Duration::from_millis(3_000));
    let mut heard: Vec<Heard> = heard_rx.try_iter().collect();
    for line in d_lines.try_iter() {
        let mut parts = line.splitn(3, ' ');
        assert_eq!(parts.next(), Some("heard"), "D said {line:?}");
        let name = parts.next().expect("name").to_owned();
        let us = parts.next().expect("us").parse().expect("us");
        heard.push(("D".to_owned(), name, us));
    }
    let pre: Vec<String> = heard
        .iter()
        .filter(|(_, n, _)| n == "pre")
        .map(|(l, ..)| l.clone())
        .collect();
    eprintln!(
        "e58 case=scope B registered {registered} released {} ms after return; heard before the stall by [{}]; {} {} {} {}",
        ms(release, returned),
        pre.join(" "),
        summarise("A", &raised, &heard, release),
        summarise("B", &raised, &heard, release),
        summarise("D", &raised, &heard, release),
        summarise("C", &raised_other, &heard, release),
    );
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    drop(registrar);
    drop((a_group, c_group, a, c));
    app.quit();
    other.quit();
}

/// `remove`, `remove-late` and `remove-nostall`.
fn remove(case: &str) {
    let (mut app, hwnd) = support::mockapp("e58", "uia");
    let (tx, heard_rx) = mpsc::channel::<Heard>();
    let (dropped_tx, dropped) = mpsc::channel::<u64>();
    let registrar = Registrar::start(hwnd, "B", tx, Some(dropped_tx));
    let stall = case != "remove-nostall";
    if stall {
        app.write("stall-until-release quiet");
        app.expect_line("stall started", String::new);
    }
    let registered = registrar.order(Order::Register);
    let returned = common::now_us();
    std::thread::sleep(Duration::from_millis(200));
    if stall {
        app.write("release");
        app.expect_line("applied", String::new);
    }
    let release = common::now_us();
    let remove_at = if case == "remove-late" { 3_000 } else { 500 };
    let mut raised: Vec<(String, i64, u64)> = Vec::new();
    let mut removed = String::new();
    let mut removed_us = 0;
    let mut steps: Vec<(u64, Option<usize>)> = vec![(remove_at, None), (1_000, Some(0)), (4_000, Some(1))];
    steps.sort_by_key(|(at, _)| *at);
    for (offset, event) in steps {
        let target = release + offset * 1_000;
        let now = common::now_us();
        if target > now {
            std::thread::sleep(Duration::from_micros(target - now));
        }
        match event {
            None => {
                removed_us = common::now_us();
                removed = registrar.order(Order::Remove);
            }
            Some(i) => {
                let name = format!("ev{i}");
                app.send(&format!("set-name {} {name}", NODES[i]));
                app.send(&format!("focus {}", NODES[i]));
                let at = common::now_us();
                raised.push((name, ms(at, release), at));
            }
        }
    }
    let released = dropped
        .recv_timeout(Duration::from_millis(15_000).saturating_sub(Duration::from_micros(common::now_us() - removed_us)))
        .map_or_else(
            |_| format!("not released within {} ms of the removal", ms(common::now_us(), removed_us)),
            |us| format!("released {} ms after the removal began", ms(us, removed_us)),
        );
    let heard: Vec<Heard> = heard_rx.try_iter().collect();
    eprintln!(
        "e58 case={case} registered {registered} released {} ms after return; removal at +{} from the stall's end: {removed}; handler {released}; {}",
        ms(release, returned),
        ms(removed_us, release),
        summarise("B", &raised, &heard, release),
    );
    drop(registrar);
    app.quit();
}

/// The handler, reading the sender's cached name, and saying when UIA
/// releases it.
mod hearing {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use std::sync::Mutex;
    use std::sync::mpsc::Sender;

    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        UIA_EVENT_ID,
    };

    pub(super) fn new(
        label: &'static str,
        heard: Sender<super::Heard>,
        released: Option<Sender<u64>>,
    ) -> IUIAutomationEventHandler {
        Hearing {
            label,
            heard: Mutex::new(heard),
            released: Mutex::new(released),
        }
        .into()
    }

    #[windows_core::implement(IUIAutomationEventHandler)]
    struct Hearing {
        label: &'static str,
        heard: Mutex<Sender<super::Heard>>,
        released: Mutex<Option<Sender<u64>>>,
    }

    impl Drop for Hearing {
        fn drop(&mut self) {
            if let Some(released) = self.released.lock().expect("lock").take() {
                let _ = released.send(crate::common::now_us());
            }
        }
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
            let _ = self
                .heard
                .lock()
                .expect("lock")
                .send((self.label.to_owned(), name, us));
            Ok(())
        }
    }
}

