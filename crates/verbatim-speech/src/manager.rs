//! The speech manager: the running pipeline (architecture section 6).
//!
//! Two dedicated threads carry every utterance. The *queue thread* owns the
//! priority lanes and decides ordering and cancellation; the *synth thread*
//! owns the active driver and is where [`SynthDriver::speak`] blocks. The
//! audio mixer's own thread plays what the synth thread writes. None of
//! them touches application code, so the founding responsiveness rule
//! holds: nothing here blocks on another app.
//!
//! Priority lanes follow architecture section 6. An `Interrupt` utterance
//! cancels the current utterance and empties both lanes before it speaks; a
//! `Next` utterance jumps ahead of the queue but behind the current one;
//! `Queued` appends.
//!
//! Endings (decision D17). Every utterance [`SpeechManager::speak`] accepts
//! ends exactly once, reported through [`SpeechEvents::utterance_ended`].
//! Until the queue thread hands an utterance to the synth thread, the queue
//! thread owns its ending: one cleared from a lane is cancelled there. From
//! that hand-over on, the mixer owns it: the queue thread registers the
//! utterance with the mixer first, and the mixer reports it completed when
//! its last frame has played, cancelled when the mixer is told to cancel,
//! or failed when synthesis or audio fails. Cancellation is per utterance:
//! each job carries its own flag, and the mixer refuses audio for an
//! utterance it has already ended.

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use tracing::{info, trace, warn};
use verbatim_audio::{Mixer, PcmFormat, PlaybackEvent, Source};
use verbatim_model::{SpeechPriority, Utterance, UtteranceEnding, UtteranceId};

use crate::driver::{IndexMark, SpeechSequence, SynthDriver, SynthError, SynthSink};
use crate::events::SpeechEvents;
use crate::registry::SynthRegistry;
use crate::settings::{SettingId, SettingValue, SynthChoice, SynthId};
use crate::theme::{PlainTheme, Theme};
use crate::trim::{Piece, Trimmer};

/// The mint counter for utterance ids, process-wide.
static NEXT_UTTERANCE: AtomicU64 = AtomicU64::new(1);

/// The current settings snapshot of the active driver, returned when the
/// manager builds or switches drivers so the settings host can mirror it.
pub(crate) struct DriverState {
    pub choice: SynthChoice,
    pub descriptors: Vec<crate::settings::SettingDescriptor>,
    pub values: Vec<(SettingId, SettingValue)>,
}

/// Reads every current setting value from a driver, in descriptor order.
fn snapshot_values(driver: &dyn SynthDriver) -> Vec<(SettingId, SettingValue)> {
    driver
        .supported_settings()
        .iter()
        .filter_map(|descriptor| {
            let id = descriptor.id().clone();
            driver.setting(&id).map(|value| (id, value))
        })
        .collect()
}

fn driver_state(driver: &dyn SynthDriver) -> DriverState {
    DriverState {
        choice: SynthChoice {
            id: driver.id(),
            display_name: driver.display_name(),
        },
        descriptors: driver.supported_settings(),
        values: snapshot_values(driver),
    }
}

/// Reads a synthesizer's saved setting values, e.g. from persisted config.
///
/// Called on the pipeline's synth thread each time a synthesizer starts, so
/// it sees values saved since startup. A value the synthesizer refuses is
/// skipped with a warning, and the synthesizer keeps its own value for it.
pub type SavedSettingsFn = Box<dyn Fn(&SynthId) -> Vec<(SettingId, SettingValue)> + Send>;

/// Configuration for [`SpeechManager::new`].
///
/// The manager depends on nothing above the pipeline: persistence and
/// observability are injected, and drivers arrive as a [`SynthRegistry`], so
/// this crate never reaches for config or the control plane directly.
pub struct SpeechManagerConfig {
    /// The synth drivers available to switch between.
    pub registry: SynthRegistry,
    /// The synthesizer to make active at startup. When it cannot start, the
    /// other registered synthesizers are tried in registration order.
    pub initial_synth: SynthId,
    /// The saved setting values of a synthesizer, applied whenever one
    /// starts, at startup or on a switch.
    pub saved_settings: SavedSettingsFn,
    /// The mixer speech plays through. The manager adds its own source.
    pub mixer: Arc<Mixer>,
    /// Optional observer for each utterance's milestones and ending.
    pub events: Option<Arc<dyn SpeechEvents>>,
    /// The presentation theme flattening utterances (decision D12);
    /// `None` selects [`PlainTheme`], plain speech.
    pub theme: Option<Box<dyn Theme>>,
}

/// Commands the queue thread accepts, from the manager, the settings host, and
/// (as `SynthFinished`) the synth thread itself.
pub(crate) enum QueueEvent {
    Speak(UtteranceId, Box<Utterance>),
    ApplySetting {
        id: SettingId,
        value: SettingValue,
    },
    RestoreSettings(Vec<(SettingId, SettingValue)>),
    SwitchSynth {
        id: SynthId,
        reply: Sender<Result<DriverState, SynthError>>,
    },
    SynthFinished,
    Shutdown,
}

/// Everything the manager needs from the synth thread once the initial driver
/// is up: its settings snapshot plus the full (static) synthesizer list.
struct StartupInfo {
    state: DriverState,
    choices: Vec<SynthChoice>,
}

/// Commands the synth thread accepts from the queue thread.
enum SynthCommand {
    Job(Job),
    SetSetting {
        id: SettingId,
        value: SettingValue,
    },
    RestoreSettings(Vec<(SettingId, SettingValue)>),
    Switch {
        id: SynthId,
        reply: Sender<Result<DriverState, SynthError>>,
    },
    Shutdown,
}

/// One sequence handed to the synth thread, with its own cancellation flag.
struct Job {
    sequence: SpeechSequence,
    cancel: Arc<AtomicBool>,
}

/// The running speech pipeline.
///
/// Constructed with [`SpeechManager::new`]; feed it structured utterances with
/// [`speak`](SpeechManager::speak) and hand the settings GUI a handle from
/// [`settings_host`](SpeechManager::settings_host). Dropping the manager stops
/// both threads.
pub struct SpeechManager {
    queue_tx: Sender<QueueEvent>,
    /// Kept so the mixer outlives the threads writing to it.
    _mixer: Arc<Mixer>,
    initial_state: DriverState,
    choices: Vec<SynthChoice>,
    queue_handle: Option<JoinHandle<()>>,
    synth_handle: Option<JoinHandle<()>>,
}

impl SpeechManager {
    /// Builds the pipeline and starts its threads, starting the initial
    /// synthesizer with its saved settings.
    ///
    /// When the initial synthesizer cannot start, the other registered
    /// synthesizers are tried in registration order and the first that
    /// starts becomes active, as NVDA falls back (docs/nvda/synth-drivers.md).
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when no registered synthesizer can
    /// start.
    pub fn new(config: SpeechManagerConfig) -> Result<Self, SynthError> {
        let SpeechManagerConfig {
            registry,
            initial_synth,
            saved_settings,
            mixer,
            events,
            theme,
        } = config;
        let theme = theme.unwrap_or_else(|| Box::new(PlainTheme));
        let source = mixer.add_source(playback_listener(events.clone()));

        let (queue_tx, queue_rx) = unbounded::<QueueEvent>();
        let (synth_tx, synth_rx) = unbounded::<SynthCommand>();
        let (startup_tx, startup_rx) = bounded::<Result<StartupInfo, SynthError>>(1);

        let synth_handle = {
            let source = source.clone();
            let queue_tx = queue_tx.clone();
            std::thread::Builder::new()
                .name("verbatim-synth".to_owned())
                .spawn(move || {
                    synth_thread(
                        &registry,
                        &initial_synth,
                        &saved_settings,
                        &source,
                        &synth_rx,
                        &queue_tx,
                        &startup_tx,
                    );
                })
                .map_err(|error| {
                    SynthError::Unavailable(format!("failed to start synth thread: {error}"))
                })?
        };

        // Wait for the synth thread to build the initial driver before the
        // pipeline is considered up.
        let startup = match startup_rx.recv() {
            Ok(Ok(startup)) => startup,
            Ok(Err(error)) => {
                let _ = synth_handle.join();
                return Err(error);
            }
            Err(_) => {
                let _ = synth_handle.join();
                return Err(SynthError::Unavailable(
                    "synth thread exited during startup".to_owned(),
                ));
            }
        };
        let StartupInfo {
            state: initial_state,
            choices,
        } = startup;

        let queue_handle = std::thread::Builder::new()
            .name("verbatim-speech-queue".to_owned())
            .spawn(move || {
                QueueThread {
                    synth_tx,
                    source,
                    events,
                    theme,
                    next_lane: VecDeque::new(),
                    queued_lane: VecDeque::new(),
                    in_flight: None,
                }
                .run(&queue_rx);
            })
            .map_err(|error| {
                SynthError::Unavailable(format!("failed to start queue thread: {error}"))
            })?;

        Ok(Self {
            queue_tx,
            _mixer: mixer,
            initial_state,
            choices,
            queue_handle: Some(queue_handle),
            synth_handle: Some(synth_handle),
        })
    }

    /// Renders and enqueues one utterance according to its priority lane,
    /// returning the id its milestones and ending are reported under.
    ///
    /// Non-blocking: the utterance is handed to the queue thread and this
    /// returns at once.
    pub fn speak(&self, utterance: Utterance) -> UtteranceId {
        let id = UtteranceId(NEXT_UTTERANCE.fetch_add(1, Ordering::Relaxed));
        let _ = self
            .queue_tx
            .send(QueueEvent::Speak(id, Box::new(utterance)));
        id
    }

    /// A settings-GUI handle backed by this manager.
    ///
    /// `persist` is called on [`commit`](crate::SpeechSettingsHost::commit)
    /// with the active synth id and its current values; this crate never
    /// depends on the config layer.
    #[must_use]
    pub fn settings_host(&self, persist: crate::host::PersistFn) -> crate::host::SettingsHost {
        crate::host::SettingsHost::new(
            self.queue_tx.clone(),
            &self.initial_state,
            self.choices.clone(),
            persist,
        )
    }
}

impl Drop for SpeechManager {
    fn drop(&mut self) {
        let _ = self.queue_tx.send(QueueEvent::Shutdown);
        if let Some(handle) = self.queue_handle.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.synth_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Turns the mixer's playback events for speech into [`SpeechEvents`]
/// calls, on the audio thread.
fn playback_listener(events: Option<Arc<dyn SpeechEvents>>) -> verbatim_audio::PlaybackListener {
    Arc::new(move |event| {
        let now = Instant::now();
        match event {
            PlaybackEvent::Started {
                utterance,
                trace_id,
            } => {
                trace!(target: "verbatim::speech", %utterance, trace_id = %trace_id, "audio_started");
                if let Some(events) = &events {
                    events.audio_started(utterance, trace_id, now);
                }
            }
            PlaybackEvent::Mark {
                utterance,
                trace_id,
                mark,
            } => {
                trace!(target: "verbatim::speech", %utterance, trace_id = %trace_id, mark, "mark_reached");
                if let Some(events) = &events {
                    events.mark_reached(utterance, trace_id, IndexMark(mark), now);
                }
            }
            PlaybackEvent::Ended {
                utterance,
                trace_id,
                ending,
            } => {
                trace!(target: "verbatim::speech", %utterance, trace_id = %trace_id, ?ending, "utterance_ended");
                if let Some(events) = &events {
                    events.utterance_ended(utterance, trace_id, &ending, now);
                }
            }
        }
    })
}

/// The queue thread: owns the priority lanes and dispatches one job at a
/// time to the synth thread.
struct QueueThread {
    synth_tx: Sender<SynthCommand>,
    source: Source,
    events: Option<Arc<dyn SpeechEvents>>,
    theme: Box<dyn Theme>,
    next_lane: VecDeque<SpeechSequence>,
    queued_lane: VecDeque<SpeechSequence>,
    /// The cancellation flag of the job the synth thread is working on.
    in_flight: Option<Arc<AtomicBool>>,
}

impl QueueThread {
    fn run(mut self, queue_rx: &Receiver<QueueEvent>) {
        while let Ok(event) = queue_rx.recv() {
            match event {
                QueueEvent::Speak(id, utterance) => self.speak(id, &utterance),
                QueueEvent::ApplySetting { id, value } => {
                    let _ = self.synth_tx.send(SynthCommand::SetSetting { id, value });
                }
                QueueEvent::RestoreSettings(values) => {
                    let _ = self.synth_tx.send(SynthCommand::RestoreSettings(values));
                }
                QueueEvent::SwitchSynth { id, reply } => {
                    // Silence current speech so the switch, and the caller
                    // waiting on its reply, are not blocked behind a long
                    // utterance.
                    self.cancel_everything();
                    let _ = self.synth_tx.send(SynthCommand::Switch { id, reply });
                }
                QueueEvent::SynthFinished => {
                    self.in_flight = None;
                    self.pump();
                }
                QueueEvent::Shutdown => {
                    self.cancel_everything();
                    let _ = self.synth_tx.send(SynthCommand::Shutdown);
                    break;
                }
            }
        }
    }

    fn speak(&mut self, id: UtteranceId, utterance: &Utterance) {
        let sequence = self.theme.flatten(utterance, id);
        if let Some(events) = &self.events {
            events.utterance_queued(id, sequence.trace_id, &sequence.text(), Instant::now());
        }
        trace!(
            target: "verbatim::speech",
            utterance = %id,
            trace_id = %sequence.trace_id,
            priority = ?utterance.priority,
            "utterance_queued"
        );
        match utterance.priority {
            SpeechPriority::Interrupt => {
                self.cancel_everything();
                self.next_lane.push_back(sequence);
            }
            SpeechPriority::Next => self.next_lane.push_back(sequence),
            SpeechPriority::Queued => self.queued_lane.push_back(sequence),
        }
        self.pump();
    }

    /// Hands the next sequence to the synth thread when it is idle. The
    /// mixer is told about the utterance first, so from here on the mixer
    /// owns its ending.
    fn pump(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let Some(sequence) = self
            .next_lane
            .pop_front()
            .or_else(|| self.queued_lane.pop_front())
        else {
            return;
        };
        self.source.register(sequence.utterance, sequence.trace_id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.in_flight = Some(Arc::clone(&cancel));
        let _ = self
            .synth_tx
            .send(SynthCommand::Job(Job { sequence, cancel }));
    }

    /// Cancels everything: the job being synthesized, every sequence waiting
    /// in a lane, and every utterance the mixer has not finished playing.
    fn cancel_everything(&mut self) {
        if let Some(cancel) = &self.in_flight {
            cancel.store(true, Ordering::Release);
        }
        let now = Instant::now();
        for sequence in self.next_lane.drain(..).chain(self.queued_lane.drain(..)) {
            if let Some(events) = &self.events {
                events.utterance_ended(
                    sequence.utterance,
                    sequence.trace_id,
                    &UtteranceEnding::Cancelled,
                    now,
                );
            }
        }
        self.source.cancel_all();
    }
}

/// The synth thread: owns the active driver and runs
/// [`SynthDriver::speak`].
fn synth_thread(
    registry: &SynthRegistry,
    initial_synth: &SynthId,
    saved_settings: &SavedSettingsFn,
    source: &Source,
    synth_rx: &Receiver<SynthCommand>,
    queue_tx: &Sender<QueueEvent>,
    startup_tx: &Sender<Result<StartupInfo, SynthError>>,
) {
    // Start a driver with its saved settings before reporting startup
    // success.
    let mut driver = match start_first_available(registry, initial_synth, saved_settings) {
        Ok(driver) => driver,
        Err(error) => {
            let _ = startup_tx.send(Err(error));
            return;
        }
    };
    let _ = startup_tx.send(Ok(StartupInfo {
        state: driver_state(driver.as_ref()),
        choices: registry.choices(),
    }));

    while let Ok(command) = synth_rx.recv() {
        match command {
            SynthCommand::Job(job) => {
                run_job(driver.as_mut(), source, &job);
                let _ = queue_tx.send(QueueEvent::SynthFinished);
            }
            SynthCommand::SetSetting { id, value } => {
                if let Err(error) = driver.set_setting(&id, value) {
                    warn!(target: "verbatim::speech", %id, %error, "applying setting failed");
                }
            }
            SynthCommand::RestoreSettings(values) => {
                for (id, value) in values {
                    if let Err(error) = driver.set_setting(&id, value) {
                        warn!(target: "verbatim::speech", %id, %error, "restoring setting failed");
                    }
                }
            }
            SynthCommand::Switch { id, reply } => {
                match start_synth(registry, &id, saved_settings) {
                    Ok(new_driver) => {
                        driver = new_driver;
                        let _ = reply.send(Ok(driver_state(driver.as_ref())));
                    }
                    Err(error) => {
                        let _ = reply.send(Err(error));
                    }
                }
            }
            SynthCommand::Shutdown => break,
        }
    }
}

/// Starts `preferred`, or when it cannot start, the first other registered
/// synthesizer that can, in registration order: NVDA's fallback
/// (docs/nvda/synth-drivers.md), less its silent last resort.
fn start_first_available(
    registry: &SynthRegistry,
    preferred: &SynthId,
    saved_settings: &SavedSettingsFn,
) -> Result<Box<dyn SynthDriver>, SynthError> {
    let others = registry
        .choices()
        .into_iter()
        .map(|choice| choice.id)
        .filter(|id| id != preferred);
    let mut failures = Vec::new();
    for id in std::iter::once(preferred.clone()).chain(others) {
        match start_synth(registry, &id, saved_settings) {
            Ok(driver) => {
                if failures.is_empty() {
                    info!(target: "verbatim::speech", synth = %id, "synthesizer started");
                } else {
                    warn!(
                        target: "verbatim::speech",
                        synth = %id,
                        configured = %preferred,
                        "the configured synthesizer could not start; fell back"
                    );
                }
                return Ok(driver);
            }
            Err(error) => {
                warn!(target: "verbatim::speech", synth = %id, %error, "synthesizer could not start");
                failures.push(format!("{id}: {error}"));
            }
        }
    }
    Err(SynthError::Unavailable(format!(
        "no synthesizer could start ({})",
        failures.join("; ")
    )))
}

/// Builds the synthesizer registered as `id` and applies its saved settings.
fn start_synth(
    registry: &SynthRegistry,
    id: &SynthId,
    saved_settings: &SavedSettingsFn,
) -> Result<Box<dyn SynthDriver>, SynthError> {
    let mut driver = registry.build(id)?;
    apply_saved_settings(driver.as_mut(), &saved_settings(id));
    Ok(driver)
}

/// Applies saved values in the driver's descriptor order, which puts the
/// voice first. A value the descriptors or the driver refuse, such as a
/// voice no longer installed, is skipped with a warning and the driver
/// keeps its own value, so one stale setting never costs speech
/// (docs/nvda/synth-drivers.md). Saved values for settings the driver does
/// not have are ignored.
fn apply_saved_settings(driver: &mut dyn SynthDriver, saved: &[(SettingId, SettingValue)]) {
    for descriptor in driver.supported_settings() {
        let id = descriptor.id();
        let Some((_, value)) = saved.iter().find(|(saved_id, _)| saved_id == id) else {
            continue;
        };
        let applied = crate::host::validate(std::slice::from_ref(&descriptor), id, value)
            .and_then(|()| driver.set_setting(id, value.clone()));
        if let Err(error) = applied {
            warn!(
                target: "verbatim::speech",
                synth = %driver.id(),
                setting = %id,
                %error,
                "saved setting refused; keeping the synthesizer's own value"
            );
        }
    }
}

/// Synthesizes one job into the mixer. The job's utterance is already
/// registered with the mixer, which reports its ending; this only tells the
/// mixer when the audio is complete or that synthesis failed.
fn run_job(driver: &mut dyn SynthDriver, source: &Source, job: &Job) {
    let sequence = &job.sequence;
    let utterance = sequence.utterance;
    if job.cancel.load(Ordering::Acquire) {
        // Cancelled before it started; the mixer has already ended it.
        return;
    }
    let pieces = if driver.places_marks() || !sequence.has_marks() {
        vec![(sequence.clone(), None)]
    } else {
        sequence.split_at_marks()
    };
    let mut sink = PipelineSink {
        source,
        utterance,
        cancel: &job.cancel,
        trimmer: Trimmer::default(),
        stopped: false,
    };
    let mut result = Ok(());
    for (piece, mark) in pieces {
        if !piece.items.is_empty() {
            result = driver.speak(&piece, &mut sink);
            if result.is_err() || sink.stopped || job.cancel.load(Ordering::Acquire) {
                break;
            }
        }
        if let Some(mark) = mark {
            sink.index_reached(mark);
        }
    }
    match result {
        Ok(()) => {
            let tail = sink.trimmer.finish();
            let _ = sink.forward(tail);
            source.finish(utterance);
        }
        Err(error) => {
            warn!(target: "verbatim::speech", %utterance, %error, "synthesis failed");
            source.fail(utterance, error.to_string());
        }
    }
}

/// Bridges a driver's [`SynthSink`] output, trimmed of silence, to the
/// mixer.
struct PipelineSink<'a> {
    source: &'a Source,
    utterance: UtteranceId,
    cancel: &'a AtomicBool,
    trimmer: Trimmer,
    /// The mixer refused audio: the utterance has ended.
    stopped: bool,
}

impl PipelineSink<'_> {
    fn forward(&mut self, pieces: Vec<Piece>) -> ControlFlow<()> {
        for piece in pieces {
            match piece {
                Piece::Pcm(format, samples) => {
                    if self
                        .source
                        .write(self.utterance, format, &samples)
                        .is_break()
                    {
                        self.stopped = true;
                        return ControlFlow::Break(());
                    }
                }
                Piece::Mark(mark) => self.source.mark(self.utterance, mark.0),
            }
        }
        ControlFlow::Continue(())
    }
}

impl SynthSink for PipelineSink<'_> {
    fn push_pcm(&mut self, format: PcmFormat, samples: &[i16]) -> ControlFlow<()> {
        if self.stopped || self.cancel.load(Ordering::Acquire) {
            return ControlFlow::Break(());
        }
        let pieces = self.trimmer.push(format, samples);
        self.forward(pieces)
    }

    fn index_reached(&mut self, mark: IndexMark) {
        if !self.stopped {
            let pieces = self.trimmer.mark(mark);
            let _ = self.forward(pieces);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.stopped || self.cancel.load(Ordering::Acquire)
    }
}
