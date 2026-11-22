//! `FocusRead`'s arms answer the same by their programs and by their
//! calls (the one-path design, section 8): on every fixture
//! `focus_reports` uses, each read of a focus runs through a real
//! outpost's own `batch::answer` and the executors' `FocusRead` arms,
//! once with remote operations on and once with them off, against the
//! same mockapp, and each answer is the same both ways.
//!
//! A UI Automation fixture's focus is read as the listener's route twice,
//! the second time with the first committed, so that the listener's walk
//! meets an ancestor Core knows; as the application's focus; and as a
//! test of the focus with the read of it under the test. An MSAA
//! fixture's first nodes are each read as an acquired candidate, and its
//! focus as the application's focus, alone and under a test of its
//! focused state; MSAA has no programs, so its reads pin that remote
//! operations change nothing on its path. Each configuration starts a new
//! outpost, so the node ids the two outposts mint, in the same order, are
//! the same.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_model::TextDetails;
use verbatim_model::batch::{Batch, Check, Condition, Op, Place, Placement, Step, Subject};
use verbatim_model::matcher::Matcher;
use verbatim_model::property::{PropertyId, PropertyValue};
use verbatim_model::request::WindowVerdict;
use verbatim_process::arguments::OutpostOptions;
use windows::Win32::Foundation::HWND;

use common::outpost::{Depth, OutpostUnderTest, focus_set, node_ids};

/// How many of an MSAA fixture's first nodes are read as candidates.
const MSAA_CANDIDATES: usize = 8;

/// The batch Core sends to read the focus under a test of its focused
/// state (`verbatim-core`'s current focus at `FullIfFocused`).
fn tested_focus(known: Vec<verbatim_model::request::KnownAncestor>) -> Batch {
    Batch {
        steps: vec![
            Step {
                op: Op::Test {
                    subject: Subject::Focus,
                    matcher: Matcher::Is(PropertyId::FocusedWithin, PropertyValue::Flag(false)),
                },
                when: None,
            },
            Step {
                op: Op::FocusRead {
                    subject: Subject::Focus,
                    known,
                    set: focus_set(),
                    details: TextDetails::NONE,
                },
                when: Some(Condition {
                    on: Place(0),
                    check: Check::Matched,
                    negated: true,
                }),
            },
        ],
        placement: Placement::Queue,
        spare: None,
    }
}

/// Each read of `fixture`'s focus, by label, read by an outpost with
/// remote operations `remote`.
fn reads(
    (app, hwnd, backend): (&mut common::MockApp, HWND, &str),
    (focus, then): (Option<&str>, Option<&str>),
    remote: bool,
) -> Vec<(String, String)> {
    if let Some(focus) = focus {
        common::apply(app, hwnd, &format!("set-focus {focus}"));
    }
    let outpost = OutpostUnderTest::with_options(
        app,
        OutpostOptions {
            remote_operations: remote,
        },
    );
    let mut read = Vec::new();
    if backend == "uia" {
        let uia = verbatim_uia::Uia::new().expect("a UIA client");
        let focused = common::listener_focus(&uia, hwnd.0 as isize);
        outpost.read_focus_as(focused.live().element());
        let (route, _) = outpost.uia_focus_read(&focused);
        read.push(("the route".to_owned(), format!("{route:#?}")));
        let (again, _) = outpost.uia_focus_read(&focused);
        read.push(("the route again".to_owned(), format!("{again:#?}")));
        if let Some(then) = then {
            common::apply(app, hwnd, &format!("set-focus {then}"));
            let next = common::listener_focus(&uia, hwnd.0 as isize);
            let (route, _) = outpost.uia_focus_read(&next);
            read.push(("the next focus's route".to_owned(), format!("{route:#?}")));
            common::apply(app, hwnd, &format!("set-focus {}", focus.unwrap_or("root")));
        }
        outpost.forget_focus();
        let (now, _) = outpost.current_focus();
        read.push(("the focus".to_owned(), format!("{now:#?}")));
        let (tested, _) = outpost.batch(hwnd, WindowVerdict::Uia, tested_focus(outpost.known()));
        read.push((
            "the focus under its test".to_owned(),
            format!("{:#?}", tested.outcome),
        ));
    } else {
        for index in 1..=MSAA_CANDIDATES {
            let (candidate, _) = outpost.msaa_run_reply(node_ids(hwnd, index), Depth::Full);
            read.push((format!("candidate {index}"), format!("{candidate:#?}")));
        }
        let (now, _) = outpost.current_focus();
        read.push(("the focus".to_owned(), format!("{now:#?}")));
        outpost.forget_focus();
        let (tested, _) = outpost.batch(
            hwnd,
            WindowVerdict::ProbeFirst,
            tested_focus(outpost.known()),
        );
        read.push((
            "the focus under its test".to_owned(),
            format!("{:#?}", tested.outcome),
        ));
    }
    drop(outpost);
    read
}

/// Reads `fixture`'s focus, its element `focus` focused first, if any,
/// through `backend`, with remote operations on and off, and asserts each
/// read answered the same both ways.
fn agree(fixture: &str, backend: &str, focus: Option<&str>) {
    common::init_com();
    let title = common::unique_title("mockapp-focus-paths-agree");
    let mut app = common::spawn_visible(fixture, backend, &title);
    let hwnd = common::find_window(&title);
    let then = THEN
        .iter()
        .find(|(named, _)| *named == fixture)
        .filter(|_| backend == "uia")
        .map(|&(_, then)| then);
    let remote = reads((&mut app, hwnd, backend), (focus, then), true);
    let classic = reads((&mut app, hwnd, backend), (focus, then), false);
    assert_eq!(remote.len(), classic.len());
    for ((label, remote), (_, classic)) in remote.iter().zip(&classic) {
        assert_eq!(
            remote, classic,
            "{fixture} ({backend}), {label}: the reads with remote operations on and off answer differently"
        );
    }
    app.quit();
}

/// Every fixture `focus_reports` reads a focus in, by backend, with the
/// element focused in it, `None` where the fixture starts with its focus
/// inside a window of its own, and the message box whose dialog text
/// `call_counts` pins.
const FIXTURES: &[(&str, &str, Option<&str>)] = &[
    ("ancestry.json", "msaa", Some("deep")),
    ("ancestry.json", "uia", Some("apple")),
    ("caret_watch.json", "uia", Some("ok")),
    ("console_window.json", "uia", Some("text")),
    ("controller.json", "uia", Some("search")),
    ("counts.json", "msaa", Some("first")),
    ("counts.json", "uia", Some("first")),
    ("described_group.json", "uia", Some("louder")),
    ("dialog.json", "msaa", Some("yes")),
    ("dialog_class_window.json", "uia", Some("yes")),
    ("focus_child.json", "msaa", Some("categories")),
    ("list_view_class_window.json", "uia", Some("beta")),
    ("list_view_twins.json", "msaa", Some("root")),
    ("modal_owner.json", "uia", Some("copy")),
    ("msaa_class_window.json", "uia", Some("btn1")),
    ("nested.json", "uia", Some("far")),
    ("popup_focused_item.json", "msaa", Some("root")),
    ("read_only_field.json", "msaa", Some("location")),
    ("reuse.json", "uia", Some("inner")),
    ("reuse_terminal.json", "uia", Some("first")),
    ("reuse_text.json", "uia", Some("first")),
    ("save_as.json", "msaa", Some("save")),
    ("small.json", "msaa", Some("btn1")),
    ("small.json", "uia", Some("item1")),
    ("stale_tree.json", "msaa", Some("root")),
    ("switcher.json", "uia", Some("window_b")),
    ("text.json", "uia", Some("doc")),
    ("toast_element.json", "uia", Some("toast")),
    ("toast_element_other_class.json", "uia", Some("toast")),
    ("uia_island.json", "uia", None),
    ("uia_island_beside.json", "uia", Some("apply")),
];

/// The UI Automation fixtures whose focus moves on to a second element
/// after the first is committed, deeper below an ancestor the first
/// shares, so that the listener's walk meets it with several ancestors
/// below it.
const THEN: &[(&str, &str)] = &[("ancestry.json", "deep"), ("counts.json", "item1")];

fn focus_reads_agree() {
    for &(fixture, backend, focus) in FIXTURES {
        agree(fixture, backend, focus);
    }
}

fn main() {
    harness::run_isolated(&[("focus_reads_agree", focus_reads_agree)]);
}
