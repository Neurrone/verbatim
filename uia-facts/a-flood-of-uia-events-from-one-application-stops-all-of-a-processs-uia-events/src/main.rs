//! A flood of UIA events from one application stops all of a process's UIA
//! events
//! (`uia-facts/a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events/readme.md`).
//!
//! Flooding windows are UIA windows of provider programs that raise events
//! on their UI thread at a set pace (`raise-rate`). B is a UIA window whose
//! node 1 is renamed `P<k>` as a probe. Each client is a listening process
//! of its own (`uia_facts_support::listen`), every registration of a
//! client on one client object, so each trial's clients start with no
//! backlog and a stopped client is ended with the trial.

use std::time::Duration;

use uia_facts_support::args::{Args, trials};
use uia_facts_support::listen::{self, Line, Listener};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::stats::{as_f64, median, us_to_ms};
use uia_facts_support::{note, now_us, uia};

/// The desktop of its own the `stops` case runs on, where the flooding
/// windows can be shown, as a desktop-wide registration hears only shown
/// windows' events.
const DESKTOP: &str = "uia-facts";
/// How long `stops` watches after the flood: `q2long` watched 90 s, and
/// this case narrows the claim to what fits the program's run time.
const WATCH_S: u64 = 10;
/// The events each of `stops`' windows floods, 3 s at 10,000 a second,
/// as `q2` flooded: with 10,000 the text-flooded client kept hearing in 3
/// of 13 runs.
const FLOOD: usize = 30_000;

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, rest)) = &args.role {
        assert_eq!(name, listen::ROLE, "unknown role {name}");
        listen::run(rest);
    }
    let mut report = Report::start(
        "a-flood-of-uia-events-from-one-application-stops-all-of-a-processs-uia-events",
    );
    if args.wants("stops") {
        report.case("stops", "three windows each flooding 30,000 events at 10,000 a second for 3 s, name changes, text changes and notifications; a client registered for each flood and for B's name changes, and one for B's alone; B renamed every 250 ms until 10 s after the flood");
        stops(&args, &mut report);
    }
    if args.wants("rates") {
        report.case("rates", "a window flooding name changes at 2,000 a second for 3 s, as q2_thr did; one client registered for them and for B's name changes; B renamed every 100 ms until the flood's events are all heard");
        rates(&args, &mut report, 2_000);
    }
    // Printed for trends only, so run only when named.
    if args.names("rate-5000") {
        report.case("rate-5000", "the same at 5,000 a second for 3 s");
        rates(&args, &mut report, 5_000);
    }
    if args.wants("backlog") {
        report.case("backlog", "bursts of 250, 500, 1,000 and 2,000 name changes raised back to back, each at a client of its own; its processor time and the events it hears until it has heard them all, up to 1.5 s");
        backlog(&args, &mut report);
    }
    report.finish();
}

/// Renames B's node 1 `P<k>` and raises its name change; answers when.
fn probe(b: &mut Provider, k: usize) -> u64 {
    b.request(&format!("rename 0 1 P{k}"));
    b.request("raise name 0 1").parse().expect("a time")
}

/// The probes `lines` heard. A probe is told by its sender's name, which
/// the worker reads when it takes the event from its queue, so probes
/// queued together all carry the latest probe's name: each is counted,
/// not named.
fn probes_heard(lines: &[Line]) -> usize {
    lines
        .iter()
        .filter(|l| matches!(l, Line::Heard { kind, name, .. } if kind == "name" && name.starts_with('P')))
        .count()
}

/// Asks `client` its processor time.
fn cpu(client: &mut Listener) -> Option<u64> {
    client.send("cpu");
    match client.next(Duration::from_secs(5), |l| matches!(l, Line::Cpu(_))) {
        Some(Line::Cpu(us)) => Some(us),
        _ => None,
    }
}

/// Every event `lines` heard of `kind` from a sender named `name`.
fn count(lines: &[Line], kind: &str, name: &str) -> usize {
    lines
        .iter()
        .filter(|l| matches!(l, Line::Heard { kind: k, name: n, .. } if k == kind && n == name))
        .count()
}

/// Every event `lines` heard after `from`.
fn heard_after(lines: &[Line], from: u64) -> usize {
    lines
        .iter()
        .filter(|l| matches!(l, Line::Heard { at, .. } if *at > from))
        .count()
}

#[expect(clippy::too_many_lines, reason = "one long trial, its claims in turn")]
fn stops(args: &Args, report: &mut Report) {
    let desk = Some(DESKTOP);
    let mut name_stopped = Vec::new();
    let mut text_outcome = Vec::new();
    let mut notification_stopped = Vec::new();
    let mut other_whole = Vec::new();
    let mut busy = Vec::new();
    let mut counts = Vec::new();
    for _ in 0..trials() {
        let mut floods: Vec<Provider> = (0..3)
            .map(|_| Provider::launch_on(desk, &["--windows", "uia:0"]))
            .collect();
        floods[2].request("show 0");
        let mut b = Provider::launch_on(desk, &["--windows", "uia:0"]);
        let b_name = format!("name={}", b.hwnd(0));
        let mut clients = [
            Listener::start_on(
                desk,
                "name changes",
                &[format!("name={}", floods[0].hwnd(0)), b_name.clone()],
            ),
            Listener::start_on(
                desk,
                "text changes",
                &[format!("text={}", floods[1].hwnd(0)), b_name.clone()],
            ),
            Listener::start_on(
                desk,
                "desktop-wide notifications",
                &["desknotification".to_owned(), b_name.clone()],
            ),
            Listener::start_on(desk, "B's alone", std::slice::from_ref(&b_name)),
        ];
        let mut pids = vec![("B", b.pid)];
        pids.extend(clients.iter().map(|c| (c.label.as_str(), c.pid())));
        args.pause_for_debugger(&pids);
        // As q2 did, each handler is warmed with one event first.
        for (i, (flood, what)) in floods
            .iter_mut()
            .zip(["name 0 1", "text 0 3", "notification 0 1"])
            .enumerate()
        {
            flood.request(&format!("raise {what}"));
            let kind = ["name", "text", "notification"][i];
            let warm = clients[i].next(
                Duration::from_secs(5),
                |l| matches!(l, Line::Heard { kind: k, .. } if k == kind),
            );
            note::evidence(&format!("{} warmed: {}", clients[i].label, warm.is_some()));
        }
        let start = now_us();
        let mut sent = Vec::new();
        let flood_end = std::thread::scope(|scope| {
            let raising: Vec<_> = floods
                .iter_mut()
                .zip(["name 0 1", "text 0 3", "notification 0 1"])
                .map(|(provider, what)| {
                    scope.spawn(move || {
                        provider.request(&format!("raise-rate {what} 10000 {FLOOD}"));
                        now_us()
                    })
                })
                .collect();
            let mut k = 0;
            while raising.iter().any(|r| !r.is_finished()) {
                sent.push((k, probe(&mut b, k)));
                k += 1;
                std::thread::sleep(Duration::from_millis(250));
            }
            raising
                .into_iter()
                .map(|r| r.join().expect("a flood ends"))
                .max()
                .expect("three floods")
        });
        note::evidence(&format!(
            "the floods took {:.0} ms",
            us_to_ms(flood_end - start)
        ));
        let before: Vec<Option<u64>> = clients.iter_mut().map(cpu).collect();
        note::waiting(&format!("{WATCH_S} s, B renamed every 250 ms"));
        let watch_end = flood_end + WATCH_S * 1_000_000;
        let mut k = sent.len();
        while now_us() < watch_end {
            sent.push((k, probe(&mut b, k)));
            k += 1;
            std::thread::sleep(Duration::from_millis(250));
        }
        let after: Vec<Option<u64>> = clients.iter_mut().map(cpu).collect();
        let lines: Vec<Vec<Line>> = clients
            .iter()
            .map(|c| c.lines_until(now_us() + 500_000))
            .collect();
        for (i, client) in clients.iter().enumerate() {
            let probes = probes_heard(&lines[i]);
            note::evidence(&format!(
                "{}: {} events heard after the flood's end; {} of {} probes heard; processor time {:?} s at the flood's end, {:?} s {WATCH_S} s later",
                client.label,
                heard_after(&lines[i], flood_end),
                probes,
                sent.len(),
                before[i].map(|us| us_to_ms(us) / 1_000.0),
                after[i].map(|us| us_to_ms(us) / 1_000.0)
            ));
        }
        name_stopped.push(heard_after(&lines[0], flood_end) == 0);
        counts.push(count(&lines[0], "name", "First"));
        let (Some(b), Some(a)) = (before[0], after[0]) else {
            panic!("the name-flooded client reports its processor time");
        };
        busy.push(us_to_ms(a.saturating_sub(b)) / 1_000.0);
        text_outcome.push(heard_after(&lines[1], flood_end) == 0);
        notification_stopped.push(heard_after(&lines[2], flood_end) == 0);
        let whole = probes_heard(&lines[3]) == sent.len();
        other_whole.push(whole);
    }
    report.claim(
        &format!("name changes: the flooded client hears no event, the flood's or B's, from the flood's end to {WATCH_S} s after"),
        &runs(&name_stopped),
        "none of 349 probes in the 90 s after (q2long_one_c1_10k)",
        relation(&name_stopped),
    );
    report.claim(
        "name changes: the flood's events the flooded client heard",
        &runs(&counts),
        "83 to 110 of 30,000 (q2, q2long), 95 of 30,000 (the first run)",
        Verdict::Trend,
    );
    let core: Vec<bool> = busy
        .iter()
        .map(|s| *s >= 0.8 * as_f64(usize::try_from(WATCH_S).expect("a small number")))
        .collect();
    report.claim(
        &format!("name changes: the flooded client's processor time over the {WATCH_S} s, at least 0.8 of one core"),
        &format!("{} ({:.1} s)", runs(&core), median(&busy)),
        "about 2 s every 2 s throughout (q2long)",
        relation(&core),
    );
    if uia::settings().coalescing {
        let going: Vec<bool> = text_outcome.iter().map(|stopped| !stopped).collect();
        report.claim(
            "text changes, event coalescing on: the flooded client keeps hearing events after the flood's end",
            &runs(&going),
            "2,529 and 2,641 of 30,000 delivered, B's probes heard (q2 m_one_c1_text)",
            relation(&going),
        );
    } else {
        report.claim(
            &format!("text changes, event coalescing off: the flooded client hears no event from the flood's end to {WATCH_S} s after"),
            &runs(&text_outcome),
            "83 to 110 of 30,000 delivered and the probes not (q2 m_one_c0_text)",
            relation(&text_outcome),
        );
    }
    report.claim(
        &format!("desktop-wide notifications: the flooded client hears no event from the flood's end to {WATCH_S} s after"),
        &runs(&notification_stopped),
        "",
        Verdict::New,
    );
    report.claim(
        "a client process registered for B's name changes alone hears every probe, during the floods and after",
        &runs(&other_whole),
        "1.4 to 1.8 ms at the median at every rate (q2, layout two)",
        relation(&other_whole),
    );
}

#[expect(
    clippy::too_many_lines,
    reason = "one trial a rate, its claims in turn"
)]
fn rates(args: &Args, report: &mut Report, rate: usize) {
    {
        let mut lost_any = Vec::new();
        let mut lost_counts = Vec::new();
        let mut drained = Vec::new();
        let mut flood_heard = Vec::new();
        for _ in 0..trials() {
            let mut flood = Provider::launch(&["--windows", "uia:0"]);
            let mut b = Provider::launch(&["--windows", "uia:0"]);
            let client = Listener::start(
                "flooded",
                &[
                    format!("name={}", flood.hwnd(0)),
                    format!("name={}", b.hwnd(0)),
                ],
            );
            args.pause_for_debugger(&[
                ("flooding", flood.pid),
                ("B", b.pid),
                ("client", client.pid()),
            ]);
            flood.request("raise name 0 1");
            let warm = client.next(Duration::from_secs(5), |l| matches!(l, Line::Heard { .. }));
            note::evidence(&format!("warmed: {}", warm.is_some()));
            let total = rate * 3;
            let mut sent = Vec::new();
            let mut lines = Vec::new();
            let flood_end = std::thread::scope(|scope| {
                let raising = scope.spawn(|| {
                    flood.request(&format!("raise-rate name 0 1 {rate} {total}"));
                    now_us()
                });
                let mut k = 0;
                while !raising.is_finished() {
                    sent.push((k, probe(&mut b, k)));
                    k += 1;
                    std::thread::sleep(Duration::from_millis(100));
                }
                raising.join().expect("the flood ends")
            });
            note::waiting("up to 15 s, the flood's events heard");
            let limit = flood_end + 15_000_000;
            let mut k = sent.len();
            // Drained: every flood event heard, or none heard for 2 s, so
            // the client's queue is empty.
            let mut seen = 0;
            let mut still_since = now_us();
            let drained_now = loop {
                lines.extend(client.lines_until(now_us()));
                let heard = count(&lines, "name", "First");
                if heard != seen {
                    seen = heard;
                    still_since = now_us();
                }
                if heard >= total || now_us() >= still_since + 2_000_000 {
                    break true;
                }
                if now_us() >= limit {
                    break false;
                }
                sent.push((k, probe(&mut b, k)));
                k += 1;
                std::thread::sleep(Duration::from_millis(100));
            };
            lines.extend(client.lines_until(now_us() + 500_000));
            let heard = count(&lines, "name", "First");
            let last = lines
                .iter()
                .filter_map(|l| match l {
                    Line::Heard { name, at, .. } if name == "First" => Some(*at),
                    _ => None,
                })
                .max();
            let probes = probes_heard(&lines);
            let never = sent.len().saturating_sub(probes);
            note::evidence(&format!(
                "{rate} a second: {heard} of {total} flood events heard, the last {:?} s after the flood's end; {never} of {} probes never heard",
                last.map(|l| us_to_ms(l.saturating_sub(flood_end)) / 1_000.0),
                sent.len()
            ));
            lost_any.push(never > 0);
            lost_counts.push(never);
            flood_heard.push(heard);
            drained.push(drained_now);
        }
        // A probe is lost only once the flood has drained: before then it
        // may still be queued.
        // Every probe is heard beside a client that heard every flood
        // event: q2_thr's log holds a delivery for each of its 304 probes,
        // its "never delivered" counted by name, which the worker reads
        // when it takes each event, so queued probes carried a later
        // probe's name. A client that stopped leaves probes unheard for
        // that reason, so judges nothing.
        let all_heard = flood_heard.iter().all(|h| *h == rate * 3);
        let verdict = if rate == 5_000 {
            // The provider program's flood stops the client at this rate,
            // where mockapp's was delivered in full: printed for trends.
            Verdict::Trend
        } else if all_heard && drained.iter().all(|d| *d) {
            relation(&lost_any.iter().map(|lost| !lost).collect::<Vec<_>>())
        } else {
            Verdict::Inconclusive("the client did not hear every flood event".to_owned())
        };
        report.claim(
            &format!("{rate} a second for 3 s: every one of B's probes is heard"),
            &format!(
                "{} ({lost_counts:?} never heard)",
                runs(&lost_any.iter().map(|lost| !lost).collect::<Vec<_>>())
            ),
            "a delivery for each of 304 probes, counted (q2_thr_one_c1.log)",
            verdict,
        );
        report.claim(
            &format!("{rate} a second for 3 s: the flood's events heard"),
            &format!("{flood_heard:?} of {}", rate * 3),
            "every one, the last 2.9 and 10.4 s after the flood (q2_thr_one_c1)",
            Verdict::Trend,
        );
    }
}

fn backlog(args: &Args, report: &mut Report) {
    let mut results = Vec::new();
    for size in [250usize, 500, 1_000, 2_000] {
        let mut flood = Provider::launch(&["--windows", "uia:0"]);
        let mut client = Listener::start("flooded", &[format!("name={}", flood.hwnd(0))]);
        args.pause_for_debugger(&[("flooding", flood.pid), ("client", client.pid())]);
        flood.request("raise name 0 1");
        let warm = client.next(Duration::from_secs(5), |l| matches!(l, Line::Heard { .. }));
        note::evidence(&format!("warmed: {}", warm.is_some()));
        let before = cpu(&mut client).expect("the client reports its processor time");
        let raised: u64 = flood
            .request(&format!("raise-rate name 0 1 0 {size}"))
            .parse()
            .expect("a time");
        // Until the client has heard the whole burst, or up to 1.5 s.
        let limit = now_us() + 1_500_000;
        let mut lines = Vec::new();
        while count(&lines, "name", "First") < size && now_us() < limit {
            lines.extend(client.lines_until((now_us() + 20_000).min(limit)));
        }
        let after = cpu(&mut client).expect("the client reports its processor time");
        lines.extend(client.lines_until(now_us()));
        let heard = count(&lines, "name", "First");
        note::evidence(&format!("{size} raised from {raised}: {heard} heard"));
        results.push(format!(
            "{size}: {heard} heard, {:.0} ms of processor time",
            us_to_ms(after.saturating_sub(before))
        ));
    }
    report.claim(
        "bursts of name changes raised back to back: the events heard, up to 1.5 s, and the client's processor time until then",
        &results.join("; "),
        "",
        Verdict::New,
    );
}
