//! How a real outpost reports a UIA focus from mockapp's provider: under
//! which node, when the provider reuses a dead element's runtime id.
//!
//! The outpost runs in this process and reads the focused element from the
//! test (`common::outpost`), as `call_counts.rs` describes; mockapp's focus
//! moves with `set-focus`, which raises no event.

mod common;
#[path = "common/harness.rs"]
mod harness;

use verbatim_outpost::OutpostOptions;
use verbatim_uia::{ElementExt as _, Uia};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_HasKeyboardFocusPropertyId,
};

use common::outpost::OutpostUnderTest;

/// A UIA client on this thread over mockapp's window.
struct Client {
    uia: Uia,
    hwnd: HWND,
}

impl Client {
    fn new(hwnd: HWND) -> Self {
        Self {
            uia: Uia::new().expect("a UIA client"),
            hwnd,
        }
    }

    /// The element mockapp reports as having the keyboard focus, built with
    /// the listener's cache request, as a focus event's element arrives.
    fn focused(&self) -> IUIAutomationElement {
        let cache = self.uia.base_cache_request().expect("a cache request");
        let root = self
            .uia
            .element_from_handle(self.hwnd.0 as isize, &cache)
            .expect("mockapp's root element");
        let condition = self
            .uia
            .property_condition(UIA_HasKeyboardFocusPropertyId, &VARIANT::from(true))
            .expect("a condition");
        root.find_first_build_cache(TreeScope_Descendants, &condition, &cache)
            .expect("the search")
            .expect("mockapp reports a focused element")
    }
}

/// File Explorer, going back from a subfolder, gave the parent folder's
/// item the runtime id of the subfolder's item it had just destroyed. The
/// outpost reports the new focus under a new node, since the element it
/// held under that id is gone; a focus repeated on the new element keeps
/// its node, since that element still has the focus. Both ways the outpost
/// reads a focus: with remote operations, where the held element's focus
/// is read in the same round trip, and without.
///
/// The calls each focus makes are pinned. Remotely, the held element's focus
/// is read in the focus's one round trip, so a repeated focus costs the two
/// calls any focus does (the focused element and the program); a held
/// element that is gone fails the whole run before it starts, which is run
/// again without it, one call more. Classically, the held element's focus
/// is one call of its own.
fn reused_runtime_id(remote: bool) {
    let path = if remote { "remote" } else { "classic" };
    let (gone_cost, kept_cost) = if remote { (3, 2) } else { (5, 4) };
    let title = common::unique_title(&format!("mockapp-reuse-{path}"));
    let mut app = common::spawn("reuse.json", "uia", &title);
    let client = Client::new(common::find_window(&title));
    let outpost = OutpostUnderTest::with_options(
        app.pid(),
        OutpostOptions {
            remote_operations: remote,
        },
    );

    app.send("set-focus delta");
    let delta = client.focused();
    let reported = outpost.uia_focus(&delta);
    assert_eq!(reported.node.name.as_deref(), Some("delta.txt"), "{path}");
    let dead_node = reported.node.id;

    app.send("take-runtime-id inner delta");
    app.send("set-focus inner");
    let inner = client.focused();
    assert_eq!(
        verbatim_uia::runtime_id(&inner),
        verbatim_uia::runtime_id(&delta),
        "{path}: mockapp gave Inner the dead item's runtime id"
    );
    let reported = outpost.uia_focus(&inner);
    assert_eq!(reported.node.name.as_deref(), Some("Inner"), "{path}");
    let new_node = reported.node.id;
    assert_ne!(new_node, dead_node, "{path}: a new node for a new element");
    assert_eq!(
        reported.calls.uia, gone_cost,
        "{path}: the calls the focus made"
    );

    let reported = outpost.uia_focus(&inner);
    assert_eq!(
        reported.node.id, new_node,
        "{path}: the element that has the focus keeps its node"
    );
    assert_eq!(
        reported.calls.uia, kept_cost,
        "{path}: the calls the repeated focus made"
    );

    drop(outpost);
    app.quit();
}

fn a_reused_runtime_id_names_a_new_node_remote() {
    reused_runtime_id(true);
}

fn a_reused_runtime_id_names_a_new_node_classic() {
    reused_runtime_id(false);
}

fn main() {
    harness::run(&[
        (
            "a_reused_runtime_id_names_a_new_node_remote",
            a_reused_runtime_id_names_a_new_node_remote,
        ),
        (
            "a_reused_runtime_id_names_a_new_node_classic",
            a_reused_runtime_id_names_a_new_node_classic,
        ),
    ]);
}
