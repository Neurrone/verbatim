//! The latency ledger (architecture section 9): one timeline per `TraceId` —
//! event observed, speech queued, audio started — assembled from the outpost
//! event stream and the speech pipeline's observer callbacks, served to
//! `verbatim-inspect latency`, and mirrored to control-plane speech
//! subscribers.
//!
//! It also logs one line per announcement, at `info` on the
//! `verbatim::latency` target, when its audio starts: how long each stage
//! took, from Windows raising the event to the audio engine taking the first
//! sample, in milliseconds. Time spent waiting behind earlier speech is
//! reported but not counted, since it is not latency but the queue doing its
//! job.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use verbatim_control::protocol::LatencyRecord;
use verbatim_control::server::ControlServer;
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};
use verbatim_outpost::protocol::{EventTiming, now_us};
use verbatim_speech::SpeechEvents;

/// Milliseconds since the Unix epoch, the ledger's shared time base — the
/// outpost stamps its observations with the same clock.
pub fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// One timeline under assembly.
struct Entry {
    trace_id: TraceId,
    event_observed_at_ms: Option<u64>,
    speech_queued_at_ms: Option<u64>,
    audio_started_at_ms: Option<u64>,
    stages: Stages,
}

/// When an announcement reached each stage, in microseconds since the Unix
/// epoch, for its latency line. The speech stages are those of the trace's
/// first utterance.
#[derive(Default)]
struct Stages {
    event: Option<EventTiming>,
    core_received: Option<u64>,
    reduced: Option<u64>,
    utterance: Option<UtteranceId>,
    text: String,
    queued: Option<u64>,
    synthesis_started: Option<u64>,
    synthesizer_audio: Option<u64>,
    audio_to_mixer: Option<u64>,
    audio_started: Option<u64>,
}

impl Stages {
    /// The announcement's latency line, or `None` when it was not spoken.
    fn line(&self) -> Option<String> {
        let queued = self.queued?;
        let heard = self.audio_started?;
        let event = self.event.unwrap_or_default();
        let at = |us: u64| (us != 0).then_some(us);
        let ms = |from: Option<u64>, to: Option<u64>| match (from, to) {
            // Microseconds between two stages fit an f64 exactly.
            (Some(from), Some(to)) => Some(
                f64::from(u32::try_from(to.saturating_sub(from)).unwrap_or(u32::MAX)) / 1_000.0,
            ),
            _ => None,
        };
        let observed = at(event.observed_at_us);
        let received_by_outpost = at(event.relayed_at_us).or(observed);
        let windows = event.raised_ms_ago.map(f64::from);
        let parts = [
            ("Windows", windows),
            ("listener to outpost", ms(observed, at(event.relayed_at_us))),
            (
                "outpost queue",
                ms(received_by_outpost, at(event.dequeued_at_us)),
            ),
            (
                "outpost read",
                ms(at(event.dequeued_at_us), at(event.published_at_us)),
            ),
            ("to Core", ms(at(event.published_at_us), self.core_received)),
            ("reducer", ms(self.core_received, self.reduced)),
            ("to speech", ms(self.reduced, Some(queued))),
        ];
        let start = observed.or(self.core_received).unwrap_or(queued);
        let event_side = ms(Some(start), Some(queued)).unwrap_or(0.0) + windows.unwrap_or(0.0);
        let started = self.synthesis_started.unwrap_or(queued);
        let waited = ms(Some(queued), Some(started)).unwrap_or(0.0);
        let speech_side = ms(Some(started), Some(heard)).unwrap_or(0.0);
        let speech = [
            (
                "synthesis",
                ms(self.synthesis_started, self.synthesizer_audio),
            ),
            (
                "leading silence",
                ms(self.synthesizer_audio, self.audio_to_mixer),
            ),
            ("mixer and device", ms(self.audio_to_mixer, Some(heard))),
        ];
        let list = |parts: &[(&str, Option<f64>)]| {
            parts
                .iter()
                .filter_map(|(name, value)| value.map(|value| format!("{name} {value:.1}")))
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!(
            "{:.1} ms for {:?}: {event_side:.1} ms to speech and {speech_side:.1} ms to sound, \
             not counting {waited:.1} ms waiting behind earlier speech. Event: {}. Speech: {}.",
            event_side + speech_side,
            self.text,
            list(&parts),
            list(&speech),
        ))
    }
}

/// A bounded ring of recent timelines.
///
/// Also implements [`SpeechEvents`], so the speech pipeline reports queue and
/// audio-start milestones directly into it; those callbacks run on pipeline
/// threads, so everything here is a quick map update under one mutex.
pub struct LatencyLedger {
    entries: Mutex<VecDeque<Entry>>,
    capacity: usize,
    /// The control server, once it exists, for mirroring speech frames to
    /// subscribers. A `OnceLock` because the server is constructed after the
    /// speech pipeline that owns this observer.
    server: Arc<OnceLock<ControlServer>>,
}

impl LatencyLedger {
    /// A ledger holding up to `capacity` recent timelines.
    pub fn new(capacity: usize, server: Arc<OnceLock<ControlServer>>) -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
            capacity,
            server,
        }
    }

    /// Records when an OS event behind `trace_id` was first observed.
    pub fn event_observed(&self, trace_id: TraceId, at_ms: u64) {
        self.update(trace_id, |entry| {
            entry.event_observed_at_ms = Some(at_ms);
        });
    }

    /// Records that Core received the event behind `trace_id` from its
    /// outpost, with when it passed each point there.
    pub fn event_received(&self, trace_id: TraceId, timing: EventTiming) {
        let now = now_us();
        self.update(trace_id, |entry| {
            entry.stages.event = Some(timing);
            entry.stages.core_received = Some(now);
        });
    }

    /// Records that the reducer has handled the input behind `trace_id`.
    pub fn reduced(&self, trace_id: TraceId) {
        let now = now_us();
        self.update(trace_id, |entry| {
            entry.stages.reduced.get_or_insert(now);
        });
    }

    /// Records a speech milestone for `utterance`, when it is its trace's
    /// first.
    fn speech_stage(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        stage: impl FnOnce(&mut Stages) -> &mut Option<u64>,
    ) {
        let now = now_us();
        self.update(trace_id, |entry| {
            if entry.stages.utterance == Some(utterance) {
                stage(&mut entry.stages).get_or_insert(now);
            }
        });
    }

    /// The most recent timelines, newest first, at most `n`.
    pub fn recent(&self, n: u32) -> Vec<LatencyRecord> {
        let entries = self.entries.lock().expect("ledger lock");
        entries
            .iter()
            .rev()
            .take(n as usize)
            .map(|entry| LatencyRecord {
                trace_id: entry.trace_id,
                // A timeline that started in Core (the startup announcement,
                // a future self-originated message) has no OS observation;
                // report the queue time as its start.
                event_observed_at_ms: entry
                    .event_observed_at_ms
                    .or(entry.speech_queued_at_ms)
                    .unwrap_or(0),
                speech_queued_at_ms: entry.speech_queued_at_ms,
                audio_started_at_ms: entry.audio_started_at_ms,
            })
            .collect()
    }

    /// Applies `apply` to the entry for `trace_id`, creating it when new and
    /// evicting the oldest entry beyond capacity; returns `apply`'s result,
    /// so callers can read fields under the same lock they write under.
    fn update<R>(&self, trace_id: TraceId, apply: impl FnOnce(&mut Entry) -> R) -> R {
        let mut entries = self.entries.lock().expect("ledger lock");
        if let Some(entry) = entries.iter_mut().rev().find(|e| e.trace_id == trace_id) {
            return apply(entry);
        }
        let mut entry = Entry {
            trace_id,
            event_observed_at_ms: None,
            speech_queued_at_ms: None,
            audio_started_at_ms: None,
            stages: Stages::default(),
        };
        let result = apply(&mut entry);
        entries.push_back(entry);
        while entries.len() > self.capacity {
            entries.pop_front();
        }
        result
    }
}

impl SpeechEvents for LatencyLedger {
    fn utterance_queued(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        text: &str,
        _at: std::time::Instant,
    ) {
        let at_ms = now_ms();
        let now = now_us();
        let event_observed_at_ms = self.update(trace_id, |entry| {
            entry.speech_queued_at_ms = Some(at_ms);
            if entry.stages.utterance.is_none() {
                entry.stages.utterance = Some(utterance);
                text.clone_into(&mut entry.stages.text);
                entry.stages.queued = Some(now);
            }
            entry.event_observed_at_ms
        });
        if let Some(server) = self.server.get() {
            server.broadcast_speech(
                utterance,
                trace_id,
                text.to_owned(),
                event_observed_at_ms,
                at_ms,
            );
        }
    }

    fn audio_started(&self, utterance: UtteranceId, trace_id: TraceId, _at: std::time::Instant) {
        let at_ms = now_ms();
        // A trace's timeline ends at its first audio: when several
        // utterances share a trace, the first to be heard counts.
        let now = now_us();
        let line = self.update(trace_id, |entry| {
            entry.audio_started_at_ms.get_or_insert(at_ms);
            if entry.stages.utterance != Some(utterance) || entry.stages.audio_started.is_some() {
                return None;
            }
            entry.stages.audio_started = Some(now);
            entry.stages.line()
        });
        if let Some(line) = line {
            tracing::info!(target: "verbatim::latency", "{line}");
        }
        if let Some(server) = self.server.get() {
            server.broadcast_speech_started(utterance, at_ms);
        }
    }

    fn synthesis_started(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        _at: std::time::Instant,
    ) {
        self.speech_stage(utterance, trace_id, |stages| &mut stages.synthesis_started);
    }

    fn synthesizer_audio(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        _at: std::time::Instant,
    ) {
        self.speech_stage(utterance, trace_id, |stages| &mut stages.synthesizer_audio);
    }

    fn audio_to_mixer(&self, utterance: UtteranceId, trace_id: TraceId, _at: std::time::Instant) {
        self.speech_stage(utterance, trace_id, |stages| &mut stages.audio_to_mixer);
    }

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        _trace_id: TraceId,
        ending: &UtteranceEnding,
        _at: std::time::Instant,
    ) {
        // No ledger field to fill — the timeline stops at audio-started — so
        // this only mirrors the ending to speech subscribers, letting a
        // consumer wait for an utterance to be heard in full.
        if let Some(server) = self.server.get() {
            server.broadcast_speech_ended(utterance, ending.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger() -> LatencyLedger {
        LatencyLedger::new(3, Arc::new(OnceLock::new()))
    }

    #[test]
    fn assembles_a_full_timeline() {
        let ledger = ledger();
        let trace = TraceId::mint();
        ledger.event_observed(trace, 100);
        ledger.utterance_queued(
            UtteranceId(1),
            trace,
            "Rate slider 50",
            std::time::Instant::now(),
        );
        ledger.audio_started(UtteranceId(1), trace, std::time::Instant::now());

        let records = ledger.recent(10);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.trace_id, trace);
        assert_eq!(record.event_observed_at_ms, 100);
        assert!(record.speech_queued_at_ms.is_some());
        assert!(record.audio_started_at_ms.is_some());
    }

    #[test]
    fn the_latency_line_names_each_stage_and_leaves_out_the_wait() {
        let stages = Stages {
            event: Some(EventTiming {
                raised_ms_ago: Some(1),
                observed_at_us: 1_000,
                relayed_at_us: 1_500,
                dequeued_at_us: 2_000,
                published_at_us: 9_000,
            }),
            core_received: Some(9_300),
            reduced: Some(9_400),
            utterance: Some(UtteranceId(1)),
            text: "OK button".to_owned(),
            queued: Some(9_500),
            synthesis_started: Some(59_500),
            synthesizer_audio: Some(60_500),
            audio_to_mixer: Some(60_700),
            audio_started: Some(62_500),
        };
        let line = stages.line().expect("spoken");
        assert!(
            line.starts_with("12.5 ms for \"OK button\": 9.5 ms to speech and 3.0 ms to sound, not counting 50.0 ms waiting"),
            "{line}"
        );
        assert!(line.contains("outpost read 7.0"), "{line}");
        assert!(
            line.contains("synthesis 1.0, leading silence 0.2, mixer and device 1.8"),
            "{line}"
        );
    }

    #[test]
    fn evicts_oldest_beyond_capacity_and_lists_newest_first() {
        let ledger = ledger();
        let traces: Vec<TraceId> = (0..5).map(|_| TraceId::mint()).collect();
        for (index, trace) in traces.iter().enumerate() {
            let at = u64::try_from(index).expect("small index") + 1;
            ledger.event_observed(*trace, at);
        }
        let records = ledger.recent(10);
        assert_eq!(records.len(), 3, "capacity bounds the ring");
        assert_eq!(records[0].trace_id, traces[4], "newest first");
        assert_eq!(records[2].trace_id, traces[2]);
    }

    #[test]
    fn core_originated_speech_uses_queue_time_as_start() {
        let ledger = ledger();
        let trace = TraceId::mint();
        ledger.utterance_queued(
            UtteranceId(1),
            trace,
            "Verbatim is starting.",
            std::time::Instant::now(),
        );
        let records = ledger.recent(1);
        assert_eq!(
            Some(records[0].event_observed_at_ms),
            records[0].speech_queued_at_ms
        );
    }
}
