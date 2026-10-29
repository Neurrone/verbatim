//! Experiments of the cross-process calls study (2026-10-29), not tests:
//! `docs/design/cross-process-calls-study.md`. One scratch program, kept in
//! `experiments/cross-process-calls-study-program/` and not built by the
//! workspace. `XP_EXP` names the experiment; `XP_ROLE=helper` runs this
//! binary as a helper UIA client process driven over its stdin. Every
//! result line starts `RESULT` or `SUMMARY` and goes to stdout.

mod common;

use std::io::{BufRead as _, BufReader, Write as _};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::outpost::OutpostUnderTest;
use verbatim_outpost::focus_read::{FocusEvent, Judged};
use verbatim_outpost::protocol::{DeliveredFact, OutpostToSupervisor, Query};
use verbatim_uia::{
    Ancestor, Cached, Fill, FocusInput, ListenerEvent, Node, NodeGates, Part, Registration, Scope,
    Shape, Subscription, Uia,
};
use verbatim_uia_rops::{Builder, FocusQuery, focus_ancestry_classic};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, UIA_MenuOpenedEventId, UIA_NamePropertyId,
    UIA_ProviderDescriptionPropertyId, UIA_SelectionItem_ElementSelectedEventId,
};
use windows::core::AgileReference;

const ROLE: &str = "XP_ROLE";

fn say(line: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}

fn ms(since: Instant) -> String {
    format!("{:.1}", since.elapsed().as_secs_f64() * 1000.0)
}

/// The outpost's node request: `Context::node_request` with every detail on,
/// as `experiments/getfocusedelement-after-the-focus-moves/` built it.
fn node_request(uia: &Uia) -> verbatim_uia::Request<Node> {
    uia.request::<Node>(NodeGates {
        role: (),
        value: (),
        states: (),
        description: Some(()),
        shortcut: Some(()),
        position: Some(()),
        place: Some(()),
        cell: Some(()),
        level: true,
    })
    .expect("the outpost's node request")
}

/// Every process id a provider description names.
fn described_pids(description: &str) -> Vec<u32> {
    description
        .split("[pid:")
        .skip(1)
        .filter_map(|rest| rest.split(',').next()?.trim().parse().ok())
        .collect()
}

/// The outpost's lookup by handle, `window_element` with the node request
/// (as `focus_path::find` and a focus-now read make it), described.
fn outpost_lookup(uia: &Uia, hwnd: isize) -> String {
    let request = node_request(uia);
    match uia.window_element(hwnd, &request) {
        Ok(element) => {
            let description = element
                .observe_cached(UIA_ProviderDescriptionPropertyId, false)
                .ok()
                .and_then(|value| windows::core::BSTR::try_from(&value).ok())
                .map(|text| text.to_string())
                .unwrap_or_default();
            format!(
                "own name={:?} pids={:?}",
                element.read().name,
                described_pids(&description)
            )
        }
        Err(error) => format!("refused-or-failed {:?}", error.message()),
    }
}

/// One remote operation reading `element`'s name, the collide tool's
/// operation (`experiments/the-listeners-read-below-another-processs-window/`).
fn name_op(element: &IUIAutomationElement) -> Result<String, String> {
    let mut b = Builder::new();
    let e = b.import_element(element);
    let name = b.property(e, UIA_NamePropertyId.0);
    let s = b.stringify(name);
    let s = b.add_to_results(s);
    let outcome = b.finish().execute().map_err(|e| format!("{e:?}"))?;
    outcome.get(s).map_err(|e| format!("{e:?}"))
}

fn terminate() -> ! {
    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    // SAFETY: ends this process, as the harness ends its own, so UIA's
    // detach code never runs (`docs/uia-bugs.md`).
    unsafe {
        let _ = TerminateProcess(GetCurrentProcess(), 0);
    }
    std::process::exit(0)
}

/// The helper: commands on stdin, answers on stdout. Calls that wait run
/// on threads of their own, each with its own client, so the helper keeps
/// taking commands.
fn run_helper() -> ! {
    let _ = verbatim_uia::init_mta();
    let uia = Uia::new().expect("the helper's client");
    let mut grabbed: Option<AgileReference<IUIAutomationElement>> = None;
    let mut registrations = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("lookup") => {
                let hwnd: isize = parts.next().unwrap().parse().unwrap();
                let tag = parts.next().unwrap_or("lookup").to_owned();
                std::thread::spawn(move || {
                    let _ = verbatim_uia::init_mta();
                    let uia = Uia::new().expect("a client");
                    let started = Instant::now();
                    let outcome = outpost_lookup(&uia, hwnd);
                    say(&format!("{tag} done {} {outcome}", ms(started)));
                });
            }
            Some("grab") => {
                let hwnd: isize = parts.next().unwrap().parse().unwrap();
                let live = uia.window_live(hwnd).expect("the grab");
                let first = name_op(live.element());
                grabbed = Some(AgileReference::new(live.element()).unwrap());
                say(&format!("grabbed {first:?}"));
            }
            Some(command @ ("read" | "op")) => {
                let agile = grabbed.clone().expect("an element grabbed");
                let op = command == "op";
                std::thread::spawn(move || {
                    let _ = verbatim_uia::init_mta();
                    let element = agile.resolve().unwrap();
                    let started = Instant::now();
                    let result = if op {
                        name_op(&element)
                    } else {
                        // SAFETY: a live element.
                        unsafe { element.CurrentName() }
                            .map(|name| name.to_string())
                            .map_err(|error| error.message())
                    };
                    let tag = if op { "op" } else { "read" };
                    say(&format!("{tag} done {} {result:?}", ms(started)));
                });
                say(&format!("{command} sent"));
            }
            Some("notify") => {
                use windows::Win32::UI::Accessibility::NotifyWinEvent;
                let event: u32 = parts.next().unwrap().parse().unwrap();
                let hwnd: isize = parts.next().unwrap().parse().unwrap();
                let object: i32 = parts.next().unwrap().parse().unwrap();
                let child: i32 = parts.next().unwrap().parse().unwrap();
                // SAFETY: raises a WinEvent; no pointers.
                unsafe { NotifyWinEvent(event, HWND(hwnd as *mut _), object, child) };
                say(&format!(
                    "notified thread={}",
                    // SAFETY: no preconditions.
                    unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }
                ));
            }
            Some("register") => {
                // The listener's desktop-wide registration's shape
                // (`listener.rs`, `subscriptions`): focus, selection and
                // menu opening, notifications, one `Registration` over the
                // desktop, on the process's registering thread.
                let focus = Subscription::Focus {
                    callback: Arc::new(|_: &Cached<ListenerEvent>| {}),
                };
                let selection = Subscription::Event {
                    event: UIA_SelectionItem_ElementSelectedEventId,
                    callback: Arc::new(|_: &Cached<ListenerEvent>| {}),
                };
                let menu = Subscription::Event {
                    event: UIA_MenuOpenedEventId,
                    callback: Arc::new(|_: &Cached<ListenerEvent>| {}),
                };
                let notifications = Subscription::Notifications {
                    callback: Arc::new(
                        |_: &Cached<ListenerEvent>,
                         _: windows::Win32::UI::Accessibility::NotificationKind,
                         _: windows::Win32::UI::Accessibility::NotificationProcessing,
                         _: Option<String>,
                         _: Option<String>| {},
                    ),
                };
                let parts = [focus, selection, menu, notifications]
                    .into_iter()
                    .map(Part::shaped::<ListenerEvent>)
                    .collect();
                let started = Instant::now();
                let result = Registration::with_parts(parts, Scope::Desktop);
                say(&format!("registered {} ok={}", ms(started), result.is_ok()));
                if let Ok(registration) = result {
                    registrations.push(registration);
                }
            }
            Some("quit") | None => break,
            Some(other) => say(&format!("unknown {other}")),
        }
    }
    terminate()
}

/// This binary run again as a helper.
struct Helper {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl Helper {
    fn spawn() -> Self {
        common::contain_children();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .env(ROLE, "helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("a helper");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if tx.send(line.trim().to_owned()).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The helper's next line, which must start with `prefix`.
    fn expect(&self, prefix: &str, timeout: Duration) -> String {
        let line = self
            .lines
            .recv_timeout(timeout)
            .unwrap_or_else(|_| panic!("the helper did not say {prefix:?} within {timeout:?}"));
        assert!(
            line.starts_with(prefix),
            "the helper said {line:?}, not {prefix:?}"
        );
        line
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn(fixture: &str, backend: &str, label: &str) -> (common::MockApp, HWND) {
    let title = common::unique_title(&format!("xproc-{label}"));
    let app = common::spawn(fixture, backend, &title);
    let hwnd = common::find_window(&title);
    (app, hwnd)
}

fn set_parent(child: HWND, parent: HWND) {
    use windows::Win32::UI::WindowsAndMessaging::SetParent;
    // SAFETY: two live windows of other processes.
    unsafe { SetParent(child, Some(parent)) }.expect("SetParent");
}

/// Provider calls and `WM_GETOBJECT`s, by name, one window's since its
/// counters were reset.
fn counted(hwnd: HWND) -> String {
    let mut all: Vec<String> = common::read_hits(hwnd)
        .into_iter()
        .map(|(name, n)| format!("{name}={n}"))
        .collect();
    all.extend(
        common::read_get_objects(hwnd)
            .into_iter()
            .map(|(name, n)| format!("WM_GETOBJECT({name})={n}")),
    );
    if all.is_empty() {
        "none".to_owned()
    } else {
        all.join(",")
    }
}

fn trials() -> usize {
    std::env::var("XP_TRIALS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(6)
}

// ---------------------------------------------------------------------
// second_form: two processes' new connections waiting in a busy
// application while a third process's call is pending, and the same with
// the listener's read below the application's window as one of them.
// ---------------------------------------------------------------------

fn exp_second_form() {
    let (p_app, p) = spawn("small.json", "uia", "secondform-parent");
    let (mut c_app, c) = spawn("counts.json", "uia", "secondform-child");
    // Made while the child is still top-level, as the boundary test makes it.
    let outpost = OutpostUnderTest::new(&c_app);
    set_parent(c, p);
    c_app.send("set-focus first");
    let p_app = Arc::new(Mutex::new(p_app));
    let mut x = Helper::spawn();
    let mut b = Helper::spawn();
    let mut d = Helper::spawn();
    x.send(&format!("grab {}", p.0 as isize));
    x.expect("grabbed", Duration::from_secs(10));
    let uia = Uia::new().unwrap();
    let focused = common::listener_focus(&uia, c.0 as isize);
    say(&format!(
        "RESULT setup p_pid={} c_pid={} x={} b={} d={}",
        p_app.lock().unwrap().pid(),
        c_app.pid(),
        x.pid(),
        b.pid(),
        d.pid()
    ));
    let arms = std::env::var("XP_ARMS").unwrap_or_else(|_| {
        "two-lookups,lookup-listener,one-lookup,listener-only,two-lookups-stalled,free"
            .to_owned()
    });
    for arm in arms.split(',') {
        for n in 0..trials() {
            // Every arm but the stalled one and the free one has a third
            // process's call pending: X's plain read, held in P's provider.
            let pending = !matches!(arm, "two-lookups-stalled" | "free");
            let stalled = arm == "two-lookups-stalled";
            let busy = pending;
            if busy {
                let mut p_app = p_app.lock().unwrap();
                p_app.hold();
                x.send("read");
                x.expect("read sent", Duration::from_secs(5));
                let held = p_app.held();
                assert_eq!(held, "GetPropertyValue", "the call held");
            }
            if stalled {
                p_app.lock().unwrap().stall(Duration::from_millis(300));
            }
            let started = Instant::now();
            let lookups = match arm {
                "two-lookups" | "two-lookups-stalled" => 2,
                "lookup-listener" | "one-lookup" | "free" => 1,
                _ => 0,
            };
            if lookups >= 1 {
                b.send(&format!("lookup {} b", p.0 as isize));
            }
            if lookups >= 2 {
                d.send(&format!("lookup {} d", p.0 as isize));
            }
            // The release, 300 ms after the connections are sent: mockapp
            // shows no evidence of a request reaching its held thread.
            let releaser = busy.then(|| {
                let p_app = Arc::clone(&p_app);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(300));
                    p_app.lock().unwrap().release();
                    Instant::now()
                })
            });
            let listener = if matches!(arm, "lookup-listener" | "listener-only" | "free") {
                let read_started = Instant::now();
                let (judged, calls) = outpost.listener_read(&focused);
                let outcome = match &judged {
                    Judged::Read(read) => format!(
                        "read chain={:?}",
                        read.ancestors.as_ref().map(|chain| chain
                            .iter()
                            .map(|a| a.snapshot.name.clone())
                            .collect::<Vec<_>>())
                    ),
                    Judged::Dropped(why) => format!("dropped {why:?}"),
                    other => format!("{other:?}"),
                };
                format!(
                    "listener={}ms uia_calls={} {outcome}",
                    ms(read_started),
                    calls.uia
                )
            } else {
                "listener=none".to_owned()
            };
            let released = releaser.map(|thread| thread.join().unwrap());
            if stalled {
                p_app.lock().unwrap().stall_ended(Duration::from_millis(300));
            }
            let released_at = released.map_or("free".to_owned(), |at| {
                format!("{:.1}", (at - started).as_secs_f64() * 1000.0)
            });
            let mut answers = Vec::new();
            for (helper, tag) in [(&b, "b"), (&d, "d")].into_iter().take(lookups) {
                answers.push(helper.expect(tag, Duration::from_secs(15)));
            }
            if pending {
                answers.push(x.expect("read done", Duration::from_secs(15)));
            }
            say(&format!(
                "RESULT exp=second_form arm={arm} n={n} released_ms={released_at} {listener} answers={answers:?}"
            ));
        }
    }
    drop(outpost);
    c_app.quit();
    if let Ok(p_app) = Arc::try_unwrap(p_app) {
        p_app.into_inner().unwrap().quit();
    }
}

// ---------------------------------------------------------------------
// retire: the old outpost's remote operation queued in a busy application
// while its replacement makes its lookup by handle (`docs/uia-bugs.md`'s
// first entry, with the outposts' own calls).
// ---------------------------------------------------------------------

fn exp_retire() {
    let (mut p_app, p) = spawn("counts.json", "uia", "retire");
    let mut old = Helper::spawn();
    let mut new = Helper::spawn();
    old.send(&format!("grab {}", p.0 as isize));
    old.expect("grabbed", Duration::from_secs(10));
    for arm in ["queued", "busy-only"] {
        for n in 0..trials() {
            let stall = Duration::from_millis(1000);
            p_app.stall(stall);
            let started = Instant::now();
            if arm == "queued" {
                old.send("op");
                old.expect("op sent", Duration::from_secs(5));
                // No evidence of arrival; the operation is sent first.
                std::thread::sleep(Duration::from_millis(50));
            }
            new.send(&format!("lookup {} new", p.0 as isize));
            let ended = p_app.stall_ended(stall);
            let answer = new.expect("new", Duration::from_secs(15));
            let op = (arm == "queued").then(|| old.expect("op done", Duration::from_secs(15)));
            say(&format!(
                "RESULT exp=retire arm={arm} n={n} stall_ended_us={ended} since_stall_ms={} new={answer:?} old={op:?}",
                ms(started)
            ));
        }
    }
    p_app.quit();
}

// ---------------------------------------------------------------------
// search: the runtime-id search over a top-level window's subtree, and the
// tree dump, when the window has another process's child window.
// ---------------------------------------------------------------------

fn exp_search() {
    let parent = std::env::var("XP_PARENT").unwrap_or_else(|_| "counts.json:uia".to_owned());
    let (fixture, backend) = parent.split_once(':').unwrap();
    let p_title = common::unique_title("xproc-search-parent");
    let mut p_app = common::spawn_shown(fixture, backend, &p_title);
    let p = common::find_window(&p_title);
    let c_title = common::unique_title("xproc-search-child");
    let mut c_app = common::spawn_shown("tree.json", "uia", &c_title);
    let c = common::find_window(&c_title);
    say(&format!("RESULT exp=search setup parent={parent}"));
    let uia = Uia::new().unwrap();
    let request = node_request(&uia);
    for arm in ["separate", "child", "child-stalled"] {
        if arm == "child" {
            set_parent(c, p);
        }
        for n in 0..trials() {
            common::reset_hits(p);
            common::reset_hits(c);
            // The outpost's search (`read.rs`, `resolve_uia_element`): the
            // top-level window's element, then `FindFirstBuildCache` over
            // its subtree for a runtime id no element has, the search for an
            // element that is gone.
            let stall = Duration::from_millis(2000);
            if arm == "child-stalled" {
                c_app.stall(stall);
            }
            let started = Instant::now();
            let root = uia.window_live(p.0 as isize).unwrap();
            let found = uia.find_by_runtime_id(&root, &[42, 1, 2, 3], &request);
            let took = ms(started);
            if arm == "child-stalled" {
                c_app.stall_ended(stall);
            }
            say(&format!(
                "RESULT exp=search arm={arm} n={n} took_ms={took} found={:?} p=[{}] c=[{}]",
                found.as_ref().map(Option::is_some).map_err(|e| e.message()),
                counted(p),
                counted(c)
            ));
        }
    }
    // The product path: a node whose element is gone, asked for its
    // ancestors, is searched for (`resolve_uia_element`).
    let mut outpost = OutpostUnderTest::new(&p_app);
    let uia_parent = backend == "uia";
    let node = if uia_parent {
        p_app.send("set-focus first");
        let focused = common::listener_focus(&uia, p.0 as isize);
        let (judged, _) = outpost.listener_read(&focused);
        let reported = outpost.focus(judged.into_fact(0, FocusEvent::Focus));
        p_app.send("disconnect first");
        Some(reported.node.id)
    } else {
        None
    };
    for n in 0..trials() {
        let Some(node) = node else { break };
        common::reset_hits(p);
        common::reset_hits(c);
        let started = Instant::now();
        let id = outpost.ask(Query::Ancestors { node_id: node });
        let reply = outpost.next();
        say(&format!(
            "RESULT exp=search arm=ancestors-query n={n} took_ms={} reply={} p=[{}] c=[{}]",
            ms(started),
            short_reply(&reply, id),
            counted(p),
            counted(c)
        ));
    }
    // The tree dump for `verbatim-inspect`.
    for n in 0..trials().min(3) {
        common::reset_hits(p);
        common::reset_hits(c);
        let started = Instant::now();
        let id = outpost.ask(Query::DumpTree { locations: false });
        let reply = outpost.next();
        say(&format!(
            "RESULT exp=search arm=dump-tree n={n} took_ms={} reply={} p=[{}] c=[{}]",
            ms(started),
            short_reply(&reply, id),
            counted(p),
            counted(c)
        ));
    }
    drop(outpost);
    c_app.quit();
    p_app.quit();
}

fn short_reply(reply: &OutpostToSupervisor, id: u64) -> String {
    match reply {
        OutpostToSupervisor::Reply {
            request_id,
            outcome,
            ..
        } if *request_id == id => {
            let text = format!("{outcome:?}");
            text.chars().take(160).collect()
        }
        other => format!("unexpected {}", format!("{other:?}").chars().take(160).collect::<String>()),
    }
}

// ---------------------------------------------------------------------
// hook: a WinEvent raised by one process naming another process's window.
// ---------------------------------------------------------------------

static HOOKED: Mutex<Vec<(u32, isize, i32, u32)>> = Mutex::new(Vec::new());

unsafe extern "system" fn record(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    _child: i32,
    thread: u32,
    _time: u32,
) {
    HOOKED
        .lock()
        .unwrap()
        .push((event, hwnd.0 as isize, object, thread));
}

fn pump_for(duration: Duration) {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MsgWaitForMultipleObjectsEx, MWMO_INPUTAVAILABLE, PM_REMOVE,
        PeekMessageW, QS_ALLINPUT, TranslateMessage,
    };
    let until = Instant::now() + duration;
    let mut msg = MSG::default();
    while Instant::now() < until {
        // SAFETY: `msg` is a local the call writes.
        while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
            // SAFETY: the message just retrieved.
            let _ = unsafe { TranslateMessage(&raw const msg) };
            // SAFETY: as above.
            unsafe { DispatchMessageW(&raw const msg) };
        }
        let left = u32::try_from(until.saturating_duration_since(Instant::now()).as_millis())
            .unwrap_or(0);
        // SAFETY: no handles.
        let _ = unsafe { MsgWaitForMultipleObjectsEx(None, left, QS_ALLINPUT, MWMO_INPUTAVAILABLE) };
    }
}

fn exp_hook() {
    use windows::Win32::UI::Accessibility::SetWinEventHook;
    use windows::Win32::UI::WindowsAndMessaging::{
        EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE, OBJID_CLIENT, WINEVENT_OUTOFCONTEXT,
    };
    let (a_app, a) = spawn("small.json", "msaa", "hook-a");
    let (b_app, b) = spawn("small.json", "msaa", "hook-b");
    let mut raiser = Helper::spawn();
    // Part 1: which process-scoped hooks receive an event the helper
    // raises naming B's window.
    for (label, pid) in [("raiser", raiser.pid()), ("window-owner", b_app.pid())] {
        HOOKED.lock().unwrap().clear();
        // SAFETY: an out-of-context hook with a callback that only records.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_FOCUS,
                EVENT_OBJECT_NAMECHANGE,
                None,
                Some(record),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        for event in [EVENT_OBJECT_FOCUS, EVENT_OBJECT_NAMECHANGE] {
            raiser.send(&format!(
                "notify {event} {} {} 0",
                b.0 as isize,
                OBJID_CLIENT.0
            ));
            let line = raiser.expect("notified", Duration::from_secs(5));
            pump_for(Duration::from_millis(300));
            say(&format!("RESULT exp=hook part=scope hook_scoped_to={label} event={event:#x} {line}"));
        }
        say(&format!(
            "RESULT exp=hook part=scope hook_scoped_to={label} received={:?}",
            HOOKED.lock().unwrap()
        ));
        // SAFETY: the hook just set.
        let _ = unsafe { windows::Win32::UI::Accessibility::UnhookWinEvent(hook) };
    }
    // Part 2: A's outpost handed an MSAA focus naming B's window, as its own
    // hook would deliver the event the helper raised in A's name.
    let outpost = OutpostUnderTest::new(&a_app);
    for (arm, front) in [("a-in-front", a), ("b-in-front", b)] {
        outpost.set_foreground(front);
        for n in 0..trials() {
            common::reset_hits(a);
            common::reset_hits(b);
            let started = Instant::now();
            outpost.deliver(DeliveredFact::MsaaFocus {
                hwnd: b.0 as isize,
                id_object: 1,
                id_child: 0,
            });
            outpost.settle();
            let took = ms(started);
            let mut said = Vec::new();
            while let Ok(message) = outpost.messages.try_recv() {
                let text = format!("{message:?}");
                let name = text
                    .split("name: Some(\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .unwrap_or("?")
                    .to_owned();
                let kind: String = text.chars().take(60).collect();
                said.push(format!("{kind}... first_name={name:?}"));
            }
            say(&format!(
                "RESULT exp=hook part=outpost arm={arm} n={n} took_ms={took} said={said:?} a=[{}] b=[{}]",
                counted(a),
                counted(b)
            ));
        }
    }
    drop(outpost);
    a_app.quit();
    b_app.quit();
}

// ---------------------------------------------------------------------
// walk_race: the window hierarchy changed between the walk's boundary
// check and its step.
// ---------------------------------------------------------------------

fn exp_walk_race() {
    use windows::Win32::UI::WindowsAndMessaging::{
        GWLP_HWNDPARENT, SW_HIDE, SW_SHOWNOACTIVATE, SetWindowLongPtrW, ShowWindow,
    };
    let uia = Uia::new().unwrap();
    for arm in ["control", "reparented", "reparented-busy", "owner-shown"] {
        for n in 0..trials() {
            let (p_app, p) = spawn("small.json", "uia", "race-parent");
            let (mut c_app, c) = spawn("counts.json", "uia", "race-child");
            if arm == "owner-shown" {
                // SAFETY: two live windows of other processes; P is hidden.
                unsafe { SetWindowLongPtrW(c, GWLP_HWNDPARENT, p.0 as isize) };
                // SAFETY: as above.
                let _ = unsafe { ShowWindow(p, SW_HIDE) };
            }
            c_app.send("set-focus first");
            let sender = common::listener_focus(&uia, c.0 as isize);
            let input = sender.narrow::<FocusInput>();
            let request = uia.request::<Ancestor>(common::ancestor_gates()).unwrap();
            let element_request = uia.request::<Ancestor>(Ancestor::all_on()).unwrap();
            let fill = Fill::new(&request);
            let element_fill = Fill::new(&element_request);
            let p_app = std::cell::RefCell::new(p_app);
            let steps = std::cell::Cell::new(0u32);
            let changed_at = std::cell::Cell::new(None::<Instant>);
            // The step from the child's window's own element is the third:
            // the focus to `Settings`, `Settings` to the window, then up.
            let before_call = |call: &'static str| {
                if call != "a step to the parent" {
                    return;
                }
                steps.set(steps.get() + 1);
                if steps.get() != 3 {
                    return;
                }
                match arm {
                    "reparented" | "reparented-busy" => set_parent(c, p),
                    "owner-shown" => {
                        // SAFETY: a live window of another process.
                        let _ = unsafe { ShowWindow(p, SW_SHOWNOACTIVATE) };
                    }
                    _ => {}
                }
                common::reset_hits(p);
                if arm == "reparented-busy" {
                    p_app.borrow_mut().stall(Duration::from_millis(1000));
                }
                changed_at.set(Some(Instant::now()));
            };
            let query = FocusQuery {
                element: &input,
                known: &[],
                previous: None,
                depth_limit: 64,
                fill: &fill,
                element_fill: &element_fill,
                out_of_time: None,
                require_focus: false,
                refresh: true,
                nearest_window: Some(c.0 as isize),
                before_call: Some(&before_call),
                value_available: None,
            };
            let started = Instant::now();
            let read = focus_ancestry_classic(&uia, &query);
            let after_change = changed_at.get().map_or("none".to_owned(), ms);
            let took = ms(started);
            if arm == "reparented-busy" {
                p_app.borrow_mut().stall_ended(Duration::from_millis(1000));
            }
            let outcome = match &read {
                Ok(verbatim_uia_rops::FocusAncestry::Focused(ancestry)) => format!(
                    "chain={:?} unknown={}",
                    ancestry
                        .ancestors
                        .iter()
                        .map(|a| a.read().node.name)
                        .collect::<Vec<_>>(),
                    ancestry.ancestors_unknown()
                ),
                Ok(other) => format!("{other:?}"),
                Err(error) => format!("error {error}"),
            };
            say(&format!(
                "RESULT exp=walk_race arm={arm} n={n} steps={} took_ms={took} after_change_ms={after_change} {outcome} p=[{}]",
                steps.get(),
                counted(p)
            ));
            c_app.quit();
            p_app.into_inner().quit();
        }
    }
}

// ---------------------------------------------------------------------
// acc_focus: an MSAA focus read whose `accFocus` names another process's
// child window, the keyboard focus moved into it after the check.
// ---------------------------------------------------------------------

fn exp_acc_focus() {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VT_I4};
    use windows::Win32::UI::Accessibility::{
        AccessibleObjectFromWindow, IAccessible, WindowFromAccessibleObject,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, OBJID_CLIENT,
        SW_RESTORE, SW_SHOWMINNOACTIVE, SetForegroundWindow, ShowWindow,
    };
    common::init_com();
    let focus_window = || {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap(),
            ..Default::default()
        };
        // SAFETY: `info` has its size set.
        unsafe { GetGUIThreadInfo(0, &raw mut info) }.ok()?;
        let window = if info.hwndFocus.0.is_null() {
            info.hwndActive
        } else {
            info.hwndFocus
        };
        Some(window)
    };
    let title = common::unique_title("xproc-accfocus-parent");
    let p_app = common::spawn_shown("boundary_win32_host.json", "win32", &title);
    let p = common::find_window(&title);
    let (c_app, c) = spawn("small.json", "msaa", "accfocus-child");
    set_parent(c, p);
    // SAFETY: a live window.
    let _ = unsafe { ShowWindow(c, windows::Win32::UI::WindowsAndMessaging::SW_SHOW) };
    let mut p_pid = 0;
    // SAFETY: a live window, a local out-parameter.
    let p_thread = unsafe { GetWindowThreadProcessId(p, Some(&raw mut p_pid)) };
    // SAFETY: attaches this thread's input to P's, so it can set the focus
    // in P's and C's windows, whose input is attached by `SetParent`.
    let attached = unsafe { AttachThreadInput(GetCurrentThreadId(), p_thread, true) }.as_bool();
    // SAFETY: a live window; no out-parameter.
    let c_thread = unsafe { GetWindowThreadProcessId(c, None) };
    // SAFETY: as above, for C's thread.
    let attached_c = unsafe { AttachThreadInput(GetCurrentThreadId(), c_thread, true) }.as_bool();
    say(&format!("RESULT exp=acc_focus attached_c={attached_c}"));
    say(&format!("RESULT exp=acc_focus setup p_pid={p_pid} c_pid={} attached={attached}", c_app.pid()));
    // P's own child window, on P's thread ("Embedded").
    // SAFETY: a live window; finds its child of mockapp's UIA window class.
    let own_child = unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowExW(
            Some(p),
            None,
            windows::core::w!("VerbatimMockAppUiaWindow"),
            None,
        )
    }
    .expect("P's own child window");
    for arm in ["moved", "moved-own-child", "control"] {
        for n in 0..trials() {
            // P to the front, its own window focused.
            // SAFETY: live windows.
            unsafe {
                let _ = ShowWindow(p, SW_SHOWMINNOACTIVE);
                let _ = ShowWindow(p, SW_RESTORE);
                let _ = SetForegroundWindow(p);
                let _ = SetFocus(Some(p));
            }
            pump_for(Duration::from_millis(200));
            // The check `focused_snapshot` makes (`acquire.rs` 1204 to
            // 1209): the focus window and its owner.
            let checked = focus_window();
            let mut owner = 0;
            if let Some(window) = checked {
                // SAFETY: a window handle and a local out-parameter.
                unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
            }
            // SAFETY: no preconditions.
            let front = unsafe { GetForegroundWindow() } == p;
            let mut client: Option<IAccessible> = None;
            // SAFETY: an out-pointer for the interface.
            let acquired = unsafe {
                AccessibleObjectFromWindow(
                    p,
                    OBJID_CLIENT.0 as u32,
                    &<IAccessible as windows::core::Interface>::IID,
                    std::ptr::from_mut(&mut client).cast(),
                )
            };
            if arm.starts_with("moved") {
                let target = if arm == "moved" { c } else { own_child };
                // SAFETY: the window shares this thread's input now.
                let previous = unsafe { SetFocus(Some(target)) };
                say(&format!(
                    "RESULT exp=acc_focus set_focus_previous={previous:?} error={:?}",
                    windows::core::Error::from_thread().message()
                ));
            }
            let after = focus_window();
            common::reset_hits(c);
            let started = Instant::now();
            let answer = client.as_ref().map(|client| {
                // SAFETY: a live IAccessible.
                let focus: VARIANT = unsafe { client.accFocus() }.unwrap_or_default();
                let vt = focus.vt().0;
                if vt == VT_DISPATCH.0 {
                    let object: Option<IAccessible> = windows::core::IUnknown::try_from(&focus)
                        .ok()
                        .and_then(|unknown| windows::core::Interface::cast(&unknown).ok());
                    let window = object.as_ref().and_then(|object| {
                        let mut hwnd = HWND::default();
                        // SAFETY: a live object and a local out-parameter.
                        unsafe { WindowFromAccessibleObject(object, Some(&raw mut hwnd)) }.ok()?;
                        Some(hwnd)
                    });
                    let name = object.as_ref().and_then(|object| {
                        // SAFETY: a live object.
                        unsafe { object.get_accName(&VARIANT::from(0i32)) }.ok()
                    });
                    format!(
                        "object window_is_c={} window_is_own_child={} name={:?}",
                        window == Some(c),
                        window == Some(own_child),
                        name.map(|n| n.to_string())
                    )
                } else if vt == VT_I4.0 {
                    format!("child {}", i32::try_from(&focus).unwrap_or(-99))
                } else {
                    format!("vt {vt}")
                }
            });
            say(&format!(
                "RESULT exp=acc_focus arm={arm} n={n} checked_is_p={} owner_is_p={} front={front} acquired={} after_is_c={} after_is_own_child={} took_ms={} answer={answer:?} c=[{}]",
                checked == Some(p),
                owner == p_pid,
                acquired.is_ok(),
                after == Some(c),
                after == Some(own_child),
                ms(started),
                counted(c)
            ));
        }
    }
    c_app.quit();
    p_app.quit();
}

// ---------------------------------------------------------------------
// register: the listener's desktop-wide registration while an
// application is stalled.
// ---------------------------------------------------------------------

fn exp_register() {
    use windows::Win32::UI::WindowsAndMessaging::{
        SW_RESTORE, SW_SHOWMINNOACTIVE, SetForegroundWindow, ShowWindow,
    };
    let arms = std::env::var("XP_ARMS").unwrap_or_default();
    for (arm, backend, front, stalled) in [
        ("uia-front-stalled", "uia", true, true),
        ("uia-hidden-stalled", "uia", false, true),
        ("uia-front-free", "uia", true, false),
        ("msaa-front-stalled", "msaa", true, true),
    ] {
        if !arms.is_empty() && !arms.split(',').any(|a| a == arm) {
            continue;
        }
        for n in 0..trials().min(3) {
            let title = common::unique_title("xproc-register");
            let mut app = if front {
                common::spawn_shown("counts.json", backend, &title)
            } else {
                common::spawn("counts.json", backend, &title)
            };
            let hwnd = common::find_window(&title);
            if front {
                // SAFETY: a live window.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                pump_for(Duration::from_millis(300));
            }
            // SAFETY: no preconditions.
            let in_front =
                unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } == hwnd;
            let mut helper = Helper::spawn();
            common::reset_hits(hwnd);
            let stall = Duration::from_millis(
                std::env::var("XP_STALL_MS")
                    .ok()
                    .and_then(|ms| ms.parse().ok())
                    .unwrap_or(3000),
            );
            if stalled {
                app.stall(stall);
            }
            let started = Instant::now();
            helper.send("register");
            let line = helper.expect("registered", Duration::from_secs(30));
            let since = ms(started);
            if stalled {
                app.stall_ended(stall);
            }
            say(&format!(
                "RESULT exp=register arm={arm} n={n} in_front={in_front} since_stall_ms={since} {line} app=[{}]",
                counted(hwnd)
            ));
            drop(helper);
            app.quit();
        }
    }
}

// ---------------------------------------------------------------------
// compare: the listener's repeat comparison of a sender with the focus
// Core holds, when that focus is another application's element.
// ---------------------------------------------------------------------

fn exp_compare() {
    let (mut a_app, a) = spawn("counts.json", "uia", "compare-a");
    let (mut b_app, b) = spawn("counts.json", "uia", "compare-b");
    a_app.send("set-focus first");
    b_app.send("set-focus first");
    let uia = Uia::new().unwrap();
    let held_a = common::listener_focus(&uia, a.0 as isize);
    let sender_b = common::listener_focus(&uia, b.0 as isize);
    for arm in ["free", "a-stalled"] {
        for n in 0..trials() {
            common::reset_hits(a);
            common::reset_hits(b);
            let stall = Duration::from_millis(1500);
            if arm == "a-stalled" {
                a_app.stall(stall);
            }
            let started = Instant::now();
            let same = uia.compare_elements(sender_b.live().element(), held_a.live().element());
            let took = ms(started);
            if arm == "a-stalled" {
                a_app.stall_ended(stall);
            }
            say(&format!(
                "RESULT exp=compare arm={arm} n={n} took_ms={took} same={same:?} a=[{}] b=[{}]",
                counted(a),
                counted(b)
            ));
        }
    }
    a_app.quit();
    b_app.quit();
}

// ---------------------------------------------------------------------
// msaa_onward: the outpost's MSAA continuation above the process boundary
// when the parent window's application is busy.
// ---------------------------------------------------------------------

fn exp_msaa_onward() {
    let uia = Uia::new().unwrap();
    let arms = std::env::var("XP_ARMS").unwrap_or_default();
    for arm in ["free", "parent-stalled-4s", "parent-stalled-12s"] {
        if !arms.is_empty() && !arms.split(',').any(|a| a == arm) {
            continue;
        }
        for n in 0..trials().min(3) {
            let (mut p_app, p) = spawn("small.json", "msaa", "onward-parent");
            let (mut c_app, c) = spawn("counts.json", "uia", "onward-child");
            let outpost = OutpostUnderTest::new(&c_app);
            set_parent(c, p);
            c_app.send("set-focus first");
            let focused = common::listener_focus(&uia, c.0 as isize);
            let (judged, _) = outpost.listener_read(&focused);
            let stall = match arm {
                "parent-stalled-4s" => Some(Duration::from_millis(4000)),
                "parent-stalled-12s" => Some(Duration::from_millis(12000)),
                _ => None,
            };
            if let Some(stall) = stall {
                p_app.stall(stall);
            }
            let started = Instant::now();
            outpost.deliver(judged.into_fact(0, FocusEvent::Focus));
            let said = outpost.messages.recv_timeout(Duration::from_secs(30));
            let took = ms(started);
            let text = format!("{said:?}");
            let names: Vec<&str> = text
                .split("name: Some(\"")
                .skip(1)
                .filter_map(|rest| rest.split('"').next())
                .collect();
            if let Some(stall) = stall {
                p_app.stall_ended(stall);
            }
            say(&format!(
                "RESULT exp=msaa_onward arm={arm} n={n} reported_after_ms={took} names={names:?} kind={}",
                text.chars().take(40).collect::<String>()
            ));
            outpost.settle();
            while outpost.messages.try_recv().is_ok() {}
            drop(outpost);
            c_app.quit();
            p_app.quit();
        }
    }
}

fn main() {
    if std::env::var(ROLE).is_ok() {
        run_helper();
    }
    common::init_com();
    let exp = std::env::var("XP_EXP").expect("XP_EXP names the experiment");
    match exp.as_str() {
        "second_form" => exp_second_form(),
        "retire" => exp_retire(),
        "search" => exp_search(),
        "hook" => exp_hook(),
        "walk_race" => exp_walk_race(),
        "acc_focus" => exp_acc_focus(),
        "register" => exp_register(),
        "compare" => exp_compare(),
        "msaa_onward" => exp_msaa_onward(),
        other => panic!("no experiment {other}"),
    }
    say("SUMMARY done");
    terminate()
}
