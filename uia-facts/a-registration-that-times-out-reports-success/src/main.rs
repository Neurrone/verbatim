//! A registration that times out reports success
//! (`uia-facts/a-registration-that-times-out-reports-success/readme.md`).
//!
//! Every registration is an event handler group holding UIA's focus event,
//! subtree scope, the sender's name cached, on the provider program's
//! window, as e57 and e58 made it. Each focus event is raised on node 5,
//! renamed `ev<i>` first, so that the handler names the event it heard.
//! A stall is the window's UI thread busy until released.
//!
//! - `ett`: E-TT's registration: both timeouts at 1,000 ms, the provider
//!   holding `FragmentRoot`. 5 trials.
//! - `connection`: a registration during a stall returns after the
//!   connection timeout, whatever the transaction timeout: 500 and
//!   5,000 ms, and the reverse. 5 trials each.
//! - `e57`: connection timeout 2,000 ms, transaction timeout 5,000 ms; the
//!   group registered during a stall, the stall released 200 ms after the
//!   registration returns, and a focus event raised every 50 ms from the
//!   stall's end to 2,500 ms after it; heard events listened for 2 s
//!   after the last. 5 trials.
//! - `e58-silence`: as `e57` with no event raised, the provider's log read
//!   6 s after the release. 3 trials.
//! - `e58-scope`: two provider programs, P and Q. Before the stall of P,
//!   this process's client object registers group A on P's window and C on
//!   Q's, and a second client process registers D on P's window. During
//!   the stall another client object registers B on P's window. Focus
//!   events are raised on P every 100 ms from the stall's end to 3,000 ms
//!   after it, and on Q every 100 ms from 50 to 2,950 ms. 3 trials.
//! - `e58-remove`: as `e57`, the group removed 500 ms after the release
//!   (`remove`), 3,000 ms after it (`remove-late`), or with no stall
//!   (`remove-nostall`); focus events at 1,000 and 4,000 ms after the
//!   release; the handler's release watched for 15 s. 3 trials each.
//! - `between`: the inferred loss of events raised between a focus change
//!   and a registration made for the new focus: a name change raised just
//!   after the focus event, and one raised after the registration returns.
//!   3 trials.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use uia_facts_support::args::Args;
use uia_facts_support::events::{self, Handler, Heard, Report as Event};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{
    Report, Verdict, band, band_text, deterministic, in_band, relation, runs,
};
use uia_facts_support::role::{self, Role};
use uia_facts_support::stats::{summary_ms, us_to_ms};
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{exit_now, now_us};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationElement, IUIAutomationEventHandlerGroup, TreeScope_Element,
    UIA_NamePropertyId,
};

/// The node every focus event is raised on.
const NODE: usize = 5;
/// The stall's release after the registration returns, e57's 200 ms.
const RELEASE_AFTER: Duration = Duration::from_millis(200);
/// How long events are listened for after the last raise: e57 listened 2
/// to 4 s; events after the gap arrived within 1 to 11 ms.
const LISTEN: Duration = Duration::from_secs(2);

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, "listen", "the one role is listen");
        listen(rest[0].parse().expect("a window handle"));
    }
    let mut report = Report::start("a-registration-that-times-out-reports-success");
    if args.wants("ett") {
        ett(&args, &mut report);
    }
    if args.wants("connection") {
        connection(&mut report, 500, 5_000, "532 to 551 ms");
        connection(&mut report, 5_000, 500, "5,453 to 5,541 ms");
    }
    if args.wants("e57") {
        e57(&args, &mut report);
    }
    if args.wants("e58-silence") {
        e58_silence(&mut report);
    }
    if args.wants("e58-scope") {
        e58_scope(&mut report);
    }
    if args.wants("e58-remove") {
        e58_remove(&mut report);
    }
    if args.wants("between") {
        between(&mut report);
    }
    report.finish();
}

/// The second client process: registers a focus group on window `hwnd`,
/// says `ready`, and reports `heard <name> <time>` for each event until
/// its standard input closes.
fn listen(hwnd: isize) -> ! {
    let client = uia::client();
    uia::set_timeouts(&client, 2_000, 5_000);
    let element = uia::element(&client, hwnd).expect("the window's element");
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    events::add_group(&client, &element, &group).expect("the registration");
    std::thread::spawn(move || {
        for report in reports {
            if let Event::Heard(heard) = report {
                role::report(&format!("heard {} {}", heard.name, heard.at));
            }
        }
    });
    role::report("ready");
    for _ in role::commands() {}
    exit_now(0)
}

/// A provider program, a client with the timeouts set, and its window's
/// element fetched before any stall.
fn setup(
    connection_ms: u32,
    transaction_ms: u32,
) -> (Provider, IUIAutomation6, IUIAutomationElement) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    let client = uia::client();
    uia::set_timeouts(&client, connection_ms, transaction_ms);
    let element = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    (provider, client, element)
}

/// Raises a focus event on node `NODE` of window `window` at each offset
/// after `from`, the node renamed `<prefix><i>` first; answers each event's
/// name and when it was raised.
fn raise_at(
    provider: &mut Provider,
    window: usize,
    prefix: &str,
    from: Instant,
    offsets_ms: &[u64],
) -> Vec<(String, u64)> {
    let mut raised = Vec::new();
    for (i, offset) in offsets_ms.iter().enumerate() {
        let due = from + Duration::from_millis(*offset);
        uia::pause(due.saturating_duration_since(Instant::now()));
        let name = format!("{prefix}{i}");
        provider.request(&format!("rename {window} {NODE} {name}"));
        let at: u64 = provider
            .request(&format!("raise focus {window} {NODE}"))
            .parse()
            .expect("the time it was raised");
        raised.push((name, at));
    }
    raised
}

/// Each raised event's delay in ms when heard, `None` when lost.
fn delays(raised: &[(String, u64)], heard: &[Heard]) -> Vec<Option<f64>> {
    let by_name: HashMap<&str, u64> = heard.iter().map(|h| (h.name.as_str(), h.at)).collect();
    raised
        .iter()
        .map(|(name, at)| {
            by_name
                .get(name.as_str())
                .map(|h| us_to_ms(h.saturating_sub(*at)))
        })
        .collect()
}

/// A registration made during a stall of the provider's UI thread, timed.
/// Answers the registration's outcome and the stall's end.
struct Stalled {
    registration: Timed,
    released: Instant,
    released_us: u64,
}

fn register_stalled(
    provider: &mut Provider,
    client: &IUIAutomation6,
    element: &IUIAutomationElement,
    group: &IUIAutomationEventHandlerGroup,
) -> Stalled {
    provider.request("busy 0");
    let registration = Timed::of(|| events::add_group(client, element, group));
    uia::pause(RELEASE_AFTER);
    let released_us = now_us();
    provider.request("release");
    Stalled {
        registration,
        released: Instant::now(),
        released_us,
    }
}

fn ett(args: &Args, report: &mut Report) {
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        let (mut provider, client, element) = setup(1_000, 1_000);
        let (handler, _reports) = Handler::make();
        let group = events::focus_group(&client, &handler).expect("a group");
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        provider.request("hold FragmentRoot");
        let registration = Timed::of(|| events::add_group(&client, &element, &group));
        provider.request("release");
        times.push(registration.ms());
        outcomes.push(registration.outcome);
    }
    report.claim(
        "E-TT's registration, its provider held in FragmentRoot, reports success",
        &runs(&outcomes),
        "\"ok\"",
        deterministic(&outcomes, &"ok".to_owned()),
    );
    let window = band(&[1_000.0]);
    report.claim(
        "it returns after the timeout",
        &summary_ms(&times),
        &format!("1,089 ms, {}", band_text(window)),
        in_band(&times, window),
    );
}

fn connection(report: &mut Report, connection_ms: u32, transaction_ms: u32, recorded: &str) {
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..5 {
        let (mut provider, client, element) = setup(connection_ms, transaction_ms);
        let (handler, _reports) = Handler::make();
        let group = events::focus_group(&client, &handler).expect("a group");
        let stalled = register_stalled(&mut provider, &client, &element, &group);
        times.push(stalled.registration.ms());
        outcomes.push(stalled.registration.outcome);
    }
    let claim = format!(
        "a registration during a stall at connection {connection_ms} ms, transaction {transaction_ms} ms"
    );
    report.claim(
        &format!("{claim}, reports success"),
        &runs(&outcomes),
        "\"ok\"",
        deterministic(&outcomes, &"ok".to_owned()),
    );
    let window = band(&[f64::from(connection_ms)]);
    report.claim(
        &format!("{claim}, returns after the connection timeout"),
        &summary_ms(&times),
        &format!("{recorded}, {}", band_text(window)),
        in_band(&times, window),
    );
}

fn e57(args: &Args, report: &mut Report) {
    let offsets: Vec<u64> = (0..=50).map(|i| i * 50).collect();
    let mut outcomes = Vec::new();
    let mut times = Vec::new();
    let mut early_lost = Vec::new();
    let mut late_heard = Vec::new();
    let mut prompt = Vec::new();
    let mut no_loss_after = Vec::new();
    let mut edges = Vec::new();
    for _ in 0..5 {
        let (mut provider, client, element) = setup(2_000, 5_000);
        let (handler, reports) = Handler::make();
        let group = events::focus_group(&client, &handler).expect("a group");
        args.pause_for_debugger(&[("provider", provider.pid), ("client", std::process::id())]);
        let stalled = register_stalled(&mut provider, &client, &element, &group);
        let raised = raise_at(&mut provider, 0, "ev", stalled.released, &offsets);
        uia::pause(LISTEN);
        let (heard, _) = events::drain(&reports);
        let delays = delays(&raised, &heard);
        outcomes.push(stalled.registration.outcome.clone());
        times.push(stalled.registration.ms());
        let at = |i: usize| offsets[i];
        early_lost.push(
            (0..delays.len())
                .filter(|i| at(*i) <= 1_500)
                .all(|i| delays[i].is_none()),
        );
        late_heard.push(
            (0..delays.len())
                .filter(|i| at(*i) >= 2_300)
                .all(|i| delays[i].is_some()),
        );
        prompt.push(delays.iter().flatten().all(|d| *d < 100.0));
        let first = delays.iter().position(Option::is_some);
        no_loss_after.push(first.is_some_and(|f| delays[f..].iter().all(Option::is_some)));
        let returned = stalled.registration.end;
        let last_lost = first.and_then(|f| f.checked_sub(1)).map(|i| raised[i].1);
        let first_heard = first.map(|f| raised[f].1);
        edges.push(format!(
            "last lost {} ms, first heard {} ms after the return",
            last_lost.map_or("none".to_owned(), |t| format!(
                "{:.0}",
                us_to_ms(t.saturating_sub(returned))
            )),
            first_heard.map_or("none".to_owned(), |t| format!(
                "{:.0}",
                us_to_ms(t.saturating_sub(returned))
            ))
        ));
        let _ = events::remove_group(&client, &element, &group);
    }
    report.claim(
        "e57's registration during a stall reports success",
        &runs(&outcomes),
        "\"ok\"",
        deterministic(&outcomes, &"ok".to_owned()),
    );
    let window = band(&[2_000.0]);
    report.claim(
        "it returns after the connection timeout",
        &summary_ms(&times),
        &format!("2,190 to 2,226 ms, {}", band_text(window)),
        in_band(&times, window),
    );
    report.claim(
        "every event raised up to 1,500 ms after the stall's end is lost",
        &runs(&early_lost),
        "true (lost up to 1,750 ms)",
        relation(&early_lost),
    );
    report.claim(
        "every event raised from 2,300 ms after the stall's end is heard",
        &runs(&late_heard),
        "true (heard from 1,800 ms)",
        relation(&late_heard),
    );
    report.claim(
        "every event heard is heard within 100 ms of its raise: lost, not late",
        &runs(&prompt),
        "true (1 to 11 ms)",
        relation(&prompt),
    );
    report.claim(
        "no event is lost after the first one heard",
        &runs(&no_loss_after),
        "true",
        relation(&no_loss_after),
    );
    report.claim(
        "the gap's end after the registration returned",
        &edges.join("; "),
        "1,981 to 2,001 ms",
        Verdict::Trend,
    );
}

fn e58_silence(report: &mut Report) {
    let mut advised = Vec::new();
    let mut silent = Vec::new();
    for _ in 0..3 {
        let (mut provider, client, element) = setup(2_000, 5_000);
        let (handler, _reports) = Handler::make();
        let group = events::focus_group(&client, &handler).expect("a group");
        provider.log();
        let stalled = register_stalled(&mut provider, &client, &element, &group);
        // The silence is the claim: six seconds with nothing reaching the
        // provider, as e58 watched.
        uia::pause(Duration::from_secs(6));
        let log = provider.log();
        let backlog_end = stalled.released_us + 20_000;
        let adds = log
            .iter()
            .filter(|e| e.kind == "advise" && e.name == "AdviseEventAdded" && e.extra == "20005")
            .map(|e| e.start >= stalled.released_us && e.start <= backlog_end)
            .collect::<Vec<_>>();
        advised.push(adds);
        silent.push(log.iter().filter(|e| e.start > backlog_end).count());
    }
    report.claim(
        "the provider is advised of the focus event once, at the stall's end",
        &runs(&advised),
        "one advise, within 20 ms of the release",
        deterministic(&advised, &vec![true]),
    );
    report.claim(
        "with no event raised, nothing reaches the provider in the 6 s after the stall's end",
        &runs(&silent),
        "0 entries",
        deterministic(&silent, &0),
    );
}

fn e58_scope(report: &mut Report) {
    let mut same_loss = Vec::new();
    let mut other_window = Vec::new();
    let mut other_process = Vec::new();
    for _ in 0..3 {
        let (mut p_app, client, p_element) = setup(2_000, 5_000);
        let mut q_app = Provider::launch(&["--windows", "uia:0"]);
        let q_element = uia::element(&client, q_app.hwnd(0)).expect("Q's window");
        let (a_handler, a_reports) = Handler::make();
        let group_a = events::focus_group(&client, &a_handler).expect("group A");
        events::add_group(&client, &p_element, &group_a).expect("A");
        let (c_handler, c_reports) = Handler::make();
        let group_c = events::focus_group(&client, &c_handler).expect("group C");
        events::add_group(&client, &q_element, &group_c).expect("C");
        let listener_d = Role::start("listen", &[p_app.hwnd(0).to_string()]);
        assert_eq!(listener_d.expect_line(), "ready", "D registers");
        let p_element_shared = uia::share(&p_element);
        p_app.request("busy 0");
        // B registers on a thread of its own, which keeps its client
        // object and group until the trial ends.
        let (registered_b, b_registered) = std::sync::mpsc::channel();
        let (end_b, b_ends) = std::sync::mpsc::channel::<()>();
        let thread_b = uia::spawn(move || {
            let other = uia::client();
            let (handler, reports) = Handler::make();
            let group = events::focus_group(&other, &handler).expect("group B");
            let registered = events::add_group(&other, &uia::resolve(&p_element_shared), &group);
            let _ = registered_b.send((registered.is_ok(), reports));
            let _ = b_ends.recv();
        });
        // B returns after the connection timeout; the stall ends 200 ms
        // after, as in e57.
        let (b_ok, b_reports) = b_registered.recv().expect("B registers");
        assert!(b_ok, "B reports success");
        uia::pause(RELEASE_AFTER);
        p_app.request("release");
        let released = Instant::now();
        let p_offsets: Vec<u64> = (0..=30).map(|i| i * 100).collect();
        let q_offsets: Vec<u64> = (0..30).map(|i| 50 + i * 100).collect();
        let mut p_raised = Vec::new();
        let mut q_raised = Vec::new();
        for (i, offset) in p_offsets.iter().enumerate() {
            p_raised.extend(raise_at(
                &mut p_app,
                0,
                &format!("p{i}-"),
                released,
                &[*offset],
            ));
            if let Some(q_offset) = q_offsets.get(i) {
                q_raised.extend(raise_at(
                    &mut q_app,
                    0,
                    &format!("q{i}-"),
                    released,
                    &[*q_offset],
                ));
            }
        }
        uia::pause(LISTEN);
        let a_lost = lost(&p_raised, &events::drain(&a_reports).0);
        let b_lost = lost(&p_raised, &events::drain(&b_reports).0);
        let c_lost = lost(&q_raised, &events::drain(&c_reports).0);
        let mut d_heard = Vec::new();
        while let Some(line) = listener_d.line(Duration::from_millis(100)) {
            let words: Vec<&str> = line.split(' ').collect();
            if let ["heard", name, at] = words.as_slice() {
                d_heard.push(Heard {
                    name: (*name).to_owned(),
                    id: 0,
                    at: at.parse().expect("a time"),
                });
            }
        }
        let d_lost = lost(&p_raised, &d_heard);
        same_loss.push(!a_lost.is_empty() && a_lost == b_lost);
        other_window.push(c_lost.len());
        other_process.push(d_lost.len());
        let _ = end_b.send(());
        let _ = thread_b.join();
    }
    report.claim(
        "A, registered before the stall on another client object, loses the same events as B",
        &runs(&same_loss),
        "true",
        relation(&same_loss),
    );
    report.claim(
        "C, on another process's window, loses no event",
        &runs(&other_window),
        "0 of 30",
        deterministic(&other_window, &0),
    );
    report.claim(
        "D, in another client process on the same window, loses no event",
        &runs(&other_process),
        "0 of 31",
        deterministic(&other_process, &0),
    );
}

/// The names of the raised events not heard.
fn lost(raised: &[(String, u64)], heard: &[Heard]) -> Vec<String> {
    raised
        .iter()
        .filter(|(name, _)| !heard.iter().any(|h| h.name == *name))
        .map(|(name, _)| name.clone())
        .collect()
}

/// A removal case's outcome: the removal's, whether the handler was freed
/// within 15 s, and whether the events at 1,000 and 4,000 ms were heard.
type Removal = (String, bool, bool, bool);

/// One removal case: the removal's outcome, whether the handler was freed
/// within 15 s, and whether the events at 1,000 and 4,000 ms were heard.
fn removal(stall: bool, remove_after: Duration) -> Removal {
    let (mut provider, client, element) = setup(2_000, 5_000);
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    drop(handler);
    let released = if stall {
        register_stalled(&mut provider, &client, &element, &group).released
    } else {
        events::add_group(&client, &element, &group).expect("the registration");
        Instant::now()
    };
    // The removal and the two events, in the order of their times.
    let mut raised = Vec::new();
    let mut removed = None;
    let mut steps = [
        (remove_after, None),
        (Duration::from_secs(1), Some("ev")),
        (Duration::from_secs(4), Some("late")),
    ];
    steps.sort_by_key(|s| s.0);
    for (at, event) in steps {
        if let Some(prefix) = event {
            raised.extend(raise_at(
                &mut provider,
                0,
                prefix,
                released,
                &[duration_ms(at)],
            ));
        } else {
            uia::pause((released + at).saturating_duration_since(Instant::now()));
            removed = Some(Timed::of(|| {
                events::remove_group(&client, &element, &group)
            }));
        }
    }
    drop(group);
    let removed = removed.expect("the removal ran");
    let freed_by = Instant::now() + Duration::from_secs(15);
    let mut heard = Vec::new();
    let mut freed = false;
    while Instant::now() < freed_by && !freed {
        if let Ok(report) = reports.recv_timeout(Duration::from_millis(100)) {
            match report {
                Event::Heard(h) => heard.push(h),
                Event::Freed => freed = true,
            }
        }
    }
    let delays = delays(&raised, &heard);
    (
        removed.outcome,
        freed,
        delays.first().copied().flatten().is_some(),
        delays.last().copied().flatten().is_some(),
    )
}

fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).expect("a short duration")
}

fn e58_remove(report: &mut Report) {
    let cases: [(&str, bool, u64, Removal); 3] = [
        (
            "remove",
            true,
            500,
            ("UIA_E_TIMEOUT".to_owned(), false, false, true),
        ),
        (
            "remove-late",
            true,
            3_000,
            ("ok".to_owned(), true, false, false),
        ),
        (
            "remove-nostall",
            false,
            500,
            ("ok".to_owned(), true, false, false),
        ),
    ];
    for (name, stall, after, recorded) in cases {
        let trials: Vec<Removal> = (0..3)
            .map(|_| removal(stall, Duration::from_millis(after)))
            .collect();
        report.claim(
            &format!("{name}: the removal's outcome, the handler freed within 15 s, the events at 1,000 and 4,000 ms heard"),
            &runs(&trials),
            &format!("{recorded:?}"),
            deterministic(&trials, &recorded),
        );
    }
}

fn between(report: &mut Report) {
    let mut results = Vec::new();
    for _ in 0..3 {
        let (mut provider, client, element) = setup(2_000, 5_000);
        let (focus_handler, focus_reports) = Handler::make();
        let group = events::focus_group(&client, &focus_handler).expect("a group");
        events::add_group(&client, &element, &group).expect("the focus registration");
        let node = uia::node(&client, &element, NODE).expect("the node");
        provider.request(&format!("focus 0 {NODE}"));
        provider.request(&format!("rename 0 {NODE} before"));
        provider.request(&format!("raise focus 0 {NODE}"));
        provider.request(&format!("raise name 0 {NODE}"));
        // The focus event is the evidence the client acts on.
        let focus = wait_event(&focus_reports);
        let (handler, reports) = Handler::make();
        let request = events::name_cache(&client).expect("a cache request");
        let property = events::as_property_handler(&handler);
        // SAFETY: live client, element, cache request and handler; the
        // property array is one id.
        let registered = unsafe {
            client.AddPropertyChangedEventHandlerNativeArray(
                &node,
                TreeScope_Element,
                &request,
                &property,
                &[UIA_NamePropertyId],
            )
        };
        provider.request(&format!("rename 0 {NODE} after"));
        provider.request(&format!("raise name 0 {NODE}"));
        uia::pause(LISTEN);
        let heard = events::drain(&reports).0;
        let names: Vec<String> = heard.iter().map(|h| h.name.clone()).collect();
        results.push((
            focus.is_some(),
            registered.is_ok(),
            names.contains(&"before".to_owned()),
            names.contains(&"after".to_owned()),
        ));
    }
    report.claim(
        "a name change raised between a focus event and a registration made for that focus, heard; one after it, heard",
        &runs(&results),
        "(focus heard, registered, before heard, after heard)",
        Verdict::New,
    );
}

fn wait_event(reports: &Receiver<Event>) -> Option<Heard> {
    match reports.recv_timeout(Duration::from_secs(10)) {
        Ok(Event::Heard(h)) => Some(h),
        _ => None,
    }
}
