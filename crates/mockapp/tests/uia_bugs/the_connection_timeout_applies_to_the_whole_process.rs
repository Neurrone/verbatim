//! `docs/uia-bugs.md`, "The connection timeout applies to the whole
//! process" (E1 in its experiments folder): the connection and
//! transaction timeouts a client sets belong to its process, and the last
//! setting by any client is what every client reads; and Verbatim's
//! workaround, that `verbatim_uia` sets both once per process, to
//! `CALL_TIMEOUT`, during UIA's first-time setup, and nothing else sets
//! them.

use std::sync::mpsc;

use verbatim_uia::CALL_TIMEOUT;
use windows::Win32::UI::Accessibility::{IUIAutomation, IUIAutomation2};
use windows::core::Interface;

use crate::{common, support};

/// The connection and transaction timeouts `client` reads, in
/// milliseconds.
fn timeouts(client: &IUIAutomation) -> (u32, u32) {
    let client: IUIAutomation2 = client.cast().expect("an IUIAutomation2 client");
    // SAFETY: a local read on a live client.
    let connection = unsafe { client.ConnectionTimeout() }.expect("the connection timeout");
    // SAFETY: as above.
    let transaction = unsafe { client.TransactionTimeout() }.expect("the transaction timeout");
    (connection, transaction)
}

/// Sets both of the process's timeouts through `client`.
fn set_timeouts(client: &IUIAutomation, connection: u32, transaction: u32) {
    let client: IUIAutomation2 = client.cast().expect("an IUIAutomation2 client");
    // SAFETY: a live client; a plain integer.
    unsafe { client.SetConnectionTimeout(connection) }.expect("the connection timeout set");
    // SAFETY: as above.
    unsafe { client.SetTransactionTimeout(transaction) }.expect("the transaction timeout set");
}

/// Both timeouts set through a client on this thread are what a client on
/// another thread, made before, reads.
pub fn timeouts_set_through_one_client_are_read_by_another() {
    let (ready, made) = mpsc::channel();
    let (ask, asked) = mpsc::channel::<()>();
    let (answer, answered) = mpsc::channel();
    let other = std::thread::spawn(move || {
        let uia = support::client();
        ready.send(()).expect("the test is waiting");
        while asked.recv().is_ok() {
            answer
                .send(timeouts(uia.client()))
                .expect("the test is waiting");
        }
    });
    made.recv().expect("the other client is made");
    let uia = support::client();
    set_timeouts(uia.client(), 1_000, 1_500);
    ask.send(()).expect("the other client is waiting");
    assert_eq!(
        answered
            .recv_timeout(common::WAIT_TIMEOUT)
            .expect("the other client's reading"),
        (1_000, 1_500)
    );
    drop(ask);
    other.join().expect("the other client's thread");
}

/// The first client `verbatim_uia` makes in a process reads both timeouts
/// as `CALL_TIMEOUT`, which UIA's first-time setup set, in place of UIA's
/// defaults of 2 and 20 seconds.
pub fn the_first_client_in_a_process_has_verbatims_timeouts() {
    let uia = support::client();
    let call_timeout = u32::try_from(CALL_TIMEOUT.as_millis()).expect("the timeout fits");
    assert_eq!(timeouts(uia.client()), (call_timeout, call_timeout));
}

/// Timeouts a test sets through one client are kept while `verbatim_uia`
/// makes another client, on another thread, since making a client sets
/// nothing.
pub fn making_a_client_keeps_the_timeouts_another_set() {
    let uia = support::client();
    set_timeouts(uia.client(), 1_000, 1_500);
    let made = std::thread::spawn(|| timeouts(support::client().client()))
        .join()
        .expect("the other client's thread");
    assert_eq!(
        (made, timeouts(uia.client())),
        ((1_000, 1_500), (1_000, 1_500))
    );
}
