//! `verbatim.exe` — the composition root (architecture section 1).
//!
//! Startup order: refuse to run outside an interactive session, namespace
//! trace IDs, load config, start tracing, replace any running instance, load locales, bring up the speech pipeline, the
//! supervisor and its focus listener (decision D13), the reducer thread, the
//! keyboard hook, and the control plane — then run the wxWidgets
//! GUI loop on this, the process main thread, until shutdown is requested from
//! the menu, the control plane, or a replacing instance.

mod clipboard;
mod datetime;
mod error_sound;
mod flight_dump;
mod harness;
mod held_keys;
mod idle;
mod key_reports;
mod latency;
mod live;
mod requests;
mod screen_reader_flag;
mod shell_requests;
mod single_instance;
mod speech_events;
mod terminal_settings;
mod themes;

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use verbatim_audio::{AudioDevice, Mixer, SilentDevice, WavRecorder};
use verbatim_audio_wasapi::WasapiDevice;
use verbatim_config::{ConfigStore, ConfigValue};
use verbatim_control::protocol::{FocusReport, OutpostState, OutpostStatus, StatusInfo};
use verbatim_control::server::{ControlServer, ServerHandlers};
use verbatim_core::{ReducerRecorder, SrState, reduce};
use verbatim_gui::{GuiCommand, GuiEvent, GuiHandle, ShellItemKind, run_gui};
use verbatim_input::{DecisionConfig, GestureMap, KeyboardLayout, SharedGestureMap};
use verbatim_input_windows::{ForegroundView, Injected, Injector, InputHook, KeyHolding};
use verbatim_model::protocol::{CoreToOutpost, EventTiming, OutpostToCore, Query, QueryOutcome};
use verbatim_model::{
    Earcon, Effect, ForegroundWindow, GestureId, GuiRequest, Input, LockKey, OutpostId, Pid,
    ReaderSettings, TextRequest, TraceId, WindowHandle,
};
use verbatim_outpost::supervisor::{EndReason, Frames};
use verbatim_outpost::{OutpostMessage, OutpostOptions, Supervisor};
use verbatim_process::port_queue::{PortQueue, Poster, Received, port_queue};
use verbatim_speech::hosting::synth_ids;
use verbatim_speech::{
    QueueItem, SettingId, SettingValue, SpeakToken, SpeechInbox, SpeechInboxTaker, SpeechManager,
    SpeechManagerConfig, SpeechSettingsHost, SynthId, SynthRegistry,
};
use verbatim_synth_capture::CaptureSynth;

use latency::LatencyLedger;
use live::LiveOutposts;
use requests::{Asker, DumpTicket, DumpTreeResult, RequestId, RequestTable};

/// The one binding not carried by the keyboard layout's own script table:
/// Verbatim+V opens the menu. The review, object-navigation, time, and
/// tray-list bindings all come from `verbatim_input::bindings_for` for the
/// active layout (roadmap M3).
pub(crate) const SHOW_MENU_GESTURE: &str = "kb:verbatim+v";

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

/// Writes a line to standard error, as `eprintln!` does, but ignores a
/// failed write instead of panicking: a full disk or a closed pipe under
/// redirected output must not take the process down.
macro_rules! report_error {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

fn main() -> ExitCode {
    // SAFETY: first thing in `main`, before any thread starts: every
    // process Verbatim launches reads the same clock.
    unsafe { verbatim_process::clock::hand_on() };
    // Keep Core's trace IDs disjoint from every outpost's; they meet in the
    // latency ledger and the flight recorder.
    TraceId::namespace(std::process::id());

    let exe_dir = exe_dir();
    let config = load_config(&exe_dir);
    init_tracing(config.settings().log_filter.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "verbatim starting");

    // Checked before replacing a running instance, so a launch that cannot
    // work never stops one that does.
    if let Err(diagnosis) = check_interactive_session() {
        tracing::error!(diagnosis, "verbatim cannot run in this session");
        report_error!("verbatim: {diagnosis}");
        return ExitCode::FAILURE;
    }

    // Locales come first: a running instance that cannot be ended is
    // reported in a message box.
    load_locales(&exe_dir, &config);

    // Replace a running instance before creating anything it might still own.
    let instance = match single_instance::acquire_replacing() {
        Ok(guard) => guard,
        Err(error) => {
            tracing::error!(%error, "single-instance startup failed");
            report_error!("verbatim: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Windows' screen reader flag, set while this instance runs and cleared
    // as it goes, before the startup mutex is released, so an instance
    // replacing this one sets it again after it is cleared.
    let _screen_reader = screen_reader_flag::ScreenReaderFlag::raise();

    match run(config, &instance) {
        Ok(()) => {
            tracing::info!("verbatim exiting");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "verbatim failed to start");
            report_error!("verbatim: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Refuses a session no screen reader can work in: one whose window station
/// is not interactive, which is where `WinRM`, PowerShell Direct, and
/// services start processes (the "session 0" problem). There is no input
/// desktop there to read or speak for, and without this check the launch
/// would fail later in ways that do not name the cause. A locked or secure
/// input desktop is not refused: the session is still the user's.
fn check_interactive_session() -> Result<(), String> {
    let session = verbatim_process::session::current()
        .map_err(|error| format!("could not read this process's session: {error}"))?;
    if session.interactive_window_station {
        Ok(())
    } else {
        Err(format!(
            "this process's window station is not interactive (session {}), so no screen reader can work here. It was probably started from a non-interactive context such as WinRM, PowerShell Direct, or a service, rather than from the signed-in user's desktop.",
            session.id
        ))
    }
}

/// Everything after the process-level preliminaries; errors here are startup
/// failures reported to the user.
#[allow(
    clippy::too_many_lines,
    reason = "the composition root wires every subsystem together in one place; splitting it would scatter the startup order this function exists to make legible"
)]
fn run(
    config: ConfigStore,
    instance: &single_instance::InstanceGuard,
) -> Result<(), Box<dyn std::error::Error>> {
    let own_pid = std::process::id();

    // The control server is created late (it needs the other pieces'
    // handlers), but earlier pieces need to reach it for broadcasting.
    let server_slot: Arc<OnceLock<ControlServer>> = Arc::new(OnceLock::new());
    let ledger = Arc::new(LatencyLedger::new(
        LatencyLedger::DEFAULT_CAPACITY,
        LatencyLedger::DEFAULT_MAX_BYTES,
        Arc::clone(&server_slot),
    ));

    // The flight recorder, and its panic-time dump trigger: installed as
    // early as possible, chaining the previous hook, so a panic on any
    // thread from here on writes a dump before the process dies.
    let recorder: SharedRecorder = Arc::new(Mutex::new(ReducerRecorder::with_default_bounds(
        SrState::new(),
    )));
    let dumps_dir = exe_dir().join(DUMPS_FOLDER);
    flight_dump::install_panic_hook(Arc::clone(&recorder), dumps_dir.clone());

    // Every child process (the synthesizer host below, the outposts and the
    // listener later) logs into this launch's directory, prepared first.
    verbatim_process::prepare_launch_logs(&exe_dir());

    // The keyboard layout selects which review and object-navigation
    // bindings are active (roadmap M3); read it before `config` moves into
    // the store. File-only in M3, so no live re-read is needed. The config
    // and input crates each own a `KeyboardLayout` (deliberately decoupled),
    // so translate here at the seam.
    let keyboard_layout = match config.settings().keyboard.layout {
        verbatim_config::KeyboardLayout::Desktop => KeyboardLayout::Desktop,
        verbatim_config::KeyboardLayout::Laptop => KeyboardLayout::Laptop,
    };
    // How every outpost reads UIA, fixed for their lives: a developer
    // setting, read once like the layout.
    let outpost_options = OutpostOptions {
        remote_operations: config.settings().uia.remote_operations,
    };

    // The reader settings the reducer reads, handed to it first.
    let reader_settings = config.settings().reader;

    // The reducer thread's port: every outpost's frames and the
    // supervisor's notices (through the supervisor's sink, below), the
    // keys, gestures and typing the keyboard hook reports, in the order it
    // decided them, the index marks speech reaches, GUI events
    // and control-plane commands, each one packet posted to it. The port is
    // first in, first out across all its posters, so the reducer takes its
    // inputs in the order they reached it, as NVDA queues keys with events
    // (`docs/design/communication.md`, section 4.5). A post never waits.
    let (command_tx, command_rx) = port_queue::<ShellCommand>()?;

    // Speech pipeline: eSpeak NG through WASAPI by default, observed by the
    // latency ledger and reporting the marks it reaches to the reducer;
    // VERBATIM_TEST_AUDIO=null swaps in device-free test audio (see
    // build_speech_manager). It reads each synthesizer's saved settings from
    // the store whenever it starts one.
    let store = Arc::new(Mutex::new(config));
    let speech_events = Arc::new(speech_events::ShellSpeechEvents {
        ledger: Arc::clone(&ledger),
        commands: command_tx.clone(),
    });
    // The theme the configuration names, with its sounds, presents speech
    // from the first word and decides what the reducer fetches; the theme
    // panel switches it.
    let theme = {
        let store = store.lock().unwrap_or_else(PoisonError::into_inner);
        let (loaded, options) = themes::configured(&store);
        themes::prepare(&loaded, &store.sounds_dir(), options)
    };
    themes::send_fetches(&command_tx, &theme);
    let (manager, speak_token) = build_speech_manager(&store, speech_events, theme)?;
    // Errors logged from here on play the error sound, through the reducer
    // thread.
    error_sound::report_to(command_tx.clone());

    // Settings host: the GUI's live handle; commit persists to the base
    // profile through the config store.
    let settings_host = manager.settings_host(persist_fn(Arc::clone(&store)));

    // Supervisor (one outpost process per application, decision D9) plus its
    // dedicated focus listener (decision D13), which detects focus and
    // foreground changes desktop-wide and reports them as facts the supervisor
    // routes; outpost status is mirrored for the control plane. Core no longer
    // runs its own foreground hook — the listener absorbs it, and a foreground
    // change reaches the reducer as a focus on the window, which moves its
    // attention.
    // The processes the end-to-end harness names in `VERBATIM_IGNORE_PIDS`
    // (the owner's own Windows Terminal), ignored entirely: nothing is ever
    // read from them. Unset for real users, so nothing is ignored.
    let ignored = Arc::new(verbatim_outpost::supervisor::IgnoredProcesses::from_env());
    let supervisor = Arc::new(Supervisor::new(
        outpost_sink(command_tx.clone()),
        outpost_options,
        ignored,
    )?);
    let outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>> = Arc::new(Mutex::new(HashMap::new()));
    // Set once the focus listener first reports ready; part of the
    // readiness the control plane's status reports.
    let listener_ready = Arc::new(AtomicBool::new(false));
    let focus_known = Arc::new(AtomicBool::new(false));
    // The GUI's handle, once the GUI thread is up; part of readiness.
    let gui_handle: Arc<OnceLock<GuiHandle>> = Arc::new(OnceLock::new());
    let readiness = Arc::new(harness::Readiness::new(
        own_pid,
        Arc::clone(&gui_handle),
        Arc::clone(&listener_ready),
        Arc::clone(&focus_known),
        Arc::clone(&outposts),
    ));
    let idle_line = Arc::new(harness::IdleLine::default());

    warm_own_outpost(&supervisor, &outposts, own_pid);

    // Which caret keys the keyboard hook holds back while the focus is
    // text, switched by the reducer thread from Core's focus
    // (`docs/design/key-holding.md`).
    let holding = Arc::new(KeyHolding::new(
        verbatim_input::caret_bindings()
            .into_iter()
            .map(|(gesture, _)| gesture),
    ));

    // Core's view of the window in front, for the keyboard hook: whether
    // it may hold keys there, and which thread's layout translates them,
    // published by the reducer thread (architecture section 5).
    let foreground = Arc::new(ForegroundView::new());

    // The reducer thread: normalized events and review commands in, speech,
    // fetches, text requests, activations, and clipboard copies out.
    {
        let context = ReducerContext {
            holding: Arc::clone(&holding),
            foreground: Arc::clone(&foreground),
            clipboard: clipboard::Clipboard::start(command_tx.clone())?,
            manager: Arc::clone(&manager),
            supervisor: Arc::clone(&supervisor),
            ledger: Arc::clone(&ledger),
            server_slot: Arc::clone(&server_slot),
            outposts: Arc::clone(&outposts),
            recorder: Arc::clone(&recorder),
            listener_ready: Arc::clone(&listener_ready),
            focus_known: Arc::clone(&focus_known),
            store: Arc::clone(&store),
            readiness: Arc::clone(&readiness),
            idle_line: Arc::clone(&idle_line),
            gui_handle: Arc::clone(&gui_handle),
        };
        // The only thread that speaks: it holds the speaking token, which
        // the loop hands back when it ends.
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || {
                let _token = reducer_loop(command_rx, &context, reader_settings, speak_token);
            })?;
    }

    // GUI events out of the GUI thread: Exit requests shutdown.
    let (gui_event_tx, gui_event_rx) = unbounded::<GuiEvent>();
    {
        let gui_handle = Arc::clone(&gui_handle);
        let command_tx = command_tx.clone();
        thread::Builder::new()
            .name("verbatim-gui-events".to_owned())
            .spawn(move || gui_event_loop(&gui_event_rx, &gui_handle, &command_tx))?;
    }

    // The gesture map shared by the hook and the control plane's validation.
    let bound_gestures = bound_gestures(keyboard_layout);

    // Keyboard hook. Everything it decides goes to the reducer's port, and
    // a key that cancels speech to the speech queue's, both made before
    // it, through the callbacks in `key_reports`, in the order the keys
    // were pressed; a key that cancels speech also tells the reducer,
    // whose say-all stops. Started before the control plane, whose
    // injected gestures and idle waiters go through it.
    let hook = InputHook::start(
        decision_config(&store),
        Arc::clone(&bound_gestures),
        key_reports::speech_effect(manager.control(), command_tx.clone()),
        key_reports::reports(
            key_reports::Tables::for_layout(keyboard_layout),
            Arc::clone(&ledger),
            command_tx.clone(),
        ),
        holding,
        foreground,
    )?;

    // Control plane.
    let server = ControlServer::start(control_handlers(ControlHandlersConfig {
        own_pid,
        settings_host: settings_host.clone(),
        outposts: Arc::clone(&outposts),
        ledger: Arc::clone(&ledger),
        bound_gestures: Arc::clone(&bound_gestures),
        injector: hook.injector(),
        gui_handle: Arc::clone(&gui_handle),
        command_tx: command_tx.clone(),
        recorder: Arc::clone(&recorder),
        dumps_dir: dumps_dir.clone(),
        readiness: Arc::clone(&readiness),
        idle_line: Arc::clone(&idle_line),
    }))?;
    server_slot
        .set(server)
        .map_err(|_| "control server slot set twice")?;

    // The start, which the reducer reports with the start sound, and the
    // initial outpost target (the focus listener only reports foreground
    // changes after this point).
    let _ = command_tx.send(ShellCommand::Input(Box::new(Input::Started)));
    // Ask the foreground application for its current focus: its outpost is
    // started if needed, and the answer is spoken like a switch to it.
    if let Some(pid) = foreground_pid() {
        // A console window's outpost is its console host's.
        if let Some(server) = focus_now_server(pid) {
            note_targeted_pid(&outposts, server);
        }
        let _ = command_tx.send(ShellCommand::FocusNow(pid));
    }

    // The GUI loop owns the main thread until shutdown.
    let host_for_gui: Arc<dyn SpeechSettingsHost> = Arc::new(settings_host);
    let theme_host: Arc<dyn verbatim_gui::ThemeHost> = Arc::new(themes::AppThemeHost {
        store: Arc::clone(&store),
        manager: Arc::clone(&manager),
        commands: command_tx.clone(),
    });
    let terminal_host: Arc<dyn verbatim_gui::TerminalHost> =
        Arc::new(terminal_settings::AppTerminalHost {
            store: Arc::clone(&store),
            commands: command_tx.clone(),
        });
    let shell_host: Arc<dyn verbatim_gui::ShellItemsHost> = Arc::new(AppShellItemsHost {
        commands: command_tx.clone(),
    });
    let handle_slot = Arc::clone(&gui_handle);
    let gui_readiness = Arc::clone(&readiness);
    run_gui(
        host_for_gui,
        theme_host,
        terminal_host,
        shell_host,
        gui_event_tx,
        move |handle| {
            let _ = handle_slot.set(handle);
            gui_readiness.notify();
        },
    )?;

    // The wx loop has exited (Exit menu item, control-plane quit, or a
    // replacing instance's WM_QUIT). The keyboard hook stops first. The
    // outposts and the focus listener are shut down cleanly while the exit
    // sound plays, which is heard before Verbatim goes, within a bound:
    // each is asked to shut down and killed through its job only if it
    // has not exited in time. Job objects still kill anything left when
    // the process exits. An instance being replaced plays no exit sound
    // and stops at once, unlike NVDA (Dickson, 2026-10-10; `docs/parity/system.md`,
    // "Replacing a running instance").
    drop(hook);
    let shutdown = {
        let supervisor = Arc::clone(&supervisor);
        thread::Builder::new()
            .name("verbatim-shutdown".to_owned())
            .spawn(move || supervisor.shutdown())?
    };
    // The reducer decides what is heard of the exit; this thread waits
    // until it has been.
    let deadline = Instant::now() + EXIT_SOUND_TIMEOUT;
    let replaced = instance.being_replaced();
    let (heard_tx, heard_rx) = crossbeam_channel::bounded(1);
    let _ = command_tx.send(ShellCommand::Exiting {
        replaced,
        heard: heard_tx,
    });
    let heard = match heard_rx.recv_deadline(deadline) {
        Ok(Some(exit_sound)) => exit_sound.wait_until(deadline),
        Ok(None) => true,
        Err(_) => false,
    };
    if replaced {
        tracing::info!("being replaced by a new instance; no exit sound");
    } else if !heard {
        tracing::warn!(
            timeout = ?EXIT_SOUND_TIMEOUT,
            "the exit sound was not heard in time; exiting anyway"
        );
    }
    if let Ok(summary) = shutdown.join() {
        tracing::info!(
            clean = summary.clean,
            exited = summary.exited,
            killed = summary.killed,
            killed_on_failure = summary.killed_on_failure,
            "outposts and the focus listener have ended: how every one that ended while Verbatim ran ended"
        );
    } else {
        tracing::error!("shutting down the outposts failed; their jobs end them");
    }
    Ok(())
}

/// The longest Verbatim waits for its exit sound to be heard before it
/// exits on any quit but being replaced: NVDA's exit sound plays for about
/// half a second.
const EXIT_SOUND_TIMEOUT: Duration = Duration::from_secs(2);

/// Names a WAV file to record everything Verbatim plays into.
const RECORD_AUDIO_ENV: &str = "VERBATIM_RECORD_AUDIO";

/// The synthesizer host executable, next to this one.
const SYNTH_HOST_EXE: &str = "verbatim-synth-host.exe";

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
                report_error!("verbatim: could not create default config files: {error}");
            }
            config
        }
        Err(error) => {
            report_error!("verbatim: config error, continuing with defaults: {error}");
            ConfigStore::load(&std::env::temp_dir().join("verbatim-defaults"))
                .unwrap_or_else(|fallback| panic!("default config must load: {fallback}"))
        }
    }
}

/// Installs the process-wide tracing subscriber; `RUST_LOG` wins over the
/// configured filter, which wins over plain `info`. Every error logged
/// also plays the error sound ([`error_sound`]).
fn init_tracing(configured: Option<&str>) {
    use tracing_subscriber::EnvFilter;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(configured.unwrap_or("info")))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        // A write that fails (a full disk, a closed pipe) is dropped: by
        // default the formatter reports it with `eprint!`, which panics when
        // standard error fails too, and a panic in a hook or a COM callback
        // aborts the process.
        .log_internal_errors(false)
        .finish()
        .with(error_sound::ErrorSoundLayer)
        .init();
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

/// Builds the speech pipeline: eSpeak NG (or the configured synthesizer),
/// each in a synthesizer host, through the mixer and WASAPI by default,
/// observed by the latency ledger, and presenting speech with `theme`.
/// `VERBATIM_TEST_AUDIO=null` is a
/// test-only escape hatch (documented in docs/crates/verbatim-audio.md) that
/// adds the capture synth and plays through [`SilentDevice`] instead, which
/// takes real time but makes no sound.
///
/// # Errors
///
/// Returns an error if no registered synthesizer can start, the audio
/// device cannot open, or the speech queue's port cannot be made.
fn build_speech_manager(
    store: &Arc<Mutex<ConfigStore>>,
    events: Arc<dyn verbatim_speech::SpeechEvents>,
    theme: verbatim_speech::ActiveTheme,
) -> Result<(Arc<SpeechManager>, SpeakToken), verbatim_speech::SynthError> {
    let test_audio = std::env::var("VERBATIM_TEST_AUDIO").is_ok_and(|value| value == "null");
    let mut registry = SynthRegistry::new();
    // Every native synthesizer runs in a synthesizer host process next to
    // this executable (decision D18).
    // eSpeak NG is the default, so it is listed first.
    let host_exe = exe_dir().join(SYNTH_HOST_EXE);
    for (id, name_key) in [
        (synth_ids::ESPEAK, "synth-name-espeak"),
        (synth_ids::ONECORE, "synth-name-onecore"),
    ] {
        registry.register(
            SynthId::new(id),
            verbatim_i18n::message(name_key),
            verbatim_synth_hosted::factory(host_exe.clone(), SynthId::new(id)),
        );
    }
    if test_audio {
        tracing::warn!(
            "VERBATIM_TEST_AUDIO=null: test audio mode is active; the capture synth is available and audio plays silently in real time"
        );
        register_test_audio(&mut registry);
    }
    let initial_synth = initial_synth(&store.lock().unwrap_or_else(PoisonError::into_inner));
    let device: Box<dyn AudioDevice> = if test_audio {
        Box::new(SilentDevice::new())
    } else {
        Box::new(
            WasapiDevice::new()
                .map_err(|error| verbatim_speech::SynthError::Unavailable(error.to_string()))?,
        )
    };
    // `VERBATIM_RECORD_AUDIO=<file.wav>` records everything Verbatim plays,
    // as heard, for the end-to-end harness's videos (decision D16).
    let recorder = std::env::var_os(RECORD_AUDIO_ENV).and_then(|path| {
        WavRecorder::create(std::path::Path::new(&path))
            .inspect_err(|error| tracing::warn!(%error, "cannot record audio"))
            .ok()
    });
    let mixer = match recorder {
        Some(recorder) => Mixer::start_with_tap(device, Box::new(recorder)),
        None => Mixer::start(device),
    }
    .map_err(|error| verbatim_speech::SynthError::Unavailable(error.to_string()))?;
    // The speech queue's inbox is a completion port, so the keyboard hook's
    // cancel takes no lock (`docs/design/ownership-input.md`, "The speech
    // queue's inbox").
    let (inbox, taker) = port_queue::<QueueItem>()
        .map_err(|error| verbatim_speech::SynthError::Unavailable(error.to_string()))?;
    let (manager, token) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth,
        saved_settings: saved_settings_fn(Arc::clone(store)),
        mixer: Arc::new(mixer),
        events: Some(events),
        theme,
        presenter: None,
        inbox: (Arc::new(PortInbox(inbox)), Box::new(PortTaker(taker))),
    })?;
    Ok((Arc::new(manager), token))
}

/// The speech queue's inbox as a completion port's posting end: one
/// `PostQueuedCompletionStatus`, which takes no lock and never waits.
struct PortInbox(Poster<QueueItem>);

impl SpeechInbox for PortInbox {
    fn post(&self, item: QueueItem) -> Result<(), QueueItem> {
        self.0.send(item)
    }
}

/// The speech queue thread's end of its completion port.
struct PortTaker(PortQueue<QueueItem>);

impl SpeechInboxTaker for PortTaker {
    fn take(&mut self) -> Option<QueueItem> {
        self.0.recv()
    }
}

/// Registers the capture synth from `verbatim-synth-capture` alongside
/// the real synthesizers, for `VERBATIM_TEST_AUDIO=null` runs. Test-only: never active
/// unless that environment variable is set at startup.
fn register_test_audio(registry: &mut SynthRegistry) {
    registry.register(
        SynthId::new("capture"),
        "Capture (test audio)",
        Box::new(|| Ok(Box::new(CaptureSynth::new()) as Box<dyn verbatim_speech::SynthDriver>)),
    );
}

/// The configured synthesizer, or eSpeak NG when none is configured. One
/// that is not registered, or cannot start, is left to the speech manager's
/// fallback, which tries eSpeak NG next because it is registered first.
fn initial_synth(config: &ConfigStore) -> SynthId {
    config
        .active()
        .synthesizer()
        .map_or_else(|| SynthId::new(synth_ids::ESPEAK), SynthId::new)
}

/// The callback the speech manager reads each synthesizer's saved settings
/// through whenever it starts one, at startup or on a switch, so a switch
/// sees values committed since startup.
fn saved_settings_fn(store: Arc<Mutex<ConfigStore>>) -> verbatim_speech::SavedSettingsFn {
    Box::new(move |synth| {
        saved_settings(&store.lock().unwrap_or_else(PoisonError::into_inner), synth)
    })
}

/// Persisted setting values for a synthesizer, mapped from config values to
/// driver values.
fn saved_settings(config: &ConfigStore, synth: &SynthId) -> Vec<(SettingId, SettingValue)> {
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
/// synth's values into the base profile, and the synth itself as the
/// configured one when it is the user's choice rather than a fallback, and
/// save it.
fn persist_fn(store: Arc<Mutex<ConfigStore>>) -> verbatim_speech::PersistFn {
    Box::new(move |synth_id, chosen, values| {
        let mut store = store
            .lock()
            .map_err(|_| "config store poisoned".to_owned())?;
        let speech = &mut store.settings_mut().speech;
        if chosen {
            speech.synthesizer = Some(synth_id.0.clone());
        }
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

/// The hook configuration from global settings: the Verbatim modifier keys
/// and NVDA's two speech interrupt settings.
fn decision_config(store: &Arc<Mutex<ConfigStore>>) -> DecisionConfig {
    let store = store.lock().expect("config store lock");
    let keys = store.settings().verbatim_keys;
    let keyboard = &store.settings().keyboard;
    DecisionConfig {
        caps_lock: keys.caps_lock,
        insert: keys.insert,
        numpad_insert: keys.numpad_insert,
        share_modifier: keys.share_modifier,
        interrupt_for_characters: keyboard.speech_interrupt_for_characters,
        interrupt_for_enter: keyboard.speech_interrupt_for_enter,
        ..DecisionConfig::default()
    }
}

/// Saves reader settings the reducer changed with a toggle key
/// (`Effect::SettingsChanged`), or the settings dialog's Terminal page
/// changed, into the base profile.
fn save_reader_settings(store: &Mutex<ConfigStore>, settings: ReaderSettings) {
    let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
    store.settings_mut().reader = settings;
    if let Err(error) = store.save_settings() {
        tracing::warn!(%error, "the reader settings could not be saved");
    }
}

/// Keeps the display on while say-all reads, or lets it turn off again
/// (`Effect::KeepDisplayOn`), NVDA's "Prevent display from turning off
/// during say all". The request belongs to the calling thread, the reducer
/// thread, which lives as long as Verbatim does.
fn keep_display_on(on: bool) {
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let flags = if on {
        ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: SetThreadExecutionState takes plain flags and only records
    // the calling thread's request.
    if unsafe { SetThreadExecutionState(flags) }.0 == 0 {
        tracing::warn!(on, "the display could not be kept on");
    }
}

/// Notes that `target` is (or is about to be) watched by an outpost, adding a
/// `Starting` placeholder to the status mirror only if this pid is not
/// already known. Multiple outposts coexist under decision D9, so unlike
/// M1's single-outpost policy this must never clear existing entries: a
/// foreground change to a pid Core already has an outpost for is routed to
/// that outpost by the supervisor rather than starting a new one, and its
/// existing status entry is left alone.
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
/// listener reports Core's window taking foreground (decision D13), a fact the
/// supervisor routes to this outpost, which reports the focus on the window.
fn warm_own_outpost(
    supervisor: &Arc<Supervisor>,
    outposts: &Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    own_pid: u32,
) {
    note_targeted_pid(outposts, Pid(own_pid));
    supervisor.ensure_spawned(Pid(own_pid));
}

/// The process owning the window `hwnd`, read with a local call.
fn window_pid(hwnd: isize) -> Option<Pid> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid: u32 = 0;
    // SAFETY: tolerates any handle; `pid` is a local out-parameter.
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&raw mut pid)) };
    (pid != 0).then_some(Pid(pid))
}

/// The list dialogs' way to the shell's items and a clicked item's
/// position: a request on the reducer thread's port, answered by the
/// taskbar process's outpost.
struct AppShellItemsHost {
    commands: Commands,
}

impl verbatim_gui::ShellItemsHost for AppShellItemsHost {
    fn request(&self, kind: ShellItemKind, ask: verbatim_gui::ShellAsk) -> bool {
        let sent = self
            .commands
            .send(ShellCommand::ShellItems(kind, ask))
            .is_ok();
        if !sent {
            tracing::debug!("the reducer thread is gone; the shell's items are not read");
        }
        sent
    }
}

/// The process that owns the taskbar window (`Shell_TrayWnd`), whose
/// outpost answers the shell items query; `None` when there is no taskbar.
/// Local calls.
fn taskbar_process() -> Option<Pid> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    // SAFETY: FindWindowW reads window state only.
    let taskbar = unsafe {
        FindWindowW(
            &windows::core::HSTRING::from("Shell_TrayWnd"),
            windows::core::PCWSTR::null(),
        )
    }
    .ok()
    .filter(|hwnd| !hwnd.is_invalid())?;
    window_pid(taskbar.0 as isize)
}

/// The process whose outpost answers a focus-now query for the
/// application `pid`. When Windows names `pid` as the owner of the window in
/// front, the process that serves that window
/// ([`verbatim_outpost::provider_process`]), which differs from its owner
/// only for a console window, whose console host serves it; `None` when it
/// cannot be read, for a console window whose host cannot be read from its
/// client or a window closed since, so no outpost is asked: the client's
/// own serves none of its console window. `pid` itself for any other
/// window in front. Local calls only.
fn focus_now_server(pid: Pid) -> Option<Pid> {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: no preconditions.
    let hwnd = unsafe { GetForegroundWindow() }.0 as isize;
    if window_pid(hwnd) != Some(pid) {
        return Some(pid);
    }
    verbatim_outpost::provider_process(hwnd).map(Pid)
}

/// The application holding the system foreground right now, read with a
/// local call.
fn foreground_pid() -> Option<Pid> {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: no preconditions.
    let hwnd = unsafe { GetForegroundWindow() };
    let mut pid: u32 = 0;
    // SAFETY: tolerates any handle; `pid` is a local out-parameter.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    (pid != 0).then_some(Pid(pid))
}

/// The longest a held caret key waits for the read of the caret before it
/// (`TextOp::ReadCaret`): past it the read is ended as unanswered and the
/// key is passed on as the hook would have passed it
/// (`docs/design/key-holding.md`, "The bound").
const HELD_KEY_BOUND: Duration = Duration::from_millis(150);

/// The reducer thread's dependencies, bundled to keep [`reducer_loop`]'s
/// signature from growing an argument for every subsystem it touches.
struct ReducerContext {
    /// Whether the keyboard hook holds caret keys back, set from Core's
    /// focus after each input.
    holding: Arc<KeyHolding>,
    /// Core's view of the window in front for the keyboard hook, set after
    /// each input.
    foreground: Arc<ForegroundView>,
    manager: Arc<SpeechManager>,
    /// Where copies to the clipboard are made, off the reducer thread.
    clipboard: clipboard::Clipboard,
    supervisor: Arc<Supervisor>,
    ledger: Arc<LatencyLedger>,
    server_slot: Arc<OnceLock<ControlServer>>,
    outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>>,
    recorder: SharedRecorder,
    listener_ready: Arc<AtomicBool>,
    /// Set once Core knows a focus, for the control plane's readiness.
    focus_known: Arc<AtomicBool>,
    /// The configuration, where reader settings changed by a toggle key are
    /// saved.
    store: Arc<Mutex<ConfigStore>>,
    /// What the control plane reports as readiness, and the harness's
    /// readiness event.
    readiness: Arc<harness::Readiness>,
    /// The control plane's `AwaitIdle` waiters.
    idle_line: Arc<harness::IdleLine>,
    /// The GUI's handle, once the GUI thread is up, which the menu and the
    /// shell's lists are shown through (`Effect::Show`).
    gui_handle: Arc<OnceLock<GuiHandle>>,
}

/// Where every thread posts the reducer thread's inputs: its port.
pub(crate) type Commands = Poster<ShellCommand>;

/// Everything handed to the reducer thread, each one packet posted to its
/// port and so taken in the order it arrived: a message from the
/// supervisor (an outpost's frame, or an outpost's start or end), a
/// reducer input (a command, a script, a lock key, a caret key, typed text,
/// or a cancelled speech from the keyboard hook, a mark speech reached), or
/// a control-plane tree dump, answered on the given channel.
pub(crate) enum ShellCommand {
    /// A message from the supervisor, put in line by [`outpost_sink`].
    Outpost(OutpostMessage),
    Input(Box<Input>),
    /// A caret key the hook passed, an `Input::CaretKey`, with when the hook
    /// saw it in microseconds since the Unix epoch, where its latency line
    /// starts.
    CaretKey {
        input: Box<Input>,
        pressed_at_us: u64,
    },
    /// A caret key the hook held back, an `Input::HeldKey` (`None` for a
    /// key that is no caret key, which is passed on at once), with its
    /// trace, its gesture, when the hook saw it, and the stroke to send
    /// again once Core passes it on.
    HeldKey {
        input: Option<Box<Input>>,
        trace_id: TraceId,
        gesture: GestureId,
        pressed_at_us: u64,
        stroke: verbatim_input_windows::HeldStroke,
    },
    /// A key transition the hook deferred behind a held key, sent on in
    /// its turn.
    DeferredKey {
        stroke: verbatim_input_windows::HeldStroke,
        pressed: bool,
    },
    /// A control-plane tree dump, reading locations when the flag says.
    DumpTree(DumpTicket, bool, Sender<DumpTreeResult>),
    /// The control-plane caller waiting for this tree dump stopped waiting:
    /// withdraw the dump if its outpost has not started it.
    DumpTreeGivenUp(DumpTicket),
    /// Ask this application for its current focus: at startup, for the
    /// foreground application.
    FocusNow(Pid),
    /// Verbatim is exiting: the reducer is told, and whatever it plays to
    /// be heard before Verbatim exits (the exit sound) is handed back on
    /// `heard`, for the main thread to wait on; `None` when nothing is.
    Exiting {
        replaced: bool,
        heard: Sender<Option<verbatim_speech::EarconHeard>>,
    },
    /// The settings dialog's Terminal page applied a change: merge it into
    /// the reducer's reader settings, give it them, and save them.
    TerminalSettings(verbatim_gui::TerminalChange),
    /// An end-to-end harness's numbered key, handled by the hook:
    /// everything it caused is ahead of this in line.
    InputHandled(u64),
    /// A control-plane `AwaitIdle` waiter reached its place in line behind
    /// everything the hook handled before it, named by its token in the
    /// [`harness::IdleLine`].
    AwaitIdle(u64),
    /// Answer the control plane's `DumpFocus` on the channel.
    DumpFocus(Sender<FocusReport>),
    /// The tray or taskbar list dialog wants the shell's items, or where a
    /// clicked item is: ask the outpost of the process that owns the
    /// taskbar, and hand the outcome to the delivery callback.
    ShellItems(ShellItemKind, verbatim_gui::ShellAsk),
}

/// What the reducer thread owns: the reducer state, the request table, and
/// the live-outpost set. The thread never blocks on a handoff: speech and
/// control-plane broadcasts are channel sends that never wait, a control
/// request is answered on a channel with room for its one answer, and every
/// query gets its outcome through the request table. Commands for an outpost
/// are queued on that outpost's writer thread, never written to its pipe
/// here.
struct ReducerThread<'a> {
    context: &'a ReducerContext,
    state: SrState,
    requests: RequestTable,
    /// Outpost generations that have started and not yet ended. A message
    /// from an outpost that has ended is dropped before the reducer sees it.
    live: LiveOutposts,
    /// Applications to ask for their current focus as soon as their outpost
    /// is ready: the startup foreground application, and the attention
    /// application after its outpost or the listener was replaced: by the
    /// process whose outpost is awaited, the application to ask for, which
    /// differs for a console host, asked for its console's client.
    focus_now_wanted: HashMap<Pid, Pid>,
    /// The shell's items wanted by the list dialogs, each asked of the
    /// taskbar's process once its outpost is ready.
    shell_items_wanted: shell_requests::ShellItemsWanted,
    /// The views last sent to the supervisor.
    views: (Option<Pid>, BTreeSet<OutpostId>),
    /// The outposts a focus report of which was handled since the views
    /// were last sent, which are written the nodes Core holds in them
    /// ([`send_views`](Self::send_views)).
    focus_reported: BTreeSet<OutpostId>,
    /// The highest numbered harness key handled so far.
    handled_input: u64,
    /// Turns the outposts' frames, which the supervisor's relay posts
    /// unparsed, into the messages they carry, in each outpost's order.
    frames: Frames,
    /// The caret keys the hook held back, until Core passes them on.
    held_keys: held_keys::HeldKeys,
    /// The held key waiting for the watch of the key ahead of it, and when
    /// its wait ends if no answer has come ([`Effect::BoundHeldKeyWait`]):
    /// one of the reducer thread's deadlines, so it adds no wake-up source.
    held_wait: Option<(TraceId, Instant)>,
    /// The focused terminal's quiet-period timer, as Core armed it
    /// ([`Effect::TerminalTimer`]): when it is due on the shared clock, and
    /// as an instant, one of the reducer thread's deadlines.
    terminal_timer: Option<(u64, Instant)>,
    /// Control-plane `AwaitIdle` waiters that reached their place in line,
    /// and the idle barrier's round through every child that answers them
    /// (`docs/design/communication.md`, section 6.7).
    barrier: idle::IdleBarrier<Sender<()>>,
    /// Given spares that failed before their `Ready` and whose `Ended` is
    /// still to come: their applications' held facts went to the next
    /// generation ([`OutpostMessage::Regenerated`]), so their end asks
    /// for no focus.
    regenerated: HashSet<OutpostId>,
    /// Whether Verbatim is exiting, so the exit sound is played to be
    /// waited on, and the exit sound the reducer played then, for the main
    /// thread to wait on until it has been heard ([`ShellCommand::Exiting`]).
    exiting: bool,
    exit_sound: Option<verbatim_speech::EarconHeard>,
    /// The lock keys whose state is to be read, each with when, in press
    /// order ([`Effect::ReadToggleKey`]): the reducer thread's deadlines.
    toggle_reads: VecDeque<(LockKey, Instant)>,
    /// The harness's marks of handled input held behind a lock key's
    /// announcement still to come.
    marks: idle::HeldMarks,
    /// Inputs an effect produced, applied once the current input's effects
    /// have all been executed, so one input is reduced at a time.
    follow_ups: Vec<Input>,
    /// The right to speak, which only this thread holds.
    token: SpeakToken,
}

impl ReducerThread<'_> {
    /// Handles one message from the supervisor.
    fn on_outpost_message(&mut self, message: OutpostMessage) {
        match message {
            OutpostMessage::Started {
                outpost,
                target_pid,
            } => {
                self.live.started(outpost, target_pid);
                self.send_fetches();
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
            } => self.on_outpost_ended(outpost, target_pid, reason),
            OutpostMessage::Regenerated {
                outpost,
                next,
                target_pid,
            } => {
                tracing::info!(%outpost, %next, %target_pid, "a given spare failed before it was ready; the next generation holds its application's facts");
                self.barrier.handed_on(outpost, next);
                // Its `Ended` follows only if the app heard it started.
                if self.live.application_of(outpost).is_some() {
                    self.regenerated.insert(outpost);
                }
            }
            OutpostMessage::SpareReady { spare, waiting, .. } => {
                tracing::debug!(%spare, waiting, "spare outpost ready");
                if waiting >= verbatim_outpost::supervisor::SPARES {
                    self.context.readiness.spares_ready();
                }
            }
            OutpostMessage::ListenerReady { replacement } => {
                self.context.listener_ready.store(true, Ordering::Release);
                self.context.readiness.notify();
                // Facts were lost while there was no listener: read the
                // foreground afresh and ask its application for the focus.
                if replacement && let Some(pid) = foreground_pid() {
                    self.want_focus_now(pid);
                }
            }
            OutpostMessage::MenuOrSwitchEnded { ended_at_us } => {
                self.apply(Input::MenuOrSwitchEnded { ended_at_us });
            }
            OutpostMessage::ForegroundWindow(report) => {
                self.apply(Input::ForegroundWindow(ForegroundWindow {
                    window: WindowHandle(u64::from_ne_bytes(report.window.to_ne_bytes())),
                    thread: report.thread,
                    console: report.console,
                    integrity: report.integrity,
                    own_integrity: verbatim_outpost::integrity::own_integrity(),
                }));
            }
            OutpostMessage::IdleQueried { round, outposts } => {
                self.barrier.queried(round, outposts);
            }
            // `Frames` turns these into the messages they carry, before
            // anything reaches here.
            OutpostMessage::Frame(_) => {}
            OutpostMessage::NotWatched { target_pid } => {
                // The application is not running, or is ignored: no outpost
                // will answer for it, so its focus is no longer awaited,
                // nor the shell's items.
                self.focus_now_wanted.remove(&target_pid);
                self.shell_items_wanted.refuse(target_pid);
                let mut outposts = self.context.outposts.lock().expect("outposts lock");
                if outposts
                    .get(&target_pid)
                    .is_some_and(|status| status.outpost_pid.is_none())
                {
                    outposts.remove(&target_pid);
                }
            }
        }
    }

    /// The reducer asked for the focus of whichever application is in
    /// front (`Effect::ReadForegroundFocus`), after a menu or the Alt+Tab
    /// switcher closed: usually not the one that owned the menu, as NVDA's
    /// fake focus. Only the control is reported, as NVDA queues a focus on
    /// it and nothing for its window. An application with no ready outpost
    /// is asked for its window too, since attention may be moving to it.
    fn fake_focus(&mut self, trace_id: TraceId) {
        let Some(pid) = foreground_pid().filter(|&pid| !self.context.supervisor.is_ignored(pid))
        else {
            return;
        };
        let Some(outpost) = self.live.ready(pid) else {
            self.want_focus_now(pid);
            return;
        };
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
            CoreToOutpost::Query {
                trace_id,
                request_id: id.0,
                query: Query::FocusNow,
                timing: EventTiming::default(),
            },
        );
    }

    /// Whether `event` is a focus in a top-level window of a process
    /// Verbatim ignores, which another application's outpost can report as
    /// the window around its own focus: it is dropped, so an ignored process
    /// never becomes the focus or the attention. A local read.
    fn in_ignored_window(
        &self,
        event: &verbatim_model::NormalizedEvent,
        window: Option<&verbatim_model::WindowFacts>,
    ) -> bool {
        let (verbatim_model::NormalizedEvent::FocusChanged { .. }, Some(window)) = (event, window)
        else {
            return false;
        };
        let owner = window_pid(isize::try_from(window.top_level.0.cast_signed()).unwrap_or(0));
        let ignored = owner.is_some_and(|pid| self.context.supervisor.is_ignored(pid));
        if ignored {
            tracing::info!(
                ?owner,
                "a focus in a window of an ignored process is dropped"
            );
        }
        ignored
    }

    /// Asks the outpost serving the application `pid` for the current focus
    /// now if it is ready, or as soon as it is, starting one if there is
    /// none. A console's client whose console window is in front is served
    /// by its console host's outpost, found locally ([`focus_now_server`]),
    /// whether or not that outpost has reported for it yet, and by none
    /// when that host cannot be read: the client's own outpost serves none
    /// of the console window, and connects to no other application's window
    /// (`docs/design/listener-without-connections.md`, section 4.3).
    fn want_focus_now(&mut self, pid: Pid) {
        if self.context.supervisor.is_ignored(pid) {
            return; // Nothing is asked of a process Verbatim ignores.
        }
        let Some(server) = focus_now_server(pid) else {
            tracing::info!(
                %pid,
                "no outpost is asked for the focus: the window in front has no provider process that can be read"
            );
            return;
        };
        if server != pid {
            let host = server;
            if let Some(outpost) = self.live.ready(host) {
                self.live.reports_for(outpost, pid);
                self.focus_now(outpost, pid);
            } else {
                self.focus_now_wanted.insert(host, pid);
                self.context.supervisor.ensure_spawned(host);
            }
        } else if let Some(outpost) = self.live.ready(pid) {
            self.focus_now(outpost, pid);
        } else {
            let target = self.live.target_for(pid);
            self.focus_now_wanted.insert(target, pid);
            self.context.supervisor.ensure_spawned(target);
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
            CoreToOutpost::Query {
                trace_id,
                request_id: id.0,
                query: Query::FocusNow,
                timing: EventTiming::default(),
            },
        );
    }

    /// Turns one message from a live outpost into reducer input, or routes a
    /// reply to whoever asked.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn on_outpost_input(&mut self, source: Pid, outpost: OutpostId, message: OutpostToCore) {
        match message {
            OutpostToCore::Event {
                trace_id,
                observed_at_us,
                backend,
                ref window,
                application,
                timing,
                ref event,
            } => {
                // The latency timeline starts when the event was first
                // observed. That is `observed_at_us` except for a foreground
                // change, which is ordered by when its window became the
                // foreground, later than Windows raised it.
                // An event read is the outpost's application answering.
                self.live.answered(outpost);
                // An event in a console window, reported by the console
                // host's outpost, belongs to the console's client, and so
                // does that outpost from now on.
                if let Some(application) = application {
                    self.live.reports_for(outpost, application);
                }
                if self.in_ignored_window(event, window.as_ref()) {
                    // Core has judged the focus, refused: the outpost hears
                    // the nodes Core holds after it, as after any focus
                    // report.
                    if matches!(event, verbatim_model::NormalizedEvent::FocusChanged { .. }) {
                        self.focus_reported.insert(outpost);
                    }
                    return;
                }
                let first_observed_us = match timing.observed_at_us {
                    0 => observed_at_us,
                    us => us,
                };
                self.context
                    .ledger
                    .event_observed(trace_id, first_observed_us);
                self.context.ledger.event_received(trace_id, timing);
                // The event is copied for the control plane only when some
                // connection is subscribed to events.
                if let Some(server) = self.context.server_slot.get()
                    && server.has_event_subscribers()
                {
                    server.broadcast_event(
                        trace_id,
                        application.unwrap_or(source),
                        backend,
                        window.clone(),
                        event.clone(),
                    );
                }
                if let Some(input) = verbatim_model::protocol::event_input(source, message) {
                    self.apply(input);
                }
            }
            OutpostToCore::Reply {
                trace_id,
                request_id,
                outcome,
                timing,
            } => {
                // A reply the worker answered carries its timing and calls;
                // a withdrawn or abandoned query's carries none.
                if timing.dequeued_at_us != 0 {
                    self.context.ledger.event_received(trace_id, timing);
                }
                self.finish(RequestId(request_id), outpost, outcome);
            }
            OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            } => {
                tracing::info!(%outpost, %outpost_pid, %target_pid, "outpost ready");
                self.live.mark_ready(outpost);
                if let Some(application) = self.focus_now_wanted.remove(&target_pid) {
                    if application != target_pid {
                        // A console host's replacement, asked for its
                        // console's client.
                        self.live.reports_for(outpost, application);
                    }
                    self.focus_now(outpost, application);
                }
                for wanted in self.shell_items_wanted.take(target_pid) {
                    self.ask_shell_items(outpost, wanted.kind, wanted.ask);
                }
                self.context.outposts.lock().expect("outposts lock").insert(
                    target_pid,
                    OutpostStatus {
                        target_pid,
                        outpost_pid: Some(outpost_pid),
                        state: OutpostState::Ready,
                    },
                );
                self.context.readiness.notify();
            }
            OutpostToCore::Fault { detail } => {
                tracing::warn!(%source, %outpost, detail, "outpost fault");
            }
            OutpostToCore::IdleAnswer { round } => self.barrier.answered(round, outpost),
            // The relay consumes these, and hands none of them on.
            OutpostToCore::Pong { .. }
            | OutpostToCore::TargetExited
            | OutpostToCore::SpareReady { .. } => {}
        }
    }

    /// Handles work handed over by another thread.
    fn on_command(&mut self, command: ShellCommand) {
        match command {
            ShellCommand::Outpost(message) => {
                if let Some(message) = self.frames.arrived(message) {
                    self.on_outpost_message(message);
                }
            }
            ShellCommand::Input(input) => self.apply(*input),
            ShellCommand::CaretKey {
                input,
                pressed_at_us,
            } => {
                if let Input::CaretKey { trace_id, .. } = &*input {
                    self.context.ledger.key_pressed(*trace_id, pressed_at_us);
                }
                self.apply(*input);
            }
            ShellCommand::HeldKey {
                input,
                trace_id,
                gesture,
                pressed_at_us,
                stroke,
            } => {
                self.held_keys
                    .held(trace_id, &gesture, stroke, pressed_at_us);
                if let Some(input) = input {
                    self.context.ledger.key_pressed(trace_id, pressed_at_us);
                    self.apply(*input);
                } else {
                    self.held_keys.pass_on(trace_id);
                }
            }
            ShellCommand::DeferredKey { stroke, pressed } => {
                self.held_keys.deferred(stroke, pressed);
            }
            ShellCommand::DumpTree(ticket, locations, reply) => {
                self.dump_tree(ticket, locations, reply);
            }
            ShellCommand::ShellItems(kind, ask) => self.shell_items(kind, ask),
            ShellCommand::DumpTreeGivenUp(ticket) => self.dump_tree_given_up(ticket),
            ShellCommand::FocusNow(pid) => self.want_focus_now(pid),
            ShellCommand::Exiting { replaced, heard } => {
                self.exiting = true;
                self.apply(Input::Exiting { replaced });
                let _ = heard.send(self.exit_sound.take());
            }
            ShellCommand::TerminalSettings(change) => self.change_terminal_settings(change),
            ShellCommand::InputHandled(input) => {
                // Behind a lock key's announcement still to come, as the
                // key's stroke caused it.
                for input in self.marks.take(input, self.state.announces_toggle_key()) {
                    self.input_handled(input);
                }
            }
            ShellCommand::AwaitIdle(token) => {
                if let Some(waiter) = self.context.idle_line.take(token) {
                    self.barrier.push(waiter.after_input, waiter.reply);
                }
            }
            ShellCommand::DumpFocus(reply) => {
                let (focus, ancestors, navigator) = self.state.focus_report();
                let _ = reply.send(FocusReport {
                    focus: focus.cloned(),
                    ancestors: ancestors.to_vec(),
                    navigator: navigator.cloned(),
                });
            }
        }
    }

    /// Notes that the harness's numbered input `input` has been handled,
    /// and tells the control plane.
    fn input_handled(&mut self, input: u64) {
        self.handled_input = self.handled_input.max(input);
        if let Some(server) = self.context.server_slot.get() {
            server.broadcast_input_handled(input);
        }
    }

    /// Moves the idle barrier on (`docs/design/communication.md`, section
    /// 6.7), between steps, so the reducer is never in the middle of one.
    /// Once every outpost the round asked has answered or ended, takes in
    /// everything already queued for the reducer, then checks its own
    /// conditions: when they hold, the round has passed, and the speech
    /// queue answers each of its waiters once it has taken in everything
    /// the reducer gave it; when not, the round waits for them to clear.
    /// Starts a round, through the supervisor's relay, when one is due.
    fn advance_idle_barrier(&mut self, command_rx: &mut PortQueue<ShellCommand>) {
        if !self.barrier.has_waiters() {
            return;
        }
        if self.barrier.all_answered() {
            // What is queued now is ahead of the answers, or came with
            // them: it is handled before the conditions are checked.
            while let Some(command) = command_rx.try_recv() {
                self.on_command(command);
                self.send_views();
            }
            let clear = self.idle_conditions_clear();
            for reply in self.barrier.finish(clear) {
                self.context.manager.after_queued(move || {
                    let _ = reply.send(());
                });
            }
        }
        let clear = self.idle_conditions_clear();
        if let Some(round) = self.barrier.start(self.handled_input, clear) {
            self.context.supervisor.query_idle(round);
        }
        let requests = self.requests.outstanding();
        let starting = self.live.starting();
        let focus_wanted = self.focus_now_wanted.len();
        let standing = match self.barrier.standing() {
            idle::Standing::NoRound => "no round under way".to_owned(),
            idle::Standing::AwaitingRelay(round) => {
                format!("round {round} waits for the listener's answer or an outpost's launch")
            }
            idle::Standing::AwaitingOutposts(round, outposts) => {
                format!("round {round} waits for the answers of outposts {outposts:?}")
            }
            idle::Standing::Clearing(round) => {
                format!("round {round} was answered and waits for the reducer to clear")
            }
        };
        *self
            .context
            .idle_line
            .outstanding
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = format!(
            "handled input {}, {standing}, {requests} outpost requests outstanding, {starting} outposts starting, focus wanted from {focus_wanted} applications, terminal timer {}",
            self.handled_input,
            if self.terminal_timer.is_some() {
                "armed"
            } else {
                "not armed"
            }
        );
    }

    /// The reducer's own conditions for the idle barrier: no request to an
    /// outpost is outstanding, no outpost is starting (one still starting
    /// has facts or a focus query waiting for it, as a replacement for a
    /// crashed one does), no application's focus is wanted, no key the
    /// hook held back is still waiting to reach its application
    /// (`docs/design/key-holding.md`), no key or command waits in Core's
    /// held-key line, no lock key's announcement is still to come, and no
    /// terminal's quiet-period timer is armed, since what waits for it is
    /// spoken when it ends.
    fn idle_conditions_clear(&self) -> bool {
        self.requests.outstanding() == 0
            && self.held_keys.is_empty()
            && !self.state.holds_inputs_back()
            && !self.state.announces_toggle_key()
            && self.terminal_timer.is_none()
            && self.live.starting() == 0
            && self.focus_now_wanted.is_empty()
    }

    /// Merges a change from the settings dialog's Terminal page into the
    /// reader settings the reducer has now, toggle keys included, so none
    /// is undone; gives the reducer the result, and saves it.
    fn change_terminal_settings(&mut self, change: verbatim_gui::TerminalChange) {
        let mut settings = self.state.settings();
        change.apply_to(&mut settings);
        self.apply(Input::Settings(settings));
        save_reader_settings(&self.context.store, settings);
    }

    /// Delivers a query's outcome through the request table, applying the
    /// reducer input it produces, if any.
    fn finish(&mut self, id: RequestId, outpost: OutpostId, outcome: QueryOutcome) {
        // A query the outpost's watchdog abandoned passed its deadline in
        // a cross-process call: the application is not responding, which
        // is reported once per stall.
        match outcome {
            QueryOutcome::Abandoned => {
                if self.live.query_abandoned(outpost) {
                    self.apply(Input::ApplicationNotResponding { outpost });
                }
            }
            QueryOutcome::Done(_) | QueryOutcome::Gone => self.live.answered(outpost),
            _ => {}
        }
        for input in self.requests.finish(id, outpost, outcome) {
            self.apply(input);
        }
    }

    /// Runs one input through the reducer, records it in the flight
    /// recorder, and executes the resulting effects.
    fn apply(&mut self, input: Input) {
        let trace_id = input.trace_id().unwrap_or_else(TraceId::mint);
        if let Input::Command { command, .. } = &input {
            tracing::info!(%trace_id, ?command, "gesture");
        } else if let Input::Script { script, .. } = &input {
            tracing::info!(%trace_id, ?script, "gesture");
        }
        // A focus report, taken or refused, has its outpost told the nodes
        // Core holds there once the message is handled ([`send_views`]).
        if let Input::Event {
            event: verbatim_model::NormalizedEvent::FocusChanged { node, .. },
            ..
        } = &input
        {
            self.focus_reported.insert(node.id.outpost());
        }
        let effects = reduce(&mut self.state, &input);
        if self.state.focused().is_some() && !self.context.focus_known.swap(true, Ordering::AcqRel)
        {
            self.context.readiness.notify();
        }
        self.context.ledger.reduced(trace_id);
        self.context.holding.set(self.state.holds_caret_keys());
        self.context.foreground.set(self.state.hook_foreground());
        {
            let mut recorder = self
                .context
                .recorder
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            recorder.record_input(input, effects.len(), &self.state);
        }
        for effect in effects {
            self.execute(trace_id, effect);
        }
        for input in std::mem::take(&mut self.follow_ups) {
            self.apply(input);
        }
        // The marks held behind a lock key's announcement go once it has
        // been made.
        for input in self.marks.release(self.state.announces_toggle_key()) {
            self.input_handled(input);
        }
    }

    /// Sends the views derived from the reducer state, when they change: to
    /// the supervisor, the application holding attention and the outposts in
    /// which the state holds nodes; to each live outpost, the nodes held in
    /// it with the position of its last message handled here (outpost
    /// redesign, "Held objects").
    ///
    /// Called once a message or command has been handled whole, never
    /// between the inputs one message produces: a focus-now answer is a
    /// foreground change and then a focus, and held nodes sent after the
    /// first alone, with the answer's position as acknowledged, would have
    /// the outpost release the focus it reported in that same answer
    /// before Core had taken it. The text anchors the state holds in each
    /// outpost go with its nodes.
    ///
    /// The held nodes are written to an outpost when the set Core holds
    /// there changed, or after a focus report of its was handled, so the
    /// outpost releases what Core let go of within one pipe hop
    /// (`docs/design/ownership-one-judge.md`, "Manageability"). Any other
    /// change, of the held anchors or of the position of the last message
    /// handled alone, is left for the outpost, and the next command written
    /// to it, a request or the relay's ping within three seconds, carries
    /// it (`docs/design/communication.md`, step 11): the caret's anchor
    /// changes with every key, and a write for each would cost a pipe
    /// write and the outpost's reader wake on the typing path.
    fn send_views(&mut self) {
        let held = self.state.held_nodes();
        let anchors = self.state.held_anchors();
        // The supervisor keeps outposts by the process they watch: a
        // console's client holding attention keeps its console host's.
        let views = (
            self.state
                .attention()
                .map(|application| self.live.target_for(application)),
            held.keys().copied().collect::<BTreeSet<_>>(),
        );
        if views != self.views {
            self.context.supervisor.note_views(views.0, views.1.clone());
            self.views = views;
        }
        self.send_fetches();
        for (outpost, live) in self.live.iter_mut() {
            let nodes: BTreeSet<u64> = held
                .get(outpost)
                .map(|nodes| nodes.iter().map(|id| id.number()).collect())
                .unwrap_or_default();
            let outpost_anchors: BTreeSet<u64> = anchors
                .get(outpost)
                .map(|anchors| anchors.iter().map(|anchor| anchor.0).collect())
                .unwrap_or_default();
            let reported = self.focus_reported.contains(outpost);
            match live.held_changed(&nodes, &outpost_anchors) {
                live::HeldChange::None => {}
                change => {
                    let (nodes, anchors) = (
                        nodes.into_iter().collect(),
                        outpost_anchors.into_iter().collect(),
                    );
                    if change == live::HeldChange::Nodes || reported {
                        self.context.supervisor.send_nodes_held(
                            *outpost,
                            nodes,
                            anchors,
                            live.position,
                        );
                    } else {
                        self.context.supervisor.leave_nodes_held(
                            *outpost,
                            nodes,
                            anchors,
                            live.position,
                        );
                    }
                }
            }
        }
        self.focus_reported.clear();
    }

    /// Tells each live outpost the details the active theme wants read
    /// (`SrState::fetches`), when they differ from what it was last told:
    /// a detail whose indication is off is not read at all. A new outpost
    /// is told at once, before it reports anything.
    fn send_fetches(&mut self) {
        let fetches = self.state.fetches();
        for (outpost, live) in self.live.iter_mut() {
            if live.fetches_sent != Some(fetches) {
                match self
                    .context
                    .supervisor
                    .send_to_outpost(*outpost, CoreToOutpost::Fetches(fetches))
                {
                    Ok(()) => live.fetches_sent = Some(fetches),
                    // A full queue is retried after the next input; a closed
                    // one belongs to an outpost that is ending.
                    Err(error) => {
                        tracing::debug!(%error, %outpost, "the fetches were not sent yet");
                    }
                }
            }
        }
    }

    /// Executes one reducer effect.
    fn execute(&mut self, trace_id: TraceId, effect: Effect) {
        match effect {
            Effect::Speak(utterance, key) => {
                self.context.ledger.speech_requested(utterance.trace_id);
                // Speech a key press caused, stamped by Core with its
                // number, is fenced off by a later key press's cancel
                // (`SpeechControl::cancel_through`).
                self.context
                    .manager
                    .speak_for_key(&self.token, utterance, key);
            }
            Effect::StopSpeech => self.context.manager.control().cancel(),
            Effect::DropExpiredSpeech(now) => self.context.manager.control().drop_expired(now),
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
                let command = CoreToOutpost::Query {
                    trace_id,
                    request_id: id.0,
                    query: Query::Navigate {
                        node_id: query.node_id,
                        kind: query.kind,
                    },
                    timing: EventTiming::default(),
                };
                self.send(outpost, id, command);
            }
            Effect::Activate { node_id } => {
                let outpost = node_id.outpost();
                let id = self.requests.begin(outpost, Asker::Activation { trace_id });
                let command = CoreToOutpost::Query {
                    trace_id,
                    request_id: id.0,
                    query: Query::Activate { node_id },
                    timing: EventTiming::default(),
                };
                self.send(outpost, id, command);
            }
            Effect::CopyToClipboard(text) => self.context.clipboard.copy(trace_id, text),
            Effect::Text(request) => self.text_request(trace_id, request),
            Effect::KeepDisplayOn(on) => keep_display_on(on),
            Effect::SettingsChanged(settings) => {
                save_reader_settings(&self.context.store, settings);
            }
            // The exit sound is heard before Verbatim exits: the main thread
            // waits for it ([`ShellCommand::Exiting`]). A preview of it is
            // played as any other earcon.
            Effect::PlayEarcon(Earcon::Exit) if self.exiting => {
                self.exit_sound = Some(
                    self.context
                        .manager
                        .play_earcon_heard(&self.token, Earcon::Exit),
                );
            }
            Effect::PlayEarcon(earcon) => self.context.manager.play_earcon(&self.token, earcon),
            Effect::ReadForegroundFocus => self.fake_focus(trace_id),
            Effect::PassHeldKey(held) => self.held_keys.pass_on(held),
            Effect::BoundHeldKeyWait { key, within_ms } => {
                self.held_wait = Some((key, Instant::now() + Duration::from_millis(within_ms)));
            }
            Effect::TerminalTimer { due_ms } => {
                self.terminal_timer = due_ms.map(|due_ms| {
                    let wait = due_ms.saturating_sub(latency::now_ms());
                    (due_ms, Instant::now() + Duration::from_millis(wait))
                });
            }
            Effect::ForKey(key, effect) => self.execute(key, *effect),
            Effect::ReadClock { repeat } => {
                self.follow_ups.push(read_clock(trace_id, repeat));
            }
            Effect::Show(request) => send_gui_command(
                &self.context.gui_handle,
                match request {
                    GuiRequest::Menu => GuiCommand::ShowMenu,
                    GuiRequest::ShellList(kind) => GuiCommand::OpenShellItemList(kind),
                },
            ),
            Effect::ReadToggleKey { toggle, due_ms } => {
                let wait = due_ms.saturating_sub(latency::now_ms());
                self.toggle_reads
                    .push_back((toggle, Instant::now() + Duration::from_millis(wait)));
            }
            _ => {}
        }
    }

    /// Sends a text request to the outpost of the node it names; the answer
    /// re-enters the reducer as `Input::TextCompleted`.
    fn text_request(&mut self, trace_id: TraceId, request: TextRequest) {
        let TextRequest {
            query_id,
            node_id,
            op,
        } = request;
        let outpost = node_id.outpost();
        let asker = Asker::Text { query_id, trace_id };
        // A caret watch waits for the application, possibly until its bound
        // when the key moved nothing: it does not keep Verbatim from being
        // idle.
        let id = if matches!(op, verbatim_model::TextOp::AwaitCaret(_)) {
            self.requests.begin_passive(outpost, asker)
        } else if op == verbatim_model::TextOp::ReadCaret {
            // A held key waits for this read; past the bound it is passed
            // on without it (`docs/design/key-holding.md`).
            self.requests
                .begin_until(outpost, asker, Instant::now() + HELD_KEY_BOUND)
        } else {
            self.requests.begin(outpost, asker)
        };
        let command = CoreToOutpost::Query {
            trace_id,
            request_id: id.0,
            query: Query::Text { node_id, op },
            timing: EventTiming::default(),
        };
        self.send(outpost, id, command);
    }

    /// Sends request `id` to `outpost`, failing it at once when it cannot be
    /// sent, so it still gets its one outcome.
    fn send(&mut self, outpost: OutpostId, id: RequestId, command: CoreToOutpost) {
        if let Err(error) = self.context.supervisor.send_to_outpost(outpost, command) {
            tracing::warn!(%error, %outpost, "a query could not reach its outpost");
            self.finish(id, outpost, QueryOutcome::Failed(error.to_string()));
        }
    }

    /// Starts a control-plane tree dump of the application holding attention,
    /// each element's location read only with `locations`. The answer goes
    /// to `reply`; the requester waits on it with its own timeout, and a
    /// late answer to a request it gave up on is dropped.
    fn dump_tree(&mut self, ticket: DumpTicket, locations: bool, reply: Sender<DumpTreeResult>) {
        let Some(pid) = self.state.attention() else {
            let _ = reply.try_send(Err("no application holds attention yet".to_owned()));
            return;
        };
        let Some(outpost) = self.live.newest(pid) else {
            let _ = reply.try_send(Err(format!("no outpost is watching pid {pid}")));
            return;
        };
        let id = self
            .requests
            .begin(outpost, Asker::DumpTree { ticket, reply });
        let command = CoreToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id: id.0,
            query: Query::DumpTree { locations },
            timing: EventTiming::default(),
        };
        self.send(outpost, id, command);
    }

    /// Asks the outpost of the process that owns the taskbar `ask` of the
    /// shell's surface `kind`, its items or where a clicked item is,
    /// starting one when there is none, so verbatim.exe makes no UIA
    /// connection into the shell's windows
    /// (`docs/design/listener-without-connections.md`, decision 9). The
    /// outcome goes to `ask`'s callback, exactly once; `None` when there is no
    /// taskbar, when its process gets no outpost or none is ready by
    /// [`shell_requests::SHELL_ITEMS_DEADLINE`] from now, or when no answer
    /// has come by that long after the query was sent.
    fn shell_items(&mut self, kind: ShellItemKind, ask: verbatim_gui::ShellAsk) {
        let Some(pid) = taskbar_process() else {
            tracing::warn!(?kind, "the taskbar window was not found");
            ask.answer_nothing();
            return;
        };
        let deadline = Instant::now() + shell_requests::SHELL_ITEMS_DEADLINE;
        if let Some(outpost) = self.live.ready(pid) {
            self.ask_shell_items(outpost, kind, ask);
        } else {
            self.shell_items_wanted.push(pid, kind, ask, deadline);
            self.context.supervisor.ensure_spawned(pid);
        }
    }

    /// Sends `outpost` the shell items query or the position query `ask`
    /// says, answered with nothing by Core if no outcome has come
    /// [`shell_requests::SHELL_ITEMS_DEADLINE`] from now, after the
    /// outpost's own deadline for the walk has run out.
    fn ask_shell_items(
        &mut self,
        outpost: OutpostId,
        kind: ShellItemKind,
        ask: verbatim_gui::ShellAsk,
    ) {
        let deadline = Instant::now() + shell_requests::SHELL_ITEMS_DEADLINE;
        let (asker, query) = match ask {
            verbatim_gui::ShellAsk::Items(deliver) => {
                (Asker::ShellItems { deliver }, Query::ShellItems { kind })
            }
            verbatim_gui::ShellAsk::Position(item, deliver) => (
                Asker::ShellItemPosition { deliver },
                Query::ShellItemPosition { kind, item },
            ),
        };
        let id = self.requests.begin_until(outpost, asker, deadline);
        let command = CoreToOutpost::Query {
            trace_id: TraceId::mint(),
            request_id: id.0,
            query,
            timing: EventTiming::default(),
        };
        self.send(outpost, id, command);
    }

    /// Handles the end of `outpost`, which watched `target_pid`, for
    /// `reason`.
    fn on_outpost_ended(&mut self, outpost: OutpostId, target_pid: Pid, reason: EndReason) {
        tracing::info!(%outpost, %target_pid, %reason, "outpost ended");
        // Everything it sent is ahead of its end: in the idle
        // barrier's round, its end is its answer.
        self.barrier.ended(outpost);
        // An outpost that failed, crashed or killed, is replaced by
        // the supervisor whatever application it served, at once or
        // after its backoff (`docs/design/focus-pipeline.md`,
        // section 19.5). For the attention application, or the
        // focus's when that is another (a Settings page's content,
        // inside ApplicationFrameHost's window), the replacement is
        // asked for the focus once it is ready, which is taken
        // silently if the user already heard it; the reducer's own
        // view of attention decides, rather than the supervisor's,
        // which follows it by a message. A retired outpost, or one
        // whose application exited, is not replaced. A console
        // host's outpost reports for the console's client. A given
        // spare that failed before its `Ready` loses nothing: its
        // application's held facts went to the next generation,
        // which reports them, so nothing is asked.
        let regenerated = self.regenerated.remove(&outpost);
        let replaced =
            !regenerated && !matches!(reason, EndReason::Retired | EndReason::TargetExited);
        let application = self.live.application_of(outpost).unwrap_or(target_pid);
        if replaced
            && (self.state.attention() == Some(application)
                || self.state.focus_source() == Some(application))
        {
            self.focus_now_wanted.insert(target_pid, application);
        }
        if self.live.ended(outpost) {
            self.context
                .outposts
                .lock()
                .expect("outposts lock")
                .remove(&target_pid);
        }
        if !regenerated && !replaced {
            self.no_outpost_follows(target_pid);
        }
        self.apply(Input::OutpostEnded { outpost });
        for input in self.requests.outpost_ended(outpost) {
            self.apply(input);
        }
        if let Some(server) = self.context.server_slot.get() {
            server.broadcast_outpost_ended(target_pid, reason.to_string());
        }
    }

    /// Answers the shell items requests waiting for the outpost of `pid`,
    /// whose outpost ended with no replacement, its application exited or
    /// the outpost retired, with nothing, unless another of its outposts is
    /// starting.
    fn no_outpost_follows(&mut self, pid: Pid) {
        if self.live.newest(pid).is_none() {
            self.shell_items_wanted.refuse(pid);
        }
    }

    /// The earliest time a shell items request is answered with nothing
    /// if it has had no outcome by then, a request with a deadline is
    /// ended, a held key's wait for the key ahead of it ends, the focused
    /// terminal's quiet-period timer is due, or a lock key's state is read.
    fn next_deadline(&self) -> Option<Instant> {
        self.shell_items_wanted
            .next_deadline()
            .into_iter()
            .chain(self.requests.next_deadline())
            .chain(self.held_wait.map(|(_, due)| due))
            .chain(self.terminal_timer.map(|(_, due)| due))
            .chain(self.toggle_reads.front().map(|&(_, due)| due))
            .min()
    }

    /// Answers every shell items request whose deadline is `now` or
    /// earlier, still waiting for its outpost or for the outpost's answer,
    /// with nothing, ends every request whose deadline is past, and ends a
    /// held key's wait for the key ahead of it once its bound is past.
    fn expire(&mut self, now: Instant) {
        self.shell_items_wanted.expire(now);
        for input in self.requests.expire(now) {
            self.apply(input);
        }
        if let Some((trace_id, _)) = self.held_wait.take_if(|(_, due)| *due <= now) {
            self.apply(Input::HeldKeyWaitEnded { trace_id });
        }
        if let Some((at_ms, _)) = self.terminal_timer.take_if(|(_, due)| *due <= now) {
            self.apply(Input::TerminalTimer {
                trace_id: TraceId::mint(),
                at_ms,
            });
        }
        while let Some(&(toggle, due)) = self.toggle_reads.front()
            && due <= now
        {
            self.toggle_reads.pop_front();
            self.apply(read_toggle_key(toggle));
        }
    }

    /// Withdraws the tree dump named `ticket`, whose caller stopped
    /// waiting. The outpost answers "not started" if the dump was still
    /// queued; a dump already running finishes, within its own walk budget.
    /// Either way the request keeps its one outcome, which goes nowhere.
    fn dump_tree_given_up(&mut self, ticket: DumpTicket) {
        let Some((id, outpost)) = self.requests.dump_tree_request(ticket) else {
            return;
        };
        let command = CoreToOutpost::Cancel { request_id: id.0 };
        if let Err(error) = self.context.supervisor.send_to_outpost(outpost, command) {
            // The outpost is going away; its end answers the request.
            tracing::debug!(%error, %outpost, "a tree dump could not be withdrawn");
        }
    }
}

/// Posts the supervisor's messages to the reducer thread's port, in line
/// with its keys and commands, so the reducer takes them all in the order
/// they arrived (`docs/design/communication.md`, section 4.5); and lets the
/// supervisor's relay watch the reducer through the port's counts, logging
/// a stall it reports (section 6.8).
fn outpost_sink(command_tx: Commands) -> verbatim_outpost::EventSink {
    let progress = command_tx.clone();
    verbatim_outpost::EventSink::new(move |message| {
        match command_tx.send(ShellCommand::Outpost(message)) {
            Ok(()) => Ok(()),
            // The port refused the packet; the queue is still taken.
            Err(ShellCommand::Outpost(message)) if command_tx.is_open() => {
                Err(verbatim_outpost::Undelivered::Refused(message))
            }
            Err(_) => Err(verbatim_outpost::Undelivered::Gone),
        }
    })
    .watched(
        move || progress.progress(),
        |stall| {
            tracing::warn!(
                stalled_for = ?stall.stalled_for,
                waiting = stall.waiting,
                "the reducer has taken none of its inputs for three ping intervals while some waited; no outpost is ended for it"
            );
        },
    )
}

/// The reducer thread body: takes outpost messages and commands from other
/// threads off its port, in the order they arrived, feeds the pure
/// reducer, and executes its effects, so a key, a review or
/// object-navigation gesture, and an accessibility event are each handled
/// by the same reduce-and-execute step, in turn.
fn reducer_loop(
    mut command_rx: PortQueue<ShellCommand>,
    context: &ReducerContext,
    settings: ReaderSettings,
    token: SpeakToken,
) -> SpeakToken {
    let mut thread = ReducerThread {
        context,
        state: SrState::new(),
        requests: RequestTable::default(),
        live: LiveOutposts::default(),
        focus_now_wanted: HashMap::new(),
        shell_items_wanted: shell_requests::ShellItemsWanted::default(),
        views: (None, BTreeSet::new()),
        focus_reported: BTreeSet::new(),
        handled_input: 0,
        frames: context.supervisor.frames(),
        barrier: idle::IdleBarrier::default(),
        regenerated: HashSet::new(),
        exiting: false,
        exit_sound: None,
        held_keys: held_keys::HeldKeys::default(),
        held_wait: None,
        terminal_timer: None,
        toggle_reads: VecDeque::new(),
        marks: idle::HeldMarks::default(),
        follow_ups: Vec::new(),
        token,
    };
    // The settings come first, before anything they govern.
    thread.apply(Input::Settings(settings));
    thread.send_views();
    loop {
        // With no deadline outstanding, or none yet passed, nothing is
        // expired: a command costs no scan of the requests.
        let due = thread.next_deadline();
        let next = next_command(&mut command_rx, due);
        let now = Instant::now();
        if due.is_some_and(|due| now >= due) {
            thread.expire(now);
        }
        match next {
            Next::Command(command) => thread.on_command(command),
            Next::Woken => {}
            Next::Closed => break,
        }
        thread.send_views();
        thread.advance_idle_barrier(&mut command_rx);
    }
    thread.token
}

/// What the reducer thread's wait for its next command came to.
enum Next {
    /// A command to handle.
    Command(ShellCommand),
    /// No command came before the wait ended: a deadline is due, or nearly
    /// so, and the thread checks its deadlines and waits again.
    Woken,
    /// Every poster has gone: the thread ends.
    Closed,
}

/// Waits for the reducer thread's next command, or until `due`, the
/// earliest shell items request's deadline, when one is outstanding. Only
/// the queue's end ends the thread: a wait that ran out, even a clock tick
/// before `due`, as a system wait may, wakes it to check its deadlines.
fn next_command(command_rx: &mut PortQueue<ShellCommand>, due: Option<Instant>) -> Next {
    match due {
        Some(due) => {
            next_of(command_rx.recv_timeout(due.saturating_duration_since(Instant::now())))
        }
        None => command_rx.recv().map_or(Next::Closed, Next::Command),
    }
}

/// What a wait for a deadline came to, from what the queue gave: a wait
/// that ran out wakes the thread whenever it ran out, before the deadline
/// included, so nothing here compares the time with the deadline.
fn next_of(received: Received<ShellCommand>) -> Next {
    match received {
        Received::Item(command) => Next::Command(command),
        Received::TimedOut => Next::Woken,
        Received::Closed => Next::Closed,
    }
}

/// The keyboard bindings this milestone ships, as the shared gesture map the
/// hook consults and the control plane validates against: the menu gesture
/// plus every review and object-navigation binding for the active layout
/// (roadmap M3). The layout's own table already carries the time and tray
/// gestures, so they are not listed separately. The caret keys are observed,
/// not bound: they reach the application, and the hook reports them for the
/// reducer to speak what they did (milestone M4).
fn bound_gestures(layout: KeyboardLayout) -> SharedGestureMap {
    let mut gestures = vec![GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding")];
    gestures.extend(
        verbatim_input::bindings_for(layout)
            .into_iter()
            .map(|(gesture, _action)| gesture),
    );
    GestureMap::new(gestures)
        .with_observed(
            verbatim_input::caret_bindings()
                .into_iter()
                .map(|(gesture, _key)| gesture)
                .chain(verbatim_input::clearing_keys()),
        )
        .into_shared()
}

/// Sends one command to the GUI when it is up; during the startup window
/// before `run_gui`'s ready callback fires, the command is dropped with a
/// warning (there is no GUI to act on it yet). Never waits.
fn send_gui_command(gui_handle: &Arc<OnceLock<GuiHandle>>, command: GuiCommand) {
    if let Some(handle) = gui_handle.get() {
        handle.send(command);
    } else {
        tracing::warn!(?command, "gesture before the GUI was ready; dropped");
    }
}

/// A lock key's state, read once Windows has changed it, for the reducer to
/// announce ("caps lock on"): its toggle bit, as `GetKeyState` reads it on
/// this thread, which reads no keyboard messages, as the hook thread reads
/// Num Lock's before each key.
fn read_toggle_key(toggle: LockKey) -> Input {
    let vk = verbatim_input::ToggleKey::of_lock_key(toggle).vk();
    // SAFETY: GetKeyState takes a virtual-key code and reads key state.
    let on =
        unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(i32::from(vk)) } & 1 != 0;
    Input::LockKeyToggled {
        trace_id: TraceId::mint(),
        key: toggle,
        on,
    }
}

/// The localized current time and date, read for the time gesture, whose
/// effect ran under `trace_id`, for the reducer to speak the time on a
/// first press and the date on a quick second (`repeat`).
fn read_clock(trace_id: TraceId, repeat: u8) -> Input {
    let time = datetime::local_time();
    let date = datetime::local_date();
    if time.is_none() || date.is_none() {
        tracing::warn!(
            time = time.is_some(),
            date = date.is_some(),
            "the system time or date could not be formatted"
        );
    }
    Input::ClockRead {
        trace_id,
        repeat,
        time,
        date,
    }
}

/// The GUI event thread body: the Exit item asks the app to shut down; the
/// app tells the GUI to tear down, which ends the main thread's loop. A
/// shell item that was gone, or had no point to click, when the list dialog
/// would have clicked it is told to the reducer, which says so.
fn gui_event_loop(
    gui_event_rx: &Receiver<GuiEvent>,
    gui_handle: &Arc<OnceLock<GuiHandle>>,
    command_tx: &Commands,
) {
    while let Ok(event) = gui_event_rx.recv() {
        match event {
            GuiEvent::QuitRequested => {
                tracing::info!("quit requested");
                request_shutdown(gui_handle);
            }
            GuiEvent::ShellItemGone(name) => {
                let input = Input::ShellItemGone {
                    trace_id: TraceId::mint(),
                    name,
                };
                let _ = command_tx.send(ShellCommand::Input(Box::new(input)));
            }
            GuiEvent::ShellItemUnclickable(name) => {
                let input = Input::ShellItemUnclickable {
                    trace_id: TraceId::mint(),
                    name,
                };
                let _ = command_tx.send(ShellCommand::Input(Box::new(input)));
            }
        }
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
    /// Injects gestures and idle waiters through the keyboard hook's
    /// thread, which numbers them in line with the keys.
    injector: Injector,
    gui_handle: Arc<OnceLock<GuiHandle>>,
    command_tx: Commands,
    recorder: SharedRecorder,
    dumps_dir: PathBuf,
    readiness: Arc<harness::Readiness>,
    idle_line: Arc<harness::IdleLine>,
}

/// Builds the control-plane handlers over the app's live pieces.
fn control_handlers(config: ControlHandlersConfig) -> ServerHandlers {
    let ControlHandlersConfig {
        own_pid,
        settings_host,
        outposts,
        ledger,
        bound_gestures,
        injector,
        gui_handle,
        command_tx,
        recorder,
        dumps_dir,
        readiness,
        idle_line,
    } = config;
    let focus_tx = command_tx.clone();
    let idle_injector = injector.clone();
    ServerHandlers {
        status: Box::new(move || {
            let outposts: Vec<OutpostStatus> = outposts
                .lock()
                .expect("outposts lock")
                .values()
                .cloned()
                .collect();
            let ready = readiness.is_ready();
            StatusInfo {
                pid: Pid(own_pid),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                active_synth: Some(settings_host.active_synthesizer().id.0),
                outposts,
                ready,
            }
        }),
        send_gesture: Box::new(move |identifier| {
            let gesture = GestureId::parse(identifier).map_err(|error| error.to_string())?;
            if verbatim_input::ToggleKey::of_gesture(&gesture).is_some() {
                // Its announcement reports the key's state after the key
                // reached Windows; injecting the gesture alone would report
                // the state unchanged.
                return Err(format!(
                    "gesture {gesture} follows a real lock key; send the key with SendKeys"
                ));
            }
            if !bound_gestures.load().contains(&gesture) {
                return Err(format!("gesture {gesture} is not bound"));
            }
            // Its keys never pass the hook, so the hook's thread is handed
            // it: it numbers it in line with the keys and cancels speech
            // through that number, as for a key press; executing a gesture
            // cancels speech in NVDA too, and stops say-all.
            injector
                .inject(Injected::Gesture(gesture))
                .map_err(|_| "the keyboard hook is gone".to_owned())
        }),
        latency: Box::new(move |last_n| ledger.recent(last_n)),
        dump_tree: Box::new(move |locations| request_dump_tree(&command_tx, locations)),
        dump_recorder: Box::new(move || {
            flight_dump::dump_now(&recorder, &dumps_dir)
                .map(|path| path.display().to_string())
                .map_err(|error| error.to_string())
        }),
        quit: Box::new(move || request_shutdown(&gui_handle)),
        await_idle: Box::new(move |after_input, timeout| {
            idle_line.wait(&idle_injector, after_input, timeout)
        }),
        dump_focus: Box::new(move || {
            let (reply_tx, reply_rx) = bounded(1);
            focus_tx
                .send(ShellCommand::DumpFocus(reply_tx))
                .map_err(|_| "the reducer is gone".to_owned())?;
            reply_rx
                .recv_timeout(DUMP_FOCUS_TIMEOUT)
                .map_err(|_| format!("the reducer did not answer within {DUMP_FOCUS_TIMEOUT:?}"))
        }),
    }
}

/// The longest the control plane waits for the reducer's focus report: the
/// reducer answers between steps, so only a stalled reducer takes this long.
const DUMP_FOCUS_TIMEOUT: Duration = Duration::from_secs(5);

/// Answers [`Request::DumpTree`](verbatim_control::protocol::Request::DumpTree):
/// hands the request to the reducer thread, which sends `DumpTree` to the
/// outpost of the application holding attention and records it in its
/// request table, then waits for the answer with a timeout, withdrawing the
/// dump when it expires. Every request has its own id, so a late answer to
/// one that timed out can never satisfy a newer one.
fn request_dump_tree(command_tx: &Commands, locations: bool) -> DumpTreeResult {
    let ticket = DumpTicket::mint();
    let (reply_tx, reply_rx) = bounded(1);
    command_tx
        .send(ShellCommand::DumpTree(ticket, locations, reply_tx))
        .map_err(|_| "the reducer thread is gone".to_owned())?;
    reply_rx
        .recv_timeout(DUMP_TREE_TIMEOUT)
        .unwrap_or_else(|_| {
            // Nobody will read a later answer, so the outpost need not work on it.
            let _ = command_tx.send(ShellCommand::DumpTreeGivenUp(ticket));
            Err("tree dump timed out".to_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wait for a deadline that runs out with no command wakes the
    /// reducer thread to check its deadlines rather than ending it, however
    /// the time compares with the deadline, a wait that ends before it
    /// included; only the queue's end ends it.
    #[test]
    fn a_wait_that_runs_out_wakes_the_reducer_thread_and_only_the_end_stops_it() {
        assert!(matches!(next_of(Received::TimedOut), Next::Woken));
        let (command_tx, mut command_rx) = port_queue().expect("a port");
        let due = Instant::now();
        drop(command_tx);
        assert!(matches!(
            next_command(&mut command_rx, Some(due)),
            Next::Closed
        ));
    }

    /// The supervisor's messages and the reducer's other inputs share one
    /// port, so the reducer takes them in the order they arrived: a key
    /// handled between two outpost messages comes out between them.
    #[test]
    fn outpost_messages_and_commands_reach_the_reducer_in_arrival_order() {
        let (command_tx, mut command_rx) = port_queue().expect("a port");
        let sink = outpost_sink(command_tx.clone());
        assert!(
            sink.send(OutpostMessage::NotWatched { target_pid: Pid(1) })
                .is_ok()
        );
        command_tx
            .send(ShellCommand::InputHandled(7))
            .map_err(|_| "refused")
            .expect("the receiver is held");
        assert!(
            sink.send(OutpostMessage::NotWatched { target_pid: Pid(2) })
                .is_ok()
        );
        let arrived: Vec<String> = std::iter::from_fn(|| command_rx.try_recv())
            .map(|command| match command {
                ShellCommand::Outpost(OutpostMessage::NotWatched { target_pid }) => {
                    format!("outpost message for {}", target_pid.0)
                }
                ShellCommand::InputHandled(input) => format!("key {input}"),
                _ => "something else".to_owned(),
            })
            .collect();
        assert_eq!(
            arrived,
            ["outpost message for 1", "key 7", "outpost message for 2"]
        );
    }
}
