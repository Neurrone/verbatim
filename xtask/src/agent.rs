//! The end-to-end agent a command starts for itself and ends when it
//! ends: `cargo xtask e2e`, `cargo xtask nvda`, `cargo xtask measure` and
//! `cargo xtask trace scenario` run their scenarios and
//! captures through one, so no agent started by hand or by a script is
//! left listening (`books/dev/src/how-to/e2e.md`, "Running scenarios
//! locally"). The agent is the command's child, inside its job
//! (`crate::contain`), so ending the command by any means ends it.

use std::fs;
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Where the agent listens: the endpoint `books/dev/src/how-to/e2e.md` names.
pub const ENDPOINT: &str = "127.0.0.1:44001";

/// How long the agent has to start listening.
const AGENT_START: Duration = Duration::from_secs(30);

/// Builds the agent in `root`.
pub fn build(root: &Path) -> Result<(), String> {
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "verbatim-agent"])
        .current_dir(root)
        .status()
        .map_err(|error| format!("could not run cargo build -p verbatim-agent: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo build -p verbatim-agent failed ({status})"))
    }
}

/// The agent, started for the command and ended when dropped; the
/// command's job ends it on every other path.
pub struct Agent(Child);

impl Agent {
    /// Starts the agent, logging to `logs`, and waits until it listens.
    pub fn start(root: &Path, logs: &Path) -> Result<Self, String> {
        if TcpStream::connect(ENDPOINT).is_ok() {
            return Err(format!(
                "something already listens on {ENDPOINT}, such as an agent started by hand; end it first"
            ));
        }
        let log = |name: &str| {
            fs::File::create(logs.join(name)).map_or_else(|_| Stdio::null(), Stdio::from)
        };
        let mut child = Command::new(root.join(r"target\debug\verbatim-agent.exe"))
            .args(["--bind-address", "127.0.0.1", "--port", "44001"])
            .stdout(log("agent.out.log"))
            .stderr(log("agent.err.log"))
            .spawn()
            .map_err(|error| format!("could not start the agent: {error}"))?;
        let deadline = Instant::now() + AGENT_START;
        // Listening is the evidence it is ready; its exit ends the wait.
        while TcpStream::connect(ENDPOINT).is_err() {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("the agent exited before listening ({status})"));
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return Err(format!("the agent did not listen within {AGENT_START:?}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Self(child))
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
