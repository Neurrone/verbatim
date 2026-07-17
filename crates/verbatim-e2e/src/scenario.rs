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

use verbatim_agent::protocol::{KillOutcome, ProcessState};
use verbatim_config::{ConfigStore, Settings};
use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{Frame, LatencyRecord, ReplyPayload, Request};

use crate::agent_client::AgentClient;
use crate::speech::SpeechCollector;
use crate::timeline::Timeline;
use crate::{ENDPOINT_ENV, endpoint};

/// File names the artifact collectors write under, inside the directory
/// [`crate::artifacts::scenario_dir`] names. The timeline and stderr log are
/// written for every run by [`Scenario::collect_run_artifacts`] (so a passing
/// diagnostic run leaves its announcement timings and outpost-ready timestamps
/// behind, not only a failing one); the flight-recorder dump is the
/// failure-only extra [`Scenario::collect_failure_artifacts`] adds, since it
/// needs Verbatim still up to answer `DumpRecorder`.
const TIMELINE_FILE_NAME: &str = "timeline.txt";
const STDERR_FILE_NAME: &str = "stderr.log";
const FLIGHT_RECORDER_FILE_NAME: &str = "flight-recorder.jsonl";

/// Environment variable overriding the path to `verbatim.exe`. Defaults to
/// `target/debug/verbatim.exe` under the workspace root — the ordinary
/// local debug build, and what the CI job in `.github/workflows/ci.yml`
/// builds before running this suite.
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
/// inside the guest. Set this and the whole staging step is skipped, since
/// the deploy owns it.
pub const REMOTE_ENV: &str = "VERBATIM_E2E_REMOTE";

/// Whether this is a remote (in-guest) run; see [`REMOTE_ENV`].
fn is_remote() -> bool {
    std::env::var(REMOTE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Environment variable requesting an *audible* run: [`Scenario::launch`]
/// selects the real `OneCore` synthesizer instead of the capture synth, and
/// does not set `VERBATIM_TEST_AUDIO=null`, so Verbatim speaks through the
/// real `WasapiSink` on real hardware instead of the silent, voice-free
/// path every other run uses. `cargo xtask vm test --audible` sets it for a
/// remote run; set it by hand for a runner-direct one.
///
/// Intended for human debugging only — for actually listening to a
/// scenario play out. Every speech assertion in this suite, including the
/// M1 exit regression's voice-combo section, is synth-agnostic: it captures
/// whichever voice name the active synthesizer speaks first at runtime
/// instead of asserting a literal, so those assertions pass under real
/// `OneCore` voices exactly as they do under the capture synth (confirmed
/// live). The M1 exit regression's own trailing latency check does not yet
/// pass under audible mode, though — see that test's module doc — a
/// separate, unresolved gap in the real audio path, not a speech-assertion
/// problem. This is not the routine acceptance check only because it needs
/// a listener and real audio hardware; a plain `cargo xtask vm test`
/// remains that. In runner-direct mode, real speech also means Verbatim
/// will speak over any other screen reader already running on the desktop.
pub const AUDIBLE_ENV: &str = "VERBATIM_E2E_AUDIBLE";

/// Whether this is an audible run; see [`AUDIBLE_ENV`].
#[must_use]
pub fn is_audible() -> bool {
    std::env::var(AUDIBLE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Environment variable that switches a scenario into paced mode: every
/// speech assertion additionally waits for the matched utterance's audio to
/// finish before the next input, so a human watching or a recording hears
/// each utterance in full. Set by `cargo xtask vm test --paced` (and implied
/// by `--record`, since a recording nobody can follow is pointless). Purely a
/// presentation aid — it never changes what is asserted, only the timing —
/// so ordinary fast runs leave it unset.
pub const PACED_ENV: &str = "VERBATIM_E2E_PACED";

/// Whether this is a paced run; see [`PACED_ENV`].
#[must_use]
pub fn is_paced() -> bool {
    std::env::var(PACED_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// How long [`Scenario::launch`] waits for Verbatim's control plane to come
/// up before giving up.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Interval between control-tunnel readiness polls.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// How long [`Scenario::quit_verbatim`] waits for the process to actually
/// exit after `Quit` is acknowledged (or the connection closed in its
/// place).
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Extra settle time [`Scenario::launch`] waits after the control plane
/// answers, before returning.
///
/// The control server starts (and so the tunnel answers) before
/// `verbatim-app`'s `run` finishes wiring the keyboard hook, speaks the
/// startup announcement, and hands `run_gui`'s `on_ready` callback its
/// `GuiHandle` — the handle the gesture router needs to act on anything.
/// A `SendGesture` that arrives before that handle exists is not queued;
/// `verbatim-app`'s router logs and silently drops it (confirmed live: the
/// control plane still answers `Ok`, since routing is fire-and-forget, but
/// nothing happens). There is no control-plane signal this crate can poll
/// for "the GUI is ready" without changing `verbatim-app` (outside this
/// crate's scope), so this fixed pause is the pragmatic mitigation: wx's
/// remaining setup at that point is a handful of cheap window creations,
/// not I/O, so it is comfortably done well within this window even on a
/// slow CI runner.
const GUI_SETTLE_DELAY: Duration = Duration::from_secs(1);

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
    /// Extra processes launched via [`Scenario::launch_target`]: pid paired
    /// with the image (executable file) name recorded at launch time, since
    /// pid alone is not reliable cleanup for every target application (see
    /// that method's doc comment) — killed on drop unless already removed
    /// by [`Scenario::kill_target`].
    launched: Vec<(u32, String)>,
    /// The path this launch's Verbatim has its stdout and stderr captured
    /// into (see [`verbatim_stderr_log_path`]), readable back through
    /// [`process_agent`](Self::process_agent)'s `read_file` — what
    /// [`Scenario::collect_failure_artifacts`] pulls on a scenario failure.
    stderr_log_path: String,
}

impl Scenario {
    /// Launches a fresh Verbatim through the agent named by
    /// [`crate::ENDPOINT_ENV`].
    ///
    /// In runner-direct mode (the default — see [`REMOTE_ENV`]), first
    /// stages `verbatim.exe` and `verbatim-outpost.exe` into
    /// `target/e2e-stage` under the workspace root (see [`stage_binaries`]),
    /// then writes [`verbatim_config::Settings::for_e2e`]'s fixed
    /// settings.toml there selecting the capture synthesizer (audio-free
    /// and dependency-free — it needs no installed voices, unlike
    /// `OneCore`), and launches *that* staged copy with
    /// `VERBATIM_TEST_AUDIO=null` (device-free `NullSink`, still emitting
    /// complete latency timelines). The developer's own
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
    /// Under [`AUDIBLE_ENV`] both choices flip: the settings selects
    /// `OneCore` and `VERBATIM_TEST_AUDIO=null` is not passed, so Verbatim
    /// speaks for real. See that constant's doc comment for why that is a
    /// debugging aid, not an acceptance mode.
    ///
    /// # Errors
    ///
    /// Returns an error if [`crate::ENDPOINT_ENV`] is unset, the source
    /// `verbatim.exe` (or, in runner-direct mode, `verbatim-outpost.exe`
    /// next to it) cannot be found, staging fails, the agent cannot be
    /// reached, or Verbatim's control plane never comes up within the
    /// launch timeout.
    pub fn launch() -> io::Result<Self> {
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let lock = live_instance_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let verbatim_exe = verbatim_exe_path();
        let remote = is_remote();
        let audible = is_audible();
        if !remote && !verbatim_exe.is_file() {
            return Err(io::Error::other(format!(
                "verbatim.exe not found at {} (set {VERBATIM_EXE_ENV} to override, or {REMOTE_ENV} if it lives in a guest)",
                verbatim_exe.display()
            )));
        }

        // In a remote run the path above names a location in the guest, so
        // neither staging nor the config write can happen here; `cargo
        // xtask vm deploy` staged both inside the guest already (selecting
        // the same synth, per `--audible`). In runner-direct mode, stage a
        // private copy so this suite never reads or writes the developer's
        // own build output directory.
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
            let synth_id = if audible { "onecore" } else { "capture" };
            configure_synth(&stage_dir, synth_id)?;
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
        // verbatim-app's own startup check leaves the real WasapiSink in
        // place instead of swapping in NullSink; see AUDIBLE_ENV.
        let launch_env: &[(String, String)] = if audible {
            &[]
        } else {
            &[("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned())]
        };

        let mut process_agent = AgentClient::connect(&agent_addr)?;
        // Sweep known target-application image names before doing anything
        // else, so this scenario starts from as clean a state as possible
        // even after a prior run aborted without running its own Drop
        // cleanup (a killed test process, a Ctrl+C, a panic that unwound
        // past Scenario somehow). Best-effort: a sweep failure here is
        // logged, not fatal to the launch.
        for name in crate::registry::swept_target_image_names() {
            if let Err(error) = process_agent.kill_processes_by_name(name) {
                tracing::warn!(name, %error, "failed to pre-launch sweep a target image name");
            }
        }
        let verbatim_pid = process_agent.launch_process(
            exe_str,
            &[],
            Some(exe_dir_str),
            launch_env,
            Some(&stderr_path),
        )?;

        let deadline = Instant::now() + LAUNCH_TIMEOUT;
        let control = match wait_for_control_tunnel(&agent_addr, deadline) {
            Ok(client) => client,
            Err(error) => {
                let _ = process_agent.kill_process(verbatim_pid);
                return Err(io::Error::other(format!(
                    "Verbatim's control plane never came up: {error}"
                )));
            }
        };
        let speech_tunnel = match wait_for_control_tunnel(&agent_addr, deadline) {
            Ok(client) => client,
            Err(error) => {
                let _ = process_agent.kill_process(verbatim_pid);
                return Err(io::Error::other(format!(
                    "could not open a second control-plane tunnel for speech: {error}"
                )));
            }
        };
        let timeline = Timeline::new();
        let speech = SpeechCollector::subscribe(speech_tunnel, timeline.clone(), is_paced())
            .map_err(|error| {
                let _ = process_agent.kill_process(verbatim_pid);
                io::Error::other(format!("could not subscribe to speech: {error}"))
            })?;

        // See GUI_SETTLE_DELAY's doc comment: the control plane answering
        // does not yet mean the GUI thread has installed its gesture
        // handle.
        thread::sleep(GUI_SETTLE_DELAY);

        Ok(Self {
            _lock: lock,
            process_agent,
            verbatim_pid,
            control,
            speech,
            timeline,
            launched: Vec::new(),
            stderr_log_path: stderr_path,
        })
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

    /// Launches an extra target application (for example `notepad.exe`)
    /// through the agent, tracking it (pid and image name) for cleanup on
    /// drop unless [`Scenario::kill_target`] removes it first.
    ///
    /// The image name matters as much as the pid: confirmed live against
    /// the M2 guest, launching `notepad.exe` — even a single, solo launch
    /// with no other instance already open — hands off to a differently
    /// pid'd process and the launched pid itself exits within a few
    /// seconds. A pid-only kill later can therefore be a silent no-op
    /// against a process that is already gone, leaving the real window
    /// behind as a stray; [`Scenario::kill_target`] and [`Drop`] both sweep
    /// by this recorded name for exactly that reason.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn launch_target(&mut self, command: &str, args: &[&str]) -> io::Result<u32> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let pid = self
            .process_agent
            .launch_process(command, &args, None, &[], None)?;
        self.launched.push((pid, image_name(command)));
        Ok(pid)
    }

    /// Terminates a process previously launched via
    /// [`Scenario::launch_target`] and stops tracking it for drop-time
    /// cleanup.
    ///
    /// Kills by pid first (`pid`'s own `KillOutcome` is this method's
    /// return value), then sweeps by the image name recorded at launch —
    /// see [`launch_target`](Self::launch_target)'s doc comment for why a
    /// pid-only kill can miss the process actually holding the window. A
    /// failure sweeping by name is logged and does not change this method's
    /// own result, since the pid-kill above is the primary outcome being
    /// reported.
    ///
    /// # Errors
    ///
    /// Returns an error if the pid-kill request fails.
    pub fn kill_target(&mut self, pid: u32) -> io::Result<KillOutcome> {
        let name = self
            .launched
            .iter()
            .find(|(launched_pid, _)| *launched_pid == pid)
            .map(|(_, name)| name.clone());
        self.launched
            .retain(|(launched_pid, _)| *launched_pid != pid);
        let outcome = self.process_agent.kill_process(pid)?;
        if let Some(name) = name
            && let Err(error) = self.process_agent.kill_processes_by_name(&name)
        {
            tracing::warn!(
                pid,
                name,
                %error,
                "failed to sweep by image name after kill_target"
            );
        }
        Ok(outcome)
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
    /// into). Written for every run, pass or fail, so a passing diagnostic run
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
    }

    /// Collects the failure-only extra into `dir` (created if missing): a
    /// flight-recorder dump (`Request::DumpRecorder` returns the path Core
    /// wrote it to — same machine as [`stderr_log_path`](Self::stderr_log_path)
    /// in either mode, since Core and Verbatim's own stderr capture are the
    /// same process — read back through the agent). The always-on timeline and
    /// stderr log are written separately by [`Scenario::collect_run_artifacts`].
    ///
    /// Requires Verbatim to still be answering its control plane, so
    /// [`crate::registry::run`] calls this only after a scenario has *failed*,
    /// where the run skips the clean quit and leaves Verbatim up; a passing run
    /// has already quit and has no flight recorder to dump. Best-effort,
    /// deliberately never itself a source of test failure — the control
    /// connection or the agent may be in a degraded state (Verbatim crashed,
    /// the tunnel dropped) — so a failure is logged, not propagated.
    pub fn collect_failure_artifacts(&mut self, dir: &Path) {
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
        // Kill by pid first, then sweep by the recorded image name — see
        // launch_target's doc comment for why a pid-only kill can miss the
        // process actually holding the window (confirmed live for
        // notepad.exe). Both steps are best-effort: a failure in either is
        // logged, not propagated, since this runs even when the test
        // itself already failed or panicked.
        for (pid, name) in self.launched.drain(..) {
            if let Err(error) = self.process_agent.kill_process(pid) {
                tracing::warn!(
                    pid,
                    %error,
                    "failed to kill a scenario-launched process during cleanup"
                );
            }
            if let Err(error) = self.process_agent.kill_processes_by_name(&name) {
                tracing::warn!(
                    pid,
                    name,
                    %error,
                    "failed to sweep a scenario-launched process's image name during cleanup"
                );
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
    }
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
        return Ok(r"C:\VerbatimLab\verbatim\stderr-e2e.log".to_owned());
    }
    exe_dir
        .join("stderr-e2e.log")
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("stderr log path is not valid UTF-8"))
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

/// The directory-parameterized core of [`stage_binaries`], split out so unit
/// tests can exercise the hash-skip and missing-source-binary behavior
/// against temporary directories instead of the real workspace's
/// `target/e2e-stage` (which [`stage_binaries`] hardwires as its
/// destination).
fn copy_into_stage(source_dir: &Path, stage_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(stage_dir)?;
    for name in ["verbatim.exe", "verbatim-outpost.exe"] {
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

/// Writes `settings.toml` in `dir` to [`Settings::for_e2e`]'s fixed shape,
/// selecting the synthesizer named by `synth_id`. Never a load-modify-save
/// of whatever settings already sit in `dir`: builds the store from
/// [`Settings::for_e2e`] directly ([`ConfigStore::from_settings`]) and
/// writes it fresh, so a run's configuration can never accumulate state
/// left over from a previous run.
///
/// Two call shapes, both from [`Scenario::launch`]:
///
/// - The ordinary, silent case passes `"capture"`: the capture synthesizer,
///   registered by `verbatim-app` only when `VERBATIM_TEST_AUDIO=null` is
///   set (see `verbatim-synth-capture`). Deliberately not `onecore` there:
///   `OneCoreSynth::new` fails outright when no `OneCore` voices are
///   installed, which would fail every scenario launch on a bare CI
///   runner. The capture synth needs no installed voices and exercises the
///   same setting-descriptor-driven dialog machinery with a smaller
///   descriptor set (voice choice, rate slider; no rate-boost toggle,
///   unlike `OneCore` — the M1 exit-regression test documents this where it
///   walks the Speech dialog's controls).
/// - [`AUDIBLE_ENV`]'s case passes `"onecore"`: the real synthesizer, so a
///   human listening to the run hears real speech through real hardware.
///
/// Called only in runner-direct mode, on `dir` being [`stage_binaries`]'s
/// staging directory (this suite and the staged copy share a filesystem, so
/// writing directly there is correct); VM deploys handle this differently,
/// through `xtask vm deploy`'s own `write_synth_settings`, staged and
/// parameterized the same way but kept in lockstep independently — see
/// [`Settings::for_e2e`]'s doc comment.
fn configure_synth(dir: &Path, synth_id: &str) -> io::Result<()> {
    let store = ConfigStore::from_settings(dir, Settings::for_e2e(synth_id));
    store.save_settings().map_err(|error| config_error(&error))
}

fn config_error(error: &verbatim_config::ConfigError) -> io::Error {
    io::Error::other(error.to_string())
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

        copy_into_stage(&source_dir, &stage_dir).expect("first copy");
        assert_eq!(
            fs::read(stage_dir.join("verbatim.exe")).expect("read staged verbatim.exe"),
            b"verbatim v1"
        );
        assert_eq!(
            fs::read(stage_dir.join("verbatim-outpost.exe")).expect("read staged outpost"),
            b"outpost v1"
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
    fn configure_synth_writes_fixed_settings_never_a_merge_of_existing_state() {
        let dir = temp_dir("configure-synth");
        // Seed a pre-existing settings.toml carrying state a load-modify-save
        // would have carried forward (a different locale, a different
        // synthesizer) so this test would fail if configure_synth ever
        // starts loading instead of building fresh.
        fs::write(
            dir.join(ConfigStore::SETTINGS_FILE),
            "locale = \"de\"\n[speech]\nsynthesizer = \"onecore\"\n",
        )
        .expect("seed a pre-existing settings.toml");

        configure_synth(&dir, "capture").expect("writes fixed settings");

        let store = ConfigStore::load(&dir).expect("reloads what configure_synth wrote");
        assert_eq!(store.settings(), &Settings::for_e2e("capture"));
        assert_eq!(
            store.settings().locale,
            None,
            "the pre-existing locale must not survive: configure_synth never loads existing state"
        );
    }
}
