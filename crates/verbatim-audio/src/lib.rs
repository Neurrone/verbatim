//! Audio output (architecture section 6, decisions D5 and D17).
//!
//! Everything Verbatim makes audible goes through one [`Mixer`]: each
//! producer of audio, today the speech pipeline, is a [`Source`] whose PCM
//! the mixer converts to the device's format, sums with the other sources,
//! and writes to an [`AudioDevice`] from its own audio thread. The mixer
//! keeps a source's audio until the device reports it played, so it knows
//! when each utterance and index mark is heard and reports that as a
//! [`PlaybackEvent`], whichever synthesizer produced the audio.
//!
//! The WASAPI device lives in `verbatim-audio-wasapi`, so nothing here
//! depends on Windows. [`SilentDevice`] plays at real-time speed without a
//! sound card, for machines that have none and for test audio, and the mixer
//! plays through one in the real device's place while that cannot be
//! opened, until [`DeviceNotices`] report that an open could now succeed.

#![forbid(unsafe_code)]

mod convert;
mod fallback;
mod mixer;
mod notices;
mod silent;
mod sound;
mod wav;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

pub use mixer::{AudioTap, Mixer, PlaybackEvent, PlaybackListener, Source};
pub use notices::{DeviceNotice, DeviceNotices};
pub use silent::SilentDevice;
pub use sound::Sound;
pub use wav::WavRecorder;

/// The PCM format a synthesizer produces: signed 16-bit, interleaved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    /// Samples per second per channel, e.g. 22050 or 48000.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
}

/// The format a device renders: 32-bit float, interleaved, plus how many
/// frames its buffer holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    /// Frames per second.
    pub sample_rate: u32,
    /// Interleaved channel count.
    pub channels: u16,
    /// Frames the device can hold queued at once.
    pub buffer_frames: u32,
}

/// Error from audio output.
#[derive(Debug)]
pub enum AudioError {
    /// The audio device is missing, failed, or must be reopened.
    Device(String),
    /// Converting or writing a stream failed.
    Stream(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(detail) => write!(f, "audio device error: {detail}"),
            Self::Stream(detail) => write!(f, "audio stream error: {detail}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// Where a device's own clock had got to: how many frames it had played
/// since it last started, and how long before the reading was returned
/// that was so (`docs/design/communication.md`, section 10.1, "Device
/// clock").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockReading {
    /// Frames played by the device's clock since it last started.
    pub frames: u64,
    /// How long ago the device's clock was at `frames`.
    pub age: Duration,
}

/// Wakes a device's [`AudioDevice::wait`] early, from any thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// An output device, driven only by the mixer's audio thread.
///
/// The device holds a queue of frames it is playing out. The mixer keeps it
/// topped up from [`queued_frames`](Self::queued_frames) and
/// [`write`](Self::write), and learns how far playback has got from how
/// many of the frames it wrote are no longer queued.
pub trait AudioDevice: Send {
    /// Opens the device, or reopens it after an error or a request to, and
    /// returns the format the mixer must render. Nothing is queued after
    /// opening, and playback is stopped.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when no usable device can be opened.
    /// The mixer then plays real-time silence in the device's place and
    /// calls `open` again only once [`needs_reopen`](Self::needs_reopen)
    /// asks, so a device that fails to open must ask when there is evidence
    /// an open could succeed ([`DeviceNotices`]).
    fn open(&mut self) -> Result<DeviceFormat, AudioError>;

    /// Frames written but not yet played.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the device has failed.
    fn queued_frames(&mut self) -> Result<u32, AudioError>;

    /// Queues interleaved frames, never more than the buffer has room for.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the device has failed.
    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError>;

    /// Starts playing what is queued, if stopped.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the device has failed.
    fn start(&mut self) -> Result<(), AudioError>;

    /// Stops playback and discards everything queued.
    fn stop(&mut self);

    /// Blocks until the device has played some frames, the
    /// [`waker`](Self::waker) is called, or `timeout` passes.
    fn wait(&mut self, timeout: Duration);

    /// A handle that ends a [`wait`](Self::wait) early.
    fn waker(&self) -> Waker;

    /// Whether the device asks to be reopened, for example because the
    /// system's default output device changed, or because a device became
    /// available after an open failed. A device ends any
    /// [`wait`](Self::wait) once it asks, by its waker or by a deadline of
    /// its own, since the mixer waits idle with no time limit; the mixer
    /// waits on the device's `wait` even while silence plays in its place.
    fn needs_reopen(&self) -> bool {
        false
    }

    /// Where the device's own clock has got to, while it plays, for a
    /// device that has one; `None` by default, and when it cannot be read.
    /// It is what the latency ledger's device clock stage measures, and
    /// nothing else depends on it.
    fn clock_position(&mut self) -> Option<ClockReading> {
        None
    }
}
