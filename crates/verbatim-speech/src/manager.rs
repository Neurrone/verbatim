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
//!
//! When speech is cut off (`docs/nvda/speech.md`, "Cancellation"). Besides
//! an `Interrupt` utterance, three things end speech early:
//! [`SpeechControl::cancel`], which a key press calls; speaking while
//! paused, which cancels first, as a key press would have; and
//! [`SpeechControl::drop_expired`], which a focus change calls. As in
//! NVDA, that judges only speech already handed to the synthesizer and the
//! mixer: if any of it no longer holds ([`FocusValidity`]), everything
//! handed on is stopped, since audio cannot be taken out of the middle of
//! the mixer's buffer (NVDA stops at the newest expired utterance). Waiting
//! speech is judged when its turn comes, against where the focus is then,
//! and dropped on its own if it no longer holds. [`SpeechControl::toggle_pause`], which Shift calls,
//! holds the speech where it is until it is called again or speech is
//! cancelled.

use std::collections::{HashMap, VecDeque};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use tracing::{info, trace, warn};
use verbatim_audio::{AudioError, Mixer, PcmFormat, PlaybackEvent, Sound, Source};
use verbatim_model::{
    Earcon, FocusNow, FocusValidity, Indication, SpeechPriority, TraceId, Utterance,
    UtteranceEnding, UtteranceId, UtteranceSegment,
};

use crate::driver::{
    IndexMark, SoundCue, SpeechItem, SpeechSequence, SynthDriver, SynthError, SynthSink,
};
use crate::events::SpeechEvents;
use crate::registry::SynthRegistry;
use crate::settings::{SettingId, SettingValue, SynthChoice, SynthId};
use crate::theme::{Presenter, ThemeHandle, ThemePresenter};
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
    /// The presentation stage flattening utterances (decision D12);
    /// `None` selects a [`ThemePresenter`] of the manager's own
    /// [`ThemeHandle`] ([`SpeechManager::themes`]), which starts with the
    /// built-in default theme and no sounds until one is set.
    pub theme: Option<Box<dyn Presenter>>,
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
    /// Cancel everything, as a key press does.
    Cancel,
    /// Pause speech, or resume it when paused.
    TogglePause,
    /// The focus moved: drop focus speech that no longer holds.
    DropExpired(FocusNow),
    /// The mixer ended an utterance it had been handed.
    Ended(UtteranceId),
    Shutdown,
}

/// A handle for cutting speech off, from any thread, without blocking:
/// what a key press and a focus change use. Cheap to clone.
#[derive(Clone)]
pub struct SpeechControl {
    queue_tx: Sender<QueueEvent>,
}

impl SpeechControl {
    /// Cancels current and queued speech, and ends a pause.
    pub fn cancel(&self) {
        let _ = self.queue_tx.send(QueueEvent::Cancel);
    }

    /// Pauses speech where it is, or resumes it when paused.
    pub fn toggle_pause(&self) {
        let _ = self.queue_tx.send(QueueEvent::TogglePause);
    }

    /// Tells the manager where the focus now is: speech being spoken whose
    /// focus has moved on is stopped, and waiting focus speech is judged
    /// against this when its turn comes.
    pub fn drop_expired(&self, now: FocusNow) {
        let _ = self.queue_tx.send(QueueEvent::DropExpired(now));
    }
}

/// A sequence waiting in a lane, with what its focus speech is about.
struct Waiting {
    sequence: SpeechSequence,
    validity: Option<FocusValidity>,
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
    /// The active theme, which presents speech and events.
    themes: ThemeHandle,
    /// The mixer source events' sounds play on at once.
    earcons: Source,
    events: Option<Arc<dyn SpeechEvents>>,
    /// Utterances someone waits to end ([`play_earcon_to_end`](Self::play_earcon_to_end)).
    waiters: Arc<Waiters>,
    initial_state: DriverState,
    /// The synthesizer the configuration asked for, which a fallback at
    /// startup may not be.
    configured: SynthId,
    choices: Vec<SynthChoice>,
    /// Whether speech is paused, as the queue thread last applied it.
    paused: Arc<AtomicBool>,
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
        let themes = ThemeHandle::default();
        let theme = theme.unwrap_or_else(|| Box::new(ThemePresenter::new(themes.clone())));
        // Every ending passes through the waiters on its way to the
        // observer, so a caller can wait for an utterance to be heard.
        let waiters = Arc::new(Waiters::default());
        let events: Option<Arc<dyn SpeechEvents>> = Some(Arc::new(Observer {
            inner: events,
            waiters: Arc::clone(&waiters),
        }));
        let (queue_tx, queue_rx) = unbounded::<QueueEvent>();
        let source = mixer.add_source(playback_listener(events.clone(), queue_tx.clone()));
        let earcons = earcons_source(&mixer, &waiters);

        let (synth_tx, synth_rx) = unbounded::<SynthCommand>();
        let (startup_tx, startup_rx) = bounded::<Result<StartupInfo, SynthError>>(1);

        let configured = initial_synth.clone();
        let synth_handle = {
            let source = source.clone();
            let queue_tx = queue_tx.clone();
            let events = events.clone();
            std::thread::Builder::new()
                .name("verbatim-synth".to_owned())
                .spawn(move || {
                    synth_thread(
                        &registry,
                        &initial_synth,
                        &saved_settings,
                        Output {
                            source: &source,
                            events: events.as_deref(),
                        },
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

        let queue_events = events.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let queue_paused = Arc::clone(&paused);
        let queue_handle = std::thread::Builder::new()
            .name("verbatim-speech-queue".to_owned())
            .spawn(move || {
                QueueThread {
                    synth_tx,
                    source,
                    events: queue_events,
                    theme,
                    next_lane: VecDeque::new(),
                    queued_lane: VecDeque::new(),
                    in_flight: None,
                    handed_on: VecDeque::new(),
                    paused: false,
                    paused_flag: queue_paused,
                    focus_now: None,
                }
                .run(&queue_rx);
            })
            .map_err(|error| {
                SynthError::Unavailable(format!("failed to start queue thread: {error}"))
            })?;

        Ok(Self {
            paused,
            queue_tx,
            _mixer: mixer,
            themes,
            earcons,
            events,
            waiters,
            initial_state,
            configured,
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
    #[allow(
        clippy::must_use_candidate,
        reason = "most speech is spoken without following its utterance"
    )]
    pub fn speak(&self, utterance: Utterance) -> UtteranceId {
        let id = mint_utterance();
        self.speak_as(id, utterance);
        id
    }

    /// Enqueues `utterance` under an id already minted.
    fn speak_as(&self, id: UtteranceId, utterance: Utterance) {
        let _ = self
            .queue_tx
            .send(QueueEvent::Speak(id, Box::new(utterance)));
    }

    /// The handle on the active theme, which the settings dialog switches
    /// and the shell sets at startup (an [`ActiveTheme`](crate::ActiveTheme)
    /// made from the theme the configuration names).
    #[must_use]
    pub fn themes(&self) -> ThemeHandle {
        self.themes.clone()
    }

    /// Reports an event at once, as the active theme says
    /// (`Effect::PlayEarcon`): its sound plays now on its own mixer source,
    /// mixed over any speech and never cancelled by it, and its words, when
    /// the theme speaks it, are queued as speech. Non-blocking.
    pub fn play_earcon(&self, earcon: Earcon) {
        let (sound, words) = self.themes.get().earcon(earcon);
        if let Some((sound, gain)) = sound {
            match self.earcons.play(&sound, gain) {
                Ok(()) => {
                    if let Some(events) = &self.events {
                        events.sound_played(Indication::of_earcon(earcon), Instant::now());
                    }
                }
                Err(error) => {
                    warn!(target: "verbatim::speech", ?earcon, %error, "playing an earcon failed");
                }
            }
        }
        if let Some(words) = words {
            self.speak(earcon_words(words));
        }
    }

    /// Reports an event as [`play_earcon`](Self::play_earcon) does, and
    /// waits until it has been heard: its sound has played to its end and
    /// its words, if the theme speaks it, have been spoken. For the exit
    /// sound, which must be heard before Verbatim exits. Returns `false`
    /// when `timeout` passed first, which bounds the wait however the audio
    /// device behaves.
    pub fn play_earcon_to_end(&self, earcon: Earcon, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let (sound, words) = self.themes.get().earcon(earcon);
        let (ended_tx, ended_rx) = unbounded::<UtteranceId>();
        let mut waiting = Vec::new();
        if let Some((sound, gain)) = sound {
            // The sound plays over an utterance of silence as long as it
            // is, on the events' source, so that utterance's ending says
            // the sound has played to its end.
            let id = mint_utterance();
            self.waiters.wait_for(id, ended_tx.clone());
            waiting.push(id);
            self.earcons.register(id, TraceId::mint());
            let placed = self.earcons.sound(id, &sound, gain);
            let format = sound.format();
            let frames = sound.duration().as_micros() * u128::from(format.sample_rate) / 1_000_000;
            let samples = usize::try_from(frames)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::from(format.channels));
            let _ = self.earcons.write(id, format, &vec![0; samples]);
            self.earcons.finish(id);
            match placed {
                Ok(()) => {
                    if let Some(events) = &self.events {
                        events.sound_played(Indication::of_earcon(earcon), Instant::now());
                    }
                }
                Err(error) => {
                    warn!(target: "verbatim::speech", ?earcon, %error, "playing an earcon failed");
                }
            }
        }
        if let Some(words) = words {
            let id = mint_utterance();
            self.waiters.wait_for(id, ended_tx);
            waiting.push(id);
            self.speak_as(id, earcon_words(words));
        }
        let mut heard = true;
        for _ in 0..waiting.len() {
            if ended_rx.recv_deadline(deadline).is_err() {
                heard = false;
                break;
            }
        }
        for id in waiting {
            self.waiters.forget(id);
        }
        heard
    }

    /// Plays `sound` at once at `gain` (1.0 for as recorded), on the
    /// events' mixer source: for the settings dialog to let a sound be
    /// heard, as the theme panel's sound list and volume slider do.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::Stream`] when the sound cannot be converted to
    /// the device's format.
    pub fn play_sound(&self, sound: &Sound, gain: f32) -> Result<(), AudioError> {
        self.earcons.play(sound, gain)
    }

    /// Whether speech is paused. It changes once the manager has applied a
    /// [`SpeechControl::toggle_pause`] or a resume, after the mixer has been
    /// told, so audio arriving from then on is ordered after the pause.
    #[must_use]
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// A handle for cancelling, pausing, and dropping expired speech.
    #[must_use]
    pub fn control(&self) -> SpeechControl {
        SpeechControl {
            queue_tx: self.queue_tx.clone(),
        }
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
            self.initial_state.choice.id == self.configured,
            persist,
        )
    }
}

/// Adds the source events' sounds play on at once, whose utterances (the
/// silence under a sound someone waits for) end through the [`Waiters`].
fn earcons_source(mixer: &Mixer, waiters: &Arc<Waiters>) -> Source {
    let waiters = Arc::clone(waiters);
    mixer.add_source(Arc::new(move |event| {
        if let PlaybackEvent::Ended { utterance, .. } = event {
            waiters.ended(utterance);
        }
    }))
}

/// Mints a new utterance id, process-wide.
fn mint_utterance() -> UtteranceId {
    UtteranceId(NEXT_UTTERANCE.fetch_add(1, Ordering::Relaxed))
}

/// The utterance speaking an event's words.
fn earcon_words(words: String) -> Utterance {
    Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text(words)],
        source: None,
        say_all: false,
        validity: None,
    }
}

/// Utterances someone waits to end, each with where to say it has.
#[derive(Default)]
struct Waiters(Mutex<HashMap<UtteranceId, Sender<UtteranceId>>>);

impl Waiters {
    /// Says on `ended` when `utterance` ends.
    fn wait_for(&self, utterance: UtteranceId, ended: Sender<UtteranceId>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(utterance, ended);
    }

    /// Stops waiting for `utterance`.
    fn forget(&self, utterance: UtteranceId) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&utterance);
    }

    /// `utterance` has ended: tells whoever waits for it. Cheap, as the
    /// audio thread calls it.
    fn ended(&self, utterance: UtteranceId) {
        let waiter = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&utterance);
        if let Some(waiter) = waiter {
            let _ = waiter.send(utterance);
        }
    }
}

/// The observer the pipeline reports to: the configured one, with every
/// ending also passed to the [`Waiters`].
struct Observer {
    inner: Option<Arc<dyn SpeechEvents>>,
    waiters: Arc<Waiters>,
}

impl SpeechEvents for Observer {
    fn utterance_queued(&self, utterance: UtteranceId, trace_id: TraceId, text: &str, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.utterance_queued(utterance, trace_id, text, at);
        }
    }

    fn audio_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.audio_started(utterance, trace_id, at);
        }
    }

    fn synthesis_started(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.synthesis_started(utterance, trace_id, at);
        }
    }

    fn synthesizer_audio(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.synthesizer_audio(utterance, trace_id, at);
        }
    }

    fn audio_to_mixer(&self, utterance: UtteranceId, trace_id: TraceId, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.audio_to_mixer(utterance, trace_id, at);
        }
    }

    fn mark_reached(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        mark: IndexMark,
        at: Instant,
    ) {
        if let Some(inner) = &self.inner {
            inner.mark_reached(utterance, trace_id, mark, at);
        }
    }

    fn sound_played(&self, indication: Indication, at: Instant) {
        if let Some(inner) = &self.inner {
            inner.sound_played(indication, at);
        }
    }

    fn utterance_ended(
        &self,
        utterance: UtteranceId,
        trace_id: TraceId,
        ending: &UtteranceEnding,
        at: Instant,
    ) {
        if let Some(inner) = &self.inner {
            inner.utterance_ended(utterance, trace_id, ending, at);
        }
        self.waiters.ended(utterance);
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
fn playback_listener(
    events: Option<Arc<dyn SpeechEvents>>,
    queue_tx: Sender<QueueEvent>,
) -> verbatim_audio::PlaybackListener {
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
                let _ = queue_tx.send(QueueEvent::Ended(utterance));
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
    theme: Box<dyn Presenter>,
    next_lane: VecDeque<Waiting>,
    queued_lane: VecDeque<Waiting>,
    /// The cancellation flag of the job the synth thread is working on.
    in_flight: Option<Arc<AtomicBool>>,
    /// Utterances handed to the synth thread and the mixer whose ending
    /// has not been reported yet, oldest first, for dropping expired speech.
    handed_on: VecDeque<(UtteranceId, Option<FocusValidity>)>,
    /// Speech is paused.
    paused: bool,
    /// [`paused`](Self::paused), published for [`SpeechManager::paused`]
    /// once the mixer has been told.
    paused_flag: Arc<AtomicBool>,
    /// Where the focus was last reported, for judging waiting focus speech
    /// when its turn comes.
    focus_now: Option<FocusNow>,
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
                QueueEvent::Cancel => self.cancel_everything(),
                QueueEvent::TogglePause => {
                    self.paused = !self.paused;
                    self.source.pause(self.paused);
                    self.paused_flag.store(self.paused, Ordering::Release);
                }
                QueueEvent::DropExpired(now) => self.drop_expired(now),
                QueueEvent::Ended(id) => self.handed_on.retain(|(handed, _)| *handed != id),
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
        // Speech arriving while paused cancels what was paused first, as
        // the key press that would otherwise have come first does.
        if self.paused {
            self.cancel_everything();
        }
        let waiting = Waiting {
            sequence,
            validity: utterance.validity,
        };
        match utterance.priority {
            SpeechPriority::Interrupt => {
                self.cancel_everything();
                self.next_lane.push_back(waiting);
            }
            SpeechPriority::Next => self.next_lane.push_back(waiting),
            SpeechPriority::Queued => self.queued_lane.push_back(waiting),
        }
        self.pump();
    }

    /// The focus moved. As NVDA does, only speech already handed on is
    /// judged now: when any of it no longer holds, everything handed on is
    /// stopped. Waiting speech is judged when its turn comes (`pump`),
    /// against where the focus is then.
    fn drop_expired(&mut self, now: FocusNow) {
        let expired = self
            .handed_on
            .iter()
            .any(|(_, validity)| validity.is_some_and(|validity| !validity.holds(&now)));
        self.focus_now = Some(now);
        if expired {
            self.stop_handed_on();
        }
    }

    /// Stops everything handed to the synth thread and the mixer.
    fn stop_handed_on(&mut self) {
        if let Some(cancel) = &self.in_flight {
            cancel.store(true, Ordering::Release);
        }
        self.source.cancel_all();
        // Everything handed on has just been cancelled. The mixer reports
        // each ending a little later, on its own thread; until then these
        // must not be judged again, or a focus change in between would
        // find them expired and stop speech handed on since.
        self.handed_on.clear();
        self.unpause();
    }

    /// Reports waiting sequences cancelled.
    fn end_waiting(&self, waiting: impl IntoIterator<Item = Waiting>) {
        let now = Instant::now();
        for waiting in waiting {
            if let Some(events) = &self.events {
                events.utterance_ended(
                    waiting.sequence.utterance,
                    waiting.sequence.trace_id,
                    &UtteranceEnding::Cancelled,
                    now,
                );
            }
        }
    }

    fn unpause(&mut self) {
        if self.paused {
            self.paused = false;
            self.source.pause(false);
            self.paused_flag.store(false, Ordering::Release);
        }
    }

    /// Hands the next sequence to the synth thread when it is idle. The
    /// mixer is told about the utterance first, so from here on the mixer
    /// owns its ending.
    fn pump(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let Waiting { sequence, validity } = loop {
            let Some(waiting) = self
                .next_lane
                .pop_front()
                .or_else(|| self.queued_lane.pop_front())
            else {
                return;
            };
            // Focus speech whose focus has moved on is dropped when its turn
            // comes, as NVDA checks it before speaking it.
            let expired = waiting
                .validity
                .zip(self.focus_now.as_ref())
                .is_some_and(|(validity, now)| !validity.holds(now));
            if !expired {
                break waiting;
            }
            self.end_waiting([waiting]);
        };
        self.handed_on.push_back((sequence.utterance, validity));
        self.source.register(sequence.utterance, sequence.trace_id);
        let cancel = Arc::new(AtomicBool::new(false));
        self.in_flight = Some(Arc::clone(&cancel));
        let _ = self
            .synth_tx
            .send(SynthCommand::Job(Job { sequence, cancel }));
    }

    /// Cancels everything: the job being synthesized, every sequence waiting
    /// in a lane, and every utterance the mixer has not finished playing;
    /// and ends a pause.
    fn cancel_everything(&mut self) {
        let waiting = self
            .next_lane
            .drain(..)
            .chain(self.queued_lane.drain(..))
            .collect::<Vec<_>>();
        self.end_waiting(waiting);
        self.stop_handed_on();
    }
}

/// Where the synth thread's audio goes, and who hears of its milestones.
#[derive(Clone, Copy)]
struct Output<'a> {
    source: &'a Source,
    events: Option<&'a dyn SpeechEvents>,
}

/// The synth thread: owns the active driver and runs
/// [`SynthDriver::speak`].
fn synth_thread(
    registry: &SynthRegistry,
    initial_synth: &SynthId,
    saved_settings: &SavedSettingsFn,
    output: Output<'_>,
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
                run_job(driver.as_mut(), output, &job);
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
fn run_job(driver: &mut dyn SynthDriver, output: Output<'_>, job: &Job) {
    let Output { source, events } = output;
    // Sounds become marks the sink places them at, so they keep their
    // places in the audio however the driver handles marks.
    let (sequence, sounds) = sounds_as_marks(&job.sequence);
    let sequence = &sequence;
    let utterance = sequence.utterance;
    if job.cancel.load(Ordering::Acquire) {
        // Cancelled before it started; the mixer has already ended it.
        return;
    }
    if let Some(events) = events {
        events.synthesis_started(utterance, sequence.trace_id, Instant::now());
    }
    // A driver that changes pitch itself is given the pitch changes. For
    // any other, a pitch change is its own pitch setting, changed between
    // pieces from the value it had when the job began, and always put back;
    // a driver with no pitch setting is not split for pitch at all.
    let inline_pitch = driver.changes_pitch();
    let base_pitch = match driver.setting(&PITCH) {
        Some(SettingValue::Number(pitch)) if !inline_pitch && sequence.has_pitch_changes() => {
            Some(pitch)
        }
        _ => None,
    };
    let split_marks = !driver.places_marks() && sequence.has_marks();
    let split_pitch = base_pitch.is_some();
    let sequence = if inline_pitch || split_pitch {
        sequence.clone()
    } else {
        sequence.without_pitch_changes()
    };
    let pieces = if split_marks || split_pitch {
        sequence.split(split_marks, split_pitch)
    } else {
        vec![(sequence.clone(), None)]
    };
    let mut sink = PipelineSink {
        source,
        utterance,
        trace_id: sequence.trace_id,
        sounds: &sounds,
        events,
        cancel: &job.cancel,
        trimmer: Trimmer::default(),
        stopped: false,
        driver_audio: false,
        mixer_audio: false,
    };
    let mut result = Ok(());
    for (piece, mark) in pieces {
        if piece.has_text() {
            result = driver.speak(&piece, &mut sink);
            if result.is_err() || sink.stopped || job.cancel.load(Ordering::Acquire) {
                break;
            }
        } else {
            // Nothing to synthesize, as in a sound with no words: its marks,
            // and the sounds standing as marks, are placed where it stands.
            for item in &piece.items {
                if let SpeechItem::Mark(mark) = item {
                    sink.index_reached(*mark);
                }
            }
        }
        match mark {
            Some(SpeechItem::Mark(mark)) => sink.index_reached(mark),
            Some(SpeechItem::Pitch(offset)) => {
                if let Some(base) = base_pitch {
                    set_pitch(driver, base.saturating_add(offset));
                }
            }
            _ => {}
        }
    }
    if let Some(base) = base_pitch {
        set_pitch(driver, base);
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

/// The marks standing for sounds have this bit set; the reducer's marks
/// never reach it.
const SOUND_MARK: u64 = 1 << 63;

/// `sequence` with each sound replaced by a mark with [`SOUND_MARK`] set
/// and the sound's index, and the sounds in order.
fn sounds_as_marks(sequence: &SpeechSequence) -> (SpeechSequence, Vec<SoundCue>) {
    let mut sounds = Vec::new();
    let items = sequence
        .items
        .iter()
        .map(|item| match item {
            SpeechItem::Sound(cue) => {
                sounds.push(cue.clone());
                SpeechItem::Mark(IndexMark(SOUND_MARK | (sounds.len() as u64 - 1)))
            }
            other => other.clone(),
        })
        .collect();
    (
        SpeechSequence {
            utterance: sequence.utterance,
            trace_id: sequence.trace_id,
            language: sequence.language.clone(),
            items,
        },
        sounds,
    )
}

/// The setting a [`SpeechItem::Pitch`] changes.
static PITCH: std::sync::LazyLock<SettingId> = std::sync::LazyLock::new(|| SettingId::new("pitch"));

/// Sets the driver's pitch, within the 0 to 100 every numeric setting
/// uses.
fn set_pitch(driver: &mut dyn SynthDriver, pitch: i32) {
    if let Err(error) = driver.set_setting(&PITCH, SettingValue::Number(pitch.clamp(0, 100))) {
        warn!(target: "verbatim::speech", %error, "changing pitch for a capital failed");
    }
}

/// Bridges a driver's [`SynthSink`] output, trimmed of silence, to the
/// mixer.
struct PipelineSink<'a> {
    source: &'a Source,
    utterance: UtteranceId,
    trace_id: verbatim_model::TraceId,
    /// The sequence's sounds, placed where the marks standing for them
    /// arrive.
    sounds: &'a [SoundCue],
    events: Option<&'a dyn SpeechEvents>,
    /// The driver has given audio, and the trimmer has passed audio on.
    driver_audio: bool,
    mixer_audio: bool,
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
                    if !self.mixer_audio {
                        self.mixer_audio = true;
                        if let Some(events) = self.events {
                            events.audio_to_mixer(self.utterance, self.trace_id, Instant::now());
                        }
                    }
                    if self
                        .source
                        .write(self.utterance, format, &samples)
                        .is_break()
                    {
                        self.stopped = true;
                        return ControlFlow::Break(());
                    }
                }
                Piece::Mark(mark) if mark.0 & SOUND_MARK != 0 => {
                    let index = usize::try_from(mark.0 & !SOUND_MARK).unwrap_or(usize::MAX);
                    if let Some(cue) = self.sounds.get(index)
                        && let Err(error) = self.source.sound(self.utterance, &cue.sound, cue.gain)
                    {
                        warn!(target: "verbatim::speech", utterance = %self.utterance, %error, "placing a sound failed");
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
        if !self.driver_audio {
            self.driver_audio = true;
            if let Some(events) = self.events {
                events.synthesizer_audio(self.utterance, self.trace_id, Instant::now());
            }
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
