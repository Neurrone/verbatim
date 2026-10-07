//! Multi-outpost regression (decision D9): a real switch between two
//! applications' outposts. A second application, the harness's Windows
//! Forms text box ([`super::text_box`]), comes to the foreground and is
//! read by an outpost of its own;
//! Verbatim+V then moves the foreground to Verbatim's own menu, read by
//! Verbatim's own outpost, and Escape returns it to the second
//! application, whose outpost is the same process it was before: it was
//! kept, not respawned, while the menu had the foreground.
//!
//! The text box is the fixed second application because it is the same on
//! every machine, and it is a real application, which reports its focus
//! again when its window is activated again, as `mockapp`'s scripted
//! providers do not.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The window's name in the run.
const NAME: &str = "second-application";

/// The text box's name.
const BOX_NAME: &str = "Note";

/// The text box's one line.
const LINE: &str = "Hello from the second application.";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::TargetPid(super::text_box::open(
        scenario, NAME, BOX_NAME, LINE,
    )?))
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::TargetPid(target) = *state else {
        panic!("setup records the second application's process");
    };
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);
    let outpost = super::outpost_of(scenario, target);

    super::open_verbatim_menu(scenario);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&["Settings... s"]);
    scenario.send_keys(&["escape"]).expect("sends escape");
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);

    assert_eq!(
        super::outpost_of(scenario, target),
        outpost,
        "the second application's outpost was kept while Verbatim's menu had the foreground"
    );
}
