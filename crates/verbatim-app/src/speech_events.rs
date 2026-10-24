//! The speech pipeline's observer: the latency ledger, plus the index marks
//! the reducer placed, which re-enter it as `Input::MarkReached` so say-all
//! advances as playback reaches each piece (`docs/crates/verbatim-core.md`,
//! "Say-all"), and each utterance with marks cut off before playback
//! reached them all, which re-enters as `Input::UtteranceCancelled` so
//! nothing the reducer waits on is lost by a cut, or failed, which
//! re-enters as `Input::UtteranceFailed`, so what waited carries on past
//! it.
//!
//! Every synthesizer reports its marks: `OneCore` places them itself from its
//! bookmarks, and eSpeak NG from its mark events
//! (`docs/crates/verbatim-synth-espeak.md`, "Index marks"); a driver that
//! cannot place marks has each sequence split at them by the speech manager
//! (`docs/crates/verbatim-speech.md`, "Mark fallback"). Either way the mixer
//! reports a mark when the device has played the audio before it.

use std::sync::Arc;
use std::time::Instant;

use crate::Commands;
use verbatim_model::{Indication, Input, SpeechMark, TraceId, UtteranceEnding, UtteranceId};
use verbatim_speech::{IndexMark, SpeechEvents, SynthTiming};

use crate::ShellCommand;
use crate::latency::LatencyLedger;

/// Forwards every milestone to the latency ledger, and each mark reached to
/// the reducer thread. Runs on the pipeline's threads, so it only posts to
/// the reducer's port, which never waits.
pub(crate) struct ShellSpeechEvents {
    pub(crate) ledger: Arc<LatencyLedger>,
    pub(crate) commands: Commands,
}

impl SpeechEvents for ShellSpeechEvents {
    fn utterance_queued(&self, utterance: UtteranceId, trace_id: TraceId, text: &str, at: Instant) {
        self.ledger.utterance_queued(utterance, trace_id, text, at);
    }

    fn audio_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.audio_started(utterance, trace_id, at);
    }

    fn audio_heard(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.audio_heard(utterance, trace_id, at);
    }

    fn synthesis_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.synthesis_started(utterance, trace_id, at);
    }

    fn synthesizer_audio(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        self.ledger.synthesizer_audio(utterance, trace_id, at);
    }

    fn synth_timing(&self, utterance: UtteranceId, trace_id: TraceId, timing: SynthTiming) {
        self.ledger.synth_timing(utterance, trace_id, timing);
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

    fn marks_cancelled(&self, _: UtteranceId, _: TraceId, marks: &[IndexMark], _: Instant) {
        let input = Input::UtteranceCancelled {
            marks: marks.iter().map(|mark| SpeechMark(mark.0)).collect(),
            at_ms: crate::latency::now_ms(),
        };
        if self
            .commands
            .send(ShellCommand::Input(Box::new(input)))
            .is_err()
        {
            tracing::debug!("the reducer thread is gone; a cut utterance is not reported");
        }
    }

    fn marks_failed(&self, _: UtteranceId, _: TraceId, marks: &[IndexMark], _: Instant) {
        let input = Input::UtteranceFailed {
            marks: marks.iter().map(|mark| SpeechMark(mark.0)).collect(),
        };
        if self
            .commands
            .send(ShellCommand::Input(Box::new(input)))
            .is_err()
        {
            tracing::debug!("the reducer thread is gone; a failed utterance is not reported");
        }
    }

    fn sound_played(&self, indication: Indication, at: Instant) {
        self.ledger.sound_played(indication, at);
    }

    fn speech_paused(&self, paused: bool, at: Instant) {
        self.ledger.speech_paused(paused, at);
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

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use verbatim_process::port_queue::port_queue;

    use super::*;

    /// An observer posting to a fresh reducer port, and that port's taker.
    fn observer() -> (
        ShellSpeechEvents,
        verbatim_process::port_queue::PortQueue<ShellCommand>,
    ) {
        let (commands, taker) = port_queue().expect("a port");
        let ledger = Arc::new(LatencyLedger::new(
            3,
            LatencyLedger::DEFAULT_MAX_BYTES,
            Arc::new(OnceLock::new()),
        ));
        (ShellSpeechEvents { ledger, commands }, taker)
    }

    /// The single input posted to the reducer, unboxed.
    fn only_input(taker: &mut verbatim_process::port_queue::PortQueue<ShellCommand>) -> Input {
        let Some(ShellCommand::Input(input)) = taker.try_recv() else {
            panic!("the observer posted no input to the reducer");
        };
        assert!(
            taker.try_recv().is_none(),
            "the observer posted more than one command"
        );
        *input
    }

    /// A cut utterance's marks re-enter the reducer as
    /// `Input::UtteranceCancelled`, every mark in order, stamped with the
    /// time it was reported.
    #[test]
    fn a_cut_utterance_reaches_the_reducer_with_its_marks() {
        let (events, mut taker) = observer();
        let before = crate::latency::now_ms();
        events.marks_cancelled(
            UtteranceId(1),
            TraceId::mint(),
            &[IndexMark(5), IndexMark(6)],
            Instant::now(),
        );
        let after = crate::latency::now_ms();
        let Input::UtteranceCancelled { marks, at_ms } = only_input(&mut taker) else {
            panic!("the input is not a cut utterance");
        };
        assert_eq!(marks, vec![SpeechMark(5), SpeechMark(6)]);
        assert!(
            (before..=after).contains(&at_ms),
            "{at_ms} is outside {before}..={after}"
        );
    }

    /// A failed utterance's marks re-enter the reducer as
    /// `Input::UtteranceFailed`, every mark in order.
    #[test]
    fn a_failed_utterance_reaches_the_reducer_with_its_marks() {
        let (events, mut taker) = observer();
        events.marks_failed(
            UtteranceId(1),
            TraceId::mint(),
            &[IndexMark(5), IndexMark(6)],
            Instant::now(),
        );
        assert_eq!(
            only_input(&mut taker),
            Input::UtteranceFailed {
                marks: vec![SpeechMark(5), SpeechMark(6)],
            }
        );
    }
}
