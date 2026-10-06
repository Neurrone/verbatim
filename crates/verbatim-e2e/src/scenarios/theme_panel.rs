//! The settings dialog's Theme page (milestone M4 item 8,
//! `phase6-design.md`, "The settings dialog" and "How the run is
//! verified"): the page is read, the indications tree is walked through an
//! entry of every category, and the button role is changed to a sound,
//! which Verbatim then plays in place of the word "button".
//!
//! The walk: open the settings dialog and move to the Theme category; Tab
//! to the theme list (the built-in default theme) and on to the find field
//! and the tree, whose first category is selected; in each category,
//! expand it, hear its first indication with its setting, and go back to
//! the next category. Then type "button" in the find field, Tab into the
//! filtered tree, and select "button: speech". Set "Report as" to sound:
//! the default theme cannot be changed, so Verbatim asks for a new theme's
//! name, and Enter accepts the one it offers. Choose a sound, and Tab to
//! the Preview button, which is now announced with the sound in place of
//! its role ("sound: role-button" in the speech stream); press it and hear
//! the sample. Reset the indication, which is spoken again, remove the new
//! theme, and Cancel.
//!
//! Every step waits for the speech it causes, with a deadline; nothing
//! waits a fixed time. The new theme is made in the themes folder beside
//! the staged Verbatim and removed again by the walk itself; a run that
//! fails part-way can leave it there, which only adds a second theme to the
//! list of later runs.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each step's speech is given to arrive.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// Controls Tabbed through one at a time to reach one, at most.
const MAX_TABS: u32 = 16;

/// Each category of the tree, the first indication in it as the default
/// theme reports it, and the category after it.
const CATEGORIES: [(&str, &str); 6] = [
    ("Roles", "window: speech"),
    ("States", "selected: speech"),
    ("Properties", "description: speech"),
    (
        "Text formatting",
        "spelling error: speech and sound (textError.wav)",
    ),
    ("Structure", "blank: speech"),
    ("Events", "application not responding: sound"),
];

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::None)
}

/// Presses `keys` and waits for speech containing each of `heard`, in
/// order.
fn press(scenario: &mut Scenario, keys: &[&str], heard: &[&str]) {
    scenario.send_keys(keys).expect("sends the keys");
    scenario.speech().expect_in_order(heard, STEP_TIMEOUT);
}

/// Tabs until a control whose announcement contains every one of `parts`
/// has the focus, hearing each control's speech out before the next Tab.
fn tab_to(scenario: &mut Scenario, parts: &[&str]) {
    let mut heard = String::new();
    for _ in 0..MAX_TABS {
        scenario.send_keys(&["tab"]).expect("sends tab");
        heard = scenario
            .speech()
            .expect_change_capturing(&heard, STEP_TIMEOUT);
        // A control can say more after its name, such as an edit field's
        // text, as its own utterance; hear it out before moving on, so the
        // next Tab does not cut it off.
        scenario.speech().wait_until_quiet(STEP_TIMEOUT);
        if parts.iter().all(|part| heard.contains(part)) {
            return;
        }
    }
    panic!("never reached {parts:?}; last heard {heard:?}");
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario, STEP_TIMEOUT);

    // The Theme category, after Speech, and its theme list.
    press(scenario, &["downarrow"], &["Theme"]);
    press(scenario, &["tab"], &["Default"]);

    // The tree, after the find field: its first category is selected. Each
    // category is expanded to hear its first indication, then left by going
    // back to it, collapsing it, and moving down to the next.
    tab_to(scenario, &["Find"]);
    press(scenario, &["tab"], &["Roles"]);
    for (index, (category, first)) in CATEGORIES.iter().enumerate() {
        press(scenario, &["rightarrow", "downarrow"], &[first]);
        if let Some((next, _)) = CATEGORIES.get(index + 1) {
            press(scenario, &["leftarrow"], &[category]);
            press(scenario, &["leftarrow", "downarrow"], &[next]);
        }
    }

    // The find field narrows the tree to the buttons; the first category
    // is selected again, expanded, and its first match is the button role.
    press(scenario, &["shift+tab"], &["Find"]);
    for letter in ["b", "u", "t", "t", "o", "n"] {
        scenario.send_keys(&[letter]).expect("types a letter");
        scenario.speech().expect_exactly(&[letter], STEP_TIMEOUT);
    }
    press(scenario, &["tab"], &["Roles"]);
    press(scenario, &["downarrow"], &["button: speech"]);

    // Report as sound: the built-in theme asks for a new theme to make the
    // change in, offering a name, which Enter accepts.
    press(scenario, &["tab"], &["Report as"]);
    press(scenario, &["downarrow"], &["Default copy"]);
    press(scenario, &["enter"], &["Report as"]);

    // A sound for it, played as it is chosen.
    press(scenario, &["tab"], &["Sound"]);
    press(scenario, &["downarrow"], &["browseMode.wav"]);

    // The buttons are now announced with the sound in place of their role,
    // and Preview's sample plays it too.
    press(scenario, &["tab"], &["Preview", "sound: role-button"]);
    press(scenario, &["enter"], &["Sample", "sound: role-button"]);
    press(scenario, &["tab"], &["Reset", "sound: role-button"]);

    // Reset speaks the role again; the focus moves to the tree, whose item
    // says so.
    press(scenario, &["enter"], &["button: speech"]);

    // The new theme is removed, which selects the default theme again. The
    // confirmation is announced by its title and focused button; Verbatim
    // does not yet read a dialog's text on entering it (`docs/parity.md`).
    tab_to(scenario, &["Remove", "button"]);
    press(scenario, &["enter"], &["Remove Theme", "No"]);
    press(scenario, &["y"], &["Default"]);

    scenario.send_keys(&["escape"]).expect("sends escape");
    scenario
        .wait_for_window_to_close("Verbatim Settings", STEP_TIMEOUT)
        .expect("the settings dialog closes on Escape");
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "must match ScenarioDef::teardown's fn-pointer signature"
)]
pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // The harness writes fixed settings before every launch, and the walk
    // removes the theme it made.
}
