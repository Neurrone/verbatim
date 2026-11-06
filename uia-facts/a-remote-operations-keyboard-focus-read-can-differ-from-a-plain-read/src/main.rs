//! A remote operation's keyboard focus read can differ from a plain read
//! (`uia-facts/a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read/readme.md`).
//!
//! Each read is of `HasKeyboardFocus`, plainly (`CurrentHasKeyboardFocus`)
//! and inside a remote operation.
//!
//! - `provider`: the provider program's hidden window 0, its tree's focus
//!   on node 3; node 3, node 4 and the window's own element read both
//!   ways. The provider answers for itself; the case shows whether the two
//!   reads differ on a provider that does.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, runs};
use uia_facts_support::{rop, uia};
use windows::Win32::UI::Accessibility::{IUIAutomationElement, UIA_HasKeyboardFocusPropertyId};

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report =
        Report::start("a-remote-operations-keyboard-focus-read-can-differ-from-a-plain-read");
    if args.wants("provider") {
        report.case("provider", "moving the hidden provider window's tree focus to node 3; reading node 3's, node 4's and the window's keyboard focus plainly and inside a remote operation");
        provider(&mut report);
    }
    report.finish();
}

/// `HasKeyboardFocus` read plainly, as `true` or `false`, or the error.
fn plain(element: &IUIAutomationElement) -> String {
    // SAFETY: a live element.
    match unsafe { element.CurrentHasKeyboardFocus() } {
        Ok(value) => value.as_bool().to_string(),
        Err(error) => uia::hresult(error.code()),
    }
}

/// `HasKeyboardFocus` read inside a remote operation, as `true` or
/// `false`, or the error.
fn remote(element: &IUIAutomationElement) -> String {
    match rop::property(element, UIA_HasKeyboardFocusPropertyId.0, false) {
        Ok(value) => value.to_lowercase(),
        Err(error) => uia::hresult(error.code()),
    }
}

/// Both reads of `element`, as `plain/remote`.
fn both(element: &IUIAutomationElement) -> String {
    format!("{}/{}", plain(element), remote(element))
}

fn provider(report: &mut Report) {
    let mut answers = Vec::new();
    for _ in 0..trials() {
        let mut provider = Provider::launch(&["--windows", "uia:0"]);
        provider.request("focus 0 3");
        let client = uia::client();
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let three = uia::node(&client, &root, 3).expect("node 3");
        let four = uia::node(&client, &root, 4).expect("node 4");
        answers.push(format!(
            "node 3 {}, node 4 {}, the window {}",
            both(&three),
            both(&four),
            both(&root)
        ));
    }
    report.claim(
        "the provider program's reads, plain/remote operation",
        &runs(&answers),
        "",
        Verdict::New,
    );
}
