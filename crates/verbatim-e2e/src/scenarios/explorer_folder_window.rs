//! A File Explorer folder window: opening it, arrowing through its items,
//! opening a subfolder and going back (roadmap M3's Explorer scenario).
//!
//! The folder is the scenario's own (`Scenario::open_folder`): a subfolder
//! and three files, so the list and its positions are known. On opening, the
//! window is announced by its title, the folder's name and "File
//! Explorer", then "Items View list", and the first item, not yet
//! selected, with its position; each arrow press the item and its
//! position; Enter on the subfolder its first item; Backspace back to the
//! subfolder's entry, which Explorer selects once it has the focus, which
//! is announced as "selected".

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    let title = scenario.open_folder(
        "folder",
        &["alpha.txt", "beta.txt", "gamma.txt", "Inner\\delta.txt"],
    )?;
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
    let steps: [(&str, &[&str]); 6] = [
        ("downarrow", &["alpha.txt 2 of 4"]),
        ("downarrow", &["beta.txt 3 of 4"]),
        ("uparrow", &["alpha.txt 2 of 4"]),
        ("uparrow", &["Inner 1 of 4"]),
        ("enter", &["delta.txt not selected 1 of 1"]),
        ("backspace", &["Inner not selected 1 of 4", "selected"]),
    ];
    for (key, heard) in steps {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.speech().expect(heard);
    }
}
