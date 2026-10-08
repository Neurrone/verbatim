//! Ending a child cleanly: the shutdown message, a wait for the process to
//! exit, and a kill through its job only when it has not exited within
//! [`SHUTDOWN_LIMIT`]. Every way an outpost or the listener ends goes
//! through [`retire`]: Verbatim exiting, a child replaced or restarted, an
//! outpost retired or ended because its application exited.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use super::owner::Child;
use super::writer::Outgoing;

/// How long a child has to exit after the shutdown message before it is
/// killed through its job.
///
/// A clean shutdown is quick (measured over the end-to-end suite: see
/// `docs/crates/verbatim-outpost.md`, "Shutting down"), except while a call
/// or a remote operation into an application is in progress, which the
/// outpost lets finish rather than cut off: killing a UI Automation client
/// in the middle of a call is the leading suspect in a Windows Terminal
/// crash inside `UIAutomationCore.dll` (`phase6-design.md`, "Windows
/// Terminal crash of 2026-10-08"). Such a call ends by UIA's own timeouts
/// even when the application never answers: a call waiting on a provider
/// that does not answer is ended by UIA's transaction timeout, 20 seconds,
/// UIA's default, which Verbatim leaves as it is (a classic call and a
/// remote operation's `Execute` alike, as mockapp's `remote_ops` tests
/// pin), and the connection timeout, 10 seconds, ends a call to a provider
/// that cannot be reached. So 20 seconds covers every call UIA ends by
/// itself, and the second on top covers the shutdown's own work. Waiting
/// longer could only wait on an MSAA call into a hung application, which
/// nothing times out; that one is killed.
pub(super) const SHUTDOWN_LIMIT: Duration = Duration::from_secs(21);

/// How a child ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// It exited with code 0 after the shutdown message.
    Clean,
    /// It had already exited, or exited with another code: it crashed or
    /// failed rather than shutting down.
    Exited(Option<u32>),
    /// It had not exited within the limit and was killed through its job.
    Killed,
}

/// Ends `child`: unless it has already exited, sends it the shutdown
/// message, closes its command pipe once that is written, and waits up to
/// `limit` for it to exit; kills it through its job if it has not. Logs
/// how it ended, and every kill with `why` and what it was waiting for, as
/// a warning. `label` names the child in the log. Blocks until the child
/// has ended, so the owner runs it on a thread of its own.
pub(super) fn retire(child: Child, label: &str, why: &str, limit: Duration) -> Ending {
    let started = Instant::now();
    let Child { process, writer } = child;
    let contained = &process.contained;
    let process_id = process.process_id;
    if contained.has_exited() {
        writer.close();
        let code = contained.exit_code();
        tracing::info!(label, %process_id, why, ?code, "ended: it had already exited");
        return Ending::Exited(code);
    }
    let _ = writer.push(Outgoing::Shutdown);
    writer.close();
    if contained.wait_for_exit(limit) {
        let code = contained.exit_code();
        let elapsed_ms = started.elapsed().as_millis();
        if code == Some(0) {
            tracing::info!(label, %process_id, why, elapsed_ms, "shut down cleanly");
            return Ending::Clean;
        }
        tracing::warn!(label, %process_id, why, elapsed_ms, ?code, "exited with an error while shutting down");
        return Ending::Exited(code);
    }
    tracing::warn!(
        label,
        %process_id,
        why,
        limit_ms = limit.as_millis(),
        "fell back to killing: it had not exited within the shutdown limit after the shutdown message, most likely still in a call into an application that does not answer"
    );
    if let Err(error) = contained.kill() {
        tracing::error!(label, %process_id, %error, "could not be killed; its job handle is closed instead");
    }
    Ending::Killed
}

/// How the children that ended during this Verbatim's life ended.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShutdownSummary {
    /// Children that shut down cleanly.
    pub clean: usize,
    /// Children that had already exited, or exited with an error, when
    /// they were ended.
    pub exited: usize,
    /// Children that had to be killed: the fallback.
    pub killed: usize,
}

impl ShutdownSummary {
    /// Counts one ending.
    pub(super) fn count(&mut self, ending: Ending) {
        match ending {
            Ending::Clean => self.clean += 1,
            Ending::Exited(_) => self.exited += 1,
            Ending::Killed => self.killed += 1,
        }
    }
}
