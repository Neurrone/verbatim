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
//! While it loads a folder, on the window's opening and after Enter or
//! Backspace, File Explorer may give the keyboard focus for a moment to a
//! placeholder, a static text element named "Working on it...", and then
//! give it to the item: on GitHub's runners in 8 of 18 recorded runs after
//! Enter, after Backspace in runs 39558690998 and 39583748471, and on the
//! opening in run 39603654902; in none of 5 local runs of this scenario,
//! though `measure_focus_explorer` has met it on a development machine.
//! Whether it comes depends on how quickly the machine loads the folder,
//! so it is an optional utterance (`docs/testing.md`, "Optional
//! utterances", approved by Dickson for Enter on 2026-10-17 and for
//! Backspace and the opening on 2026-10-21) rather than an expectation
//! that depends on the machine: heard in full, cut off by the item's
//! focus, or not said at all. Where it lands depends on the focus
//! listener's own read of the placeholder (`verbatim-listener`'s
//! `focus_read.rs`). When File Explorer removes it after the listener's
//! nearest-window walk, the listener's classic read of it fails, or its
//! read budget runs out, and the listener reports it with its ancestors
//! unknown, so it is spoken with no ancestors, before what the item's
//! focus brings: "Items View list", and on the opening the window's title
//! before it; when the read succeeds, its ancestry ends in the list, so
//! that speech comes first and the placeholder between it and the item.
//! NVDA's own handling depends on the same kind of timing and speaks a
//! placeholder whose element is gone by its main thread's handling
//! without its list (`docs/parity/focus-events.md`, "A focus whose
//! element is gone before its read"), so each of the three steps accepts
//! the placeholder at either position, an explicit exception to the
//! one-outcome rule (Dickson, 2026-10-21). The two optional places would
//! also accept the placeholder said at both, so each step asserts that it
//! was said at most once. The item is still required: a run where the
//! placeholder keeps the focus fails.

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
    expect_after_load(
        scenario,
        &[&window, "Items View list"],
        "Inner not selected 1 of 4",
    );
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
    expect_after_load(scenario, &["Items View list"], "delta not selected 1 of 1");
    scenario.send_keys(&["backspace"]).expect("sends the key");
    expect_after_load(scenario, &["Items View list"], "Inner 1 of 4");
}

/// The speech of a folder load: `context`, the window's title on the
/// opening and "Items View list", then `item`, with File Explorer's
/// placeholder optional before `context` or between it and the item, and
/// said at most once in all.
fn expect_after_load(scenario: &mut Scenario, context: &[&str], item: &str) {
    let placeholder = "Working on it...";
    let before = scenario.speech().optional_outcomes().len();
    let mut expected = vec![optional(placeholder)];
    expected.extend(context.iter().copied().map(heard));
    expected.extend([optional(placeholder), heard(item)]);
    scenario.speech().expect_sequence(&expected);
    let said: usize = scenario.speech().optional_outcomes()[before..]
        .iter()
        .map(|outcome| outcome.endings.len())
        .sum();
    assert!(
        said <= 1,
        "{placeholder:?} was said {said} times in the folder load ending in {item:?}, where at most once is accepted"
    );
}
