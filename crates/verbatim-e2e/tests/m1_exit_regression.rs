//! The M1 exit criteria (`docs/roadmap.md`), scripted as the M2 regression
//! `docs/roadmap.md` promises: with Verbatim running, Verbatim+V opens the
//! menu, every menu item and every control in the Speech settings dialog
//! announces name, role, value, and state on focus, adjusting the rate
//! slider and the voice combo box speaks each new value, and
//! keypress-to-audio latency is traced end to end.
//!
//! Expected strings come from `crates/verbatim-i18n/i18n/en/verbatim.ftl`
//! and `crates/verbatim-gui/src/plan.rs`'s `accessible_name` (which strips
//! `&` mnemonic markers), not guesswork; every check is a substring match
//! rather than an exact string so wording the platform controls (list and
//! button chrome, MSAA's own phrasing) cannot make this test fragile.
//!
//! Deliberately uses the capture synthesizer, not `OneCore`: see
//! `Scenario::launch`'s doc comment for why. One consequence: the capture
//! synth's Speech page offers a voice choice and a rate slider but no
//! toggle, so this walk asserts value changes (both directions on the
//! slider, and the combo box changed and changed back) but no check-box
//! state change. The reducer's checked and not-checked announcements are
//! covered by `verbatim-core`'s unit tests and, cross-process, by
//! `mockapp`'s scripted state-change events; a state-change assertion
//! belongs here too the day a toggle appears on a synthesizer page this
//! suite can drive without installed `OneCore` voices.
//!
//! Every expectation below was confirmed live, running this suite against
//! the M2 Hyper-V guest (`cargo xtask vm test`). Two of them were wrong when
//! first inferred from source, and the live transcript corrected them: the
//! Verbatim+V menu opens with nothing selected, so the first Down arrow is
//! what selects "Settings...", and the rate slider's up arrow *decreases*
//! its value rather than increasing it. The Tab order through the Speech
//! dialog likewise does not follow the widget-creation order in
//! `crates/verbatim-gui/src/dialog.rs`: wx's traversal descends the
//! container panel's subtree (the category list, the Change button, then
//! the driver-generated controls) before returning to the dialog's own
//! direct children (OK, Cancel, Apply). The read-only synthesizer name
//! field takes no tab stop at all, so it is announced when the dialog opens
//! but never reached by Tab, and this walk does not assert on it.
//!
//! This suite launches a real Verbatim on a real desktop, and the outpost's
//! foreground tracking is desktop-wide (`docs/architecture.md` section 1's
//! `ForegroundTrigger`): unrelated foreground activity is visible to it and
//! can pollute a run with irrelevant speech, and latency on a working
//! machine is far less predictable than on an idle VM. Every step therefore
//! waits generously and exactly once, never retrying its input: a step that
//! was merely slow rather than lost still completes, so a retry would
//! overlap a second popup or keypress with the first and fight it for
//! keyboard focus. `expect_in_order`'s tolerance for unrelated intervening
//! utterances is what makes a single patient wait safe. This sensitivity is
//! itself an argument for decision D3's dedicated, otherwise-idle VM, not a
//! substitute for it.

use std::time::Duration;

use verbatim_e2e::Scenario;

/// How long each single-step announcement is given to arrive. Generous —
/// see this file's module doc for why steps are not retried, only waited
/// on patiently: a slow dev machine or CI runner should never make this
/// test flaky, and a real regression will hang until this fires either
/// way.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one scripted walk of the M1 exit criteria, deliberately linear so a failure's line number says exactly which step regressed"
)]
fn m1_exit_criteria_menu_and_speech_dialog() {
    let Some(_endpoint) = verbatim_e2e::endpoint() else {
        println!("VERBATIM_E2E_ENDPOINT is not set; skipping the live E2E suite");
        return;
    };

    let mut scenario = Scenario::launch().expect("launches Verbatim through the agent");

    // Verbatim+V opens the menu with nothing selected; Down arrow selects
    // the first item, Settings, announcing its name and role.
    scenario
        .send_gesture("kb:verbatim+v")
        .expect("sends the Verbatim+V gesture");
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Settings", "menu item"], STEP_TIMEOUT);

    // Walk to the second (and last) item.
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Exit", "menu item"], STEP_TIMEOUT);

    // Back to Settings, then open it.
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["Settings", "menu item"], STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("sends enter");

    // The settings dialog opens with focus in the category list, announced
    // by name and role.
    //
    // Note what is *not* asserted: the selected category ("Speech"). Focus
    // lands on the list itself, and Verbatim does not yet announce a
    // focused list's selected child the way NVDA does. An earlier version of
    // this test did assert it — and passed — but only because of a bug: UIA
    // events for non-window elements were bypassing per-window arbitration,
    // so a wx dialog that belongs to MSAA was also being announced by the
    // UIA stack, which reports focus at list-item granularity. Fixing that
    // (see `uia_passes_filter` in verbatim-outpost) removed the accidental
    // announcement and revealed the real gap. Announcing a list's selection
    // on focus is genuine screen-reader behavior and belongs with the
    // selection and object-navigation work in M3; this assertion tightens to
    // include it then.
    scenario
        .speech()
        .expect_in_order(&["Categories", "list"], STEP_TIMEOUT);

    // Tab walks the dialog in the live-confirmed order: Change... button,
    // then the capture synth's three driver-generated controls, then OK,
    // Cancel, Apply (see this file's module doc for why that order, not
    // creation order, is correct).
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Change", "button"], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    // Voice: a Choice descriptor, defaulting to the first option.
    scenario
        .speech()
        .expect_in_order(&["Voice", "combo box", "Capture A"], STEP_TIMEOUT);
    // Changing the combo box's selection while it is focused speaks the
    // new value; changing it back speaks the original, proving the
    // announcement tracks the selection rather than firing once.
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Capture B"], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["Capture A"], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    // Rate: a standard 0..=100 numeric descriptor; the capture synth
    // defaults it to 50.
    scenario
        .speech()
        .expect_in_order(&["Rate", "slider", "50"], STEP_TIMEOUT);
    // Adjusting the rate slider speaks the bare new value (the
    // slider-drag/arrow-key case documented on verbatim-core's `reduce`).
    // Confirmed live against the VM: on this wx slider the up arrow
    // *decreases* the value and the down arrow increases it, so up takes
    // 50 to 49 and two downs take it back through 50 to 51 — both
    // directions exercised, with the exact values the capture synth's
    // single-step numeric descriptor produces.
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario.speech().expect_in_order(&["49"], STEP_TIMEOUT);
    scenario
        .send_keys(&["downarrow", "downarrow"])
        .expect("sends downarrow twice");
    scenario.speech().expect_in_order(&["51"], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["OK", "button"], STEP_TIMEOUT);
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Cancel", "button"], STEP_TIMEOUT);
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Apply", "button"], STEP_TIMEOUT);

    // Close the dialog (Cancel path via the dialog's escape id) and report
    // the latency timelines this whole walk produced.
    scenario.send_keys(&["escape"]).expect("sends escape");

    let records = scenario
        .report_latency(200)
        .expect("fetches and prints latency records");
    assert!(
        !records.is_empty(),
        "expected at least one latency record from the gestures and keys sent during this test"
    );

    scenario
        .quit_verbatim()
        .expect("asks Verbatim to quit through the control plane");
}
