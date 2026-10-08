//! An outpost that dies is replaced (decision D9): reading goes on after
//! the outpost watching an application is ended. `mockapp`, showing a fixed
//! text box, comes to the foreground and is read by its outpost; that
//! outpost is ended by its process id, as a crash would end it. Once Core
//! reports the outpost's end on its event subscription, the scenario waits
//! for Verbatim to be idle, which includes its replacement having started
//! and answered for the focus, and Verbatim, which already announced the
//! focus, has said nothing more; then the
//! review cursor's read-line command reads the text box's line through the
//! outpost Verbatim started in its place, a new process.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::protocol::Frame;
use verbatim_model::Pid;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The window's name in the run.
const NAME: &str = "outpost-crash";

/// The text box's one line.
const LINE: &str = "Read again by a new outpost.";

/// Waits until `events`, a subscription to Core's events, reports that the
/// outpost watching `target` ended.
fn wait_for_outpost_end(events: &mut verbatim_control::client::Client, target: u32) {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match events.next_frame() {
            Ok(Frame::OutpostEnded { target_pid, .. }) if target_pid == Pid(target) => return,
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
            "Core did not report the end of the outpost watching {target} within {EXIT_TIMEOUT:?}"
        );
    }
}

/// How long the ended outpost is given to exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    Ok(ScenarioState::TargetPid(super::open_text_box_app(
        scenario, NAME, LINE,
    )?))
}

pub(crate) fn body(scenario: &mut Scenario, state: &mut ScenarioState) {
    let ScenarioState::TargetPid(target) = *state else {
        panic!("setup records the application's process");
    };
    super::expect_text_box_app(scenario, NAME, LINE);
    let outpost = super::outpost_of(scenario, target);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    scenario
        .end_verbatim_process(outpost, EXIT_TIMEOUT)
        .expect("ends the application's outpost");
    wait_for_outpost_end(&mut events, target);
    scenario.expect_nothing_more();

    scenario
        .send_gesture("kb:numpad8")
        .expect("sends the read-line gesture");
    scenario.speech().expect(&[LINE]);
    let replacement = super::outpost_of(scenario, target);
    assert_ne!(
        replacement, outpost,
        "the application's outpost was replaced"
    );
}
