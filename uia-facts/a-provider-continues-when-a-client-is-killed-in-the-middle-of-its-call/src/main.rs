//! A provider continues when a client is killed in the middle of its call
//! (`uia-facts/a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call/readme.md`).
//!
//! The provider program P serves window 0. A client process K, a second
//! copy of this program, holds window 0's elements; P holds K's call in
//! its provider until released; this program ends K with
//! `TerminateProcess` while the call is held, then releases it. P then
//! has to answer: its own request line, and a name read from this
//! program's own client object. A trial is a new P and a new K. Kinds of
//! call, each its own case:
//!
//! - `read`: node 1's name (`GetPropertyValue`).
//! - `text`: node 3's document range's text (`GetText`).
//! - `operation`: a remote operation reading the window's name.
//! - `events`: no call held; K holds a name-change handler on the window
//!   while P raises name changes back to back, and is ended in the middle
//!   of them.
//!
//! Each kind runs with the provider's COM threading and without it, where
//! its calls run on UIA's I/O thread, as Windows Terminal's text provider
//! declares (`ServerSideProvider` alone). One more case,
//! `text-and-events`, puts together what Windows Terminal's provider does
//! beside a client's call, without COM threading: K holds name-change and
//! notification handlers and its `GetText` is held, while P raises
//! notifications back to back on its UI thread, and K is ended among them.

use std::time::Duration;

use uia_facts_support::args::Args;
use uia_facts_support::events::{self, Handler};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, rate};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{apps, exit_now, now_us, rop, uia};
use windows::Win32::UI::Accessibility::{
    IUIAutomationTextPattern, TreeScope_Subtree, UIA_NamePropertyId, UIA_TextPatternId,
};

/// Trials of each kind.
const TRIALS: usize = 20;
/// How long P is given to answer after the release before it is judged
/// hung: longer than any of its answers, which take milliseconds.
const ANSWER: Duration = Duration::from_secs(5);

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "killed", "the one role is killed");
        killed_role(rest);
    }
    let mut report =
        Report::start("a-provider-continues-when-a-client-is-killed-in-the-middle-of-its-call");
    let kinds = [
        ("read", "GetPropertyValue w=0 n=1", "name read"),
        ("text", "GetText", "GetText"),
        ("operation", "GetPropertyValue w=0 n=0", "remote operation"),
    ];
    for threading in ["com", "nocom"] {
        let served = if threading == "com" {
            "with COM threading"
        } else {
            "without COM threading, on UIA's I/O thread"
        };
        for (name, hold, what) in kinds {
            if args.wants(name) {
                report.case(name, &format!("{TRIALS} trials, the provider {served}: holding the client's {what} in the provider, ending the client, releasing the call; then the provider's answers"));
                held(&mut report, threading, name, hold);
            }
        }
        if args.wants("events") {
            report.case("events", &format!("{TRIALS} trials, the provider {served}: the client holding a name-change handler while the provider raises name changes back to back; ending the client among them; then the provider's answers"));
            flood(&mut report, threading);
        }
    }
    if args.wants("text-and-events") {
        report.case("text-and-events", &format!("{TRIALS} trials, the provider without COM threading: the client holding name-change and notification handlers and its GetText held, while the provider raises notifications back to back on its UI thread; ending the client among them; then the provider's answers"));
        text_and_events(&mut report);
    }
    report.finish();
}

/// The client: `<hwnd>`. Fetches window `hwnd`'s element, node 1 and node
/// 3's document range, and reports `ready`; then makes the call it is
/// told (`read`, `text`, `operation`, or `listen`, which registers a
/// name-change handler and reports `listening`), and is ended in it.
fn killed_role(rest: &[String]) -> ! {
    let hwnd: isize = rest[0].parse().expect("a window handle");
    let client = uia::client();
    let root = uia::element(&client, hwnd).expect("the window's element");
    let first = uia::node(&client, &root, 1).expect("node 1");
    let notes = uia::node(&client, &root, 3).expect("node 3");
    // SAFETY: a live element; the text pattern's interface.
    let pattern: IUIAutomationTextPattern =
        unsafe { notes.GetCurrentPatternAs(UIA_TextPatternId) }.expect("the text pattern");
    // SAFETY: a live pattern.
    let range = unsafe { pattern.DocumentRange() }.expect("the document range");
    role::report("ready");
    let mut kept = Vec::new();
    for command in role::commands() {
        match command.as_str() {
            "read" => {
                let _ = uia::name(&first);
            }
            // SAFETY: a live range.
            "text" => drop(unsafe { range.GetText(-1) }),
            "operation" => drop(rop::name(&root)),
            "listen" => {
                let (handler, reports) = Handler::make();
                let cache = events::name_cache(&client).expect("a cache request");
                let handler = events::as_property_handler(&handler);
                // SAFETY: live client, element, cache request and handler.
                unsafe {
                    client.AddPropertyChangedEventHandlerNativeArray(
                        &root,
                        TreeScope_Subtree,
                        &cache,
                        &handler,
                        &[UIA_NamePropertyId],
                    )
                }
                .expect("the name-change handler");
                let (notifications, heard) = Handler::make();
                let notifications = events::as_notification_handler(&notifications);
                // SAFETY: live client, element, cache request and handler.
                unsafe {
                    client.AddNotificationEventHandler(
                        &root,
                        TreeScope_Subtree,
                        &cache,
                        &notifications,
                    )
                }
                .expect("the notification handler");
                kept.push(reports);
                kept.push(heard);
                role::report("listening");
            }
            other => panic!("unknown command {other}"),
        }
        role::report("returned");
    }
    exit_now(0)
}

/// What P did once K was ended: `continues` when it answered its request
/// line and a new name read within [`ANSWER`]; otherwise `ended <code>`
/// or `hung`.
fn after(provider: &mut Provider) -> String {
    let hwnd = provider.hwnd(0);
    let (answered, answer) = std::sync::mpsc::channel();
    let _read = uia::spawn(move || {
        let client = uia::client();
        uia::set_timeouts(&client, 2_000, 4_000);
        let read = uia::element(&client, hwnd)
            .and_then(|root| uia::node(&client, &root, 2))
            .and_then(|second| uia::name(&second));
        let _ = answered.send(read);
    });
    let read = answer.recv_timeout(ANSWER);
    if let Some(code) = provider.exit_code() {
        return format!("ended 0x{code:08X}");
    }
    let Ok(read) = read else {
        return "hung".to_owned();
    };
    match read {
        Ok(name) if name == "Second" && provider.answers(ANSWER) => "continues".to_owned(),
        Ok(name) => format!("answered {name}"),
        Err(error) => match provider.exit_code() {
            Some(code) => format!("ended 0x{code:08X}"),
            None => format!("read failed {}", uia::hresult(error.code())),
        },
    }
}

/// The verdict of `outcomes`: how many trials P continued in, against
/// the first run's 20 of 20.
fn judge(report: &mut Report, what: &str, outcomes: &[String]) {
    let continued = outcomes.iter().filter(|o| *o == "continues").count();
    for other in outcomes.iter().filter(|o| *o != "continues") {
        uia_facts_support::note::evidence(&format!("a trial's provider: {other}"));
    }
    report.claim(
        &format!("trials in which the provider continued after a client ended in {what}"),
        &format!("{continued} of {}", outcomes.len()),
        "20 of 20 (the first run)",
        rate(continued, outcomes.len(), 20, 20),
    );
    if continued < outcomes.len() {
        uia_facts_support::note::print_evidence();
    }
}

/// Whether P's log holds the end of the held call of `method`, on node
/// `node` when given, at or after `released`: the call returned into a
/// client that was gone.
fn returned(provider: &mut Provider, method: &str, node: Option<usize>, released: u64) -> bool {
    let (_, found) = provider.log_until(ANSWER, |e| {
        e.is_call(method) && e.end >= released && node.is_none_or(|n| e.node == Some(n))
    });
    found
}

/// Ends process `pid` 20 ms after it starts, a deliberate offset, while
/// P runs `request`, which raises events back to back; answers whether
/// the kill came before the raise had answered, so among the events.
fn kill_among(provider: &mut Provider, pid: u32, request: &str) -> bool {
    let (when, kill_time) = std::sync::mpsc::channel();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        let at = now_us();
        apps::kill(pid);
        let _ = when.send(at);
    });
    provider.request(request);
    let raised = now_us();
    killer.join().expect("the killer");
    let at = kill_time.recv().expect("the kill's time");
    at < raised
}

fn held(report: &mut Report, threading: &str, name: &str, hold: &str) {
    let mut outcomes = Vec::new();
    let method = hold.split(' ').next().expect("a method");
    let node = hold
        .split(' ')
        .find_map(|w| w.strip_prefix("n="))
        .map(|n| n.parse().expect("a node"));
    for _ in 0..TRIALS {
        let mut provider = Provider::launch(&["--windows", "uia:0", "--threading", threading]);
        let mut client = Role::start("killed", &[provider.hwnd(0).to_string()]);
        assert_eq!(client.expect_line(), "ready", "the client is ready");
        provider.request(&format!("hold {hold}"));
        client.send(name);
        provider
            .held(Duration::from_secs(10))
            .expect("the client's call is held");
        apps::kill(client.pid);
        let _ = client.wait(Duration::from_secs(5));
        provider.log();
        let released = now_us();
        provider.request("release");
        let back = returned(&mut provider, method, node, released);
        let outcome = after(&mut provider);
        outcomes.push(if back {
            outcome
        } else {
            format!("the held call not returned ({outcome})")
        });
    }
    judge(
        report,
        &format!("its {name} call ({threading}), the held call returning into it"),
        &outcomes,
    );
}

fn flood(report: &mut Report, threading: &str) {
    let mut outcomes = Vec::new();
    for _ in 0..TRIALS {
        let mut provider = Provider::launch(&["--windows", "uia:0", "--threading", threading]);
        let mut client = Role::start("killed", &[provider.hwnd(0).to_string()]);
        assert_eq!(client.expect_line(), "ready", "the client is ready");
        client.send("listen");
        assert_eq!(client.expect_line(), "listening", "the client listens");
        assert_eq!(
            client.expect_line(),
            "returned",
            "the registration returned"
        );
        let among = kill_among(&mut provider, client.pid, "raise-rate name 0 1 0 2000");
        let _ = client.wait(Duration::from_secs(5));
        let outcome = after(&mut provider);
        outcomes.push(if among {
            outcome
        } else {
            format!("ended after the events were raised ({outcome})")
        });
    }
    judge(
        report,
        &format!("the provider's raising of its handler's events ({threading})"),
        &outcomes,
    );
}

fn text_and_events(report: &mut Report) {
    let mut outcomes = Vec::new();
    for _ in 0..TRIALS {
        let mut provider = Provider::launch(&["--windows", "uia:0", "--threading", "nocom"]);
        let mut client = Role::start("killed", &[provider.hwnd(0).to_string()]);
        assert_eq!(client.expect_line(), "ready", "the client is ready");
        client.send("listen");
        assert_eq!(client.expect_line(), "listening", "the client listens");
        assert_eq!(
            client.expect_line(),
            "returned",
            "the registration returned"
        );
        provider.request("hold GetText");
        client.send("text");
        provider
            .held(Duration::from_secs(10))
            .expect("the client's GetText is held");
        let among = kill_among(
            &mut provider,
            client.pid,
            "raise-rate notification 0 3 0 2000",
        );
        let _ = client.wait(Duration::from_secs(5));
        provider.log();
        let released = now_us();
        provider.request("release");
        let back = returned(&mut provider, "GetText", None, released);
        let outcome = after(&mut provider);
        outcomes.push(match (among, back) {
            (true, true) => outcome,
            (false, _) => format!("ended after the events were raised ({outcome})"),
            (true, false) => format!("the held call not returned ({outcome})"),
        });
    }
    judge(
        report,
        "the provider's raising of its handlers' events, its GetText held and returning into it (nocom)",
        &outcomes,
    );
}
