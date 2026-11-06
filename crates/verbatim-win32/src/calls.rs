//! The call gate (`books/dev/src/reference/crates/verbatim-win32.md`,
//! `calls`): what every call the backends count consults first, on a
//! thread that installed one, in release builds as in debug ones. An
//! outpost's worker installs a gate bound to its charge, so once its
//! generation is abandoned each counted call fails with
//! [`CallError::Abandoned`] and makes no call, and the window messages it
//! sends test their window's readability with the outpost's reader, which
//! a test replaces. A thread with no gate makes every call, and tests a
//! window's readability by the system's judgement.
//!
//! The gate's failure is its own error, never a fact about the application:
//! no caller reads it as "gone", "import failed" or "no provider". It stops
//! counted calls only; the requests one UIA method makes inside itself, a
//! COM release, and an agile reference's resolve as a program unwinds are
//! not stopped.

use std::cell::RefCell;
use std::sync::Arc;

use windows::Win32::Foundation::E_ABORT;

use crate::window::{Readability, readability as system_readability};

/// What a gate is bound to: an outpost worker's charge.
pub trait Charge: Send + Sync {
    /// Whether the worker's generation was abandoned.
    fn is_dead(&self) -> bool;
    /// A counted call is about to be made: the program in hand calls into
    /// its application.
    fn calling(&self);
    /// Window `hwnd` was found hung or ghosted at a check before a call into
    /// it, and nothing was sent.
    fn found_hung(&self, hwnd: isize);
    /// Whether window `hwnd` can be read, by the outpost's reader.
    fn readability(&self, hwnd: isize) -> Readability;
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
/// worker was abandoned, and told of the call otherwise.
///
/// # Errors
///
/// [`CallError::Abandoned`] once the worker was abandoned.
pub fn check() -> Result<(), CallError> {
    GATE.with_borrow(|gate| match gate {
        Some(charge) if charge.is_dead() => Err(CallError::Abandoned),
        Some(charge) => {
            charge.calling();
            Ok(())
        }
        None => Ok(()),
    })
}

/// Whether window `hwnd` can be read before a window message is sent to
/// it: by this thread's gate's reader, which is told when the window is
/// found hung, or by the system's judgement on a thread with none. Local
/// calls.
#[must_use]
pub fn message_readability(hwnd: isize) -> Readability {
    GATE.with_borrow(|gate| match gate {
        Some(charge) => {
            let found = charge.readability(hwnd);
            if found.is_hung() {
                charge.found_hung(hwnd);
            }
            found
        }
        None => system_readability(hwnd),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    struct Recorded {
        dead: AtomicBool,
        calls: AtomicUsize,
        hung: Mutex<Vec<isize>>,
    }

    impl Charge for Recorded {
        fn is_dead(&self) -> bool {
            self.dead.load(Ordering::Relaxed)
        }
        fn calling(&self) {
            self.calls.fetch_add(1, Ordering::Relaxed);
        }
        fn found_hung(&self, hwnd: isize) {
            self.hung.lock().expect("the record").push(hwnd);
        }
        fn readability(&self, hwnd: isize) -> Readability {
            if hwnd == 7 {
                Readability::Hung
            } else {
                Readability::Readable
            }
        }
    }

    /// A thread's gate refuses every counted call once its worker is
    /// abandoned, with its own error, and tells its charge of each call it
    /// lets through and each window it finds hung; another thread's calls
    /// pass it by.
    #[test]
    fn a_gate_refuses_every_call_once_its_worker_is_abandoned() {
        std::thread::spawn(|| {
            let charge = Arc::new(Recorded::default());
            install(Arc::clone(&charge) as Arc<dyn Charge>);
            assert_eq!(check(), Ok(()));
            assert_eq!(charge.calls.load(Ordering::Relaxed), 1);
            assert_eq!(message_readability(7), Readability::Hung);
            assert_eq!(message_readability(8), Readability::Readable);
            assert_eq!(*charge.hung.lock().expect("the record"), [7]);
            charge.dead.store(true, Ordering::Relaxed);
            assert_eq!(check(), Err(CallError::Abandoned));
            assert_eq!(charge.calls.load(Ordering::Relaxed), 1);
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
}
