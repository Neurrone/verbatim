//! `docs/uia-bugs.md`, "A timeout bounds each provider call, not an
//! operation" (E-TT in the experiments folder it shares with "A
//! registration that times out reports success"): a search made of
//! several provider calls waits a full transaction timeout for each call
//! a stalled provider does not answer, so it fails only after several
//! timeouts.
//!
//! mockapp's window thread is stalled until the test releases it, taking
//! no message at all (`stall-until-release quiet`), and the search's own
//! failure is the evidence the test waits for, so the test depends on no
//! count of the calls the search makes.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    IUIAutomation2, TreeScope_Descendants, UIA_E_TIMEOUT, UIA_NamePropertyId,
};
use windows::core::{AgileReference, BSTR, HRESULT, Interface};

use crate::{common, support};

/// The connection and transaction timeouts this test sets, short so that
/// the waits are too.
const TIMEOUT: Duration = Duration::from_millis(500);

/// `FindFirst` over the descendants of mockapp's window, its element
/// fetched before the stall, fails with `UIA_E_TIMEOUT` more than two
/// timeouts after it began, while the window thread is still stalled.
pub fn a_search_waits_a_timeout_for_each_call_its_stalled_provider_does_not_answer() {
    let (mut app, hwnd) = support::mockapp("search-timeout", "uia");
    let uia = support::client();
    let client: IUIAutomation2 = uia.client().cast().expect("an IUIAutomation2 client");
    let milliseconds = u32::try_from(TIMEOUT.as_millis()).expect("the timeout fits");
    // SAFETY: a live client; a plain integer.
    unsafe { client.SetConnectionTimeout(milliseconds) }.expect("the connection timeout set");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(milliseconds) }.expect("the transaction timeout set");
    let window =
        AgileReference::new(&support::element(uia.client(), hwnd)).expect("an agile reference");

    app.write("stall-until-release quiet");
    app.expect_line("stall started", String::new);
    let (done, searched) = mpsc::channel();
    std::thread::spawn(move || {
        let uia = support::client();
        let window = window
            .resolve()
            .expect("the window's element on this thread");
        // SAFETY: a local call on a live client.
        let condition = unsafe {
            uia.client()
                .CreatePropertyCondition(UIA_NamePropertyId, &VARIANT::from(BSTR::from("Second")))
        }
        .expect("a condition");
        let began = Instant::now();
        // SAFETY: a live element and condition; cross-process calls.
        let found = unsafe { window.FindFirst(TreeScope_Descendants, &condition) };
        let _ = done.send((
            found.map(drop).map_err(|error| error.code()),
            began.elapsed(),
        ));
    });
    let (found, took) = searched
        .recv_timeout(common::WAIT_TIMEOUT)
        .expect("the search ends while the window thread is stalled");
    assert_eq!(found, Err(HRESULT(UIA_E_TIMEOUT.cast_signed())));
    assert!(
        took > 2 * TIMEOUT,
        "the search failed {took:?} after it began"
    );
    app.release();
    app.quit();
}
