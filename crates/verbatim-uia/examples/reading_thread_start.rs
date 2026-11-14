//! What the listener's reading thread costs to start, per read, once its
//! application has gone idle: whether each start pays COM's and UI
//! Automation's setup, as a number.
//!
//! The listener starts a reading thread for an application when an event
//! of it arrives and none runs, and the thread ends when the application
//! has no more events (`verbatim-listener`'s `focus_read.rs`, `spawn` and
//! `run`). The thread makes its focus reader, whose only call is
//! `Uia::new`: joining COM's multithreaded apartment and creating a UI
//! Automation client (`CUIAutomation8`, connection recovery and event
//! coalescing on, its proxies told to ignore window events). It reads, and on
//! its end drops the reader, releases this crate's thread state and leaves
//! the apartment. UI Automation's once-per-process setup is made before
//! the first round, as the listener's registrations made it long before.
//!
//! Each round, a fresh thread named `verbatim-listener-read`, as the
//! listener's are, makes that setup and one read, the read a UI
//! Automation focus read starts with: the event's element resolved from
//! its agile reference, the walk's two cache requests built (the
//! ancestors' and the focus's, every detail on, as the listener's
//! `FocusReader::walk` builds them) and the element refreshed with the
//! focus's request, one cross-process call (`focus_ancestry_classic`'s
//! fresh cache). The element here is the target window's, looked up once
//! by its handle. Then the same read is made on one long-lived thread,
//! set up once at the start, woken by a channel. Rounds are spaced by the
//! interval, so each starts from idle threads.
//!
//! Phases of a fresh thread, in microseconds: the thread's start (the
//! spawn call to the thread's first instruction), joining the apartment
//! (`init_mta`, timed apart from the client by joining first, so the
//! client's own join is a nested one and the thread leaves twice), the UI
//! Automation client (`Uia::new`), the read, the teardown, and the whole
//! from the spawn to the read's end. Of the long-lived thread: the
//! hand-off (the send to the thread's wake), the read, and the whole from
//! the send to the read's end. A round whose read fails on either thread
//! is counted and left out.
//!
//! Run: `cargo run -p verbatim-uia --example reading_thread_start
//! [--release] -- --process <name>` or `-- --hwnd <handle>`. Options:
//! `--process <name>` (the one process of that name, with or without
//! `.exe`; its first visible top-level window), `--hwnd N` (a window
//! handle, decimal or `0x` hexadecimal), `--rounds N` (200), `--warmup N`
//! (5, rounds run first and left out), `--interval-ms N` (100, the pause
//! after each thread's read).

mod common;

use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use common::{fail, first_visible_window, micros, print_header, print_row, process_named};
use verbatim_uia::{Ancestor, AncestorGates, Live, Node, NodeGates, Shape as _, Uia};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::AgileReference;

const EXAMPLE: &str = "reading_thread_start";

/// The listener's reading threads' name.
const READING_THREAD: &str = "verbatim-listener-read";

struct Settings {
    hwnd: Option<isize>,
    process: Option<String>,
    rounds: usize,
    warmup: usize,
    interval: Duration,
}

fn parse_settings(arguments: &[String]) -> Settings {
    let mut settings = Settings {
        hwnd: None,
        process: None,
        rounds: 200,
        warmup: 5,
        interval: Duration::from_millis(100),
    };
    let mut at = 0;
    while at < arguments.len() {
        match arguments[at].as_str() {
            "--hwnd" => settings.hwnd = Some(parse_hwnd(common::value(EXAMPLE, arguments, at))),
            "--process" => {
                settings.process = Some(common::value(EXAMPLE, arguments, at).to_owned());
            }
            "--rounds" => settings.rounds = to_usize(common::number(EXAMPLE, arguments, at)),
            "--warmup" => settings.warmup = to_usize(common::number(EXAMPLE, arguments, at)),
            "--interval-ms" => {
                settings.interval = Duration::from_millis(common::number(EXAMPLE, arguments, at));
            }
            other => fail(EXAMPLE, &format!("unknown option {other}")),
        }
        at += 2;
    }
    if settings.hwnd.is_some() == settings.process.is_some() {
        fail(EXAMPLE, "give one of --hwnd and --process");
    }
    if settings.rounds == 0 {
        fail(EXAMPLE, "--rounds must be at least 1");
    }
    settings
}

fn parse_hwnd(text: &str) -> isize {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => isize::from_str_radix(hex, 16),
        None => text.parse(),
    };
    parsed.unwrap_or_else(|_| fail(EXAMPLE, &format!("--hwnd {text} is not a window handle")))
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or_else(|_| fail(EXAMPLE, "a number too large"))
}

/// The read a UI Automation focus read starts with, on `uia`'s thread:
/// the element resolved, the walk's two requests built, and the element
/// refreshed with the focus's.
fn read(uia: &Uia, element: &AgileReference<IUIAutomationElement>) -> windows::core::Result<()> {
    let element = element.resolve()?;
    let ancestors = uia.request::<Ancestor>(AncestorGates {
        node: NodeGates {
            place: None,
            cell: None,
            counts: None,
            ..Node::all_on()
        },
        ..Ancestor::all_on()
    })?;
    let focus = uia.request::<Ancestor>(AncestorGates {
        node: NodeGates {
            counts: None,
            ..Node::all_on()
        },
        ..Ancestor::all_on()
    })?;
    let fresh = Live::new(element).refresh(&focus)?;
    drop((fresh, ancestors));
    Ok(())
}

/// A fresh thread's stamps, each taken on the thread.
struct Fresh {
    began: Instant,
    joined: Instant,
    client: Instant,
    read: Instant,
    ended: Instant,
}

/// One round on a fresh thread, spawned at the returned stamp: `None`
/// when the setup or the read failed.
fn fresh_round(element: &Arc<AgileReference<IUIAutomationElement>>) -> (Instant, Option<Fresh>) {
    let element = Arc::clone(element);
    let spawned = Instant::now();
    let thread = thread::Builder::new()
        .name(READING_THREAD.to_owned())
        .spawn(move || -> Option<Fresh> {
            let began = Instant::now();
            verbatim_uia::init_mta().ok()?;
            let joined = Instant::now();
            let uia = Uia::new();
            let client = Instant::now();
            let outcome = uia.as_ref().ok().map(|uia| read(uia, &element));
            let read_at = Instant::now();
            // The reading thread's end (`Reading::finish`): the reader
            // dropped, this crate's thread state released, and the
            // apartment left, once for the client's join and once for
            // the join timed apart.
            let made = uia.is_ok();
            drop(uia);
            verbatim_uia::release_thread_state();
            if made {
                verbatim_uia::leave_mta();
            }
            verbatim_uia::leave_mta();
            let ended = Instant::now();
            outcome?.ok()?;
            Some(Fresh {
                began,
                joined,
                client,
                read: read_at,
                ended,
            })
        })
        .unwrap_or_else(|error| fail(EXAMPLE, &format!("a thread could not start: {error}")));
    let stamps = thread.join().unwrap_or(None);
    (spawned, stamps)
}

/// The long-lived thread's stamps for one read: when it woke and when the
/// read ended, `None` when the read failed.
type Kept = Option<(Instant, Instant)>;

/// Starts the long-lived reading thread, set up once: each send of a
/// stamp asks it for one read, answered with its [`Kept`] stamps.
fn long_lived(
    element: &Arc<AgileReference<IUIAutomationElement>>,
) -> (
    mpsc::Sender<()>,
    mpsc::Receiver<Kept>,
    thread::JoinHandle<()>,
) {
    let element = Arc::clone(element);
    let (ask, asked) = mpsc::channel::<()>();
    let (answer, answers) = mpsc::channel::<Kept>();
    let thread = thread::Builder::new()
        .name(READING_THREAD.to_owned())
        .spawn(move || {
            let uia = Uia::new().unwrap_or_else(|error| {
                fail(EXAMPLE, &format!("the long-lived thread's client: {error}"))
            });
            while asked.recv().is_ok() {
                let woke = Instant::now();
                let outcome = read(&uia, &element);
                let done = Instant::now();
                if answer.send(outcome.ok().map(|()| (woke, done))).is_err() {
                    break;
                }
            }
            drop(uia);
            verbatim_uia::release_thread_state();
            verbatim_uia::leave_mta();
        })
        .unwrap_or_else(|error| fail(EXAMPLE, &format!("a thread could not start: {error}")));
    (ask, answers, thread)
}

#[derive(Default)]
struct Samples {
    start: Vec<f64>,
    join: Vec<f64>,
    client: Vec<f64>,
    fresh_read: Vec<f64>,
    teardown: Vec<f64>,
    fresh_whole: Vec<f64>,
    hand_off: Vec<f64>,
    kept_read: Vec<f64>,
    kept_whole: Vec<f64>,
    fresh_failed: usize,
    kept_failed: usize,
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let settings = parse_settings(&arguments);
    let hwnd = match (&settings.hwnd, &settings.process) {
        (Some(hwnd), _) => *hwnd,
        (None, Some(name)) => first_visible_window(EXAMPLE, process_named(EXAMPLE, name)),
        (None, None) => unreachable!("parse_settings asks for one"),
    };
    // This thread's client makes UI Automation's once-per-process setup
    // and looks the window's element up, once.
    let uia = Uia::new().unwrap_or_else(|error| fail(EXAMPLE, &format!("a client: {error}")));
    let live = uia
        .window_live(hwnd)
        .unwrap_or_else(|error| fail(EXAMPLE, &format!("window {hwnd:#x}'s element: {error}")));
    let element = Arc::new(
        AgileReference::new(live.element())
            .unwrap_or_else(|error| fail(EXAMPLE, &format!("an agile reference: {error}"))),
    );
    let (ask, answers, kept) = long_lived(&element);
    let mut samples = Samples::default();
    for round in 0..settings.warmup + settings.rounds {
        let counted = round >= settings.warmup;
        let (spawned, fresh) = fresh_round(&element);
        thread::sleep(settings.interval);
        let sent = Instant::now();
        ask.send(())
            .unwrap_or_else(|_| fail(EXAMPLE, "the long-lived thread ended"));
        let kept_stamps = answers
            .recv()
            .unwrap_or_else(|_| fail(EXAMPLE, "the long-lived thread ended"));
        thread::sleep(settings.interval);
        if !counted {
            continue;
        }
        match fresh {
            Some(fresh) => {
                samples.start.push(micros(fresh.began - spawned));
                samples.join.push(micros(fresh.joined - fresh.began));
                samples.client.push(micros(fresh.client - fresh.joined));
                samples.fresh_read.push(micros(fresh.read - fresh.client));
                samples.teardown.push(micros(fresh.ended - fresh.read));
                samples.fresh_whole.push(micros(fresh.read - spawned));
            }
            None => samples.fresh_failed += 1,
        }
        match kept_stamps {
            Some((woke, done)) => {
                samples.hand_off.push(micros(woke - sent));
                samples.kept_read.push(micros(done - woke));
                samples.kept_whole.push(micros(done - sent));
            }
            None => samples.kept_failed += 1,
        }
    }
    drop(ask);
    let _ = kept.join();
    drop((live, element, uia));
    verbatim_uia::release_thread_state();
    verbatim_uia::leave_mta();
    report(&settings, hwnd, &samples);
}

fn report(settings: &Settings, hwnd: isize, samples: &Samples) {
    println!(
        "window {hwnd:#x}, {} rounds after {} warm-up, {} ms apart, {}",
        settings.rounds,
        settings.warmup,
        settings.interval.as_millis(),
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    print_header("phase, microseconds");
    print_row("fresh thread: start", &samples.start);
    print_row("fresh thread: joining the apartment", &samples.join);
    print_row("fresh thread: UI Automation client", &samples.client);
    print_row("fresh thread: read", &samples.fresh_read);
    print_row("fresh thread: teardown", &samples.teardown);
    print_row("fresh thread: spawn to read's end", &samples.fresh_whole);
    print_row("long-lived thread: hand-off", &samples.hand_off);
    print_row("long-lived thread: read", &samples.kept_read);
    print_row("long-lived thread: send to read's end", &samples.kept_whole);
    println!(
        "failed rounds: {} on a fresh thread, {} on the long-lived thread",
        samples.fresh_failed, samples.kept_failed
    );
}
