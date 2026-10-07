//! Rapid focus churn (outpost redesign, step 7): many focus changes in
//! quick succession inside one application must leave Verbatim's focus and
//! navigator on the control that really has focus, with the outpost still
//! answering. Against Verbatim's own settings dialog, which every run has.
//!
//! The churn is a single burst of real keystrokes: Tab six times, then
//! Shift+Tab six times, sent in one request so they arrive faster than the
//! outpost reads each focus. Tab order is a fixed cycle, so the burst ends
//! where it started, on the selected category item. The outpost's limiter
//! keeps only the newest focus events of each batch, and the newest is the
//! focus the dialog started with, so nothing is said for the burst.
//!
//! The evidence the burst was processed: Core has handled the burst's last
//! key, and the dialog's focus has come back to the category item, as the
//! focus event Core receives says. Once Verbatim is idle, nothing was said;
//! then the report of the current navigator object must be exactly the
//! category item, which proves the final focus won, and its answer proves
//! the outpost still answers queries.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::Frame;
use verbatim_model::NormalizedEvent;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// How long the dialog's focus is given to come back.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(15);

/// The category item the burst starts and ends on, by name.
const CATEGORY: &str = "Speech";

/// Waits on `events` until Core receives a focus on the node named `name`.
fn wait_for_focus_on(events: &mut ControlClient, name: &str) {
    let deadline = Instant::now() + FOCUS_TIMEOUT;
    loop {
        match events.next_frame() {
            Ok(Frame::Event {
                event: NormalizedEvent::FocusChanged { node, .. },
                ..
            }) if node.name.as_deref() == Some(name) => return,
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
            "Core received no focus on {name:?} within {FOCUS_TIMEOUT:?} after the burst"
        );
    }
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_speech_settings(scenario);
    let mut events = scenario
        .subscribe_events()
        .expect("subscribes to Verbatim's events");
    let mut burst = vec!["tab"; 6];
    burst.extend(["shift+tab"; 6]);
    scenario.send_keys(&burst).expect("sends the burst");
    wait_for_focus_on(&mut events, CATEGORY);
    scenario.expect_nothing_more();

    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report-current-object");
    scenario
        .speech()
        .expect(&["Speech list item focused selected 1 of 3"]);
    super::close_settings_to_desktop(scenario);
}
