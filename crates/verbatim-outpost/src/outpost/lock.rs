//! The outpost's one kind of lock: every lock `verbatim-outpost` declares
//! is a [`Lock`], which records, in debug builds, that the thread holds it,
//! so a call into an application made while it is held fails the check of
//! `verbatim_win32::held_locks` (`books/dev/src/reference/crates/verbatim-win32.md`, `held_locks`). The crate's `clippy.toml` disallows
//! `std::sync::Mutex`, so no lock escapes the check. In release builds a
//! `Lock` is the mutex and its guard, with nothing recorded.

use std::ops::{Deref, DerefMut};
#[expect(
    clippy::disallowed_types,
    reason = "the one place the outpost's mutex is declared"
)]
use std::sync::{Mutex, MutexGuard, PoisonError};

use verbatim_win32::held_locks::{self, Held};

/// A mutex whose holding is recorded in debug builds. A poisoned lock is
/// taken as it is, as every lock of the outpost was.
#[derive(Default)]
pub(crate) struct Lock<T> {
    #[expect(
        clippy::disallowed_types,
        reason = "the one place the outpost's mutex is declared"
    )]
    inner: Mutex<T>,
}

/// A held [`Lock`]: dropping it lets the lock go and, in debug builds,
/// empties the slot it filled.
pub(crate) struct LockGuard<'a, T> {
    #[expect(
        clippy::disallowed_types,
        reason = "the one place the outpost's mutex is declared"
    )]
    guard: MutexGuard<'a, T>,
    _held: Held,
}

impl<T> Lock<T> {
    /// A lock holding `value`.
    #[expect(
        clippy::disallowed_types,
        reason = "the one place the outpost's mutex is declared"
    )]
    pub(crate) const fn new(value: T) -> Self {
        Self {
            inner: Mutex::new(value),
        }
    }

    /// Takes the lock, recording in debug builds that the thread holds it,
    /// taken where this was called.
    #[track_caller]
    pub(crate) fn lock(&self) -> LockGuard<'_, T> {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        LockGuard {
            guard,
            _held: held_locks::hold(std::panic::Location::caller()),
        }
    }
}

impl<T> Deref for LockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for LockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::Instant;

    use super::*;

    /// The payload's message, when it is a failed check's.
    fn check_message(payload: &(dyn std::any::Any + Send)) -> Option<String> {
        payload
            .downcast_ref::<held_locks::LockHeldAcrossCall>()
            .map(|failed| failed.message.clone())
    }

    /// What the check says of a counted call made at line `call` of this
    /// file while the locks taken at lines `held` are held.
    fn counted_under(call: u32, held: &[u32]) -> String {
        let held: Vec<String> = held
            .iter()
            .map(|line| format!("{}:{line}", file!()))
            .collect();
        format!(
            "a counted call into an application at {}:{call} while the thread holds the outpost's locks taken at {}",
            file!(),
            held.join(", ")
        )
    }

    /// A counted call made under one of the outpost's locks panics with the
    /// check's own payload, naming the call's place and the lock's.
    #[test]
    fn a_call_counted_under_an_outpost_lock_panics() {
        let lock = Lock::new(0_u32);
        let taken_at = line!() + 1;
        let guard = lock.lock();
        let called_at = line!() + 2;
        let payload = std::panic::catch_unwind(|| {
            verbatim_uia::calls::count(verbatim_model::CallKind::Uia);
        })
        .expect_err("the counted call panicked");
        drop(guard);
        assert_eq!(
            check_message(&*payload),
            Some(counted_under(called_at, &[taken_at]))
        );
        let _ = verbatim_uia::calls::take();
    }

    /// Guards dropped out of order: taking A and B and dropping A leaves B
    /// alone recorded, which the check names.
    #[test]
    fn a_lock_dropped_out_of_order_leaves_the_other_held() {
        let (a, b) = (Lock::new(()), Lock::new(()));
        let held_a = a.lock();
        let b_at = line!() + 1;
        let held_b = b.lock();
        drop(held_a);
        let called_at = line!() + 2;
        let payload = std::panic::catch_unwind(|| {
            verbatim_uia::calls::count(verbatim_model::CallKind::Uia);
        })
        .expect_err("the counted call panicked");
        drop(held_b);
        assert_eq!(
            check_message(&*payload),
            Some(counted_under(called_at, &[b_at]))
        );
        let _ = verbatim_uia::calls::take();
    }

    /// The child's side of
    /// [`a_call_under_a_lock_inside_a_worker_program_aborts_the_process`],
    /// through the worker's catching wrapper:
    /// run only as that test's child, which its environment names, so a
    /// run of every ignored test passes over it.
    #[test]
    #[ignore = "run only as the child of the abort test"]
    fn lock_check_child() {
        if std::env::var_os(CHILD).is_none() {
            return;
        }
        let _ = super::super::worker::catching(child_program);
    }

    /// The lines of [`child_program`]'s lock and call, which the parent
    /// expects the check's message to name.
    const CHILD_LOCK_LINE: u32 = line!() + 5;
    const CHILD_CALL_LINE: u32 = line!() + 5;
    /// A program that makes a counted call under a lock.
    fn child_program() {
        let lock = Lock::new(());
        let _guard = lock.lock();
        verbatim_uia::calls::count(verbatim_model::CallKind::Uia);
    }

    /// The environment variable that makes [`lock_check_child`] run.
    const CHILD: &str = "VERBATIM_LOCK_CHECK_CHILD";

    /// The status of a process ended by `std::process::abort`, a fast fail
    /// on Windows (`STATUS_STACK_BUFFER_OVERRUN`).
    const ABORTED: i32 = 0xC000_0409_u32.cast_signed();

    /// A call under a lock inside a worker program, whose panic the worker
    /// catches, ends the process: its own binary run as a child runs the
    /// program through the worker's catching wrapper, and ends with the
    /// abort's status, its standard error naming the lock's place.
    #[test]
    fn a_call_under_a_lock_inside_a_worker_program_aborts_the_process() {
        let output = Command::new(std::env::current_exe().expect("this test binary"))
            .args([
                "--exact",
                "outpost::lock::tests::lock_check_child",
                "--ignored",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .expect("the child runs");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(ABORTED), "{stderr}");
        let message = format!(
            "the lock check failed: {}",
            counted_under(CHILD_CALL_LINE, &[CHILD_LOCK_LINE])
        );
        assert_eq!(
            stderr
                .lines()
                .filter(|line| line.starts_with("the lock check failed: "))
                .collect::<Vec<_>>(),
            [message.as_str()],
            "{stderr}"
        );
    }

    /// The check's cost in this build: a million acquisitions of a `Lock`,
    /// which records each in debug builds, against a million of the plain
    /// mutex, which records none.
    #[test]
    #[ignore = "a measurement, run by hand"]
    #[expect(
        clippy::disallowed_types,
        clippy::cast_precision_loss,
        reason = "the plain mutex is the measurement's baseline"
    )]
    fn measure_lock_check() {
        const ACQUISITIONS: u32 = 1_000_000;
        let recorded = Lock::new(0_u64);
        let plain = Mutex::new(0_u64);
        let started = Instant::now();
        for _ in 0..ACQUISITIONS {
            *recorded.lock() += 1;
        }
        let with_record = started.elapsed();
        let started = Instant::now();
        for _ in 0..ACQUISITIONS {
            *plain.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        }
        let without = started.elapsed();
        let per =
            |elapsed: std::time::Duration| elapsed.as_nanos() as f64 / f64::from(ACQUISITIONS);
        println!(
            "debug build: {:.1} ns per acquisition with the record, {:.1} ns without, {:.1} ns added",
            per(with_record),
            per(without),
            per(with_record) - per(without)
        );
    }
}
