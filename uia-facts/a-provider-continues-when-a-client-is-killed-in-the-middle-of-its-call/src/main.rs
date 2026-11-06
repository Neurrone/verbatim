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
use uia_facts_support::report::{Report, Verdict};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{apps, exit_now, rop, uia};
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
    let read = uia::spawn(move || {
        let client = uia::client();
        uia::set_timeouts(&client, 2_000, 4_000);
        uia::element(&client, hwnd)
            .and_then(|root| uia::node(&client, &root, 2))
            .and_then(|second| uia::name(&second))
    });
    let start = std::time::Instant::now();
    while !read.is_finished() && start.elapsed() < ANSWER {
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Some(code) = provider.exit_code() {
        return format!("ended 0x{code:08X}");
    }
    if !read.is_finished() {
        return "hung".to_owned();
    }
    match read.join().expect("the read") {
        Ok(name) if name == "Second" && provider.answers(ANSWER) => "continues".to_owned(),
        Ok(name) => format!("answered {name}"),
        Err(error) => match provider.exit_code() {
            Some(code) => format!("ended 0x{code:08X}"),
            None => format!("read failed {}", uia::hresult(error.code())),
        },
    }
}

/// The verdict of `outcomes`: how many trials P continued in; a new
/// answer, since nothing recorded it.
fn judge(report: &mut Report, what: &str, outcomes: &[String]) {
    let continued = outcomes.iter().filter(|o| *o == "continues").count();
    for other in outcomes.iter().filter(|o| *o != "continues") {
        uia_facts_support::note::evidence(&format!("a trial's provider: {other}"));
    }
    report.claim(
        &format!("trials in which the provider continued after a client ended in {what}"),
        &format!("{continued} of {}", outcomes.len()),
        "",
        Verdict::New,
    );
    if continued < outcomes.len() {
        uia_facts_support::note::print_evidence();
    }
}

fn held(report: &mut Report, threading: &str, name: &str, hold: &str) {
    let mut outcomes = Vec::new();
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
        provider.request("release");
        outcomes.push(after(&mut provider));
    }
    judge(report, &format!("its {name} call ({threading})"), &outcomes);
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
        // P raises 2,000 name changes back to back on its UI thread, and K
        // is ended 20 ms after they are asked for, a deliberate offset,
        // since nothing marks K's handler running.
        let pid = client.pid;
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            apps::kill(pid);
        });
        provider.request("raise-rate name 0 1 0 2000");
        killer.join().expect("the killer");
        let _ = client.wait(Duration::from_secs(5));
        outcomes.push(after(&mut provider));
    }
    judge(
        report,
        &format!("its handler's events ({threading})"),
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
        // K is ended 20 ms after the notifications are asked for, a
        // deliberate offset, since nothing marks K's handler running.
        let pid = client.pid;
        let killer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            apps::kill(pid);
        });
        provider.request("raise-rate notification 0 3 0 2000");
        killer.join().expect("the killer");
        let _ = client.wait(Duration::from_secs(5));
        provider.request("release");
        outcomes.push(after(&mut provider));
    }
    judge(
        report,
        "its held GetText beside its handlers' events (nocom)",
        &outcomes,
    );
}
