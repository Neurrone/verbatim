//! The speech pipeline's observer: the latency ledger, plus the index marks
//! the reducer placed, which re-enter it as `Input::MarkReached` so say-all
//! advances as playback reaches each piece (`docs/crates/verbatim-core.md`,
//! "Say-all").
//!
//! Every synthesizer reports its marks: `OneCore` places them itself from its
//! bookmarks, and eSpeak NG from its mark events
//! (`docs/crates/verbatim-synth-espeak.md`, "Index marks"); a driver that
//! cannot place marks has each sequence split at them by the speech manager
//! (`docs/crates/verbatim-speech.md`, "Mark fallback"). Either way the mixer
//! reports a mark when the device has played the audio before it.

use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::Sender;
use verbatim_model::{Indication, Input, SpeechMark, TraceId, UtteranceEnding, UtteranceId};
use verbatim_speech::{IndexMark, SpeechEvents};

use crate::ShellCommand;
use crate::latency::LatencyLedger;

/// Forwards every milestone to the latency ledger, and each mark reached to
/// the reducer thread. Runs on the pipeline's threads, so it only sends on
/// an unbounded channel, which never waits.
pub(crate) struct ShellSpeechEvents {
    pub(crate) ledger: Arc<LatencyLedger>,
    pub(crate) commands: Sender<ShellCommand>,
}

impl SpeechEvents for ShellSpeechEvents {
    fn utterance_queued(&self, utterance: UtteranceId, trace_id: TraceId, text: &str, at: Instant) {
        self.ledger.utterance_queued(utterance, trace_id, text, at);
    }

    fn audio_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.audio_started(utterance, trace_id, at);
    }

    fn synthesis_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.synthesis_started(utterance, trace_id, at);
    }

    fn synthesizer_audio(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.synthesizer_audio(utterance, trace_id, at);
    }

    fn audio_to_mixer(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.audio_to_mixer(utterance, trace_id, at);
    }

    fn mark_reached(&self, _: UtteranceId, _: TraceId, mark: IndexMark, _: Instant) {
        let input = Input::MarkReached {
            mark: SpeechMark(mark.0),
        };
        if self
            .commands
            .send(ShellCommand::Input(Box::new(input)))
            .is_err()
        {
            tracing::debug!("the reducer thread is gone; a mark is dropped");
        }
    }

    fn sound_played(&self, indication: Indication, at: Instant) {
        self.ledger.sound_played(indication, at);
    }

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        ending: &UtteranceEnding,
        at: Instant,
    ) {
        self.ledger.utterance_ended(utterance, trace_id, ending, at);
    }
}
