//! [`Scenario`]: the lifecycle owner for one live, agent-driven Verbatim run.
//!
//! A scenario is a guard struct, not a checklist of manual cleanup calls: it
//! launches Verbatim (and, on request, target applications) through the M2
//! agent, and its [`Drop`] impl kills everything it launched, unconditionally,
//! even if the test that created it panicked partway through. This matters
//! because every live test in this suite runs against the developer's real
//! desktop, launching a real `verbatim.exe` that injects real keystrokes.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use verbatim_agent::protocol::{KillOutcome, ProcessState};
use verbatim_config::ConfigStore;
use verbatim_control::client::{Client as ControlClient, ok_or_error};
use verbatim_control::protocol::{LatencyRecord, Request};

use crate::agent_client::AgentClient;
use crate::speech::SpeechCollector;
use crate::{ENDPOINT_ENV, endpoint};

/// Environment variable overriding the path to `verbatim.exe`. Defaults to
/// `target/debug/verbatim.exe` under the workspace root — the ordinary
/// local debug build, and what the CI job in `.github/workflows/ci.yml`
/// builds before running this suite.
pub const VERBATIM_EXE_ENV: &str = "VERBATIM_E2E_VERBATIM_EXE";

/// Environment variable marking a *remote* run: the agent, Verbatim, and
/// its configuration live on another machine (the Hyper-V guest), so
/// [`VERBATIM_EXE_ENV`] names a path in that machine's filesystem, not this
/// one's. `cargo xtask vm test` sets it.
///
/// The distinction matters because two of [`Scenario::launch`]'s steps —
/// checking that `verbatim.exe` exists, and writing the capture-synth
/// `settings.toml` next to it — are ordinary host filesystem operations.
/// In the default runner-direct mode the suite and Verbatim share a
/// filesystem, so both are correct. Against a VM they are not: the guest
/// path does not exist here, and `cargo xtask vm deploy` has already staged
/// the same `settings.toml` inside the guest. Set this and both steps are
/// skipped, since the deploy owns them.
pub const REMOTE_ENV: &str = "VERBATIM_E2E_REMOTE";

/// Whether this is a remote (in-guest) run; see [`REMOTE_ENV`].
fn is_remote() -> bool {
    std::env::var(REMOTE_ENV).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
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
    /// Extra processes launched via [`Scenario::launch_target`], killed on
    /// drop unless already removed by [`Scenario::kill_target`].
    launched: Vec<u32>,
}

impl Scenario {
    /// Launches a fresh Verbatim through the agent named by
    /// [`crate::ENDPOINT_ENV`]: writes a `settings.toml` next to
    /// `verbatim.exe` selecting the capture synthesizer (audio-free and
    /// dependency-free — it needs no installed voices, unlike `OneCore`),
    /// launches it with `VERBATIM_TEST_AUDIO=null` (device-free
    /// `NullSink`, still emitting complete latency timelines), and waits
    /// for its control plane to answer over the agent's tunnel. Opens a
    /// second, dedicated tunnel connection for speech collection (see
    /// [`crate::speech::SpeechCollector`] for why it must not share the
    /// command connection).
    ///
    /// # Errors
    ///
    /// Returns an error if [`crate::ENDPOINT_ENV`] is unset, `verbatim.exe`
    /// cannot be found, the agent cannot be reached, or Verbatim's control
    /// plane never comes up within the launch timeout.
    pub fn launch() -> io::Result<Self> {
        let agent_addr =
            endpoint().ok_or_else(|| io::Error::other(format!("{ENDPOINT_ENV} is not set")))?;
        let lock = live_instance_lock()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let verbatim_exe = verbatim_exe_path();
        let remote = is_remote();
        if !remote && !verbatim_exe.is_file() {
            return Err(io::Error::other(format!(
                "verbatim.exe not found at {} (set {VERBATIM_EXE_ENV} to override, or {REMOTE_ENV} if it lives in a guest)",
                verbatim_exe.display()
            )));
        }
        let exe_dir = verbatim_exe
            .parent()
            .ok_or_else(|| io::Error::other("verbatim.exe path has no parent directory"))?;
        // In a remote run the path above names a location in the guest, so
        // neither the existence check nor the config write can happen here;
        // `cargo xtask vm deploy` staged both inside the guest already.
        if !remote {
            configure_capture_synth(exe_dir)?;
        }

        let exe_str = verbatim_exe
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe path is not valid UTF-8"))?;
        let exe_dir_str = exe_dir
            .to_str()
            .ok_or_else(|| io::Error::other("verbatim.exe directory is not valid UTF-8"))?;

        let mut process_agent = AgentClient::connect(&agent_addr)?;
        let verbatim_pid = process_agent.launch_process(
            exe_str,
            &[],
            Some(exe_dir_str),
            &[("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned())],
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
        let speech = SpeechCollector::subscribe(speech_tunnel).map_err(|error| {
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
            launched: Vec::new(),
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
        ok_or_error(self.control.request(Request::SendKeys {
            keys: keys.iter().map(|key| (*key).to_owned()).collect(),
        })?)?;
        Ok(())
    }

    /// Launches an extra target application (for example `notepad.exe`)
    /// through the agent, tracking it for cleanup on drop unless
    /// [`Scenario::kill_target`] removes it first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn launch_target(&mut self, command: &str, args: &[&str]) -> io::Result<u32> {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        let pid = self
            .process_agent
            .launch_process(command, &args, None, &[])?;
        self.launched.push(pid);
        Ok(pid)
    }

    /// Terminates a process previously launched via
    /// [`Scenario::launch_target`] and stops tracking it for drop-time
    /// cleanup.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn kill_target(&mut self, pid: u32) -> io::Result<KillOutcome> {
        self.launched.retain(|&launched| launched != pid);
        self.process_agent.kill_process(pid)
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
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                // Verbatim tore down before the reply arrived: the intended
                // outcome, just observed from the losing side of the race.
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
}

impl Drop for Scenario {
    fn drop(&mut self) {
        for pid in self.launched.drain(..) {
            if let Err(error) = self.process_agent.kill_process(pid) {
                tracing::warn!(
                    pid,
                    %error,
                    "failed to kill a scenario-launched process during cleanup"
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
/// does not depend on the caller's working directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("verbatim-e2e lives two directories under the workspace root")
        .to_path_buf()
}

/// The `verbatim.exe` path a scenario launches: [`VERBATIM_EXE_ENV`] when
/// set, otherwise `target/debug/verbatim.exe` under the workspace root.
fn verbatim_exe_path() -> PathBuf {
    if let Ok(path) = std::env::var(VERBATIM_EXE_ENV) {
        return PathBuf::from(path);
    }
    workspace_root()
        .join("target")
        .join("debug")
        .join("verbatim.exe")
}

/// Writes `settings.toml` in `exe_dir` selecting the capture synthesizer
/// (id `capture`, registered by `verbatim-app` only when
/// `VERBATIM_TEST_AUDIO=null` is set — see `verbatim-synth-capture`).
///
/// Deliberately not `onecore`: `OneCoreSynth::new` fails outright when no
/// `OneCore` voices are installed, which would fail every scenario launch
/// on a bare CI runner. The capture synth needs no installed voices and
/// exercises the same setting-descriptor-driven dialog machinery with a
/// smaller descriptor set (voice choice, rate slider; no rate-boost
/// toggle, unlike `OneCore` — the M1 exit-regression test documents this
/// where it walks the Speech dialog's controls).
///
/// In runner-direct mode (this suite and the target share a filesystem)
/// writing directly here is correct; VM deploys handle this differently,
/// through `xtask vm deploy`.
fn configure_capture_synth(exe_dir: &Path) -> io::Result<()> {
    let mut store = ConfigStore::load(exe_dir).map_err(|error| config_error(&error))?;
    store.settings_mut().speech.synthesizer = Some("capture".to_owned());
    store.save_settings().map_err(|error| config_error(&error))
}

fn config_error(error: &verbatim_config::ConfigError) -> io::Error {
    io::Error::other(error.to_string())
}
