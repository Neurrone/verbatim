//! `verbatim.exe` — the composition root (architecture section 1).
//!
//! Startup order: namespace trace IDs, load config, start tracing, replace
//! any running instance, load locales, bring up the speech pipeline, the
//! supervisor and its focus listener (decision D13), the reducer and router
//! threads, the control plane, and the keyboard hook — then run the wxDragon
//! GUI loop on this, the process main thread, until shutdown is requested from
//! the menu, the control plane, or a replacing instance.

mod clipboard;
mod datetime;
mod flight_dump;
mod latency;
mod live;
mod requests;
mod single_instance;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use verbatim_audio::{AudioSink, NullSink};
use verbatim_audio_wasapi::WasapiSink;
use verbatim_config::{ConfigStore, ConfigValue};
use verbatim_control::protocol::{OutpostState, OutpostStatus, StatusInfo};
use verbatim_control::server::{ControlServer, ServerHandlers};
use verbatim_core::{ReducerRecorder, SrState, reduce};
use verbatim_gui::{GuiCommand, GuiEvent, GuiHandle, ShellItemKind, run_gui};
use verbatim_input::{
    DecisionConfig, EmittedGesture, GestureMap, KeyboardLayout, ScriptAction, SharedGestureMap,
};
use verbatim_input_windows::InputHook;
use verbatim_model::{
    Effect, GestureId, Input, OutpostId, Pid, ReviewCommand, SpeechPriority, TraceId, Utterance,
    UtteranceSegment,
};
use verbatim_outpost::protocol::{OutpostToSupervisor, Query, QueryOutcome, SupervisorToOutpost};
use verbatim_outpost::supervisor::EndReason;
use verbatim_outpost::{OutpostMessage, Supervisor};
use verbatim_speech::{
    SettingId, SettingValue, SpeechManager, SpeechManagerConfig, SpeechSettingsHost, SynthId,
    SynthRegistry,
};
use verbatim_synth_capture::CaptureSynth;

use latency::LatencyLedger;
use live::LiveOutposts;
use requests::{Asker, DumpTreeResult, RequestId, RequestTable};

/// The one binding not carried by the keyboard layout's own script table:
/// Verbatim+V opens the menu. The review, object-navigation, time, and
/// tray-list bindings all come from `verbatim_input::bindings_for` for the
/// active layout (roadmap M3).
const SHOW_MENU_GESTURE: &str = "kb:verbatim+v";

/// How long a `DumpTree` control-plane request waits for the outpost's
/// answer before giving up.
const DUMP_TREE_TIMEOUT: Duration = Duration::from_secs(5);

/// The reducer's flight recorder, shared between the reducer thread (which
/// records each input as it processes it) and both dump triggers: a
/// `DumpRecorder` control-plane request and the panic hook installed at
/// startup.
type SharedRecorder = Arc<Mutex<ReducerRecorder>>;

/// The folder flight-recorder dumps are written to, next to the executable.
const DUMPS_FOLDER: &str = "dumps";

fn main() -> ExitCode {
    // Keep Core's trace IDs disjoint from every outpost's; they meet in the
    // latency ledger and the flight recorder.
    TraceId::namespace(std::process::id());

    let exe_dir = exe_dir();
    let config = load_config(&exe_dir);
    init_tracing(config.settings().log_filter.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "verbatim starting");

    // Replace a running instance before creating anything it might still own.
    let _instance = match single_instance::acquire_replacing() {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(%error, "single-instance startup failed");
            eprintln!("verbatim: {error}");
            return ExitCode::FAILURE;
        }
    };

    load_locales(&exe_dir, &config);

    match run(config) {
        Ok(()) => {
            tracing::info!("verbatim exiting");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "verbatim failed to start");
            eprintln!("verbatim: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Everything after the process-level preliminaries; errors here are startup
/// failures reported to the user.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root wires every subsystem together in one place; splitting it would scatter the startup order this function exists to make legible"
)]
fn run(config: ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    let own_pid = std::process::id();

    // The control server is created late (it needs the other pieces'
    // handlers), but earlier pieces need to reach it for broadcasting.
    let server_slot: Arc<OnceLock<ControlServer>> = Arc::new(OnceLock::new());
    let ledger = Arc::new(LatencyLedger::new(256, Arc::clone(&server_slot)));

    // The flight recorder, and its panic-time dump trigger: installed as
    // early as possible, chaining the previous hook, so a panic on any
    // thread from here on writes a dump before the process dies.
    let recorder: SharedRecorder = Arc::new(Mutex::new(ReducerRecorder::new(1024)));
    let dumps_dir = exe_dir().join(DUMPS_FOLDER);
    flight_dump::install_panic_hook(Arc::clone(&recorder), dumps_dir.clone());

    // Speech pipeline: OneCore through WASAPI by default, observed by the
    // latency ledger; VERBATIM_TEST_AUDIO=null swaps in device-free test
    // audio (see build_speech_manager).
    let manager = build_speech_manager(&config, &ledger)?;

    // The keyboard layout selects which review and object-navigation
    // bindings are active (roadmap M3); read it before `config` moves into
    // the store. File-only in M3, so no live re-read is needed. The config
    // and input crates each own a `KeyboardLayout` (deliberately decoupled),
    // so translate here at the seam.
    let keyboard_layout = match config.settings().keyboard.layout {
        verbatim_config::KeyboardLayout::Desktop => KeyboardLayout::Desktop,
        verbatim_config::KeyboardLayout::Laptop => KeyboardLayout::Laptop,
    };

    // Settings host: the GUI's live handle; commit persists to the base
    // profile through the config store.
    let store = Arc::new(Mutex::new(config));
    let settings_host = manager.settings_host(persist_fn(Arc::clone(&store)));

    // Supervisor (one outpost process per application, decision D9) plus its
    // dedicated focus listener (decision D13), which detects focus and
    // foreground changes desktop-wide and reports them as facts the supervisor
    // routes; outpost status is mirrored for the control plane. Core no longer
    // runs its own foreground hook — the listener absorbs it, and a foreground
    // change reaches the reducer as a focus on the window, which moves its
    // attention.
    let (outpost_tx, outpost_rx) = unbounded::<OutpostMessage>();
    let supervisor = Arc::new(Supervisor::new(outpost_tx)?);
    let outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>> = Arc::new(Mutex::new(HashMap::new()));

    warm_own_outpost(&supervisor, &outposts, own_pid);

    // Review and object-navigation commands from the router, and tree dumps
    // from the control plane, reach the reducer thread over this channel; it
    // selects on it alongside the outpost stream (see `reducer_loop`).
    let (command_tx, command_rx) = unbounded::<ShellCommand>();

    // The reducer thread: normalized events and review commands in, speech,
    // fetches, activations, and clipboard copies out.
    {
        let context = ReducerContext {
            manager: Arc::clone(&manager),
            supervisor: Arc::clone(&supervisor),
            ledger: Arc::clone(&ledger),
            server_slot: Arc::clone(&server_slot),
            outposts: Arc::clone(&outposts),
            recorder: Arc::clone(&recorder),
        };
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || reducer_loop(&outpost_rx, &command_rx, &context))?;
    }

    // The gesture router: bound gestures become GUI commands, direct speech,
    // or — for review and object navigation — reducer commands sent over
    // `command_tx`. The GUI handle arrives once the GUI thread is up.
    let gui_handle: Arc<OnceLock<GuiHandle>> = Arc::new(OnceLock::new());
    let (gesture_tx, gesture_rx) = bounded::<EmittedGesture>(64);
    {
        let gui_handle = Arc::clone(&gui_handle);
        let manager = Arc::clone(&manager);
        let command_tx = command_tx.clone();
        thread::Builder::new()
            .name("verbatim-router".to_owned())
            .spawn(move || {
                router_loop(
                    &gesture_rx,
                    &gui_handle,
                    &manager,
                    &command_tx,
                    keyboard_layout,
                );
            })?;
    }

    // GUI events out of the GUI thread: Exit requests shutdown.
    let (gui_event_tx, gui_event_rx) = unbounded::<GuiEvent>();
    {
        let gui_handle = Arc::clone(&gui_handle);
        thread::Builder::new()
            .name("verbatim-gui-events".to_owned())
            .spawn(move || gui_event_loop(&gui_event_rx, &gui_handle))?;
    }

    // The gesture map shared by the hook and the control plane's validation.
    let bound_gestures = bound_gestures(keyboard_layout);

    // Control plane.
    let server = ControlServer::start(control_handlers(ControlHandlersConfig {
        own_pid,
        settings_host: settings_host.clone(),
        outposts: Arc::clone(&outposts),
        ledger: Arc::clone(&ledger),
        bound_gestures: Arc::clone(&bound_gestures),
        gesture_tx: gesture_tx.clone(),
        gui_handle: Arc::clone(&gui_handle),
        command_tx: command_tx.clone(),
        recorder: Arc::clone(&recorder),
        dumps_dir: dumps_dir.clone(),
    }))?;
    server_slot
        .set(server)
        .map_err(|_| "control server slot set twice")?;

    // Keyboard hook, last among the input paths so nothing is swallowed
    // before there is somewhere to route it.
    let _hook = InputHook::start(
        decision_config(&store),
        Arc::clone(&bound_gestures),
        gesture_tx,
    )?;

    // First words, and the initial outpost target (the trigger only fires on
    // foreground changes after this point).
    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Queued,
        segments: vec![UtteranceSegment::text(verbatim_i18n::startup_message())],
        source: None,
    });
    // Ask the foreground application for its current focus: its outpost is
    // started if needed, and the answer is spoken like a switch to it.
    if let Some(pid) = foreground_pid() {
        note_targeted_pid(&outposts, pid);
        let _ = command_tx.send(ShellCommand::FocusNow(pid));
    }

    // The GUI loop owns the main thread until shutdown.
    let host_for_gui: Arc<dyn SpeechSettingsHost> = Arc::new(settings_host);
    let handle_slot = Arc::clone(&gui_handle);
    run_gui(host_for_gui, gui_event_tx, move |handle| {
        let _ = handle_slot.set(handle);
    })?;

    // The wx loop has exited (Exit menu item, control-plane quit, or a
    // replacing instance's WM_QUIT). The keyboard hook stops when `_hook`
    // drops; job objects kill the outposts and the focus listener when the
    // process exits.
    Ok(())
}

/// The folder `verbatim.exe` runs from — the root for config, profiles, and
/// locales (the portable layout).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Loads configuration, falling back to defaults (without overwriting the
/// files) when they are unreadable or corrupt — a screen reader that will
/// not start is worse than one with default settings.
fn load_config(exe_dir: &std::path::Path) -> ConfigStore {
    match ConfigStore::load(exe_dir) {
        Ok(config) => {
            // Materialize missing files with defaults so settings.toml and
            // profiles/base.toml are discoverable and hand-editable from the
            // first run; existing files are never touched.
            if let Err(error) = config.ensure_files_exist() {
                eprintln!("verbatim: could not create default config files: {error}");
            }
            config
        }
        Err(error) => {
            eprintln!("verbatim: config error, continuing with defaults: {error}");
            ConfigStore::load(&std::env::temp_dir().join("verbatim-defaults"))
                .unwrap_or_else(|fallback| panic!("default config must load: {fallback}"))
        }
    }
}

/// Installs the process-wide tracing subscriber; `RUST_LOG` wins over the
/// configured filter, which wins over plain `info`.
fn init_tracing(configured: Option<&str>) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(configured.unwrap_or("info")))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Loads the configured locale from the `locale` folder next to the
/// executable; embedded English remains the fallback.
fn load_locales(exe_dir: &std::path::Path, config: &ConfigStore) {
    let Some(locale) = config.settings().locale.as_deref() else {
        return;
    };
    let Ok(requested) = locale.parse() else {
        tracing::warn!(locale, "configured locale is not a valid language tag");
        return;
    };
    match verbatim_i18n::load_locale_dir(&exe_dir.join("locale"), &[requested]) {
        Ok(loaded) => tracing::info!(?loaded, "locales loaded"),
        Err(error) => tracing::warn!(%error, "locale folder not loaded; using embedded English"),
    }
}

/// Builds the speech pipeline: `OneCore` through WASAPI by default,
/// observed by the latency ledger. `VERBATIM_TEST_AUDIO=null` is a
/// test-only escape hatch (documented in docs/crates/verbatim-audio.md) that swaps in
/// the device-free capture synth and [`NullSink`] instead, so E2E and CI
/// runs work with no sound card.
///
/// # Errors
///
/// Returns an error if the initial synthesizer fails to construct.
fn build_speech_manager(
    config: &ConfigStore,
    ledger: &Arc<LatencyLedger>,
) -> Result<Arc<SpeechManager>, verbatim_speech::SynthError> {
    let test_audio = std::env::var("VERBATIM_TEST_AUDIO").is_ok_and(|value| value == "null");
    let mut registry = SynthRegistry::new();
    verbatim_synth_onecore::register(&mut registry);
    if test_audio {
        tracing::warn!(
            "VERBATIM_TEST_AUDIO=null: test audio mode is active; using the capture synth and a null audio sink, no sound will play"
        );
        register_test_audio(&mut registry);
    }
    let initial_synth = initial_synth(config, &registry);
    let initial_settings = initial_settings(config, &initial_synth);
    let sink: Box<dyn AudioSink> = if test_audio {
        Box::new(NullSink::new())
    } else {
        Box::new(WasapiSink::new())
    };
    Ok(Arc::new(SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth,
        initial_settings,
        sink,
        events: Some(Arc::clone(ledger) as Arc<dyn verbatim_speech::SpeechEvents>),
        theme: None,
    })?))
}

/// Registers the capture synth from `verbatim-synth-capture` alongside
/// `OneCore`, for `VERBATIM_TEST_AUDIO=null` runs. Test-only: never active
/// unless that environment variable is set at startup.
fn register_test_audio(registry: &mut SynthRegistry) {
    registry.register(
        SynthId::new("capture"),
        "Capture (test audio)",
        Box::new(|| Ok(Box::new(CaptureSynth::new()) as Box<dyn verbatim_speech::SynthDriver>)),
    );
}

/// The configured synthesizer when it exists in the registry, otherwise
/// `OneCore`.
fn initial_synth(config: &ConfigStore, registry: &SynthRegistry) -> SynthId {
    let configured = config
        .active()
        .synthesizer()
        .map_or_else(|| SynthId::new("onecore"), SynthId::new);
    if registry.contains(&configured) {
        configured
    } else {
        tracing::warn!(synth = %configured, "configured synthesizer not installed; using onecore");
        SynthId::new("onecore")
    }
}

/// Persisted setting values for the initial synthesizer, mapped from config
/// values to driver values.
fn initial_settings(config: &ConfigStore, synth: &SynthId) -> Vec<(SettingId, SettingValue)> {
    config
        .active()
        .synth_settings(&synth.0)
        .into_iter()
        .filter_map(|(id, value)| {
            let value = match value {
                ConfigValue::Integer(number) => SettingValue::Number(i32::try_from(number).ok()?),
                ConfigValue::Text(option) => SettingValue::Choice(option),
                ConfigValue::Flag(flag) => SettingValue::Toggle(flag),
            };
            Some((SettingId::new(id), value))
        })
        .collect()
}

/// The persist callback the settings host commits through: write the active
/// synth and its values into the base profile and save it.
fn persist_fn(store: Arc<Mutex<ConfigStore>>) -> verbatim_speech::PersistFn {
    Box::new(move |synth_id, values| {
        let mut store = store
            .lock()
            .map_err(|_| "config store poisoned".to_owned())?;
        let speech = &mut store.settings_mut().speech;
        speech.synthesizer = Some(synth_id.0.clone());
        let settings = speech.synth_settings.entry(synth_id.0.clone()).or_default();
        for (id, value) in values {
            let config_value = match value {
                SettingValue::Number(number) => ConfigValue::Integer(i64::from(*number)),
                SettingValue::Choice(option) => ConfigValue::Text(option.clone()),
                SettingValue::Toggle(flag) => ConfigValue::Flag(*flag),
            };
            settings.insert(id.0.clone(), config_value);
        }
        store.save_settings().map_err(|error| error.to_string())
    })
}

/// The hook configuration from global settings.
fn decision_config(store: &Arc<Mutex<ConfigStore>>) -> DecisionConfig {
    let store = store.lock().expect("config store lock");
    let keys = store.settings().verbatim_keys;
    DecisionConfig {
        caps_lock: keys.caps_lock,
        insert: keys.insert,
        numpad_insert: keys.numpad_insert,
        share_modifier: keys.share_modifier,
        ..DecisionConfig::default()
    }
}

/// Notes that `target` is (or is about to be) watched by an outpost, adding a
/// `Starting` placeholder to the status mirror only if this pid is not
/// already known. Multiple outposts coexist under decision D9, so unlike
/// M1's single-outpost policy this must never clear existing entries: a
/// foreground change to a pid Core already has an outpost for re-announces
/// through that outpost (see `Supervisor::note_foreground`) rather than
/// starting a new one, and its existing status entry is left alone.
fn note_targeted_pid(outposts: &Arc<Mutex<HashMap<Pid, OutpostStatus>>>, target: Pid) {
    let mut outposts = outposts.lock().expect("outposts lock");
    outposts.entry(target).or_insert(OutpostStatus {
        target_pid: target,
        outpost_pid: None,
        state: OutpostState::Starting,
    });
}

/// Warms an outpost for Core's own process as early in startup as possible,
/// well before the first gesture can plausibly arrive: Verbatim reading its
/// own GUI is a first-class scenario, and a cold outpost spawn (process
/// creation, `WinEvent` hook install, UIA registration) measurably races a
/// real keypress sent immediately after the popup menu takes foreground —
/// see `Supervisor::ensure_spawned`'s doc comment for the live VM failure
/// this fixes. Does not touch foreground tracking; that happens when the focus
/// listener reports Core's window taking foreground (decision D13), delivered
/// as `OutpostMessage::ForegroundChanged`.
fn warm_own_outpost(
    supervisor: &Arc<Supervisor>,
    outposts: &Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    own_pid: u32,
) {
    note_targeted_pid(outposts, Pid(own_pid));
    supervisor.ensure_spawned(Pid(own_pid));
}

/// The application holding the system foreground right now, read with a
/// local call.
fn foreground_pid() -> Option<Pid> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: reading the current foreground window and its process id.
    let pid = unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        pid
    };
    (pid != 0).then_some(Pid(pid))
}

/// The reducer thread's dependencies, bundled to keep [`reducer_loop`]'s
/// signature from growing an argument for every subsystem it touches.
struct ReducerContext {
    manager: Arc<SpeechManager>,
    supervisor: Arc<Supervisor>,
    ledger: Arc<LatencyLedger>,
    server_slot: Arc<OnceLock<ControlServer>>,
    outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    recorder: SharedRecorder,
}

/// Work handed to the reducer thread by other threads: a reducer input (a
/// review or object-navigation command from the router), or a control-plane
/// tree dump, answered on the given channel.
enum ShellCommand {
    Input(Box<Input>),
    DumpTree(Sender<DumpTreeResult>),
    /// Ask this application for its current focus: at startup, for the
    /// foreground application.
    FocusNow(Pid),
}

/// What the reducer thread owns: the reducer state, the request table, and
/// the live-outpost set. The thread never blocks on a handoff: speech and
/// control-plane broadcasts are channel sends that never wait, a control
/// request is answered on a channel with room for its one answer, and every
/// query gets its outcome through the request table. Writes to an outpost's
/// pipe still happen here until the supervisor gives each outpost a writer
/// thread.
struct ReducerThread<'a> {
    context: &'a ReducerContext,
    state: SrState,
    requests: RequestTable,
    /// Outpost incarnations that have started and not yet ended. A message
    /// from an outpost that has ended is dropped before the reducer sees it.
    live: LiveOutposts,
    /// Applications to ask for their current focus as soon as their outpost
    /// is ready: the startup foreground application, and the attention
    /// application after its outpost or the listener was replaced.
    focus_now_wanted: HashSet<Pid>,
    /// The views last sent to the supervisor.
    views: (Option<Pid>, BTreeSet<OutpostId>),
}

/// How many messages from an outpost may be handled before the nodes held in
/// it are sent again although they have not changed, so that it can release
/// the nodes it reported meanwhile.
const HELD_RESEND_INTERVAL: u64 = 256;

impl ReducerThread<'_> {
    /// Handles one message from the supervisor.
    fn on_outpost_message(&mut self, message: OutpostMessage) {
        match message {
            OutpostMessage::Started {
                outpost,
                target_pid,
            } => {
                self.live.started(outpost, target_pid);
                self.context.outposts.lock().expect("outposts lock").insert(
                    target_pid,
                    OutpostStatus {
                        target_pid,
                        outpost_pid: None,
                        state: OutpostState::Starting,
                    },
                );
            }
            OutpostMessage::Event {
                pid,
                outpost,
                position,
                message,
            } => {
                // An outpost the supervisor has ended can still have messages
                // in flight; they are dropped here, before the reducer. An
                // accepted one is handled from here on: anything the reducer
                // keeps from it is in the held nodes sent after this input.
                if self.live.accept(outpost, position) {
                    self.on_outpost_input(pid, outpost, *message);
                }
            }
            OutpostMessage::Ended {
                outpost,
                target_pid,
                reason,
            } => {
                tracing::info!(%outpost, %target_pid, %reason, "outpost ended");
                // A crashed or killed outpost of the attention application
                // is replaced at once; ask the replacement for the focus,
                // which is taken silently if the user already heard it.
                if reason != EndReason::Retired && self.state.attention() == Some(target_pid) {
                    self.focus_now_wanted.insert(target_pid);
                } else if reason != EndReason::Retired
                    && self.state.focus_source() == Some(target_pid)
                {
                    // The focus's application is not the attention one (a
                    // Settings page's content, inside ApplicationFrameHost's
                    // window): its outpost is replaced only when asked.
                    self.focus_now_wanted.insert(target_pid);
                    self.context.supervisor.ensure_spawned(target_pid);
                }
                if self.live.ended(outpost) {
                    self.context
                        .outposts
                        .lock()
                        .expect("outposts lock")
                        .remove(&target_pid);
                }
                self.apply(Input::OutpostEnded { outpost });
                for input in self.requests.outpost_ended(outpost) {
                    self.apply(input);
                }
            }
            OutpostMessage::ListenerReplaced => {
                // Facts were lost while there was no listener: read the
                // foreground afresh and ask its application for the focus.
                if let Some(pid) = foreground_pid() {
                    self.want_focus_now(pid);
                }
            }
            OutpostMessage::MenuOrSwitchEnded { ended_at_ms } => self.fake_focus(ended_at_ms),
        }
    }

    /// A menu or the Alt+Tab switcher closed at `ended_at_ms`: unless a
    /// focus observed since then has been applied, reads the focus of
    /// whichever application is in front, usually not the one that owned the
    /// menu, as NVDA's fake focus does when no focus event was validly
    /// processed with the end. Only the control is reported, as NVDA queues
    /// a focus on it and nothing for its window. An application with no
    /// ready outpost is asked for its window too, since attention may be
    /// moving to it.
    fn fake_focus(&mut self, ended_at_ms: u64) {
        if self
            .state
            .latest_focus_observed_at()
            .is_some_and(|observed| observed >= ended_at_ms)
        {
            return;
        }
        let Some(pid) = foreground_pid() else {
            return;
        };
        let Some(outpost) = self.live.ready(pid) else {
            self.want_focus_now(pid);
            return;
        };
        let trace_id = TraceId::mint();
        let id = self.requests.begin(
            outpost,
            Asker::FakeFocus {
                source: pid,
                trace_id,
            },
        );
        self.send(
            outpost,
            id,
            SupervisorToOutpost::Query {
                trace_id,
                request_id: id.0,
                query: Query::FocusNow,
            },
        );
    }

    /// Asks `pid`'s outpost for the current focus now if it is ready, or as
    /// soon as it is, starting one if there is none.
    fn want_focus_now(&mut self, pid: Pid) {
        if let Some(outpost) = self.live.ready(pid) {
            self.focus_now(outpost, pid);
        } else {
            self.focus_now_wanted.insert(pid);
            self.context.supervisor.ensure_spawned(pid);
        }
    }

    /// Sends the focus-now query.
    fn focus_now(&mut self, outpost: OutpostId, source: Pid) {
        let trace_id = TraceId::mint();
        let id = self
            .requests
            .begin(outpost, Asker::FocusNow { source, trace_id });
        self.send(
            outpost,
            id,
            SupervisorToOutpost::Query {
                trace_id,
                request_id: id.0,
                query: Query::FocusNow,
            },
        );
    }

    /// Turns one message from a live outpost into reducer input, or routes a
    /// reply to whoever asked.
    fn on_outpost_input(&mut self, source: Pid, outpost: OutpostId, message: OutpostToSupervisor) {
        match message {
            OutpostToSupervisor::Event {
                trace_id,
                observed_at_ms,
                backend,
                window,
                event,
            } => {
                self.context.ledger.event_observed(trace_id, observed_at_ms);
                if let Some(server) = self.context.server_slot.get() {
                    server.broadcast_event(trace_id, source, backend, window, event.clone());
                }
                self.apply(Input::Event {
                    trace_id,
                    observed_at_ms,
                    source,
                    backend,
                    window,
                    event,
                });
            }
            OutpostToSupervisor::Reply {
                request_id,
                outcome,
                ..
            } => self.finish(RequestId(request_id), outpost, outcome),
            OutpostToSupervisor::Ready {
                outpost_pid,
                target_pid,
            } => {
                tracing::info!(%outpost, %outpost_pid, %target_pid, "outpost ready");
                self.live.mark_ready(outpost);
                if self.focus_now_wanted.remove(&target_pid) {
                    self.focus_now(outpost, target_pid);
                }
                self.context.outposts.lock().expect("outposts lock").insert(
                    target_pid,
                    OutpostStatus {
                        target_pid,
                        outpost_pid: Some(outpost_pid),
                        state: OutpostState::Ready,
                    },
                );
            }
            OutpostToSupervisor::Fault { detail } => {
                tracing::warn!(%source, %outpost, detail, "outpost fault");
            }
            // Pong and any future message kinds carry nothing for the reducer.
            _ => {}
        }
    }

    /// Handles work handed over by another thread.
    fn on_command(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::Input(input) => self.apply(*input),
            ShellCommand::DumpTree(reply) => self.dump_tree(reply),
            ShellCommand::FocusNow(pid) => self.want_focus_now(pid),
        }
    }

    /// Delivers a query's outcome through the request table, applying the
    /// reducer input it produces, if any.
    fn finish(&mut self, id: RequestId, outpost: OutpostId, outcome: QueryOutcome) {
        for input in self.requests.finish(id, outpost, outcome) {
            self.apply(input);
        }
    }

    /// Runs one input through the reducer, records it in the flight
    /// recorder, and executes the resulting effects.
    fn apply(&mut self, input: Input) {
        let trace_id = match &input {
            Input::Event { trace_id, .. }
            | Input::FetchCompleted { trace_id, .. }
            | Input::Command { trace_id, .. } => *trace_id,
            _ => TraceId::mint(),
        };
        let (next, effects) = reduce(&self.state, &input);
        self.state = next;
        {
            let mut recorder = self
                .context
                .recorder
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            recorder.record_input(input, effects.len());
        }
        for effect in effects {
            self.execute(trace_id, effect);
        }
        self.send_views();
    }

    /// Sends the views derived from the reducer state, when they change: to
    /// the supervisor, the application holding attention and the outposts in
    /// which the state holds nodes; to each live outpost, the nodes held in
    /// it with the position of its last message handled here (outpost
    /// redesign, "Held objects"). The held nodes are also sent again every
    /// [`HELD_RESEND_INTERVAL`] messages, so an outpost whose held nodes do
    /// not change still releases what it reported meanwhile.
    fn send_views(&mut self) {
        let held = self.state.held_nodes();
        let views = (
            self.state.attention(),
            held.keys().copied().collect::<BTreeSet<_>>(),
        );
        if views != self.views {
            self.context.supervisor.note_views(views.0, views.1.clone());
            self.views = views;
        }
        for (outpost, live) in self.live.iter_mut() {
            let nodes: BTreeSet<u64> = held
                .get(outpost)
                .map(|nodes| nodes.iter().map(|id| id.number()).collect())
                .unwrap_or_default();
            let (sent, acknowledged) = &live.held_sent;
            if nodes != *sent || live.position >= acknowledged + HELD_RESEND_INTERVAL {
                self.context.supervisor.send_nodes_held(
                    *outpost,
                    nodes.iter().copied().collect(),
                    live.position,
                );
                live.held_sent = (nodes, live.position);
            }
        }
    }

    /// Executes one reducer effect.
    fn execute(&mut self, trace_id: TraceId, effect: Effect) {
        match effect {
            Effect::Speak(utterance) => self.context.manager.speak(utterance),
            Effect::StopSpeech => {
                // The reducer interrupts through utterance priority and
                // never emits this; log so a future change is visible.
                tracing::debug!("StopSpeech effect ignored");
            }
            Effect::Fetch(query) => {
                let outpost = query.node_id.outpost();
                let id = self.requests.begin(
                    outpost,
                    Asker::Reducer {
                        query_id: query.query_id,
                        kind: query.kind,
                        trace_id,
                    },
                );
                let command = SupervisorToOutpost::Query {
                    trace_id,
                    request_id: id.0,
                    query: Query::Navigate {
                        node_id: query.node_id,
                        kind: query.kind,
                    },
                };
                self.send(outpost, id, command);
            }
            Effect::Activate { node_id } => {
                let outpost = node_id.outpost();
                let id = self.requests.begin(outpost, Asker::Activation { trace_id });
                let command = SupervisorToOutpost::Query {
                    trace_id,
                    request_id: id.0,
                    query: Query::Activate { node_id },
                };
                self.send(outpost, id, command);
            }
            Effect::CopyToClipboard(text) => clipboard::copy(&self.context.manager, &text),
            _ => {}
        }
    }

    /// Sends request `id` to `outpost`, failing it at once when it cannot be
    /// sent, so it still gets its one outcome.
    fn send(&mut self, outpost: OutpostId, id: RequestId, command: SupervisorToOutpost) {
        if let Err(error) = self.context.supervisor.send_to_outpost(outpost, command) {
            tracing::warn!(%error, %outpost, "a query could not reach its outpost");
            self.finish(id, outpost, QueryOutcome::Failed(error.to_string()));
        }
    }

    /// Starts a control-plane tree dump of the application holding attention.
    /// The answer goes to `reply`; the requester waits on it with its own
    /// timeout, and a late answer to a request it gave up on is dropped.
    fn dump_tree(&mut self, reply: Sender<DumpTreeResult>) {
        let Some(pid) = self.state.attention() else {
            let _ = reply.try_send(Err("no application holds attention yet".to_owned()));
            return;
        };
        let Some(outpost) = self.live.newest(pid) else {
            let _ = reply.try_send(Err(format!("no outpost is watching pid {pid}")));
            return;
        };
        let id = self.requests.begin(outpost, Asker::DumpTree(reply));
        let command = SupervisorToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id: id.0,
            query: Query::DumpTree,
        };
        self.send(outpost, id, command);
    }
}

/// The reducer thread body: drains outpost messages and commands from other
/// threads, feeds the pure reducer, and executes its effects. Selects on
/// both sources so a review or object-navigation gesture is handled with the
/// same reduce-and-execute step as an accessibility event.
fn reducer_loop(
    outpost_rx: &Receiver<OutpostMessage>,
    command_rx: &Receiver<ShellCommand>,
    context: &ReducerContext,
) {
    let mut thread = ReducerThread {
        context,
        state: SrState::new(),
        requests: RequestTable::default(),
        live: LiveOutposts::default(),
        focus_now_wanted: HashSet::new(),
        views: (None, BTreeSet::new()),
    };
    loop {
        crossbeam_channel::select! {
            recv(outpost_rx) -> message => {
                let Ok(message) = message else { break };
                thread.on_outpost_message(message);
            }
            recv(command_rx) -> command => {
                let Ok(command) = command else { break };
                thread.on_command(command);
            }
        }
    }
}

/// The router thread body: bound gestures become imperative commands —
/// GUI commands or direct speech — never reducer inputs.
///
/// Multi-press: the double-press variants (Verbatim+F12 twice quickly
/// speaks the date, Verbatim+F11 twice quickly lists the taskbar) take the
/// press count `verbatim-input` puts on each emitted gesture.
fn router_loop(
    gesture_rx: &Receiver<EmittedGesture>,
    gui_handle: &Arc<OnceLock<GuiHandle>>,
    manager: &Arc<SpeechManager>,
    command_tx: &crossbeam_channel::Sender<ShellCommand>,
    layout: KeyboardLayout,
) {
    let show_menu = GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding");
    // The active layout's gesture-to-script table, looked up per press.
    let scripts: HashMap<GestureId, ScriptAction> =
        verbatim_input::bindings_for(layout).into_iter().collect();
    while let Ok(emitted) = gesture_rx.recv() {
        tracing::info!(trace_id = %emitted.trace_id, gesture = %emitted.gesture, "gesture");
        if emitted.gesture == show_menu {
            send_gui_command(gui_handle, GuiCommand::ShowMenu);
            continue;
        }
        if let Some(key) = verbatim_input::ToggleKey::of_gesture(&emitted.gesture) {
            report_toggle_key(manager, key);
            continue;
        }
        let Some(action) = scripts.get(&emitted.gesture) else {
            continue;
        };
        // Time and the shell list are imperative shell concerns; everything
        // else is a review or object-navigation command for the reducer.
        match action {
            ScriptAction::SpeakTime => speak_time_or_date(manager, emitted.repeat),
            ScriptAction::ShowTrayList => send_gui_command(
                gui_handle,
                GuiCommand::OpenShellItemList(shell_list_kind(emitted.repeat)),
            ),
            other => {
                if let Some(command) = review_command_of(*other) {
                    let input = Input::Command {
                        trace_id: emitted.trace_id,
                        command,
                        repeat: emitted.repeat,
                    };
                    if command_tx
                        .send(ShellCommand::Input(Box::new(input)))
                        .is_err()
                    {
                        tracing::warn!("reducer command channel closed; dropping gesture");
                    }
                }
            }
        }
    }
}

/// Maps a keyboard [`ScriptAction`] to the reducer's [`ReviewCommand`], or
/// `None` for the two actions the router handles itself (time and the tray
/// list). The two enums are deliberately separate — the input crate owns
/// key scripts, the model owns reducer commands — so this is the one place
/// they meet.
fn review_command_of(action: ScriptAction) -> Option<ReviewCommand> {
    Some(match action {
        ScriptAction::ReportCurrentObject => ReviewCommand::ReportObject,
        ScriptAction::MoveToParent => ReviewCommand::Parent,
        ScriptAction::MoveToNextSibling => ReviewCommand::NextSibling,
        ScriptAction::MoveToPreviousSibling => ReviewCommand::PreviousSibling,
        ScriptAction::MoveToFirstChild => ReviewCommand::FirstChild,
        ScriptAction::MoveReviewCursorToFocus => ReviewCommand::ToFocus,
        ScriptAction::ActivateCurrentObject => ReviewCommand::Activate,
        ScriptAction::ReviewTop => ReviewCommand::ReviewTop,
        ScriptAction::ReviewPreviousLine => ReviewCommand::ReviewPreviousLine,
        ScriptAction::ReviewCurrentLine => ReviewCommand::ReviewCurrentLine,
        ScriptAction::ReviewNextLine => ReviewCommand::ReviewNextLine,
        ScriptAction::ReviewPreviousWord => ReviewCommand::ReviewPreviousWord,
        ScriptAction::ReviewCurrentWord => ReviewCommand::ReviewCurrentWord,
        ScriptAction::ReviewNextWord => ReviewCommand::ReviewNextWord,
        ScriptAction::ReviewStartOfLine => ReviewCommand::ReviewStartOfLine,
        ScriptAction::ReviewPreviousCharacter => ReviewCommand::ReviewPreviousCharacter,
        ScriptAction::ReviewCurrentCharacter => ReviewCommand::ReviewCurrentCharacter,
        ScriptAction::ReviewNextCharacter => ReviewCommand::ReviewNextCharacter,
        ScriptAction::ReviewEndOfLine => ReviewCommand::ReviewEndOfLine,
        ScriptAction::ReviewBottom => ReviewCommand::ReviewBottom,
        // `SpeakTime` and `ShowTrayList` are handled by the router itself
        // and never reach here; `ScriptAction` is also non-exhaustive, so an
        // unmapped future action is simply not routed to the reducer until
        // it is given a command.
        _ => return None,
    })
}

/// The keyboard bindings this milestone ships, as the shared gesture map the
/// hook consults and the control plane validates against: the menu gesture
/// plus every review and object-navigation binding for the active layout
/// (roadmap M3). The layout's own table already carries the time and tray
/// gestures, so they are not listed separately.
fn bound_gestures(layout: KeyboardLayout) -> SharedGestureMap {
    let mut gestures = vec![GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding")];
    gestures.extend(
        verbatim_input::bindings_for(layout)
            .into_iter()
            .map(|(gesture, _action)| gesture),
    );
    GestureMap::new(gestures).into_shared()
}

/// Sends one command to the GUI when it is up; during the startup window
/// before `run_gui`'s ready callback fires, the command is dropped with a
/// warning (there is no GUI to act on it yet).
fn send_gui_command(gui_handle: &Arc<OnceLock<GuiHandle>>, command: GuiCommand) {
    if let Some(handle) = gui_handle.get() {
        handle.send(command);
    } else {
        tracing::warn!(?command, "gesture before the GUI was ready; dropped");
    }
}

/// Speaks the localized current time (`repeat` 0) or date (any higher
/// count) at Interrupt priority, as a plain text span with no source node.
/// `repeat` is the number of extra quick presses — the multi-press seam
/// described on [`router_loop`]; today it always arrives as 0.
/// Announces a lock key's new state ("caps lock on") 30 milliseconds after
/// it reached the operating system, as NVDA does: Windows has changed the
/// state by then.
fn report_toggle_key(manager: &Arc<SpeechManager>, key: verbatim_input::ToggleKey) {
    let manager = Arc::clone(manager);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        // SAFETY: GetKeyState takes a virtual-key code and reads key state.
        let on = unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(key.vk()))
        } & 1
            != 0;
        let id = match key {
            verbatim_input::ToggleKey::CapsLock => "toggle-caps-lock",
            verbatim_input::ToggleKey::NumLock => "toggle-num-lock",
            verbatim_input::ToggleKey::ScrollLock => "toggle-scroll-lock",
        };
        manager.speak(Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Interrupt,
            segments: vec![UtteranceSegment::text(
                verbatim_i18n::messages::toggle_key_state(id, on),
            )],
            source: None,
        });
    });
}

fn speak_time_or_date(manager: &SpeechManager, repeat: u8) {
    let formatted = if repeat == 0 {
        datetime::local_time()
    } else {
        datetime::local_date()
    };
    let Some(text) = formatted else {
        tracing::warn!(repeat, "the system time or date could not be formatted");
        return;
    };
    manager.speak(Utterance {
        trace_id: TraceId::mint(),
        priority: SpeechPriority::Interrupt,
        segments: vec![UtteranceSegment::text(text)],
        source: None,
    });
}

/// The shell surface Verbatim+F11 lists: the system tray on a single press
/// (`repeat` 0), the taskbar on a double press — the same multi-press seam
/// as [`speak_time_or_date`]. Pure, and unit tested below.
fn shell_list_kind(repeat: u8) -> ShellItemKind {
    if repeat == 0 {
        ShellItemKind::SystemTray
    } else {
        ShellItemKind::Taskbar
    }
}

/// The GUI event thread body: the Exit item asks the app to shut down; the
/// app tells the GUI to tear down, which ends the main thread's loop.
fn gui_event_loop(gui_event_rx: &Receiver<GuiEvent>, gui_handle: &Arc<OnceLock<GuiHandle>>) {
    while let Ok(event) = gui_event_rx.recv() {
        let GuiEvent::QuitRequested = event;
        tracing::info!("quit requested");
        request_shutdown(gui_handle);
    }
}

/// Asks the GUI loop to end, which unwinds `main`; exits directly if the GUI
/// never came up.
fn request_shutdown(gui_handle: &Arc<OnceLock<GuiHandle>>) {
    match gui_handle.get() {
        Some(handle) => handle.send(GuiCommand::Shutdown),
        None => std::process::exit(0),
    }
}

/// The app's live pieces [`control_handlers`] wires into [`ServerHandlers`];
/// grouped into one struct because the control plane needs a handle to
/// nearly every subsystem.
struct ControlHandlersConfig {
    own_pid: u32,
    settings_host: verbatim_speech::SettingsHost,
    outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    ledger: Arc<LatencyLedger>,
    bound_gestures: SharedGestureMap,
    gesture_tx: crossbeam_channel::Sender<EmittedGesture>,
    gui_handle: Arc<OnceLock<GuiHandle>>,
    command_tx: Sender<ShellCommand>,
    recorder: SharedRecorder,
    dumps_dir: PathBuf,
}

/// Builds the control-plane handlers over the app's live pieces.
fn control_handlers(config: ControlHandlersConfig) -> ServerHandlers {
    let ControlHandlersConfig {
        own_pid,
        settings_host,
        outposts,
        ledger,
        bound_gestures,
        gesture_tx,
        gui_handle,
        command_tx,
        recorder,
        dumps_dir,
    } = config;
    ServerHandlers {
        status: Box::new(move || StatusInfo {
            pid: Pid(own_pid),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            active_synth: Some(settings_host.active_synthesizer().id.0),
            outposts: outposts
                .lock()
                .expect("outposts lock")
                .values()
                .cloned()
                .collect(),
        }),
        send_gesture: Box::new(move |identifier| {
            let gesture = GestureId::parse(identifier).map_err(|error| error.to_string())?;
            if !bound_gestures.load().contains(&gesture) {
                return Err(format!("gesture {gesture} is not bound"));
            }
            gesture_tx
                .send(EmittedGesture {
                    trace_id: TraceId::mint(),
                    gesture,
                    // An injected gesture is always a single, first press;
                    // multi-press counting applies to real key streams in
                    // the decision machine, not control-plane injection.
                    repeat: 0,
                })
                .map_err(|_| "the gesture router is gone".to_owned())
        }),
        latency: Box::new(move |last_n| ledger.recent(last_n)),
        dump_tree: Box::new(move || request_dump_tree(&command_tx)),
        dump_recorder: Box::new(move || {
            flight_dump::dump_now(&recorder, &dumps_dir)
                .map(|path| path.display().to_string())
                .map_err(|error| error.to_string())
        }),
        quit: Box::new(move || request_shutdown(&gui_handle)),
    }
}

/// Answers [`Request::DumpTree`](verbatim_control::protocol::Request::DumpTree):
/// hands the request to the reducer thread, which sends `DumpTree` to the
/// outpost of the application holding attention and records it in its
/// request table, then waits for the answer with a timeout. Every request
/// has its own id, so a late answer to one that timed out can never satisfy
/// a newer one.
fn request_dump_tree(command_tx: &Sender<ShellCommand>) -> DumpTreeResult {
    let (reply_tx, reply_rx) = bounded(1);
    command_tx
        .send(ShellCommand::DumpTree(reply_tx))
        .map_err(|_| "the reducer thread is gone".to_owned())?;
    reply_rx
        .recv_timeout(DUMP_TREE_TIMEOUT)
        .unwrap_or_else(|_| Err("tree dump timed out".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_press_lists_the_system_tray() {
        assert_eq!(shell_list_kind(0), ShellItemKind::SystemTray);
    }

    #[test]
    fn a_repeated_press_lists_the_taskbar() {
        // The multi-press seam: today the router always passes 0; once
        // verbatim-input's press counting is wired, any repeat selects the
        // taskbar list.
        assert_eq!(shell_list_kind(1), ShellItemKind::Taskbar);
        assert_eq!(shell_list_kind(3), ShellItemKind::Taskbar);
    }
}
