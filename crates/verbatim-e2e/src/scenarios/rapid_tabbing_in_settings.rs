//! Rapid focus churn (outpost redesign, step 7): many focus changes in
//! quick succession inside one application must leave Verbatim's focus and
//! navigator on the control that really has focus, with the outpost still
//! answering. Against Verbatim's own settings dialog, which every run has.
//!
//! The churn is a single burst of real keystrokes: Tab six times, then
//! Shift+Tab six times, sent in one request so they arrive faster than the
//! outpost reads each focus. Tab order is a fixed cycle, so the burst ends
//! where it started, on the selected category item.
//!
//! This is the one scenario whose speech is deliberately not all asserted
//! by its text. Which controls the burst passes through get read, and so
//! announced, depends on how the dialog's focus changes and the outpost's
//! reads interleave, as it would in NVDA, and the scenario's intent is the
//! end state, not the path. The evidence the burst is over is Core
//! receiving the focus on the category item again, and then Verbatim
//! having handled the burst's last key and being idle. Everything said
//! until then is then known: when the outpost read the whole burst as one
//! focus, the one the dialog started with, nothing; otherwise every
//! utterance before the final announcement must end cut off, none heard in
//! full, whatever its text, and the final announcement, the category list
//! entered and the category item, is asserted exactly, queued once and
//! heard in full. The report of the current navigator object must then be
//! exactly the category item, which proves the final focus won, and its
//! answer proves the outpost still answers queries.

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

/// The category item the burst starts and ends on, by name.
const CATEGORY: &str = "Speech";

/// The final announcement: the category list entered, then its selected
/// item, where the burst ends.
const FINAL: [&str; 2] = ["Categories: list Alt+c", "Speech 1 of 3"];

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
    let said = scenario.take_until_idle();
    // When every focus the burst passed through was read as one, the focus
    // is the one the dialog started with, and nothing is said.
    if let Some((last, before)) = said.split_last() {
        let Some((entered, passed)) = before.split_last() else {
            panic!(
                "the burst's final announcement lacks {:?}: {said:?}",
                FINAL[0]
            );
        };
        assert_eq!(
            [entered.text.as_str(), last.text.as_str()],
            FINAL,
            "the burst's final announcement, after {passed:?}"
        );
        for heard in passed {
            scenario.speech().expect_ended(heard, Ending::Cancelled);
        }
        scenario.speech().expect_ended(entered, Ending::Completed);
        scenario.speech().expect_ended(last, Ending::Completed);
    }

    scenario
        .send_gesture("kb:verbatim+numpad5")
        .expect("sends report-current-object");
    scenario
        .speech()
        .expect(&["Speech list item focused selected 1 of 3"]);
    super::close_settings_to_desktop(scenario);
}
