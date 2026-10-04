//! The synthesizer host end to end (decision D18): `HostedSynth` driving the
//! real `verbatim-synth-host.exe` with the `OneCore` synthesizer, which
//! every Windows 11 machine and GitHub's Windows runners have.

use std::ops::ControlFlow;
use std::path::PathBuf;

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_speech::{
    IndexMark, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver, SynthError,
    SynthId, SynthSink,
};
use verbatim_synth_hosted::HostedSynth;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

fn host_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_verbatim-synth-host"))
}

fn sequence(id: u64, items: Vec<SpeechItem>) -> SpeechSequence {
    SpeechSequence {
        utterance: UtteranceId(id),
        trace_id: TraceId::mint(),
        language: None,
        items,
    }
}

/// What a sink received, in order.
#[derive(Debug, PartialEq, Eq)]
enum Received {
    Audio(usize),
    Mark(IndexMark),
}

/// Collects everything; calls `on_audio` at each push and stops when it
/// says so.
struct Collect<F: FnMut() -> ControlFlow<()>> {
    received: Vec<Received>,
    on_audio: F,
    cancelled: bool,
}

impl<F: FnMut() -> ControlFlow<()>> SynthSink for Collect<F> {
    fn push_pcm(&mut self, _format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        self.received.push(Received::Audio(samples.len()));
        (self.on_audio)()
    }

    fn index_reached(&mut self, mark: IndexMark) {
        self.received.push(Received::Mark(mark));
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

fn kill(pid: u32) {
    // SAFETY: the handle is used once and closed.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, false, pid).expect("opens the host process");
        TerminateProcess(process, 1).expect("ends the host process");
        let _ = CloseHandle(process);
    }
}

#[test]
fn a_hosted_synthesizer_streams_audio_and_marks_in_order() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts");
    assert!(synth.places_marks(), "OneCore places marks itself");
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(
                1,
                vec![
                    SpeechItem::Text("first".to_owned()),
                    SpeechItem::Mark(IndexMark(1)),
                    SpeechItem::Text("second".to_owned()),
                ],
            ),
            &mut sink,
        )
        .expect("speaks");
    let mark = sink
        .received
        .iter()
        .position(|item| *item == Received::Mark(IndexMark(1)))
        .expect("the mark is reported");
    assert!(
        sink.received[..mark]
            .iter()
            .any(|item| matches!(item, Received::Audio(_)))
            && sink.received[mark..]
                .iter()
                .any(|item| matches!(item, Received::Audio(_))),
        "the mark falls between the two words' audio: {:?}",
        sink.received
    );
}

/// `OneCore` speaks a pitch change inside its SSML: the host says so, and a
/// raised capital is spoken rather than refused.
#[test]
fn onecore_speaks_a_raised_capital_within_one_utterance() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts");
    assert!(synth.changes_pitch(), "OneCore changes pitch itself");
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(
                1,
                vec![
                    SpeechItem::Pitch(30),
                    SpeechItem::Text("B".to_owned()),
                    SpeechItem::Pitch(0),
                ],
            ),
            &mut sink,
        )
        .expect("speaks");
    assert!(
        sink.received
            .iter()
            .any(|item| matches!(item, Received::Audio(_))),
        "the capital is heard"
    );
}

#[test]
fn a_host_that_dies_fails_its_utterance_and_the_next_gets_a_new_host_with_the_same_settings() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts");
    let rate = SettingId::new("rate");
    synth
        .set_setting(&rate, SettingValue::Number(70))
        .expect("sets the rate");
    let first_pid = synth.process_id().expect("a host is running");

    let mut killed = false;
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || {
            if !killed {
                killed = true;
                kill(first_pid);
            }
            ControlFlow::Continue(())
        },
    };
    let result = synth.speak(
        &sequence(
            1,
            vec![SpeechItem::Text(
                "a sentence long enough to still be streaming when its host dies".to_owned(),
            )],
        ),
        &mut sink,
    );
    assert!(
        matches!(result, Err(SynthError::Synthesis(_))),
        "the utterance fails: {result:?}"
    );

    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(2, vec![SpeechItem::Text("again".to_owned())]),
            &mut sink,
        )
        .expect("the next utterance speaks");
    assert_ne!(sink.received, []);
    let second_pid = synth.process_id().expect("a new host is running");
    assert_ne!(first_pid, second_pid);
    assert_eq!(synth.setting(&rate), Some(SettingValue::Number(70)));
}

#[test]
fn a_cancelled_utterance_ends_promptly_and_the_host_carries_on() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts");
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Break(()),
    };
    synth
        .speak(
            &sequence(
                1,
                vec![SpeechItem::Text(
                    "a long sentence that is cut off after its first chunk of audio".to_owned(),
                )],
            ),
            &mut sink,
        )
        .expect("a cancelled utterance is not a failure");
    assert_eq!(
        sink.received.len(),
        1,
        "nothing is relayed after the cancel"
    );

    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(2, vec![SpeechItem::Text("next".to_owned())]),
            &mut sink,
        )
        .expect("the same host speaks the next utterance");
}

#[test]
fn an_utterance_cancelled_before_any_audio_ends_without_any_and_the_host_carries_on() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts");
    let pid = synth.process_id();
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: true,
        on_audio: || ControlFlow::Continue(()),
    };
    let text = "a long paragraph that would take OneCore a while to synthesize, ".repeat(20);
    synth
        .speak(&sequence(1, vec![SpeechItem::Text(text)]), &mut sink)
        .expect("a cancelled utterance is not a failure");
    assert_eq!(
        sink.received,
        [],
        "nothing is relayed for a cancelled utterance"
    );
    assert_eq!(synth.process_id(), pid, "the host is kept");

    // Nothing of the cancelled utterance is left on the pipe for the next.
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(2, vec![SpeechItem::Text("next".to_owned())]),
            &mut sink,
        )
        .expect("the same host speaks the next utterance");
    assert_ne!(sink.received, []);
    assert_eq!(synth.process_id(), pid);
}

#[test]
fn espeak_ng_speaks_from_a_folder_with_a_non_ascii_name() {
    // The host and eSpeak NG's data, side by side as deployed, in a folder
    // whose name the ANSI code page cannot represent.
    let built = host_exe();
    let folder = std::env::temp_dir().join(format!("verbatim-Zoë-日本-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    copy_tree(
        &built.with_file_name("espeak-ng-data"),
        &folder.join("espeak-ng-data"),
    );
    let exe = folder.join("verbatim-synth-host.exe");
    std::fs::copy(&built, &exe).expect("copies the host");

    let mut synth = HostedSynth::start(exe, SynthId::new("espeak")).expect("eSpeak NG starts");
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(1, vec![SpeechItem::Text("hello".to_owned())]),
            &mut sink,
        )
        .expect("speaks");
    assert_ne!(sink.received, []);
    drop(synth);
    let _ = std::fs::remove_dir_all(&folder);
}

#[test]
fn a_host_that_died_while_idle_is_replaced_before_the_next_utterance() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("espeak")).expect("the host starts");
    let first_pid = synth.process_id().expect("a host is running");
    kill(first_pid);
    // The next request finds the host gone (or reaches it as it dies, and
    // is sent again), and the utterance is spoken by a new host, not lost.
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    };
    synth
        .speak(
            &sequence(1, vec![SpeechItem::Text("hello".to_owned())]),
            &mut sink,
        )
        .expect("the utterance is spoken");
    assert_ne!(sink.received, []);
    assert_ne!(synth.process_id(), Some(first_pid));
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("creates the folder");
    for entry in std::fs::read_dir(from).expect("reads the data") {
        let entry = entry.expect("reads an entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("reads a type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copies a file");
        }
    }
}
