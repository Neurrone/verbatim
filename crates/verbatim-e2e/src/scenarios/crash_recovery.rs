//! Each of Verbatim's own processes that dies is replaced, and Verbatim
//! goes on (decisions D9 and D18): its synthesizer host, an application's
//! outpost, and its focus listener, each ended by its process id, as a
//! crash would end it.
//!
//! The synthesizer host: Verbatim's menu is opened and heard; Verbatim has
//! exactly one synthesizer host, which is ended; the next announcement must
//! still be heard in full, from the one host Verbatim started in its place,
//! a new process.
//!
//! An outpost: `mockapp`, showing a fixed text box, comes to the
//! foreground and is read by its outpost; that outpost is ended. Once Core
//! reports the outpost's end on its event subscription, the scenario waits
//! for Verbatim to be idle, which includes its replacement having started
//! and answered for the focus, and Verbatim, which already announced the
//! focus, has said nothing more; then the review cursor's read-line command
//! reads the text box's line through the outpost Verbatim started in its
//! place, a new process.
//!
//! The focus listener: it is ended, and once its replacement has joined
//! Verbatim's job, the scenario waits for Verbatim to be idle, which asks
//! the replacement behind its start, and Verbatim has said nothing more;
//! then a second `mockapp` text box opens and is heard. Returning to an application that has an outpost would not
//! show the replacement at work: that outpost's own hook takes in its
//! application's focus and foreground events, and reads the focus itself
//! when no listener sends a read, so the desktop, for one, is announced
//! with no listener. An application with no outpost is seen only through
//! the listener's facts, which start its outpost, so the new
//! application's announcement can have come only from the replacement,
//! the one listener running once the first has exited.
//!
//! The scenario is in the shell group (`Group::Shell`), which covers the
//! synthesizer host's replacement with the others.

use std::io;
use std::time::{Duration, Instant};

use verbatim_control::protocol::Frame;
use verbatim_model::Pid;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// The synthesizer host's executable.
const HOST: &str = "verbatim-synth-host.exe";

/// The focus listener's executable.
const LISTENER: &str = "verbatim-listener.exe";

/// The text box window's name in the run.
const NAME: &str = "outpost-crash";

/// The text box's one line.
const LINE: &str = "Read again by a new outpost.";

/// The second text box window's name in the run, opened once the focus
/// listener was replaced.
const LISTENER_NAME: &str = "listener-crash";

/// The second text box's one line.
const LISTENER_LINE: &str = "Seen by a new focus listener.";

/// How long an ended process is given to exit, and its replacement to
/// start.
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Verbatim's one process of `image`'s process id.
fn the_one(scenario: &mut Scenario, image: &str) -> u32 {
    let found: Vec<u32> = scenario
        .verbatim_children()
        .expect("lists Verbatim's processes")
        .into_iter()
        .filter(|child| child.image.eq_ignore_ascii_case(image))
        .map(|child| child.pid)
        .collect();
    let [one] = found.as_slice() else {
        panic!("expected exactly one {image}, found {found:?}");
    };
    *one
}

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

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // The synthesizer host.
    super::open_verbatim_menu(scenario);
    let host = the_one(scenario, HOST);
    scenario
        .end_verbatim_process(host, EXIT_TIMEOUT)
        .expect("ends the synthesizer host");

    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&["Settings... s"]);
    let replacement = the_one(scenario, HOST);
    assert_ne!(replacement, host, "the synthesizer host was replaced");

    scenario.send_keys(&["escape"]).expect("sends escape");
    super::expect_desktop(scenario);

    // An application's outpost.
    let target =
        super::open_text_box_app(scenario, NAME, LINE).expect("opens the text box application");
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

    // The focus listener.
    let listener = the_one(scenario, LISTENER);
    scenario
        .end_verbatim_process(listener, EXIT_TIMEOUT)
        .expect("ends the focus listener");
    scenario
        .wait_for_verbatim_process(LISTENER, &[listener], EXIT_TIMEOUT)
        .expect("a new focus listener joins Verbatim's job");
    scenario.expect_nothing_more();
    super::open_text_box_app(scenario, LISTENER_NAME, LISTENER_LINE)
        .expect("opens a second text box application");
    super::expect_text_box_app(scenario, LISTENER_NAME, LISTENER_LINE);
}
