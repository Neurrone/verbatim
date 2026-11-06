//! A client process's events run on one worker
//! (`uia-facts/a-client-processs-events-run-on-one-worker/readme.md`).
//!
//! The client is this process, its client objects from support's
//! `uia::client()`. The provider program holds B1 and B2, UIA windows on
//! UI threads of their own, and C, a third, whose provider calls can be
//! held. An event is a name change of a window's node 1, renamed so that
//! the handler can tell events apart. Every handler registers name changes
//! on its window's subtree with the name cached.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::events::{self, Handler, Heard, OnEvent};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, deterministic, relation, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::us_to_ms;
use uia_facts_support::{note, now_us, uia};
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenThread, THREAD_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationElement, TreeScope_Subtree, UIA_NamePropertyId,
};

/// Every handler's report receiver, by the handler.
type Reports = std::sync::mpsc::Receiver<events::Report>;

/// Registers name changes on window `hwnd`'s subtree on `client`, with a
/// handler that runs `on_event` first; answers its reports.
fn register(client: &IUIAutomation6, hwnd: isize, on_event: Option<OnEvent>) -> Reports {
    let (handler, reports) = Handler::calling(on_event);
    let element = uia::element(client, hwnd).expect("the window's element");
    let cache = events::name_cache(client).expect("a cache request");
    let handler = events::as_property_handler(&handler);
    // SAFETY: live client, element, cache request and handler.
    unsafe {
        client.AddPropertyChangedEventHandlerNativeArray(
            &element,
            TreeScope_Subtree,
            &cache,
            &handler,
            &[UIA_NamePropertyId],
        )
    }
    .expect("the name-change handler");
    reports
}

/// Renames window `w`'s node 1 `name` and raises its name change; answers
/// when it was raised.
fn raise(provider: &mut Provider, w: usize, name: &str) -> u64 {
    provider.request(&format!("rename {w} 1 {name}"));
    provider
        .request(&format!("raise name {w} 1"))
        .parse()
        .expect("the time it was raised")
}

/// Waits up to `limit` for an event named `name` in `reports`, keeping
/// every event heard meanwhile in `heard`.
fn wait_for(
    reports: &Reports,
    heard: &mut Vec<Heard>,
    name: &str,
    limit: Duration,
) -> Option<Heard> {
    if let Some(h) = heard.iter().find(|h| h.name == name) {
        return Some(h.clone());
    }
    let end = now_us() + u64::try_from(limit.as_micros()).expect("a short limit");
    loop {
        let left = end.saturating_sub(now_us());
        if left == 0 {
            return None;
        }
        match reports.recv_timeout(Duration::from_micros(left)) {
            Ok(events::Report::Heard(h)) => {
                note::evidence(&format!(
                    "heard {} on thread {} at +{:.1} ms",
                    h.name,
                    h.thread,
                    us_to_ms(h.at.saturating_sub(end.saturating_sub(left)))
                ));
                heard.push(h.clone());
                if h.name == name {
                    return Some(h);
                }
            }
            Ok(events::Report::Freed) => {}
            Err(_) => return None,
        }
    }
}

/// A handler step that sleeps `ms` when it hears `name`, recording when it
/// returned in `returned`.
fn sleeping(name: &'static str, ms: u64, returned: Arc<Mutex<Option<u64>>>) -> OnEvent {
    Box::new(move |sender: &IUIAutomationElement| {
        // SAFETY: a live element UIA passed to the callback.
        let named = unsafe { sender.CachedName() }.is_ok_and(|n| n == name);
        if named {
            std::thread::sleep(Duration::from_millis(ms));
            *returned.lock().expect("a lock no one poisons") = Some(now_us());
        }
    })
}

/// A case: its name, what it does, and its function.
type Case = (&'static str, &'static str, fn(&Args, &mut Report));

/// The `events` role, a client process with no other registration:
/// `events <hwnd> keep` registers name changes on the window once, reports
/// `registered <registering thread id>`, then `heard <thread id>` for each
/// event; `events <hwnd> remove`, for each `register` command, registers,
/// reports `registered <thread id>`, and once it hears an event reports
/// `heard <thread id>` and removes the registration.
fn events_role(hwnd: isize, keep: bool) -> ! {
    let client = uia::client();
    // SAFETY: no precondition.
    let me = unsafe { GetCurrentThreadId() };
    let forward = |reports: &Reports, once: bool| loop {
        match reports.recv_timeout(Duration::from_secs(5)) {
            Ok(events::Report::Heard(h)) => {
                role::report(&format!("heard {}", h.thread));
                if once {
                    return;
                }
            }
            Ok(events::Report::Freed) => {}
            Err(_) => {
                if once {
                    role::report("none");
                    return;
                }
            }
        }
    };
    if keep {
        let reports = register(&client, hwnd, None);
        role::report(&format!("registered {me}"));
        // The parent ends this process when its trial is over.
        loop {
            forward(&reports, true);
        }
    }
    for _ in role::commands() {
        let reports = register(&client, hwnd, None);
        role::report(&format!("registered {me}"));
        forward(&reports, true);
        // SAFETY: a live client object.
        unsafe { client.RemoveAllEventHandlers() }.expect("the registration removed");
    }
    uia_facts_support::exit_now(0)
}

/// The thread id a `heard` line of the `events` role names, or `None` for
/// a line that heard nothing.
fn heard_thread(line: &str) -> Option<u32> {
    line.strip_prefix("heard ")
        .map(|t| t.parse().expect("a thread id"))
}

/// The thread ids of the `heard` lines `role` reports until it has
/// reported `count` or none comes for `silence`.
fn heard_threads(role: &Role, count: usize, silence: Duration) -> Vec<u32> {
    let mut ids = Vec::new();
    while ids.len() < count {
        match role.line(silence) {
            Some(line) => ids.extend(heard_thread(&line)),
            None => break,
        }
    }
    ids
}

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "events", "unknown role {name}");
        events_role(rest[0].parse().expect("a window handle"), rest[1] == "keep");
    }
    let mut report = Report::start("a-client-processs-events-run-on-one-worker");
    let cases: [Case; 5] = [
        (
            "shared",
            "2 and then 8 client objects, the first with a handler that takes 1 s on B1's event, the rest on B2's, raised 100 ms later",
            shared,
        ),
        (
            "threads",
            "one handler: four events 800 ms apart, each awaited, asking every 50 ms after each whether its thread has ended; four events to registrations each removed after its event, the next once its thread has ended; then four raised at once",
            threads,
        ),
        (
            "slow-handler",
            "one client object: a handler that takes 0.5 s on B1's event, and B2's event raised 50 ms later",
            slow_handler,
        ),
        (
            "slow-query",
            "one client object: a handler that reads a name from C, whose provider holds the read for 1 s, and B2's event raised 100 ms later",
            slow_query,
        ),
        (
            "drops",
            "a handler that takes 0.2 s on every event, and 20 name changes raised at once",
            drops,
        ),
    ];
    // The default run checks what Verbatim relies on, the shared worker
    // and what a slow handler or query holds; `threads` and `drops` run
    // only when named.
    let default = ["shared", "slow-handler", "slow-query"];
    for (name, what, run) in cases {
        let runs = if default.contains(&name) {
            args.wants(name)
        } else {
            args.names(name)
        };
        if runs {
            report.case(name, what);
            run(&args, &mut report);
        }
    }
    report.finish();
}

fn shared(args: &Args, report: &mut Report) {
    for k in [2usize, 8] {
        let mut after = Vec::new();
        for _ in 0..trials() {
            let mut provider = Provider::launch(&["--windows", "uia:0,uia:1"]);
            args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
            let returned = Arc::new(Mutex::new(None));
            let clients: Vec<IUIAutomation6> = (0..k).map(|_| uia::client()).collect();
            let first = register(
                &clients[0],
                provider.hwnd(0),
                Some(sleeping("slow", 1_000, returned.clone())),
            );
            let rest: Vec<Reports> = clients[1..]
                .iter()
                .map(|c| register(c, provider.hwnd(1), None))
                .collect();
            let raised = raise(&mut provider, 0, "slow");
            uia::pause(Duration::from_millis(100), "B2's event after B1's");
            raise(&mut provider, 1, "later");
            note::waiting("up to 3 s, each object's handler");
            let mut held = true;
            for reports in &rest {
                let heard = wait_for(reports, &mut Vec::new(), "later", Duration::from_secs(3));
                let back = *returned.lock().expect("a lock no one poisons");
                note::evidence(&format!(
                    "an object heard B2's event {:?} ms after B1's raise; the slow handler returned {:?} ms after it",
                    heard.as_ref().map(|h| us_to_ms(h.at - raised)),
                    back.map(|b| us_to_ms(b - raised))
                ));
                held &= matches!((heard, back), (Some(h), Some(b)) if h.at >= b);
            }
            let _ = events::drain(&first);
            after.push(held);
        }
        let claim = format!(
            "{k} client objects: every other object's handler hears B2's event only after the first object's slow handler returns"
        );
        if k == 2 {
            report.claim(
                &claim,
                &runs(&after),
                "two objects share the worker (h3's client variants)",
                relation(&after),
            );
        } else {
            report.claim(&claim, &runs(&after), "", Verdict::New);
        }
    }
}

/// Waits until `at` on [`now_us`]'s clock.
fn until(at: u64) {
    std::thread::sleep(Duration::from_micros(at.saturating_sub(now_us())));
}

/// Whether thread `id` has ended.
fn ended(id: u32) -> bool {
    // SAFETY: no precondition; the handle is closed below.
    let Ok(thread) = (unsafe { OpenThread(THREAD_SYNCHRONIZE, false, id) }) else {
        return true;
    };
    // SAFETY: the thread opened above; a zero wait only reads its state.
    let done = unsafe { WaitForSingleObject(thread, 0) } == WAIT_OBJECT_0;
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(thread) };
    done
}

#[expect(
    clippy::too_many_lines,
    reason = "three sequences of four events, then their claims"
)]
fn threads(args: &Args, report: &mut Report) {
    let mut distinct = Vec::new();
    let mut distinct_removed = Vec::new();
    let mut not_registering = Vec::new();
    let mut ends = Vec::new();
    let mut burst = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0,uia:1,uia:2"]);
        // Each sequence runs in a client process of its own with no other
        // registration, which would keep its worker.
        let keeping = Role::start("events", &[provider.hwnd(0).to_string(), "keep".to_owned()]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", keeping.pid)]);
        let registering: u32 = keeping
            .expect_line()
            .strip_prefix("registered ")
            .expect("the registration is made")
            .parse()
            .expect("a thread id");
        let mut ids = Vec::new();
        let mut gone = Vec::new();
        for i in 0..4 {
            let raised = raise(&mut provider, 0, &format!("apart{i}"));
            let Some(thread) = heard_threads(&keeping, 1, Duration::from_secs(3))
                .first()
                .copied()
            else {
                gone.push("not heard".to_owned());
                continue;
            };
            ids.push(thread);
            // The originals' events came 800 ms apart; ask every 50 ms
            // until then whether the event's thread has ended.
            let mut end = None;
            while now_us() < raised + 750_000 && end.is_none() {
                std::thread::sleep(Duration::from_millis(50));
                if ended(thread) {
                    end = Some(us_to_ms(now_us() - raised));
                }
            }
            gone.push(end.map_or_else(
                || "running".to_owned(),
                |ms| format!("{:.0}", (ms / 50.0).ceil() * 50.0),
            ));
            until(raised + 800_000);
        }
        note::evidence(&format!(
            "threads of four events 800 ms apart to one registration: {ids:?}; ended (ms after the raise, to 50 ms): {gone:?}"
        ));
        not_registering.push(!ids.is_empty() && ids.iter().all(|id| *id != registering));
        ends.push(gone);
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        distinct.push(unique.len());
        // As the original ran them: each event to a registration of its
        // own, removed once its event is heard, 800 ms apart.
        let mut removing = Role::start(
            "events",
            &[provider.hwnd(1).to_string(), "remove".to_owned()],
        );
        let mut removed = Vec::new();
        for i in 0..4 {
            removing.send("register");
            assert!(
                removing.expect_line().starts_with("registered "),
                "the registration is made"
            );
            raise(&mut provider, 1, &format!("removed{i}"));
            let thread = heard_thread(&removing.expect_line());
            removed.extend(thread);
            // The next registration once this event's thread has ended,
            // asked every 10 ms for up to 800 ms, the originals' time
            // between events.
            if let Some(thread) = thread {
                let limit = now_us() + 800_000;
                while !ended(thread) && now_us() < limit {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        note::evidence(&format!(
            "threads of four events, each to a registration removed after it: {removed:?}"
        ));
        removed.sort_unstable();
        removed.dedup();
        distinct_removed.push(removed.len());
        // Four events at once, to a registration of their own; the handler
        // runs are counted, since the senders' names are read when the
        // worker takes each event.
        let bursting = Role::start("events", &[provider.hwnd(2).to_string(), "keep".to_owned()]);
        assert!(
            bursting.expect_line().starts_with("registered "),
            "the registration is made"
        );
        for i in 0..4 {
            raise(&mut provider, 2, &format!("burst{i}"));
        }
        let mut ids = heard_threads(&bursting, 4, Duration::from_millis(1_000));
        note::evidence(&format!("threads of four events raised at once: {ids:?}"));
        ids.sort_unstable();
        ids.dedup();
        burst.push(ids.len());
    }
    report.claim(
        "distinct thread ids of four events 800 ms apart, each to a registration removed after its event",
        &runs(&distinct_removed),
        "4 (four events on four thread ids, ops.ps1's run 2, which removed each handler after its event)",
        deterministic(&distinct_removed, &4),
    );
    report.claim(
        "distinct thread ids of four events 800 ms apart to one registration kept",
        &runs(&distinct),
        "",
        Verdict::New,
    );
    report.claim(
        "every handler runs on a thread other than the one that registered it",
        &runs(&not_registering),
        "on the worker, not the registering thread",
        deterministic(&not_registering, &true),
    );
    report.claim(
        "when each of the four events' threads had ended after its raise, to 50 ms",
        &runs(&ends),
        "",
        Verdict::New,
    );
    report.claim(
        "distinct thread ids of four events raised at once",
        &runs(&burst),
        "",
        Verdict::New,
    );
}

fn slow_handler(args: &Args, report: &mut Report) {
    let mut after = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0,uia:1"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let client = uia::client();
        let returned = Arc::new(Mutex::new(None));
        let _first = register(
            &client,
            provider.hwnd(0),
            Some(sleeping("slow", 500, returned.clone())),
        );
        let second = register(&client, provider.hwnd(1), None);
        let raised = raise(&mut provider, 0, "slow");
        uia::pause(Duration::from_millis(50), "B2's event after B1's");
        raise(&mut provider, 1, "later");
        let heard = wait_for(&second, &mut Vec::new(), "later", Duration::from_secs(3));
        let back = *returned.lock().expect("a lock no one poisons");
        note::evidence(&format!(
            "B2's event heard {:?} ms after B1's raise; the slow handler returned {:?} ms after it",
            heard.as_ref().map(|h| us_to_ms(h.at - raised)),
            back.map(|b| us_to_ms(b - raised))
        ));
        after.push(matches!((heard, back), (Some(h), Some(b)) if h.at >= b));
    }
    report.claim(
        "a slow handler delays a later event of another registration until it returns",
        &runs(&after),
        "",
        Verdict::New,
    );
}

fn slow_query(args: &Args, report: &mut Report) {
    let mut after = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0,uia:1,uia:2"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let client = uia::client();
        let c_root = uia::element(&client, provider.hwnd(2)).expect("C's element");
        let c_node = uia::share(&uia::node(&client, &c_root, 1).expect("C's node 1"));
        let queried = Arc::new(Mutex::new(None));
        let done = queried.clone();
        let on_event: OnEvent = Box::new(move |sender: &IUIAutomationElement| {
            // SAFETY: a live element UIA passed to the callback.
            if unsafe { sender.CachedName() }.is_ok_and(|n| n == "query") {
                let outcome = uia::name(&uia::resolve(&c_node));
                *done.lock().expect("a lock no one poisons") =
                    Some((uia::outcome(&outcome), now_us()));
            }
        });
        let _first = register(&client, provider.hwnd(0), Some(on_event));
        let second = register(&client, provider.hwnd(1), None);
        provider.request("hold GetPropertyValue w=2 n=1");
        let raised = raise(&mut provider, 0, "query");
        provider
            .held(Duration::from_secs(10))
            .expect("the handler's read reaches C");
        uia::pause(
            Duration::from_millis(100),
            "B2's event after the read is held",
        );
        raise(&mut provider, 1, "later");
        uia::pause(Duration::from_millis(900), "the read held 1 s");
        let released = now_us();
        provider.request("release");
        let heard = wait_for(&second, &mut Vec::new(), "later", Duration::from_secs(3));
        note::evidence(&format!(
            "B2's event heard {:?} ms after B1's raise; C released {:.1} ms after it; the read {:?}",
            heard.as_ref().map(|h| us_to_ms(h.at - raised)),
            us_to_ms(released - raised),
            *queried.lock().expect("a lock no one poisons")
        ));
        after.push(heard.is_some_and(|h| h.at >= released));
    }
    report.claim(
        "a query inside a handler, into a provider that holds it, delays a later event of another registration until it returns",
        &runs(&after),
        "",
        Verdict::New,
    );
}

fn drops(args: &Args, report: &mut Report) {
    const COUNT: usize = 20;
    /// The handler's time for each event.
    const HANDLER_MS: u64 = 200;
    let mut counts = Vec::new();
    let mut fewer = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let client = uia::client();
        let slow: OnEvent = Box::new(|_| std::thread::sleep(Duration::from_millis(HANDLER_MS)));
        let reports = register(&client, provider.hwnd(0), Some(slow));
        for i in 0..COUNT {
            raise(&mut provider, 0, &format!("e{i}"));
        }
        // The handler runs are counted, not named: a sender's name is read
        // when the worker takes its event, so events queued together carry
        // the latest name. The count ends at 20, or once no run has come
        // for 2 s, ten times the handler's time.
        note::waiting("up to 6 s, the twenty events at 0.2 s each and 2 s with none");
        let silence = Duration::from_secs(2);
        let mut count = 0;
        while count < COUNT {
            match reports.recv_timeout(silence) {
                Ok(events::Report::Heard(_)) => count += 1,
                Ok(events::Report::Freed) => {}
                Err(_) => break,
            }
        }
        note::evidence(&format!("the handler ran {count} times"));
        counts.push(count);
        fewer.push(count < COUNT);
    }
    report.claim(
        &format!("of {COUNT} name changes raised at once, to a handler that takes 0.2 s each, some are never delivered"),
        &format!("{} ({counts:?} delivered)", runs(&fewer)),
        "",
        Verdict::New,
    );
}
