//! How many property-change events following the focus at element and
//! ancestors scope brings, per focus change, in one application.
//!
//! The application's UI Automation focus is followed as NVDA follows it
//! (`UIAHandler/__init__.py` 642 to 679 and 697 to 755): a property-changed
//! registration over the focus and its ancestors (`TreeScope_Element` with
//! `TreeScope_Ancestors`, `Scope::ElementsAndAncestors`), for NVDA's local
//! group's properties (`localEventHandlerGroupUIAPropertyIds`, 247: name,
//! help text, expand and collapse state, toggle state, enabled, value,
//! controller for and item status), moved to each new focus. A desktop-wide
//! focus registration hears each focus; a focus in the target process is
//! handed to a thread of the example's own, as NVDA hands it to its
//! multithreaded-apartment thread, which first checks that the element
//! still has the focus (`GetFocusedElement` and `CompareElements`, as
//! NVDA's `isFocus` check does) and then moves the registration to it. A
//! focus in another process moves the registration to nothing, so that
//! every event counted belongs to a focus of the target. A focus event for
//! the element already followed is not a focus change. Every registration
//! is this crate's `Registration`, the one the outpost follows with.
//!
//! The counts are of the events that reach the registration's callback,
//! which is what would reach an outpost following this way; what Core
//! would receive is at most that. Each event is attributed to the focus
//! followed when it arrives, and classed by its element's runtime id: the
//! focus itself, an earlier focus (an event of the old scope still in
//! flight), or else an ancestor of the focus; one arriving while nothing is
//! followed is counted apart. The last focus's count runs to the run's end.
//!
//! The report: focus changes seen, with the focus events skipped as no
//! longer the focus; the events per focus change (min, median, p90, p99,
//! max) and their total; the count by property, named; and the count by
//! where each came from.
//!
//! Run: `cargo run -p verbatim-uia --example follow_counts [--release] --
//! --process <name> --seconds N`. Options: `--process <name>` (the one
//! process of that name, with or without `.exe`, in any case; required),
//! `--seconds N` (30, how long to follow; use the application meanwhile).

mod common;

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use common::{fail, print_header, print_row, process_named};
use verbatim_uia::{Agile, Cached, Part, RecordedEvent, Registration, Scope, Subscription, Uia};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HelpTextPropertyId, UIA_IsEnabledPropertyId, UIA_ItemStatusPropertyId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;

const EXAMPLE: &str = "follow_counts";

/// NVDA's local group's properties, with the names the report gives them
/// (`localEventHandlerGroupUIAPropertyIds`, `UIAHandler/__init__.py` 227
/// to 249: every property NVDA maps to an event but the three it follows
/// desktop-wide).
const PROPERTIES: [(UIA_PROPERTY_ID, &str); 8] = [
    (UIA_NamePropertyId, "Name"),
    (UIA_HelpTextPropertyId, "HelpText"),
    (
        UIA_ExpandCollapseExpandCollapseStatePropertyId,
        "ExpandCollapseState",
    ),
    (UIA_ToggleToggleStatePropertyId, "ToggleState"),
    (UIA_IsEnabledPropertyId, "IsEnabled"),
    (UIA_ValueValuePropertyId, "Value"),
    (UIA_ControllerForPropertyId, "ControllerFor"),
    (UIA_ItemStatusPropertyId, "ItemStatus"),
];

struct Settings {
    process: String,
    seconds: u64,
}

fn parse_settings(arguments: &[String]) -> Settings {
    let mut process = None;
    let mut seconds = 30;
    let mut at = 0;
    while at < arguments.len() {
        match arguments[at].as_str() {
            "--process" => process = Some(common::value(EXAMPLE, arguments, at).to_owned()),
            "--seconds" => seconds = common::number(EXAMPLE, arguments, at),
            other => fail(EXAMPLE, &format!("unknown option {other}")),
        }
        at += 2;
    }
    Settings {
        process: process.unwrap_or_else(|| fail(EXAMPLE, "--process is required")),
        seconds,
    }
}

/// Where an event's element stood to the focus followed when it arrived.
#[derive(Default)]
struct Origins {
    focus: u64,
    earlier_focus: u64,
    ancestor: u64,
    nothing_followed: u64,
}

/// What the run has counted, under one lock that the focus thread and UI
/// Automation's callback threads share.
#[derive(Default)]
struct Counts {
    /// The runtime id of the focus followed, if one is.
    focus: Option<Vec<i32>>,
    /// The runtime ids of every focus followed before it.
    earlier: HashSet<Vec<i32>>,
    /// The events of the focus followed now.
    current: u64,
    /// The events of each focus followed before, in order.
    spans: Vec<u64>,
    /// Focus events skipped because the element no longer had the focus
    /// when the focus thread came to it.
    no_longer_focused: u64,
    /// Focus events for the element already followed.
    repeats: u64,
    by_property: BTreeMap<i32, u64>,
    origins: Origins,
}

impl Counts {
    /// Ends the span of the focus followed, if one is.
    fn end_span(&mut self) {
        if let Some(focus) = self.focus.take() {
            self.spans.push(self.current);
            self.earlier.insert(focus);
        }
        self.current = 0;
    }

    fn property_changed(&mut self, runtime_id: &[i32], property: i32) {
        *self.by_property.entry(property).or_default() += 1;
        match &self.focus {
            None => self.origins.nothing_followed += 1,
            Some(focus) => {
                self.current += 1;
                if focus.as_slice() == runtime_id {
                    self.origins.focus += 1;
                } else if self.earlier.contains(runtime_id) {
                    self.origins.earlier_focus += 1;
                } else {
                    self.origins.ancestor += 1;
                }
            }
        }
    }
}

type Shared = Arc<Mutex<Counts>>;

fn lock(counts: &Shared) -> std::sync::MutexGuard<'_, Counts> {
    counts.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the focus thread is handed: a focus of the target, a focus
/// elsewhere, or the run's end.
enum Focused {
    Target(Agile<RecordedEvent>),
    Elsewhere,
    Stop,
}

/// The focus thread: NVDA's multithreaded-apartment thread, which checks
/// each focus of the target still has the focus and moves the property
/// registration to it.
fn follow(focuses: &mpsc::Receiver<Focused>, properties: &Registration, counts: &Shared) {
    let uia = Uia::new().unwrap_or_else(|error| fail(EXAMPLE, &format!("a client: {error}")));
    let request = uia
        .request_fixed::<RecordedEvent>()
        .unwrap_or_else(|error| fail(EXAMPLE, &format!("a cache request: {error}")));
    while let Ok(focused) = focuses.recv() {
        let agile = match focused {
            Focused::Target(agile) => agile,
            Focused::Elsewhere => {
                lock(counts).end_span();
                properties.retarget(Scope::Nothing);
                continue;
            }
            Focused::Stop => break,
        };
        let Ok(element) = agile.resolve() else {
            continue;
        };
        let runtime_id = element.runtime_id();
        if lock(counts).focus.as_ref() == Some(&runtime_id) {
            lock(counts).repeats += 1;
            continue;
        }
        let still_focused = uia.focused(&request).and_then(|focus| {
            uia.compare_elements(focus.live().element(), element.live().element())
        });
        if !matches!(still_focused, Ok(true)) {
            lock(counts).no_longer_focused += 1;
            continue;
        }
        let Ok(reference) = AgileReference::new(element.live().element()) else {
            continue;
        };
        {
            let mut counts = lock(counts);
            counts.end_span();
            counts.focus = Some(runtime_id);
        }
        properties.retarget(Scope::ElementsAndAncestors(vec![reference]));
    }
    drop((request, uia));
    verbatim_uia::release_thread_state();
    verbatim_uia::leave_mta();
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let settings = parse_settings(&arguments);
    let pid = process_named(EXAMPLE, &settings.process);
    let counts: Shared = Arc::default();

    let properties = Arc::new(
        Registration::with_parts(
            vec![Part::shaped::<RecordedEvent>(Subscription::Properties {
                properties: PROPERTIES.iter().map(|(id, _)| *id).collect(),
                callback: {
                    let counts = Arc::clone(&counts);
                    Arc::new(move |element: &Cached<RecordedEvent>, property| {
                        let runtime_id = element.runtime_id();
                        lock(&counts).property_changed(&runtime_id, property);
                    })
                },
            })],
            Scope::Nothing,
        )
        .unwrap_or_else(|error| fail(EXAMPLE, &format!("the property registration: {error}"))),
    );

    let (focus_tx, handed) = mpsc::channel::<Focused>();
    let stop = focus_tx.clone();
    let focus_tx = Mutex::new(focus_tx);
    let focus = Registration::with_parts(
        vec![Part::shaped::<RecordedEvent>(Subscription::Focus {
            callback: Arc::new(move |element: &Cached<RecordedEvent>| {
                let focused = if element.read().process_id == Some(pid) {
                    match element.agile() {
                        Ok(agile) => Focused::Target(agile),
                        Err(_) => return,
                    }
                } else {
                    Focused::Elsewhere
                };
                let _ = focus_tx
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .send(focused);
            }),
        })],
        Scope::Desktop,
    )
    .unwrap_or_else(|error| fail(EXAMPLE, &format!("the focus registration: {error}")));

    let follower = {
        let properties = Arc::clone(&properties);
        let counts = Arc::clone(&counts);
        thread::Builder::new()
            .name("follow-counts-focus".to_owned())
            .spawn(move || follow(&handed, &properties, &counts))
            .unwrap_or_else(|error| fail(EXAMPLE, &format!("a thread could not start: {error}")))
    };

    println!(
        "following process {pid} ({}) for {} seconds",
        settings.process, settings.seconds
    );
    thread::sleep(Duration::from_secs(settings.seconds));

    // No focus is handed on once the focus registration has closed, so
    // the stop is the last thing the focus thread is handed.
    focus.close();
    drop(focus);
    let _ = stop.send(Focused::Stop);
    let _ = follower.join();
    properties.close();
    let mut counts = lock(&counts);
    counts.end_span();
    report(&settings, pid, &counts);
    drop(counts);
    drop(properties);
    verbatim_uia::release_mta_usage();
}

fn report(settings: &Settings, pid: u32, counts: &Counts) {
    println!(
        "process {pid} ({}), {} seconds, {}",
        settings.process,
        settings.seconds,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    println!(
        "focus changes seen: {} (focus events skipped: {} for the focus already followed, {} no longer the focus)",
        counts.spans.len(),
        counts.repeats,
        counts.no_longer_focused
    );
    let total: u64 = counts.by_property.values().sum();
    println!("property-change events: {total}");
    #[allow(clippy::cast_precision_loss, reason = "counts of events")]
    let per_focus: Vec<f64> = counts.spans.iter().map(|&count| count as f64).collect();
    print_header("events per focus change");
    print_row("property changes", &per_focus);
    println!("by property:");
    for (property, count) in &counts.by_property {
        let name = PROPERTIES
            .iter()
            .find(|(id, _)| id.0 == *property)
            .map_or_else(
                || format!("property {property}"),
                |(_, name)| (*name).to_owned(),
            );
        println!("  {name:<24} {count:>9}");
    }
    let origins = &counts.origins;
    println!("by origin:");
    println!("  {:<24} {:>9}", "the focus", origins.focus);
    println!("  {:<24} {:>9}", "an ancestor", origins.ancestor);
    println!("  {:<24} {:>9}", "an earlier focus", origins.earlier_focus);
    println!(
        "  {:<24} {:>9}",
        "nothing followed", origins.nothing_followed
    );
}
