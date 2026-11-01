//! The debug-build check that no call into an application is made while
//! the calling thread holds one of the outpost's locks
//! (`docs/design/logic-tests.md`, "Failure paths and locks").
//!
//! Each thread keeps a record of eight slots, each empty or holding the
//! place a lock was taken. The outpost's lock wrapper fills the first empty
//! slot as it takes a lock ([`hold`]), and its guard empties that slot as
//! it is dropped, so the record is right whatever order guards drop in. The
//! places the outpost's code reaches an application call [`check`]: every
//! call the backends count, the making of an agile reference, the entry of
//! a bounded wait on another thread's call, and the release of the store's
//! removed objects. A check that finds a slot filled writes a message
//! naming what was made and every lock held to standard error and the log,
//! then panics with a payload of its own type ([`LockHeldAcrossCall`]); the
//! places that catch a panic around application calls abort the process in
//! debug builds when the payload is this one ([`abort_if_lock_check`]), so
//! no caught panic hides it.
//!
//! In release builds nothing is recorded and nothing is checked: [`hold`]
//! and [`check`] do nothing.

use std::any::Any;
use std::panic::Location;

/// How many locks one thread may hold at once; a ninth fails the check.
#[cfg(debug_assertions)]
const SLOTS: usize = 8;

#[cfg(debug_assertions)]
thread_local! {
    /// Where each lock the thread holds was taken, by slot.
    static HELD: std::cell::Cell<[Option<&'static Location<'static>>; SLOTS]> =
        const { std::cell::Cell::new([None; SLOTS]) };
}

/// The panic payload of a failed check, whose message has already been
/// written to standard error and the log.
#[derive(Debug)]
pub struct LockHeldAcrossCall {
    /// What was made, and where each lock held was taken.
    pub message: String,
}

/// One lock the thread holds, recorded in its slot until dropped.
#[must_use = "the slot is emptied when this is dropped"]
pub struct Held {
    #[cfg(debug_assertions)]
    slot: usize,
}

/// Records that the calling thread now holds a lock taken at `at`, in the
/// first empty slot. A ninth lock held at once fails as a check does.
pub fn hold(at: &'static Location<'static>) -> Held {
    #[cfg(debug_assertions)]
    {
        let slot = HELD
            .try_with(|held| {
                let mut slots = held.get();
                let slot = slots.iter().position(Option::is_none)?;
                slots[slot] = Some(at);
                held.set(slots);
                Some(slot)
            })
            .ok();
        match slot {
            Some(Some(slot)) => Held { slot },
            // The thread's locals are being destroyed: nothing is recorded.
            None => Held { slot: SLOTS },
            Some(None) => fail(format!(
                "a ninth lock taken at {} while the thread holds eight, taken at {}",
                place(at),
                places(&held_places())
            )),
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = at;
        Held {}
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        if self.slot < SLOTS {
            // A guard dropped while the thread's locals are being
            // destroyed does nothing.
            let _ = HELD.try_with(|held| {
                let mut slots = held.get();
                slots[self.slot] = None;
                held.set(slots);
            });
        }
    }
}

/// Where each lock the calling thread holds was taken.
#[cfg(debug_assertions)]
fn held_places() -> Vec<&'static Location<'static>> {
    HELD.try_with(|held| held.get().into_iter().flatten().collect())
        .unwrap_or_default()
}

/// `at` as `file:line`.
#[cfg(debug_assertions)]
fn place(at: &Location<'_>) -> String {
    format!("{}:{}", at.file(), at.line())
}

/// `places`, each as `file:line`, separated by commas.
#[cfg(debug_assertions)]
fn places(places: &[&'static Location<'static>]) -> String {
    places
        .iter()
        .map(|at| place(at))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Checks, in debug builds, that the calling thread holds none of the
/// outpost's locks as it makes `what`, a call into an application or a
/// wait on one; fails as the module says if it does.
#[track_caller]
pub fn check(what: &str) {
    #[cfg(debug_assertions)]
    {
        let held = held_places();
        if !held.is_empty() {
            fail(format!(
                "{what} at {} while the thread holds the outpost's locks taken at {}",
                place(Location::caller()),
                places(&held)
            ));
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = what;
}

/// Writes `message` to standard error and the log, then panics with it as
/// a [`LockHeldAcrossCall`], since the default panic hook prints a payload
/// of any type but a string as `Box<dyn Any>`.
#[cfg(debug_assertions)]
fn fail(message: String) -> ! {
    eprintln!("the lock check failed: {message}");
    tracing::error!("the lock check failed: {message}");
    std::panic::panic_any(LockHeldAcrossCall { message })
}

/// Ends the process, in debug builds, when `payload`, a panic caught around
/// application calls, is a failed check's, whose message is already in
/// standard error and the log: a program answered as panicked could still
/// pass a scenario, and only the process's end is seen by every test and
/// run.
pub fn abort_if_lock_check(payload: &(dyn Any + Send)) {
    #[cfg(debug_assertions)]
    if payload.is::<LockHeldAcrossCall>() {
        std::process::abort();
    }
    #[cfg(not(debug_assertions))]
    let _ = payload;
}
