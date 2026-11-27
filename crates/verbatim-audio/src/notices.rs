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
//! the failure. A device that opened in a format the mixer refused is
//! opened again when a device arrives or a device's properties change, as
//! changing its format in place raises only property changes.

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
    /// A device's properties changed, as they do when its format is
    /// changed in place: one refused for its format could now fit.
    PropertyChanged,
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
    property_changed: AtomicBool,
    /// How the last open ended: the one record of whether silence plays in
    /// the device's place, and of what then asks for a reopen.
    outcome: Mutex<Outcome>,
    waker: Waker,
}

/// How the last open ended, or that one is under way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// It succeeded, or one is under way.
    Open,
    /// It failed: an arrival asks for another, and so does `retry_at`, for
    /// a failure the system sends no notification for.
    Failed { retry_at: Option<Instant> },
    /// It succeeded in a format the mixer refused: an arrival or a
    /// property change asks for another.
    Refused,
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
                property_changed: AtomicBool::new(false),
                outcome: Mutex::new(Outcome::Open),
                waker,
            }),
        }
    }

    /// Records a notification, from any thread, and wakes the audio thread,
    /// which decides from [`needs_reopen`](Self::needs_reopen) in its pass
    /// whether to reopen. Every notice wakes it, so that whether to wake is
    /// never decided here from a flag the audio thread may be changing: the
    /// flag is set before the wake, and the pass after the wake reads it,
    /// whatever the open's progress.
    pub fn notify(&self, notice: DeviceNotice) {
        let inner = &*self.inner;
        match notice {
            DeviceNotice::DefaultChanged => inner.default_changed.store(true, Ordering::SeqCst),
            DeviceNotice::Available => inner.available.store(true, Ordering::SeqCst),
            DeviceNotice::PropertyChanged => {
                inner.property_changed.store(true, Ordering::SeqCst);
            }
        }
        (inner.waker)();
    }

    /// An open is about to be tried: every earlier notification is
    /// answered by it.
    pub fn opening(&self) {
        let inner = &*self.inner;
        self.set_outcome(Outcome::Open);
        inner.default_changed.store(false, Ordering::SeqCst);
        inner.available.store(false, Ordering::SeqCst);
        inner.property_changed.store(false, Ordering::SeqCst);
    }

    /// The open succeeded.
    pub fn opened(&self) {
        self.set_outcome(Outcome::Open);
    }

    /// The open failed. With `retry_after`, for a failure the system sends
    /// no notification for, the device also asks to be reopened once that
    /// long has passed; without it, only a notification does.
    pub fn failed(&self, retry_after: Option<Duration>) {
        self.set_outcome(Outcome::Failed {
            // A retry past the clock's range is never due.
            retry_at: retry_after.and_then(|after| Instant::now().checked_add(after)),
        });
    }

    /// The open succeeded, but the mixer refused its format: a device
    /// arriving, or a device's properties changing (its format changed in
    /// place raises only that), asks for another open, one per notice, and
    /// no deadline does, so a format that stays too large is not tried
    /// again and again.
    pub fn refused(&self) {
        self.set_outcome(Outcome::Refused);
    }

    /// Whether the last open failed, or was refused for its format, since
    /// the open now under way or the last one began: the one record of it,
    /// which the device answers [`open_failed`] from.
    ///
    /// [`open_failed`]: crate::AudioDevice::open_failed
    #[must_use]
    pub fn open_failed(&self) -> bool {
        self.outcome() != Outcome::Open
    }

    /// Whether the device should be opened again: the default device
    /// changed; or the last open failed and since then a device became
    /// available or its retry deadline passed; or it was refused and since
    /// then a device became available or changed its properties.
    #[must_use]
    pub fn needs_reopen(&self) -> bool {
        let inner = &*self.inner;
        if inner.default_changed.load(Ordering::SeqCst) {
            return true;
        }
        match self.outcome() {
            Outcome::Open => false,
            Outcome::Failed { retry_at } => {
                inner.available.load(Ordering::SeqCst)
                    || retry_at.is_some_and(|at| Instant::now() >= at)
            }
            Outcome::Refused => {
                inner.available.load(Ordering::SeqCst)
                    || inner.property_changed.load(Ordering::SeqCst)
            }
        }
    }

    /// `timeout`, shortened so that a wait ends by the retry deadline of a
    /// failed open, when it has one.
    #[must_use]
    pub fn wait_limit(&self, timeout: Duration) -> Duration {
        match self.outcome() {
            Outcome::Failed { retry_at: Some(at) } => {
                timeout.min(at.saturating_duration_since(Instant::now()))
            }
            Outcome::Open | Outcome::Failed { retry_at: None } | Outcome::Refused => timeout,
        }
    }

    fn outcome(&self) -> Outcome {
        *self
            .inner
            .outcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn set_outcome(&self, outcome: Outcome) {
        *self
            .inner
            .outcome
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = outcome;
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
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "every notice wakes");

        notices.notify(DeviceNotice::DefaultChanged);
        assert!(notices.needs_reopen());
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_device_that_arrives_while_an_open_is_failing_asks_for_another_open() {
        let (notices, wakes) = notices();
        notices.opening();
        notices.notify(DeviceNotice::Available);
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "every notice wakes");
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

    /// A device refused for its format is opened again when a device's
    /// properties change, as changing its format in place raises only
    /// that; one notice asks for one open. An open device, or one that
    /// failed for another reason, ignores property changes.
    #[test]
    fn a_property_change_reopens_only_a_device_refused_for_its_format() {
        let (notices, wakes) = notices();
        notices.opening();
        notices.opened();
        notices.notify(DeviceNotice::PropertyChanged);
        assert!(!notices.needs_reopen(), "an open device");

        notices.opening();
        notices.failed(None);
        notices.notify(DeviceNotice::PropertyChanged);
        assert!(!notices.needs_reopen(), "a failure of another kind");

        notices.opening();
        notices.refused();
        assert!(notices.open_failed());
        assert!(!notices.needs_reopen());
        assert_eq!(
            notices.wait_limit(Duration::MAX),
            Duration::MAX,
            "no deadline"
        );
        notices.notify(DeviceNotice::PropertyChanged);
        assert!(notices.needs_reopen());
        assert_eq!(wakes.load(Ordering::SeqCst), 3, "every notice wakes");

        notices.opening();
        notices.refused();
        assert!(
            !notices.needs_reopen(),
            "the notice was answered by one open"
        );
    }

    #[test]
    fn a_retry_past_the_clocks_range_is_never_due() {
        let (notices, _wakes) = notices();
        notices.opening();
        notices.failed(Some(Duration::MAX));
        assert!(!notices.needs_reopen());
        assert_eq!(notices.wait_limit(Duration::MAX), Duration::MAX);
    }
}
