//! Audio output (architecture section 6, decision D5).
//!
//! The [`AudioSink`] trait decouples the speech pipeline from the audio
//! backend; the only initial implementation is WASAPI in event-driven shared
//! mode with small buffers, in service of the 50 ms keypress-to-audio
//! budget. The WASAPI sink lands with workstream WS-A of milestone M1; this
//! module freezes the seam the pipeline and the synth threads code against.

mod null;
mod wasapi;

use std::fmt;

use verbatim_model::TraceId;

pub use null::NullSink;
pub use wasapi::WasapiSink;

/// The PCM stream format a synth driver produces and a sink consumes.
///
/// Samples are signed 16-bit throughout the M1 pipeline; formats differing
/// in rate or channel count are renegotiated per utterance via
/// [`AudioSink::begin`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    /// Samples per second per channel, e.g. 22050 or 48000.
    pub sample_rate: u32,
    /// Interleaved channel count; 1 for every M1 synth.
    pub channels: u16,
}

/// Error from an [`AudioSink`].
#[derive(Debug)]
pub enum AudioError {
    /// The audio device is missing or failed to initialize.
    Device(String),
    /// Writing to an initialized stream failed.
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

/// Where PCM goes. One utterance at a time: `begin`, any number of `write`s,
/// then `end` — or `stop` at any point to discard immediately.
///
/// Implementations emit the "audio started" trace event, tagged with the
/// `TraceId` passed to [`begin`](AudioSink::begin), when the first buffer is
/// actually submitted to the device — that timestamp is the final leg of the
/// keypress-to-audio latency timeline (architecture section 9).
pub trait AudioSink: Send {
    /// Starts an utterance's stream in the given format.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when the device cannot be opened or
    /// does not accept the format.
    fn begin(&mut self, format: PcmFormat, trace_id: TraceId) -> Result<(), AudioError>;

    /// Writes interleaved 16-bit samples, blocking for backpressure when
    /// device buffers are full.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the stream has failed; the
    /// caller abandons the utterance.
    fn write(&mut self, samples: &[i16]) -> Result<(), AudioError>;

    /// Finishes the utterance, letting buffered audio drain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the drain fails.
    fn end(&mut self) -> Result<(), AudioError>;

    /// Discards buffered audio immediately; used to interrupt speech.
    fn stop(&mut self);
}
