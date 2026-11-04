//! `books/dev/src/reference/uia-bugs.md`, "A hung window named in a system focus event holds a
//! process's UIA events": the case under "Not affected" from E-delivery,
//! that a server-side provider held while raising its own event holds only
//! its own events. No test reproduces the hold itself; the entry says why.

use crate::support::{self, Handler};

/// A server-side provider held while raising its own focus event holds
/// only its own events (E-delivery, in "Not affected"): with mockapp A's
/// provider held in the call UIA makes into it as A raises its focus,
/// B's focus event reaches this process, and A's arrives once A is
/// released. A's event cannot arrive while A is held, so nothing on A's
/// handler then is proved by the hold itself. Each application then raises
/// a marker, A's once it is released, which must be the next event on its
/// own handler, so an extra event of either, which would come before its
/// application's marker, fails the test, rather than a check that nothing
/// more came, which could run before an event arrived.
pub fn a_provider_held_raising_its_event_holds_only_its_own_events() {
    let (mut a, a_hwnd) = support::mockapp("held-raise-a", "uia");
    let (mut b, b_hwnd) = support::mockapp("held-raise-b", "uia");
    let uia = support::client();
    let on_a = Handler::new();
    let on_b = Handler::new();
    let _a_group = support::group_on(
        uia.client(),
        &support::element(uia.client(), a_hwnd),
        &on_a,
        false,
    );
    let _b_group = support::group_on(
        uia.client(),
        &support::element(uia.client(), b_hwnd),
        &on_b,
        false,
    );

    a.hold();
    a.write("focus btn1");
    assert_eq!(a.held(), "ProviderOptions");
    b.send("focus btn1");
    assert_eq!(on_b.next(), support::from(&b, "Original Name"));
    on_a.heard_nothing_more();
    a.release();
    a.expect_line("applied", String::new);
    assert_eq!(on_a.next(), support::from(&a, "Original Name"));
    a.send("focus item1");
    assert_eq!(on_a.next(), support::from(&a, "First"));
    b.send("focus item1");
    assert_eq!(on_b.next(), support::from(&b, "First"));
    a.quit();
    b.quit();
}
