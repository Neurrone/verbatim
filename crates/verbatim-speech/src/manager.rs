//! The speech manager: the running pipeline (architecture section 6).
//!
//! Two dedicated threads carry every utterance. The *queue thread* owns the
//! priority lanes and decides ordering and cancellation; the *synth thread*
//! owns the active driver and the audio sink and is where
//! [`SynthDriver::speak`] blocks. They never touch application code, so the
//! founding responsiveness rule holds: nothing here blocks on another app.
//!
//! Priority lanes follow architecture section 6. An `Interrupt` utterance
//! cancels the current utterance and empties both lanes before it speaks; a
//! `Next` utterance jumps ahead of the queue but behind the current one;
//! `Queued` appends. Cancellation is cooperative: the queue thread sets a
//! shared flag that the sink returns `ControlFlow::Break` on, the driver
//! returns from `speak` promptly, and the synth thread then discards buffered
//! audio with [`AudioSink::stop`].

use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use tracing::{trace, warn};
use verbatim_audio::{AudioSink, PcmFormat};
use verbatim_model::{TraceId, Utterance};

use crate::driver::{IndexMark, SpeechRequest, SynthDriver, SynthError, SynthSink};
use crate::events::SpeechEvents;
use crate::registry::SynthRegistry;
use crate::render::render_utterance;
use crate::settings::{SettingId, SettingValue, SynthChoice, SynthId};

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

/// Configuration for [`SpeechManager::new`].
///
/// The manager depends on nothing above the pipeline: persistence and
/// observability are injected, and drivers arrive as a [`SynthRegistry`], so
/// this crate never reaches for config or the control plane directly.
pub struct SpeechManagerConfig {
    /// The synth drivers available to switch between.
    pub registry: SynthRegistry,
    /// The synthesizer to make active at startup.
    pub initial_synth: SynthId,
    /// Setting values to apply to the initial synth, e.g. from persisted
    /// config.
    pub initial_settings: Vec<(SettingId, SettingValue)>,
    /// Where synthesized audio goes.
    pub sink: Box<dyn AudioSink>,
    /// Optional observer for the queue and audio-start milestones.
    pub events: Option<Arc<dyn SpeechEvents>>,
}

/// Commands the queue thread accepts, from the manager, the settings host, and
/// (as `SynthFinished`) the synth thread itself.
pub(crate) enum QueueEvent {
    Speak(Box<Utterance>),
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
    Job(SpeechRequest),
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

/// The running speech pipeline.
///
/// Constructed with [`SpeechManager::new`]; feed it structured utterances with
/// [`speak`](SpeechManager::speak) and hand the settings GUI a handle from
/// [`settings_host`](SpeechManager::settings_host). Dropping the manager stops
/// both threads.
pub struct SpeechManager {
    queue_tx: Sender<QueueEvent>,
    initial_state: DriverState,
    choices: Vec<SynthChoice>,
    queue_handle: Option<JoinHandle<()>>,
    synth_handle: Option<JoinHandle<()>>,
}

impl SpeechManager {
    /// Builds the pipeline and starts its threads, initializing the active
    /// synthesizer and applying the initial settings.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when the initial synthesizer cannot
    /// be built, or [`SynthError::Setting`] when an initial setting value is
    /// rejected by the driver.
    pub fn new(config: SpeechManagerConfig) -> Result<Self, SynthError> {
        let SpeechManagerConfig {
            registry,
            initial_synth,
            initial_settings,
            sink,
            events,
        } = config;

        let cancel = Arc::new(AtomicBool::new(false));
        let (queue_tx, queue_rx) = unbounded::<QueueEvent>();
        let (synth_tx, synth_rx) = unbounded::<SynthCommand>();
        let (startup_tx, startup_rx) = bounded::<Result<StartupInfo, SynthError>>(1);

        let synth_handle = {
            let cancel = Arc::clone(&cancel);
            let events = events.clone();
            let queue_tx = queue_tx.clone();
            std::thread::Builder::new()
                .name("verbatim-synth".to_owned())
                .spawn(move || {
                    synth_thread(
                        registry,
                        initial_synth,
                        initial_settings,
                        sink,
                        cancel,
                        events,
                        synth_rx,
                        queue_tx,
                        startup_tx,
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

        let queue_handle = {
            let cancel = Arc::clone(&cancel);
            let events = events.clone();
            std::thread::Builder::new()
                .name("verbatim-speech-queue".to_owned())
                .spawn(move || {
                    queue_thread(queue_rx, synth_tx, cancel, events);
                })
                .map_err(|error| {
                    SynthError::Unavailable(format!("failed to start queue thread: {error}"))
                })?
        };

        Ok(Self {
            queue_tx,
            initial_state,
            choices,
            queue_handle: Some(queue_handle),
            synth_handle: Some(synth_handle),
        })
    }

    /// Renders and enqueues one utterance according to its priority lane.
    ///
    /// Non-blocking: the utterance is handed to the queue thread and this
    /// returns at once. A dropped pipeline silently discards the utterance.
    pub fn speak(&self, utterance: Utterance) {
        let _ = self.queue_tx.send(QueueEvent::Speak(Box::new(utterance)));
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

/// The queue thread: owns the priority lanes and dispatches one job at a time
/// to the synth thread.
// The parameters are moved in to be owned for the thread's lifetime, even
// where the body only reads them.
#[allow(clippy::needless_pass_by_value)]
fn queue_thread(
    queue_rx: Receiver<QueueEvent>,
    synth_tx: Sender<SynthCommand>,
    cancel: Arc<AtomicBool>,
    events: Option<Arc<dyn SpeechEvents>>,
) {
    let mut next_lane: VecDeque<SpeechRequest> = VecDeque::new();
    let mut queued_lane: VecDeque<SpeechRequest> = VecDeque::new();
    let mut busy = false;

    let pump = |next_lane: &mut VecDeque<SpeechRequest>,
                queued_lane: &mut VecDeque<SpeechRequest>,
                busy: &mut bool| {
        if *busy {
            return;
        }
        let request = next_lane.pop_front().or_else(|| queued_lane.pop_front());
        if let Some(request) = request {
            // Reset the cancel flag for this fresh job right before dispatch.
            cancel.store(false, Ordering::Release);
            *busy = true;
            let _ = synth_tx.send(SynthCommand::Job(request));
        }
    };

    while let Ok(event) = queue_rx.recv() {
        match event {
            QueueEvent::Speak(utterance) => {
                let priority = utterance.priority;
                let request = render_utterance(&utterance);
                let at = Instant::now();
                if let Some(observer) = &events {
                    observer.utterance_queued(request.trace_id, &request.text, at);
                }
                trace!(
                    target: "verbatim::speech",
                    trace_id = %request.trace_id,
                    priority = ?priority,
                    "utterance_queued"
                );
                match priority {
                    verbatim_model::SpeechPriority::Interrupt => {
                        cancel.store(true, Ordering::Release);
                        next_lane.clear();
                        queued_lane.clear();
                        next_lane.push_back(request);
                    }
                    verbatim_model::SpeechPriority::Next => next_lane.push_back(request),
                    verbatim_model::SpeechPriority::Queued => queued_lane.push_back(request),
                }
                pump(&mut next_lane, &mut queued_lane, &mut busy);
            }
            QueueEvent::ApplySetting { id, value } => {
                let _ = synth_tx.send(SynthCommand::SetSetting { id, value });
            }
            QueueEvent::RestoreSettings(values) => {
                let _ = synth_tx.send(SynthCommand::RestoreSettings(values));
            }
            QueueEvent::SwitchSynth { id, reply } => {
                // Interrupt any current speech so the switch, and the caller
                // waiting on its reply, are not blocked behind a long
                // utterance.
                cancel.store(true, Ordering::Release);
                next_lane.clear();
                queued_lane.clear();
                let _ = synth_tx.send(SynthCommand::Switch { id, reply });
            }
            QueueEvent::SynthFinished => {
                busy = false;
                pump(&mut next_lane, &mut queued_lane, &mut busy);
            }
            QueueEvent::Shutdown => {
                let _ = synth_tx.send(SynthCommand::Shutdown);
                break;
            }
        }
    }
}

/// The synth thread: owns the active driver and the audio sink and runs
/// [`SynthDriver::speak`].
// The parameters are moved in to be owned for the thread's lifetime, even
// where the body only reads them.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
fn synth_thread(
    registry: SynthRegistry,
    initial_synth: SynthId,
    initial_settings: Vec<(SettingId, SettingValue)>,
    mut sink: Box<dyn AudioSink>,
    cancel: Arc<AtomicBool>,
    events: Option<Arc<dyn SpeechEvents>>,
    synth_rx: Receiver<SynthCommand>,
    queue_tx: Sender<QueueEvent>,
    startup_tx: Sender<Result<StartupInfo, SynthError>>,
) {
    // Build the initial driver and apply persisted settings before reporting
    // startup success.
    let mut driver = match registry.build(&initial_synth) {
        Ok(driver) => driver,
        Err(error) => {
            let _ = startup_tx.send(Err(error));
            return;
        }
    };
    for (id, value) in initial_settings {
        if let Err(error) = driver.set_setting(&id, value) {
            let _ = startup_tx.send(Err(error));
            return;
        }
    }
    let _ = startup_tx.send(Ok(StartupInfo {
        state: driver_state(driver.as_ref()),
        choices: registry.choices(),
    }));

    let events = events.as_deref();
    while let Ok(command) = synth_rx.recv() {
        match command {
            SynthCommand::Job(request) => {
                run_job(driver.as_mut(), sink.as_mut(), &cancel, events, &request);
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
            SynthCommand::Switch { id, reply } => match registry.build(&id) {
                Ok(new_driver) => {
                    driver = new_driver;
                    let _ = reply.send(Ok(driver_state(driver.as_ref())));
                }
                Err(error) => {
                    let _ = reply.send(Err(error));
                }
            },
            SynthCommand::Shutdown => break,
        }
    }
}

/// Runs one synthesis job end to end: streams PCM through the sink, honoring
/// cancellation, and finishes or discards the audio accordingly.
fn run_job(
    driver: &mut dyn SynthDriver,
    sink: &mut dyn AudioSink,
    cancel: &AtomicBool,
    events: Option<&dyn SpeechEvents>,
    request: &SpeechRequest,
) {
    let format = driver.pcm_format();
    let mut pipeline = PipelineSink {
        audio: sink,
        cancel,
        events,
        trace_id: request.trace_id,
        format,
        begun: false,
        first_written: false,
        error: None,
    };
    let result = driver.speak(request, &mut pipeline);
    let begun = pipeline.begun;
    let sink_error = pipeline.error.take();
    // End the mutable borrow of `sink` before using it directly.
    drop(pipeline);

    let cancelled = cancel.load(Ordering::Acquire);
    let had_sink_error = sink_error.is_some();
    let had_synth_error = result.is_err();
    if begun {
        if cancelled || had_sink_error || had_synth_error {
            sink.stop();
        } else if let Err(error) = sink.end() {
            warn!(target: "verbatim::speech", %error, "draining audio failed");
        }
    }
    if let Some(error) = sink_error {
        warn!(target: "verbatim::speech", %error, "audio sink error, utterance dropped");
    }
    if let Err(error) = result {
        warn!(target: "verbatim::speech", %error, "synthesis failed, utterance dropped");
    }
    // The utterance played to completion (audio began, drained cleanly, no
    // cancellation) — report it, on the synth thread, right after `sink.end()`
    // above returned from draining. An interrupted or failed utterance is
    // deliberately silent here, so this pairs with `audio_started`.
    if begun
        && !cancelled
        && !had_sink_error
        && !had_synth_error
        && let Some(observer) = events
    {
        observer.utterance_finished(request.trace_id, Instant::now());
    }
}

/// Bridges a driver's [`SynthSink`] output to the [`AudioSink`], opening the
/// stream on the first PCM, reporting cancellation, and firing the
/// audio-started milestone.
struct PipelineSink<'a> {
    audio: &'a mut dyn AudioSink,
    cancel: &'a AtomicBool,
    events: Option<&'a dyn SpeechEvents>,
    trace_id: TraceId,
    format: PcmFormat,
    begun: bool,
    first_written: bool,
    error: Option<verbatim_audio::AudioError>,
}

impl SynthSink for PipelineSink<'_> {
    fn push_pcm(&mut self, samples: &[i16]) -> ControlFlow<()> {
        if self.cancel.load(Ordering::Acquire) {
            return ControlFlow::Break(());
        }
        if !self.begun {
            if let Err(error) = self.audio.begin(self.format, self.trace_id) {
                self.error = Some(error);
                return ControlFlow::Break(());
            }
            self.begun = true;
        }
        if let Err(error) = self.audio.write(samples) {
            self.error = Some(error);
            return ControlFlow::Break(());
        }
        if !self.first_written {
            self.first_written = true;
            if let Some(observer) = self.events {
                observer.audio_started(self.trace_id, Instant::now());
            }
        }
        if self.cancel.load(Ordering::Acquire) {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    fn index_reached(&mut self, mark: IndexMark) {
        // M1 has no index-callback registry yet; record the crossing on the
        // trace so say-all and braille-sync consumers can be layered on later.
        trace!(
            target: "verbatim::speech",
            trace_id = %self.trace_id,
            mark = mark.0,
            "index_reached"
        );
    }
}
