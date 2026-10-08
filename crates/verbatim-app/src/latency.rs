//! The latency ledger (architecture section 9): one timeline per `TraceId` —
//! event observed, speech queued, audio started — assembled from the outpost
//! event stream and the speech pipeline's observer callbacks, served to
//! `verbatim-inspect latency`, and mirrored to control-plane speech
//! subscribers.
//!
//! It also logs one line per announcement, at `info` on the
//! `verbatim::latency` target, when its audio starts: how long each stage
//! took, from Windows raising the event to the audio engine taking the first
//! sample, in milliseconds, with the cross-process calls the outpost's read
//! made (`docs/performance.md`). A caret key's line starts when the keyboard
//! hook saw the key, and divides the outpost's work into the wait for
//! evidence that the key did something and the read after it. Time spent
//! waiting behind earlier speech is
//! reported but not counted, since it is not latency but the queue doing its
//! job. The same stages and counts travel to `verbatim-inspect latency` in
//! each record.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use verbatim_control::protocol::{LatencyRecord, LatencyStage, LatencyStageKind};
use verbatim_control::server::ControlServer;
use verbatim_model::{CallCounts, Indication, TraceId, UtteranceEnding, UtteranceId};
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
/// first utterance, and the event's are those of the one message from an
/// outpost whose reduction first asked for speech on the trace
/// ([`LatencyLedger::speech_requested`]): the newest Core received for the
/// trace until then. A message received after it, such as a text focus's
/// caret report, which speaks the focus's line, changes nothing here, so a
/// line never mixes two messages' stages.
#[derive(Default)]
struct Stages {
    /// Whether the trace's speech has been asked for, which fixes the
    /// message whose stages these are.
    speech_requested: bool,
    /// When the keyboard hook saw the caret key behind the trace.
    key_pressed: Option<u64>,
    /// When the reducer first handled the trace's input before any outpost
    /// answered for it: a caret key, or a command, which may ask an outpost.
    requested: Option<u64>,
    event: Option<EventTiming>,
    core_received: Option<u64>,
    /// When the reducer handled what an outpost sent for the trace.
    reduced: Option<u64>,
    utterance: Option<UtteranceId>,
    text: String,
    queued: Option<u64>,
    synthesis_started: Option<u64>,
    synthesizer_audio: Option<u64>,
    audio_to_mixer: Option<u64>,
    audio_started: Option<u64>,
}

impl Entry {
    /// An estimate of the memory this entry holds, in bytes: its inline size
    /// plus its utterance text's buffer, the only part that grows.
    fn estimated_bytes(&self) -> usize {
        size_of::<Self>() + self.stages.text.capacity()
    }
}

/// Microseconds as milliseconds; microseconds between two stages fit an
/// f64 exactly.
fn us_to_ms(us: u64) -> f64 {
    f64::from(u32::try_from(us).unwrap_or(u32::MAX)) / 1_000.0
}

/// The calls in `total` that are not in `part`, by kind.
fn calls_besides(total: CallCounts, part: CallCounts) -> CallCounts {
    CallCounts {
        uia: total.uia.saturating_sub(part.uia),
        msaa: total.msaa.saturating_sub(part.msaa),
        window_messages: total.window_messages.saturating_sub(part.window_messages),
    }
}

impl Stages {
    /// How long each stage the announcement has passed took, in pipeline
    /// order, with the cross-process calls of the caret wait and the
    /// outpost read.
    fn breakdown(&self) -> Vec<LatencyStage> {
        let event = self.event.unwrap_or_default();
        let at = |us: u64| (us != 0).then_some(us);
        let span = |from: Option<u64>, to: Option<u64>| Some(to?.saturating_sub(from?));
        let observed = at(event.observed_at_us);
        let relayed = at(event.relayed_at_us);
        let received_by_outpost = relayed.or(observed);
        let dequeued = at(event.dequeued_at_us);
        let awaited = at(event.awaited_at_us);
        let read_calls = calls_besides(event.calls, event.awaited_calls);
        [
            (
                LatencyStageKind::Windows,
                event.raised_ms_ago.map(|ms| u64::from(ms) * 1_000),
                None,
            ),
            (
                LatencyStageKind::HookToCore,
                span(self.key_pressed, self.requested),
                None,
            ),
            (
                LatencyStageKind::ListenerToOutpost,
                span(observed, relayed),
                None,
            ),
            // A request Core made, not an event it relayed from the
            // listener, which the stage before covers.
            (
                LatencyStageKind::CoreToOutpost,
                observed
                    .is_none()
                    .then(|| span(self.requested, relayed))
                    .flatten(),
                None,
            ),
            (
                LatencyStageKind::OutpostQueue,
                span(received_by_outpost, dequeued),
                None,
            ),
            (
                LatencyStageKind::CaretWait,
                span(dequeued, awaited),
                Some(event.awaited_calls),
            ),
            (
                LatencyStageKind::OutpostRead,
                span(awaited.or(dequeued), at(event.published_at_us)),
                Some(read_calls),
            ),
            (
                LatencyStageKind::ToCore,
                span(at(event.published_at_us), self.core_received),
                None,
            ),
            (
                LatencyStageKind::Reducer,
                span(self.core_received, self.reduced),
                None,
            ),
            (
                LatencyStageKind::ToSpeech,
                span(self.reduced.or(self.requested), self.queued),
                None,
            ),
            (
                LatencyStageKind::Synthesis,
                span(self.synthesis_started, self.synthesizer_audio),
                None,
            ),
            (
                LatencyStageKind::LeadingSilence,
                span(self.synthesizer_audio, self.audio_to_mixer),
                None,
            ),
            (
                LatencyStageKind::MixerAndDevice,
                span(self.audio_to_mixer, self.audio_started),
                None,
            ),
        ]
        .into_iter()
        .filter_map(|(kind, duration_us, calls)| {
            Some(LatencyStage {
                kind,
                duration_us: duration_us?,
                calls,
            })
        })
        .collect()
    }

    /// The announcement's latency line, or `None` when it was not spoken.
    fn line(&self) -> Option<String> {
        let queued = self.queued?;
        let heard = self.audio_started?;
        let event = self.event.unwrap_or_default();
        let ms = |from: u64, to: u64| us_to_ms(to.saturating_sub(from));
        let observed = (event.observed_at_us != 0).then_some(event.observed_at_us);
        let windows = event.raised_ms_ago.map_or(0.0, f64::from);
        // The trace's first point: the caret key, the event, the request
        // Core made, or the outpost's message reaching Core.
        let start = self
            .key_pressed
            .or(observed)
            .or(self.requested)
            .or(self.core_received)
            .unwrap_or(queued);
        let event_side = ms(start, queued) + windows;
        let started = self.synthesis_started.unwrap_or(queued);
        let waited = ms(queued, started);
        let speech_side = ms(started, heard);
        let stages = self.breakdown();
        let list = |event_side: bool| {
            stages
                .iter()
                .filter(|stage| stage.kind.is_event_side() == event_side)
                .map(|stage| {
                    let calls = stage
                        .calls
                        .filter(|calls| !calls.is_empty())
                        .map(|calls| format!(" ({} calls)", calls.total()))
                        .unwrap_or_default();
                    format!(
                        "{} {:.1}{calls}",
                        stage.kind.label(),
                        us_to_ms(stage.duration_us)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        Some(format!(
            "{:.1} ms for {:?}: {event_side:.1} ms to speech and {speech_side:.1} ms to sound, \
             not counting {waited:.1} ms waiting behind earlier speech. Event: {}. Speech: {}.",
            event_side + speech_side,
            self.text,
            list(true),
            list(false),
        ))
    }
}

/// The ledger's timelines, oldest first, with their estimated total size.
#[derive(Default)]
struct Timelines {
    entries: VecDeque<Entry>,
    bytes: usize,
}

/// A bounded ring of recent timelines, bounded both by count and by an
/// estimate of the bytes they hold (see `Entry::estimated_bytes`), dropping
/// the oldest first. The newest timeline is always kept, so a single
/// utterance larger than the byte bound is held until the next trace
/// arrives.
///
/// Also implements [`SpeechEvents`], so the speech pipeline reports queue and
/// audio-start milestones directly into it; those callbacks run on pipeline
/// threads, so everything here is a quick map update under one mutex.
pub struct LatencyLedger {
    timelines: Mutex<Timelines>,
    capacity: usize,
    max_bytes: usize,
    /// The control server, once it exists, for mirroring speech frames to
    /// subscribers. A `OnceLock` because the server is constructed after the
    /// speech pipeline that owns this observer.
    server: Arc<OnceLock<ControlServer>>,
    /// The ledger's clock, in microseconds since the Unix epoch: the
    /// outpost's [`now_us`], which a test replaces with its own.
    clock: fn() -> u64,
}

impl LatencyLedger {
    /// The timeline bound the shell uses: 256, enough for `verbatim-inspect
    /// latency` to look back over a few minutes of announcements.
    pub const DEFAULT_CAPACITY: usize = 256;

    /// The byte bound the shell uses: 1 MiB. An ordinary timeline, whose
    /// utterance is a control's name and role, is under half a kilobyte, so
    /// 256 of them stay far below it and the count governs normal use; long
    /// utterances (a say-all chunk, a terminal's output) reach this bound
    /// first, which keeps the ledger's memory fixed whatever is spoken.
    pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

    /// A ledger holding up to `capacity` recent timelines and about
    /// `max_bytes` of them.
    pub fn new(capacity: usize, max_bytes: usize, server: Arc<OnceLock<ControlServer>>) -> Self {
        Self {
            timelines: Mutex::new(Timelines::default()),
            capacity,
            max_bytes,
            server,
            clock: now_us,
        }
    }

    /// A ledger with no control server that reads `clock` for the time.
    #[cfg(test)]
    fn with_clock(capacity: usize, max_bytes: usize, clock: fn() -> u64) -> Self {
        Self {
            clock,
            ..Self::new(capacity, max_bytes, Arc::new(OnceLock::new()))
        }
    }

    /// Records when the keyboard hook saw the caret key behind `trace_id`,
    /// in microseconds since the Unix epoch: where its timeline starts.
    pub fn key_pressed(&self, trace_id: TraceId, at_us: u64) {
        self.update(trace_id, |entry| {
            entry.event_observed_at_ms = Some(at_us / 1_000);
            entry.stages.key_pressed = Some(at_us);
        });
    }

    /// Records when an OS event behind `trace_id` was first observed.
    pub fn event_observed(&self, trace_id: TraceId, at_ms: u64) {
        self.update(trace_id, |entry| {
            // The message the trace's speech came from, as its stages.
            if !entry.stages.speech_requested {
                entry.event_observed_at_ms = Some(at_ms);
            }
        });
    }

    /// Records that Core received the event or query reply behind
    /// `trace_id` from its outpost, with when it passed each point there and
    /// the cross-process calls the outpost made for it. Until the trace's
    /// speech is asked for, each message replaces the one before whole,
    /// with its own reduction to come; after it, a message is not the
    /// speech's, and is left out ([`Stages`]).
    pub fn event_received(&self, trace_id: TraceId, timing: EventTiming) {
        let now = (self.clock)();
        self.update(trace_id, |entry| {
            let stages = &mut entry.stages;
            if !stages.speech_requested {
                stages.event = Some(timing);
                stages.core_received = Some(now);
                stages.reduced = None;
            }
        });
    }

    /// Records that the reducer has handled the input behind `trace_id`:
    /// what an outpost sent for it, once Core has received that, and
    /// otherwise the trace's own input (a caret key or a command), which
    /// may ask an outpost.
    pub fn reduced(&self, trace_id: TraceId) {
        let now = (self.clock)();
        self.update(trace_id, |entry| {
            let stages = &mut entry.stages;
            if stages.speech_requested {
                // A message after the speech's, which is not its.
            } else if stages.core_received.is_some() {
                stages.reduced.get_or_insert(now);
            } else {
                stages.requested.get_or_insert(now);
            }
        });
    }

    /// Records that the reducer asked for speech on `trace_id`, as Core
    /// hands it to the speech pipeline: the message it handled last for the
    /// trace is the one the speech came from, whose stages the trace keeps.
    /// The pipeline queues the utterance on its own thread, by which time
    /// Core may have received the trace's next message.
    pub fn speech_requested(&self, trace_id: TraceId) {
        self.update(trace_id, |entry| entry.stages.speech_requested = true);
    }

    /// Records a speech milestone for `utterance`, when it is its trace's
    /// first.
    fn speech_stage(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        stage: impl FnOnce(&mut Stages) -> &mut Option<u64>,
    ) {
        let now = (self.clock)();
        self.update(trace_id, |entry| {
            if entry.stages.utterance == Some(utterance) {
                stage(&mut entry.stages).get_or_insert(now);
            }
        });
    }

    /// The most recent timelines, newest first, at most `n`.
    pub fn recent(&self, n: u32) -> Vec<LatencyRecord> {
        let timelines = self.timelines.lock().expect("ledger lock");
        timelines
            .entries
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
                stages: entry.stages.breakdown(),
            })
            .collect()
    }

    /// Applies `apply` to the entry for `trace_id`, creating it when new and
    /// evicting the oldest entries beyond either bound; returns `apply`'s
    /// result, so callers can read fields under the same lock they write
    /// under.
    fn update<R>(&self, trace_id: TraceId, apply: impl FnOnce(&mut Entry) -> R) -> R {
        let mut timelines = self.timelines.lock().expect("ledger lock");
        let Timelines { entries, bytes } = &mut *timelines;
        let result = if let Some(entry) = entries.iter_mut().rev().find(|e| e.trace_id == trace_id)
        {
            let before = entry.estimated_bytes();
            let result = apply(entry);
            *bytes = *bytes - before + entry.estimated_bytes();
            result
        } else {
            let mut entry = Entry {
                trace_id,
                event_observed_at_ms: None,
                speech_queued_at_ms: None,
                audio_started_at_ms: None,
                stages: Stages::default(),
            };
            let result = apply(&mut entry);
            *bytes += entry.estimated_bytes();
            entries.push_back(entry);
            result
        };
        while entries.len() > 1 && (entries.len() > self.capacity || *bytes > self.max_bytes) {
            if let Some(oldest) = entries.pop_front() {
                *bytes -= oldest.estimated_bytes();
            }
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
        let now = (self.clock)();
        let at_ms = now / 1_000;
        let event_observed_at_ms = self.update(trace_id, |entry| {
            // The first utterance of a trace is queued first and heard
            // first, so its queue time pairs with the trace's audio start.
            entry.speech_queued_at_ms.get_or_insert(at_ms);
            if entry.stages.utterance.is_none() {
                entry.stages.utterance = Some(utterance);
                text.clone_into(&mut entry.stages.text);
                entry.stages.queued = Some(now);
            }
            entry.event_observed_at_ms
        });
        // The text is copied for the control plane only when some connection
        // is subscribed to speech.
        if let Some(server) = self.server.get()
            && server.has_speech_subscribers()
        {
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
        // A trace's timeline ends at its first audio: when several
        // utterances share a trace, the first to be heard counts.
        let now = (self.clock)();
        let at_ms = now / 1_000;
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

    fn sound_played(&self, indication: Indication, _at: std::time::Instant) {
        // Reported to speech subscribers as `sound:` and the indication's
        // id, as a sound in an utterance's text reads, so the end-to-end
        // suite asserts an event's sound as it asserts words.
        if let Some(server) = self.server.get()
            && server.has_speech_subscribers()
        {
            server.broadcast_sound(indication.id(), now_ms());
        }
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
    use std::cell::Cell;

    thread_local! {
        /// The time a test's ledger reads, in microseconds since the Unix
        /// epoch, set by the test on its own thread.
        static FAKE_US: Cell<u64> = const { Cell::new(0) };
    }

    fn fake_clock() -> u64 {
        FAKE_US.get()
    }

    /// A ledger whose clock reads `FAKE_US`, starting at `at_us`.
    fn ledger_at(at_us: u64) -> LatencyLedger {
        FAKE_US.set(at_us);
        LatencyLedger::with_clock(3, LatencyLedger::DEFAULT_MAX_BYTES, fake_clock)
    }

    fn ledger() -> LatencyLedger {
        LatencyLedger::new(
            3,
            LatencyLedger::DEFAULT_MAX_BYTES,
            Arc::new(OnceLock::new()),
        )
    }

    #[test]
    fn long_utterances_are_bounded_by_bytes_oldest_first() {
        let max_bytes = 100_000;
        let ledger = LatencyLedger::new(256, max_bytes, Arc::new(OnceLock::new()));
        let mut traces = Vec::new();
        for index in 0..100 {
            let trace = TraceId::mint();
            traces.push(trace);
            let text = "x".repeat(10_000 + index);
            ledger.utterance_queued(
                UtteranceId(index as u64),
                trace,
                &text,
                std::time::Instant::now(),
            );
            let timelines = ledger.timelines.lock().expect("ledger lock");
            let actual: usize = timelines.entries.iter().map(Entry::estimated_bytes).sum();
            assert_eq!(timelines.bytes, actual, "the running total is exact");
            assert!(timelines.bytes <= max_bytes);
        }
        // The newest are kept, as many as fit the bound: one more, the
        // newest of those dropped, would not have fitted.
        let records = ledger.recent(256);
        let kept = records.len();
        let bytes_of = |index: usize| size_of::<Entry>() + 10_000 + index;
        let kept_bytes: usize = (100 - kept..100).map(bytes_of).sum();
        assert!(kept_bytes <= max_bytes, "{kept} kept in {kept_bytes} bytes");
        assert!(
            kept_bytes + bytes_of(99 - kept) > max_bytes,
            "{kept} kept, though one more fitted"
        );
        let kept_traces: Vec<TraceId> = records.iter().map(|record| record.trace_id).collect();
        let newest: Vec<TraceId> = traces[100 - kept..].iter().rev().copied().collect();
        assert_eq!(kept_traces, newest, "the oldest were dropped first");
    }

    #[test]
    fn a_single_utterance_over_the_byte_bound_is_kept_until_the_next_trace() {
        let ledger = LatencyLedger::new(256, 1_000, Arc::new(OnceLock::new()));
        let first = TraceId::mint();
        let text = "x".repeat(5_000);
        ledger.utterance_queued(UtteranceId(1), first, &text, std::time::Instant::now());
        assert_eq!(ledger.recent(10)[0].trace_id, first);

        let second = TraceId::mint();
        ledger.event_observed(second, 1);
        let records = ledger.recent(10);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].trace_id, second);
    }

    #[test]
    fn assembles_a_full_timeline() {
        let ledger = ledger_at(250_400);
        let trace = TraceId::mint();
        ledger.event_observed(trace, 100);
        ledger.utterance_queued(
            UtteranceId(1),
            trace,
            "Rate slider 50",
            std::time::Instant::now(),
        );
        FAKE_US.set(262_900);
        ledger.audio_started(UtteranceId(1), trace, std::time::Instant::now());

        assert_eq!(
            ledger.recent(10),
            [LatencyRecord {
                trace_id: trace,
                event_observed_at_ms: 100,
                speech_queued_at_ms: Some(250),
                audio_started_at_ms: Some(262),
                stages: Vec::new(),
            }]
        );
    }

    #[test]
    fn a_trace_keeps_its_first_utterances_queue_time() {
        // A window, then its control, in one trace: the timeline runs from
        // the first utterance's queuing to the first audio.
        let ledger = ledger_at(1_000_000);
        let trace = TraceId::mint();
        let now = std::time::Instant::now;
        ledger.utterance_queued(UtteranceId(1), trace, "Notepad", now());
        FAKE_US.set(1_020_000);
        ledger.audio_started(UtteranceId(1), trace, now());
        FAKE_US.set(1_030_000);
        ledger.utterance_queued(UtteranceId(2), trace, "Text editor", now());

        let record = &ledger.recent(1)[0];
        assert_eq!(record.speech_queued_at_ms, Some(1_000));
        assert_eq!(record.audio_started_at_ms, Some(1_020));
    }

    #[test]
    fn the_latency_line_names_each_stage_and_leaves_out_the_wait() {
        let stages = Stages {
            speech_requested: true,
            key_pressed: None,
            requested: None,
            event: Some(EventTiming {
                raised_ms_ago: Some(1),
                observed_at_us: 1_000,
                relayed_at_us: 1_500,
                dequeued_at_us: 2_000,
                published_at_us: 9_000,
                ..EventTiming::default()
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
        assert_eq!(
            stages.line().as_deref(),
            Some(
                "12.5 ms for \"OK button\": 9.5 ms to speech and 3.0 ms to sound, \
                 not counting 50.0 ms waiting behind earlier speech. \
                 Event: Windows 1.0, listener to outpost 0.5, outpost queue 0.5, \
                 outpost read 7.0, to Core 0.3, reducer 0.1, to speech 0.1. \
                 Speech: synthesis 1.0, leading silence 0.2, mixer and device 1.8."
            )
        );
    }

    #[test]
    fn a_record_carries_each_stage_and_the_outpost_reads_calls() {
        let ledger = ledger();
        let trace = TraceId::mint();
        let calls = verbatim_model::CallCounts {
            uia: 3,
            msaa: 0,
            window_messages: 1,
        };
        ledger.event_received(
            trace,
            EventTiming {
                observed_at_us: 1_000,
                dequeued_at_us: 2_000,
                published_at_us: 9_000,
                calls,
                ..EventTiming::default()
            },
        );
        let record = &ledger.recent(1)[0];
        let kinds: Vec<_> = record.stages.iter().map(|stage| stage.kind).collect();
        assert_eq!(
            kinds,
            [
                LatencyStageKind::OutpostQueue,
                LatencyStageKind::OutpostRead,
                LatencyStageKind::ToCore,
            ],
            "only the stages the trace has passed"
        );
        let read = record.stages[1];
        assert_eq!(read.duration_us, 7_000);
        assert_eq!(read.calls, Some(calls));
        assert_eq!(record.stages[0].calls, None);

        let stages = Stages {
            event: Some(EventTiming {
                observed_at_us: 1_000,
                dequeued_at_us: 2_000,
                published_at_us: 9_000,
                calls,
                ..EventTiming::default()
            }),
            core_received: Some(9_300),
            queued: Some(9_500),
            audio_started: Some(10_000),
            ..Stages::default()
        };
        assert_eq!(
            stages.line().as_deref(),
            Some(
                "9.0 ms for \"\": 8.5 ms to speech and 0.5 ms to sound, \
                 not counting 0.0 ms waiting behind earlier speech. \
                 Event: outpost queue 1.0, outpost read 7.0 (4 calls), to Core 0.3. \
                 Speech: ."
            )
        );
    }

    #[test]
    fn a_caret_keys_line_starts_at_the_hook_and_divides_the_wait_from_the_read() {
        let stages = Stages {
            speech_requested: true,
            key_pressed: Some(1_000),
            requested: Some(1_400),
            event: Some(EventTiming {
                relayed_at_us: 1_600,
                dequeued_at_us: 1_700,
                awaited_at_us: 101_700,
                published_at_us: 103_700,
                calls: CallCounts {
                    uia: 69,
                    msaa: 0,
                    window_messages: 0,
                },
                awaited_calls: CallCounts {
                    uia: 60,
                    msaa: 0,
                    window_messages: 0,
                },
                ..EventTiming::default()
            }),
            core_received: Some(104_000),
            reduced: Some(104_100),
            utterance: Some(UtteranceId(1)),
            text: "alpha beta gamma".to_owned(),
            queued: Some(104_500),
            synthesis_started: Some(104_500),
            synthesizer_audio: Some(105_500),
            audio_to_mixer: Some(105_600),
            audio_started: Some(106_500),
        };
        let kinds: Vec<_> = stages.breakdown().iter().map(|stage| stage.kind).collect();
        assert_eq!(
            kinds,
            [
                LatencyStageKind::HookToCore,
                LatencyStageKind::CoreToOutpost,
                LatencyStageKind::OutpostQueue,
                LatencyStageKind::CaretWait,
                LatencyStageKind::OutpostRead,
                LatencyStageKind::ToCore,
                LatencyStageKind::Reducer,
                LatencyStageKind::ToSpeech,
                LatencyStageKind::Synthesis,
                LatencyStageKind::LeadingSilence,
                LatencyStageKind::MixerAndDevice,
            ]
        );
        assert_eq!(
            stages.line().as_deref(),
            Some(
                "105.5 ms for \"alpha beta gamma\": 103.5 ms to speech and 2.0 ms to sound, \
                 not counting 0.0 ms waiting behind earlier speech. \
                 Event: hook to Core 0.4, Core to outpost 0.2, outpost queue 0.1, \
                 caret wait 100.0 (60 calls), outpost read 2.0 (9 calls), to Core 0.3, \
                 reducer 0.1, to speech 0.4. \
                 Speech: synthesis 1.0, leading silence 0.1, mixer and device 0.9."
            )
        );
    }

    #[test]
    fn the_ledger_splits_a_caret_keys_request_from_its_answer() {
        let ledger = ledger();
        let trace = TraceId::mint();
        ledger.key_pressed(trace, 5_000_000);
        ledger.reduced(trace);
        ledger.event_received(
            trace,
            EventTiming {
                relayed_at_us: now_us(),
                dequeued_at_us: now_us(),
                published_at_us: now_us(),
                ..EventTiming::default()
            },
        );
        ledger.reduced(trace);
        let record = &ledger.recent(1)[0];
        assert_eq!(record.event_observed_at_ms, 5_000, "it starts at the hook");
        let kinds: Vec<_> = record.stages.iter().map(|stage| stage.kind).collect();
        assert_eq!(
            kinds,
            [
                LatencyStageKind::HookToCore,
                LatencyStageKind::CoreToOutpost,
                LatencyStageKind::OutpostQueue,
                LatencyStageKind::OutpostRead,
                LatencyStageKind::ToCore,
                LatencyStageKind::Reducer,
            ],
            "the request's reduction and the answer's are separate stages"
        );
    }

    /// A text focus is announced as its message is reduced, and its line
    /// when its caret report, a second message on the same trace, is: the
    /// announcement's line and record keep the focus message's stages, with
    /// its listener stage, not the caret report's, though the pipeline
    /// queues the announcement only after Core has received the caret
    /// report.
    #[test]
    fn a_line_keeps_the_stages_of_the_message_its_speech_came_from() {
        let ledger = ledger_at(9_300);
        let trace = TraceId::mint();
        let now = std::time::Instant::now;
        ledger.event_observed(trace, 1);
        ledger.event_received(
            trace,
            EventTiming {
                observed_at_us: 1_000,
                relayed_at_us: 1_500,
                dequeued_at_us: 2_000,
                published_at_us: 9_000,
                ..EventTiming::default()
            },
        );
        FAKE_US.set(9_400);
        ledger.reduced(trace);
        ledger.speech_requested(trace);
        FAKE_US.set(21_300);
        ledger.event_observed(trace, 20);
        ledger.event_received(
            trace,
            EventTiming {
                dequeued_at_us: 20_000,
                published_at_us: 21_000,
                ..EventTiming::default()
            },
        );
        FAKE_US.set(21_400);
        ledger.reduced(trace);
        ledger.speech_requested(trace);
        FAKE_US.set(21_500);
        ledger.utterance_queued(UtteranceId(1), trace, "Text editor", now());
        ledger.utterance_queued(UtteranceId(2), trace, "hello", now());
        FAKE_US.set(30_000);
        ledger.audio_started(UtteranceId(1), trace, now());

        let record = &ledger.recent(1)[0];
        assert_eq!(record.event_observed_at_ms, 1);
        let stages: Vec<_> = record
            .stages
            .iter()
            .map(|stage| (stage.kind, stage.duration_us))
            .collect();
        assert_eq!(
            stages,
            [
                (LatencyStageKind::ListenerToOutpost, 500),
                (LatencyStageKind::OutpostQueue, 500),
                (LatencyStageKind::OutpostRead, 7_000),
                (LatencyStageKind::ToCore, 300),
                (LatencyStageKind::Reducer, 100),
                (LatencyStageKind::ToSpeech, 12_100),
            ]
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
