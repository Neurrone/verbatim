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
//! Deliberately defaults to the capture synthesizer in runner-direct mode
//! (see `Scenario::launch`'s doc comment for why), but `cargo xtask vm
//! test` deploys and runs the real `OneCore` synthesizer by default now —
//! see `Scenario::launch`'s `AUDIBLE_ENV` doc comment. One consequence of
//! the capture synth specifically: its Speech page offers a voice choice
//! and a rate slider but no toggle, so this walk asserts value changes
//! (both directions on the slider, and the combo box changed and changed
//! back) but no check-box state change. The reducer's checked and
//! not-checked announcements are covered by `verbatim-core`'s unit tests
//! and, cross-process, by `mockapp`'s scripted state-change events; a
//! state-change assertion belongs here too the day a toggle appears on a
//! synthesizer page this suite can drive without installed `OneCore`
//! voices.
//!
//! The voice combo box asserts a fixed pair of expected names per mode
//! (see `expected_voices` below) rather than capturing whatever the active
//! synthesizer happens to announce first: the capture synth always offers
//! the same two fixed names ("Capture A", "Capture B"), and the VM's golden
//! image always installs the same `OneCore` voice set in the same order
//! ("Microsoft David" default, "Microsoft Zira" listed next), confirmed
//! live, so both modes can assert literal names instead. One structural
//! difference between the two synths' descriptor sets still shows up in the
//! Tab order, confirmed live: `OneCore`'s Speech page has more controls
//! between the rate slider and OK than the capture synth's two-descriptor
//! page does — at least a "Rate boost" toggle and a pitch slider, and
//! possibly more depending on the guest's installed voices — so this walk's
//! Tab step after the rate slider tabs forward one control at a time,
//! bounded, until OK is reached (see the code comment there), capturing
//! each control's own announced value there instead of asserting a literal
//! one, since those extra controls' values are not fixed the way the voice
//! names are. It does not assert on any of those extra controls' own
//! values either way, for the same reason the capture synth's Speech page
//! cannot expose a toggle to assert on: see the previous paragraph.
//!
//! What does *not* yet pass under an audible run: the final
//! `report_latency` call (see its own code comment below), which requires
//! at least one traced utterance to have actually reached audio.
//! Confirmed live, with a connected `cargo xtask vm connect` session and
//! settle pauses up to six seconds tried, that every timeline in an
//! audible run is still reported interrupted before audio — a real gap
//! somewhere in the real `OneCore`/`WasapiSink` audio path this suite
//! cannot diagnose further from the E2E side, tracked separately from the
//! speech-assertion work this scenario's own history is about.
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
//! outpost's foreground tracking is desktop-wide
//! (`docs/architecture.md` section 1's `ForegroundTrigger`): unrelated
//! foreground activity is visible to it and can pollute a run with
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
//! to be `crates/verbatim-e2e/tests/m1_exit_regression.rs`'s whole test
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
use crate::scenario::is_audible;

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

/// The voice combo box's default and second-listed voice name, as a fixed
/// pair per mode — see this module's doc for why both modes have a known,
/// literal pair rather than a value captured at runtime. `cargo xtask vm
/// test` (audible by default now) always deploys `OneCore` with "Microsoft
/// David" as the default voice and "Microsoft Zira" listed next (confirmed
/// live); a non-audible run (runner-direct with `VERBATIM_E2E_AUDIBLE`
/// unset) uses the capture synth's two fixed names instead.
fn expected_voices() -> (&'static str, &'static str) {
    if is_audible() {
        ("Microsoft David", "Microsoft Zira")
    } else {
        ("Capture A", "Capture B")
    }
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
    // this scenario did assert it — and passed — but only because of a bug:
    // UIA events for non-window elements were bypassing per-window
    // arbitration, so a wx dialog that belongs to MSAA was also being
    // announced by the UIA stack, which reports focus at list-item
    // granularity. Fixing that (see `uia_passes_filter` in
    // verbatim-outpost) removed the accidental announcement and revealed
    // the real gap. Announcing a list's selection on focus is genuine
    // screen-reader behavior and belongs with the selection and
    // object-navigation work in M3; this assertion tightens to include it
    // then.
    scenario
        .speech()
        .expect_in_order(&["Categories", "list"], STEP_TIMEOUT);

    // Tab walks the dialog in the live-confirmed order: Change... button,
    // then the capture synth's three driver-generated controls, then OK,
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
    // "Voice combo box Capture A collapsed" against the capture synth) —
    // confirmed live against the VM.
    let (default_voice, other_voice) = expected_voices();
    scenario
        .speech()
        .expect_in_order(&["Voice", "combo box", default_voice], STEP_TIMEOUT);
    // Changing the combo box's selection while it is focused speaks the
    // new value; changing it back speaks the original, proving the
    // announcement tracks the selection rather than firing once. Confirmed
    // live: unlike the initial focus announcement, a value-change speaks
    // only the bare name with no role or state chrome (e.g. bare "Capture
    // B").
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&[other_voice], STEP_TIMEOUT);
    scenario.send_keys(&["uparrow"]).expect("sends uparrow");
    scenario
        .speech()
        .expect_in_order(&[default_voice], STEP_TIMEOUT);

    scenario.send_keys(&["tab"]).expect("sends tab");
    // Rate: a standard 0..=100 numeric descriptor. Capture its starting
    // value from the focus announcement ("Rate slider <n>") rather than
    // pinning a literal, so this walk holds for whatever rate the deployed
    // configuration selected — `Settings::for_e2e` sets it uniformly
    // (`verbatim_config::E2E_RATE`) for both the capture synth and OneCore,
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

    // OneCore's Speech page has more controls between the rate slider and
    // OK than the capture synth does — confirmed live: at least a "Rate
    // boost" toggle and a pitch slider, neither of which the capture synth
    // exposes at all (this module's doc). Rather than assume a fixed
    // count, tab forward one control at a time until OK is reached, bounded
    // generously so a real regression still fails loudly instead of
    // spinning. Each step's utterance is captured the same
    // duplicate-tolerant way the voice combo box needed: a value's own last
    // announcement (starting with the rate slider's captured <n>+1) can
    // re-announce once more before the next control's real focus change
    // lands, so each capture waits for whatever differs from the previous
    // one rather than assuming the very next utterance is already the new
    // control. This walk does not assert on any of these extra controls'
    // own values, for the same reason the capture synth's page cannot: see
    // the module doc.
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

    // `report_latency` requires that at least one queued utterance actually
    // reached audio (`crate::latency::report`'s doc comment). Every step
    // above waits only for an utterance to be *queued*, not for it to
    // finish playing, so under the capture synth's near-instant `NullSink`
    // the very next utterance almost always still reaches audio before
    // being interrupted; this settle pause gives the last, otherwise
    // uninterrupted utterance room to finish starting on a slower path.
    //
    // Confirmed live under audible mode, though: even a six-second pause
    // here does not make this assertion pass — every one of the run's
    // recorded timelines is still reported "interrupted before audio", with
    // or without a connected Enhanced Session. That rules out plain timing
    // and points at something deeper than this scenario's pace, somewhere
    // in the real `OneCore`/`WasapiSink` audio path (outside this crate).
    // This pause stays as a genuine, cheap improvement for borderline
    // cases; the underlying audible-mode gap is unresolved and tracked
    // separately, not fixed by this scenario.
    std::thread::sleep(Duration::from_millis(1500));

    let records = scenario
        .report_latency(200)
        .expect("fetches and prints latency records");
    assert!(
        !records.is_empty(),
        "expected at least one latency record from the gestures and keys sent during this scenario"
    );
}
