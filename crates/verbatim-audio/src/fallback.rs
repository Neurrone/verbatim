//! The mixer's device: the real device, or real-time silence while the real
//! device cannot be opened.
//!
//! Speech must go on whatever the device does, so when the real device
//! fails to open, the mixer plays through a [`SilentDevice`] instead, which
//! takes as long as the audio would and makes no sound, and opens the real
//! device again only when it asks to be reopened
//! ([`AudioDevice::needs_reopen`]): on a notification that could make an
//! open succeed, or at a deadline the device set for a failure the system
//! sends no notification for ([`DeviceNotices`](crate::DeviceNotices)).
//! Nothing here keeps a timer of its own.

use std::time::Duration;

use tracing::warn;

use crate::silent::PERIOD as SILENT_PERIOD;
use crate::{AudioDevice, AudioError, ClockReading, DeviceFormat, SilentDevice, Waker};

/// The real device, with real-time silence in its place while it cannot be
/// opened.
pub(crate) struct Fallback {
    device: Box<dyn AudioDevice>,
    silent: SilentDevice,
    /// The real device's last open failed, so `silent` plays in its place.
    failed: bool,
}

impl Fallback {
    pub(crate) fn new(device: Box<dyn AudioDevice>) -> Self {
        Self {
            device,
            silent: SilentDevice::new(),
            failed: false,
        }
    }

    /// Opens the real device, or, when it fails, the silent device in its
    /// place, and returns the format to render.
    pub(crate) fn open_or_silence(&mut self) -> DeviceFormat {
        self.silent.stop();
        match self.device.open() {
            Ok(format) => {
                self.failed = false;
                format
            }
            Err(error) => {
                warn!(
                    target: "verbatim::audio",
                    %error,
                    "the audio device could not be opened; playing silently in real time until it asks to be reopened"
                );
                self.failed = true;
                self.silent.open_silent()
            }
        }
    }
}

impl AudioDevice for Fallback {
    /// As [`Fallback::open_or_silence`]; never fails.
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        Ok(self.open_or_silence())
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        if self.failed {
            self.silent.queued_frames()
        } else {
            self.device.queued_frames()
        }
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        if self.failed {
            self.silent.write(samples)
        } else {
            self.device.write(samples)
        }
    }

    fn start(&mut self) -> Result<(), AudioError> {
        if self.failed {
            self.silent.start()
        } else {
            self.device.start()
        }
    }

    fn stop(&mut self) {
        if self.failed {
            self.silent.stop();
        } else {
            self.device.stop();
        }
    }

    /// While silence plays in its place, the wait is still the real
    /// device's, since the mixer's waker and the device's notifications end
    /// that one; it lasts at most one period while the silence is playing,
    /// as the silent device's own wait does.
    fn wait(&mut self, timeout: Duration) {
        if self.failed && self.silent.is_playing() {
            self.device.wait(timeout.min(SILENT_PERIOD));
        } else {
            self.device.wait(timeout);
        }
    }

    fn waker(&self) -> Waker {
        self.device.waker()
    }

    fn needs_reopen(&self) -> bool {
        self.device.needs_reopen()
    }

    fn clock_position(&mut self) -> Option<ClockReading> {
        if self.failed {
            None
        } else {
            self.device.clock_position()
        }
    }
}
