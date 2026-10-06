//! [`crate::protocol::Request::OpenControlTunnel`]: once acknowledged, the
//! connection stops being an agent-protocol message stream and becomes a
//! raw byte pipe between the TCP client and Verbatim's control-plane named
//! pipe, which deliberately never listens on the network itself.
//!
//! Opening the pipe ([`open`]) is split from running the relay ([`run`])
//! so the caller (`server::handle_connection`) can report a failure to
//! open in the pre-tunnel reply, per the protocol's contract, before
//! committing the connection to raw byte relaying.
//!
//! The pipe side uses overlapped I/O for the same reason
//! `verbatim_control::server`'s pipe transport does (see that module's
//! doc comment): a synchronous (non-overlapped) pipe handle serializes
//! reads and writes at the driver level, so a blocking read pending on one
//! thread would stall a concurrent write on another, even across
//! independent handles to the same instance. A full-duplex tunnel needs
//! one thread reading and another writing at the same time, so the pipe
//! handle here is opened with `FILE_FLAG_OVERLAPPED` and each direction
//! uses its own event, exactly as the server does on its side of the same
//! pipe.

use std::io::{self, BufReader, Read, Write as _};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::thread;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_NOT_CONNECTED, HANDLE, WAIT_EVENT, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_MODE,
    OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::{HRESULT, PCWSTR};

/// Opens `pipe_name`, ready for [`run`].
///
/// # Errors
///
/// Returns an error if the pipe cannot be opened (Verbatim is not running,
/// or the pipe name is wrong).
pub(crate) fn open(pipe_name: &str) -> io::Result<OverlappedPipe> {
    OverlappedPipe::open(pipe_name)
}

/// Relays bytes between `pipe` and the TCP connection (`reader`, still
/// possibly holding buffered bytes read before the tunnel handoff, and
/// `writer`, a clone of the same socket) in both directions until either
/// side closes. Logs the per-direction byte totals and why each direction
/// ended when the tunnel closes: with two independent relays and three
/// processes in the path, a "client heard nothing" report is otherwise
/// unattributable — the counts say whether the bytes ever left the pipe,
/// and the reasons tell a clean close from a failure.
///
/// Whichever direction ends first ends the other. The TCP side is shut
/// down, which ends a blocked socket read; the pipe is stopped
/// ([`OverlappedPipe::stop`]), which ends a pipe read or write whether it
/// is already waiting or not yet issued, since a stop stays in effect.
pub(crate) fn run(pipe: OverlappedPipe, reader: BufReader<TcpStream>, writer: TcpStream) {
    let pipe = Arc::new(pipe);
    // A clone of the socket kept on this thread purely so it can be shut
    // down from here once the pipe-to-tcp direction ends, unblocking the
    // tcp-to-pipe thread's blocking read below.
    let Ok(shutdown_handle) = reader.get_ref().try_clone() else {
        return;
    };

    let to_pipe = {
        let pipe = Arc::clone(&pipe);
        thread::spawn(move || {
            let (bytes, ended) = copy_tcp_to_pipe(reader, &pipe);
            // The client is done sending (EOF) or gone (error): end the
            // pipe-to-tcp direction too.
            pipe.stop();
            (bytes, ended)
        })
    };

    let (to_tcp_bytes, to_tcp_ended) = copy_pipe_to_tcp(&pipe, writer);
    // Verbatim closed the pipe, or the pipe errored: end the tcp-to-pipe
    // direction too, whether it is reading the socket or writing the pipe.
    let _ = shutdown_handle.shutdown(Shutdown::Both);
    pipe.stop();

    let (to_pipe_bytes, to_pipe_ended) = to_pipe
        .join()
        .unwrap_or_else(|_| (0, "the relay thread panicked".to_owned()));
    eprintln!(
        "verbatim-agent: control tunnel closed; relayed {to_pipe_bytes} bytes to the pipe ({to_pipe_ended}), {to_tcp_bytes} bytes to tcp ({to_tcp_ended})"
    );
}

/// Copies from `reader` to `pipe` until EOF or an error on either side.
/// Generic so it runs directly against the tunnel handoff's `BufReader`,
/// which may still hold bytes read (but not yet consumed) before the
/// handoff. Returns the bytes copied and why the loop ended.
fn copy_tcp_to_pipe<R: Read>(mut reader: R, pipe: &OverlappedPipe) -> (u64, String) {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) => return (total, "tcp end of stream".to_owned()),
            Err(error) => return (total, format!("tcp read error: {error}")),
            Ok(n) => n,
        };
        if let Err(end) = pipe.write_all(&buf[..read]) {
            return (total, format!("pipe write ended: {end}"));
        }
        total += read as u64;
    }
}

/// Copies from `pipe` to `tcp` until EOF or an error on either side.
/// Returns the bytes copied and why the loop ended.
fn copy_pipe_to_tcp(pipe: &OverlappedPipe, mut tcp: TcpStream) -> (u64, String) {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = match pipe.read(&mut buf) {
            Ok(n) => n,
            Err(end) => return (total, format!("pipe read ended: {end}")),
        };
        total += read as u64;
        if let Err(error) = tcp.write_all(&buf[..read]) {
            return (total, format!("tcp write error: {error}"));
        }
    }
}

/// Why a pipe read or write transferred nothing: kept apart so the tunnel's
/// closing log line can tell a clean close from a failure.
#[derive(Debug)]
enum PipeEnd {
    /// Verbatim closed its end of the pipe.
    Closed,
    /// The other relay direction ended first and stopped this one
    /// ([`OverlappedPipe::stop`]).
    Stopped,
    /// Any other failure, with the OS error.
    Failed(windows::core::Error),
}

impl std::fmt::Display for PipeEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("pipe closed"),
            Self::Stopped => f.write_str("stopped by the other direction"),
            Self::Failed(error) => write!(f, "pipe error: {error}"),
        }
    }
}

/// A client-opened named pipe handle, read and written from two dedicated
/// threads via overlapped I/O (see this module's doc comment).
pub(crate) struct OverlappedPipe {
    handle: HANDLE,
    read_event: HANDLE,
    write_event: HANDLE,
    /// Manual-reset and never reset: once [`stop`](Self::stop) signals it,
    /// every read or write in progress or issued later ends promptly.
    stop_event: HANDLE,
}

// SAFETY: `handle` is used from two threads, but each issues only its own
// direction of I/O (`ReadFile` versus `WriteFile`) with its own event —
// the same documented-safe pattern `verbatim_control::server`'s `RawPipe`
// uses. `read_event` and `write_event` are each used from exactly one of
// those threads; `stop_event` is only signalled and waited on, which any
// thread may do.
unsafe impl Send for OverlappedPipe {}
// SAFETY: as above.
unsafe impl Sync for OverlappedPipe {}

impl OverlappedPipe {
    fn open(pipe_name: &str) -> io::Result<Self> {
        let wide: Vec<u16> = pipe_name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a valid, null-terminated wide string for the
        // duration of this call.
        let handle = unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                (FILE_GENERIC_READ | FILE_GENERIC_WRITE).0,
                FILE_SHARE_MODE(0),
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
        }
        .map_err(io::Error::other)?;

        let mut events = Vec::with_capacity(3);
        for _ in 0..3 {
            match create_event() {
                Ok(event) => events.push(event),
                Err(error) => {
                    close(handle);
                    events.into_iter().for_each(close);
                    return Err(error);
                }
            }
        }
        Ok(Self {
            handle,
            read_event: events[0],
            write_event: events[1],
            stop_event: events[2],
        })
    }

    /// Ends this pipe's relaying: a read or write waiting now, or issued at
    /// any point afterwards, returns [`PipeEnd::Stopped`]. Safe to call
    /// from any thread, any number of times.
    fn stop(&self) {
        // SAFETY: `stop_event` is a valid event handle owned by `self`.
        unsafe {
            let _ = SetEvent(self.stop_event);
        }
    }

    fn is_stopped(&self) -> bool {
        // SAFETY: `stop_event` is a valid event handle owned by `self`; a
        // zero timeout only polls it.
        unsafe { WaitForSingleObject(self.stop_event, 0) == WAIT_OBJECT_0 }
    }

    /// Reads at least one byte. Only the pipe-to-tcp thread calls this.
    fn read(&self, buf: &mut [u8]) -> Result<usize, PipeEnd> {
        if self.is_stopped() {
            return Err(PipeEnd::Stopped);
        }
        let mut overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..OVERLAPPED::default()
        };
        // SAFETY: `buf` and `overlapped` stay valid until `complete` has
        // waited for this operation to finish, cancelled or not; this is
        // the only thread that ever issues `ReadFile` on this handle.
        let issued = unsafe { ReadFile(self.handle, Some(buf), None, Some(&raw mut overlapped)) };
        let transferred = self.complete(issued, &overlapped)?;
        Ok(transferred as usize)
    }

    /// Writes all of `buf`. Only the tcp-to-pipe thread calls this.
    fn write_all(&self, mut buf: &[u8]) -> Result<(), PipeEnd> {
        while !buf.is_empty() {
            if self.is_stopped() {
                return Err(PipeEnd::Stopped);
            }
            let mut overlapped = OVERLAPPED {
                hEvent: self.write_event,
                ..OVERLAPPED::default()
            };
            // SAFETY: `buf` and `overlapped` stay valid until `complete`
            // has waited for this operation to finish, cancelled or not;
            // this is the only thread that ever issues `WriteFile` on this
            // handle.
            let issued =
                unsafe { WriteFile(self.handle, Some(buf), None, Some(&raw mut overlapped)) };
            let transferred = self.complete(issued, &overlapped)?;
            buf = &buf[transferred as usize..];
        }
        Ok(())
    }

    /// Waits for the operation `overlapped` describes to complete, or for
    /// [`stop`](Self::stop), whichever comes first. On a stop it cancels the
    /// operation and still waits for it to finish, since the OS writes into
    /// `overlapped` and the caller's buffer until then. Returns the bytes
    /// transferred; an operation that completed despite a racing stop keeps
    /// its data.
    fn complete(
        &self,
        issued: windows::core::Result<()>,
        overlapped: &OVERLAPPED,
    ) -> Result<u32, PipeEnd> {
        if let Err(error) = issued
            && error.code() != HRESULT::from_win32(ERROR_IO_PENDING.0)
        {
            return Err(classify(error, false));
        }
        // SAFETY: both handles are valid events owned by `self`.
        let woke = unsafe {
            WaitForMultipleObjects(&[overlapped.hEvent, self.stop_event], false, INFINITE)
        };
        let stopped = woke == WAIT_EVENT(WAIT_OBJECT_0.0 + 1);
        if woke != WAIT_OBJECT_0 {
            // Stopped, or the wait itself failed: either way the operation
            // must not outlive this call.
            // SAFETY: `overlapped` describes an operation issued on this
            // handle by this thread.
            unsafe {
                let _ = CancelIoEx(self.handle, Some(overlapped));
            }
        }
        let mut transferred = 0u32;
        // SAFETY: `overlapped` is still valid; this blocks until the
        // operation has finished, which a cancellation makes prompt.
        match unsafe { GetOverlappedResult(self.handle, overlapped, &raw mut transferred, true) } {
            Ok(()) => Ok(transferred),
            Err(error) => Err(classify(error, stopped)),
        }
    }
}

/// Sorts an OS error from a pipe operation into a [`PipeEnd`]. A cancelled
/// operation counts as stopped only when this side asked for the stop.
fn classify(error: windows::core::Error, stopped: bool) -> PipeEnd {
    let code = error.code();
    if code == HRESULT::from_win32(ERROR_BROKEN_PIPE.0)
        || code == HRESULT::from_win32(ERROR_PIPE_NOT_CONNECTED.0)
    {
        PipeEnd::Closed
    } else if stopped && code == HRESULT::from_win32(ERROR_OPERATION_ABORTED.0) {
        PipeEnd::Stopped
    } else {
        PipeEnd::Failed(error)
    }
}

fn create_event() -> io::Result<HANDLE> {
    // SAFETY: manual-reset, initially unsignaled, unnamed event; no
    // preconditions.
    unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(io::Error::other)
}

fn close(handle: HANDLE) {
    // SAFETY: `handle` is owned by the caller and not used again after
    // this point.
    unsafe {
        let _ = CloseHandle(handle);
    }
}

impl Drop for OverlappedPipe {
    fn drop(&mut self) {
        close(self.handle);
        close(self.read_event);
        close(self.write_event);
        close(self.stop_event);
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
    use windows::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };

    use super::*;

    /// How long a test waits for something that must happen promptly before
    /// declaring it stuck. Generous: every case here should finish in
    /// milliseconds, and the only failure being detected is never finishing.
    const STUCK: Duration = Duration::from_secs(5);

    /// The server end of a test pipe, closed on drop.
    struct ServerEnd(HANDLE);

    impl Drop for ServerEnd {
        fn drop(&mut self) {
            close(self.0);
        }
    }

    /// Creates a named pipe and connects an [`OverlappedPipe`] to it as the
    /// tunnel would. The server end stays idle unless a test acts on it.
    fn pipe_pair(name: &str) -> (ServerEnd, OverlappedPipe) {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: `wide` is a valid, null-terminated wide string for the
        // duration of this call.
        let server = unsafe {
            CreateNamedPipeW(
                PCWSTR(wide.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                4096,
                4096,
                0,
                None,
            )
        };
        assert!(!server.is_invalid(), "creates the test pipe");
        let server = ServerEnd(server);
        let client = OverlappedPipe::open(name).expect("opens the test pipe");
        (server, client)
    }

    /// Runs `read` on its own thread and returns its outcome, or `None` if
    /// it is still blocked after [`STUCK`].
    fn read_on_a_thread(pipe: &Arc<OverlappedPipe>) -> Option<Result<usize, String>> {
        let (sender, receiver) = mpsc::channel();
        let pipe = Arc::clone(pipe);
        thread::spawn(move || {
            let mut buf = [0u8; 16];
            let _ = sender.send(pipe.read(&mut buf).map_err(|end| end.to_string()));
        });
        receiver.recv_timeout(STUCK).ok()
    }

    #[test]
    fn a_stop_before_a_read_is_issued_still_ends_it() {
        // The case cancellation alone missed: the other direction ends while
        // this one is between reads, so nothing is pending to cancel.
        let (_server, pipe) = pipe_pair(r"\\.\pipe\verbatim-agent-tunnel-test-a");
        let pipe = Arc::new(pipe);
        pipe.stop();
        let outcome = read_on_a_thread(&pipe).expect("the read must not block after a stop");
        assert_eq!(outcome, Err("stopped by the other direction".to_owned()));
    }

    #[test]
    fn a_stop_ends_a_read_already_waiting() {
        let (_server, pipe) = pipe_pair(r"\\.\pipe\verbatim-agent-tunnel-test-b");
        let pipe = Arc::new(pipe);
        let stopper = {
            let pipe = Arc::clone(&pipe);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                pipe.stop();
            })
        };
        let outcome = read_on_a_thread(&pipe).expect("the read must end once stopped");
        stopper.join().expect("the stopper does not panic");
        assert_eq!(outcome, Err("stopped by the other direction".to_owned()));
    }

    #[test]
    fn the_server_closing_reads_as_closed_not_as_a_failure() {
        let (server, pipe) = pipe_pair(r"\\.\pipe\verbatim-agent-tunnel-test-c");
        drop(server);
        let mut buf = [0u8; 16];
        let outcome = pipe.read(&mut buf).map_err(|end| end.to_string());
        assert_eq!(outcome, Err("pipe closed".to_owned()));
    }

    #[test]
    fn the_tunnel_ends_when_the_client_leaves_an_idle_pipe() {
        let (_server, pipe) = pipe_pair(r"\\.\pipe\verbatim-agent-tunnel-test-d");
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds a loopback port");
        let client =
            TcpStream::connect(listener.local_addr().expect("has an address")).expect("connects");
        let (accepted, _) = listener.accept().expect("accepts");
        let reader = BufReader::new(accepted.try_clone().expect("clones the socket"));

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            run(pipe, reader, accepted);
            let _ = sender.send(());
        });
        drop(client);
        receiver
            .recv_timeout(STUCK)
            .expect("the tunnel must close once the client has gone, with the pipe idle");
    }
}
