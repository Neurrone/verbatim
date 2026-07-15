//! Multi-outpost regression (decision D9): a real switch between two
//! applications' outposts, each staying alive while the other holds
//! foreground, exercising the whole foreground-announcement flow — window
//! then focused control, generation-guarded retries, hidden-frame
//! suppression, and re-announcing an existing outpost rather than
//! respawning it — end to end against real Windows 11 Notepad and
//! Verbatim's own GUI.
//!
//! Windows 11 Notepad focuses its edit control almost instantly on launch,
//! and its top-level window's own name never arrives as a focus event (only
//! as a name-changed property on a node nothing ever focuses) — so "Notepad"
//! is heard at all only because the newly authoritative outpost announces
//! the top-level window deliberately, before the focused control. That is
//! exactly what this test proves, twice: once for the freshly spawned
//! outpost, and again after Verbatim's own outpost took foreground and gave
//! it back, proving Notepad's outpost was kept alive and re-announced
//! rather than respawned.
//!
//! Same discipline as the other live tests in this crate: single generous
//! waits, substring matchers, tolerant of unrelated intervening utterances,
//! never re-sending input, and relying on `Scenario`'s `Drop` guard to clean
//! up every launched process even on panic.
//!
//! This test launches Notepad exactly once and never relies on Windows 11
//! Notepad's single-instance handoff itself — the foreground switch below
//! reuses that one instance rather than launching a second. It does,
//! though, depend on `kill_target`'s image-name sweep at the end: confirmed
//! live, `launch_target`'s own returned pid for Notepad reliably exits on
//! its own within a few seconds of launch (the handoff to a differently
//! pid'd process happens even for a single, solo launch — see
//! `Scenario::launch_target`'s doc comment), so a pid-only kill there would
//! silently do nothing and leave the real window as a stray.

use std::time::Duration;

use verbatim_e2e::Scenario;

/// How long each step's announcement is given to arrive. Generous for the
/// same reasons `m1_exit_regression`'s `STEP_TIMEOUT` is: this suite runs on
/// a real desktop where unrelated activity and process-spawn latency are
/// both real, and a genuine regression should hang until this fires rather
/// than flake on a slow run.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

#[test]
fn switching_between_notepad_and_verbatim_reannounces_both() {
    let Some(_endpoint) = verbatim_e2e::endpoint() else {
        println!("VERBATIM_E2E_ENDPOINT is not set; skipping the live E2E suite");
        return;
    };

    let mut scenario = Scenario::launch().expect("launches Verbatim through the agent");

    // Launching Notepad brings it to the foreground; a fresh outpost spawns
    // to watch it, and the newly authoritative outpost announces the
    // top-level window ("Notepad", from the window-level synthetic
    // FocusChanged) followed by the focused control (Notepad's edit area,
    // role "edit" — see crates/verbatim-i18n/i18n/en/verbatim.ftl's
    // role-editable-text).
    let notepad_pid = scenario
        .launch_target("notepad.exe", &[])
        .expect("launches notepad through the agent");
    scenario
        .speech()
        .expect_in_order(&["Notepad", "edit"], STEP_TIMEOUT);

    // Verbatim+V brings Verbatim's own hidden frame and popup menu to
    // foreground. The hidden frame itself must never be announced (decision
    // D9's hidden-frame suppression); the menu opens with nothing selected,
    // so the first Down arrow is what produces a real, ordinary WinEvent
    // focus announcement for "Settings..." — proving Verbatim's own outpost
    // works correctly while Notepad's outpost is still alive in the
    // background (multi-outpost coexistence, not a retarget of one shared
    // outpost).
    scenario
        .send_gesture("kb:verbatim+v")
        .expect("sends the Verbatim+V gesture");
    scenario.send_keys(&["downarrow"]).expect("sends downarrow");
    scenario
        .speech()
        .expect_in_order(&["Settings", "menu item"], STEP_TIMEOUT);

    // Escape closes the menu without selecting anything; verbatim-gui's
    // postPopup hides the hidden frame again and the foreground falls back
    // to whatever was previously foreground — Notepad, per pre_popup's and
    // post_popup's own doc comments.
    scenario.send_keys(&["escape"]).expect("sends escape");

    // Notepad regains foreground. Its outpost was never retired (it lost
    // foreground for only a few keystrokes' worth of time, nowhere near the
    // idle-retirement threshold), so this exercises the "send the existing
    // outpost an AnnounceFocus" path, not a respawn: the same window and
    // control announcement sequence as the first launch, heard again.
    scenario
        .speech()
        .expect_in_order(&["Notepad", "edit"], STEP_TIMEOUT);

    scenario
        .kill_target(notepad_pid)
        .expect("kills notepad through the agent");

    scenario
        .quit_verbatim()
        .expect("asks Verbatim to quit through the control plane");
}
