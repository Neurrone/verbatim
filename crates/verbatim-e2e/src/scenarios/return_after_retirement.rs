//! An application returned to after its outpost was retired (risk R2's
//! idle retirement; `books/dev/src/reference/adr/0001-focus.md`, "Spare outposts"): the
//! harness's Windows Forms text box comes to the foreground and is read by
//! an outpost of its own; a second text box, another process, then takes
//! the foreground and keeps it. Once Core reports, on its event
//! subscription, that the first box's outpost was retired, the first box is
//! brought forward again as its taskbar button does, and is announced as it
//! was the first time, read by a new outpost. The scenario's Verbatim
//! retires an outpost after 10 seconds without attention, looked for every
//! second, rather than two minutes, looked for every 30 seconds, through
//! the test-only `VERBATIM_TEST_RETIREMENT`
//! ([`crate::scenario::QUICK_RETIREMENT`]); the retirement's code is the
//! same.
//!
//! The Windows Forms text box is the fixed application because it is the
//! same on every machine and, being a real application, reports its focus
//! again when its window is activated again, as `mockapp`'s scripted
//! providers do not.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::protocol::Frame;
use verbatim_model::Pid;

use crate::registry::ScenarioState;
use crate::scenario::{Scenario, harness_marker};

pub(crate) use super::no_teardown as teardown;

/// The first window's name in the run: the application returned to.
const NAME: &str = "returned-to";

/// The second window's name in the run, which holds attention meanwhile.
const OTHER: &str = "held-meanwhile";

/// Both text boxes' name.
const BOX_NAME: &str = "Note";

/// The first text box's one line.
const LINE: &str = "Read again after retirement.";

/// The second text box's one line.
const OTHER_LINE: &str = "Holding attention meanwhile.";

/// How long the first box's outpost may take to be retired: the
/// scenario's 10 seconds without attention, its sweep's 1-second interval,
/// and a minute for a busy machine.
const RETIREMENT_TIMEOUT: Duration = Duration::from_secs(71);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::TargetPid(super::text_box::open(
        scenario, NAME, BOX_NAME, LINE,
    )?))
}

/// Waits until `events`, a subscription to Core's events, reports that the
/// outpost watching `target` was retired.
fn wait_for_retirement(events: &mut verbatim_control::client::Client, target: u32) {
    let deadline = Instant::now() + RETIREMENT_TIMEOUT;
    loop {
        match events.next_frame() {
            Ok(Frame::OutpostEnded { target_pid, reason }) if target_pid == Pid(target) => {
                assert_eq!(
                    reason, "retired",
                    "the outpost watching {target} ended, but was not retired"
                );
                return;
            }
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
            "Core did not report the retirement of the outpost watching {target} within {RETIREMENT_TIMEOUT:?}"
        );
    }
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::TargetPid(target) = *state else {
        panic!("setup records the first application's process");
    };
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);
    let retired = super::outpost_of(scenario, target);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");

    super::text_box::open(scenario, OTHER, BOX_NAME, OTHER_LINE)
        .expect("opens the second text box");
    super::text_box::expect_announced(scenario, OTHER, BOX_NAME, OTHER_LINE);
    wait_for_retirement(&mut events, target);
    scenario.expect_nothing_more();

    scenario
        .bring_window_forward(&harness_marker(NAME))
        .expect("brings the first text box forward");
    super::text_box::expect_announced(scenario, NAME, BOX_NAME, LINE);
    assert_ne!(
        super::outpost_of(scenario, target),
        retired,
        "a new outpost reads the application its retired outpost watched"
    );
}
