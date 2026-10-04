//! A device that plays at real-time speed and makes no sound.
//!
//! Used when a machine has no audio device (GitHub's hosted runners have
//! none), and for test audio. Because frames leave its queue at the rate a
//! real device would play them, an utterance still takes its real duration
//! and its ending still means "would have been heard by now".

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::{AudioDevice, AudioError, DeviceFormat, Waker};

/// The silent device's rate and layout: the usual shared-mode mix format.
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

/// Its queue: 40 ms, like the WASAPI device's.
const BUFFER_FRAMES: u32 = SAMPLE_RATE / 25;

/// How often a running silent device wakes its waiter: 10 ms, a typical
/// device period.
const PERIOD: Duration = Duration::from_millis(10);

/// Plays frames at real-time speed into silence.
pub struct SilentDevice {
    queued: f64,
    running_since: Option<Instant>,
    wake: Arc<(Mutex<bool>, Condvar)>,
}

impl SilentDevice {
    /// A stopped silent device with nothing queued.
    #[must_use]
    pub fn new() -> Self {
        Self {
            queued: 0.0,
            running_since: None,
            wake: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    /// Removes from the queue what real time has played since the last
    /// call.
    fn advance(&mut self) {
        if let Some(since) = self.running_since {
            let now = Instant::now();
            let played = now.duration_since(since).as_secs_f64() * f64::from(SAMPLE_RATE);
            self.queued = (self.queued - played).max(0.0);
            self.running_since = Some(now);
        }
    }
}

impl SilentDevice {
    /// Whether the device is playing (started, not stopped).
    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.running_since.is_some()
    }
}

impl Default for SilentDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioDevice for SilentDevice {
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        self.queued = 0.0;
        self.running_since = None;
        Ok(DeviceFormat {
            sample_rate: SAMPLE_RATE,
            channels: CHANNELS,
            buffer_frames: BUFFER_FRAMES,
        })
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        self.advance();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the queue never exceeds BUFFER_FRAMES"
        )]
        Ok(self.queued.ceil() as u32)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        self.advance();
        #[expect(
            clippy::cast_precision_loss,
            reason = "one write is at most a buffer of frames"
        )]
        let frames = (samples.len() / usize::from(CHANNELS)) as f64;
        self.queued += frames;
        Ok(())
    }

    fn start(&mut self) -> Result<(), AudioError> {
        if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.running_since = None;
        self.queued = 0.0;
    }

    fn wait(&mut self, timeout: Duration) {
        let timeout = if self.running_since.is_some() {
            timeout.min(PERIOD)
        } else {
            timeout
        };
        let (flag, condvar) = &*self.wake;
        let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut woken, _) = condvar
            .wait_timeout_while(guard, timeout, |woken| !*woken)
            .unwrap_or_else(PoisonError::into_inner);
        *woken = false;
    }

    fn waker(&self) -> Waker {
        let wake = Arc::clone(&self.wake);
        Arc::new(move || {
            let (flag, condvar) = &*wake;
            *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
            condvar.notify_all();
        })
    }
}
