//! A device-free [`AudioSink`] for tests and CI (`VERBATIM_TEST_AUDIO=null`
//! in `verbatim-app`): accepts any [`PcmFormat`] and discards PCM, but still
//! completes the keypress-to-audio latency timeline (architecture section
//! 9) by emitting the `audio_started` trace event on the first write of
//! each utterance — exactly when [`crate::WasapiSink`] emits it on its
//! first buffer submission to the real device — so `LatencyLedger` still
//! records an audio-start time with no sound card required.

use tracing::trace;

use crate::{AudioError, AudioSink, PcmFormat};

/// Discards PCM instead of playing it, for headless or CI test runs.
/// Documented as test-only; never used in a normal Verbatim session.
#[derive(Debug, Default)]
pub struct NullSink {
    trace_id: Option<verbatim_model::TraceId>,
    first_buffer_submitted: bool,
}

impl NullSink {
    /// Creates a sink that discards every utterance's audio.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl AudioSink for NullSink {
    fn begin(
        &mut self,
        _format: PcmFormat,
        trace_id: verbatim_model::TraceId,
    ) -> Result<(), AudioError> {
        self.trace_id = Some(trace_id);
        self.first_buffer_submitted = false;
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Result<(), AudioError> {
        if samples.is_empty() {
            return Ok(());
        }
        if !self.first_buffer_submitted {
            self.first_buffer_submitted = true;
            if let Some(trace_id) = self.trace_id {
                // The final leg of the keypress-to-audio timeline, exactly
                // as WasapiSink emits it on its first real buffer.
                trace!(target: "verbatim::audio", trace_id = %trace_id, "audio_started");
            }
        }
        Ok(())
    }

    fn end(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn stop(&mut self) {
        self.first_buffer_submitted = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::TraceId;

    fn format() -> PcmFormat {
        PcmFormat {
            sample_rate: 22_050,
            channels: 1,
        }
    }

    #[test]
    fn accepts_any_format_and_discards_writes_without_error() {
        let mut sink = NullSink::new();
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        sink.write(&[0i16; 64]).expect("write succeeds");
        sink.write(&[1, 2, 3]).expect("write succeeds");
        sink.end().expect("end succeeds");
    }

    #[test]
    fn first_write_after_begin_marks_the_first_buffer() {
        let mut sink = NullSink::new();
        assert!(!sink.first_buffer_submitted);
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        assert!(!sink.first_buffer_submitted);
        sink.write(&[0i16; 8]).expect("write succeeds");
        assert!(sink.first_buffer_submitted);
    }

    #[test]
    fn empty_write_does_not_mark_the_first_buffer() {
        let mut sink = NullSink::new();
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        sink.write(&[]).expect("write succeeds");
        assert!(!sink.first_buffer_submitted);
    }

    #[test]
    fn stop_resets_first_buffer_state_for_the_next_utterance() {
        let mut sink = NullSink::new();
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        sink.write(&[0i16; 8]).expect("write succeeds");
        assert!(sink.first_buffer_submitted);
        sink.stop();
        assert!(!sink.first_buffer_submitted);
    }

    #[test]
    fn begin_resets_first_buffer_state_for_a_new_utterance() {
        let mut sink = NullSink::new();
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        sink.write(&[0i16; 8]).expect("write succeeds");
        assert!(sink.first_buffer_submitted);
        sink.begin(format(), TraceId::mint())
            .expect("begin succeeds");
        assert!(!sink.first_buffer_submitted);
    }
}
