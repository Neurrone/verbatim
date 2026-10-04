//! Mixer tests (decision D17) against a device whose playback the test
//! advances by hand, so every ending and mark can be checked against
//! exactly how much audio has played.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use verbatim_audio::{
    AudioDevice, AudioError, DeviceFormat, Mixer, PcmFormat, PlaybackEvent, Source, Waker,
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
}

/// A device that plays only when the test says so.
#[derive(Clone, Default)]
struct ManualDevice {
    state: Arc<(Mutex<DeviceState>, Condvar)>,
    reopen: Arc<AtomicBool>,
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
}

impl AudioDevice for ManualDevice {
    fn open(&mut self) -> Result<DeviceFormat, AudioError> {
        self.reopen.store(false, Ordering::SeqCst);
        self.state.0.lock().unwrap().queued = 0;
        Ok(FORMAT)
    }

    fn queued_frames(&mut self) -> Result<u32, AudioError> {
        Ok(self.state.0.lock().unwrap().queued)
    }

    fn write(&mut self, samples: &[f32]) -> Result<(), AudioError> {
        let mut state = self.state.0.lock().unwrap();
        state.queued += u32::try_from(samples.len()).unwrap();
        state.written.extend_from_slice(samples);
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
        let state = state.lock().unwrap();
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
    _mixer: Mixer,
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
        _mixer: mixer,
    }
}

fn utterance(id: u64) -> UtteranceId {
    UtteranceId(id)
}

/// Samples whose value tells which frame of the test they are.
fn samples(count: usize, value: i16) -> Vec<i16> {
    vec![value; count]
}

impl Harness {
    fn speak(&self, id: u64, frames: usize) -> TraceId {
        let trace = TraceId::mint();
        self.source.register(utterance(id), trace);
        assert!(
            self.source
                .write(utterance(id), PCM, &samples(frames, 1_000))
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

    fn nothing_more(&self) {
        assert_eq!(
            self.events.recv_timeout(Duration::from_millis(200)).ok(),
            None,
            "no further playback event"
        );
    }

    /// Plays `frames` one at a time, each only once the mixer has written
    /// it (a device cannot play what it has not been given), and stops early
    /// if nothing more is written within a second.
    fn play(&self, frames: u32) {
        for _ in 0..frames {
            let deadline = std::time::Instant::now() + Duration::from_secs(1);
            while self.device.state.0.lock().unwrap().queued == 0 {
                if std::time::Instant::now() > deadline {
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
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
    // utterance's audio straight after the five frames that played.
    let third = harness.speak(3, 4);
    harness.play(1);
    assert_eq!(harness.next(), started(3, third));
    harness.play(3);
    assert_eq!(harness.next(), ended(3, third, UtteranceEnding::Completed));
    let written = harness.device.written();
    assert!(
        written.len() >= 14,
        "the third utterance was written after the cut"
    );
}

#[test]
fn a_failed_utterance_is_reported_failed_and_its_unheard_audio_dropped() {
    let harness = harness();
    let trace = TraceId::mint();
    harness.source.register(utterance(1), trace);
    let _ = harness.source.write(utterance(1), PCM, &samples(20, 1_000));
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
    harness.play(20);
    harness.nothing_more();
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
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "the write is held back"
    );
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
    let deadline = std::time::Instant::now() + WAIT;
    while harness.device.written().len() < 16 {
        assert!(
            std::time::Instant::now() < deadline,
            "the unplayed frames are written again"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    harness.nothing_more();
    harness.play(6);
    assert_eq!(harness.next(), ended(1, trace, UtteranceEnding::Completed));
    assert_eq!(harness.device.written()[10..], [1_000.0 / 32_768.0; 6]);
}
