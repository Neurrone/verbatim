//! Mixer tests (decision D17) against a device whose playback the test
//! advances by hand, so every ending and mark can be checked against
//! exactly how much audio has played.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use verbatim_audio::{
    AudioDevice, AudioError, AudioTap, DeviceFormat, Mixer, PcmFormat, PlaybackEvent, Sound,
    Source, Waker,
};
use verbatim_model::{TraceId, UtteranceEnding, UtteranceId};

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
    /// How many times the mixer has begun to wait for the device: once at
    /// the end of every pass, when it has done all it could.
    waits: u64,
}

/// A device that plays only when the test says so.
#[derive(Clone, Default)]
struct ManualDevice {
    state: Arc<(Mutex<DeviceState>, Condvar)>,
    reopen: Arc<AtomicBool>,
    /// The next poll fails, as a device that was unplugged does.
    fail: Arc<AtomicBool>,
    /// The format the device comes back in when reopened, if not [`FORMAT`].
    reopened_format: Arc<Mutex<Option<DeviceFormat>>>,
}

impl ManualDevice {
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

    /// Returns once the mixer has made a whole pass that began after this
    /// call, so everything the test did before it is reflected in what the
    /// mixer has written and reported (it reports during the pass, before
    /// it waits). The mixer is woken first; if it was in the middle of a
    /// pass, that pass ends in a wait which the wake-up cuts short, and the
    /// pass after it is a whole one. Fails at [`WAIT`].
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
        self.state.0.lock().unwrap().queued = 0;
        Ok(self.reopened_format.lock().unwrap().unwrap_or(FORMAT))
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(AudioError::Device("the device was unplugged".to_owned()));
        }
        Ok(self.state.0.lock().unwrap().queued)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
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
        let (state, condvar) = &*self.state;
        let mut state = state.lock().unwrap();
        state.waits += 1;
        // For `settle`, which waits on the same condition variable.
        condvar.notify_all();
        let (mut state, _) = condvar
            .wait_timeout_while(state, timeout, |state| !state.woken)
            .unwrap();
        state.woken = false;
    }

    fn waker(&self) -> Waker {
        let device = self.clone();
        Arc::new(move || device.play(0))
    }

    fn needs_reopen(&self) -> bool {
        self.reopen.load(Ordering::SeqCst)
    }
}

struct Harness {
    device: ManualDevice,
    source: Source,
    events: Receiver<PlaybackEvent>,
    mixer: Mixer,
}

fn harness() -> Harness {
    let device = ManualDevice::default();
    let mixer = Mixer::start(Box::new(device.clone())).expect("the manual device opens");
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let source = mixer.add_source(Arc::new(move |event| {
        let _ = sender.lock().unwrap().send(event);
    }));
    Harness {
        device,
        source,
        events,
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
    fn speak(&self, id: u64, frames: usize) -> TraceId {
        self.speak_at(id, frames, 1_000)
    }

    /// Speaks `frames` frames of the 16-bit sample `value`.
    fn speak_at(&self, id: u64, frames: usize, value: i16) -> TraceId {
        let trace = TraceId::mint();
        self.source.register(utterance(id), trace);
        assert!(
            self.source
                .write(utterance(id), PCM, &samples(frames, value))
                .is_continue()
        );
        self.source.finish(utterance(id));
        trace
    }

    fn next(&self) -> PlaybackEvent {
        self.events
            .recv_timeout(WAIT)
            .expect("a playback event arrives")
    }

    /// Asserts that the mixer, having done everything the test asked of it
    /// so far, reported nothing more.
    fn nothing_more(&self) {
        self.device.settle();
        assert_eq!(
            self.events.try_recv().ok(),
            None,
            "no further playback event"
        );
    }

    /// Plays `frames` one at a time, each only once the mixer has written
    /// it (a device cannot play what it has not been given), and stops early
    /// once the mixer has done all it could and written nothing more.
    fn play(&self, frames: u32) {
        for _ in 0..frames {
            if self.device.state.0.lock().unwrap().queued == 0 {
                self.device.settle();
                if self.device.state.0.lock().unwrap().queued == 0 {
                    return;
                }
            }
            self.device.play(1);
        }
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
    let harness = harness();
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
fn marks_are_reported_when_playback_reaches_them() {
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(7, 1_000));
    harness.source.mark(utterance(1), 42);
    let _ = harness.source.write(utterance(1), PCM, &samples(3, 1_000));
    harness.source.finish(utterance(1));

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
    let harness = harness();
    let first = harness.speak(1, 10);
    let silent = TraceId::mint();
    harness.source.register(utterance(2), silent);
    harness.source.finish(utterance(2));

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
    let harness = harness();
    let first = harness.speak(1, 20);
    let second = TraceId::mint();
    harness.source.register(utterance(2), second);
    harness.play(5);
    assert_eq!(harness.next(), started(1, first));
    // Five frames played, and the mixer filled the device's queue again:
    // fifteen frames of the first utterance were written.
    harness.device.settle();
    assert_eq!(harness.device.written(), [SPOKEN; 15]);

    harness.source.cancel_all();
    assert_eq!(harness.next(), ended(1, first, UtteranceEnding::Cancelled));
    assert_eq!(harness.next(), ended(2, second, UtteranceEnding::Cancelled));
    assert!(
        harness
            .source
            .write(utterance(1), PCM, &samples(5, 1_000))
            .is_break(),
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

#[test]
fn a_failed_utterance_is_reported_failed_and_its_unheard_audio_dropped() {
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(20, 1_000));
    // The device's queue holds ten frames of it, none played.
    harness.device.settle();
    assert_eq!(harness.device.written(), [SPOKEN; 10]);
    harness
        .source
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
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let (done_tx, done_rx) = mpsc::channel();
    let source = harness.source.clone();
    let writer = std::thread::spawn(move || {
        // Fifty frames fit ahead of playback; the sixty-first must wait.
        let _ = source.write(utterance(1), PCM, &samples(60, 1_000));
        done_tx.send(()).unwrap();
    });
    // The writer adds what fits in one step, under the mixer's lock, and
    // waits for room before letting go of it; so once the mixer has given
    // the device anything, the writer is waiting, and with nothing played
    // it has no room.
    harness
        .device
        .wait_written(1, "the mixer writes the first frames");
    assert!(done_rx.try_recv().is_err(), "the write is held back");
    harness.play(10);
    done_rx
        .recv_timeout(WAIT)
        .expect("the write finishes once audio has played");
    writer.join().unwrap();
}

#[test]
fn a_reopened_device_is_given_again_what_had_not_played() {
    let harness = harness();
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
    let harness = harness();
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
    let harness = harness();
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
    let device = ManualDevice::default();
    let tapped = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let mixer = Mixer::start_with_tap(
        Box::new(device.clone()),
        Box::new(Collected(Arc::clone(&tapped))),
    )
    .expect("the manual device opens");
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let source = mixer.add_source(Arc::new(move |event| {
        let _ = sender.lock().unwrap().send(event);
    }));
    let harness = Harness {
        device,
        source,
        events,
        mixer,
    };
    let trace = harness.speak(1, 20);
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));
    harness.source.cancel_all();
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
    let harness = harness();
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
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(3, 1_000));
    harness
        .source
        .sound(utterance(1), &sound(5, 2_000), 1.0)
        .expect("the sound converts");
    let _ = harness.source.write(utterance(1), PCM, &samples(4, 1_000));
    harness.source.finish(utterance(1));

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
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    // Ten frames, enough to start the device while the utterance is still
    // being written.
    let _ = harness.source.write(utterance(1), PCM, &samples(10, 1_000));
    harness
        .source
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
    let _ = harness.source.write(utterance(1), PCM, &samples(10, 1_000));
    harness.play(10);
    harness.device.settle();
    let mut expected = vec![speech; 10];
    expected.extend([both; 10]);
    assert_frames(&harness.device.written(), &expected);

    // The rest of the word is heard under the sound too, and once the
    // utterance is finished the sound plays on past it.
    let _ = harness.source.write(utterance(1), PCM, &samples(10, 1_000));
    harness.source.finish(utterance(1));
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
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(3, 1_000));
    harness
        .source
        .sound(utterance(1), &sound(40, 2_000), 0.5)
        .expect("the sound converts");
    let _ = harness.source.write(utterance(1), PCM, &samples(30, 1_000));
    harness.source.finish(utterance(1));
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));

    harness.source.cancel_all();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));
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
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(30, 1_000));
    harness
        .source
        .sound(utterance(1), &sound(3, 2_000), 1.0)
        .expect("the sound converts");
    harness.source.finish(utterance(1));
    harness.play(2);
    assert_eq!(harness.next(), started(1, trace));
    harness.source.cancel_all();
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Cancelled));
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
    let harness = harness();
    let earcons = harness.mixer.add_source(Arc::new(|_| {}));
    let trace = harness.speak(1, 30);
    harness.play(5);
    assert_eq!(harness.next(), started(1, trace));

    earcons
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

#[test]
fn a_sound_played_at_once_on_a_quiet_device_plays_alone() {
    let harness = harness();
    harness
        .source
        .play(&sound(4, 2_000), 2.0)
        .expect("the sound converts");
    harness.play(4);
    assert_frames(&harness.device.written(), &[level(2_000) * 2.0; 4]);
}

#[test]
fn a_partly_heard_sound_goes_on_where_it_was_after_the_device_reopens() {
    let harness = harness();
    // Ten frames, the device's whole queue, each a different level.
    let ramp: Vec<i16> = (1..=10).map(|frame| frame * 100).collect();
    let ramp_sound = Sound::from_pcm(PCM, ramp.clone()).expect("a usable sound");
    harness
        .source
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
