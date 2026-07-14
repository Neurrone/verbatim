//! The observability seam for the speech pipeline (architecture section 9).
//!
//! The pipeline reports two moments on the keypress-to-audio timeline through
//! a [`SpeechEvents`] observer: when an utterance is queued, and when its
//! audio starts. The control plane and latency probes implement this trait;
//! the pipeline also emits `tracing` spans and events carrying the same
//! `TraceId` at the same points, so a timeline is available whether or not an
//! observer is attached.

use std::time::Instant;

use verbatim_model::TraceId;

/// Receives pipeline milestones for one utterance, keyed by its `TraceId`.
///
/// Implementations must be cheap and non-blocking: they run on the pipeline's
/// own threads (queue thread for [`utterance_queued`](SpeechEvents::utterance_queued),
/// synth thread for [`audio_started`](SpeechEvents::audio_started)) and any
/// time spent here is time not spent synthesizing.
pub trait SpeechEvents: Send + Sync {
    /// An utterance has been rendered and placed on a priority lane. `text` is
    /// the rendered speech text; `at` is the observation instant.
    fn utterance_queued(&self, trace_id: TraceId, text: &str, at: Instant);

    /// The first audio buffer of an utterance has been handed to the sink.
    fn audio_started(&self, trace_id: TraceId, at: Instant);
}
