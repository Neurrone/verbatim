//! The agent's TCP listener: accepts connections, enforces the `Hello`
//! handshake, dispatches every request except
//! [`Request::OpenControlTunnel`] to the matching module, and hands that
//! one off to [`tunnel`] once its pre-tunnel reply is sent.
//!
//! One thread per connection, matching the control-plane server's model;
//! unlike that server, there is no shared registry or broadcast to
//! synchronize, so each connection's handling is entirely self-contained.

use std::io::{self, BufReader};
use std::net::{TcpListener, TcpStream};
use std::thread;

use tracing::warn;
use verbatim_control::protocol::{read_message, write_message};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::protocol::{
    AGENT_PROTOCOL_VERSION, Frame, IGNORE_PIDS_ENV, ReplyPayload, Request, RequestEnvelope,
};
use crate::{
    desktop, events, files, focus, jobs, keys, process, session, terminals, tunnel, typing,
};

/// The number the next key stroke or typed character the agent injects
/// carries for the end-to-end harness (`verbatim_input::harness`), shared by
/// every connection so numbers never repeat while the agent runs.
static NEXT_INPUT: AtomicU64 = AtomicU64::new(1);

/// Accepts connections on `listener` until it errors, spawning a thread
/// per connection. Each connection is pointed at `pipe_name` for
/// [`Request::OpenControlTunnel`].
///
/// Blocks the calling thread; callers that need to keep doing other work
/// (tests, in particular) run this on a background thread.
pub fn serve(listener: &TcpListener, pipe_name: &str) {
    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                let pipe_name = pipe_name.to_owned();
                thread::spawn(move || handle_connection(&stream, &pipe_name));
            }
            Err(error) => {
                warn!(%error, "accept failed; agent TCP listener stopping");
                break;
            }
        }
    }
}

/// Answers a message that failed to parse with a best-effort
/// [`Frame::Error`] naming the failure, correlated to id 0 since the
/// envelope's own id could not be read. An earlier version closed the
/// connection silently instead, which a live debugging session experienced
/// as an empty reply with the only diagnosis in the agent's own log inside
/// the guest.
fn reject_malformed(writer: &mut TcpStream, error: &io::Error) {
    warn!(%error, "malformed agent request; closing connection");
    let _ = write_message(
        writer,
        &Frame::Error {
            to: 0,
            message: format!("malformed request: {error}"),
        },
    );
}

/// Runs the request loop for one connection until the peer closes it, a
/// malformed message arrives (answered by [`reject_malformed`]), the first
/// request is not [`Request::Hello`], or [`Request::OpenControlTunnel`]
/// hands the connection off to [`tunnel::run`].
fn handle_connection(stream: &TcpStream, pipe_name: &str) {
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(error) => {
            warn!(%error, "failed to clone an accepted agent connection");
            return;
        }
    };
    let reader_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            warn!(%error, "failed to clone an accepted agent connection");
            return;
        }
    };
    let mut reader = BufReader::new(reader_stream);

    let mut hello_seen = false;
    loop {
        let envelope: RequestEnvelope = match read_message(&mut reader) {
            Ok(Some(envelope)) => envelope,
            Ok(None) => return,
            Err(error) => {
                reject_malformed(&mut writer, &error);
                return;
            }
        };

        if !hello_seen {
            match envelope.request {
                Request::Hello { protocol_version }
                    if protocol_version == AGENT_PROTOCOL_VERSION =>
                {
                    hello_seen = true;
                    let reply = Frame::Reply {
                        to: envelope.id,
                        payload: ReplyPayload::Hello {
                            protocol_version: AGENT_PROTOCOL_VERSION,
                        },
                    };
                    if write_message(&mut writer, &reply).is_err() {
                        return;
                    }
                    continue;
                }
                Request::Hello { protocol_version } => {
                    let _ = write_message(
                        &mut writer,
                        &Frame::Error {
                            to: envelope.id,
                            message: format!(
                                "agent speaks protocol version {AGENT_PROTOCOL_VERSION}, client offered {protocol_version}"
                            ),
                        },
                    );
                    return;
                }
                _ => {
                    let _ = write_message(
                        &mut writer,
                        &Frame::Error {
                            to: envelope.id,
                            message: "first request on a connection must be Hello".to_owned(),
                        },
                    );
                    return;
                }
            }
        }

        if matches!(envelope.request, Request::OpenControlTunnel) {
            match tunnel::open(pipe_name) {
                Ok(pipe) => {
                    let reply = Frame::Reply {
                        to: envelope.id,
                        payload: ReplyPayload::TunnelReady,
                    };
                    if write_message(&mut writer, &reply).is_err() {
                        return;
                    }
                    tunnel::run(pipe, reader, writer);
                    return;
                }
                Err(error) => {
                    let _ = write_message(
                        &mut writer,
                        &Frame::Error {
                            to: envelope.id,
                            message: format!("failed to open the control-plane pipe: {error}"),
                        },
                    );
                    continue;
                }
            }
        }

        let frame = dispatch(envelope.id, envelope.request);
        if write_message(&mut writer, &frame).is_err() {
            return;
        }
    }
}

/// Dispatches one already-validated (post-`Hello`, non-tunnel) request to
/// a [`Frame`] reply or error.
fn dispatch(id: u64, request: Request) -> Frame {
    match request {
        Request::Hello { .. } => Frame::Reply {
            to: id,
            payload: ReplyPayload::Hello {
                protocol_version: AGENT_PROTOCOL_VERSION,
            },
        },
        request @ Request::LaunchProcess { .. } => launch(id, request),
        Request::KillProcess { pid } => match process::kill(pid) {
            Ok(outcome) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Killed(outcome),
            },
            Err(error) => error_frame(id, &error),
        },
        Request::WriteStdin { pid, text } => match process::write_stdin(pid, &text) {
            Ok(()) => Frame::Reply {
                to: id,
                payload: ReplyPayload::StdinWritten,
            },
            Err(error) => error_frame(id, &error),
        },
        request @ (Request::EndLaunched
        | Request::ChildProcesses { .. }
        | Request::JobExits { .. }
        | Request::WaitForJobEmpty { .. }
        | Request::WaitForExit { .. }
        | Request::WaitForFile { .. }
        | Request::CreateEvent { .. }
        | Request::WaitForEvent { .. }) => process_request(id, request),
        Request::ProcessStatus { pid } => match process::status(pid) {
            Ok(state) => Frame::Reply {
                to: id,
                payload: ReplyPayload::ProcessStatus(state),
            },
            Err(error) => error_frame(id, &error),
        },
        Request::SessionInfo => match session::current() {
            Ok(info) => Frame::Reply {
                to: id,
                payload: ReplyPayload::SessionInfo(info),
            },
            Err(error) => error_frame(id, &io::Error::other(error)),
        },
        Request::ReadFile { path } => match files::read_base64(&path) {
            Ok(data_base64) => Frame::Reply {
                to: id,
                payload: ReplyPayload::FileContents { data_base64 },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::ReadFileChunk { path, offset } => match files::read_chunk_base64(&path, offset) {
            Ok(data_base64) => Frame::Reply {
                to: id,
                payload: ReplyPayload::FileContents { data_base64 },
            },
            Err(error) => error_frame(id, &error),
        },
        request @ (Request::ForegroundInfo
        | Request::CloseWindows { .. }
        | Request::SetForeground { .. }
        | Request::WaitForWindow { .. }
        | Request::MinimizeAll { .. }
        | Request::TakeForeignTerminalWindows
        | Request::WriteFile { .. }
        | Request::DeleteFile { .. }
        | Request::DeleteFolder { .. }) => desktop_request(id, request),
        request @ (Request::FocusedElement
        | Request::FocusByAutomationId { .. }
        | Request::MisspeltWords
        | Request::KeyToggled { .. }) => state_request(id, request),
        Request::ListFiles { path } => list_reply(id, files::list(&path)),
        Request::ListFolders { path } => list_reply(id, files::list_folders(&path)),
        Request::SendKeys { keys } => send_keys(id, &keys),
        Request::TypeText { text } => type_text(id, &text),
        Request::OpenControlTunnel => {
            unreachable!("OpenControlTunnel is handled in handle_connection before dispatch")
        }
    }
}

/// Answers [`Request::LaunchProcess`]: launches the program, and for a
/// launch of Verbatim names the processes it is to ignore, held open with
/// the launch.
fn launch(id: u64, request: Request) -> Frame {
    let Request::LaunchProcess {
        command,
        args,
        working_dir,
        env,
        stderr_to,
        console_title,
        minimized,
        withhold_foreground,
        ignore_foreign_terminals,
        stdin_piped,
    } = request
    else {
        return error_frame(id, &io::Error::other("not a launch request"));
    };
    let mut env = env;
    let held = if ignore_foreign_terminals {
        match process::foreign_terminal_processes() {
            Ok(terminals) => {
                let pids: Vec<String> = terminals.iter().map(|(pid, _)| pid.to_string()).collect();
                env.push((IGNORE_PIDS_ENV.to_owned(), pids.join(",")));
                terminals
            }
            Err(error) => return error_frame(id, &error),
        }
    } else {
        Vec::new()
    };
    let ignored: Vec<u32> = held.iter().map(|(pid, _)| *pid).collect();
    match process::launch(
        &command,
        &args,
        working_dir.as_deref(),
        &env,
        stderr_to.as_deref(),
        console_title.as_deref(),
        (minimized, withhold_foreground, stdin_piped),
    ) {
        Ok((pid, foreground_allowed)) => {
            process::keep_with_launch(pid, held.into_iter().map(|(_, handle)| handle));
            Frame::Reply {
                to: id,
                payload: ReplyPayload::Launched {
                    pid,
                    foreground_allowed,
                    ignored,
                },
            }
        }
        Err(error) => error_frame(id, &error),
    }
}

/// Answers the requests about processes and the evidence they leave:
/// ending the launched children, listing a process's children or a job's
/// exits, and waiting for a process to exit, a file to appear, or an event
/// to be set.
fn process_request(id: u64, request: Request) -> Frame {
    match request {
        Request::EndLaunched => match process::end_launched() {
            Ok(ended) => Frame::Reply {
                to: id,
                payload: ReplyPayload::EndedLaunched { ended },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::ChildProcesses { pid } => match process::child_processes(pid) {
            Ok(processes) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Processes { processes },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::JobExits { pid } => match jobs::exits(pid) {
            Ok(exits) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Exits { exits },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::WaitForJobEmpty { pid, timeout_ms } => {
            match jobs::wait_until_empty(pid, Duration::from_millis(timeout_ms)) {
                Ok(exits) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::Exits { exits },
                },
                Err(error) => error_frame(id, &error),
            }
        }
        Request::WaitForExit { pid, timeout_ms } => {
            match process::wait_for_exit(pid, Duration::from_millis(timeout_ms)) {
                Ok(state) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::ProcessStatus(state),
                },
                Err(error) => error_frame(id, &error),
            }
        }
        Request::WaitForFile { path, timeout_ms } => {
            match files::wait_for(&path, Duration::from_millis(timeout_ms)) {
                Ok(exists) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::FileExists { exists },
                },
                Err(error) => error_frame(id, &error),
            }
        }
        Request::CreateEvent { name } => match events::create(&name) {
            Ok(()) => Frame::Reply {
                to: id,
                payload: ReplyPayload::EventCreated,
            },
            Err(error) => error_frame(id, &error),
        },
        Request::WaitForEvent {
            name,
            pid,
            timeout_ms,
        } => match events::wait(&name, pid, Duration::from_millis(timeout_ms)) {
            Ok(outcome) => Frame::Reply {
                to: id,
                payload: ReplyPayload::EventWait(outcome),
            },
            Err(error) => error_frame(id, &error),
        },
        other => Frame::Error {
            to: id,
            message: format!("not a process request: {other:?}"),
        },
    }
}

/// Answers the requests that read state a test fixes its expectations
/// from, independently of the screen reader under test: the focused
/// element, an element focused by its identifier, the focused text's
/// misspelt words, and a lock key's state.
fn state_request(id: u64, request: Request) -> Frame {
    match request {
        Request::FocusedElement => match focus::focused_element() {
            Ok(element) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Focused(element),
            },
            Err(error) => error_frame(id, &error),
        },
        Request::FocusByAutomationId { automation_id } => {
            match focus::focus_by_automation_id(&automation_id) {
                Ok(count) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::Children { count },
                },
                Err(error) => error_frame(id, &error),
            }
        }
        Request::MisspeltWords => match focus::misspelt_words() {
            Ok(words) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Words { words },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::KeyToggled { key } => match keys::toggled(&key) {
            Ok(on) => Frame::Reply {
                to: id,
                payload: ReplyPayload::KeyToggled { on },
            },
            Err(error) => error_frame(id, &error),
        },
        other => Frame::Error {
            to: id,
            message: format!("not a state request: {other:?}"),
        },
    }
}

/// Answers [`Request::ListFiles`] or [`Request::ListFolders`] with the names
/// listed, or the error listing them.
fn list_reply(id: u64, names: io::Result<Vec<String>>) -> Frame {
    match names {
        Ok(names) => Frame::Reply {
            to: id,
            payload: ReplyPayload::FileNames { names },
        },
        Err(error) => error_frame(id, &error),
    }
}

/// Answers [`Request::SendKeys`], injecting nothing unless every key name
/// parses, each combination numbered for the harness.
fn send_keys(id: u64, keys: &[String]) -> Frame {
    match verbatim_control::send_keys::parse_all(keys)
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))
        .and_then(|combos| {
            let first = NEXT_INPUT.fetch_add(combos.len() as u64, Ordering::Relaxed);
            verbatim_control::send_keys::inject_numbered(&combos, first)
        }) {
        Ok(input) => Frame::Reply {
            to: id,
            payload: ReplyPayload::KeysSent { input },
        },
        Err(error) => error_frame(id, &error),
    }
}

/// Answers [`Request::TypeText`], typing nothing unless every character can
/// be typed.
fn type_text(id: u64, text: &str) -> Frame {
    let first = NEXT_INPUT.fetch_add(text.chars().count() as u64, Ordering::Relaxed);
    match typing::type_text(text, first) {
        Ok(input) => Frame::Reply {
            to: id,
            payload: ReplyPayload::TextTyped { input },
        },
        Err(error) => error_frame(id, &error),
    }
}

/// Answers the requests that read or change the desktop and its files for a
/// test: the foreground report, closing windows, writing and deleting a
/// file, and deleting a folder.
fn desktop_request(id: u64, request: Request) -> Frame {
    match request {
        Request::TakeForeignTerminalWindows => match terminals::take_foreign() {
            Ok(windows) => Frame::Reply {
                to: id,
                payload: ReplyPayload::TerminalWindows { windows },
            },
            Err(error) => error_frame(id, &error),
        },
        Request::ForegroundInfo => Frame::Reply {
            to: id,
            payload: ReplyPayload::ForegroundInfo(desktop::foreground_info()),
        },
        Request::CloseWindows {
            title_contains,
            timeout_ms,
        } => Frame::Reply {
            to: id,
            payload: ReplyPayload::WindowsClosed {
                remaining: desktop::close_windows(
                    &title_contains,
                    Duration::from_millis(timeout_ms),
                ),
            },
        },
        Request::SetForeground { window } => Frame::Reply {
            to: id,
            payload: ReplyPayload::Foreground {
                taken: desktop::set_foreground(window),
            },
        },
        Request::WaitForWindow {
            condition,
            timeout_ms,
        } => {
            let (met, desktop) = desktop::wait_for(&condition, Duration::from_millis(timeout_ms));
            Frame::Reply {
                to: id,
                payload: ReplyPayload::WindowState { met, desktop },
            }
        }
        Request::MinimizeAll { timeout_ms } => {
            let (minimized, desktop_in_front, desktop) =
                desktop::minimize_all(Duration::from_millis(timeout_ms));
            Frame::Reply {
                to: id,
                payload: ReplyPayload::Minimized {
                    minimized,
                    desktop_in_front,
                    desktop,
                },
            }
        }
        Request::WriteFile { path, data_base64 } => {
            match files::write_base64(&path, &data_base64) {
                Ok(()) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::FileWritten,
                },
                Err(error) => error_frame(id, &error),
            }
        }
        Request::DeleteFile { path } => match files::delete(&path) {
            Ok(()) => Frame::Reply {
                to: id,
                payload: ReplyPayload::FileDeleted,
            },
            Err(error) => error_frame(id, &error),
        },
        Request::DeleteFolder { path } => match files::delete_folder(&path) {
            Ok(()) => Frame::Reply {
                to: id,
                payload: ReplyPayload::FolderDeleted,
            },
            Err(error) => error_frame(id, &error),
        },
        other => Frame::Error {
            to: id,
            message: format!("not a desktop request: {other:?}"),
        },
    }
}

fn error_frame(id: u64, error: &io::Error) -> Frame {
    Frame::Error {
        to: id,
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use verbatim_control::client::{Client as ControlClient, ok_or_error};
    use verbatim_control::protocol::{
        Frame as ControlFrame, ReplyPayload as ControlReplyPayload, Request as ControlRequest,
        StatusInfo,
    };
    use verbatim_control::server::{ControlServer, ServerHandlers};
    use verbatim_model::Pid;

    use super::*;
    use crate::protocol::{KillOutcome, ProcessState};

    /// Starts an agent listening on an ephemeral loopback port, pointed at
    /// `pipe_name`, and returns its address. The accept loop runs on a
    /// background thread for the life of the test process (there is no
    /// shutdown hook, matching the other in-process server tests in this
    /// workspace, which rely on process exit to reclaim the thread).
    fn start_agent(pipe_name: &str) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds an ephemeral port");
        let addr = listener.local_addr().expect("has a local address");
        let pipe_name = pipe_name.to_owned();
        thread::spawn(move || serve(&listener, &pipe_name));
        addr
    }

    /// A minimal agent-protocol client for tests: writes a `RequestEnvelope`
    /// and reads back one `Frame`, tracking correlation ids itself.
    struct TestClient {
        stream: TcpStream,
        reader: BufReader<TcpStream>,
        next_id: u64,
    }

    impl TestClient {
        fn connect(addr: std::net::SocketAddr) -> Self {
            let stream = TcpStream::connect(addr).expect("connects to the agent");
            let reader = BufReader::new(stream.try_clone().expect("clones the stream"));
            Self {
                stream,
                reader,
                next_id: 1,
            }
        }

        fn request(&mut self, request: Request) -> Frame {
            let id = self.next_id;
            self.next_id += 1;
            write_message(&mut self.stream, &RequestEnvelope { id, request })
                .expect("writes a request");
            read_message(&mut self.reader)
                .expect("reads a reply")
                .expect("connection stays open")
        }

        /// Asserts that the agent closed the connection: the next read
        /// finds its end.
        fn assert_closed(&mut self) {
            let next: Option<Frame> = read_message(&mut self.reader).expect("reads to the end");
            assert_eq!(next, None, "the agent closes the connection");
        }

        fn hello(&mut self) {
            let frame = self.request(Request::Hello {
                protocol_version: AGENT_PROTOCOL_VERSION,
            });
            assert!(
                matches!(
                    frame,
                    Frame::Reply {
                        payload: ReplyPayload::Hello { .. },
                        ..
                    }
                ),
                "expected a Hello reply, got {frame:?}"
            );
        }
    }

    #[test]
    fn first_request_must_be_hello() {
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-a");
        let mut client = TestClient::connect(addr);
        let frame = client.request(Request::SessionInfo);
        assert_eq!(
            frame,
            Frame::Error {
                to: 1,
                message: "first request on a connection must be Hello".to_owned(),
            }
        );
        client.assert_closed();
    }

    #[test]
    fn malformed_request_gets_an_error_frame_before_the_close() {
        use std::io::Write as _;

        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-f");
        let mut client = TestClient::connect(addr);
        client.hello();

        client
            .stream
            .write_all(b"this is not json\n")
            .expect("writes the malformed line");
        let frame: Frame = read_message(&mut client.reader)
            .expect("reads a frame")
            .expect("an error frame arrives before the connection closes");
        match frame {
            Frame::Error { to, message } => {
                assert_eq!(to, 0, "an unparseable envelope has no id to correlate to");
                assert!(
                    message.contains("malformed request"),
                    "error should name the parse failure, got: {message}"
                );
            }
            other @ Frame::Reply { .. } => panic!("expected an error frame, got {other:?}"),
        }
    }

    #[test]
    fn hello_version_mismatch_is_refused() {
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-b");
        let mut client = TestClient::connect(addr);
        let frame = client.request(Request::Hello {
            protocol_version: AGENT_PROTOCOL_VERSION + 1,
        });
        assert_eq!(
            frame,
            Frame::Error {
                to: 1,
                message: format!(
                    "agent speaks protocol version {AGENT_PROTOCOL_VERSION}, \
                     client offered {}",
                    AGENT_PROTOCOL_VERSION + 1
                ),
            }
        );
        client.assert_closed();
    }

    #[test]
    fn launch_status_kill_wait_lifecycle_over_the_wire() {
        let _launching = crate::process::LAUNCHING
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-c");
        let mut client = TestClient::connect(addr);
        client.hello();

        let launch_reply = client.request(Request::LaunchProcess {
            command: "powershell".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "Wait-Event".to_owned(),
            ],
            working_dir: None,
            env: vec![],
            stderr_to: None,
            console_title: None,
            minimized: false,
            withhold_foreground: false,
            ignore_foreign_terminals: false,
            stdin_piped: false,
        });
        let Frame::Reply {
            payload: ReplyPayload::Launched { pid, .. },
            ..
        } = launch_reply
        else {
            panic!("expected a Launched reply, got {launch_reply:?}");
        };

        let status_reply = client.request(Request::ProcessStatus { pid });
        assert_eq!(
            status_reply,
            Frame::Reply {
                to: 3,
                payload: ReplyPayload::ProcessStatus(ProcessState::Running),
            }
        );

        let kill_reply = client.request(Request::KillProcess { pid });
        assert_eq!(
            kill_reply,
            Frame::Reply {
                to: 4,
                payload: ReplyPayload::Killed(KillOutcome::Terminated),
            }
        );

        let wait_reply = client.request(Request::WaitForExit {
            pid,
            timeout_ms: 30_000,
        });
        assert_eq!(
            wait_reply,
            Frame::Reply {
                to: 5,
                payload: ReplyPayload::ProcessStatus(ProcessState::Exited { exit_code: Some(1) }),
            }
        );
    }

    #[test]
    fn session_info_over_the_wire() {
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-d");
        let mut client = TestClient::connect(addr);
        client.hello();
        let frame = client.request(Request::SessionInfo);
        // The agent runs in this process, so its session is this process's,
        // read here for the expected reply.
        assert_eq!(
            frame,
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::SessionInfo(
                    crate::session::current().expect("reads this process's session")
                ),
            }
        );
    }

    /// A line break is a named key, never typed text, so the request fails
    /// before any key is sent; nothing reaches the desktop the test runs on.
    #[test]
    fn type_text_refuses_text_it_cannot_type_over_the_wire() {
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-h");
        let mut client = TestClient::connect(addr);
        client.hello();
        let frame = client.request(Request::TypeText {
            text: "echo\n".to_owned(),
        });
        match frame {
            Frame::Error { message, .. } => assert!(
                message.contains("control character"),
                "the error names the reason: {message}"
            ),
            other @ Frame::Reply { .. } => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn read_file_over_the_wire() {
        let addr = start_agent(r"\\.\pipe\verbatim-agent-test-unused-e");
        let path = std::env::temp_dir().join("verbatim-agent-server-test.txt");
        std::fs::write(&path, b"over the wire").expect("writes the temp file");

        let mut client = TestClient::connect(addr);
        client.hello();
        let frame = client.request(Request::ReadFile {
            path: path.to_str().expect("utf8 path").to_owned(),
        });
        let Frame::Reply {
            payload: ReplyPayload::FileContents { data_base64 },
            ..
        } = frame
        else {
            panic!("expected a FileContents reply, got {frame:?}");
        };
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data_base64)
            .expect("valid base64");
        assert_eq!(decoded, b"over the wire");

        std::fs::remove_file(&path).ok();
    }

    /// End to end: an agent tunnels a real control-plane connection to a
    /// stub `ControlServer`. Exercises the full handoff — the agent
    /// protocol's `Hello`/`OpenControlTunnel` on a raw `TcpStream`, then
    /// the same stream handed to `verbatim_control::client::Client` to
    /// complete the control protocol's own `Hello` and a `Status` request
    /// against stub handlers.
    #[test]
    fn tunnel_reaches_a_real_control_server() {
        let pipe_name = r"\\.\pipe\verbatim-agent-test-tunnel";
        let handlers = ServerHandlers {
            status: Box::new(|| StatusInfo {
                pid: Pid(4242),
                version: "test".to_owned(),
                active_synth: None,
                outposts: Vec::new(),
                ready: true,
            }),
            send_gesture: Box::new(|_| Ok(())),
            latency: Box::new(|_| Vec::new()),
            dump_tree: Box::new(|| Err("not exercised".to_owned())),
            dump_recorder: Box::new(|| Err("not exercised".to_owned())),
            quit: Box::new(|| {}),
            await_idle: Box::new(|_, _| Ok(())),
            dump_focus: Box::new(|| Err("not exercised".to_owned())),
        };
        let control_server =
            ControlServer::start_on(pipe_name, handlers).expect("starts the control server");

        let addr = start_agent(pipe_name);

        // Raw TcpStream first: complete the agent protocol's Hello and
        // OpenControlTunnel by hand, exactly as a real E2E client would,
        // since `verbatim_control::client::Client` speaks a different
        // protocol and must not be used for this half of the handshake.
        let mut stream = TcpStream::connect(addr).expect("connects to the agent");
        {
            let mut reader = BufReader::new(stream.try_clone().expect("clones the stream"));
            write_message(
                &mut stream,
                &RequestEnvelope {
                    id: 1,
                    request: Request::Hello {
                        protocol_version: AGENT_PROTOCOL_VERSION,
                    },
                },
            )
            .expect("writes agent Hello");
            let hello_reply: Frame = read_message(&mut reader).expect("reads").expect("not EOF");
            assert!(matches!(
                hello_reply,
                Frame::Reply {
                    payload: ReplyPayload::Hello { .. },
                    ..
                }
            ));

            write_message(
                &mut stream,
                &RequestEnvelope {
                    id: 2,
                    request: Request::OpenControlTunnel,
                },
            )
            .expect("writes OpenControlTunnel");
            let tunnel_reply: Frame = read_message(&mut reader).expect("reads").expect("not EOF");
            assert!(
                matches!(
                    tunnel_reply,
                    Frame::Reply {
                        payload: ReplyPayload::TunnelReady,
                        ..
                    }
                ),
                "expected TunnelReady, got {tunnel_reply:?}"
            );
            // `reader`'s BufReader is dropped here having consumed exactly
            // the two reply lines; the underlying socket has no buffered
            // bytes left unread, so handing the bare `stream` to `Client`
            // next is safe.
        }

        let mut control_client =
            ControlClient::from_tcp_stream(stream).expect("completes the control Hello");
        let status_reply = ok_or_error(
            control_client
                .request(ControlRequest::Status)
                .expect("sends Status through the tunnel"),
        )
        .expect("not an error");
        match status_reply {
            ControlFrame::Reply {
                payload: ControlReplyPayload::Status(status),
                ..
            } => assert_eq!(status.pid, Pid(4242)),
            other => panic!("unexpected reply to Status: {other:?}"),
        }

        drop(control_client);
        drop(control_server);
    }
}
