//! The observability seam for the speech pipeline (architecture section 9,
//! decision D17).
//!
//! The pipeline reports each utterance's life through a [`SpeechEvents`]
//! observer: queued, its audio started, each index mark reached, and its one
//! ending. Everything after queuing is measured at playback by the audio
//! mixer. The control plane and latency probes implement this trait.

use std::time::Instant;

use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use crate::driver::IndexMark;

/// Receives pipeline milestones for each utterance.
///
/// Implementations must be cheap and non-blocking: they run on the
/// pipeline's own threads (the queue thread for
/// [`utterance_queued`](SpeechEvents::utterance_queued), and for an
/// utterance dropped before synthesis; the audio thread for the rest), and
/// any time spent here is time not spent speaking.
pub trait SpeechEvents: Send + Sync {
    /// An utterance has been rendered and placed on a priority lane. `text`
    /// is its rendered speech text.
    fn utterance_queued(&self, utterance: UtteranceId, trace_id: TraceId, text: &str, at: Instant);

    /// The utterance's first audio frame has played.
    fn audio_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant);

    /// Playback has reached index mark `mark` in the utterance.
    fn mark_reached(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        mark: IndexMark,
        at: Instant,
    ) {
        let _ = (utterance, trace_id, mark, at);
    }

    /// The utterance has ended, once, as `ending` says.
    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        ending: &UtteranceEnding,
        at: Instant,
    );
}
