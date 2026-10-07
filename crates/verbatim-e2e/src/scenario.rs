//! [`Scenario`]: the lifecycle owner for one live, agent-driven Verbatim run.
//!
//! A scenario is a guard struct, not a checklist of manual cleanup calls: it
//! launches Verbatim (and, on request, target applications) through the M2
//! agent, and its [`Drop`] impl kills everything it launched, unconditionally,
//! even if the test that created it panicked partway through. This matters
//! because every live test in this suite runs against the developer's real
//! desktop, launching a real `verbatim.exe` that injects real keystrokes.
//!
//! Configuration is always fixed and isolated, never the developer's own
//! live state. In runner-direct mode (the default; see [`REMOTE_ENV`]),
//! [`Scenario::launch`] copies `verbatim.exe` and `verbatim-outpost.exe`
//! into `target/e2e-stage` under the workspace root and writes
//! [`verbatim_config::Settings::for_e2e`]'s fixed settings there, then
//! launches that staged copy — never the developer's own
//! `target/debug/verbatim.exe` and its `settings.toml`, which a runner-direct
//! suite would otherwise read, mutate, and leave dirty. In remote mode,
//! `cargo xtask vm deploy` stages the guest side equivalently (its own
//! `write_synth_settings`, kept in lockstep with the same fixed-settings
//! shape — see `Settings::for_e2e`'s doc comment).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_agent::protocol::{ForegroundInfo, KillOutcome, ProcessState, WindowInfo};
use verbatim_config::{ConfigStore, Settings};
use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, LatencyRecord, ReplyPayload, Request};

use crate::agent_client::AgentClient;
use crate::recording::Recording;
use crate::speech::SpeechCollector;
use crate::timeline::Timeline;
use crate::{ENDPOINT_ENV, endpoint};

/// File names the artifact collectors write under, inside the directory
/// [`crate::artifacts::scenario_dir`] names. The timeline and stderr log are
/// written for every run by [`Scenario::collect_run_artifacts`] (so a passing
/// run leaves its announcement timings and outpost-ready timestamps
/// behind, not only a failing one); the flight-recorder dump is written for
/// every run too by [`Scenario::collect_flight_recorder`], taken before the
/// clean quit since it needs Verbatim still up to answer `DumpRecorder`.
const TIMELINE_FILE_NAME: &str = "timeline.txt";
const STDERR_FILE_NAME: &str = "stderr.log";

/// How long [`Scenario::launch_target`] waits for the launched application's
/// window before bringing it to the foreground.
const LAUNCH_FOREGROUND_TIMEOUT: Duration = Duration::from_secs(10);

/// Text in the title of every window the harness opens on purpose: the
/// document [`Scenario::open_document`] writes is named with it, so its
/// window can be told from the user's own windows of the same application,
/// found by title, and closed by title, as NVDA's system tests name their
/// Notepad documents.
pub const DOCUMENT_MARKER: &str = "verbatim-e2e-";

/// How long a window the harness asks to close is given to go.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often [`Scenario::save_document`] checks whether the document's
/// title still marks unsaved changes.
const SAVE_POLL: Duration = Duration::from_millis(50);

/// The extensions of the harness files the pre-launch sweep deletes: the
/// documents [`Scenario::open_document_with`] writes, and the fixtures
/// written at [`Scenario::harness_file`] paths, such as the spelling errors
/// scenario's `mockapp` fixture.
const HARNESS_FILE_EXTENSIONS: [&str; 2] = ["txt", "json"];

/// One application a scenario launched, for cleanup.
struct Launched {
    pid: u32,
    image: String,
    /// The title marker of the harness document it opened, when it opened
    /// one: such an application is closed by that title, never swept by
    /// image name, so the user's own windows of it are left alone.
    marker: Option<String>,
    /// Whether the launch is terminated by pid when its window does not
    /// close. Not for a launcher that may hand its window to a process the
    /// user's own windows share, such as Windows Terminal's `wt.exe`:
    /// terminating its job could end that process.
    kill_if_open: bool,
    /// The harness document it opened, deleted once its window has gone.
    document: Option<String>,
    /// Whether no window of the application was open before the harness
    /// opened its document, so the window left behind once the document's
    /// tab has closed, holding only tabs Notepad restored from its last
    /// session, is closed too.
    close_application: bool,
}
const FLIGHT_RECORDER_FILE_NAME: &str = "flight-recorder.jsonl";

/// Environment variable overriding the path to `verbatim.exe`. Defaults to
/// `target/debug/verbatim.exe` under the workspace root — the ordinary
/// local debug build, which a runner-direct run builds itself before
/// staging it (see [`build_default_source_binaries`]). Setting this skips
/// that build: the named binaries are staged as they are.
///
/// This names where [`Scenario::launch`] finds the *source* binaries to
/// stage from in runner-direct mode, not where it launches from: setting it
/// chooses the source build, never the fixed configuration regime — the
/// staged copy under `target/e2e-stage` is still what actually runs, with
/// [`verbatim_config::Settings::for_e2e`]'s settings next to it.
pub const VERBATIM_EXE_ENV: &str = "VERBATIM_E2E_VERBATIM_EXE";

/// Environment variable marking a *remote* run: the agent, Verbatim, and
/// its configuration live on another machine (the Hyper-V guest), so
/// [`VERBATIM_EXE_ENV`] names a path in that machine's filesystem, not this
/// one's. `cargo xtask vm test` sets it.
///
/// The distinction matters because [`Scenario::launch`]'s runner-direct
/// staging step — checking the source `verbatim.exe` exists, copying it and
/// `verbatim-outpost.exe` into `target/e2e-stage`, and writing the fixed
/// `settings.toml` there — is an ordinary host filesystem operation. In the
/// default runner-direct mode the suite and Verbatim share a filesystem, so
/// it is correct. Against a VM it is not: the guest path does not exist
/// here, and `cargo xtask vm deploy` has already staged the equivalent
/// inside the guest. Set this and the staging step is skipped, since the
/// deploy owns it; only `settings.toml` is written afresh, through the
/// agent, so a scenario's own settings ([`Scenario::launch_with_settings`])
/// apply in the guest too and never outlive the scenario.
pub const REMOTE_ENV: &str = "VERBATIM_E2E_REMOTE";

/// Whether this is a remote (in-guest) run; see [`REMOTE_ENV`].
fn is_remote() -> bool {
    std::env::var(REMOTE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Environment variable requesting an *audible* run: [`Scenario::launch`]
/// does not set `VERBATIM_TEST_AUDIO=null`, so Verbatim speaks through the
/// real audio device instead of the silent real-time device a default run
/// uses. Both speak through eSpeak NG and take the same time, so every
/// assertion is the same either way. `cargo xtask vm test` always sets it,
/// since every VM run is audible; set it by hand for an audible
/// runner-direct run, which then speaks over any other screen reader
/// running on the desktop.
pub const AUDIBLE_ENV: &str = "VERBATIM_E2E_AUDIBLE";

/// Whether this is an audible run; see [`AUDIBLE_ENV`].
#[must_use]
pub fn is_audible() -> bool {
    std::env::var(AUDIBLE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// How long [`Scenario::launch`] waits for Verbatim's control plane to come
/// up before giving up.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Interval between control-tunnel readiness polls.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long [`Scenario::quit_verbatim`] waits for the process to actually
/// exit after `Quit` is acknowledged (or the connection closed in its
/// place).
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`Scenario::launch`] waits for Verbatim to report itself
/// ready after the control plane answers.
///
/// The control server starts, and so the tunnel answers, before
/// `verbatim-app` hands the gesture router its `GuiHandle`, and before the
/// focus listener and the outpost reading Verbatim's own windows are
/// ready; a gesture or key sent before then can go unheard. Verbatim
/// reports when all three are ready in its status (`StatusInfo::ready`),
/// which launch waits for; this only bounds a failure.
const GUI_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// How often launch asks whether the GUI is ready.
const GUI_READY_POLL: Duration = Duration::from_millis(10);

/// Enforces one live Verbatim instance at a time within this process.
///
/// This is a same-process guard, not a cross-process one: tests must also
/// run with `--test-threads=1` (documented on this type and in every test
/// file), since Windows itself has no notion of "only one verbatim.exe" —
/// that discipline is `single_instance::acquire_replacing` inside
/// `verbatim.exe`, which *replaces* a running instance rather than
/// refusing to start, so two scenarios racing each other would each think
/// they own a instance that the other just killed out from under it.
fn live_instance_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Owns the lifecycle of one live Verbatim instance driven through the M2
/// agent: launching it with a capture-synth, audio-free configuration,
/// waiting for its control plane to answer, and exposing the connections a
/// test drives it with. Dropping a `Scenario` kills Verbatim and every
/// process it separately launched (via [`Scenario::launch_target`]),
/// unconditionally.
pub struct Scenario {
    _lock: MutexGuard<'static, ()>,
    process_agent: AgentClient,
    verbatim_pid: u32,
    control: ControlClient,
    speech: SpeechCollector,
    /// The shared action-and-speech log this scenario's gesture and key
    /// injection writes to; [`speech`](Self::speech)'s collector holds a
    /// clone of the same handle and writes utterances to it, so a failure
    /// can print both interleaved in time order. See [`crate::timeline`].
    timeline: Timeline,
    /// Extra processes launched via [`Scenario::launch_target`] or
    /// [`Scenario::open_document`], cleaned up on drop unless already
    /// removed by [`Scenario::kill_target`].
    launched: Vec<Launched>,
    /// Harness folders named by [`Scenario::harness_folder`], deleted on
    /// drop once every launched application has ended.
    folders: Vec<String>,
    /// Harness files named by [`Scenario::harness_file`], deleted on drop
    /// once every launched application has ended.
    files: Vec<String>,
    /// Whether [`Scenario::open_document_with`] had Verbatim report the
    /// focus, because a window of the application was already open (see
    /// [`Scenario::take_focus_reported`]).
    focus_reported: bool,
    /// The path this launch's Verbatim has its stdout and stderr captured
    /// into (see [`verbatim_stderr_log_path`]), readable back through
    /// [`process_agent`](Self::process_agent)'s `read_file` — what
    /// [`Scenario::collect_run_artifacts`] pulls on every run.
    stderr_log_path: String,
    /// The video of this run, while it is being captured (see
    /// [`crate::recording`]).
    recording: Option<Recording>,
}

impl Scenario {
    /// Launches a fresh Verbatim through the agent named by
    /// [`crate::ENDPOINT_ENV`].
    ///
    /// In runner-direct mode (the default — see [`REMOTE_ENV`]), first
    /// builds the default source binaries unless [`VERBATIM_EXE_ENV`]
    /// overrides them (see [`build_default_source_binaries`]), then
    /// stages `verbatim.exe`, `verbatim-outpost.exe`,
    /// `verbatim-synth-host.exe`, and `mockapp.exe` into
    /// `target/e2e-stage` under the workspace root (see [`stage_binaries`]),
    /// then writes [`verbatim_config::Settings::for_e2e`]'s fixed
    /// settings.toml there selecting eSpeak NG, and launches *that* staged
    /// copy with `VERBATIM_TEST_AUDIO=null` (the silent real-time device,
    /// still measuring complete latency timelines). The developer's own
    /// `target/debug/verbatim.exe` and its `settings.toml` are never read or
    /// written by this. In remote mode `cargo xtask vm deploy` already
    /// staged the guest side equivalently, so this launches
    /// [`VERBATIM_EXE_ENV`]'s path directly instead.
    ///
    /// Either way, before any of that, this first sweeps
    /// [`crate::registry::swept_target_image_names`] on the agent's guest,
    /// so every scenario begins from as clean a state as possible even after
    /// a prior run aborted before its own [`Drop`] cleanup ran. Once
    /// Verbatim itself is
    /// launched, this waits for its control plane to answer over the
    /// agent's tunnel and opens a second, dedicated tunnel connection for
    /// speech collection (see [`crate::speech::SpeechCollector`] for why it
    /// must not share the command connection).
    ///
    /// Under [`AUDIBLE_ENV`], `VERBATIM_TEST_AUDIO=null` is not passed, so
    /// Verbatim speaks through the real audio device.
    ///
    /// # Errors
    ///
    /// Returns an error if [`crate::ENDPOINT_ENV`] is unset, building the
    /// default source binaries fails, the source `verbatim.exe` (or, in
    /// runner-direct mode, `verbatim-outpost.exe` next to it) cannot be
    /// found, staging fails, the agent cannot be
    /// reached, or Verbatim's control plane never comes up within the
    /// launch timeout.
    pub fn launch() -> io::Result<Self> {
        Self::launch_with_settings(None)
    }

    /// [`Scenario::launch`] with `configure` applied to the fixed settings
    /// before they are written, for a scenario that needs a reader setting
    /// other than its default (see
    /// [`crate::registry::ScenarioDef::settings`]). The settings are written
    /// afresh for every launch, in a remote run too, through the agent, so
    /// one scenario's settings never reach the next.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch`], and if the settings cannot be written.
    pub fn launch_with_settings(configure: Option<fn(&mut Settings)>) -> io::Result<Self> {
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let lock = live_instance_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let verbatim_exe = verbatim_exe_path();
        let remote = is_remote();
        let audible = is_audible();
        // A runner-direct run of the default source build builds it first, so
        // it can never stage a binary older than the source under test (see
        // build_default_source_binaries). An override names a build the
        // caller chose, and a remote run's binaries were deployed by xtask.
        if !remote && std::env::var_os(VERBATIM_EXE_ENV).is_none() {
            build_default_source_binaries()?;
        }
        if !remote && !verbatim_exe.is_file() {
            return Err(io::Error::other(format!(
                "verbatim.exe not found at {} (set {VERBATIM_EXE_ENV} to override, or {REMOTE_ENV} if it lives in a guest)",
                verbatim_exe.display()
            )));
        }

        // In a remote run the path above names a location in the guest, so
        // staging cannot happen here; `cargo xtask vm deploy` staged the
        // binaries inside the guest already, and the settings are written
        // through the agent below. In
        // runner-direct mode, stage a private copy so this suite never
        // reads or writes the developer's own build output directory.
        let (launch_exe, launch_dir) = if remote {
            let exe_dir = verbatim_exe
                .parent()
                .ok_or_else(|| io::Error::other("verbatim.exe path has no parent directory"))?
                .to_path_buf();
            (verbatim_exe, exe_dir)
        } else {
            let source_dir = verbatim_exe
                .parent()
                .ok_or_else(|| io::Error::other("verbatim.exe path has no parent directory"))?;
            let stage_dir = stage_binaries(source_dir)?;
            // eSpeak NG, the default synthesizer, in every run; a silent
            // run differs only in playing through the silent device.
            write_settings(&stage_dir, run_settings(configure))?;
            let staged_exe = stage_dir.join("verbatim.exe");
            (staged_exe, stage_dir)
        };

        let exe_str = launch_exe
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe path is not valid UTF-8"))?;
        let exe_dir_str = launch_dir
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe directory is not valid UTF-8"))?;
        let stderr_path = verbatim_stderr_log_path(&launch_dir, remote)?;

        // Audible mode omits VERBATIM_TEST_AUDIO=null entirely, so
        // verbatim-app plays through the real device rather than the silent
        // one; see AUDIBLE_ENV.
        let mut launch_env: Vec<(String, String)> = if audible {
            Vec::new()
        } else {
            vec![("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned())]
        };

        let mut process_agent = AgentClient::connect(&agent_addr)?;
        if remote {
            write_remote_settings(&mut process_agent, exe_dir_str, run_settings(configure))?;
        }
        sweep_leftovers(&mut process_agent, exe_dir_str);
        // The capture starts before Verbatim, so the video shows it start.
        let mut recording = start_recording(&mut process_agent, exe_dir_str);
        if let Some(recording) = &recording {
            launch_env.push(recording.audio_env());
        }
        let started = process_agent
            .launch_process(
                exe_str,
                &[],
                Some(exe_dir_str),
                &launch_env,
                Some(&stderr_path),
            )
            .and_then(|pid| {
                connect(&agent_addr)
                    .map(|connected| (pid, connected))
                    .inspect_err(|_| {
                        let _ = process_agent.kill_process(pid);
                    })
            });
        let (verbatim_pid, (control, speech, timeline)) = match started {
            Ok(started) => started,
            Err(error) => {
                if let Some(recording) = &mut recording {
                    recording.stop(&mut process_agent);
                }
                return Err(error);
            }
        };

        // The control plane answering does not yet mean Verbatim can act on
        // input; wait until it says it can.
        let mut control = control;
        if let Err(error) = wait_for_gui(&mut control) {
            let _ = process_agent.kill_process(verbatim_pid);
            if let Some(recording) = &mut recording {
                recording.stop(&mut process_agent);
            }
            return Err(error);
        }

        Ok(Self {
            _lock: lock,
            process_agent,
            verbatim_pid,
            control,
            speech,
            timeline,
            launched: Vec::new(),
            folders: Vec::new(),
            files: Vec::new(),
            focus_reported: false,
            stderr_log_path: stderr_path,
            recording,
        })
    }

    /// Ends this run's video and saves it, with Verbatim's audio, to `to`.
    /// Best-effort: a failure is printed as a warning, never failing the
    /// scenario. Does nothing when this run is not recording.
    pub fn finish_recording(&mut self, to: &Path) {
        let Some(mut recording) = self.recording.take() else {
            return;
        };
        match recording.finish(&mut self.process_agent, to) {
            Ok(()) => println!("recording saved to {}", to.display()),
            Err(error) => eprintln!("WARNING: could not save the recording: {error}"),
        }
    }

    /// Opens one more connection to Verbatim's control plane, subscribed to
    /// the normalized events Core receives: for a scenario that waits for
    /// evidence no speech shows, such as a terminal's caret reaching its
    /// prompt. Events from before the call are not on it.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot be reached, the tunnel cannot
    /// be opened, or the subscription is refused.
    pub fn subscribe_events(&mut self) -> io::Result<ControlClient> {
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let mut events =
            AgentClient::connect(&agent_addr).and_then(AgentClient::open_control_tunnel)?;
        ok_or_error(events.request(Request::SubscribeEvents)?)?;
        Ok(events)
    }

    /// The primary control-plane connection: status, gestures, keys,
    /// latency, tree dumps, and quit.
    pub fn control(&mut self) -> &mut ControlClient {
        &mut self.control
    }

    /// The dedicated speech-collector connection.
    pub fn speech(&mut self) -> &mut SpeechCollector {
        &mut self.speech
    }

    /// Routes a gesture identifier through Verbatim's gesture router, as if
    /// the keys had been pressed (`Request::SendGesture`).
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn send_gesture(&mut self, identifier: &str) -> io::Result<()> {
        self.timeline.push_gesture(identifier);
        ok_or_error(self.control.request(Request::SendGesture {
            identifier: identifier.to_owned(),
        })?)?;
        Ok(())
    }

    /// Synthesizes real OS keyboard input reaching whatever has focus
    /// (`Request::SendKeys`), each entry a plus-joined combination such as
    /// `shift+tab`.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn send_keys(&mut self, keys: &[&str]) -> io::Result<()> {
        self.timeline.push_keys(keys);
        ok_or_error(self.control.request(Request::SendKeys {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        })?)?;
        Ok(())
    }

    /// Types `text` as real key presses through the agent's `TypeText`, each
    /// character mapped to its key and shift state in the foreground
    /// window's keyboard layout, so Verbatim's keyboard hook sees ordinary
    /// typing. Named keys such as Enter go through
    /// [`Scenario::send_keys`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when a character
    /// cannot be typed, in which case nothing was typed.
    pub fn type_text(&mut self, text: &str) -> io::Result<()> {
        self.timeline.push_text(text);
        self.process_agent.type_text(text)
    }

    /// The directory, on the agent's machine, that this run's harness
    /// files go in: the one holding Verbatim's executable and its captured
    /// log.
    ///
    /// # Errors
    ///
    /// Returns an error if the log path has no parent directory.
    pub fn run_directory(&self) -> io::Result<String> {
        Path::new(&self.stderr_log_path)
            .parent()
            .and_then(Path::to_str)
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("no directory for the run's harness files"))
    }

    /// The path, on the agent's machine, of the harness folder `name` of
    /// this run: named with [`harness_marker`], in
    /// [`Scenario::run_directory`]. The folder is made by the first file
    /// written into it ([`Scenario::write_agent_file`]), and deleted with
    /// everything in it when the scenario is dropped, after the
    /// applications it launched have ended; one an aborted run left behind
    /// is deleted by the next launch's sweep.
    ///
    /// # Errors
    ///
    /// As [`Scenario::run_directory`].
    pub fn harness_folder(&mut self, name: &str) -> io::Result<String> {
        let folder = format!(r"{}\{}", self.run_directory()?, harness_marker(name));
        if !self.folders.contains(&folder) {
            self.folders.push(folder.clone());
        }
        Ok(folder)
    }

    /// The path, on the agent's machine, of the harness file `name` of this
    /// run with `extension`: named with [`harness_marker`], in
    /// [`Scenario::run_directory`], such as a fixture an application reads.
    /// Write it with [`Scenario::write_agent_file`]. It is deleted when the
    /// scenario is dropped, after the applications it launched have ended;
    /// one an aborted run left behind is deleted by the next launch's sweep.
    ///
    /// # Errors
    ///
    /// As [`Scenario::run_directory`].
    pub fn harness_file(&mut self, name: &str, extension: &str) -> io::Result<String> {
        let file = format!(
            r"{}\{}.{extension}",
            self.run_directory()?,
            harness_marker(name)
        );
        if !self.files.contains(&file) {
            self.files.push(file.clone());
        }
        Ok(file)
    }

    /// Writes a file on the agent's machine, creating or replacing it and
    /// any missing parent directories.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn write_agent_file(&mut self, path: &str, contents: &[u8]) -> io::Result<()> {
        self.process_agent.write_file(path, contents)
    }

    /// Waits up to `timeout` for a file to exist on the agent's machine and
    /// returns its contents: the evidence a script the scenario started has
    /// reached the point that writes it.
    ///
    /// # Errors
    ///
    /// Returns the last read error if the file cannot be read within
    /// `timeout`.
    pub fn wait_for_agent_file(&mut self, path: &str, timeout: Duration) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.process_agent.read_file(path) {
                Ok(contents) => return Ok(contents),
                Err(error) if Instant::now() >= deadline => {
                    return Err(io::Error::other(format!(
                        "{path} was not written within {timeout:?}: {error}"
                    )));
                }
                Err(_) => thread::sleep(SAVE_POLL),
            }
        }
    }

    /// Launches `command`, which opens a window titled with `title`, a
    /// title of this run's own ([`harness_marker`]), and tracks it to be
    /// closed by that title at cleanup, so the user's own windows of the
    /// same program are never touched. The window is not waited for; see
    /// [`Scenario::bring_titled_window_forward`]. With `kill_if_open`
    /// false, a window that will not close is reported and left open rather
    /// than its launch terminated, for a launcher whose window may belong
    /// to a process the user's own windows share, such as `wt.exe`'s.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, for example
    /// because no such program is installed.
    pub fn launch_titled(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        kill_if_open: bool,
    ) -> io::Result<u32> {
        let pid = self
            .process_agent
            .launch_process(command, args, None, &[], None)?;
        self.launched.push(Launched {
            pid,
            image: image_name(command),
            marker: Some(title.to_owned()),
            kill_if_open,
            document: None,
            close_application: false,
        });
        Ok(pid)
    }

    /// Waits up to `timeout` for a visible top-level window whose title
    /// contains `title`, finds which program owns it (a console's window,
    /// for example, belongs to the console host rather than the shell it
    /// runs), and brings it to the foreground. Returns that program's image
    /// name.
    ///
    /// # Errors
    ///
    /// Returns an error, with the foreground report, if no such window
    /// appears or it does not take the foreground.
    pub fn bring_titled_window_forward(
        &mut self,
        title: &str,
        timeout: Duration,
    ) -> io::Result<String> {
        let deadline = Instant::now() + timeout;
        let image = loop {
            let info = self.process_agent.foreground_info()?;
            if let Some(window) = info
                .windows
                .iter()
                .find(|window| window.title.contains(title))
            {
                break window.image.clone();
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "no window titled {title:?} appeared within {timeout:?}: {}",
                    describe_foreground(&info)
                )));
            }
            thread::sleep(POLL_INTERVAL);
        };
        self.require_window_in_front(&image, Some(title))?;
        Ok(image)
    }

    /// Launches an extra target application (for example `msinfo32.exe`)
    /// through the agent and brings its window to the foreground, as a
    /// user's launch would put it in front, tracking it for cleanup on drop
    /// unless [`Scenario::kill_target`] removes it first. An application
    /// whose window does not take the foreground fails the launch, naming
    /// what held the foreground instead, so a scenario never goes on to
    /// assert speech for a window that is not in front.
    ///
    /// The image name is recorded for cleanup as well as the pid: an
    /// application can hand its window off to another process of the same
    /// image. For an application the user may also have open, such as
    /// Notepad, use [`Scenario::open_document`] instead, which never sweeps
    /// by image name.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the window does not take the
    /// foreground.
    pub fn launch_target(&mut self, command: &str, args: &[&str]) -> io::Result<u32> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let pid = self
            .process_agent
            .launch_process(command, &args, None, &[], None)?;
        let image = image_name(command);
        self.launched.push(Launched {
            pid,
            image: image.clone(),
            marker: None,
            kill_if_open: true,
            document: None,
            close_application: false,
        });
        self.require_window_in_front(&image, None)?;
        Ok(pid)
    }

    /// Opens a harness document in `application` (for example
    /// `notepad.exe`): writes an empty text file named with
    /// [`DOCUMENT_MARKER`] next to Verbatim's executable, launches the
    /// application on it, and brings the window whose title names it to the
    /// foreground, as NVDA's system tests open Notepad on a uniquely named
    /// file and wait for that window. The application is closed by that
    /// title at cleanup, so the user's own windows of it are never touched;
    /// in Notepad, the document's tab is closed rather than its window, so
    /// Notepad does not keep it for its next session, and the window too
    /// only when the harness opened it. The document is then deleted. When
    /// a window of the application was already open, Verbatim is asked to
    /// report the focus once the window holds the document (see
    /// `report_focus`), so a scenario hears the focus in full whether or
    /// not one was; [`Scenario::take_focus_reported`] tells which it hears.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the window does not take the
    /// foreground.
    pub fn open_document(&mut self, application: &str) -> io::Result<u32> {
        self.open_document_with(application, "notes", "")
    }

    /// [`Scenario::open_document`] on a document holding `contents`, named
    /// with [`DOCUMENT_MARKER`] and `name`, for a scenario that reads or
    /// edits text.
    ///
    /// # Errors
    ///
    /// As [`Scenario::open_document`].
    pub fn open_document_with(
        &mut self,
        application: &str,
        name: &str,
        contents: &str,
    ) -> io::Result<u32> {
        let marker = harness_marker(name);
        let directory = Path::new(&self.stderr_log_path)
            .parent()
            .and_then(Path::to_str)
            .ok_or_else(|| io::Error::other("no directory for the harness document"))?;
        let path = format!("{directory}\\{marker}.txt");
        self.process_agent.write_file(&path, contents.as_bytes())?;
        let image = image_name(application);
        let already_open = self
            .process_agent
            .foreground_info()?
            .windows
            .iter()
            .any(|window| window.image.eq_ignore_ascii_case(&image));
        let pid = self.process_agent.launch_process(
            application,
            std::slice::from_ref(&path),
            None,
            &[],
            None,
        )?;
        self.launched.push(Launched {
            pid,
            image: image.clone(),
            marker: Some(marker.clone()),
            kill_if_open: true,
            document: Some(path),
            close_application: !already_open,
        });
        self.require_window_in_front(&image, Some(&marker))?;
        self.focus_reported = already_open;
        if already_open {
            self.report_focus()?;
        }
        Ok(pid)
    }

    /// Whether [`Scenario::open_document_with`] found a window of the
    /// application already open, and so had Verbatim report the focus with
    /// Verbatim+Tab: the scenario then hears the focus reported, the
    /// harness tab's text area and the line at its caret, rather than the
    /// window coming to the foreground and its text area taking the focus.
    /// It concerns only what is heard first after opening, so it is cleared
    /// as it is read: a later return to the window is heard as the window
    /// coming to the foreground again, whichever way it was opened.
    pub fn take_focus_reported(&mut self) -> bool {
        std::mem::take(&mut self.focus_reported)
    }

    /// Has Verbatim report the focus, once the harness document's window
    /// is in front holding the document. Windows 11 Notepad opens a
    /// document in a window already open as a new tab: the window comes to
    /// the foreground still showing the tab it had, whose text area takes
    /// the focus and is announced, and only then switches to the new tab,
    /// whose text area takes the focus again and cuts that announcement
    /// off. The window's title naming the harness document, which the
    /// caller has waited for, is the evidence the switch has happened; once
    /// everything said up to then has ended, Verbatim+Tab reports the
    /// focus, as a user would ask where they are, and the scenario hears
    /// the harness tab's text area reported once, in full.
    fn report_focus(&mut self) -> io::Result<()> {
        self.speech.wait_until_quiet(LAUNCH_FOREGROUND_TIMEOUT);
        self.send_gesture("kb:verbatim+tab")
    }

    /// Saves the harness document `name` ([`Scenario::open_document_with`])
    /// when its window is in front with unsaved changes, and waits until its
    /// title no longer marks them. An edited document left unsaved would be
    /// restored by Windows 11 Notepad the next time it opens, which then
    /// asks whether to keep the changes when the harness writes the file
    /// afresh. Nothing is sent when another window is in front.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the title still marks unsaved
    /// changes after `timeout`.
    pub fn save_document(&mut self, name: &str, timeout: Duration) -> io::Result<()> {
        let marker = harness_marker(name);
        let unsaved_in_front = |agent: &mut AgentClient| -> io::Result<bool> {
            Ok(agent.foreground_info()?.foreground.is_some_and(|window| {
                window.title.contains(&marker) && window.title.starts_with('*')
            }))
        };
        if !unsaved_in_front(&mut self.process_agent)? {
            return Ok(());
        }
        self.send_keys(&["control+s"])?;
        let deadline = Instant::now() + timeout;
        while unsaved_in_front(&mut self.process_agent)? {
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "{marker} still had unsaved changes after {timeout:?}"
                )));
            }
            thread::sleep(SAVE_POLL);
        }
        Ok(())
    }

    /// Waits until the harness document `name`'s window, in front, marks
    /// unsaved changes in its title: the evidence that an edit with nothing
    /// to hear, such as a paste, has reached the document.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the title does not mark
    /// unsaved changes within `timeout`.
    pub fn expect_unsaved(&mut self, name: &str, timeout: Duration) -> io::Result<()> {
        let marker = harness_marker(name);
        let deadline = Instant::now() + timeout;
        loop {
            let unsaved = self
                .process_agent
                .foreground_info()?
                .foreground
                .is_some_and(|window| {
                    window.title.contains(&marker) && window.title.starts_with('*')
                });
            if unsaved {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "{marker} did not show unsaved changes within {timeout:?}"
                )));
            }
            thread::sleep(SAVE_POLL);
        }
    }

    /// Opens a harness folder in File Explorer: writes `files` (paths
    /// relative to the folder, empty contents) into a folder named
    /// [`DOCUMENT_MARKER`] plus `name` next to Verbatim's executable
    /// ([`Scenario::harness_folder`]), opens it, and brings the window
    /// titled with that name to the foreground. The window is closed by its
    /// title at cleanup, never by image name, since `explorer.exe` is also
    /// the shell, and the folder is then deleted. Returns the folder's name,
    /// which is the window's title.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the window does not take the
    /// foreground.
    pub fn open_folder(&mut self, name: &str, files: &[&str]) -> io::Result<String> {
        let marker = harness_marker(name);
        let folder = self.harness_folder(name)?;
        for file in files {
            self.process_agent
                .write_file(&format!("{folder}\\{file}"), b"")?;
        }
        let pid = self.process_agent.launch_process(
            "explorer.exe",
            std::slice::from_ref(&folder),
            None,
            &[],
            None,
        )?;
        self.launched.push(Launched {
            pid,
            image: "explorer.exe".to_owned(),
            marker: Some(marker.clone()),
            kill_if_open: true,
            document: None,
            close_application: false,
        });
        self.require_window_in_front("explorer.exe", Some(&marker))?;
        Ok(marker)
    }

    /// Opens a page of the Settings app by its `ms-settings:` URI and brings
    /// the Settings window to the foreground. The Settings app is a single
    /// instance, so the scenario lists `SystemSettings.exe` among its target
    /// images and it is ended by image name at cleanup.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the window does not take the
    /// foreground.
    pub fn open_settings_page(&mut self, uri: &str) -> io::Result<()> {
        let pid = self.process_agent.launch_process(
            "explorer.exe",
            &[uri.to_owned()],
            None,
            &[],
            None,
        )?;
        // The launching explorer.exe hands the URI to the Settings app and
        // exits; only SystemSettings.exe is swept at cleanup.
        self.launched.push(Launched {
            pid,
            image: "SystemSettings.exe".to_owned(),
            marker: None,
            kill_if_open: true,
            document: None,
            close_application: false,
        });
        self.require_window_in_front("ApplicationFrameHost.exe", Some("Settings"))
    }

    /// Brings `image`'s window, titled with `title_contains` when given, to
    /// the foreground, failing with the foreground report when it does not
    /// get there.
    fn require_window_in_front(
        &mut self,
        image: &str,
        title_contains: Option<&str>,
    ) -> io::Result<()> {
        if self.process_agent.bring_to_foreground(
            image,
            title_contains,
            LAUNCH_FOREGROUND_TIMEOUT,
        )? {
            return Ok(());
        }
        let window =
            title_contains.map_or_else(String::new, |title| format!(" (window titled {title:?})"));
        Err(io::Error::other(format!(
            "{image}{window} did not take the foreground: {}",
            self.foreground_report()
        )))
    }

    /// Waits until no window titled with `title_contains` is in the
    /// foreground: evidence that a dialog closed, for a step that must not
    /// race the close. A key sent right after the one that closes a dialog
    /// can otherwise reach the dialog's thread first, since Windows hands a
    /// thread its posted messages before its pending input. `timeout` only
    /// bounds failure.
    ///
    /// # Errors
    ///
    /// Returns an error, with the foreground report, if the window is still
    /// in front when `timeout` runs out, or if the foreground cannot be read.
    pub fn wait_for_window_to_close(
        &mut self,
        title_contains: &str,
        timeout: Duration,
    ) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let info = self.process_agent.foreground_info()?;
            let open = info
                .foreground
                .as_ref()
                .is_some_and(|window| window.title.contains(title_contains));
            if !open {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "the window titled {title_contains:?} stayed in front: {}",
                    describe_foreground(&info)
                )));
            }
            thread::sleep(POLL_INTERVAL);
        }
    }

    /// One line describing the foreground window and the visible windows,
    /// for failure messages and the run's artifacts.
    pub fn foreground_report(&mut self) -> String {
        match self.process_agent.foreground_info() {
            Ok(info) => describe_foreground(&info),
            Err(error) => format!("the foreground could not be read: {error}"),
        }
    }

    /// Establishes the state every scenario starts from: a real, uncloaked
    /// window in the foreground. The Start menu's search window can be left
    /// holding the foreground, cloaked, after it closes; the desktop is then
    /// brought to the foreground instead. Fails, with the foreground report,
    /// when that state cannot be reached, as NVDA's system tests fail with
    /// the foreground window's title.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the state cannot be reached.
    pub fn establish_baseline(&mut self) -> io::Result<()> {
        if usable_foreground(&self.process_agent.foreground_info()?) {
            return Ok(());
        }
        let _ = self.process_agent.bring_to_foreground(
            "explorer.exe",
            Some("Program Manager"),
            LAUNCH_FOREGROUND_TIMEOUT,
        )?;
        let info = self.process_agent.foreground_info()?;
        if usable_foreground(&info) {
            return Ok(());
        }
        Err(io::Error::other(format!(
            "no usable foreground window to start from: {}",
            describe_foreground(&info)
        )))
    }

    /// Ends every process named `image` (for example
    /// `verbatim-synth-host.exe`), returning how many were ended.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn kill_processes_by_name(&mut self, image: &str) -> io::Result<u32> {
        self.process_agent.kill_processes_by_name(image)
    }

    /// Ends an application a scenario launched and stops tracking it: one
    /// that opened a harness document is closed by its title, and
    /// terminated by pid only if it does not close; any other is
    /// terminated by pid and then swept by image name, since its window
    /// may belong to a process it handed off to.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails.
    pub fn kill_target(&mut self, pid: u32) -> io::Result<KillOutcome> {
        let Some(index) = self
            .launched
            .iter()
            .position(|launched| launched.pid == pid)
        else {
            return self.process_agent.kill_process(pid);
        };
        let launched = self.launched.remove(index);
        self.end(&launched)
    }

    fn end(&mut self, launched: &Launched) -> io::Result<KillOutcome> {
        if let Some(marker) = &launched.marker {
            let outcome = self.close_marked(launched, marker);
            if let Some(path) = &launched.document
                && let Err(error) = self.process_agent.delete_file(path)
            {
                tracing::warn!(path, %error, "failed to delete a harness document");
            }
            return outcome;
        }
        let outcome = self.process_agent.kill_process(launched.pid)?;
        if let Err(error) = self.process_agent.kill_processes_by_name(&launched.image) {
            tracing::warn!(
                pid = launched.pid,
                image = launched.image,
                %error,
                "failed to sweep by image name"
            );
        }
        Ok(outcome)
    }

    /// Closes an application opened on a harness document or window, by
    /// its title: Notepad's harness tab first (see [`close_notepad_tabs`]),
    /// then, when the harness opened Notepad's window, the window it leaves.
    fn close_marked(&mut self, launched: &Launched, marker: &str) -> io::Result<KillOutcome> {
        if is_notepad(&launched.image) {
            if let Err(error) = close_notepad_tabs(&mut self.process_agent, marker) {
                tracing::warn!(marker, %error, "a harness tab did not close; closing its window");
            }
            if launched.close_application {
                close_notepad_windows(&mut self.process_agent)?;
            }
        }
        let remaining = self.process_agent.close_windows(marker, CLOSE_TIMEOUT)?;
        if remaining == 0 {
            return Ok(KillOutcome::AlreadyExited);
        }
        if !launched.kill_if_open {
            return Err(io::Error::other(format!(
                "{remaining} window(s) titled {marker:?} did not close, and are left open"
            )));
        }
        tracing::warn!(marker, remaining, "a harness document window did not close");
        self.process_agent.kill_process(launched.pid)
    }

    /// Asks the agent whether `pid` is still running.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn process_status(&mut self, pid: u32) -> io::Result<ProcessState> {
        self.process_agent.process_status(pid)
    }

    /// Asks Verbatim to exit cleanly through the control plane
    /// (`Request::Quit`), then confirms through the agent that the process
    /// actually exited.
    ///
    /// The reply to `Quit` races Verbatim's own teardown: the control server
    /// is dropped as the process exits, and the connection can close before
    /// the queued `Ok` frame is written or relayed through the tunnel. A
    /// closed connection after sending `Quit` is therefore treated as
    /// success, not an error — the request's entire purpose is that the
    /// process goes away — and the authoritative confirmation is the agent
    /// reporting the pid as exited, which this polls for.
    ///
    /// # Errors
    ///
    /// Returns an error if the request cannot be sent, an explicit error
    /// frame comes back, or the process is still running after the exit
    /// deadline.
    pub fn quit_verbatim(&mut self) -> io::Result<()> {
        match self.control.request(Request::Quit) {
            Ok(frame) => {
                ok_or_error(frame)?;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                ) =>
            {
                // Verbatim tore down before the reply arrived (end of file),
                // or the reply outlasted the socket's fixed read timeout on a
                // slow teardown — both observed live. Either way the reply is
                // not the authority on whether the quit worked; the process
                // poll below is, so a lost reply is tolerated and a process
                // that will not die still fails.
            }
            Err(error) => return Err(error),
        }

        let deadline = Instant::now() + QUIT_TIMEOUT;
        loop {
            match self.process_agent.process_status(self.verbatim_pid)? {
                ProcessState::Exited { .. } => return Ok(()),
                ProcessState::Running if Instant::now() >= deadline => {
                    return Err(io::Error::other(format!(
                        "Verbatim (pid {}) is still running {} seconds after Quit",
                        self.verbatim_pid,
                        QUIT_TIMEOUT.as_secs()
                    )));
                }
                ProcessState::Running => thread::sleep(POLL_INTERVAL),
            }
        }
    }

    /// Fetches, asserts, and prints the scenario's recent latency
    /// timelines; see [`crate::latency::report`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if any returned record has no audio-start timestamp.
    pub fn report_latency(&mut self, last_n: u32) -> io::Result<Vec<LatencyRecord>> {
        crate::latency::report(&mut self.control, last_n)
    }

    /// Fetches the scenario's recent latency timelines with no printing and
    /// no assertion; see [`crate::latency::fetch`]. Used by
    /// [`crate::registry::run`] to fill in every scenario's run summary
    /// (`docs/roadmap.md`'s M3 Track B item), regardless of whether that
    /// scenario itself calls [`report_latency`](Self::report_latency).
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn latency_snapshot(&mut self, last_n: u32) -> io::Result<Vec<LatencyRecord>> {
        crate::latency::fetch(&mut self.control, last_n)
    }

    /// Collects the always-on run artifacts into `dir` (created if missing):
    /// the interleaved [`crate::timeline::Timeline`] (`timeline.txt`, also
    /// printed on an `expect_*` panic — this additionally writes it to a file)
    /// and Verbatim's captured stderr log (`stderr.log`, via the agent's
    /// `read_file` from the path this launch already told the agent to capture
    /// into). Written for every run, pass or fail, so a passing run
    /// still leaves enough to read announcement timings (the timeline's
    /// millisecond offsets) and outpost-ready timestamps (Verbatim's stderr).
    ///
    /// Reads only the agent and this scenario's own in-memory timeline, never
    /// Verbatim's control connection, so it is correct to call after
    /// [`Scenario::quit_verbatim`] has already torn Verbatim down — indeed the
    /// stderr log is most complete once the process has exited and flushed.
    ///
    /// Best-effort: each piece is attempted independently and a failure on one
    /// is logged to stderr rather than aborting the other or the run.
    pub fn collect_run_artifacts(&mut self, dir: &Path) {
        if let Err(error) = fs::create_dir_all(dir) {
            eprintln!(
                "could not create run-artifacts directory {}: {error}",
                dir.display()
            );
            return;
        }

        if let Err(error) = fs::write(dir.join(TIMELINE_FILE_NAME), self.timeline.render()) {
            eprintln!("could not write the run timeline: {error}");
        }

        match self.process_agent.read_file(&self.stderr_log_path) {
            Ok(bytes) => {
                if let Err(error) = fs::write(dir.join(STDERR_FILE_NAME), bytes) {
                    eprintln!("could not write Verbatim's fetched stderr log: {error}");
                }
            }
            Err(error) => {
                eprintln!(
                    "could not read Verbatim's stderr log at {} back through the agent: {error}",
                    self.stderr_log_path
                );
            }
        }

        self.collect_outpost_logs(dir);
    }

    /// Fetches every outpost and listener log this Verbatim launch wrote,
    /// from its own directory under `logs` next to Verbatim's executable
    /// (`logs\<Verbatim's pid>`, one per launch, so nothing from another run
    /// is mixed in), into `dir` under the names the supervisor gave them:
    /// `listener.log` and `outpost-<image>-<pid>.log` per application, Core's
    /// own as `outpost-verbatim-<pid>.log`. Listing the directory finds the
    /// application that actually held the window even when a launch handed
    /// off to another process, as Windows 11 Notepad does.
    fn collect_outpost_logs(&mut self, dir: &Path) {
        let Some(logs_dir) = self.outpost_logs_dir() else {
            eprintln!("could not derive the outpost logs directory from the stderr log path");
            return;
        };
        let names = match self.process_agent.list_files(&logs_dir) {
            Ok(names) => names,
            Err(error) => {
                eprintln!("could not list {logs_dir} through the agent: {error}");
                return;
            }
        };
        if !names.iter().any(|name| name == "listener.log") {
            eprintln!("{logs_dir} holds no listener.log; the focus listener never started");
        }
        for name in names {
            let remote_path = format!(r"{logs_dir}\{name}");
            match self.process_agent.read_file(&remote_path) {
                Ok(bytes) => {
                    if let Err(error) = fs::write(dir.join(&name), bytes) {
                        eprintln!("could not write the fetched {name}: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("could not read {remote_path} back through the agent: {error}");
                }
            }
        }
    }

    /// This launch's log directory, `logs\<Verbatim's pid>` next to
    /// Verbatim's executable, derived from the captured stderr log path's
    /// parent (both live in the same directory — see
    /// [`verbatim_stderr_log_path`]).
    fn outpost_logs_dir(&self) -> Option<String> {
        Path::new(&self.stderr_log_path)
            .parent()?
            .join("logs")
            .join(self.verbatim_pid.to_string())
            .to_str()
            .map(str::to_owned)
    }

    /// Dumps the reducer flight recorder into `dir` (created if missing): a
    /// flight-recorder dump (`Request::DumpRecorder` returns the path Core
    /// wrote it to — same machine as [`stderr_log_path`](Self::stderr_log_path)
    /// in either mode, since Core and Verbatim's own stderr capture are the
    /// same process — read back through the agent). The always-on timeline and
    /// stderr log are written separately by [`Scenario::collect_run_artifacts`].
    ///
    /// Requires Verbatim to still be answering its control plane, so
    /// [`crate::registry::run`] calls this on every run *before* the clean quit
    /// (and, on a failing run, while Verbatim is still up because the quit was
    /// skipped): the reducer inputs it captures are wanted for a passing run
    /// too, to chase symptoms the pass/fail verdict alone does not explain.
    /// Best-effort, deliberately never itself a source of test failure — the
    /// control connection or the agent may be in a degraded state (Verbatim
    /// crashed, the tunnel dropped) — so a failure is logged, not propagated.
    pub fn collect_flight_recorder(&mut self, dir: &Path) {
        if let Err(error) = fs::create_dir_all(dir) {
            eprintln!(
                "could not create failure-artifacts directory {}: {error}",
                dir.display()
            );
            return;
        }

        match self.control.request(Request::DumpRecorder) {
            Ok(frame) => match ok_or_error(frame) {
                Ok(Frame::Reply {
                    payload: ReplyPayload::DumpRecorder { path },
                    ..
                }) => match self.process_agent.read_file(&path) {
                    Ok(bytes) => {
                        if let Err(error) = fs::write(dir.join(FLIGHT_RECORDER_FILE_NAME), bytes) {
                            eprintln!("could not write the fetched flight-recorder dump: {error}");
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "could not read the flight-recorder dump at {path} back through the agent: {error}"
                        );
                    }
                },
                Ok(other) => {
                    eprintln!(
                        "unexpected reply to DumpRecorder while collecting failure artifacts: {other:?}"
                    );
                }
                Err(error) => eprintln!("DumpRecorder was refused: {error}"),
            },
            Err(error) => eprintln!("could not request a flight-recorder dump: {error}"),
        }
    }
}

impl Drop for Scenario {
    fn drop(&mut self) {
        // End every launched application (see `kill_target`). Best-effort:
        // a failure is logged, not propagated, since this runs even when
        // the test itself already failed or panicked.
        for launched in std::mem::take(&mut self.launched) {
            if let Err(error) = self.end(&launched) {
                tracing::warn!(
                    pid = launched.pid,
                    %error,
                    "failed to end a scenario-launched application during cleanup"
                );
            }
        }
        for folder in std::mem::take(&mut self.folders) {
            if let Err(error) = delete_harness_folder(
                &mut self.process_agent,
                &folder,
                Instant::now() + CLOSE_TIMEOUT,
            ) {
                tracing::warn!(folder, %error, "failed to delete a harness folder");
            }
        }
        for file in std::mem::take(&mut self.files) {
            if let Err(error) = self.process_agent.delete_file(&file) {
                tracing::warn!(file, %error, "failed to delete a harness file");
            }
        }
        // Best-effort clean quit first (a no-op if the connection is
        // already gone), then guarantee Verbatim is actually gone via the
        // agent regardless of whether Quit landed — the guard's whole
        // point is that this happens even if the test panicked before
        // reaching its own quit step.
        let _ = self.control.request(Request::Quit);
        if let Err(error) = self.process_agent.kill_process(self.verbatim_pid) {
            tracing::warn!(
                pid = self.verbatim_pid,
                %error,
                "failed to kill Verbatim during scenario cleanup"
            );
        }
        if let Some(recording) = &mut self.recording {
            recording.stop(&mut self.process_agent);
        }
    }
}

/// Sweeps known target-application image names, and closes by title the
/// harness documents and windows a prior run left open (so the user's own
/// windows of the same application are left alone), so a scenario starts
/// from as clean a state as possible even after a prior run aborted without
/// running its own Drop cleanup (a killed test process, a Ctrl+C, a panic
/// that unwound past Scenario somehow). A harness tab left in Notepad is
/// closed as a tab ([`close_notepad_tabs`]), and the harness documents,
/// files ([`HARNESS_FILE_EXTENSIONS`]), and folders left in `directory`,
/// Verbatim's launch directory, are deleted. Best-effort: a failure is
/// logged, not fatal to the launch.
fn sweep_leftovers(agent: &mut AgentClient, directory: &str) {
    for name in crate::registry::swept_target_image_names() {
        if let Err(error) = agent.kill_processes_by_name(name) {
            tracing::warn!(name, %error, "failed to pre-launch sweep a target image name");
        }
    }
    if let Err(error) = close_notepad_tabs(agent, DOCUMENT_MARKER) {
        tracing::warn!(%error, "failed to close leftover harness tabs in Notepad");
    }
    if let Err(error) = agent.close_windows(DOCUMENT_MARKER, CLOSE_TIMEOUT) {
        tracing::warn!(%error, "failed to close leftover harness documents");
    }
    match agent.list_files(directory) {
        Ok(names) => {
            for name in names.iter().filter(|name| {
                name.starts_with(DOCUMENT_MARKER)
                    && Path::new(name).extension().is_some_and(|extension| {
                        HARNESS_FILE_EXTENSIONS
                            .iter()
                            .any(|harness| extension.eq_ignore_ascii_case(harness))
                    })
            }) {
                if let Err(error) = agent.delete_file(&format!("{directory}\\{name}")) {
                    tracing::warn!(name, %error, "failed to delete a leftover harness file");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "failed to list leftover harness documents"),
    }
    match agent.list_folders(directory) {
        Ok(names) => {
            for name in names
                .iter()
                .filter(|name| name.starts_with(DOCUMENT_MARKER))
            {
                let folder = format!(r"{directory}\{name}");
                if let Err(error) = delete_harness_folder(agent, &folder, Instant::now()) {
                    tracing::warn!(name, %error, "failed to delete a leftover harness folder");
                }
            }
        }
        Err(error) => tracing::warn!(%error, "failed to list leftover harness folders"),
    }
}

/// Deletes the harness folder at `path` with everything in it, refusing
/// any folder whose name does not start with [`DOCUMENT_MARKER`]. A program
/// whose window has just closed can still have the folder open while it
/// exits, such as the shell a terminal ran in it, so a failed delete is
/// tried again until the folder is gone or `deadline` passes.
fn delete_harness_folder(agent: &mut AgentClient, path: &str, deadline: Instant) -> io::Result<()> {
    if !is_harness_name(path) {
        return Err(io::Error::other(format!(
            "{path} is not a harness folder, so it is not deleted"
        )));
    }
    loop {
        match agent.delete_folder(path) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(SAVE_POLL),
        }
    }
}

/// Whether the last component of `path`, a path on the agent's machine,
/// is named with [`DOCUMENT_MARKER`].
fn is_harness_name(path: &str) -> bool {
    path.rsplit(['\\', '/'])
        .next()
        .is_some_and(|name| name.starts_with(DOCUMENT_MARKER))
}

/// Whether `image` is Windows 11 Notepad's.
fn is_notepad(image: &str) -> bool {
    image.eq_ignore_ascii_case("notepad.exe")
}

/// Closes every Notepad tab whose title holds `marker`, one at a time, by
/// bringing its window to the foreground and pressing Control+W, saving it
/// first when its title marks unsaved changes. Windows 11 Notepad keeps
/// every tab of a window that closes for its next session, so closing the
/// harness document's window would leave its tab behind for good, while a
/// closed tab is forgotten; a window whose last tab closes closes with it.
/// The window's title names its selected tab, so only a harness tab ever
/// gets the key, and the user's own tabs in the same window are left as
/// they were. Each step waits for its evidence, the title the window had
/// going away, within [`CLOSE_TIMEOUT`] in all.
fn close_notepad_tabs(agent: &mut AgentClient, marker: &str) -> io::Result<()> {
    let deadline = Instant::now() + CLOSE_TIMEOUT;
    loop {
        let Some(window) = agent
            .foreground_info()?
            .windows
            .into_iter()
            .find(|window| is_notepad(&window.image) && window.title.contains(marker))
        else {
            return Ok(());
        };
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::other(format!(
                "Notepad's tab {:?} did not close within {CLOSE_TIMEOUT:?}",
                window.title
            )));
        }
        agent.bring_to_foreground(&window.image, Some(marker), remaining)?;
        // Checked again just before the key: Control+W closes whichever tab
        // the window in front has selected.
        let Some(front) = agent
            .foreground_info()?
            .foreground
            .filter(|front| is_notepad(&front.image) && front.title.contains(marker))
        else {
            return Err(io::Error::other(format!(
                "Notepad's tab {:?} could not be brought to the foreground",
                window.title
            )));
        };
        let key = if front.title.starts_with('*') {
            "control+s"
        } else {
            "control+w"
        };
        agent.send_keys(&[key.to_owned()])?;
        wait_for_title_gone(agent, &front.title, deadline)?;
    }
}

/// Waits until no visible top-level window is titled exactly `title`.
fn wait_for_title_gone(agent: &mut AgentClient, title: &str, deadline: Instant) -> io::Result<()> {
    loop {
        let present = agent
            .foreground_info()?
            .windows
            .iter()
            .any(|window| window.title == title);
        if !present {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "the window titled {title:?} was still there after {CLOSE_TIMEOUT:?}"
            )));
        }
        thread::sleep(SAVE_POLL);
    }
}

/// Closes every Notepad window, each by its own title, once the harness's
/// tab has closed in a window the harness opened: what is left holds only
/// the tabs Notepad restored from its last session, which it keeps again.
fn close_notepad_windows(agent: &mut AgentClient) -> io::Result<()> {
    let titles: Vec<String> = agent
        .foreground_info()?
        .windows
        .into_iter()
        .filter(|window| is_notepad(&window.image))
        .map(|window| window.title)
        .collect();
    for title in titles {
        let remaining = agent.close_windows(&title, CLOSE_TIMEOUT)?;
        if remaining > 0 {
            tracing::warn!(title, remaining, "a Notepad window did not close");
        }
    }
    Ok(())
}

/// Starts this run's video, when recording (see [`crate::recording`]).
fn start_recording(agent: &mut AgentClient, dir: &str) -> Option<Recording> {
    if !crate::recording::enabled() {
        return None;
    }
    Recording::start(agent, dir)
        .inspect_err(|error| {
            eprintln!("not recording a video: ffmpeg could not be started: {error}");
        })
        .ok()
}

/// Connects to a just-launched Verbatim: the command connection, then a
/// second one subscribed to its speech.
fn connect(agent_addr: &str) -> io::Result<(ControlClient, SpeechCollector, Timeline)> {
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    let control = wait_for_control_tunnel(agent_addr, deadline).map_err(|error| {
        io::Error::other(format!("Verbatim's control plane never came up: {error}"))
    })?;
    let speech_tunnel = wait_for_control_tunnel(agent_addr, deadline).map_err(|error| {
        io::Error::other(format!(
            "could not open a second control-plane tunnel for speech: {error}"
        ))
    })?;
    let timeline = Timeline::new();
    let speech = SpeechCollector::subscribe(speech_tunnel, timeline.clone())
        .map_err(|error| io::Error::other(format!("could not subscribe to speech: {error}")))?;
    Ok((control, speech, timeline))
}

/// Repeatedly connects a fresh [`AgentClient`] and attempts
/// [`AgentClient::open_control_tunnel`] until it succeeds or `deadline`
/// passes. A fresh connection per attempt is deliberate and simple: the
/// agent happily serves many connections (one thread each), and an
/// `OpenControlTunnel` that fails because Verbatim has not created its pipe
/// yet leaves nothing worth reusing.
fn wait_for_control_tunnel(agent_addr: &str, deadline: Instant) -> io::Result<ControlClient> {
    let mut last_error = io::Error::other("launch timeout elapsed before any attempt");
    while Instant::now() < deadline {
        match AgentClient::connect(agent_addr).and_then(AgentClient::open_control_tunnel) {
            Ok(client) => return Ok(client),
            Err(error) => last_error = error,
        }
        thread::sleep(POLL_INTERVAL);
    }
    Err(last_error)
}

/// The workspace root, computed from this crate's own manifest directory
/// (`<root>/crates/verbatim-e2e`), so the default `verbatim.exe` location
/// does not depend on the caller's working directory. `pub(crate)` because
/// [`crate::artifacts::artifacts_root`] reuses it for the same reason:
/// `target/e2e-artifacts` sits next to `target/e2e-stage`, both under the
/// same workspace root.
pub(crate) fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("verbatim-e2e lives two directories under the workspace root")
        .to_path_buf()
}

/// The source `verbatim.exe` path: [`VERBATIM_EXE_ENV`] when set, otherwise
/// `target/debug/verbatim.exe` under the workspace root. In a remote run
/// this is launched directly; in runner-direct mode it is only the source
/// [`stage_binaries`] copies from, never launched itself.
fn verbatim_exe_path() -> PathBuf {
    resolve_verbatim_exe_path(std::env::var(VERBATIM_EXE_ENV).ok())
}

/// The pure resolution logic behind [`verbatim_exe_path`], split out so it
/// can be unit tested without mutating the real process environment:
/// `override_path`, when given, wins outright, otherwise the default under
/// [`workspace_root`].
fn resolve_verbatim_exe_path(override_path: Option<String>) -> PathBuf {
    if let Some(path) = override_path {
        return PathBuf::from(path);
    }
    workspace_root()
        .join("target")
        .join("debug")
        .join("verbatim.exe")
}

/// The name of the file a launched Verbatim's stdout and stderr are captured
/// into, next to its executable; `cargo xtask vm logs` pulls it by this name.
pub const VERBATIM_STDERR_LOG_NAME: &str = "stderr-e2e.log";

/// The path a launched Verbatim's stdout and stderr are captured into (see
/// [`crate::agent_client::AgentClient::launch_process`]'s `stderr_to`),
/// truncated fresh on every launch so each scenario's log is its own and
/// never a stale mix of a previous run's crash.
///
/// In a remote run this is a fixed guest-side path matching what
/// `cargo xtask vm logs` pulls back out (`xtask/src/vm/logs.rs`); `exe_dir`
/// is ignored in that case since it names a location on this host, not the
/// guest. In runner-direct mode it sits next to the staged `verbatim.exe`
/// copy itself, alongside its `settings.toml`.
fn verbatim_stderr_log_path(exe_dir: &Path, remote: bool) -> io::Result<String> {
    if remote {
        return Ok(format!(
            r"C:\VerbatimLab\verbatim\{VERBATIM_STDERR_LOG_NAME}"
        ));
    }
    exe_dir
        .join(VERBATIM_STDERR_LOG_NAME)
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("stderr log path is not valid UTF-8"))
}

/// Builds `verbatim-app` and `verbatim-outpost` (debug profile, the default
/// source build [`verbatim_exe_path`] resolves to) once per test process.
///
/// `cargo test -p verbatim-e2e` builds only this crate and its library
/// dependencies, never Verbatim's executables, so without this a
/// runner-direct run silently staged whatever `target/debug/verbatim.exe`
/// a past build left behind. Building here is a no-op when nothing changed.
/// Its output goes straight to the terminal. The outcome is remembered, so
/// each test process builds at most once.
///
/// # Errors
///
/// Returns an error if cargo cannot be launched or the build fails.
fn build_default_source_binaries() -> io::Result<()> {
    static OUTCOME: OnceLock<Result<(), String>> = OnceLock::new();
    OUTCOME
        .get_or_init(|| {
            // Cargo sets CARGO for the processes it runs, tests included.
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = std::process::Command::new(cargo)
                .args([
                    "build",
                    "-p",
                    "verbatim-app",
                    "-p",
                    "verbatim-outpost",
                    "-p",
                    "verbatim-synth-host",
                    "-p",
                    "mockapp",
                ])
                .current_dir(workspace_root())
                .status()
                .map_err(|error| format!("could not launch cargo build: {error}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "cargo build -p verbatim-app -p verbatim-outpost -p verbatim-synth-host \
                     -p mockapp failed ({status}); set \
                     {VERBATIM_EXE_ENV} to stage an existing build instead"
                ))
            }
        })
        .clone()
        .map_err(io::Error::other)
}

/// Copies `verbatim.exe` and `verbatim-outpost.exe` from `source_dir` (the
/// directory [`verbatim_exe_path`] resolved — the ordinary build output, or
/// [`VERBATIM_EXE_ENV`]'s override directory) into the fixed staging
/// directory `target/e2e-stage` under the workspace root, and returns that
/// staging directory.
///
/// The outpost binary must come along, not just `verbatim.exe`:
/// `verbatim_outpost::supervisor::Supervisor::new` resolves it next to
/// whatever `verbatim.exe` is actually running as (`std::env::current_exe`),
/// so once the launched copy lives in the staging directory, the outpost
/// must live there too or the supervisor cannot find it.
///
/// A file already staged with matching contents is left alone rather than
/// re-copied (see [`files_match`]) — the common case in a tight edit-test
/// loop where nothing changed since the last run.
///
/// # Errors
///
/// Returns an error if a required source binary is missing or a copy fails.
fn stage_binaries(source_dir: &Path) -> io::Result<PathBuf> {
    let stage_dir = workspace_root().join("target").join("e2e-stage");
    copy_into_stage(source_dir, &stage_dir)?;
    Ok(stage_dir)
}

/// The executables a run needs side by side: the app finds the outpost and
/// the synthesizer host next to itself, and a scenario reading scripted text
/// launches `mockapp` from the run's directory
/// ([`Scenario::run_directory`]).
const STAGED_BINARIES: [&str; 4] = [
    "verbatim.exe",
    "verbatim-outpost.exe",
    "verbatim-synth-host.exe",
    "mockapp.exe",
];

/// The directory-parameterized core of [`stage_binaries`], split out so unit
/// tests can exercise the hash-skip and missing-source-binary behavior
/// against temporary directories instead of the real workspace's
/// `target/e2e-stage` (which [`stage_binaries`] hardwires as its
/// destination).
fn copy_into_stage(source_dir: &Path, stage_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(stage_dir)?;
    for name in STAGED_BINARIES {
        let source = source_dir.join(name);
        if !source.is_file() {
            return Err(io::Error::other(format!(
                "{name} not found at {} (set {VERBATIM_EXE_ENV} to override its directory, or {REMOTE_ENV} if it lives in a guest)",
                source.display()
            )));
        }
        let destination = stage_dir.join(name);
        if !files_match(&source, &destination)? {
            fs::copy(&source, &destination)?;
        }
    }
    copy_dir_into_stage(&source_dir.join(ESPEAK_DATA), &stage_dir.join(ESPEAK_DATA))?;
    copy_dir_into_stage(&source_dir.join(SOUNDS), &stage_dir.join(SOUNDS))
}

/// eSpeak NG's data directory, which the synthesizer host reads next to
/// itself; the eSpeak NG crate's build puts it next to the executables.
const ESPEAK_DATA: &str = "espeak-ng-data";

/// The shared sounds the default theme plays, which Verbatim reads next to
/// itself; `verbatim-app`'s build puts them next to the executables. A run
/// without them would speak each sound's indication instead, so scenarios
/// asserting sounds need them staged.
const SOUNDS: &str = "sounds";

/// Copies a directory tree into the stage, file by file, skipping files
/// that already match.
fn copy_dir_into_stage(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Err(io::Error::other(format!(
            "{} not found; building verbatim-synth-host builds eSpeak NG's data, and building verbatim-app copies the sounds",
            source.display()
        )));
    }
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_into_stage(&entry.path(), &target)?;
        } else if !files_match(&entry.path(), &target)? {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Whether `source` and `destination` already hold identical bytes.
/// `destination` not existing counts as "does not match" rather than an
/// error, since that is the ordinary first-run case. A plain full-content
/// comparison, not a cryptographic hash — both files are already local and
/// this only guards a same-machine copy against a `cargo build` no-op, not
/// against tampering, so simplicity wins over speed here.
fn files_match(source: &Path, destination: &Path) -> io::Result<bool> {
    if !destination.is_file() {
        return Ok(false);
    }
    Ok(fs::read(source)? == fs::read(destination)?)
}

/// The settings a run writes: [`Settings::for_e2e`] selecting eSpeak NG
/// ([`ESPEAK_ID`]), with the scenario's own `configure` applied.
fn run_settings(configure: Option<fn(&mut Settings)>) -> Settings {
    let mut settings = Settings::for_e2e(ESPEAK_ID);
    if let Some(configure) = configure {
        configure(&mut settings);
    }
    settings
}

/// Writes `settings` as the `settings.toml` of a remote run's Verbatim, in
/// `exe_dir` on the agent's machine: serialized here exactly as
/// [`write_settings`] writes it, into a directory of this process's own,
/// and sent through the agent.
fn write_remote_settings(
    agent: &mut AgentClient,
    exe_dir: &str,
    settings: Settings,
) -> io::Result<()> {
    let local = std::env::temp_dir().join(format!("verbatim-e2e-settings-{}", std::process::id()));
    fs::create_dir_all(&local)?;
    write_settings(&local, settings)?;
    let contents = fs::read(local.join(ConfigStore::SETTINGS_FILE))?;
    let _ = fs::remove_dir_all(&local);
    agent.write_file(
        &format!(r"{exe_dir}\{}", ConfigStore::SETTINGS_FILE),
        &contents,
    )
}

/// Writes `settings.toml` in `dir` as `settings`, normally
/// [`run_settings`]'s fixed shape. Never a load-modify-save
/// of whatever settings already sit in `dir`: builds the store from
/// [`Settings::for_e2e`] directly ([`ConfigStore::from_settings`]) and
/// writes it fresh, so a run's configuration can never accumulate state
/// left over from a previous run.
///
/// Every run selects eSpeak NG ([`ESPEAK_ID`]), the default synthesizer:
/// it is built with Verbatim, so it needs nothing installed on the machine
/// (`OneCore` voices are), and a silent run hears exactly what an audible
/// one does, through the silent real-time device.
///
/// Called only in runner-direct mode, on `dir` being [`stage_binaries`]'s
/// staging directory (this suite and the staged copy share a filesystem, so
/// writing directly there is correct); VM deploys handle this differently,
/// through `xtask vm deploy`'s own `write_synth_settings`, staged and
/// parameterized the same way but kept in lockstep independently — see
/// [`Settings::for_e2e`]'s doc comment.
fn write_settings(dir: &Path, settings: Settings) -> io::Result<()> {
    let store = ConfigStore::from_settings(dir, settings);
    store.save_settings().map_err(|error| config_error(&error))
}

fn config_error(error: &verbatim_config::ConfigError) -> io::Error {
    io::Error::other(error.to_string())
}

/// The synthesizer every run selects.
const ESPEAK_ID: &str = "espeak";

/// Waits until Verbatim reports itself ready
/// ([`GUI_READY_TIMEOUT`]).
fn wait_for_gui(control: &mut ControlClient) -> io::Result<()> {
    let deadline = Instant::now() + GUI_READY_TIMEOUT;
    loop {
        if let Frame::Reply {
            payload: ReplyPayload::Status(status),
            ..
        } = control.request(Request::Status)?
            && status.ready
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "Verbatim did not report itself ready within {GUI_READY_TIMEOUT:?}"
            )));
        }
        thread::sleep(GUI_READY_POLL);
    }
}

/// The image (executable file) name [`Scenario::launch_target`] records
/// for later cleanup: just the file name component of `command`, matching
/// what Windows itself reports as a process's image name (what
/// `verbatim_agent::protocol::Request::KillProcessesByName` compares
/// against) — never a full path. Falls back to `command` verbatim on the
/// rare path that has no file name component at all, so this never fails
/// outright over what is only ever used as a best-effort cleanup key.
fn image_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| command.to_owned(), str::to_owned)
}

/// Unit coverage for the runner-direct staging pieces that a live agent
/// connection is not needed to exercise: fixed-settings generation, the
/// hash-skip copy behavior, and [`VERBATIM_EXE_ENV`]'s override resolution.
/// [`Scenario::launch`] itself, and therefore the end-to-end staging flow
/// these pieces compose into, is only exercised live — by
/// `.github/workflows/ci.yml`'s `e2e` job (runner-direct) and
/// `cargo xtask vm test` (remote) — since it needs a running agent.
/// Whether `info` names a real foreground window to start a scenario from:
/// one exists and it is not cloaked.
fn usable_foreground(info: &ForegroundInfo) -> bool {
    info.foreground
        .as_ref()
        .is_some_and(|window| !window.cloaked)
}

/// `info` as one line: the foreground window, then the visible windows.
fn describe_foreground(info: &ForegroundInfo) -> String {
    let describe = |window: &WindowInfo| {
        format!(
            "{:?} ({}, class {}{})",
            window.title,
            window.image,
            window.class,
            if window.cloaked { ", cloaked" } else { "" }
        )
    };
    let foreground = info
        .foreground
        .as_ref()
        .map_or_else(|| "none".to_owned(), describe);
    let windows: Vec<String> = info.windows.iter().map(describe).collect();
    format!(
        "foreground window {foreground}; visible windows: {}",
        windows.join(", ")
    )
}

/// The text naming a harness document, folder, or window `name` in this
/// run: the shared [`DOCUMENT_MARKER`], `name`, and a token unique to this
/// run of the test binary. A document of a fixed name would let an
/// application restore state saved by an earlier run, as Windows 11 Notepad
/// restores a file's last selection, which a failed run can leave anywhere;
/// a new name has none. A window titled with it is closed by that title at
/// cleanup, and by the next launch's sweep if a run aborted before its
/// cleanup.
#[must_use]
pub fn harness_marker(name: &str) -> String {
    static TOKEN: OnceLock<String> = OnceLock::new();
    format!(
        "{DOCUMENT_MARKER}{name}-{}",
        TOKEN.get_or_init(document_token)
    )
}

/// A token unique to this run of the test binary: the milliseconds since the Unix epoch, in
/// base 36, so a harness document's name stays short.
fn document_token() -> String {
    let mut value = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let mut digits = Vec::new();
    loop {
        let digit = u8::try_from(value % 36).unwrap_or(0);
        digits.push(char::from_digit(u32::from(digit), 36).unwrap_or('0'));
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("verbatim-e2e-scenario-tests")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn resolve_verbatim_exe_path_honors_the_override() {
        assert_eq!(
            resolve_verbatim_exe_path(Some(r"C:\somewhere\verbatim.exe".to_owned())),
            PathBuf::from(r"C:\somewhere\verbatim.exe"),
            "the override names the source directory to stage from, not a launch path"
        );
    }

    #[test]
    fn resolve_verbatim_exe_path_defaults_under_the_workspace_root() {
        assert_eq!(
            resolve_verbatim_exe_path(None),
            workspace_root()
                .join("target")
                .join("debug")
                .join("verbatim.exe")
        );
    }

    #[test]
    fn only_a_folder_named_with_the_marker_is_a_harness_folder() {
        assert!(is_harness_name(r"C:\stage\verbatim-e2e-folder-abc"));
        assert!(!is_harness_name(r"C:\stage\logs"));
        assert!(!is_harness_name(r"C:\verbatim-e2e-folder-abc\logs"));
    }

    #[test]
    fn files_match_true_for_identical_content() {
        let dir = temp_dir("files-match-identical");
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        fs::write(&a, b"same bytes").expect("write a");
        fs::write(&b, b"same bytes").expect("write b");
        assert!(files_match(&a, &b).expect("compares"));
    }

    #[test]
    fn files_match_false_for_different_content() {
        let dir = temp_dir("files-match-different");
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        fs::write(&a, b"one").expect("write a");
        fs::write(&b, b"two").expect("write b");
        assert!(!files_match(&a, &b).expect("compares"));
    }

    #[test]
    fn files_match_false_when_destination_missing() {
        let dir = temp_dir("files-match-missing");
        let a = dir.join("a.bin");
        fs::write(&a, b"content").expect("write a");
        let b = dir.join("missing.bin");
        assert!(!files_match(&a, &b).expect("compares"));
    }

    #[test]
    fn copy_into_stage_copies_missing_and_skips_unchanged_and_recopies_changed() {
        let source_dir = temp_dir("stage-source");
        let stage_dir = temp_dir("stage-destination");
        fs::write(source_dir.join("verbatim.exe"), b"verbatim v1").expect("seed verbatim.exe");
        fs::write(source_dir.join("verbatim-outpost.exe"), b"outpost v1")
            .expect("seed verbatim-outpost.exe");
        fs::write(source_dir.join("verbatim-synth-host.exe"), b"host v1")
            .expect("seed verbatim-synth-host.exe");
        fs::write(source_dir.join("mockapp.exe"), b"mockapp v1").expect("seed mockapp.exe");
        fs::create_dir_all(source_dir.join(ESPEAK_DATA).join("voices"))
            .expect("seed the eSpeak NG data directory");
        fs::write(
            source_dir.join(ESPEAK_DATA).join("voices").join("en"),
            b"voice",
        )
        .expect("seed an eSpeak NG data file");
        fs::create_dir_all(source_dir.join(SOUNDS)).expect("seed the sounds folder");
        fs::write(source_dir.join(SOUNDS).join("exit.wav"), b"sound").expect("seed a sound");

        copy_into_stage(&source_dir, &stage_dir).expect("first copy");
        assert_eq!(
            fs::read(stage_dir.join(SOUNDS).join("exit.wav")).expect("read the staged sound"),
            b"sound"
        );
        assert_eq!(
            fs::read(stage_dir.join("verbatim.exe")).expect("read staged verbatim.exe"),
            b"verbatim v1"
        );
        assert_eq!(
            fs::read(stage_dir.join("verbatim-outpost.exe")).expect("read staged outpost"),
            b"outpost v1"
        );
        assert_eq!(
            fs::read(stage_dir.join(ESPEAK_DATA).join("voices").join("en"))
                .expect("read the staged eSpeak NG data"),
            b"voice"
        );

        // Simulate a stale staged copy left over from an earlier build, then
        // confirm a second call notices the mismatch and overwrites it
        // rather than leaving it (the hash-skip path only applies when the
        // bytes already match).
        fs::write(
            stage_dir.join("verbatim.exe"),
            b"stale, must be overwritten",
        )
        .expect("simulate an out-of-date staged copy");
        copy_into_stage(&source_dir, &stage_dir).expect("second copy overwrites the mismatch");
        assert_eq!(
            fs::read(stage_dir.join("verbatim.exe")).expect("read staged verbatim.exe"),
            b"verbatim v1",
            "a changed destination must be copied over again, not left stale"
        );
    }

    #[test]
    fn copy_into_stage_errors_when_the_outpost_binary_is_missing() {
        let source_dir = temp_dir("stage-missing-source");
        let stage_dir = temp_dir("stage-missing-destination");
        // Only verbatim.exe, not the outpost: the supervisor resolves the
        // outpost next to whatever verbatim.exe is running as, so both must
        // be staged together or the launched copy cannot find it.
        fs::write(source_dir.join("verbatim.exe"), b"verbatim").expect("seed verbatim.exe");

        let error = copy_into_stage(&source_dir, &stage_dir)
            .expect_err("a missing verbatim-outpost.exe must be an error, not a silent skip");
        assert!(error.to_string().contains("verbatim-outpost.exe"));
    }

    #[test]
    fn write_settings_writes_fixed_settings_never_a_merge_of_existing_state() {
        let dir = temp_dir("configure-synth");
        // Seed a pre-existing settings.toml carrying state a load-modify-save
        // would have carried forward (a different locale, a different
        // synthesizer) so this test would fail if write_settings ever
        // starts loading instead of building fresh.
        fs::write(
            dir.join(ConfigStore::SETTINGS_FILE),
            "locale = \"de\"\n[speech]\nsynthesizer = \"onecore\"\n",
        )
        .expect("seed a pre-existing settings.toml");

        write_settings(&dir, Settings::for_e2e("capture")).expect("writes fixed settings");

        let store = ConfigStore::load(&dir).expect("reloads what write_settings wrote");
        assert_eq!(store.settings(), &Settings::for_e2e("capture"));
        assert_eq!(
            store.settings().locale,
            None,
            "the pre-existing locale must not survive: write_settings never loads existing state"
        );
    }

    #[test]
    fn a_scenario_s_settings_change_only_what_it_configures() {
        assert_eq!(run_settings(None), Settings::for_e2e(ESPEAK_ID));
        let configured = run_settings(Some(|settings| {
            settings.reader.speak_terminal_passwords = true;
        }));
        let mut expected = Settings::for_e2e(ESPEAK_ID);
        expected.reader.speak_terminal_passwords = true;
        assert_eq!(configured, expected);
    }
}
