//! The audio thread's store: each source's frames, pending events,
//! utterances and voices, allocated before the audio thread starts and
//! never grown on it (`docs/design/ownership-output.md`, "Real-time audio").
//! The audio thread takes a record from a lane only when the store has
//! room for it ([`SourceState::admits`]), and nothing here allocates or
//! frees: memory the store is done with leaves through the outbound ring.
//!
//! Positions. Each source numbers its frames from zero as they arrive
//! (source positions); the mixer numbers the frames it writes to the device
//! (mix positions). When the mixer takes frames from a source it records a
//! segment saying which source positions went to which mix positions. The
//! device reports how many of the written frames are still queued, so the
//! number played is the number written minus the number queued, and a
//! source position has been heard once its mix position has been played.
//!
//! Retention. A source keeps its frames until they have played, not merely
//! until they were written to the device. So when the device's queue must
//! be discarded (a cancel, a pause, a failure, a device that failed or asks
//! to be reopened) the mixer rewinds to what was played and mixes the other
//! sources again from there, and nothing they had queued is lost.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use rtrb::Producer;
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use super::records::{NewVoice, Outbound};
use crate::PlaybackEvent;

/// Utterances a source tracks at once.
pub(crate) const TRACKED: usize = 64;

/// Pending events a source takes records for; it holds up to [`TRACKED`]
/// more, for the endings a cancel or a failure adds at once.
pub(crate) const EVENTS: usize = 1_024;

/// Voices a source mixes at once.
pub(crate) const VOICES: usize = 64;

/// Unplayed mixed runs a source keeps track of; a source that has this
/// many is not mixed until some have played.
const SEGMENTS: usize = 1_024;

/// The largest device format the mixer accepts: 8 channels at 192 kHz.
const MAX_CHANNELS: usize = 8;
const MAX_RATE: usize = 192_000;

/// The margin a format must leave in the reserve beyond a source's
/// capacity: one device period of 10 ms, so a source at its bound still
/// has room for the frames a pass takes before it lets played ones go.
pub(crate) const MARGIN_MS: u32 = 10;

/// The samples a source that takes frames reserves: its capacity, a 40 ms
/// device queue and 40 ms ahead, and the margin, at the largest format.
pub(crate) const SAMPLES: usize = MAX_RATE * (80 + MARGIN_MS as usize) / 1_000 * MAX_CHANNELS;

/// One frame of one source's pending events.
pub(crate) struct Pending {
    position: u64,
    utterance: UtteranceId,
    trace_id: TraceId,
    kind: Kind,
}

enum Kind {
    /// Fires when the frame at the position has played.
    Start,
    /// Fires when every frame before the position has played.
    Mark(u64),
    /// Fires when every frame before the position has played.
    End,
    /// Fires at once: an ending a cancel, a failure or a sound's end made.
    Ended(UtteranceEnding),
    /// Fires at once: the utterance failed because its frames were
    /// converted for a device format no longer in use. The delivery thread
    /// writes the reason, so the audio thread allocates nothing for it.
    FormatChanged,
}

/// One utterance registered and not yet ended.
struct Tracked {
    utterance: UtteranceId,
    trace_id: TraceId,
    /// The cancel generation it was registered under.
    generation: u64,
    /// Source position of its first frame.
    start: u64,
    /// Still accepting frames.
    writing: bool,
    /// Its start event has been queued.
    started: bool,
}

/// Source positions `source_start..source_start + len` were written to the
/// device at mix positions `mix_start..mix_start + len`.
#[derive(Clone, Copy, Debug)]
struct Segment {
    source_start: u64,
    mix_start: u64,
    len: u64,
}

/// A sound mixed over a source's audio.
struct Voice {
    /// The utterance it belongs to, ended with it; `None` for a sound
    /// played at once.
    utterance: Option<UtteranceId>,
    /// Its utterance ends when it has played to its end
    /// ([`Record::PlayToEnd`](super::records::Record::PlayToEnd)).
    ends: bool,
    /// The source position it starts at.
    trigger: u64,
    /// Interleaved device-format frames.
    frames: Arc<[f32]>,
    gain: f32,
    state: VoiceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VoiceState {
    /// Waiting for the source's audio to reach its start.
    Waiting,
    /// Started and partly heard, then rewound: it goes on from frame
    /// `heard` at the next frames mixed.
    Resume { heard: u64 },
    /// Playing: frame `n` of it is at mix position `start + n`.
    Playing { start: u64 },
    /// Done with, waiting for room in the outbound ring to leave.
    Retired,
}

/// One source's state, owned by the audio thread.
pub(crate) struct SourceState {
    /// Channels per frame of the device format the data is in.
    pub(crate) channels: usize,
    /// The samples this source may hold: [`SAMPLES`] for a source that
    /// takes frames, none for one that only plays sounds.
    sample_room: usize,
    /// Interleaved device-format frames from `data_start` onward.
    data: VecDeque<f32>,
    data_start: u64,
    /// Position after the last frame written.
    write_end: u64,
    /// Position after the last frame mixed.
    mixed_end: u64,
    /// Mixed frames not yet played, oldest first.
    segments: VecDeque<Segment>,
    events: VecDeque<Pending>,
    tracked: Vec<Tracked>,
    /// Held where it is: none of its frames are mixed.
    pub(crate) paused: bool,
    /// Resumed from a pause, and not mixed from since: the device running
    /// dry meanwhile was the pause, not an underrun.
    resumed: bool,
    /// Sounds mixed over the source's audio, until they have played.
    voices: Vec<Voice>,
    /// Utterances whose start has played, with the mix position of their
    /// first frame, until the device's clock passes it.
    awaiting_clock: Vec<(UtteranceId, TraceId, u64)>,
    /// The source's cancel generation as this thread last read it.
    pub(crate) generation: u64,
    /// Frames of the source's lane let go of, played or discarded, in the
    /// lane's own count: what the producer compares its count with.
    pub(crate) released: u64,
    /// Frames were let go of this pass.
    pub(crate) released_now: bool,
}

impl SourceState {
    /// A source with room for [`SAMPLES`] samples when `takes_frames`,
    /// allocated now, before the audio thread starts.
    pub(crate) fn new(takes_frames: bool) -> Self {
        let sample_room = if takes_frames { SAMPLES } else { 0 };
        Self {
            channels: 0,
            sample_room,
            data: VecDeque::with_capacity(sample_room),
            data_start: 0,
            write_end: 0,
            mixed_end: 0,
            segments: VecDeque::with_capacity(SEGMENTS),
            events: VecDeque::with_capacity(EVENTS + TRACKED + 1),
            tracked: Vec::with_capacity(TRACKED),
            paused: false,
            resumed: false,
            voices: Vec::with_capacity(VOICES),
            awaiting_clock: Vec::with_capacity(TRACKED),
            generation: 0,
            released: 0,
            released_now: false,
        }
    }

    /// Frames held, written and not yet played.
    pub(crate) fn held(&self) -> u64 {
        self.write_end - self.data_start
    }

    /// Whether the store has room for what a record of each kind adds.
    pub(crate) fn admits_register(&self) -> bool {
        self.tracked.len() < TRACKED && self.events.len() < EVENTS
    }

    /// Whether `samples` more samples fit, with the start event they may
    /// add.
    pub(crate) fn admits_frames(&self, samples: usize) -> bool {
        self.data.len() + samples <= self.sample_room && self.events.len() < EVENTS
    }

    pub(crate) fn admits_event(&self) -> bool {
        self.events.len() < EVENTS
    }

    pub(crate) fn admits_voice(&self) -> bool {
        self.voices.len() < VOICES
    }

    /// The tracked utterance `utterance`, if it still accepts frames.
    fn writing(&mut self, utterance: UtteranceId) -> Option<&mut Tracked> {
        self.tracked
            .iter_mut()
            .find(|tracked| tracked.utterance == utterance && tracked.writing)
    }

    /// Ends `utterance` at once with `ending`, before every event pending.
    fn end_now(&mut self, utterance: UtteranceId, trace_id: TraceId, ending: UtteranceEnding) {
        self.events.push_front(Pending {
            position: 0,
            utterance,
            trace_id,
            kind: Kind::Ended(ending),
        });
    }

    /// Registers `utterance` under cancel generation `generation`: ended
    /// cancelled at once when the source has been cancelled since.
    pub(crate) fn register(&mut self, utterance: UtteranceId, trace_id: TraceId, generation: u64) {
        if generation < self.generation {
            self.end_now(utterance, trace_id, UtteranceEnding::Cancelled);
            return;
        }
        self.tracked.push(Tracked {
            utterance,
            trace_id,
            generation,
            start: self.write_end,
            writing: true,
            started: false,
        });
    }

    /// Takes `frames` frames of `utterance` from `samples`, or discards
    /// them when the utterance no longer accepts frames or `current` is
    /// false (they are in a device format no longer in use). Frames in an
    /// old format for an utterance still being written fail it: it was
    /// registered after the device changed format, its audio converted
    /// before, and none of it can play.
    pub(crate) fn frames(
        &mut self,
        utterance: UtteranceId,
        frames: u64,
        samples: (&[f32], &[f32]),
        current: bool,
    ) {
        if !current {
            self.release_count(frames);
            if let Some(index) = self
                .tracked
                .iter()
                .position(|tracked| tracked.utterance == utterance && tracked.writing)
            {
                self.end_failed(index, Kind::FormatChanged);
            }
            return;
        }
        let write_end = self.write_end;
        let Some(tracked) = self.writing(utterance) else {
            self.release_count(frames);
            return;
        };
        if !tracked.started {
            tracked.started = true;
            let trace_id = tracked.trace_id;
            self.events.push_back(Pending {
                position: write_end,
                utterance,
                trace_id,
                kind: Kind::Start,
            });
        }
        self.data.extend(samples.0.iter().copied());
        self.data.extend(samples.1.iter().copied());
        self.write_end += frames;
    }

    /// Records index mark `mark` at the current end of `utterance`'s audio.
    pub(crate) fn mark(&mut self, utterance: UtteranceId, mark: u64) {
        let position = self.write_end;
        if let Some(tracked) = self.writing(utterance) {
            let trace_id = tracked.trace_id;
            self.events.push_back(Pending {
                position,
                utterance,
                trace_id,
                kind: Kind::Mark(mark),
            });
        }
    }

    /// Adds `voice` at the current end of the source's audio, for
    /// `utterance` (`None` for a sound played at once), when its utterance
    /// still accepts frames and `current` is true; otherwise its frames
    /// leave at once.
    pub(crate) fn voice(
        &mut self,
        utterance: Option<UtteranceId>,
        voice: NewVoice,
        current: bool,
        out: &mut Producer<Outbound>,
    ) {
        let accepted =
            current && utterance.is_none_or(|utterance| self.writing(utterance).is_some());
        if !accepted {
            let _ = out.push(Outbound::Frames(voice.frames));
            return;
        }
        self.voices.push(Voice {
            utterance,
            ends: false,
            trigger: self.write_end,
            frames: voice.frames,
            gain: voice.gain,
            state: VoiceState::Waiting,
        });
    }

    /// Plays `voice` at once as `utterance`, which ends when it has played
    /// to its end; when `current` is false, its frames leave and the
    /// utterance ends at once, as nothing of it can play.
    pub(crate) fn play_to_end(
        &mut self,
        utterance: UtteranceId,
        trace_id: TraceId,
        voice: NewVoice,
        current: bool,
        out: &mut Producer<Outbound>,
    ) {
        if !current {
            let _ = out.push(Outbound::Frames(voice.frames));
            self.end_now(utterance, trace_id, UtteranceEnding::Cancelled);
            return;
        }
        self.tracked.push(Tracked {
            utterance,
            trace_id,
            generation: self.generation,
            start: self.write_end,
            writing: false,
            started: true,
        });
        self.voices.push(Voice {
            utterance: Some(utterance),
            ends: true,
            trigger: self.write_end,
            frames: voice.frames,
            gain: voice.gain,
            state: VoiceState::Waiting,
        });
    }

    /// Ends `utterance`'s audio: it completes once the device has played
    /// its last frame.
    pub(crate) fn finish(&mut self, utterance: UtteranceId) {
        let position = self.write_end;
        if let Some(tracked) = self.writing(utterance) {
            tracked.writing = false;
            let trace_id = tracked.trace_id;
            self.events.push_back(Pending {
                position,
                utterance,
                trace_id,
                kind: Kind::End,
            });
        }
    }

    /// Ends `utterance` as failed for `reason`, discarding whatever of it
    /// has not played, after the device's queue was discarded. Gives the
    /// reason back when the utterance had already ended.
    pub(crate) fn fail(&mut self, utterance: UtteranceId, reason: String) -> Option<String> {
        let Some(index) = self
            .tracked
            .iter()
            .position(|tracked| tracked.utterance == utterance)
        else {
            return Some(reason);
        };
        self.end_failed(index, Kind::Ended(UtteranceEnding::Failed(reason)));
        None
    }

    /// Ends the tracked utterance at `index` at once with `ending`, a
    /// failure, discarding whatever of it has not played.
    fn end_failed(&mut self, index: usize, ending: Kind) {
        let tracked = self.tracked.remove(index);
        let utterance = tracked.utterance;
        // The failing utterance is the one being written, so it is the last
        // in the source: everything of it not yet played goes.
        self.truncate(tracked.start.max(self.mixed_end));
        self.events.retain(|pending| pending.utterance != utterance);
        self.retire_voices(|voice| voice.utterance == Some(utterance));
        self.events.push_front(Pending {
            position: 0,
            utterance,
            trace_id: tracked.trace_id,
            kind: ending,
        });
    }

    /// Ends every utterance registered under a cancel generation older than
    /// `generation` as cancelled, after the device's queue was discarded:
    /// their frames not yet played, their events, their voices and every
    /// voice played at once go, and the utterances registered since keep
    /// their audio, moved up to where the cancelled audio began.
    pub(crate) fn cancel_older(&mut self, generation: u64) {
        let spared = self
            .tracked
            .iter()
            .find(|tracked| tracked.generation >= generation)
            .map_or(self.write_end, |tracked| tracked.start);
        let from = self.mixed_end.max(self.data_start);
        let removed = spared.saturating_sub(from);
        if removed > 0 {
            let start = self.sample_index(from);
            let end = self.sample_index(spared);
            self.data.drain(start..end);
            self.release_count(removed);
        }
        // Events: the cancelled utterances' go; the rest move up.
        let mut index = 0;
        while index < self.events.len() {
            let pending = &mut self.events[index];
            let cancelled = self.tracked.iter().any(|tracked| {
                tracked.utterance == pending.utterance && tracked.generation < generation
            });
            if cancelled && !matches!(pending.kind, Kind::Ended(_) | Kind::FormatChanged) {
                let _ = self.events.remove(index);
                continue;
            }
            if pending.position >= spared
                && !matches!(pending.kind, Kind::Ended(_) | Kind::FormatChanged)
            {
                pending.position -= removed;
            }
            index += 1;
        }
        // Voices: the cancelled utterances' and those played at once go.
        let tracked = &self.tracked;
        let cancelled = |voice: &Voice| {
            voice.utterance.is_none_or(|utterance| {
                tracked.iter().any(|tracked| {
                    tracked.utterance == utterance && tracked.generation < generation
                })
            })
        };
        let mut index = 0;
        while index < self.voices.len() {
            let voice = &self.voices[index];
            if voice.state != VoiceState::Retired && cancelled(voice) {
                self.voices[index].state = VoiceState::Retired;
            } else if voice.state == VoiceState::Waiting && voice.trigger >= spared {
                self.voices[index].trigger -= removed;
            }
            index += 1;
        }
        // The utterances: ended in the order registered, before every
        // event pending.
        let mut index = self.tracked.len();
        while index > 0 {
            index -= 1;
            if self.tracked[index].generation < generation {
                let tracked = self.tracked.remove(index);
                self.end_now(
                    tracked.utterance,
                    tracked.trace_id,
                    UtteranceEnding::Cancelled,
                );
            } else if self.tracked[index].start >= spared {
                self.tracked[index].start -= removed;
            }
        }
        self.write_end -= removed;
        self.mixed_end = self.mixed_end.min(self.write_end);
    }

    /// Ends every utterance not yet ended with `ending`, discarding all
    /// audio not yet played: for a device that changed format, and at
    /// shutdown. Endings already pending stay.
    pub(crate) fn end_all(&mut self, ending: &UtteranceEnding) {
        let cut = self.mixed_end;
        self.truncate(cut);
        self.events
            .retain(|pending| matches!(pending.kind, Kind::Ended(_) | Kind::FormatChanged));
        self.retire_voices(|_| true);
        let mut index = self.tracked.len();
        while index > 0 {
            index -= 1;
            let tracked = self.tracked.remove(index);
            self.end_now(tracked.utterance, tracked.trace_id, ending.clone());
        }
    }

    /// The index in `data` of the first sample of source position
    /// `position`.
    fn sample_index(&self, position: u64) -> usize {
        usize::try_from(position - self.data_start).unwrap_or(usize::MAX) * self.channels
    }

    /// Counts `frames` more frames let go of.
    fn release_count(&mut self, frames: u64) {
        self.released += frames;
        self.released_now = true;
    }

    /// Retires every voice `which` picks.
    fn retire_voices(&mut self, which: impl Fn(&Voice) -> bool) {
        for voice in &mut self.voices {
            if voice.state != VoiceState::Retired && which(voice) {
                voice.state = VoiceState::Retired;
            }
        }
    }

    /// Sends the frames of retired voices out while the ring has room;
    /// returns whether it sent any.
    pub(crate) fn flush_retired(&mut self, out: &mut Producer<Outbound>) -> bool {
        let mut sent = false;
        let mut index = 0;
        while index < self.voices.len() {
            if self.voices[index].state != VoiceState::Retired {
                index += 1;
                continue;
            }
            if out.is_full() {
                break;
            }
            let voice = self.voices.swap_remove(index);
            let _ = out.push(Outbound::Frames(voice.frames));
            sent = true;
        }
        sent
    }

    /// Whether every voice has left: nothing retired waits for the ring.
    pub(crate) fn nothing_retired(&self) -> bool {
        self.voices
            .iter()
            .all(|voice| voice.state != VoiceState::Retired)
    }

    /// Whether events are still pending.
    pub(crate) fn events_pending(&self) -> bool {
        !self.events.is_empty()
    }

    /// The mix position of the frame at source position `position`, if it
    /// has been mixed and not yet released.
    fn mix_position(&self, position: u64) -> Option<u64> {
        self.segments
            .iter()
            .find(|segment| {
                position >= segment.source_start && position < segment.source_start + segment.len
            })
            .map(|segment| segment.mix_start + (position - segment.source_start))
    }

    /// Whether the frame at `position` has played.
    fn frame_played(&self, position: u64, played: u64) -> bool {
        if position < self.data_start {
            return true;
        }
        self.mix_position(position).is_some_and(|mix| mix < played)
    }

    /// Whether every frame before `position` has played.
    fn played_up_to(&self, position: u64, played: u64) -> bool {
        position <= self.data_start || self.frame_played(position - 1, played)
    }

    /// Retires the voices that have played to their end, ending the
    /// utterance of one that ends it, then reports every event whose moment
    /// has played, while the ring has room, then lets go of the frames that
    /// have played. Returns whether it sent anything.
    pub(crate) fn fire(
        &mut self,
        source: usize,
        played: u64,
        at: Instant,
        out: &mut Producer<Outbound>,
    ) -> bool {
        self.release_voices(played);
        let mut sent = false;
        while let Some(pending) = self.events.front() {
            let due = match pending.kind {
                Kind::Start => self.frame_played(pending.position, played),
                Kind::Mark(_) | Kind::End => self.played_up_to(pending.position, played),
                Kind::Ended(_) | Kind::FormatChanged => true,
            };
            if !due || out.is_full() {
                break;
            }
            let Some(pending) = self.events.pop_front() else {
                break;
            };
            let event = match pending.kind {
                Kind::Start => {
                    if let Some(mix) = self.mix_position(pending.position)
                        && self.awaiting_clock.len() < self.awaiting_clock.capacity()
                    {
                        self.awaiting_clock
                            .push((pending.utterance, pending.trace_id, mix));
                    }
                    PlaybackEvent::Started {
                        utterance: pending.utterance,
                        trace_id: pending.trace_id,
                    }
                }
                Kind::Mark(mark) => PlaybackEvent::Mark {
                    utterance: pending.utterance,
                    trace_id: pending.trace_id,
                    mark,
                },
                Kind::End => {
                    self.tracked
                        .retain(|tracked| tracked.utterance != pending.utterance);
                    PlaybackEvent::Ended {
                        utterance: pending.utterance,
                        trace_id: pending.trace_id,
                        ending: UtteranceEnding::Completed,
                    }
                }
                Kind::Ended(ending) => PlaybackEvent::Ended {
                    utterance: pending.utterance,
                    trace_id: pending.trace_id,
                    ending,
                },
                Kind::FormatChanged => {
                    let _ = out.push(Outbound::FormatChanged {
                        source,
                        utterance: pending.utterance,
                        trace_id: pending.trace_id,
                        at,
                    });
                    sent = true;
                    continue;
                }
            };
            let _ = out.push(Outbound::Event { source, event, at });
            sent = true;
        }
        self.release_frames(played);
        sent
    }

    /// Retires voices that have played to their end, ending the utterance
    /// of one that ends it.
    fn release_voices(&mut self, played: u64) {
        let channels = self.channels;
        let mut index = 0;
        while index < self.voices.len() {
            let voice = &self.voices[index];
            if let VoiceState::Playing { start } = voice.state
                && start + voice_len(voice, channels) <= played
            {
                if voice.ends
                    && let Some(utterance) = voice.utterance
                    && let Some(at) = self
                        .tracked
                        .iter()
                        .position(|tracked| tracked.utterance == utterance)
                {
                    let tracked = self.tracked.remove(at);
                    self.end_now(utterance, tracked.trace_id, UtteranceEnding::Completed);
                }
                self.voices[index].state = VoiceState::Retired;
            }
            index += 1;
        }
    }

    /// Lets go of frames that have played.
    fn release_frames(&mut self, played: u64) {
        let channels = self.channels;
        while let Some(segment) = self.segments.front_mut() {
            let done = played.saturating_sub(segment.mix_start).min(segment.len);
            if done == 0 {
                break;
            }
            let samples = usize::try_from(done).unwrap_or(usize::MAX) * channels;
            self.data.drain(..samples.min(self.data.len()));
            self.data_start += done;
            segment.source_start += done;
            segment.mix_start += done;
            segment.len -= done;
            let more = segment.len > 0;
            self.release_count(done);
            if more {
                break;
            }
            self.segments.pop_front();
        }
    }

    /// Forgets mixing done at or after mix position `played`, so those
    /// frames are mixed again: a voice not yet heard waits for its place
    /// again, and one partly heard goes on from where it was.
    pub(crate) fn rewind(&mut self, played: u64) {
        let channels = self.channels;
        for voice in &mut self.voices {
            if let VoiceState::Playing { start } = voice.state {
                let len = voice_len(voice, channels);
                if start >= played {
                    voice.state = VoiceState::Waiting;
                } else {
                    let heard = (played - start).min(len);
                    voice.state = if heard == len {
                        VoiceState::Retired
                    } else {
                        VoiceState::Resume { heard }
                    };
                }
            }
        }
        while let Some(segment) = self.segments.back_mut() {
            if segment.mix_start >= played {
                self.mixed_end -= segment.len;
                self.segments.pop_back();
            } else {
                let keep = (played - segment.mix_start).min(segment.len);
                self.mixed_end -= segment.len - keep;
                segment.len = keep;
                break;
            }
        }
    }

    /// Discards every frame from source position `from` onward.
    fn truncate(&mut self, from: u64) {
        let from = from.max(self.data_start);
        if from < self.write_end {
            let removed = self.write_end - from;
            let keep = self.sample_index(from);
            self.data.truncate(keep);
            self.write_end = from;
            self.release_count(removed);
        }
        self.mixed_end = self.mixed_end.min(from);
    }

    /// The device changed to `channels` channels: nothing of the old format
    /// is held any more ([`end_all`](Self::end_all) came first).
    pub(crate) fn set_channels(&mut self, channels: usize) {
        self.channels = channels;
    }

    /// Frames written and not yet mixed.
    fn unmixed(&self) -> u64 {
        self.write_end - self.mixed_end
    }

    /// Whether `voice`'s utterance is still being written. Until it is
    /// not, the voice is mixed only alongside the utterance's own audio, so
    /// every frame of that audio is heard at its place under the sound,
    /// whenever it was synthesized: a sound placed between words starts
    /// with the next word, and the word does not slip past the sound's end
    /// because playback reached the sound before the word was written. Once
    /// the utterance has written all its audio, the sound plays on past it.
    fn still_written(&self, voice: &Voice) -> bool {
        voice.utterance.is_some_and(|utterance| {
            self.tracked
                .iter()
                .any(|tracked| tracked.utterance == utterance && tracked.writing)
        })
    }

    /// Frames that may be mixed now, with the next mix written at mix
    /// position `written`: the audio written and not yet mixed, or further
    /// while a voice it has reached plays on and its utterance has written
    /// all its audio ([`Self::still_written`]); none while paused, or while
    /// it keeps track of as many runs as it can.
    pub(crate) fn mixable(&self, written: u64) -> u64 {
        if self.paused || self.segments.len() == SEGMENTS {
            return 0;
        }
        let channels = self.channels;
        self.voices
            .iter()
            .map(|voice| {
                let len = voice_len(voice, channels);
                match voice.state {
                    VoiceState::Retired => 0,
                    _ if self.still_written(voice) => 0,
                    VoiceState::Waiting
                        if voice.trigger >= self.mixed_end && voice.trigger <= self.write_end =>
                    {
                        voice.trigger - self.mixed_end + len
                    }
                    VoiceState::Waiting => 0,
                    VoiceState::Resume { heard } => len - heard,
                    VoiceState::Playing { start } => (start + len).saturating_sub(written),
                }
            })
            .fold(self.unmixed(), u64::max)
    }

    /// Whether the source has an utterance still being written.
    pub(crate) fn writing_any(&self) -> bool {
        self.tracked.iter().any(|tracked| tracked.writing)
    }

    /// Starts the voices the next mix reaches, which takes `data` frames of
    /// the source's own audio to mix position `written` onward: a voice
    /// resuming starts with the first of them, and a waiting voice where its
    /// place is among them, or at their end when that is all the audio
    /// written and its utterance has no more to write
    /// ([`Self::still_written`]).
    fn start_voices(&mut self, written: u64, data: u64) {
        let (mixed_end, write_end) = (self.mixed_end, self.write_end);
        for index in 0..self.voices.len() {
            let writing = self.still_written(&self.voices[index]);
            let voice = &mut self.voices[index];
            match voice.state {
                VoiceState::Resume { heard } => {
                    voice.state = VoiceState::Playing {
                        start: written.saturating_sub(heard),
                    };
                }
                VoiceState::Waiting
                    if voice.trigger >= mixed_end
                        && (voice.trigger < mixed_end + data
                            || (voice.trigger == write_end
                                && mixed_end + data == write_end
                                && !writing)) =>
                {
                    voice.state = VoiceState::Playing {
                        start: written + (voice.trigger - mixed_end),
                    };
                }
                VoiceState::Waiting | VoiceState::Playing { .. } | VoiceState::Retired => {}
            }
        }
    }

    /// Adds the playing voices' frames for mix positions `written` onward
    /// to `mix`.
    fn mix_voices(&self, written: u64, mix: &mut [f32]) {
        let channels = self.channels;
        let end = written + (mix.len() / channels.max(1)) as u64;
        for voice in &self.voices {
            let VoiceState::Playing { start } = voice.state else {
                continue;
            };
            let from = start.max(written);
            let to = (start + voice_len(voice, channels)).min(end);
            if from >= to {
                continue;
            }
            let out = usize::try_from(from - written).unwrap_or(usize::MAX) * channels;
            let input = usize::try_from(from - start).unwrap_or(usize::MAX) * channels;
            let count = usize::try_from(to - from).unwrap_or(usize::MAX) * channels;
            for (sample, frame) in mix[out..out + count]
                .iter_mut()
                .zip(&voice.frames[input..input + count])
            {
                *sample += frame * voice.gain;
            }
        }
    }

    /// Mixes the source's next frames, up to `frames`, into `mix`, which
    /// goes to mix positions from `written`, recording which of its frames
    /// went where.
    pub(crate) fn mix_into(&mut self, frames: u64, written: u64, mix: &mut [f32]) {
        if self.mixable(written) == 0 {
            return;
        }
        let channels = self.channels;
        self.resumed = false;
        let take = self.unmixed().min(frames);
        self.start_voices(written, take);
        self.mix_voices(written, mix);
        if take == 0 {
            return;
        }
        let offset = self.sample_index(self.mixed_end);
        let count = usize::try_from(take).unwrap_or(usize::MAX) * channels;
        for (out, sample) in mix.iter_mut().zip(self.data.range(offset..offset + count)) {
            *out += sample;
        }
        let source_start = self.mixed_end;
        match self.segments.back_mut() {
            Some(last)
                if last.mix_start + last.len == written
                    && last.source_start + last.len == source_start =>
            {
                last.len += take;
            }
            _ => self.segments.push_back(Segment {
                source_start,
                mix_start: written,
                len: take,
            }),
        }
        self.mixed_end += take;
    }

    /// Holds the source where it is, or lets it go on.
    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.resumed = self.paused && !paused;
        self.paused = paused;
    }

    /// Whether an utterance is in the middle of playing: some of its frames
    /// have already gone to the device, and more are still being written or
    /// waiting to be mixed. An utterance whose first frames have not been
    /// mixed yet is starting, not in the middle.
    pub(crate) fn mid_utterance(&self) -> bool {
        if self.paused || self.resumed {
            return false;
        }
        self.tracked.iter().any(|tracked| {
            self.mixed_end > tracked.start && (tracked.writing || self.unmixed() > 0)
        })
    }

    /// Whether any start waits for the device's clock.
    pub(crate) fn awaits_clock(&self) -> bool {
        !self.awaiting_clock.is_empty()
    }

    /// Reports, while the ring has room, each start the device's clock,
    /// at mix position `clock`, has passed, and when: `age` before `at`,
    /// plus the frames since at `rate`. Returns whether it sent any.
    pub(crate) fn heard_by_clock(
        &mut self,
        source: usize,
        clock: u64,
        rate: f64,
        age: std::time::Duration,
        at: Instant,
        out: &mut Producer<Outbound>,
    ) -> bool {
        let mut sent = false;
        let mut index = 0;
        while index < self.awaiting_clock.len() {
            let (utterance, trace_id, mix) = self.awaiting_clock[index];
            if mix >= clock || out.is_full() {
                index += 1;
                continue;
            }
            // The clock was at `clock` `age` ago, and passed the frame
            // `clock - mix` frames before that.
            #[expect(
                clippy::cast_precision_loss,
                reason = "a few seconds of frames, far inside f64's exact range"
            )]
            let behind = std::time::Duration::from_secs_f64((clock - mix) as f64 / rate);
            let _ = out.push(Outbound::Event {
                source,
                event: PlaybackEvent::Heard {
                    utterance,
                    trace_id,
                    ago: age + behind,
                },
                at,
            });
            self.awaiting_clock.swap_remove(index);
            sent = true;
        }
        sent
    }

    /// Forgets every start waiting for the device's clock: the device has
    /// stopped, and its clock starts again from zero.
    pub(crate) fn forget_clock(&mut self) {
        self.awaiting_clock.clear();
    }
}

/// A voice's length in frames of `channels` channels.
fn voice_len(voice: &Voice, channels: usize) -> u64 {
    (voice.frames.len() / channels.max(1)) as u64
}
