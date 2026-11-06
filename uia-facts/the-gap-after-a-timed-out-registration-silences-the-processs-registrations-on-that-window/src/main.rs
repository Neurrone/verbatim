//! The gap after a timed-out registration silences the process's
//! registrations on that window
//! (`uia-facts/the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window/readme.md`):
//! e58's reach of the gap, a removal during it, and the silence after the
//! stall. The registration's return and the gap itself are
//! `uia-facts/a-registration-that-times-out-reports-success/`'s.
//!
//! Every registration is an event handler group holding UIA's focus event,
//! subtree scope, the sender's name cached, on the provider program's
//! window, as e58 made it, at both timeouts 500 ms, since the gap's end
//! follows neither timeout. Each focus event renames the next of nodes 5
//! to 8 in turn, raises its name change, moves the tree's focus to it and
//! raises its focus event, so the handler names the event it heard. A
//! stall is the window's UI thread busy until released, 200 ms after the
//! registration returns. Every trial runs in a client process of its own
//! (the `trial` role), as e58 ran each trial in a test process of its own.
//!
//! - `e58-silence`: a registration during a stall and no event raised; the
//!   provider's log read 3 s after the release.
//! - `e58-scope`: two provider programs, P and Q. Before the stall of P,
//!   this process's client object registers group A on P's window and C on
//!   Q's, and a second client process registers D on P's window. During
//!   the stall another client object registers B on P's window. Focus
//!   events are raised on P every 100 ms from the stall's end to 2,400 ms
//!   after it, and on Q every 100 ms from 50 to 2,350 ms.
//! - `e58-remove`: the group removed 500 ms after the release, in the gap
//!   (`remove`); once it has heard an event raised after the release, past
//!   the gap (`remove-late`); or with no stall (`remove-nostall`). After the
//!   removal, a focus event every 100 ms until the handler is freed or one
//!   is heard, up to 3 s; once freed, one more event, listened for 0.5 s.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use uia_facts_support::args::{self, Args, trials};
use uia_facts_support::events::{self, Handler, Heard, Report as Event};
use uia_facts_support::note;
use uia_facts_support::provider::{Entry, Provider};
use uia_facts_support::report::{Report, deterministic, relation, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::uia::{self, Timed};
use uia_facts_support::{exit_now, now_us};
use windows::Win32::UI::Accessibility::{
    IUIAutomation6, IUIAutomationElement, IUIAutomationEventHandlerGroup,
};

/// The nodes focus events cycle through, as e58 cycled four focusable
/// nodes of mockapp's tree.
const NODES: [usize; 4] = [5, 6, 7, 8];
/// The stall's release after the registration returns, e58's 200 ms.
const RELEASE_AFTER: Duration = Duration::from_millis(200);
/// How long events are listened for after the last raise, enough to hear
/// an event that arrives at all, since events after the gap arrived within
/// 1 to 11 ms.
const LISTEN: Duration = Duration::from_millis(500);
/// Both timeouts every trial sets: short, since the gap's end follows
/// neither.
const TIMEOUT_MS: u32 = 500;
/// How long `e58-silence` watches the provider's log after the release.
const SILENCE: Duration = Duration::from_secs(3);
/// How long a removal case raises events after the removal, waiting for
/// the handler's release or for an event heard.
const AFTER_REMOVAL: Duration = Duration::from_secs(3);

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
            "listen" => listen(rest[0].parse().expect("a window handle")),
            "trial" => trial(rest),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report = Report::start(
        "the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window",
    );
    if args.wants("e58-silence") {
        report.case("e58-silence", "registering during a stall and raising no event; watching the provider's log for 3 s after the release");
        e58_silence(&mut report);
    }
    if args.wants("e58-scope") {
        report.case("e58-scope", "registering groups on two providers, from two client objects and from a second process; stalling one provider; raising events on both");
        e58_scope(&mut report);
    }
    if args.wants("e58-remove") {
        report.case("e58-remove", "registering during a stall; removing the group 500 ms after the release, or once it has heard an event, or with no stall; then raising events until the handler is freed or one is heard");
        e58_remove(&mut report);
    }
    report.finish();
}

/// Whether the client objects have connection recovery on (`--recovery`):
/// the gap comes from it, so each gap claim is judged against the record
/// for the setting in use.
fn recovery() -> bool {
    uia::settings().recovery
}

/// Runs one trial in a client process of its own; answers the trial's
/// result words.
fn in_own_process(case: &str, params: &[&str]) -> Vec<String> {
    let mut words = vec![case.to_owned()];
    words.extend(params.iter().map(|p| (*p).to_owned()));
    let child = Role::start("trial", &words);
    let line = child
        .line(Duration::from_secs(120))
        .expect("the trial reports its result");
    let result = line
        .strip_prefix("result ")
        .unwrap_or_else(|| panic!("the trial said {line}"));
    result.split(' ').map(str::to_owned).collect()
}

/// The trial role: runs the named trial and reports `result <words>`.
fn trial(rest: &[String]) -> ! {
    let words: Vec<String> = match rest[0].as_str() {
        "e58-silence" => trial_silence(),
        "e58-scope" => trial_scope(),
        "removal" => {
            let (outcome, freed, heard) = removal(&rest[1]);
            vec![outcome, freed.to_string(), heard.to_string()]
        }
        other => panic!("unknown trial {other}"),
    };
    role::report(&format!("result {}", words.join(" ")));
    exit_now(0)
}

fn flag(word: &str) -> bool {
    word == "true"
}

/// The second client process: registers a focus group on window `hwnd`,
/// says `ready`, and reports `heard <name> <time>` for each event until
/// its standard input closes.
fn listen(hwnd: isize) -> ! {
    let client = uia::client();
    uia::set_timeouts(&client, TIMEOUT_MS, TIMEOUT_MS);
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

/// A provider program, a client with the settings in use and both
/// timeouts set, and its window's element fetched before any stall.
fn setup() -> (Provider, IUIAutomation6, IUIAutomationElement) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    args::pause_here(&[("provider", provider.pid), ("trial", std::process::id())]);
    let client = uia::client();
    uia::set_timeouts(&client, TIMEOUT_MS, TIMEOUT_MS);
    let element = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    (provider, client, element)
}

/// Raises one focus event on the provider's window: node
/// `NODES[i % 4]` renamed `name` with a name change raised, the tree's
/// focus moved to it and its focus event raised. Answers when the focus
/// event was raised.
fn raise(provider: &mut Provider, i: usize, name: &str) -> u64 {
    let node = NODES[i % NODES.len()];
    provider.request(&format!("rename 0 {node} {name}"));
    provider.request(&format!("raise name 0 {node}"));
    provider.request(&format!("focus 0 {node}"));
    provider
        .request(&format!("raise focus 0 {node}"))
        .parse()
        .expect("the time it was raised")
}

/// Raises a focus event at each offset after `from`, named
/// `<prefix><i>`; answers each event's name.
fn raise_at(
    provider: &mut Provider,
    prefix: &str,
    from: Instant,
    offsets_ms: &[u64],
    first: usize,
) -> Vec<String> {
    let mut raised = Vec::new();
    for (i, offset) in offsets_ms.iter().enumerate() {
        let due = from + Duration::from_millis(*offset);
        uia::pause(
            due.saturating_duration_since(Instant::now()),
            "until the next event is due",
        );
        let name = format!("{prefix}{i}");
        raise(provider, first + i, &name);
        raised.push(name);
    }
    raised
}

/// A registration made during a stall: the provider's UI thread busy, the
/// registration timed, the stall released 200 ms after it returns.
/// Answers when the stall was released.
fn register_stalled(
    provider: &mut Provider,
    client: &IUIAutomation6,
    element: &IUIAutomationElement,
    group: &IUIAutomationEventHandlerGroup,
) -> (Timed, u64) {
    provider.request("busy 0");
    let registration = Timed::of(|| events::add_group(client, element, group));
    uia::pause(
        RELEASE_AFTER,
        "until the stall is released, 200 ms after the registration returned",
    );
    let released = now_us();
    provider.request("release");
    (registration, released)
}

/// The names of the raised events not heard.
fn lost(raised: &[String], heard: &[Heard]) -> Vec<String> {
    raised
        .iter()
        .filter(|name| !heard.iter().any(|h| h.name == **name))
        .cloned()
        .collect()
}

/// Whether `entry` is the provider's advise of a focus event handler.
fn is_focus_advise(entry: &Entry) -> bool {
    entry.kind == "advise" && entry.name == "AdviseEventAdded" && entry.extra == "20005"
}

/// The end of the stall's backlog: the registration's own request runs as
/// the stall ends, on the window's root, and ends with the provider's
/// advise of the focus event (`FragmentRoot`, `GetEmbeddedFragmentRoots`,
/// `ProviderOptions` on node 0, then `AdviseEventAdded`, by the provider's
/// log). So the end of the first advise after the release, when every
/// entry from the release up to it is a call on the root. `None` when
/// there is no such advise, or something else came between.
fn backlog_end(log: &[Entry], released_us: u64) -> Option<u64> {
    let mut after: Vec<&Entry> = log.iter().filter(|e| e.start >= released_us).collect();
    after.sort_by_key(|e| e.start);
    let advise = after.iter().position(|e| is_focus_advise(e))?;
    let unbroken = after[..advise]
        .iter()
        .all(|e| e.kind == "call" && e.window == Some(0) && e.node == Some(0));
    unbroken.then(|| after[advise].end)
}

/// One e58 silence trial: each focus advise's place (whether it ends the
/// stall's backlog), comma-separated or `-` when none, and the count of
/// log entries after the backlog.
fn trial_silence() -> Vec<String> {
    let (mut provider, client, element) = setup();
    let (handler, _reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    provider.log();
    let (_, released) = register_stalled(&mut provider, &client, &element, &group);
    // The silence is the claim: seconds with nothing reaching the
    // provider after the release.
    uia::pause(
        SILENCE,
        "watching the provider log for 3 s of silence after the release",
    );
    let log = provider.log();
    let backlog = backlog_end(&log, released);
    let adds: Vec<String> = log
        .iter()
        .filter(|e| is_focus_advise(e))
        .map(|e| (Some(e.end) == backlog).to_string())
        .collect();
    let adds = if adds.is_empty() {
        "-".to_owned()
    } else {
        adds.join(",")
    };
    // With no backlog found, everything after the release counts.
    let quiet_from = backlog.unwrap_or(released);
    vec![
        adds,
        log.iter()
            .filter(|e| e.start > quiet_from)
            .count()
            .to_string(),
    ]
}

fn e58_silence(report: &mut Report) {
    let mut advised = Vec::new();
    let mut silent: Vec<usize> = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("e58-silence", &[]);
        advised.push(
            words[0]
                .split(',')
                .filter(|w| *w != "-")
                .map(flag)
                .collect::<Vec<_>>(),
        );
        silent.push(words[1].parse().expect("a count"));
    }
    report.claim(
        "the provider is advised of the focus event once, at the stall's end",
        &runs(&advised),
        "one advise, after the release, ending the stall's backlog",
        deterministic(&advised, &vec![true]),
    );
    report.claim(
        "with no event raised, nothing reaches the provider in the 3 s after the stall's end",
        &runs(&silent),
        "0 entries (in 6 s, e58)",
        deterministic(&silent, &0),
    );
}

fn e58_scope(report: &mut Report) {
    let mut same_loss = Vec::new();
    let mut other_window: Vec<usize> = Vec::new();
    let mut other_process: Vec<usize> = Vec::new();
    let mut a_and_b: Vec<(usize, usize)> = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("e58-scope", &[]);
        same_loss.push(flag(&words[0]));
        other_window.push(words[1].parse().expect("a count"));
        other_process.push(words[2].parse().expect("a count"));
        a_and_b.push((
            words[3].parse::<usize>().expect("a count"),
            words[4].parse::<usize>().expect("a count"),
        ));
        note::evidence(&format!(
            "A lost {}, B lost {}, C lost {}, D lost {}",
            words[3], words[4], words[1], words[2]
        ));
    }
    if recovery() {
        report.claim(
            "A, registered before the stall on another client object, loses the same events as B",
            &runs(&same_loss),
            "true",
            relation(&same_loss),
        );
    } else {
        report.claim(
            "with connection recovery off, the events A and B lose",
            &runs(&a_and_b),
            "(0, 0)",
            deterministic(&a_and_b, &(0, 0)),
        );
    }
    report.claim(
        "C, on another process's window, loses no event",
        &runs(&other_window),
        "0",
        deterministic(&other_window, &0),
    );
    report.claim(
        "D, in another client process on the same window, loses no event",
        &runs(&other_process),
        "0",
        deterministic(&other_process, &0),
    );
}

/// One e58 scope trial: whether A lost exactly B's events, and how many C,
/// D, A and B lost.
fn trial_scope() -> Vec<String> {
    let (mut p_app, client, p_element) = setup();
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
    // B registers on a thread of its own, which keeps its client object
    // and group until the trial ends.
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
    let (b_ok, b_reports) = b_registered.recv().expect("B registers");
    assert!(b_ok, "B reports success");
    uia::pause(
        RELEASE_AFTER,
        "until the stall is released, 200 ms after the registration returned",
    );
    p_app.request("release");
    let released = Instant::now();
    let p_offsets: Vec<u64> = (0..=24).map(|i| i * 100).collect();
    let q_offsets: Vec<u64> = (0..24).map(|i| 50 + i * 100).collect();
    let mut p_raised = Vec::new();
    let mut q_raised = Vec::new();
    for (i, offset) in p_offsets.iter().enumerate() {
        p_raised.extend(raise_at(
            &mut p_app,
            &format!("p{i}-"),
            released,
            &[*offset],
            i,
        ));
        if let Some(q_offset) = q_offsets.get(i) {
            q_raised.extend(raise_at(
                &mut q_app,
                &format!("q{i}-"),
                released,
                &[*q_offset],
                i,
            ));
        }
    }
    uia::pause(LISTEN, "listening after the last event");
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
                thread: 0,
            });
        }
    }
    let d_lost = lost(&p_raised, &d_heard);
    let _ = end_b.send(());
    let _ = thread_b.join();
    vec![
        (!a_lost.is_empty() && a_lost == b_lost).to_string(),
        c_lost.len().to_string(),
        d_lost.len().to_string(),
        a_lost.len().to_string(),
        b_lost.len().to_string(),
    ]
}

/// The events a handler heard and whether it was freed, from `reports`
/// over `limit`.
fn watch(
    reports: &std::sync::mpsc::Receiver<Event>,
    heard: &mut Vec<Heard>,
    freed: &mut bool,
    limit: Duration,
) {
    let until = Instant::now() + limit;
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match reports.recv_timeout(left) {
            Ok(Event::Heard(h)) => heard.push(h),
            Ok(Event::Freed) => *freed = true,
            Err(_) => return,
        }
    }
}

/// One removal trial, `when` being `remove`, `remove-late` or
/// `remove-nostall`. Answers the removal's outcome, whether the handler
/// was freed, and whether an event raised after the removal was heard.
fn removal(when: &str) -> (String, bool, bool) {
    let (mut provider, client, element) = setup();
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    drop(handler);
    let mut heard: Vec<Heard> = Vec::new();
    let mut freed = false;
    let mut count = 0;
    if when == "remove-nostall" {
        events::add_group(&client, &element, &group).expect("the registration");
    } else {
        register_stalled(&mut provider, &client, &element, &group);
        if when == "remove" {
            uia::pause(
                Duration::from_millis(500),
                "until the removal, 500 ms after the release",
            );
        } else {
            // Past the gap: once an event raised after the release is
            // heard, raised every 100 ms.
            note::waiting("up to 3 s, an event heard after the release");
            let until = Instant::now() + AFTER_REMOVAL;
            while heard.is_empty() && Instant::now() < until {
                raise(&mut provider, count, &format!("before{count}"));
                count += 1;
                watch(&reports, &mut heard, &mut freed, Duration::from_millis(100));
            }
            assert!(!heard.is_empty(), "the registration delivers after the gap");
        }
    }
    let removed = Timed::of(|| events::remove_group(&client, &element, &group));
    drop(group);
    heard.clear();
    // After the removal: an event every 100 ms until the handler is freed
    // or one is heard; once freed, one more event, listened for 0.5 s.
    note::waiting("up to 3 s, the handler's release or an event heard");
    let until = Instant::now() + AFTER_REMOVAL;
    while heard.is_empty() && !freed && Instant::now() < until {
        raise(&mut provider, count, &format!("after{count}"));
        count += 1;
        watch(&reports, &mut heard, &mut freed, Duration::from_millis(100));
    }
    if freed && heard.is_empty() {
        raise(&mut provider, count, &format!("after{count}"));
        watch(&reports, &mut heard, &mut freed, LISTEN);
    }
    note::evidence(&format!(
        "{when}: removal {} after {:.1} ms; freed {freed}; heard after it {:?}",
        removed.outcome,
        removed.ms(),
        heard.iter().map(|h| h.name.as_str()).collect::<Vec<_>>()
    ));
    (removed.outcome, freed, !heard.is_empty())
}

/// A removal case's outcome: the removal's, whether the handler was
/// freed, and whether an event raised after the removal was heard.
type Removal = (String, bool, bool);

fn e58_remove(report: &mut Report) {
    let gone = ("ok".to_owned(), true, false);
    let cases: [(&str, Removal); 3] = [
        (
            "remove",
            if recovery() {
                ("UIA_E_TIMEOUT".to_owned(), false, true)
            } else {
                gone.clone()
            },
        ),
        ("remove-late", gone.clone()),
        ("remove-nostall", gone.clone()),
    ];
    let words: HashMap<&str, &str> = [
        ("remove", "500 ms after the release, in the gap"),
        ("remove-late", "once it has heard an event, past the gap"),
        ("remove-nostall", "with no stall"),
    ]
    .into_iter()
    .collect();
    for (name, recorded) in cases {
        let trials: Vec<Removal> = (0..trials())
            .map(|_| {
                let words = in_own_process("removal", &[name]);
                (words[0].clone(), flag(&words[1]), flag(&words[2]))
            })
            .collect();
        report.claim(
            &format!(
                "{name}, removed {}: (the removal's outcome, the handler freed, an event raised after it heard)",
                words[name]
            ),
            &runs(&trials),
            &format!("{recorded:?}"),
            deterministic(&trials, &recorded),
        );
    }
}
