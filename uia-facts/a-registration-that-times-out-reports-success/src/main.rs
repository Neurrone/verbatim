//! A registration that times out reports success
//! (`uia-facts/a-registration-that-times-out-reports-success/readme.md`):
//! the registration's return and the gap after it. The gap's reach, a
//! removal during it and the silence after the stall are
//! `uia-facts/the-gap-after-a-timed-out-registration-silences-the-processs-registrations-on-that-window/`'s.
//!
//! Every registration is an event handler group holding UIA's focus event,
//! subtree scope, the sender's name cached, on the provider program's
//! window, as e57 made it. Each focus event is raised as e57 raised it
//! through mockapp's `set-name` and `focus`: the next of nodes 5 to 8 in
//! turn is renamed `ev<i>` with a name change raised, then the tree's focus
//! moves to it and its focus event is raised, so the handler names the
//! event it heard. A stall is the window's UI thread busy until released.
//! Every trial runs in a client process of its own (the `trial` role), as
//! e57 ran each trial in a test process of its own, so no other trial's
//! registration is in the process.
//!
//! - `ett`: E-TT's registration: both timeouts at 1,000 ms, the provider
//!   holding `FragmentRoot`.
//! - `connection`: a registration during a stall returns after the
//!   connection timeout, whatever the transaction timeout: 1,000 and
//!   500 ms, and the reverse.
//! - `e57`: connection timeout 2,000 ms, transaction timeout 5,000 ms; the
//!   group registered during a stall, the stall released 200 ms after the
//!   registration returns, and a focus event raised every 50 ms from the
//!   stall's end to 2,500 ms after it, every 10 ms from 1,700 to 1,850 ms;
//!   heard events listened for 0.5 s after the last; the provider's log
//!   read after. Then, with no stall, ten events from the return.
//! - `release-later`, `next-event` and `second-process`, each at both
//!   timeouts 500 ms, so that the gap's end is shown at other timeouts
//!   than `e57`'s: the stall released 1,000 and 3,000 ms after the return;
//!   events at 1,500 and 2,500 ms after the stall's end alone; `e57`
//!   registered by a second client process.
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
/// 1 to 11 ms. In `e57` it is the watch of its "lost, not late" claim: no
/// event lost arrives within it, the last raise 2,500 ms after the stall's
/// end, about 700 ms after the gap.
const LISTEN: Duration = Duration::from_millis(500);
/// Both timeouts of the gap trials other than `e57`'s: short, since the
/// gap's end follows neither timeout.
const SHORT: &str = "500";

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        match name.as_str() {
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
        report.case("connection", "stalling the provider's UI thread; registering at a 1 s connection and 500 ms transaction timeout, then the reverse; timing the call");
        connection(
            &mut report,
            1_000,
            500,
            "1,089 ms (E-TT, both timeouts 1,000 ms)",
        );
        connection(&mut report, 500, 1_000, "532 to 551 ms");
    }
    if args.wants("e57") {
        report.case("e57", "registering a focus handler while the provider's UI thread is stalled; releasing 200 ms after the call returns; raising a focus event every 50 ms, and every 10 ms near the gap's end; reading the provider's log; then the same with no stall");
        e57(&mut report);
        control(&mut report);
    }
    if args.wants("release-later") {
        report.case("release-later", "registering during a stall at both timeouts 500 ms; releasing it 1 or 3 s after the call returns; raising focus events after it");
        release_later(&mut report);
    }
    if args.wants("next-event") {
        report.case("next-event", "registering during a stall at both timeouts 500 ms; raising a focus event 1.5 s after the stall's end and another at 2.5 s");
        next_event(&mut report);
    }
    if args.wants("second-process") {
        report.case("second-process", "registering from a second client process during a stall at both timeouts 500 ms; raising a focus event every 50 ms after the stall's end");
        second_process(&mut report);
    }
    if args.wants("between") {
        report.case("between", "raising a focus event and a name change at once; registering for name changes on hearing the focus; raising a second name change");
        between(&mut report);
    }
    report.finish();
}

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

/// Runs one trial in a client process of its own, as e57 ran each trial in
/// a test process of its own, so no registration of another trial or case
/// is in the process; answers the trial's result words.
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
        other => panic!("unknown trial {other}"),
    };
    role::report(&format!("result {}", words.join(" ")));
    exit_now(0)
}

fn flag(word: &str) -> bool {
    word == "true"
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
/// mockapp's `set-name` and `focus`: the next of [`NODES`] in turn is
/// renamed `<prefix><i>` with a name change raised, then the tree's focus
/// moves to it and its focus event is raised. Answers each event's name
/// and when its focus event was raised.
fn raise_at(
    provider: &mut Provider,
    prefix: &str,
    from: Instant,
    offsets_ms: &[u64],
) -> Vec<(String, u64)> {
    let mut raised = Vec::new();
    for (i, offset) in offsets_ms.iter().enumerate() {
        let due = from + Duration::from_millis(*offset);
        uia::pause(
            due.saturating_duration_since(Instant::now()),
            "until the next event is due",
        );
        let node = NODES[i % NODES.len()];
        let name = format!("{prefix}{i}");
        provider.request(&format!("rename 0 {node} {name}"));
        provider.request(&format!("raise name 0 {node}"));
        provider.request(&format!("focus 0 {node}"));
        let at: u64 = provider
            .request(&format!("raise focus 0 {node}"))
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

/// `e57`'s offsets after the stall's end: every 50 ms to 2,500 ms, and
/// every 10 ms from 1,700 to 1,850 ms, about the gap's end.
fn e57_offsets() -> Vec<u64> {
    let mut offsets: Vec<u64> = (0..=50).map(|i| i * 50).collect();
    offsets.extend((170..=185).map(|i| i * 10));
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// The events raised up to this offset are 50 ms apart and in the gap, so
/// provider work after each one comes before the next is raised.
const SPACED_BY: u64 = 1_500;

/// One e57 trial. Answers, in order: the registration's outcome and time;
/// whether every event up to 1,500 ms was lost, every one from 2,300 ms
/// heard, every heard one prompt, and none lost after the first heard;
/// the last lost and first heard event's raise after the return, in ms,
/// or `none`; whether every event was heard; whether every event raised
/// in the gap up to [`SPACED_BY`] was followed by provider work before the
/// next raise; the provider log's entries and `WM_GETOBJECT`s between the
/// stall's backlog and the first delivery; whether that backlog was the
/// registration's request alone; and each event's offset and delay.
fn trial_e57() -> Vec<String> {
    let offsets = e57_offsets();
    let (mut provider, client, element) = setup(2_000, 5_000);
    let (handler, reports) = Handler::make();
    let group = events::focus_group(&client, &handler).expect("a group");
    provider.log();
    let stalled = register_stalled(&mut provider, &client, &element, &group);
    let raised = raise_at(&mut provider, "ev", stalled.released, &offsets);
    uia::pause(
        LISTEN,
        "listening after the last event, so an event lost is not one heard late",
    );
    let log = provider.log();
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
            format!("{:.1}", us_to_ms(t.saturating_sub(returned)))
        })
    };
    // The onset: provider work after each event raised in the gap, and
    // what reached the provider between the backlog and the first
    // delivery.
    let first_heard = first.unwrap_or(raised.len());
    let backlog = backlog_end(&log, stalled.released_us);
    let backlog_until = backlog.unwrap_or(stalled.released_us);
    let gap_end = raised.get(first_heard).map_or(u64::MAX, |r| r.1);
    let followed = (0..first_heard).filter(|i| at(*i) <= SPACED_BY).all(|i| {
        let from = raised[i].1;
        let to = raised.get(i + 1).map_or(u64::MAX, |r| r.1);
        log.iter().any(|e| e.start > from && e.start < to)
    });
    let in_gap: Vec<&Entry> = log
        .iter()
        .filter(|e| e.start > backlog_until && e.start < gap_end)
        .collect();
    let getobject = in_gap
        .iter()
        .filter(|e| e.kind == "msg" && e.name == "WM_GETOBJECT")
        .count();
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
        (first_heard > 0 && followed).to_string(),
        in_gap.len().to_string(),
        getobject.to_string(),
        backlog.is_some().to_string(),
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

#[expect(clippy::too_many_lines, reason = "e57's claims, read in turn")]
fn e57(report: &mut Report) {
    let mut outcomes = Vec::new();
    let mut times = Vec::new();
    let mut early_lost = Vec::new();
    let mut late_heard = Vec::new();
    let mut prompt = Vec::new();
    let mut no_loss_after = Vec::new();
    let mut all_heard = Vec::new();
    let mut edges = Vec::new();
    let mut within = Vec::new();
    let mut followed = Vec::new();
    let mut counts = Vec::new();
    let mut backlogs = Vec::new();
    for _ in 0..trials() {
        let words = in_own_process("e57", &[]);
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
        // The edge lies between the last lost and the first heard; it
        // matches when that interval meets the recorded 1,981 to 2,001 ms.
        within.push(
            match (words[6].parse::<f64>().ok(), words[7].parse::<f64>().ok()) {
                (Some(lost), Some(heard)) => lost <= 2_001.0 && heard >= 1_981.0,
                _ => false,
            },
        );
        all_heard.push(flag(&words[8]));
        followed.push(flag(&words[9]));
        counts.push(format!("{} entries, {} WM_GETOBJECT", words[10], words[11]));
        backlogs.push(flag(&words[12]));
        note::evidence(&format!("events, offset:delay ms: {}", words[13]));
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
        "every event heard is heard within 100 ms of its raise, and none lost is heard in the 0.5 s after the last: lost, not late",
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
    if recovery() {
        report.claim(
            "in 10 ms steps near it, the gap's end lies between the last event lost and the first heard",
            &format!("{} ({})", runs(&within), edges.join("; ")),
            "1,981 to 2,001 ms after the return",
            relation(&within),
        );
        report.claim(
            "every event raised in the gap 50 ms apart is followed by provider work before the next",
            &runs(&followed),
            "true",
            relation(&followed),
        );
    }
    report.claim(
        "the stall's backlog is the registration's request alone, ended by its advise",
        &runs(&backlogs),
        "true",
        relation(&backlogs),
    );
    report.claim(
        "what reached the provider between the backlog and the first delivery",
        &counts.join("; "),
        "997 to 1,088 entries, no WM_GETOBJECT",
        Verdict::Trend,
    );
}

/// e57's registering client in a second process: makes a client with
/// the settings it was started with and the timeouts given, fetches the
/// window's element, says `ready`; on `go` registers the focus group and
/// says `registered <outcome> <start> <end>`, then `heard <name> <time>`
/// for each event, until its standard input closes.
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
    let raised = raise_at(&mut provider, "ev", from, offsets);
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

/// One event of a [`trial_gap`]: its offset after the stall's end, and its
/// delay when heard.
struct GapEvent {
    offset: u64,
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
                    delay: parts[2].parse().ok(),
                }
            })
            .collect();
        note::evidence(&format!(
            "events, offset:after the return:delay ms: {}",
            words[2]
        ));
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

fn release_later(report: &mut Report) {
    // Released 1,000 ms after the return the gap ends 950 to 1,000 ms after
    // the stall's end; released 3,000 ms after, it has ended already, so a
    // few events show that every one is heard.
    for (after, expected, offsets) in [
        (1_000, clear_of_gap(700, 1_300), gap_offsets(50, 1_600)),
        (3_000, (None, 0), gap_offsets(50, 300)),
    ] {
        let trials = gaps(
            trials(),
            &[SHORT, SHORT, "stall", &after.to_string(), "local", &offsets],
        );
        let edges: Vec<(Option<u64>, Option<u64>)> = trials.iter().map(Gap::edge).collect();
        let split: Vec<bool> = trials.iter().map(|g| g.split(expected)).collect();
        report.claim(
            &format!("released {after} ms after the return, both timeouts {SHORT} ms, after the stall's end, {}", clear_text(expected)),
            &format!("{} (last lost, first heard: {})", runs(&split), runs(&edges)),
            "true",
            relation(&split),
        );
    }
}

fn next_event(report: &mut Report) {
    let offsets = "1500,2500";
    let recorded = if recovery() {
        vec![false, true]
    } else {
        vec![true, true]
    };
    let trials = gaps(trials(), &[SHORT, SHORT, "stall", "200", "local", offsets]);
    let heard: Vec<Vec<bool>> = trials
        .iter()
        .map(|g| g.events.iter().map(|e| e.delay.is_some()).collect())
        .collect();
    report.claim(
        &format!("events raised only at {offsets} ms after the stall's end, both timeouts {SHORT} ms: heard"),
        &runs(&heard),
        &format!("{recorded:?} (1,500 ms alone never heard; of 1,000 and 2,500 ms the first lost)"),
        deterministic(&heard, &recorded),
    );
}

fn second_process(report: &mut Report) {
    let offsets = gap_offsets(50, 2_400);
    let trials = gaps(
        trials(),
        &[SHORT, SHORT, "stall", "200", "remote", &offsets],
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
            "registered from a second client process, both timeouts {SHORT} ms, after the stall's end, {}",
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
    let offsets = gap_offsets(50, 450);
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

fn wait_event(reports: &Receiver<Event>) -> Option<Heard> {
    match reports.recv_timeout(Duration::from_secs(10)) {
        Ok(Event::Heard(h)) => Some(h),
        _ => None,
    }
}
