//! A typed host-side client for the M2 in-guest agent protocol
//! (`verbatim_agent::protocol`).
//!
//! Connects over TCP, completes the agent's `Hello` handshake, and exposes
//! the request vocabulary as plain methods.
//! [`AgentClient::open_control_tunnel`] is the seam into Verbatim's own
//! control plane: it hands the same TCP connection to
//! [`verbatim_control::client::Client`] via the tunnel handoff the agent
//! protocol defines, so from that point on the connection speaks the
//! control protocol instead.

use std::io::{self, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use verbatim_agent::protocol::{
    AGENT_PROTOCOL_VERSION, Frame, KillOutcome, ProcessState, ReplyPayload, Request,
    RequestEnvelope, SessionInfo,
};
use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::{read_message, write_message};

/// Read timeout applied to the socket once it becomes a control-plane
/// tunnel, so a Verbatim that stops answering fails the caller's next call
/// instead of hanging the test suite forever. Deliberately short so callers
/// that poll on top of it (readiness checks, [`crate::speech::SpeechCollector`]'s
/// wait loop) wake up often enough to recheck their own, longer deadlines.
pub const CONTROL_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Per-attempt cap on establishing the TCP connection itself. Without one,
/// an unanswered connect to a guest that is mid-restore or renewing its
/// address waits out the OS default (tens of seconds), which once consumed
/// a launch-poll deadline in a single attempt; bounding each attempt is
/// what makes the callers' retry loops actually retry.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A connection to the M2 agent, past its `Hello` handshake.
pub struct AgentClient {
    stream: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u64,
}

impl AgentClient {
    /// Connects to the agent at `addr` and completes its `Hello` handshake.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established, or the
    /// handshake fails or is refused (a protocol-version mismatch).
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let socket_addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::other("the agent address resolved to no socket address"))?;
        let stream = TcpStream::connect_timeout(&socket_addr, CONNECT_TIMEOUT)?;
        let reader = BufReader::new(stream.try_clone()?);
        let mut client = Self {
            stream,
            reader,
            next_id: 1,
        };
        match client.request(Request::Hello {
            protocol_version: AGENT_PROTOCOL_VERSION,
        })? {
            Frame::Reply {
                payload: ReplyPayload::Hello { .. },
                ..
            } => Ok(client),
            Frame::Error { message, .. } => Err(io::Error::other(format!(
                "agent Hello was refused: {message}"
            ))),
            other @ Frame::Reply { .. } => Err(unexpected("Hello", &other)),
        }
    }

    /// Sends one request and waits for its matching reply or error.
    fn request(&mut self, request: Request) -> io::Result<Frame> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(&mut self.stream, &RequestEnvelope { id, request })?;
        loop {
            let frame: Frame = read_message(&mut self.reader)?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "agent closed the connection")
            })?;
            match &frame {
                Frame::Reply { to, .. } | Frame::Error { to, .. } if *to == id => return Ok(frame),
                _ => {}
            }
        }
    }

    /// Spawns `command` on the agent's guest, inheriting its interactive
    /// session. `env` is added to, not replacing, the agent's own
    /// environment. `stderr_to`, when set, asks the agent to capture the
    /// child's stdout and stderr into that path (truncated first) instead
    /// of leaving them uncaptured — see
    /// `verbatim_agent::protocol::Request::LaunchProcess`. Returns the
    /// spawned process's OS pid.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the agent could not spawn
    /// the process.
    pub fn launch_process(
        &mut self,
        command: &str,
        args: &[String],
        working_dir: Option<&str>,
        env: &[(String, String)],
        stderr_to: Option<&str>,
    ) -> io::Result<u32> {
        match self.request(Request::LaunchProcess {
            command: command.to_owned(),
            args: args.to_vec(),
            working_dir: working_dir.map(str::to_owned),
            env: env.to_vec(),
            stderr_to: stderr_to.map(str::to_owned),
        })? {
            Frame::Reply {
                payload: ReplyPayload::Launched { pid },
                ..
            } => Ok(pid),
            other => Err(unexpected("LaunchProcess", &other)),
        }
    }

    /// Terminates `pid` on the guest.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn kill_process(&mut self, pid: u32) -> io::Result<KillOutcome> {
        match self.request(Request::KillProcess { pid })? {
            Frame::Reply {
                payload: ReplyPayload::Killed(outcome),
                ..
            } => Ok(outcome),
            other => Err(unexpected("KillProcess", &other)),
        }
    }

    /// Terminates every process on the guest whose image (executable file)
    /// name matches `name`, case-insensitively — see
    /// `verbatim_agent::protocol::Request::KillProcessesByName`. Returns
    /// how many were actually terminated; zero is a normal, successful
    /// outcome, not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn kill_processes_by_name(&mut self, name: &str) -> io::Result<u32> {
        match self.request(Request::KillProcessesByName {
            name: name.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::KilledByName { terminated },
                ..
            } => Ok(terminated),
            other => Err(unexpected("KillProcessesByName", &other)),
        }
    }

    /// Asks whether `pid` is still running on the guest.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn process_status(&mut self, pid: u32) -> io::Result<ProcessState> {
        match self.request(Request::ProcessStatus { pid })? {
            Frame::Reply {
                payload: ReplyPayload::ProcessStatus(state),
                ..
            } => Ok(state),
            other => Err(unexpected("ProcessStatus", &other)),
        }
    }

    /// Asks the agent for its own session diagnostics — whether it is
    /// running in an interactive window station, the classic "session 0"
    /// check.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn session_info(&mut self) -> io::Result<SessionInfo> {
        match self.request(Request::SessionInfo)? {
            Frame::Reply {
                payload: ReplyPayload::SessionInfo(info),
                ..
            } => Ok(info),
            other => Err(unexpected("SessionInfo", &other)),
        }
    }

    /// Reads a small file's contents from the guest.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the reply is not valid
    /// base64.
    pub fn read_file(&mut self, path: &str) -> io::Result<Vec<u8>> {
        match self.request(Request::ReadFile {
            path: path.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FileContents { data_base64 },
                ..
            } => STANDARD.decode(data_base64).map_err(io::Error::other),
            other => Err(unexpected("ReadFile", &other)),
        }
    }

    /// Asks the agent to stop speaking its own protocol on this connection
    /// and relay Verbatim's control-plane pipe instead, then completes the
    /// control protocol's own `Hello` on the same socket and returns a
    /// ready [`ControlClient`].
    ///
    /// Sets [`CONTROL_READ_TIMEOUT`] on the socket before the handoff, so a
    /// Verbatim that stops answering fails the caller's next call instead of
    /// hanging it silently.
    ///
    /// # Errors
    ///
    /// Returns an error if the agent has no control-plane pipe to tunnel to
    /// yet — the common case while Verbatim is still starting, which
    /// callers poll for (see [`crate::scenario::Scenario::launch`]) — or if
    /// the control protocol's own handshake fails.
    pub fn open_control_tunnel(mut self) -> io::Result<ControlClient> {
        match self.request(Request::OpenControlTunnel)? {
            Frame::Reply {
                payload: ReplyPayload::TunnelReady,
                ..
            } => {}
            Frame::Error { message, .. } => {
                return Err(io::Error::other(format!(
                    "OpenControlTunnel refused: {message}"
                )));
            }
            other @ Frame::Reply { .. } => return Err(unexpected("OpenControlTunnel", &other)),
        }
        // No more agent-protocol reads happen on this connection: drop the
        // reader before handing the plain stream to the control client, so
        // nothing it wrote is left stranded in this reader's own buffer
        // (the same handoff discipline verbatim-agent's own tunnel test
        // documents).
        drop(self.reader);
        self.stream.set_read_timeout(Some(CONTROL_READ_TIMEOUT))?;
        ControlClient::from_tcp_stream(self.stream)
    }
}

fn unexpected(request_name: &str, frame: &Frame) -> io::Error {
    io::Error::other(format!("unexpected reply to {request_name}: {frame:?}"))
}
