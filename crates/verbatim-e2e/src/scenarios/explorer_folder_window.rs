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
//! While it loads a folder after Enter or Backspace, File Explorer may give
//! the keyboard focus for a moment to a placeholder, a static text element
//! named "Working on it...", and then give it to the item: on GitHub's
//! runners in 8 of 18 recorded runs after Enter, and after Backspace in
//! runs 39558690998 and 39583748471; in none of 5 local runs of this
//! scenario, though `measure_focus_explorer` has met it on a development
//! machine. Whether it comes depends on how quickly the machine loads the
//! folder, so it is an optional utterance (`docs/testing.md`, "Optional
//! utterances", approved by Dickson for Enter on 2026-10-17 and for
//! Backspace on 2026-10-21) rather than an expectation that depends on the
//! machine: heard in full, cut off by the item's focus, or not said at
//! all. Where it lands depends on whether the outpost's read of the
//! placeholder's focus still finds its element. When File Explorer has
//! already removed it (the outpost logs "the focus's own element looked
//! for by its path" with found=false, and the focus is reported with
//! `ancestors_unknown`), the placeholder is spoken with no ancestors,
//! before "Items View list", which comes with the item's focus; when the
//! read finds it, its ancestry ends in the new list, so "Items View list"
//! is spoken first and the placeholder between it and the item. NVDA's
//! own handling depends on the same timing and speaks a placeholder whose
//! element is gone by its main thread's read without its list
//! (`docs/parity/focus-events.md`, "A focus whose element is gone before
//! its read"), so each step accepts the placeholder at either position,
//! an explicit exception to the one-outcome rule (Dickson, 2026-10-21).
//! The two optional places would also accept the placeholder said at
//! both, so each step asserts that it was said at most once. The item is
//! still required: a run where the placeholder keeps the focus fails. No
//! run has had it on the window's opening, so it is not declared there.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{heard, optional};

pub(crate) use super::no_teardown as teardown;

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
    for (key, said) in steps {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.speech().expect(said);
    }
    scenario.send_keys(&["enter"]).expect("sends the key");
    expect_after_load(scenario, "delta not selected 1 of 1");
    scenario.send_keys(&["backspace"]).expect("sends the key");
    expect_after_load(scenario, "Inner 1 of 4");
}

/// The speech of a folder load: "Items View list" and `item`, with File
/// Explorer's placeholder optional before the list or between it and the
/// item, and said at most once in all.
fn expect_after_load(scenario: &mut Scenario, item: &str) {
    let placeholder = "Working on it...";
    let before = scenario.speech().optional_outcomes().len();
    let expected = [
        optional(placeholder),
        heard("Items View list"),
        optional(placeholder),
        heard(item),
    ];
    scenario.speech().expect_sequence(&expected);
    let said: usize = scenario.speech().optional_outcomes()[before..]
        .iter()
        .map(|outcome| outcome.endings.len())
        .sum();
    assert!(
        said <= 1,
        "{placeholder:?} was said {said} times in one step, where at most once is accepted"
    );
}
