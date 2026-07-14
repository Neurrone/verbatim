//! Agent protocol v0.
//!
//! Newline-delimited compact JSON over TCP, framed with
//! [`verbatim_control::protocol::write_message`] and
//! [`verbatim_control::protocol::read_message`] — the same wire convention
//! the control plane uses, reused rather than reinvented. The envelope
//! style mirrors the control protocol too: every [`Request`] carries a
//! client-chosen correlation id in a [`RequestEnvelope`], and every
//! [`Frame`] answering one echoes that id.
//!
//! This is a deliberately separate vocabulary from
//! [`verbatim_control::protocol`]: the agent's pids are raw OS process ids
//! for test process management (Notepad, `verbatim.exe` itself), not
//! [`verbatim_model::Pid`]s naming an *observed* application in the
//! accessibility domain the control plane speaks about. Keeping the two
//! protocols and their pid types apart avoids conflating "the process this
//! test is driving" with "the process Verbatim is watching".

use serde::{Deserialize, Serialize};

/// The protocol version this vocabulary defines.
pub const AGENT_PROTOCOL_VERSION: u32 = 0;

/// The default TCP port the agent listens on.
///
/// Deliberately not a port in the 47000s: on a real development machine, a
/// whole band around 47600 (47600, 47601, 47650, 47712, 47800, 47900 all
/// tried) refused every bind attempt with "address already in use", while
/// nothing owned any of those ports per `Get-NetTCPConnection` and none of
/// them appear in `netsh interface ipv4/ipv6 show excludedportrange` — an
/// invisible reservation, almost certainly security software, that
/// standard tooling cannot see or explain. Since this protocol's whole
/// purpose is talking to unpredictable Hyper-V guests and CI runners,
/// picking a default outside that band (44001, empirically clear on the
/// same machine) is cheaper than debugging invisible port policy on every
/// machine this ever runs on. Override with `--port` when even this one is
/// unavailable.
pub const DEFAULT_PORT: u16 = 44001;

/// One client request with its correlation id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    /// Client-chosen id echoed in the matching reply or error.
    pub id: u64,
    /// The request.
    pub request: Request,
}

/// A client request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Request {
    /// Must be the first request on a connection; agrees on a protocol
    /// version. Refused on mismatch, mirroring the control server's rule.
    Hello {
        /// The highest protocol version the client speaks.
        protocol_version: u32,
    },
    /// Spawns a process via `std::process::Command`, inheriting the
    /// agent's own interactive session — the reason this request exists at
    /// all rather than something `WinRM` or PowerShell Direct could do.
    /// Stdio is not captured.
    LaunchProcess {
        /// The executable to run.
        command: String,
        /// Arguments, in order.
        args: Vec<String>,
        /// Working directory; `None` inherits the agent's.
        working_dir: Option<String>,
        /// Additional environment variables, added to (not replacing) the
        /// agent's own environment.
        env: Vec<(String, String)>,
    },
    /// Terminates a process by pid.
    KillProcess {
        /// The OS process id, as returned by a prior
        /// [`ReplyPayload::Launched`].
        pid: u32,
    },
    /// Asks whether a process is still running.
    ProcessStatus {
        /// The OS process id.
        pid: u32,
    },
    /// Asks for the agent's own session diagnostics: session id, whether
    /// its window station is interactive, and the input desktop's name
    /// when it can be opened. Exists so a screen reader test that can
    /// never work in a non-interactive session (the classic session 0
    /// problem) fails with a diagnosis instead of a mystery.
    SessionInfo,
    /// Reads a small file's contents, base64-encoded — logs, dumps. Refused
    /// above a size limit documented on [`ReplyPayload::FileContents`].
    ReadFile {
        /// Path to the file, agent-local.
        path: String,
    },
    /// Asks the agent to stop speaking this protocol on this connection and
    /// instead relay raw bytes to and from Verbatim's control-plane named
    /// pipe. After the reply to this request, the connection is a raw
    /// tunnel: the caller must switch to the control protocol's own
    /// framing immediately, starting with its own `Hello`.
    OpenControlTunnel,
}

/// One server-to-client frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Frame {
    /// Successful answer to a request.
    Reply {
        /// The request id this answers.
        to: u64,
        /// The payload.
        payload: ReplyPayload,
    },
    /// Failed answer to a request.
    Error {
        /// The request id this answers.
        to: u64,
        /// Human-readable reason.
        message: String,
    },
}

/// Payload of a [`Frame::Reply`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReplyPayload {
    /// Answer to [`Request::Hello`]: the version the agent will speak.
    Hello {
        /// Agreed protocol version.
        protocol_version: u32,
    },
    /// Answer to [`Request::LaunchProcess`].
    Launched {
        /// The spawned process's OS pid.
        pid: u32,
    },
    /// Answer to [`Request::KillProcess`].
    Killed(KillOutcome),
    /// Answer to [`Request::ProcessStatus`].
    ProcessStatus(ProcessState),
    /// Answer to [`Request::SessionInfo`].
    SessionInfo(SessionInfo),
    /// Answer to [`Request::ReadFile`]: the file's raw bytes, base64
    /// encoded. Files larger than a few megabytes are refused as a
    /// [`Frame::Error`] instead (see the agent's `files` module for the
    /// exact cap) — this request is for logs and small dumps, not bulk
    /// transfer.
    FileContents {
        /// The file's contents, base64 encoded (standard alphabet, with
        /// padding).
        data_base64: String,
    },
    /// Answer to [`Request::OpenControlTunnel`]: the agent successfully
    /// opened Verbatim's control-plane pipe and is ready to relay bytes.
    /// A failure to open that pipe is reported as a [`Frame::Error`]
    /// instead, before any tunneling begins.
    TunnelReady,
}

/// Outcome of [`Request::KillProcess`], distinguishing "terminated it" from
/// "it was already gone" — both success, deterministically reported rather
/// than folding the second case into an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KillOutcome {
    /// The process was running and was terminated.
    Terminated,
    /// The process had already exited; there was nothing to terminate.
    AlreadyExited,
}

/// Answer to [`Request::ProcessStatus`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessState {
    /// The process is still running.
    Running,
    /// The process has exited, with its exit code when it could be read.
    Exited {
        /// The process's exit code, when the OS reported one.
        exit_code: Option<i32>,
    },
}

/// Answer to [`Request::SessionInfo`]: diagnostics about the agent's own
/// process, since a screen reader driven from a non-interactive session
/// (session 0, or a non-interactive window station) can never work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// The Windows session id the agent process is running in
    /// (`ProcessIdToSessionId`).
    pub session_id: u32,
    /// Whether the agent's process window station is interactive
    /// (`GetProcessWindowStation` plus the `WSF_VISIBLE` flag from
    /// `GetUserObjectInformationW`).
    pub interactive_window_station: bool,
    /// The name of the current input desktop, when `OpenInputDesktop`
    /// succeeds. `None` when it cannot be opened, which itself is
    /// diagnostic: a non-interactive window station has no input desktop.
    pub input_desktop_name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_control::protocol::{read_message, write_message};

    #[test]
    fn requests_and_frames_round_trip() {
        let request = RequestEnvelope {
            id: 7,
            request: Request::LaunchProcess {
                command: "notepad.exe".to_owned(),
                args: vec![],
                working_dir: None,
                env: vec![("VERBATIM_TEST_AUDIO".to_owned(), "null".to_owned())],
            },
        };
        let frame = Frame::Reply {
            to: 7,
            payload: ReplyPayload::Launched { pid: 4242 },
        };

        let mut buffer = Vec::new();
        write_message(&mut buffer, &request).expect("writes");
        write_message(&mut buffer, &frame).expect("writes");

        let mut reader = buffer.as_slice();
        let read_request: RequestEnvelope = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        let read_frame: Frame = read_message(&mut reader)
            .expect("reads")
            .expect("not end of stream");
        assert_eq!(read_request, request);
        assert_eq!(read_frame, frame);
    }

    #[test]
    fn kill_and_status_and_session_info_round_trip() {
        let messages = [
            Frame::Reply {
                to: 1,
                payload: ReplyPayload::Killed(KillOutcome::AlreadyExited),
            },
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::ProcessStatus(ProcessState::Exited { exit_code: Some(0) }),
            },
            Frame::Reply {
                to: 3,
                payload: ReplyPayload::SessionInfo(SessionInfo {
                    session_id: 1,
                    interactive_window_station: true,
                    input_desktop_name: Some("Default".to_owned()),
                }),
            },
        ];

        let mut buffer = Vec::new();
        for message in &messages {
            write_message(&mut buffer, message).expect("writes");
        }
        let mut reader = buffer.as_slice();
        for expected in &messages {
            let read: Frame = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            assert_eq!(&read, expected);
        }
    }
}
