//! A registration that times out reports success
//! (`uia-facts/a-registration-that-times-out-reports-success/readme.md`).
//!
//! Every registration is an event handler group holding UIA's focus event,
//! subtree scope, the sender's name cached, on the provider program's
//! window, as e57 and e58 made it. Each focus event is raised as e57 raised
//! it through mockapp's `set-name` and `focus`: the next of nodes 5 to 8 in
//! turn is renamed `ev<i>` with a name change raised, then the tree's focus
//! moves to it and its focus event is raised, so the handler names the
//! event it heard. A stall is the window's UI thread busy until released.
//! Every trial runs in a client process of its own (the `trial` role), as
//! e57 and e58 ran each trial in a test process of its own, so no other
//! trial's registration is in the process.
//!
//! - `ett`: E-TT's registration: both timeouts at 1,000 ms, the provider
//!   holding `FragmentRoot`.
//! - `connection`: a registration during a stall returns after the
//!   connection timeout, whatever the transaction timeout: 500 and
//!   5,000 ms, and the reverse.
//! - `e57`: connection timeout 2,000 ms, transaction timeout 5,000 ms; the
//!   group registered during a stall, the stall released 200 ms after the
//!   registration returns, and a focus event raised every 50 ms from the
//!   stall's end to 2,500 ms after it; heard events listened for 0.5 s
//!   after the last. Then the same at a 500 ms connection timeout and at a
//!   500 ms transaction timeout; with events every 10 ms from 1,700 to
//!   1,850 ms after the stall's end; and with no stall, events from the
//!   return.
//! - `release-later`: the stall released 1,000 and 3,000 ms after the
//!   return; events every 50 ms from the stall's end.
//! - `next-event`: a single event 1,500 ms after the stall's end, and the
//!   pair at 1,000 and 2,500 ms.
//! - `second-process`: `e57` registered by a second client process.
//! - `onset`: `e58-silence` with a focus event every 100 ms from the
//!   stall's end to 3,000 ms after it, and the provider's log.
//! - `e58-silence`: as `e57` with no event raised, the provider's log read
//!   6 s after the release.
//! - `e58-scope`: two provider programs, P and Q. Before the stall of P,
//!   this process's client object registers group A on P's window and C on
//!   Q's, and a second client process registers D on P's window. During
//!   the stall another client object registers B on P's window. Focus
//!   events are raised on P every 100 ms from the stall's end to 3,000 ms
//!   after it, and on Q every 100 ms from 50 to 2,950 ms.
//! - `e58-remove`: as `e57`, the group removed 500 ms after the release
//!   (`remove`), 3,000 ms after it (`remove-late`), or with no stall
//!   (`remove-nostall`); focus events at 1,000 and 4,000 ms after the
//!   release; the handler's release watched for 5 s.
//! - `between`: the inferred loss of events raised between a focus change
//!   and a registration made for the new focus: a name change raised just
//!   after the focus event, and one raised after the registration returns.

use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use uia_facts_support::args::{self, Args, trials};
use uia_facts_support::events::{self, Handler, Heard, Report as Event};
use uia_facts_support::note;
use uia_facts_support::provider::{Entry, Provider};
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

/// The node `between` raises its events on.
const NODE: usize = 5;
/// The nodes focus events cycle through, as e57 cycled four focusable
/// nodes of mockapp's tree.
const NODES: [usize; 4] = [5, 6, 7, 8];
/// The stall's release after the registration returns, e57's 200 ms.
const RELEASE_AFTER: Duration = Duration::from_millis(200);
/// How long events are listened for after the last raise, enough to hear
/// an event that arrives at all, since events after the gap arrived within
/// 1 to 11 ms. No case that listens this long judges an event lost rather
/// than late.
const LISTEN: Duration = Duration::from_millis(500);
/// How long the `e57` trial listens after the last raise, as e57 listened
/// 2 to 4 s: its "lost, not late" claim holds that no event lost arrives
/// later, so it listens past the gap's 2 s.
const LISTEN_E57: Duration = Duration::from_secs(3);

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
            "listen" => listen(rest[0].parse().expect("a window handle")),
            "registrar" => registrar(rest),
            "trial" => trial(rest),
            other => panic!("unknown role {other}"),
        }
    }
    let mut report = Report::start("a-registration-that-times-out-reports-success");
    if args.wants("ett") {
        report.case("ett", "holding the provider's FragmentRoot with both timeouts at 1 s; registering a focus handler group and timing the call");
        ett(&mut report);
    }
    if args.wants("connection") {
        report.case("connection", "stalling the provider's UI thread; registering at a 500 ms connection and 5 s transaction timeout, then the reverse; timing the call");
        connection(&mut report, 500, 5_000, "532 to 551 ms");
        connection(&mut report, 5_000, 500, "5,453 to 5,541 ms");
    }
    if args.wants("e57") {
        report.case("e57", "registering a focus handler while the provider's UI thread is stalled; releasing 200 ms after the call returns; raising a focus event every 50 ms; then the same at other timeouts, at 10 ms steps near the gap's end, and with no stall");
        e57(&mut report);
        timeouts(&mut report);
        edge(&mut report);
        control(&mut report);
    }
    if args.wants("release-later") {
        report.case("release-later", "registering during a stall; releasing it 1 to 3 s after the call returns, or letting it outlast the gap; raising focus events after it");
        release_later(&mut report);
    }
    if args.wants("next-event") {
        report.case("next-event", "registering during a stall; raising one focus event 1.5 or 2 s after the stall's end, or a pair at 1 and 2.5 s");
        next_event(&mut report);
    }
    if args.wants("second-process") {
        report.case("second-process", "registering from a second client process during a stall; raising a focus event every 50 ms after the stall's end");
        second_process(&mut report);
    }
    if args.wants("onset") {
        report.case("onset", "registering during a stall; raising a focus event every 100 ms for 3 s; reading the provider's log for work after each event");
        onset(&mut report);
    }
    if args.wants("e58-silence") {
        report.case("e58-silence", "registering during a stall and raising no event; watching the provider's log for 6 s after the release");
        e58_silence(&mut report);
    }
    if args.wants("e58-scope") {
        report.case("e58-scope", "registering groups on two providers, from two client objects and from a second process; stalling one provider; raising events on both");
        e58_scope(&mut report);
    }
    if args.wants("e58-remove") {
        report.case("e58-remove", "registering during a stall; removing the group 500 ms or 3 s after the release, or with no stall; raising events at 1 and 4 s");
        e58_remove(&mut report);
    }
    if args.wants("between") {
        report.case("between", "raising a focus event and a name change at once; registering for name changes on hearing the focus; raising a second name change");
        between(&mut report);
    }
    report.finish();
}

/// Runs one trial in a client process of its own, as e57 and e58 ran each
/// trial in a test process of its own, so no registration of another trial
/// or case is in the process; answers the trial's result words.
/// Whether the client objects have connection recovery on (`--recovery`):
/// the gap after a timed-out registration comes from it, so each gap claim
/// is judged against the record for the setting in use, and with recovery
/// off no event is lost.
fn recovery() -> bool {
    uia::settings().recovery
}

/// The offsets after the stall's end, clear of the gap's end, up to which
/// every event is lost and from which every event is heard: with
/// connection recovery on, `lost_by` and `heard_from`, each about 300 ms
/// from the recorded end so a tick of jitter cannot move an event across
/// it; with recovery off, none lost and every one heard.
fn clear_of_gap(lost_by: u64, heard_from: u64) -> (Option<u64>, u64) {
    if recovery() {
        (Some(lost_by), heard_from)
    } else {
        (None, 0)
    }
}

/// The text of [`clear_of_gap`]'s answer.
fn clear_text((lost_by, heard_from): (Option<u64>, u64)) -> String {
    match lost_by {
        Some(lost_by) => {
            format!("every event lost up to {lost_by} ms and heard from {heard_from} ms")
        }
        None => "every event heard".to_owned(),
    }
}

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
    let number = |i: usize| -> u64 { rest[i].parse().expect("a number") };
    let words: Vec<String> = match rest[0].as_str() {
        "ett" => trial_ett(),
        "connection" => trial_connection(
            u32::try_from(number(1)).expect("a timeout"),
            u32::try_from(number(2)).expect("a timeout"),
        ),
        "e57" => trial_e57(),
        "e58-silence" => trial_silence(),
        "e58-scope" => trial_scope(),
        "removal" => {
            let removal = removal(rest[1] == "stall", Duration::from_millis(number(2)));
            vec![
                removal.0,
                removal.1.to_string(),
                removal.2.to_string(),
                removal.3.to_string(),
            ]
        }
        "between" => trial_between(),
        "gap" => {
            let offsets: Vec<u64> = rest[6]
                .split(',')
                .map(|o| o.parse().expect("an offset"))
                .collect();
            trial_gap(
                u32::try_from(number(1)).expect("a timeout"),
                u32::try_from(number(2)).expect("a timeout"),
                rest[3] == "stall",
                number(4),
                rest[5] == "remote",
                &offsets,
            )
        }
        "onset" => trial_onset(),
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

/// A provider program, a client with the settings in use and the
/// timeouts set, and its window's element fetched before any stall.
fn setup(
    connection_ms: u32,
    transaction_ms: u32,
) -> (Provider, IUIAutomation6, IUIAutomationElement) {
    let provider = Provider::launch(&["--windows", "uia:0"]);
    args::pause_here(&[("provider", provider.pid), ("trial", std::process::id())]);
    let client = uia::client();
    uia::set_timeouts(&client, connection_ms, transaction_ms);
    let element = uia::element(&client, provider.hwnd(0)).expect("the window's element");
    (provider, client, element)
}

/// Raises a focus event at each offset after `from`, as e57 did with
/// mockapp's `set-name` and `focus`: the next of [`NODES`] in turn, from
/// the `first`'th, is renamed `<prefix><i>` with a name change raised, then
/// the tree's focus moves to it and its focus event is raised. Answers each
/// event's name and when its focus event was raised.
fn raise_at(
    provider: &mut Provider,
    window: usize,
    prefix: &str,
    from: Instant,
    offsets_ms: &[u64],
    first: usize,
) -> Vec<(String, u64)> {
    let mut raised = Vec::new();
    for (i, offset) in offsets_ms.iter().enumerate() {
        let due = from + Duration::from_millis(*offset);
        uia::pause(
            due.saturating_duration_since(Instant::now()),
            "until the next event is due",
        );
        let node = NODES[(first + i) % NODES.len()];
        let name = format!("{prefix}{i}");
        provider.request(&format!("rename {window} {node} {name}"));
        provider.request(&format!("raise name {window} {node}"));
        provider.request(&format!("focus {window} {node}"));
        let at: u64 = provider
            .request(&format!("raise focus {window} {node}"))
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
    note::waiting_ms(
        uia::timeouts(client).0,
        "the registration's connection timeout",
    );
    let registration = Timed::of(|| events::add_group(client, element, group));
    uia::pause(
        RELEASE_AFTER,
        "until the stall is released, 200 ms after the registration returned",
    );
    let released_us = now_us();
    provider.request("release");
    Stalled {
        registration,
        released: Instant::now(),
        released_us,
    }
}

fn trial_ett() -> Vec<String> {
    let (mut provider, client, element) = setup(1_000, 1_000);
    let (handler, _reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    provider.request("hold FragmentRoot");
    note::waiting_ms(
        uia::timeouts(&client).0,
        "the registration's connection timeout",
    );
    let registration = Timed::of(|| events::add_group(&client, &element, &group));
    provider.request("release");
    vec![registration.outcome.clone(), registration.ms().to_string()]
}

fn ett(report: &mut Report) {
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("ett", &[]);
        outcomes.push(words[0].clone());
        times.push(words[1].parse().expect("a time"));
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

fn trial_connection(connection_ms: u32, transaction_ms: u32) -> Vec<String> {
    let (mut provider, client, element) = setup(connection_ms, transaction_ms);
    let (handler, _reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    let stalled = register_stalled(&mut provider, &client, &element, &group);
    vec![
        stalled.registration.outcome.clone(),
        stalled.registration.ms().to_string(),
    ]
}

fn connection(report: &mut Report, connection_ms: u32, transaction_ms: u32, recorded: &str) {
    let mut times = Vec::new();
    let mut outcomes = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process(
            "connection",
            &[&connection_ms.to_string(), &transaction_ms.to_string()],
        );
        outcomes.push(words[0].clone());
        times.push(words[1].parse().expect("a time"));
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

/// One e57 trial: the registration's outcome and time; whether every event
/// up to 1,500 ms was lost, every one from 2,300 ms heard, every heard one
/// prompt, and none lost after the first heard; and the last lost and first
/// heard event's raise after the return, in ms, or `none`.
fn trial_e57() -> Vec<String> {
    let offsets: Vec<u64> = (0..=50).map(|i| i * 50).collect();
    let (mut provider, client, element) = setup(2_000, 5_000);
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    let stalled = register_stalled(&mut provider, &client, &element, &group);
    let raised = raise_at(&mut provider, 0, "ev", stalled.released, &offsets, 0);
    uia::pause(
        LISTEN_E57,
        "listening after the last event, so an event lost is not one heard late",
    );
    let (heard, _) = events::drain(&reports);
    let delays = delays(&raised, &heard);
    let at = |i: usize| offsets[i];
    let early_lost = (0..delays.len())
        .filter(|i| at(*i) <= 1_500)
        .all(|i| delays[i].is_none());
    let late_heard = (0..delays.len())
        .filter(|i| at(*i) >= 2_300)
        .all(|i| delays[i].is_some());
    let prompt = delays.iter().flatten().all(|d| *d < 100.0);
    let first = delays.iter().position(Option::is_some);
    let no_loss_after = first.is_some_and(|f| delays[f..].iter().all(Option::is_some));
    let returned = stalled.registration.end;
    let after = |t: Option<u64>| {
        t.map_or("none".to_owned(), |t| {
            format!("{:.0}", us_to_ms(t.saturating_sub(returned)))
        })
    };
    vec![
        stalled.registration.outcome.clone(),
        stalled.registration.ms().to_string(),
        early_lost.to_string(),
        late_heard.to_string(),
        prompt.to_string(),
        no_loss_after.to_string(),
        after(first.and_then(|f| f.checked_sub(1)).map(|i| raised[i].1)),
        after(first.map(|f| raised[f].1)),
        delays.iter().all(Option::is_some).to_string(),
        // Each event's offset after the stall's end and its delay, or
        // `lost`, for the evidence printed under a claim that differs.
        offsets
            .iter()
            .zip(&delays)
            .map(|(offset, delay)| {
                delay.map_or(format!("{offset}:lost"), |d| format!("{offset}:{d:.0}"))
            })
            .collect::<Vec<_>>()
            .join(","),
    ]
}

fn e57(report: &mut Report) {
    let mut outcomes = Vec::new();
    let mut times = Vec::new();
    let mut early_lost = Vec::new();
    let mut late_heard = Vec::new();
    let mut prompt = Vec::new();
    let mut no_loss_after = Vec::new();
    let mut all_heard = Vec::new();
    let mut edges = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("e57", &[]);
        all_heard.push(flag(&words[8]));
        outcomes.push(words[0].clone());
        times.push(words[1].parse().expect("a time"));
        early_lost.push(flag(&words[2]));
        late_heard.push(flag(&words[3]));
        prompt.push(flag(&words[4]));
        no_loss_after.push(flag(&words[5]));
        edges.push(format!(
            "last lost {} ms, first heard {} ms after the return",
            words[6], words[7]
        ));
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
    if recovery() {
        report.claim(
            "every event raised up to 1,500 ms after the stall's end is lost",
            &runs(&early_lost),
            "true (lost up to 1,750 ms)",
            relation(&early_lost),
        );
    } else {
        report.claim(
            "with connection recovery off, every event raised from the stall's end is heard",
            &runs(&all_heard),
            "true",
            relation(&all_heard),
        );
    }
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

/// One e58 silence trial: each focus advise's timing (within 20 ms of the
/// release or not), comma-separated or `-` when none, and the count of log
/// entries after the backlog.
fn trial_silence() -> Vec<String> {
    let (mut provider, client, element) = setup(2_000, 5_000);
    let (handler, _reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    provider.log();
    let stalled = register_stalled(&mut provider, &client, &element, &group);
    // The silence is the claim: six seconds with nothing reaching the
    // provider, as e58 watched.
    uia::pause(
        Duration::from_secs(6),
        "watching the provider log for 6 s of silence after the release",
    );
    let log = provider.log();
    let backlog_end = backlog_end(&log, stalled.released_us);
    // Each advise: whether it is the one that ends the stall's backlog.
    let adds: Vec<String> = log
        .iter()
        .filter(|e| is_focus_advise(e))
        .map(|e| (Some(e.end) == backlog_end).to_string())
        .collect();
    let adds = if adds.is_empty() {
        "-".to_owned()
    } else {
        adds.join(",")
    };
    // With no backlog found, everything after the release counts.
    let quiet_from = backlog_end.unwrap_or(stalled.released_us);
    vec![
        adds,
        log.iter()
            .filter(|e| e.start > quiet_from)
            .count()
            .to_string(),
    ]
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
/// entry from the release up to it is a call on the root: an unbroken run
/// of the registration's request. `None` when there is no such advise, or
/// something else came between.
fn backlog_end(log: &[Entry], released_us: u64) -> Option<u64> {
    let mut after: Vec<&Entry> = log.iter().filter(|e| e.start >= released_us).collect();
    after.sort_by_key(|e| e.start);
    let advise = after.iter().position(|e| is_focus_advise(e))?;
    let unbroken = after[..advise]
        .iter()
        .all(|e| e.kind == "call" && e.window == Some(0) && e.node == Some(0));
    unbroken.then(|| after[advise].end)
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
        "with no event raised, nothing reaches the provider in the 6 s after the stall's end",
        &runs(&silent),
        "0 entries",
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
        a_and_b.push((
            words[3].parse::<usize>().expect("a count"),
            words[4].parse::<usize>().expect("a count"),
        ));
        other_window.push(words[1].parse().expect("a count"));
        other_process.push(words[2].parse().expect("a count"));
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

/// One e58 scope trial: whether A lost exactly B's events, and how many C
/// and D lost.
fn trial_scope() -> Vec<String> {
    {
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
        note::waiting_ms(2_000, "B's registration's connection timeout");
        let (b_ok, b_reports) = b_registered.recv().expect("B registers");
        assert!(b_ok, "B reports success");
        uia::pause(
            RELEASE_AFTER,
            "until the stall is released, 200 ms after the registration returned",
        );
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
                i,
            ));
            if let Some(q_offset) = q_offsets.get(i) {
                q_raised.extend(raise_at(
                    &mut q_app,
                    0,
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
}

/// The names of the raised events not heard.
fn lost(raised: &[(String, u64)], heard: &[Heard]) -> Vec<String> {
    raised
        .iter()
        .filter(|(name, _)| !heard.iter().any(|h| h.name == *name))
        .map(|(name, _)| name.clone())
        .collect()
}

/// How long a removal case watches for the handler's release: a release
/// comes within milliseconds of a removal that succeeds, so its absence
/// over this bound is the evidence that a failed removal left it in place.
const FREED_WATCH: Duration = Duration::from_secs(5);

/// A removal case's outcome: the removal's, whether the handler was freed
/// within [`FREED_WATCH`], and whether the events at 1,000 and 4,000 ms
/// were heard.
type Removal = (String, bool, bool, bool);

/// One removal case: the removal's outcome, whether the handler was freed
/// within [`FREED_WATCH`], and whether the events at 1,000 and 4,000 ms
/// were heard.
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
                raised.len(),
            ));
        } else {
            uia::pause(
                (released + at).saturating_duration_since(Instant::now()),
                "until the next event is due",
            );
            removed = Some(Timed::of(|| {
                events::remove_group(&client, &element, &group)
            }));
        }
    }
    drop(group);
    let removed = removed.expect("the removal ran");
    note::waiting(&format!(
        "up to {} s, watching for the handler's release",
        FREED_WATCH.as_secs()
    ));
    let freed_by = Instant::now() + FREED_WATCH;
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
        // With connection recovery off there is no gap: the removal
        // succeeds and the events before it are heard.
        let recorded = match (recovery(), name) {
            (false, "remove") => ("ok".to_owned(), true, false, false),
            (false, "remove-late") => ("ok".to_owned(), true, true, false),
            _ => recorded,
        };
        let trials: Vec<Removal> = (0..trials())
            .map(|_| {
                let words = in_own_process(
                    "removal",
                    &[if stall { "stall" } else { "nostall" }, &after.to_string()],
                );
                (
                    words[0].clone(),
                    flag(&words[1]),
                    flag(&words[2]),
                    flag(&words[3]),
                )
            })
            .collect();
        report.claim(
            &format!("{name}: the removal's outcome, the handler freed within 5 s, the events at 1,000 and 4,000 ms heard"),
            &runs(&trials),
            &format!("{recorded:?}"),
            deterministic(&trials, &recorded),
        );
    }
}

fn between(report: &mut Report) {
    let mut results = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("between", &[]);
        results.push((
            flag(&words[0]),
            flag(&words[1]),
            flag(&words[2]),
            flag(&words[3]),
        ));
    }
    report.claim(
        "(the focus heard, the registration made, a name change raised between them heard, one after it heard)",
        &runs(&results),
        "(true, true, false, true)",
        deterministic(&results, &(true, true, false, true)),
    );
}

/// One trial of `between`: the focus heard, the registration made, and the
/// name changes before and after it heard.
fn trial_between() -> Vec<String> {
    {
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
        uia::pause(LISTEN, "listening after the last event");
        let heard = events::drain(&reports).0;
        let names: Vec<String> = heard.iter().map(|h| h.name.clone()).collect();
        vec![
            focus.is_some().to_string(),
            registered.is_ok().to_string(),
            names.contains(&"before".to_owned()).to_string(),
            names.contains(&"after".to_owned()).to_string(),
        ]
    }
}

/// e57's registering client in a second process: makes a client with
/// the combination it was started with and the timeouts given, fetches the window's element,
/// says `ready`; on `go` registers the focus group and says
/// `registered <outcome> <start> <end>`, then `heard <name> <time>` for each
/// event, until its standard input closes.
fn registrar(rest: &[String]) -> ! {
    let hwnd: isize = rest[0].parse().expect("a window handle");
    let connection: u32 = rest[1].parse().expect("a timeout");
    let transaction: u32 = rest[2].parse().expect("a timeout");
    let client = uia::client();
    uia::set_timeouts(&client, connection, transaction);
    let element = uia::element(&client, hwnd).expect("the window's element");
    role::report("ready");
    let mut commands = role::commands();
    let _ = commands.next();
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    note::waiting_ms(
        uia::timeouts(&client).0,
        "the registration's connection timeout",
    );
    let registration = Timed::of(|| events::add_group(&client, &element, &group));
    std::thread::spawn(move || {
        for report in reports {
            if let Event::Heard(heard) = report {
                role::report(&format!("heard {} {}", heard.name, heard.at));
            }
        }
    });
    role::report(&format!(
        "registered {} {} {}",
        registration.outcome, registration.start, registration.end
    ));
    for _ in commands {}
    exit_now(0)
}

/// One trial of an e57 variant: a registration with the timeouts given,
/// made in this process or, with `remote`, by a second client process,
/// during a stall released `release_after` ms after it returns, or with no
/// stall; then a focus event at each offset after the stall's end (the
/// return, with no stall), listened for [`LISTEN`] after the last. Answers the
/// registration's outcome and time, and each event as
/// `<offset>:<raised after the return, ms>:<delay ms or x>`.
fn trial_gap(
    connection: u32,
    transaction: u32,
    stall: bool,
    release_after: u64,
    remote: bool,
    offsets: &[u64],
) -> Vec<String> {
    let (mut provider, client, element) = setup(connection, transaction);
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    let mut registrar = remote.then(|| {
        let role = Role::start(
            "registrar",
            &[
                provider.hwnd(0).to_string(),
                connection.to_string(),
                transaction.to_string(),
            ],
        );
        assert_eq!(role.expect_line(), "ready", "the registrar is ready");
        role
    });
    if stall {
        provider.request("busy 0");
        note::waiting_ms(connection, "the registration's connection timeout");
    }
    let (outcome, returned) = if let Some(role) = registrar.as_mut() {
        role.send("go");
        let line = role.expect_line();
        let words: Vec<&str> = line.split(' ').collect();
        assert_eq!(words[0], "registered", "the registrar registers");
        let start: u64 = words[2].parse().expect("a time");
        let end: u64 = words[3].parse().expect("a time");
        (format!("{} {}", words[1], us_to_ms(end - start)), end)
    } else {
        let registration = Timed::of(|| events::add_group(&client, &element, &group));
        (
            format!("{} {}", registration.outcome, registration.ms()),
            registration.end,
        )
    };
    if stall {
        uia::pause(
            Duration::from_millis(release_after),
            "until the stall is released",
        );
        provider.request("release");
    }
    let from = Instant::now();
    let raised = raise_at(&mut provider, 0, "ev", from, offsets, 0);
    uia::pause(LISTEN, "listening after the last event");
    let heard: Vec<Heard> = match &registrar {
        Some(role) => {
            let mut heard = Vec::new();
            while let Some(line) = role.line(Duration::from_millis(100)) {
                let words: Vec<&str> = line.split(' ').collect();
                if let ["heard", name, at] = words.as_slice() {
                    heard.push(Heard {
                        name: (*name).to_owned(),
                        id: 0,
                        at: at.parse().expect("a time"),
                        thread: 0,
                    });
                }
            }
            heard
        }
        None => events::drain(&reports).0,
    };
    let delays = delays(&raised, &heard);
    let events: Vec<String> = offsets
        .iter()
        .zip(&raised)
        .zip(&delays)
        .map(|((offset, (_, at)), delay)| {
            format!(
                "{offset}:{:.0}:{}",
                us_to_ms(at.saturating_sub(returned)),
                delay.map_or("x".to_owned(), |d| format!("{d:.0}"))
            )
        })
        .collect();
    let mut words: Vec<String> = outcome.split(' ').map(str::to_owned).collect();
    words.push(events.join(","));
    words
}

/// One event of a [`trial_gap`]: its offset after the stall's end, when it
/// was raised after the registration returned, and its delay when heard.
struct GapEvent {
    offset: u64,
    after_return: f64,
    delay: Option<f64>,
}

/// A [`trial_gap`]'s result.
struct Gap {
    outcome: String,
    took: f64,
    events: Vec<GapEvent>,
}

impl Gap {
    fn parse(words: &[String]) -> Self {
        let events = words[2]
            .split(',')
            .map(|event| {
                let parts: Vec<&str> = event.split(':').collect();
                GapEvent {
                    offset: parts[0].parse().expect("an offset"),
                    after_return: parts[1].parse().expect("a time"),
                    delay: parts[2].parse().ok(),
                }
            })
            .collect();
        Self {
            outcome: words[0].clone(),
            took: words[1].parse().expect("a time"),
            events,
        }
    }

    /// The last offset lost before the first heard, and the first heard.
    fn edge(&self) -> (Option<u64>, Option<u64>) {
        let first = self.events.iter().position(|e| e.delay.is_some());
        let last_lost = first
            .and_then(|f| f.checked_sub(1))
            .map(|i| self.events[i].offset)
            .or_else(|| {
                first
                    .is_none()
                    .then(|| self.events.last().map(|e| e.offset))
                    .flatten()
            });
        (last_lost, first.map(|f| self.events[f].offset))
    }

    /// Whether every event raised up to `lost_by` ms after the stall's end
    /// was lost and every one from `heard_from` ms heard.
    fn split(&self, (lost_by, heard_from): (Option<u64>, u64)) -> bool {
        self.events.iter().all(|e| {
            if lost_by.is_some_and(|by| e.offset <= by) {
                e.delay.is_none()
            } else if e.offset >= heard_from {
                e.delay.is_some()
            } else {
                true
            }
        })
    }
}

fn gap_offsets(step: u64, to: u64) -> String {
    (0..=to / step)
        .map(|i| (i * step).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Runs `trials` gap trials, each in a process of its own.
fn gaps(trials: usize, params: &[&str]) -> Vec<Gap> {
    (0..trials)
        .map(|_| Gap::parse(&in_own_process("gap", params)))
        .collect()
}

fn timeouts(report: &mut Report) {
    let offsets = gap_offsets(50, 2_500);
    let expected = clear_of_gap(1_500, 2_300);
    for (connection, transaction) in [(500, 5_000), (2_000, 500)] {
        let trials = gaps(
            trials(),
            &[
                &connection.to_string(),
                &transaction.to_string(),
                "stall",
                "200",
                "local",
                &offsets,
            ],
        );
        let edges: Vec<(Option<u64>, Option<u64>)> = trials.iter().map(Gap::edge).collect();
        let split: Vec<bool> = trials.iter().map(|g| g.split(expected)).collect();
        report.claim(
            &format!("at connection {connection} ms and transaction {transaction} ms, after the stall's end, {}", clear_text(expected)),
            &format!("{} (last lost, first heard: {})", runs(&split), runs(&edges)),
            "true",
            relation(&split),
        );
    }
}

fn edge(report: &mut Report) {
    let offsets: String = (170..=185)
        .map(|i| (i * 10).to_string())
        .collect::<Vec<_>>()
        .join(",");
    let trials = gaps(
        trials(),
        &["2000", "5000", "stall", "200", "local", &offsets],
    );
    let mut within = Vec::new();
    let mut rows = Vec::new();
    for gap in &trials {
        let first = gap.events.iter().position(|e| e.delay.is_some());
        let last_lost = first
            .and_then(|f| f.checked_sub(1))
            .map(|i| gap.events[i].after_return);
        let first_heard = first.map(|f| gap.events[f].after_return);
        rows.push(format!(
            "last lost {last_lost:?} ms, first heard {first_heard:?} ms after the return"
        ));
        // The edge lies between the last lost and the first heard; it
        // matches when that interval meets the recorded 1,981 to 2,001 ms.
        within.push(match (last_lost, first_heard) {
            (Some(lost), Some(heard)) => lost <= 2_001.0 && heard >= 1_981.0,
            _ => false,
        });
    }
    if recovery() {
        report.claim(
            "in 10 ms steps, the gap's end lies between the last event lost and the first heard",
            &rows.join("; "),
            "1,981 to 2,001 ms after the return",
            relation(&within),
        );
    } else {
        let all_heard: Vec<bool> = trials
            .iter()
            .map(|g| g.events.iter().all(|e| e.delay.is_some()))
            .collect();
        report.claim(
            "with connection recovery off, every event of the 10 ms steps is heard",
            &runs(&all_heard),
            "true",
            relation(&all_heard),
        );
    }
}

fn release_later(report: &mut Report) {
    let offsets = gap_offsets(50, 2_000);
    // Released 1,000 ms after the return the gap ends 950 to 1,000 ms after
    // the stall's end; released 3,000 ms after, it has ended already.
    for (connection, after, expected) in [
        (2_000, 1_000, clear_of_gap(700, 1_300)),
        (2_000, 3_000, (None, 0)),
    ] {
        let trials = gaps(
            trials(),
            &[
                &connection.to_string(),
                "5000",
                "stall",
                &after.to_string(),
                "local",
                &offsets,
            ],
        );
        let edges: Vec<(Option<u64>, Option<u64>)> = trials.iter().map(Gap::edge).collect();
        let split: Vec<bool> = trials.iter().map(|g| g.split(expected)).collect();
        report.claim(
            &format!("released {after} ms after the return at connection {connection} ms, after the stall's end, {}", clear_text(expected)),
            &format!("{} (last lost, first heard: {})", runs(&split), runs(&edges)),
            "true",
            relation(&split),
        );
    }
}

fn next_event(report: &mut Report) {
    for (offsets, recorded) in [("1500", vec![false]), ("1000,2500", vec![false, true])] {
        let recorded = if recovery() {
            recorded
        } else {
            vec![true; recorded.len()]
        };
        let trials = gaps(
            trials(),
            &["2000", "5000", "stall", "200", "local", offsets],
        );
        let heard: Vec<Vec<bool>> = trials
            .iter()
            .map(|g| g.events.iter().map(|e| e.delay.is_some()).collect())
            .collect();
        report.claim(
            &format!("events raised only at {offsets} ms after the stall's end: heard"),
            &runs(&heard),
            &format!("{recorded:?}"),
            deterministic(&heard, &recorded),
        );
    }
}

fn second_process(report: &mut Report) {
    let offsets = gap_offsets(50, 2_500);
    let trials = gaps(
        trials(),
        &["2000", "5000", "stall", "200", "remote", &offsets],
    );
    let edges: Vec<(Option<u64>, Option<u64>)> = trials.iter().map(Gap::edge).collect();
    let outcomes: Vec<String> = trials.iter().map(|g| g.outcome.clone()).collect();
    report.claim(
        "registered from a second client process, the registration reports success",
        &runs(&outcomes),
        "\"ok\"",
        deterministic(&outcomes, &"ok".to_owned()),
    );
    let expected = clear_of_gap(1_500, 2_300);
    let split: Vec<bool> = trials.iter().map(|g| g.split(expected)).collect();
    report.claim(
        &format!(
            "registered from a second client process, after the stall's end, {}",
            clear_text(expected)
        ),
        &format!(
            "{} (last lost, first heard: {})",
            runs(&split),
            runs(&edges)
        ),
        "true",
        relation(&split),
    );
}

fn control(report: &mut Report) {
    let offsets = gap_offsets(50, 2_500);
    let trials = gaps(
        trials(),
        &["2000", "5000", "nostall", "0", "local", &offsets],
    );
    let all_heard: Vec<bool> = trials
        .iter()
        .map(|g| g.events.iter().all(|e| e.delay.is_some_and(|d| d < 100.0)))
        .collect();
    let quick: Vec<bool> = trials
        .iter()
        .map(|g| g.outcome == "ok" && g.took < 100.0)
        .collect();
    let took: Vec<f64> = trials.iter().map(|g| g.took).collect();
    report.claim(
        "with no stall, the registration returns at once",
        &format!("{} ({})", runs(&quick), summary_ms(&took)),
        "3 ms",
        relation(&quick),
    );
    report.claim(
        "with no stall, every event is heard within 100 ms of its raise",
        &runs(&all_heard),
        "true",
        relation(&all_heard),
    );
}

/// One e58 onset trial: as e57, a focus event every 100 ms from the
/// stall's end to 3,000 ms after it, the provider's log read after.
/// Answers the last lost and first heard offsets, whether every event
/// raised in the gap was followed by provider work before the next raise,
/// and the gap's log entries and `WM_GETOBJECT`s.
fn trial_onset() -> Vec<String> {
    let offsets: Vec<u64> = (0..=30).map(|i| i * 100).collect();
    let (mut provider, client, element) = setup(2_000, 5_000);
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    provider.log();
    let stalled = register_stalled(&mut provider, &client, &element, &group);
    let raised = raise_at(&mut provider, 0, "ev", stalled.released, &offsets, 0);
    uia::pause(LISTEN, "listening after the last event");
    let log = provider.log();
    let (heard, _) = events::drain(&reports);
    let delays = delays(&raised, &heard);
    let first = delays
        .iter()
        .position(Option::is_some)
        .unwrap_or(raised.len());
    let backlog = backlog_end(&log, stalled.released_us);
    let backlog_end = backlog.unwrap_or(stalled.released_us);
    let gap_end = raised.get(first).map_or(u64::MAX, |r| r.1);
    // Provider work answering each event raised in the gap: entries after
    // its raise and before the next raise's requests began.
    let followed = (0..first).all(|i| {
        let from = raised[i].1;
        let to = raised.get(i + 1).map_or(u64::MAX, |r| r.1);
        log.iter().any(|e| e.start > from && e.start < to)
    });
    let in_gap: Vec<_> = log
        .iter()
        .filter(|e| e.start > backlog_end && e.start < gap_end)
        .collect();
    let getobject = in_gap
        .iter()
        .filter(|e| e.kind == "msg" && e.name == "WM_GETOBJECT")
        .count();
    vec![
        first
            .checked_sub(1)
            .map_or("none".to_owned(), |i| offsets[i].to_string()),
        offsets
            .get(first)
            .map_or("none".to_owned(), ToString::to_string),
        (first > 0 && followed).to_string(),
        in_gap.len().to_string(),
        getobject.to_string(),
        backlog.is_some().to_string(),
    ]
}

fn onset(report: &mut Report) {
    let mut backlogs = Vec::new();
    let mut edges = Vec::new();
    let mut followed = Vec::new();
    let mut counts = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("onset", &[]);
        edges.push((words[0].clone(), words[1].clone()));
        followed.push(flag(&words[2]));
        counts.push(format!("{} entries, {} WM_GETOBJECT", words[3], words[4]));
        backlogs.push(flag(&words[5]));
    }
    // The events come every 100 ms; the first heard lies clear of the
    // recorded 1,800 ms by 300 ms either way, or is the first event when
    // there is no gap.
    let expected = clear_of_gap(1_500, 2_100);
    let split: Vec<bool> = edges
        .iter()
        .map(|(_, first)| {
            let first: Option<u64> = first.parse().ok();
            match expected {
                (Some(lost_by), heard_from) => {
                    first.is_some_and(|f| f > lost_by && f <= heard_from)
                }
                (None, _) => first == Some(0),
            }
        })
        .collect();
    report.claim(
        &format!("onset: after the stall's end, {}", clear_text(expected)),
        &format!(
            "{} (last lost, first heard: {})",
            runs(&split),
            runs(&edges)
        ),
        "true",
        relation(&split),
    );
    // With connection recovery off no event is lost, so there is no gap
    // whose events could be followed.
    if recovery() {
        report.claim(
            "onset: every event raised in the gap is followed by provider work before the next",
            &runs(&followed),
            "true",
            relation(&followed),
        );
    }
    report.claim(
        "onset: the stall's backlog is the registration's request alone, ended by its advise",
        &runs(&backlogs),
        "true",
        relation(&backlogs),
    );
    report.claim(
        "onset: what reached the provider between the backlog and the first delivery",
        &counts.join("; "),
        "1,086 to 1,088 items",
        Verdict::Trend,
    );
}

fn wait_event(reports: &Receiver<Event>) -> Option<Heard> {
    match reports.recv_timeout(Duration::from_secs(10)) {
        Ok(Event::Heard(h)) => Some(h),
        _ => None,
    }
}
