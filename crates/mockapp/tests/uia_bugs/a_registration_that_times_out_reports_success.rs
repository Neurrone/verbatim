//! `docs/uia-bugs.md`, "A registration that times out reports success"
//! (E-TT in its experiments folder): with short timeouts,
//! `AddEventHandlerGroup` on a window whose provider is held in its call
//! returns success while the provider still has not answered, where a
//! single read fails with `UIA_E_TIMEOUT`.
//!
//! mockapp holds its provider's next call until the test releases it
//! (`hold`), and says which call it holds: the evidence that the client's
//! call has reached the provider. The window's element is fetched before
//! the hold. Both of the process's timeouts are set short: the read is
//! bounded by the transaction timeout, and the registration, measured
//! while these tests were written, by the connection timeout.
//!
//! A registration that timed out delivers nothing for about 2 seconds
//! after it returned (e57), and no test shows it: nothing marks
//! the end of that gap, so a test could not tell an event lost in it from
//! one delivered late (`docs/uia-bugs.md` says why).

use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, IUIAutomation6, IUIAutomationElement, TreeScope_Subtree, UIA_E_TIMEOUT,
    UIA_MenuOpenedEventId,
};
use windows::core::{AgileReference, HRESULT, Interface};

use crate::common;
use crate::support::{self, Handler};

/// The connection and transaction timeouts these tests set, short so that
/// the waits are too.
const TIMEOUT: Duration = Duration::from_millis(500);

/// mockapp's window element, read before anything is held, on a client
/// whose process's timeouts are both [`TIMEOUT`].
fn window_with_short_timeouts(hwnd: HWND) -> AgileReference<IUIAutomationElement> {
    let uia = support::client();
    let client: IUIAutomation2 = uia.client().cast().expect("an IUIAutomation2 client");
    let milliseconds = u32::try_from(TIMEOUT.as_millis()).expect("the timeout fits");
    // SAFETY: a live client; a plain integer.
    unsafe { client.SetConnectionTimeout(milliseconds) }.expect("the connection timeout set");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(milliseconds) }.expect("the transaction timeout set");
    AgileReference::new(&support::element(uia.client(), hwnd)).expect("an agile reference")
}

/// Runs `call` with the window's element on a thread of its own, with a
/// client of its own, and returns how it ended once it has.
fn on_a_thread<T: Send + 'static>(
    window: AgileReference<IUIAutomationElement>,
    call: impl FnOnce(&verbatim_uia::Uia, &IUIAutomationElement) -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (done, ended) = mpsc::channel();
    std::thread::spawn(move || {
        let uia = support::client();
        let window = window
            .resolve()
            .expect("the window's element on this thread");
        let _ = done.send(call(&uia, &window));
    });
    ended
}

/// An event handler group registered on the window, its provider held in
/// `FragmentRoot`, returns success while the provider is still held.
pub fn a_group_registered_while_its_provider_is_held_reports_success() {
    let (mut app, hwnd) = support::mockapp("timeout-registration", "uia");
    let window = window_with_short_timeouts(hwnd);
    app.hold();
    let registered = on_a_thread(window, move |uia, window| {
        let client: IUIAutomation6 = uia.client().cast().expect("an IUIAutomation6 client");
        let cache = support::cache(uia.client());
        let handler = Handler::new();
        // SAFETY: a local call on a live client.
        let group = unsafe { client.CreateEventHandlerGroup() }.expect("an event handler group");
        // SAFETY: a live group, cache request and handler.
        unsafe {
            group.AddAutomationEventHandler(
                UIA_MenuOpenedEventId,
                TreeScope_Subtree,
                &cache,
                &handler.events,
            )
        }
        .expect("the event in the group");
        // SAFETY: a live client, element and group.
        unsafe { client.AddEventHandlerGroup(window, &group) }.map_err(|error| error.code())
    });
    assert_eq!(app.held(), "FragmentRoot");
    let registered = registered
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the registration returns while the provider is held");
    assert_eq!(registered, Ok(()));
    app.release();
    app.quit();
}

/// A read of the window's name, its provider held, fails with
/// `UIA_E_TIMEOUT` while the provider is still held.
pub fn a_read_while_its_provider_is_held_fails() {
    let (mut app, hwnd) = support::mockapp("timeout-read", "uia");
    let window = window_with_short_timeouts(hwnd);
    app.hold();
    let read = on_a_thread(window, |_, window| {
        // SAFETY: a live element; a cross-process read.
        unsafe { window.CurrentName() }
            .map(|name| name.to_string())
            .map_err(|error| error.code())
    });
    assert_eq!(app.held(), "GetPropertyValue");
    let read = read
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the read returns while the provider is held");
    assert_eq!(read, Err(HRESULT(UIA_E_TIMEOUT.cast_signed())));
    app.release();
    app.quit();
}
