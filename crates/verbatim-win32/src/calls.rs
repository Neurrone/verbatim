//! The call gate (`books/dev/src/reference/crates/verbatim-win32.md`,
//! `calls`): what every call the backends count consults first, on a
//! thread that installed one, in release builds as in debug ones. An
//! outpost's worker installs a gate bound to its charge, so once its
//! generation is abandoned each counted call fails with
//! [`CallError::Abandoned`] and makes no call. A thread with no gate makes
//! every call.
//!
//! The gate's failure is its own error, never a fact about the application:
//! no caller reads it as "gone", "import failed" or "no provider". It stops
//! counted calls only; the requests one UIA method makes inside itself, a
//! COM release, and an agile reference's resolve as a program unwinds are
//! not stopped.
//!
//! Beside the gate is the one per-thread count of the cross-process calls
//! the backends make (`books/dev/src/reference/performance.md`, "What
//! counts as a call"), each counted by [`count`] as the gate lets it
//! through. `verbatim-uia` counts a UIA client method that the
//! application's provider answers and the raw probe's window message;
//! `verbatim-ia2` an `IAccessible` method, `IAccIdentity`'s identity
//! string, the acquisitions, a `QueryInterface` for any interface but
//! `IUnknown` on another process's object, and a list view's or tree
//! view's window message (each module's documentation lists its calls).
//! One API call counts once, however many round trips it makes inside it.
//! The count is per thread, since each outpost has one worker thread making
//! all of its calls: the worker takes it around each entry it handles
//! ([`take`]) and sends it with the entry's event or reply. Other threads
//! count too and never take, which costs nothing.

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use windows::Win32::Foundation::E_ABORT;

use verbatim_model::{CallCounts, CallKind};

/// What a gate is bound to: an outpost worker's charge.
pub trait Charge: Send + Sync {
    /// Whether the worker's generation was abandoned.
    fn is_dead(&self) -> bool;
}

thread_local! {
    /// The gate this thread installed, if any.
    static GATE: RefCell<Option<Arc<dyn Charge>>> = const { RefCell::new(None) };
}

/// Why a counted call was not made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallError {
    /// The worker making it was abandoned.
    Abandoned,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Abandoned => f.write_str("the worker was abandoned; no call is made"),
        }
    }
}

impl std::error::Error for CallError {}

/// The HRESULT a call the gate refused fails with, `E_ABORT`, which no
/// caller reads as a fact about the application.
pub const ABANDONED: windows::core::HRESULT = E_ABORT;

impl From<CallError> for windows::core::Error {
    fn from(error: CallError) -> Self {
        Self::new(ABANDONED, error.to_string())
    }
}

/// Whether `error` is a call the gate refused.
#[must_use]
pub fn is_abandoned(error: &windows::core::Error) -> bool {
    error.code() == ABANDONED
}

/// Installs `charge`'s gate on this thread until [`uninstall`]: an
/// outpost's worker, one per generation.
pub fn install(charge: Arc<dyn Charge>) {
    GATE.with_borrow_mut(|gate| *gate = Some(charge));
}

/// Removes this thread's gate, dropping what it is bound to now rather
/// than in the thread's thread-local destructors: a worker uninstalls it as
/// its thread ends. Every call made after it passes, as on a thread with no
/// gate.
pub fn uninstall() {
    GATE.with_borrow_mut(Option::take);
}

/// Consults this thread's gate before a counted call: refused once its
/// worker was abandoned.
///
/// # Errors
///
/// [`CallError::Abandoned`] once the worker was abandoned.
pub fn check() -> Result<(), CallError> {
    GATE.with_borrow(|gate| match gate {
        Some(charge) if charge.is_dead() => Err(CallError::Abandoned),
        Some(_) | None => Ok(()),
    })
}

thread_local! {
    /// This thread's calls since the last [`take`].
    static COUNTS: Cell<CallCounts> = const {
        Cell::new(CallCounts {
            uia: 0,
            msaa: 0,
            window_messages: 0,
            connections: 0,
        })
    };
}

/// Counts one cross-process call of `kind` on this thread, once this
/// thread's gate lets it be made ([`check`]).
///
/// # Errors
///
/// [`CallError::Abandoned`] once the outpost worker making it was
/// abandoned: the call is not made, nor counted.
#[track_caller]
pub fn count(kind: CallKind) -> Result<(), CallError> {
    // No counted call is made under one of the outpost's locks, checked in
    // debug builds.
    crate::held_locks::check("a counted call into an application");
    check()?;
    COUNTS.with(|counts| {
        let mut current = counts.get();
        current.record(kind);
        counts.set(current);
    });
    Ok(())
}

/// Counts, on this thread, that the call just counted under its kind
/// connected to a window by its handle ([`CallCounts::connections`]).
pub fn count_connection() {
    COUNTS.with(|counts| {
        let mut current = counts.get();
        current.record_connection();
        counts.set(current);
    });
}

/// This thread's calls since the last take, resetting them to zero.
#[must_use]
pub fn take() -> CallCounts {
    COUNTS.with(Cell::take)
}

/// This thread's calls since the last take, leaving them counted: for
/// logging the calls one step of an entry made, by the difference between
/// two looks.
#[must_use]
pub fn peek() -> CallCounts {
    COUNTS.with(Cell::get)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    #[derive(Default)]
    struct Recorded {
        dead: AtomicBool,
    }

    impl Charge for Recorded {
        fn is_dead(&self) -> bool {
            self.dead.load(Ordering::Relaxed)
        }
    }

    /// A thread's gate refuses every counted call once its worker is
    /// abandoned, with its own error; another thread's calls pass it by.
    #[test]
    fn a_gate_refuses_every_call_once_its_worker_is_abandoned() {
        std::thread::spawn(|| {
            let charge = Arc::new(Recorded::default());
            install(Arc::clone(&charge) as Arc<dyn Charge>);
            assert_eq!(check(), Ok(()));
            charge.dead.store(true, Ordering::Relaxed);
            assert_eq!(check(), Err(CallError::Abandoned));
            assert!(is_abandoned(&CallError::Abandoned.into()));
            std::thread::spawn(|| assert_eq!(check(), Ok(())))
                .join()
                .expect("a thread with no gate");
        })
        .join()
        .expect("the gated thread");
    }

    /// Uninstalling a thread's gate drops its hold on the charge at once,
    /// and the thread's calls then pass as on a thread with none.
    #[test]
    fn uninstalling_a_gate_lets_go_of_its_charge() {
        std::thread::spawn(|| {
            let charge = Arc::new(Recorded::default());
            charge.dead.store(true, Ordering::Relaxed);
            install(Arc::clone(&charge) as Arc<dyn Charge>);
            assert_eq!(Arc::strong_count(&charge), 2);
            uninstall();
            assert_eq!(Arc::strong_count(&charge), 1);
            assert_eq!(check(), Ok(()));
        })
        .join()
        .expect("the gated thread");
    }

    /// A thread's calls are counted by kind, taken once, and its own.
    #[test]
    fn take_returns_and_resets_this_threads_counts() {
        let _ = take();
        count(CallKind::Uia).expect("no gate on this thread");
        count(CallKind::Msaa).expect("no gate on this thread");
        count(CallKind::WindowMessage).expect("no gate on this thread");
        count_connection();
        assert_eq!(
            take(),
            CallCounts {
                uia: 1,
                msaa: 1,
                window_messages: 1,
                connections: 1
            }
        );
        assert!(take().is_empty());
        count(CallKind::Msaa).expect("no gate on this thread");
        assert_eq!(peek().msaa, 1);
        assert_eq!(take().msaa, 1);
        // Another thread's calls are its own.
        std::thread::spawn(|| count(CallKind::Uia).expect("no gate on this thread"))
            .join()
            .expect("the thread ran");
        assert!(take().is_empty());
    }
}
