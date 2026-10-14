//! Overlapped pipes and an I/O completion port, for a thread that reads and
//! writes many pipes without blocking on any of them
//! (`docs/design/communication.md`, sections 4 and 6).
//!
//! [`pipe`] makes a pipe as the standard library makes its child pipes:
//! with `NtCreateNamedPipeFile` on the named pipe file system's root and an
//! empty name, so the pipe has no name at any moment and no other process
//! can open it, and its other end opened relative to the first handle with
//! `NtOpenFile`. Verbatim's end is overlapped; the other end is overlapped
//! too, or synchronous, as the caller asks ([`End`]). Neither end is
//! inheritable until a launch makes the child's so.
//!
//! [`Port`] is the completion port. A handle joins it through
//! [`Port::associate`], which also asks the system to skip the port when an
//! operation completes at once (`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`),
//! so an operation that finishes at once is handled where it was started
//! and only one that waits comes back through the port. [`PipeIo`] is one
//! operation slot: an `OVERLAPPED` and its buffer, which stay where they
//! are, owned by the slot, for as long as an operation is in flight.
//!
//! The module's rule for its `unsafe` code: the memory an operation uses is
//! never freed or touched while the operation is in flight. A slot learns
//! that its operation ended only from the operation itself, by completing
//! at once or by its completion arriving through the port
//! ([`PipeIo::complete`]); a slot dropped with its operation still in
//! flight leaks that memory rather than free it under the kernel.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr::NonNull;
use std::sync::OnceLock;
use std::time::Duration;

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_SYNCHRONOUS_IO_NONALERT, NtOpenFile,
};
use windows::Win32::Foundation::{
    ERROR_IO_PENDING, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE, NTSTATUS, UNICODE_STRING,
    WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_WRITE_ATTRIBUTES, ReadFile, SYNCHRONIZE, SetFileCompletionNotificationModes, WriteFile,
};
use windows::Win32::System::IO::{
    CreateIoCompletionPort, GetOverlappedResult, GetQueuedCompletionStatusEx, IO_STATUS_BLOCK,
    OVERLAPPED, OVERLAPPED_ENTRY, PostQueuedCompletionStatus,
};

/// `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` (`winbase.h`): an operation that
/// completes at once queues no completion to the port.
const FILE_SKIP_COMPLETION_PORT_ON_SUCCESS: u8 = 1;

/// `FILE_SKIP_SET_EVENT_ON_HANDLE` (`winbase.h`): the handle's own event is
/// not signaled when an operation completes, since nothing waits on it.
const FILE_SKIP_SET_EVENT_ON_HANDLE: u8 = 2;

/// The pipe buffer [`pipe`] asks for when given 0, as `CreatePipe` does.
const DEFAULT_BUFFER: u32 = 4096;

/// A byte-stream pipe read in byte mode, whose calls wait
/// (`FILE_PIPE_BYTE_STREAM_TYPE`, `FILE_PIPE_BYTE_STREAM_MODE` and
/// `FILE_PIPE_QUEUE_OPERATION`, all 0, `ntifs.h`).
const FILE_PIPE_BYTE_STREAM: u32 = 0;

/// How long a client may wait for the pipe, which nothing does: the other
/// end is opened at once. Negative is relative, in 100-nanosecond units,
/// as the standard library passes it.
const DEFAULT_TIMEOUT: i64 = -50 * 10_000_000;

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `NtCreateNamedPipeFile` (`ntifs.h`), which the `windows` crate does
    /// not bind; the standard library declares it the same way for its own
    /// child pipes.
    fn NtCreateNamedPipeFile(
        file: *mut HANDLE,
        access: u32,
        attributes: *const OBJECT_ATTRIBUTES,
        status: *mut IO_STATUS_BLOCK,
        share: u32,
        disposition: u32,
        options: u32,
        pipe_type: u32,
        read_mode: u32,
        completion_mode: u32,
        instances: u32,
        inbound_quota: u32,
        outbound_quota: u32,
        timeout: *const i64,
    ) -> NTSTATUS;
}

/// Which way data flows on a pipe, seen from Verbatim's end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Verbatim's end reads; the other end writes.
    Inbound,
    /// Verbatim's end writes; the other end reads.
    Outbound,
}

/// How a pipe's other end is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// For overlapped I/O, through a completion port.
    Overlapped,
    /// For synchronous reads and writes, as a child given an ordinary pipe
    /// uses it.
    Synchronous,
}

/// Makes one pipe with no name: Verbatim's end, overlapped, returned first,
/// and the other end, opened on it at once as `other` says, returned
/// second; neither inheritable. `buffer` is the pipe's buffer in the
/// direction data flows, `0` for 4 KiB, `CreatePipe`'s default.
///
/// # Errors
///
/// Returns the error of the step that failed: opening the named pipe file
/// system's root, creating the pipe, or opening its other end.
pub fn pipe(flow: Flow, buffer: u32, other: End) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let buffer = if buffer == 0 { DEFAULT_BUFFER } else { buffer };
    let (ours_access, theirs_access) = match flow {
        Flow::Inbound => (
            FILE_GENERIC_READ.0 | FILE_WRITE_ATTRIBUTES.0,
            FILE_GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0,
        ),
        Flow::Outbound => (
            FILE_GENERIC_WRITE.0 | FILE_READ_ATTRIBUTES.0,
            FILE_GENERIC_READ.0 | FILE_WRITE_ATTRIBUTES.0,
        ),
    };
    let empty = UNICODE_STRING::default();
    let mut status = IO_STATUS_BLOCK::default();
    let attributes = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(0),
        RootDirectory: pipe_root()?,
        ObjectName: &raw const empty,
        ..OBJECT_ATTRIBUTES::default()
    };
    let timeout = DEFAULT_TIMEOUT;
    let mut ours = HANDLE::default();
    // SAFETY: every pointer is to a local alive across the call; the empty
    // name relative to the root makes an unnamed pipe; options 0 make the
    // handle asynchronous; the handle returned is checked and then owned.
    let created = unsafe {
        NtCreateNamedPipeFile(
            &raw mut ours,
            ours_access | SYNCHRONIZE.0,
            &raw const attributes,
            &raw mut status,
            FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0,
            FILE_CREATE.0,
            0,
            FILE_PIPE_BYTE_STREAM,
            FILE_PIPE_BYTE_STREAM,
            FILE_PIPE_BYTE_STREAM,
            1,
            buffer,
            buffer,
            &raw const timeout,
        )
    };
    created.ok().map_err(io::Error::other)?;
    // SAFETY: a valid handle the call just returned, owned by nothing else.
    let ours = unsafe { OwnedHandle::from_raw_handle(ours.0 as RawHandle) };
    let relative = OBJECT_ATTRIBUTES {
        Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(0),
        RootDirectory: HANDLE(ours.as_raw_handle()),
        ObjectName: &raw const empty,
        ..OBJECT_ATTRIBUTES::default()
    };
    let options = match other {
        End::Overlapped => FILE_NON_DIRECTORY_FILE.0,
        End::Synchronous => FILE_NON_DIRECTORY_FILE.0 | FILE_SYNCHRONOUS_IO_NONALERT.0,
    };
    let mut theirs = HANDLE::default();
    // SAFETY: as above; the other end is opened relative to our handle,
    // which stays open across the call.
    let opened = unsafe {
        NtOpenFile(
            &raw mut theirs,
            theirs_access | SYNCHRONIZE.0,
            &raw const relative,
            &raw mut status,
            FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0,
            options,
        )
    };
    opened.ok().map_err(io::Error::other)?;
    // SAFETY: a valid handle the call just returned, owned by nothing else.
    let theirs = unsafe { OwnedHandle::from_raw_handle(theirs.0 as RawHandle) };
    Ok((ours, theirs))
}

/// The named pipe file system's root, `\Device\NamedPipe\`, opened once for
/// the process's life: unnamed pipes are made relative to it.
fn pipe_root() -> io::Result<HANDLE> {
    static ROOT: OnceLock<Result<isize, i32>> = OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let path: Vec<u16> = r"\Device\NamedPipe\".encode_utf16().collect();
        let bytes = u16::try_from(path.len() * 2).unwrap_or(0);
        let name = UNICODE_STRING {
            Length: bytes,
            MaximumLength: bytes,
            Buffer: windows::core::PWSTR(path.as_ptr().cast_mut()),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: u32::try_from(size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(0),
            ObjectName: &raw const name,
            ..OBJECT_ATTRIBUTES::default()
        };
        let mut status = IO_STATUS_BLOCK::default();
        let mut handle = HANDLE::default();
        // SAFETY: every pointer is to a local alive across the call; the
        // handle is kept for the process's life.
        let opened = unsafe {
            NtOpenFile(
                &raw mut handle,
                GENERIC_READ.0 | SYNCHRONIZE.0,
                &raw const attributes,
                &raw mut status,
                FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0,
                FILE_SYNCHRONOUS_IO_NONALERT.0,
            )
        };
        opened
            .ok()
            .map(|()| handle.0 as isize)
            .map_err(|error| error.code().0)
    });
    match root {
        Ok(handle) => Ok(HANDLE(*handle as *mut core::ffi::c_void)),
        Err(code) => Err(io::Error::other(format!(
            "the named pipe file system could not be opened: {code:#x}"
        ))),
    }
}

/// An I/O completion port: where the overlapped operations of every file
/// associated with it complete, and where other threads post wake-ups, all
/// taken by one thread waiting in [`Port::wait`].
#[derive(Debug)]
pub struct Port {
    handle: OwnedHandle,
}

impl Port {
    /// A new port, run by one thread at a time.
    ///
    /// # Errors
    ///
    /// Returns the error if the port cannot be created.
    pub fn new() -> io::Result<Self> {
        // SAFETY: creating a port with no file; the handle returned is
        // owned below.
        let handle = unsafe { CreateIoCompletionPort(INVALID_HANDLE_VALUE, None, 0, 1) }
            .map_err(win32_error)?;
        Ok(Self {
            // SAFETY: a valid handle just returned, owned by nothing else.
            handle: unsafe { OwnedHandle::from_raw_handle(handle.0 as RawHandle) },
        })
    }

    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.handle.as_raw_handle())
    }

    /// Associates `file`, opened for overlapped I/O, with the port under
    /// `key`, which every completion of its operations carries, and asks
    /// the system to queue no completion for an operation that finishes at
    /// once. Only a file associated this way can be used with a
    /// [`PipeIo`].
    ///
    /// # Errors
    ///
    /// Returns the error if either step fails; the file is then dropped.
    pub fn associate(&self, file: OwnedHandle, key: usize) -> io::Result<PortFile> {
        let handle = HANDLE(file.as_raw_handle());
        // SAFETY: both handles are open; the association lasts as long as
        // the file's handle.
        unsafe { CreateIoCompletionPort(handle, Some(self.raw()), key, 0) }.map_err(win32_error)?;
        // SAFETY: an open handle; the call only changes how its
        // completions are reported.
        unsafe {
            SetFileCompletionNotificationModes(
                handle,
                FILE_SKIP_COMPLETION_PORT_ON_SUCCESS | FILE_SKIP_SET_EVENT_ON_HANDLE,
            )
        }
        .map_err(win32_error)?;
        Ok(PortFile { file, key })
    }

    /// Posts a wake-up carrying `key` and no operation
    /// ([`Completion::is_posted`]). Any thread may post.
    ///
    /// # Errors
    ///
    /// Returns the error if the port refuses it.
    pub fn post(&self, key: usize) -> io::Result<()> {
        // SAFETY: the port is open; no OVERLAPPED is passed.
        unsafe { PostQueuedCompletionStatus(self.raw(), 0, key, None) }.map_err(win32_error)
    }

    /// Waits up to `timeout`, or for ever when `None`, for completions and
    /// wake-ups, and takes as many as are queued, up to the room in
    /// `completions`. Returns how many it took; 0 when the time ran out.
    ///
    /// # Errors
    ///
    /// Returns the error if the wait fails.
    pub fn wait(
        &self,
        completions: &mut Completions,
        timeout: Option<Duration>,
    ) -> io::Result<usize> {
        let milliseconds = timeout.map_or(u32::MAX, |timeout| {
            // Rounded up, so a deadline is never woken for early.
            u32::try_from(timeout.as_micros().div_ceil(1_000)).unwrap_or(u32::MAX - 1)
        });
        let mut taken = 0u32;
        // SAFETY: the port is open; the entries are written by the call,
        // up to their number, and `taken` says how many.
        let waited = unsafe {
            GetQueuedCompletionStatusEx(
                self.raw(),
                &mut completions.entries,
                &raw mut taken,
                milliseconds,
                false,
            )
        };
        match waited {
            Ok(()) => {
                completions.taken = taken as usize;
                Ok(completions.taken)
            }
            Err(error) if error.code() == WIN32_ERROR(WAIT_TIMEOUT.0).to_hresult() => {
                completions.taken = 0;
                Ok(0)
            }
            Err(error) => {
                completions.taken = 0;
                Err(win32_error(error))
            }
        }
    }
}

/// Room for the completions one [`Port::wait`] takes.
pub struct Completions {
    entries: Vec<OVERLAPPED_ENTRY>,
    taken: usize,
}

impl Completions {
    /// Room for `capacity` completions per wait.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: vec![OVERLAPPED_ENTRY::default(); capacity.max(1)],
            taken: 0,
        }
    }

    /// The completions the last wait took, in the order the port gave
    /// them.
    pub fn iter(&self) -> impl Iterator<Item = Completion> + '_ {
        self.entries[..self.taken].iter().map(|entry| Completion {
            key: entry.lpCompletionKey,
            overlapped: entry.lpOverlapped,
        })
    }
}

/// One completion or wake-up taken from a [`Port`].
#[derive(Clone, Copy, Debug)]
pub struct Completion {
    /// The key the file was associated with, or the wake-up was posted
    /// with.
    pub key: usize,
    /// The operation that completed; null for a wake-up.
    overlapped: *mut OVERLAPPED,
}

impl Completion {
    /// Whether this is a wake-up [`Port::post`] posted rather than an
    /// operation's completion.
    #[must_use]
    pub fn is_posted(&self) -> bool {
        self.overlapped.is_null()
    }

    /// The pointer the completion carries: an operation's `OVERLAPPED`,
    /// or whatever a packet posted with one carried ([`crate::port_queue`]).
    pub(crate) fn pointer(&self) -> *mut OVERLAPPED {
        self.overlapped
    }
}

/// A file associated with a [`Port`] ([`Port::associate`]).
#[derive(Debug)]
pub struct PortFile {
    file: OwnedHandle,
    key: usize,
}

impl PortFile {
    /// The key its completions carry.
    #[must_use]
    pub fn key(&self) -> usize {
        self.key
    }

    fn raw(&self) -> HANDLE {
        HANDLE(self.file.as_raw_handle())
    }
}

/// How an operation [`PipeIo`] started went.
#[derive(Debug)]
pub enum Started {
    /// It completed at once, moving this many bytes; no completion will
    /// come through the port.
    Done(usize),
    /// It is in flight; its completion will come through the port.
    Pending,
    /// It failed at once; nothing is in flight.
    Failed(io::Error),
}

/// An operation's `OVERLAPPED` and buffer, at one fixed address for as long
/// as the slot lives. `repr(C)` with the `OVERLAPPED` first, so a
/// completion's `OVERLAPPED` pointer is the operation's address.
#[repr(C)]
struct Operation {
    overlapped: OVERLAPPED,
    /// Always initialized: a read is given the whole of it, a write its
    /// bytes.
    buffer: Vec<u8>,
    /// How many bytes of `buffer` the last finished read brought.
    filled: usize,
}

/// One overlapped operation slot: a read or a write at a time, never two.
/// Its memory stays put, and is neither touched nor freed, while its
/// operation is in flight.
pub struct PipeIo {
    operation: NonNull<Operation>,
    /// How many bytes a read asks for unless told otherwise.
    read_size: usize,
    in_flight: bool,
    /// Whether the operation last started was a read.
    reading: bool,
}

// SAFETY: the slot owns its operation's memory outright; the raw pointer is
// only an allocation it moves with, never shared, so moving the slot to
// another thread moves sole ownership.
unsafe impl Send for PipeIo {}

impl std::fmt::Debug for PipeIo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PipeIo")
            .field("in_flight", &self.in_flight)
            .finish_non_exhaustive()
    }
}

impl PipeIo {
    /// A slot whose reads ask for up to `read_size` bytes; a slot used only
    /// for writes passes 0, and its buffer grows to the largest write.
    #[must_use]
    pub fn new(read_size: usize) -> Self {
        let operation = Box::new(Operation {
            overlapped: OVERLAPPED::default(),
            buffer: Vec::new(),
            filled: 0,
        });
        Self {
            operation: NonNull::from(Box::leak(operation)),
            read_size,
            in_flight: false,
            reading: false,
        }
    }

    /// Whether an operation is in flight.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.in_flight
    }

    /// Whether `completion` is this slot's operation's.
    #[must_use]
    pub fn owns(&self, completion: &Completion) -> bool {
        completion.overlapped == self.operation.as_ptr().cast::<OVERLAPPED>()
    }

    /// Starts a read from `file` into the buffer.
    ///
    /// # Panics
    ///
    /// Panics if an operation is already in flight.
    pub fn read(&mut self, file: &PortFile) -> Started {
        self.read_up_to(file, self.read_size)
    }

    /// Starts a read from `file` of up to `size` bytes into the buffer,
    /// which takes that size; memory it holds far beyond it, after a
    /// larger read, is given back.
    ///
    /// # Panics
    ///
    /// Panics if an operation is already in flight.
    pub fn read_up_to(&mut self, file: &PortFile, size: usize) -> Started {
        assert!(!self.in_flight, "one operation at a time");
        // SAFETY: nothing is in flight, so nothing but this slot uses the
        // operation's memory now.
        let operation = unsafe { self.operation.as_mut() };
        operation.overlapped = OVERLAPPED::default();
        operation.filled = 0;
        // Only bytes the buffer has never held are zeroed.
        operation.buffer.resize(size, 0);
        if operation.buffer.capacity() > size.saturating_mul(4) {
            operation.buffer.shrink_to_fit();
        }
        let overlapped = &raw mut operation.overlapped;
        let buffer = operation.buffer.as_mut_slice();
        // SAFETY: the buffer and the OVERLAPPED are in the slot's own
        // allocation, which stays put and is neither touched nor freed until
        // the read completes: at once (handled here), or by its completion
        // through the port, which `complete` alone accepts. No reference to
        // them is used again before then.
        let result = unsafe { ReadFile(file.raw(), Some(buffer), None, Some(overlapped)) };
        self.started(file, result, true)
    }

    /// Starts a write of `bytes` to `file`, copying them into the slot,
    /// where they stay until the write completes; the slot's allocation is
    /// reused from one write to the next.
    ///
    /// # Panics
    ///
    /// Panics if an operation is already in flight.
    pub fn write(&mut self, file: &PortFile, bytes: &[u8]) -> Started {
        self.write_with(file, |buffer| {
            buffer.extend_from_slice(bytes);
            Ok::<(), std::convert::Infallible>(())
        })
        .unwrap_or_else(|never| match never {})
    }

    /// Starts a write to `file` of the bytes `put` puts in the slot's
    /// emptied buffer, where they stay until the write completes, so
    /// nothing is copied. When `put` fails, nothing is written and its
    /// error is returned; when it puts nothing, nothing is written either,
    /// and the write is done at once, having moved nothing.
    ///
    /// # Errors
    ///
    /// Returns `put`'s error.
    ///
    /// # Panics
    ///
    /// Panics if an operation is already in flight.
    pub fn write_with<E>(
        &mut self,
        file: &PortFile,
        put: impl FnOnce(&mut Vec<u8>) -> Result<(), E>,
    ) -> Result<Started, E> {
        assert!(!self.in_flight, "one operation at a time");
        // SAFETY: nothing is in flight, so nothing but this slot uses the
        // operation's memory now.
        let operation = unsafe { self.operation.as_mut() };
        operation.overlapped = OVERLAPPED::default();
        operation.filled = 0;
        operation.buffer.clear();
        put(&mut operation.buffer)?;
        if operation.buffer.is_empty() {
            return Ok(Started::Done(0));
        }
        let overlapped = &raw mut operation.overlapped;
        let buffer = operation.buffer.as_slice();
        // SAFETY: as for `read`: the bytes and the OVERLAPPED stay in the
        // slot's allocation, untouched and not freed, until the write
        // completes.
        let result = unsafe { WriteFile(file.raw(), Some(buffer), None, Some(overlapped)) };
        Ok(self.started(file, result, false))
    }

    fn started(
        &mut self,
        file: &PortFile,
        result: windows::core::Result<()>,
        read: bool,
    ) -> Started {
        self.reading = read;
        match result {
            // Completed at once, so no completion will come: the slot's
            // memory is free again, and the count is in the OVERLAPPED.
            Ok(()) => match self.result(file, read) {
                Ok(moved) => Started::Done(moved),
                Err(error) => Started::Failed(error),
            },
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {
                self.in_flight = true;
                Started::Pending
            }
            Err(error) => Started::Failed(win32_error(error)),
        }
    }

    /// Takes `completion`, this slot's operation's, which ended it, and
    /// returns how many bytes it moved, or its error. A cancelled
    /// operation's error is `ERROR_OPERATION_ABORTED`.
    ///
    /// # Errors
    ///
    /// Returns the operation's error.
    ///
    /// # Panics
    ///
    /// Panics if `completion` is not this slot's ([`owns`](Self::owns)),
    /// or nothing is in flight.
    pub fn complete(&mut self, file: &PortFile, completion: &Completion) -> io::Result<usize> {
        assert!(
            self.owns(completion) && self.in_flight,
            "a completion for this slot's operation in flight"
        );
        // Its completion came through the port: the kernel is done with
        // the slot's memory.
        self.in_flight = false;
        self.result(file, self.reading)
    }

    /// The finished operation's outcome, from its OVERLAPPED.
    fn result(&mut self, file: &PortFile, read: bool) -> io::Result<usize> {
        let mut moved = 0u32;
        // SAFETY: the operation has finished, so the OVERLAPPED is only this
        // slot's and is only read.
        let overlapped = unsafe { &raw const (*self.operation.as_ptr()).overlapped };
        // SAFETY: as above; `bwait` is false, so nothing waits.
        let result = unsafe { GetOverlappedResult(file.raw(), overlapped, &raw mut moved, false) };
        result.map_err(win32_error)?;
        let moved = moved as usize;
        if read {
            // SAFETY: nothing is in flight, so the operation is only this
            // slot's.
            let operation = unsafe { self.operation.as_mut() };
            operation.filled = moved.min(operation.buffer.len());
        }
        Ok(moved)
    }

    /// The bytes the last read brought, once it has finished.
    ///
    /// # Panics
    ///
    /// Panics if an operation is in flight.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        assert!(!self.in_flight, "the read has not finished");
        // SAFETY: nothing is in flight, so the buffer is only this slot's.
        let operation = unsafe { self.operation.as_ref() };
        &operation.buffer[..operation.filled]
    }
}

impl Drop for PipeIo {
    fn drop(&mut self) {
        if self.in_flight {
            // The kernel may still write to the operation's memory: leak it
            // rather than free it under the kernel. Owners wait for the
            // completion first, which a closed pipe or an exited child
            // brings; this is the last line of defence.
            return;
        }
        // SAFETY: allocated by `Box::leak` in `new` and freed once, here,
        // with nothing in flight.
        drop(unsafe { Box::from_raw(self.operation.as_ptr()) });
    }
}

/// A Windows error as an I/O error with its Win32 code, so its kind (a
/// broken pipe, say) is known.
pub(crate) fn win32_error(error: windows::core::Error) -> io::Error {
    let code = error.code().0.cast_unsigned();
    // HRESULT_FROM_WIN32: facility 7, the Win32 code in the low 16 bits.
    if code & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error(i32::try_from(code & 0xFFFF).unwrap_or(0))
    } else {
        io::Error::other(error)
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{Read, Write};

    use windows::Wdk::Storage::FileSystem::{
        FILE_MODE_INFORMATION, FILE_SYNCHRONOUS_IO_ALERT, FileModeInformation,
        NtQueryInformationFile,
    };

    use super::*;

    /// Whether `handle` was opened for synchronous I/O.
    fn is_synchronous(handle: &OwnedHandle) -> bool {
        let mut mode = FILE_MODE_INFORMATION::default();
        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: an open handle; `mode` is the class's structure, written
        // by the call.
        unsafe {
            NtQueryInformationFile(
                HANDLE(handle.as_raw_handle()),
                &raw mut status,
                (&raw mut mode).cast(),
                u32::try_from(size_of::<FILE_MODE_INFORMATION>()).expect("small"),
                FileModeInformation,
            )
        }
        .ok()
        .expect("the mode is readable");
        mode.Mode & (FILE_SYNCHRONOUS_IO_NONALERT.0 | FILE_SYNCHRONOUS_IO_ALERT.0) != 0
    }

    #[test]
    fn the_other_end_is_overlapped_or_synchronous_as_asked() {
        let (ours, theirs) = pipe(Flow::Outbound, 0, End::Overlapped).expect("a pipe");
        assert!(!is_synchronous(&ours), "Verbatim's end is overlapped");
        assert!(!is_synchronous(&theirs), "the other end is overlapped");
        let (_, theirs) = pipe(Flow::Outbound, 0, End::Synchronous).expect("a pipe");
        assert!(is_synchronous(&theirs), "the other end is synchronous");
    }

    #[test]
    fn a_read_on_an_overlapped_other_end_waits_and_completes_through_the_port() {
        let port = Port::new().expect("a port");
        let (ours, theirs) = pipe(Flow::Outbound, 0, End::Overlapped).expect("a pipe");
        let ours = port.associate(ours, 1).expect("associated");
        let theirs = port.associate(theirs, 2).expect("associated");
        let mut reading = PipeIo::new(64);
        assert!(matches!(reading.read(&theirs), Started::Pending));
        let mut writing = PipeIo::new(0);
        assert!(matches!(writing.write(&ours, b"three\n"), Started::Done(6)));
        let mut completions = Completions::with_capacity(4);
        assert_eq!(port.wait(&mut completions, None).expect("waited"), 1);
        let completion = completions.iter().next().expect("one completion");
        assert_eq!(completion.key, 2);
        assert_eq!(reading.complete(&theirs, &completion).expect("read"), 6);
        assert_eq!(reading.data(), b"three\n");
    }

    #[test]
    fn an_overlapped_write_reaches_a_synchronous_end() {
        let port = Port::new().expect("a port");
        let (ours, theirs) = pipe(Flow::Outbound, 0, End::Synchronous).expect("a pipe");
        let ours = port.associate(ours, 7).expect("associated");
        let mut slot = PipeIo::new(0);
        assert!(matches!(slot.write(&ours, b"one\n"), Started::Done(4)));
        let mut read = [0u8; 4];
        File::from(theirs).read_exact(&mut read).expect("read");
        assert_eq!(&read, b"one\n");
    }

    #[test]
    fn a_read_that_waits_completes_through_the_port_when_a_synchronous_end_writes() {
        let port = Port::new().expect("a port");
        let (ours, theirs) = pipe(Flow::Inbound, 0, End::Synchronous).expect("a pipe");
        let ours = port.associate(ours, 9).expect("associated");
        let mut slot = PipeIo::new(64);
        assert!(matches!(slot.read(&ours), Started::Pending));
        File::from(theirs).write_all(b"two\n").expect("written");
        let mut completions = Completions::with_capacity(4);
        assert_eq!(port.wait(&mut completions, None).expect("waited"), 1);
        let completion = completions.iter().next().expect("one completion");
        assert_eq!(completion.key, 9);
        assert!(slot.owns(&completion));
        assert_eq!(slot.complete(&ours, &completion).expect("read"), 4);
        assert_eq!(slot.data(), b"two\n");
    }
}
