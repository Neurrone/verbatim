//! What the listener's asked read shares with the tests that stand in for
//! the listener: the log of a read's calls (`CallLog`), and
//! [`read_object`], the listener's answer to Core's batch on a kept
//! element, for a test that holds the element itself.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use verbatim_uia::{Cached, ListenerEvent, Uia};
use verbatim_win32::window::WindowReadabilityReader;

/// The log of one focus read's calls, for diagnosis alone: it times each
/// call by the system's clock, and a call that takes longer than
/// [`SLOW_CALL`] is logged at info with its name and how long it took, so
/// the slowest calls of a read can be seen (Dickson, 2026-10-20). Each
/// call costs one clock reading, at its start, which is also the end of
/// the call before it; the last ends when the log is dropped, at the
/// read's end. It ends nothing. A test may observe each call as it begins
/// ([`read_object_observed`]).
#[derive(Default)]
pub(crate) struct CallLog {
    /// The call under way, by name, and when it began.
    call: Cell<Option<(&'static str, Instant)>>,
    /// Told each call's name as it begins, in a test.
    observer: Option<CallObserver>,
}

/// How long one call of a focus read may take before it is logged at info.
const SLOW_CALL: Duration = Duration::from_secs(1);

impl CallLog {
    /// The call named `name` begins, and the one before it, if any, has
    /// ended.
    pub(crate) fn call(&self, name: &'static str) {
        let now = Instant::now();
        self.call_ended(now);
        self.call.set(Some((name, now)));
        if let Some(observer) = &self.observer {
            observer(name);
        }
    }

    /// A log that tells `observer` each call's name as it begins.
    pub(crate) fn observed(observer: Option<CallObserver>) -> Self {
        Self {
            call: Cell::new(None),
            observer,
        }
    }

    /// The call under way, if any, ended at `now`: logged if it was slow.
    fn call_ended(&self, now: Instant) {
        if let Some((call, began)) = self.call.take() {
            let elapsed = now.saturating_duration_since(began);
            if elapsed > SLOW_CALL {
                tracing::info!(
                    call,
                    elapsed_ms = elapsed.as_millis(),
                    "a call of a UIA focus read took longer than a second"
                );
            }
        }
    }
}

impl Drop for CallLog {
    /// The read has ended, and with it its last call.
    fn drop(&mut self) {
        if self.call.get().is_some() {
            self.call_ended(Instant::now());
        }
    }
}

/// Told the name of each call a read is about to make
/// ([`read_object_observed`]).
pub type CallObserver = Arc<dyn Fn(&'static str) + Send + Sync>;

/// The listener's answer to Core's batch `batch` on `element`, the
/// element a UI Automation event named, read on `uia`, of process `pid`,
/// whose nearest window Core names, `nearest`: the read a reading thread
/// makes for a kept element (`listener::asked`), judging windows by
/// `readability`, as the listener judges them by the system's state. For a
/// test that stands in for the listener, holding the element itself.
#[must_use]
pub fn read_object(
    (uia, readability): (&Uia, &WindowReadabilityReader),
    element: &Cached<ListenerEvent>,
    (pid, nearest): (verbatim_model::Pid, isize),
    batch: &verbatim_model::batch::Batch,
) -> verbatim_model::request::Outcome {
    crate::listener::asked::batch_on_element(
        (uia, readability),
        element,
        (pid, nearest),
        batch,
        &CallLog::default(),
    )
}

/// [`read_object`], telling `observer` the name of each call the read is
/// about to make, before it makes it, so a test can change the
/// application's windows between two of them.
#[must_use]
pub fn read_object_observed(
    (uia, readability): (&Uia, &WindowReadabilityReader),
    element: &Cached<ListenerEvent>,
    (pid, nearest): (verbatim_model::Pid, isize),
    batch: &verbatim_model::batch::Batch,
    observer: CallObserver,
) -> verbatim_model::request::Outcome {
    crate::listener::asked::batch_on_element(
        (uia, readability),
        element,
        (pid, nearest),
        batch,
        &CallLog::observed(Some(observer)),
    )
}
