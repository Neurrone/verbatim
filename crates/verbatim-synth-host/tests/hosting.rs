//! The synthesizer host end to end (decision D18): `HostedSynth` driving the
//! real `verbatim-synth-host.exe` with the `OneCore` synthesizer, which
//! every Windows 11 machine and GitHub's Windows runners have, or with
//! eSpeak NG.
//!
//! Which `OneCore` voice speaks depends on the machine, so no test expects
//! particular samples. A new host's first utterance is reproducible,
//! though: the same text with the same settings gives the same samples on
//! one machine, which is what lets the tests compare speech exactly. (A
//! later utterance of the same host need not: `OneCore` carries state from
//! one utterance to the next.)

use std::cell::Cell;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use verbatim_audio::PcmFormat;
use verbatim_model::{TraceId, UtteranceId};
use verbatim_speech::{
    IndexMark, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver, SynthError,
    SynthId, SynthSink,
};
use verbatim_synth_hosted::HostedSynth;
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

/// The longest a cancelled utterance may take to end, from the sink asking
/// to stop to `speak` returning: one round trip to the host, which ends its
/// synthesis and answers. Measured at 0.06 to 0.6 ms on the development
/// machine; the budget leaves room for a slower runner while staying well
/// under what a listener would notice.
const CANCEL_BUDGET: Duration = Duration::from_millis(20);

/// The longest a killed host may take to exit; it only bounds a hang.
const EXIT_TIMEOUT_MS: u32 = 10_000;

fn host_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_verbatim-synth-host"))
}

fn onecore() -> HostedSynth {
    HostedSynth::start(host_exe(), SynthId::new("onecore")).expect("the host starts")
}

fn sequence(id: u64, items: Vec<SpeechItem>) -> SpeechSequence {
    SpeechSequence {
        utterance: UtteranceId(id),
        trace_id: TraceId::mint(),
        language: None,
        items,
    }
}

fn text(text: &str) -> Vec<SpeechItem> {
    vec![SpeechItem::Text(text.to_owned())]
}

/// What a sink received, in order.
#[derive(Debug, PartialEq, Eq)]
enum Received {
    Audio(Vec<i16>),
    Mark(IndexMark),
}

/// Every sample received, in order.
fn samples(received: &[Received]) -> Vec<i16> {
    received
        .iter()
        .filter_map(|item| match item {
            Received::Audio(samples) => Some(samples.as_slice()),
            Received::Mark(_) => None,
        })
        .flatten()
        .copied()
        .collect()
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
        self.received.push(Received::Audio(samples.to_vec()));
        (self.on_audio)()
    }

    fn index_reached(&mut self, mark: IndexMark) {
        self.received.push(Received::Mark(mark));
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled
    }
}

/// A sink that takes everything.
fn collect_all() -> Collect<impl FnMut() -> ControlFlow<()>> {
    Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || ControlFlow::Continue(()),
    }
}

/// Everything `synth` relays for `items`.
fn spoken(synth: &mut HostedSynth, id: u64, items: Vec<SpeechItem>) -> Vec<Received> {
    let mut sink = collect_all();
    synth
        .speak(&sequence(id, items), &mut sink)
        .expect("speaks");
    sink.received
}

/// Ends the process `pid` and waits until it has exited.
fn kill(pid: u32) {
    // SAFETY: opening a process by id; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) }
        .expect("opens the host process");
    // SAFETY: the handle just opened, with terminate access.
    unsafe { TerminateProcess(process, 1) }.expect("ends the host process");
    // SAFETY: the handle just opened, with synchronize access.
    let exited = unsafe { WaitForSingleObject(process, EXIT_TIMEOUT_MS) };
    // SAFETY: the handle opened above, closed once.
    unsafe { CloseHandle(process) }.expect("closes the process handle");
    assert_eq!(exited, WAIT_OBJECT_0, "the host process exits");
}

#[test]
fn a_hosted_synthesizer_streams_audio_and_marks_in_order() {
    let mut synth = onecore();
    assert!(synth.places_marks(), "OneCore places marks itself");
    let received = spoken(
        &mut synth,
        1,
        vec![
            SpeechItem::Text("first".to_owned()),
            SpeechItem::Mark(IndexMark(1)),
            SpeechItem::Text("second".to_owned()),
        ],
    );
    let marks: Vec<usize> = received
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, Received::Mark(_)))
        .map(|(index, _)| index)
        .collect();
    let [mark] = marks[..] else {
        panic!("exactly one mark is reported: {marks:?}");
    };
    assert_eq!(received[mark], Received::Mark(IndexMark(1)));
    assert!(
        !samples(&received[..mark]).is_empty() && !samples(&received[mark..]).is_empty(),
        "the mark falls between the two words' audio"
    );
}

/// `OneCore` speaks a pitch change inside its SSML: the host says so, and a
/// raised capital is spoken at the raised pitch, neither refused, ignored,
/// nor read aloud. Each utterance is a new host's first, so the same letter
/// twice is the same speech, and the raised letter's differing from it is
/// the markup's doing; and it is shorter than the markup read aloud as
/// text.
#[test]
fn onecore_speaks_a_raised_capital_within_one_utterance() {
    assert!(onecore().changes_pitch(), "OneCore changes pitch itself");
    let first = |items| samples(&spoken(&mut onecore(), 1, items));
    let plain = first(text("B"));
    assert_eq!(first(text("B")), plain);
    let raised = first(vec![
        SpeechItem::Pitch(30),
        SpeechItem::Text("B".to_owned()),
        SpeechItem::Pitch(0),
    ]);
    assert!(raised != plain, "the pitch markup changes the speech");
    let read_aloud = first(text("<prosody pitch=\"30%\">B</prosody>"));
    assert!(
        raised.len() < read_aloud.len(),
        "the markup is not read aloud: {} samples against {}",
        raised.len(),
        read_aloud.len()
    );
}

/// A host killed while it streams an utterance fails that utterance; the
/// next one is spoken by a new host, which was given the old one's rate:
/// its speech is exactly that of a host set to the same rate, and not
/// that of a host left at the default.
#[test]
fn a_host_that_dies_fails_its_utterance_and_the_next_gets_a_new_host_with_the_same_settings() {
    let rate = SettingId::new("rate");
    let mut synth = onecore();
    synth
        .set_setting(&rate, SettingValue::Number(70))
        .expect("sets the rate");
    let first_pid = synth.process_id().expect("a host is running");

    // The host is ended, and has exited, before the first audio is taken,
    // and the utterance is long enough that the pipe cannot hold the rest
    // of it.
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
            text("a sentence long enough to still be streaming when its host dies"),
        ),
        &mut sink,
    );
    assert!(
        matches!(result, Err(SynthError::Synthesis(_))),
        "the utterance fails: {result:?}"
    );

    let again = samples(&spoken(&mut synth, 2, text("again")));
    let second_pid = synth.process_id().expect("a new host is running");
    assert_ne!(first_pid, second_pid);

    let mut at_70 = onecore();
    at_70
        .set_setting(&rate, SettingValue::Number(70))
        .expect("sets the rate");
    assert!(
        again == samples(&spoken(&mut at_70, 1, text("again"))),
        "the new host speaks as a host set to the same rate"
    );
    assert!(
        again != samples(&spoken(&mut onecore(), 1, text("again"))),
        "the rate changes the speech"
    );
}

#[test]
fn a_cancelled_utterance_ends_promptly_and_the_host_carries_on() {
    let mut synth = onecore();
    let pid = synth.process_id().expect("a host is running");
    let asked_to_stop = Cell::new(None);
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: false,
        on_audio: || {
            asked_to_stop.set(Some(Instant::now()));
            ControlFlow::Break(())
        },
    };
    synth
        .speak(
            &sequence(
                1,
                text("a long sentence that is cut off after its first chunk of audio"),
            ),
            &mut sink,
        )
        .expect("a cancelled utterance is not a failure");
    let took = asked_to_stop.get().expect("audio arrived").elapsed();
    assert_eq!(
        sink.received.len(),
        1,
        "nothing is relayed after the cancel"
    );
    assert!(
        took <= CANCEL_BUDGET,
        "the utterance ended {took:?} after the cancel"
    );

    assert_ne!(spoken(&mut synth, 2, text("next")), []);
    assert_eq!(
        synth.process_id(),
        Some(pid),
        "the same host speaks the next utterance"
    );
}

#[test]
fn an_utterance_cancelled_before_any_audio_ends_without_any_and_the_host_carries_on() {
    let mut synth = onecore();
    let pid = synth.process_id().expect("a host is running");
    let mut sink = Collect {
        received: Vec::new(),
        cancelled: true,
        on_audio: || ControlFlow::Continue(()),
    };
    let text_ = "a long paragraph that would take OneCore a while to synthesize, ".repeat(20);
    synth
        .speak(&sequence(1, text(&text_)), &mut sink)
        .expect("a cancelled utterance is not a failure");
    assert_eq!(
        sink.received,
        [],
        "nothing is relayed for a cancelled utterance"
    );
    assert_eq!(synth.process_id(), Some(pid), "the host is kept");

    // Nothing of the cancelled utterance is left on the pipe for the next.
    assert_ne!(spoken(&mut synth, 2, text("next")), []);
    assert_eq!(synth.process_id(), Some(pid));
}

#[test]
fn espeak_ng_speaks_from_a_folder_with_a_non_ascii_name() {
    // The host and eSpeak NG's data, side by side as deployed, in a folder
    // whose name the ANSI code page cannot represent.
    let built = host_exe();
    let folder = std::env::temp_dir().join(format!("verbatim-Zoë-日本-{}", std::process::id()));
    if folder.exists() {
        std::fs::remove_dir_all(&folder).expect("removes an earlier run's folder");
    }
    copy_tree(
        &built.with_file_name("espeak-ng-data"),
        &folder.join("espeak-ng-data"),
    );
    let exe = folder.join("verbatim-synth-host.exe");
    std::fs::copy(&built, &exe).expect("copies the host");

    let mut synth = HostedSynth::start(exe, SynthId::new("espeak")).expect("eSpeak NG starts");
    let received = spoken(&mut synth, 1, text("hello"));
    drop(synth);
    std::fs::remove_dir_all(&folder).expect("removes the folder");
    assert!(!samples(&received).is_empty(), "eSpeak NG speaks");
}

/// A host that died between utterances, and has exited, is found gone
/// before the next request, which a new host speaks.
#[test]
fn a_host_that_died_while_idle_is_replaced_before_the_next_utterance() {
    let mut synth =
        HostedSynth::start(host_exe(), SynthId::new("espeak")).expect("the host starts");
    let first_pid = synth.process_id().expect("a host is running");
    kill(first_pid);
    assert!(
        !samples(&spoken(&mut synth, 1, text("hello"))).is_empty(),
        "the utterance is spoken"
    );
    let second_pid = synth.process_id().expect("a new host is running");
    assert_ne!(second_pid, first_pid);
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
