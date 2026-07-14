//! The latency ledger (architecture section 9): one timeline per `TraceId` —
//! event observed, speech queued, audio started — assembled from the outpost
//! event stream and the speech pipeline's observer callbacks, served to
//! `verbatim-inspect latency`, and mirrored to control-plane speech
//! subscribers.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use verbatim_control::protocol::LatencyRecord;
use verbatim_control::server::ControlServer;
use verbatim_model::TraceId;
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
    /// The rendered speech text, kept so the audio-start follow-up frame can
    /// repeat it for readable watcher output.
    text: Option<String>,
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
            text: None,
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
    fn utterance_queued(&self, trace_id: TraceId, text: &str, _at: std::time::Instant) {
        let at_ms = now_ms();
        let event_observed_at_ms = self.update(trace_id, |entry| {
            entry.speech_queued_at_ms = Some(at_ms);
            entry.text = Some(text.to_owned());
            entry.event_observed_at_ms
        });
        if let Some(server) = self.server.get() {
            server.broadcast_speech(trace_id, text.to_owned(), event_observed_at_ms, at_ms, None);
        }
    }

    fn audio_started(&self, trace_id: TraceId, _at: std::time::Instant) {
        let at_ms = now_ms();
        let (event_observed_at_ms, queued_at_ms, text) = self.update(trace_id, |entry| {
            entry.audio_started_at_ms = Some(at_ms);
            (
                entry.event_observed_at_ms,
                entry.speech_queued_at_ms,
                entry.text.clone(),
            )
        });
        // The follow-up frame completing the timeline for live watchers: an
        // interrupted utterance never starts audio and never gets one.
        if let Some(server) = self.server.get() {
            server.broadcast_speech(
                trace_id,
                text.unwrap_or_default(),
                event_observed_at_ms,
                queued_at_ms.unwrap_or(at_ms),
                Some(at_ms),
            );
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
        ledger.utterance_queued(trace, "Rate slider 50", std::time::Instant::now());
        ledger.audio_started(trace, std::time::Instant::now());

        let records = ledger.recent(10);
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.trace_id, trace);
        assert_eq!(record.event_observed_at_ms, 100);
        assert!(record.speech_queued_at_ms.is_some());
        assert!(record.audio_started_at_ms.is_some());
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
        ledger.utterance_queued(trace, "Verbatim is starting.", std::time::Instant::now());
        let records = ledger.recent(1);
        assert_eq!(
            Some(records[0].event_observed_at_ms),
            records[0].speech_queued_at_ms
        );
    }
}
