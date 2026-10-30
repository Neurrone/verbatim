//! One pass of the audio thread (`docs/design/ownership-output.md`,
//! "Real-time audio"): it drains every lane, then reads each source's
//! cancel generation and pause, then fires events, then mixes and writes
//! the device. It takes no lock, waits on nothing, allocates nothing and
//! frees nothing; `cargo xtask ci` holds this module to the audio rule
//! (`xtask/src/ownership.rs`). Its thread waits only in the device's own
//! wait, between passes, and recovers from a device error on the recovery
//! path (`recover.rs`), outside this module.
//!
//! The order of a pass is what makes a cancel exact: a record pushed after
//! the synth thread saw a cancel is drained in the same pass as the raised
//! generation it follows, since the queue thread raises the generation
//! before it cancels the job's token, so an utterance that the cancel cut
//! never ends completed.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use rtrb::{Consumer, Producer};

use super::records::{Outbound, Record};
use super::store::SourceState;
use super::{Shared, SourceShared, source_capacity};
use crate::{AudioDevice, AudioError, AudioTap, DeviceFormat, Signal};

/// How much audio must be ready before a stopped device is started, unless
/// the utterance is already complete: 10 ms. Without it, an utterance whose
/// first write is only a few frames (what is left once leading silence is
/// trimmed) starts the device, which plays them and runs dry before the
/// next frames arrive: an audible click.
const START_MS: u32 = 10;

/// The audio thread's longest wait between checks while playing.
const PLAYING_WAIT: Duration = Duration::from_millis(100);

/// The audio thread's wait while idle: until a producer, a cancel, a pause
/// or the device wakes it. A device that will ask to be reopened ends the
/// wait itself ([`AudioDevice::needs_reopen`]), so idle the thread does not
/// wake to look.
const IDLE_WAIT: Duration = Duration::MAX;

/// What the audio thread does after a pass.
pub(crate) enum Pass {
    /// Waits on the device, for at most this long.
    Wait(Duration),
    /// Recovers the device: it failed with the error, or asked to be
    /// reopened (`None`).
    Recover(Option<AudioError>),
    /// Stops: the mixer is being dropped.
    Shutdown,
}

/// One producer's lane, as the audio thread takes from it.
pub(crate) struct LaneEnd {
    /// The source it plays on.
    pub(crate) source: usize,
    pub(crate) records: Consumer<Record>,
    /// The samples its frames records name, for the synth thread's lane.
    pub(crate) samples: Option<Consumer<f32>>,
}

/// The audio thread's state, which nothing else reads or writes.
pub(crate) struct AudioLoop {
    pub(crate) shared: Arc<Shared>,
    pub(crate) sources: Vec<SourceState>,
    /// Each source's atomics, by the same index.
    pub(crate) controls: Vec<Arc<SourceShared>>,
    pub(crate) lanes: Vec<LaneEnd>,
    pub(crate) out: Producer<Outbound>,
    /// The delivery thread's event.
    pub(crate) delivery: Arc<dyn Signal>,
    pub(crate) format: DeviceFormat,
    /// The format generation, as records name it.
    pub(crate) generation: u64,
    /// Mix positions: frames written to the device, and played.
    pub(crate) written: u64,
    pub(crate) played: u64,
    pub(crate) running: bool,
    /// The mix position at which the device's clock last started from zero.
    pub(crate) clock_base: u64,
    /// The device ran dry while an utterance was part-way through. It is an
    /// audible gap if more of that utterance then plays; an utterance whose
    /// trailing silence was trimmed also runs the device dry, harmlessly.
    pub(crate) ran_dry: bool,
    pub(crate) mix: Vec<f32>,
    pub(crate) tap: Option<Box<dyn AudioTap>>,
    /// For the tap: mixed frames written to the device and not yet played,
    /// from mix position `tapped` on.
    pub(crate) tap_pending: VecDeque<f32>,
    pub(crate) tapped: u64,
    pub(crate) underruns: u64,
    pub(crate) underruns_sent: u64,
    /// Something went into the outbound ring this pass.
    pub(crate) sent: bool,
}

impl AudioLoop {
    /// One pass, ending in what the thread does next.
    pub(crate) fn pass(&mut self, device: &mut dyn AudioDevice) -> Pass {
        if self.shared.shutdown.load(Ordering::Acquire) {
            return Pass::Shutdown;
        }
        let at = Instant::now();
        // A device asking to be reopened can still say how much it has
        // played, so that is read before it is reopened: reopening from the
        // position of an earlier poll would write again frames already
        // heard.
        let reopen = device.needs_reopen();
        let queued = match device.queued_frames() {
            Ok(queued) => queued,
            Err(error) => return Pass::Recover(Some(error)),
        };
        let mut queued = u64::from(queued).min(self.written - self.played);
        self.played = self.written - queued;
        if reopen {
            return Pass::Recover(None);
        }
        self.feed_tap();
        if self.running && queued == 0 && self.sources.iter().any(SourceState::mid_utterance) {
            self.ran_dry = true;
        }

        self.drain(device, &mut queued);
        self.read_controls(device, &mut queued);
        self.fire(at);
        self.heard_by_clock(device, at);
        self.publish_room();

        let frames = self.frames_to_mix(queued);
        if frames > 0 {
            self.note_underrun();
            self.mix_sources(frames);
            if !self.running {
                // The device's clock starts again from zero at this frame.
                self.clock_base = self.written;
            }
            let running = self.running;
            let result = device
                .write(&self.mix)
                .and_then(|()| if running { Ok(()) } else { device.start() });
            if let Err(error) = result {
                self.end_pass();
                return Pass::Recover(Some(error));
            }
            if self.tap.is_some() {
                self.tap_pending.extend(self.mix.iter().copied());
            }
            self.written += frames;
            self.running = true;
        } else if self.running && queued == 0 {
            device.stop();
            self.running = false;
            self.forget_clock();
        }
        self.end_pass();
        Pass::Wait(if self.running {
            PLAYING_WAIT
        } else {
            IDLE_WAIT
        })
    }

    /// Wakes the delivery thread when this pass sent it anything.
    pub(crate) fn end_pass(&mut self) {
        if self.sent {
            self.sent = false;
            self.delivery.set();
        }
    }

    /// Gives the tap the frames that have played since it was last fed.
    fn feed_tap(&mut self) {
        let Some(tap) = self.tap.as_mut() else {
            return;
        };
        if self.played > self.tapped {
            let count = usize::try_from(self.played - self.tapped).unwrap_or(usize::MAX)
                * usize::from(self.format.channels);
            let count = count.min(self.tap_pending.len());
            tap.played(&self.tap_pending.make_contiguous()[..count], self.format);
            self.tap_pending.drain(..count);
            self.tapped = self.played;
        }
    }

    /// Takes every record its source has room for from every lane, in each
    /// lane's order. A lane's source gets its room event when the pass took
    /// any record and left the lane at most half full: a level, read after
    /// the drain, so a producer that found the lane full is woken by the
    /// first pass that leaves it half free, whenever it filled it.
    fn drain(&mut self, device: &mut dyn AudioDevice, queued: &mut u64) {
        for index in 0..self.lanes.len() {
            let source = self.lanes[index].source;
            let capacity = self.lanes[index].records.buffer().capacity();
            let mut took = 0;
            while let Ok(record) = self.lanes[index].records.peek() {
                if !self.admits(source, record) {
                    break;
                }
                let Ok(record) = self.lanes[index].records.pop() else {
                    break;
                };
                took += 1;
                self.apply(index, source, record, device, queued);
            }
            if took > 0 && self.lanes[index].records.slots() <= capacity / 2 {
                self.controls[source].room.set();
            }
        }
    }

    /// Whether source `source`'s store, and the outbound ring, have room
    /// for what `record` adds.
    fn admits(&self, source: usize, record: &Record) -> bool {
        let store = &self.sources[source];
        let ring = !self.out.is_full();
        match record {
            Record::Register { .. } => store.admits_register(),
            Record::Frames { samples, .. } => store.admits_frames(*samples),
            Record::Mark { .. } | Record::Finish { .. } => store.admits_event(),
            Record::Sound { .. } | Record::Play { .. } => ring && store.admits_voice(),
            Record::PlayToEnd { .. } => ring && store.admits_voice() && store.admits_register(),
            Record::Fail { .. } => ring && store.admits_event(),
        }
    }

    /// Carries out `record`, taken from lane `lane` for source `source`.
    fn apply(
        &mut self,
        lane: usize,
        source: usize,
        record: Record,
        device: &mut dyn AudioDevice,
        queued: &mut u64,
    ) {
        let generation = self.generation;
        let current = |format: u64| format == generation;
        match record {
            Record::Register {
                utterance,
                trace_id,
                generation: registered,
            } => self.sources[source].register(utterance, trace_id, registered),
            Record::Frames {
                utterance,
                format,
                frames,
                samples,
            } => {
                let current = current(format);
                let store = &mut self.sources[source];
                match self.lanes[lane]
                    .samples
                    .as_mut()
                    .map(|ring| ring.read_chunk(samples))
                {
                    Some(Ok(chunk)) => {
                        store.frames(utterance, frames, chunk.as_slices(), current);
                        chunk.commit_all();
                    }
                    // A lane pushes its samples before the record that
                    // names them, so they are there; were they not, the
                    // frames are counted let go of, as discarded.
                    _ => store.frames(utterance, frames, (&[], &[]), false),
                }
            }
            Record::Mark { utterance, mark } => self.sources[source].mark(utterance, mark),
            Record::Sound { utterance, voice } => {
                let current = current(voice.format);
                self.sources[source].voice(Some(utterance), voice, current, &mut self.out);
                self.sent = true;
            }
            Record::Play { voice } => {
                let current = current(voice.format);
                self.sources[source].voice(None, voice, current, &mut self.out);
                self.sent = true;
            }
            Record::PlayToEnd {
                utterance,
                trace_id,
                voice,
            } => {
                let current = current(voice.format);
                self.sources[source].play_to_end(
                    utterance,
                    trace_id,
                    voice,
                    current,
                    &mut self.out,
                );
                self.sent = true;
            }
            Record::Finish { utterance } => self.sources[source].finish(utterance),
            Record::Fail { utterance, reason } => {
                // A failure discards the device's queue, so what of the
                // utterance had been queued is not heard either.
                self.discard(device, queued);
                if let Some(reason) = self.sources[source].fail(utterance, reason) {
                    let _ = self.out.push(Outbound::Reason(reason));
                    self.sent = true;
                }
            }
        }
    }

    /// Reads each source's cancel generation and pause: a raised
    /// generation or a changed pause discards the device's queue, then the
    /// cancel or the pause is applied against what has played.
    fn read_controls(&mut self, device: &mut dyn AudioDevice, queued: &mut u64) {
        let changed = self
            .sources
            .iter()
            .zip(&self.controls)
            .any(|(store, control)| {
                control.cancel_generation.load(Ordering::Acquire) > store.generation
                    || control.paused.load(Ordering::Acquire) != store.paused
            });
        if !changed {
            return;
        }
        self.discard(device, queued);
        for (store, control) in self.sources.iter_mut().zip(&self.controls) {
            let generation = control.cancel_generation.load(Ordering::Acquire);
            if generation > store.generation {
                store.cancel_older(generation);
                store.generation = generation;
            }
            let paused = control.paused.load(Ordering::Acquire);
            if paused != store.paused {
                store.set_paused(paused);
            }
        }
    }

    /// Discards the device's queue and rewinds every source to what has
    /// played, so a request is carried out against what was heard.
    pub(crate) fn discard(&mut self, device: &mut dyn AudioDevice, queued: &mut u64) {
        if self.running {
            device.stop();
            self.running = false;
            self.forget_clock();
        }
        self.written = self.played;
        self.tap_pending.clear();
        *queued = 0;
        let played = self.played;
        for store in &mut self.sources {
            store.rewind(played);
        }
    }

    /// Sends every event whose moment has played, and what each source is
    /// done with, voices retired in this pass included, while the ring has
    /// room.
    pub(crate) fn fire(&mut self, at: Instant) {
        let played = self.played;
        for (index, store) in self.sources.iter_mut().enumerate() {
            let fired = store.fire(index, played, at, &mut self.out);
            let flushed = store.flush_retired(&mut self.out);
            self.sent |= fired || flushed;
        }
    }

    /// Reports, for each utterance whose start has played, whether the
    /// device's clock has passed its first frame, and when, from one reading
    /// of the clock, read only while the device runs.
    fn heard_by_clock(&mut self, device: &mut dyn AudioDevice, at: Instant) {
        if !self.running || !self.sources.iter().any(SourceState::awaits_clock) {
            return;
        }
        let Some(reading) = device.clock_position() else {
            return;
        };
        let rate = f64::from(self.format.sample_rate.max(1));
        let clock = self.clock_base + reading.frames;
        for (index, store) in self.sources.iter_mut().enumerate() {
            self.sent |= store.heard_by_clock(index, clock, rate, reading.age, at, &mut self.out);
        }
    }

    /// Publishes how many frames each source has let go of, and sets the
    /// room event of a source that let go of frames this pass and has at
    /// least half its room free: a level, so a producer that waits after
    /// this still finds it set.
    fn publish_room(&mut self) {
        let capacity = source_capacity(self.format);
        for (store, control) in self.sources.iter_mut().zip(&self.controls) {
            if !store.released_now {
                continue;
            }
            store.released_now = false;
            control.released.store(store.released, Ordering::Release);
            if capacity.saturating_sub(store.held()) >= capacity / 2 {
                control.room.set();
            }
        }
    }

    /// Forgets every start waiting for the device's clock.
    pub(crate) fn forget_clock(&mut self) {
        for store in &mut self.sources {
            store.forget_clock();
        }
    }

    /// How many frames to mix now: as many as the device has room for and
    /// some source has, except that a stopped device waits for
    /// [`START_MS`] of audio unless what it has is complete (zero then
    /// means wait; each record wakes the audio thread).
    fn frames_to_mix(&self, queued: u64) -> u64 {
        let room = u64::from(self.format.buffer_frames).saturating_sub(queued);
        let written = self.written;
        let available = self
            .sources
            .iter()
            .map(|store| store.mixable(written))
            .max()
            .unwrap_or(0);
        let frames = room.min(available);
        let start_frames = u64::from(self.format.sample_rate * START_MS / 1_000);
        let still_writing = self
            .sources
            .iter()
            .any(|store| store.mixable(written) > 0 && store.writing_any());
        if !self.running && frames < start_frames && still_writing {
            0
        } else {
            frames
        }
    }

    /// Counts an underrun when the device ran dry and more of an utterance
    /// it was part-way through is about to play, publishes the count, and
    /// sends it for the delivery thread to report while the ring has room.
    fn note_underrun(&mut self) {
        if self.ran_dry && self.sources.iter().any(SourceState::mid_utterance) {
            self.underruns += 1;
            self.shared
                .underruns
                .store(self.underruns, Ordering::Release);
        }
        self.ran_dry = false;
        if self.underruns > self.underruns_sent && !self.out.is_full() {
            let _ = self.out.push(Outbound::Underrun(self.underruns));
            self.underruns_sent = self.underruns;
            self.sent = true;
        }
    }

    /// Sums the next `frames` frames of every source into the mix.
    fn mix_sources(&mut self, frames: u64) {
        let channels = usize::from(self.format.channels);
        let samples = usize::try_from(frames).unwrap_or(usize::MAX) * channels;
        self.mix.clear();
        self.mix.resize(samples, 0.0);
        let written = self.written;
        for store in &mut self.sources {
            store.mix_into(frames, written, &mut self.mix);
        }
        for sample in &mut self.mix {
            *sample = sample.clamp(-1.0, 1.0);
        }
    }
}
