//! The mixer (decision D17): one audio thread that sums every source into
//! the device and reports when each utterance and index mark is heard.
//!
//! Ownership (`docs/design/ownership-output.md`, "Real-time audio"). The
//! audio thread owns the mixer's state, and nothing outside it reads or
//! writes that state. Everything it takes in comes through queues built
//! before it starts: each producing thread has a lane of its own, a
//! single-producer, single-consumer ring of [records](records::Record) and,
//! for the synth thread, of samples ([`Lane`], [`SpeechLane`]), whose push
//! never waits; a cancel and a pause are counters the speech queue thread
//! sets ([`SourceControl`]). Everything it gives out leaves through one
//! outbound ring to a delivery thread, which runs the playback listeners,
//! logs, and frees what the audio thread is done with. The audio thread
//! takes no lock, waits only on the device, and allocates and frees nothing
//! once the device is open.
//!
//! Wakes are plain events. A producer sets the audio thread's wake event
//! after every push, and the speech queue thread after every cancel and
//! pause. The audio thread sets a source's room event after a pass that
//! let go of its frames and left at least half its room free, or that took
//! at least half its lane's records or took from it when it was full; the
//! synth thread waits on it in a loop when its source or its lane is full.
//! The room event is a level, so a wait that begins after it was set
//! returns at once. The audio thread sets the delivery thread's event after
//! a pass that sent it anything, and the delivery thread wakes the audio
//! thread after draining a full ring.
//!
//! Ordering. The speech queue thread raises a source's cancel generation
//! before it cancels the job's token, and every utterance is registered
//! under the generation current when it was handed on; a pass drains every
//! lane, then reads the generations, then fires events. So a record the
//! synth thread pushed after it saw the token cancelled is taken in the
//! same pass as the raised generation, and an utterance the cancel cut
//! ends cancelled, never completed.
//!
//! Endings. Every utterance a lane registers ends exactly once with a
//! [`PlaybackEvent::Ended`]: completed when the device has played its last
//! frame, cancelled by a raised generation or at shutdown, or failed by
//! [`SpeechLane::fail`] or by the device changing format under it.
//!
//! Sounds (`docs/crates/verbatim-audio.md`). A source also mixes sounds over
//! its own audio, each a *voice*: one placed in an utterance
//! ([`SpeechLane::sound`]) starts when the source's audio reaches its place
//! and plays on over what follows, and one played at once ([`Lane::play`])
//! starts with the next frames mixed. A voice is counted in mix positions
//! from the frame it started at, so it rewinds with everything else when
//! the device's queue is discarded, resuming where it had been heard, and a
//! cancel ends every voice of the source.

mod delivery;
mod records;
mod recover;
mod run;
mod store;

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rtrb::{Producer, RingBuffer};
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

use crate::convert::Converter;
use crate::fallback::Fallback;
use crate::{AudioDevice, AudioError, DeviceFormat, PcmFormat, Signal, Sound, Waker};
use records::{NewVoice, Record};
use run::{AudioLoop, LaneEnd, Pass};
use store::SourceState;

/// How far ahead of the device a source may write, beyond the device's own
/// queue: 40 ms. This is the backpressure that keeps a fast synthesizer
/// from running ahead of what is heard.
const AHEAD_MS: u32 = 40;

/// Records the synth thread's lane holds.
const SPEECH_RECORDS: usize = 1_024;

/// Records a lane that only plays sounds holds: earcons come at most a few
/// a second, so these fill only when the audio thread has stalled.
const SOUND_RECORDS: usize = 256;

/// Records the outbound ring holds.
const OUTBOUND: usize = 1_024;

/// Why an utterance fails when the device comes back in another sample
/// rate or channel count.
const FORMAT_CHANGED: &str = "the audio device changed format";

/// Something the mixer reports about one utterance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackEvent {
    /// The utterance's first frame has played: the device has taken it
    /// from its queue.
    Started {
        /// The utterance.
        utterance: UtteranceId,
        /// Its trace.
        trace_id: TraceId,
    },
    /// The device's own clock has passed the utterance's first frame, `ago`
    /// before the time this is reported with: reported only by a device
    /// with a clock
    /// ([`AudioDevice::clock_position`](crate::AudioDevice::clock_position)),
    /// after [`Started`](Self::Started), unless playback stopped first.
    Heard {
        /// The utterance.
        utterance: UtteranceId,
        /// Its trace.
        trace_id: TraceId,
        /// How long before the report's time the clock passed the frame.
        ago: Duration,
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

/// Receives the mixer's output as it plays (see [`MixerBuilder::tap`]).
/// Called on the audio thread, so `played` must not lock, wait or allocate.
pub trait AudioTap: Send {
    /// The device has opened in `format`, on the audio thread's recovery
    /// path, where the tap may allocate what `played` needs; `wake` is an
    /// event of the device's kind that `played` may set.
    fn opened(&mut self, format: DeviceFormat, wake: &Arc<dyn Signal>) {
        let _ = (format, wake);
    }

    /// Interleaved frames in `format` that have just played, following the
    /// frames of the previous call.
    fn played(&mut self, samples: &[f32], format: DeviceFormat);
}

/// Receives a source's [`PlaybackEvent`]s, each with the time the audio
/// thread read in the pass that reported it. Called on the mixer's delivery
/// thread, so it must be quick and must not call back into the mixer.
pub type PlaybackListener = Arc<dyn Fn(PlaybackEvent, Instant) + Send + Sync>;

/// What the audio thread publishes, and the flag that stops it.
pub(crate) struct Shared {
    /// Ends the audio thread's wait on the device.
    waker: Waker,
    shutdown: AtomicBool,
    /// The format generation, sample rate and channels ([`pack_format`]).
    format: AtomicU64,
    /// The device's buffer, in frames.
    buffer_frames: AtomicU64,
    underruns: AtomicU64,
}

/// One source's counters, shared between the audio thread and the threads
/// that play on it.
pub(crate) struct SourceShared {
    /// Raised by the speech queue thread to cancel every utterance
    /// registered before.
    cancel_generation: AtomicU64,
    /// Set by the speech queue thread to hold the source where it is.
    paused: AtomicBool,
    /// Frames of the source's lane the audio thread has let go of, played
    /// or discarded, in the lane's count.
    released: AtomicU64,
    /// Set when the source or its lane has room again
    /// (`docs/design/ownership-output.md`, "Real-time audio").
    room: Arc<dyn Signal>,
}

/// The format generation's bits in a packed format.
const GENERATION_MASK: u64 = 0xFFFF;

/// Packs the format generation's low bits, the channels and the sample rate
/// into one word, so a producer reads them together.
pub(crate) fn pack_format(generation: u64, format: DeviceFormat) -> u64 {
    ((generation & GENERATION_MASK) << 48)
        | (u64::from(format.channels) << 32)
        | u64::from(format.sample_rate)
}

/// The format generation, as records name it: its low bits.
fn record_generation(generation: u64) -> u64 {
    generation & GENERATION_MASK
}

/// How many frames a source may hold unplayed: the device's queue and
/// [`AHEAD_MS`] beyond it.
pub(crate) fn source_capacity(format: DeviceFormat) -> u64 {
    u64::from(format.buffer_frames) + u64::from(format.sample_rate * AHEAD_MS / 1_000)
}

impl Shared {
    /// The published format generation and device format.
    fn format(&self) -> (u64, DeviceFormat) {
        let packed = self.format.load(Ordering::Acquire);
        let buffer = self.buffer_frames.load(Ordering::Acquire);
        let format = DeviceFormat {
            sample_rate: u32::try_from(packed & 0xFFFF_FFFF).unwrap_or(0),
            channels: u16::try_from((packed >> 32) & 0xFFFF).unwrap_or(0),
            buffer_frames: u32::try_from(buffer).unwrap_or(u32::MAX),
        };
        (packed >> 48, format)
    }

    fn stopped(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }
}

/// A source made with [`MixerBuilder::source`], to make lanes and a
/// control for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceKey(usize);

/// Builds the mixer: its sources, each thread's lane and the speech queue
/// thread's control, all made before the audio thread starts.
pub struct MixerBuilder {
    device: Fallback,
    shared: Arc<Shared>,
    sources: Vec<BuiltSource>,
    lanes: Vec<LaneEnd>,
    tap: Option<Box<dyn AudioTap>>,
    on_delivery_thread: Option<Box<dyn FnOnce() + Send>>,
}

struct BuiltSource {
    listener: PlaybackListener,
    shared: Arc<SourceShared>,
    takes_frames: bool,
}

impl MixerBuilder {
    /// A mixer to play through `device`, which it opens when it starts.
    #[must_use]
    pub fn new(device: Box<dyn AudioDevice>) -> Self {
        let device = Fallback::new(device);
        let shared = Arc::new(Shared {
            waker: device.waker(),
            shutdown: AtomicBool::new(false),
            format: AtomicU64::new(0),
            buffer_frames: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        });
        Self {
            device,
            shared,
            sources: Vec::new(),
            lanes: Vec::new(),
            tap: None,
            on_delivery_thread: None,
        }
    }

    /// Adds a source whose playback events go to `listener`.
    pub fn source(&mut self, listener: PlaybackListener) -> SourceKey {
        self.sources.push(BuiltSource {
            listener,
            shared: Arc::new(SourceShared {
                cancel_generation: AtomicU64::new(0),
                paused: AtomicBool::new(false),
                released: AtomicU64::new(0),
                room: self.device.signal(),
            }),
            takes_frames: false,
        });
        SourceKey(self.sources.len() - 1)
    }

    /// The control of `source`'s cancel generation and pause, for the one
    /// thread that cancels and pauses it.
    #[must_use]
    pub fn control(&self, source: SourceKey) -> SourceControl {
        SourceControl {
            shared: Arc::clone(&self.shared),
            source: Arc::clone(&self.sources[source.0].shared),
        }
    }

    /// A lane that plays sounds on `source`, for one thread: it holds
    /// [`SOUND_RECORDS`] records, and a sound that finds it full is not
    /// played.
    pub fn lane(&mut self, source: SourceKey) -> Lane {
        let (producer, consumer) = RingBuffer::new(SOUND_RECORDS);
        self.lanes.push(LaneEnd {
            source: source.0,
            records: consumer,
            samples: None,
        });
        Lane {
            shared: Arc::clone(&self.shared),
            source: Arc::clone(&self.sources[source.0].shared),
            records: producer,
        }
    }

    /// The lane that writes utterances to `source`, for the one thread that
    /// synthesizes them: a ring of records and a ring of samples, sized for
    /// the source's capacity at the largest format the mixer accepts.
    pub fn speech_lane(&mut self, source: SourceKey) -> SpeechLane {
        let (records, record_end) = RingBuffer::new(SPEECH_RECORDS);
        let (samples, sample_end) = RingBuffer::new(store::SAMPLES);
        self.sources[source.0].takes_frames = true;
        self.lanes.push(LaneEnd {
            source: source.0,
            records: record_end,
            samples: Some(sample_end),
        });
        SpeechLane {
            lane: Lane {
                shared: Arc::clone(&self.shared),
                source: Arc::clone(&self.sources[source.0].shared),
                records,
            },
            samples,
            converter: None,
            pushed: 0,
            utterance: None,
            converted: Vec::new(),
        }
    }

    /// Gives `tap` every frame once it has played (decision D16): a copy of
    /// exactly what was heard, in order, for recording. Frames written to
    /// the device and then discarded (an interrupt, a reopen) never reach
    /// it.
    pub fn tap(&mut self, tap: Box<dyn AudioTap>) {
        self.tap = Some(tap);
    }

    /// Runs `start` on the delivery thread before it delivers anything:
    /// for the application to raise its priority, since it only moves
    /// messages (`docs/design/communication.md`, section 6.1).
    pub fn on_delivery_thread(&mut self, start: Box<dyn FnOnce() + Send>) {
        self.on_delivery_thread = Some(start);
    }

    /// Starts the audio thread and the delivery thread, and opens the
    /// device on the audio thread. When the device cannot be opened, the
    /// mixer plays real-time silence in its place until it asks to be
    /// reopened ([`AudioDevice::open`]).
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Device`] when a thread cannot start.
    pub fn start(self) -> Result<Mixer, AudioError> {
        let Self {
            mut device,
            shared,
            sources,
            lanes,
            tap,
            on_delivery_thread,
        } = self;
        let (out, ring) = RingBuffer::new(OUTBOUND);
        let delivery = device.signal();
        let tap_signal = device.signal();
        let listeners: Vec<PlaybackListener> = sources
            .iter()
            .map(|source| Arc::clone(&source.listener))
            .collect();
        let audio = AudioLoop {
            shared: Arc::clone(&shared),
            sources: sources
                .iter()
                .map(|source| SourceState::new(source.takes_frames))
                .collect(),
            controls: sources
                .iter()
                .map(|source| Arc::clone(&source.shared))
                .collect(),
            lanes,
            out,
            delivery: Arc::clone(&delivery),
            format: DeviceFormat {
                sample_rate: 0,
                channels: 0,
                buffer_frames: 0,
            },
            generation: 0,
            written: 0,
            played: 0,
            running: false,
            clock_base: 0,
            ran_dry: false,
            mix: Vec::new(),
            tap,
            tap_pending: std::collections::VecDeque::new(),
            tapped: 0,
            underruns: 0,
            underruns_sent: 0,
            sent: false,
        };
        let delivery_thread = {
            let waker = shared.waker.clone();
            let delivery = Arc::clone(&delivery);
            std::thread::Builder::new()
                .name("verbatim-audio-events".to_owned())
                .spawn(move || {
                    if let Some(start) = on_delivery_thread {
                        start();
                    }
                    delivery::run(ring, &listeners, delivery.as_ref(), &waker);
                })
                .map_err(|error| AudioError::Device(format!("start delivery thread: {error}")))?
        };
        let (opened_tx, opened_rx) = mpsc::channel();
        let audio_thread = std::thread::Builder::new()
            .name("verbatim-audio".to_owned())
            .spawn(move || {
                let mut audio = audio;
                recover::open(&mut audio, &mut device, &tap_signal);
                let _ = opened_tx.send(());
                audio_thread(audio, &mut device, &tap_signal);
                // The outbound ring's producer has gone with the loop: the
                // delivery thread, woken, finds it abandoned and ends.
                delivery.set();
            });
        let audio_thread = match audio_thread {
            Ok(thread) => thread,
            Err(error) => {
                shared.shutdown.store(true, Ordering::Release);
                let _ = delivery_thread.join();
                return Err(AudioError::Device(format!("start audio thread: {error}")));
            }
        };
        if opened_rx.recv().is_err() {
            let _ = audio_thread.join();
            let _ = delivery_thread.join();
            return Err(AudioError::Device(
                "audio thread exited while opening".to_owned(),
            ));
        }
        Ok(Mixer {
            shared,
            audio: Some(audio_thread),
            delivery: Some(delivery_thread),
        })
    }
}

/// The audio thread: passes, each followed by a wait on the device, the
/// recovery path when the device fails, and shutdown.
fn audio_thread(mut audio: AudioLoop, device: &mut Fallback, tap_signal: &Arc<dyn Signal>) {
    loop {
        match audio.pass(device) {
            Pass::Wait(timeout) => device.wait(timeout),
            Pass::Recover(error) => recover::recover(&mut audio, device, tap_signal, error),
            Pass::Shutdown => {
                recover::shut_down(&mut audio, device);
                return;
            }
        }
    }
}

/// The running mixer. Dropping it stops the audio thread, ending every
/// utterance still registered as cancelled, then the delivery thread once
/// it has delivered every report.
pub struct Mixer {
    shared: Arc<Shared>,
    audio: Option<JoinHandle<()>>,
    delivery: Option<JoinHandle<()>>,
}

impl Mixer {
    /// The device format the mixer renders in now.
    #[must_use]
    pub fn format(&self) -> DeviceFormat {
        self.shared.format().1
    }

    /// How many times the device ran dry in the middle of an utterance: a
    /// gap the listener could hear. Used to size the device buffer.
    #[must_use]
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Acquire)
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        (self.shared.waker)();
        if let Some(thread) = self.audio.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.delivery.take() {
            let _ = thread.join();
        }
    }
}

/// The control of one source's cancel generation and pause, for the
/// speech queue thread. Neither waits for the audio thread: each is one
/// atomic store and a wake, and the audio thread carries it out at its
/// next pass, discarding the device's queue.
#[derive(Clone)]
pub struct SourceControl {
    shared: Arc<Shared>,
    source: Arc<SourceShared>,
}

impl SourceControl {
    /// The source's cancel generation now, to register the next utterance
    /// under.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.source.cancel_generation.load(Ordering::Acquire)
    }

    /// Raises the cancel generation, which ends every utterance registered
    /// under an older one as cancelled, discarding everything of them not
    /// yet played, and stops every voice of the source. Utterances
    /// registered under the generation it raised to are spared. Raise it
    /// before cancelling a job's token, so a record pushed after the token
    /// was seen is ordered after the cancel.
    pub fn cancel(&self) {
        self.source.cancel_generation.fetch_add(1, Ordering::AcqRel);
        (self.shared.waker)();
    }

    /// Holds the source's audio where it is, or lets it go on. What the
    /// device had queued is taken back, so a pause is heard at once, and
    /// playback events wait with the audio.
    pub fn pause(&self, paused: bool) {
        self.source.paused.store(paused, Ordering::Release);
        (self.shared.waker)();
    }
}

/// Why a lane refused a record.
enum Refused {
    /// The mixer has stopped.
    Stopped,
    /// The lane is full; the record is given back.
    Full(Record),
}

/// One thread's lane of sounds to one source. Its pushes never wait: a
/// sound that finds the lane full is refused.
pub struct Lane {
    shared: Arc<Shared>,
    source: Arc<SourceShared>,
    records: Producer<Record>,
}

impl Lane {
    /// Pushes `record` and wakes the audio thread.
    fn push(&mut self, record: Record) -> Result<(), Refused> {
        if self.shared.stopped() {
            return Err(Refused::Stopped);
        }
        self.records.push(record).map_err(|error| match error {
            rtrb::PushError::Full(record) => Refused::Full(record),
        })?;
        (self.shared.waker)();
        Ok(())
    }

    /// `sound` in the device format now, with the format generation to name
    /// it by.
    fn voice(&self, sound: &Sound, gain: f32) -> Result<NewVoice, AudioError> {
        let (generation, format) = self.shared.format();
        Ok(NewVoice {
            frames: sound.frames_for(format)?,
            gain,
            format: generation,
        })
    }

    /// Plays `sound` at once, mixed over whatever else is playing, at `gain`
    /// (1.0 for as recorded): for a source that carries only sounds, such as
    /// the earcons of events. A cancel of the source stops it.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the sound cannot be converted to
    /// the device's format, or when the lane is full or the mixer stopped,
    /// and the sound is not played.
    pub fn play(&mut self, sound: &Sound, gain: f32) -> Result<(), AudioError> {
        let voice = self.voice(sound, gain)?;
        self.push(Record::Play { voice })
            .map_err(|refused| refusal(&refused))
    }

    /// Plays `sound` at once, as [`play`](Self::play) does, as the utterance
    /// `utterance`, which ends when the sound has played to its end, or at
    /// once when it cannot play: for the exit sound, which must be heard
    /// before Verbatim exits.
    ///
    /// # Errors
    ///
    /// As [`play`](Self::play); the utterance then never ends, so the
    /// caller ends it.
    pub fn play_to_end(
        &mut self,
        utterance: UtteranceId,
        trace_id: TraceId,
        sound: &Sound,
        gain: f32,
    ) -> Result<(), AudioError> {
        let voice = self.voice(sound, gain)?;
        self.push(Record::PlayToEnd {
            utterance,
            trace_id,
            voice,
        })
        .map_err(|refused| refusal(&refused))
    }

    /// Pushes `record`, waiting on the source's room event while the lane
    /// is full. `Break` when the mixer has stopped.
    fn push_waiting(&mut self, mut record: Record) -> ControlFlow<()> {
        loop {
            match self.push(record) {
                Ok(()) => return ControlFlow::Continue(()),
                Err(Refused::Stopped) => return ControlFlow::Break(()),
                Err(Refused::Full(back)) => {
                    record = back;
                    self.source.room.wait();
                }
            }
        }
    }
}

/// The error for a refused sound.
fn refusal(refused: &Refused) -> AudioError {
    AudioError::Stream(
        match refused {
            Refused::Stopped => "the mixer has stopped",
            Refused::Full(_) => "the mixer's lane is full",
        }
        .to_owned(),
    )
}

/// The synth thread's lane to the speech source: utterances registered,
/// their audio converted to the device format on this thread, marks,
/// sounds, endings and failures. It keeps the source no more than a device
/// queue and [`AHEAD_MS`] ahead of what has played, waiting on the
/// source's room event when it is.
pub struct SpeechLane {
    lane: Lane,
    samples: Producer<f32>,
    converter: Option<ActiveConverter>,
    /// Frames pushed, in all.
    pushed: u64,
    /// The utterance registered last, with the cancel generation it was
    /// registered under.
    utterance: Option<(UtteranceId, u64)>,
    /// The frames of the write under way, converted.
    converted: Vec<f32>,
}

/// A converter, tied to the format generation and utterance it was built
/// for.
struct ActiveConverter {
    generation: u64,
    utterance: UtteranceId,
    converter: Converter,
}

impl SpeechLane {
    /// Registers `utterance`, which will be written next, under the
    /// source's cancel generation `generation`, the one current when it
    /// was handed on. From here until its [`PlaybackEvent::Ended`], the
    /// mixer owns its ending; one registered after a cancel raised the
    /// generation ends cancelled at once.
    pub fn register(&mut self, utterance: UtteranceId, trace_id: TraceId, generation: u64) {
        self.utterance = Some((utterance, generation));
        let _ = self.lane.push_waiting(Record::Register {
            utterance,
            trace_id,
            generation,
        });
    }

    /// Whether `utterance` still accepts audio: it is the one registered
    /// last, the source has not been cancelled since, and the device format
    /// is the one its audio was converted for.
    fn accepts(&self, utterance: UtteranceId, generation: Option<u64>) -> bool {
        let registered = self.utterance.is_some_and(|(registered, cancel)| {
            registered == utterance
                && self.lane.source.cancel_generation.load(Ordering::Acquire) <= cancel
        });
        let format = generation
            .is_none_or(|generation| record_generation(self.lane.shared.format().0) == generation);
        registered && format && !self.lane.shared.stopped()
    }

    /// Writes PCM for `utterance`, waiting while the source is too far
    /// ahead of playback.
    ///
    /// Returns `Break` when the utterance no longer accepts audio: it was
    /// cancelled or failed, it was never registered, or the mixer stopped.
    pub fn write(
        &mut self,
        utterance: UtteranceId,
        format: PcmFormat,
        samples: &[i16],
    ) -> ControlFlow<()> {
        let (generation, device) = self.lane.shared.format();
        let generation = record_generation(generation);
        let reusable = self.converter.as_ref().is_some_and(|active| {
            active.generation == generation && active.converter.source() == format
        });
        if let Some(active) = self.converter.as_mut()
            && reusable
            && active.utterance != utterance
        {
            // A new utterance: whatever the last one left in the converter
            // (it was cut off before finishing) is dropped.
            active.converter.finish(&mut Vec::new());
            active.utterance = utterance;
        }
        if !reusable {
            match Converter::new(format, device) {
                Ok(converter) => {
                    self.converter = Some(ActiveConverter {
                        generation,
                        utterance,
                        converter,
                    });
                }
                Err(error) => {
                    self.fail(utterance, error.to_string());
                    return ControlFlow::Break(());
                }
            }
        }
        let mut converted = std::mem::take(&mut self.converted);
        converted.clear();
        if let Some(active) = self.converter.as_mut() {
            active.converter.push(samples, &mut converted);
        }
        let flow = self.append(utterance, generation, device, &converted);
        self.converted = converted;
        flow
    }

    /// Pushes `frames`, converted for format generation `generation` of
    /// `format`, in pieces as room allows.
    fn append(
        &mut self,
        utterance: UtteranceId,
        generation: u64,
        format: DeviceFormat,
        frames: &[f32],
    ) -> ControlFlow<()> {
        let channels = usize::from(format.channels.max(1));
        let capacity = source_capacity(format);
        let mut offset = 0;
        loop {
            if !self.accepts(utterance, Some(generation)) {
                return ControlFlow::Break(());
            }
            if offset >= frames.len() {
                return ControlFlow::Continue(());
            }
            let held = self.pushed - self.lane.source.released.load(Ordering::Acquire);
            let room = capacity.saturating_sub(held);
            let left = ((frames.len() - offset) / channels) as u64;
            let wanted = left.min(capacity / 2).max(1);
            let ring = (self.samples.slots() / channels) as u64;
            if room.min(ring) < wanted || self.lane.records.is_full() {
                self.lane.source.room.wait();
                continue;
            }
            let take = room.min(ring).min(left);
            let count = usize::try_from(take).unwrap_or(usize::MAX) * channels;
            if self
                .samples
                .push_entire_slice(&frames[offset..offset + count])
                .is_err()
            {
                self.lane.source.room.wait();
                continue;
            }
            let record = Record::Frames {
                utterance,
                format: generation,
                frames: take,
                samples: count,
            };
            if self.lane.push(record).is_err() {
                return ControlFlow::Break(());
            }
            self.pushed += take;
            offset += count;
        }
    }

    /// Records index mark `mark` at the current end of `utterance`'s audio.
    pub fn mark(&mut self, utterance: UtteranceId, mark: u64) {
        // The mark goes after the frames converted so far. A few frames can
        // still be inside the resampler; they belong before the mark, so the
        // mark is heard up to a few milliseconds early.
        let _ = self.lane.push_waiting(Record::Mark { utterance, mark });
    }

    /// Places `sound` at the current end of `utterance`'s audio, as
    /// [`mark`](Self::mark) places a mark: it starts when playback reaches
    /// that place and plays on over the audio that follows, at `gain` (1.0
    /// for as recorded). Until the utterance is finished it is mixed only
    /// alongside the utterance's own audio, so what follows its place is
    /// heard under it at its place however late it was synthesized: playback
    /// that reaches the place first waits there for that audio, or for
    /// [`finish`](Self::finish). It belongs to the utterance: failing the
    /// utterance drops it, and a cancel stops it. It does not delay the
    /// utterance's ending, which comes when its own audio has played.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the sound cannot be converted to
    /// the device's format.
    pub fn sound(
        &mut self,
        utterance: UtteranceId,
        sound: &Sound,
        gain: f32,
    ) -> Result<(), AudioError> {
        let voice = self.lane.voice(sound, gain)?;
        let _ = self.lane.push_waiting(Record::Sound { utterance, voice });
        Ok(())
    }

    /// Ends `utterance`'s audio: it completes once the device has played
    /// its last frame. The finish is pushed whatever became of the
    /// converter's tail, so an utterance the audio thread still tracks is
    /// always ended.
    pub fn finish(&mut self, utterance: UtteranceId) {
        let mut tail = Vec::new();
        let mut format = None;
        if let Some(active) = self.converter.as_mut()
            && active.utterance == utterance
        {
            active.converter.finish(&mut tail);
            format = Some(active.generation);
        }
        if let Some(generation) = format
            && !tail.is_empty()
        {
            let (published, device) = self.lane.shared.format();
            if record_generation(published) == generation {
                let _ = self.append(utterance, generation, device, &tail);
            }
        }
        let _ = self.lane.push_waiting(Record::Finish { utterance });
    }

    /// Ends `utterance` as failed, discarding whatever of it has not
    /// played.
    pub fn fail(&mut self, utterance: UtteranceId, reason: String) {
        let _ = self.lane.push_waiting(Record::Fail { utterance, reason });
    }
}
