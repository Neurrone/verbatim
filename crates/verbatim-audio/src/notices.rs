//! What the system reports about its output devices, turned into the
//! requests to reopen that a device makes through
//! [`AudioDevice::needs_reopen`](crate::AudioDevice::needs_reopen).
//!
//! The rule (`docs/design/communication.md`, section 16, on audio device
//! recovery): a device that failed to open is opened again only on
//! evidence that an open could now succeed. That evidence is a
//! notification: the default device changed, or a device arrived or became
//! usable. An open device is reopened only when the default device
//! changed, since a device arriving does not change which one plays. A
//! failure the system sends no notification for clearing, such as a
//! device another program holds for itself or one reconfigured in place,
//! is also retried at a deadline, which the device names when it reports
//! the failure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::Waker;

/// A notification from the system about its output devices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceNotice {
    /// The default output device changed, to another device or to none.
    DefaultChanged,
    /// A device arrived or became usable: one that a device which failed
    /// to open could now open.
    Available,
}

/// Turns device notifications into requests to reopen, shared between the
/// thread the system calls with notifications and the audio thread that
/// opens the device. Cloning gives another handle to the same state.
///
/// The audio thread calls [`opening`](Self::opening) before each open and
/// [`opened`](Self::opened) or [`failed`](Self::failed) after it; a
/// notification that comes while an open is under way is kept, so it is
/// never lost between the attempt and its outcome.
#[derive(Clone)]
pub struct DeviceNotices {
    inner: Arc<Inner>,
}

struct Inner {
    default_changed: AtomicBool,
    available: AtomicBool,
    /// The last open failed: an arrival is then a reason to reopen.
    failed: AtomicBool,
    /// When to try again after a failure the system sends no notification
    /// for; read only while `failed` is set.
    retry_at: Mutex<Option<Instant>>,
    waker: Waker,
}

impl DeviceNotices {
    /// Notices that call `waker` whenever one asks for the device to be
    /// reopened, so the audio thread's wait ends.
    #[must_use]
    pub fn new(waker: Waker) -> Self {
        Self {
            inner: Arc::new(Inner {
                default_changed: AtomicBool::new(false),
                available: AtomicBool::new(false),
                failed: AtomicBool::new(false),
                retry_at: Mutex::new(None),
                waker,
            }),
        }
    }

    /// Records a notification, from any thread, and wakes the audio thread
    /// when it asks for the device to be reopened.
    pub fn notify(&self, notice: DeviceNotice) {
        let inner = &*self.inner;
        match notice {
            DeviceNotice::DefaultChanged => {
                inner.default_changed.store(true, Ordering::SeqCst);
                (inner.waker)();
            }
            DeviceNotice::Available => {
                inner.available.store(true, Ordering::SeqCst);
                // An open device is not woken for an arrival. An open under
                // way reads the flag when it reports its outcome, and the
                // audio thread looks at `needs_reopen` before it next waits.
                if inner.failed.load(Ordering::SeqCst) {
                    (inner.waker)();
                }
            }
        }
    }

    /// An open is about to be tried: every earlier notification is
    /// answered by it.
    pub fn opening(&self) {
        let inner = &*self.inner;
        inner.failed.store(false, Ordering::SeqCst);
        inner.default_changed.store(false, Ordering::SeqCst);
        inner.available.store(false, Ordering::SeqCst);
    }

    /// The open succeeded.
    pub fn opened(&self) {
        self.inner.failed.store(false, Ordering::SeqCst);
    }

    /// The open failed. With `retry_after`, for a failure the system sends
    /// no notification for, the device also asks to be reopened once that
    /// long has passed; without it, only a notification does.
    pub fn failed(&self, retry_after: Option<Duration>) {
        let inner = &*self.inner;
        *inner
            .retry_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            retry_after.map(|after| Instant::now() + after);
        inner.failed.store(true, Ordering::SeqCst);
    }

    /// Whether the device should be opened again: the default device
    /// changed, or the last open failed and since then a device became
    /// available or its retry deadline passed.
    #[must_use]
    pub fn needs_reopen(&self) -> bool {
        let inner = &*self.inner;
        if inner.default_changed.load(Ordering::SeqCst) {
            return true;
        }
        if !inner.failed.load(Ordering::SeqCst) {
            return false;
        }
        inner.available.load(Ordering::SeqCst)
            || self.retry_at().is_some_and(|at| Instant::now() >= at)
    }

    /// `timeout`, shortened so that a wait ends by the retry deadline of a
    /// failed open, when it has one.
    #[must_use]
    pub fn wait_limit(&self, timeout: Duration) -> Duration {
        if !self.inner.failed.load(Ordering::SeqCst) {
            return timeout;
        }
        self.retry_at().map_or(timeout, |at| {
            timeout.min(at.saturating_duration_since(Instant::now()))
        })
    }

    fn retry_at(&self) -> Option<Instant> {
        *self
            .inner
            .retry_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;

    use super::*;

    /// Notices whose waker counts its calls.
    fn notices() -> (DeviceNotices, Arc<AtomicU32>) {
        let wakes = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&wakes);
        let notices = DeviceNotices::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        (notices, wakes)
    }

    #[test]
    fn a_failed_open_waits_without_a_limit_until_a_device_is_available() {
        let (notices, wakes) = notices();
        notices.opening();
        notices.failed(None);
        assert!(!notices.needs_reopen());
        assert_eq!(notices.wait_limit(Duration::MAX), Duration::MAX);

        notices.notify(DeviceNotice::Available);
        assert!(notices.needs_reopen());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        notices.opening();
        assert!(!notices.needs_reopen());
    }

    #[test]
    fn an_open_device_is_reopened_on_a_default_change_and_not_on_an_arrival() {
        let (notices, wakes) = notices();
        notices.opening();
        notices.opened();

        notices.notify(DeviceNotice::Available);
        assert!(!notices.needs_reopen());
        assert_eq!(wakes.load(Ordering::SeqCst), 0);

        notices.notify(DeviceNotice::DefaultChanged);
        assert!(notices.needs_reopen());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_device_that_arrives_while_an_open_is_failing_asks_for_another_open() {
        let (notices, wakes) = notices();
        notices.opening();
        notices.notify(DeviceNotice::Available);
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        notices.failed(None);
        assert!(notices.needs_reopen());
    }

    #[test]
    fn a_failure_with_a_deadline_asks_for_another_open_once_it_has_passed() {
        let (notices, _wakes) = notices();
        notices.opening();
        notices.failed(Some(Duration::ZERO));
        assert!(notices.needs_reopen());
        assert_eq!(notices.wait_limit(Duration::MAX), Duration::ZERO);

        notices.opening();
        notices.opened();
        assert_eq!(notices.wait_limit(Duration::MAX), Duration::MAX);
    }
}
