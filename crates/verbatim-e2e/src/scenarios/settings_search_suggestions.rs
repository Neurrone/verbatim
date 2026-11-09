//! Selection in a list the focus controls: the Settings app's search
//! suggestions, read as the selection moves through them while the
//! focus stays in the search box, as NVDA reads them
//! (`docs/parity/changes-and-selection.md`, "Selection in a list the
//! focus controls").
//!
//! Read-only, so it changes no setting. `ms-settings:system` opens the
//! System page with the keyboard focus in its search box, as its opening
//! speech asserts. "sound" is typed in one injection, all five characters
//! before Settings' search delay, so the suggestions open once, after the
//! last; Settings selects the first itself and announces that suggestions
//! are available. Down Arrow twice and Up Arrow then move the selection,
//! the focus staying in the box. How many suggestions there are, and
//! what they are called, comes from Settings' search index, which differs
//! between editions and builds, so the agent reads the list's size and its
//! first three names through UI Automation, independently of Verbatim,
//! once the list is filled and before the scenario fixes what it expects.
//! The evidence that it is filled is Verbatim's utterance for the first
//! suggestion: Settings fills the list all at once, from no children to
//! all of them, so once Verbatim has said the first suggestion's name and
//! position the list holds every item. The scenario waits for that
//! utterance to be queued without matching it, has the agent read the
//! list, and only then asserts the utterance exactly, built from the
//! agent's read; Verbatim's words fix only when the agent reads, never
//! what is expected. The agent watches for no event in Settings, since
//! Settings raises its structure events only while a client listens for
//! them, and a listener of the harness's would change what Settings does
//! in the step asserted.
//! The size is the set size the first item reports (`SizeOfSet`), the
//! property Verbatim's "of N" comes from, not the count of the list's
//! realized children, which a virtualized list would make smaller.
//!
//! Local-only: GitHub's runner's Settings raises no selection event for a
//! first suggestion as the list opens, so "Suggestions available" plays to
//! completion there, while this machine's Settings selects the first
//! suggestion as the list opens and the scenario expects it to cut the
//! sound off.
//!
//! In some launches Settings replaces the list's items as it opens, when
//! the first suggestion is the list's current selection rather than the
//! item its event named (`docs/parity/changes-and-selection.md`,
//! "Selection in a list the focus controls"). The order of the first
//! suggestion before the suggestions-opened sound holds by a margin of about 50 ms: the
//! selection must reach the outpost before Settings' second change of the
//! search box's relation.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;
use crate::speech::{cut_off, heard};

pub(crate) use super::no_teardown as teardown;

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.open_settings_page("ms-settings:system")?;
    Ok(ScenarioState::None)
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    scenario.speech().expect(&[
        "Settings",
        "Settings window",
        "Search box, Find a setting edit",
        "blank",
    ]);
    // Each character's echo is cut off by the next key, the last heard in
    // full; then the application's notification that suggestions are
    // available, cut off by its selection of the first one.
    scenario.type_text("sound").expect("types the query");
    // NVDA says the first suggestion as "list item", the name its event's
    // cache holds, which Settings has not set yet; Verbatim reads its name.
    scenario.deviates_from_nvda(
        "docs/parity/changes-and-selection.md",
        "Selection in a list the focus controls",
    );
    scenario.speech().expect_sequence(&[
        cut_off("s"),
        cut_off("o"),
        cut_off("u"),
        cut_off("n"),
        heard("d"),
        cut_off("Suggestions available"),
    ]);
    // Verbatim's utterance for the first suggestion shows that Settings
    // has filled the list; it is asserted below, from the agent's read.
    scenario.speech().await_next_queued();
    let (count, names) = scenario
        .controlled_items(3)
        .expect("the agent reads the suggestions");
    let [first, second, third] = names.as_slice() else {
        panic!("the agent read {names:?}, not three suggestions");
    };
    let said = |name: &str, position: u32| format!("{name} {position} of {count}");
    // The search box's suggestions relation plays the suggestions-opened
    // indication after the first suggestion is said.
    scenario
        .speech()
        .expect_sequence(&[heard(&said(first, 1)), heard("sound: suggestions-opened")]);

    // Down Arrow, which moves no caret in the one-line box, moves the
    // selection; NVDA queues the box's line for the key and cuts it off
    // with the suggestion, which Verbatim says alone.
    for (key, said) in [
        ("downarrow", said(second, 2)),
        ("downarrow", said(third, 3)),
        ("uparrow", said(second, 2)),
    ] {
        scenario.send_keys(&[key]).expect("sends the key");
        scenario.deviates_from_nvda("docs/parity/text.md", "TextInfo-equivalent layer");
        scenario.speech().expect(&[&said]);
    }
}
