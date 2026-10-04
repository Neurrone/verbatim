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
//! Every run speaks through eSpeak NG, the default synthesizer, whether
//! audible or silent (a silent run plays through the silent real-time
//! device), so the Speech page always has the same controls: voice,
//! variant, rate, pitch, inflection, and volume. The voice combo box asserts
//! eSpeak NG's default voice and the one listed after it, both fixed by its
//! data; the variant asserts its default, Max. eSpeak NG's page has no
//! toggle, so this walk asserts value changes (both directions on the
//! slider, and the combo box changed and changed back) but no check-box
//! state change. The reducer's checked and not-checked announcements are
//! covered by `verbatim-core`'s unit tests and, cross-process, by
//! `mockapp`'s scripted state-change events. After the rate slider the walk
//! tabs forward one control at a time, bounded, until OK is reached (see
//! the code comment there), without asserting the other sliders' values.
//!
//! On audio: every speech assertion waits for its utterance to be heard in
//! full (decision D17), so the final `report_latency` asserts that the
//! walk's timelines reached audio, audible or not.
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
//! This scenario launches a real Verbatim on a real desktop, and the
//! focus listener's foreground tracking is desktop-wide
//! (`docs/architecture.md` section 1's focus listener, decision D13):
//! unrelated foreground activity is visible to it and can pollute a run with
//! irrelevant speech, and latency on a working machine is far less
//! predictable than on an idle VM. Every step therefore waits generously
//! and exactly once, never retrying its input: a step that was merely slow
//! rather than lost still completes, so a retry would overlap a second
//! popup or keypress with the first and fight it for keyboard focus.
//! `expect_in_order`'s tolerance for unrelated intervening utterances is
//! what makes a single patient wait safe. This sensitivity is itself an
//! argument for decision D3's dedicated, otherwise-idle VM, not a
//! substitute for it.
//!
//! Ported into the scenario registry (milestone M3 Track B) from what used
//! to be `crates/verbatim-e2e/tests/menu_and_settings_dialog.rs`'s whole test
//! body; that file is now the thin `#[test]` wrapper calling
//! [`crate::registry::run_named`]. The final `quit_verbatim` call this
//! scenario used to make itself is now [`crate::registry::run`]'s own,
//! generic, always-asserted final step for every scenario — everything
//! else, including the `report_latency` call and its own assertion, stays
//! here unchanged, since keypress-to-audio latency is itself part of the M1
//! exit criteria this scenario scripts, not a generic harness nicety.

use std::io;
use std::time::Duration;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

/// How long each single-step announcement is given to arrive. Generous —
/// see this module's doc for why steps are not retried, only waited on
/// patiently: a slow dev machine or CI runner should never make this
/// scenario flaky, and a real regression will hang until this fires either
/// way.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// How many extra Tab stops past the rate slider this walk tolerates while
/// looking for OK, bounded so a real regression (OK never reached) still
/// fails loudly instead of tabbing indefinitely — see the code comment
/// where this is used.
const MAX_EXTRA_SYNTH_CONTROLS: u32 = 5;

/// The voice combo box's default and next-listed voice: eSpeak NG's English
/// (Great Britain), and English (Scotland) after it in eSpeak NG's own
/// order.
fn expected_voices() -> (&'static str, &'static str) {
    ("English (Great Britain)", "English (Scotland)")
}

/// The last run of ASCII digits in `text`, parsed as an integer: the value a
/// numeric slider announces at the end of its focus announcement ("Rate
/// slider 80") or as the bare "80" after an adjustment. `None` when there is
/// no digit run at all, so the caller can fail with the offending utterance.
fn trailing_number(text: &str) -> Option<i64> {
    text.split(|c: char| !c.is_ascii_digit())
        .rfind(|run| !run.is_empty())
        .and_then(|run| run.parse().ok())
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "must match ScenarioDef::setup's fn-pointer signature, shared with scenarios whose setup can fail"
)]
pub(crate) fn setup(_scenario: &mut Scenario) -> io::Result<ScenarioState> {
    // Nothing beyond Scenario::launch itself: this scenario exercises only
    // Verbatim's own menu and Speech settings dialog, never a second target
    // application.
    Ok(ScenarioState::None)
}

pub(crate) fn teardown(_scenario: &mut Scenario, _state: ScenarioState) {
    // Nothing to restore.
}

#[allow(
    clippy::too_many_lines,
    reason = "one scripted walk of the M1 exit criteria, deliberately linear so a failure's line number says exactly which step regressed"
)]
pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // Verbatim+V opens the menu with nothing selected (the shared helper
    // waits for the popup's announcement — see `scenarios::open_verbatim_menu`
    // for the cold-guest race that wait closes); Down arrow then selects
    // the first item, Settings, announcing its name and role.
    super::open_verbatim_menu(scenario, STEP_TIMEOUT);
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Settings..."], STEP_TIMEOUT);

    // Walk to the second (and last) item.
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario.speech().expect_in_order(&["Exit"], STEP_TIMEOUT);

    // Back to Settings, then open it.
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&["Settings..."], STEP_TIMEOUT);
    scenario.send_keys(&["enter"]).expect("sends enter");

    // The settings dialog opens with focus in the category list, announced
    // by name and role — and, since M3's selection work, followed by the
    // selected category ("Speech"): the outpost enriches the focus event
    // with the list's selected child (MSAA `accSelection` on this wx list
    // box) and the reducer speaks it right after the list. This assertion
    // was deliberately loose before M3 (an earlier accidental version of
    // the announcement came from a cross-backend arbitration bug, fixed at
    // the end of M2, and the real behavior did not exist yet); it is the
    // tightened form the original comment promised.
    scenario
        .speech()
        .expect_in_order(&["Categories", "list", "Speech"], STEP_TIMEOUT);

    // Tab walks the dialog in the live-confirmed order: Change... button,
    // then eSpeak NG's six driver-generated controls, then OK,
    // Cancel, Apply (see this module's doc for why that order, not creation
    // order, is correct).
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Change", "button"], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    // Voice: a Choice descriptor. Fixed expected names per expected_voices()
    // rather than a value captured at runtime — see this module's doc for
    // why both modes have a known, literal pair. This first utterance
    // carries the full focus announcement (role and state included, e.g.
    // "Voice combo box English (Great Britain) collapsed") —
    // confirmed live against the VM.
    let (default_voice, other_voice) = expected_voices();
    scenario
        .speech()
        .expect_in_order(&["Voice", "combo box", default_voice], STEP_TIMEOUT);
    // Changing the combo box's selection while it is focused speaks the
    // new value; changing it back speaks the original, proving the
    // announcement tracks the selection rather than firing once. Confirmed
    // live: unlike the initial focus announcement, a value-change speaks
    // only the bare name with no role or state chrome (e.g. bare "English
    // (Scotland)").
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&[other_voice], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&[default_voice], STEP_TIMEOUT);

    // eSpeak NG's variant, Max by default.
    scenario.send_keys(&["tab"]).expect("sends tab");
    scenario
        .speech()
        .expect_in_order(&["Variant", "combo box", "Max"], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    // Rate: a standard 0..=100 numeric descriptor. Capture its starting
    // value from the focus announcement ("Rate slider <n>") rather than
    // pinning a literal, so this walk holds for whatever rate the deployed
    // configuration selected — `Settings::for_e2e` sets it uniformly
    // (`verbatim_config::E2E_RATE`) whichever synthesizer it selects,
    // and this scenario must not have to change when that value does.
    let rate_focus = scenario
        .speech()
        .expect_in_order_capturing(&["Rate", "slider"], STEP_TIMEOUT);
    let rate = trailing_number(&rate_focus)
        .unwrap_or_else(|| panic!("rate slider announced no numeric value: {rate_focus:?}"));
    // Adjusting the rate slider speaks the bare new value (the
    // slider-drag/arrow-key case documented on verbatim-core's `reduce`).
    // Confirmed live against the VM: on this wx slider the up arrow
    // *decreases* the value and the down arrow increases it, so up takes the
    // captured <n> to <n>-1 and two downs take it back through <n> to <n>+1
    // — both directions exercised, asserted relative to the captured start
    // rather than a fixed value.
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&[&(rate - 1).to_string()], STEP_TIMEOUT);
    scenario
        .send_keys(&["downarrow", "downarrow"])
        .expect("sends downarrow twice");
    let rate_plus_one = (rate + 1).to_string();
    scenario
        .speech()
        .expect_in_order(&[&rate_plus_one], STEP_TIMEOUT);

    // eSpeak NG's page has three more sliders between the rate slider and
    // OK (pitch, inflection, volume); rather than assume a fixed count, tab
    // forward one control at a time until OK is reached, bounded generously
    // so a real regression still fails loudly instead of spinning. Each
    // step's utterance is captured the same
    // duplicate-tolerant way the voice combo box needed: a value's own last
    // announcement (starting with the rate slider's captured <n>+1) can
    // re-announce once more before the next control's real focus change
    // lands, so each capture waits for whatever differs from the previous
    // one rather than assuming the very next utterance is already the new
    // control. This walk does not assert on any of these extra controls'
    // own values.
    let mut last_seen = rate_plus_one;
    let mut extra_controls = 0u32;
    loop {
        scenario.send_keys(&["tab"]).expect("sends tab");
        last_seen = scenario
            .speech()
            .expect_change_capturing(&last_seen, STEP_TIMEOUT);
        if last_seen.contains("OK") && last_seen.contains("button") {
            break;
        }
        extra_controls += 1;
        assert!(
            extra_controls <= MAX_EXTRA_SYNTH_CONTROLS,
            "tabbed past {MAX_EXTRA_SYNTH_CONTROLS} controls after the rate slider without reaching OK; last seen {last_seen:?}"
        );
    }
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
        "expected at least one latency record from the gestures and keys sent during this scenario"
    );
}
