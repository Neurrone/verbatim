//! The mixer (decision D17): one audio thread that sums every source into
//! the device and reports when each utterance and index mark is heard.
//!
//! Positions. Each source numbers its frames from zero as they are written
//! (source positions); the mixer numbers the frames it writes to the device
//! (mix positions). When the mixer takes frames from a source it records a
//! segment saying which source positions went to which mix positions. The
//! device reports how many of the written frames are still queued, so the
//! number played is the number written minus the number queued, and a
//! source position has been heard once its mix position has been played.
//!
//! Retention. A source keeps its frames until they have played, not merely
//! until they were written to the device. So when the device's queue must
//! be discarded (an interrupt, a device that failed, a request to reopen)
//! the mixer rewinds to what was played and mixes the other sources again
//! from there, and nothing they had queued is lost.
//!
//! Endings. Every utterance a source registers ends exactly once with a
//! [`PlaybackEvent::Ended`]: completed when the device has played its last
//! frame, cancelled by [`Source::cancel_all`], or failed by
//! [`Source::fail`] or by the device changing format under it.
//!
//! Sounds (`phase6-design.md`, "Earcons"). A source also mixes sounds over
//! its own audio, each a *voice*: one placed in an utterance
//! ([`Source::sound`]) starts when the source's audio reaches its place
//! and plays on over what follows, and one played at once
//! ([`Source::play`]) starts with the next frames mixed. A voice is
//! counted in mix positions from the frame it started at, so it rewinds
//! with everything else when the device's queue is discarded, resuming
//! where it had been heard, and a cancel ends every voice of the source.

use std::collections::{HashMap, VecDeque};
use std::ops::ControlFlow;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use tracing::warn;
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use crate::convert::Converter;
use crate::{AudioDevice, AudioError, DeviceFormat, PcmFormat, Sound, Waker};

/// How far ahead of the device a source may write, beyond the device's own
/// queue: 40 ms. This is the backpressure that keeps a fast synthesizer
/// from running ahead of what is heard.
const AHEAD_MS: u32 = 40;

/// How much audio must be ready before a stopped device is started, unless
/// the utterance is already complete: 10 ms. Without it, an utterance whose
/// first write is only a few frames (what is left once leading silence is
/// trimmed) starts the device, which plays them and runs dry before the
/// next frames arrive: an audible click.
const START_MS: u32 = 10;

/// The audio thread's longest wait between checks while playing.
const PLAYING_WAIT: Duration = Duration::from_millis(100);

/// The audio thread's longest wait while idle; it also bounds how long a
/// request to reopen the device can go unnoticed.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// How long to wait before trying again to open a device that failed.
const REOPEN_RETRY: Duration = Duration::from_secs(1);

/// Something the mixer reports about one utterance, on its audio thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// The utterance's first frame has played.
    Started {
        /// The utterance.
        utterance: UtteranceId,
        /// Its trace.
        trace_id: TraceId,
    },
    /// Playback has reached an index mark.
    Mark {
        /// The utterance.
        utterance: UtteranceId,
        /// Its trace.
        trace_id: TraceId,
        /// The mark's value, as the source wrote it.
        mark: u64,
    },
    /// The utterance has ended; nothing more is reported about it.
    Ended {
        /// The utterance.
        utterance: UtteranceId,
        /// Its trace.
        trace_id: TraceId,
        /// How it ended.
        ending: UtteranceEnding,
    },
}

/// Receives the mixer's output as it plays (see [`Mixer::start_with_tap`]).
/// Called on the audio thread, so it must be quick.
pub trait AudioTap: Send {
    /// Interleaved frames in `format` that have just played, following the
    /// frames of the previous call.
    fn played(&mut self, samples: &[f32], format: DeviceFormat);
}

/// Receives a source's [`PlaybackEvent`]s. Called on the audio thread, so
/// it must be quick and must not call back into the mixer.
pub type PlaybackListener = Arc<dyn Fn(PlaybackEvent) + Send + Sync>;

/// The running mixer. Dropping it stops the audio thread, ending every
/// utterance still registered as cancelled.
pub struct Mixer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// A producer of audio: one stream of utterances played one after another.
/// Cloning gives another handle to the same source.
#[derive(Clone)]
pub struct Source {
    shared: Arc<Shared>,
    id: u64,
    converter: Arc<Mutex<Option<ActiveConverter>>>,
}

/// A source's converter, tied to the device format and utterance it was
/// built for.
struct ActiveConverter {
    generation: u64,
    utterance: UtteranceId,
    converter: Converter,
}

struct Shared {
    state: Mutex<State>,
    /// Writers waiting for room, and requesters waiting for completion.
    changed: Condvar,
    waker: Waker,
}

struct State {
    format: DeviceFormat,
    /// Bumped whenever the device format changes, so converters rebuild.
    generation: u64,
    sources: HashMap<u64, SourceState>,
    next_source: u64,
    requests: VecDeque<(u64, Request)>,
    next_request: u64,
    completed_request: u64,
    underruns: u64,
    shutdown: bool,
}

enum Request {
    CancelAll {
        source: u64,
    },
    Pause {
        source: u64,
        paused: bool,
    },
    Fail {
        source: u64,
        utterance: UtteranceId,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Fires when the frame at the position has played.
    Start,
    /// Fires when every frame before the position has played.
    Mark(u64),
    /// Fires when every frame before the position has played.
    End,
}

struct Pending {
    position: u64,
    utterance: UtteranceId,
    trace_id: TraceId,
    kind: Kind,
}

/// One utterance registered and not yet ended.
struct Tracked {
    utterance: UtteranceId,
    trace_id: TraceId,
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
}

struct SourceState {
    listener: PlaybackListener,
    /// Channels per frame of the device format the data is in.
    channels: usize,
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
    paused: bool,
    /// Resumed from a pause, and not mixed from since: the device running
    /// dry meanwhile was the pause, not an underrun.
    resumed: bool,
    /// Sounds mixed over the source's audio, until they have played.
    voices: Vec<Voice>,
}

impl Mixer {
    /// Starts the audio thread and opens `device` on it.
    ///
    /// # Errors
    ///
    /// Returns the device's error when it cannot be opened, or
    /// [`AudioError::Device`] when the thread cannot start.
    pub fn start(device: Box<dyn AudioDevice>) -> Result<Self, AudioError> {
        Self::start_inner(device, None)
    }

    /// Like [`Mixer::start`], with `tap` given every frame once it has
    /// played (decision D16): a copy of exactly what was heard, in order,
    /// for recording. Frames written to the device and then discarded (an
    /// interrupt, a reopen) never reach it.
    ///
    /// # Errors
    ///
    /// As [`Mixer::start`].
    pub fn start_with_tap(
        device: Box<dyn AudioDevice>,
        tap: Box<dyn AudioTap>,
    ) -> Result<Self, AudioError> {
        Self::start_inner(device, Some(tap))
    }

    fn start_inner(
        mut device: Box<dyn AudioDevice>,
        tap: Option<Box<dyn AudioTap>>,
    ) -> Result<Self, AudioError> {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                // Replaced by the opened device's format before anything
                // can use it: `start` returns only after the open.
                format: DeviceFormat {
                    sample_rate: 0,
                    channels: 0,
                    buffer_frames: 0,
                },
                generation: 0,
                sources: HashMap::new(),
                next_source: 0,
                requests: VecDeque::new(),
                next_request: 1,
                completed_request: 0,
                underruns: 0,
                shutdown: false,
            }),
            changed: Condvar::new(),
            waker: device.waker(),
        });
        let (opened_tx, opened_rx) = mpsc::channel();
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("verbatim-audio".to_owned())
                .spawn(move || match device.open() {
                    Ok(format) => {
                        shared.lock().format = format;
                        let _ = opened_tx.send(Ok(()));
                        run(&shared, device.as_mut(), tap);
                    }
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                    }
                })
                .map_err(|error| AudioError::Device(format!("start audio thread: {error}")))?
        };
        let opened = opened_rx.recv().unwrap_or_else(|_| {
            Err(AudioError::Device(
                "audio thread exited while opening".to_owned(),
            ))
        });
        if let Err(error) = opened {
            let _ = thread.join();
            return Err(error);
        }
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Adds a source whose playback events go to `listener`.
    pub fn add_source(&self, listener: PlaybackListener) -> Source {
        let mut state = self.shared.lock();
        let id = state.next_source;
        state.next_source += 1;
        let channels = usize::from(state.format.channels);
        state.sources.insert(
            id,
            SourceState {
                listener,
                channels,
                data: VecDeque::new(),
                data_start: 0,
                write_end: 0,
                mixed_end: 0,
                segments: VecDeque::new(),
                events: VecDeque::new(),
                tracked: Vec::new(),
                paused: false,
                resumed: false,
                voices: Vec::new(),
            },
        );
        Source {
            shared: Arc::clone(&self.shared),
            id,
            converter: Arc::new(Mutex::new(None)),
        }
    }

    /// The device format the mixer renders in now.
    #[must_use]
    pub fn format(&self) -> DeviceFormat {
        self.shared.lock().format
    }

    /// How many times the device ran dry in the middle of an utterance: a
    /// gap the listener could hear. Used to size the device buffer.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.shared.lock().underruns
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.changed.notify_all();
        (self.shared.waker)();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// Queues a request for the audio thread and waits until it has been
    /// carried out, so that whatever the caller does next happens after it.
    fn request(&self, request: Request) {
        let mut state = self.lock();
        if state.shutdown {
            return;
        }
        let number = state.next_request;
        state.next_request += 1;
        state.requests.push_back((number, request));
        (self.waker)();
        while state.completed_request < number && !state.shutdown {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Source {
    /// Registers an utterance that will be written next. From here until its
    /// [`PlaybackEvent::Ended`], the mixer owns its ending.
    pub fn register(&self, utterance: UtteranceId, trace_id: TraceId) {
        let mut state = self.shared.lock();
        if state.shutdown {
            return;
        }
        if let Some(source) = state.sources.get_mut(&self.id) {
            let start = source.write_end;
            source.tracked.push(Tracked {
                utterance,
                trace_id,
                start,
                writing: true,
                started: false,
            });
        }
    }

    /// Writes PCM for `utterance`, blocking while the source is too far
    /// ahead of playback.
    ///
    /// Returns `Break` when the utterance no longer accepts audio: it was
    /// cancelled or failed, or it was never registered.
    pub fn write(
        &self,
        utterance: UtteranceId,
        format: PcmFormat,
        samples: &[i16],
    ) -> ControlFlow<()> {
        let mut frames = Vec::new();
        {
            let (generation, device) = {
                let state = self.shared.lock();
                (state.generation, state.format)
            };
            let mut slot = lock(&self.converter);
            let reusable = slot.as_ref().is_some_and(|active| {
                active.generation == generation && active.converter.source() == format
            });
            if let Some(active) = slot.as_mut()
                && reusable
                && active.utterance != utterance
            {
                // A new utterance: whatever the last one left in the
                // converter (it was cut off before finishing) is dropped.
                active.converter.finish(&mut Vec::new());
                active.utterance = utterance;
            }
            if !reusable {
                match Converter::new(format, device) {
                    Ok(converter) => {
                        *slot = Some(ActiveConverter {
                            generation,
                            utterance,
                            converter,
                        });
                    }
                    Err(error) => {
                        drop(slot);
                        self.fail(utterance, error.to_string());
                        return ControlFlow::Break(());
                    }
                }
            }
            if let Some(active) = slot.as_mut() {
                active.converter.push(samples, &mut frames);
            }
        }
        self.append(utterance, &frames)
    }

    /// Records index mark `mark` at the current end of `utterance`'s audio.
    pub fn mark(&self, utterance: UtteranceId, mark: u64) {
        // The mark goes after the frames converted so far. A few frames can
        // still be inside the resampler; they belong before the mark, so the
        // mark is heard up to a few milliseconds early.
        let mut state = self.shared.lock();
        if let Some(source) = state.sources.get_mut(&self.id)
            && let Some(tracked) = source.writing(utterance)
        {
            let trace_id = tracked.trace_id;
            let position = source.write_end;
            source.events.push_back(Pending {
                position,
                utterance,
                trace_id,
                kind: Kind::Mark(mark),
            });
        }
    }

    /// Places `sound` at the current end of `utterance`'s audio, as
    /// [`mark`](Self::mark) places a mark: it starts when playback reaches
    /// that place and plays on over the audio that follows, at `gain` (1.0
    /// for as recorded). It belongs to the utterance: failing the utterance
    /// drops it, and [`cancel_all`](Self::cancel_all) stops it. It does not
    /// delay the utterance's ending, which comes when its own audio has
    /// played.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the sound cannot be converted to
    /// the device's format.
    pub fn sound(
        &self,
        utterance: UtteranceId,
        sound: &Sound,
        gain: f32,
    ) -> Result<(), AudioError> {
        self.add_voice(Some(utterance), sound, gain)
    }

    /// Plays `sound` at once, mixed over whatever else is playing, at `gain`
    /// (1.0 for as recorded): for a source that carries only sounds, such as
    /// the earcons of events. [`cancel_all`](Self::cancel_all) stops it.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the sound cannot be converted to
    /// the device's format.
    pub fn play(&self, sound: &Sound, gain: f32) -> Result<(), AudioError> {
        self.add_voice(None, sound, gain)
    }

    fn add_voice(
        &self,
        utterance: Option<UtteranceId>,
        sound: &Sound,
        gain: f32,
    ) -> Result<(), AudioError> {
        let (generation, format) = {
            let state = self.shared.lock();
            (state.generation, state.format)
        };
        // Converted outside the lock, which the audio thread needs.
        let frames = sound.frames_for(format)?;
        {
            let mut state = self.shared.lock();
            if state.shutdown || state.generation != generation {
                // The device changed format meanwhile: these frames cannot
                // be played, and the utterance has failed anyway.
                return Ok(());
            }
            let Some(source) = state.sources.get_mut(&self.id) else {
                return Ok(());
            };
            if let Some(utterance) = utterance
                && source.writing(utterance).is_none()
            {
                return Ok(());
            }
            let trigger = source.write_end;
            source.voices.push(Voice {
                utterance,
                trigger,
                frames,
                gain,
                state: VoiceState::Waiting,
            });
        }
        (self.shared.waker)();
        Ok(())
    }

    /// Ends `utterance`'s audio: it completes once the device has played
    /// its last frame.
    pub fn finish(&self, utterance: UtteranceId) {
        let mut tail = Vec::new();
        if let Some(active) = lock(&self.converter).as_mut()
            && active.utterance == utterance
        {
            active.converter.finish(&mut tail);
        }
        if !tail.is_empty() && self.append(utterance, &tail).is_break() {
            return;
        }
        {
            let mut state = self.shared.lock();
            if let Some(source) = state.sources.get_mut(&self.id)
                && let Some(tracked) = source.writing(utterance)
            {
                tracked.writing = false;
                let trace_id = tracked.trace_id;
                let position = source.write_end;
                source.events.push_back(Pending {
                    position,
                    utterance,
                    trace_id,
                    kind: Kind::End,
                });
            }
        }
        (self.shared.waker)();
    }

    /// Ends `utterance` as failed, discarding whatever of it has not
    /// played. Returns once the failure has been reported.
    pub fn fail(&self, utterance: UtteranceId, reason: String) {
        self.shared.request(Request::Fail {
            source: self.id,
            utterance,
            reason,
        });
    }

    /// Ends every utterance of this source not yet ended as cancelled,
    /// discarding everything not yet played, at once. Utterances registered
    /// after this returns are unaffected.
    pub fn cancel_all(&self) {
        self.shared.request(Request::CancelAll { source: self.id });
    }

    /// Holds this source's audio where it is, or lets it go on. What the
    /// device had queued is taken back, so a pause is heard at once, and
    /// playback events wait with the audio.
    pub fn pause(&self, paused: bool) {
        self.shared.request(Request::Pause {
            source: self.id,
            paused,
        });
    }

    /// Appends device frames for `utterance`, waiting for room.
    fn append(&self, utterance: UtteranceId, frames: &[f32]) -> ControlFlow<()> {
        let mut offset = 0;
        let mut state = self.shared.lock();
        loop {
            if state.shutdown {
                return ControlFlow::Break(());
            }
            let format = state.format;
            let channels = usize::from(format.channels);
            let capacity =
                u64::from(format.buffer_frames) + u64::from(format.sample_rate * AHEAD_MS / 1_000);
            let Some(source) = state.sources.get_mut(&self.id) else {
                return ControlFlow::Break(());
            };
            let Some(tracked) = source.writing(utterance) else {
                return ControlFlow::Break(());
            };
            if offset >= frames.len() {
                return ControlFlow::Continue(());
            }
            if !tracked.started {
                tracked.started = true;
                let trace_id = tracked.trace_id;
                let position = source.write_end;
                source.events.push_back(Pending {
                    position,
                    utterance,
                    trace_id,
                    kind: Kind::Start,
                });
            }
            let unplayed = source.write_end - source.data_start;
            let room = capacity.saturating_sub(unplayed);
            if room == 0 {
                state = self
                    .shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let left = (frames.len() - offset) / channels;
            let take = usize::try_from(room).unwrap_or(usize::MAX).min(left);
            source
                .data
                .extend(&frames[offset..offset + take * channels]);
            source.write_end += take as u64;
            offset += take * channels;
            (self.shared.waker)();
        }
    }
}

impl SourceState {
    /// The tracked utterance `utterance`, if it still accepts frames.
    fn writing(&mut self, utterance: UtteranceId) -> Option<&mut Tracked> {
        self.tracked
            .iter_mut()
            .find(|tracked| tracked.utterance == utterance && tracked.writing)
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

    /// Reports every event whose moment has played, then lets go of the
    /// frames that have played.
    fn fire(&mut self, played: u64, notes: &mut Vec<(PlaybackListener, PlaybackEvent)>) {
        while let Some(pending) = self.events.front() {
            let due = match pending.kind {
                Kind::Start => self.frame_played(pending.position, played),
                Kind::Mark(_) | Kind::End => self.played_up_to(pending.position, played),
            };
            if !due {
                break;
            }
            let pending = self.events.pop_front().expect("front was just read");
            let event = match pending.kind {
                Kind::Start => PlaybackEvent::Started {
                    utterance: pending.utterance,
                    trace_id: pending.trace_id,
                },
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
            };
            notes.push((Arc::clone(&self.listener), event));
        }
        self.release(played);
    }

    /// Drops frames that have played, and voices that have played to their
    /// end.
    fn release(&mut self, played: u64) {
        let channels = self.channels;
        self.voices.retain(|voice| match voice.state {
            VoiceState::Playing { start } => start + voice_len(voice, channels) > played,
            VoiceState::Waiting | VoiceState::Resume { .. } => true,
        });
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
            if segment.len > 0 {
                break;
            }
            self.segments.pop_front();
        }
    }

    /// Forgets mixing done at or after mix position `played`, so those
    /// frames are mixed again: a voice not yet heard waits for its place
    /// again, and one partly heard goes on from where it was.
    fn rewind(&mut self, played: u64) {
        let channels = self.channels;
        self.voices.retain_mut(|voice| {
            if let VoiceState::Playing { start } = voice.state {
                let len = voice_len(voice, channels);
                if start >= played {
                    voice.state = VoiceState::Waiting;
                } else {
                    let heard = (played - start).min(len);
                    if heard == len {
                        return false;
                    }
                    voice.state = VoiceState::Resume { heard };
                }
            }
            true
        });
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

    /// Discards every frame from source position `from` onward, with the
    /// events placed there.
    fn truncate(&mut self, from: u64) {
        let from = from.max(self.data_start);
        let keep = usize::try_from(from - self.data_start).unwrap_or(usize::MAX) * self.channels;
        self.data.truncate(keep);
        self.write_end = from;
        self.mixed_end = self.mixed_end.min(from);
    }

    /// Frames written and not yet mixed.
    fn unmixed(&self) -> u64 {
        self.write_end - self.mixed_end
    }

    /// Frames that may be mixed now, with the next mix written at mix
    /// position `written`: the audio written and not yet mixed, or further
    /// while a voice it has reached plays on; none while paused.
    fn mixable(&self, written: u64) -> u64 {
        if self.paused {
            return 0;
        }
        let channels = self.channels;
        self.voices
            .iter()
            .map(|voice| {
                let len = voice_len(voice, channels);
                match voice.state {
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

    /// Starts the voices the next mix reaches, which takes `data` frames of
    /// the source's own audio to mix position `written` onward: a voice
    /// resuming starts with the first of them, and a waiting voice where its
    /// place is among them, or at their end when that is all the audio
    /// written.
    fn start_voices(&mut self, written: u64, data: u64) {
        let (mixed_end, write_end) = (self.mixed_end, self.write_end);
        for voice in &mut self.voices {
            match voice.state {
                VoiceState::Resume { heard } => {
                    voice.state = VoiceState::Playing {
                        start: written.saturating_sub(heard),
                    };
                }
                VoiceState::Waiting
                    if voice.trigger >= mixed_end
                        && (voice.trigger < mixed_end + data
                            || (voice.trigger == write_end && mixed_end + data == write_end)) =>
                {
                    voice.state = VoiceState::Playing {
                        start: written + (voice.trigger - mixed_end),
                    };
                }
                VoiceState::Waiting | VoiceState::Playing { .. } => {}
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

    /// Whether an utterance is in the middle of playing: some of its frames
    /// have already gone to the device, and more are still being written or
    /// waiting to be mixed. An utterance whose first frames have not been
    /// mixed yet is starting, not in the middle.
    fn mid_utterance(&self) -> bool {
        if self.paused || self.resumed {
            return false;
        }
        self.tracked.iter().any(|tracked| {
            self.mixed_end > tracked.start && (tracked.writing || self.unmixed() > 0)
        })
    }
}

/// A voice's length in frames of `channels` channels.
fn voice_len(voice: &Voice, channels: usize) -> u64 {
    (voice.frames.len() / channels.max(1)) as u64
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The audio thread.
fn run(shared: &Shared, device: &mut dyn AudioDevice, mut tap: Option<Box<dyn AudioTap>>) {
    let mut written: u64 = 0;
    let mut played: u64 = 0;
    // For the tap: mixed frames written to the device and not yet played,
    // from mix position `tapped` on.
    let mut tap_pending: VecDeque<f32> = VecDeque::new();
    let mut tapped: u64 = 0;
    let mut running = false;
    let mut mix: Vec<f32> = Vec::new();
    // The device ran dry while an utterance was part-way through. It is an
    // audible gap if more of that utterance then plays; an utterance whose
    // trailing silence was trimmed also runs the device dry, harmlessly.
    let mut ran_dry = false;
    loop {
        let mut notes: Vec<(PlaybackListener, PlaybackEvent)> = Vec::new();
        let mut state = shared.lock();
        if state.shutdown {
            device.stop();
            end_all(&mut state, &UtteranceEnding::Cancelled, &mut notes);
            drop(state);
            shared.changed.notify_all();
            deliver(notes);
            return;
        }
        let queued = poll_device(device, written, &mut played);
        let mut queued = match queued {
            Ok(queued) => u64::from(queued).min(written - played),
            Err(error) => {
                drop(state);
                recover(shared, device, played, &error);
                written = played;
                tap_pending.clear();
                running = false;
                continue;
            }
        };
        played = written - queued;
        if let Some(tap) = tap.as_mut() {
            feed_tap(
                tap.as_mut(),
                &mut tap_pending,
                &mut tapped,
                played,
                state.format,
            );
        }
        if running && queued == 0 && state.sources.values().any(SourceState::mid_utterance) {
            ran_dry = true;
        }

        // Requests discard the device's queue, so they are carried out
        // against what has actually played.
        if !state.requests.is_empty() {
            if running {
                device.stop();
                running = false;
            }
            written = played;
            tap_pending.clear();
            queued = 0;
            carry_out_requests(&mut state, played, &mut notes);
        }

        for source in state.sources.values_mut() {
            source.fire(played, &mut notes);
        }

        let frames = frames_to_mix(&state, queued, running, written);
        if frames > 0 {
            if ran_dry && state.sources.values().any(SourceState::mid_utterance) {
                state.underruns += 1;
                warn!(
                    target: "verbatim::audio",
                    underruns = state.underruns,
                    "the device ran dry in the middle of an utterance: an audible gap"
                );
            }
            ran_dry = false;
            mix_sources(&mut state, frames, written, &mut mix);
            let result = device
                .write(&mix)
                .and_then(|()| if running { Ok(()) } else { device.start() });
            if let Err(error) = result {
                drop(state);
                deliver(notes);
                recover(shared, device, played, &error);
                written = played;
                tap_pending.clear();
                running = false;
                continue;
            }
            if tap.is_some() {
                tap_pending.extend(&mix);
            }
            written += frames;
            running = true;
        } else if running && queued == 0 {
            device.stop();
            running = false;
        }
        drop(state);
        shared.changed.notify_all();
        deliver(notes);
        device.wait(if running { PLAYING_WAIT } else { IDLE_WAIT });
    }
}

/// How many frames to mix now: as many as the device has room for and some
/// source has, except that a stopped device waits for [`START_MS`] of audio
/// unless what it has is complete (zero then means wait; each write wakes
/// the audio thread).
fn frames_to_mix(state: &State, queued: u64, running: bool, written: u64) -> u64 {
    let room = u64::from(state.format.buffer_frames).saturating_sub(queued);
    let available = state
        .sources
        .values()
        .map(|source| source.mixable(written))
        .max()
        .unwrap_or(0);
    let frames = room.min(available);
    let start_frames = u64::from(state.format.sample_rate * START_MS / 1_000);
    let still_writing = state.sources.values().any(|source| {
        source.mixable(written) > 0 && source.tracked.iter().any(|tracked| tracked.writing)
    });
    if !running && frames < start_frames && still_writing {
        0
    } else {
        frames
    }
}

/// Gives the tap the frames that have played since it was last fed.
fn feed_tap(
    tap: &mut dyn AudioTap,
    pending: &mut VecDeque<f32>,
    tapped: &mut u64,
    played: u64,
    format: DeviceFormat,
) {
    if played > *tapped {
        let count =
            usize::try_from(played - *tapped).unwrap_or(usize::MAX) * usize::from(format.channels);
        let frames: Vec<f32> = pending.drain(..count.min(pending.len())).collect();
        tap.played(&frames, format);
        *tapped = played;
    }
}

/// Carries out every queued request, after the device's queue was
/// discarded: rewinds every source to what had played, then applies the
/// requests in order and tells each requester it is done.
fn carry_out_requests(
    state: &mut State,
    played: u64,
    notes: &mut Vec<(PlaybackListener, PlaybackEvent)>,
) {
    for source in state.sources.values_mut() {
        source.rewind(played);
        source.fire(played, notes);
    }
    while let Some((number, request)) = state.requests.pop_front() {
        carry_out(state, request, notes);
        state.completed_request = number;
    }
}

/// Sums the next `frames` frames of every source into `mix`, recording for
/// each source which of its frames went to mix positions from `written`.
fn mix_sources(state: &mut State, frames: u64, written: u64, mix: &mut Vec<f32>) {
    let channels = usize::from(state.format.channels);
    let frame_count = usize::try_from(frames).unwrap_or(usize::MAX);
    mix.clear();
    mix.resize(frame_count * channels, 0.0);
    for source in state.sources.values_mut() {
        if source.mixable(written) == 0 {
            continue;
        }
        source.resumed = false;
        let take = source.unmixed().min(frames);
        source.start_voices(written, take);
        source.mix_voices(written, mix);
        if take == 0 {
            continue;
        }
        let offset =
            usize::try_from(source.mixed_end - source.data_start).unwrap_or(usize::MAX) * channels;
        let count = usize::try_from(take).unwrap_or(usize::MAX) * channels;
        for (out, sample) in mix
            .iter_mut()
            .zip(source.data.range(offset..offset + count))
        {
            *out += sample;
        }
        let source_start = source.mixed_end;
        match source.segments.back_mut() {
            Some(last)
                if last.mix_start + last.len == written
                    && last.source_start + last.len == source_start =>
            {
                last.len += take;
            }
            _ => source.segments.push_back(Segment {
                source_start,
                mix_start: written,
                len: take,
            }),
        }
        source.mixed_end += take;
    }
    for sample in mix.iter_mut() {
        *sample = sample.clamp(-1.0, 1.0);
    }
}

/// Carries out one request, after the device queue was discarded and every
/// source rewound to what had played.
fn carry_out(
    state: &mut State,
    request: Request,
    notes: &mut Vec<(PlaybackListener, PlaybackEvent)>,
) {
    match request {
        Request::CancelAll { source } => {
            if let Some(source) = state.sources.get_mut(&source) {
                let cut = source.mixed_end;
                source.truncate(cut);
                source.events.clear();
                source.voices.clear();
                for tracked in source.tracked.drain(..) {
                    notes.push((
                        Arc::clone(&source.listener),
                        PlaybackEvent::Ended {
                            utterance: tracked.utterance,
                            trace_id: tracked.trace_id,
                            ending: UtteranceEnding::Cancelled,
                        },
                    ));
                }
            }
        }
        Request::Pause { source, paused } => {
            if let Some(source) = state.sources.get_mut(&source) {
                source.resumed = source.paused && !paused;
                source.paused = paused;
            }
        }
        Request::Fail {
            source,
            utterance,
            reason,
        } => {
            if let Some(source) = state.sources.get_mut(&source)
                && let Some(index) = source
                    .tracked
                    .iter()
                    .position(|tracked| tracked.utterance == utterance)
            {
                let tracked = source.tracked.remove(index);
                // The failing utterance is the one being written, so it is
                // the last in the source: everything of it not yet played
                // goes.
                source.truncate(tracked.start.max(source.mixed_end));
                source
                    .events
                    .retain(|pending| pending.utterance != utterance);
                source
                    .voices
                    .retain(|voice| voice.utterance != Some(utterance));
                notes.push((
                    Arc::clone(&source.listener),
                    PlaybackEvent::Ended {
                        utterance,
                        trace_id: tracked.trace_id,
                        ending: UtteranceEnding::Failed(reason),
                    },
                ));
            }
        }
    }
}

/// Recovers from a device error or a request to reopen: discards what was
/// queued, rewinds every source to what had played, and opens the device
/// again, with the shared state unlocked while the device is opened so
/// writers and requesters are never held up by it. A device that comes back
/// in a different format cannot play the frames already converted for the
/// old one, so every utterance not yet ended fails.
/// How many written frames the device still has queued, with `played`
/// brought up to date first. A device asking to be reopened can still say
/// how much it has played, so that is read before the error that has it
/// reopened: reopening from the position of an earlier poll would write
/// again frames already heard.
fn poll_device(
    device: &mut dyn AudioDevice,
    written: u64,
    played: &mut u64,
) -> Result<u32, AudioError> {
    let reopen = device.needs_reopen();
    let queued = device.queued_frames()?;
    if reopen {
        *played = written - u64::from(queued).min(written - *played);
        return Err(AudioError::Device(
            "the device asked to be reopened".to_owned(),
        ));
    }
    Ok(queued)
}

fn recover(shared: &Shared, device: &mut dyn AudioDevice, played: u64, error: &AudioError) {
    warn!(target: "verbatim::audio", %error, "reopening the audio device");
    device.stop();
    let mut notes = Vec::new();
    {
        let mut state = shared.lock();
        for source in state.sources.values_mut() {
            source.rewind(played);
            source.fire(played, &mut notes);
        }
    }
    deliver(notes);
    let format = loop {
        match device.open() {
            Ok(format) => break format,
            Err(error) => {
                warn!(target: "verbatim::audio", %error, "the audio device could not be opened; trying again");
                if shared.lock().shutdown {
                    return;
                }
                std::thread::sleep(REOPEN_RETRY);
            }
        }
    };
    let mut notes = Vec::new();
    {
        let mut state = shared.lock();
        if (format.sample_rate, format.channels)
            != (state.format.sample_rate, state.format.channels)
        {
            end_all(
                &mut state,
                &UtteranceEnding::Failed("the audio device changed format".to_owned()),
                &mut notes,
            );
            state.generation += 1;
            for source in state.sources.values_mut() {
                source.channels = usize::from(format.channels);
            }
        }
        state.format = format;
    }
    shared.changed.notify_all();
    deliver(notes);
}

/// Ends every utterance not yet ended in every source with `ending`,
/// discarding all audio not yet played.
fn end_all(
    state: &mut State,
    ending: &UtteranceEnding,
    notes: &mut Vec<(PlaybackListener, PlaybackEvent)>,
) {
    for source in state.sources.values_mut() {
        let cut = source.mixed_end;
        source.truncate(cut);
        source.events.clear();
        source.voices.clear();
        for tracked in source.tracked.drain(..) {
            notes.push((
                Arc::clone(&source.listener),
                PlaybackEvent::Ended {
                    utterance: tracked.utterance,
                    trace_id: tracked.trace_id,
                    ending: ending.clone(),
                },
            ));
        }
    }
}

fn deliver(notes: Vec<(PlaybackListener, PlaybackEvent)>) {
    for (listener, event) in notes {
        listener(event);
    }
}
