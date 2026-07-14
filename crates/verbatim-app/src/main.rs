//! `verbatim.exe` — the composition root (architecture section 1).
//!
//! Startup order: namespace trace IDs, load config, start tracing, replace
//! any running instance, load locales, bring up the speech pipeline, the
//! supervisor and foreground trigger, the reducer and router threads, the
//! control plane, and the keyboard hook — then run the wxDragon GUI loop on
//! this, the process main thread, until shutdown is requested from the menu,
//! the control plane, or a replacing instance.

mod flight_dump;
mod latency;
mod single_instance;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use verbatim_audio::{AudioSink, NullSink, WasapiSink};
use verbatim_config::{ConfigStore, ConfigValue};
use verbatim_control::protocol::{OutpostState, OutpostStatus, StatusInfo};
use verbatim_control::server::{ControlServer, ServerHandlers};
use verbatim_core::{ReducerRecorder, SrState, reduce};
use verbatim_gui::{GuiCommand, GuiEvent, GuiHandle, run_gui};
use verbatim_input::{DecisionConfig, EmittedGesture, GestureMap, InputHook, SharedGestureMap};
use verbatim_model::{
    Effect, GestureId, Input, Pid, SpeechPriority, TraceId, TreeNode, Utterance, UtteranceSegment,
};
use verbatim_outpost::protocol::{OutpostToSupervisor, SupervisorToOutpost};
use verbatim_outpost::{ForegroundTrigger, OutpostMessage, Supervisor};
use verbatim_speech::{
    SettingId, SettingValue, SpeechManager, SpeechManagerConfig, SpeechSettingsHost, SynthId,
    SynthRegistry,
};
use verbatim_synth_capture::CaptureSynth;

use latency::LatencyLedger;

/// The one keyboard binding milestone M1 ships: Verbatim+V opens the menu.
const SHOW_MENU_GESTURE: &str = "kb:verbatim+v";

/// How long a `DumpTree` control-plane request waits for the outpost's
/// answer before giving up.
const DUMP_TREE_TIMEOUT: Duration = Duration::from_secs(5);

/// The one-shot reply channel for an in-flight `DumpTree` request.
type DumpTreeReplySender = Sender<Result<(TreeNode, bool), String>>;

/// A slot for at most one in-flight `DumpTree` request at a time: the
/// control-plane handler registers a sender here and waits on its
/// receiver; `reducer_loop` routes the outpost's `DumpTreeReply` into it
/// instead of the reducer, since a tree dump is a one-shot diagnostic
/// query, not reducer-shaped input. A second concurrent request finds the
/// slot occupied and fails immediately rather than queuing.
type PendingDumpTree = Arc<Mutex<Option<DumpTreeReplySender>>>;

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
fn run(config: ConfigStore) -> Result<(), Box<dyn std::error::Error>> {
    let own_pid = std::process::id();

    // The control server is created late (it needs the other pieces'
    // handlers), but earlier pieces need to reach it for broadcasting.
    let server_slot: Arc<OnceLock<ControlServer>> = Arc::new(OnceLock::new());
    let ledger = Arc::new(LatencyLedger::new(256, Arc::clone(&server_slot)));
    let pending_dump_tree: PendingDumpTree = Arc::new(Mutex::new(None));

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

    // Settings host: the GUI's live handle; commit persists to the base
    // profile through the config store.
    let store = Arc::new(Mutex::new(config));
    let settings_host = manager.settings_host(persist_fn(Arc::clone(&store)));

    // Supervisor plus the foreground trigger that retargets the single M1
    // outpost; outpost status is mirrored for the control plane.
    let (outpost_tx, outpost_rx) = unbounded::<OutpostMessage>();
    let supervisor = Arc::new(Supervisor::new(outpost_tx)?);
    let outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>> = Arc::new(Mutex::new(HashMap::new()));
    let trigger = {
        let supervisor = Arc::clone(&supervisor);
        let outposts = Arc::clone(&outposts);
        ForegroundTrigger::new(Arc::new(move |pid, _hwnd| {
            if pid == 0 {
                return;
            }
            mark_outpost_starting(&outposts, Pid(pid));
            if let Err(error) = supervisor.target(Pid(pid)) {
                tracing::warn!(%error, pid, "failed to target foreground application");
            }
        }))
    };

    // The reducer thread: normalized events in, speech and fetches out.
    {
        let context = ReducerContext {
            manager: Arc::clone(&manager),
            supervisor: Arc::clone(&supervisor),
            ledger: Arc::clone(&ledger),
            server_slot: Arc::clone(&server_slot),
            outposts: Arc::clone(&outposts),
            pending_dump_tree: Arc::clone(&pending_dump_tree),
            recorder: Arc::clone(&recorder),
        };
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || reducer_loop(&outpost_rx, &context))?;
    }

    // The gesture router: bound gestures to imperative commands, never into
    // the reducer. The GUI handle arrives once the GUI thread is up.
    let gui_handle: Arc<OnceLock<GuiHandle>> = Arc::new(OnceLock::new());
    let (gesture_tx, gesture_rx) = bounded::<EmittedGesture>(64);
    {
        let gui_handle = Arc::clone(&gui_handle);
        thread::Builder::new()
            .name("verbatim-router".to_owned())
            .spawn(move || router_loop(&gesture_rx, &gui_handle))?;
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
    let bound_gestures: SharedGestureMap =
        GestureMap::new([GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding")])
            .into_shared();

    // Control plane.
    let server = ControlServer::start(control_handlers(ControlHandlersConfig {
        own_pid,
        settings_host: settings_host.clone(),
        outposts: Arc::clone(&outposts),
        ledger: Arc::clone(&ledger),
        bound_gestures: Arc::clone(&bound_gestures),
        gesture_tx: gesture_tx.clone(),
        gui_handle: Arc::clone(&gui_handle),
        supervisor: Arc::clone(&supervisor),
        pending_dump_tree: Arc::clone(&pending_dump_tree),
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
    });
    target_current_foreground(&supervisor, &outposts);

    // The GUI loop owns the main thread until shutdown.
    let host_for_gui: Arc<dyn SpeechSettingsHost> = Arc::new(settings_host);
    let handle_slot = Arc::clone(&gui_handle);
    run_gui(host_for_gui, gui_event_tx, move |handle| {
        let _ = handle_slot.set(handle);
    })?;

    // The wx loop has exited (Exit menu item, control-plane quit, or a
    // replacing instance's WM_QUIT). Stop the input and foreground hooks
    // explicitly; job objects kill the outposts when the process exits.
    drop(trigger);
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
/// test-only escape hatch (documented in docs/overview.md) that swaps in
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

/// Marks an outpost as starting in the status mirror; M1 runs one outpost,
/// so any previous entry is replaced.
fn mark_outpost_starting(outposts: &Arc<Mutex<HashMap<Pid, OutpostStatus>>>, target: Pid) {
    let mut outposts = outposts.lock().expect("outposts lock");
    outposts.clear();
    outposts.insert(
        target,
        OutpostStatus {
            target_pid: target,
            outpost_pid: None,
            state: OutpostState::Starting,
        },
    );
}

/// Targets whatever is in the foreground right now, so the first outpost
/// exists before the first foreground *change*.
fn target_current_foreground(
    supervisor: &Arc<Supervisor>,
    outposts: &Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
) {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: reading the current foreground window and its process id.
    let pid = unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&raw mut pid));
        pid
    };
    if pid == 0 {
        return;
    }
    mark_outpost_starting(outposts, Pid(pid));
    if let Err(error) = supervisor.target(Pid(pid)) {
        tracing::warn!(%error, pid, "failed to target the initial foreground application");
    }
}

/// The reducer thread's dependencies, bundled to keep [`reducer_loop`]'s
/// signature from growing an argument for every subsystem it touches.
struct ReducerContext {
    manager: Arc<SpeechManager>,
    supervisor: Arc<Supervisor>,
    ledger: Arc<LatencyLedger>,
    server_slot: Arc<OnceLock<ControlServer>>,
    outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    pending_dump_tree: PendingDumpTree,
    recorder: SharedRecorder,
}

/// Turns one message from an outpost into a reducer [`Input`], handling the
/// side effects (status mirroring, event broadcast, routing a `DumpTree`
/// reply into its one-shot slot) that happen either way. `None` for message
/// kinds that carry nothing for the reducer: `Ready`, `Fault`,
/// `DumpTreeReply`, `Pong`, and any future kind.
fn incoming_input(
    source: Pid,
    message: OutpostToSupervisor,
    ledger: &LatencyLedger,
    server_slot: &OnceLock<ControlServer>,
    outposts: &Mutex<HashMap<Pid, OutpostStatus>>,
    pending_dump_tree: &PendingDumpTree,
) -> Option<Input> {
    match message {
        OutpostToSupervisor::Event {
            trace_id,
            observed_at_ms,
            backend,
            version,
            event,
        } => {
            ledger.event_observed(trace_id, observed_at_ms);
            if let Some(server) = server_slot.get() {
                server.broadcast_event(trace_id, source, backend, version, event.clone());
            }
            Some(Input::Event {
                trace_id,
                source,
                backend,
                version,
                event,
            })
        }
        OutpostToSupervisor::FetchReply {
            trace_id,
            query_id,
            result,
        } => Some(Input::FetchCompleted {
            trace_id,
            query_id,
            result,
        }),
        OutpostToSupervisor::Ready {
            outpost_pid,
            target_pid,
        } => {
            tracing::info!(%outpost_pid, %target_pid, "outpost ready");
            let mut outposts = outposts.lock().expect("outposts lock");
            // M1 runs one outpost; a Ready for a retargeted outpost
            // replaces any entry for its previous target.
            outposts.retain(|_, status| status.outpost_pid != Some(outpost_pid));
            outposts.insert(
                target_pid,
                OutpostStatus {
                    target_pid,
                    outpost_pid: Some(outpost_pid),
                    state: OutpostState::Ready,
                },
            );
            None
        }
        OutpostToSupervisor::Fault { detail } => {
            tracing::warn!(%source, detail, "outpost fault");
            None
        }
        OutpostToSupervisor::DumpTreeReply { result, .. } => {
            // A tree dump is a one-shot diagnostic query, not reducer-shaped
            // input: route it straight into the pending control-plane
            // request's reply slot instead.
            let sender = pending_dump_tree
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            match sender {
                Some(sender) => {
                    let _ = sender.send(result.map(|dumped| (dumped.root, dumped.truncated)));
                }
                None => {
                    tracing::warn!("received a DumpTree reply with no pending request; dropped");
                }
            }
            None
        }
        // Pong and any future message kinds carry nothing for the reducer.
        _ => None,
    }
}

/// The reducer thread body: drains outpost messages, feeds the pure reducer,
/// and executes its effects.
fn reducer_loop(outpost_rx: &Receiver<OutpostMessage>, context: &ReducerContext) {
    let ReducerContext {
        manager,
        supervisor,
        ledger,
        server_slot,
        outposts,
        pending_dump_tree,
        recorder,
    } = context;
    let mut state = SrState::new();

    while let Ok((source, message)) = outpost_rx.recv() {
        let Some(input) = incoming_input(
            source,
            message,
            ledger,
            server_slot,
            outposts,
            pending_dump_tree,
        ) else {
            continue;
        };

        let trace_id = match &input {
            Input::Event { trace_id, .. } | Input::FetchCompleted { trace_id, .. } => *trace_id,
            _ => TraceId::mint(),
        };
        let (next, effects) = reduce(&state, &input);
        {
            let mut recorder = recorder.lock().unwrap_or_else(PoisonError::into_inner);
            recorder.record_input(input, effects.len());
        }
        state = next;

        for effect in effects {
            match effect {
                Effect::Speak(utterance) => manager.speak(utterance),
                Effect::StopSpeech => {
                    // M1's reducer interrupts through utterance priority and
                    // never emits this; log so a future change is visible.
                    tracing::debug!("StopSpeech effect ignored in M1");
                }
                Effect::Fetch(query) => {
                    if let Err(error) =
                        supervisor.send(&SupervisorToOutpost::Fetch { trace_id, query })
                    {
                        tracing::warn!(%error, "fetch could not reach the outpost");
                    }
                }
                _ => {}
            }
        }
    }
}

/// The router thread body: bound gestures become imperative commands sent
/// straight to the GUI — never reducer inputs.
fn router_loop(gesture_rx: &Receiver<EmittedGesture>, gui_handle: &Arc<OnceLock<GuiHandle>>) {
    let show_menu = GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding");
    while let Ok(emitted) = gesture_rx.recv() {
        tracing::info!(trace_id = %emitted.trace_id, gesture = %emitted.gesture, "gesture");
        if emitted.gesture == show_menu {
            if let Some(handle) = gui_handle.get() {
                handle.send(GuiCommand::ShowMenu);
            } else {
                tracing::warn!("gesture before the GUI was ready; dropped");
            }
        }
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
    supervisor: Arc<Supervisor>,
    pending_dump_tree: PendingDumpTree,
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
        supervisor,
        pending_dump_tree,
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
                })
                .map_err(|_| "the gesture router is gone".to_owned())
        }),
        latency: Box::new(move |last_n| ledger.recent(last_n)),
        dump_tree: Box::new(move || request_dump_tree(&supervisor, &pending_dump_tree)),
        dump_recorder: Box::new(move || {
            flight_dump::dump_now(&recorder, &dumps_dir)
                .map(|path| path.display().to_string())
                .map_err(|error| error.to_string())
        }),
        quit: Box::new(move || request_shutdown(&gui_handle)),
    }
}

/// Answers [`Request::DumpTree`](verbatim_control::protocol::Request::DumpTree):
/// registers a one-shot reply sender in `pending_dump_tree`, sends
/// `DumpTree` to the outpost through `supervisor`, and waits with a
/// timeout. `reducer_loop` routes the outpost's answer into the slot. A
/// second concurrent request while one is already pending is rejected
/// immediately rather than queued.
fn request_dump_tree(
    supervisor: &Arc<Supervisor>,
    pending_dump_tree: &PendingDumpTree,
) -> Result<(TreeNode, bool), String> {
    let (reply_tx, reply_rx) = bounded(1);
    {
        let mut slot = pending_dump_tree
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err("a tree dump is already in progress".to_owned());
        }
        *slot = Some(reply_tx);
    }

    let trace_id = TraceId::mint();
    if let Err(error) = supervisor.send(&SupervisorToOutpost::DumpTree { trace_id }) {
        *pending_dump_tree
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        return Err(format!("could not reach the outpost: {error}"));
    }

    if let Ok(result) = reply_rx.recv_timeout(DUMP_TREE_TIMEOUT) {
        return result;
    }
    *pending_dump_tree
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    Err("tree dump timed out".to_owned())
}
