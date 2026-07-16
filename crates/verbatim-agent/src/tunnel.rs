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

use windows::Win32::Foundation::{CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_MODE,
    OPEN_EXISTING, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::CreateEventW;
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
/// unattributable — the counts say whether the bytes ever left the pipe.
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
            // The client is done sending (EOF) or gone (error): stop any
            // pending pipe read so the pipe-to-tcp direction also ends.
            pipe.cancel_pending_io();
            (bytes, ended)
        })
    };

    let (to_tcp_bytes, to_tcp_ended) = copy_pipe_to_tcp(&pipe, writer);
    // Verbatim closed the pipe, or the pipe errored: unblock the client
    // side too.
    let _ = shutdown_handle.shutdown(Shutdown::Both);
    pipe.cancel_pending_io();

    let (to_pipe_bytes, to_pipe_ended) = to_pipe.join().unwrap_or((0, "join failed"));
    eprintln!(
        "verbatim-agent: control tunnel closed; relayed {to_pipe_bytes} bytes to the pipe ({to_pipe_ended}), {to_tcp_bytes} bytes to tcp ({to_tcp_ended})"
    );
}

/// Copies from `reader` to `pipe` until EOF or an error on either side.
/// Generic so it runs directly against the tunnel handoff's `BufReader`,
/// which may still hold bytes read (but not yet consumed) before the
/// handoff. Returns the bytes copied and why the loop ended.
fn copy_tcp_to_pipe<R: Read>(mut reader: R, pipe: &OverlappedPipe) -> (u64, &'static str) {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) => return (total, "tcp end of stream"),
            Err(_) => return (total, "tcp read error"),
            Ok(n) => n,
        };
        if pipe.write_all(&buf[..read]).is_err() {
            return (total, "pipe write error");
        }
        total += read as u64;
    }
}

/// Copies from `pipe` to `tcp` until EOF or an error on either side.
/// Returns the bytes copied and why the loop ended.
fn copy_pipe_to_tcp(pipe: &OverlappedPipe, mut tcp: TcpStream) -> (u64, &'static str) {
    let mut buf = [0u8; 8192];
    let mut total = 0u64;
    loop {
        let read = match pipe.read(&mut buf) {
            Ok(0) => return (total, "pipe end of stream"),
            Err(_) => return (total, "pipe read error"),
            Ok(n) => n,
        };
        if tcp.write_all(&buf[..read]).is_err() {
            return (total, "tcp write error");
        }
        total += read as u64;
    }
}

/// A client-opened named pipe handle, read and written from two dedicated
/// threads via overlapped I/O (see this module's doc comment).
pub(crate) struct OverlappedPipe {
    handle: HANDLE,
    read_event: HANDLE,
    write_event: HANDLE,
}

// SAFETY: `handle` is used from two threads, but each issues only its own
// direction of I/O (`ReadFile` versus `WriteFile`) with its own event —
// the same documented-safe pattern `verbatim_control::server`'s `RawPipe`
// uses. `read_event` and `write_event` are each used from exactly one of
// those threads.
unsafe impl Send for OverlappedPipe {}
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

        let read_event = match create_event() {
            Ok(event) => event,
            Err(error) => {
                close(handle);
                return Err(error);
            }
        };
        let write_event = match create_event() {
            Ok(event) => event,
            Err(error) => {
                close(handle);
                close(read_event);
                return Err(error);
            }
        };
        Ok(Self {
            handle,
            read_event,
            write_event,
        })
    }

    fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let mut overlapped = OVERLAPPED {
            hEvent: self.read_event,
            ..OVERLAPPED::default()
        };
        let mut read = 0u32;
        // SAFETY: `buf` and `overlapped` are valid for the duration of
        // this call, including the blocking wait below; this is the only
        // thread that ever issues `ReadFile` on this handle.
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
        // SAFETY: `overlapped` is still valid; blocks until the read
        // (which may have already completed above) finishes or is
        // cancelled by `cancel_pending_io`.
        match unsafe {
            GetOverlappedResult(
                self.handle,
                &raw const overlapped,
                &raw mut transferred,
                true,
            )
        } {
            Ok(()) => Ok(transferred as usize),
            Err(_) => Ok(0),
        }
    }

    fn write_all(&self, mut buf: &[u8]) -> io::Result<()> {
        while !buf.is_empty() {
            let mut overlapped = OVERLAPPED {
                hEvent: self.write_event,
                ..OVERLAPPED::default()
            };
            let mut written = 0u32;
            // SAFETY: `buf` and `overlapped` are valid for the duration of
            // this call, including the blocking wait below; this is the
            // only thread that ever issues `WriteFile` on this handle.
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
            // SAFETY: `overlapped` is still valid; blocks until the write
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

    /// Cancels any I/O this handle has pending, from any thread — how the
    /// direction that finished first wakes the other one out of its
    /// blocking `GetOverlappedResult` wait.
    fn cancel_pending_io(&self) {
        // SAFETY: `self.handle` is a valid, open handle; cancelling with
        // no specific `OVERLAPPED` cancels everything pending on it.
        unsafe {
            let _ = CancelIoEx(self.handle, None);
        }
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
    }
}
