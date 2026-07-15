//! The control-plane server: a named-pipe listener that speaks the v0
//! protocol (architecture section 10, `protocol.rs`).
//!
//! [`ControlServer::start`] runs an accept loop on its own thread, spawning
//! a reader thread and a writer thread per connection. The reader thread
//! parses [`RequestEnvelope`]s and dispatches them against
//! [`ServerHandlers`], the seam that keeps this crate decoupled from the
//! rest of the application. The writer thread owns the pipe's write side
//! and drains a per-connection bounded queue, so a slow or stalled client
//! can never block the app that is broadcasting events and speech
//! ([`ControlServer::broadcast_event`], [`ControlServer::broadcast_speech`]).
//!
//! Every pipe instance is opened with `FILE_FLAG_OVERLAPPED`. A single
//! synchronous (non-overlapped) `HANDLE` serializes its I/O at the driver
//! level: a blocking `ReadFile` pending on one thread measurably blocks a
//! concurrent `WriteFile` on another thread for the same pipe instance,
//! even when the two calls use independent handles obtained via
//! `DuplicateHandle` — confirmed by a minimal repro before this code
//! settled on overlapped I/O. `RawPipe` therefore issues every read, write,
//! and `ConnectNamedPipe` as an overlapped operation and immediately blocks
//! on its completion via `GetOverlappedResult`, so from a caller's
//! perspective every call still behaves synchronously, but the driver
//! tracks the read and write directions independently. One handle is
//! shared between the reader and writer threads (each issues only its own
//! kind of operation on it, with its own dedicated event) — this is
//! Microsoft's documented pattern for one file handle used by multiple
//! threads doing different I/O directions concurrently.
//!
//! The request-dispatch loop ([`run_session`]) is generic over `R: BufRead`
//! and sends every outbound frame through a `Sender<Frame>`, so it can be
//! exercised in tests without a real pipe: feed it a `BufRead` of
//! pre-framed requests (or the reader half of `std::io::pipe`, kept open
//! across several requests) and inspect the frames it produces on a
//! `crossbeam_channel::Receiver<Frame>`. The transport (named pipe,
//! security descriptor, accept loop) is a thin layer on top that the
//! `#[ignore]`d integration test exercises for real.

use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded};
use tracing::warn;
use verbatim_model::{Backend, NormalizedEvent, Pid, SnapshotVersion, TraceId};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
    HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken};
use windows::core::{HRESULT, PCWSTR, PWSTR};

use crate::protocol::{
    Frame, LatencyRecord, PIPE_NAME, PROTOCOL_VERSION, ReplyPayload, Request, RequestEnvelope,
    StatusInfo, read_message, write_message,
};
use crate::send_keys;

/// Depth of the per-connection outbound queue. Broadcast frames beyond this
/// are dropped for that connection (with a `tracing::warn`) rather than
/// backing up; replies are also sent through this queue but are never
/// dropped by application logic (only if the connection is already gone).
const OUTBOUND_QUEUE_DEPTH: usize = 256;

/// Answers [`Request::SendGesture`]; `Err` carries a human-readable reason
/// reported to the client as [`Frame::Error`].
type SendGestureHandler = Box<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

/// Callbacks injected by the application so this crate never depends on it.
///
/// Each field answers one family of [`Request`]s; `ControlServer` calls
/// these from connection threads, never from the accept thread itself, so
/// a slow handler stalls at most one client's requests.
pub struct ServerHandlers {
    /// Answers [`Request::Status`].
    pub status: Box<dyn Fn() -> StatusInfo + Send + Sync>,
    /// Answers [`Request::SendGesture`].
    pub send_gesture: SendGestureHandler,
    /// Answers [`Request::Latency`], given `last_n`.
    pub latency: Box<dyn Fn(u32) -> Vec<LatencyRecord> + Send + Sync>,
    /// Answers [`Request::DumpTree`].
    pub dump_tree: Box<dyn Fn() -> Result<(verbatim_model::TreeNode, bool), String> + Send + Sync>,
    /// Answers [`Request::DumpRecorder`] with the path the dump was written
    /// to.
    pub dump_recorder: Box<dyn Fn() -> Result<String, String> + Send + Sync>,
    /// Answers [`Request::Quit`] by asking the application to exit.
    pub quit: Box<dyn Fn() + Send + Sync>,
}

/// Identifies one live connection for the [`Registry`].
type ConnectionId = u64;

/// What the registry keeps for one live connection: enough for
/// [`ControlServer::broadcast_event`] and [`ControlServer::broadcast_speech`]
/// to reach it, and nothing transport-specific — this is what makes
/// [`run_session`] testable without a real pipe.
#[derive(Clone)]
struct ConnectionEntry {
    outbound: Sender<Frame>,
    events_subscribed: Arc<AtomicBool>,
    speech_subscribed: Arc<AtomicBool>,
}

/// Live connections, shared between the accept loop (which inserts and
/// removes entries) and broadcast calls (which fan out to them).
type Registry = Arc<Mutex<HashMap<ConnectionId, ConnectionEntry>>>;

/// Live pipe instances, transport-specific and separate from [`Registry`]
/// (which stays transport-agnostic so [`run_session`] is testable without a
/// real pipe). Used only so [`ControlServer`]'s `Drop` can force-disconnect
/// every live client.
type Connections = Arc<Mutex<HashMap<ConnectionId, Arc<RawPipe>>>>;

/// Runs the request-dispatch loop for one connection until the peer closes
/// it, a malformed message arrives, or the first request is not
/// [`Request::Hello`].
///
/// Registers a [`ConnectionEntry`] under `conn_id` for the duration of the
/// call (removed again before returning), so broadcasts issued concurrently
/// from other threads reach this connection while it is subscribed.
fn run_session<R: BufRead>(
    mut reader: R,
    conn_id: ConnectionId,
    handlers: &ServerHandlers,
    registry: &Registry,
    outbound: &Sender<Frame>,
) {
    let events_subscribed = Arc::new(AtomicBool::new(false));
    let speech_subscribed = Arc::new(AtomicBool::new(false));
    registry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            conn_id,
            ConnectionEntry {
                outbound: outbound.clone(),
                events_subscribed: Arc::clone(&events_subscribed),
                speech_subscribed: Arc::clone(&speech_subscribed),
            },
        );

    let mut hello_seen = false;
    loop {
        let envelope: RequestEnvelope = match read_message(&mut reader) {
            Ok(Some(envelope)) => envelope,
            Ok(None) => break,
            Err(error) => {
                warn!(%error, "malformed control-plane request; closing connection");
                break;
            }
        };

        if !hello_seen {
            if let Request::Hello { protocol_version } = envelope.request {
                hello_seen = true;
                let negotiated = negotiate_protocol_version(protocol_version);
                let reply = Frame::Reply {
                    to: envelope.id,
                    payload: ReplyPayload::Hello {
                        protocol_version: negotiated,
                    },
                };
                if outbound.send(reply).is_err() {
                    break;
                }
                continue;
            }
            let _ = outbound.send(Frame::Error {
                to: envelope.id,
                message: "first request on a connection must be Hello".to_owned(),
            });
            break;
        }

        let frame = dispatch_request(
            envelope.id,
            envelope.request,
            handlers,
            &events_subscribed,
            &speech_subscribed,
        );
        if outbound.send(frame).is_err() {
            break;
        }
    }

    registry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&conn_id);
}

/// Negotiates the protocol version for a connection: the lower of what the
/// client offers and what this build speaks. `PROTOCOL_VERSION` is `0`
/// today, so this always resolves to `0` and trips clippy's
/// `unnecessary_min_or_max`; the `min` is kept anyway because it becomes
/// real negotiation the moment the protocol version is bumped.
#[expect(
    clippy::unnecessary_min_or_max,
    reason = "PROTOCOL_VERSION is 0 today; this becomes real negotiation once it is bumped"
)]
fn negotiate_protocol_version(client_version: u32) -> u32 {
    client_version.min(PROTOCOL_VERSION)
}

/// Dispatches one already-validated (post-Hello) request to a [`Frame`]
/// reply or error, applying subscription side effects along the way.
fn dispatch_request(
    id: u64,
    request: Request,
    handlers: &ServerHandlers,
    events_subscribed: &AtomicBool,
    speech_subscribed: &AtomicBool,
) -> Frame {
    match request {
        Request::Hello { protocol_version } => Frame::Reply {
            to: id,
            payload: ReplyPayload::Hello {
                protocol_version: negotiate_protocol_version(protocol_version),
            },
        },
        Request::Status => Frame::Reply {
            to: id,
            payload: ReplyPayload::Status((handlers.status)()),
        },
        Request::SubscribeEvents => {
            events_subscribed.store(true, Ordering::Relaxed);
            Frame::Reply {
                to: id,
                payload: ReplyPayload::Ok,
            }
        }
        Request::SubscribeSpeech => {
            speech_subscribed.store(true, Ordering::Relaxed);
            Frame::Reply {
                to: id,
                payload: ReplyPayload::Ok,
            }
        }
        Request::SendGesture { identifier } => match (handlers.send_gesture)(&identifier) {
            Ok(()) => Frame::Reply {
                to: id,
                payload: ReplyPayload::Ok,
            },
            Err(message) => Frame::Error { to: id, message },
        },
        Request::SendKeys { keys } => match send_keys::parse_all(&keys) {
            Ok(combos) => match send_keys::inject(&combos) {
                Ok(()) => Frame::Reply {
                    to: id,
                    payload: ReplyPayload::Ok,
                },
                Err(error) => Frame::Error {
                    to: id,
                    message: error.to_string(),
                },
            },
            Err(message) => Frame::Error { to: id, message },
        },
        Request::Latency { last_n } => Frame::Reply {
            to: id,
            payload: ReplyPayload::Latency((handlers.latency)(last_n)),
        },
        Request::DumpTree => match (handlers.dump_tree)() {
            Ok((root, truncated)) => Frame::Reply {
                to: id,
                payload: ReplyPayload::DumpTree { root, truncated },
            },
            Err(message) => Frame::Error { to: id, message },
        },
        Request::DumpRecorder => match (handlers.dump_recorder)() {
            Ok(path) => Frame::Reply {
                to: id,
                payload: ReplyPayload::DumpRecorder { path },
            },
            Err(message) => Frame::Error { to: id, message },
        },
        Request::Quit => {
            (handlers.quit)();
            Frame::Reply {
                to: id,
                payload: ReplyPayload::Ok,
            }
        }
    }
}

/// A raw named-pipe instance, closed and disconnected on drop.
///
/// Holds *two* handles to the same pipe instance: `read_handle` (the
/// original, also used for `ConnectNamedPipe`/`DisconnectNamedPipe`) and
/// `write_handle` (a duplicate). Read and write happen from different
/// threads (the connection's reader and writer threads); a synchronous
/// (non-overlapped) `HANDLE` only supports one in-flight I/O operation at a
/// time, so issuing a blocking `ReadFile` on one thread and a blocking
/// `WriteFile` on another concurrently *on the same handle value* can block
/// one behind the other. Two independent handles to the same instance (via
/// `DuplicateHandle`) avoid that entirely; disconnecting through either one
/// tears down the whole instance.
struct RawPipe {
    handle: HANDLE,
    /// Dedicated to `ConnectNamedPipe` (issued once, from the accept
    /// thread, before handoff) and every subsequent `ReadFile` (issued
    /// serially thereafter from the connection's reader thread). One event
    /// per direction is required: overlapped operations on the same handle
    /// in different directions can be genuinely concurrent, and each needs
    /// its own completion signal.
    read_event: HANDLE,
    /// Dedicated to every `WriteFile`, issued serially from the
    /// connection's writer thread.
    write_event: HANDLE,
}

// Safety: `HANDLE` is a plain kernel handle value. `handle` is used from
// two threads (the connection's reader and writer), but each issues only
// its own direction of I/O (`ReadFile`/`ConnectNamedPipe` versus
// `WriteFile`) with its own `OVERLAPPED`/event — Microsoft's documented
// pattern for one handle shared by threads that each perform a different
// kind of operation on it. `read_event` and `write_event` are likewise
// used from exactly one of those threads each.
unsafe impl Send for RawPipe {}
unsafe impl Sync for RawPipe {}

impl RawPipe {
    /// Connects this instance, blocking the calling thread until a client
    /// attaches. Uses `read_event` since it runs before handoff to the
    /// reader thread, which reuses the same event for its `ReadFile` calls.
    fn connect(&self) -> windows::core::Result<()> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..OVERLAPPED::default()
        };
        // Safety: `overlapped` is valid for the duration of this call and
        // is not read again after `wait_overlapped` returns.
        let result = unsafe { ConnectNamedPipe(self.handle, Some(&raw mut overlapped)) };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
                let mut transferred = 0u32;
                // Safety: `overlapped` is still valid (same stack frame,
                // not yet returned); blocks until the pending connect
                // completes.
                unsafe {
                    GetOverlappedResult(
                        self.handle,
                        &raw const overlapped,
                        &raw mut transferred,
                        true,
                    )
                }
            }
            Err(error) => Err(error),
        }
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..OVERLAPPED::default()
        };
        let mut read = 0u32;
        // Safety: `buf` and `overlapped` are valid, exclusively-borrowed
        // for the duration of this call (including the blocking wait
        // below); this thread is the only one that ever issues `ReadFile`
        // on this handle.
        let result = unsafe {
            ReadFile(
                self.handle,
                Some(buf),
                Some(&raw mut read),
                Some(&raw mut overlapped),
            )
        };
        if let Err(error) = &result {
            if error.code() == HRESULT::from_win32(ERROR_BROKEN_PIPE.0) {
                return Ok(0);
            }
            if error.code() != HRESULT::from_win32(ERROR_IO_PENDING.0) {
                return Err(io::Error::other(error.clone()));
            }
        }
        let mut transferred = 0u32;
        // Safety: `overlapped` is still valid; blocks until the read (which
        // may have already completed above) finishes.
        match unsafe {
            GetOverlappedResult(
                self.handle,
                &raw const overlapped,
                &raw mut transferred,
                true,
            )
        } {
            Ok(()) => Ok(transferred as usize),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_BROKEN_PIPE.0) => Ok(0),
            Err(error) => Err(io::Error::other(error)),
        }
    }

    fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let mut overlapped = OVERLAPPED {
                hEvent: self.write_event,
                ..OVERLAPPED::default()
            };
            let mut written = 0u32;
            // Safety: `buf` and `overlapped` are valid for the duration of
            // this call (including the blocking wait below); this thread
            // is the only one that ever issues `WriteFile` on this handle.
            let result = unsafe {
                WriteFile(
                    self.handle,
                    Some(buf),
                    Some(&raw mut written),
                    Some(&raw mut overlapped),
                )
            };
            if let Err(error) = &result
                && error.code() != HRESULT::from_win32(ERROR_IO_PENDING.0)
            {
                return Err(io::Error::other(error.clone()));
            }
            let mut transferred = 0u32;
            // Safety: `overlapped` is still valid; blocks until the write
            // (which may have already completed above) finishes.
            unsafe {
                GetOverlappedResult(
                    self.handle,
                    &raw const overlapped,
                    &raw mut transferred,
                    true,
                )
            }
            .map_err(io::Error::other)?;
            buf = &buf[transferred as usize..];
        }
        Ok(())
    }

    /// Forces any blocked `ReadFile`/`WriteFile` on this instance (on any
    /// thread) to fail, and disconnects the client.
    fn disconnect(&self) {
        // Safety: `self.handle` is a valid named-pipe server handle.
        unsafe {
            let _ = DisconnectNamedPipe(self.handle);
        }
    }
}

impl Drop for RawPipe {
    fn drop(&mut self) {
        self.disconnect();
        // Safety: `handle`, `read_event`, and `write_event` are all owned
        // exclusively by this `RawPipe` and not used again after this
        // point.
        unsafe {
            let _ = CloseHandle(self.handle);
            let _ = CloseHandle(self.read_event);
            let _ = CloseHandle(self.write_event);
        }
    }
}

/// A `Read` view of one end of a [`RawPipe`], for wrapping in `BufReader`.
struct PipeReader(Arc<RawPipe>);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

/// A `Write` view of one end of a [`RawPipe`], owned by the writer thread.
struct PipeWriter(Arc<RawPipe>);

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Owns a security descriptor allocated by
/// `ConvertStringSecurityDescriptorToSecurityDescriptorW`, freed with
/// `LocalFree` on drop.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// Safety: the underlying block is heap memory with no thread affinity; it
// is only ever read by `CreateNamedPipeW`.
unsafe impl Send for SecurityDescriptor {}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // Safety: `self.0` was allocated by
            // `ConvertStringSecurityDescriptorToSecurityDescriptorW`, which
            // documents `LocalFree` as the correct release call.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }
}

/// Renders the current process token's user SID as a string, for building
/// the owner-only SDDL security descriptor below.
fn current_user_sid_string() -> io::Result<String> {
    // Safety: `GetCurrentProcess` returns a pseudo-handle valid for the
    // lifetime of the process; no cleanup is required for it.
    let process = unsafe { GetCurrentProcess() };
    let mut token = HANDLE::default();
    // Safety: `token` is a valid out-pointer for the duration of the call.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) }.map_err(io::Error::other)?;

    let result = (|| {
        let mut needed = 0u32;
        // Safety: a null buffer with `needed` as the size out-pointer is
        // the documented way to ask `GetTokenInformation` for the required
        // buffer size; it is expected to report `ERROR_INSUFFICIENT_BUFFER`.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &raw mut needed) };
        // `Vec<u64>`, not `Vec<u8>`, so the buffer is 8-byte aligned: it is
        // read back below as a `TOKEN_USER`, which contains a pointer-sized
        // field and would otherwise be under-aligned.
        let mut buffer = vec![0u64; needed.div_ceil(8) as usize];
        // Safety: `buffer` is sized to `needed` (rounded up) as reported
        // above, and valid for writes of that length.
        unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &raw mut needed,
            )
        }
        .map_err(io::Error::other)?;

        // Safety: `buffer` was filled by `GetTokenInformation` above with a
        // `TOKEN_USER` (guaranteed by the `TokenUser` information class),
        // is large enough because we sized it from that same call, and is
        // suitably aligned because it is backed by a `Vec<u64>`.
        let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        let mut sid_string = PWSTR::null();
        // Safety: `token_user.User.Sid` is a valid SID for as long as
        // `buffer` is alive, which outlives this call.
        unsafe { ConvertSidToStringSidW(token_user.User.Sid, &raw mut sid_string) }
            .map_err(io::Error::other)?;
        // Safety: `sid_string` was just allocated by the call above and is
        // a valid, null-terminated wide string.
        let rendered = unsafe { sid_string.to_string() }.map_err(io::Error::other);
        // Safety: `sid_string` was allocated by `ConvertSidToStringSidW`,
        // which documents `LocalFree` as the correct release call.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid_string.0.cast())));
        }
        rendered
    })();

    // Safety: `token` was opened by `OpenProcessToken` above and is not
    // used again after this point.
    unsafe {
        let _ = CloseHandle(token);
    }
    result
}

/// Builds a security descriptor granting full access only to the current
/// interactive user, via SDDL (`ConvertStringSecurityDescriptorToSecurityDescriptorW`
/// with `D:P(A;;GA;;;<sid>)`): a protected DACL containing one ACE that
/// grants `GENERIC_ALL` to the owning user's SID and nobody else. This is
/// documented as equivalent to, and simpler than, building the ACL by hand
/// with `InitializeSecurityDescriptor`/`SetSecurityDescriptorDacl`.
fn build_owner_only_security_descriptor() -> io::Result<SecurityDescriptor> {
    let sid = current_user_sid_string()?;
    let sddl = format!("D:P(A;;GA;;;{sid})");
    let wide: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // Safety: `wide` is a valid, null-terminated wide string for the
    // duration of this call; `descriptor` is a valid out-pointer.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &raw mut descriptor,
            None,
        )
    }
    .map_err(io::Error::other)?;
    Ok(SecurityDescriptor(descriptor))
}

/// Creates one named-pipe server instance, ready to accept a connection.
fn create_pipe_instance(pipe_name: &str, security: &SecurityDescriptor) -> io::Result<RawPipe> {
    let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .expect("SECURITY_ATTRIBUTES size fits in u32"),
        lpSecurityDescriptor: security.0.0,
        bInheritHandle: false.into(),
    };
    // Safety: `wide` is a valid, null-terminated wide string; `attributes`
    // is valid for the duration of this call and points at a security
    // descriptor kept alive by the caller. `FILE_FLAG_OVERLAPPED` is set
    // (see this module's doc comment for why: a synchronous handle
    // serializes reads and writes at the driver level even across
    // independent handles to the same instance).
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    open_mode.0 |= FILE_FLAG_OVERLAPPED.0;
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(wide.as_ptr()),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            4096,
            0,
            Some(&raw const attributes),
        )
    };
    if handle.is_invalid() {
        // Safety: trivially safe; no preconditions.
        let error = unsafe { GetLastError() };
        return Err(io::Error::other(format!(
            "CreateNamedPipeW failed: {error:?}"
        )));
    }

    let create_event = || -> io::Result<HANDLE> {
        // Safety: manual-reset, initially-unsignaled, unnamed event; no
        // preconditions.
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(io::Error::other)
    };
    let read_event = match create_event() {
        Ok(event) => event,
        Err(error) => {
            // Safety: `handle` is not used again on this path.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err(error);
        }
    };
    let write_event = match create_event() {
        Ok(event) => event,
        Err(error) => {
            // Safety: neither handle is used again on this path.
            unsafe {
                let _ = CloseHandle(handle);
                let _ = CloseHandle(read_event);
            }
            return Err(error);
        }
    };

    Ok(RawPipe {
        handle,
        read_event,
        write_event,
    })
}

/// Spawns the reader and writer threads for one freshly connected pipe
/// instance, registering it in `registry` for the duration of the session
/// and in `connections` for the duration of the pipe instance's lifetime
/// (the latter is removed by the reader thread once `run_session` returns).
fn spawn_connection(
    raw: RawPipe,
    conn_id: ConnectionId,
    handlers: Arc<ServerHandlers>,
    registry: Registry,
    connections: Connections,
) {
    let raw = Arc::new(raw);
    connections
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(conn_id, Arc::clone(&raw));

    let (outbound_tx, outbound_rx) = bounded::<Frame>(OUTBOUND_QUEUE_DEPTH);

    let writer_raw = Arc::clone(&raw);
    thread::Builder::new()
        .name(format!("verbatim-control-writer-{conn_id}"))
        .spawn(move || run_writer(PipeWriter(writer_raw), &outbound_rx))
        .expect("spawning the control-plane writer thread");

    thread::Builder::new()
        .name(format!("verbatim-control-reader-{conn_id}"))
        .spawn(move || {
            let reader = io::BufReader::new(PipeReader(raw));
            run_session(reader, conn_id, &handlers, &registry, &outbound_tx);
            connections
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&conn_id);
        })
        .expect("spawning the control-plane reader thread");
}

/// Drains outbound frames onto the wire until the channel disconnects
/// (every sender, including the reader thread's and the registry's own
/// clone, has been dropped) or a write fails (the client is gone).
fn run_writer<W: Write>(mut writer: W, outbound_rx: &Receiver<Frame>) {
    while let Ok(frame) = outbound_rx.recv() {
        if write_message(&mut writer, &frame).is_err() {
            break;
        }
    }
}

/// Accepts connections until `shutdown` is set, spawning a reader and
/// writer thread pair for each.
fn accept_loop(
    pipe_name: &str,
    security: &SecurityDescriptor,
    shutdown: &AtomicBool,
    registry: &Registry,
    connections: &Connections,
    handlers: &Arc<ServerHandlers>,
    next_conn_id: &AtomicU64,
) {
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let raw = match create_pipe_instance(pipe_name, security) {
            Ok(raw) => raw,
            Err(error) => {
                warn!(%error, "failed to create a control-plane pipe instance; accept loop stopping");
                break;
            }
        };

        if raw.connect().is_err() {
            // The instance could not be connected (or shutdown tore it
            // down first via a dummy connect); try again unless shutting
            // down.
            drop(raw);
            if shutdown.load(Ordering::Acquire) {
                break;
            }
            continue;
        }

        if shutdown.load(Ordering::Acquire) {
            // This connection is either shutdown's own dummy client or a
            // real one racing the shutdown; either way, stop accepting.
            drop(raw);
            break;
        }

        let conn_id = next_conn_id.fetch_add(1, Ordering::Relaxed);
        spawn_connection(
            raw,
            conn_id,
            Arc::clone(handlers),
            Arc::clone(registry),
            Arc::clone(connections),
        );
    }
}

/// A running control-plane server: one named-pipe listener plus its live
/// connections.
///
/// Dropping it stops accepting new connections and disconnects every live
/// client (architecture section 10's "no control plane at all in
/// `--secure` instances" and ordinary process shutdown both go through
/// this path).
pub struct ControlServer {
    pipe_name: String,
    shutdown: Arc<AtomicBool>,
    registry: Registry,
    connections: Connections,
    accept_thread: Option<JoinHandle<()>>,
}

impl ControlServer {
    /// Starts the server on the well-known [`PIPE_NAME`].
    ///
    /// # Errors
    ///
    /// Returns an error if the owner-only security descriptor cannot be
    /// built or the accept thread cannot be spawned.
    pub fn start(handlers: ServerHandlers) -> io::Result<Self> {
        Self::start_on(PIPE_NAME, handlers)
    }

    /// Starts the server on `pipe_name`, overridable so tests do not
    /// contend with a real, already-running Verbatim instance.
    ///
    /// # Errors
    ///
    /// Returns an error if the owner-only security descriptor cannot be
    /// built or the accept thread cannot be spawned.
    pub fn start_on(pipe_name: &str, handlers: ServerHandlers) -> io::Result<Self> {
        let security = build_owner_only_security_descriptor()?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));
        let handlers = Arc::new(handlers);
        let next_conn_id = Arc::new(AtomicU64::new(0));
        let pipe_name_owned = pipe_name.to_owned();

        let accept_thread = {
            let shutdown = Arc::clone(&shutdown);
            let registry = Arc::clone(&registry);
            let connections = Arc::clone(&connections);
            let pipe_name = pipe_name_owned.clone();
            thread::Builder::new()
                .name("verbatim-control-accept".to_owned())
                .spawn(move || {
                    accept_loop(
                        &pipe_name,
                        &security,
                        &shutdown,
                        &registry,
                        &connections,
                        &handlers,
                        &next_conn_id,
                    );
                })
                .map_err(io::Error::other)?
        };

        Ok(Self {
            pipe_name: pipe_name_owned,
            shutdown,
            registry,
            connections,
            accept_thread: Some(accept_thread),
        })
    }

    /// Fans a normalized accessibility event out to every connection
    /// subscribed via [`Request::SubscribeEvents`].
    pub fn broadcast_event(
        &self,
        trace_id: TraceId,
        source: Pid,
        backend: Backend,
        version: SnapshotVersion,
        event: NormalizedEvent,
    ) {
        let frame = Frame::Event {
            trace_id,
            source,
            backend,
            version,
            event,
        };
        self.fan_out(&frame, |entry| {
            entry.events_subscribed.load(Ordering::Relaxed)
        });
    }

    /// Fans a captured utterance out to every connection subscribed via
    /// [`Request::SubscribeSpeech`].
    pub fn broadcast_speech(
        &self,
        trace_id: TraceId,
        text: String,
        event_observed_at_ms: Option<u64>,
        queued_at_ms: u64,
        audio_started_at_ms: Option<u64>,
    ) {
        let frame = Frame::Speech {
            trace_id,
            text,
            event_observed_at_ms,
            queued_at_ms,
            audio_started_at_ms,
        };
        self.fan_out(&frame, |entry| {
            entry.speech_subscribed.load(Ordering::Relaxed)
        });
    }

    /// Fans a [`Frame::SpeechFinished`] out to every speech subscriber,
    /// marking that `trace_id`'s audio has finished playing.
    pub fn broadcast_speech_finished(&self, trace_id: TraceId) {
        let frame = Frame::SpeechFinished { trace_id };
        self.fan_out(&frame, |entry| {
            entry.speech_subscribed.load(Ordering::Relaxed)
        });
    }

    fn fan_out(&self, frame: &Frame, subscribed: impl Fn(&ConnectionEntry) -> bool) {
        let registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        for entry in registry.values() {
            if !subscribed(entry) {
                continue;
            }
            if let Err(TrySendError::Full(_)) = entry.outbound.try_send(frame.clone()) {
                warn!("dropping a control-plane frame for a slow client");
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);

        // Kick the accept thread out of its blocking `ConnectNamedPipe`
        // wait: connecting a throwaway client causes that call to return,
        // at which point the accept loop observes `shutdown` and exits
        // without spawning a session for it. The accept loop only ever has
        // one instance listening at a time, so a connect attempt reaches
        // it; retried briefly in case it lands in the short gap between
        // one instance connecting and the next being created.
        for _ in 0..20 {
            if std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.pipe_name)
                .is_ok()
            {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }

        if let Some(thread) = self.accept_thread.take() {
            let _ = thread.join();
        }

        // Disconnecting every live client forces its reader/writer
        // threads' blocking `ReadFile`/`WriteFile` calls to fail, so those
        // threads exit on their own; we do not join them here, since they
        // hold no resource this process needs back synchronously.
        let connections = self
            .connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for raw in connections.values() {
            raw.disconnect();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use verbatim_model::NodeId;

    use super::*;

    /// Handlers with fixed, predictable answers for dispatch tests.
    fn test_handlers() -> ServerHandlers {
        ServerHandlers {
            status: Box::new(|| StatusInfo {
                pid: Pid(4242),
                version: "test".to_owned(),
                active_synth: None,
                outposts: Vec::new(),
            }),
            send_gesture: Box::new(|identifier| {
                if identifier == "kb:verbatim+v" {
                    Ok(())
                } else {
                    Err(format!("unbound gesture: {identifier}"))
                }
            }),
            latency: Box::new(|_last_n| Vec::new()),
            dump_tree: Box::new(|| Err("dump_tree not exercised by this test".to_owned())),
            dump_recorder: Box::new(|| Err("dump_recorder not exercised by this test".to_owned())),
            quit: Box::new(|| {}),
        }
    }

    /// Reads the next frame with a generous timeout, so a stuck dispatch
    /// fails the test instead of hanging the suite.
    fn recv_frame(outbound_rx: &Receiver<Frame>) -> Frame {
        outbound_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a frame within the timeout")
    }

    // Exercises `run_session` directly (no real pipe): the reader half of
    // `std::io::pipe` stands in for the client-to-server direction, kept
    // open across several requests so the session stays live while we
    // interleave a direct broadcast against the registry it populated —
    // exactly what `ControlServer::broadcast_event` does over a real
    // connection.
    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "a single connected session exercising several request kinds in sequence"
    )]
    fn session_dispatches_requests_and_delivers_a_broadcast_to_a_subscriber() {
        let (reader, mut writer) = io::pipe().expect("creates an in-process pipe pair");
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        let handlers = test_handlers();
        let (outbound_tx, outbound_rx) = bounded::<Frame>(OUTBOUND_QUEUE_DEPTH);

        let session_registry = Arc::clone(&registry);
        let session = thread::spawn(move || {
            run_session(
                io::BufReader::new(reader),
                1,
                &handlers,
                &session_registry,
                &outbound_tx,
            );
        });

        write_message(
            &mut writer,
            &RequestEnvelope {
                id: 1,
                request: Request::Hello {
                    protocol_version: 0,
                },
            },
        )
        .expect("writes Hello");
        assert_eq!(
            recv_frame(&outbound_rx),
            Frame::Reply {
                to: 1,
                payload: ReplyPayload::Hello {
                    protocol_version: 0
                },
            }
        );

        write_message(
            &mut writer,
            &RequestEnvelope {
                id: 2,
                request: Request::Status,
            },
        )
        .expect("writes Status");
        match recv_frame(&outbound_rx) {
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::Status(status),
            } => assert_eq!(status.pid, Pid(4242)),
            other => panic!("unexpected reply to Status: {other:?}"),
        }

        write_message(
            &mut writer,
            &RequestEnvelope {
                id: 3,
                request: Request::SubscribeEvents,
            },
        )
        .expect("writes SubscribeEvents");
        assert_eq!(
            recv_frame(&outbound_rx),
            Frame::Reply {
                to: 3,
                payload: ReplyPayload::Ok,
            }
        );

        // The session registered itself under connection id 1; broadcast
        // against the registry directly, the same call
        // `ControlServer::broadcast_event` makes.
        let trace_id = TraceId::mint();
        {
            let registry = registry.lock().unwrap_or_else(PoisonError::into_inner);
            let entry = registry.get(&1).expect("connection is registered");
            assert!(entry.events_subscribed.load(Ordering::Relaxed));
            entry
                .outbound
                .try_send(Frame::Event {
                    trace_id,
                    source: Pid(999),
                    backend: Backend::Uia,
                    version: SnapshotVersion(1),
                    event: NormalizedEvent::ValueChanged {
                        node_id: NodeId::new(1),
                        value: Some("hello".to_owned()),
                    },
                })
                .expect("queues onto the connection's outbound channel");
        }
        match recv_frame(&outbound_rx) {
            Frame::Event { trace_id: got, .. } => assert_eq!(got, trace_id),
            other => panic!("unexpected frame: {other:?}"),
        }

        write_message(
            &mut writer,
            &RequestEnvelope {
                id: 4,
                request: Request::SendGesture {
                    identifier: "kb:unbound".to_owned(),
                },
            },
        )
        .expect("writes SendGesture");
        match recv_frame(&outbound_rx) {
            Frame::Error { to: 4, .. } => {}
            other => panic!("expected an error reply for an unbound gesture: {other:?}"),
        }

        drop(writer);
        session.join().expect("session thread does not panic");
        assert!(
            registry
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "the connection entry is removed once the session ends"
        );
    }

    #[test]
    fn first_request_must_be_hello() {
        let (reader, mut writer) = io::pipe().expect("creates an in-process pipe pair");
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        let handlers = test_handlers();
        let (outbound_tx, outbound_rx) = bounded::<Frame>(OUTBOUND_QUEUE_DEPTH);

        let session = thread::spawn(move || {
            run_session(
                io::BufReader::new(reader),
                1,
                &handlers,
                &registry,
                &outbound_tx,
            );
        });

        write_message(
            &mut writer,
            &RequestEnvelope {
                id: 1,
                request: Request::Status,
            },
        )
        .expect("writes Status");
        match recv_frame(&outbound_rx) {
            Frame::Error { to: 1, .. } => {}
            other => panic!("expected an error for a non-Hello first request: {other:?}"),
        }

        drop(writer);
        session.join().expect("session thread does not panic");
    }

    // Exercises the real named-pipe transport end to end: a distinct test
    // pipe name so it never collides with an already-running Verbatim
    // instance. Run explicitly (`cargo test -p verbatim-control -- --ignored`)
    // since it is unsuited to unattended CI parallelism (one pipe name, one
    // process).
    #[test]
    #[ignore = "starts a real named pipe; run explicitly, not part of the default suite"]
    fn integration_hello_status_subscribe_and_broadcast_over_a_real_pipe() {
        let pipe_name = r"\\.\pipe\verbatim-control-test-ws-e";
        let server = ControlServer::start_on(pipe_name, test_handlers()).expect("starts");

        // The accept thread creates its first pipe instance asynchronously;
        // retry briefly rather than racing it.
        let mut client = None;
        for _ in 0..100 {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(pipe_name)
            {
                Ok(opened) => {
                    client = Some(opened);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut client = client.expect("connects to the test pipe within the retry window");
        let mut client_reader =
            io::BufReader::new(client.try_clone().expect("duplicates the pipe handle"));

        write_message(
            &mut client,
            &RequestEnvelope {
                id: 1,
                request: Request::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
            },
        )
        .expect("writes Hello");
        let reply: Frame = read_message(&mut client_reader)
            .expect("reads")
            .expect("not EOF");
        assert_eq!(
            reply,
            Frame::Reply {
                to: 1,
                payload: ReplyPayload::Hello {
                    protocol_version: PROTOCOL_VERSION,
                },
            }
        );

        write_message(
            &mut client,
            &RequestEnvelope {
                id: 2,
                request: Request::Status,
            },
        )
        .expect("writes Status");
        let reply: Frame = read_message(&mut client_reader)
            .expect("reads")
            .expect("not EOF");
        assert!(matches!(
            reply,
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::Status(_),
            }
        ));

        write_message(
            &mut client,
            &RequestEnvelope {
                id: 3,
                request: Request::SubscribeEvents,
            },
        )
        .expect("writes SubscribeEvents");
        let reply: Frame = read_message(&mut client_reader)
            .expect("reads")
            .expect("not EOF");
        assert_eq!(
            reply,
            Frame::Reply {
                to: 3,
                payload: ReplyPayload::Ok,
            }
        );

        let trace_id = TraceId::mint();
        server.broadcast_event(
            trace_id,
            Pid(999),
            Backend::Msaa,
            SnapshotVersion(1),
            NormalizedEvent::ValueChanged {
                node_id: NodeId::new(1),
                value: Some("hi".to_owned()),
            },
        );

        let event_frame: Frame = read_message(&mut client_reader)
            .expect("reads")
            .expect("not EOF");
        match event_frame {
            Frame::Event { trace_id: got, .. } => assert_eq!(got, trace_id),
            other => panic!("unexpected frame: {other:?}"),
        }

        drop(client_reader);
        drop(client);
        drop(server);
    }
}
