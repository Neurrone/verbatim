//! Rapid focus churn (outpost redesign, step 7): many focus changes in
//! quick succession inside one application must leave Verbatim's focus and
//! navigator on the control that really has focus, with the outpost still
//! answering. Against Verbatim's own settings dialog, which every run has.
//!
//! The churn is a single burst of real keystrokes: Shift+Tab six times,
//! then Tab seven times, sent in one request so they arrive faster than
//! the outpost reads each focus. The dialog's Tab order is a fixed cycle of
//! eleven stops, so the burst goes back from the selected category item
//! through the six stops before it and forward again through them and the
//! item, ending one stop on, on the Change button, which it reaches only
//! with its last key. The burst's final focus is always a control other
//! than the one the dialog started with, so it is always announced.
//!
//! This is the one scenario whose speech is deliberately not all asserted
//! by its text. Which controls the burst passes through get read, and so
//! announced, depends on how the dialog's focus changes and the outpost's
//! reads interleave, as it would in NVDA, and the scenario's intent is the
//! end state, not the path. The evidence the burst is over is Core
//! receiving the focus on the Change button, and then Verbatim having
//! handled the burst's last key and being idle. Every utterance until then
//! but the last two must end cut off, none heard in full, whatever its
//! text; the last two are the Change button's announcement, exactly, heard
//! in full: the group box it sits in, "Synthesizer grouping", entered from
//! the category list, then the button.
//! The report of the current navigator object must then be exactly the
//! Change button, which proves the final focus won, and its answer proves
//! the outpost still answers queries.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::Frame;
use verbatim_model::NormalizedEvent;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::Ending;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// How long the dialog's focus is given to come back.
const FOCUS_TIMEOUT: Duration = Duration::from_secs(15);

/// The control the burst ends on, by name.
const LAST_CONTROL: &str = "Change...";

/// The burst's final announcement: the group box the control it ends on
/// sits in, entered from the category list, and the control.
const FINAL: [&str; 2] = ["Synthesizer grouping", "Change... button Alt+h"];

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
    let mut burst = vec!["shift+tab"; 6];
    burst.extend(["tab"; 7]);
    scenario.send_keys(&burst).expect("sends the burst");
    wait_for_focus_on(&mut events, LAST_CONTROL);
    let said = scenario.take_until_idle();
    assert!(said.len() >= FINAL.len(), "the burst said only {said:?}");
    let (passed, last) = said.split_at(said.len() - FINAL.len());
    let last_texts: Vec<&str> = last.iter().map(|heard| heard.text.as_str()).collect();
    assert_eq!(
        last_texts, FINAL,
        "the burst's final announcement, after {passed:?}"
    );
    for heard in passed {
        scenario.speech().expect_ended(heard, Ending::Cancelled);
    }
    for heard in last {
        scenario.speech().expect_ended(heard, Ending::Completed);
    }

    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report-current-object");
    scenario
        .speech()
        .expect(&["Change... button focused Alt+h"]);
    super::close_settings_to_desktop(scenario);
}
