//! Verbatim's start, its replacement by a second start, and its exit, as
//! Windows sees them (`docs/parity/system.md`, "Windows' screen reader
//! flag" and "Replacing a running instance"). Once Verbatim has started,
//! Windows' screen reader flag is set. A second Verbatim started then
//! replaces the first: the first exits with code 0, having shut its
//! outposts and listener down cleanly and said nothing more, and the
//! second announces as a start does, with the flag still set. The second
//! is then exited through its menu, Verbatim+V and Exit: as NVDA exits,
//! speech is cancelled and the exit sound is the last thing played, and
//! Verbatim exits with code 0 ([`Scenario::send_keys_exiting`]); whether
//! the desktop's focus is spoken before it is not asserted (Dickson,
//! 2026-11-03). Once it has exited, the flag is cleared. The flag is read
//! through the agent, independently of Verbatim.

use crate::registry::{ScenarioState, startup_speech};
use crate::scenario::Scenario;

pub(crate) use super::no_setup as setup;

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    assert!(
        scenario
            .screen_reader_flag()
            .expect("reads the screen reader flag"),
        "Windows' screen reader flag is set once Verbatim has started"
    );

    scenario
        .replace_verbatim()
        .expect("a second Verbatim replaces the first");
    scenario.speech().expect_sequence(&startup_speech());
    super::expect_desktop(scenario);
    assert!(
        scenario
            .screen_reader_flag()
            .expect("reads the screen reader flag"),
        "Windows' screen reader flag is set once the second Verbatim has started"
    );

    super::open_verbatim_menu(scenario);
    for heard in ["Settings... s", "Exit x"] {
        scenario.send_keys(&["downarrow"]).expect("sends downarrow");
        scenario.speech().expect(&[heard]);
    }
    scenario
        .send_keys_exiting(&["enter"], "sound: exit")
        .expect("Verbatim exits through its menu");
}

/// Runs once Verbatim has exited through its menu: its exit cleared the
/// flag.
pub(crate) fn teardown(scenario: &mut Scenario, _state: ScenarioState) {
    assert!(
        !scenario
            .screen_reader_flag()
            .expect("reads the screen reader flag"),
        "Windows' screen reader flag is cleared once Verbatim has exited"
    );
}
