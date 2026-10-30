//! Pipeline tests (architecture section 13, layer 3): priority lanes,
//! cancellation, and each utterance's single ending (decision D17), driven
//! by a step-controlled synth through the real mixer on a silent real-time
//! device; token rendering and the mark fallback asserted through the
//! capture synth; and the settings host round-tripped, its changes answered
//! and its commits saved on the synth thread.

use std::ops::ControlFlow;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, unbounded};
use verbatim_audio::{MixerBuilder, PcmFormat, PlaybackEvent, SilentDevice, SpeechLane};
use verbatim_model::{
    FocusNow, FocusValidity, KeySeq, NodeId, ObjectKey, Role, SegmentContent, SpeechMark,
    SpeechPriority, Theme, ThemeOptions, TraceId, Utterance, UtteranceEnding, UtteranceId,
    UtteranceSegment,
};
use verbatim_speech::{
    ActiveTheme, IndexMark, Presenter, SettingDescriptor, SettingId, SettingValue, SettingsAnswer,
    SettingsHost, SpeakToken, SpeechEvents, SpeechItem, SpeechManager, SpeechManagerConfig,
    SpeechSequence, SpeechSettingsHost, SpeechSettingsStore, SynthChoice, SynthDriver, SynthError,
    SynthFactory, SynthId, SynthRegistry, SynthSink,
};
use verbatim_synth_capture::{CaptureLog, CaptureSynth};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

const FORMAT: PcmFormat = PcmFormat {
    sample_rate: 22_050,
    channels: 1,
};

/// What the pipeline reported: every ending and every mark, in order.
#[derive(Default)]
struct Recorder {
    endings: Mutex<Vec<(UtteranceId, UtteranceEnding)>>,
    /// Signalled whenever an ending is reported.
    ended: Condvar,
    marks: Mutex<Vec<(UtteranceId, IndexMark)>>,
    /// Every pause, `true`, and resume, `false`, in the order reported.
    pauses: Mutex<Vec<bool>>,
    /// Signalled whenever a pause or resume is reported.
    paused: Condvar,
    /// Where the control synth hears how far playback has got.
    progress: Option<Sender<Progress>>,
    /// Where each utterance with marks cut off, or failed, is reported
    /// with its marks and whether it failed.
    cut: Option<Sender<Unheard>>,
}

/// An utterance with marks that ended unheard: its id, its marks, and
/// whether it failed (rather than being cut off).
type Unheard = (UtteranceId, Vec<IndexMark>, bool);

/// How far playback has got, as the control synth paces itself by it.
enum Progress {
    /// Playback reached a mark.
    Mark(UtteranceId, IndexMark),
    /// An utterance ended.
    Ended(UtteranceId),
}

impl SpeechEvents for Recorder {
    fn utterance_queued(&self, _: UtteranceId, _: TraceId, _: &str, _: Instant) {}

    fn speech_paused(&self, paused: bool, _: Instant) {
        self.pauses.lock().unwrap().push(paused);
        self.paused.notify_all();
    }

    fn audio_started(&self, _: UtteranceId, _: TraceId, _: Instant) {}

    fn mark_reached(&self, utterance: UtteranceId, _: TraceId, mark: IndexMark, _: Instant) {
        self.marks.lock().unwrap().push((utterance, mark));
        if let Some(progress) = &self.progress {
            let _ = progress.send(Progress::Mark(utterance, mark));
        }
    }

    fn marks_cancelled(&self, utterance: UtteranceId, _: TraceId, marks: &[IndexMark], _: Instant) {
        if let Some(cut) = &self.cut {
            let _ = cut.send((utterance, marks.to_vec(), false));
        }
    }

    fn marks_failed(&self, utterance: UtteranceId, _: TraceId, marks: &[IndexMark], _: Instant) {
        if let Some(cut) = &self.cut {
            let _ = cut.send((utterance, marks.to_vec(), true));
        }
    }

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        _: TraceId,
        ending: &UtteranceEnding,
        _: Instant,
    ) {
        self.endings
            .lock()
            .unwrap()
            .push((utterance, ending.clone()));
        self.ended.notify_all();
        if let Some(progress) = &self.progress {
            let _ = progress.send(Progress::Ended(utterance));
        }
    }
}

impl Recorder {
    /// Waits until at least `count` endings have been reported, then
    /// returns them all, in the order reported. Fails at [`STEP_TIMEOUT`].
    fn endings(&self, count: usize) -> Vec<(UtteranceId, UtteranceEnding)> {
        self.wait_until(
            |endings| endings.len() >= count,
            || format!("{count} endings"),
        )
    }

    /// Waits until `id` has ended, then returns every ending reported, in
    /// the order reported. Fails at [`STEP_TIMEOUT`].
    fn endings_through(&self, id: UtteranceId) -> Vec<(UtteranceId, UtteranceEnding)> {
        self.wait_until(
            |endings| endings.iter().any(|(utterance, _)| *utterance == id),
            || format!("the ending of {id:?}"),
        )
    }

    fn wait_until(
        &self,
        done: impl Fn(&[(UtteranceId, UtteranceEnding)]) -> bool,
        what: impl Fn() -> String,
    ) -> Vec<(UtteranceId, UtteranceEnding)> {
        let (endings, timeout) = self
            .ended
            .wait_timeout_while(self.endings.lock().unwrap(), STEP_TIMEOUT, |endings| {
                !done(endings)
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "waited {STEP_TIMEOUT:?} for {}; ended: {:?}",
            what(),
            *endings
        );
        endings.clone()
    }

    /// Waits until at least `count` pauses and resumes have been reported,
    /// then returns them all, in the order reported. Fails at
    /// [`STEP_TIMEOUT`].
    fn pauses(&self, count: usize) -> Vec<bool> {
        let (pauses, timeout) = self
            .paused
            .wait_timeout_while(self.pauses.lock().unwrap(), STEP_TIMEOUT, |pauses| {
                pauses.len() < count
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "waited {STEP_TIMEOUT:?} for {count} pauses and resumes; reported: {:?}",
            *pauses
        );
        pauses.clone()
    }

    /// The endings reported so far, in the order reported.
    fn ended_so_far(&self) -> Vec<(UtteranceId, UtteranceEnding)> {
        self.endings.lock().unwrap().clone()
    }
}

/// A synth whose `speak` announces its start and then streams quiet tone
/// until the test signals it to finish or the pipeline cancels it, and
/// announces its return. Text "fail" fails at once. Text "wait" pushes
/// nothing and waits for the cancel to wake it, as a driver waiting for its
/// synthesizer does ([`SynthSink::on_cancel`]), failing if nothing wakes it.
struct ControlSynth {
    started: Sender<String>,
    finish: Receiver<()>,
    returned: Sender<String>,
    /// How far playback has got, from the harness's [`Recorder`].
    progress: Receiver<Progress>,
}

impl ControlSynth {
    /// Streams tone for `utterance`, whose text is `text`, until told to
    /// finish or cancelled. It pushes one chunk at a time with a mark after
    /// it, and pushes the next once playback reaches that mark, so it stays
    /// just ahead of the device and never waits in the mixer for room. Until
    /// then it waits for the mark, for the test to say finish, or for the
    /// utterance to end, which a cancel does; the next push then learns of
    /// the cancel.
    fn stream(
        &self,
        utterance: UtteranceId,
        text: &str,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        if text == "fail" {
            return Err(SynthError::Synthesis("asked to fail".to_owned()));
        }
        if text == "wait" {
            let (woken_tx, woken) = crossbeam_channel::bounded::<()>(1);
            sink.on_cancel(Box::new(move || {
                let _ = woken_tx.try_send(());
            }));
            return woken.recv_timeout(STEP_TIMEOUT).map_err(|_| {
                SynthError::Synthesis("the cancel did not wake the control synth".to_owned())
            });
        }
        let chunk = [1_000i16; 16];
        let deadline = Instant::now() + STEP_TIMEOUT;
        for mark in 1.. {
            if let ControlFlow::Break(()) = sink.push_pcm(FORMAT, &chunk) {
                return Ok(());
            }
            let mark = IndexMark(mark);
            sink.index_reached(mark);
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                crossbeam_channel::select! {
                    recv(self.finish) -> _ => return Ok(()),
                    recv(self.progress) -> progress => match progress {
                        Ok(Progress::Mark(played, reached))
                            if played == utterance && reached == mark => break,
                        Ok(Progress::Ended(ended)) if ended == utterance => break,
                        Ok(_) => {}
                        Err(_) => return Ok(()),
                    },
                    default(remaining) => {
                        return Err(SynthError::Synthesis("control synth timed out".to_owned()));
                    }
                }
            }
        }
        Ok(())
    }
}

impl SynthDriver for ControlSynth {
    fn id(&self) -> SynthId {
        SynthId::new("control")
    }

    fn display_name(&self) -> String {
        "Control synth".to_owned()
    }

    fn supported_settings(&self) -> Vec<verbatim_speech::SettingDescriptor> {
        Vec::new()
    }

    fn setting(&self, _id: &SettingId) -> Option<SettingValue> {
        None
    }

    fn set_setting(&mut self, id: &SettingId, _value: SettingValue) -> Result<(), SynthError> {
        Err(SynthError::Setting(format!("no setting {id}")))
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
        self.started.send(text.clone()).unwrap();
        let result = self.stream(sequence.utterance, &text, sink);
        let _ = self.returned.send(text);
        result
    }
}

fn queued(text: &str) -> Utterance {
    utterance(text, SpeechPriority::Queued)
}

fn utterance(text: &str, priority: SpeechPriority) -> Utterance {
    Utterance {
        trace_id: TraceId::mint(),
        priority,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
        say_all: false,
        validity: None,
    }
}

fn mixer() -> MixerBuilder {
    MixerBuilder::new(Box::new(SilentDevice::new()))
}

type SavedStore = Arc<Mutex<Vec<(SynthId, Vec<(SettingId, SettingValue)>)>>>;

/// A synthesizer saved, whether as the user's choice, and its values.
type Persisted = (SynthId, bool, Vec<(SettingId, SettingValue)>);

/// The speech section as a test keeps it: what a synthesizer starts with,
/// read live from `saved` as a commit leaves it, and every commit recorded.
#[derive(Clone, Default)]
struct TestStore {
    saved: SavedStore,
    persisted: Arc<(Mutex<Vec<Persisted>>, Condvar)>,
}

impl SpeechSettingsStore for TestStore {
    fn saved(&self, synth: &SynthId) -> Vec<(SettingId, SettingValue)> {
        self.saved
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| id == synth)
            .map(|(_, values)| values.clone())
            .unwrap_or_default()
    }

    fn persist(&mut self, synth: &SynthId, chosen: bool, values: &[(SettingId, SettingValue)]) {
        let (persisted, changed) = &*self.persisted;
        persisted
            .lock()
            .unwrap()
            .push((synth.clone(), chosen, values.to_vec()));
        changed.notify_all();
    }
}

impl TestStore {
    /// Waits until at least `count` commits have been saved, then returns
    /// them all, in order. Fails at [`STEP_TIMEOUT`].
    fn persisted(&self, count: usize) -> Vec<Persisted> {
        let (persisted, changed) = &*self.persisted;
        let (persisted, timeout) = changed
            .wait_timeout_while(persisted.lock().unwrap(), STEP_TIMEOUT, |persisted| {
                persisted.len() < count
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "waited {STEP_TIMEOUT:?} for {count} commits; saved: {:?}",
            *persisted
        );
        persisted.clone()
    }
}

/// Every answer the synth thread gives `host`, as they come.
fn answers(host: &SettingsHost) -> Receiver<SettingsAnswer> {
    let (answer_tx, answers) = unbounded();
    host.answer_to(Box::new(move |answer| {
        let _ = answer_tx.send(answer);
    }));
    answers
}

/// The next answer. Fails at [`STEP_TIMEOUT`].
fn answer(answers: &Receiver<SettingsAnswer>) -> SettingsAnswer {
    answers
        .recv_timeout(STEP_TIMEOUT)
        .expect("the synth thread answers")
}

struct ControlHarness {
    manager: SpeechManager,
    token: SpeakToken,
    recorder: Arc<Recorder>,
    started: Receiver<String>,
    finish: Sender<()>,
    /// The text of each utterance the synth has returned from speaking.
    returned: Receiver<String>,
    /// A lane to another source of the mixer the manager plays through, for
    /// a test to play other audio alongside its speech, and its endings.
    probe: SpeechLane,
    probe_ended: Receiver<UtteranceEnding>,
}

fn control_manager() -> ControlHarness {
    control_manager_reporting(None)
}

/// [`control_manager`], reporting each utterance cut off with its marks to
/// `cut`.
fn control_manager_reporting(cut: Option<Sender<Unheard>>) -> ControlHarness {
    let (started_tx, started_rx) = unbounded::<String>();
    let (finish_tx, finish_rx) = unbounded::<()>();
    let (returned_tx, returned_rx) = unbounded::<String>();
    let (progress_tx, progress_rx) = unbounded::<Progress>();
    let mut mixer = mixer();
    let (ended_tx, probe_ended) = unbounded::<UtteranceEnding>();
    let probe_source = mixer.source(Arc::new(move |event, _| {
        if let PlaybackEvent::Ended { ending, .. } = event {
            let _ = ended_tx.send(ending);
        }
    }));
    let probe = mixer.speech_lane(probe_source);

    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("control"),
        "Control synth",
        Box::new(move || {
            Ok(Box::new(ControlSynth {
                started: started_tx.clone(),
                finish: finish_rx.clone(),
                returned: returned_tx.clone(),
                progress: progress_rx.clone(),
            }) as Box<dyn SynthDriver>)
        }),
    );

    let recorder = Arc::new(Recorder {
        progress: Some(progress_tx),
        cut,
        ..Recorder::default()
    });
    let (manager, token, _sounds) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("control"),
        settings: Box::new(TestStore::default()),
        mixer,
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: theme_without_sounds(),
        presenter: None,
        inbox: verbatim_speech::channel_inbox(),
    })
    .expect("pipeline starts");

    ControlHarness {
        manager,
        token,
        recorder,
        started: started_rx,
        finish: finish_tx,
        returned: returned_rx,
        probe,
        probe_ended,
    }
}

impl ControlHarness {
    /// Every ending reported before a marker utterance, spoken and finished
    /// now, ended, sorted by utterance; the marker's own ending is checked
    /// to be the last and the only one of it. The queue thread and the mixer
    /// each report endings in order, so any second ending of earlier speech
    /// would be reported before the marker's, and is caught here. The two
    /// threads' reports interleave as they run, so the endings before the
    /// marker are compared by utterance rather than in the order reported.
    fn endings_before_a_marker(&self) -> Vec<(UtteranceId, UtteranceEnding)> {
        let marker = self.manager.speak(&self.token, queued("marker"));
        assert_eq!(recv_started(&self.started), "marker");
        self.finish.send(()).unwrap();
        let mut endings = self.recorder.endings_through(marker);
        assert_eq!(
            endings.pop(),
            Some((marker, UtteranceEnding::Completed)),
            "the marker's ending is the last: {endings:?}"
        );
        endings.sort_by_key(|(utterance, _)| *utterance);
        endings
    }
}

/// The built-in default theme, with no sound files to play: the pipeline
/// tests hear words alone.
fn theme_without_sounds() -> ActiveTheme {
    ActiveTheme::new(Theme::builtin_default(), |_| None, ThemeOptions::default())
}

fn recv_started(started: &Receiver<String>) -> String {
    started.recv_timeout(STEP_TIMEOUT).expect("synth started")
}

fn capture_manager(
    presenter: Option<Box<dyn Presenter>>,
) -> (SpeechManager, SpeakToken, CaptureLog, Arc<Recorder>) {
    let log: CaptureLog = CaptureSynth::new().log();
    let log_for_factory = Arc::clone(&log);
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("capture"),
        "Capture synth",
        Box::new(move || {
            Ok(
                Box::new(CaptureSynth::with_log(Arc::clone(&log_for_factory)))
                    as Box<dyn SynthDriver>,
            )
        }),
    );
    let recorder = Arc::new(Recorder::default());
    let (manager, token, _sounds) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("capture"),
        settings: Box::new(TestStore::default()),
        mixer: mixer(),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: theme_without_sounds(),
        presenter,
        inbox: verbatim_speech::channel_inbox(),
    })
    .expect("pipeline starts");
    (manager, token, log, recorder)
}

#[test]
fn next_lane_jumps_ahead_of_queued_and_each_completes_once_played() {
    let harness = control_manager();

    let first = harness.manager.speak(&harness.token, queued("first"));
    assert_eq!(recv_started(&harness.started), "first");

    // With "first" streaming, enqueue a queued then a next utterance.
    let second = harness
        .manager
        .speak(&harness.token, queued("second-queued"));
    let third = harness.manager.speak(
        &harness.token,
        utterance("third-next", SpeechPriority::Next),
    );

    // Finish each; the pipeline must serve the next-lane utterance first.
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "third-next");
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "second-queued");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.recorder.endings(3),
        vec![
            (first, UtteranceEnding::Completed),
            (third, UtteranceEnding::Completed),
            (second, UtteranceEnding::Completed),
        ]
    );
}

#[test]
fn interrupt_cancels_current_and_queued_each_exactly_once() {
    let harness = control_manager();

    let current = harness.manager.speak(&harness.token, queued("current"));
    assert_eq!(recv_started(&harness.started), "current");

    // Queue a victim, then interrupt: the interrupt cancels "current" and
    // drops "queued-victim" before speaking.
    let victim = harness
        .manager
        .speak(&harness.token, queued("queued-victim"));
    let urgent = harness.manager.speak(
        &harness.token,
        utterance("urgent", SpeechPriority::Interrupt),
    );

    assert_eq!(recv_started(&harness.started), "urgent");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (current, UtteranceEnding::Cancelled),
            (victim, UtteranceEnding::Cancelled),
            (urgent, UtteranceEnding::Completed),
        ]
    );
}

/// An utterance of `text` between index marks `marks`, queued.
fn marked(text: &str, marks: [u64; 2]) -> Utterance {
    Utterance {
        segments: vec![
            UtteranceSegment::new(SegmentContent::Mark(SpeechMark(marks[0]))),
            UtteranceSegment::text(text),
            UtteranceSegment::new(SegmentContent::Mark(SpeechMark(marks[1]))),
        ],
        ..queued(text)
    }
}

/// Every utterance with marks that is cut off, the one being spoken and
/// one still waiting, is reported once with all its marks, so the reducer
/// waiting on them hears of the cut; an utterance with marks that plays to
/// its end, or one with none, is not.
#[test]
fn utterances_with_marks_cut_off_are_reported_with_their_marks() {
    let (cut_tx, cut_rx) = unbounded();
    let harness = control_manager_reporting(Some(cut_tx));
    let current = harness
        .manager
        .speak(&harness.token, marked("current", [1, 2]));
    assert_eq!(recv_started(&harness.started), "current");
    let waiting = harness
        .manager
        .speak(&harness.token, marked("waiting", [3, 4]));
    let _plain = harness.manager.speak(&harness.token, queued("plain"));
    let urgent = harness.manager.speak(
        &harness.token,
        utterance("urgent", SpeechPriority::Interrupt),
    );
    let mut reported = vec![
        cut_rx.recv_timeout(STEP_TIMEOUT).expect("a cut reported"),
        cut_rx
            .recv_timeout(STEP_TIMEOUT)
            .expect("a second cut reported"),
    ];
    reported.sort_by_key(|(utterance, _, _)| *utterance);
    assert_eq!(
        reported,
        vec![
            (current, vec![IndexMark(1), IndexMark(2)], false),
            (waiting, vec![IndexMark(3), IndexMark(4)], false),
        ]
    );
    assert_eq!(recv_started(&harness.started), "urgent");
    harness.finish.send(()).unwrap();
    // One with marks played to its end.
    let heard = harness
        .manager
        .speak(&harness.token, marked("heard", [5, 6]));
    assert_eq!(recv_started(&harness.started), "heard");
    harness.finish.send(()).unwrap();
    let endings = harness.recorder.endings_through(heard);
    assert_eq!(endings.last(), Some(&(heard, UtteranceEnding::Completed)));
    assert!(endings.contains(&(urgent, UtteranceEnding::Completed)));
    // The mixer hands the queue thread an ending before it reports it, so
    // everything the queue thread could report of "heard" is before this.
    let (done_tx, done_rx) = unbounded();
    harness.manager.after_queued(move || {
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(STEP_TIMEOUT)
        .expect("the queue thread answers");
    assert_eq!(cut_rx.try_recv().ok(), None);
}

/// An utterance with marks that fails is reported as failed, not cut, so
/// what waited on its marks carries on past it.
#[test]
fn an_utterance_with_marks_that_fails_is_reported_as_failed() {
    let (cut_tx, cut_rx) = unbounded();
    let harness = control_manager_reporting(Some(cut_tx));
    let failing = harness
        .manager
        .speak(&harness.token, marked("fail", [1, 2]));
    assert_eq!(recv_started(&harness.started), "fail");
    assert_eq!(
        cut_rx
            .recv_timeout(STEP_TIMEOUT)
            .expect("the failure reported"),
        (failing, vec![IndexMark(1), IndexMark(2)], true)
    );
}

#[test]
fn failed_synthesis_ends_the_utterance_as_failed_and_speech_carries_on() {
    let harness = control_manager();

    let failing = harness.manager.speak(&harness.token, queued("fail"));
    assert_eq!(recv_started(&harness.started), "fail");
    let next = harness.manager.speak(&harness.token, queued("next"));
    assert_eq!(recv_started(&harness.started), "next");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.recorder.endings(2),
        vec![
            (
                failing,
                UtteranceEnding::Failed("synthesis failed: asked to fail".to_owned())
            ),
            (next, UtteranceEnding::Completed),
        ]
    );
}

#[test]
fn renders_tokens_through_capture_synth() {
    let (manager, token, log, recorder) = capture_manager(None);

    let id = manager.speak(
        &token,
        Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments: vec![
                UtteranceSegment::text("Settings"),
                UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
            ],
            source: None,
            say_all: false,
            validity: None,
        },
    );

    assert_eq!(recorder.endings(1), vec![(id, UtteranceEnding::Completed)]);
    let requests = log.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].sequence.text(), "Settings menu item");
}

/// A theme that puts a mark after each segment.
struct MarkingTheme;

impl Presenter for MarkingTheme {
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence {
        let mut items = Vec::new();
        for (index, segment) in utterance.segments.iter().enumerate() {
            if let SegmentContent::Text(text) = &segment.content {
                items.push(SpeechItem::Text(text.clone()));
            }
            items.push(SpeechItem::Mark(IndexMark(index as u64 + 1)));
        }
        SpeechSequence {
            utterance: id,
            trace_id: utterance.trace_id,
            language: None,
            items,
        }
    }
}

#[test]
fn a_synth_that_cannot_place_marks_gets_the_sequence_split_and_marks_stay_exact() {
    let (manager, token, log, recorder) = capture_manager(Some(Box::new(MarkingTheme)));

    let id = manager.speak(
        &token,
        Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments: vec![UtteranceSegment::text("one"), UtteranceSegment::text("two")],
            source: None,
            say_all: false,
            validity: None,
        },
    );

    assert_eq!(recorder.endings(1), vec![(id, UtteranceEnding::Completed)]);
    let texts: Vec<String> = log
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.sequence.text())
        .collect();
    assert_eq!(
        texts,
        ["one", "two"],
        "one synthesis per piece, no marks inside"
    );
    assert_eq!(
        *recorder.marks.lock().unwrap(),
        vec![(id, IndexMark(1)), (id, IndexMark(2))]
    );
}

/// A silent synth that places marks itself: it records each sequence it is
/// given, pushes audio for each text item, and reports each mark where it
/// stands.
struct MarkPlacingSynth {
    given: Arc<Mutex<Vec<Vec<SpeechItem>>>>,
}

impl SynthDriver for MarkPlacingSynth {
    fn id(&self) -> SynthId {
        SynthId::new("marks")
    }

    fn display_name(&self) -> String {
        "Mark-placing synth".to_owned()
    }

    fn supported_settings(&self) -> Vec<verbatim_speech::SettingDescriptor> {
        Vec::new()
    }

    fn setting(&self, _id: &SettingId) -> Option<SettingValue> {
        None
    }

    fn set_setting(&mut self, id: &SettingId, _value: SettingValue) -> Result<(), SynthError> {
        Err(SynthError::Setting(format!("no setting {id}")))
    }

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        self.given.lock().unwrap().push(sequence.items.clone());
        for item in &sequence.items {
            match item {
                SpeechItem::Text(_) => {
                    let _ = sink.push_pcm(FORMAT, &[1_000i16; 64]);
                }
                SpeechItem::Mark(mark) => sink.index_reached(*mark),
                _ => {}
            }
        }
        Ok(())
    }
}

/// A synthesizer that places marks itself, as eSpeak NG and `OneCore` do, is
/// given the whole sequence, marks included, in one call, and the marks it
/// reports are the ones playback reaches.
#[test]
fn a_synth_that_places_marks_gets_the_whole_sequence_in_one_call() {
    let given = Arc::new(Mutex::new(Vec::new()));
    let for_factory = Arc::clone(&given);
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("marks"),
        "Mark-placing synth",
        Box::new(move || {
            Ok(Box::new(MarkPlacingSynth {
                given: Arc::clone(&for_factory),
            }) as Box<dyn SynthDriver>)
        }),
    );
    let recorder = Arc::new(Recorder::default());
    let (manager, token, _sounds) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("marks"),
        settings: Box::new(TestStore::default()),
        mixer: mixer(),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: theme_without_sounds(),
        presenter: Some(Box::new(MarkingTheme)),
        inbox: verbatim_speech::channel_inbox(),
    })
    .expect("pipeline starts");

    let id = manager.speak(
        &token,
        Utterance {
            segments: vec![UtteranceSegment::text("one"), UtteranceSegment::text("two")],
            ..queued("unused")
        },
    );

    assert_eq!(recorder.endings(1), vec![(id, UtteranceEnding::Completed)]);
    assert_eq!(
        *given.lock().unwrap(),
        vec![vec![
            SpeechItem::Text("one".to_owned()),
            SpeechItem::Mark(IndexMark(1)),
            SpeechItem::Text("two".to_owned()),
            SpeechItem::Mark(IndexMark(2)),
        ]]
    );
    assert_eq!(
        *recorder.marks.lock().unwrap(),
        vec![(id, IndexMark(1)), (id, IndexMark(2))]
    );
}

#[test]
fn settings_host_get_set_commit_revert() {
    let store = TestStore::default();
    let log: CaptureLog = CaptureSynth::new().log();
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("capture"),
        "Capture synth",
        Box::new(move || {
            Ok(Box::new(CaptureSynth::with_log(Arc::clone(&log))) as Box<dyn SynthDriver>)
        }),
    );
    let manager = settings_manager(registry, "capture", &store).expect("pipeline starts");
    let host = manager.settings_host();
    let answers = answers(&host);

    let rate = SettingId::new("rate");
    let voice = SettingId::new("voice");

    // Defaults from the capture synth.
    assert_eq!(host.active_synthesizer().id, SynthId::new("capture"));
    assert_eq!(
        host.synthesizers(),
        vec![SynthChoice {
            id: SynthId::new("capture"),
            display_name: "Capture synth".to_owned(),
        }]
    );
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(50)));

    // A change in range is applied and answered with the driver's values,
    // which the mirror then has.
    host.set_setting(&rate, SettingValue::Number(75), 1)
        .unwrap();
    assert_eq!(answer(&answers).change, 1);
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));

    // Out of range is rejected and does not change the mirror.
    let err = host
        .set_setting(&rate, SettingValue::Number(500), 2)
        .unwrap_err();
    assert!(matches!(err, SynthError::Setting(_)));
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));

    // A choice change and a commit save the driver's values.
    host.set_setting(&voice, SettingValue::Choice("capture-b".to_owned()), 3)
        .unwrap();
    assert_eq!(answer(&answers).change, 3);
    host.commit().unwrap();
    assert_eq!(
        store.persisted(1),
        vec![(
            SynthId::new("capture"),
            true,
            vec![
                (voice.clone(), SettingValue::Choice("capture-b".to_owned())),
                (rate.clone(), SettingValue::Number(75)),
            ]
        )]
    );

    // An uncommitted change reverts to the committed value.
    host.set_setting(&rate, SettingValue::Number(10), 4)
        .unwrap();
    assert_eq!(answer(&answers).change, 4);
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(10)));
    host.revert();
    assert_eq!(answer(&answers).change, 4);
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(75)));
}

fn about(text: &str, node: u64, had_focus: bool) -> Utterance {
    Utterance {
        say_all: false,
        validity: Some(FocusValidity {
            node: NodeId::new(node),
            had_focus,
            window: None,
        }),
        ..queued(text)
    }
}

#[test]
fn a_cancel_ends_current_and_queued_speech() {
    let harness = control_manager();
    let current = harness.manager.speak(&harness.token, queued("current"));
    assert_eq!(recv_started(&harness.started), "current");
    let waiting = harness.manager.speak(&harness.token, queued("waiting"));

    harness.manager.control().cancel();

    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (current, UtteranceEnding::Cancelled),
            (waiting, UtteranceEnding::Cancelled),
        ]
    );
}

/// A cancel wakes a driver waiting for its synthesizer, which has pushed no
/// audio yet, through the waker it registered, so the utterance ends
/// cancelled with nothing woken at intervals to look.
#[test]
fn a_cancel_wakes_a_driver_waiting_for_its_synthesizer() {
    let harness = control_manager();
    let waiting = harness.manager.speak(&harness.token, queued("wait"));
    assert_eq!(recv_started(&harness.started), "wait");

    harness.manager.control().cancel();

    assert_eq!(
        harness
            .returned
            .recv_timeout(STEP_TIMEOUT)
            .expect("the driver returned"),
        "wait"
    );
    assert_eq!(
        harness.endings_before_a_marker(),
        vec![(waiting, UtteranceEnding::Cancelled)]
    );
}

/// A key press's cancel fences off speech an earlier key press caused
/// that reaches the manager after it: key 2's cancel is posted to the
/// queue's inbox before key 1's speech, as when key 1's speech is still on
/// its way from the reducer, and key 1's speech ends cancelled without
/// being spoken, as if it had come first, while key 2's own speech and
/// speech no key caused are spoken.
#[test]
fn speech_an_earlier_key_caused_is_dropped_after_a_later_keys_cancel() {
    let harness = control_manager();
    harness.manager.control().cancel_through(KeySeq::new(2));
    let earlier =
        harness
            .manager
            .speak_for_key(&harness.token, queued("key one"), Some(KeySeq::new(1)));
    let later =
        harness
            .manager
            .speak_for_key(&harness.token, queued("key two"), Some(KeySeq::new(2)));
    let unkeyed = harness
        .manager
        .speak_for_key(&harness.token, queued("a focus"), None);
    assert_eq!(recv_started(&harness.started), "key two");
    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "a focus");
    harness.finish.send(()).unwrap();

    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (earlier, UtteranceEnding::Cancelled),
            (later, UtteranceEnding::Completed),
            (unkeyed, UtteranceEnding::Completed),
        ]
    );
}

/// Speech for a focus the user has left is dropped, with everything queued
/// before it, as NVDA culls expired focus speech; speech after it stays.
#[test]
fn expired_focus_speech_is_dropped_with_what_came_before_it() {
    let harness = control_manager();
    let first = harness
        .manager
        .speak(&harness.token, about("first control", 1, true));
    assert_eq!(recv_started(&harness.started), "first control");
    let second = harness
        .manager
        .speak(&harness.token, about("second control", 2, true));
    let message = harness.manager.speak(&harness.token, queued("a message"));

    // Focus is now on node 3, inside nothing: both controls have expired.
    harness.manager.control().drop_expired(FocusNow {
        focus: ObjectKey::of_id(NodeId::new(3)),
        ancestors: Vec::new(),
        foreground: None,
        menu_item_of_focus: None,
    });

    assert_eq!(recv_started(&harness.started), "a message");
    harness.finish.send(()).unwrap();
    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (first, UtteranceEnding::Cancelled),
            (second, UtteranceEnding::Cancelled),
            (message, UtteranceEnding::Completed),
        ]
    );
}

/// Shift pauses speech where it is: its ending waits until it is resumed.
/// Speech arriving while paused cancels what was paused, which ends the
/// pause. Each pause and resume is reported once applied.
#[test]
fn a_pause_holds_speech_until_resumed_and_new_speech_cancels_it() {
    let mut harness = control_manager();
    let held = harness.manager.speak(&harness.token, queued("held"));
    assert_eq!(recv_started(&harness.started), "held");
    harness.manager.control().toggle_pause();
    // The pause reaches the mixer through the queue thread, the audio
    // through the synth thread; once the pause is reported applied, audio
    // sent after it is ordered after it at the mixer.
    assert_eq!(harness.recorder.pauses(1), [true]);
    harness.finish.send(()).unwrap();
    assert_eq!(
        harness
            .returned
            .recv_timeout(STEP_TIMEOUT)
            .expect("synth returned"),
        "held"
    );
    // The evidence that "held" would have ended by now were it not paused:
    // 100 ms of audio on another source of the same mixer has played to
    // the end, and the mixer mixes every source into the same frames.
    play_to_the_end(
        &mut harness.probe,
        &harness.probe_ended,
        Duration::from_millis(100),
    );
    assert_eq!(harness.recorder.ended_so_far(), [], "held while paused");
    harness.manager.control().toggle_pause();
    assert_eq!(
        harness.recorder.endings_through(held),
        [(held, UtteranceEnding::Completed)]
    );

    let paused = harness.manager.speak(&harness.token, queued("paused"));
    assert_eq!(recv_started(&harness.started), "paused");
    harness.manager.control().toggle_pause();
    let next = harness.manager.speak(&harness.token, queued("next"));
    assert_eq!(recv_started(&harness.started), "next");
    harness.finish.send(()).unwrap();
    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (held, UtteranceEnding::Completed),
            (paused, UtteranceEnding::Cancelled),
            (next, UtteranceEnding::Completed),
        ]
    );
    assert_eq!(harness.recorder.pauses(4), [true, false, true, false]);
}

/// Plays `length` of quiet tone through a source of its own on `mixer` and
/// waits until it has all played.
fn play_to_the_end(probe: &mut SpeechLane, ended: &Receiver<UtteranceEnding>, length: Duration) {
    let utterance = UtteranceId(u64::MAX);
    probe.register(utterance, TraceId::mint(), 0);
    let frames = usize::try_from(length.as_millis()).unwrap() * FORMAT.sample_rate as usize / 1_000;
    assert!(
        probe
            .write(utterance, FORMAT, &vec![1_000i16; frames])
            .is_continue(),
        "the probe's audio is accepted"
    );
    probe.finish(utterance);
    assert_eq!(
        ended.recv_timeout(STEP_TIMEOUT).expect("the probe ended"),
        UtteranceEnding::Completed
    );
}

/// A silent synth with a voice and a rate, under any id, that refuses the
/// voice "refused" the way eSpeak NG refuses a variant it cannot load, and
/// the rate 60.
struct SettingsSynth {
    id: &'static str,
    voice: String,
    rate: i32,
}

impl SettingsSynth {
    fn factory(id: &'static str) -> SynthFactory {
        Box::new(move || {
            Ok(Box::new(SettingsSynth {
                id,
                voice: "default".to_owned(),
                rate: 50,
            }) as Box<dyn SynthDriver>)
        })
    }
}

impl SynthDriver for SettingsSynth {
    fn id(&self) -> SynthId {
        SynthId::new(self.id)
    }

    fn display_name(&self) -> String {
        self.id.to_owned()
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        let options = ["default", "other", "refused"]
            .map(|voice| (voice.to_owned(), voice.to_owned()))
            .to_vec();
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".to_owned(),
                options,
            },
            SettingDescriptor::standard_numeric("rate", "setting-rate"),
        ]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        match id.0.as_str() {
            "voice" => Some(SettingValue::Choice(self.voice.clone())),
            "rate" => Some(SettingValue::Number(self.rate)),
            _ => None,
        }
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("voice", SettingValue::Choice(voice)) if voice != "refused" => self.voice = voice,
            ("rate", SettingValue::Number(rate)) if rate != 60 => self.rate = rate,
            (_, value) => return Err(SynthError::Setting(format!("{id} refuses {value:?}"))),
        }
        Ok(())
    }

    fn places_marks(&self) -> bool {
        true
    }

    fn speak(&mut self, _: &SpeechSequence, _: &mut dyn SynthSink) -> Result<(), SynthError> {
        Ok(())
    }
}

/// A factory for a synthesizer whose host cannot start.
fn broken_factory() -> SynthFactory {
    Box::new(|| {
        Err(SynthError::Unavailable(
            "the host would not start".to_owned(),
        ))
    })
}

/// A manager over `registry` whose speech section is `store`.
fn settings_manager(
    registry: SynthRegistry,
    initial: &str,
    store: &TestStore,
) -> Result<SpeechManager, SynthError> {
    SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new(initial),
        settings: Box::new(store.clone()),
        mixer: mixer(),
        events: None,
        theme: theme_without_sounds(),
        presenter: None,
        inbox: verbatim_speech::channel_inbox(),
    })
    .map(|(manager, _token, _sounds)| manager)
}

/// A store whose synthesizers start with `saved`.
fn saving(saved: Vec<(SynthId, Vec<(SettingId, SettingValue)>)>) -> TestStore {
    TestStore {
        saved: Arc::new(Mutex::new(saved)),
        ..TestStore::default()
    }
}

fn voice_and_rate(voice: &str, rate: i32) -> Vec<(SettingId, SettingValue)> {
    vec![
        (
            SettingId::new("voice"),
            SettingValue::Choice(voice.to_owned()),
        ),
        (SettingId::new("rate"), SettingValue::Number(rate)),
    ]
}

#[test]
fn a_refused_saved_setting_keeps_the_synths_value_and_the_others_apply() {
    let voice = SettingId::new("voice");
    let rate = SettingId::new("rate");
    // The first voice is not among the options; the driver itself refuses
    // the second.
    for refused in ["not-installed", "refused"] {
        let store = saving(vec![(SynthId::new("one"), voice_and_rate(refused, 70))]);
        let mut registry = SynthRegistry::new();
        registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
        let manager = settings_manager(registry, "one", &store).expect("startup survives");
        let host = manager.settings_host();
        assert_eq!(
            host.setting(&voice),
            Some(SettingValue::Choice("default".to_owned()))
        );
        assert_eq!(host.setting(&rate), Some(SettingValue::Number(70)));
    }
}

#[test]
fn startup_falls_back_in_registration_order_when_the_configured_synth_cannot_start() {
    let store = saving(vec![(SynthId::new("working"), voice_and_rate("other", 30))]);
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("broken"), "Broken", broken_factory());
    registry.register(
        SynthId::new("working"),
        "Working",
        SettingsSynth::factory("working"),
    );
    registry.register(
        SynthId::new("spare"),
        "Spare",
        SettingsSynth::factory("spare"),
    );
    registry.register(SynthId::new("configured"), "Configured", broken_factory());

    let manager = settings_manager(registry, "configured", &store).expect("a fallback starts");
    let host = manager.settings_host();
    // The configured synth failed, then the first registered one; the next
    // one starts, with its own saved settings.
    assert_eq!(host.active_synthesizer().id, SynthId::new("working"));
    assert_eq!(
        host.setting(&SettingId::new("voice")),
        Some(SettingValue::Choice("other".to_owned()))
    );
    assert_eq!(
        host.setting(&SettingId::new("rate")),
        Some(SettingValue::Number(30))
    );

    // Only when nothing can start does startup fail.
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("broken"), "Broken", broken_factory());
    assert!(matches!(
        settings_manager(registry, "missing", &store),
        Err(SynthError::Unavailable(_))
    ));
}

/// Switches `host` to synthesizer `id` and waits for the outcome, which the
/// switch hands over once the synthesizer has started or failed to.
fn switch(host: &verbatim_speech::SettingsHost, id: &str) -> Result<(), SynthError> {
    let (done_tx, done) = crossbeam_channel::bounded(1);
    host.switch_synthesizer(
        &SynthId::new(id),
        Box::new(move |outcome| {
            let _ = done_tx.send(outcome);
        }),
    );
    done.recv().expect("the switch reports its outcome")
}

#[test]
fn a_synth_that_cannot_start_fails_the_switch_and_the_previous_one_stays() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    registry.register(SynthId::new("broken"), "Broken", broken_factory());
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();

    assert!(matches!(
        switch(&host, "broken"),
        Err(SynthError::Unavailable(_))
    ));
    assert_eq!(host.active_synthesizer().id, SynthId::new("one"));
}

#[test]
fn switching_synth_starts_it_with_its_saved_settings() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    registry.register(SynthId::new("two"), "Two", SettingsSynth::factory("two"));
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();

    // Saved after startup, as a commit would, with a voice that is gone.
    store
        .saved
        .lock()
        .unwrap()
        .push((SynthId::new("two"), voice_and_rate("not-installed", 20)));
    switch(&host, "two").expect("switches");

    assert_eq!(host.active_synthesizer().id, SynthId::new("two"));
    assert_eq!(
        host.setting(&SettingId::new("rate")),
        Some(SettingValue::Number(20))
    );
    assert_eq!(
        host.setting(&SettingId::new("voice")),
        Some(SettingValue::Choice("default".to_owned()))
    );
}

/// `inner`, which starts only once `gate` lets it, as a synthesizer host
/// that takes its time to start does.
fn gated(inner: SynthFactory, gate: crossbeam_channel::Receiver<()>) -> SynthFactory {
    Box::new(move || {
        gate.recv().expect("the test opens the gate");
        inner()
    })
}

/// NVDA's switch blocks its GUI, so its settings dialog cannot be closed
/// during one; Verbatim's does not. A change and a commit made while the
/// switch is under way are carried out behind it on the synth thread, so
/// what is saved is the synthesizer the switch started, with the change.
#[test]
fn a_change_and_a_commit_during_a_switch_save_the_synth_the_switch_started() {
    let store = TestStore::default();
    let (open, gate) = crossbeam_channel::bounded(0);
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    registry.register(
        SynthId::new("two"),
        "Two",
        gated(SettingsSynth::factory("two"), gate),
    );
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();

    let (done_tx, done) = crossbeam_channel::bounded(1);
    host.switch_synthesizer(
        &SynthId::new("two"),
        Box::new(move |outcome| {
            let _ = done_tx.send(outcome);
        }),
    );
    host.set_setting(&SettingId::new("rate"), SettingValue::Number(75), 1)
        .expect("a rate in range");
    host.commit().expect("commits");
    open.send(()).expect("the switch waits at the gate");
    done.recv()
        .expect("the switch reports its outcome")
        .expect("switches");
    assert_eq!(
        store.persisted(1),
        vec![(SynthId::new("two"), true, voice_and_rate("default", 75))]
    );
}

/// A revert asked for while a switch is under way is carried out behind
/// it, so the previous synthesizer's values never reach the new one; when
/// the switch fails, the previous synthesizer's committed values are
/// restored.
#[test]
fn a_revert_during_a_switch_that_fails_restores_the_committed_values() {
    let store = TestStore::default();
    let (open, gate) = crossbeam_channel::bounded(0);
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    registry.register(
        SynthId::new("broken"),
        "Broken",
        gated(broken_factory(), gate),
    );
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();
    let answers = answers(&host);
    let rate = SettingId::new("rate");
    host.set_setting(&rate, SettingValue::Number(75), 1)
        .expect("a rate in range");
    assert_eq!(answer(&answers).change, 1);

    let (done_tx, done) = crossbeam_channel::bounded(1);
    host.switch_synthesizer(
        &SynthId::new("broken"),
        Box::new(move |outcome| {
            let _ = done_tx.send(outcome);
        }),
    );
    host.revert();
    assert_eq!(
        host.setting(&rate),
        Some(SettingValue::Number(75)),
        "nothing is restored while the switch is under way"
    );
    open.send(()).expect("the switch waits at the gate");
    assert!(matches!(
        done.recv().expect("the switch reports its outcome"),
        Err(SynthError::Unavailable(_))
    ));
    assert_eq!(
        answer(&answers),
        SettingsAnswer {
            change: 1,
            values: voice_and_rate("default", 50),
        }
    );
    assert_eq!(host.active_synthesizer().id, SynthId::new("one"));
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(50)));
}

/// A commit saves what the driver has, read from it on the synth thread
/// after every change before it, as NVDA saves: a value the driver refused
/// is never saved, and the mirror has the driver's value.
#[test]
fn a_commit_after_a_refused_change_saves_the_drivers_value() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();
    let rate = SettingId::new("rate");

    host.set_setting(&rate, SettingValue::Number(60), 1)
        .expect("a rate in range");
    host.commit().expect("commits");
    assert_eq!(
        store.persisted(1),
        vec![(SynthId::new("one"), true, voice_and_rate("default", 50))]
    );
    assert_eq!(host.setting(&rate), Some(SettingValue::Number(50)));
}

/// The synth thread answers a change the driver refused with the driver's
/// own value, to the settings GUI as well as into the mirror.
#[test]
fn a_refused_change_is_answered_with_the_drivers_value() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("one"), "One", SettingsSynth::factory("one"));
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();
    let answers = answers(&host);

    host.set_setting(&SettingId::new("rate"), SettingValue::Number(60), 7)
        .expect("a rate in range");
    assert_eq!(
        answer(&answers),
        SettingsAnswer {
            change: 7,
            values: voice_and_rate("default", 50),
        }
    );
    assert_eq!(
        host.setting(&SettingId::new("rate")),
        Some(SettingValue::Number(50))
    );
}

/// The active synthesizer's name is the one the registry, built by
/// verbatim.exe in the user's language, gives it, not the one its driver
/// reports from the synthesizer host, at startup and after a switch.
#[test]
fn the_active_synthesizer_is_named_as_the_registry_names_it() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("one"),
        "Registry One",
        SettingsSynth::factory("one"),
    );
    registry.register(
        SynthId::new("two"),
        "Registry Two",
        SettingsSynth::factory("two"),
    );
    let manager = settings_manager(registry, "one", &store).expect("pipeline starts");
    let host = manager.settings_host();
    assert_eq!(
        host.active_synthesizer(),
        SynthChoice {
            id: SynthId::new("one"),
            display_name: "Registry One".to_owned(),
        }
    );
    switch(&host, "two").expect("switches");
    assert_eq!(
        host.active_synthesizer(),
        SynthChoice {
            id: SynthId::new("two"),
            display_name: "Registry Two".to_owned(),
        }
    );
}

/// A silent synth with a pitch setting that records the pitch it held for
/// each piece of text it spoke.
struct PitchSynth {
    pitch: i32,
    spoken: Arc<Mutex<Vec<(String, i32)>>>,
}

impl SynthDriver for PitchSynth {
    fn id(&self) -> SynthId {
        SynthId::new("pitch")
    }

    fn display_name(&self) -> String {
        "Pitch synth".to_owned()
    }

    fn supported_settings(&self) -> Vec<verbatim_speech::SettingDescriptor> {
        vec![verbatim_speech::SettingDescriptor::standard_numeric(
            "pitch",
            "setting-pitch",
        )]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        (id.0.as_str() == "pitch").then_some(SettingValue::Number(self.pitch))
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("pitch", SettingValue::Number(pitch)) => {
                self.pitch = pitch;
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
        self.spoken
            .lock()
            .unwrap()
            .push((sequence.text(), self.pitch));
        let _ = sink.push_pcm(FORMAT, &[1_000i16; 64]);
        Ok(())
    }
}

/// NVDA's raised pitch for capitals when spelling: the capital is spoken
/// with the pitch setting 30 higher, and the setting is put back after.
#[test]
fn a_spelled_capital_is_spoken_at_a_raised_pitch() {
    let spoken = Arc::new(Mutex::new(Vec::new()));
    let for_factory = Arc::clone(&spoken);
    let mut registry = SynthRegistry::new();
    registry.register(
        SynthId::new("pitch"),
        "Pitch synth",
        Box::new(move || {
            Ok(Box::new(PitchSynth {
                pitch: 50,
                spoken: Arc::clone(&for_factory),
            }) as Box<dyn SynthDriver>)
        }),
    );
    let recorder = Arc::new(Recorder::default());
    let (manager, token, _sounds) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth: SynthId::new("pitch"),
        settings: Box::new(TestStore::default()),
        mixer: mixer(),
        events: Some(Arc::clone(&recorder) as Arc<dyn SpeechEvents>),
        theme: theme_without_sounds(),
        presenter: None,
        inbox: verbatim_speech::channel_inbox(),
    })
    .expect("pipeline starts");

    let spelled = manager.speak(
        &token,
        Utterance {
            segments: vec![
                UtteranceSegment::text("a"),
                UtteranceSegment::new(SegmentContent::SpelledCapital("B".to_owned())),
                UtteranceSegment::text("c"),
            ],
            ..queued("unused")
        },
    );
    let after = manager.speak(&token, queued("after"));
    assert_eq!(
        recorder.endings(2),
        vec![
            (spelled, UtteranceEnding::Completed),
            (after, UtteranceEnding::Completed),
        ]
    );
    assert_eq!(
        *spoken.lock().unwrap(),
        vec![
            ("a".to_owned(), 50),
            ("B".to_owned(), 80),
            ("c".to_owned(), 50),
            ("after".to_owned(), 50),
        ]
    );
}

/// Waiting focus speech is judged when its turn comes, as NVDA judges it:
/// valid speech queued ahead of expired speech is still spoken.
#[test]
fn waiting_focus_speech_is_judged_when_its_turn_comes() {
    let harness = control_manager();
    let message = harness.manager.speak(&harness.token, queued("a message"));
    assert_eq!(recv_started(&harness.started), "a message");
    let current = harness
        .manager
        .speak(&harness.token, about("the new focus", 3, true));
    let left = harness
        .manager
        .speak(&harness.token, about("a control left behind", 2, true));
    harness.manager.control().drop_expired(FocusNow {
        focus: ObjectKey::of_id(NodeId::new(3)),
        ancestors: Vec::new(),
        foreground: None,
        menu_item_of_focus: None,
    });

    harness.finish.send(()).unwrap();
    assert_eq!(recv_started(&harness.started), "the new focus");
    harness.finish.send(()).unwrap();
    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (message, UtteranceEnding::Completed),
            (current, UtteranceEnding::Completed),
            (left, UtteranceEnding::Cancelled),
        ]
    );
}

/// A cancel ends everything handed on at once, though the mixer reports
/// the endings a little later: a focus change in between must not find the
/// cancelled speech expired and stop speech handed on since.
#[test]
fn a_focus_change_just_after_a_cancel_spares_speech_handed_on_since() {
    let harness = control_manager();
    let left = harness
        .manager
        .speak(&harness.token, about("a control left behind", 1, true));
    assert_eq!(recv_started(&harness.started), "a control left behind");
    let control = harness.manager.control();
    control.cancel();
    let dialog = harness
        .manager
        .speak(&harness.token, about("a dialog", 2, false));
    control.drop_expired(FocusNow {
        focus: ObjectKey::of_id(NodeId::new(3)),
        ancestors: vec![ObjectKey::of_id(NodeId::new(2))],
        foreground: None,
        menu_item_of_focus: None,
    });
    assert_eq!(recv_started(&harness.started), "a dialog");
    harness.finish.send(()).unwrap();
    assert_eq!(
        harness.endings_before_a_marker(),
        vec![
            (left, UtteranceEnding::Cancelled),
            (dialog, UtteranceEnding::Completed),
        ]
    );
}

/// A synthesizer started in place of the configured one is not saved as
/// the user's choice, so the configured one is tried again at the next
/// start, as NVDA does; one chosen in the dialog is.
#[test]
fn a_fallback_synth_is_not_saved_as_the_users_choice() {
    let store = TestStore::default();
    let mut registry = SynthRegistry::new();
    registry.register(SynthId::new("configured"), "Configured", broken_factory());
    registry.register(
        SynthId::new("working"),
        "Working",
        SettingsSynth::factory("working"),
    );
    registry.register(
        SynthId::new("spare"),
        "Spare",
        SettingsSynth::factory("spare"),
    );
    let manager = settings_manager(registry, "configured", &store).expect("a fallback starts");
    let host = manager.settings_host();

    host.commit().expect("commits");
    switch(&host, "spare").expect("switches");
    host.commit().expect("commits");
    assert_eq!(
        store.persisted(2),
        vec![
            (
                SynthId::new("working"),
                false,
                voice_and_rate("default", 50)
            ),
            (SynthId::new("spare"), true, voice_and_rate("default", 50))
        ]
    );
}
