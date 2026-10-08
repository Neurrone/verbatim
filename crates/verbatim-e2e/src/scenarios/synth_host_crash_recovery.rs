//! A synthesizer host that dies is replaced (decision D18): speech goes on
//! after Verbatim's own `verbatim-synth-host.exe` is ended. Verbatim's menu
//! is opened and heard; Verbatim has exactly one synthesizer host, which is
//! ended by its process id, as a crash would end it; the next announcement
//! must still be heard in full, from the one host Verbatim started in its
//! place, a new process.

use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::{no_setup as setup, no_teardown as teardown};

/// The synthesizer host's executable.
const HOST: &str = "verbatim-synth-host.exe";

/// How long the ended host is given to exit.
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Verbatim's one synthesizer host's process id.
fn the_host(scenario: &mut Scenario) -> u32 {
    let hosts: Vec<u32> = scenario
        .verbatim_children()
        .expect("lists Verbatim's processes")
        .into_iter()
        .filter(|child| child.image.eq_ignore_ascii_case(HOST))
        .map(|child| child.pid)
        .collect();
    let [host] = hosts.as_slice() else {
        panic!("expected exactly one synthesizer host, found {hosts:?}");
    };
    *host
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    super::open_verbatim_menu(scenario);
    let host = the_host(scenario);
    scenario
        .end_verbatim_process(host, EXIT_TIMEOUT)
        .expect("ends the synthesizer host");

    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect(&["Settings... s"]);
    let replacement = the_host(scenario);
    assert_ne!(replacement, host, "the synthesizer host was replaced");

    scenario.send_keys(&["escape"]).expect("sends escape");
    super::expect_desktop(scenario);
}
