//! The synth driver contract.
//!
//! Deliberately synchronous: a driver's [`speak`](SynthDriver::speak) blocks
//! on its own dedicated synth thread, pushing PCM into a [`SynthSink`] and
//! honoring cooperative cancellation through the sink's return value. This
//! shape maps one-to-one onto a Wasm component world later (an exported
//! `speak` calling host imports for PCM and index marks) and onto the
//! sandboxed native synth host process — Verbatim owns all threading, a
//! driver owns none.

use std::fmt;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};
use verbatim_audio::PcmFormat;
use verbatim_model::TraceId;

use crate::settings::{SettingDescriptor, SettingId, SettingValue, SynthId};

/// An index mark inside a speech request, echoed back by the driver as
/// synthesis passes it; the basis for say-all continuation, braille sync,
/// and latency probes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IndexMark(pub u64);

/// Placement of an [`IndexMark`] within a request's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMark {
    /// Character offset into [`SpeechRequest::text`] (0 marks the start,
    /// `text.chars().count()` the end).
    pub position: usize,
    /// The mark to echo when synthesis reaches the position.
    pub mark: IndexMark,
}

/// One rendered utterance handed to a synth driver: plain text plus marks,
/// after token rendering and dictionary/symbol processing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeechRequest {
    /// The trace this speech belongs to.
    pub trace_id: TraceId,
    /// The text to synthesize.
    pub text: String,
    /// BCP 47 language tag, when known; drivers pick a matching voice when
    /// they can.
    pub language: Option<String>,
    /// Index marks to echo, ordered by position. Drivers that cannot track
    /// positions mid-utterance echo every mark when synthesis completes.
    pub marks: Vec<RequestMark>,
}

/// Error from a synth driver.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SynthError {
    /// The synthesizer cannot be initialized or has stopped working.
    Unavailable(String),
    /// A setting id or value the driver does not accept.
    Setting(String),
    /// Synthesis of one request failed.
    Synthesis(String),
}

impl fmt::Display for SynthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(f, "synthesizer unavailable: {detail}"),
            Self::Setting(detail) => write!(f, "synthesizer setting rejected: {detail}"),
            Self::Synthesis(detail) => write!(f, "synthesis failed: {detail}"),
        }
    }
}

impl std::error::Error for SynthError {}

/// Receives a driver's output during [`SynthDriver::speak`].
pub trait SynthSink {
    /// Accepts interleaved 16-bit PCM in the driver's [`PcmFormat`].
    ///
    /// A return of `ControlFlow::Break(())` tells the driver to stop
    /// synthesizing now — the cooperative-cancellation path; the driver
    /// returns from `speak` promptly without pushing further audio.
    fn push_pcm(&mut self, samples: &[i16]) -> ControlFlow<()>;

    /// Reports that synthesis passed an index mark.
    fn index_reached(&mut self, mark: IndexMark);
}

/// A speech synthesizer, whatever its origin: built-in, Wasm component, or
/// the sandboxed native host (architecture section 6).
///
/// Synchronous by contract. Verbatim calls `speak` on a dedicated synth
/// thread and never on an event, reducer, or GUI thread; drivers block
/// freely (`OneCore` blocks on `WinRT` internally) and must not spawn
/// threads of their own.
pub trait SynthDriver: Send {
    /// Stable identifier, used in config and the synthesizer list.
    fn id(&self) -> SynthId;

    /// Human-readable name for the synthesizer list.
    fn display_name(&self) -> String;

    /// The PCM format `speak` produces. Fixed per driver instance; a driver
    /// whose format depends on a setting reports the current value.
    fn pcm_format(&self) -> PcmFormat;

    /// The settings this driver supports, in display order — the source the
    /// settings GUI generates its controls from.
    fn supported_settings(&self) -> Vec<SettingDescriptor>;

    /// The current value of one setting, or `None` for an unknown id.
    fn setting(&self, id: &SettingId) -> Option<SettingValue>;

    /// Changes one setting, taking effect from the next `speak`.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Setting`] for an unknown id or a value outside
    /// the descriptor's range.
    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError>;

    /// Synthesizes one request, blocking until it finishes or the sink
    /// requests cancellation via `ControlFlow::Break`.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Synthesis`] when the request cannot be
    /// produced; the pipeline drops the utterance and recovers.
    fn speak(
        &mut self,
        request: &SpeechRequest,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError>;
}
