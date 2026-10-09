//! [`Scenario`]: the lifecycle owner for one live, agent-driven Verbatim run.
//!
//! A scenario launches Verbatim (and, on request, target applications)
//! through the M2 agent, drives it, and cleans up what it opened. Every
//! scenario starts from the same desktop: every window minimized, as the
//! taskbar's Show Desktop leaves it, whether or not the run is recorded.
//! Cleanup ([`Scenario::clean_up`]) closes each window the scenario opened
//! by its title (a Windows 11 Notepad document by closing its tab, never
//! its window, since Notepad keeps the tabs of a closed window for its next
//! session), checks that the process that owned it has exited when the
//! scenario started that program, ends anything else it launched by its
//! process id, and fails the scenario when anything will not close.
//! Nothing is ever ended by its image name.
//!
//! Configuration is always fixed and isolated, never the developer's own
//! live state. In runner-direct mode (the default; see [`REMOTE_ENV`]),
//! [`Scenario::launch`] copies Verbatim's executables into
//! `target/e2e-stage` under the workspace root and writes
//! [`verbatim_config::Settings::for_e2e`]'s fixed settings there, then
//! launches that staged copy. In remote mode, `cargo xtask vm deploy`
//! stages the guest side equivalently.
//!
//! The harness waits for evidence, never for time: Verbatim sets an event
//! the agent created when it is ready for input, windows are waited for on
//! window events, processes on their handles, files on folder changes, and
//! speech on the speech connection, each with a timeout that bounds only a
//! hang.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use verbatim_agent::protocol::{
    EventOutcome, ForegroundInfo, ProcessExit, ProcessInfo, ProcessState, WindowCondition,
    WindowInfo,
};
use verbatim_config::{ConfigStore, Settings};
use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, LatencyRecord, ReplyPayload, Request, StatusInfo};

use crate::agent_client::{AgentClient, Launched as AgentLaunch};
use crate::recording::Recording;
use crate::speech::SpeechCollector;
use crate::timeline::Timeline;
use crate::{ENDPOINT_ENV, endpoint};

/// File names the artifact collectors write under, inside the directory
/// [`crate::artifacts::scenario_dir`] names.
const TIMELINE_FILE_NAME: &str = "timeline.txt";
const STDERR_FILE_NAME: &str = "stderr.log";
const FLIGHT_RECORDER_FILE_NAME: &str = "flight-recorder.jsonl";
const FOCUS_FILE_NAME: &str = "focus.txt";
const LATENCY_FILE_NAME: &str = "latency.csv";
const AUDIO_FILE_NAME: &str = "verbatim-audio.wav";

/// Text in the title of every window the harness opens on purpose: the
/// document a [`Document`] names is named with it, so
/// its window can be told from the user's own windows of the same
/// application, found by title, and closed by title (a Notepad harness tab
/// as a tab), as NVDA's system tests name their Notepad documents.
pub const DOCUMENT_MARKER: &str = "verbatim-e2e-";

/// How long a window the harness waits for is given to appear and take the
/// foreground, or to go: a bound on a hang.
pub const WINDOW_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a window the harness asks to close, or the process that owned
/// it, is given to go.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long minimizing every window may take.
const MINIMIZE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long [`Scenario::launch`] waits for Verbatim to say it is ready.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long [`Scenario::quit_verbatim`] waits for the process to exit: a
/// bound on a hang, longer than the 21 seconds Verbatim gives an outpost to
/// shut down before it kills it, which it waits for before it exits.
const QUIT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long, once Verbatim has exited, every process of its job has to
/// have exited too: Verbatim waits for its outposts and the focus listener
/// before it exits, and its synthesizer host ends with its job at once.
const JOB_EMPTY_TIMEOUT: Duration = Duration::from_secs(15);

/// The executable of Verbatim's outposts and its focus listener.
const OUTPOST_IMAGE: &str = "verbatim-outpost.exe";

/// The folder Windows Error Reporting writes crash dumps of Verbatim's
/// processes into, once `vm\scripts\Enable-VerbatimCrashDumps.ps1` has
/// configured it (an elevated, one-time change). Without it there are no
/// dumps to collect, and a crash still fails the scenario.
pub const CRASH_DUMP_FOLDER: &str = r"C:\ProgramData\Verbatim\CrashDumps";

/// The images whose crash dumps the harness collects.
const VERBATIM_IMAGES: [&str; 3] = [
    "verbatim.exe",
    "verbatim-outpost.exe",
    "verbatim-synth-host.exe",
];

/// The extensions of the harness files the pre-launch sweep deletes: the
/// documents a [`Document`] names, and the fixtures
/// written at [`Scenario::harness_file`] paths.
const HARNESS_FILE_EXTENSIONS: [&str; 2] = ["txt", "json"];

/// Environment variable overriding the path to `verbatim.exe`. Defaults to
/// `target/debug/verbatim.exe` under the workspace root, which a
/// runner-direct run builds itself before staging it (see
/// [`build_default_source_binaries`]). Setting this skips that build: the
/// named binaries are staged as they are.
pub const VERBATIM_EXE_ENV: &str = "VERBATIM_E2E_VERBATIM_EXE";

/// Environment variable marking a *remote* run: the agent, Verbatim, and
/// its configuration live on another machine (the Hyper-V guest), so
/// [`VERBATIM_EXE_ENV`] names a path in that machine's filesystem, not this
/// one's. `cargo xtask vm test` sets it.
pub const REMOTE_ENV: &str = "VERBATIM_E2E_REMOTE";

/// Whether this is a remote (in-guest) run; see [`REMOTE_ENV`].
fn is_remote() -> bool {
    std::env::var(REMOTE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Environment variable requesting an *audible* run: [`Scenario::launch`]
/// does not set `VERBATIM_TEST_AUDIO=null`, so Verbatim speaks through the
/// real audio device instead of the silent real-time device a default run
/// uses. Both speak through eSpeak NG and take the same time, so every
/// assertion is the same either way.
pub const AUDIBLE_ENV: &str = "VERBATIM_E2E_AUDIBLE";

/// Whether this is an audible run; see [`AUDIBLE_ENV`].
#[must_use]
pub fn is_audible() -> bool {
    std::env::var(AUDIBLE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Environment variable naming a tracing filter for the Verbatim a
/// scenario launches, in `RUST_LOG`'s syntax: [`Scenario::launch`] passes
/// it to Verbatim as `RUST_LOG`, which its outposts inherit, so a run can
/// log what it does not by default (the terminal reads' timings, at debug
/// on `verbatim_outpost`). Unset, Verbatim logs with
/// [`DEFAULT_RUST_LOG`]; set but empty, as its own configuration says.
pub const RUST_LOG_ENV: &str = "VERBATIM_E2E_RUST_LOG";

/// The tracing filter every scenario's Verbatim logs with unless
/// [`RUST_LOG_ENV`] says otherwise: everything at info, and the outposts'
/// and the focus listener's debug lines, so a failed run's artifacts say
/// what each outpost read and when, what it released, and what the listener
/// reported, not only what the flight recorder kept.
pub const DEFAULT_RUST_LOG: &str = "info,verbatim_outpost=debug";

/// Enforces one live Verbatim instance at a time within this process; the
/// tests also run with `--test-threads=1`.
fn live_instance_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// One application a scenario launched, for cleanup.
struct Launched {
    pid: u32,
    /// The title of its window, which names it: it is closed by this title.
    /// `None` for a program with no window of the scenario's own, which is
    /// ended by its process id.
    title: Option<String>,
    /// The processes that owned its windows, recorded as each was waited
    /// for: they must exit once the windows close, when `owners_exit`.
    owners: Vec<u32>,
    /// Whether the programs owning its windows exit when they close: the
    /// scenario started the program, which had no window open before.
    /// Not for a window a shared process owns, such as File Explorer's.
    owners_exit: bool,
    /// The harness document it opened, deleted once its window has gone.
    document: Option<String>,
    /// Whether it is Windows 11 Notepad, whose harness tab is closed as a
    /// tab, so Notepad does not keep it for its next session.
    notepad: bool,
    /// Other processes that must exit once its windows close, such as the
    /// shell a terminal ran ([`Scenario::expect_exit_at_cleanup`]).
    also_exit: Vec<u32>,
}

/// Owns the lifecycle of one live Verbatim instance driven through the M2
/// agent.
pub struct Scenario {
    _lock: MutexGuard<'static, ()>,
    agent: AgentClient,
    verbatim_pid: u32,
    /// The processes Verbatim was told to ignore entirely: the owner's own
    /// Windows Terminal and its console hosts. No outpost may be started
    /// for one ([`Scenario::collect_run_artifacts`]).
    ignored: Vec<u32>,
    control: ControlClient,
    speech: SpeechCollector,
    timeline: Timeline,
    launched: Vec<Launched>,
    folders: Vec<String>,
    files: Vec<String>,
    stderr_log_path: String,
    run_dir: String,
    recording: Option<Recording>,
    /// The number of the last key or character the scenario injected.
    last_input: Option<u64>,
    /// Processes of Verbatim's own that the scenario expects to exit, such
    /// as a synthesizer host it ends on purpose.
    expected_exits: Vec<u32>,
    /// The crash dumps that were already there when the scenario started.
    dumps_before: Option<Vec<String>>,
    /// Whether [`Scenario::clean_up`] has run.
    cleaned_up: bool,
    /// Whether Verbatim has been asked to quit.
    quit: bool,
}

impl Scenario {
    /// Launches a fresh Verbatim through the agent named by
    /// [`crate::ENDPOINT_ENV`], with the fixed e2e settings.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch_with`].
    pub fn launch() -> io::Result<Self> {
        Self::launch_with(None, None)
    }

    /// Launches a fresh Verbatim with `configure` applied to the fixed
    /// settings, from the desktop every scenario starts from, with
    /// `document` open in Windows 11 Notepad when there is one.
    ///
    /// In order: closes the Notepad harness tabs an earlier run left, as
    /// tabs, and ends every process the agent launched for an earlier run
    /// that is still running, by its own handle; stages the binaries and
    /// writes the settings (runner-direct mode); closes any window an
    /// earlier run left open by its harness title (a Notepad harness tab as
    /// a tab) and deletes the harness files it left; opens `document`
    /// ([`Document`]), and closes it again if the launch fails after; minimizes every
    /// window, as Show Desktop does, and waits until they are and the
    /// desktop is in front; starts the recording, when recording; creates the
    /// event Verbatim sets when it is ready, launches it, and waits for the
    /// event; then opens the command connection and the speech connection.
    /// The first speech subscription receives the speech Verbatim queued
    /// before it, so its startup speech is asserted like any other.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint is unset, building or staging
    /// fails, the agent cannot be reached, the desktop cannot be brought to
    /// its starting state, an earlier run's leftovers cannot be cleaned up,
    /// the recording cannot start, or Verbatim does not become ready.
    #[expect(
        clippy::too_many_lines,
        reason = "the launch's steps in order, each one's failure cleaned up where it happens"
    )]
    pub fn launch_with(
        configure: Option<fn(&mut Settings)>,
        document: Option<Document>,
    ) -> io::Result<Self> {
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let lock = live_instance_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        // An earlier run that ended without its cleanup can have left its
        // Verbatim running from the stage, which staging would then fail to
        // overwrite: everything the agent launched is ended first. A Notepad
        // an earlier run left has its harness tabs closed as tabs before
        // that: Windows 11 Notepad ended by its handle keeps its tabs, and
        // opens them again next time, without their documents, which the
        // sweep deletes.
        let mut agent = AgentClient::connect(&agent_addr)?;
        close_notepad_tabs(&mut agent, DOCUMENT_MARKER)?;
        let ended = agent.end_launched()?;
        if ended > 0 {
            println!("ended {ended} process(es) an earlier run left running");
        }
        // Windows Terminal windows are watched from here on: one opened
        // before the run is not the run's (`Scenario::foreign_terminal_windows`).
        let _ = agent.take_foreign_terminal_windows()?;

        let verbatim_exe = verbatim_exe_path();
        let remote = is_remote();
        if !remote && std::env::var_os(VERBATIM_EXE_ENV).is_none() {
            build_default_source_binaries()?;
        }
        if !remote && !verbatim_exe.is_file() {
            return Err(io::Error::other(format!(
                "verbatim.exe not found at {} (set {VERBATIM_EXE_ENV} to override, or {REMOTE_ENV} if it lives in a guest)",
                verbatim_exe.display()
            )));
        }
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
            write_settings(&stage_dir, run_settings(configure))?;
            (stage_dir.join("verbatim.exe"), stage_dir)
        };
        let exe_str = launch_exe
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe path is not valid UTF-8"))?
            .to_owned();
        let run_dir = launch_dir
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe directory is not valid UTF-8"))?
            .to_owned();
        let stderr_path = verbatim_stderr_log_path(&launch_dir, remote)?;

        if remote {
            write_remote_settings(&mut agent, &run_dir, run_settings(configure))?;
        }
        // Leftovers are closed first, while a window of theirs is still where
        // it was, not minimized.
        sweep_leftovers(&mut agent, &run_dir)?;
        let mut opened = Vec::new();
        if let Some(document) = document {
            opened.push(open_document(&mut agent, &run_dir, &document)?);
        }
        let (minimized, desktop) = agent.minimize_all(MINIMIZE_TIMEOUT)?;
        if !minimized {
            close_documents(&mut agent, &opened)?;
            return Err(io::Error::other(format!(
                "not every window was minimized within {MINIMIZE_TIMEOUT:?}: {}",
                describe_foreground(&desktop)
            )));
        }
        let dumps_before = crash_dumps(&mut agent);

        let mut recording = if crate::recording::enabled() {
            match Recording::start(&mut agent, &run_dir) {
                Ok(recording) => Some(recording),
                Err(error) => {
                    close_documents(&mut agent, &opened)?;
                    return Err(io::Error::other(format!(
                        "the recording could not start: {error}"
                    )));
                }
            }
        } else {
            None
        };

        let ready_event = ready_event_name();
        agent.create_event(&ready_event)?;
        let mut env: Vec<(String, String)> = vec![
            (READY_EVENT_ENV.to_owned(), ready_event.clone()),
            crate::recording::audio_env(&run_dir),
        ];
        if !is_audible() {
            env.push(("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned()));
        }
        let filter = std::env::var(RUST_LOG_ENV).unwrap_or_else(|_| DEFAULT_RUST_LOG.to_owned());
        if !filter.is_empty() {
            env.push(("RUST_LOG".to_owned(), filter));
        }
        let launched = agent.launch_verbatim(&exe_str, &run_dir, &env, &stderr_path);
        let (verbatim_pid, ignored) = match launched {
            Ok((launched, ignored)) => (launched.pid, ignored),
            Err(error) => {
                if let Some(recording) = &mut recording {
                    recording.stop(&mut agent)?;
                }
                close_documents(&mut agent, &opened)?;
                return Err(error);
            }
        };
        let ready = agent.wait_for_event(&ready_event, verbatim_pid, READY_TIMEOUT);
        let connected = match ready {
            Ok(EventOutcome::Signalled) => connect(&agent_addr),
            Ok(EventOutcome::Exited { exit_code }) => Err(io::Error::other(format!(
                "Verbatim exited ({exit_code:?}) before it was ready; see {stderr_path}"
            ))),
            Ok(EventOutcome::TimedOut) => Err(io::Error::other(format!(
                "Verbatim did not say it was ready within {READY_TIMEOUT:?}"
            ))),
            Err(error) => Err(error),
        };
        let (mut control, speech, timeline) = match connected {
            Ok(connected) => connected,
            Err(error) => {
                agent.kill_process(verbatim_pid)?;
                if let Some(recording) = &mut recording {
                    recording.stop(&mut agent)?;
                }
                close_documents(&mut agent, &opened)?;
                return Err(error);
            }
        };
        let status = ok_or_error(control.request(Request::Status)?)?;
        if !matches!(
            status,
            Frame::Reply {
                payload: ReplyPayload::Status(ref info),
                ..
            } if info.ready
        ) {
            return Err(io::Error::other(format!(
                "Verbatim set its readiness event but its status is not ready: {status:?}"
            )));
        }

        Ok(Self {
            _lock: lock,
            agent,
            verbatim_pid,
            ignored,
            control,
            speech,
            timeline,
            launched: opened,
            folders: Vec::new(),
            files: Vec::new(),
            stderr_log_path: stderr_path,
            run_dir,
            recording,
            last_input: None,
            expected_exits: Vec::new(),
            dumps_before,
            cleaned_up: false,
            quit: false,
        })
    }

    /// Opens one more connection to Verbatim's control plane, subscribed to
    /// the normalized events Core receives and to the inputs it has
    /// handled: for a scenario that waits for an application event no
    /// speech shows. Events from before the call are not on it.
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

    /// The primary control-plane connection: status, gestures, latency,
    /// tree dumps, and quit.
    pub fn control(&mut self) -> &mut ControlClient {
        &mut self.control
    }

    /// Verbatim's status: its synthesizer, its outposts, and whether it is
    /// ready.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or is answered otherwise.
    pub fn status(&mut self) -> io::Result<StatusInfo> {
        match ok_or_error(self.control.request(Request::Status)?)? {
            Frame::Reply {
                payload: ReplyPayload::Status(status),
                ..
            } => Ok(status),
            other => Err(io::Error::other(format!(
                "unexpected reply to Status: {other:?}"
            ))),
        }
    }

    /// The dedicated speech-collector connection.
    pub fn speech(&mut self) -> &mut SpeechCollector {
        &mut self.speech
    }

    /// The scenario's timeline of injected input and speech.
    #[must_use]
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// Verbatim's process id.
    #[must_use]
    pub fn verbatim_pid(&self) -> u32 {
        self.verbatim_pid
    }

    /// Routes a gesture identifier through Verbatim's gesture router, as if
    /// the keys had been pressed (`Request::SendGesture`), once every
    /// utterance heard so far has been asserted.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn send_gesture(&mut self, identifier: &str) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before the gesture {identifier}"));
        self.timeline.push_gesture(identifier);
        ok_or_error(self.control.request(Request::SendGesture {
            identifier: identifier.to_owned(),
        })?)?;
        Ok(())
    }

    /// Injects real key strokes through the agent, which numbers each one,
    /// so Verbatim can say when it has handled it; each entry is a
    /// plus-joined combination such as `shift+tab`. Every utterance heard
    /// so far must have been asserted first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn send_keys(&mut self, keys: &[&str]) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before the keys {keys:?}"));
        self.timeline.push_keys(keys);
        let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        self.last_input = Some(self.agent.send_keys(&keys)?);
        Ok(())
    }

    /// Types `text` as real key presses through the agent's `TypeText`,
    /// each character mapped to its key and shift state in the foreground
    /// window's keyboard layout, and numbered like a key stroke. Every
    /// utterance heard so far must have been asserted first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when a character
    /// cannot be typed, in which case nothing was typed.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn type_text(&mut self, text: &str) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before typing {text:?}"));
        self.timeline.push_text(text);
        self.last_input = Some(self.agent.type_text(text)?);
        Ok(())
    }

    /// Asserts that nothing more was said: once Verbatim has handled the
    /// scenario's last injected input and is idle, every utterance it
    /// queued has been matched by an assertion
    /// ([`SpeechCollector::expect_nothing_more`]). Every scenario ends with
    /// this.
    ///
    /// # Panics
    ///
    /// Panics if anything unasserted was said, or Verbatim does not become
    /// idle.
    pub fn expect_nothing_more(&mut self) {
        self.speech.expect_nothing_more(self.last_input);
    }

    /// Every utterance not yet matched that was queued before Verbatim
    /// has handled the scenario's last input and is idle
    /// ([`SpeechCollector::take_until_idle`]), for the caller to assert on.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle.
    pub fn take_until_idle(&mut self) -> Vec<crate::speech::Heard> {
        self.speech.take_until_idle(self.last_input)
    }

    /// The directory, on the agent's machine, that this run's harness
    /// files go in: the one holding Verbatim's executable.
    #[must_use]
    pub fn run_directory(&self) -> &str {
        &self.run_dir
    }

    /// The path, on the agent's machine, of the harness folder `name` of
    /// this run, deleted with everything in it at cleanup, after the
    /// applications the scenario launched have ended.
    pub fn harness_folder(&mut self, name: &str) -> String {
        let folder = format!(r"{}\{}", self.run_dir, harness_marker(name));
        if !self.folders.contains(&folder) {
            self.folders.push(folder.clone());
        }
        folder
    }

    /// The path, on the agent's machine, of the harness file `name` of this
    /// run with `extension`, deleted at cleanup.
    pub fn harness_file(&mut self, name: &str, extension: &str) -> String {
        let file = format!(r"{}\{}.{extension}", self.run_dir, harness_marker(name));
        if !self.files.contains(&file) {
            self.files.push(file.clone());
        }
        file
    }

    /// Writes a file on the agent's machine, creating or replacing it and
    /// any missing parent directories.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn write_agent_file(&mut self, path: &str, contents: &[u8]) -> io::Result<()> {
        self.agent.write_file(path, contents)
    }

    /// The names of the folders directly inside `path` on the agent's
    /// machine.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn list_agent_folders(&mut self, path: &str) -> io::Result<Vec<String>> {
        self.agent.list_folders(path)
    }

    /// Deletes the folder `path` on the agent's machine, with everything in
    /// it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn delete_agent_folder(&mut self, path: &str) -> io::Result<()> {
        self.agent.delete_folder(path)
    }

    /// Waits up to `timeout`, on changes in its folder, for a file to exist
    /// on the agent's machine, and returns its contents: the evidence a
    /// script the scenario started has reached the point that writes it.
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not appear within `timeout` or
    /// cannot be read.
    pub fn wait_for_agent_file(&mut self, path: &str, timeout: Duration) -> io::Result<Vec<u8>> {
        if !self.agent.wait_for_file(path, timeout)? {
            return Err(io::Error::other(format!(
                "{path} was not written within {timeout:?}"
            )));
        }
        self.agent.read_file(path)
    }

    /// Launches `command`, which opens a window titled with `title`, a
    /// title of this run's own ([`harness_marker`]) or one no window had
    /// before ([`Scenario::require_absent`]), its first window minimized
    /// and inactive, then brings that window forward as clicking its
    /// taskbar button does, and waits for it to take the foreground.
    /// The window is closed by its title at cleanup, and when
    /// `owner_exits`, the process that owned it must exit then.
    ///
    /// A window opened in front is refused the foreground unless the agent
    /// may allow it, which it may only while it injected the last input
    /// (`docs/tooling.md`, "Windows' foreground lock keeps launched
    /// applications behind"); a minimized window restored and set as the
    /// foreground, as Notepad's documents are brought forward, takes it
    /// whatever input came last. A program that ignores the minimized show
    /// state, as msinfo32 does, is minimized by the agent before it is
    /// restored.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, no window
    /// titled with `title` opens, or it does not take the foreground.
    pub fn launch_titled(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        owner_exits: bool,
    ) -> io::Result<WindowInfo> {
        let launch = self.agent.launch_minimized(command, args)?;
        self.bring_forward(launch, title, owner_exits)
    }

    /// Records `launch` for cleanup, its window titled with `title` and its
    /// owner exiting at cleanup when `owner_exits`, waits for that window,
    /// opened minimized and inactive, to appear, restores it and sets it as
    /// the foreground, injecting nothing, and waits for it to take the
    /// foreground ([`Scenario::launch_titled`]).
    fn bring_forward(
        &mut self,
        launch: AgentLaunch,
        title: &str,
        owner_exits: bool,
    ) -> io::Result<WindowInfo> {
        self.launched.push(Launched {
            pid: launch.pid,
            title: Some(title.to_owned()),
            owners: Vec::new(),
            owners_exit: owner_exits,
            document: None,
            notepad: false,
            also_exit: Vec::new(),
        });
        let (present, desktop) = self.agent.wait_for_window(
            WindowCondition::Present {
                title_contains: title.to_owned(),
            },
            WINDOW_TIMEOUT,
        )?;
        // A cloaked window is not shown: the Settings app keeps its closed
        // window so, and shows it again when opened.
        let window = desktop
            .windows
            .iter()
            .find(|window| present && !window.cloaked && window.title.contains(title))
            .ok_or_else(|| {
                io::Error::other(format!(
                    "no window titled {title:?} opened within {WINDOW_TIMEOUT:?}: {}",
                    describe_foreground(&desktop)
                ))
            })?;
        if !self.agent.set_foreground(window.window)? {
            return Err(io::Error::other(format!(
                "the window titled {title:?} could not be brought to the foreground: {}",
                describe_foreground(&desktop)
            )));
        }
        self.require_in_front(title, launch)
    }

    /// Launches `command`, which opens a window titled with `title`, a
    /// title of this run's own ([`harness_marker`]), minimized and inactive,
    /// brings it forward as [`Scenario::launch_titled`] does, and fails
    /// unless the process the agent launched owns it: for a program that
    /// could otherwise hand its command line to an instance already
    /// running, such as Windows Terminal. The window is closed by its title
    /// at cleanup, and the process must exit then.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, the window
    /// does not take the foreground, or another process owns it.
    pub fn launch_owning_window(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
    ) -> io::Result<WindowInfo> {
        let launch = self.agent.launch_minimized(command, args)?;
        let launched = launch.pid;
        let window = self.bring_forward(launch, title, true)?;
        if window.pid != launched {
            return Err(io::Error::other(format!(
                "the window titled {title:?} belongs to {} (pid {}), not to the process the harness launched, pid {launched}: {}",
                window.image,
                window.pid,
                self.foreground_report()
            )));
        }
        Ok(window)
    }

    /// Every visible, titled, unowned top-level window, minimized and
    /// cloaked ones included.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn top_level_windows(&mut self) -> io::Result<Vec<WindowInfo>> {
        Ok(self.agent.foreground_info()?.windows)
    }

    /// Launches the console program `command` with `args`, its console
    /// window titled `title` from its first frame and opened minimized and
    /// inactive, and brings it forward as [`Scenario::launch_titled`]
    /// does.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, or the window
    /// does not take the foreground.
    pub fn launch_console(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
    ) -> io::Result<WindowInfo> {
        let launch = self.agent.launch_console(command, args, title)?;
        self.bring_forward(launch, title, true)
    }

    /// Brings the harness document `name`, which Notepad opened before
    /// Verbatim started ([`Document`]) and the starting state minimized, to
    /// the foreground, as clicking its taskbar button does, and waits on
    /// window events until it is in front.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails, no window is titled with the
    /// document, or it does not take the foreground in time.
    pub fn bring_document_forward(&mut self, name: &str) -> io::Result<()> {
        self.bring_window_forward(&harness_marker(name))
    }

    /// Brings the window titled with `marker`, one the scenario opened and
    /// another window has since taken the foreground from, to the
    /// foreground as clicking its taskbar button does, and waits on window
    /// events until it is in front: a user switching back to it.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails, no window is titled with
    /// `marker`, or it does not take the foreground in time.
    pub fn bring_window_forward(&mut self, marker: &str) -> io::Result<()> {
        let marker = marker.to_owned();
        let desktop = self.agent.foreground_info()?;
        let window = desktop
            .windows
            .iter()
            .find(|window| window.title.contains(&marker))
            .ok_or_else(|| {
                io::Error::other(format!(
                    "no window is titled {marker:?}: {}",
                    describe_foreground(&desktop)
                ))
            })?;
        if !self.agent.set_foreground(window.window)? {
            return Err(io::Error::other(format!(
                "the window {:?} could not be brought to the foreground: {}",
                window.title,
                describe_foreground(&desktop)
            )));
        }
        self.wait_for(
            WindowCondition::Foreground {
                title_contains: marker.clone(),
                unsaved: None,
            },
            WINDOW_TIMEOUT,
            &format!("{marker} to be in front"),
        )
    }

    /// Saves the harness document `name` with Control+S, as its window is
    /// in front with unsaved changes, and waits, on its title changing,
    /// until the title no longer marks them.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the title still marks unsaved
    /// changes after `timeout`.
    pub fn save_document(&mut self, name: &str, timeout: Duration) -> io::Result<()> {
        let marker = harness_marker(name);
        self.send_keys(&["control+s"])?;
        self.wait_for(
            WindowCondition::Foreground {
                title_contains: marker.clone(),
                unsaved: Some(false),
            },
            timeout,
            &format!("{marker} to be saved"),
        )
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
        self.wait_for(
            WindowCondition::Foreground {
                title_contains: marker.clone(),
                unsaved: Some(true),
            },
            timeout,
            &format!("{marker} to show unsaved changes"),
        )
    }

    /// Opens a harness folder in File Explorer: writes `files` (paths
    /// relative to the folder, empty contents) into a folder named
    /// [`DOCUMENT_MARKER`] plus `name` ([`Scenario::harness_folder`]),
    /// opens it, and waits for the window titled with that name to take
    /// the foreground. It is the one launch that still opens its window in
    /// front, needing the agent's right to let it take the foreground
    /// (`docs/tooling.md`): File Explorer raises its only foreground event
    /// as its window is created, before it is shown, so a window Windows
    /// refused then, or one opened minimized, is never heard when brought
    /// forward later (`phase6-design.md`, "Decisions to confirm with
    /// Dickson"). The window is closed by its title at cleanup;
    /// `explorer.exe` is the shell, so its process stays. Returns the
    /// folder's name, which is the window's title.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the window does not take the
    /// foreground.
    pub fn open_folder(&mut self, name: &str, files: &[&str]) -> io::Result<String> {
        let marker = harness_marker(name);
        let folder = self.harness_folder(name);
        for file in files {
            self.agent.write_file(&format!(r"{folder}\{file}"), b"")?;
        }
        let launch = self.agent.launch_process(
            "explorer.exe",
            std::slice::from_ref(&folder),
            None,
            &[],
            None,
        )?;
        self.launched.push(Launched {
            pid: launch.pid,
            title: Some(marker.clone()),
            owners: Vec::new(),
            owners_exit: false,
            document: None,
            notepad: false,
            also_exit: Vec::new(),
        });
        self.require_in_front(&marker, launch)?;
        Ok(marker)
    }

    /// Opens a page of the Settings app by its `ms-settings:` URI, minimized
    /// and inactive, and brings the Settings window forward as
    /// [`Scenario::launch_titled`] does, so it takes the foreground
    /// whatever input came last. No Settings window
    /// may be open before, so the window is the scenario's own; it is
    /// closed by its title at cleanup. The Settings app's process may stay,
    /// suspended, with its window cloaked, as it does after a user closes
    /// it.
    ///
    /// # Errors
    ///
    /// Returns an error if a Settings window is already open, a request
    /// fails, or the window does not take the foreground.
    pub fn open_settings_page(&mut self, uri: &str) -> io::Result<WindowInfo> {
        self.require_absent(SETTINGS_TITLE)?;
        self.launch_titled("explorer.exe", &[uri.to_owned()], SETTINGS_TITLE, false)
    }

    /// Has cleanup wait, once the window titled `title` that the scenario
    /// opened has closed, for process `pid` to exit too, failing the
    /// scenario if it does not: for a process the window ran, such as a
    /// terminal's shell, which holds the harness folder open until it exits.
    /// The window is the one whose title `title` contains, so a Windows
    /// Terminal tab titled with its window's first tab's title and more
    /// names that window.
    ///
    /// # Panics
    ///
    /// Panics if the scenario opened no window so titled.
    pub fn expect_exit_at_cleanup(&mut self, title: &str, pid: u32) {
        let launched = self
            .launched
            .iter_mut()
            .find(|launched| {
                launched
                    .title
                    .as_deref()
                    .is_some_and(|launched| title.contains(launched))
            })
            .unwrap_or_else(|| panic!("the scenario opened no window titled {title:?}"));
        launched.also_exit.push(pid);
    }

    /// Fails unless no visible window, cloaked ones aside, is titled with
    /// `title`: for a scenario that launches a program whose window has a
    /// title not of this run's own, so the window is the scenario's.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or such a window is open.
    pub fn require_absent(&mut self, title: &str) -> io::Result<()> {
        let info = self.agent.foreground_info()?;
        // A cloaked window is not shown: a suspended app keeps its window
        // so, as the Settings app does once its window is closed.
        if info
            .windows
            .iter()
            .any(|window| !window.cloaked && window.title.contains(title))
        {
            return Err(io::Error::other(format!(
                "a window titled {title:?} is already open, so the scenario's would not be its own: {}",
                describe_foreground(&info)
            )));
        }
        Ok(())
    }

    /// Waits for the window titled with `title`, which `launch` opened, to
    /// take the foreground, and records the process that owns it.
    fn require_in_front(&mut self, title: &str, launch: AgentLaunch) -> io::Result<WindowInfo> {
        let (met, desktop) = self.agent.wait_for_window(
            WindowCondition::Foreground {
                title_contains: title.to_owned(),
                unsaved: None,
            },
            WINDOW_TIMEOUT,
        )?;
        let window = desktop.foreground.clone().filter(|_| met).ok_or_else(|| {
            io::Error::other(format!(
                "the window titled {title:?} did not take the foreground within {WINDOW_TIMEOUT:?}{}: {}",
                if launch.foreground_allowed {
                    ""
                } else {
                    " (Windows did not let the agent allow its launch to take the foreground)"
                },
                describe_foreground(&desktop)
            ))
        })?;
        if let Some(launched) = self
            .launched
            .iter_mut()
            .find(|launched| launched.pid == launch.pid)
            && !launched.owners.contains(&window.pid)
        {
            launched.owners.push(window.pid);
        }
        Ok(window)
    }

    /// Waits up to `timeout`, on window events, for `condition`, failing
    /// with the desktop's state and `what` when it does not hold.
    fn wait_for(
        &mut self,
        condition: WindowCondition,
        timeout: Duration,
        what: &str,
    ) -> io::Result<()> {
        let (met, desktop) = self.agent.wait_for_window(condition, timeout)?;
        if met {
            Ok(())
        } else {
            Err(io::Error::other(format!(
                "waited {timeout:?} for {what}: {}",
                describe_foreground(&desktop)
            )))
        }
    }

    /// Waits, on window events, until no window titled with
    /// `title_contains` is in the foreground: evidence that a dialog
    /// closed, for a step that must not race the close.
    ///
    /// # Errors
    ///
    /// Returns an error, with the foreground report, if the window is still
    /// in front when `timeout` runs out.
    pub fn wait_for_window_to_close(
        &mut self,
        title_contains: &str,
        timeout: Duration,
    ) -> io::Result<()> {
        self.wait_for(
            WindowCondition::NotForeground {
                title_contains: title_contains.to_owned(),
            },
            timeout,
            &format!("the window titled {title_contains:?} to leave the foreground"),
        )
    }

    /// Runs `command` with `args`, a program that hands what it is asked to
    /// a running instance of itself and exits, such as Windows Terminal's
    /// executable opening a tab in a window of the harness's running
    /// Windows Terminal, and waits for it to exit.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start it or it does not exit
    /// within the window timeout.
    pub fn run_handing_off(&mut self, command: &str, args: &[String]) -> io::Result<()> {
        let launch = self.agent.launch_process(command, args, None, &[], None)?;
        match self.agent.wait_for_exit(launch.pid, WINDOW_TIMEOUT)? {
            ProcessState::Exited { .. } => Ok(()),
            ProcessState::Running => Err(io::Error::other(format!(
                "{command} (pid {}) did not exit within {WINDOW_TIMEOUT:?}",
                launch.pid
            ))),
        }
    }

    /// Waits, on window events, until a window titled with
    /// `title_contains` is in the foreground, and returns it.
    ///
    /// # Errors
    ///
    /// Returns an error, with the foreground report, if it is not in front
    /// within `timeout`.
    pub fn wait_for_window_in_front(
        &mut self,
        title_contains: &str,
        timeout: Duration,
    ) -> io::Result<WindowInfo> {
        let (met, desktop) = self.agent.wait_for_window(
            WindowCondition::Foreground {
                title_contains: title_contains.to_owned(),
                unsaved: None,
            },
            timeout,
        )?;
        desktop.foreground.clone().filter(|_| met).ok_or_else(|| {
            io::Error::other(format!(
                "waited {timeout:?} for the window titled {title_contains:?} to take the foreground: {}",
                describe_foreground(&desktop)
            ))
        })
    }

    /// One line describing the foreground window and the visible windows,
    /// for failure messages and the run's artifacts.
    pub fn foreground_report(&mut self) -> String {
        match self.agent.foreground_info() {
            Ok(info) => describe_foreground(&info),
            Err(error) => format!("the foreground could not be read: {error}"),
        }
    }

    /// What Verbatim says as the desktop takes the focus, the state every
    /// scenario starts from: the desktop's window, its list of icons, and
    /// the icon that has the focus, whose name, position, and selection the
    /// agent reads through UI Automation, independently of Verbatim, since
    /// they belong to the machine. The window must be the desktop's.
    ///
    /// # Errors
    ///
    /// Returns an error if the foreground is not the desktop, or the agent
    /// cannot read the focus.
    pub fn desktop_speech(&mut self) -> io::Result<Vec<String>> {
        let info = self.agent.foreground_info()?;
        if info
            .foreground
            .as_ref()
            .is_none_or(|window| window.title != DESKTOP_TITLE)
        {
            return Err(io::Error::other(format!(
                "the desktop is not in the foreground: {}",
                describe_foreground(&info)
            )));
        }
        let focus = self.agent.focused_element()?;
        let mut speech = vec![DESKTOP_TITLE.to_owned(), "Desktop list".to_owned()];
        if let Some((position, size)) = focus.position {
            let state = match focus.selected {
                Some(false) => " not selected",
                _ => "",
            };
            speech.push(format!("{}{state} {position} of {size}", focus.name));
        }
        Ok(speech)
    }

    /// Has the agent focus the foreground window's element whose UI
    /// Automation identifier is `automation_id`, injecting no input, once
    /// every utterance heard so far has been asserted; returns how many
    /// children it has, read through UI Automation, independently of
    /// Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn focus_by_automation_id(&mut self, automation_id: &str) -> io::Result<u32> {
        self.speech
            .require_all_asserted(&format!("before focusing {automation_id:?}"));
        self.agent.focus_by_automation_id(automation_id)
    }

    /// The words of the focused text that its application marks as
    /// misspelt, read by the agent independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn misspelt_words(&mut self) -> io::Result<Vec<String>> {
        self.agent.misspelt_words()
    }

    /// Whether the lock key `key` (such as `scrolllock`) is on, read by the
    /// agent independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn key_toggled(&mut self, key: &str) -> io::Result<bool> {
        self.agent.key_toggled(key)
    }

    /// The process id of the program the scenario launched last, and the
    /// processes it started that are still running.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario has launched nothing, or the agent
    /// cannot list them.
    pub fn launched_children(&mut self) -> io::Result<(u32, Vec<ProcessInfo>)> {
        let pid = self
            .launched
            .last()
            .map(|launched| launched.pid)
            .ok_or_else(|| io::Error::other("the scenario has launched no program"))?;
        Ok((pid, self.agent.child_processes(pid)?))
    }

    /// The processes Verbatim started, such as its synthesizer host.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn verbatim_children(&mut self) -> io::Result<Vec<ProcessInfo>> {
        self.agent.child_processes(self.verbatim_pid)
    }

    /// Ends one of Verbatim's own processes, which the scenario then
    /// expects to have exited, and waits for it to go.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be ended or does not exit
    /// within `timeout`.
    pub fn end_verbatim_process(&mut self, pid: u32, timeout: Duration) -> io::Result<()> {
        self.expected_exits.push(pid);
        self.agent.kill_process(pid)?;
        match self.agent.wait_for_exit(pid, timeout)? {
            ProcessState::Exited { .. } => Ok(()),
            ProcessState::Running => Err(io::Error::other(format!(
                "process {pid} did not exit within {timeout:?}"
            ))),
        }
    }

    /// The processes in Verbatim's job that exited and that the scenario
    /// did not end on purpose, other than those that exited cleanly with
    /// code 0: a crash, a kill by Verbatim's own supervisor, or any other
    /// unexpected end of one of Verbatim's processes.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn unexpected_exits(&mut self) -> io::Result<Vec<ProcessExit>> {
        let expected = self.expected_exits.clone();
        let verbatim = self.verbatim_pid;
        Ok(self
            .agent
            .job_exits(self.verbatim_pid)?
            .into_iter()
            .filter(|exit| {
                exit.pid != verbatim
                    && !expected.contains(&exit.pid)
                    && (exit.abnormal || exit.exit_code != Some(0))
            })
            .collect())
    }

    /// Asks Verbatim to exit cleanly (`Request::Quit`), and waits on its
    /// process handle for it to exit with code 0.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, Verbatim does not exit within
    /// the timeout, or it exits with any other code.
    pub fn quit_verbatim(&mut self) -> io::Result<()> {
        self.quit = true;
        // The reply races Verbatim's own teardown: the connection can close
        // before it is written. The exit code, read below, is the
        // authority on whether the quit worked.
        match self.control.request(Request::Quit) {
            Ok(frame) => {
                ok_or_error(frame)?;
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(error) => return Err(error),
        }
        match self.agent.wait_for_exit(self.verbatim_pid, QUIT_TIMEOUT)? {
            ProcessState::Exited { exit_code: Some(0) } => Ok(()),
            ProcessState::Exited { exit_code } => Err(io::Error::other(format!(
                "Verbatim (pid {}) exited with {exit_code:?} after Quit",
                self.verbatim_pid
            ))),
            ProcessState::Running => Err(io::Error::other(format!(
                "Verbatim (pid {}) is still running {QUIT_TIMEOUT:?} after Quit",
                self.verbatim_pid
            ))),
        }
    }

    /// Checks, once Verbatim has quit, that it left no outpost or focus
    /// listener behind and that every one shut down cleanly rather than
    /// being killed: each `verbatim-outpost.exe` process of its job, but
    /// one the scenario ended on purpose, exited with code 0, not
    /// abnormally, and before Verbatim itself did. A child Verbatim's
    /// supervisor had to kill exits with `verbatim_process::KILLED_EXIT_CODE`;
    /// one still running when Verbatim exited ends with its job afterwards.
    /// At least one must have exited, the focus listener. Returns every
    /// problem found.
    ///
    /// # Errors
    ///
    /// Returns an error if the job's processes cannot be read, or some are
    /// still running once [`JOB_EMPTY_TIMEOUT`] has passed.
    pub fn outposts_shut_down_cleanly(&mut self) -> io::Result<Vec<String>> {
        let exits = self
            .agent
            .wait_for_job_empty(self.verbatim_pid, JOB_EMPTY_TIMEOUT)?;
        let verbatim_exited_at = exits.iter().position(|exit| exit.pid == self.verbatim_pid);
        let mut problems = Vec::new();
        let mut outposts = 0;
        for (index, exit) in exits.iter().enumerate() {
            if !exit.image.eq_ignore_ascii_case(OUTPOST_IMAGE)
                || self.expected_exits.contains(&exit.pid)
            {
                continue;
            }
            outposts += 1;
            let killed = exit.exit_code
                == Some(i32::from_ne_bytes(
                    verbatim_process::KILLED_EXIT_CODE.to_ne_bytes(),
                ));
            if killed {
                problems.push(format!(
                    "outpost process {} was killed: it did not exit within its time limit after the shutdown message",
                    exit.pid
                ));
            } else if exit.exit_code != Some(0) || exit.abnormal {
                problems.push(format!(
                    "outpost process {} did not shut down cleanly: exit code {:?}{}",
                    exit.pid,
                    exit.exit_code,
                    if exit.abnormal {
                        ", abnormally (a crash)"
                    } else {
                        ""
                    }
                ));
            } else if verbatim_exited_at.is_none_or(|at| index > at) {
                problems.push(format!(
                    "outpost process {} was still running when Verbatim exited, and ended with its job",
                    exit.pid
                ));
            }
        }
        if outposts == 0 {
            problems.push(format!(
                "no outpost process of Verbatim's exited, not even the focus listener: {exits:?}"
            ));
        }
        Ok(problems)
    }

    /// The Windows Terminal windows shown during the run by a process the
    /// agent did not launch, which must be none: the terminal scenarios use
    /// the harness's portable copy, which the agent launches, and every
    /// other program the harness starts has no console window, so nothing
    /// the run does may reach the user's own Windows Terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot be asked.
    pub fn foreign_terminal_windows(&mut self) -> io::Result<Vec<WindowInfo>> {
        self.agent.take_foreign_terminal_windows()
    }

    /// Fetches the scenario's recent latency timelines.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn latency_snapshot(&mut self, last_n: u32) -> io::Result<Vec<LatencyRecord>> {
        crate::latency::fetch(&mut self.control, last_n)
    }

    /// Saves Core's focus, its ancestors, and the navigator object into
    /// `dir` as `focus.txt`, while Verbatim is up.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file cannot be written.
    pub fn collect_focus(&mut self, dir: &Path) -> io::Result<()> {
        let report = match ok_or_error(self.control.request(Request::DumpFocus)?)? {
            Frame::Reply {
                payload: ReplyPayload::Focus(report),
                ..
            } => report,
            other => {
                return Err(io::Error::other(format!(
                    "unexpected reply to DumpFocus: {other:?}"
                )));
            }
        };
        let mut text = String::new();
        let describe = |node: &verbatim_model::NodeSnapshot| format!("{node:?}");
        text.push_str("focus:\n");
        text.push_str(
            &report
                .focus
                .as_ref()
                .map_or_else(|| "(none)".to_owned(), describe),
        );
        text.push_str("\n\nancestors, outermost first:\n");
        for ancestor in &report.ancestors {
            text.push_str(&describe(ancestor));
            text.push('\n');
        }
        text.push_str("\nnavigator:\n");
        text.push_str(
            &report
                .navigator
                .as_ref()
                .map_or_else(|| "(none)".to_owned(), describe),
        );
        text.push('\n');
        fs::create_dir_all(dir)?;
        fs::write(dir.join(FOCUS_FILE_NAME), text)
    }

    /// Dumps the reducer flight recorder into `dir`, while Verbatim is up.
    ///
    /// # Errors
    ///
    /// Returns an error if the dump cannot be made, read, or written.
    pub fn collect_flight_recorder(&mut self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = match ok_or_error(self.control.request(Request::DumpRecorder)?)? {
            Frame::Reply {
                payload: ReplyPayload::DumpRecorder { path },
                ..
            } => path,
            other => {
                return Err(io::Error::other(format!(
                    "unexpected reply to DumpRecorder: {other:?}"
                )));
            }
        };
        let bytes = self.agent.read_file(&path)?;
        fs::write(dir.join(FLIGHT_RECORDER_FILE_NAME), bytes)
    }

    /// Collects the run's artifacts into `dir`, once Verbatim has exited:
    /// the timeline, the latency report, Verbatim's stderr log and every
    /// outpost and listener log of this launch, Verbatim's audio, and the
    /// crash dumps of Verbatim's processes written during the run. Returns
    /// every artifact that could not be collected, every crash dump, and
    /// every outpost started for a process Verbatim was told to ignore, its
    /// log named for that process, each a failure of the run.
    pub fn collect_run_artifacts(&mut self, dir: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        if let Err(error) = fs::create_dir_all(dir) {
            return vec![format!("could not create {}: {error}", dir.display())];
        }
        if let Err(error) = fs::write(dir.join(TIMELINE_FILE_NAME), self.timeline.render()) {
            problems.push(format!("could not write the timeline: {error}"));
        }
        if let Err(error) = fs::write(dir.join(LATENCY_FILE_NAME), self.latency_csv()) {
            problems.push(format!("could not write the latency report: {error}"));
        }
        match self.agent.read_file(&self.stderr_log_path) {
            Ok(bytes) => {
                if let Err(error) = fs::write(dir.join(STDERR_FILE_NAME), bytes) {
                    problems.push(format!("could not write Verbatim's stderr log: {error}"));
                }
            }
            Err(error) => problems.push(format!(
                "could not read Verbatim's stderr log at {}: {error}",
                self.stderr_log_path
            )),
        }
        let audio = crate::recording::audio_path(&self.run_dir);
        if let Err(error) = self.agent.copy_file(&audio, &dir.join(AUDIO_FILE_NAME)) {
            problems.push(format!("could not copy Verbatim's audio {audio}: {error}"));
        }
        problems.extend(self.collect_outpost_logs(dir));
        problems.extend(self.collect_crash_dumps(dir));
        problems
    }

    /// The latency report: one line per utterance that carries the time of
    /// the event behind it, with the step it answered.
    fn latency_csv(&self) -> String {
        let mut csv = String::from("step,utterance,event_to_queue_ms,event_to_audio_ms\n");
        for row in self.speech.latency_rows() {
            let quote = |text: &str| format!("\"{}\"", text.replace('"', "\"\""));
            let _ = writeln!(
                csv,
                "{},{},{},{}",
                quote(row.step.as_deref().unwrap_or("")),
                quote(&row.text),
                row.event_to_queue_ms,
                row.event_to_audio_ms
                    .map_or_else(String::new, |ms| ms.to_string())
            );
        }
        csv
    }

    /// Fetches every outpost and listener log this Verbatim launch wrote,
    /// from `logs\<Verbatim's pid>` next to its executable.
    fn collect_outpost_logs(&mut self, dir: &Path) -> Vec<String> {
        let logs_dir = format!(r"{}\logs\{}", self.run_dir, self.verbatim_pid);
        let names = match self.agent.list_files(&logs_dir) {
            Ok(names) => names,
            Err(error) => return vec![format!("could not list {logs_dir}: {error}")],
        };
        let mut problems = Vec::new();
        if !names.iter().any(|name| name == "listener.log") {
            problems.push(format!(
                "{logs_dir} holds no listener.log; the focus listener never started"
            ));
        }
        for pid in &self.ignored {
            if let Some(name) = names.iter().find(|name| outpost_log_names(name, *pid)) {
                problems.push(format!(
                    "an outpost was started for process {pid}, which Verbatim was told to ignore (the owner's Windows Terminal): {name}"
                ));
            }
        }
        for name in names {
            let remote_path = format!(r"{logs_dir}\{name}");
            // In chunks: an outpost's debug log over a long flood is larger
            // than one read may be.
            if let Err(error) = self.agent.copy_file(&remote_path, &dir.join(&name)) {
                problems.push(format!("could not collect {remote_path}: {error}"));
            }
        }
        // Deleted once collected, so a later launch that Windows gives the
        // same process id starts with a folder of its own.
        if let Err(error) = self.agent.delete_folder(&logs_dir) {
            problems.push(format!("could not delete {logs_dir}: {error}"));
        }
        problems
    }

    /// Copies the crash dumps of Verbatim's processes written during the
    /// run into `dir`, each reported as a failure; nothing when crash dumps
    /// are not configured on the agent's machine.
    fn collect_crash_dumps(&mut self, dir: &Path) -> Vec<String> {
        let Some(before) = self.dumps_before.clone() else {
            return Vec::new();
        };
        let Some(after) = crash_dumps(&mut self.agent) else {
            return vec![format!(
                "could not list {CRASH_DUMP_FOLDER} at the end of the run"
            )];
        };
        let mut problems = Vec::new();
        for name in after.into_iter().filter(|name| !before.contains(name)) {
            let remote = format!(r"{CRASH_DUMP_FOLDER}\{name}");
            match self.agent.copy_file(&remote, &dir.join(&name)) {
                Ok(()) => problems.push(format!("a Verbatim process crashed: its dump is {name}")),
                Err(error) => problems.push(format!(
                    "a Verbatim process crashed, and its dump {remote} could not be copied: {error}"
                )),
            }
        }
        problems
    }

    /// Finishes this run's video and saves it, with Verbatim's audio, to
    /// `to`. Does nothing when the run is not recording.
    ///
    /// # Errors
    ///
    /// Returns an error if the video cannot be finished or saved.
    pub fn finish_recording(&mut self, to: &Path) -> io::Result<()> {
        let Some(mut recording) = self.recording.take() else {
            return Ok(());
        };
        recording.finish(&mut self.agent, to)?;
        println!("recording saved to {}", to.display());
        Ok(())
    }

    /// Closes what the scenario opened, last opened first: each window by
    /// its title (a Notepad harness tab as a tab), waiting for it to go and,
    /// when the scenario started its program, for the process that owned
    /// it to exit; anything launched without a window of its own by its
    /// process id; then the harness documents, folders, and files. Returns
    /// everything that could not be closed or deleted, each a failure.
    pub fn clean_up(&mut self) -> Vec<String> {
        self.cleaned_up = true;
        let mut problems = Vec::new();
        for launched in std::mem::take(&mut self.launched).into_iter().rev() {
            problems.extend(self.close(&launched));
        }
        for folder in std::mem::take(&mut self.folders) {
            if let Err(error) = self.agent.delete_folder(&folder) {
                problems.push(format!(
                    "could not delete the harness folder {folder}: {error}"
                ));
            }
        }
        for file in std::mem::take(&mut self.files) {
            if let Err(error) = self.agent.delete_file(&file) {
                problems.push(format!("could not delete the harness file {file}: {error}"));
            }
        }
        problems
    }

    /// Closes one launched application, as [`Scenario::clean_up`] says.
    fn close(&mut self, launched: &Launched) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(title) = &launched.title {
            if launched.notepad {
                // Every harness tab, the scenario's and any Notepad restored
                // from an earlier run's session, so Notepad can exit. Never
                // its window: Notepad keeps the tabs of a window that closes
                // for its next session.
                if let Err(error) = close_notepad_tabs(&mut self.agent, DOCUMENT_MARKER) {
                    problems.push(error.to_string());
                }
            } else {
                match self.agent.close_windows(title, CLOSE_TIMEOUT) {
                    Ok(0) => {}
                    Ok(remaining) => problems.push(format!(
                        "{remaining} window(s) titled {title:?} did not close within {CLOSE_TIMEOUT:?}"
                    )),
                    Err(error) => problems.push(format!("could not close {title:?}: {error}")),
                }
            }
            let owners: &[u32] = if launched.owners_exit {
                &launched.owners
            } else {
                &[]
            };
            for &owner in owners.iter().chain(&launched.also_exit) {
                match self.agent.wait_for_exit(owner, CLOSE_TIMEOUT) {
                    Ok(ProcessState::Exited { .. }) => {}
                    Ok(ProcessState::Running) => problems.push(format!(
                        "process {owner}, which owned or ran in the window titled {title:?}, did not exit within {CLOSE_TIMEOUT:?} of the window closing{}",
                        if launched.notepad {
                            format!(
                                " (Windows 11 Notepad keeps running while it has tabs open, \
                                 such as tabs it restored from its previous session, which \
                                 must be closed before the Notepad scenarios run): {}",
                                self.foreground_report()
                            )
                        } else {
                            String::new()
                        }
                    )),
                    Err(error) => problems.push(format!(
                        "could not wait for process {owner} to exit: {error}"
                    )),
                }
            }
        } else {
            let ended = self
                .agent
                .kill_process(launched.pid)
                .and_then(|_| self.agent.wait_for_exit(launched.pid, CLOSE_TIMEOUT));
            match ended {
                Ok(ProcessState::Exited { .. }) => {}
                Ok(ProcessState::Running) => problems.push(format!(
                    "process {} did not exit within {CLOSE_TIMEOUT:?} of being ended",
                    launched.pid
                )),
                Err(error) => {
                    problems.push(format!("could not end process {}: {error}", launched.pid));
                }
            }
        }
        if let Some(document) = &launched.document
            && let Err(error) = self.agent.delete_file(document)
        {
            problems.push(format!(
                "could not delete the harness document {document}: {error}"
            ));
        }
        problems
    }
}

impl Drop for Scenario {
    /// A scenario dropped without its cleanup having run, which happens
    /// only when the run panicked before reaching it: Verbatim is ended by
    /// its process id and what the scenario opened is closed. Anything
    /// that will not close is printed; the run has already failed.
    fn drop(&mut self) {
        if !self.quit {
            let _ = self.control.request(Request::Quit);
            match self.agent.wait_for_exit(self.verbatim_pid, QUIT_TIMEOUT) {
                Ok(ProcessState::Exited { .. }) => {}
                _ => {
                    if let Err(error) = self.agent.kill_process(self.verbatim_pid) {
                        eprintln!(
                            "could not end Verbatim (pid {}): {error}",
                            self.verbatim_pid
                        );
                    }
                }
            }
        }
        if !self.cleaned_up {
            for problem in self.clean_up() {
                eprintln!("cleanup after a failed run: {problem}");
            }
        }
        if let Some(recording) = &mut self.recording
            && let Err(error) = recording.stop(&mut self.agent)
        {
            eprintln!("could not stop the recording: {error}");
        }
    }
}

/// The desktop window's title.
pub(crate) const DESKTOP_TITLE: &str = "Program Manager";

/// The title the Settings app's window has.
const SETTINGS_TITLE: &str = "Settings";

/// The variable naming the event Verbatim sets once it is ready for input;
/// read by `verbatim-app`.
const READY_EVENT_ENV: &str = "VERBATIM_READY_EVENT";

/// A name for this launch's readiness event, unique to this test process
/// and launch.
fn ready_event_name() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        r"Local\verbatim-e2e-ready-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The crash dumps in [`CRASH_DUMP_FOLDER`] of Verbatim's processes, or
/// `None` when the folder does not exist, which is when crash dumps are
/// not configured.
fn crash_dumps(agent: &mut AgentClient) -> Option<Vec<String>> {
    agent.list_files(CRASH_DUMP_FOLDER).ok().map(|names| {
        names
            .into_iter()
            .filter(|name| {
                VERBATIM_IMAGES
                    .iter()
                    .any(|image| name.to_ascii_lowercase().starts_with(image))
            })
            .collect()
    })
}

/// Cleans up what an earlier run that ended without its own cleanup left
/// behind: every process the agent launched that is still running is ended
/// by its own handle; every window titled with [`DOCUMENT_MARKER`] is
/// closed (a Notepad harness tab as a tab); and the harness documents,
/// files, and folders left in `directory` are deleted.
///
/// # Errors
///
/// Returns an error naming anything that could not be cleaned up.
fn sweep_leftovers(agent: &mut AgentClient, directory: &str) -> io::Result<()> {
    // Tabs first, as tabs: a Notepad ended by its handle keeps them for its
    // next session, without their documents, which are deleted below.
    close_notepad_tabs(agent, DOCUMENT_MARKER)?;
    let ended = agent.end_launched()?;
    if ended > 0 {
        println!("ended {ended} process(es) an earlier run left running");
    }
    let remaining = agent.close_windows(DOCUMENT_MARKER, CLOSE_TIMEOUT)?;
    if remaining > 0 {
        return Err(io::Error::other(format!(
            "{remaining} window(s) an earlier run left open, titled with {DOCUMENT_MARKER:?}, did not close"
        )));
    }
    for name in agent.list_files(directory)?.iter().filter(|name| {
        name.starts_with(DOCUMENT_MARKER)
            && Path::new(name).extension().is_some_and(|extension| {
                HARNESS_FILE_EXTENSIONS
                    .iter()
                    .any(|harness| extension.eq_ignore_ascii_case(harness))
            })
    }) {
        agent.delete_file(&format!(r"{directory}\{name}"))?;
    }
    for name in agent
        .list_folders(directory)?
        .iter()
        .filter(|name| name.starts_with(DOCUMENT_MARKER))
    {
        agent.delete_folder(&format!(r"{directory}\{name}"))?;
    }
    Ok(())
}

/// A harness document a scenario edits in Windows 11 Notepad, opened
/// before Verbatim starts ([`Scenario::launch_with`]): the starting state
/// minimizes it with every other window, and the scenario brings it
/// forward ([`Scenario::bring_document_forward`]), so what Verbatim says of
/// it is what it says of a window coming back to the foreground. A window
/// Windows 11 Notepad opens is titled "Notepad" alone for a moment as it
/// first takes the foreground, and then renamed with the document; opened
/// while Verbatim runs, whether Verbatim announces it before or after the
/// rename is a race.
#[derive(Clone, Debug)]
pub struct Document {
    /// The document's name, after [`DOCUMENT_MARKER`].
    pub name: &'static str,
    /// The document's text, its caret at the start.
    pub contents: String,
}

/// Opens `document` in Windows 11 Notepad and waits on window events until
/// it is in the foreground titled with the document. Notepad opens
/// minimized and inactive and is brought forward once its window is
/// titled with the document. No Notepad window may be open before, so the
/// window is the scenario's own and Notepad's process exits once it
/// closes; at cleanup the harness tab is closed as a tab, so Notepad does
/// not keep it for its next session, and the document is deleted.
fn open_document(
    agent: &mut AgentClient,
    run_dir: &str,
    document: &Document,
) -> io::Result<Launched> {
    let marker = harness_marker(document.name);
    let open: Vec<String> = agent
        .foreground_info()?
        .windows
        .into_iter()
        .filter(|window| is_notepad(&window.image))
        .map(|window| window.title)
        .collect();
    if !open.is_empty() {
        return Err(io::Error::other(format!(
            "Notepad must not be open when a scenario starts, so its window is the scenario's own; close these first: {open:?}"
        )));
    }
    let path = format!(r"{run_dir}\{marker}.txt");
    agent.write_file(&path, document.contents.as_bytes())?;
    let launch = agent.launch_minimized("notepad.exe", std::slice::from_ref(&path))?;
    let mut launched = Launched {
        pid: launch.pid,
        title: Some(marker.clone()),
        owners: Vec::new(),
        owners_exit: true,
        document: Some(path),
        notepad: true,
        also_exit: Vec::new(),
    };
    let (present, desktop) = agent.wait_for_window(
        WindowCondition::Present {
            title_contains: marker.clone(),
        },
        WINDOW_TIMEOUT,
    )?;
    let window = desktop
        .windows
        .iter()
        .find(|window| present && window.title.contains(&marker))
        .ok_or_else(|| {
            io::Error::other(format!(
                "no window titled {marker:?} opened within {WINDOW_TIMEOUT:?}: {}",
                describe_foreground(&desktop)
            ))
        })?;
    if !agent.set_foreground(window.window)? {
        return Err(io::Error::other(format!(
            "Notepad's window {:?} could not be brought to the foreground: {}",
            window.title,
            describe_foreground(&desktop)
        )));
    }
    let (met, desktop) = agent.wait_for_window(
        WindowCondition::Foreground {
            title_contains: marker.clone(),
            unsaved: None,
        },
        WINDOW_TIMEOUT,
    )?;
    let window = desktop.foreground.clone().filter(|_| met).ok_or_else(|| {
        io::Error::other(format!(
            "the window titled {marker:?} did not take the foreground within {WINDOW_TIMEOUT:?}: {}",
            describe_foreground(&desktop)
        ))
    })?;
    launched.owners.push(window.pid);
    Ok(launched)
}

/// Whether `image` is Windows 11 Notepad's.
fn is_notepad(image: &str) -> bool {
    image.eq_ignore_ascii_case("notepad.exe")
}

/// Closes every Notepad tab whose title holds `marker`, one at a time:
/// brings its window to the foreground, which the agent may do without
/// input as the program that injected the last input, and presses
/// Control+W, saving first when the title marks unsaved changes. Windows
/// 11 Notepad keeps every tab of a window that closes for its next
/// session, while a closed tab is forgotten; a window whose last tab
/// closes closes with it. Each step waits, on window events, for its
/// evidence: the title the window had going away.
fn close_notepad_tabs(agent: &mut AgentClient, marker: &str) -> io::Result<()> {
    loop {
        let Some(window) = agent
            .foreground_info()?
            .windows
            .into_iter()
            .find(|window| is_notepad(&window.image) && window.title.contains(marker))
        else {
            return Ok(());
        };
        if !agent.set_foreground(window.window)? {
            return Err(io::Error::other(format!(
                "Notepad's tab {:?} could not be brought to the foreground to close it",
                window.title
            )));
        }
        let key = if window.title.starts_with('*') {
            "control+s"
        } else {
            "control+w"
        };
        agent.send_keys(&[key.to_owned()])?;
        let (gone, desktop) = agent.wait_for_window(
            WindowCondition::Absent {
                title_contains: window.title.clone(),
            },
            CLOSE_TIMEOUT,
        )?;
        if !gone {
            return Err(io::Error::other(format!(
                "Notepad's tab {:?} did not close within {CLOSE_TIMEOUT:?} (a tab Notepad restored from an earlier session whose document is gone shows a \"Cannot find the file\" dialog, which blocks closing it; dismiss it and close the tab): {}",
                window.title,
                describe_foreground(&desktop)
            )));
        }
    }
}

/// Closes the harness documents a launch opened, as tabs, and deletes them,
/// for a launch that fails after opening them: left open, Notepad would be
/// ended by its handle at the next launch and keep them for its next
/// session. Fails when a tab does not close.
fn close_documents(agent: &mut AgentClient, opened: &[Launched]) -> io::Result<()> {
    if opened.iter().all(|launched| launched.document.is_none()) {
        return Ok(());
    }
    close_notepad_tabs(agent, DOCUMENT_MARKER)?;
    for document in opened
        .iter()
        .filter_map(|launched| launched.document.as_deref())
    {
        agent.delete_file(document)?;
    }
    Ok(())
}

/// Connects to a just-launched, ready Verbatim: the command connection,
/// then a second one subscribed to its speech.
fn connect(agent_addr: &str) -> io::Result<(ControlClient, SpeechCollector, Timeline)> {
    let control = AgentClient::connect(agent_addr)?.open_control_tunnel()?;
    let speech_tunnel = AgentClient::connect(agent_addr)?.open_control_tunnel()?;
    let timeline = Timeline::new();
    let speech = SpeechCollector::subscribe(speech_tunnel, timeline.clone())
        .map_err(|error| io::Error::other(format!("could not subscribe to speech: {error}")))?;
    Ok((control, speech, timeline))
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
            use std::os::windows::process::CommandExt as _;
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let status = std::process::Command::new(cargo)
                .creation_flags(crate::CREATE_NO_WINDOW)
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
/// The terminal scenarios' portable Windows Terminal is unpacked there too
/// ([`crate::windows_terminal::prepare`]), downloaded only when it is
/// missing.
///
/// # Errors
///
/// Returns an error if a required source binary is missing, a copy fails,
/// or Windows Terminal cannot be downloaded or unpacked.
fn stage_binaries(source_dir: &Path) -> io::Result<PathBuf> {
    let stage_dir = stage_directory();
    copy_into_stage(source_dir, &stage_dir)?;
    crate::windows_terminal::prepare(&stage_dir)?;
    Ok(stage_dir)
}

/// The stage of a runner-direct run, `target/e2e-stage` under the
/// workspace root, from which Verbatim and the terminal scenarios' Windows
/// Terminal run.
#[must_use]
pub fn stage_directory() -> PathBuf {
    workspace_root().join("target").join("e2e-stage")
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
    fs::remove_dir_all(&local)?;
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

/// `info` as one line: the foreground window, then the visible windows.
fn describe_foreground(info: &ForegroundInfo) -> String {
    let describe = |window: &WindowInfo| {
        format!(
            "{:?} ({} pid {}, class {}{}{})",
            window.title,
            window.image,
            window.pid,
            window.class,
            if window.cloaked { ", cloaked" } else { "" },
            if window.minimized { ", minimized" } else { "" }
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

/// Whether `name` is the log of an outpost watching process `pid`, as the
/// supervisor names it: `outpost-<image>-<pid>.log`, or `outpost-<pid>.log`
/// when the image could not be read.
fn outpost_log_names(name: &str, pid: u32) -> bool {
    let Some(stem) = name
        .strip_prefix("outpost-")
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    let pid_part = stem.rsplit('-').next().unwrap_or(stem);
    pid_part == pid.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_outpost_log_is_told_by_the_pid_it_ends_with() {
        assert!(outpost_log_names(
            "outpost-windowsterminal-29864.log",
            29864
        ));
        assert!(outpost_log_names("outpost-29864.log", 29864));
        assert!(!outpost_log_names(
            "outpost-windowsterminal-129864.log",
            29864
        ));
        assert!(!outpost_log_names("listener.log", 29864));
        assert!(!outpost_log_names("synth-espeak.log", 29864));
    }

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
