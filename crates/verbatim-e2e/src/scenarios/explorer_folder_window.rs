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
//! (`docs/parity/focus-context.md`, "Focus-ancestry context"). NVDA's
//! captures at the pin say exactly these in each of 10 runs, with no
//! placeholder and no pane, and the same opening for a new window made by
//! Control+N in each of 10 more. NVDA also says the folder's tab,
//! selected, after the opening in 7 of the 10 runs, from a UIA selection
//! event of the tab item (`SelectionItem_ElementSelected`), which the
//! expectation leaves out.
//!
//! While it loads a folder, on the window's opening and after Enter or
//! Backspace, File Explorer may give the keyboard focus for a moment to a
//! placeholder, a static text element named "Working on it...", and then
//! give it to the item. The listener reads every focus's keyboard focus
//! live, as NVDA does, so a placeholder that has lost the focus by its read
//! is dropped. NVDA's captures at the pin say no placeholder, so it is
//! never expected, and a run that says it fails.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

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
    // Every move was within the folder window: no report would have spoken
    // a cached ancestor.
    scenario.expect_live_re_reads(0);
}

/// The speech of a folder load: `context`, the window's title on the
/// opening and "Items View list", then `item`.
fn expect_after_load(scenario: &mut Scenario, context: &[&str], item: &str) {
    let mut expected = context.to_vec();
    expected.push(item);
    scenario.speech().expect(&expected);
}
