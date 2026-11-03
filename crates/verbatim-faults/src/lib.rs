//! Fault points: the hand-offs whose failure paths the tests run
//! (`docs/crates/verbatim-faults.md`, "Failure paths").
//!
//! A fault point is a call to [`hit`] placed immediately before the
//! operation it stands for, with a subject naming that call: the item
//! posted, the thread and generation started, the registration and element
//! or handler. When a test has armed the point with [`arm`] and its
//! selector picks the subject, `hit` answers true and the code takes the
//! path the real failure takes, with the error the real call returns, so
//! the test runs the production failure path rather than a copy of it.
//!
//! Points are compiled only under `cfg(any(test, feature = "faults"))` in
//! the crates that place them, so a crate's own unit tests reach its points
//! with no feature, and a release build or the end-to-end build has none.
//! The table of armed points is process-wide, under one lock: tests that
//! arm points run one at a time in their binary, or select subjects no
//! other test of the binary makes.

#![forbid(unsafe_code)]

use std::any::Any;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// Each fault point, and the subject it passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// A completion port refuses a queue's packet
    /// (`verbatim_process::port_queue::Poster::send`): the item is given
    /// back. Subject: the item.
    PortRefusesPost,
    /// The relay's port refuses its mailbox's wake
    /// (`verbatim-supervisor`'s `Mailbox::send`). Subject: the item sent.
    RelayWake,
    /// A port refuses one of the keyboard hook's posts (`verbatim-app`'s
    /// `key_reports`). Subject: the post.
    KeyPostRefused,
    /// A thread on a request's path cannot be started: the listener's
    /// reading thread, subject `(name, pid, generation)`, and the outpost's
    /// worker, subject `(name, generation)`.
    ThreadStart,
    /// `RemoveEventHandlerGroup` fails (`verbatim-uia`'s subscription).
    /// Subject: `(registration, runtime id)` of the element.
    RemoveGroup,
    /// `AddFocusChangedEventHandler` fails. Subject: `(registration,
    /// handler index)`.
    AddFocusHandler,
    /// `RemoveFocusChangedEventHandler` fails. Subject: `(registration,
    /// handler index)`.
    RemoveFocusHandler,
    /// An observation point, not a failure ([`reach`]): an outpost's
    /// shutdown begins to wait for the calls in progress
    /// (`verbatim-outpost`'s `Outpost::shutdown`). Subject: the outpost's
    /// target process id.
    ShutdownWaitsForCalls,
    /// An observation point ([`reach`]): one of `verbatim-uia`'s
    /// registration calls into an application ended by UIA's connection
    /// timeout, as its time past the timeout shows, since such a call
    /// reports success. Subject: the registration's id.
    RegistrationCallPassedTimeout,
    /// An observation point ([`reach`]): an outpost's caret key's watch
    /// ended without evidence (`verbatim-outpost`'s shell, carrying out the
    /// reducer's `WatchEnded`). Subject: why it ended, the reducer's
    /// `&'static str`.
    CaretWatchEnded,
}

/// Whether a subject is the one a test names: false for a subject of
/// another type.
type Selects = Box<dyn Fn(&dyn Any) -> bool + Send>;

/// A point armed by a test.
struct Armed {
    id: u64,
    fault: Fault,
    selects: Selects,
    /// How many more times it fires.
    left: usize,
    /// How many times it has fired.
    fired: usize,
}

/// The armed points.
struct Table {
    next: u64,
    armed: Vec<Armed>,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    next: 0,
    armed: Vec::new(),
});

/// Notified each time a point fires, for [`Guard::wait_fired`].
static FIRED: Condvar = Condvar::new();

fn table() -> MutexGuard<'static, Table> {
    TABLE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Arms `fault` to fire `times` times, each at a call whose subject, of
/// type `S`, `selects` picks. Dropping the guard disarms it, and asserts
/// that it fired exactly `times` times: a fault that never fired proves
/// nothing.
pub fn arm<S: Any>(
    fault: Fault,
    selects: impl Fn(&S) -> bool + Send + 'static,
    times: usize,
) -> Guard {
    let mut table = table();
    let id = table.next;
    table.next += 1;
    table.armed.push(Armed {
        id,
        fault,
        selects: Box::new(move |subject| subject.downcast_ref::<S>().is_some_and(&selects)),
        left: times,
        fired: 0,
    });
    Guard { id, fault, times }
}

/// Whether the fault point `fault` fires at this call, whose subject is
/// `subject`: true when a test armed it with a selector that picks the
/// subject, and it has firings left.
pub fn hit<S: Any>(fault: Fault, subject: &S) -> bool {
    let mut table = table();
    let Some(armed) = table
        .armed
        .iter_mut()
        .find(|armed| armed.fault == fault && armed.left > 0 && (armed.selects)(subject))
    else {
        return false;
    };
    armed.left -= 1;
    armed.fired += 1;
    FIRED.notify_all();
    true
}

/// An observation point: what a point calls when the code reaches the
/// place it stands for, where nothing fails. It counts as a firing of each
/// armed point whose selector picks the subject, even past the times it
/// was armed for, so a test waits with [`Guard::wait_fired`] for the
/// evidence that the code got there, and a guard's drop fails a test whose
/// code got there more often than it was armed for; the code goes on
/// either way.
pub fn reach<S: Any>(fault: Fault, subject: &S) {
    let mut table = table();
    let mut fired = false;
    for armed in table
        .armed
        .iter_mut()
        .filter(|armed| armed.fault == fault && (armed.selects)(subject))
    {
        armed.left = armed.left.saturating_sub(1);
        armed.fired += 1;
        fired = true;
    }
    if fired {
        FIRED.notify_all();
    }
}

/// [`hit`] with a subject built by `subject` only while `fault` is armed,
/// for a subject whose making is itself a call the build without fault
/// points does not make.
pub fn hit_with<S: Any>(fault: Fault, subject: impl FnOnce() -> S) -> bool {
    let armed = table()
        .armed
        .iter()
        .any(|armed| armed.fault == fault && armed.left > 0);
    armed && hit(fault, &subject())
}

/// An armed fault point ([`arm`]).
#[must_use = "dropping the guard disarms the point and checks it fired"]
pub struct Guard {
    id: u64,
    fault: Fault,
    times: usize,
}

impl Guard {
    /// How many times the point has fired so far.
    #[must_use]
    pub fn fired(&self) -> usize {
        table()
            .armed
            .iter()
            .find(|armed| armed.id == self.id)
            .map_or(0, |armed| armed.fired)
    }

    /// Waits up to `timeout` for the point to have fired every time it was
    /// armed to, the evidence that the failure it stands for has happened;
    /// returns whether it has.
    #[must_use]
    pub fn wait_fired(&self, timeout: Duration) -> bool {
        let table = table();
        let (table, _) = FIRED
            .wait_timeout_while(table, timeout, |table| {
                table
                    .armed
                    .iter()
                    .find(|armed| armed.id == self.id)
                    .is_some_and(|armed| armed.fired < self.times)
            })
            .unwrap_or_else(PoisonError::into_inner);
        table
            .armed
            .iter()
            .find(|armed| armed.id == self.id)
            .is_some_and(|armed| armed.fired == self.times)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let fired = {
            let mut table = table();
            let at = table.armed.iter().position(|armed| armed.id == self.id);
            at.map_or(0, |at| table.armed.swap_remove(at).fired)
        };
        if !std::thread::panicking() {
            assert_eq!(
                fired, self.times,
                "the fault point {:?} fired {fired} times, not {}",
                self.fault, self.times
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_fires_only_for_the_subject_selected_and_only_as_often_as_armed() {
        let guard = arm(Fault::RelayWake, |subject: &u32| *subject == 7, 1);
        assert!(!hit(Fault::RelayWake, &6_u32), "another subject");
        assert!(!hit(Fault::RelayWake, &7_i64), "a subject of another type");
        assert!(!hit(Fault::ThreadStart, &7_u32), "another point");
        assert!(hit(Fault::RelayWake, &7_u32));
        assert!(!hit(Fault::RelayWake, &7_u32), "it fired its one time");
        assert_eq!(guard.fired(), 1);
        drop(guard);
        assert!(!hit(Fault::RelayWake, &7_u32), "disarmed");
    }

    #[test]
    fn an_observation_point_reached_counts_as_a_firing() {
        let guard = arm(
            Fault::ShutdownWaitsForCalls,
            |subject: &u32| *subject == 3,
            1,
        );
        reach(Fault::ShutdownWaitsForCalls, &4_u32);
        assert_eq!(guard.fired(), 0, "another subject");
        reach(Fault::ShutdownWaitsForCalls, &3_u32);
        assert!(guard.wait_fired(Duration::ZERO));
    }

    #[test]
    #[should_panic(expected = "fired 2 times, not 1")]
    fn an_observation_point_reached_past_its_times_fails_the_test() {
        let guard = arm(Fault::CaretWatchEnded, |_: &&'static str| true, 1);
        reach(Fault::CaretWatchEnded, &"first");
        reach(Fault::CaretWatchEnded, &"second");
        assert_eq!(guard.fired(), 2);
    }

    #[test]
    #[should_panic(expected = "fired 0 times, not 1")]
    fn a_point_that_never_fired_fails_the_test() {
        let _guard = arm(Fault::KeyPostRefused, |subject: &u8| *subject == 1, 1);
    }
}
