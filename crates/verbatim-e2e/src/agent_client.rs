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
//!
//! Every request that waits on the agent's side (for a window, a process,
//! a file, or an event) is read with a timeout longer than the wait it
//! asks for, so a wait that runs out is answered as such on a live
//! connection, and only an agent that stops answering altogether ends the
//! request with a read timeout.

use std::io::{self, BufReader};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use verbatim_agent::protocol::{
    AGENT_PROTOCOL_VERSION, EventOutcome, FocusedElement, ForegroundInfo, Frame, KillOutcome,
    ProcessExit, ProcessInfo, ProcessState, ReplyPayload, Request, RequestEnvelope, SessionInfo,
    WindowCondition, WindowInfo,
};
use verbatim_control::client::Client as ControlClient;
use verbatim_control::protocol::{MessageReader, write_message};

/// Read timeout applied to the socket once it becomes a control-plane
/// tunnel, so a Verbatim that stops answering fails the caller's next read
/// instead of hanging the test suite forever. A read that times out is not
/// a failure by itself: the speech collector reads with it and checks its
/// own deadline for what it waits for.
pub const CONTROL_READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounds every reply and write on the agent's own protocol for a request
/// that does not wait, so an agent that stops answering fails the caller's
/// request instead of hanging the suite.
const AGENT_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How much longer than the wait it asked for a waiting request's reply
/// may take: the agent answers when its wait ends, so this only covers
/// the round trip.
const WAIT_REPLY_MARGIN: Duration = Duration::from_secs(10);

/// Cap on establishing the TCP connection itself.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A process the agent launched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launched {
    /// Its OS process id.
    pub pid: u32,
    /// Whether Windows let the agent allow it to take the foreground with
    /// its first window, as a program a user starts may.
    pub foreground_allowed: bool,
}

/// A connection to the M2 agent, past its `Hello` handshake.
pub struct AgentClient {
    stream: TcpStream,
    reader: MessageReader<BufReader<TcpStream>>,
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
        stream.set_read_timeout(Some(AGENT_REPLY_TIMEOUT))?;
        stream.set_write_timeout(Some(AGENT_REPLY_TIMEOUT))?;
        let reader = MessageReader::new(BufReader::new(stream.try_clone()?));
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
            let frame: Frame = self.reader.read()?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "agent closed the connection")
            })?;
            match &frame {
                Frame::Reply { to, .. } | Frame::Error { to, .. } if *to == id => return Ok(frame),
                _ => {}
            }
        }
    }

    /// Sends a request that waits on the agent's side for up to `wait`, and
    /// reads its reply with a timeout [`WAIT_REPLY_MARGIN`] longer.
    fn request_waiting(&mut self, request: Request, wait: Duration) -> io::Result<Frame> {
        self.set_read_timeout(wait + WAIT_REPLY_MARGIN)?;
        let frame = self.request(request);
        self.set_read_timeout(AGENT_REPLY_TIMEOUT)?;
        frame
    }

    /// Sets the read timeout of the socket replies are read from: the
    /// reader's own duplicate of the connection's handle, whose timeout is
    /// its own.
    fn set_read_timeout(&self, timeout: Duration) -> io::Result<()> {
        self.reader
            .get_ref()
            .get_ref()
            .set_read_timeout(Some(timeout))
    }

    /// Spawns `command` on the agent's guest, inheriting its interactive
    /// session. `env` is added to, not replacing, the agent's own
    /// environment. `stderr_to`, when set, asks the agent to capture the
    /// child's stdout and stderr into that path (truncated first).
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
    ) -> io::Result<Launched> {
        self.launch(Request::LaunchProcess {
            command: command.to_owned(),
            args: args.to_vec(),
            working_dir: working_dir.map(str::to_owned),
            env: env.to_vec(),
            stderr_to: stderr_to.map(str::to_owned),
            console_title: None,
            minimized: false,
            withhold_foreground: false,
            ignore_foreign_terminals: false,
        })
    }

    /// Launches Verbatim, as [`AgentClient::launch_process`] launches any
    /// program, telling it to ignore entirely every Windows Terminal the
    /// agent did not launch, with the console hosts it runs: the owner's
    /// own, which a Verbatim under test must never read
    /// (`VERBATIM_IGNORE_PIDS`). Answers the launch and the pids of the
    /// processes Verbatim was told to ignore.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the agent could not spawn
    /// the process.
    pub fn launch_verbatim(
        &mut self,
        command: &str,
        working_dir: &str,
        env: &[(String, String)],
        stderr_to: &str,
    ) -> io::Result<(Launched, Vec<u32>)> {
        self.launch_answer(Request::LaunchProcess {
            command: command.to_owned(),
            args: Vec::new(),
            working_dir: Some(working_dir.to_owned()),
            env: env.to_vec(),
            stderr_to: Some(stderr_to.to_owned()),
            console_title: None,
            minimized: false,
            withhold_foreground: false,
            ignore_foreign_terminals: true,
        })
    }

    /// Launches `command` with `args`, as [`AgentClient::launch_process`]
    /// does, its first window opening minimized and inactive, for the
    /// caller to bring forward once it is ready.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the agent could not spawn
    /// the process.
    pub fn launch_minimized(&mut self, command: &str, args: &[String]) -> io::Result<Launched> {
        self.launch(Request::LaunchProcess {
            command: command.to_owned(),
            args: args.to_vec(),
            working_dir: None,
            env: Vec::new(),
            stderr_to: None,
            console_title: None,
            minimized: true,
            withhold_foreground: false,
            ignore_foreign_terminals: false,
        })
    }

    /// Launches `command` with `args`, as [`AgentClient::launch_process`]
    /// does, its first window opening as the program opens it, without the
    /// agent's right to take the foreground: for a program the caller
    /// brings forward itself, which takes the foreground by another's right
    /// or not at all.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the agent could not spawn
    /// the process.
    pub fn launch_without_foreground_right(
        &mut self,
        command: &str,
        args: &[String],
    ) -> io::Result<Launched> {
        self.launch(Request::LaunchProcess {
            command: command.to_owned(),
            args: args.to_vec(),
            working_dir: None,
            env: Vec::new(),
            stderr_to: None,
            console_title: None,
            minimized: false,
            withhold_foreground: true,
            ignore_foreign_terminals: false,
        })
    }

    /// Launches the console program `command` with `args`, as
    /// [`AgentClient::launch_process`] does, its console window titled
    /// `title` from its first frame and opening minimized and inactive, for
    /// the caller to bring forward once it is ready.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the agent could not spawn
    /// the process.
    pub fn launch_console(
        &mut self,
        command: &str,
        args: &[String],
        title: &str,
    ) -> io::Result<Launched> {
        self.launch(Request::LaunchProcess {
            command: command.to_owned(),
            args: args.to_vec(),
            working_dir: None,
            env: Vec::new(),
            stderr_to: None,
            console_title: Some(title.to_owned()),
            minimized: true,
            withhold_foreground: false,
            ignore_foreign_terminals: false,
        })
    }

    /// Sends a launch request and reads its reply.
    fn launch(&mut self, request: Request) -> io::Result<Launched> {
        self.launch_answer(request).map(|(launched, _)| launched)
    }

    /// Sends a launch request and reads its reply, with the pids of the
    /// processes the launched program was told to ignore.
    fn launch_answer(&mut self, request: Request) -> io::Result<(Launched, Vec<u32>)> {
        match self.request(request)? {
            Frame::Reply {
                payload:
                    ReplyPayload::Launched {
                        pid,
                        foreground_allowed,
                        ignored,
                    },
                ..
            } => Ok((
                Launched {
                    pid,
                    foreground_allowed,
                },
                ignored,
            )),
            other => Err(unexpected("LaunchProcess", &other)),
        }
    }

    /// Terminates `pid` on the guest, with everything in its job when the
    /// agent launched it.
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

    /// Ends every process the agent launched that is still running, by its
    /// own handle, and returns how many were.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn end_launched(&mut self) -> io::Result<u32> {
        match self.request(Request::EndLaunched)? {
            Frame::Reply {
                payload: ReplyPayload::EndedLaunched { ended },
                ..
            } => Ok(ended),
            other => Err(unexpected("EndLaunched", &other)),
        }
    }

    /// The processes whose parent is `pid`.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn child_processes(&mut self, pid: u32) -> io::Result<Vec<ProcessInfo>> {
        match self.request(Request::ChildProcesses { pid })? {
            Frame::Reply {
                payload: ReplyPayload::Processes { processes },
                ..
            } => Ok(processes),
            other => Err(unexpected("ChildProcesses", &other)),
        }
    }

    /// The processes that have exited in the job of `pid`, which the agent
    /// launched, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn job_exits(&mut self, pid: u32) -> io::Result<Vec<ProcessExit>> {
        match self.request(Request::JobExits { pid })? {
            Frame::Reply {
                payload: ReplyPayload::Exits { exits },
                ..
            } => Ok(exits),
            other => Err(unexpected("JobExits", &other)),
        }
    }

    /// Brings `window` to the foreground without injecting input, and
    /// returns whether it is the foreground window.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn set_foreground(&mut self, window: u64) -> io::Result<bool> {
        match self.request(Request::SetForeground { window })? {
            Frame::Reply {
                payload: ReplyPayload::Foreground { taken },
                ..
            } => Ok(taken),
            other => Err(unexpected("SetForeground", &other)),
        }
    }

    /// Waits up to `timeout`, on window events, for `condition`, and
    /// returns whether it held and the desktop then.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn wait_for_window(
        &mut self,
        condition: WindowCondition,
        timeout: Duration,
    ) -> io::Result<(bool, ForegroundInfo)> {
        match self.request_waiting(
            Request::WaitForWindow {
                condition,
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::WindowState { met, desktop },
                ..
            } => Ok((met, desktop)),
            other => Err(unexpected("WaitForWindow", &other)),
        }
    }

    /// Minimizes every window, as Show Desktop does, waits up to `timeout`
    /// for every window that can be minimized to be, then for the desktop
    /// to hold the foreground; returns whether each held, and the desktop.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn minimize_all(&mut self, timeout: Duration) -> io::Result<(bool, bool, ForegroundInfo)> {
        match self.request_waiting(
            Request::MinimizeAll {
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload:
                    ReplyPayload::Minimized {
                        minimized,
                        desktop_in_front,
                        desktop,
                    },
                ..
            } => Ok((minimized, desktop_in_front, desktop)),
            other => Err(unexpected("MinimizeAll", &other)),
        }
    }

    /// Waits up to `timeout` for `pid` to exit, and reports whether it is
    /// still running.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn wait_for_exit(&mut self, pid: u32, timeout: Duration) -> io::Result<ProcessState> {
        match self.request_waiting(
            Request::WaitForExit {
                pid,
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::ProcessStatus(state),
                ..
            } => Ok(state),
            other => Err(unexpected("WaitForExit", &other)),
        }
    }

    /// Waits up to `timeout` for every process in the job of `pid`, which
    /// the agent launched, to have exited, and returns every exit in the
    /// job, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, naming the processes still
    /// running when the wait ran out.
    pub fn wait_for_job_empty(
        &mut self,
        pid: u32,
        timeout: Duration,
    ) -> io::Result<Vec<ProcessExit>> {
        match self.request_waiting(
            Request::WaitForJobEmpty {
                pid,
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::Exits { exits },
                ..
            } => Ok(exits),
            other => Err(unexpected("WaitForJobEmpty", &other)),
        }
    }

    /// Takes the Windows Terminal windows shown since the last call by
    /// processes the agent did not launch; the first call starts the watch
    /// and returns none.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn take_foreign_terminal_windows(&mut self) -> io::Result<Vec<WindowInfo>> {
        match self.request(Request::TakeForeignTerminalWindows)? {
            Frame::Reply {
                payload: ReplyPayload::TerminalWindows { windows },
                ..
            } => Ok(windows),
            other => Err(unexpected("TakeForeignTerminalWindows", &other)),
        }
    }

    /// Waits up to `timeout`, on changes in its folder, for `path` to exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn wait_for_file(&mut self, path: &str, timeout: Duration) -> io::Result<bool> {
        match self.request_waiting(
            Request::WaitForFile {
                path: path.to_owned(),
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::FileExists { exists },
                ..
            } => Ok(exists),
            other => Err(unexpected("WaitForFile", &other)),
        }
    }

    /// Creates the named event `name`, not yet set, for a process launched
    /// later to set.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn create_event(&mut self, name: &str) -> io::Result<()> {
        match self.request(Request::CreateEvent {
            name: name.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::EventCreated,
                ..
            } => Ok(()),
            other => Err(unexpected("CreateEvent", &other)),
        }
    }

    /// Waits up to `timeout` for the event `name` to be set, or `pid` to
    /// exit first.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn wait_for_event(
        &mut self,
        name: &str,
        pid: u32,
        timeout: Duration,
    ) -> io::Result<EventOutcome> {
        match self.request_waiting(
            Request::WaitForEvent {
                name: name.to_owned(),
                pid,
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::EventWait(outcome),
                ..
            } => Ok(outcome),
            other => Err(unexpected("WaitForEvent", &other)),
        }
    }

    /// The focused element as UI Automation reports it to the agent, read
    /// independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn focused_element(&mut self) -> io::Result<FocusedElement> {
        match self.request(Request::FocusedElement)? {
            Frame::Reply {
                payload: ReplyPayload::Focused(element),
                ..
            } => Ok(element),
            other => Err(unexpected("FocusedElement", &other)),
        }
    }

    /// Focuses the foreground window's element whose UI Automation
    /// identifier is `automation_id`, injecting no input, and returns how
    /// many children it has.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn focus_by_automation_id(&mut self, automation_id: &str) -> io::Result<u32> {
        match self.request(Request::FocusByAutomationId {
            automation_id: automation_id.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::Children { count },
                ..
            } => Ok(count),
            other => Err(unexpected("FocusByAutomationId", &other)),
        }
    }

    /// The words of the focused text that its application marks as
    /// misspelt, as the agent reads them through UI Automation,
    /// independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn misspelt_words(&mut self) -> io::Result<Vec<String>> {
        match self.request(Request::MisspeltWords)? {
            Frame::Reply {
                payload: ReplyPayload::Words { words },
                ..
            } => Ok(words),
            other => Err(unexpected("MisspeltWords", &other)),
        }
    }

    /// Whether the lock key `key` (such as `scrolllock`) is on, as the
    /// agent reads it, independently of Verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn key_toggled(&mut self, key: &str) -> io::Result<bool> {
        match self.request(Request::KeyToggled {
            key: key.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::KeyToggled { on },
                ..
            } => Ok(on),
            other => Err(unexpected("KeyToggled", &other)),
        }
    }

    /// The guest's foreground window and visible top-level windows.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn foreground_info(&mut self) -> io::Result<ForegroundInfo> {
        match self.request(Request::ForegroundInfo)? {
            Frame::Reply {
                payload: ReplyPayload::ForegroundInfo(info),
                ..
            } => Ok(info),
            other => Err(unexpected("ForegroundInfo", &other)),
        }
    }

    /// Asks every visible top-level window whose title contains
    /// `title_contains` to close, waiting up to `timeout`, on window events,
    /// for them to go. Returns how many were still open.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn close_windows(&mut self, title_contains: &str, timeout: Duration) -> io::Result<u32> {
        match self.request_waiting(
            Request::CloseWindows {
                title_contains: title_contains.to_owned(),
                timeout_ms: millis(timeout),
            },
            timeout,
        )? {
            Frame::Reply {
                payload: ReplyPayload::WindowsClosed { remaining },
                ..
            } => Ok(remaining),
            other => Err(unexpected("CloseWindows", &other)),
        }
    }

    /// Writes a small file on the guest, creating or replacing it.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn write_file(&mut self, path: &str, contents: &[u8]) -> io::Result<()> {
        match self.request(Request::WriteFile {
            path: path.to_owned(),
            data_base64: STANDARD.encode(contents),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FileWritten,
                ..
            } => Ok(()),
            other => Err(unexpected("WriteFile", &other)),
        }
    }

    /// Deletes a file on the guest; one already gone is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn delete_file(&mut self, path: &str) -> io::Result<()> {
        match self.request(Request::DeleteFile {
            path: path.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FileDeleted,
                ..
            } => Ok(()),
            other => Err(unexpected("DeleteFile", &other)),
        }
    }

    /// Injects real OS key strokes on the guest, each a plus-joined
    /// combination such as `shift+tab`, each numbered for the harness's
    /// barrier. Returns the last stroke's number.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when a key name is
    /// unknown, in which case nothing was sent.
    pub fn send_keys(&mut self, keys: &[String]) -> io::Result<u64> {
        match self.request(Request::SendKeys {
            keys: keys.to_vec(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::KeysSent { input },
                ..
            } => Ok(input),
            other => Err(unexpected("SendKeys", &other)),
        }
    }

    /// Types `text` on the guest as real key presses, each character mapped
    /// to its key and shift state in the foreground window's keyboard
    /// layout and numbered for the harness's barrier. Returns the last
    /// character's number.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, including when a character
    /// cannot be typed in that layout, in which case nothing was typed.
    pub fn type_text(&mut self, text: &str) -> io::Result<u64> {
        match self.request(Request::TypeText {
            text: text.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::TextTyped { input },
                ..
            } => Ok(input),
            other => Err(unexpected("TypeText", &other)),
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

    /// Reads a guest file of any size, in chunks, into `to` on this machine;
    /// `to` appears only once the whole file has been copied.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails, a reply is not valid base64, or
    /// `to` cannot be written.
    pub fn copy_file(&mut self, path: &str, to: &std::path::Path) -> io::Result<()> {
        // Written beside `to` and renamed once complete, so a failed copy
        // never leaves a partial file under the final name.
        let mut partial = to.as_os_str().to_owned();
        partial.push(".part");
        let partial = std::path::PathBuf::from(partial);
        self.copy_into(path, &partial)?;
        std::fs::rename(&partial, to)
    }

    fn copy_into(&mut self, path: &str, to: &std::path::Path) -> io::Result<()> {
        use std::io::Write;
        let mut out = std::fs::File::create(to)?;
        let mut offset = 0u64;
        loop {
            let chunk = match self.request(Request::ReadFileChunk {
                path: path.to_owned(),
                offset,
            })? {
                Frame::Reply {
                    payload: ReplyPayload::FileContents { data_base64 },
                    ..
                } => STANDARD.decode(data_base64).map_err(io::Error::other)?,
                other => return Err(unexpected("ReadFileChunk", &other)),
            };
            if chunk.is_empty() {
                return Ok(());
            }
            out.write_all(&chunk)?;
            offset += chunk.len() as u64;
        }
    }

    /// Lists the names of the files directly inside a guest directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the directory cannot be
    /// read.
    pub fn list_files(&mut self, path: &str) -> io::Result<Vec<String>> {
        match self.request(Request::ListFiles {
            path: path.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FileNames { names },
                ..
            } => Ok(names),
            other => Err(unexpected("ListFiles", &other)),
        }
    }

    /// Lists the names of the folders directly inside `path` on the guest.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails.
    pub fn list_folders(&mut self, path: &str) -> io::Result<Vec<String>> {
        match self.request(Request::ListFolders {
            path: path.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FileNames { names },
                ..
            } => Ok(names),
            other => Err(unexpected("ListFolders", &other)),
        }
    }

    /// Deletes a folder and everything in it on the guest; one already gone
    /// is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, for example because a process
    /// still has the folder open.
    pub fn delete_folder(&mut self, path: &str) -> io::Result<()> {
        match self.request(Request::DeleteFolder {
            path: path.to_owned(),
        })? {
            Frame::Reply {
                payload: ReplyPayload::FolderDeleted,
                ..
            } => Ok(()),
            other => Err(unexpected("DeleteFolder", &other)),
        }
    }

    /// Asks the agent to stop speaking its own protocol on this connection
    /// and relay Verbatim's control-plane pipe instead, then completes the
    /// control protocol's own `Hello` on the same socket and returns a
    /// ready [`ControlClient`], reading with [`CONTROL_READ_TIMEOUT`].
    ///
    /// # Errors
    ///
    /// Returns an error if the agent cannot open Verbatim's control pipe,
    /// or if the control protocol's own handshake fails.
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
        // nothing it wrote is left stranded in this reader's own buffer.
        drop(self.reader);
        self.stream.set_read_timeout(Some(CONTROL_READ_TIMEOUT))?;
        ControlClient::from_tcp_stream(self.stream)
    }
}

/// `duration` in whole milliseconds, for the protocol.
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn unexpected(request_name: &str, frame: &Frame) -> io::Error {
    io::Error::other(format!("unexpected reply to {request_name}: {frame:?}"))
}
