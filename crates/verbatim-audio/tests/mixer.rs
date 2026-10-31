//! Mixer tests (decision D17) against a device whose playback the test
//! advances by hand, so every ending and mark can be checked against
//! exactly how much audio has played.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use verbatim_audio::{
    AudioDevice, AudioError, AudioTap, ClockReading, DeviceFormat, DeviceNotice, DeviceNotices,
    Lane, Mixer, MixerBuilder, PcmFormat, PlaybackEvent, Sound, SourceControl, SpeechLane, Waker,
    WavRecorder,
};
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

/// Counts every allocation and free made on each thread, so the audio
/// thread's own can be read on it ([`ManualDevice`] copies them at each
/// write and wait).
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
}

fn count(made: u64, released: u64) {
    let _ = ALLOCATIONS.try_with(|counts| {
        let (allocations, frees) = counts.get();
        counts.set((allocations + made, frees + released));
    });
}

// SAFETY: every call is passed on to the system allocator unchanged; the
// counting touches only a constant-initialized thread-local, which never
// allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        // SAFETY: as the caller's contract for `alloc`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        count(0, 1);
        // SAFETY: as the caller's contract for `dealloc`.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count(1, 0);
        // SAFETY: as the caller's contract for `alloc_zeroed`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count(1, 1);
        // SAFETY: as the caller's contract for `realloc`.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// One frame per millisecond, mono, a ten-frame device queue: small numbers
/// that make positions easy to follow. A source may then run 50 frames
/// ahead of playback (the queue plus the mixer's 40 ms).
const FORMAT: DeviceFormat = DeviceFormat {
    sample_rate: 1_000,
    channels: 1,
    buffer_frames: 10,
};

const PCM: PcmFormat = PcmFormat {
    sample_rate: 1_000,
    channels: 1,
};

const WAIT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct DeviceState {
    queued: u32,
    /// Every frame the mixer has written, in order.
    written: Vec<f32>,
    stops: u32,
    woken: bool,
    /// The mixer's wait ignores wakes until released.
    held: bool,
    /// How many times the mixer has begun to wait for the device: once at
    /// the end of every pass, when it has done all it could.
    waits: u64,
    /// The time limit of every wait the mixer has begun, in order.
    wait_limits: Vec<Duration>,
    /// How many times the mixer has opened the device.
    opens: u32,
}

/// A device that plays only when the test says so.
#[derive(Clone)]
struct ManualDevice {
    state: Arc<(Mutex<DeviceState>, Condvar)>,
    reopen: Arc<AtomicBool>,
    /// The next poll fails, as a device that was unplugged does.
    fail: Arc<AtomicBool>,
    /// The format the device comes back in when reopened, if not [`FORMAT`].
    reopened_format: Arc<Mutex<Option<DeviceFormat>>>,
    /// What the device's clock reads, if it has one.
    clock: Arc<Mutex<Option<ClockReading>>>,
    /// How many of the next opens fail.
    open_failures: Arc<AtomicU32>,
    /// The audio thread's allocations and frees, as counted at its last
    /// write or wait.
    allocations: Arc<(AtomicU64, AtomicU64)>,
    /// The production notices, which the test notifies as Windows would.
    notices: DeviceNotices,
}

impl Default for ManualDevice {
    fn default() -> Self {
        let state: Arc<(Mutex<DeviceState>, Condvar)> = Arc::default();
        let woken = Arc::clone(&state);
        Self {
            state,
            reopen: Arc::default(),
            fail: Arc::default(),
            reopened_format: Arc::default(),
            clock: Arc::default(),
            open_failures: Arc::default(),
            allocations: Arc::default(),
            notices: DeviceNotices::new(Arc::new(move || {
                let (state, condvar) = &*woken;
                state.lock().unwrap().woken = true;
                condvar.notify_all();
            })),
        }
    }
}

impl ManualDevice {
    /// A device whose records of what it was given have room for `frames`
    /// frames and as many waits, so recording them allocates nothing.
    fn reserved(frames: usize) -> Self {
        let device = Self::default();
        {
            let mut state = device.state.0.lock().unwrap();
            state.written.reserve(frames);
            state.wait_limits.reserve(frames);
        }
        device
    }

    /// Plays `frames` of what is queued and wakes the mixer.
    fn play(&self, frames: u32) {
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        state.queued = state.queued.saturating_sub(frames);
        state.woken = true;
        condvar.notify_all();
    }

    fn written(&self) -> Vec<f32> {
        self.state.0.lock().unwrap().written.clone()
    }

    fn waits(&self) -> u64 {
        self.state.0.lock().unwrap().waits
    }

    /// Holds the mixer in its wait, whatever wakes it, until released.
    fn hold(&self) {
        self.state.0.lock().unwrap().held = true;
    }

    fn release(&self) {
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        state.held = false;
        state.woken = true;
        condvar.notify_all();
    }

    /// Records the calling thread's allocations and frees, on the audio
    /// thread.
    fn count_allocations(&self) {
        let (allocations, frees) = ALLOCATIONS.with(Cell::get);
        self.allocations.0.store(allocations, Ordering::SeqCst);
        self.allocations.1.store(frees, Ordering::SeqCst);
    }

    fn allocations(&self) -> (u64, u64) {
        (
            self.allocations.0.load(Ordering::SeqCst),
            self.allocations.1.load(Ordering::SeqCst),
        )
    }

    /// Returns once the mixer has written at least `frames` frames in all,
    /// woken by each write. Fails at [`WAIT`], saying `what` did not happen.
    fn wait_written(&self, frames: usize, what: &str) {
        let (state, condvar) = &*self.state;
        let state = state.lock().unwrap();
        let (_state, wait) = condvar
            .wait_timeout_while(state, WAIT, |state| state.written.len() < frames)
            .unwrap();
        assert!(!wait.timed_out(), "{what} within {WAIT:?}");
    }

    /// Returns once the mixer has opened the device `opens` times in all.
    /// Fails at [`WAIT`], saying `what` did not happen.
    fn wait_opened(&self, opens: u32, what: &str) {
        let (state, condvar) = &*self.state;
        let state = state.lock().unwrap();
        let (_state, wait) = condvar
            .wait_timeout_while(state, WAIT, |state| state.opens < opens)
            .unwrap();
        assert!(!wait.timed_out(), "{what} within {WAIT:?}");
    }

    /// Returns once the mixer has made a whole pass that began after this
    /// call, so everything the test did before it is reflected in what the
    /// mixer has written and sent to its delivery thread (it sends during
    /// the pass, before it waits). The mixer is woken first; if it was in
    /// the middle of a pass, that pass ends in a wait which the wake-up
    /// cuts short, and the pass after it is a whole one. Fails at [`WAIT`].
    fn settle(&self) {
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        let target = state.waits + 2;
        let deadline = std::time::Instant::now() + WAIT;
        while state.waits < target {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "the mixer made no whole pass within {WAIT:?}"
            );
            state.woken = true;
            condvar.notify_all();
            state = condvar.wait_timeout(state, remaining).unwrap().0;
        }
    }
}

impl AudioDevice for ManualDevice {
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        self.reopen.store(false, Ordering::SeqCst);
        self.notices.opening();
        {
            let (state, condvar) = &*self.state;
            let mut state = state.lock().unwrap();
            state.queued = 0;
            state.opens += 1;
            // For `wait_opened`, which waits on the same condition variable.
            condvar.notify_all();
        }
        let failures = self.open_failures.load(Ordering::SeqCst);
        if failures > 0 {
            self.open_failures.store(failures - 1, Ordering::SeqCst);
            self.notices.failed(None);
            return Err(AudioError::Device("no audio device".to_owned()));
        }
        self.notices.opened();
        Ok(self.reopened_format.lock().unwrap().unwrap_or(FORMAT))
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(AudioError::Device("the device was unplugged".to_owned()));
        }
        Ok(self.state.0.lock().unwrap().queued)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        self.count_allocations();
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        state.queued += u32::try_from(samples.len()).unwrap();
        state.written.extend_from_slice(samples);
        // For `wait_written`, which waits on the same condition variable.
        condvar.notify_all();
        Ok(())
    }

    fn start(&mut self) -> Result<(), AudioError> {
        Ok(())
    }

    fn stop(&mut self) {
        let mut state = self.state.0.lock().unwrap();
        state.queued = 0;
        state.stops += 1;
    }

    fn wait(&mut self, timeout: Duration) {
        self.count_allocations();
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        state.waits += 1;
        state.wait_limits.push(timeout);
        // For `settle`, which waits on the same condition variable.
        condvar.notify_all();
        let (mut state, _) = condvar
            .wait_timeout_while(state, timeout, |state| !state.woken || state.held)
            .unwrap();
        state.woken = false;
    }

    fn waker(&self) -> Waker {
        let device = self.clone();
        Arc::new(move || device.play(0))
    }

    fn needs_reopen(&self) -> bool {
        self.reopen.load(Ordering::SeqCst) || self.notices.needs_reopen()
    }

    fn clock_position(&mut self) -> Option<ClockReading> {
        *self.clock.lock().unwrap()
    }
}

/// A mixer on a manual device with three sources: speech, with the synth
/// thread's lane, a lane of sounds and the queue thread's control; events,
/// with a lane of sounds; and a probe, whose empty utterances show when the
/// delivery thread has delivered everything sent before them.
struct Harness {
    device: ManualDevice,
    lane: SpeechLane,
    sounds: Lane,
    control: SourceControl,
    events: Receiver<PlaybackEvent>,
    earcons: Lane,
    earcon_events: Receiver<PlaybackEvent>,
    probe: SpeechLane,
    probed: Receiver<UtteranceId>,
    probes: u64,
    mixer: Mixer,
}

/// A listener that sends every event to the receiver returned with it.
fn listener() -> (verbatim_audio::PlaybackListener, Receiver<PlaybackEvent>) {
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    (
        Arc::new(move |event, _| {
            let _ = sender.lock().unwrap().send(event);
        }),
        events,
    )
}

fn harness() -> Harness {
    harness_on(ManualDevice::default(), None)
}

fn harness_on(device: ManualDevice, tap: Option<Box<dyn AudioTap>>) -> Harness {
    let mut builder = MixerBuilder::new(Box::new(device.clone()));
    let (speech_listener, events) = listener();
    let speech = builder.source(speech_listener);
    let control = builder.control(speech);
    let lane = builder.speech_lane(speech);
    let sounds = builder.lane(speech);
    let (earcon_listener, earcon_events) = listener();
    let earcon_source = builder.source(earcon_listener);
    let earcons = builder.lane(earcon_source);
    let (probe_tx, probed) = mpsc::channel();
    let probe_tx = Mutex::new(probe_tx);
    let probe_source = builder.source(Arc::new(move |event, _| {
        if let PlaybackEvent::Ended { utterance, .. } = event {
            let _ = probe_tx.lock().unwrap().send(utterance);
        }
    }));
    let probe = builder.speech_lane(probe_source);
    if let Some(tap) = tap {
        builder.tap(tap);
    }
    let mixer = builder.start().expect("the mixer starts");
    Harness {
        device,
        lane,
        sounds,
        control,
        events,
        earcons,
        earcon_events,
        probe,
        probed,
        probes: 0,
        mixer,
    }
}

fn utterance(id: u64) -> UtteranceId {
    UtteranceId(id)
}

/// Samples whose value tells which frame of the test they are.
fn samples(count: usize, value: i16) -> Vec<i16> {
    vec![value; count]
}

/// The device sample [`Harness::speak`] writes: 1,000 as 16-bit audio.
const SPOKEN: f32 = 1_000.0 / 32_768.0;

impl Harness {
    /// Registers utterance `id` under the source's cancel generation now.
    fn register(&mut self, id: u64) -> TraceId {
        let trace = TraceId::mint();
        let generation = self.control.generation();
        self.lane.register(utterance(id), trace, generation);
        trace
    }

    fn write(&mut self, id: u64, frames: usize) -> ControlFlow<()> {
        self.lane.write(utterance(id), PCM, &samples(frames, 1_000))
    }

    fn speak(&mut self, id: u64, frames: usize) -> TraceId {
        self.speak_at(id, frames, 1_000)
    }

    /// Speaks `frames` frames of the 16-bit sample `value`.
    fn speak_at(&mut self, id: u64, frames: usize, value: i16) -> TraceId {
        let trace = self.register(id);
        assert!(
            self.lane
                .write(utterance(id), PCM, &samples(frames, value))
                .is_continue()
        );
        self.lane.finish(utterance(id));
        trace
    }

    fn next(&self) -> PlaybackEvent {
        self.events
            .recv_timeout(WAIT)
            .expect("a playback event arrives")
    }

    /// Returns once the delivery thread has delivered everything the mixer
    /// sent before now: the probe's empty utterance, which ends in the pass
    /// that takes it, is delivered after all of it.
    fn delivered(&mut self) {
        self.probes += 1;
        let probe = utterance(u64::MAX - self.probes);
        self.probe.register(probe, TraceId::mint(), 0);
        self.probe.finish(probe);
        assert_eq!(
            self.probed.recv_timeout(WAIT),
            Ok(probe),
            "the probe is delivered"
        );
    }

    /// Asserts that the mixer, having done everything the test asked of it
    /// so far, reported nothing more.
    fn nothing_more(&mut self) {
        self.device.settle();
        self.delivered();
        assert_eq!(
            self.events.try_recv().ok(),
            None,
            "no further playback event"
        );
    }

    fn play(&self, frames: u32) {
        play(&self.device, frames);
    }
}

/// Plays `frames` one at a time, each only once the mixer has written it
/// (a device cannot play what it has not been given), and stops early once
/// the mixer has done all it could and written nothing more.
fn play(device: &ManualDevice, frames: u32) {
    for _ in 0..frames {
        if device.state.0.lock().unwrap().queued == 0 {
            device.settle();
            if device.state.0.lock().unwrap().queued == 0 {
                return;
            }
        }
        device.play(1);
    }
}

fn ended(id: u64, trace: TraceId, ending: UtteranceEnding) -> PlaybackEvent {
    PlaybackEvent::Ended {
        utterance: utterance(id),
        trace_id: trace,
        ending,
    }
}

fn started(id: u64, trace: TraceId) -> PlaybackEvent {
    PlaybackEvent::Started {
        utterance: utterance(id),
        trace_id: trace,
    }
}

#[test]
fn an_utterance_completes_only_when_the_device_has_played_its_last_frame() {
    let mut harness = harness();
    let trace = harness.speak(1, 20);
    harness.nothing_more();

    harness.play(1);
    assert_eq!(harness.next(), started(1, trace));
    harness.play(18);
    harness.nothing_more();
    harness.play(1);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
}

#[test]
fn a_start_is_heard_when_the_devices_clock_passes_its_first_frame() {
    let mut harness = harness();
    let trace = harness.speak(1, 20);
    harness.play(1);
    assert_eq!(harness.next(), started(1, trace));
    *harness.device.clock.lock().unwrap() = Some(ClockReading {
        frames: 0,
        age: Duration::from_millis(2),
    });
    harness.nothing_more();
    // Three frames past the first, at one frame a millisecond, read two
    // milliseconds ago: the clock passed it five milliseconds ago.
    *harness.device.clock.lock().unwrap() = Some(ClockReading {
        frames: 3,
        age: Duration::from_millis(2),
    });
    harness.device.settle();
    assert_eq!(
        harness.next(),
        PlaybackEvent::Heard {
            utterance: utterance(1),
            trace_id: trace,
            ago: Duration::from_millis(5),
        }
    );
    harness.nothing_more();
}

/// A source may hold 50 frames unplayed (the 10-frame queue and 40 ms
/// beyond it). A producer that fills it waits on the source's room event,
/// which the mixer sets once at least half of that, 25 frames, is free, and
/// also when the audio thread empties half the lane's records, so a wake
/// alone does not let it on: it goes on only with room for what it needs.
/// With 75 frames to write it needs 25 more after the first 50, so with 24
/// frames played it still waits, and the 25th lets it on.
#[test]
fn a_producer_waiting_for_room_is_woken_when_half_its_room_is_free() {
    let mut harness = harness();
    harness.register(1);
    let (done_tx, done_rx) = mpsc::channel();
    let mut lane = harness.lane;
    let writer = std::thread::spawn(move || {
        let flow = lane.write(utterance(1), PCM, &samples(75, 1_000));
        done_tx.send(flow).unwrap();
        lane
    });
    harness
        .device
        .wait_written(10, "the mixer writes the first frames");
    play(&harness.device, 24);
    harness.device.settle();
    assert_eq!(
        done_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty),
        "with 24 frames free the producer still waits"
    );
    play(&harness.device, 1);
    assert_eq!(
        done_rx.recv_timeout(WAIT),
        Ok(ControlFlow::Continue(())),
        "the producer writes the rest once 25 frames are free"
    );
    harness.lane = writer.join().unwrap();
}

#[test]
fn a_producer_waiting_for_room_is_woken_at_shutdown_and_its_write_answers_break() {
    let mut harness = harness();
    harness.register(1);
    let (done_tx, done_rx) = mpsc::channel();
    let mut lane = harness.lane;
    let writer = std::thread::spawn(move || {
        let flow = lane.write(utterance(1), PCM, &samples(60, 1_000));
        done_tx.send(flow).unwrap();
    });
    harness
        .device
        .wait_written(10, "the mixer writes the first frames");
    harness.device.settle();
    assert_eq!(
        done_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty),
        "the producer waits for room"
    );
    drop(harness.mixer);
    assert_eq!(done_rx.recv_timeout(WAIT), Ok(ControlFlow::Break(())));
    writer.join().unwrap();
}

#[test]
fn marks_are_reported_when_playback_reaches_them() {
    let mut harness = harness();
    let trace = harness.register(1);
    let _ = harness.write(1, 7);
    harness.lane.mark(utterance(1), 42);
    let _ = harness.write(1, 3);
    harness.lane.finish(utterance(1));

    harness.play(6);
    assert_eq!(harness.next(), started(1, trace));
    harness.nothing_more();
    harness.play(1);
    assert_eq!(
        harness.next(),
        PlaybackEvent::Mark {
            utterance: utterance(1),
            trace_id: trace,
            mark: 42
        }
    );
    harness.play(3);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
}

#[test]
fn an_utterance_without_audio_completes_when_the_audio_before_it_has_played() {
    let mut harness = harness();
    let first = harness.speak(1, 10);
    let silent = harness.register(2);
    harness.lane.finish(utterance(2));

    harness.play(1);
    assert_eq!(harness.next(), started(1, first));
    harness.play(8);
    harness.nothing_more();
    harness.play(1);
    assert_eq!(harness.next(), ended(1, first, UtteranceEnding::Completed));
    assert_eq!(harness.next(), ended(2, silent, UtteranceEnding::Completed));
}

#[test]
fn cancelling_ends_every_unheard_utterance_and_spares_later_ones() {
    let mut harness = harness();
    let first = harness.speak(1, 20);
    let second = harness.register(2);
    harness.play(5);
    assert_eq!(harness.next(), started(1, first));
    // Five frames played, and the mixer filled the device's queue again:
    // fifteen frames of the first utterance were written.
    harness.device.settle();
    assert_eq!(harness.device.written(), [SPOKEN; 15]);

    harness.control.cancel();
    assert_eq!(harness.next(), ended(1, first, UtteranceEnding::Cancelled));
    assert_eq!(harness.next(), ended(2, second, UtteranceEnding::Cancelled));
    assert!(
        harness.write(2, 5).is_break(),
        "a cancelled utterance accepts no more audio"
    );

    // The device's queue was discarded: what follows is the next
    // utterance's audio, and nothing of the first is written again.
    let third = harness.speak_at(3, 4, 2_000);
    harness.play(1);
    assert_eq!(harness.next(), started(3, third));
    harness.play(3);
    assert_eq!(harness.next(), ended(3, third, UtteranceEnding::Completed));
    harness.nothing_more();
    let mut expected = vec![SPOKEN; 15];
    expected.extend([2_000.0 / 32_768.0; 4]);
    assert_eq!(harness.device.written(), expected);
}

/// An utterance handed on before a cancel and registered after it, under
/// the generation before the cancel, ends cancelled with nothing of it
/// written; one handed on after the cancel, under its generation, plays in
/// full (`docs/design/ownership.md`).
#[test]
fn an_utterance_registered_under_a_generation_before_a_cancel_ends_cancelled_unheard() {
    let mut harness = harness();
    harness.control.cancel();
    harness.control.cancel();
    let late = TraceId::mint();
    harness.lane.register(utterance(1), late, 1);
    let _ = harness.write(1, 20);
    harness.lane.finish(utterance(1));
    assert_eq!(harness.next(), ended(1, late, UtteranceEnding::Cancelled));

    let spared = TraceId::mint();
    harness.lane.register(utterance(2), spared, 2);
    assert!(harness.write(2, 10).is_continue());
    harness.lane.finish(utterance(2));
    harness.play(1);
    assert_eq!(harness.next(), started(2, spared));
    harness.play(9);
    assert_eq!(harness.next(), ended(2, spared, UtteranceEnding::Completed));
    harness.nothing_more();
    assert_eq!(harness.device.written(), [SPOKEN; 10]);
}

/// The sequence of 3ebb7be0: a cancel raises the generation, then the synth
/// thread, woken, finishes the empty utterance it was writing. The mixer
/// takes the finish and the raised generation in one pass, reads the
/// generation after draining the lane and before firing, so the utterance
/// ends cancelled, never completed.
#[test]
fn an_empty_utterance_finished_after_a_cancel_in_one_pass_ends_cancelled() {
    let mut harness = harness();
    harness.device.settle();
    harness.device.hold();
    let trace = harness.register(1);
    harness.control.cancel();
    harness.lane.finish(utterance(1));
    harness.device.release();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));
    harness.nothing_more();
}

#[test]
fn a_failed_utterance_is_reported_failed_and_its_unheard_audio_dropped() {
    let mut harness = harness();
    let trace = harness.register(1);
    let _ = harness.write(1, 20);
    // The device's queue holds ten frames of it, none played.
    harness
        .device
        .wait_written(10, "the mixer fills the device's queue");
    harness.device.settle();
    assert_eq!(harness.device.written(), [SPOKEN; 10]);
    harness
        .lane
        .fail(utterance(1), "synthesis failed".to_owned());
    assert_eq!(
        harness.next(),
        ended(
            1,
            trace,
            UtteranceEnding::Failed("synthesis failed".to_owned())
        )
    );
    // What the device had queued was taken back, and nothing more of the
    // utterance is written: there is nothing left to play.
    harness.play(20);
    harness.nothing_more();
    assert_eq!(harness.device.state.0.lock().unwrap().queued, 0);
    assert_eq!(harness.device.written(), [SPOKEN; 10]);
}

#[test]
fn a_write_waits_while_the_source_is_too_far_ahead_of_playback() {
    let mut harness = harness();
    harness.register(1);
    let (done_tx, done_rx) = mpsc::channel();
    let mut lane = harness.lane;
    let writer = std::thread::spawn(move || {
        // Fifty frames fit ahead of playback; the sixty-first must wait.
        let _ = lane.write(utterance(1), PCM, &samples(60, 1_000));
        done_tx.send(()).unwrap();
        lane
    });
    harness
        .device
        .wait_written(10, "the mixer writes the first frames");
    harness.device.settle();
    assert!(done_rx.try_recv().is_err(), "the write is held back");
    play(&harness.device, 30);
    done_rx
        .recv_timeout(WAIT)
        .expect("the write finishes once audio has played");
    harness.lane = writer.join().unwrap();
}

#[test]
fn a_reopened_device_is_given_again_what_had_not_played() {
    let mut harness = harness();
    let trace = harness.speak(1, 10);
    harness.play(4);
    assert_eq!(harness.next(), started(1, trace));

    // Six frames were queued when the device asked to be reopened; they
    // must be written again, so the utterance is still heard in full.
    harness.device.reopen.store(true, Ordering::SeqCst);
    harness.device.play(0);
    harness
        .device
        .wait_written(16, "the unplayed frames are written again");
    harness.nothing_more();
    harness.play(6);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    assert_eq!(harness.device.written()[10..], [1_000.0 / 32_768.0; 6]);
}

#[test]
fn a_failed_device_is_reopened_and_the_utterance_completes_only_once_heard() {
    let mut harness = harness();
    let trace = harness.speak(1, 10);
    harness.play(4);
    assert_eq!(harness.next(), started(1, trace));
    // The mixer has seen the four frames play.
    harness.device.settle();

    harness.device.fail.store(true, Ordering::SeqCst);
    harness.device.play(0);
    // The reopened device is given exactly the six frames not yet played,
    // and nothing is reported until it has played them.
    harness.nothing_more();
    assert_eq!(harness.device.written(), [SPOKEN; 16]);
    harness.play(5);
    harness.nothing_more();
    harness.play(1);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    harness.nothing_more();
    assert_eq!(harness.device.written(), [SPOKEN; 16]);
}

#[test]
fn a_device_that_comes_back_in_another_format_fails_the_unheard_utterance() {
    let mut harness = harness();
    let trace = harness.speak(1, 10);
    harness.play(4);
    assert_eq!(harness.next(), started(1, trace));

    *harness.device.reopened_format.lock().unwrap() = Some(DeviceFormat {
        sample_rate: 2_000,
        ..FORMAT
    });
    harness.device.reopen.store(true, Ordering::SeqCst);
    harness.device.play(0);
    assert_eq!(
        harness.next(),
        ended(
            1,
            trace,
            UtteranceEnding::Failed("the audio device changed format".to_owned())
        )
    );
    harness.nothing_more();
}

/// An utterance whose registration and frames are still in the lane when
/// the device comes back in another format is registered after the
/// reopen, and its frames, converted for the old format, cannot play: it
/// fails, rather than completing with nothing heard.
#[test]
fn an_utterance_whose_frames_were_converted_for_a_format_the_device_left_fails() {
    let mut harness = harness();
    harness.device.settle();
    harness.device.hold();
    let trace = harness.register(1);
    assert!(harness.write(1, 10).is_continue());
    harness.lane.finish(utterance(1));
    *harness.device.reopened_format.lock().unwrap() = Some(DeviceFormat {
        sample_rate: 2_000,
        ..FORMAT
    });
    harness.device.reopen.store(true, Ordering::SeqCst);
    harness.device.release();
    assert_eq!(
        harness.next(),
        ended(
            1,
            trace,
            UtteranceEnding::Failed("the audio device changed format".to_owned())
        )
    );
    harness.nothing_more();
    assert_eq!(harness.device.written(), []);
}

#[test]
fn a_device_that_fails_to_open_is_replaced_by_silence_until_a_notice_and_then_opened() {
    let device = ManualDevice::default();
    device.open_failures.store(1, Ordering::SeqCst);
    let mut harness = harness_on(device, None);

    // Real-time silence plays in the device's place, in its own format, so
    // an utterance still takes its time and completes.
    assert_eq!(
        harness.mixer.format(),
        DeviceFormat {
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 1_920,
        }
    );
    // A whole idle pass first, so the waits below begin with an idle one.
    harness.device.settle();
    let trace = harness.speak(1, 20);
    assert_eq!(harness.next(), started(1, trace));
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    harness.nothing_more();

    // While the silence played, the mixer's waits on the device were
    // capped at the silent device's 10 ms period, so it kept time; idle,
    // before and after, it waits with no time limit and has not tried the
    // device again: only a notice ends that wait.
    {
        let state = harness.device.state.0.lock().unwrap();
        assert_eq!(state.opens, 1);
        let mut limits = state.wait_limits.clone();
        limits.dedup();
        assert_eq!(
            limits,
            [Duration::MAX, Duration::from_millis(10), Duration::MAX]
        );
        assert_eq!(state.written, []);
    }

    // A device became available: the mixer opens the device again, in its
    // format, and plays through it.
    harness.device.notices.notify(DeviceNotice::Available);
    harness
        .device
        .wait_opened(2, "the device is opened again on the notice");
    harness.device.settle();
    assert_eq!(harness.mixer.format(), FORMAT);
    let trace = harness.speak(2, 10);
    harness.play(1);
    assert_eq!(harness.next(), started(2, trace));
    harness.play(9);
    assert_eq!(harness.next(), ended(2, trace, UtteranceEnding::Completed));
    harness.nothing_more();
    assert_eq!(harness.device.written(), [SPOKEN; 10]);
    assert_eq!(harness.device.state.0.lock().unwrap().opens, 2);
}

/// Collects what the tap is given, and wakes whoever waits on the
/// condition variable each time it is given more.
struct Collected(Arc<(Mutex<Vec<f32>>, Condvar)>);

impl AudioTap for Collected {
    fn played(&mut self, samples: &[f32], _format: DeviceFormat) {
        let (frames, fed) = &*self.0;
        frames.lock().unwrap().extend_from_slice(samples);
        fed.notify_all();
    }
}

#[test]
fn the_tap_gets_exactly_what_played_and_never_what_was_cut_off() {
    let tapped = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let mut harness = harness_on(
        ManualDevice::default(),
        Some(Box::new(Collected(Arc::clone(&tapped)))),
    );
    let trace = harness.speak(1, 20);
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));
    harness.control.cancel();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));

    // Ten frames had been written to the device; only the five played are
    // recorded.
    let (frames, fed) = &*tapped;
    let frames = frames.lock().unwrap();
    let (frames, wait) = fed
        .wait_timeout_while(frames, WAIT, |frames| frames.len() < 5)
        .unwrap();
    assert!(!wait.timed_out(), "the tap is given what played");
    drop(frames);
    harness.device.settle();
    assert_eq!(tapped.0.lock().unwrap().len(), 5);
}

/// A device that played some frames and then asked to be reopened, before
/// the mixer polled it again, is given again only what it had not played:
/// how far it got is read before it is reopened.
#[test]
fn a_device_reopened_between_polls_is_not_given_again_what_it_played() {
    let mut harness = harness();
    harness.speak(1, 10);
    harness.device.wait_written(10, "the frames are written");
    {
        // Four frames play and the device asks to be reopened, with no
        // wake-up between, so the mixer sees both at its next poll.
        let mut state = harness.device.state.0.lock().unwrap();
        state.queued -= 4;
        harness.device.reopen.store(true, Ordering::SeqCst);
    }
    harness.device.play(0);
    harness
        .device
        .wait_written(16, "the unplayed frames are written again");
    harness.device.settle();
    assert_eq!(
        harness.device.written().len(),
        16,
        "only the six frames not played are written again"
    );
}

/// A sound of `frames` frames, every sample `value`, in the test's PCM
/// format.
fn sound(frames: usize, value: i16) -> Sound {
    Sound::from_pcm(PCM, samples(frames, value)).expect("a usable sound")
}

/// A sample of `value` as the mixer writes it.
fn level(value: i16) -> f32 {
    f32::from(value) / 32_768.0
}

fn assert_frames(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len(), "{actual:?}");
    for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() < 1e-6,
            "frame {index}: {actual} where {expected} was expected"
        );
    }
}

#[test]
fn a_sound_in_an_utterance_starts_at_its_place_and_plays_over_what_follows() {
    let mut harness = harness();
    let trace = harness.register(1);
    let _ = harness.write(1, 3);
    harness
        .lane
        .sound(utterance(1), &sound(5, 2_000), 1.0)
        .expect("the sound converts");
    let _ = harness.write(1, 4);
    harness.lane.finish(utterance(1));

    harness.play(1);
    assert_eq!(harness.next(), started(1, trace));
    harness.play(6);
    // The utterance's own audio has played; the sound plays on past it,
    // without holding back the ending.
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    harness.play(1);
    let (speech, both, alone) = (level(1_000), level(1_000) + level(2_000), level(2_000));
    assert_frames(
        &harness.device.written(),
        &[speech, speech, speech, both, both, both, both, alone],
    );
}

#[test]
fn a_sound_is_mixed_only_with_its_utterances_audio_until_the_utterance_is_finished() {
    let mut harness = harness();
    let trace = harness.register(1);
    // Ten frames, enough to start the device while the utterance is still
    // being written.
    let _ = harness.write(1, 10);
    harness
        .lane
        .sound(utterance(1), &sound(25, 2_000), 1.0)
        .expect("the sound converts");
    harness.play(10);
    assert_eq!(harness.next(), started(1, trace));
    // Playback has reached the sound's place, and the next word has not
    // been written: nothing more is mixed, the sound alone included.
    harness.device.settle();
    let speech = level(1_000);
    let (both, alone) = (level(1_000) + level(2_000), level(2_000));
    assert_frames(&harness.device.written(), &[speech; 10]);

    // Part of the next word, enough to start the device again: the sound
    // starts with it, and goes no further than it while the rest has not
    // been written.
    let _ = harness.write(1, 10);
    harness.play(10);
    harness.device.settle();
    let mut expected = vec![speech; 10];
    expected.extend([both; 10]);
    assert_frames(&harness.device.written(), &expected);

    // The rest of the word is heard under the sound too, and once the
    // utterance is finished the sound plays on past it.
    let _ = harness.write(1, 10);
    harness.lane.finish(utterance(1));
    harness.play(10);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    harness.play(5);
    expected.extend([both; 10]);
    expected.extend([alone; 5]);
    assert_frames(&harness.device.written(), &expected);
    harness.nothing_more();
}

#[test]
fn a_sound_is_cancelled_with_its_utterance() {
    let mut harness = harness();
    let trace = harness.register(1);
    let _ = harness.write(1, 3);
    harness
        .lane
        .sound(utterance(1), &sound(40, 2_000), 0.5)
        .expect("the sound converts");
    let _ = harness.write(1, 30);
    harness.lane.finish(utterance(1));
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));

    harness.control.cancel();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));
    harness.device.settle();
    let cut = harness.device.written().len();

    // What follows the cut is the next utterance alone: the sound went with
    // the utterance it belonged to.
    let next = harness.speak(2, 4);
    harness.play(1);
    assert_eq!(harness.next(), started(2, next));
    harness.play(3);
    assert_eq!(harness.next(), ended(2, next, UtteranceEnding::Completed));
    assert_frames(&harness.device.written()[cut..], &[level(1_000); 4]);
}

#[test]
fn a_sound_not_yet_reached_is_dropped_when_its_utterance_is_cancelled() {
    let mut harness = harness();
    let trace = harness.register(1);
    let _ = harness.write(1, 30);
    harness
        .lane
        .sound(utterance(1), &sound(3, 2_000), 1.0)
        .expect("the sound converts");
    harness.lane.finish(utterance(1));
    harness.play(2);
    assert_eq!(harness.next(), started(1, trace));
    harness.control.cancel();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));
    harness.nothing_more();
    let speech = level(1_000);
    assert!(
        harness
            .device
            .written()
            .iter()
            .all(|frame| (frame - speech).abs() < 1e-6),
        "the sound was never mixed"
    );
}

#[test]
fn a_sound_played_at_once_is_mixed_over_speech_from_another_source() {
    let mut harness = harness();
    let trace = harness.speak(1, 30);
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));

    harness
        .earcons
        .play(&sound(3, 2_000), 1.0)
        .expect("the sound converts");
    harness.play(25);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));

    let written = harness.device.written();
    let both = level(1_000) + level(2_000);
    let mixed: Vec<usize> = written
        .iter()
        .enumerate()
        .filter(|(_, frame)| (*frame - both).abs() < 1e-6)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(mixed.len(), 3, "the sound is mixed in once: {written:?}");
    assert_eq!(mixed[2] - mixed[0], 2, "over three frames in a row");
    assert_eq!(written.len(), 30);
}

/// The mixer waits with no time limit while idle, so only the push itself
/// can wake it: a sound played at once into an idle mixer is mixed at
/// once, alone.
#[test]
fn a_sound_played_into_an_idle_mixer_is_mixed_at_once() {
    let mut harness = harness();
    harness.device.settle();
    harness
        .sounds
        .play(&sound(4, 2_000), 2.0)
        .expect("the sound converts");
    harness
        .device
        .wait_written(4, "the sound is written at once");
    harness.play(4);
    harness.nothing_more();
    assert_frames(&harness.device.written(), &[level(2_000) * 2.0; 4]);
}

/// A sound played to its end, converted for a device format that the
/// device left before the mixer took it, cannot play: it is reported ended
/// at once, and nothing is written, so a wait for it, as for the exit
/// sound, ends.
#[test]
fn a_sound_played_to_its_end_for_an_older_format_ends_at_once_with_nothing_written() {
    let mut harness = harness();
    harness.device.settle();
    *harness.device.reopened_format.lock().unwrap() = Some(DeviceFormat {
        sample_rate: 2_000,
        ..FORMAT
    });
    // The device asks to be reopened, and the mixer, idle with no time
    // limit, sees it at the pass the push wakes it for, before it takes
    // the push.
    harness.device.reopen.store(true, Ordering::SeqCst);
    let trace = TraceId::mint();
    harness
        .earcons
        .play_to_end(utterance(1), trace, &sound(4, 2_000), 1.0)
        .expect("the sound converts");
    assert_eq!(
        harness.earcon_events.recv_timeout(WAIT),
        Ok(ended(1, trace, UtteranceEnding::Cancelled))
    );
    harness.nothing_more();
    assert_eq!(harness.device.written(), []);
}

#[test]
fn a_partly_heard_sound_goes_on_where_it_was_after_the_device_reopens() {
    let mut harness = harness();
    // Ten frames, the device's whole queue, each a different level.
    let ramp: Vec<i16> = (1..=10).map(|frame| frame * 100).collect();
    let ramp_sound = Sound::from_pcm(PCM, ramp.clone()).expect("a usable sound");
    harness
        .sounds
        .play(&ramp_sound, 1.0)
        .expect("the sound converts");
    harness.play(4);

    // Six frames were queued when the device asked to be reopened; the
    // sound goes on from its fifth frame, so all of it is heard once.
    harness.device.reopen.store(true, Ordering::SeqCst);
    harness.device.play(0);
    harness
        .device
        .wait_written(16, "the unplayed frames are written again");
    harness.play(6);
    let expected: Vec<f32> = ramp.iter().map(|value| level(*value)).collect();
    let written = harness.device.written();
    assert_frames(&written[..10], &expected);
    assert_frames(&written[10..], &expected[4..]);
}

/// Plays speech with a mark and a sound in it while an earcon plays, then
/// speech cut by a cancel, then speech paused and resumed, utterances
/// numbered from `first`.
fn exercise(harness: &mut Harness, first: u64) {
    let trace = harness.register(first);
    let _ = harness.write(first, 10);
    harness.lane.mark(utterance(first), 1);
    harness
        .lane
        .sound(utterance(first), &sound(5, 2_000), 1.0)
        .expect("the sound converts");
    let _ = harness.write(first, 10);
    harness.lane.finish(utterance(first));
    harness
        .earcons
        .play(&sound(3, 3_000), 1.0)
        .expect("the earcon converts");
    harness.play(20);
    assert_eq!(harness.next(), started(first, trace));
    assert!(matches!(
        harness.next(),
        PlaybackEvent::Mark { mark: 1, .. }
    ));
    assert_eq!(
        harness.next(),
        ended(first, trace, UtteranceEnding::Completed)
    );

    let cut = harness.speak(first + 1, 30);
    harness.play(5);
    assert_eq!(harness.next(), started(first + 1, cut));
    harness.control.cancel();
    assert_eq!(
        harness.next(),
        ended(first + 1, cut, UtteranceEnding::Cancelled)
    );

    let held = harness.speak(first + 2, 20);
    harness.play(3);
    assert_eq!(harness.next(), started(first + 2, held));
    harness.control.pause(true);
    harness.device.settle();
    harness.control.pause(false);
    harness.play(20);
    assert_eq!(
        harness.next(),
        ended(first + 2, held, UtteranceEnding::Completed)
    );
}

/// The audio thread allocates and frees nothing once the device is open
/// (`docs/design/ownership.md`): after a warm-up
/// that plays everything once, 200 more passes that play speech with marks,
/// a sound in an utterance, an earcon, a cancel, a pause and a resume, all
/// recorded by the WAV tap, add no allocation and no free on it.
#[test]
fn the_audio_thread_allocates_and_frees_nothing_while_it_plays() {
    let dir = std::env::temp_dir().join(format!(
        "verbatim-mixer-allocates-nothing-{}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    let recorder = WavRecorder::create(&dir.join("audio.wav")).expect("the recording starts");
    let mut harness = harness_on(ManualDevice::reserved(1 << 16), Some(Box::new(recorder)));

    exercise(&mut harness, 1);
    while harness.device.waits() < 20 {
        harness.device.settle();
    }
    harness.device.settle();
    let before = harness.device.allocations();
    let from = harness.device.waits();

    exercise(&mut harness, 10);
    while harness.device.waits() < from + 200 {
        harness.device.settle();
    }
    assert_eq!(harness.device.allocations(), before);

    drop(harness);
    std::fs::remove_dir_all(dir).unwrap();
}
