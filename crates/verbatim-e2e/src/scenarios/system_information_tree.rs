//! Object navigation through a real Win32 tree view (milestone M3):
//! msinfo32's `SysTreeView32` category tree, read over MSAA. Regression
//! coverage for the flat-exposure defect found in live testing: MSAA
//! presents every visible tree item as a flat sibling list under the tree
//! control, so before the `TVM_*`-based logical navigation in
//! `verbatim_ia2::acquire`, next-sibling moved into a node's own children,
//! previous-sibling moved to its logical parent, parent landed on the
//! unnamed tree control (spoken as just "unknown"), and first-child was
//! always a silent edge. Every assertion below fails against that behavior.
//!
//! The walk: focus starts on the "System Summary" root item; move to its
//! first child ("Hardware Resources"), to the next sibling ("Components"),
//! back to the previous sibling ("Hardware Resources"), up to the parent
//! ("System Summary"), up again onto the tree control itself (role "tree
//! view", never an item), and finally snap the navigator back to focus.
//! Substring matches, tolerant of state wording, like the other scenarios.
//! A tree item is spoken as NVDA speaks it on focus and object navigation,
//! by name without its "tree view item" role, so each step is anchored on
//! the item's name and level. As in NVDA, the level comes before the name
//! when it differs from the last level spoken that way, and after the rest
//! when it does not ("Where the level goes" in `docs/nvda/speech.md`). The
//! tree control itself is spoken by its bare role, never as an item.

use std::io;

use crate::registry::ScenarioState;
use crate::scenario::Scenario;

pub(crate) use super::no_teardown as teardown;

/// The title of msinfo32's window.
const TITLE: &str = "System Information";

pub(crate) fn setup(scenario: &mut Scenario) -> io::Result<ScenarioState> {
    scenario.launch_target("msinfo32.exe", &[], TITLE)?;
    Ok(ScenarioState::None)
}

/// Sends `gesture` and asserts exactly `heard`.
fn navigate(scenario: &mut Scenario, gesture: &str, heard: &str) {
    scenario.send_gesture(gesture).expect("sends the gesture");
    scenario.speech().expect(&[heard]);
}

pub(crate) fn body(scenario: &mut Scenario, _state: &mut ScenarioState) {
    // msinfo32 opens with its window, its tree, and focus on the tree's
    // "System Summary" root item.
    scenario.speech().expect(&[
        "System Information dialog",
        "tree view",
        "level 0 System Summary expanded 1 of 1",
    ]);
    // First child of the root, one level deeper, so the level comes first;
    // its next sibling, a real sibling, and back, at the same level, so
    // the level comes last; the logical parent item, a level up again; and
    // the tree control itself, the root item's parent.
    navigate(
        scenario,
        "kb:verbatim+numpad2",
        "level 1 Hardware Resources not selected collapsed 1 of 3",
    );
    navigate(
        scenario,
        "kb:verbatim+numpad6",
        "Components not selected collapsed 2 of 3 level 1",
    );
    navigate(
        scenario,
        "kb:verbatim+numpad4",
        "Hardware Resources not selected collapsed 1 of 3 level 1",
    );
    navigate(
        scenario,
        "kb:verbatim+numpad8",
        "level 0 System Summary expanded 1 of 1",
    );
    navigate(scenario, "kb:verbatim+numpad8", "tree view");
    // Back to the focus: the focused "System Summary" item.
    navigate(
        scenario,
        "kb:verbatim+numpadminus",
        "Move to focus System Summary expanded 1 of 1 level 0",
    );
}
