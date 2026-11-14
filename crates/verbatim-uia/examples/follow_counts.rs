//! How many property-change events following the focus at element and
//! ancestors scope brings, per focus change, in one application.
//!
//! The application's UI Automation focus is followed as NVDA follows it
//! (`UIAHandler/__init__.py` 642 to 679 and 697 to 755): a property-changed
//! registration over the focus and its ancestors (`TreeScope_Element` with
//! `TreeScope_Ancestors`, `Scope::ElementsAndAncestors`), for NVDA's local
//! group's properties (`localEventHandlerGroupUIAPropertyIds`, 247: name,
//! help text, expand and collapse state, toggle state, enabled, value,
//! controller for and item status), moved to each new focus. Every
//! registration is this crate's `Registration`, the one the outpost
//! follows with.
//!
//! A desktop-wide focus registration hears each focus. The application is
//! every process of the name given, whenever it started: File Explorer's
//! folder windows can run in an `explorer.exe` of their own, apart from
//! the shell's. A focus of the application is handed to a thread of the
//! example's own, NVDA's multithreaded-apartment thread, which judges it
//! in NVDA's order, each check counted when it refuses:
//!
//! 1. Native UI Automation (`isNativeUIAElement`, 1516): the element's
//!    nearest window (`getNearestWindowHandle`) is judged by its classes
//!    and then the raw probe and the checks after it, as the listener
//!    judges a window (`isUIAWindow`). An element of a window with no
//!    provider of its own, served by UI Automation's proxies, is one NVDA
//!    reads through MSAA, and never follows this way.
//! 2. Not the focus already followed (the duplicate test, 902 to 923, by
//!    runtime id).
//! 3. NVDA's acceptance test for a focus (`shouldAcceptEvent`, 924 to
//!    930): the window is related to the foreground window.
//! 4. Still holding the keyboard focus (`shouldAllowUIAFocusEvent`,
//!    `NVDAObjects/UIA/__init__.py` 1632 to 1637, a live read; a failed
//!    read allows it).
//! 5. Still the focus (`isFocus` in `addLocalEventHandlerGroupToElement`,
//!    703 to 711): `GetFocusedElement`, with no cache request, and
//!    `CompareElements`; a failure of either counts as not the focus, as
//!    NVDA counts it, and is reported with its error.
//!
//! A focus that passes is followed: the registration moves to it, and the
//! thread waits for the move to be made, as NVDA's thread waits for its
//! `AddEventHandlerGroup`, before its ancestors are read with a raw-view
//! walk up to the desktop's root,
//! by runtime id, so that each event is classed by where it came from. A
//! focus refused at the first or last check, or a focus in another
//! process, ends the following, as NVDA's focus leaving a UI Automation
//! object removes its group (`event_loseFocus`); a focus refused at the
//! others leaves the focus followed as it was.
//!
//! The counts are of the events that reach the registration's callback,
//! which is what would reach an outpost following this way; what Core
//! would receive is at most that. Each event is attributed to the focus
//! followed when it arrives, and classed by its element's runtime id: the
//! focus itself, an ancestor of the focus, an earlier focus (an event of
//! the old scope still in flight), or another element; one arriving while
//! nothing is followed is counted apart. An event that arrives before the
//! focus's ancestors are read is classed once they are. The last focus's
//! count runs to the run's end.
//!
//! The report: the processes seen; the focus changes followed, with each
//! check's refusals (the not-native ones by window class, the failed
//! focus checks by error); the events per focus change (min, median, p90,
//! p99, max) and their total; the count by property, named; and the count
//! by where each came from. With `--events`, each focus's judgement and
//! each event are also printed as they happen, one line each: `followed`,
//! `refused` or `elsewhere` for a focus, `registered` once the
//! registration has moved to a focus followed, and `event` for a property
//! change.
//!
//! Run: `cargo run -p verbatim-uia --example follow_counts [--release] --
//! --process <name> --seconds N`. Options: `--process <name>` (every
//! process of that name, with or without `.exe`, in any case, including
//! one started during the run; required), `--seconds N` (30, how long to
//! follow; use the application meanwhile), `--events` (print each focus
//! and event as it happens).

mod common;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use common::{fail, print_header, print_row, process_stem, processes};
use verbatim_reader_rules::Window;
use verbatim_reader_rules::acceptance::{EventName, RequestedEvents, accepts_event};
use verbatim_reader_rules::class::{
    ClassVerdict, PostProbeCheck, class_verdict, has_windows_forms_class, post_probe_check,
};
use verbatim_uia::{
    Agile, Cached, Direction, Live, Part, RecordedEvent, Registration, Scope, Subscription, Uia,
};
use verbatim_win32::window::{SystemWindows, foreground_window_handle, window_classes};
use windows::Win32::UI::Accessibility::{
    UIA_ControllerForPropertyId, UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_HelpTextPropertyId, UIA_IsEnabledPropertyId, UIA_ItemStatusPropertyId, UIA_NamePropertyId,
    UIA_PROPERTY_ID, UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};
use windows::core::AgileReference;

const EXAMPLE: &str = "follow_counts";

/// The most raw-view parents read above a focus: deeper than any real
/// application's tree, so a walk that has not reached the root by then is
/// looping.
const MAX_ANCESTORS: usize = 64;

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
    events: bool,
}

fn parse_settings(arguments: &[String]) -> Settings {
    let mut process = None;
    let mut seconds = 30;
    let mut events = false;
    let mut at = 0;
    while at < arguments.len() {
        match arguments[at].as_str() {
            "--process" => process = Some(common::value(EXAMPLE, arguments, at).to_owned()),
            "--seconds" => seconds = common::number(EXAMPLE, arguments, at),
            "--events" => {
                events = true;
                at += 1;
                continue;
            }
            other => fail(EXAMPLE, &format!("unknown option {other}")),
        }
        at += 2;
    }
    Settings {
        process: process.unwrap_or_else(|| fail(EXAMPLE, "--process is required")),
        seconds,
        events,
    }
}

/// The name a property is reported by.
fn property_name(property: i32) -> String {
    PROPERTIES
        .iter()
        .find(|(id, _)| id.0 == property)
        .map_or_else(
            || format!("property {property}"),
            |(_, name)| (*name).to_owned(),
        )
}

/// Where an event's element stood to the focus followed when it arrived.
#[derive(Clone, Copy)]
enum Origin {
    Focus,
    Ancestor,
    EarlierFocus,
    Other,
}

impl Origin {
    fn describe(self) -> &'static str {
        match self {
            Self::Focus => "the focus",
            Self::Ancestor => "an ancestor",
            Self::EarlierFocus => "an earlier focus",
            Self::Other => "another element",
        }
    }
}

#[derive(Default)]
struct Origins {
    focus: u64,
    ancestor: u64,
    earlier_focus: u64,
    other: u64,
    nothing_followed: u64,
}

impl Origins {
    fn add(&mut self, origin: Origin) {
        match origin {
            Origin::Focus => self.focus += 1,
            Origin::Ancestor => self.ancestor += 1,
            Origin::EarlierFocus => self.earlier_focus += 1,
            Origin::Other => self.other += 1,
        }
    }
}

/// An event waiting for the focus's ancestors to be read: its element's
/// runtime id, its property and its element's name.
struct Waiting {
    runtime_id: Vec<i32>,
    property: i32,
    name: String,
}

/// What is known of the followed focus's ancestors.
enum Ancestry {
    /// Being read; the events that need them to be classed wait here.
    Reading(Vec<Waiting>),
    /// Read: their runtime ids.
    Read(HashSet<Vec<i32>>),
}

/// The focus followed.
struct Followed {
    runtime_id: Vec<i32>,
    ancestry: Ancestry,
}

/// Why a focus of the application was not followed.
#[derive(Default)]
struct Refusals {
    /// Not native UI Automation, by its nearest window's class, or
    /// `(no window)`.
    not_native: BTreeMap<String, u64>,
    /// The focus already followed.
    repeats: u64,
    /// Refused by the acceptance test.
    not_accepted: u64,
    /// No longer holding the keyboard focus.
    lost_keyboard_focus: u64,
    /// `GetFocusedElement` named another element.
    no_longer_focused: u64,
    /// `GetFocusedElement` or `CompareElements` failed, by error.
    focus_check_failed: BTreeMap<String, u64>,
    /// The element could not be resolved on the focus thread.
    unresolved: u64,
}

/// What the run has counted, under one lock that the focus thread and UI
/// Automation's callback threads share.
#[derive(Default)]
struct Counts {
    /// Whether each line is printed as it happens (`--events`).
    print: bool,
    /// The focus followed, if one is.
    focus: Option<Followed>,
    /// The runtime ids of every focus followed before it.
    earlier: HashSet<Vec<i32>>,
    /// The events of the focus followed now.
    current: u64,
    /// The events of each focus followed before, in order.
    spans: Vec<u64>,
    /// The application's processes whose focus events came.
    processes: BTreeSet<u32>,
    /// Focus events of other processes.
    elsewhere: u64,
    refusals: Refusals,
    by_property: BTreeMap<i32, u64>,
    origins: Origins,
}

impl Counts {
    /// Ends the span of the focus followed, if one is, classing any event
    /// still waiting for its ancestors as from another element.
    fn end_span(&mut self) {
        if let Some(followed) = self.focus.take() {
            if let Ancestry::Reading(waiting) = followed.ancestry {
                for event in waiting {
                    self.classed(&event, Origin::Other);
                }
            }
            self.spans.push(self.current);
            self.earlier.insert(followed.runtime_id);
        }
        self.current = 0;
    }

    /// Starts following the focus `runtime_id`, its ancestors not yet read.
    fn follow(&mut self, runtime_id: Vec<i32>, name: &str) {
        self.end_span();
        if self.print {
            println!("followed {name:?} {runtime_id:?}");
        }
        self.focus = Some(Followed {
            runtime_id,
            ancestry: Ancestry::Reading(Vec::new()),
        });
    }

    /// The followed focus's ancestors, read: classes the events that
    /// waited for them.
    fn ancestors_read(&mut self, ancestors: HashSet<Vec<i32>>) {
        let Some(followed) = self.focus.as_mut() else {
            return;
        };
        let waiting = match std::mem::replace(&mut followed.ancestry, Ancestry::Read(ancestors)) {
            Ancestry::Reading(waiting) => waiting,
            Ancestry::Read(_) => Vec::new(),
        };
        for event in waiting {
            let origin = self.origin(&event.runtime_id);
            self.classed(&event, origin);
        }
    }

    /// Where an element stands to the focus followed, its ancestors read.
    fn origin(&self, runtime_id: &[i32]) -> Origin {
        let ancestor = matches!(
            self.focus.as_ref().map(|followed| &followed.ancestry),
            Some(Ancestry::Read(ancestors)) if ancestors.contains(runtime_id)
        );
        if ancestor {
            Origin::Ancestor
        } else if self.earlier.contains(runtime_id) {
            Origin::EarlierFocus
        } else {
            Origin::Other
        }
    }

    fn classed(&mut self, event: &Waiting, origin: Origin) {
        self.origins.add(origin);
        if self.print {
            println!(
                "event {} from {} {:?} {:?}",
                property_name(event.property),
                origin.describe(),
                event.name,
                event.runtime_id
            );
        }
    }

    fn property_changed(&mut self, event: Waiting) {
        *self.by_property.entry(event.property).or_default() += 1;
        let Some(followed) = self.focus.as_mut() else {
            self.origins.nothing_followed += 1;
            if self.print {
                println!(
                    "event {} while nothing is followed {:?} {:?}",
                    property_name(event.property),
                    event.name,
                    event.runtime_id
                );
            }
            return;
        };
        self.current += 1;
        if followed.runtime_id == event.runtime_id {
            self.classed(&event, Origin::Focus);
            return;
        }
        match &mut followed.ancestry {
            Ancestry::Reading(waiting) => waiting.push(event),
            Ancestry::Read(_) => {
                let origin = self.origin(&event.runtime_id);
                self.classed(&event, origin);
            }
        }
    }

    fn refused(&mut self, why: &str, name: &str) {
        if self.print {
            println!("refused {name:?}: {why}");
        }
    }
}

type Shared = Arc<Mutex<Counts>>;

fn lock(counts: &Shared) -> std::sync::MutexGuard<'_, Counts> {
    counts.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the focus thread is handed: a focus of the application, a focus
/// elsewhere, or the run's end.
enum Focused {
    Target(Agile<RecordedEvent>),
    Elsewhere,
    Stop,
}

/// Whether `hwnd` is a window NVDA reads through UI Automation
/// (`isUIAWindow`): by its classes, then the raw probe and the checks
/// after it, as the listener's `window_judged` decides. A console with a
/// provider counts as UI Automation, as there.
fn window_is_uia(hwnd: isize) -> bool {
    let classes = window_classes(hwnd);
    match class_verdict(&classes) {
        ClassVerdict::Uia => true,
        ClassVerdict::NonUia => false,
        ClassVerdict::Unknown => {
            verbatim_uia::raw_probe(hwnd).has_provider()
                && match post_probe_check(&classes) {
                    Some(PostProbeCheck::Console) | None => true,
                    Some(PostProbeCheck::WindowsFormsListView) => {
                        has_windows_forms_class(&classes.raw)
                    }
                }
        }
    }
}

/// The runtime ids of `element`'s raw-view ancestors below the desktop's
/// root, read by stepping to each parent in turn; the walk ends at the
/// root, at a step that fails, or after [`MAX_ANCESTORS`].
fn ancestors_of(uia: &Uia, element: &Live, root: &[i32]) -> HashSet<Vec<i32>> {
    let mut ancestors = HashSet::new();
    let Ok(walker) = uia.raw_view_walker() else {
        return ancestors;
    };
    let mut current = element.clone();
    while ancestors.len() < MAX_ANCESTORS {
        let Ok(parent) = current.step(&walker, Direction::Parent) else {
            break;
        };
        let runtime_id = parent.runtime_id();
        if runtime_id.as_slice() == root {
            break;
        }
        ancestors.insert(runtime_id);
        current = parent;
    }
    ancestors
}

/// The focus thread: NVDA's multithreaded-apartment thread, which judges
/// each focus of the application in NVDA's order and moves the property
/// registration to the one that passes.
#[expect(clippy::too_many_lines, reason = "one step per check, in NVDA's order")]
fn follow(focuses: &mpsc::Receiver<Focused>, properties: &Registration, counts: &Shared) {
    let uia = Uia::new().unwrap_or_else(|error| fail(EXAMPLE, &format!("a client: {error}")));
    let root = uia.root_element().map_or_else(
        |error| fail(EXAMPLE, &format!("the desktop's root: {error}")),
        |root| verbatim_uia::runtime_id(&root),
    );
    while let Ok(focused) = focuses.recv() {
        let agile = match focused {
            Focused::Target(agile) => agile,
            Focused::Elsewhere => {
                let mut counts = lock(counts);
                counts.elsewhere += 1;
                if counts.print {
                    println!("elsewhere");
                }
                if counts.focus.is_some() {
                    counts.end_span();
                    drop(counts);
                    properties.retarget(Scope::Nothing);
                }
                continue;
            }
            Focused::Stop => break,
        };
        let Ok(element) = agile.resolve() else {
            lock(counts).refusals.unresolved += 1;
            continue;
        };
        let read = element.read();
        let name = read.name.clone().unwrap_or_default();
        let runtime_id = element.runtime_id();
        let live = element.live();

        // 1. Native UI Automation: its nearest window is a UIA window.
        let window = verbatim_uia::nearest_window_lookup(live.element())
            .ok()
            .flatten();
        let native = window.is_some_and(window_is_uia);
        if !native {
            let class = window.map_or_else(
                || "(no window)".to_owned(),
                |window| window_classes(window).raw,
            );
            let mut counts = lock(counts);
            counts.refused(&format!("not native UIA, window class {class:?}"), &name);
            *counts.refusals.not_native.entry(class).or_default() += 1;
            if counts.focus.is_some() {
                counts.end_span();
                drop(counts);
                properties.retarget(Scope::Nothing);
            }
            continue;
        }

        // 2. Not the focus already followed.
        {
            let mut counts = lock(counts);
            if counts
                .focus
                .as_ref()
                .is_some_and(|followed| followed.runtime_id == runtime_id)
            {
                counts.refusals.repeats += 1;
                counts.refused("the focus already followed", &name);
                continue;
            }
        }

        // 3. The acceptance test, against the foreground window now.
        let windows = SystemWindows {
            foreground: foreground_window_handle(),
        };
        let accepted = accepts_event(
            EventName::GainFocus,
            window.and_then(Window::new),
            &windows,
            &RequestedEvents::default(),
        );
        if !accepted {
            let mut counts = lock(counts);
            counts.refusals.not_accepted += 1;
            counts.refused("not related to the foreground window", &name);
            continue;
        }

        // 4. Still holding the keyboard focus; a failed read allows it.
        // SAFETY: a live element of this thread's apartment; a plain read.
        let holds = unsafe { live.element().CurrentHasKeyboardFocus() };
        if matches!(holds.map(windows_core::BOOL::as_bool), Ok(false)) {
            let mut counts = lock(counts);
            counts.refusals.lost_keyboard_focus += 1;
            counts.refused("no longer holds the keyboard focus", &name);
            continue;
        }

        // 5. Still the focus: `GetFocusedElement` and `CompareElements`.
        // SAFETY: the client is this thread's own; the call takes no
        // arguments.
        let still_focused = unsafe { uia.client().GetFocusedElement() }
            .and_then(|focus| uia.compare_elements(&focus, live.element()));
        let refusal = match still_focused {
            Ok(true) => None,
            Ok(false) => Some(None),
            Err(error) => Some(Some(error.message())),
        };
        if let Some(error) = refusal {
            let mut counts = lock(counts);
            match error {
                None => {
                    counts.refusals.no_longer_focused += 1;
                    counts.refused("no longer the focus", &name);
                }
                Some(error) => {
                    counts.refused(&format!("the focus check failed: {error}"), &name);
                    *counts.refusals.focus_check_failed.entry(error).or_default() += 1;
                }
            }
            if counts.focus.is_some() {
                counts.end_span();
                drop(counts);
                properties.retarget(Scope::Nothing);
            }
            continue;
        }

        let Ok(reference) = AgileReference::new(live.element()) else {
            lock(counts).refusals.unresolved += 1;
            continue;
        };
        lock(counts).follow(runtime_id, &name);
        properties.retarget(Scope::ElementsAndAncestors(vec![reference]));
        // NVDA's thread adds the group with a call it waits for before
        // judging the next focus.
        properties.settle();
        if lock(counts).print {
            println!("registered {name:?}");
        }
        let ancestors = ancestors_of(&uia, live, &root);
        lock(counts).ancestors_read(ancestors);
    }
    drop(uia);
    verbatim_uia::release_thread_state();
    verbatim_uia::leave_mta();
}

/// Whether process `pid` is named `stem`, from `known` or else from a
/// fresh list of the processes, which `known` then keeps.
fn is_named(known: &Mutex<HashMap<u32, bool>>, pid: u32, stem: &str) -> bool {
    let mut known = known.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&named) = known.get(&pid) {
        return named;
    }
    let named = processes(EXAMPLE)
        .into_iter()
        .any(|(id, image)| id == pid && image == stem);
    known.insert(pid, named);
    named
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let settings = parse_settings(&arguments);
    let stem = process_stem(&settings.process);
    let counts: Shared = Arc::new(Mutex::new(Counts {
        print: settings.events,
        ..Counts::default()
    }));

    let properties = Arc::new(
        Registration::with_parts(
            vec![Part::shaped::<RecordedEvent>(Subscription::Properties {
                properties: PROPERTIES.iter().map(|(id, _)| *id).collect(),
                callback: {
                    let counts = Arc::clone(&counts);
                    Arc::new(move |element: &Cached<RecordedEvent>, property| {
                        let event = Waiting {
                            runtime_id: element.runtime_id(),
                            property,
                            name: element.read().name.unwrap_or_default(),
                        };
                        lock(&counts).property_changed(event);
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
    let known = Mutex::new(HashMap::new());
    let seen = Arc::clone(&counts);
    let focus = Registration::with_parts(
        vec![Part::shaped::<RecordedEvent>(Subscription::Focus {
            callback: Arc::new(move |element: &Cached<RecordedEvent>| {
                let pid = element.read().process_id;
                let focused = match pid {
                    Some(pid) if is_named(&known, pid, &stem) => {
                        lock(&seen).processes.insert(pid);
                        match element.agile() {
                            Ok(agile) => Focused::Target(agile),
                            Err(_) => return,
                        }
                    }
                    _ => Focused::Elsewhere,
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
        "following the processes named {} for {} seconds",
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
    report(&settings, &counts);
    drop(counts);
    drop(properties);
    verbatim_uia::release_mta_usage();
}

fn report(settings: &Settings, counts: &Counts) {
    println!(
        "processes named {} ({:?}), {} seconds, {}",
        settings.process,
        counts.processes,
        settings.seconds,
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    let refusals = &counts.refusals;
    println!("focus changes followed: {}", counts.spans.len());
    println!("focus events of other processes: {}", counts.elsewhere);
    println!("focus events not followed:");
    let not_native: u64 = refusals.not_native.values().sum();
    println!("  {:<40} {:>9}", "not native UIA", not_native);
    for (class, count) in &refusals.not_native {
        println!("    window class {class:<29} {count:>9}");
    }
    println!(
        "  {:<40} {:>9}",
        "the focus already followed", refusals.repeats
    );
    println!(
        "  {:<40} {:>9}",
        "not related to the foreground", refusals.not_accepted
    );
    println!(
        "  {:<40} {:>9}",
        "no longer holding the keyboard focus", refusals.lost_keyboard_focus
    );
    println!(
        "  {:<40} {:>9}",
        "no longer the focus", refusals.no_longer_focused
    );
    let failed: u64 = refusals.focus_check_failed.values().sum();
    println!("  {:<40} {:>9}", "the focus check failed", failed);
    for (error, count) in &refusals.focus_check_failed {
        println!("    {error}: {count}");
    }
    println!("  {:<40} {:>9}", "not resolved", refusals.unresolved);
    let total: u64 = counts.by_property.values().sum();
    println!("property-change events: {total}");
    #[allow(clippy::cast_precision_loss, reason = "counts of events")]
    let per_focus: Vec<f64> = counts.spans.iter().map(|&count| count as f64).collect();
    print_header("events per focus change");
    print_row("property changes", &per_focus);
    println!("by property:");
    for (property, count) in &counts.by_property {
        println!("  {:<24} {count:>9}", property_name(*property));
    }
    let origins = &counts.origins;
    println!("by origin:");
    println!("  {:<24} {:>9}", "the focus", origins.focus);
    println!("  {:<24} {:>9}", "an ancestor", origins.ancestor);
    println!("  {:<24} {:>9}", "an earlier focus", origins.earlier_focus);
    println!("  {:<24} {:>9}", "another element", origins.other);
    println!(
        "  {:<24} {:>9}",
        "nothing followed", origins.nothing_followed
    );
}
