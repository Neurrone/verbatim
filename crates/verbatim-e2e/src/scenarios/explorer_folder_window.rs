//! A File Explorer folder window: opening it, arrowing through its items,
//! opening a subfolder and going back (roadmap M3's Explorer scenario).
//!
//! The folder is the scenario's own (`Scenario::open_folder`): a subfolder
//! and three files, so the list and its positions are known. The files
//! have no extension, so the names File Explorer shows are the same
//! whether or not it hides the extensions of known file types, a setting
//! of the user's that GitHub's runner and a development machine set
//! differently ("alpha.txt" is shown as "alpha" where it is on). Naming
//! them so, rather than reading the setting, keeps one fixed expectation
//! and changes no setting. On opening, the
//! window is announced by its title, the folder's name and "File
//! Explorer", then "Items View list", and the first item, not yet
//! selected, with its position; each arrow press the item and its
//! position; Enter on the subfolder "Items View list" and its first item;
//! Backspace "Items View list" and the subfolder's entry, selected again,
//! with its position, as NVDA says it. Enter and Backspace each replace
//! the items view list with a new element, so it is entered anew
//! (`docs/parity/focus-context.md`, "Focus-ancestry context").
//!
//! As a folder loads after Enter or Backspace, File Explorer may give the
//! keyboard focus for a moment to a placeholder, a static text element
//! named "Working on it...", depending on how quickly the folder loads.
//! Whether it did in a run is read from that run's own evidence, the
//! focuses Core receives (Dickson, 2026-10-17; `docs/testing.md`, "Exact
//! assertions"): when Core received a focus on the placeholder before the
//! item's, "Working on it..." is expected first, before "Items View list"
//! and the item, and otherwise it is expected absent. NVDA's captures never
//! had the placeholder, so where NVDA would say it relative to "Items View
//! list" is not known; first is where Verbatim says it, the placeholder's
//! focus coming before the item's.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::Frame;
use verbatim_model::{NormalizedEvent, Role};

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The name of File Explorer's placeholder while a folder loads.
const PLACEHOLDER: &str = "Working on it...";

/// How long the item's focus is given to reach Core after the key.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(15);

/// Waits on `events` until Core receives the focus on the list item named
/// `item`, and returns whether it received a focus on File Explorer's
/// placeholder before it.
fn placeholder_before(events: &mut ControlClient, item: &str) -> bool {
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    let mut placeholder = false;
    loop {
        match events.next_frame() {
            Ok(Frame::Event {
                event: NormalizedEvent::FocusChanged { node, .. },
                ..
            }) => match (node.role, node.name.as_deref()) {
                (Role::ListItem, Some(name)) if name == item => return placeholder,
                (Role::StaticText, Some(PLACEHOLDER)) => placeholder = true,
                _ => {}
            },
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => panic!("the event subscription failed: {error}"),
        }
        assert!(
            Instant::now() < deadline,
            "Core received no focus on {item:?} within {FOCUS_TIMEOUT:?}"
        );
    }
}

/// Presses `key`, which loads a folder whose focused item is `item`, and
/// asserts `heard`, after "Working on it..." when Core received a focus on
/// File Explorer's placeholder before the item's. In compare mode NVDA
/// offers no events, and the placeholder is not expected.
fn load_folder(scenario: &mut Scenario, key: &str, item: &str, heard: &[&str]) {
    let mut events = (!scenario.is_comparing()).then(|| {
        scenario
            .subscribe_events()
            .expect("subscribes to Verbatim's events")
    });
    scenario.send_keys(&[key]).expect("sends the key");
    let placeholder = events
        .as_mut()
        .is_some_and(|events| placeholder_before(events, item));
    let expected: Vec<&str> = if placeholder {
        std::iter::once(PLACEHOLDER)
            .chain(heard.iter().copied())
            .collect()
    } else {
        heard.to_vec()
    };
    scenario.speech().expect(&expected);
}

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let title = scenario.open_folder("folder", &["alpha", "beta", "gamma", "Inner\\delta"])?;
    Ok(ScenarioState::Title(title))
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::Title(title) = state else {
        panic!("setup opens the folder");
    };
    let window = format!("{title} - File Explorer");
    scenario
        .speech()
        .expect(&[&window, "Items View list", "Inner not selected 1 of 4"]);
    let steps: [(&str, &[&str]); 4] = [
        ("downarrow", &["alpha 2 of 4"]),
        ("downarrow", &["beta 3 of 4"]),
        ("uparrow", &["alpha 2 of 4"]),
        ("uparrow", &["Inner 1 of 4"]),
    ];
    for (key, heard) in steps {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.speech().expect(heard);
    }
    load_folder(
        scenario,
        "enter",
        "delta",
        &["Items View list", "delta not selected 1 of 1"],
    );
    load_folder(
        scenario,
        "backspace",
        "Inner",
        &["Items View list", "Inner 1 of 4"],
    );
}
