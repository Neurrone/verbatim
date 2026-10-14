//! The host protocol of `docs/design/communication.md` section 7, with a
//! scripted driver whose timing the test decides: `HostedSynth` asks for
//! the next utterance right behind a cancel without waiting for the
//! cancelled one's end, and drops the cancelled one's frames wherever it
//! next reads; a driver waiting for a synthesis that never completes is
//! stopped by the cancel through the command event, with no timer; and a
//! cancel that comes while a frame larger than the pipe is pended leaves
//! that frame whole, so the host carries on; and a host frozen in its
//! driver is ended once it has owed an answer for the hang limit, rather
//! than a write to it waiting for good; and the host's first-chunk stage
//! runs from its taking the request in, so a request's wait in the host
//! behind a cancelled utterance is in it.
//!
//! This test binary runs itself as a synthesizer host: `HostedSynth`
//! launches it with the host's command line, which its own `main`
//! recognizes, and it serves Core through the host's own serving loop
//! (`verbatim_synth_host::serve`) with the scripted driver. Nothing is
//! played or shown.
//!
//! The scripted driver speaks text made of chunk lengths, such as "5 5":
//! for each chunk, it first waits on the test's gate, a named semaphore,
//! then pushes that many samples, plus the `rate` setting, each the
//! utterance's id times 100 plus the chunk's index. The text "never" makes
//! it wait as `OneCore` does, through `OneCore`'s own wait, for a
//! synthesis that never completes, after setting the test's named event.
//! The text "frozen" sets that event and then waits on the gate, which the
//! test never opens, reading no command: a host frozen in a native call.
//! A chunk "hold" pushes nothing: it waits at the gate, then reads the
//! commands that have come by asking whether it was cancelled, then
//! [`HANDSHAKES`] times sets the test's event and waits at the gate again;
//! it is held even once the utterance is cancelled, as a driver finishes
//! the chunk it is in.

use std::ops::ControlFlow;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_process::event::Event;
use verbatim_speech::hosting::now_us;
use verbatim_speech::{
    CancelToken, CancelWake, IndexMark, SettingDescriptor, SettingId, SettingValue, SpeechItem,
    SpeechSequence, SynthDriver, SynthError, SynthId, SynthSink, SynthTiming,
};
use verbatim_synth_host::{parse_args, serve};
use verbatim_synth_hosted::HostedSynth;
use verbatim_synth_onecore::{Waited, wait_for_synthesis};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    CreateEventW, CreateSemaphoreW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, OpenSemaphoreW,
    ReleaseSemaphore, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
};
use windows::core::HSTRING;

/// The audio format the scripted driver speaks in.
const FORMAT: PcmFormat = PcmFormat {
    sample_rate: 22_050,
    channels: 1,
};

/// The longest the test waits for the scripted driver to begin waiting;
/// it only bounds a hang.
const WAIT_LIMIT_MS: u32 = 20_000;

/// The gate's name for the script named `script`.
fn gate_name(script: &str) -> HSTRING {
    HSTRING::from(format!("verbatim-test-gate-{script}"))
}

/// The name of the event the driver sets as it begins waiting for a
/// synthesis that never completes.
fn waiting_name(script: &str) -> HSTRING {
    HSTRING::from(format!("verbatim-test-waiting-{script}"))
}

/// The value of every sample of chunk `index` of `utterance`.
fn value(utterance: u64, index: usize) -> i16 {
    i16::try_from(utterance * 100 + index as u64).expect("a small value")
}

/// Chunk `index` of `utterance`, `length` samples long.
fn chunk(utterance: u64, index: usize, length: usize) -> Vec<i16> {
    vec![value(utterance, index); length]
}

/// The scripted driver, in the stand-in host.
struct Scripted {
    gate: HANDLE,
    waiting: HANDLE,
    /// The host's command event.
    interrupt: Arc<Event>,
    rate: i32,
}

// SAFETY: the handles are the stand-in process's own, open for its life,
// and used only by the one thread that serves Core.
unsafe impl Send for Scripted {}

impl SynthDriver for Scripted {
    fn id(&self) -> SynthId {
        SynthId::new("scripted")
    }

    fn display_name(&self) -> String {
        "Scripted".to_owned()
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        vec![SettingDescriptor::standard_numeric("rate", "setting-rate")]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        (id.0 == "rate").then_some(SettingValue::Number(self.rate))
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("rate", SettingValue::Number(rate)) => {
                self.rate = rate;
                Ok(())
            }
            _ => Err(SynthError::Setting(format!("no setting {id}"))),
        }
    }

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        let text = sequence.text();
        if text == "never" {
            let never = Event::new().expect("an event");
            // SAFETY: the stand-in's open event handle.
            unsafe { SetEvent(self.waiting) }.expect("the test's event is set");
            return match wait_for_synthesis(&never, Some(&self.interrupt), sink)? {
                Waited::Cancelled => Ok(()),
                Waited::Completed => Err(SynthError::Synthesis(
                    "a synthesis that never completes completed".to_owned(),
                )),
            };
        }
        if text == "frozen" {
            // SAFETY: the stand-in's open event handle.
            unsafe { SetEvent(self.waiting) }.expect("the test's event is set");
            // SAFETY: the stand-in's open semaphore handle, which the test
            // never releases for this text.
            unsafe { WaitForSingleObject(self.gate, INFINITE) };
            return Ok(());
        }
        let extra = usize::try_from(self.rate).expect("a rate of 0 to 100");
        for (index, length) in text.split_whitespace().enumerate() {
            if length == "hold" {
                // SAFETY: the stand-in's open semaphore handle.
                let waited = unsafe { WaitForSingleObject(self.gate, WAIT_LIMIT_MS) };
                assert_eq!(waited, WAIT_OBJECT_0, "the gate opens");
                let cancelled = sink.is_cancelled();
                for _ in 0..HANDSHAKES {
                    // SAFETY: the stand-in's open event handle.
                    unsafe { SetEvent(self.waiting) }.expect("the test's event is set");
                    // SAFETY: as above.
                    let waited = unsafe { WaitForSingleObject(self.gate, WAIT_LIMIT_MS) };
                    assert_eq!(waited, WAIT_OBJECT_0, "the gate opens");
                }
                if cancelled {
                    return Ok(());
                }
                continue;
            }
            let length: usize = length.parse().expect("a chunk length");
            // SAFETY: the stand-in's open semaphore handle.
            let waited = unsafe { WaitForSingleObject(self.gate, WAIT_LIMIT_MS) };
            assert_eq!(waited, WAIT_OBJECT_0, "the gate opens");
            let samples = chunk(sequence.utterance.0, index, length + extra);
            if sink.push_pcm(FORMAT, &samples).is_break() {
                return Ok(());
            }
        }
        Ok(())
    }
}

/// The stand-in host: serves Core with the scripted driver.
fn stand_in(args: &[String]) -> ExitCode {
    let (pipe_in, pipe_out, script) = parse_args(args).expect("the host's command line");
    // SAFETY: the test's `HostedSynth` passes the values of the pipe ends
    // it created for this process, which inherited them and uses them
    // nowhere else.
    let (commands, replies) =
        unsafe { verbatim_process::inherited_pipe_handles(pipe_in, pipe_out) }
            .expect("the inherited pipes");
    // SAFETY: opening named objects the test created.
    let gate = unsafe { OpenSemaphoreW(SYNCHRONIZATION_SYNCHRONIZE, false, &gate_name(&script)) }
        .expect("the test's gate");
    // SAFETY: as above.
    let waiting = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &waiting_name(&script)) }
        .expect("the test's event");
    serve(commands, replies, |command_event| {
        Ok(Box::new(Scripted {
            gate,
            waiting,
            interrupt: Arc::clone(command_event),
            rate: 0,
        }))
    })
}

/// A script: its gate, which lets one chunk through per count, and its
/// waiting event, both named for it.
struct Script {
    name: String,
    gate: HANDLE,
    waiting: HANDLE,
}

impl Script {
    /// A new script whose gate lets `open` chunks through.
    fn new(open: i32) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let name = format!(
            "scripted-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        // SAFETY: a new named semaphore, closed in `drop`.
        let gate =
            unsafe { CreateSemaphoreW(None, open, i32::MAX, &gate_name(&name)) }.expect("a gate");
        // SAFETY: a new named auto-reset event, closed in `drop`.
        let waiting =
            unsafe { CreateEventW(None, false, false, &waiting_name(&name)) }.expect("an event");
        Self {
            name,
            gate,
            waiting,
        }
    }

    /// A `HostedSynth` running this test binary as its host.
    fn start(&self) -> HostedSynth {
        HostedSynth::start(
            std::env::current_exe().expect("the test binary"),
            SynthId::new(self.name.clone()),
        )
        .expect("the stand-in host starts")
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        // SAFETY: the semaphore this script owns, closed once.
        let _ = unsafe { CloseHandle(self.gate) };
        // SAFETY: the event this script owns, closed once.
        let _ = unsafe { CloseHandle(self.waiting) };
    }
}

fn sequence(id: u64, text: &str) -> SpeechSequence {
    SpeechSequence {
        utterance: UtteranceId(id),
        trace_id: TraceId::mint(),
        language: None,
        items: vec![SpeechItem::Text(text.to_owned())],
    }
}

/// When a test sink cancels its utterance itself.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CancelAt {
    /// Never.
    Never,
    /// At the host's first-audio stamps, before any audio is relayed.
    Stamps,
    /// At the first audio relayed.
    FirstAudio,
}

/// A sink that records the audio relayed, answers for its cancellation
/// from a token, and runs `on_register` when the driver registers its
/// waker, which `HostedSynth` does once its request is written.
struct TestSink {
    token: Arc<CancelToken>,
    cancel_at: CancelAt,
    received: Vec<Vec<i16>>,
    timing: Option<SynthTiming>,
    on_register: Option<Box<dyn FnOnce()>>,
}

impl TestSink {
    fn new(cancel_at: CancelAt) -> Self {
        Self {
            token: Arc::new(CancelToken::new()),
            cancel_at,
            received: Vec::new(),
            timing: None,
            on_register: None,
        }
    }
}

impl SynthSink for TestSink {
    fn push_pcm(&mut self, _format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        self.received.push(samples.to_vec());
        if self.cancel_at == CancelAt::FirstAudio {
            self.token.cancel();
        }
        ControlFlow::Continue(())
    }

    fn index_reached(&mut self, mark: IndexMark) {
        panic!("no mark is scripted, but {mark:?} was relayed");
    }

    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    fn synth_timing(&mut self, timing: SynthTiming) {
        self.timing = Some(timing);
        if self.cancel_at == CancelAt::Stamps {
            self.token.cancel();
        }
    }

    fn on_cancel(&mut self, wake: CancelWake) {
        self.token.on_cancel(wake);
        if let Some(on_register) = self.on_register.take() {
            on_register();
        }
    }
}

/// A cancel during an utterance, followed by a new one: the new one's
/// `Speak` is sent before the old one's `Done` is read, and none of the
/// old one's audio is relayed after the cancel. The old one's second chunk
/// waits at the gate, so the host cannot end it until the gate opens, and
/// the gate opens only once the new utterance's request is written; were
/// the driver to wait for the old one's `Done` first, it would wait for the
/// whole of its hang limit and fail. The old one's last frames, and its
/// `Done`, are dropped as the new one is read.
fn a_new_utterance_goes_right_behind_a_cancel() {
    let script = Script::new(1);
    let mut synth = script.start();
    let pid = synth.process_id();

    let mut old = TestSink::new(CancelAt::FirstAudio);
    synth
        .speak(&sequence(1, "5 5"), &mut old)
        .expect("a cancelled utterance is not a failure");
    assert_eq!(old.received, [chunk(1, 0, 5)]);

    let mut new = TestSink::new(CancelAt::Never);
    let gate = script.gate;
    // The old utterance's second chunk, then the new one's two.
    new.on_register = Some(Box::new(move || {
        // SAFETY: the script's semaphore, open while the script lives.
        unsafe { ReleaseSemaphore(gate, 3, None) }.expect("the gate opens");
    }));
    synth
        .speak(&sequence(2, "5 5"), &mut new)
        .expect("the new utterance is spoken");
    assert_eq!(new.received, [chunk(2, 0, 5), chunk(2, 1, 5)]);
    assert_eq!(old.received, [chunk(1, 0, 5)]);
    assert_eq!(synth.process_id(), pid, "the same host speaks both");
}

/// Speaks `sequence` with `sink`, which another thread cancels once the
/// scripted driver has set the script's event as it begins to wait.
fn speak_cancelled_once_waiting(
    script: &Script,
    synth: &mut HostedSynth,
    sink: &mut TestSink,
    sequence: &SpeechSequence,
) {
    let token = Arc::clone(&sink.token);
    let waiting = waiting_name(&script.name);
    let canceller = thread::spawn(move || {
        // SAFETY: opening the script's event by name; closed below.
        let event = unsafe { OpenEventW(SYNCHRONIZATION_SYNCHRONIZE, false, &waiting) }
            .expect("the script's event");
        // SAFETY: the handle just opened.
        let waited = unsafe { WaitForSingleObject(event, WAIT_LIMIT_MS) };
        // SAFETY: closed once.
        let _ = unsafe { CloseHandle(event) };
        token.cancel();
        waited == WAIT_OBJECT_0
    });
    synth
        .speak(sequence, sink)
        .expect("a cancelled utterance is not a failure");
    assert!(
        canceller.join().expect("the canceller"),
        "the driver began to wait before the cancel"
    );
}

/// A cancel before any audio stops a driver waiting, as `OneCore` does,
/// for a synthesis that never completes: the command event wakes its
/// wait, with no timer. The cancel comes from another thread once the
/// driver has begun to wait. The host's `Done` for the cancelled utterance
/// comes before anything of the next, which the test waits for, bounded
/// only by the driver's hang limit.
fn a_cancel_stops_a_synthesis_that_never_completes() {
    let script = Script::new(1_000);
    let mut synth = script.start();
    let pid = synth.process_id();

    let mut never = TestSink::new(CancelAt::Never);
    speak_cancelled_once_waiting(&script, &mut synth, &mut never, &sequence(1, "never"));
    assert_eq!(never.received, Vec::<Vec<i16>>::new());

    let mut next = TestSink::new(CancelAt::Never);
    synth
        .speak(&sequence(2, "5"), &mut next)
        .expect("the next utterance is spoken");
    assert_eq!(next.received, [chunk(2, 0, 5)]);
    assert_eq!(synth.process_id(), pid, "the same host speaks both");
}

/// The length of a chunk whose frame is far larger than the pipe's 4 KiB
/// buffer and the driver's 4 KiB read.
const LARGE: usize = 40_000;

/// Cancels an utterance whose one chunk's frame is much larger than the
/// pipe can hold, at the host's first-audio stamps, which come in the same
/// write: the driver stops reading there, so the rest of the write stays
/// pended in the host when the cancel reaches it.
fn cancel_with_a_large_frame_pended(synth: &mut HostedSynth) {
    let mut old = TestSink::new(CancelAt::Stamps);
    synth
        .speak(&sequence(1, &LARGE.to_string()), &mut old)
        .expect("a cancelled utterance is not a failure");
    assert_eq!(old.received, Vec::<Vec<i16>>::new());
}

/// A host cancelled while it waits for a frame larger than its pipe's
/// buffer to be taken finishes that frame whole, so the next utterance is
/// delivered whole by the same host, with no restart.
fn a_frame_pended_at_a_cancel_is_finished_whole() {
    let script = Script::new(1_000);
    let mut synth = script.start();
    let pid = synth.process_id();
    cancel_with_a_large_frame_pended(&mut synth);

    let mut next = TestSink::new(CancelAt::Never);
    synth
        .speak(&sequence(2, "5 5"), &mut next)
        .expect("the next utterance is spoken");
    assert_eq!(next.received, [chunk(2, 0, 5), chunk(2, 1, 5)]);
    assert_eq!(synth.process_id(), pid, "the same host speaks both");
}

/// A setting applied right after a cancel is answered without the
/// cancelled utterance's frames, the rest of its large chunk and its
/// `Done`, being taken for the answer; the setting then shapes the next
/// utterance, which the same host speaks.
fn a_setting_after_a_cancel_is_answered_past_the_cancelled_frames() {
    let script = Script::new(1_000);
    let mut synth = script.start();
    let pid = synth.process_id();
    cancel_with_a_large_frame_pended(&mut synth);

    synth
        .set_setting(&SettingId::new("rate"), SettingValue::Number(3))
        .expect("the setting is applied");
    let mut next = TestSink::new(CancelAt::Never);
    synth
        .speak(&sequence(2, "5"), &mut next)
        .expect("the next utterance is spoken");
    assert_eq!(next.received, [chunk(2, 0, 8)]);
    assert_eq!(synth.process_id(), pid, "the same host is kept");
}

/// How long the test waits for a request to a frozen host to end; it only
/// bounds a hang, and is well past the hang limit the request ends at.
const FROZEN_LIMIT: Duration = Duration::from_secs(40);

/// A host frozen in its driver, its utterance cancelled, is judged hung and
/// ended once it has owed that utterance's end for the hang limit, rather
/// than the next request waiting for good on a write the host never takes.
/// The utterance is cancelled once the driver is frozen, so the host reads
/// the cancel with no thread left to act on it; the next `Speak` is larger
/// than the command pipe's buffer and the host's read together, so its
/// write can finish only by the host reading, which it never does. That
/// request fails as a hung host's, and the host is gone. The request runs
/// on its own thread so that a write waiting for good fails the test at
/// the bound instead of hanging it.
fn a_frozen_host_is_ended_rather_than_waited_for_for_good() {
    let script = Script::new(0);
    let mut synth = script.start();
    let mut frozen = TestSink::new(CancelAt::Never);
    speak_cancelled_once_waiting(&script, &mut synth, &mut frozen, &sequence(1, "frozen"));
    assert_eq!(frozen.received, Vec::<Vec<i16>>::new());

    let (ended_tx, ended) = mpsc::channel();
    thread::spawn(move || {
        let mut next = TestSink::new(CancelAt::Never);
        let result = synth.speak(&sequence(2, &"5 ".repeat(10_000)), &mut next);
        let _ = ended_tx.send((
            result.map_err(|error| error.to_string()),
            next.received,
            synth.process_id(),
        ));
    });
    let (result, received, process) = ended
        .recv_timeout(FROZEN_LIMIT)
        .expect("the request to a frozen host ends");
    assert_eq!(
        result,
        Err("synthesis failed: the synthesizer host sent nothing for 10s".to_owned())
    );
    assert_eq!(received, Vec::<Vec<i16>>::new());
    assert_eq!(process, None, "the frozen host was ended");
}

/// How many times a "hold" hands over to the test and back once it has
/// read the commands: each is a wake in each process, so together they
/// keep the host in the old utterance, with the new request taken in, far
/// longer than the scripted driver takes to its first chunk.
const HANDSHAKES: usize = 20;

/// The host's first-chunk stage runs from its taking the request in
/// (`docs/design/communication.md`, section 10.1), so the time a request
/// waits in the host behind a cancelled utterance is in it, and the stages
/// before the first chunk leave no gap. The old utterance's second chunk
/// holds the host at the gate, which the new utterance's sink opens once
/// the new request is written; the host then reads the commands waiting,
/// the new request among them, and hands over to the sink and back
/// [`HANDSHAKES`] times. At the last, the sink stamps the shared clock and
/// opens the gate for the rest of the hold and the new utterance's chunk.
/// So the host took the new request in before that stamp and began it only
/// after, and the trip to the host and the first chunk, which together run
/// from the request's write to the first chunk, span at least from the
/// sink's first stamp, taken after the write, to its second.
fn a_request_waiting_in_the_host_is_in_its_first_chunk() {
    let script = Script::new(1);
    let mut synth = script.start();
    let pid = synth.process_id();

    let mut old = TestSink::new(CancelAt::FirstAudio);
    synth
        .speak(&sequence(1, "5 hold"), &mut old)
        .expect("a cancelled utterance is not a failure");
    assert_eq!(old.received, [chunk(1, 0, 5)]);

    let mut new = TestSink::new(CancelAt::Never);
    let (gate, waiting) = (script.gate, script.waiting);
    let stamps = Arc::new(std::sync::Mutex::new(None));
    let taken = Arc::clone(&stamps);
    new.on_register = Some(Box::new(move || {
        let written = now_us();
        // SAFETY: the script's semaphore, open while the script lives.
        unsafe { ReleaseSemaphore(gate, 1, None) }.expect("the gate opens");
        for handshake in 1..=HANDSHAKES {
            // SAFETY: the script's event, open while the script lives.
            let waited = unsafe { WaitForSingleObject(waiting, WAIT_LIMIT_MS) };
            assert_eq!(waited, WAIT_OBJECT_0, "the host read the new request");
            if handshake == HANDSHAKES {
                let held = now_us();
                *taken.lock().expect("the stamps") = Some((written, held));
                // SAFETY: as above.
                unsafe { ReleaseSemaphore(gate, 2, None) }.expect("the gate opens");
            } else {
                // SAFETY: as above.
                unsafe { ReleaseSemaphore(gate, 1, None) }.expect("the gate opens");
            }
        }
    }));
    synth
        .speak(&sequence(2, "5"), &mut new)
        .expect("the new utterance is spoken");
    assert_eq!(new.received, [chunk(2, 0, 5)]);
    assert_eq!(synth.process_id(), pid, "the same host speaks both");
    let timing = new.timing.expect("the first audio's stages");
    let (written, held) = stamps
        .lock()
        .expect("the stamps")
        .expect("the sink stamped");
    assert!(
        timing.to_host_us + timing.first_chunk_us >= held - written,
        "{timing:?} leaves out part of the {} us from the request's write to the host beginning it",
        held - written
    );
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--pipe-in") {
        return stand_in(&args);
    }
    // libtest's own options, such as a name filter, are accepted and
    // ignored: the binary runs all its tests.
    let tests: [(&str, fn()); 6] = [
        (
            "a_new_utterance_goes_right_behind_a_cancel",
            a_new_utterance_goes_right_behind_a_cancel,
        ),
        (
            "a_cancel_stops_a_synthesis_that_never_completes",
            a_cancel_stops_a_synthesis_that_never_completes,
        ),
        (
            "a_frame_pended_at_a_cancel_is_finished_whole",
            a_frame_pended_at_a_cancel_is_finished_whole,
        ),
        (
            "a_setting_after_a_cancel_is_answered_past_the_cancelled_frames",
            a_setting_after_a_cancel_is_answered_past_the_cancelled_frames,
        ),
        (
            "a_frozen_host_is_ended_rather_than_waited_for_for_good",
            a_frozen_host_is_ended_rather_than_waited_for_for_good,
        ),
        (
            "a_request_waiting_in_the_host_is_in_its_first_chunk",
            a_request_waiting_in_the_host_is_in_its_first_chunk,
        ),
    ];
    for (name, test) in tests {
        test();
        println!("test {name} ... ok");
    }
    ExitCode::SUCCESS
}
