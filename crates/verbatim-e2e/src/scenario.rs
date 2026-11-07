//! [`Scenario`]: the lifecycle owner for one live, agent-driven Verbatim run.
//!
//! A scenario launches Verbatim (and, on request, target applications)
//! through the M2 agent, drives it, and cleans up what it opened. Every
//! scenario starts from the same desktop: every window minimized, as the
//! taskbar's Show Desktop leaves it, whether or not the run is recorded.
//! Cleanup ([`Scenario::clean_up`]) closes the windows the scenario opened
//! by the processes that own them when the scenario started that program,
//! and otherwise by the window's handle (a Windows 11 Notepad document by
//! closing its tab, never its window, since Notepad keeps the tabs of a
//! closed window for its next session; the Settings app's window by its
//! title, as [`Closing::ByTitle`] says why). It checks that the processes
//! that owned them have exited when the scenario started that program,
//! and that the process it launched has exited, ending it through the
//! agent's own handle when it has not, and fails the scenario when
//! anything will not close. It then proves nothing was left: every window
//! shown that was not open when the scenario started must have gone,
//! whatever its title has become. Nothing is ever ended by its image name
//! or its title.
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
//!
//! In compare mode ([`crate::compare`]) a scenario runs against NVDA
//! instead: no Verbatim is launched, its speech is NVDA's, read from the
//! transcript add-on ([`crate::nvda`]), and what needs Verbatim itself, such
//! as its status or its processes, is an error. A keyboard gesture is
//! pressed as keys, the Verbatim modifier as Insert, and whatever NVDA does
//! with them is what the step records.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use verbatim_agent::protocol::{
    EventOutcome, ForegroundInfo, ForegroundOutcome, LaunchEnd, ListPosition, MinimizeOutcome,
    ProcessDiagnostics, ProcessExit, ProcessInfo, ProcessState, SwitcherItem, TaskbarCommand,
    WaitChain, WaitNode, WaitNodeKind, WaitNodeStatus, WindowCondition, WindowInfo,
};
use verbatim_config::{ConfigStore, Settings};
use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, ReplyPayload, Request, StatusInfo};

use crate::agent_client::{AgentClient, Launched as AgentLaunch};
use crate::artifacts::OutpostCrash;
use crate::recording::Recording;
use crate::speech::{Expected, SpeechCollector};
use crate::timeline::Timeline;
use crate::{ENDPOINT_ENV, endpoint};

/// File names the artifact collectors write under, inside the directory
/// [`crate::artifacts::scenario_dir`] names.
const TIMELINE_FILE_NAME: &str = "timeline.txt";
const STDERR_FILE_NAME: &str = "stderr.log";
const FOCUS_FILE_NAME: &str = "focus.txt";
const LATENCY_FILE_NAME: &str = "latency.csv";
const AUDIO_FILE_NAME: &str = "verbatim-audio.wav";
/// The wait chains recorded when a window did not open or take the
/// foreground in time
/// ([`window_wait_chains`]), one chain a line.
const WAIT_CHAINS_FILE_NAME: &str = "wait-chains.txt";

/// The start of the names of the files recording the launched process
/// when its window did not open or take the foreground in time
/// ([`LaunchEvidence`]): `launched-process-<pid>.txt`, its processor time
/// and modules, and `launched-process-<pid>.dmp`, its minidump.
const LAUNCHED_PROCESS_FILE_STEM: &str = "launched-process";

/// Text in the title of every window the harness opens on purpose: the
/// document a [`Document`] names is named with it, so
/// its window can be told from the user's own windows of the same
/// application and found by title (a Notepad harness tab closed as a tab),
/// as NVDA's system tests name their Notepad documents.
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

/// The executables of Verbatim's children that the supervisor ends: its
/// outposts' and its focus listener's.
const CHILD_IMAGES: [&str; 2] = ["verbatim-outpost.exe", "verbatim-listener.exe"];

/// The executable of Verbatim's synthesizer host, which ends with
/// Verbatim's job rather than being shut down.
const SYNTH_HOST_IMAGE: &str = "verbatim-synth-host.exe";

/// Whether `image` is one of [`CHILD_IMAGES`].
fn is_child_image(image: &str) -> bool {
    CHILD_IMAGES
        .iter()
        .any(|child| image.eq_ignore_ascii_case(child))
}

/// The folder Windows Error Reporting writes crash dumps of Verbatim's
/// processes into, once `vm\scripts\Enable-VerbatimCrashDumps.ps1` has
/// configured it (an elevated, one-time change). Without it there are no
/// dumps to collect, and a crash still fails the scenario.
pub const CRASH_DUMP_FOLDER: &str = r"C:\ProgramData\Verbatim\CrashDumps";

/// The images whose crash dumps the harness collects.
const VERBATIM_IMAGES: [&str; 4] = [
    "verbatim.exe",
    "verbatim-outpost.exe",
    "verbatim-listener.exe",
    "verbatim-synth-host.exe",
];

/// The extensions of the harness files the pre-launch sweep deletes: the
/// documents a [`Document`] names, and the fixtures
/// written at [`Scenario::harness_file`] paths.
const HARNESS_FILE_EXTENSIONS: [&str; 3] = ["txt", "json", "dmp"];

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
/// and the focus listener's debug lines, their logic crates' among them,
/// since a directive matches its target as a prefix, and the supervisor's,
/// so a failed run's artifacts say
/// what each outpost read and when, what it released, and what the listener
/// reported, not only what the flight recorder kept; every move of a UIA
/// subscription, with each call it made and its result
/// (`books/dev/src/explanation/design/focus-pipeline.md`, section 17); and the arrival of every
/// UIA event and `WinEvent` in each process, a `WinEvent` with how long
/// ago it was raised, so a delay before an event reached Verbatim shows.
pub const DEFAULT_RUST_LOG: &str = "info,verbatim_outpost=debug,verbatim_listener=debug,verbatim_supervisor=debug,verbatim_uia::subscribe=debug,verbatim_uia::com=debug,verbatim_ia2::hook=debug";

/// Enforces one live Verbatim instance at a time within this process; the
/// tests also run with `--test-threads=1`.
fn live_instance_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// One application a scenario launched, for cleanup.
struct Launched {
    /// The process the agent launched, which must have exited once its
    /// windows have closed, and is ended through the agent's own handle
    /// when it has not ([`Scenario::end_launch`]).
    pid: u32,
    /// The title of its window, which names it in reports and for
    /// [`Scenario::expect_exit_at_cleanup`]. `None` for a program with no
    /// window of the scenario's own, which is ended by its process id.
    title: Option<String>,
    /// The window it opened, found by its title as it appeared.
    window: Option<WindowInfo>,
    /// The processes that owned its windows, recorded as each was waited
    /// for: they must exit once the windows close, when `closing` is
    /// [`Closing::ByOwner`] or [`Closing::NotepadTabs`].
    owners: Vec<u32>,
    /// How its windows are closed.
    closing: Closing,
    /// The harness document it opened, deleted once its window has gone.
    document: Option<String>,
    /// Other processes that must exit once its windows close, such as the
    /// shell a terminal ran ([`Scenario::expect_exit_at_cleanup`]).
    also_exit: Vec<u32>,
}

/// How cleanup closes the windows of one launch ([`Scenario::clean_up`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Closing {
    /// Every window of each process that owned one, by its process
    /// (`Request::CloseProcessWindows`), whatever their titles: the
    /// scenario started the program, which had no window open before, so
    /// every window of it is the scenario's, and its owners must exit.
    ByOwner,
    /// The window it opened, by its handle, whatever its title has become:
    /// a window a process shared with windows that are not the scenario's
    /// owns, such as a File Explorer window, which File Explorer retitles
    /// as it navigates or when its folder is deleted, or a second window of
    /// the harness's running Windows Terminal.
    ByHandle,
    /// The window titled with its title: the Settings app's window, the one
    /// window closed by title. Its process, `ApplicationFrameHost.exe`,
    /// frames every packaged app's windows, so its windows are not all the
    /// scenario's, and the window may be the Settings app's cloaked frame
    /// from before the scenario, which Windows shows again when Settings
    /// opens, so it is in the scenario's starting windows, which the agent
    /// never closes by handle.
    ByTitle,
    /// Windows 11 Notepad's harness tabs, closed as tabs, which only their
    /// titles tell apart, so Notepad does not keep them for its next
    /// session; its owners must exit ([`Scenario::end_notepad`]).
    NotepadTabs,
}

impl Closing {
    /// Whether the processes that owned the windows must exit once they
    /// close.
    fn owners_exit(self) -> bool {
        matches!(self, Self::ByOwner | Self::NotepadTabs)
    }

    /// How a launch whose owner exits when `owner_exits` closes: by its
    /// owners when the scenario started the program, by its window's
    /// handle when a shared process owns it.
    fn launched(owner_exits: bool) -> Self {
        if owner_exits {
            Self::ByOwner
        } else {
            Self::ByHandle
        }
    }
}

/// Owns the lifecycle of one live Verbatim instance driven through the M2
/// agent.
pub struct Scenario {
    _lock: MutexGuard<'static, ()>,
    agent: AgentClient,
    /// Verbatim's process id; `None` in compare mode, which launches no
    /// Verbatim.
    verbatim_pid: Option<u32>,
    /// The processes Verbatim was told to ignore entirely: the owner's own
    /// Windows Terminal and its console hosts. No outpost may be started
    /// for one ([`Scenario::collect_run_artifacts`]).
    ignored: Vec<u32>,
    /// The command connection to Verbatim; `None` in compare mode.
    control: Option<ControlClient>,
    speech: SpeechCollector,
    timeline: Timeline,
    launched: Vec<Launched>,
    /// The processes the scenario started or brought a window of forward,
    /// as its cleanup closed them: each launch and the owners of its
    /// windows ([`Scenario::cleanup_report`]).
    started: Vec<u32>,
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
    /// The crash dumps written during the run that were copied into its
    /// artifacts, by file name.
    dumps_collected: Vec<String>,
    /// Whether [`Scenario::clean_up`] has run.
    cleaned_up: bool,
    /// Whether Verbatim has been asked to quit.
    quit: bool,
    /// Whether Verbatim exited by itself at a step's keys
    /// ([`Scenario::send_keys_exiting`]): nothing is left to ask it.
    exited: bool,
    /// The keys the scenario holds down ([`Scenario::hold_key`]), in the
    /// order it pressed them, released at the latest by
    /// [`Scenario::release_held_keys`].
    held: Vec<String>,
    /// The NVDA the scenario runs against in compare mode; `None` when it
    /// runs against Verbatim.
    nvda: Option<NvdaRun>,
    /// What was recorded when a window the scenario launched did not open
    /// or take the foreground in time ([`LaunchEvidence`]), for the
    /// scenario's artifacts.
    launch_evidence: Vec<LaunchEvidence>,
    /// The executable Verbatim was launched from, for a second start
    /// ([`Scenario::replace_verbatim`]); empty in compare mode.
    launch_exe: String,
    /// The environment Verbatim was launched with, but for its readiness
    /// event, for a second start.
    launch_env: Vec<(String, String)>,
    /// Each Verbatim a second start replaced, by its process id, with the
    /// path of its stderr log, whose artifacts are collected with the
    /// run's.
    replaced: Vec<(u32, String)>,
}

/// The NVDA a compare-mode scenario runs against.
struct NvdaRun {
    /// Its version, as the transcript add-on's `Hello` gives it.
    version: String,
    /// The quiet period each step waits for.
    quiet: crate::compare::QuietPeriod,
}

impl Scenario {
    /// Launches a fresh Verbatim through the agent named by
    /// [`crate::ENDPOINT_ENV`], with the fixed e2e settings.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch_with`].
    pub fn launch() -> io::Result<Self> {
        Self::launch_with(None, None, false)
    }

    /// Launches a fresh Verbatim with `configure` applied to the fixed
    /// settings, from the desktop every scenario starts from, with
    /// `document` open in Windows 11 Notepad when there is one, and its
    /// idle retirement shortened when `quick_retirement` is set
    /// ([`QUICK_RETIREMENT`]).
    ///
    /// In order: closes the Notepad harness tabs an earlier run left, as
    /// tabs, and ends every process the agent launched for an earlier run
    /// that is still running, by its own handle; stages the binaries and
    /// writes the settings (runner-direct mode); closes any window an
    /// earlier run left open: those of a scenario abandoned before its
    /// cleanup by handle, the rest by their harness title (a Notepad harness
    /// tab as a tab), and deletes the harness files it left; records the
    /// windows the scenario starts with, for its cleanup; opens `document`
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
    pub fn launch_with(
        configure: Option<fn(&mut Settings)>,
        document: Option<Document>,
        quick_retirement: bool,
    ) -> io::Result<Self> {
        Self::start(configure, document, quick_retirement, false)
    }

    /// [`Scenario::launch_with`] in compare mode ([`crate::compare`]): the
    /// same up to opening `document`, building and staging included, since
    /// scenarios start `mockapp` and the portable Windows Terminal from the
    /// stage; it then connects to NVDA's transcript add-on in the agent's
    /// session, before minimizing, so NVDA's announcement of the desktop is
    /// heard, and minimizes as that does. It launches no Verbatim and
    /// records no video.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch_with`], before Verbatim's launch; and if the
    /// run is remote, or NVDA's transcript add-on cannot be reached in the
    /// agent's session.
    pub fn compare_with(
        configure: Option<fn(&mut Settings)>,
        document: Option<Document>,
    ) -> io::Result<Self> {
        Self::start(configure, document, false, true)
    }

    /// [`Scenario::launch_with`], or in compare mode, when `comparing`,
    /// [`Scenario::compare_with`].
    #[expect(
        clippy::too_many_lines,
        reason = "the launch's steps in order, each one's failure cleaned up where it happens"
    )]
    fn start(
        configure: Option<fn(&mut Settings)>,
        document: Option<Document>,
        quick_retirement: bool,
        comparing: bool,
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
        let ended = agent.end_launched(CLOSE_TIMEOUT)?;
        if ended > 0 {
            println!("ended {ended} process(es) an earlier run left running");
        }
        // Windows Terminal windows are watched from here on: one opened
        // before the run is not the run's (`Scenario::foreign_terminal_windows`).
        let _ = agent.take_foreign_terminal_windows()?;

        let verbatim_exe = verbatim_exe_path();
        let remote = is_remote();
        if comparing && remote {
            return Err(io::Error::other(format!(
                "compare mode runs on this machine, where NVDA's transcript add-on is read on 127.0.0.1; unset {REMOTE_ENV}"
            )));
        }
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
        // The windows the scenario starts with, recorded before it opens
        // anything, its document included, so cleanup can prove that every
        // window it opened has gone whatever the window's title has become.
        // Minimizing, which comes after, changes no window's identity.
        agent.record_windows()?;
        let mut opened = Vec::new();
        if let Some(document) = document {
            opened.push(open_document(&mut agent, &run_dir, &document)?);
        }
        let nvda = if comparing {
            match connect_nvda(&mut agent) {
                Ok(connected) => Some(connected),
                Err(error) => {
                    let closed = close_documents(&mut agent, &opened);
                    return Err(with_cleanup(error, "closing the harness documents", closed));
                }
            }
        } else {
            None
        };
        let (taskbar, outcome, desktop) = agent.minimize_all(MINIMIZE_TIMEOUT)?;
        if let Some(failure) = minimize_failure(&taskbar, outcome, &desktop) {
            let closed = close_documents(&mut agent, &opened);
            return Err(with_cleanup(
                io::Error::other(failure),
                "closing the harness documents",
                closed,
            ));
        }
        if let Some((speech, timeline, nvda)) = nvda {
            if crate::recording::enabled() {
                println!("compare mode records no video");
            }
            return Ok(Self {
                _lock: lock,
                agent,
                verbatim_pid: None,
                ignored: Vec::new(),
                control: None,
                speech,
                timeline,
                launched: opened,
                started: Vec::new(),
                folders: Vec::new(),
                files: Vec::new(),
                stderr_log_path: String::new(),
                run_dir,
                recording: None,
                last_input: None,
                expected_exits: Vec::new(),
                dumps_before: None,
                dumps_collected: Vec::new(),
                cleaned_up: false,
                quit: false,
                exited: false,
                held: Vec::new(),
                nvda: Some(nvda),
                launch_evidence: Vec::new(),
                launch_exe: String::new(),
                launch_env: Vec::new(),
                replaced: Vec::new(),
            });
        }
        let dumps_before = crash_dumps(&mut agent);

        let mut recording = if crate::recording::enabled() {
            match Recording::start(&mut agent, &run_dir) {
                Ok(recording) => Some(recording),
                Err(error) => {
                    let closed = close_documents(&mut agent, &opened);
                    return Err(with_cleanup(
                        io::Error::other(format!("the recording could not start: {error}")),
                        "closing the harness documents",
                        closed,
                    ));
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
        if quick_retirement {
            let (name, value) = QUICK_RETIREMENT;
            env.push((name.to_owned(), value.to_owned()));
        }
        let filter = std::env::var(RUST_LOG_ENV).unwrap_or_else(|_| DEFAULT_RUST_LOG.to_owned());
        if !filter.is_empty() {
            env.push(("RUST_LOG".to_owned(), filter));
        }
        let launch_env = env[1..].to_vec();
        let launched = agent.launch_verbatim(&exe_str, &run_dir, &env, &stderr_path);
        let (verbatim_pid, ignored) = match launched {
            Ok((launched, ignored)) => (launched.pid, ignored),
            Err(mut error) => {
                if let Some(recording) = &mut recording {
                    let stopped = recording.stop(&mut agent);
                    error = with_cleanup(error, "stopping the recording", stopped);
                }
                let closed = close_documents(&mut agent, &opened);
                return Err(with_cleanup(error, "closing the harness documents", closed));
            }
        };
        let ready = agent.wait_for_event(&ready_event, verbatim_pid, READY_TIMEOUT);
        let connected = match ready {
            // The status check is part of connecting, so that its failure
            // takes the cleanup below as every other start failure does.
            Ok(EventOutcome::Signalled) => connect(&agent_addr).and_then(|mut connected| {
                require_ready(&mut connected.0)?;
                Ok(connected)
            }),
            Ok(EventOutcome::Exited { exit_code }) => Err(io::Error::other(format!(
                "Verbatim exited ({exit_code:?}) before it was ready; see {stderr_path}, \
                 {STDERR_FILE_NAME} in the scenario's artifacts"
            ))),
            Ok(EventOutcome::TimedOut) => Err(io::Error::other(format!(
                "Verbatim did not say it was ready within {READY_TIMEOUT:?}, and was still \
                 running when the wait ended; see {stderr_path}, {STDERR_FILE_NAME} in the \
                 scenario's artifacts"
            ))),
            Err(error) => Err(error),
        };
        let (control, speech, timeline) = match connected {
            Ok(connected) => connected,
            Err(error) => {
                let killed = agent.kill_process(verbatim_pid).map(|_| ());
                let mut error = with_cleanup(error, "ending Verbatim", killed);
                // Read once Verbatim is ended, so it holds all it wrote; the
                // runner writes it to the scenario's artifacts
                // ([`write_start_failure`]), as no scenario is made to
                // collect it.
                let stderr_log = match agent.read_whole_file(&stderr_path) {
                    Ok(bytes) => Some(bytes),
                    Err(read) => {
                        error = with_cleanup(error, &format!("reading {stderr_path}"), Err(read));
                        None
                    }
                };
                if let Some(recording) = &mut recording {
                    let stopped = recording.stop(&mut agent);
                    error = with_cleanup(error, "stopping the recording", stopped);
                }
                let closed = close_documents(&mut agent, &opened);
                let error = with_cleanup(error, "closing the harness documents", closed);
                return Err(StartFailure::error(&error, stderr_log));
            }
        };

        Ok(Self {
            _lock: lock,
            agent,
            verbatim_pid: Some(verbatim_pid),
            ignored,
            control: Some(control),
            speech,
            timeline,
            launched: opened,
            started: Vec::new(),
            folders: Vec::new(),
            files: Vec::new(),
            stderr_log_path: stderr_path,
            run_dir,
            recording,
            last_input: None,
            expected_exits: Vec::new(),
            dumps_before,
            dumps_collected: Vec::new(),
            cleaned_up: false,
            quit: false,
            exited: false,
            held: Vec::new(),
            nvda: None,
            launch_evidence: Vec::new(),
            launch_exe: exe_str,
            launch_env,
            replaced: Vec::new(),
        })
    }

    /// Whether the scenario runs against NVDA, in compare mode
    /// ([`crate::compare`]).
    #[must_use]
    pub fn is_comparing(&self) -> bool {
        self.nvda.is_some()
    }

    /// In compare mode, NVDA's version, as the transcript add-on's `Hello`
    /// gives it, and the quiet period each step waits for.
    #[must_use]
    pub fn nvda_run(&self) -> Option<(&str, crate::compare::QuietPeriod)> {
        self.nvda
            .as_ref()
            .map(|run| (run.version.as_str(), run.quiet))
    }

    /// Declares that the step under way, begun by the last input,
    /// deliberately differs from NVDA, as the parity ledger's entry headed
    /// `entry` in `file` (such as `docs/parity/review.md`) records: a note
    /// on the timeline, which compare mode judges the step against
    /// ([`crate::compare::Verdict`]). A unit test checks that every entry
    /// a scenario names is a heading in its file.
    pub fn deviates_from_nvda(&self, file: &str, entry: &str) {
        self.timeline.push_deviation(file, entry);
    }

    /// In compare mode, closes the step under way, as an input does, and
    /// starts one whose input is `action`, an action of the harness's that
    /// changes what is in front, such as launching a window or bringing
    /// one forward, recorded on the timeline: so what NVDA says about the
    /// window is not counted with the step before. A normal run is
    /// unchanged by it.
    ///
    /// # Panics
    ///
    /// Panics, in compare mode, as
    /// [`SpeechCollector::require_all_asserted`] does.
    pub(crate) fn begin_action(&mut self, action: &str) {
        if self.is_comparing() {
            self.speech
                .require_all_asserted(&format!("before {action}"));
            self.timeline.push_action(action);
        }
    }

    /// Verbatim's process id, or, in compare mode, an error saying there is
    /// no Verbatim to `what`.
    fn pid(&self, what: &str) -> io::Result<u32> {
        self.verbatim_pid.ok_or_else(|| no_verbatim(what))
    }

    /// The command connection to Verbatim, or, in compare mode, an error
    /// saying there is no Verbatim to `what`.
    fn control_client(&mut self, what: &str) -> io::Result<&mut ControlClient> {
        self.control.as_mut().ok_or_else(|| no_verbatim(what))
    }

    /// Opens one more connection to Verbatim's control plane, subscribed to
    /// the normalized events Core receives and to the inputs it has
    /// handled: for a scenario that waits for an application event no
    /// speech shows. Events from before the call are not on it. It reads
    /// with the poll interval
    /// ([`crate::agent_client::CONTROL_POLL_INTERVAL`]), so its reader can
    /// check its own deadline.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot be reached, the tunnel cannot
    /// be opened, the subscription is refused, or the read timeout cannot
    /// be set.
    pub fn subscribe_events(&mut self) -> io::Result<ControlClient> {
        if self.is_comparing() {
            return Err(no_verbatim("subscribe to the events of (subscribe_events)"));
        }
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let mut events =
            AgentClient::connect(&agent_addr).and_then(AgentClient::open_control_tunnel)?;
        ok_or_error(events.request(Request::SubscribeEvents)?)?;
        events.set_read_timeout(Some(crate::agent_client::CONTROL_POLL_INTERVAL))?;
        Ok(events)
    }

    /// Verbatim's status: its synthesizer, its outposts, and whether it is
    /// ready.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or is answered otherwise.
    pub fn status(&mut self) -> io::Result<StatusInfo> {
        let control = self.control_client("ask for its status (status)")?;
        match ok_or_error(control.request(Request::Status)?)? {
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

    /// Presses a keyboard gesture's keys through the agent, as
    /// [`Scenario::send_keys`] presses keys, once every utterance heard so
    /// far has been asserted: the Verbatim modifier as Insert, a Verbatim
    /// modifier by default and NVDA's, so `kb:verbatim+v` as `insert+v`,
    /// and `kb:numpad8` as `numpad8`
    /// ([`verbatim_control::send_keys::gesture_keys`]). The keyboard hook
    /// decides and numbers them as a person's keys, so the same gesture
    /// pressed twice within the multi-press timeout (500 ms) is a double
    /// press; in compare mode, whatever NVDA does with the keys is what
    /// the step records.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, or if the gesture is not a
    /// keyboard gesture.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn send_gesture(&mut self, identifier: &str) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before the gesture {identifier}"));
        self.timeline.push_gesture(identifier);
        let keys =
            verbatim_control::send_keys::gesture_keys(identifier).map_err(io::Error::other)?;
        self.last_input = Some(self.agent.send_keys(&[keys])?);
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

    /// [`Scenario::send_keys`] for a step whose speech is `expected`,
    /// which it then asserts ([`SpeechCollector::expect_sequence`]): the
    /// step's leading optional utterances ([`crate::speech::optional`]) may
    /// match speech that arrived before the keys were sent, which the check
    /// before them leaves for the step
    /// ([`SpeechCollector::require_asserted_before`]), and match in
    /// sequence when it arrives after them.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion or leading optional utterance
    /// matched is waiting, or as [`SpeechCollector::expect_sequence`] does.
    pub fn send_keys_expecting(&mut self, keys: &[&str], expected: &[Expected]) -> io::Result<()> {
        self.speech
            .require_asserted_before(&format!("before the keys {keys:?}"), expected);
        self.timeline.push_keys(keys);
        let owned: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        self.last_input = Some(self.agent.send_keys(&owned)?);
        self.speech.expect_sequence(expected);
        Ok(())
    }

    /// Presses `key`, one key such as `control`, down through the agent's
    /// `PressKey` and holds it, numbered like a key stroke, so the keys sent
    /// next are pressed with it, as a person holds Control through a tab
    /// switcher; [`Scenario::release_key`] releases it. Every utterance
    /// heard so far must have been asserted first. A key still held when
    /// the body ends, passing or failing, is released then and fails the
    /// run ([`Scenario::release_held_keys`]); the agent also releases it if
    /// the connection ends first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when the key name
    /// is unknown or the key is already held.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn hold_key(&mut self, key: &str) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before holding {key:?}"));
        self.timeline.push_keys(&[&format!("hold {key}")]);
        self.last_input = Some(self.agent.press_key(key)?);
        self.held.push(key.to_owned());
        Ok(())
    }

    /// Releases `key`, which [`Scenario::hold_key`] holds, numbered like a
    /// key stroke. Every utterance heard so far must have been asserted
    /// first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when the key is not
    /// held.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn release_key(&mut self, key: &str) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before releasing {key:?}"));
        self.timeline.push_keys(&[&format!("release {key}")]);
        self.held.retain(|held| held != key);
        self.last_input = Some(self.agent.release_key(key)?);
        Ok(())
    }

    /// Releases every key the scenario still holds, the last pressed first,
    /// with no assertion: called as the body ends, passed or failed, and
    /// again by [`Scenario::clean_up`], so no modifier stays down. A key
    /// held at the body's end is a failure of the run, and each is
    /// returned as a problem, as is a release that fails.
    pub fn release_held_keys(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        for key in std::mem::take(&mut self.held).into_iter().rev() {
            match self.agent.release_key(&key) {
                Ok(_) => problems.push(format!(
                    "the scenario ended holding {key:?}, which was released"
                )),
                Err(error) => problems.push(format!(
                    "the scenario ended holding {key:?}, which could not be released: {error}"
                )),
            }
        }
        problems
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
        // Once Verbatim has exited by itself, its speech was taken to the
        // connection's end and asserted then.
        if !self.exited {
            self.speech.expect_nothing_more(self.last_input);
        }
    }

    /// Asserts that Core has asked for exactly `expected` live re-reads
    /// since Verbatim started, once nothing more is to be said
    /// ([`expect_nothing_more`](Self::expect_nothing_more)), read from the
    /// control protocol's status reply (`StatusInfo::live_re_reads`).
    ///
    /// # Panics
    ///
    /// When the status cannot be read, or the count differs.
    pub fn expect_live_re_reads(&mut self, expected: u64) {
        self.expect_nothing_more();
        let status = self.status().expect("reads Verbatim's status");
        assert_eq!(
            status.live_re_reads, expected,
            "Core's live re-reads since Verbatim started"
        );
    }

    /// Core's live re-reads since Verbatim started, read from its status once
    /// nothing more is said, for a measure scenario whose count depends on
    /// how many focus events an application raises for one element.
    ///
    /// # Panics
    ///
    /// Panics if more is said, or Verbatim does not answer its status.
    pub fn live_re_reads(&mut self) -> u64 {
        self.expect_nothing_more();
        self.status()
            .expect("reads Verbatim's status")
            .live_re_reads
    }

    /// Whether Verbatim exited by itself at a step's keys
    /// ([`Scenario::send_keys_exiting`]), so that neither its focus nor
    /// its flight recorder can be dumped and it needs no quit.
    #[must_use]
    pub fn exited(&self) -> bool {
        self.exited
    }

    /// [`Scenario::send_keys`] for a step that makes Verbatim exit, such as
    /// the Exit item of its menu: waits on its process handle for it to
    /// exit with code 0, then takes its speech to the connection's end,
    /// whose last utterance must be exactly `last`; what was said between
    /// the keys and it is not asserted (`books/dev/src/reference/testing.md`, "Exact
    /// assertions"). The run then asks Verbatim for nothing more and does
    /// not quit it ([`Scenario::quit_verbatim`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the keys cannot be sent, or Verbatim does not
    /// exit within the quit timeout or exits with another code.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting before the
    /// keys, the speech connection does not end, or `last` is not the last
    /// utterance.
    pub fn send_keys_exiting(&mut self, keys: &[&str], last: &str) -> io::Result<()> {
        let pid = self.pid("exit (send_keys_exiting)")?;
        self.send_keys(keys)?;
        let state = self.agent.wait_for_exit(pid, QUIT_TIMEOUT)?;
        // Gone, so nothing is left to quit; one still running is quit by
        // the run as usual.
        if matches!(state, ProcessState::Exited { .. }) {
            self.quit = true;
            self.exited = true;
        }
        match state {
            ProcessState::Exited { exit_code: Some(0) } => {}
            ProcessState::Exited { exit_code } => {
                return Err(io::Error::other(format!(
                    "Verbatim (pid {pid}) exited with {exit_code:?} after the keys {keys:?}"
                )));
            }
            ProcessState::Running => {
                return Err(io::Error::other(format!(
                    "Verbatim (pid {pid}) is still running {QUIT_TIMEOUT:?} after the keys {keys:?}"
                )));
            }
        }
        self.speech.expect_last_at_the_end(last);
        Ok(())
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

    /// For the measurement workloads, which assert no speech: everything
    /// Verbatim queued that no assertion matched, once it is idle after the
    /// scenario's last input, and every frame that has reached the speech
    /// connection since ([`SpeechCollector::take_all_queued`]).
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle.
    pub fn take_all_queued(&mut self) -> Vec<crate::speech::Heard> {
        self.speech.take_all_queued(self.last_input)
    }

    /// [`Scenario::take_all_queued`], letting Verbatim take up to `bound`
    /// to become idle.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle within `bound`.
    pub fn take_all_queued_within(&mut self, bound: Duration) -> Vec<crate::speech::Heard> {
        self.speech.take_all_queued_within(self.last_input, bound)
    }

    /// [`Scenario::send_keys`], one stroke every `interval_ms`, for a
    /// measurement workload's rate of key presses, such as auto-repeat's.
    /// Returns the first and last strokes' numbers.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn send_keys_paced(&mut self, keys: &[&str], interval_ms: u32) -> io::Result<(u64, u64)> {
        self.speech
            .require_all_asserted(&format!("before the keys {keys:?}"));
        self.timeline.push_keys(keys);
        let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
        let last = self.agent.send_keys_paced(&keys, interval_ms)?;
        self.last_input = Some(last);
        Ok((last + 1 - keys.len() as u64, last))
    }

    /// Holds `key` down with auto-repeat for a measurement workload
    /// ([`AgentClient::repeat_key`](crate::agent_client::AgentClient::repeat_key)):
    /// its press, `repeats` more presses, the first `delay_ms` after it and
    /// then `interval_ms` apart, and its release `interval_ms` after the
    /// last, noted on the timeline once sent. Returns the first press's and
    /// the release's numbers.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn repeat_key(
        &mut self,
        key: &str,
        delay_ms: u32,
        interval_ms: u32,
        repeats: u32,
    ) -> io::Result<(u64, u64)> {
        self.speech
            .require_all_asserted(&format!("before holding {key:?} with auto-repeat"));
        self.timeline
            .push_keys(&[&format!("hold {key} with {repeats} repeats")]);
        let last = self.agent.repeat_key(key, delay_ms, interval_ms, repeats)?;
        self.timeline.push_keys(&[&format!("released {key}")]);
        self.last_input = Some(last);
        Ok((last - u64::from(repeats) - 1, last))
    }

    /// [`Scenario::type_text`], one character every `interval_ms`, for a
    /// measurement workload's typing rate. Returns the first and last
    /// characters' numbers.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn type_text_paced(&mut self, text: &str, interval_ms: u32) -> io::Result<(u64, u64)> {
        self.speech
            .require_all_asserted(&format!("before typing {text:?}"));
        self.timeline.push_text(text);
        let last = self.agent.type_text_paced(text, interval_ms)?;
        self.last_input = Some(last);
        Ok((last + 1 - text.chars().count() as u64, last))
    }

    /// Injects `keys` through Verbatim's own control plane
    /// (`Request::SendKeys`), from inside `verbatim.exe`, so the time from
    /// its `SendInput` to its own hook seeing the key is what re-injecting
    /// a held key would cost. The keys carry no harness number, so the
    /// scenario's last input stays what it was.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    ///
    /// # Panics
    ///
    /// Panics if an utterance no assertion matched is waiting.
    pub fn send_keys_from_verbatim(&mut self, keys: &[&str]) -> io::Result<()> {
        self.speech
            .require_all_asserted(&format!("before Verbatim injects the keys {keys:?}"));
        self.timeline.push_keys(keys);
        let control = self.control_client("inject keys from (send_keys_from_verbatim)")?;
        ok_or_error(control.request(Request::SendKeys {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        })?)?;
        Ok(())
    }

    /// Closes the application the scenario launched with its window titled
    /// `title`, before the scenario ends, as cleanup closes it
    /// ([`Scenario::clean_up`]), and fails unless it closed and exited.
    ///
    /// # Errors
    ///
    /// Returns an error if the scenario launched no window so titled, or
    /// anything did not close or exit.
    pub fn close_launched(&mut self, title: &str) -> io::Result<()> {
        self.begin_action(&format!("close the application titled {title:?}"));
        let index = self
            .launched
            .iter()
            .position(|launched| launched.title.as_deref() == Some(title))
            .ok_or_else(|| {
                io::Error::other(format!("the scenario launched no window titled {title:?}"))
            })?;
        let launched = self.launched.remove(index);
        self.started.push(launched.pid);
        self.started.extend(launched.owners.iter().copied());
        let problems = self.close(&launched);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(problems.join("\n")))
        }
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
    /// At cleanup, when `owner_exits`, every window of the process that
    /// owned it is closed by that process, which must exit then; otherwise
    /// the window is closed by its handle. The launched process must have
    /// exited then too.
    ///
    /// A window opened in front is refused the foreground unless the agent
    /// may allow it, which it may only while it injected the last input
    /// (`books/dev/src/how-to/troubleshooting.md`, "Windows' foreground lock keeps launched
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
        self.launch_titled_once(command, args, title, owner_exits, None)
    }

    /// [`Scenario::launch_titled`], bringing the window forward only once
    /// the file `ready` exists, when given: the evidence that the program
    /// has written what it writes as it starts, which is then on screen
    /// before its window is announced and not new to the user.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch_titled`], and when `ready` is not written in
    /// time.
    pub fn launch_titled_once(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        owner_exits: bool,
        ready: Option<&str>,
    ) -> io::Result<WindowInfo> {
        self.begin_action(&launch_action(command, title));
        let launch = self.agent.launch_minimized(command, args)?;
        self.bring_forward(launch, title, Closing::launched(owner_exits), ready)
    }

    /// [`Scenario::launch_titled`], the program's standard input a pipe
    /// [`Scenario::write_stdin`] writes to and its stdout and stderr
    /// captured into `output_to` on the agent's machine, for a program
    /// driven by its standard input, such as `mockapp`. The process that
    /// owned the window must exit at cleanup.
    ///
    /// # Errors
    ///
    /// As [`Scenario::launch_titled`].
    pub fn launch_titled_with_stdin(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        output_to: &str,
    ) -> io::Result<WindowInfo> {
        self.begin_action(&launch_action(command, title));
        let launch = self
            .agent
            .launch_minimized_with_stdin(command, args, output_to)?;
        self.bring_forward(launch, title, Closing::ByOwner, None)
    }

    /// Writes `line` and a line feed to the standard input of `pid`,
    /// launched with [`Scenario::launch_titled_with_stdin`].
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when the process
    /// has no stdin pipe or has exited.
    pub fn write_stdin(&mut self, pid: u32, line: &str) -> io::Result<()> {
        self.begin_action(&format!("write {line:?} to the standard input of {pid}"));
        self.agent.write_stdin(pid, &format!("{line}\n"))
    }

    /// Creates the named event `name` on the agent's machine, not yet set,
    /// for a process the scenario launched to set and
    /// [`Scenario::wait_for_event`] to wait on.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn create_event(&mut self, name: &str) -> io::Result<()> {
        self.agent.create_event(name)
    }

    /// Waits up to `timeout` for `pid` to set the event `name`
    /// [`Scenario::create_event`] created: the evidence that the process
    /// reached the point that sets it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, `pid` exits first, or the
    /// event is not set within `timeout`.
    pub fn wait_for_event(&mut self, name: &str, pid: u32, timeout: Duration) -> io::Result<()> {
        match self.agent.wait_for_event(name, pid, timeout)? {
            EventOutcome::Signalled => Ok(()),
            EventOutcome::Exited { exit_code } => Err(io::Error::other(format!(
                "process {pid} exited ({exit_code:?}) before it set {name:?}"
            ))),
            EventOutcome::TimedOut => Err(io::Error::other(format!(
                "process {pid} did not set {name:?} within {timeout:?}"
            ))),
        }
    }

    /// Records `launch` for cleanup, its window titled with `title` and
    /// closed as `closing` says, waits for that window,
    /// opened minimized and inactive, to appear, restores it and sets it as
    /// the foreground, injecting nothing, and waits for it to take the
    /// foreground ([`Scenario::launch_titled`]); with `ready`, only once
    /// that file exists ([`Scenario::launch_titled_once`]).
    fn bring_forward(
        &mut self,
        launch: AgentLaunch,
        title: &str,
        closing: Closing,
        ready: Option<&str>,
    ) -> io::Result<WindowInfo> {
        self.launched.push(Launched {
            pid: launch.pid,
            title: Some(title.to_owned()),
            window: None,
            owners: Vec::new(),
            closing,
            document: None,
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
        let Some(window) = desktop
            .windows
            .iter()
            .find(|window| present && !window.cloaked && window.title.contains(title))
        else {
            let evidence =
                LaunchEvidence::record(&mut self.agent, &self.run_dir, &desktop, None, launch.pid);
            self.launch_evidence.push(evidence.clone());
            return Err(WindowWaitFailure::error(
                format!(
                    "no window titled {title:?} opened within {WINDOW_TIMEOUT:?}: {}",
                    describe_foreground(&desktop)
                ),
                evidence,
            ));
        };
        if let Some(launched) = self.launched.last_mut() {
            launched.window = Some(window.clone());
        }
        if let Some(ready) = ready
            && !self.agent.wait_for_file(ready, WINDOW_TIMEOUT)?
        {
            let message = format!(
                "{ready} was not written within {WINDOW_TIMEOUT:?}, so the window titled {title:?} was not brought forward"
            );
            let desktop = match self.agent.foreground_info() {
                Ok(desktop) => desktop,
                Err(error) => {
                    return Err(io::Error::other(format!(
                        "{message}; the desktop could not be read to record the launch: {error}"
                    )));
                }
            };
            let evidence = LaunchEvidence::record(
                &mut self.agent,
                &self.run_dir,
                &desktop,
                Some(window.window),
                launch.pid,
            );
            self.launch_evidence.push(evidence.clone());
            return Err(WindowWaitFailure::error(message, evidence));
        }
        let outcome = self.agent.set_foreground(window.window)?;
        if !outcome.in_front() {
            return Err(io::Error::other(format!(
                "the window titled {title:?} could not be brought to the foreground: {}",
                not_brought_forward(&mut self.agent, outcome)
            )));
        }
        let opened = window.window;
        self.require_in_front(title, launch, opened)
    }

    /// Launches `command`, which opens a window titled with `title`, a
    /// title of this run's own ([`harness_marker`]), minimized and inactive,
    /// brings it forward as [`Scenario::launch_titled`] does, and fails
    /// unless the process the agent launched owns it: for a program that
    /// could otherwise hand its command line to an instance already
    /// running, such as Windows Terminal. Its windows are closed by its
    /// process at cleanup, and the process must exit then. With `ready`, the window
    /// is brought forward only once that file exists
    /// ([`Scenario::launch_titled_once`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, `ready` is not
    /// written in time, the window does not take the foreground, or
    /// another process owns it.
    pub fn launch_owning_window(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        ready: Option<&str>,
    ) -> io::Result<WindowInfo> {
        self.begin_action(&launch_action(command, title));
        let launch = self.agent.launch_minimized(command, args)?;
        let launched = launch.pid;
        let window = self.bring_forward(launch, title, Closing::ByOwner, ready)?;
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

    /// Every visible, titled top-level window, owned, minimized, and
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
    /// does; with `ready`, only once that file exists
    /// ([`Scenario::launch_titled_once`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot start `command`, `ready` is not
    /// written in time, or the window does not take the foreground.
    pub fn launch_console(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
        ready: Option<&str>,
    ) -> io::Result<WindowInfo> {
        self.begin_action(&launch_action(command, title));
        let launch = self.agent.launch_console(command, args, title)?;
        self.bring_forward(launch, title, Closing::ByOwner, ready)
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

    /// Brings the desktop back to the foreground as a scenario's start puts
    /// it there ([`AgentClient::minimize_all`]), for a teardown whose body
    /// left the keyboard focus in the shell rather than in a window the
    /// harness closes: after Escape closes the Windows+X menu, the taskbar
    /// keeps the foreground and its Start button's tooltip stays shown, a
    /// window cleanup would find left open.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or the desktop does not take the
    /// foreground, naming what kept it, as the start does.
    pub fn return_to_desktop(&mut self) -> io::Result<()> {
        self.begin_action("bring the desktop back to the foreground");
        let (taskbar, outcome, desktop) = self.agent.minimize_all(MINIMIZE_TIMEOUT)?;
        match minimize_failure(&taskbar, outcome, &desktop) {
            Some(failure) => Err(io::Error::other(failure)),
            None => Ok(()),
        }
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
        self.begin_action(&format!("bring forward the window titled {marker:?}"));
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
        let outcome = self.agent.set_foreground(window.window)?;
        if !outcome.in_front() {
            return Err(io::Error::other(format!(
                "the window {:?} could not be brought to the foreground: {}",
                window.title,
                not_brought_forward(&mut self.agent, outcome)
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
    /// opens it without the agent's right to take the foreground, and
    /// brings its window, titled with that name, forward as
    /// [`Scenario::launch_titled`] does. The desktop's shell, which holds
    /// the foreground as every scenario starts, opens the folder window in
    /// front by its own right, raising its foreground event before the
    /// window is shown; when it does not, the window opens restored and
    /// inactive and is brought forward. It is not opened minimized: a
    /// folder window the shell opens minimized takes the foreground while
    /// minimized, and once restored its file list has no keyboard focus,
    /// in NVDA as in Verbatim (measured 2026-10-09;
    /// `books/dev/src/how-to/troubleshooting.md`). The window is closed at cleanup by
    /// its handle, recorded as it opened, since File Explorer retitles it
    /// as it navigates; `explorer.exe` is the shell, so its process stays.
    /// Returns the folder's name, which is the window's title.
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
        self.begin_action(&format!(
            "open the folder {marker:?} in File Explorer, brought forward"
        ));
        let launch = self
            .agent
            .launch_without_foreground_right("explorer.exe", std::slice::from_ref(&folder))?;
        self.bring_forward(launch, &marker, Closing::ByHandle, None)?;
        Ok(marker)
    }

    /// Opens a page of the Settings app by its `ms-settings:` URI, minimized
    /// and inactive, and brings the Settings window forward as
    /// [`Scenario::launch_titled`] does, so it takes the foreground
    /// whatever input came last. No Settings window
    /// may be open before, so the window is the scenario's own; it is
    /// closed by its title at cleanup ([`Closing::ByTitle`]). The Settings app's process may stay,
    /// suspended, with its window cloaked, as it does after a user closes
    /// it.
    ///
    /// # Errors
    ///
    /// Returns an error if a Settings window is already open, a request
    /// fails, or the window does not take the foreground.
    pub fn open_settings_page(&mut self, uri: &str) -> io::Result<WindowInfo> {
        self.require_absent(SETTINGS_TITLE)?;
        self.begin_action(&launch_action("explorer.exe", SETTINGS_TITLE));
        let launch = self
            .agent
            .launch_minimized("explorer.exe", &[uri.to_owned()])?;
        self.bring_forward(launch, SETTINGS_TITLE, Closing::ByTitle, None)
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

    /// Waits for the window titled with `title`, which `launch` opened as
    /// window `opened`, to take the foreground, and records the process
    /// that owns it.
    fn require_in_front(
        &mut self,
        title: &str,
        launch: AgentLaunch,
        opened: u64,
    ) -> io::Result<WindowInfo> {
        let (met, desktop) = self.agent.wait_for_window(
            WindowCondition::Foreground {
                title_contains: title.to_owned(),
                unsaved: None,
            },
            WINDOW_TIMEOUT,
        )?;
        let Some(window) = desktop.foreground.clone().filter(|_| met) else {
            let evidence = LaunchEvidence::record(
                &mut self.agent,
                &self.run_dir,
                &desktop,
                Some(opened),
                launch.pid,
            );
            self.launch_evidence.push(evidence.clone());
            return Err(WindowWaitFailure::error(
                format!(
                    "the window titled {title:?} did not take the foreground within {WINDOW_TIMEOUT:?}{}: {}",
                    if launch.foreground_allowed {
                        ""
                    } else {
                        " (Windows did not let the agent allow its launch to take the foreground)"
                    },
                    describe_foreground(&desktop)
                ),
                evidence,
            ));
        };
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
        self.begin_action(&format!("run {command} {}", args.join(" ")));
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

    /// [`Scenario::foreground_report`] for the record taken after cleanup,
    /// with a suspended Settings app named apart rather than among the
    /// visible windows: a `SystemSettings.exe` whose every window is
    /// cloaked ([`settings_prelaunch`]). The cloaked frame of
    /// `ApplicationFrameHost.exe` that hosts it stays listed.
    pub fn cleanup_report(&mut self) -> String {
        let mut info = match self.agent.foreground_info() {
            Ok(info) => info,
            Err(error) => return format!("the foreground could not be read: {error}"),
        };
        let prelaunch = settings_prelaunch(&info.windows);
        if prelaunch.is_empty() {
            return describe_foreground(&info);
        }
        let (prelaunched, shown): (Vec<WindowInfo>, Vec<WindowInfo>) = info
            .windows
            .into_iter()
            .partition(|window| prelaunch.contains(&window.pid));
        info.windows = shown;
        let named: Vec<String> = prelaunched
            .iter()
            .map(|window| format!("pid {} ({:?}, cloaked)", window.pid, window.title))
            .collect();
        format!(
            "{}; the Settings app suspended with every window cloaked, Windows' prelaunch or a \
             Settings left suspended by this or an earlier scenario, no window a scenario \
             left: {}",
            describe_foreground(&info),
            named.join(", ")
        )
    }

    /// What Verbatim says as the desktop takes the focus, the state every
    /// scenario starts from: the desktop's list of icons, then the icon that
    /// has the focus, whose name, position, and selection the agent reads
    /// through MSAA, independently of Verbatim, since they belong
    /// to the machine. The desktop's window, Program Manager, is not spoken:
    /// NVDA's captures of 2026-10-11 (NVDA alpha-57645, on this project's
    /// development machine), starting with the desktop focused and after
    /// Escape from Verbatim's settings dialog, both said "Desktop list" and
    /// the item alone, as every capture of 2026-10-12 with alpha-58022 did, and Dickson confirms NVDA never says "Program
    /// Manager". The window must be the desktop's, which its title tells.
    ///
    /// # Errors
    ///
    /// Returns an error if the foreground is not the desktop, or the agent
    /// cannot read the focus or the focused item's position.
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
        let ListPosition::At {
            position,
            count: size,
        } = focus.position
        else {
            return Err(io::Error::other(format!(
                "the desktop item's position could not be read: {}; the focused object is named {:?}",
                focus.position.describe(),
                focus.name
            )));
        };
        let state = match focus.selected {
            Some(false) => " not selected",
            _ => "",
        };
        Ok(vec![
            "Desktop list".to_owned(),
            format!("{}{state} {position} of {size}", focus.name),
        ])
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
        if self.is_comparing() {
            self.timeline.push_action(&format!(
                "focus the element with the automation id {automation_id:?}"
            ));
        }
        self.agent.focus_by_automation_id(automation_id)
    }

    /// The name of the foreground window's element whose UI Automation
    /// identifier is `automation_id`, read by the agent through UI
    /// Automation independently of Verbatim, without focusing it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn name_by_automation_id(&mut self, automation_id: &str) -> io::Result<String> {
        self.agent.name_by_automation_id(automation_id)
    }

    /// Of the first element the focused element controls (its UI
    /// Automation `ControllerFor` relation), the set size its first child
    /// reports (`SizeOfSet`) and the names of its first `names` children, read by the agent independently of
    /// Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn controlled_items(&mut self, names: u32) -> io::Result<(u32, Vec<String>)> {
        self.agent.controlled_items(names)
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

    /// Turns the lock key `key` (such as `capslock`) on or off, as `on`
    /// says, for a teardown restoring the state a scenario found: its state
    /// is read through the agent, and when it differs the key is pressed
    /// once through the agent. Called once Verbatim has quit, so nothing
    /// swallows the press; no speech is checked, since there is none.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails.
    pub fn restore_key_toggled(&mut self, key: &str, on: bool) -> io::Result<()> {
        if self.agent.key_toggled(key)? != on {
            self.agent.send_keys(&[key.to_owned()])?;
        }
        Ok(())
    }

    /// Whether Windows' screen reader flag is set, read by the agent
    /// independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn screen_reader_flag(&mut self) -> io::Result<bool> {
        self.agent.screen_reader_flag()
    }

    /// The local time, without seconds, and the local date, in the long
    /// format, each in the user's locale formats, read by the agent
    /// independently of Verbatim; when the next minute is due within
    /// `turn_within`, read once it has turned.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn local_clock(&mut self, turn_within: Duration) -> io::Result<(String, String)> {
        self.agent.local_clock(turn_within)
    }

    /// The screen position of the character on `line` at `column`, both
    /// from zero, of the focused Win32 edit control, read by the agent as
    /// the control gives it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn edit_character_position(&mut self, line: u32, column: u32) -> io::Result<(i32, i32)> {
        self.agent.edit_character_position(line, column)
    }

    /// The screen position of the character on `line` at `column`, both
    /// from zero, of the focused UI Automation element's text, read by the
    /// agent.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn uia_character_position(&mut self, line: u32, column: u32) -> io::Result<(i32, i32)> {
        self.agent.uia_character_position(line, column)
    }

    /// Every item of the open Alt+Tab switcher's list, read by the agent,
    /// in the list's order: each one's name, its row, column and position,
    /// each from one, and the list's size.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn switcher_items(&mut self) -> io::Result<Vec<SwitcherItem>> {
        self.agent.switcher_items()
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
        let pid = self.pid("list the processes of (verbatim_children)")?;
        self.agent.child_processes(pid)
    }

    /// Ends one of Verbatim's own processes, which the scenario then
    /// expects to have exited, and waits for it to go.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be ended or does not exit
    /// within `timeout`.
    pub fn end_verbatim_process(&mut self, pid: u32, timeout: Duration) -> io::Result<()> {
        self.pid("end a process of (end_verbatim_process)")?;
        self.expected_exits.push(pid);
        self.agent.kill_process(pid)?;
        match self.agent.wait_for_exit(pid, timeout)? {
            ProcessState::Exited { .. } => Ok(()),
            ProcessState::Running => Err(io::Error::other(format!(
                "process {pid} did not exit within {timeout:?}"
            ))),
        }
    }

    /// Waits up to `timeout` for a process of `image`, such as
    /// `verbatim-listener.exe`, other than those in `known`, to be running
    /// in Verbatim's job, on the job's own messages: the evidence that
    /// Verbatim replaced one the scenario ended. Returns its process id.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or none joined in time.
    pub fn wait_for_verbatim_process(
        &mut self,
        image: &str,
        known: &[u32],
        timeout: Duration,
    ) -> io::Result<u32> {
        let pid = self.pid("wait for a process of (wait_for_verbatim_process)")?;
        self.agent.wait_for_job_process(pid, image, known, timeout)
    }

    /// Starts a second Verbatim, as a user starting it again does, which
    /// replaces the running one, and goes on with the second. First, as
    /// before any input, Verbatim is idle and every utterance it queued
    /// has been asserted. The running Verbatim must then exit with code 0,
    /// every outpost and listener of its job shut down cleanly
    /// ([`Scenario::outposts_shut_down_cleanly`]), and its speech
    /// connection end with nothing more said; the second must say it is
    /// ready. Its speech is pushed to the same timeline, and its startup
    /// speech is the next the scenario asserts. The replaced Verbatim's
    /// stderr log and outpost logs are collected with the run's artifacts.
    /// Until the second is connected to, the scenario has no control
    /// connection, so a failure on the way leaves both to
    /// [`Scenario::quit_verbatim`], which ends each still running by its
    /// process id.
    ///
    /// # Errors
    ///
    /// Returns an error if the second cannot be launched or connected to,
    /// the first does not exit with code 0 or leaves its processes
    /// unclean, its speech connection does not end, or the second does not
    /// become ready.
    ///
    /// # Panics
    ///
    /// Panics if Verbatim does not become idle or anything said was not
    /// asserted, before the second start or as the first exits.
    pub fn replace_verbatim(&mut self) -> io::Result<()> {
        let replaced = self.pid("replace (replace_verbatim)")?;
        self.speech.expect_nothing_more(self.last_input);
        self.timeline.push_action("start a second Verbatim");
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let ready_event = ready_event_name();
        self.agent.create_event(&ready_event)?;
        let mut env = vec![(READY_EVENT_ENV.to_owned(), ready_event.clone())];
        env.extend(self.launch_env.iter().cloned());
        let stderr_path = replacement_stderr_path(&self.stderr_log_path);
        let (launched, ignored) =
            self.agent
                .launch_verbatim(&self.launch_exe, &self.run_dir, &env, &stderr_path)?;
        // From here the second is the scenario's Verbatim, for cleanup to
        // end whatever happens next; the first's artifacts are kept.
        let replaced_stderr = std::mem::replace(&mut self.stderr_log_path, stderr_path);
        self.replaced.push((replaced, replaced_stderr));
        self.verbatim_pid = Some(launched.pid);
        self.ignored = ignored;
        self.control = None;
        self.last_input = None;
        match self.agent.wait_for_exit(replaced, QUIT_TIMEOUT)? {
            ProcessState::Exited { exit_code: Some(0) } => {}
            ProcessState::Exited { exit_code } => {
                return Err(io::Error::other(format!(
                    "the replaced Verbatim (pid {replaced}) exited with {exit_code:?}"
                )));
            }
            ProcessState::Running => {
                return Err(io::Error::other(format!(
                    "the replaced Verbatim (pid {replaced}) is still running {QUIT_TIMEOUT:?} after the second start"
                )));
            }
        }
        let problems = self.children_shut_down_cleanly(replaced)?;
        if !problems.is_empty() {
            return Err(io::Error::other(format!(
                "the replaced Verbatim (pid {replaced}) did not shut down cleanly: {}",
                problems.join("; ")
            )));
        }
        self.speech.take_until_the_end().map_err(io::Error::other)?;
        self.speech
            .require_all_asserted("after the replaced Verbatim exited");
        match self
            .agent
            .wait_for_event(&ready_event, launched.pid, READY_TIMEOUT)?
        {
            EventOutcome::Signalled => {}
            EventOutcome::Exited { exit_code } => {
                return Err(io::Error::other(format!(
                    "the second Verbatim exited ({exit_code:?}) before it was ready; see {}",
                    self.stderr_log_path
                )));
            }
            EventOutcome::TimedOut => {
                return Err(io::Error::other(format!(
                    "the second Verbatim did not say it was ready within {READY_TIMEOUT:?}"
                )));
            }
        }
        let (mut control, speech) = connect_with(&agent_addr, self.timeline.clone())?;
        require_ready(&mut control)?;
        self.control = Some(control);
        self.speech = speech;
        Ok(())
    }

    /// Ends, by process id and with everything in its job, each Verbatim
    /// the scenario started that may still be running: the one it runs
    /// with and each a second start replaced, one of which a failed
    /// replacement can leave running. Each job's emptying, on the agent's
    /// job handle, is the evidence that none of its processes still holds
    /// a log open when the logs are collected and deleted. Returns each
    /// one that could not be ended.
    fn end_by_pid(&mut self) -> Vec<String> {
        let pids: Vec<u32> = self
            .replaced
            .iter()
            .map(|(pid, _)| *pid)
            .chain(self.verbatim_pid)
            .collect();
        let mut problems = Vec::new();
        for pid in pids {
            if let Err(error) = self.agent.kill_process(pid) {
                problems.push(format!("could not end Verbatim (pid {pid}): {error}"));
                continue;
            }
            if let Err(error) = self.agent.wait_for_job_empty(pid, JOB_EMPTY_TIMEOUT) {
                problems.push(format!(
                    "the job of Verbatim (pid {pid}) did not empty once it was ended: {error}"
                ));
            }
        }
        problems
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
        let verbatim = self.pid("read the process exits of")?;
        Ok(self
            .agent
            .job_exits(verbatim)?
            .into_iter()
            .filter(|exit| {
                exit.pid != verbatim
                    && !expected.contains(&exit.pid)
                    && (exit.abnormal || exit.exit_code != Some(0))
            })
            .collect())
    }

    /// Asks Verbatim to exit cleanly (`Request::Quit`), and waits on its
    /// process handle for it to exit with code 0. A second start
    /// ([`Scenario::replace_verbatim`]) that failed before it was connected
    /// to leaves no connection to ask: every Verbatim the scenario started
    /// is then ended by its process id instead ([`Scenario::end_by_pid`]),
    /// which is a failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, Verbatim does not exit within
    /// the timeout, it exits with any other code, or there was no
    /// connection to ask.
    pub fn quit_verbatim(&mut self) -> io::Result<()> {
        let pid = self.pid("quit")?;
        // Its exit with code 0 was checked when it exited.
        if self.exited {
            return Ok(());
        }
        self.quit = true;
        if self.control.is_none() {
            let mut problems = vec![format!(
                "Verbatim (pid {pid}) has no control connection, since its start as a replacement did not finish, so every Verbatim the scenario started was ended by its process id"
            )];
            problems.extend(self.end_by_pid());
            return Err(io::Error::other(problems.join("; ")));
        }
        // The reply races Verbatim's own teardown: the connection can close
        // before it is written. The exit code, read below, is the
        // authority on whether the quit worked.
        match self.control_client("quit")?.request(Request::Quit) {
            Ok(frame) => {
                ok_or_error(frame)?;
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {}
            Err(error) => return Err(error),
        }
        match self.agent.wait_for_exit(pid, QUIT_TIMEOUT)? {
            ProcessState::Exited { exit_code: Some(0) } => Ok(()),
            ProcessState::Exited { exit_code } => Err(io::Error::other(format!(
                "Verbatim (pid {pid}) exited with {exit_code:?} after Quit"
            ))),
            ProcessState::Running => Err(io::Error::other(format!(
                "Verbatim (pid {pid}) is still running {QUIT_TIMEOUT:?} after Quit"
            ))),
        }
    }

    /// Checks, once Verbatim has quit, that it left no outpost or focus
    /// listener behind and that every one shut down cleanly rather than
    /// being killed: each `verbatim-outpost.exe` and `verbatim-listener.exe`
    /// process of its job, but
    /// one the scenario ended on purpose, exited with code 0, not
    /// abnormally, and before Verbatim itself did. A child Verbatim's
    /// supervisor had to kill exits with `verbatim_process::KILLED_EXIT_CODE`;
    /// one still running when Verbatim exited ends with its job afterwards.
    /// At least one must have exited, the focus listener. The synthesizer
    /// host, which ends with the job, must not have crashed: no exit Windows
    /// called abnormal and no exception code, since
    /// [`Scenario::unexpected_exits`] is read before the quit. Returns every
    /// problem found.
    ///
    /// # Errors
    ///
    /// Returns an error if the job's processes cannot be read, or some are
    /// still running once [`JOB_EMPTY_TIMEOUT`] has passed.
    pub fn outposts_shut_down_cleanly(&mut self) -> io::Result<Vec<String>> {
        let pid = self.pid("check the outposts of")?;
        self.children_shut_down_cleanly(pid)
    }

    /// [`Scenario::outposts_shut_down_cleanly`] for the Verbatim `pid`,
    /// once it has exited.
    fn children_shut_down_cleanly(&mut self, pid: u32) -> io::Result<Vec<String>> {
        let exits = self.agent.wait_for_job_empty(pid, JOB_EMPTY_TIMEOUT)?;
        let verbatim_exited_at = exits.iter().position(|exit| exit.pid == pid);
        let mut problems = Vec::new();
        let mut outposts = 0;
        for (index, exit) in exits.iter().enumerate() {
            if self.expected_exits.contains(&exit.pid) {
                continue;
            }
            if exit.image.eq_ignore_ascii_case(SYNTH_HOST_IMAGE) {
                if exit.abnormal || exit.exit_code.is_some_and(is_exception_code) {
                    problems.push(format!(
                        "the synthesizer host process {} crashed: exit code {:?}{}",
                        exit.pid,
                        exit.exit_code,
                        if exit.abnormal { ", abnormally" } else { "" }
                    ));
                }
                continue;
            }
            if !is_child_image(&exit.image) {
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

    /// Saves Core's focus, its ancestors, and the navigator object into
    /// `dir` as `focus.txt`, while Verbatim is up.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the file cannot be written.
    pub fn collect_focus(&mut self, dir: &Path) -> io::Result<()> {
        let control = self.control_client("save the focus of")?;
        let report = match ok_or_error(control.request(Request::DumpFocus)?)? {
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
        let control = self.control_client("dump the flight recorder of")?;
        let path = match ok_or_error(control.request(Request::DumpRecorder)?)? {
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
        fs::write(dir.join(crate::artifacts::FLIGHT_RECORDER_FILE), bytes)
    }

    /// Collects the run's artifacts into `dir`, once Verbatim has exited:
    /// the timeline, the latency report, Verbatim's stderr log and every
    /// outpost and listener log of this launch, Verbatim's audio, and the
    /// crash dumps of Verbatim's processes written during the run. Returns
    /// every artifact that could not be collected, every crash dump, and
    /// every outpost started for a process Verbatim was told to ignore, its
    /// log named for that process, each a failure of the run. In compare
    /// mode, which has no Verbatim, only the timeline.
    pub fn collect_run_artifacts(&mut self, dir: &Path) -> Vec<String> {
        let mut problems = Vec::new();
        if let Err(error) = fs::create_dir_all(dir) {
            return vec![format!("could not create {}: {error}", dir.display())];
        }
        if let Err(error) = fs::write(dir.join(TIMELINE_FILE_NAME), self.timeline.render()) {
            problems.push(format!("could not write the timeline: {error}"));
        }
        if let Err(error) = write_launch_evidence(dir, &self.launch_evidence) {
            problems.push(format!(
                "could not write what was recorded of a launch whose window did not appear: {error}"
            ));
        }
        if self.is_comparing() {
            return problems;
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
        match self.pid("collect the logs of") {
            Ok(pid) => problems.extend(self.collect_outpost_logs(pid, dir)),
            Err(error) => problems.push(error.to_string()),
        }
        for (pid, stderr_path) in self.replaced.clone() {
            let replaced_dir = dir.join(format!("replaced-{pid}"));
            if let Err(error) = fs::create_dir_all(&replaced_dir) {
                problems.push(format!(
                    "could not create {}: {error}",
                    replaced_dir.display()
                ));
                continue;
            }
            if let Err(error) = self
                .agent
                .copy_file(&stderr_path, &replaced_dir.join(STDERR_FILE_NAME))
            {
                problems.push(format!(
                    "could not collect the replaced Verbatim's stderr log {stderr_path}: {error}"
                ));
            }
            problems.extend(self.collect_outpost_logs(pid, &replaced_dir));
        }
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

    /// Fetches every outpost and listener log the Verbatim launch `pid`
    /// wrote, from `logs\<pid>` next to its executable, into `dir`.
    fn collect_outpost_logs(&mut self, pid: u32, dir: &Path) -> Vec<String> {
        let logs_dir = format!(r"{}\logs\{pid}", self.run_dir);
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
        for name in &names {
            let remote_path = format!(r"{logs_dir}\{name}");
            // In chunks: an outpost's debug log over a long flood is larger
            // than one read may be.
            if let Err(error) = self.agent.copy_file(&remote_path, &dir.join(name)) {
                problems.push(format!("could not collect {remote_path}: {error}"));
            }
        }
        for pid in &self.ignored {
            let watching = names.iter().find(|name| {
                outpost_log_names(name, *pid)
                    || (name.starts_with(SPARE_LOG_PREFIX)
                        && fs::read_to_string(dir.join(name))
                            .is_ok_and(|log| log_assigns(&log, *pid)))
            });
            if let Some(name) = watching {
                problems.push(format!(
                    "an outpost was started for process {pid}, which Verbatim was told to ignore (the owner's Windows Terminal): {name}"
                ));
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
            // In chunks: a full dump of an outpost is a few hundred
            // megabytes.
            match self.agent.copy_file(&remote, &dir.join(&name)) {
                Ok(()) => {
                    problems.push(format!("a Verbatim process crashed: its dump is {name}"));
                    self.dumps_collected.push(name);
                }
                Err(error) => problems.push(format!(
                    "a Verbatim process crashed, and its dump {remote} could not be copied: {error}"
                )),
            }
        }
        problems
    }

    /// The outposts that crashed during the run, read from the exits the
    /// agent recorded for Verbatim's job once Verbatim has exited and
    /// [`Scenario::collect_run_artifacts`] has copied the crash dumps into
    /// `dir`: every `verbatim-outpost.exe` or `verbatim-listener.exe` the
    /// scenario did not end on
    /// purpose whose exit Windows called abnormal or whose exit code is an
    /// exception code, each with the copy of its dump in `dir` when Windows
    /// Error Reporting wrote one (`<image>.<pid>.dmp`, such as
    /// `verbatim-outpost.exe.4242.dmp`). Each is
    /// also one of [`Scenario::unexpected_exits`], so a crash fails the
    /// scenario as well as being counted here.
    ///
    /// # Errors
    ///
    /// Returns an error if the job's exits cannot be read.
    pub fn outpost_crashes(&mut self, dir: &Path) -> io::Result<Vec<OutpostCrash>> {
        let pid = self.pid("read the outpost crashes of")?;
        let exits = self.agent.job_exits(pid)?;
        Ok(exits
            .into_iter()
            .filter(|exit| {
                is_child_image(&exit.image)
                    && !self.expected_exits.contains(&exit.pid)
                    && (exit.abnormal || exit.exit_code.is_some_and(is_exception_code))
            })
            .map(|exit| {
                let name = format!("{}.{}.dmp", exit.image, exit.pid);
                OutpostCrash {
                    pid: exit.pid,
                    exit_code: exit.exit_code,
                    dump: self
                        .dumps_collected
                        .iter()
                        .find(|collected| collected.eq_ignore_ascii_case(&name))
                        .map(|collected| dir.join(collected).display().to_string()),
                }
            })
            .collect())
    }

    /// Finishes this run's video and saves it, with Verbatim's audio, to
    /// `to`, checking that it reaches `body_end_ms`, when the scenario's
    /// body ended, in milliseconds since the Unix epoch. Does nothing when
    /// the run is not recording.
    ///
    /// # Errors
    ///
    /// Returns an error if the video cannot be finished or saved, or fails
    /// its check.
    pub fn finish_recording(&mut self, to: &Path, body_end_ms: u64) -> io::Result<()> {
        let Some(mut recording) = self.recording.take() else {
            return Ok(());
        };
        recording.finish(&mut self.agent, to, body_end_ms)?;
        println!("recording saved to {}", to.display());
        Ok(())
    }

    /// Closes what the scenario opened, last opened first, as each launch's
    /// [`Closing`] says: every window of a program the scenario started by
    /// its owning process, a window a shared process owns by its handle (a
    /// File Explorer window before its folder is deleted), a Notepad
    /// harness tab as a tab, and the Settings app's window by its title;
    /// waiting for them to go and, when the scenario started the program,
    /// for the processes that owned them to exit (a Notepad that holds
    /// other tabs is ended as [`Scenario::end_notepad`] says); then for the
    /// process it launched to exit, ending it through the agent's own
    /// handle when it still runs ([`Scenario::end_launch`]); anything
    /// launched without a window of its own by its process id; then the
    /// harness documents, folders, and files. Last, it
    /// checks that every window shown now that was not open when the
    /// scenario started has gone, whatever its title has become, waiting
    /// for them on window events up to the close timeout and closing none
    /// itself. Returns everything that could not be closed or deleted, and
    /// each window left open, each a failure.
    pub fn clean_up(&mut self) -> Vec<String> {
        self.cleaned_up = true;
        let mut problems = self.release_held_keys();
        self.started
            .extend(self.launched.iter().flat_map(|launched| {
                std::iter::once(launched.pid).chain(launched.owners.iter().copied())
            }));
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
        // The closing steps close the scenario's own windows; this proves
        // nothing was left. A window left open stays open, and the
        // next scenario's start reports it again and closes it by handle.
        match self.agent.wait_for_new_windows_gone(CLOSE_TIMEOUT) {
            Ok(remaining) => problems.extend(remaining.iter().map(|window| {
                format!(
                    "a window that was not open when the scenario started was still open \
                     {CLOSE_TIMEOUT:?} after the scenario's closing steps: {}",
                    describe_window(window)
                )
            })),
            Err(error) => problems.push(format!(
                "could not check that the scenario left no window open: {error}"
            )),
        }
        problems
    }

    /// Closes one launched application, as [`Scenario::clean_up`] says.
    fn close(&mut self, launched: &Launched) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(title) = &launched.title {
            // Whether every harness tab closed, so that Notepad's remaining
            // windows hold none and may be closed ([`Scenario::end_notepad`]).
            let mut tabs_closed = false;
            match launched.closing {
                Closing::NotepadTabs => {
                    // Every harness tab, the scenario's and any Notepad
                    // restored from an earlier run's session, so Notepad can
                    // exit. Never its window while a harness tab is open:
                    // Notepad keeps the tabs of a window that closes for its
                    // next session.
                    match close_notepad_tabs(&mut self.agent, DOCUMENT_MARKER) {
                        Ok(()) => tabs_closed = true,
                        Err(error) => problems.push(error.to_string()),
                    }
                }
                Closing::ByOwner => {
                    // Each process that owned a window: the owners recorded
                    // as the window took the foreground, and the window's
                    // own when it opened but did not.
                    let mut pids = launched.owners.clone();
                    if let Some(window) = &launched.window
                        && !pids.contains(&window.pid)
                    {
                        pids.push(window.pid);
                    }
                    for pid in pids {
                        match self.agent.close_process_windows(pid, CLOSE_TIMEOUT) {
                            Ok(remaining) => problems.extend(not_closed(
                                &format!("the window(s) of process {pid}, which owned the window titled {title:?},"),
                                &remaining,
                            )),
                            Err(error) => problems.push(format!(
                                "could not close the windows of process {pid}, which owned the window titled {title:?}: {error}"
                            )),
                        }
                    }
                }
                Closing::ByHandle => {
                    // A window that never opened has nothing to close; its
                    // launch is ended below if it still runs.
                    if let Some(window) = &launched.window {
                        match self.agent.close_window(window, CLOSE_TIMEOUT) {
                            Ok(None) => {}
                            Ok(Some(left)) => problems.push(format!(
                                "the window {} did not close within {CLOSE_TIMEOUT:?}",
                                describe_window(&left)
                            )),
                            Err(error) => problems.push(format!(
                                "could not close {}: {error}",
                                describe_window(window)
                            )),
                        }
                    }
                }
                Closing::ByTitle => match self.agent.close_windows(title, CLOSE_TIMEOUT) {
                    Ok(remaining) => problems.extend(not_closed(
                        &format!("the window(s) titled {title:?}"),
                        &remaining,
                    )),
                    Err(error) => problems.push(format!("could not close {title:?}: {error}")),
                },
            }
            let owners: &[u32] = if launched.closing.owners_exit() {
                &launched.owners
            } else {
                &[]
            };
            for &owner in owners.iter().chain(&launched.also_exit) {
                if launched.closing == Closing::NotepadTabs && tabs_closed {
                    problems.extend(self.end_notepad(owner, title));
                    continue;
                }
                match self.agent.wait_for_exit(owner, CLOSE_TIMEOUT) {
                    Ok(ProcessState::Exited { .. }) => {}
                    Ok(ProcessState::Running) => problems.push(format!(
                        "process {owner}, which owned or ran in the window titled {title:?}, did not exit within {CLOSE_TIMEOUT:?} of the window closing"
                    )),
                    Err(error) => problems.push(format!(
                        "could not wait for process {owner} to exit: {error}"
                    )),
                }
            }
            let waited =
                owners.contains(&launched.pid) || launched.also_exit.contains(&launched.pid);
            problems.extend(self.end_launch(launched.pid, title, waited));
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

    /// Waits for process `pid`, which the agent launched to open the window
    /// titled `title`, to exit once its windows have closed, for up to the
    /// close timeout, or not at all when `waited`, since it owned the window
    /// and was waited for already; and when it still runs, ends it, with
    /// everything in its job, through the agent's own handles, as
    /// [`AgentClient::end_launched`] ends a launch an earlier run left, and
    /// reports that; a launch the agent no longer holds has exited, and its
    /// pid, which may name another process by then, is never opened. A launch that handed its window to another process
    /// and exited, as `explorer.exe` and a second `WindowsTerminal.exe` do,
    /// has exited long before, so a clean run waits for nothing here. The
    /// job holds only the launch and what it started, never a shared
    /// process such as the desktop's `explorer.exe`, so nothing else is
    /// ended. Returns each failure.
    fn end_launch(&mut self, pid: u32, title: &str, waited: bool) -> Vec<String> {
        let wait = if waited {
            Duration::ZERO
        } else {
            CLOSE_TIMEOUT
        };
        match self.agent.end_launch(pid, wait, CLOSE_TIMEOUT) {
            Ok(LaunchEnd::Exited) => Vec::new(),
            Ok(LaunchEnd::Ended) => vec![format!(
                "process {pid}, launched to open the window titled {title:?}, was still running after its windows closed, so it was ended with everything it started"
            )],
            Ok(LaunchEnd::StillRunning) => vec![format!(
                "process {pid}, launched to open the window titled {title:?}, did not exit after being ended"
            )],
            Err(error) => vec![format!(
                "could not end process {pid}, launched to open the window titled {title:?}: {error}"
            )],
        }
    }

    /// Ends Windows 11 Notepad process `owner` once the harness tabs of its
    /// window titled `title` have closed (Dickson, 2026-11-03). Notepad
    /// exits by itself when no tab is left; a tab that is not the
    /// harness's, such as one it restored from its previous session, keeps
    /// it running, which its remaining Notepad windows show. Those windows
    /// are closed by handle, so Notepad saves its session with the harness
    /// tabs already gone, and it is waited for. Only if it still runs is it
    /// killed, which is reported: killed before saving its session, Notepad
    /// restores the deleted harness document's tab in the next run behind a
    /// "Cannot find the file" dialog. Unsaved text in a window it closes is
    /// kept by Notepad's session; a kill loses it. The agent closes by
    /// handle only windows that were not open when the scenario started.
    /// Returns each step's failure.
    fn end_notepad(&mut self, owner: u32, title: &str) -> Vec<String> {
        let mut problems = Vec::new();
        let windows: Vec<WindowInfo> = match self.agent.foreground_info() {
            Ok(info) => info
                .windows
                .into_iter()
                .filter(|window| window.pid == owner && window.class == NOTEPAD_CLASS)
                .collect(),
            Err(error) => {
                problems.push(format!(
                    "could not find the windows of Notepad process {owner}: {error}"
                ));
                return problems;
            }
        };
        for window in &windows {
            match self.agent.close_window(window, CLOSE_TIMEOUT) {
                Ok(None) => {}
                Ok(Some(left)) => problems.push(format!(
                    "Notepad's window {} did not close within {CLOSE_TIMEOUT:?}",
                    describe_window(&left)
                )),
                Err(error) => problems.push(format!(
                    "could not close {}: {error}",
                    describe_window(window)
                )),
            }
        }
        match self.agent.wait_for_exit(owner, CLOSE_TIMEOUT) {
            Ok(ProcessState::Exited { .. }) => return problems,
            Ok(ProcessState::Running) => {}
            Err(error) => {
                problems.push(format!(
                    "could not wait for process {owner} to exit: {error}"
                ));
                return problems;
            }
        }
        let ended = self
            .agent
            .kill_process(owner)
            .and_then(|_| self.agent.wait_for_exit(owner, CLOSE_TIMEOUT));
        problems.push(match ended {
            Ok(ProcessState::Exited { .. }) => format!(
                "Notepad, process {owner}, which owned the window titled {title:?}, was killed: it                  was still running {CLOSE_TIMEOUT:?} after its harness tabs and its {} remaining                  Notepad window(s) were closed, so its session may hold a harness tab: {}",
                windows.len(),
                self.foreground_report()
            ),
            Ok(ProcessState::Running) => format!(
                "Notepad, process {owner}, which owned the window titled {title:?}, did not exit                  within {CLOSE_TIMEOUT:?} of being killed: {}",
                self.foreground_report()
            ),
            Err(error) => format!("could not end Notepad, process {owner}: {error}"),
        });
        problems
    }
}

/// The class of Windows 11 Notepad's top-level window.
const NOTEPAD_CLASS: &str = "Notepad";

impl Drop for Scenario {
    /// A scenario dropped without its cleanup having run, which happens
    /// only when the run panicked before reaching it: Verbatim is asked to
    /// quit, or, with no connection to ask or when it does not exit, ended
    /// by its process id, what the scenario opened is closed, and the
    /// capture is stopped as [`Recording::stop`] stops it, so its file is
    /// complete. Anything that will not close is printed; the run has
    /// already failed.
    fn drop(&mut self) {
        if !self.quit
            && let Some(pid) = self.verbatim_pid
        {
            let exited = self.control.as_mut().is_some_and(|control| {
                let _ = control.request(Request::Quit);
                matches!(
                    self.agent.wait_for_exit(pid, QUIT_TIMEOUT),
                    Ok(ProcessState::Exited { .. })
                )
            });
            if !exited {
                for problem in self.end_by_pid() {
                    eprintln!("cleanup after a failed run: {problem}");
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

/// The test-only variable, and its value, that shortens Verbatim's idle
/// retirement of an outpost (`verbatim_supervisor::TEST_RETIREMENT_ENV`)
/// from two minutes, looked for every 30 seconds, to 10 seconds, looked
/// for every second, for a scenario that waits for a retirement
/// (`ScenarioDef::quick_retirement`).
pub const QUICK_RETIREMENT: (&str, &str) = ("VERBATIM_TEST_RETIREMENT", "10000,1000");

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
/// Whether `exit_code` is an exception code, an `NTSTATUS` of error
/// severity (`0xC0000000` and above), as a process ended by an unhandled
/// exception or a fail-fast exits with.
fn is_exception_code(exit_code: i32) -> bool {
    u32::from_ne_bytes(exit_code.to_ne_bytes()) >= 0xC000_0000
}

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
/// by its own handle; every window an abandoned scenario left
/// ([`AgentClient::close_abandoned_windows`]) is reported and closed by its
/// handle; every window titled with [`DOCUMENT_MARKER`] is closed (a
/// Notepad harness tab as a tab); and the harness documents, files, and
/// folders left in `directory` are deleted.
///
/// # Errors
///
/// Returns an error naming anything that could not be cleaned up.
fn sweep_leftovers(agent: &mut AgentClient, directory: &str) -> io::Result<()> {
    // Tabs first, as tabs: a Notepad ended by its handle keeps them for its
    // next session, without their documents, which are deleted below.
    close_notepad_tabs(agent, DOCUMENT_MARKER)?;
    let ended = agent.end_launched(CLOSE_TIMEOUT)?;
    if ended > 0 {
        println!("ended {ended} process(es) an earlier run left running");
    }
    // The windows an earlier scenario opened and still had open when its
    // test process ended before its cleanup, or when its cleanup found
    // them open, whatever their titles have become. The agent closes only
    // windows absent from that scenario's starting windows, never the
    // owner's own (`scenario_windows` in the agent says why that is safe).
    let (found, remaining) = agent.close_abandoned_windows(CLOSE_TIMEOUT)?;
    for window in &found {
        eprintln!(
            "an earlier scenario ended before its cleanup proved it left no window open; \
             closing the window it left: {}",
            describe_window(window)
        );
    }
    if !remaining.is_empty() {
        return Err(io::Error::other(format!(
            "window(s) an earlier scenario left open did not close within {CLOSE_TIMEOUT:?}: {}",
            describe_windows(&remaining)
        )));
    }
    // What is left with the harness's title has no process or handle the
    // agent knows: the agent ended every process it launched above and
    // closed every window an abandoned scenario it watched left, so these
    // are the windows of a run whose agent has since restarted, which only
    // their titles identify.
    let remaining = agent.close_windows(DOCUMENT_MARKER, CLOSE_TIMEOUT)?;
    if !remaining.is_empty() {
        return Err(io::Error::other(format!(
            "window(s) an earlier run left open, titled with {DOCUMENT_MARKER:?}, did not close within {CLOSE_TIMEOUT:?}: {}",
            describe_windows(&remaining)
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

/// `window`'s title, class, process id, and program, and whether Windows
/// judges it not responding, for a report.
fn describe_window(window: &WindowInfo) -> String {
    let hung = if window.hung { ", not responding" } else { "" };
    format!(
        "{:?} (class {}, pid {}, {}{hung})",
        window.title, window.class, window.pid, window.image
    )
}

/// That `what`, windows asked to close, did not close, naming each of
/// `remaining`, those still shown when the wait ended; `None` when none is.
fn not_closed(what: &str, remaining: &[WindowInfo]) -> Option<String> {
    (!remaining.is_empty()).then(|| {
        format!(
            "{what} did not close within {CLOSE_TIMEOUT:?}: {}",
            describe_windows(remaining)
        )
    })
}

/// Each of `windows` as [`describe_window`] gives it, separated by
/// semicolons.
fn describe_windows(windows: &[WindowInfo]) -> String {
    windows
        .iter()
        .map(describe_window)
        .collect::<Vec<_>>()
        .join("; ")
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
        window: None,
        owners: Vec::new(),
        closing: Closing::NotepadTabs,
        document: Some(path),
        also_exit: Vec::new(),
    };
    let (present, desktop) = agent.wait_for_window(
        WindowCondition::Present {
            title_contains: marker.clone(),
        },
        WINDOW_TIMEOUT,
    )?;
    let Some(window) = desktop
        .windows
        .iter()
        .find(|window| present && window.title.contains(&marker))
    else {
        let evidence = LaunchEvidence::record(agent, run_dir, &desktop, None, launch.pid);
        return Err(WindowWaitFailure::error(
            format!(
                "no window titled {marker:?} opened within {WINDOW_TIMEOUT:?}: {}",
                describe_foreground(&desktop)
            ),
            evidence,
        ));
    };
    let outcome = agent.set_foreground(window.window)?;
    if !outcome.in_front() {
        return Err(io::Error::other(format!(
            "Notepad's window {:?} could not be brought to the foreground: {}",
            window.title,
            not_brought_forward(agent, outcome)
        )));
    }
    let opened = window.window;
    let (met, desktop) = agent.wait_for_window(
        WindowCondition::Foreground {
            title_contains: marker.clone(),
            unsaved: None,
        },
        WINDOW_TIMEOUT,
    )?;
    let Some(window) = desktop.foreground.clone().filter(|_| met) else {
        let evidence = LaunchEvidence::record(agent, run_dir, &desktop, Some(opened), launch.pid);
        return Err(WindowWaitFailure::error(
            format!(
                "the window titled {marker:?} did not take the foreground within {WINDOW_TIMEOUT:?}: {}",
                describe_foreground(&desktop)
            ),
            evidence,
        ));
    };
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
        let outcome = agent.set_foreground(window.window)?;
        if !outcome.in_front() {
            return Err(io::Error::other(format!(
                "Notepad's tab {:?} could not be brought to the foreground to close it: {}",
                window.title,
                not_brought_forward(agent, outcome)
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

/// How compare mode's timeline names launching `command`, its window titled
/// with `title` brought forward.
fn launch_action(command: &str, title: &str) -> String {
    let program = command.rsplit(['\\', '/']).next().unwrap_or(command);
    format!("launch {program}, its window titled {title:?} brought forward")
}

/// The error compare mode gives for what needs Verbatim: there is no
/// Verbatim to `what`.
fn no_verbatim(what: &str) -> io::Error {
    io::Error::other(format!(
        "compare mode runs against NVDA, so there is no Verbatim to {what}"
    ))
}

/// Connects to NVDA's transcript add-on in the agent's session and builds
/// a compare-mode speech collector on it, with its timeline, for compare
/// mode ([`crate::compare`]).
fn connect_nvda(agent: &mut AgentClient) -> io::Result<(SpeechCollector, Timeline, NvdaRun)> {
    let quiet = crate::compare::quiet_period().map_err(io::Error::other)?;
    if let Some(asked) = quiet.raised_from {
        println!(
            "compare mode: the quiet period asked for, {asked:?}, is raised to {:?}, NVDA's multi-press timeout",
            quiet.period
        );
    }
    let session = agent.session_info()?.session_id;
    let port = crate::nvda::session_port(session)?;
    let (transcript, hello) = crate::nvda::Transcript::connect(port).map_err(|error| {
        io::Error::other(format!(
            "compare mode reads NVDA's speech from its transcript add-on (books/dev/src/how-to/nvda.md), \
             so NVDA with the add-on must be running in the agent's session, session {session}: {error}"
        ))
    })?;
    let hello_unix_ms = crate::timeline::unix_ms();
    let version = hello["nvda_version"]
        .as_str()
        .unwrap_or("(unknown version)")
        .to_owned();
    let timeline = Timeline::new();
    let (source, sink) = crate::nvda::speech_source(
        Box::new(transcript),
        hello_unix_ms,
        timeline.clone(),
        quiet.period,
    )?;
    let speech = SpeechCollector::comparing(source, sink, timeline.clone())?;
    println!(
        "compare mode: NVDA {version} in session {session}, quiet period {:?}",
        quiet.period
    );
    Ok((speech, timeline, NvdaRun { version, quiet }))
}

/// Connects to a just-launched, ready Verbatim: the command connection,
/// then a second one subscribed to its speech.
fn connect(agent_addr: &str) -> io::Result<(ControlClient, SpeechCollector, Timeline)> {
    let timeline = Timeline::new();
    let (control, speech) = connect_with(agent_addr, timeline.clone())?;
    Ok((control, speech, timeline))
}

/// [`connect`], its speech pushed to `timeline`.
fn connect_with(
    agent_addr: &str,
    timeline: Timeline,
) -> io::Result<(ControlClient, SpeechCollector)> {
    let control = AgentClient::connect(agent_addr)?.open_control_tunnel()?;
    let speech_tunnel = AgentClient::connect(agent_addr)?.open_control_tunnel()?;
    let speech = SpeechCollector::subscribe(speech_tunnel, timeline)
        .map_err(|error| io::Error::other(format!("could not subscribe to speech: {error}")))?;
    Ok((control, speech))
}

/// Fails unless `control`'s Verbatim says it is ready.
fn require_ready(control: &mut ControlClient) -> io::Result<()> {
    let status = ok_or_error(control.request(Request::Status)?)?;
    if matches!(
        status,
        Frame::Reply {
            payload: ReplyPayload::Status(ref info),
            ..
        } if info.ready
    ) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "Verbatim set its readiness event but its status is not ready: {status:?}"
        )))
    }
}

/// The path of the stderr log of the Verbatim that replaces the one
/// writing to `stderr_path`: the same name with `-replacement` before its
/// extension, so the two do not share one file.
fn replacement_stderr_path(stderr_path: &str) -> String {
    match stderr_path.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-replacement.{extension}"),
        None => format!("{stderr_path}-replacement"),
    }
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

/// Builds `verbatim-app`, `verbatim-outpost` and `verbatim-listener` (debug
/// profile, the default
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
                    "verbatim-listener",
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
                    "cargo build -p verbatim-app -p verbatim-outpost -p verbatim-listener \
                     -p verbatim-synth-host -p mockapp failed ({status}); set \
                     {VERBATIM_EXE_ENV} to stage an existing build instead"
                ))
            }
        })
        .clone()
        .map_err(io::Error::other)
}

/// Copies `verbatim.exe`, `verbatim-outpost.exe`, `verbatim-listener.exe`
/// and the rest of [`STAGED_BINARIES`] from `source_dir` (the
/// directory [`verbatim_exe_path`] resolved — the ordinary build output, or
/// [`VERBATIM_EXE_ENV`]'s override directory) into the fixed staging
/// directory `target/e2e-stage` under the workspace root, and returns that
/// staging directory.
///
/// The outpost and listener binaries must come along, not just
/// `verbatim.exe`: `verbatim_supervisor::Supervisor::new` resolves both next
/// to whatever `verbatim.exe` is actually running as
/// (`std::env::current_exe`), so once the launched copy lives in the staging
/// directory, they must live there too or the supervisor cannot find them.
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

/// The executables a run needs side by side: the app finds the outpost, the
/// listener and the synthesizer host next to itself, and a scenario reading
/// scripted text
/// launches `mockapp` from the run's directory
/// ([`Scenario::run_directory`]).
const STAGED_BINARIES: [&str; 5] = [
    "verbatim.exe",
    "verbatim-outpost.exe",
    "verbatim-listener.exe",
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
            stage_copy(&source, &destination)?;
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
            stage_copy(&entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies `source` to `destination` in the stage, naming both on failure,
/// as when a process still running from the stage holds `destination`.
fn stage_copy(source: &Path, destination: &Path) -> io::Result<()> {
    fs::copy(source, destination).map(|_| ()).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "could not copy {} to {}: {error}",
                source.display(),
                destination.display()
            ),
        )
    })
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

/// The class of the desktop's window, Program Manager: the one window a
/// scenario starts with shown and not minimized.
const DESKTOP_CLASS: &str = "Progman";

/// Why the start of a scenario, every window minimized and the desktop in
/// front ([`AgentClient::minimize_all`]), did not happen: windows left
/// restored, with what became of the taskbar's command, a desktop that did
/// not take the foreground, naming the step that failed and the window
/// that kept it, or, once both held, a shown window other than the desktop
/// that is not minimized and can be activated, naming it. That last is a
/// window Show Desktop does not minimize, such as a message box or another
/// owned window, which takes the foreground whenever the window in front
/// closes. A window with `WS_EX_NOACTIVATE` never does, so it passes, as
/// Windows 11's Widgets board's window does when Windows shows it on its
/// own. `None` when every check held.
fn minimize_failure(
    taskbar: &TaskbarCommand,
    outcome: MinimizeOutcome,
    desktop: &ForegroundInfo,
) -> Option<String> {
    if let MinimizeOutcome::NotMinimized { .. } = outcome {
        return Some(format!(
            "{}; {}: {}",
            outcome.describe(),
            taskbar.describe(),
            describe_foreground(desktop)
        ));
    }
    if outcome != MinimizeOutcome::DesktopInFront {
        let holder = desktop.foreground.as_ref().map_or_else(
            || "no window".to_owned(),
            |window| {
                format!(
                    "{:?} of {} (pid {})",
                    window.title, window.image, window.pid
                )
            },
        );
        return Some(format!(
            "{}; {holder} kept it: {}",
            outcome.describe(),
            describe_foreground(desktop)
        ));
    }
    let restored: Vec<String> = desktop
        .windows
        .iter()
        .filter(|window| {
            !window.cloaked
                && !window.minimized
                && !window.no_activate
                && window.class != DESKTOP_CLASS
        })
        .map(|window| {
            format!(
                "{:?} of {} (pid {}, class {})",
                window.title, window.image, window.pid, window.class
            )
        })
        .collect();
    if !restored.is_empty() {
        return Some(format!(
            "a window other than the desktop is still shown and not minimized, so it would take \
             the foreground when a window in front closes: {}",
            restored.join(", ")
        ));
    }
    None
}

/// The processes among `windows` that are a suspended Settings app: a
/// `SystemSettings.exe` whose every window listed is cloaked. Windows
/// starts the app ahead of use and keeps it suspended with its window
/// cloaked, and keeps a Settings a scenario opened so once its window
/// closes, so it is Windows' prelaunch or a Settings left suspended by this
/// or an earlier scenario, and no window a scenario left
/// (`books/dev/src/how-to/e2e.md`, on `foreground.txt`). Which it is cannot be
/// told: a scenario
/// opens Settings through `explorer.exe`, and the window it waits for is
/// the frame host's, so no `SystemSettings.exe` is among the processes it
/// records as started.
fn settings_prelaunch(windows: &[WindowInfo]) -> Vec<u32> {
    let mut pids: Vec<u32> = windows
        .iter()
        .filter(|window| window.image.eq_ignore_ascii_case("SystemSettings.exe"))
        .map(|window| window.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();
    pids.retain(|&pid| {
        windows
            .iter()
            .filter(|window| window.pid == pid)
            .all(|window| window.cloaked)
    });
    pids
}

/// What the harness records when a window a launch was to open did not
/// open, or did not take the foreground, in time: the wait chains
/// ([`window_wait_chains`]) and the launched process's processor time,
/// modules and minidump (`Request::ProcessDiagnostics`), each part that
/// could not be read saying why.
#[derive(Clone, Debug)]
pub(crate) struct LaunchEvidence {
    /// The launched process.
    pid: u32,
    /// The wait chains, one a line.
    chains: Vec<String>,
    /// The launched process's state, processor time and modules, one fact
    /// a line ([`describe_process`]), for `launched-process-<pid>.txt`.
    process: Vec<String>,
    /// One line summing the process up, for the failure's message.
    summary: String,
    /// The minidump's bytes, for `launched-process-<pid>.dmp`, when it was
    /// written and read.
    dump: Option<Vec<u8>>,
}

impl LaunchEvidence {
    /// Records the evidence of the launch of `pid`, whose window, `window`
    /// when it opened, did not open or take the foreground in time, with
    /// `desktop` as the wait ended. The minidump is written by the agent
    /// into `run_dir`, its folder for the harness's files, read back, and
    /// deleted there.
    fn record(
        agent: &mut AgentClient,
        run_dir: &str,
        desktop: &ForegroundInfo,
        window: Option<u64>,
        pid: u32,
    ) -> Self {
        let chains = window_wait_chains(agent, desktop, window, pid);
        let dump_path = format!(r"{run_dir}\{DOCUMENT_MARKER}launched-{pid}.dmp");
        let (mut process, summary, written) = match agent.process_diagnostics(pid, &dump_path) {
            Ok(read) => {
                let (process, summary) = describe_process(pid, &read);
                (process, summary, read.dump_error.is_none())
            }
            Err(error) => {
                let why = format!("the launched process {pid} could not be read: {error}");
                (vec![why.clone()], why, false)
            }
        };
        let dump = if written {
            match agent.read_whole_file(&dump_path) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    process.push(format!("the minidump could not be read back: {error}"));
                    None
                }
            }
        } else {
            None
        };
        if let Err(error) = agent.delete_file(&dump_path) {
            process.push(format!(
                "the minidump {dump_path} could not be deleted on the agent's machine: {error}"
            ));
        }
        Self {
            pid,
            chains,
            process,
            summary,
            dump,
        }
    }
}

/// A launch whose window did not open or take the foreground in time, with
/// what was recorded of it ([`LaunchEvidence`]): the error such a launch
/// returns, so the runner can write the evidence to the scenario's
/// artifacts ([`launch_evidence_of`]) even when no [`Scenario`] was made.
#[derive(Debug)]
struct WindowWaitFailure {
    message: String,
    evidence: LaunchEvidence,
}

impl WindowWaitFailure {
    /// `message` with `evidence` summed up, as an I/O error.
    fn error(message: String, evidence: LaunchEvidence) -> io::Error {
        io::Error::other(Self { message, evidence })
    }
}

impl std::fmt::Display for WindowWaitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pid = self.evidence.pid;
        let dump = if self.evidence.dump.is_some() {
            format!(" and {LAUNCHED_PROCESS_FILE_STEM}-{pid}.dmp")
        } else {
            String::new()
        };
        write!(
            formatter,
            "{}\n{}; see {WAIT_CHAINS_FILE_NAME}, {LAUNCHED_PROCESS_FILE_STEM}-{pid}.txt{dump} in the scenario's artifacts",
            self.message, self.evidence.summary,
        )?;
        if !self.evidence.chains.is_empty() {
            write!(
                formatter,
                "\nwait chains:\n{}",
                self.evidence.chains.join("\n")
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for WindowWaitFailure {}

/// `error`, a failed start's, with `cleanup`'s failure appended when the
/// cleanup step `step` failed too, so the cause survives and the failed
/// cleanup is still reported; what the error carries for the scenario's
/// artifacts ([`WindowWaitFailure`], [`StartFailure`]) is kept.
fn with_cleanup(mut error: io::Error, step: &str, cleanup: io::Result<()>) -> io::Error {
    let Err(failure) = cleanup else {
        return error;
    };
    let note = format!("\nthen {step} failed too: {failure}");
    if let Some(inner) = error.get_mut() {
        if let Some(wait) = inner.downcast_mut::<WindowWaitFailure>() {
            wait.message.push_str(&note);
            return error;
        }
        if let Some(start) = inner.downcast_mut::<StartFailure>() {
            start.message.push_str(&note);
            return error;
        }
    }
    io::Error::new(error.kind(), format!("{error}{note}"))
}

/// A start that failed once Verbatim was launched, with Verbatim's stderr
/// log as read after it was ended: the error such a start returns, so the
/// runner can write the log to the scenario's artifacts
/// ([`write_start_failure`]) though no [`Scenario`] was made to collect
/// it.
#[derive(Debug)]
struct StartFailure {
    message: String,
    stderr_log: Option<Vec<u8>>,
}

impl StartFailure {
    /// `error`'s message with `stderr_log`, as an I/O error of its kind.
    fn error(error: &io::Error, stderr_log: Option<Vec<u8>>) -> io::Error {
        io::Error::new(
            error.kind(),
            Self {
                message: error.to_string(),
                stderr_log,
            },
        )
    }
}

impl std::fmt::Display for StartFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for StartFailure {}

/// Writes what a failed start's `error` carries to the scenario's
/// artifacts in `dir`: Verbatim's stderr log, as [`STDERR_FILE_NAME`],
/// when the start failed once Verbatim was launched ([`StartFailure`]).
/// Writes nothing otherwise.
pub(crate) fn write_start_failure(dir: &Path, error: &io::Error) -> io::Result<()> {
    let Some(log) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<StartFailure>())
        .and_then(|failure| failure.stderr_log.as_deref())
    else {
        return Ok(());
    };
    fs::create_dir_all(dir)?;
    fs::write(dir.join(STDERR_FILE_NAME), log)
}

/// What was recorded of the launch `error` failed on, when a window wait
/// ran out ([`LaunchEvidence`]).
pub(crate) fn launch_evidence_of(error: &io::Error) -> Option<&LaunchEvidence> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<WindowWaitFailure>())
        .map(|failure| &failure.evidence)
}

/// Writes what was recorded of each launch in `evidence` to `dir`: every
/// wait chain to [`WAIT_CHAINS_FILE_NAME`], and each launched process's
/// facts and minidump to files named for it
/// ([`LAUNCHED_PROCESS_FILE_STEM`]). Writes nothing when there is none.
pub(crate) fn write_launch_evidence(dir: &Path, evidence: &[LaunchEvidence]) -> io::Result<()> {
    if evidence.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(dir)?;
    let chains: Vec<&str> = evidence
        .iter()
        .flat_map(|launch| launch.chains.iter().map(String::as_str))
        .collect();
    fs::write(dir.join(WAIT_CHAINS_FILE_NAME), chains.join("\n") + "\n")?;
    for launch in evidence {
        let stem = format!("{LAUNCHED_PROCESS_FILE_STEM}-{}", launch.pid);
        fs::write(
            dir.join(format!("{stem}.txt")),
            launch.process.join("\n") + "\n",
        )?;
        if let Some(dump) = &launch.dump {
            fs::write(dir.join(format!("{stem}.dmp")), dump)?;
        }
    }
    Ok(())
}

/// The facts of launched process `pid` as the agent read them (`read`),
/// one a line: whether it runs, its processor time, which
/// `System.Windows.Forms` it loaded (`.ni.dll`, the precompiled native
/// image, or `.dll`, compiled as it loads), each module, and the
/// minidump; and one line summing up the first three.
fn describe_process(pid: u32, read: &ProcessDiagnostics) -> (Vec<String>, String) {
    let state = match read.state {
        ProcessState::Running => "running".to_owned(),
        ProcessState::Exited {
            exit_code: Some(code),
        } => format!("exited with code {code}"),
        ProcessState::Exited { exit_code: None } => "exited".to_owned(),
    };
    let time = match (read.user_ms, read.kernel_ms) {
        (Some(user), Some(kernel)) => {
            format!("processor time {user} ms in user mode and {kernel} ms in kernel mode")
        }
        _ => "processor time could not be read".to_owned(),
    };
    let forms = if read.modules_error.is_some() {
        "System.Windows.Forms unknown, since the modules could not be read".to_owned()
    } else {
        read.modules
            .iter()
            .find_map(|module| {
                let name = Path::new(module).file_name()?.to_str()?;
                if name.eq_ignore_ascii_case("System.Windows.Forms.ni.dll") {
                    Some(format!(
                        "System.Windows.Forms loaded as the precompiled native image {module}"
                    ))
                } else if name.eq_ignore_ascii_case("System.Windows.Forms.dll") {
                    Some(format!(
                        "System.Windows.Forms loaded as the plain assembly {module}"
                    ))
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "System.Windows.Forms not loaded".to_owned())
    };
    let mut lines = vec![
        format!("process {pid}: {state}"),
        time.clone(),
        forms.clone(),
    ];
    if let Some(error) = &read.modules_error {
        lines.push(format!("modules: {error}"));
    } else {
        lines.push(format!("modules, {} in load order:", read.modules.len()));
        lines.extend(read.modules.iter().cloned());
    }
    lines.push(match &read.dump_error {
        None => format!("minidump: {LAUNCHED_PROCESS_FILE_STEM}-{pid}.dmp"),
        Some(error) => format!("minidump: {error}"),
    });
    let summary = format!("launched process {pid}: {state}, {time}, {forms}");
    (lines, summary)
}

/// When a window wait ran out with `desktop` as it ended: the wait chain of
/// the thread owning `window`, the launched window when it opened but did
/// not take the foreground, then of the thread owning each window `desktop`
/// reports as not responding, and of the threads of `pid`, the process
/// launched to open the window (`Request::WaitChains`), each on one line
/// ([`describe_wait_chain`]); one line saying why when they could not be
/// read.
fn window_wait_chains(
    agent: &mut AgentClient,
    desktop: &ForegroundInfo,
    window: Option<u64>,
    pid: u32,
) -> Vec<String> {
    let mut windows: Vec<u64> = window.into_iter().collect();
    for window in desktop.foreground.iter().chain(&desktop.windows) {
        if window.hung && !windows.contains(&window.window) {
            windows.push(window.window);
        }
    }
    match agent.wait_chains(windows, vec![pid]) {
        Ok(chains) => chains.iter().map(describe_wait_chain).collect(),
        Err(error) => vec![format!("the wait chains could not be read: {error}")],
    }
}

/// `chain` as one line: whose thread it is, then each node in order, then
/// whether it is a cycle; or why it could not be read.
fn describe_wait_chain(chain: &WaitChain) -> String {
    let subject = match (chain.window, chain.thread) {
        (Some(window), 0) => format!("window {window:#x} (pid {})", chain.pid),
        (Some(window), thread) => {
            format!("window {window:#x} (pid {}), thread {thread}", chain.pid)
        }
        (None, 0) => format!("pid {}", chain.pid),
        (None, thread) => format!("pid {}, thread {thread}", chain.pid),
    };
    if let Some(error) = &chain.error {
        return format!("{subject}: could not be read: {error}");
    }
    let nodes: Vec<String> = chain.nodes.iter().map(describe_wait_node).collect();
    let cycle = if chain.cycle {
        "a cycle, so a deadlock"
    } else {
        "no cycle"
    };
    format!("{subject}: {}; {cycle}", nodes.join("; "))
}

/// One node of a wait chain: its kind, the thread for a thread node, and
/// its state.
fn describe_wait_node(node: &WaitNode) -> String {
    let kind = match (node.kind, &node.thread) {
        (WaitNodeKind::Thread, Some(thread)) if thread.image.is_empty() => {
            format!("thread {} of pid {}", thread.thread, thread.pid)
        }
        (WaitNodeKind::Thread, Some(thread)) => format!(
            "thread {} of {} pid {}",
            thread.thread, thread.image, thread.pid
        ),
        (WaitNodeKind::Thread, None) => "thread".to_owned(),
        (WaitNodeKind::CriticalSection, _) => "critical section".to_owned(),
        (WaitNodeKind::SendMessage, _) => "send message".to_owned(),
        (WaitNodeKind::Mutex, _) => "mutex".to_owned(),
        (WaitNodeKind::Alpc, _) => "ALPC".to_owned(),
        (WaitNodeKind::Com, _) => "COM".to_owned(),
        (WaitNodeKind::ComActivation, _) => "COM activation".to_owned(),
        (WaitNodeKind::ThreadWait, _) => "thread wait".to_owned(),
        (WaitNodeKind::ProcessWait, _) => "process wait".to_owned(),
        (WaitNodeKind::SocketIo, _) => "socket".to_owned(),
        (WaitNodeKind::SmbIo, _) => "SMB".to_owned(),
        (WaitNodeKind::Unknown, _) => "unknown".to_owned(),
        (WaitNodeKind::Other(number), _) => format!("type {number}"),
    };
    let status = match node.status {
        WaitNodeStatus::NoAccess => "no access".to_owned(),
        WaitNodeStatus::Running => "running".to_owned(),
        WaitNodeStatus::Blocked => "blocked".to_owned(),
        WaitNodeStatus::PidOnly => "owner's process only".to_owned(),
        WaitNodeStatus::PidOnlyRpcss => "owner's process only, the RPC subsystem".to_owned(),
        WaitNodeStatus::Owned => "owned".to_owned(),
        WaitNodeStatus::NotOwned => "not owned".to_owned(),
        WaitNodeStatus::Abandoned => "abandoned".to_owned(),
        WaitNodeStatus::Unknown => "unknown".to_owned(),
        WaitNodeStatus::Error => "error".to_owned(),
        WaitNodeStatus::Other(number) => format!("status {number}"),
    };
    format!("{kind} ({status})")
}

/// Why a window was not brought forward, from `outcome`, the step that
/// failed, and the desktop as it stands after the attempt, read from
/// `agent`, rather than as it stood before it.
fn not_brought_forward(agent: &mut AgentClient, outcome: ForegroundOutcome) -> String {
    let after = match agent.foreground_info() {
        Ok(desktop) => describe_foreground(&desktop),
        Err(error) => format!("the desktop could not be read: {error}"),
    };
    format!("{}; after the attempt, {after}", outcome.describe())
}

/// `info` as one line: the foreground window, then the visible windows.
fn describe_foreground(info: &ForegroundInfo) -> String {
    let describe = |window: &WindowInfo| {
        format!(
            "{:?} ({} pid {}, class {}{}{}{}{}{})",
            window.title,
            window.image,
            window.pid,
            window.class,
            if window.hidden { ", hidden" } else { "" },
            if window.cloaked { ", cloaked" } else { "" },
            if window.minimized { ", minimized" } else { "" },
            if window.hung { ", not responding" } else { "" },
            if window.no_activate {
                ", no-activate"
            } else {
                ""
            }
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
/// a new name has none. A window titled with it is found by that title,
/// and closed by that title by the next launch's sweep when nothing else
/// identifies it ([`sweep_leftovers`]).
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

/// How the log of a spare outpost is named, `outpost-spare-<launch>.log`:
/// its application is not known when the log is opened, so its assignment
/// line names it ([`log_assigns`]).
const SPARE_LOG_PREFIX: &str = "outpost-spare-";

/// Whether `log`, an outpost's log, has the line an outpost writes when it
/// starts watching process `pid`: "watching its application", with
/// `target_pid=<pid>`, read with its colours' escape sequences removed.
fn log_assigns(log: &str, pid: u32) -> bool {
    let wanted = format!("target_pid={pid}");
    log.lines().any(|line| {
        let plain = strip_ansi(line);
        plain.contains("watching its application")
            && plain.split_whitespace().any(|field| field == wanted)
    })
}

/// `line` without its terminal colour escape sequences (`ESC [ ... m`).
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

/// Whether `name` is the log of an outpost started for process `pid`, as
/// the supervisor names it: `outpost-<image>-<pid>.log`, or
/// `outpost-<pid>.log` when the image could not be read.
fn outpost_log_names(name: &str, pid: u32) -> bool {
    if name.starts_with(SPARE_LOG_PREFIX) {
        return false;
    }
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
mod minimize_tests {
    use super::*;
    use verbatim_agent::protocol::WaitThread;

    fn window(title: &str, image: &str, pid: u32) -> WindowInfo {
        WindowInfo {
            window: 1,
            pid,
            title: title.to_owned(),
            class: "Class".to_owned(),
            image: image.to_owned(),
            cloaked: false,
            minimized: true,
            hung: false,
            no_activate: false,
            hidden: false,
        }
    }

    /// Only a `SystemSettings.exe` with every window cloaked is taken for a
    /// suspended Settings app; the frame host's cloaked frame is not.
    #[test]
    fn only_a_settings_app_with_every_window_cloaked_is_taken_as_suspended() {
        let cloaked = |pid| WindowInfo {
            cloaked: true,
            ..window("Settings", "SystemSettings.exe", pid)
        };
        let windows = [
            cloaked(10),
            cloaked(11),
            cloaked(12),
            window("Settings", "SystemSettings.exe", 12),
            window("Settings", "SystemSettings.exe", 13),
            WindowInfo {
                cloaked: true,
                ..window("Settings", "ApplicationFrameHost.exe", 14)
            },
        ];
        assert_eq!(settings_prelaunch(&windows), [10, 11]);
    }

    #[test]
    fn a_desktop_that_did_not_take_the_foreground_names_the_window_that_kept_it() {
        let desktop = ForegroundInfo {
            foreground: Some(window("phase 6", "WindowsTerminal.exe", 29864)),
            windows: Vec::new(),
        };
        let refused = MinimizeOutcome::Refused { limit_ms: 15_000 };
        let failure =
            minimize_failure(&TaskbarCommand::Handled, refused, &desktop).expect("a failure");
        assert_eq!(
            failure,
            "every window was minimized, but SetForegroundWindow refused the desktop's window, and the desktop did not take the foreground within 15000 ms; \"phase 6\" of WindowsTerminal.exe (pid 29864) kept it: foreground window \"phase 6\" (WindowsTerminal.exe pid 29864, class Class, minimized); visible windows: "
        );
        assert_eq!(
            minimize_failure(
                &TaskbarCommand::Handled,
                MinimizeOutcome::NotMinimized { limit_ms: 15_000 },
                &desktop
            )
            .expect("a failure"),
            "not every window was minimized within 15000 ms; the taskbar handled its Minimize All command: foreground window \"phase 6\" (WindowsTerminal.exe pid 29864, class Class, minimized); visible windows: "
        );
        assert_eq!(
            minimize_failure(
                &TaskbarCommand::Handled,
                MinimizeOutcome::DesktopInFront,
                &desktop
            ),
            None
        );
    }

    /// A console window the console host has made but not yet shown, its
    /// thread waiting on the Desktop Window Manager, can hold the foreground
    /// while no shown window has its title; the report says it is hidden
    /// and not responding, so it does not read as a window that opened.
    #[test]
    fn a_hidden_foreground_window_not_responding_is_described_so() {
        let mut console = window("verbatim-e2e-console-flood", "powershell.exe", 14224);
        console.class = "ConsoleWindowClass".to_owned();
        console.minimized = false;
        console.hidden = true;
        console.hung = true;
        let desktop = ForegroundInfo {
            foreground: Some(console),
            windows: vec![window("Untitled - Notepad", "Notepad.exe", 11016)],
        };
        assert_eq!(
            describe_foreground(&desktop),
            "foreground window \"verbatim-e2e-console-flood\" (powershell.exe pid 14224, class ConsoleWindowClass, hidden, not responding); visible windows: \"Untitled - Notepad\" (Notepad.exe pid 11016, class Class, minimized)"
        );
    }

    /// A window's wait chain is one line naming the window and its thread,
    /// then each node with its state, a thread with its process, and
    /// whether it loops; a process's thread whose chain could not be read
    /// says why.
    #[test]
    fn wait_chains_are_described_node_by_node() {
        let thread = |pid: u32, thread: u32, image: &str| WaitNode {
            kind: WaitNodeKind::Thread,
            status: WaitNodeStatus::Blocked,
            thread: Some(WaitThread {
                pid,
                thread,
                image: image.to_owned(),
            }),
        };
        let object = |kind: WaitNodeKind, status: WaitNodeStatus| WaitNode {
            kind,
            status,
            thread: None,
        };
        let hung = WaitChain {
            window: Some(0x0004_0a2e),
            pid: 14224,
            thread: 5016,
            nodes: vec![
                thread(9120, 5016, "conhost.exe"),
                object(WaitNodeKind::SendMessage, WaitNodeStatus::Blocked),
                thread(4410, 812, "explorer.exe"),
                object(WaitNodeKind::Alpc, WaitNodeStatus::PidOnly),
                thread(6000, 77, ""),
            ],
            cycle: true,
            error: None,
        };
        let denied = WaitChain {
            window: None,
            pid: 14224,
            thread: 7000,
            nodes: Vec::new(),
            cycle: false,
            error: Some("Access is denied. (0x80070005)".to_owned()),
        };
        assert_eq!(
            describe_wait_chain(&hung),
            "window 0x40a2e (pid 14224), thread 5016: thread 5016 of conhost.exe pid 9120 (blocked); send message (blocked); thread 812 of explorer.exe pid 4410 (blocked); ALPC (owner's process only); thread 77 of pid 6000 (blocked); a cycle, so a deadlock"
        );
        assert_eq!(
            describe_wait_chain(&denied),
            "pid 14224, thread 7000: could not be read: Access is denied. (0x80070005)"
        );
    }

    /// A launched process is described by its state, its processor time,
    /// which `System.Windows.Forms` it loaded, its modules and its
    /// minidump, and summed up by the first three.
    #[test]
    fn a_launched_process_is_described_with_its_windows_forms_image() {
        let forms = r"C:\Windows\assembly\NativeImages_v4.0.30319_64\System.Windows.Forms\0f8d\System.Windows.Forms.ni.dll";
        let read = ProcessDiagnostics {
            state: ProcessState::Running,
            user_ms: Some(31_250),
            kernel_ms: Some(1_500),
            modules: vec![
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_owned(),
                forms.to_owned(),
            ],
            modules_error: None,
            dump_error: None,
        };
        let (lines, summary) = describe_process(14224, &read);
        let time = "processor time 31250 ms in user mode and 1500 ms in kernel mode";
        let loaded = format!("System.Windows.Forms loaded as the precompiled native image {forms}");
        assert_eq!(
            lines,
            vec![
                "process 14224: running".to_owned(),
                time.to_owned(),
                loaded.clone(),
                "modules, 2 in load order:".to_owned(),
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".to_owned(),
                forms.to_owned(),
                "minidump: launched-process-14224.dmp".to_owned(),
            ]
        );
        assert_eq!(
            summary,
            format!("launched process 14224: running, {time}, {loaded}")
        );
    }

    /// A message box Show Desktop cannot minimize, as the paging-file one
    /// GitHub's runner image shows at logon, fails the start, named; the
    /// desktop, windows that are minimized or cloaked, and a window Windows
    /// never activates, as the Widgets board's, do not.
    #[test]
    fn a_shown_window_left_restored_besides_the_desktop_fails_the_start_naming_it() {
        let mut desktop_window = window("Program Manager", "explorer.exe", 7644);
        desktop_window.class = DESKTOP_CLASS.to_owned();
        desktop_window.minimized = false;
        let mut cloaked = window("Settings", "ApplicationFrameHost.exe", 4100);
        cloaked.minimized = false;
        cloaked.cloaked = true;
        let mut message_box = window("System Properties", "SystemPropertiesPerformance.exe", 7692);
        message_box.class = "#32770".to_owned();
        message_box.minimized = false;
        let terminal = window("verbatim-agent.exe", "WindowsTerminal.exe", 5564);
        let mut widgets = window("Widgets", "msedgewebview2.exe", 13720);
        widgets.class = "Chrome_WidgetWin_1".to_owned();
        widgets.minimized = false;
        widgets.no_activate = true;
        let mut desktop = ForegroundInfo {
            foreground: Some(desktop_window.clone()),
            windows: vec![terminal, cloaked, widgets, desktop_window, message_box],
        };
        assert_eq!(
            minimize_failure(
                &TaskbarCommand::Handled,
                MinimizeOutcome::DesktopInFront,
                &desktop
            ),
            Some(
                "a window other than the desktop is still shown and not minimized, so it would \
                 take the foreground when a window in front closes: \"System Properties\" of \
                 SystemPropertiesPerformance.exe (pid 7692, class #32770)"
                    .to_owned()
            )
        );
        desktop.windows.pop();
        assert_eq!(
            minimize_failure(
                &TaskbarCommand::Handled,
                MinimizeOutcome::DesktopInFront,
                &desktop
            ),
            None
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cleanup that fails after a start failed is appended to the
    /// start's error, which keeps its cause and the stderr log it carries
    /// for the scenario's artifacts.
    #[test]
    fn a_failed_cleanup_is_appended_to_the_start_failure_it_followed() {
        let start = StartFailure::error(
            &io::Error::other("Verbatim did not say it was ready"),
            Some(b"log".to_vec()),
        );
        let error = with_cleanup(
            start,
            "closing the harness documents",
            Err(io::Error::other("a tab did not close")),
        );
        assert_eq!(
            error.to_string(),
            "Verbatim did not say it was ready\nthen closing the harness documents failed too: a tab did not close"
        );
        let dir =
            std::env::temp_dir().join(format!("verbatim-e2e-start-failure-{}", std::process::id()));
        write_start_failure(&dir, &error).expect("writes");
        assert_eq!(fs::read(dir.join(STDERR_FILE_NAME)).expect("reads"), b"log");
        fs::remove_dir_all(&dir).expect("removes");
    }

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
        assert!(!outpost_log_names("outpost-spare-7.log", 7));
    }

    #[test]
    fn a_spare_outposts_log_is_told_by_its_assignment_line() {
        let log = "\u{1b}[2m2026-10-16T07:01:28.355579Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \u{1b}[2mverbatim_outpost::outpost\u{1b}[0m\u{1b}[2m:\u{1b}[0m watching its application \u{1b}[3mtarget_pid\u{1b}[0m\u{1b}[2m=\u{1b}[0m29864 \u{1b}[3mimage\u{1b}[0m\u{1b}[2m=\u{1b}[0m\"windowsterminal\"\n";
        assert!(log_assigns(log, 29864));
        assert!(!log_assigns(log, 2986));
        assert!(!log_assigns(log, 129_864));
        assert!(!log_assigns(
            "a spare, waiting for its application\n",
            29864
        ));
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
        fs::write(source_dir.join("verbatim-listener.exe"), b"listener v1")
            .expect("seed verbatim-listener.exe");
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
            fs::read(stage_dir.join("verbatim-listener.exe")).expect("read staged listener"),
            b"listener v1"
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
