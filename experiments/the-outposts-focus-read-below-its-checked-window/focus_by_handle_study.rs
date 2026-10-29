//! Study (not kept in the product): can the outpost find its application's
//! keyboard focus from the checked focus window's element, by a connection by
//! handle and a search below it, instead of `GetFocusedElementBuildCache`?
//! Each case starts mockapp, moves its focus with `focus <id>`, reads the
//! window's element by handle, and searches the raw-view descendants for
//! `HasKeyboardFocus` (or a runtime id) in one remote operation, counting
//! the program's executed instructions and the provider calls mockapp
//! counted, and does the same with a classic `FindFirstBuildCache`.

mod common;
#[path = "common/harness.rs"]
mod harness;

use std::time::Instant;

use verbatim_uia::{Live, Node, Shape, Uia, runtime_id};
use verbatim_uia_rops::{
    Builder, NavigationDirection, Status, TypeTest, counting, kind, runtime_id_key,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId,
    UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId, UIA_RuntimeIdPropertyId,
};
use windows::core::BSTR;

/// What the search looks for.
#[derive(Clone, Copy)]
enum Target<'a> {
    /// The element whose `HasKeyboardFocus` is true.
    Focus,
    /// The element with this runtime id.
    Runtime(&'a [i32]),
}

/// The remote search: raw-view descendants of `root`, depth first, until one
/// matches; with `prune`, an element that is a window of its own (a nonzero
/// `NativeWindowHandle`) is not descended into. Answers the element found
/// and how many elements were looked at.
fn search(
    root: &IUIAutomationElement,
    target: Target<'_>,
    prune: bool,
) -> Result<(Option<IUIAutomationElement>, i32), verbatim_uia_rops::Error> {
    let mut b = Builder::new();
    let root = b.import_element(root);
    let key = match target {
        Target::Runtime(id) => Some(b.string(&runtime_id_key(id))),
        Target::Focus => None,
    };
    let found = b.new_null_element();
    let found = b.add_to_results(found);
    let visited = b.new_int(0);
    let visited = b.add_to_results(visited);
    let zero = b.int(0);
    let step = b.int(1);
    let depth = b.new_int(1);
    let current = b.navigate(root, NavigationDirection::FirstChild);
    b.while_(
        |b| {
            let null = b.is_null(current);
            b.not(null)
        },
        |b| {
            b.add_assign(visited, step);
            match key {
                Some(key) => {
                    // A runtime id that cannot be written out (the title
                    // bar's, served by UIA's own proxy) is no match.
                    let matched = b.new_bool(false);
                    b.try_catch(
                        |b| {
                            let id = b.property(current, UIA_RuntimeIdPropertyId.0);
                            let id = b.stringify(id);
                            let equal = b.equal(id, key);
                            b.set(matched, equal);
                        },
                        |_, _| {},
                    );
                    b.if_(matched, |b| {
                        b.set(found, current);
                        b.break_loop();
                    });
                }
                None => {
                    let focused = b.property(current, UIA_HasKeyboardFocusPropertyId.0);
                    let is_bool = b.is(TypeTest::Bool, focused);
                    b.if_(is_bool, |b| {
                        let focused = focused.assume::<kind::Bool>();
                        b.if_(focused, |b| {
                            b.set(found, current);
                            b.break_loop();
                        });
                    });
                }
            }
            let descend = if prune {
                let handle = b.property(current, UIA_NativeWindowHandlePropertyId.0);
                let is_int = b.is(TypeTest::Int, handle);
                let descend = b.new_bool(true);
                b.if_(is_int, |b| {
                    let handle = handle.assume::<kind::Int>();
                    let own = b.compare(handle, zero, verbatim_uia_rops::Comparison::NotEqual);
                    b.if_(own, |b| {
                        let no = b.bool(false);
                        b.set(descend, no);
                    });
                });
                Some(descend)
            } else {
                None
            };
            let child = b.navigate(current, NavigationDirection::FirstChild);
            let has_child = b.is_null(child);
            let has_child = b.not(has_child);
            let has_child = match descend {
                Some(descend) => b.and(has_child, descend),
                None => has_child,
            };
            b.if_(has_child, |b| {
                b.set(current, child);
                b.add_assign(depth, step);
                b.continue_loop();
            });
            let next = b.navigate(current, NavigationDirection::NextSibling);
            b.while_(
                |b| {
                    let null = b.is_null(next);
                    let inside = b.compare(depth, zero, verbatim_uia_rops::Comparison::GreaterThan);
                    b.and(null, inside)
                },
                |b| {
                    let parent = b.navigate(current, NavigationDirection::Parent);
                    b.set(current, parent);
                    b.subtract_assign(depth, step);
                    let above = b.compare(depth, zero, verbatim_uia_rops::Comparison::GreaterThan);
                    b.if_(above, |b| {
                        let sibling = b.navigate(current, NavigationDirection::NextSibling);
                        b.set(next, sibling);
                    });
                },
            );
            b.set(current, next);
        },
    );
    let outcome = b.finish().execute()?;
    Ok((outcome.get(found)?, outcome.get(visited)?))
}

/// One case's figures.
struct Case {
    fixture: &'static str,
    focus: &'static str,
    focus_name: &'static str,
}

const CASES: [Case; 7] = [
    Case { fixture: "tree.json", focus: "item1", focus_name: "First" },
    Case { fixture: "deep.json", focus: "banana", focus_name: "Banana" },
    Case { fixture: "wide.json", focus: "item99", focus_name: "Item 099" },
    Case { fixture: "study_wide150.json", focus: "item149", focus_name: "Item 149" },
    Case { fixture: "study_wide250.json", focus: "item249", focus_name: "Item 249" },
    Case { fixture: "study_wide400.json", focus: "item399", focus_name: "Item 399" },
    Case { fixture: "study_grid120.json", focus: "item119", focus_name: "Item 119" },
];

fn hits_text(hwnd: HWND) -> String {
    common::read_hits(hwnd)
        .iter()
        .map(|(name, count)| format!("{name}={count}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn total_hits(hwnd: HWND) -> u32 {
    common::read_hits(hwnd).iter().map(|(_, count)| count).sum()
}

fn run_case(case: &Case) {
    common::init_com();
    let title = common::unique_title("mockapp-study-focus");
    let mut app = common::spawn(case.fixture, "uia", &title);
    let hwnd = common::find_window(&title);
    let uia = Uia::new().expect("a UIA client");
    app.send(&format!("focus {}", case.focus));
    let request = uia.request::<Node>(Node::all_on()).expect("a request");

    // The reference: the focused node found by its name.
    let root = uia.window_live(hwnd.0 as isize).expect("the window's element");
    let name = VARIANT::from(BSTR::from(case.focus_name));
    let by_name = uia.property_condition(UIA_NamePropertyId, &name).expect("a condition");
    let expected = Live::new(root.element().clone())
        .find_first(TreeScope_Descendants, &by_name, &request)
        .expect("the search")
        .expect("the focused node");
    let expected_id = runtime_id(expected.live().element());

    // The connection by handle, as the outpost would make it, and its
    // own focus read from its cache.
    common::reset_hits(hwnd);
    let started = Instant::now();
    let window = uia.window_element(hwnd.0 as isize, &request).expect("by handle");
    let window_us = started.elapsed().as_micros();
    let window_hits = total_hits(hwnd);
    let window_focused = unsafe { window.live().element().CachedHasKeyboardFocus() }
        .map(|focused| focused.as_bool())
        .unwrap_or(false);
    let element = window.live().element().clone();

    for (label, target, prune) in [
        ("focus", Target::Focus, false),
        ("focus-pruned", Target::Focus, true),
        ("runtime-id", Target::Runtime(&expected_id), false),
    ] {
        // Counted: the counting form, valid under half the limit.
        counting::start();
        let counted = search(&element, target, prune);
        let counts = counting::stop();
        // Plain: the real run, timed, with the provider's calls.
        common::reset_hits(hwnd);
        let started = Instant::now();
        let plain = search(&element, target, prune);
        let plain_us = started.elapsed().as_micros();
        let hits = hits_text(hwnd);
        let total = total_hits(hwnd);
        let (found, visited, status) = match &plain {
            Ok((found, visited)) => (
                found.as_ref().map(runtime_id),
                *visited,
                "ok".to_owned(),
            ),
            Err(error) => (None, -1, format!("{error:?}")),
        };
        let counted_text = match (&counted, counts.as_slice()) {
            (Ok(_), [Some(count)]) => count.to_string(),
            (Err(error), _) => format!("not counted ({})", short(&format!("{error:?}"))),
            (_, other) => format!("{other:?}"),
        };
        println!(
            "RESULT study {} focus={} {label}: instructions={counted_text} visited={visited} found_matches={} status={} time_us={plain_us} provider_calls={total} [{hits}]",
            case.fixture,
            case.focus,
            found.as_deref() == Some(expected_id.as_slice()),
            short(&status),
        );
    }

    // The classic search: one FindFirstBuildCache for HasKeyboardFocus.
    let focused = VARIANT::from(true);
    let condition = uia
        .property_condition(UIA_HasKeyboardFocusPropertyId, &focused)
        .expect("a condition");
    common::reset_hits(hwnd);
    let started = Instant::now();
    let classic = Live::new(element.clone()).find_first(TreeScope_Descendants, &condition, &request);
    let classic_us = started.elapsed().as_micros();
    let hits = hits_text(hwnd);
    let total = total_hits(hwnd);
    let matches = matches!(&classic, Ok(Some(found)) if runtime_id(found.live().element()) == expected_id);
    println!(
        "RESULT study {} focus={} findfirst: found_matches={matches} time_us={classic_us} provider_calls={total} [{hits}]",
        case.fixture, case.focus,
    );
    println!(
        "RESULT study {} focus={} by-handle: window_focused={window_focused} time_us={window_us} provider_calls={window_hits}",
        case.fixture, case.focus,
    );
    let _ = Status::Success;
    if case.fixture == "tree.json" {
        race_window_local_work(hwnd, app.pid());
    }
    app.quit();
}

/// The local work `focused_control_in` does between reading the focus window
/// and calling `GetFocusedElementBuildCache`, with a kept verdict and a
/// client already made: `GetGUIThreadInfo`, the provider-process check, the
/// readability checks, and the two cache requests built. Timed 1,000 times.
fn race_window_local_work(hwnd: HWND, pid: u32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetClassNameW, GetGUIThreadInfo, IsHungAppWindow, IsWindow,
    };
    let uia = Uia::new().expect("a UIA client");
    let mut spans = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let started = Instant::now();
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        let _ = unsafe { GetGUIThreadInfo(0, &raw mut info) };
        let served = verbatim_uia::served_by(hwnd.0 as isize, pid);
        let _ = unsafe { IsWindow(Some(hwnd)) };
        let _ = unsafe { IsHungAppWindow(hwnd) };
        let mut class = [0u16; 64];
        let _ = unsafe { GetClassNameW(hwnd, &mut class) };
        let nodes = uia.request::<Node>(Node::all_on()).expect("a request");
        let ancestors = uia
            .request::<verbatim_uia::Ancestor>(verbatim_uia::Ancestor::all_on())
            .expect("a request");
        spans.push(started.elapsed().as_micros());
        assert!(served);
        drop((nodes, ancestors));
    }
    spans.sort_unstable();
    println!(
        "RESULT study race window local work: p50_us={} p99_us={} max_us={}",
        spans[500], spans[990], spans[999]
    );
}

fn short(text: &str) -> String {
    text.chars().take(120).collect()
}

fn tree() { run_case(&CASES[0]); }
fn deep() { run_case(&CASES[1]); }
fn wide() { run_case(&CASES[2]); }
fn wide150() { run_case(&CASES[3]); }
fn wide250() { run_case(&CASES[4]); }
fn wide400() { run_case(&CASES[5]); }
fn grid120() { run_case(&CASES[6]); }

fn main() {
    harness::run_isolated(&[
        ("tree", tree),
        ("deep", deep),
        ("wide", wide),
        ("wide150", wide150),
        ("wide250", wide250),
        ("wide400", wide400),
        ("grid120", grid120),
    ]);
}
