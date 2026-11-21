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
mod hang_check;
mod harness;
mod idle;
mod key_reports;
mod latency;
mod live;
mod requests;
mod saver;
mod screen_reader_flag;
mod session_lock;
mod single_instance;
mod speech_events;
mod speech_settings;
mod strokes;
mod terminal_settings;
mod themes;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use verbatim_audio::{AudioDevice, Lane, MixerBuilder, SilentDevice, WavRecorder};
use verbatim_audio_wasapi::WasapiDevice;
use verbatim_config::ConfigStore;
use verbatim_control::protocol::{FocusReport, OutpostState, OutpostStatus, StatusInfo};
use verbatim_control::server::{ControlServer, ServerHandlers};
use verbatim_core::{ReducerRecorder, SrState, reduce};
use verbatim_gui::{GuiCommand, GuiEvent, GuiHandle, ShellItemKind, run_gui};
use verbatim_input::DecisionConfig;
use verbatim_input_windows::{ForegroundView, InputHook};
use verbatim_model::protocol::{CoreToListener, ListenerToCore};
use verbatim_model::protocol::{CoreToOutpost, EventTiming, OutpostToCore};
use verbatim_model::request::{
    Answer, ForegroundReport, Instance, Outcome, ProcessFact, QueueEffect, QueueInput, Reader,
};
use verbatim_model::{
    Earcon, Effect, GestureId, GuiRequest, Input, LockKey, OutpostId, Pid, ReaderSettings, TraceId,
    WindowHandle,
};
use verbatim_process::arguments::OutpostOptions;
use verbatim_process::clock::now_us;
use verbatim_process::port_queue::{PortQueue, Poster, Received, port_queue};
use verbatim_speech::hosting::synth_ids;
use verbatim_speech::{
    QueueItem, SpeakToken, SpeechInbox, SpeechInboxTaker, SpeechManager, SpeechManagerConfig,
    SpeechSettingsHost, SynthId, SynthRegistry,
};
use verbatim_supervisor::{EndReason, Frames};
use verbatim_supervisor::{OutpostMessage, Supervisor};
use verbatim_synth_capture::CaptureSynth;

use latency::LatencyLedger;
use live::LiveOutposts;
use requests::{Asker, DumpTicket, DumpTreeResult};

/// The one binding not carried by the keyboard layout's own script table:
/// Verbatim+V opens the menu. The review, object-navigation, time, and
/// tray-list bindings all come from `verbatim_input::bindings_for` for the
/// active layout (roadmap M3).
pub(crate) const SHOW_MENU_GESTURE: &str = "kb:verbatim+v";

/// How long a `DumpTree` control-plane request waits for the outpost's
/// answer before giving up: the longest program that can be in flight
/// ahead of the dump
/// ([`FOCUS_READ_DEADLINE`](verbatim_model::FOCUS_READ_DEADLINE)), the
/// dump itself ([`HANDLING_DEADLINE`](verbatim_model::HANDLING_DEADLINE))
/// and a second for the answer's way back, 25 seconds. The dump can also
/// wait behind entries already queued, which no limit covers: it is a
/// developer's tool.
const DUMP_TREE_TIMEOUT: Duration = verbatim_model::FOCUS_READ_DEADLINE
    .saturating_add(verbatim_model::HANDLING_DEADLINE)
    .saturating_add(Duration::from_secs(1));

// The dump's wait holds the longest program ahead of it and the dump's own.
const _: () = assert!(
    DUMP_TREE_TIMEOUT.as_micros()
        > verbatim_model::FOCUS_READ_DEADLINE.as_micros()
            + verbatim_model::HANDLING_DEADLINE.as_micros()
);

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
    let (config, config_failed) = load_config(&exe_dir);
    init_tracing(config.settings().log_filter.as_deref());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "verbatim starting");
    // A warning, not an error, so the error sound does not play before the
    // message box says what happened.
    if let Some((error, kept)) = &config_failed {
        match kept {
            Ok(()) => {
                tracing::warn!(%error, "the settings file could not be loaded; it is kept as settings.toml.corrupted.bak and the defaults are used");
            }
            Err(rename) => {
                tracing::warn!(%error, %rename, "the settings file could not be loaded nor kept; the defaults are used and the first saved change overwrites it");
            }
        }
    }

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

    match run(config, config_failed.is_some(), &instance) {
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
    config_failed: bool,
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
    // the store. File-only, so no live re-read is needed.
    let keyboard_layout = config.settings().keyboard.layout;
    // How every outpost reads UIA, fixed for their lives: a developer
    // setting, read once like the layout.
    let outpost_options = OutpostOptions {
        remote_operations: config.settings().uia.remote_operations,
    };

    // The reader settings the reducer reads, handed to it first.
    let reader_settings = config.settings().reader;
    // The hook's configuration, read before the saver owns the file.
    let decision_config = decision_config(&config);

    // The reducer thread's port: every outpost's frames and the
    // supervisor's notices (through the supervisor's sink, below), the
    // keys, gestures and typing the keyboard hook reports, in the order it
    // decided them, the index marks speech reaches, GUI events
    // and control-plane commands, each one packet posted to it. The port is
    // first in, first out across all its posters, so the reducer takes its
    // inputs in the order they reached it, as NVDA queues keys with events
    // (`docs/design/communication.md`, section 4.5). A post never waits.
    let (command_tx, command_rx) = port_queue::<ShellCommand>()?;

    // The theme the configuration names, with its sounds, presents speech
    // from the first word and decides what Core's reads fetch; the theme
    // panel switches it.
    let theme = {
        let (loaded, options) = themes::configured(&config);
        themes::prepare(&loaded, &config.sounds_dir(), options)
    };
    themes::send_reported(&command_tx, &theme);
    let initial_synth = initial_synth(&config);
    let (themes_dir, sounds_dir) = (config.themes_dir(), config.sounds_dir());

    // The saver owns `settings.toml` from here on, and each section of it
    // moves to its one owner (`books/dev/src/explanation/design/ownership.md`): the speech section to the synth
    // thread, the reader section to the reducer thread, the theme section
    // to the GUI thread.
    let (saver, speech_writer, reader_writer, theme_writer) =
        saver::Saver::start(saver::DiskFile(exe_dir()), config.settings().clone())?;
    let theme_section = config.settings().theme.clone();
    let speech_settings =
        speech_settings::SpeechSettings::new(config.settings().speech.clone(), speech_writer);
    drop(config);

    // Speech pipeline: eSpeak NG through WASAPI by default, observed by the
    // latency ledger and reporting the marks it reaches to the reducer;
    // VERBATIM_TEST_AUDIO=null swaps in device-free test audio (see
    // build_speech_manager). The synth thread reads each synthesizer's
    // saved settings from its speech section whenever it starts one.
    let speech_events = Arc::new(speech_events::ShellSpeechEvents {
        ledger: Arc::clone(&ledger),
        commands: command_tx.clone(),
    });
    let (manager, speak_token, gui_sounds) =
        build_speech_manager(speech_settings, initial_synth, speech_events, theme)?;
    // Errors logged from here on play the error sound, through the reducer
    // thread.
    error_sound::report_to(command_tx.clone());

    // Settings host: the GUI's live handle; a commit saves on the synth
    // thread, through the speech section's writer.
    let settings_host = manager.settings_host();
    // The reducer's reader settings, as it publishes them for the Terminal
    // page.
    let reader_view = Arc::new(terminal_settings::ReaderView::new(reader_settings));

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
    let ignored = Arc::new(verbatim_process::held::IgnoredProcesses::from_env());
    let ignored_pids = ignored.pids();
    let supervisor = Arc::new(Supervisor::new(
        outpost_sink(command_tx.clone()),
        outpost_options,
        ignored,
    )?);
    let outposts: Arc<Mutex<HashMap<Pid, OutpostStatus>>> = Arc::new(Mutex::new(HashMap::new()));
    // The GUI's handle, once the GUI thread is up; part of readiness.
    let gui_handle: Arc<OnceLock<GuiHandle>> = Arc::new(OnceLock::new());
    let readiness = Arc::new(harness::Readiness::new(Arc::clone(&gui_handle)));
    let idle_line = Arc::new(harness::IdleLine::default());

    // Core's generation record takes the harness's ignored processes, and
    // starts Verbatim's own outpost at once, before the
    // first gesture can come: a cold outpost races a real keypress sent
    // right after Verbatim's menu opens.
    for fact in [
        QueueInput::Ignored(ignored_pids),
        QueueInput::Process(ProcessFact::OwnProcess(Pid(own_pid))),
    ] {
        let _ = command_tx.send(ShellCommand::Input(Box::new(Input::Queue {
            trace_id: TraceId::mint(),
            at_us: now_us(),
            input: fact,
        })));
    }
    // The session's lock state: queried now, then followed by Windows'
    // session notifications (Table 8).
    session_lock::start(command_tx.clone())?;

    // Core's view of the window in front, for the keyboard hook: whether
    // it may hold keys there, whether the focus has text whose caret keys
    // it holds (`books/dev/src/explanation/design/key-holding.md`), and which thread's layout
    // translates them, published by the reducer thread (architecture
    // section 5).
    let foreground = Arc::new(ForegroundView::new());

    // The reducer thread: normalized events and review commands in, speech,
    // fetches, text requests, activations, and clipboard copies out.
    {
        let context = ReducerContext {
            foreground: Arc::clone(&foreground),
            clipboard: clipboard::Clipboard::start(command_tx.clone())?,
            manager: Arc::clone(&manager),
            supervisor: Arc::clone(&supervisor),
            ledger: Arc::clone(&ledger),
            server_slot: Arc::clone(&server_slot),
            outposts: Arc::clone(&outposts),
            recorder: Arc::clone(&recorder),
            reader: terminal_settings::ReaderSection {
                writer: reader_writer,
                view: Arc::clone(&reader_view),
            },
            idle_line: Arc::clone(&idle_line),
            gui_handle: Arc::clone(&gui_handle),
        };
        let core_readiness = Arc::clone(&readiness);
        // The only thread that speaks: it holds the speaking token, which
        // the loop hands back when it ends. It runs at `Highest`, so a
        // message into Core is not starved by the readers' and the speech
        // side's normal threads (Dickson, 2026-11-14; ADR 0003's build 1).
        thread::Builder::new()
            .name("verbatim-reducer".to_owned())
            .spawn(move || {
                verbatim_process::scheduling::prioritize_role(
                    "the reducer",
                    verbatim_process::scheduling::ThreadPriority::Highest,
                );
                let _token = reducer_loop(
                    command_rx,
                    &context,
                    core_readiness,
                    reader_settings,
                    speak_token,
                );
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

    // The one gesture table, from which the gesture map the hook decides
    // keys against is built.
    let tables = verbatim_input::Tables::for_layout(
        keyboard_layout,
        GestureId::parse(SHOW_MENU_GESTURE).expect("valid binding"),
    );

    // Keyboard hook. Everything it decides goes to the reducer's port, and
    // a key that cancels speech to the speech queue's, both made before
    // it, through the callbacks in `key_reports`, in the order the keys
    // were pressed; a key that cancels speech also tells the reducer,
    // whose say-all stops.
    let hook_ports = key_reports::Ports::new(command_tx.clone(), manager.control());
    let hook = InputHook::start(
        decision_config,
        tables.gesture_map(),
        key_reports::speech_effect(hook_ports.clone()),
        key_reports::reports(tables, Arc::clone(&ledger), hook_ports),
        foreground,
    )?;

    // Control plane.
    let server = ControlServer::start(control_handlers(ControlHandlersConfig {
        own_pid,
        settings_host: settings_host.clone(),
        outposts: Arc::clone(&outposts),
        ledger: Arc::clone(&ledger),
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

    // The GUI loop owns the main thread until shutdown.
    let host_for_gui: Arc<dyn SpeechSettingsHost> = Arc::new(settings_host);
    let theme_host: Arc<dyn verbatim_gui::ThemeHost> = Arc::new(themes::AppThemeHost {
        themes_dir,
        sounds_dir,
        section: Mutex::new((theme_section, theme_writer)),
        sounds: Mutex::new(gui_sounds),
        manager: Arc::clone(&manager),
        commands: command_tx.clone(),
    });
    let terminal_host: Arc<dyn verbatim_gui::TerminalHost> =
        Arc::new(terminal_settings::AppTerminalHost {
            view: reader_view,
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
            // NVDA's box for a configuration file that failed to load,
            // once the GUI is up.
            if config_failed {
                handle.send(GuiCommand::ShowMessage {
                    title: verbatim_i18n::t!("config-error-title"),
                    text: verbatim_i18n::t!("config-error-text"),
                });
            }
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
    // Every save made before Exit reaches the file: the speech pipeline
    // settles first, speech cancelled so a commit does not wait behind an
    // utterance, then the saver writes what it was given, the synth
    // thread's commit included, both within one bound
    // (`books/dev/src/explanation/design/ownership.md`).
    let deadline = Instant::now() + EXIT_SAVE_TIMEOUT;
    if !manager.settle(deadline) {
        tracing::warn!(
            timeout = ?EXIT_SAVE_TIMEOUT,
            "the synthesizer did not answer in time; a settings change made just before exiting may not be saved"
        );
    }
    if !saver.finish(deadline) {
        tracing::warn!(
            timeout = ?EXIT_SAVE_TIMEOUT,
            "the settings were not all saved in time"
        );
    }
    if shutdown.join().is_err() {
        tracing::error!("shutting down the outposts failed; their jobs end them");
    }
    Ok(())
}

/// The longest Verbatim waits for its exit sound to be heard before it
/// exits on any quit but being replaced: NVDA's exit sound plays for about
/// half a second.
const EXIT_SOUND_TIMEOUT: Duration = Duration::from_secs(2);

/// The longest Verbatim waits at exit, after the exit sound, for the speech
/// pipeline to settle and the saver to write the settings, the two sharing
/// it.
const EXIT_SAVE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Loads the configuration. A settings file that fails to load is renamed
/// to `settings.toml.corrupted.bak`, replacing an older one, and the
/// defaults are used, as NVDA keeps a bad configuration file
/// (`docs/parity/system.md`, "A settings file that fails to load"): the
/// failure is given back, with how the rename went, for the log and the
/// message box once they exist. Missing files are made with the values in
/// use, so the settings are discoverable and hand-editable from the first
/// run.
fn load_config(
    exe_dir: &std::path::Path,
) -> (
    ConfigStore,
    Option<(verbatim_config::ConfigError, std::io::Result<()>)>,
) {
    let (config, failed) = match ConfigStore::load(exe_dir) {
        Ok(config) => (config, None),
        Err(error) => {
            let file = exe_dir.join(ConfigStore::SETTINGS_FILE);
            let backup = exe_dir.join(format!("{}.corrupted.bak", ConfigStore::SETTINGS_FILE));
            let kept = match std::fs::remove_file(&backup) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => std::fs::rename(&file, &backup),
            };
            let defaults = verbatim_config::Settings::default();
            (
                ConfigStore::from_settings(exe_dir, defaults),
                Some((error, kept)),
            )
        }
    };
    if let Err(error) = config.ensure_files_exist() {
        report_error!("verbatim: could not create default config files: {error}");
    }
    (config, failed)
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
/// test-only escape hatch (documented in books/dev/src/reference/crates/verbatim-audio.md) that
/// adds the capture synth and plays through [`SilentDevice`] instead, which
/// takes real time but makes no sound.
///
/// # Errors
///
/// Returns an error if no registered synthesizer can start, the audio
/// device cannot open, or the speech queue's port cannot be made.
fn build_speech_manager(
    settings: speech_settings::SpeechSettings,
    initial_synth: SynthId,
    events: Arc<dyn verbatim_speech::SpeechEvents>,
    theme: verbatim_speech::ActiveTheme,
) -> Result<(Arc<SpeechManager>, SpeakToken, Lane), verbatim_speech::SynthError> {
    let test_audio = std::env::var("VERBATIM_TEST_AUDIO").is_ok_and(|value| value == "null");
    let mut registry = SynthRegistry::new();
    // Every native synthesizer runs in a synthesizer host process next to
    // this executable (decision D18).
    // eSpeak NG is the default, so it is listed first.
    let host_exe = exe_dir().join(SYNTH_HOST_EXE);
    for (id, name) in [
        (synth_ids::ESPEAK, verbatim_i18n::t!("synth-name-espeak")),
        (synth_ids::ONECORE, verbatim_i18n::t!("synth-name-onecore")),
    ] {
        registry.register(
            SynthId::new(id),
            name,
            verbatim_synth_hosted::factory(host_exe.clone(), SynthId::new(id)),
        );
    }
    if test_audio {
        tracing::warn!(
            "VERBATIM_TEST_AUDIO=null: test audio mode is active; the capture synth is available and audio plays silently in real time"
        );
        register_test_audio(&mut registry);
    }
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
    let mut mixer = MixerBuilder::new(device);
    if let Some(recorder) = recorder {
        mixer.tap(Box::new(recorder));
    }
    // The mixer's delivery thread only moves messages, so it runs raised
    // (`docs/design/communication.md`, section 6.1).
    mixer.on_delivery_thread(Box::new(|| {
        verbatim_process::scheduling::prioritize_role(
            "the audio events delivery",
            verbatim_process::scheduling::ThreadPriority::Highest,
        );
    }));
    // The speech queue's inbox is a completion port, so the keyboard hook's
    // cancel takes no lock (`books/dev/src/explanation/design/ownership.md`).
    let (inbox, taker) = port_queue::<QueueItem>()
        .map_err(|error| verbatim_speech::SynthError::Unavailable(error.to_string()))?;
    let (manager, token, gui_sounds) = SpeechManager::new(SpeechManagerConfig {
        registry,
        initial_synth,
        settings: Box::new(settings),
        mixer,
        events,
        theme,
        inbox: (Arc::new(PortInbox(inbox)), Box::new(PortTaker(taker))),
    })?;
    Ok((Arc::new(manager), token, gui_sounds))
}

/// The speech queue's inbox as a completion port's posting end: one
/// `PostQueuedCompletionStatus`, which takes no lock and never waits.
struct PortInbox(Poster<QueueItem>);

impl SpeechInbox for PortInbox {
    fn post(&self, item: QueueItem) -> Result<(), QueueItem> {
        #[cfg(test)]
        if item
            .speaks()
            .is_some_and(|id| verbatim_faults::hit(verbatim_faults::Fault::SpeechPostRefused, &id))
        {
            return Err(item);
        }
        self.0.send(item)
    }

    fn is_open(&self) -> bool {
        self.0.is_open()
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
        .settings()
        .speech
        .synthesizer
        .as_deref()
        .map_or_else(|| SynthId::new(synth_ids::ESPEAK), SynthId::new)
}

/// The hook configuration from global settings: the Verbatim modifier keys
/// and NVDA's two speech interrupt settings.
fn decision_config(store: &ConfigStore) -> DecisionConfig {
    let keyboard = &store.settings().keyboard;
    DecisionConfig {
        keys: store.settings().verbatim_keys,
        interrupt_for_characters: keyboard.speech_interrupt_for_characters,
        interrupt_for_enter: keyboard.speech_interrupt_for_enter,
        ..DecisionConfig::default()
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

/// The list dialogs' way to the shell's items and a clicked item's
/// position: a request on the reducer thread's port, answered by the
/// taskbar process's outpost.
struct AppShellItemsHost {
    commands: Commands,
}

impl verbatim_gui::ShellItemsHost for AppShellItemsHost {
    fn request(&self, kind: ShellItemKind, ask: verbatim_gui::ShellAsk) {
        // A request the reducer thread no longer takes is dropped with the
        // command, and its responder answers "nothing".
        if self
            .commands
            .send(ShellCommand::ShellItems(kind, ask))
            .is_err()
        {
            tracing::debug!("the reducer thread is gone; the shell's items are not read");
        }
    }
}

/// The taskbar window (`Shell_TrayWnd`), which the shell items requests
/// name, so its process's outpost answers them; `None` when there is no
/// taskbar. Local calls.
fn taskbar_window() -> Option<isize> {
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
    Some(taskbar.0 as isize)
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

/// The reducer thread's dependencies, bundled to keep [`reducer_loop`]'s
/// signature from growing an argument for every subsystem it touches.
struct ReducerContext {
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
    /// The reader section of the settings, which this thread owns: reader
    /// settings changed by a toggle key or the Terminal page are published
    /// for the page and handed to the saver.
    reader: terminal_settings::ReaderSection,
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
    /// A control-plane tree dump, reading locations when the flag says.
    DumpTree(DumpTicket, bool, Sender<DumpTreeResult>),
    /// The control-plane caller waiting for this tree dump stopped waiting:
    /// withdraw the dump if its outpost has not started it.
    DumpTreeGivenUp(DumpTicket),
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
    /// A control-plane `AwaitIdle` waiter, held by the idle barrier until
    /// the harness input it names has been handled
    /// ([`ShellCommand::InputHandled`]) and a round finds everything idle.
    AwaitIdle(harness::IdleWaiter),
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
    /// The event queue's paused function, kept beside the state.
    runtime: verbatim_core::Runtime,
    /// The askers of Core's requests from outside the event queue, each by
    /// the token Core answers it under.
    outside: requests::Outside,
    /// The app's own facts of the outpost generations that have started
    /// and not yet ended.
    live: LiveOutposts,
    /// The running listener's count of its messages carrying tokens that
    /// Core handled, and the last view of the tokens Core holds sent to
    /// it ([`verbatim_core::ListenerView`]).
    listener_view: verbatim_core::ListenerView,

    /// The highest numbered harness key handled so far.
    handled_input: u64,
    /// Turns the outposts' frames, which the supervisor's relay posts
    /// unparsed, into the messages they carry, in each outpost's order.
    frames: Frames,
    /// When Core's event queue asked to be woken
    /// ([`QueueEffect::WakeAt`]), in microseconds on the run's shared
    /// clock: one of the reducer thread's deadlines.
    queue_due: Option<u64>,
    /// Control-plane `AwaitIdle` waiters that reached their place in line,
    /// and the idle barrier's round through every child that answers them
    /// (`docs/design/communication.md`, section 6.7).
    barrier: idle::IdleBarrier<Sender<()>>,
    /// The exit sound the reducer played once Verbatim was exiting
    /// ([`SrState::exiting()`]), for the main thread to wait on until it has
    /// been heard ([`ShellCommand::Exiting`]).
    exit_sound: Option<verbatim_speech::EarconHeard>,
    /// The harness's marks of handled input held behind a lock key's
    /// announcement still to come.
    marks: idle::HeldMarks,
    /// The number of the newest key press the hook reported, which a
    /// harness's mark that follows it waits on.
    last_press: Option<verbatim_model::KeySeq>,
    /// Inputs an effect produced, applied once the current input's effects
    /// have all been executed, so one input is reduced at a time.
    follow_ups: Vec<Input>,
    /// The right to speak, which only this thread holds.
    token: SpeakToken,
    /// What the control plane reports as readiness, and the harness's
    /// readiness event, given Core's parts after each step until Verbatim
    /// has been ready and dropped then, so later steps take no lock.
    readiness: Option<Arc<harness::Readiness>>,
}

impl ReducerThread<'_> {
    /// Feeds Core's event queue `input`, arriving now.
    fn queue(&mut self, input: QueueInput) {
        self.apply(queued(input));
    }

    /// Handles one message from the supervisor: the readers' tells and
    /// answers and the owner thread's process facts go to Core's event
    /// queue in the order they arrived, Core judging which generation is
    /// current.
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per message the supervisor sends, with no wildcard"
    )]
    fn on_outpost_message(&mut self, message: OutpostMessage) {
        match message {
            OutpostMessage::Started {
                outpost,
                target_pid,
                ..
            } => {
                self.live.started(outpost);
                self.context.outposts.lock().expect("outposts lock").insert(
                    target_pid,
                    OutpostStatus {
                        target_pid,
                        outpost_pid: None,
                        state: OutpostState::Starting,
                    },
                );
                self.queue(QueueInput::Process(ProcessFact::OutpostStarted {
                    process: target_pid,
                    generation: outpost.0,
                }));
            }
            OutpostMessage::Event {
                pid,
                outpost,
                position,
                message,
            } => {
                // Core drops a tell or an answer of a generation not
                // current. Handled from here on: anything the reducer keeps
                // from it is in the held nodes sent after this input.
                self.live.handled(outpost, position);
                match *message {
                    OutpostToCore::Tell(tell) => {
                        // The latency timeline starts when the event was
                        // first observed, and Core receives it now.
                        self.tell_received(&tell);
                        self.queue(QueueInput::Tell {
                            from: outpost_instance(pid, outpost),
                            tell,
                        });
                    }
                    OutpostToCore::Hung(hung) => self.queue(QueueInput::WindowHung {
                        from: outpost_instance(pid, outpost),
                        hung,
                    }),
                    OutpostToCore::Answer(answer) => {
                        tracing::debug!(target: "request_hop", request = %answer.number, "answer queued for Core's reducer");
                        self.queue(QueueInput::Answer {
                            from: outpost_instance(pid, outpost),
                            answer,
                        });
                    }
                    message => self.on_outpost_input(pid, outpost, message),
                }
            }
            OutpostMessage::Ended {
                outpost,
                target_pid,
                reason,
                ..
            } => self.on_outpost_ended(outpost, target_pid, reason),
            OutpostMessage::SpareReady { spare, waiting, .. } => {
                tracing::debug!(%spare, waiting, "spare outpost ready");
                self.queue(QueueInput::Process(ProcessFact::SparesReady(waiting)));
            }
            OutpostMessage::ListenerStarted { listener, .. } => {
                self.listener_view.started();
                self.queue(QueueInput::Process(ProcessFact::ListenerStarted(
                    listener.0,
                )));
            }
            OutpostMessage::ListenerReady { listener, .. } => {
                self.queue(QueueInput::ReaderReady(listener_instance(listener)));
            }
            OutpostMessage::ListenerEnded { listener, .. } => {
                self.queue(QueueInput::ReaderEnded(listener_instance(listener)));
            }
            OutpostMessage::Listener { listener, message } => {
                let from = listener_instance(listener);
                // Counted as the listener numbered it as it wrote it, so a
                // view's position names a point in its stream.
                if let Some(token) = message.carried_token() {
                    self.listener_view.handled(token);
                }
                if let ListenerToCore::Tell(tell) = &*message {
                    self.tell_received(tell);
                }
                match *message {
                    ListenerToCore::Foreground {
                        window,
                        root_owner,
                        process,
                        thread,
                        console,
                        integrity,
                        ..
                    } => self.foreground(window, root_owner, process, thread, console, integrity),
                    message => match listener_input(from, message) {
                        Ok(input) => self.queue(input),
                        Err(other) => tracing::error!(target: verbatim_model::DEFECT_TARGET,
                            ?other,
                            "the supervisor handed on a listener message that is no tell, destroy, answer or foreground report, a logic bug"
                        ),
                    },
                }
            }
            OutpostMessage::Pong {
                outpost,
                target_pid,
                abandoned,
            } => {
                self.queue(QueueInput::Process(ProcessFact::Pong {
                    instance: outpost_instance(target_pid, outpost),
                    abandoned,
                }));
            }
            OutpostMessage::IdleQueried {
                round, outposts, ..
            } => {
                self.barrier.queried(round, outposts);
            }
            // `Frames` turns these into the messages they carry, before
            // anything reaches here.
            OutpostMessage::Frame(_) => {}
            OutpostMessage::LaunchFailed {
                target_pid,
                application_gone,
                ..
            } => self.launch_failed(target_pid, application_gone),
            OutpostMessage::ApplicationExited { target_pid, .. } => {
                self.queue(QueueInput::Process(ProcessFact::ApplicationExited(
                    target_pid,
                )));
            }
        }
    }

    /// Records in the latency ledger that Core received `tell`, observed
    /// when its reader's callback received it.
    fn tell_received(&self, tell: &verbatim_model::tell::Event) {
        self.context
            .ledger
            .event_observed(tell.trace_id, tell.at_us);
        self.context.ledger.event_received(
            tell.trace_id,
            EventTiming {
                observed_at_us: tell.at_us,
                ..EventTiming::default()
            },
        );
    }

    /// The listener's report that `window` came to the foreground, Core's
    /// one input of it: with the window's class, a ghost window mapped to
    /// the hung window it stands in for, and the integrity levels the
    /// keyboard hook's view reads.
    fn foreground(
        &mut self,
        window: isize,
        root_owner: isize,
        process: u32,
        thread: u32,
        console: bool,
        integrity: Option<u32>,
    ) {
        // At info, with the starts it may decide, to time an outpost's
        // start from what asked for it.
        tracing::info!(process, "the listener's foreground report reached Core");
        let in_front = WindowHandle(u64::from_ne_bytes(window.to_ne_bytes()));
        let own_integrity = verbatim_win32::integrity::own_integrity();
        // A ghost window in front stands in for a hung window of another
        // process: Core's foreground test and attention judge the hung
        // window and its application, so a request to it is stopped as
        // hung and the hung window, not the ghost, joins the recovery set;
        // the keyboard hook's view keeps the ghost, the window in front.
        let judged = hang_check::judged(in_front);
        let report = if judged == in_front {
            ForegroundReport {
                window: in_front,
                ghost: None,
                root_owner: WindowHandle(u64::from_ne_bytes(root_owner.to_ne_bytes())),
                process: Pid(process),
                thread,
                class: verbatim_win32::window::window_class_name(window),
                console,
                integrity,
                own_integrity,
            }
        } else {
            let handle = window_handle(judged);
            let (thread, process) = verbatim_win32::window::window_owner(handle);
            ForegroundReport {
                window: judged,
                ghost: Some(in_front),
                root_owner: WindowHandle(u64::from_ne_bytes(
                    verbatim_win32::window::root_owner_of(handle).to_ne_bytes(),
                )),
                process: Pid(process),
                thread,
                class: verbatim_win32::window::window_class_name(handle),
                console,
                integrity,
                own_integrity,
            }
        };
        self.queue(QueueInput::Foreground(report));
    }

    /// No outpost will serve `target_pid` now: Core hears its launch failed,
    /// and a status entry only waiting for its outpost is removed.
    fn launch_failed(&mut self, target_pid: Pid, application_gone: bool) {
        self.queue(QueueInput::Process(ProcessFact::LaunchFailed {
            process: target_pid,
            application_gone,
        }));
        let mut outposts = self.context.outposts.lock().expect("outposts lock");
        if outposts
            .get(&target_pid)
            .is_some_and(|status| status.outpost_pid.is_none())
        {
            outposts.remove(&target_pid);
        }
    }

    /// Applies the inputs handling left to follow (`follow_ups`), in order.
    fn apply_follow_ups(&mut self) {
        for input in std::mem::take(&mut self.follow_ups) {
            self.apply(input);
        }
    }

    /// Turns one message from a live outpost into reducer input, or routes a
    /// reply to whoever asked.
    #[deny(clippy::wildcard_enum_match_arm)]
    fn on_outpost_input(&mut self, source: Pid, outpost: OutpostId, message: OutpostToCore) {
        match message {
            OutpostToCore::Ready {
                outpost_pid,
                target_pid,
            } => {
                tracing::info!(%outpost, %outpost_pid, %target_pid, "outpost ready");
                self.context.outposts.lock().expect("outposts lock").insert(
                    target_pid,
                    OutpostStatus {
                        target_pid,
                        outpost_pid: Some(outpost_pid),
                        state: OutpostState::Ready,
                    },
                );
                self.queue(QueueInput::ReaderReady(outpost_instance(
                    target_pid, outpost,
                )));
            }
            OutpostToCore::Fault { detail } => {
                tracing::warn!(%source, %outpost, detail, "outpost fault");
            }
            OutpostToCore::IdleAnswer { round } => self.barrier.answered(round, outpost),
            // Core's event queue takes tells and answers before this
            // function is called ([`Self::on_outpost_message`]); the relay
            // consumes the rest, and hands none of them on.
            OutpostToCore::Answer(_)
            | OutpostToCore::Tell(_)
            | OutpostToCore::Hung(_)
            | OutpostToCore::Pong { .. }
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
            ShellCommand::Input(input) => self.take_input(*input),
            ShellCommand::DumpTree(ticket, locations, reply) => {
                self.dump_tree(ticket, locations, reply);
            }
            ShellCommand::ShellItems(kind, ask) => self.shell_items(kind, ask),
            ShellCommand::DumpTreeGivenUp(ticket) => self.dump_tree_given_up(ticket),
            ShellCommand::Exiting { replaced, heard } => {
                // The hook is gone, so nothing more joins Core's stroke
                // line, and a key in it would now arrive after keys pressed
                // since: Core names the keys to release, and drops the
                // rest.
                self.queue(QueueInput::Exit);
                self.apply(Input::Exiting { replaced });
                let _ = heard.send(self.exit_sound.take());
            }
            ShellCommand::TerminalSettings(change) => self.change_terminal_settings(change),
            ShellCommand::InputHandled(input) => {
                // Behind the entries of the keys the hook reported before
                // it, and a lock key's announcement still to come, as the
                // key's stroke caused them.
                let held = self.marks.take(
                    input,
                    self.last_press,
                    self.state.announces_toggle_key(),
                    self.state.oldest_key_entry(),
                );
                for input in held {
                    self.input_handled(input);
                }
            }
            ShellCommand::AwaitIdle(waiter) => {
                self.barrier.push(waiter.after_input, waiter.reply);
            }
            ShellCommand::DumpFocus(reply) => {
                let (focus, ancestors, navigator) = self.state.focus_report();
                let _ = reply.send(FocusReport {
                    focus: focus.cloned(),
                    ancestors,
                    navigator: navigator.cloned(),
                });
            }
        }
    }

    /// Takes `input` another thread posted. A press joins Core's event
    /// queue with its caret key bound to the committed focus as Core takes
    /// it, in the order of the inputs before it, and starts its latency
    /// line.
    fn take_input(&mut self, mut input: Input) {
        if let Input::Queue {
            trace_id,
            input: QueueInput::Press(press),
            ..
        } = &mut input
        {
            self.last_press = Some(press.key_seq);
            self.context
                .ledger
                .key_pressed(*trace_id, press.pressed_at_us);
            if let verbatim_model::request::PressAction::CaretKey { bound, .. } = &mut press.action
            {
                *bound = self.state.caret_binding().map(Box::new);
            }
        }
        self.apply(input);
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
        let starting = self.state.outposts_starting();
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
            "handled input {}, {standing}, {starting} outposts starting, terminal timer {}",
            self.handled_input,
            if self.state.terminal_timer_due().is_some() {
                "armed"
            } else {
                "not armed"
            }
        );
    }

    /// The reducer's own conditions for the idle barrier: no outpost is
    /// starting (one still starting has a request waiting for it, as a
    /// replacement for a crashed one does), and no terminal's
    /// quiet-period timer is armed, since what waits for it is spoken when
    /// it ends; and Core's event queue is idle (`SrState::queue_idle`: no
    /// entry, no request waiting or out, no window watched for recovery,
    /// no stroke in line, no lock key's announcement still to come, no
    /// delay of its own due).
    fn idle_conditions_clear(&self) -> bool {
        self.state.queue_idle()
            && self.state.terminal_timer_due().is_none()
            && self.state.outposts_starting() == 0
    }

    /// Writes the settings dialog's Terminal page's four values into the
    /// reader settings the reducer has now, leaving the others; gives the
    /// reducer the result, and saves it.
    fn change_terminal_settings(&mut self, change: verbatim_gui::TerminalChange) {
        let mut settings = self.state.settings();
        change.apply_to(&mut settings);
        self.apply(Input::Settings(settings));
        self.context.reader.changed(settings);
    }

    /// Runs one input through the reducer, records it in the flight
    /// recorder, and executes the resulting effects.
    fn apply(&mut self, input: Input) {
        let trace_id = input.trace_id().unwrap_or_else(TraceId::mint);
        let effects = reduce(&mut self.state, &mut self.runtime, &input);
        if let Some(readiness) = &self.readiness
            && readiness.core((self.state.readiness(), self.state.focus_committed()))
        {
            self.readiness = None;
        }
        self.context.ledger.reduced(trace_id);
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
        self.apply_follow_ups();
        // The marks held behind their keys' entries and a lock key's
        // announcement go once those have ended and it has been made.
        for input in self.marks.release(
            self.state.announces_toggle_key(),
            self.state.oldest_key_entry(),
        ) {
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
    /// (`books/dev/src/explanation/design/ownership.md`). Any other
    /// change, of the held anchors or of the position of the last message
    /// handled alone, is left for the outpost, and the next command written
    /// to it, a request or the relay's ping within three seconds, carries
    /// it (`docs/design/communication.md`, step 11): the caret's anchor
    /// changes with every key, and a write for each would cost a pipe
    /// write and the outpost's reader wake on the typing path.
    fn send_views(&mut self) {
        // The listener's told elements Core holds: a view written at once
        // when a token Core handled is no longer held, so the listener
        // lets go of it within one pipe hop; additions are left for the
        // next command (`verbatim_core::held_tokens`).
        let held_tokens = self.state.held_tokens();
        match self.listener_view.after_pass(held_tokens.as_ref()) {
            verbatim_core::ViewSend::Nothing => {}
            verbatim_core::ViewSend::Now(view) => {
                self.context
                    .supervisor
                    .send_elements_held(elements_held(view));
            }
            verbatim_core::ViewSend::Leave(view) => {
                self.context
                    .supervisor
                    .leave_elements_held(elements_held(view));
            }
        }
        let held = self.state.held_nodes();
        let anchors = self.state.held_anchors();
        let mut told = self.state.held_told();
        for (outpost, live) in self.live.iter_mut() {
            let now = live::Held {
                nodes: held
                    .get(outpost)
                    .map(|nodes| nodes.iter().map(|id| id.number()).collect())
                    .unwrap_or_default(),
                anchors: anchors
                    .get(outpost)
                    .map(|anchors| anchors.iter().map(|anchor| anchor.0).collect())
                    .unwrap_or_default(),
                told: told.remove(outpost).unwrap_or_default(),
                acknowledged: live.position,
            };
            let change = live.held_changed(&now);
            if change == live::HeldChange::None {
                continue;
            }
            let lists = (
                now.nodes.into_iter().collect(),
                now.anchors.into_iter().collect(),
                now.told.into_iter().collect(),
            );
            // A caret tell's element Core lets go of is left, as anchors
            // are: no pipe write and no wake of the outpost per caret key.
            if change == live::HeldChange::Nodes {
                self.context
                    .supervisor
                    .send_nodes_held(*outpost, lists, now.acknowledged);
            } else {
                self.context
                    .supervisor
                    .leave_nodes_held(*outpost, lists, now.acknowledged);
            }
        }
    }

    /// Fans an event Core took out to the control plane's event stream,
    /// when some connection is subscribed to events.
    fn observed(&self, observed: verbatim_model::Observed) {
        if let Some(server) = self.context.server_slot.get()
            && server.has_event_subscribers()
        {
            let verbatim_model::Observed {
                trace_id,
                source,
                backend,
                window,
                event,
            } = observed;
            server.broadcast_event(trace_id, source, backend, window, event);
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
            Effect::CopyToClipboard(text) => self.context.clipboard.copy(trace_id, text),
            Effect::KeepDisplayOn(on) => keep_display_on(on),
            Effect::SettingsChanged(settings) => self.context.reader.changed(settings),
            // The exit sound is heard before Verbatim exits: the main thread
            // waits for it ([`ShellCommand::Exiting`]). A preview of it is
            // played as any other earcon.
            Effect::PlayEarcon(Earcon::Exit) if self.state.exiting() => {
                self.exit_sound = Some(
                    self.context
                        .manager
                        .play_earcon_heard(&mut self.token, Earcon::Exit),
                );
            }
            Effect::PlayEarcon(earcon) => self.context.manager.play_earcon(&mut self.token, earcon),
            Effect::Queue(effect) => self.execute_queue(effect),
            Effect::ForKey(key, effect) => self.execute(key, *effect),
            Effect::ReadClock { repeat, key } => {
                self.follow_ups.push(read_clock(trace_id, repeat, key));
            }
            Effect::Observed(observed) => self.observed(*observed),
            Effect::Show(request) => send_gui_command(
                &self.context.gui_handle,
                match request {
                    GuiRequest::Menu => GuiCommand::ShowMenu,
                    GuiRequest::ShellList(kind) => GuiCommand::OpenShellItemList(kind),
                },
            ),
        }
    }

    /// Carries out one of Core's event queue's effects.
    fn execute_queue(&mut self, effect: QueueEffect) {
        match effect {
            QueueEffect::Send { to, ask } => self.send_request(to, ask),
            QueueEffect::Cancel { to, cancel } => match to.reader {
                Reader::Listener => self
                    .context
                    .supervisor
                    .send_to_listener(CoreToListener::Cancel(cancel)),
                Reader::Outpost(_) => {
                    let outpost = OutpostId(to.generation);
                    if let Err(error) = self
                        .context
                        .supervisor
                        .send_to_outpost(outpost, CoreToOutpost::Cancel(cancel))
                    {
                        // Its end answers the request.
                        tracing::debug!(%error, %outpost, "a cancel could not reach its outpost");
                    }
                }
            },
            QueueEffect::Answered { asker, outcome } => {
                if let Some(waiting) = self.outside.answered(asker) {
                    requests::deliver(waiting, outcome);
                } else {
                    tracing::warn!(
                        asker,
                        ?outcome,
                        "a request outside the queue ended with no asker"
                    );
                }
            }
            QueueEffect::CheckHung(windows) => {
                for window in windows {
                    let hung = hang_check::hung(window);
                    self.follow_ups
                        .push(queued(QueueInput::HangCheck { window, hung }));
                }
            }
            QueueEffect::SendStrokes(transitions) => strokes::send(&transitions),
            QueueEffect::ReleaseAtExit(keys) => strokes::release_at_exit(&keys),
            QueueEffect::WindowDestroyed { to, window } => {
                let outpost = OutpostId(to.generation);
                if let Err(error) = self
                    .context
                    .supervisor
                    .send_to_outpost(outpost, CoreToOutpost::WindowDestroyed(window))
                {
                    tracing::debug!(%error, %outpost, "a window's destruction could not reach its outpost");
                }
            }
            QueueEffect::AnswerDropped { number, late } => {
                tracing::debug!(%number, late, "an answer was dropped");
            }
            QueueEffect::Undecodable(number) => {
                tracing::warn!(target: verbatim_model::DEFECT_TARGET,
                    %number,
                    "a malformed answer: its result is no reply its request can have"
                );
            }
            QueueEffect::ForeignHandle { batch, handle } => {
                tracing::error!(target: verbatim_model::DEFECT_TARGET,
                    ?batch,
                    ?handle,
                    "a handle of one numbered batch was given to another, a defect in Core"
                );
            }
            QueueEffect::Mismatch { replayed, original } => {
                tracing::error!(target: verbatim_model::DEFECT_TARGET,
                    replayed,
                    original,
                    "Core's front entry could not be rebuilt from its checkpoint, a defect in Core"
                );
            }
            QueueEffect::WakeAt(due) => self.queue_due = due,
            QueueEffect::StartOutpost(process) => {
                tracing::info!(%process, "Core starts an outpost");
                self.context.supervisor.start_outpost(process);
            }
            QueueEffect::EndOutpost(instance) => {
                self.context
                    .supervisor
                    .end_outpost(OutpostId(instance.generation));
            }
            QueueEffect::StaleFrame(instance) => {
                tracing::debug!(
                    ?instance,
                    "a frame of a reader's instance not current was dropped"
                );
            }
        }
    }

    /// Sends Core's request `ask` to the instance `to`, unless Windows
    /// reports its window hung, when it is answered unsent: a read of an
    /// application's current focus with the window in front, read live
    /// ([`hang_check::front`]), any other "not responding"; one that
    /// cannot be written is answered "reader ended". Each
    /// answer the shell gives enters Core's inbox after the input whose
    /// effects are being carried out, so no reducer step re-enters itself.
    fn send_request(&mut self, to: Instance, ask: verbatim_model::request::Ask) {
        let number = ask.number;
        // The check after a menu, popup or switcher end finding no window
        // in front is answered with the desktop, as NVDA's fake focus
        // reads it (Dickson, 2026-11-19).
        if matches!(
            &ask.request,
            verbatim_model::request::Question::Batch(batch) if batch.reads_focus_if_focused()
        ) && hang_check::nothing_in_front()
        {
            self.follow_ups.push(queued(QueueInput::InFront {
                number,
                front: hang_check::front(),
            }));
            return;
        }
        if hang_check::hung(ask.window) {
            // A read of an application's current focus is answered with
            // what is in front, which Core judges; every other request is
            // answered "not responding".
            let in_front = match &ask.request {
                verbatim_model::request::Question::Legacy(_) => false,
                verbatim_model::request::Question::Batch(batch) => batch.reads_focus_first(),
            };
            let refused = if in_front {
                QueueInput::InFront {
                    number,
                    front: hang_check::front(),
                }
            } else {
                QueueInput::RefusedHung(number)
            };
            self.follow_ups.push(queued(refused));
            return;
        }
        let sent = match to.reader {
            Reader::Listener => {
                self.context
                    .supervisor
                    .send_to_listener(CoreToListener::Request(Box::new(ask)));
                Ok(())
            }
            Reader::Outpost(_) => {
                tracing::debug!(target: "request_hop", request = %number, "request written to the outpost");
                self.context.supervisor.send_to_outpost(
                    OutpostId(to.generation),
                    CoreToOutpost::Request(Box::new(ask)),
                )
            }
        };
        if let Err(error) = sent {
            tracing::warn!(%error, ?to, %number, "a request could not reach its reader");
            self.follow_ups.push(queued(QueueInput::Answer {
                from: to,
                answer: Answer {
                    number,
                    outcome: Outcome::Ended,
                    probes: Vec::new(),
                    detail: String::new(),
                },
            }));
        }
    }

    /// Starts a control-plane tree dump of the application holding
    /// attention, each element's location read only with `locations`: a
    /// request of Core's from outside the event queue, naming the
    /// application's main window, with the queue's due time and hang check
    /// (`docs/design/readers-tell-core-asks-build1.md`, "Requests in
    /// Core"). The answer goes to `reply`; the requester waits on it with
    /// its own timeout, and a late answer to a request it gave up on is
    /// dropped.
    fn dump_tree(&mut self, ticket: DumpTicket, locations: bool, reply: Sender<DumpTreeResult>) {
        let Some(pid) = self.state.attention() else {
            let _ = reply.try_send(Err("no application holds attention yet".to_owned()));
            return;
        };
        let Some(main) = verbatim_win32::window::main_window_of(pid.0) else {
            let _ = reply.try_send(Err(format!("pid {pid} has no top-level window")));
            return;
        };
        let asker = self.outside.begin(Asker::DumpTree { ticket, reply });
        self.queue(QueueInput::AskOutside {
            asker,
            window: verbatim_win32::window_facts::node_window(
                &verbatim_win32::window_facts::SystemReads,
                main,
            ),
            request: verbatim_model::request::Request::DumpTree { locations }.into(),
        });
    }

    /// Asks the outpost of the process that owns the taskbar `ask` of the
    /// shell's surface `kind`, its items or where a clicked item is, so
    /// verbatim.exe makes no UIA connection into the shell's windows
    /// (`books/dev/src/explanation/design/listener-without-connections.md`,
    /// decision 9): a request of Core's from outside the event queue,
    /// naming `Shell_TrayWnd`, which Core sends once the taskbar's outpost
    /// is ready, starting one when there is none, with the queue's due time
    /// and hang check and no stop at a foreground change, so the click's
    /// read survives the dialog's closing (Dickson, 2026-11-15). The outcome
    /// goes to `ask`'s callback exactly once; `None` when there is no
    /// taskbar or for any failure.
    fn shell_items(&mut self, kind: ShellItemKind, ask: verbatim_gui::ShellAsk) {
        let Some(taskbar) = taskbar_window() else {
            // Dropped, the request is answered "nothing".
            tracing::warn!(?kind, "the taskbar window was not found");
            drop(ask);
            return;
        };
        let (asker, request) = match ask {
            verbatim_gui::ShellAsk::Items(deliver) => (
                Asker::ShellItems { deliver },
                verbatim_model::request::Request::ShellItems { kind },
            ),
            verbatim_gui::ShellAsk::Position(item, deliver) => (
                Asker::ShellItemPosition { deliver },
                verbatim_model::request::Request::ShellItemPosition { kind, item },
            ),
        };
        let asker = self.outside.begin(asker);
        self.queue(QueueInput::AskOutside {
            asker,
            window: verbatim_win32::window_facts::node_window(
                &verbatim_win32::window_facts::SystemReads,
                taskbar,
            ),
            request: request.into(),
        });
    }

    /// Handles the end of `outpost`, which watched `target_pid`, for
    /// `reason`: Core's generation record hears its reader ended, answers
    /// what was asked of it and decides its replacement.
    fn on_outpost_ended(&mut self, outpost: OutpostId, target_pid: Pid, reason: EndReason) {
        tracing::info!(%outpost, %target_pid, %reason, "outpost ended");
        // Everything it sent is ahead of its end: in the idle
        // barrier's round, its end is its answer.
        self.barrier.ended(outpost);
        // An end for its application's exit is Core's as that exit, which
        // the supervisor reports next (Table 5, column J): no re-take and
        // no replacement.
        if reason != EndReason::TargetExited {
            self.queue(QueueInput::ReaderEnded(outpost_instance(
                target_pid, outpost,
            )));
        }
        if self.live.ended(outpost) {
            self.context
                .outposts
                .lock()
                .expect("outposts lock")
                .remove(&target_pid);
        }
        if let Some(server) = self.context.server_slot.get() {
            server.broadcast_outpost_ended(target_pid, reason.to_string());
        }
    }

    /// The earliest time Core's event queue asked to be woken, the focused
    /// terminal's quiet-period timer is due, or a lock key's state is read.
    /// The last two are views of Core's state, read here after each step,
    /// so a timer whose model is gone is gone with it.
    fn next_deadline(&self) -> Option<Instant> {
        self.state
            .terminal_timer_due()
            .map(ms_to_us)
            .into_iter()
            .chain(
                self.state
                    .toggle_key_read_due()
                    .map(|(_, due_ms)| ms_to_us(due_ms)),
            )
            .min()
            .map(instant_at)
            .into_iter()
            .chain(self.queue_due.map(instant_at))
            .min()
    }

    /// Tells Core once the time it asked to be woken at is past, and moves
    /// the terminal's timer and the lock keys' reads on.
    fn expire(&mut self) {
        let shared = now_us();
        if self.queue_due.take_if(|due| *due <= shared).is_some() {
            self.queue(QueueInput::Due);
        }
        // The terminal's timer, once its due time is past; the step moves
        // the due time past it (`SrState::terminal_timer_due`).
        if let Some(at_ms) = self.state.terminal_timer_due()
            && ms_to_us(at_ms) <= shared
        {
            self.queue(QueueInput::TerminalDue { at_ms });
        }
        // Each lock key whose read is due, in press order; the step takes
        // it from the front.
        while let Some((toggle, due_ms)) = self.state.toggle_key_read_due()
            && ms_to_us(due_ms) <= shared
        {
            self.apply(read_toggle_key(toggle));
        }
    }

    /// Withdraws the tree dump named `ticket`, whose caller stopped
    /// waiting: Core stops waiting for it, with the reason "withdrawn", and
    /// its outpost drops it if it has not started; a dump already running
    /// finishes, within its own walk budget. Either way the request keeps
    /// its one answer, which goes to a receiver dropped.
    fn dump_tree_given_up(&mut self, ticket: DumpTicket) {
        if let Some(asker) = self.outside.dump(ticket) {
            self.queue(QueueInput::WithdrawOutside(asker));
        }
    }
}

/// `input` for Core's event queue, arriving now.
fn queued(input: QueueInput) -> Input {
    Input::Queue {
        trace_id: TraceId::mint(),
        at_us: now_us(),
        input,
    }
}

/// The instance of `outpost`, the generation watching `target_pid`.
fn outpost_instance(target_pid: Pid, outpost: OutpostId) -> Instance {
    Instance {
        reader: Reader::Outpost(target_pid),
        generation: outpost.0,
    }
}

/// Core's view of the listener's told elements it holds, as the listener
/// is told it.
fn elements_held(view: verbatim_core::View) -> CoreToListener {
    CoreToListener::ElementsHeld {
        generation: view.generation,
        tokens: view.tokens,
        acknowledged: view.acknowledged,
    }
}

/// The instance of the listener of generation `listener`.
/// The input Core's event queue takes for `message`, a message the
/// listener instance `from` sent that the supervisor hands on: a tell, a
/// window destroyed or reported hung, or an answer; any other message
/// back, which the supervisor never hands on.
fn listener_input(
    from: Instance,
    message: ListenerToCore,
) -> Result<QueueInput, Box<ListenerToCore>> {
    match message {
        ListenerToCore::Tell(tell) => Ok(QueueInput::Tell { from, tell }),
        ListenerToCore::Answer(answer) => Ok(QueueInput::Answer { from, answer }),
        ListenerToCore::Destroyed {
            window, process, ..
        } => Ok(QueueInput::WindowDestroyed {
            from,
            window,
            process,
        }),
        ListenerToCore::Hung(hung) => Ok(QueueInput::WindowHung { from, hung }),
        other => Err(Box::new(other)),
    }
}

fn listener_instance(listener: OutpostId) -> Instance {
    Instance {
        reader: Reader::Listener,
        generation: listener.0,
    }
}

/// The window `window` names, as the Windows calls take it.
fn window_handle(window: WindowHandle) -> isize {
    isize::from_ne_bytes(window.0.to_ne_bytes())
}

/// Posts the supervisor's messages to the reducer thread's port, in line
/// with its keys and commands, so the reducer takes them all in the order
/// they arrived (`docs/design/communication.md`, section 4.5).
fn outpost_sink(command_tx: Commands) -> verbatim_supervisor::EventSink {
    verbatim_supervisor::EventSink::posting(command_tx, ShellCommand::Outpost)
}

/// The reducer thread body: takes outpost messages and commands from other
/// threads off its port, in the order they arrived, feeds the pure
/// reducer, and executes its effects, so a key, a review or
/// object-navigation gesture, and an accessibility event are each handled
/// by the same reduce-and-execute step, in turn.
fn reducer_loop(
    mut command_rx: PortQueue<ShellCommand>,
    context: &ReducerContext,
    readiness: Arc<harness::Readiness>,
    settings: ReaderSettings,
    token: SpeakToken,
) -> SpeakToken {
    let mut thread = ReducerThread {
        context,
        state: SrState::new(),
        runtime: verbatim_core::Runtime::default(),
        outside: requests::Outside::default(),
        live: LiveOutposts::default(),
        listener_view: verbatim_core::ListenerView::default(),
        handled_input: 0,
        frames: context.supervisor.frames(),
        barrier: idle::IdleBarrier::default(),
        exit_sound: None,
        queue_due: None,
        marks: idle::HeldMarks::default(),
        last_press: None,
        follow_ups: Vec::new(),
        token,
        readiness: Some(readiness),
    };
    // The settings come first, before anything they govern.
    thread.apply(Input::Settings(settings));
    thread.send_views();
    loop {
        let due = thread.next_deadline();
        if !pass(&mut command_rx, due, &mut thread) {
            break;
        }
        thread.advance_idle_barrier(&mut command_rx);
    }
    thread.token
}

/// What one pass of the reducer loop asks of its thread: [`ReducerThread`],
/// or a test's recorder of the order.
trait Pass {
    /// Handles one command, then sends the views it changed.
    fn command(&mut self, command: ShellCommand);
    /// Judges every deadline past, then sends the views it changed.
    fn expire(&mut self);
}

impl Pass for ReducerThread<'_> {
    fn command(&mut self, command: ShellCommand) {
        self.on_command(command);
        self.send_views();
    }

    fn expire(&mut self) {
        ReducerThread::expire(self);
        self.send_views();
    }
}

/// One pass of the reducer loop: waits for the next command, or until
/// `due`, the earliest of the thread's deadlines, and handles what came.
/// Once `due` has passed, every command already waiting is handled before
/// the deadlines are judged, since each may carry a fact a judgement
/// depends on, such as a pong the relay read while the thread was held.
/// With no deadline outstanding, or none yet passed, nothing is expired: a
/// command costs no scan of the requests. Returns `false` once the port's
/// end has gone.
fn pass(
    command_rx: &mut PortQueue<ShellCommand>,
    due: Option<Instant>,
    thread: &mut impl Pass,
) -> bool {
    match next_command(command_rx, due) {
        Next::Command(command) => thread.command(command),
        Next::Woken => {}
        Next::Closed => return false,
    }
    if due.is_some_and(|due| Instant::now() >= due) {
        while let Some(command) = command_rx.try_recv() {
            thread.command(command);
        }
        thread.expire();
    }
    true
}

/// `ms` milliseconds on the run's shared clock, in microseconds.
fn ms_to_us(ms: u64) -> u64 {
    ms.saturating_mul(1_000)
}

/// The moment on the monotonic clock that `due_us`, on the run's shared
/// clock, falls at, for the reducer thread's wait. It errs late, a
/// microsecond past and with the shared clock read first, so that once the
/// monotonic clock has passed it the shared clock has passed `due_us`, and
/// the deadline the wait was for is expired when the thread checks. Both
/// clocks are the performance counter.
fn instant_at(due_us: u64) -> Instant {
    let shared = now_us();
    let monotonic = Instant::now();
    monotonic + Duration::from_micros(due_us.saturating_sub(shared).saturating_add(1))
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
/// earliest of the thread's timers ([`ReducerThread::next_deadline`]), when
/// one is due. Only
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
    queued(QueueInput::LockKeyRead { key: toggle, on })
}

/// The localized current time and date, read for the time gesture, key
/// `key`, whose effect ran under `trace_id`, for the reducer to speak the
/// time on a first press and the date on a quick second (`repeat`).
fn read_clock(trace_id: TraceId, repeat: u8, key: verbatim_model::KeySeq) -> Input {
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
        key,
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
            GuiEvent::BlockedByMessageBox => {
                let input = Input::BlockedByMessageBox {
                    trace_id: TraceId::mint(),
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
        gui_handle,
        command_tx,
        recorder,
        dumps_dir,
        readiness,
        idle_line,
    } = config;
    let focus_tx = command_tx.clone();
    let idle_tx = command_tx.clone();
    let menu_gui = Arc::clone(&gui_handle);
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
        latency: Box::new(move |last_n| ledger.recent(last_n)),
        dump_tree: Box::new(move |locations| request_dump_tree(&command_tx, locations)),
        dump_recorder: Box::new(move || {
            flight_dump::dump_now(&recorder, &dumps_dir)
                .map(|path| path.display().to_string())
                .map_err(|error| error.to_string())
        }),
        open_menu: Box::new(move || {
            // As a click on the tray icon opens it: no key, so no key
            // number.
            menu_gui
                .get()
                .ok_or_else(|| "the GUI is not up".to_owned())
                .map(|gui| gui.send(GuiCommand::ShowMenu))
        }),
        quit: Box::new(move || request_shutdown(&gui_handle)),
        await_idle: Box::new(move |after_input, timeout| {
            idle_line.wait(&idle_tx, after_input, timeout)
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

    /// The listener's window destroyed, as the supervisor hands it on,
    /// becomes Core's `WindowDestroyed` input from that listener, with or
    /// without a process.
    #[test]
    fn a_listeners_window_destroyed_is_queued_for_core() {
        let from = listener_instance(OutpostId(3));
        for process in [Some(Pid(9)), None] {
            assert_eq!(
                listener_input(
                    from,
                    ListenerToCore::Destroyed {
                        window: WindowHandle(7),
                        process,
                        at_us: 1,
                    }
                ),
                Ok(QueueInput::WindowDestroyed {
                    from,
                    window: WindowHandle(7),
                    process,
                })
            );
        }
    }

    /// The listener's report of a hung window becomes Core's `WindowHung`
    /// input from that listener.
    #[test]
    fn a_listeners_report_of_a_hung_window_is_queued_for_core() {
        let from = listener_instance(OutpostId(3));
        let facet = verbatim_model::tell::WindowFacet {
            handle: WindowHandle(7),
            class: "#32770".to_owned(),
            process: Pid(9),
            thread: 4,
        };
        let hung = verbatim_model::tell::Hung {
            window: facet.clone(),
            root: facet.clone(),
            root_owner: facet,
            at_us: 1,
        };
        assert_eq!(
            listener_input(from, ListenerToCore::Hung(hung.clone())),
            Ok(QueueInput::WindowHung { from, hung })
        );
    }

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

    /// Records the order a pass hands its thread commands and judgements.
    #[derive(Default)]
    struct Order(Vec<String>);

    impl Pass for Order {
        fn command(&mut self, command: ShellCommand) {
            self.0.push(match command {
                ShellCommand::InputHandled(input) => format!("key {input}"),
                _ => "something else".to_owned(),
            });
        }

        fn expire(&mut self) {
            self.0.push("expire".to_owned());
        }
    }

    /// A pass whose deadline has passed handles every command already
    /// waiting, in arrival order, before it judges the deadline, so a fact
    /// posted before the judgement, such as an outpost's pong read while
    /// the thread was held, is in hand when it is made.
    #[test]
    fn a_past_deadline_is_judged_after_every_waiting_command() {
        let (command_tx, mut command_rx) = port_queue().expect("a port");
        for input in 1..=3 {
            command_tx
                .send(ShellCommand::InputHandled(input))
                .map_err(|_| "refused")
                .expect("the receiver is held");
        }
        let mut order = Order::default();
        assert!(pass(&mut command_rx, Some(Instant::now()), &mut order));
        assert_eq!(order.0, ["key 1", "key 2", "key 3", "expire"]);
    }

    /// The supervisor's messages and the reducer's other inputs share one
    /// port, so the reducer takes them in the order they arrived: a key
    /// handled between two outpost messages comes out between them.
    #[test]
    fn outpost_messages_and_commands_reach_the_reducer_in_arrival_order() {
        let (command_tx, mut command_rx) = port_queue().expect("a port");
        let sink = outpost_sink(command_tx.clone());
        assert!(
            sink.send(OutpostMessage::ApplicationExited {
                target_pid: Pid(1),
                sealed: verbatim_supervisor::Sealed::for_tests(),
            })
            .is_ok()
        );
        command_tx
            .send(ShellCommand::InputHandled(7))
            .map_err(|_| "refused")
            .expect("the receiver is held");
        assert!(
            sink.send(OutpostMessage::ApplicationExited {
                target_pid: Pid(2),
                sealed: verbatim_supervisor::Sealed::for_tests(),
            })
            .is_ok()
        );
        let arrived: Vec<String> = std::iter::from_fn(|| command_rx.try_recv())
            .map(|command| match command {
                ShellCommand::Outpost(OutpostMessage::ApplicationExited { target_pid, .. }) => {
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

    use verbatim_model::UtteranceId;

    /// A settings file that fails to load is kept as its backup, replacing
    /// an older one, and Verbatim starts on the defaults rooted at its own
    /// folder; the saver's first write writes a fresh file and leaves the
    /// backup alone.
    #[test]
    fn a_bad_settings_file_is_kept_as_its_backup_and_the_first_save_leaves_it() {
        let dir =
            std::env::temp_dir().join(format!("verbatim-app-bad-settings-{}", std::process::id()));
        // No live process shares this id, so a folder already named with it
        // was left by an earlier process that was ended before its guard ran.
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("remove a dead process's folder");
        }
        std::fs::create_dir(&dir).expect("a folder of the test's own");
        let scratch = RemovedOnDrop(dir.clone());
        let bad = "[keyboard]\nlayout = \"Laptop\"\n";
        std::fs::write(dir.join("settings.toml"), bad).expect("the bad file");
        std::fs::write(dir.join("settings.toml.corrupted.bak"), "older").expect("an older backup");

        let (store, failed) = load_config(&dir);
        assert!(failed.is_some(), "the load failed");
        let backup = std::fs::read_to_string(dir.join("settings.toml.corrupted.bak"));
        assert_eq!(backup.expect("the backup"), bad);
        assert_eq!(store.settings(), &verbatim_config::Settings::default());
        assert_eq!(store.themes_dir(), dir.join("themes"));

        let (saver, _, reader, _) =
            saver::Saver::start(saver::DiskFile(dir.clone()), store.settings().clone())
                .expect("the saver starts");
        reader.save(verbatim_model::ReaderSettings {
            follow_caret: false,
            ..verbatim_model::ReaderSettings::default()
        });
        assert!(saver.finish(Instant::now() + Duration::from_secs(5)));
        let backup = std::fs::read_to_string(dir.join("settings.toml.corrupted.bak"));
        assert_eq!(backup.expect("the backup"), bad, "the save left the backup");
        assert!(
            !verbatim_config::ConfigStore::load(&dir)
                .expect("the fresh file loads")
                .settings()
                .reader
                .follow_caret
        );
        drop(scratch);
    }

    /// A folder removed when the test ends, passing or panicking, so a
    /// failed run leaves nothing for a later process with the same id to
    /// meet.
    struct RemovedOnDrop(std::path::PathBuf);

    impl Drop for RemovedOnDrop {
        fn drop(&mut self) {
            // A failure here would hide the test's own panic during
            // unwinding; the next run with this id removes what is left.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Records each ending and each failed utterance's marks, in order.
    #[derive(Default)]
    struct Endings(Mutex<Vec<String>>);

    impl verbatim_speech::SpeechEvents for Endings {
        fn utterance_queued(&self, _: UtteranceId, _: TraceId, _: &str, _: Instant) {}

        fn audio_started(&self, _: UtteranceId, _: TraceId, _: bool, _: Instant) {}

        fn utterance_ended(
            &self,
            id: UtteranceId,
            _: TraceId,
            ending: &verbatim_model::UtteranceEnding,
            _: Instant,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{id} ended {ending:?}"));
        }

        fn marks_failed(
            &self,
            id: UtteranceId,
            _: TraceId,
            marks: &[verbatim_speech::IndexMark],
            _: Instant,
        ) {
            self.0
                .lock()
                .unwrap()
                .push(format!("{id} marks failed {marks:?}"));
        }
    }

    /// A speech section with nothing saved.
    struct NothingSaved;

    impl verbatim_speech::SpeechSettingsStore for NothingSaved {
        fn saved(
            &self,
            _: &SynthId,
        ) -> Vec<(verbatim_speech::SettingId, verbatim_speech::SettingValue)> {
            Vec::new()
        }

        fn persist(
            &mut self,
            _: &SynthId,
            _: bool,
            _: &[(verbatim_speech::SettingId, verbatim_speech::SettingValue)],
        ) {
        }
    }

    /// An utterance the speech queue's port refuses ends at once, failed,
    /// with its marks reported failed, so say-all reads on; and a wait for
    /// an event whose words are refused returns at once.
    #[test]
    fn a_refused_utterance_ends_failed_with_its_marks() {
        use verbatim_model::{
            SegmentContent, SpeechMark, SpeechPriority, Utterance, UtteranceSegment,
        };

        let refusal = verbatim_faults::arm(
            verbatim_faults::Fault::SpeechPostRefused,
            |_: &UtteranceId| true,
            2,
        );
        let mut registry = SynthRegistry::new();
        register_test_audio(&mut registry);
        let (inbox, taker) = port_queue::<QueueItem>().expect("a port");
        let endings = Arc::new(Endings::default());
        let (manager, mut token, _) = SpeechManager::new(SpeechManagerConfig {
            registry,
            initial_synth: SynthId::new("capture"),
            settings: Box::new(NothingSaved),
            mixer: MixerBuilder::new(Box::new(SilentDevice::new())),
            events: Arc::clone(&endings) as Arc<dyn verbatim_speech::SpeechEvents>,
            // No sound files, so the exit sound is spoken.
            theme: verbatim_speech::ActiveTheme::new(
                verbatim_model::Theme::builtin_default(),
                |_| None,
                verbatim_model::ThemeOptions::default(),
            ),
            inbox: (Arc::new(PortInbox(inbox)), Box::new(PortTaker(taker))),
        })
        .expect("the pipeline starts");

        let id = manager.speak(
            &token,
            Utterance {
                trace_id: TraceId::mint(),
                priority: SpeechPriority::Queued,
                segments: vec![
                    UtteranceSegment::text("a line"),
                    UtteranceSegment::new(SegmentContent::Mark(SpeechMark(7))),
                ],
                source: None,
                say_all: true,
                validity: None,
            },
        );
        assert_eq!(
            *endings.0.lock().unwrap(),
            [
                format!("{id} ended Failed(\"the speech queue refused the utterance\")"),
                format!("{id} marks failed [IndexMark(7)]"),
            ]
        );

        let heard = manager.play_earcon_heard(&mut token, Earcon::Exit);
        assert!(
            heard.wait_until(Instant::now()),
            "the refused words have ended before the wait begins"
        );
        drop(refusal);
    }
}
