//! The transport for the pipes between verbatim.exe and its outposts,
//! listener and synthesizer hosts (`docs/design/communication.md`, section
//! 4): how a message is framed, how a thread reads an overlapped end,
//! blocking ([`BlockingReader`]), through a completion port
//! ([`PortReader`]), or by waiting on its event with others
//! ([`EventReader`], with [`EventWriter`] for its writes), and the
//! [`Sender`] through which verbatim.exe writes a child's commands.
//!
//! A frame is a text header giving the body's length, as the Language
//! Server Protocol's, then the body, a JSON message, and then, when the
//! header names one, a raw section of bytes the message gives the places
//! of: a terminal's text and colours, or a synthesizer host's samples
//! ([`Parts`]). The header is `Content-Length: ` and the body's length in
//! ten decimal digits; for a frame with a raw section, a line break and
//! `Raw-Length: ` with the raw section's length in ten digits; then a blank
//! line (`\r\n\r\n`). A frame without a raw section has no `Raw-Length`
//! line. The lengths have a fixed width so a frame can be encoded in
//! place, its body and raw section after a header whose digits are filled
//! in once their lengths are known; a reader takes any number of digits.
//! A frame is written with one write and read whole: the reader takes the
//! header, then asks for the whole rest of the frame at once. The end of a
//! pipe in the middle of a frame is an error, never a message.
//!
//! A [`Sender`] writes a frame inline, on the thread that sends it, when no
//! write is in flight and nothing waits ahead of it, so in the common case
//! no other thread wakes. A write the pipe cannot take at once pends; the
//! frames sent meanwhile wait in the sender's [`Backlog`], under the
//! caller's overflow policy, and the thread finishing its writes, woken
//! only by that write's completion, writes them in order: a thread of the
//! sender's own, or one serving a port the sender shares, as verbatim.exe's
//! relay does. A frame once started is always finished: nothing cancels a
//! write.

use std::io::{self, BufRead, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_TIMEOUT,
};
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::PeekNamedPipe;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::event::Event;
use crate::overlapped::{
    Completion, Completions, EventFile, PipeIo, Port, PortFile, Started, win32_error,
};

/// The largest frame, its body and raw section together, its header aside:
/// a terminal's output can be megabytes; anything larger is a fault.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// The buffer of an event pipe in the direction its data flows, so a focus
/// read with its ancestors, or a reply with a line of text, completes its
/// write at once (`docs/design/communication.md`, section 4.1).
pub const EVENT_PIPE_BUFFER: u32 = 64 * 1024;

/// What a header starts with, what starts its raw section's line, what
/// parts its lines, and what ends it.
const HEADER_NAME: &[u8] = b"Content-Length: ";
const RAW_NAME: &[u8] = b"Raw-Length: ";
const LINE_END: &[u8] = b"\r\n";
const HEADER_END: &[u8] = b"\r\n\r\n";

/// The width of each length a writer puts in the header.
const LENGTH_DIGITS: usize = 10;

/// The longest header a reader accepts.
const MAX_HEADER_BYTES: usize = 64;

/// A frame's two parts as a reader hands them out: its body, a JSON
/// message, and its raw section, `None` when its header names none. A
/// message that gives places in a raw section is decoded from both; every
/// other message from its body alone, refusing a raw section
/// ([`without_raw`](Self::without_raw)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts<'a> {
    /// The body.
    pub body: &'a [u8],
    /// The raw section, when the header names one, empty or not.
    pub raw: Option<&'a [u8]>,
}

impl<'a> Parts<'a> {
    /// A body with no raw section.
    #[must_use]
    pub fn body(body: &'a [u8]) -> Self {
        Self { body, raw: None }
    }

    /// The body of a frame that may not have a raw section.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::InvalidData`] when the frame has one.
    pub fn without_raw(self) -> io::Result<&'a [u8]> {
        match self.raw {
            None => Ok(self.body),
            Some(_) => Err(invalid(
                "a frame has a raw section its message does not take",
            )),
        }
    }
}

/// Appends a frame without a raw section to `buffer`: the header, then the
/// body `put_body` appends. What `buffer` held before is kept, so several
/// frames can be put in one buffer and written with one write
/// ([`Sender::send_all`]).
///
/// # Errors
///
/// Returns `put_body`'s error, or [`io::ErrorKind::InvalidInput`] for a
/// body larger than [`MAX_FRAME_BYTES`]; `buffer` is then left as it was.
pub fn put_frame(
    buffer: &mut Vec<u8>,
    put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    let start = buffer.len();
    let framed = append_frame(buffer, put_body, None::<fn(&mut Vec<u8>) -> io::Result<()>>);
    if framed.is_err() {
        buffer.truncate(start);
    }
    framed
}

/// Appends a frame with a raw section to `buffer`, as [`put_frame`] does:
/// the header with its `Raw-Length` line, the body `put_body` appends, then
/// the raw section `put_raw` appends after it, which may be empty.
///
/// # Errors
///
/// Returns `put_body`'s or `put_raw`'s error, or
/// [`io::ErrorKind::InvalidInput`] for a body and raw section larger
/// together than [`MAX_FRAME_BYTES`]; `buffer` is then left as it was.
pub fn put_frame_with_raw(
    buffer: &mut Vec<u8>,
    put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    put_raw: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    let start = buffer.len();
    let framed = append_frame(buffer, put_body, Some(put_raw));
    if framed.is_err() {
        buffer.truncate(start);
    }
    framed
}

fn append_frame(
    buffer: &mut Vec<u8>,
    put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
    put_raw: Option<impl FnOnce(&mut Vec<u8>) -> io::Result<()>>,
) -> io::Result<()> {
    buffer.extend_from_slice(HEADER_NAME);
    let digits = buffer.len();
    buffer.extend_from_slice(&[b'0'; LENGTH_DIGITS]);
    let raw_digits = put_raw.is_some().then(|| {
        buffer.extend_from_slice(LINE_END);
        buffer.extend_from_slice(RAW_NAME);
        let at = buffer.len();
        buffer.extend_from_slice(&[b'0'; LENGTH_DIGITS]);
        at
    });
    buffer.extend_from_slice(HEADER_END);
    let body = buffer.len();
    put_body(buffer)?;
    let length = buffer.len() - body;
    let raw_length = match put_raw {
        Some(put_raw) => {
            let raw = buffer.len();
            put_raw(buffer)?;
            buffer.len() - raw
        }
        None => 0,
    };
    let whole = length + raw_length;
    if whole > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a frame of {whole} bytes is larger than the {MAX_FRAME_BYTES} a frame may take"
            ),
        ));
    }
    let mut fill = |at: usize, length: usize| {
        let length = format!("{length:0LENGTH_DIGITS$}");
        buffer[at..at + LENGTH_DIGITS].copy_from_slice(length.as_bytes());
    };
    fill(digits, length);
    if let Some(at) = raw_digits {
        fill(at, raw_length);
    }
    Ok(())
}

/// A frame read whole and kept: the buffer it is in, its header ahead of
/// it when a reader gave its own buffer up, and where in it its body and
/// its raw section start.
#[derive(Debug, PartialEq, Eq)]
pub struct OwnedFrame {
    buffer: Vec<u8>,
    /// Where the body starts.
    start: usize,
    /// Where the raw section starts, after the body, when there is one; it
    /// runs to the buffer's end.
    raw: Option<usize>,
}

impl OwnedFrame {
    /// The frame whose body is `buffer` from `start` to `raw`, or to its
    /// end without a raw section, and whose raw section is the rest.
    fn new(buffer: Vec<u8>, start: usize, raw: Option<usize>) -> Self {
        Self { buffer, start, raw }
    }

    /// Its two parts.
    #[must_use]
    pub fn parts(&self) -> Parts<'_> {
        match self.raw {
            Some(raw) => Parts {
                body: &self.buffer[self.start..raw],
                raw: Some(&self.buffer[raw..]),
            },
            None => Parts::body(&self.buffer[self.start..]),
        }
    }
}

/// Reads one frame. Returns `Ok(None)` when the stream ends before a frame
/// begins: the writer closed its end.
///
/// # Errors
///
/// Returns an error for I/O failures; [`io::ErrorKind::UnexpectedEof`] when
/// the stream ends inside a frame; and [`io::ErrorKind::InvalidData`] for a
/// header that is not one, or lengths over [`MAX_FRAME_BYTES`], read no
/// further than the header.
pub fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<OwnedFrame>> {
    let Some(lengths) = read_header(reader)? else {
        return Ok(None);
    };
    let mut buffer = vec![0; lengths.whole()];
    reader.read_exact(&mut buffer)?;
    let raw = lengths.raw.map(|_| lengths.body);
    Ok(Some(OwnedFrame::new(buffer, 0, raw)))
}

/// The lengths a header gives: its body's, and its raw section's when it
/// names one.
#[derive(Clone, Copy, Debug)]
struct Lengths {
    body: usize,
    raw: Option<usize>,
}

impl Lengths {
    /// The body's and the raw section's together.
    fn whole(self) -> usize {
        self.body + self.raw.unwrap_or(0)
    }
}

/// Reads a header and returns the lengths it gives, or `None` at the end of
/// the stream before any of it.
fn read_header<R: BufRead>(reader: &mut R) -> io::Result<Option<Lengths>> {
    let mut header = Vec::with_capacity(MAX_HEADER_BYTES);
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            if header.is_empty() {
                return Ok(None);
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the stream ended inside a frame's header",
            ));
        }
        // Take bytes up to the header's end, if it is in view, so nothing
        // of the body is consumed.
        let mut taken = 0;
        let mut ended = false;
        for &byte in available {
            header.push(byte);
            taken += 1;
            if header.ends_with(HEADER_END) {
                ended = true;
                break;
            }
            if header.len() > MAX_HEADER_BYTES {
                break;
            }
        }
        reader.consume(taken);
        if ended {
            return parse_header(&header).map(Some);
        }
        if header.len() > MAX_HEADER_BYTES {
            return Err(invalid("a frame's header is too long"));
        }
    }
}

/// The lengths a complete header gives.
fn parse_header(header: &[u8]) -> io::Result<Lengths> {
    let not_one = || invalid("a frame's header is not a Content-Length header");
    let lines = header
        .strip_prefix(HEADER_NAME)
        .and_then(|rest| rest.strip_suffix(HEADER_END))
        .ok_or_else(not_one)?;
    let (body, raw) = match lines
        .windows(LINE_END.len())
        .position(|candidate| candidate == LINE_END)
    {
        None => (lines, None),
        Some(at) => {
            let raw = lines[at + LINE_END.len()..]
                .strip_prefix(RAW_NAME)
                .ok_or_else(not_one)?;
            (&lines[..at], Some(raw))
        }
    };
    let length = |digits: &[u8]| {
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return Err(not_one());
        }
        std::str::from_utf8(digits)
            .ok()
            .and_then(|digits| digits.parse::<usize>().ok())
            .filter(|&length| length <= MAX_FRAME_BYTES)
            .ok_or_else(too_large)
    };
    let lengths = Lengths {
        body: length(body)?,
        raw: raw.map(length).transpose()?,
    };
    if lengths.whole() > MAX_FRAME_BYTES {
        return Err(too_large());
    }
    Ok(lengths)
}

fn too_large() -> io::Error {
    invalid(&format!(
        "a frame is larger than the {MAX_FRAME_BYTES} bytes a frame may take"
    ))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// An overlapped pipe end read by a thread that blocks until each read
/// completes, as a [`Read`]: what a test's harness or a stand-in child
/// reads a pipe through. The end of the pipe, its writer having closed
/// it, reads as the end of the stream.
pub struct BlockingReader {
    pipe: OwnedHandle,
    /// The event each read's completion signals.
    event: OwnedHandle,
}

impl BlockingReader {
    /// Reads `pipe`, an end opened for overlapped I/O and not associated
    /// with a completion port.
    ///
    /// # Errors
    ///
    /// Returns the error if the event a read waits on cannot be created.
    pub fn new(pipe: OwnedHandle) -> io::Result<Self> {
        Ok(Self {
            pipe,
            event: manual_reset_event()?,
        })
    }
}

impl BlockingReader {
    /// Reads into `buffer` as [`Read::read`] does, but waits at most
    /// `timeout`: `None` when nothing came in that time, the read then
    /// cancelled (a read may be; a write never is). The end of the pipe
    /// reads as `Some(0)`.
    ///
    /// # Errors
    ///
    /// Returns the read's error.
    pub fn read_within(
        &mut self,
        buffer: &mut [u8],
        timeout: std::time::Duration,
    ) -> io::Result<Option<usize>> {
        blocking_io(&self.pipe, &self.event, Io::Read(buffer), Some(timeout))
    }

    /// How many bytes wait in the pipe, read by nobody yet: what a test
    /// that has had the writer's word that everything is written checks to
    /// know nothing more was said.
    ///
    /// # Errors
    ///
    /// Returns the error of the peek; a broken pipe, the writer gone with
    /// nothing left, is [`io::ErrorKind::BrokenPipe`].
    pub fn available(&self) -> io::Result<usize> {
        let mut available = 0u32;
        // SAFETY: an open pipe handle; only the count is written.
        unsafe {
            PeekNamedPipe(
                HANDLE(self.pipe.as_raw_handle()),
                None,
                0,
                None,
                Some(&raw mut available),
                None,
            )
        }
        .map_err(win32_error)?;
        Ok(available as usize)
    }
}

impl Read for BlockingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        blocking_io(&self.pipe, &self.event, Io::Read(buffer), None).map(Option::unwrap_or_default)
    }
}

/// An overlapped pipe end written by a thread that blocks until each write
/// completes, as a [`std::io::Write`]: for a child that writes one message
/// at a time and has no completion port, such as the stand-in children of
/// the supervisor's tests.
pub struct BlockingWriter {
    pipe: OwnedHandle,
    /// The event each write's completion signals.
    event: OwnedHandle,
}

impl BlockingWriter {
    /// Writes `pipe`, an end opened for overlapped I/O and not associated
    /// with a completion port.
    ///
    /// # Errors
    ///
    /// Returns the error if the event a write waits on cannot be created.
    pub fn new(pipe: OwnedHandle) -> io::Result<Self> {
        Ok(Self {
            pipe,
            event: manual_reset_event()?,
        })
    }
}

impl std::io::Write for BlockingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        blocking_io(&self.pipe, &self.event, Io::Write(bytes), None).map(Option::unwrap_or_default)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// An unnamed manual-reset event, owned.
fn manual_reset_event() -> io::Result<OwnedHandle> {
    // SAFETY: a manual-reset event, unnamed, with default security; the
    // handle returned is owned below.
    let event = unsafe { CreateEventW(None, true, false, None) }.map_err(win32_error)?;
    // SAFETY: a valid handle just returned, owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(event.0 as RawHandle) })
}

/// One blocking operation on an overlapped end.
enum Io<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}

/// Runs `io` on `pipe` and waits for it, at most `timeout` for a read:
/// how many bytes it moved, `None` for a read that timed out and was
/// cancelled. A broken pipe moves nothing.
fn blocking_io(
    pipe: &OwnedHandle,
    event: &OwnedHandle,
    io: Io<'_>,
    timeout: Option<std::time::Duration>,
) -> io::Result<Option<usize>> {
    let pipe = HANDLE(pipe.as_raw_handle());
    let mut overlapped = OVERLAPPED {
        hEvent: HANDLE(event.as_raw_handle()),
        ..OVERLAPPED::default()
    };
    // The buffer and the OVERLAPPED outlive the operation: whether it
    // completes at once or pends, this function returns only once
    // GetOverlappedResult, told to wait, has seen it finish, after
    // cancelling a read that timed out.
    let started = match io {
        // SAFETY: as above.
        Io::Read(buffer) => unsafe {
            ReadFile(pipe, Some(buffer), None, Some(&raw mut overlapped))
        },
        // SAFETY: as above.
        Io::Write(bytes) => unsafe {
            WriteFile(pipe, Some(bytes), None, Some(&raw mut overlapped))
        },
    };
    match started {
        Ok(()) => {}
        Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {
            if let Some(timeout) = timeout {
                let milliseconds = crate::overlapped::wait_milliseconds(timeout);
                // SAFETY: the event is open; the wait only reads it.
                let waited =
                    unsafe { WaitForSingleObject(HANDLE(event.as_raw_handle()), milliseconds) };
                if waited == WAIT_TIMEOUT {
                    // SAFETY: the operation started above on this handle
                    // with this OVERLAPPED; cancelling a read loses no
                    // data, and the wait below sees it finish.
                    let _ = unsafe { CancelIoEx(pipe, Some(&raw const overlapped)) };
                }
            }
        }
        Err(error) if error.code() == ERROR_BROKEN_PIPE.to_hresult() => return Ok(Some(0)),
        Err(error) => return Err(win32_error(error)),
    }
    let mut moved = 0u32;
    // SAFETY: the operation started above on this handle with this
    // OVERLAPPED; the call waits for it to finish.
    match unsafe { GetOverlappedResult(pipe, &raw const overlapped, &raw mut moved, true) } {
        Ok(()) => Ok(Some(moved as usize)),
        Err(error) if error.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(Some(0)),
        Err(error) if error.code() == ERROR_OPERATION_ABORTED.to_hresult() => Ok(None),
        Err(error) => Err(win32_error(error)),
    }
}

/// How many bytes a [`PortReader`] asks for when it does not know how long
/// the frame it is reading is: the event pipes' buffer.
const READ_SIZE: usize = EVENT_PIPE_BUFFER as usize;

/// How many reads that complete at once a [`PortReader`] makes in one turn
/// before it lets the rest of its port have one.
const READS_PER_TURN: usize = 16;

/// An overlapped end read through a completion port by the thread serving
/// the port, one read in flight, each frame's parts handed out whole and in
/// order (`docs/design/communication.md`, sections 4.1 and 4.8). A read
/// asks for the pipe's buffer while the next frame's length is unknown, and
/// for the whole rest of a frame once it is known, so a large frame is read
/// in one read into a buffer of its size, however the pipe driver splits
/// it. The end of the pipe between frames ends the reading; inside a frame
/// it is an error, never a message.
pub struct PortReader {
    file: PortFile,
    slot: PipeIo,
    /// Bytes read and not yet handed out: the start of a frame whose rest
    /// has not arrived.
    pending: Vec<u8>,
}

/// A frame as a [`PortReader`] hands it out
/// ([`take_bodies`](PortReader::take_bodies)): lent from the reader's
/// buffer, or, when the reader gathered the frame in a buffer of its own
/// and nothing after it, that buffer itself, so a caller that keeps the
/// body need not copy it.
#[derive(Debug)]
pub enum FrameBody<'a> {
    /// Lent for the call: a caller that keeps it copies it.
    Borrowed(Parts<'a>),
    /// The reader's buffer, given up: the whole frame, its header and then
    /// its body and raw section. Only a frame gathered over more than one
    /// read is handed out this way, a large one above all.
    Owned(OwnedFrame),
}

impl FrameBody<'_> {
    /// The frame's body and raw section.
    #[must_use]
    pub fn parts(&self) -> Parts<'_> {
        match self {
            FrameBody::Borrowed(parts) => *parts,
            FrameBody::Owned(frame) => frame.parts(),
        }
    }

    /// The frame as one the caller keeps: the reader's own buffer when it
    /// was given up, a copy of a lent frame otherwise.
    #[must_use]
    pub fn into_owned(self) -> OwnedFrame {
        match self {
            FrameBody::Borrowed(Parts { body, raw }) => {
                let mut buffer = Vec::with_capacity(body.len() + raw.map_or(0, <[u8]>::len));
                buffer.extend_from_slice(body);
                if let Some(raw) = raw {
                    buffer.extend_from_slice(raw);
                }
                OwnedFrame::new(buffer, 0, raw.map(|_| body.len()))
            }
            FrameBody::Owned(frame) => frame,
        }
    }

    /// Whether the reader's buffer was given up rather than lent.
    #[must_use]
    pub fn is_owned(&self) -> bool {
        matches!(self, FrameBody::Owned(_))
    }
}

/// What became of a [`PortReader`] in a turn.
#[derive(Debug)]
pub enum Reading {
    /// A read is in flight; its completion will come through the port.
    Waiting,
    /// Reads kept completing at once, so the reader posted its key to its
    /// port, to go on after whatever else waits there.
    Yielded,
    /// The reading ended, with nothing in flight: `Ok` when the writer
    /// closed its end between frames; the error when the pipe failed, ended
    /// inside a frame, or brought something that is not a frame, or when
    /// the caller refused a frame.
    Ended(io::Result<()>),
}

impl PortReader {
    /// Reads `file`, a pipe end associated with a port. Nothing is read
    /// until the first [`take`](Self::take).
    #[must_use]
    pub fn new(file: PortFile) -> Self {
        Self {
            file,
            slot: PipeIo::new(READ_SIZE),
            pending: Vec::new(),
        }
    }

    /// The key its completions carry.
    #[must_use]
    pub fn key(&self) -> usize {
        self.file.key()
    }

    /// Takes a turn on `completion`, carrying this reader's key: its read's
    /// completion, or a wake posted with its key, which starts the reading
    /// (or goes on with it after [`Reading::Yielded`]) unless a read is
    /// already in flight. Hands `frame` the parts of each whole frame read,
    /// in order, and reads on while reads complete at once, until one
    /// waits, `READS_PER_TURN` have been made, or the reading ends. A
    /// frame `frame` refuses ends the reading with its error.
    pub fn take(
        &mut self,
        completion: &Completion,
        port: &Port,
        mut frame: impl FnMut(Parts<'_>) -> io::Result<()>,
    ) -> Reading {
        self.take_bodies(completion, port, |body| frame(body.parts()))
    }

    /// [`take`](Self::take), handing out each body as a [`FrameBody`]: a
    /// frame that was gathered in the reader's buffer over several reads,
    /// with nothing of the next frame after it, is handed out as that
    /// buffer, given up, rather than lent, so a caller that keeps a large
    /// frame does not copy it again.
    pub fn take_bodies(
        &mut self,
        completion: &Completion,
        port: &Port,
        mut frame: impl FnMut(FrameBody<'_>) -> io::Result<()>,
    ) -> Reading {
        let mut done = if completion.is_posted() {
            if self.slot.in_flight() {
                return Reading::Waiting;
            }
            None
        } else if self.slot.in_flight() && self.slot.owns(completion) {
            Some(self.slot.complete(&self.file, completion))
        } else {
            tracing::error!(
                key = completion.key,
                "a completion that is not this reader's"
            );
            return Reading::Waiting;
        };
        let mut reads = 0;
        loop {
            if let Some(done) = done.take() {
                let handed = match done {
                    Ok(0) => return Reading::Ended(self.end()),
                    Ok(_) => self.hand_out(&mut frame),
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                        return Reading::Ended(self.end());
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = handed {
                    return Reading::Ended(Err(error));
                }
            }
            if reads == READS_PER_TURN {
                return match port.post(self.file.key()) {
                    Ok(()) => Reading::Yielded,
                    Err(error) => Reading::Ended(Err(error)),
                };
            }
            reads += 1;
            match self.slot.read_up_to(&self.file, self.wanted()) {
                Started::Pending => return Reading::Waiting,
                Started::Done(moved) => done = Some(Ok(moved)),
                Started::Failed(error) => done = Some(Err(error)),
            }
        }
    }

    /// How much the next read asks for: the rest of the frame begun, when
    /// its length is known and more than the pipe's buffer.
    fn wanted(&self) -> usize {
        match frame_bounds(&self.pending) {
            Ok(Some((header, lengths))) => (header + lengths.whole())
                .saturating_sub(self.pending.len())
                .max(READ_SIZE),
            _ => READ_SIZE,
        }
    }

    /// Hands out every frame the last read completed, keeping the start of
    /// the next. A frame the reads gathered in `pending` that fills it
    /// exactly is handed out as `pending` itself, given up; every other
    /// frame is lent.
    fn hand_out(
        &mut self,
        frame: &mut impl FnMut(FrameBody<'_>) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut lend = |parts: Parts<'_>| frame(FrameBody::Borrowed(parts));
        let data = self.slot.data();
        if self.pending.is_empty() {
            let used = whole_frames(data, &mut lend)?;
            self.pending.extend_from_slice(&data[used..]);
        } else {
            self.pending.extend_from_slice(data);
            if let Some((header, lengths)) = frame_bounds(&self.pending)?
                && header + lengths.whole() == self.pending.len()
            {
                let buffer = std::mem::take(&mut self.pending);
                let raw = lengths.raw.map(|_| header + lengths.body);
                return frame(FrameBody::Owned(OwnedFrame::new(buffer, header, raw)));
            }
            let used = whole_frames(&self.pending, &mut lend)?;
            self.pending.drain(..used);
        }
        if self.pending.is_empty() && self.pending.capacity() > READ_SIZE {
            // A large frame's memory, given back.
            self.pending = Vec::new();
        }
        Ok(())
    }

    /// The writer closed its end: a clean end between frames, an error
    /// inside one.
    fn end(&self) -> io::Result<()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the pipe ended inside a frame",
            ))
        }
    }
}

/// Hands `frame` the parts of every whole frame at the start of `bytes`, in
/// order, and returns how many bytes they took.
fn whole_frames(
    bytes: &[u8],
    frame: &mut impl FnMut(Parts<'_>) -> io::Result<()>,
) -> io::Result<usize> {
    let mut at = 0;
    while let Some((header, lengths)) = frame_bounds(&bytes[at..])? {
        let end = at + header + lengths.whole();
        if end > bytes.len() {
            break;
        }
        let body = at + header;
        let raw_start = body + lengths.body;
        frame(Parts {
            body: &bytes[body..raw_start],
            raw: lengths.raw.map(|_| &bytes[raw_start..end]),
        })?;
        at = end;
    }
    Ok(at)
}

/// The length of the header of the frame `bytes` starts with, and the
/// lengths it gives, once the header is whole; `None` before.
fn frame_bounds(bytes: &[u8]) -> io::Result<Option<(usize, Lengths)>> {
    let window = &bytes[..bytes.len().min(MAX_HEADER_BYTES)];
    match window
        .windows(HEADER_END.len())
        .position(|candidate| candidate == HEADER_END)
    {
        Some(at) => {
            let header = at + HEADER_END.len();
            parse_header(&bytes[..header]).map(|length| Some((header, length)))
        }
        None if bytes.len() >= MAX_HEADER_BYTES => Err(invalid("a frame's header is too long")),
        None => Ok(None),
    }
}

/// An overlapped end read by a thread that waits on its event together
/// with other events ([`crate::event::wait_any`]), one read in flight, each
/// frame's parts handed out whole and in order: how the synth thread reads
/// its host's frames, and a synthesizer host Core's commands
/// (`docs/design/communication.md`, sections 7.3 and 7.4). As a
/// [`PortReader`] does, a read asks for `read_size` bytes while the next
/// frame's length is unknown and for the whole rest of a frame once it is
/// known; the end of the pipe between frames ends the reading, and inside a
/// frame it is an error.
///
/// It reads only when asked to and nothing whole is left:
/// [`take`](Self::take) hands out the frames a read brought and starts no
/// other read until it is called again, so a reader that handles each
/// frame before it takes more reads no faster than it handles, which is
/// what lets a small pipe pace its writer.
pub struct EventReader {
    file: EventFile,
    slot: PipeIo,
    read_size: usize,
    /// Bytes read and not yet handed out: the start of a frame whose rest
    /// has not arrived.
    pending: Vec<u8>,
}

/// What became of an [`EventReader`] in a call to
/// [`take`](EventReader::take).
#[derive(Debug)]
pub enum Taken {
    /// Whole frames were handed out, and no read is in flight: the reader
    /// is called again before its event is waited on.
    Frames,
    /// A read is in flight; the reader's event is set when it finishes.
    Waiting,
    /// The reading ended, with nothing in flight: `Ok` when the writer
    /// closed its end between frames; the error when the pipe failed, ended
    /// inside a frame, or brought something that is not a frame, or when
    /// the caller refused a frame. The reader is not called again.
    Ended(io::Result<()>),
}

impl EventReader {
    /// Reads `pipe`, an end opened for overlapped I/O and associated with
    /// no port, asking for `read_size` bytes while a frame's length is
    /// unknown. Nothing is read until the first [`take`](Self::take).
    ///
    /// # Errors
    ///
    /// Returns the error if the event its reads signal cannot be created.
    pub fn new(pipe: OwnedHandle, read_size: usize) -> io::Result<Self> {
        Ok(Self {
            file: EventFile::new(pipe)?,
            slot: PipeIo::new(read_size),
            read_size,
            pending: Vec::new(),
        })
    }

    /// The event the read in flight sets when it finishes.
    #[must_use]
    pub fn event(&self) -> &Arc<Event> {
        self.file.event()
    }

    /// Hands `frame` the parts of every whole frame the reads so far
    /// brought, in order, reading first, never waiting, while a read in
    /// flight has finished or a new one finishes at once and nothing whole
    /// has come yet. A frame `frame` refuses ends the reading with its
    /// error.
    pub fn take(&mut self, mut frame: impl FnMut(Parts<'_>) -> io::Result<()>) -> Taken {
        loop {
            let done = if self.slot.in_flight() {
                match self.slot.poll(&self.file) {
                    None => return Taken::Waiting,
                    Some(done) => done,
                }
            } else {
                match self.slot.read_up_to(&self.file, self.wanted()) {
                    Started::Pending => return Taken::Waiting,
                    Started::Done(moved) => Ok(moved),
                    Started::Failed(error) => Err(error),
                }
            };
            match done {
                Ok(0) => return Taken::Ended(self.end()),
                Ok(_) => self.pending.extend_from_slice(self.slot.data()),
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                    return Taken::Ended(self.end());
                }
                Err(error) => return Taken::Ended(Err(error)),
            }
            match whole_frames(&self.pending, &mut frame) {
                Ok(0) => {}
                Ok(used) => {
                    self.pending.drain(..used);
                    if self.pending.is_empty() && self.pending.capacity() > self.read_size {
                        // A large frame's memory, given back.
                        self.pending = Vec::new();
                    }
                    return Taken::Frames;
                }
                Err(error) => return Taken::Ended(Err(error)),
            }
        }
    }

    /// How much the next read asks for: the rest of the frame begun, when
    /// its length is known and more than `read_size`.
    fn wanted(&self) -> usize {
        match frame_bounds(&self.pending) {
            Ok(Some((header, lengths))) => (header + lengths.whole())
                .saturating_sub(self.pending.len())
                .max(self.read_size),
            _ => self.read_size,
        }
    }

    /// The writer closed its end: a clean end between frames, an error
    /// inside one.
    fn end(&self) -> io::Result<()> {
        if self.pending.is_empty() {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the pipe ended inside a frame",
            ))
        }
    }
}

impl Drop for EventReader {
    fn drop(&mut self) {
        // A read loses nothing by being cancelled; waiting for it to end
        // lets the slot's memory be freed.
        self.slot.cancel_read(&self.file);
    }
}

/// An overlapped end written one write at a time by a thread that waits
/// for each write's completion, or on its event together with other
/// events ([`crate::event::wait_any`]): a synthesizer host's paced writes
/// to Core, and the synth thread's commands to its host
/// (`docs/design/communication.md`, sections 4.3 and 7). A write once
/// started is always finished: nothing here cancels one, and a writer
/// dropped with a write in flight leaves its memory to the kernel rather
/// than free it, as [`PipeIo`] does.
pub struct EventWriter {
    file: EventFile,
    slot: PipeIo,
    /// The length of the write in flight.
    writing: usize,
}

impl EventWriter {
    /// Writes `pipe`, an end opened for overlapped I/O and associated with
    /// no port.
    ///
    /// # Errors
    ///
    /// Returns the error if the event its writes signal cannot be created.
    pub fn new(pipe: OwnedHandle) -> io::Result<Self> {
        Ok(Self {
            file: EventFile::new(pipe)?,
            slot: PipeIo::new(0),
            writing: 0,
        })
    }

    /// The event the write in flight sets when it finishes.
    #[must_use]
    pub fn event(&self) -> &Arc<Event> {
        self.file.event()
    }

    /// Whether a write is in flight.
    #[must_use]
    pub fn in_flight(&self) -> bool {
        self.slot.in_flight()
    }

    /// Starts one write of the frames `put` appends ([`put_frame`]), with
    /// one `WriteFile`: `true` when it completed at once, `false` when it
    /// is in flight, to be finished by [`poll`](Self::poll) or
    /// [`finish`](Self::finish).
    ///
    /// # Errors
    ///
    /// Returns `put`'s error, with nothing written, or the write's.
    ///
    /// # Panics
    ///
    /// Panics if a write is in flight.
    pub fn start(&mut self, put: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<bool> {
        let mut length = 0;
        let started = self.slot.write_with(&self.file, |buffer| {
            put(buffer)?;
            length = buffer.len();
            Ok::<(), io::Error>(())
        })?;
        self.writing = length;
        match started {
            Started::Done(moved) => self.check(moved).map(|()| true),
            Started::Pending => Ok(false),
            Started::Failed(error) => Err(error),
        }
    }

    /// Whether the write in flight has finished, with a zero wait: `None`
    /// while it runs, and its outcome once it has.
    ///
    /// # Panics
    ///
    /// Panics if no write is in flight.
    pub fn poll(&mut self) -> Option<io::Result<()>> {
        let done = self.slot.poll(&self.file)?;
        Some(done.and_then(|moved| self.check(moved)))
    }

    /// Waits for the write in flight, if any, to finish.
    ///
    /// # Errors
    ///
    /// Returns the write's error.
    pub fn finish(&mut self) -> io::Result<()> {
        if !self.slot.in_flight() {
            return Ok(());
        }
        let moved = self.slot.wait(&self.file)?;
        self.check(moved)
    }

    /// Writes the frames `put` appends and waits until the write has
    /// finished.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start) and [`finish`](Self::finish).
    pub fn write(&mut self, put: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> io::Result<()> {
        if self.start(put)? {
            Ok(())
        } else {
            self.finish()
        }
    }

    /// A finished write moved every byte: a write to a byte-mode pipe that
    /// waits finishes only once the pipe has taken all of it.
    fn check(&self, moved: usize) -> io::Result<()> {
        if moved == self.writing {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("a write of {} bytes moved {moved}", self.writing),
            ))
        }
    }
}

/// What a [`Sender`] holds while a write is in flight, under the caller's
/// overflow policy, and how each item is encoded. Every item sent passes
/// through it, even one written at once, so the policy sees every item in
/// the order it was sent and every frame in the order it is written.
pub trait Backlog: Send + 'static {
    /// One frame's content, encoded only when it is written.
    type Item: Send;
    /// Why the policy refused an item.
    type Refused;

    /// Adds `item` under the policy, or refuses it.
    ///
    /// # Errors
    ///
    /// Returns the policy's refusal.
    fn push(&mut self, item: Self::Item) -> Result<(), Self::Refused>;

    /// The next item to write.
    fn pop(&mut self) -> Option<Self::Item>;

    /// Whether nothing waits.
    fn is_empty(&self) -> bool;

    /// Appends `item`'s frame to `buffer` ([`put_frame`]) just before it is
    /// written, whichever thread writes it, so a time stamped then is the
    /// write's, and frames are encoded in the order they are written. An
    /// item that appends nothing is a marker: it is reached only once every
    /// write before it has completed.
    ///
    /// # Errors
    ///
    /// Returns the encoding's error; the item is then skipped.
    fn encode(&mut self, item: Self::Item, buffer: &mut Vec<u8>) -> io::Result<()>;
}

/// Why a [`Sender`] did not take an item.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError<R> {
    /// The backlog's policy refused it.
    Refused(R),
    /// The sender is closed: it was closed, or a write failed, the child
    /// having gone.
    Closed,
}

/// The key a sender with a thread of its own gives its pipe on its own
/// port; its close is posted with the same key.
const OWN_KEY: usize = 1;

/// One writing end, shared by every thread that sends on it and by the
/// thread that finishes its writes that pend: a thread of its own
/// ([`start`](Self::start)), or one serving a port it shares
/// ([`on_port`](Self::on_port)), which hands it each completion carrying
/// its key ([`complete`](Self::complete)).
pub struct Sender<B: Backlog> {
    shared: Arc<Shared<B>>,
}

impl<B: Backlog> Clone for Sender<B> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

struct Shared<B: Backlog> {
    state: Mutex<State<B>>,
    /// Where a write that pended completes.
    port: Arc<Port>,
    /// The key the pipe's completions carry.
    key: usize,
    /// Whether a thread of the sender's own waits on the port, which a
    /// close or a failure must wake.
    own_thread: bool,
}

struct State<B: Backlog> {
    backlog: B,
    /// The one write slot.
    slot: PipeIo,
    /// The pipe, until the sender has closed and written everything,
    /// when it is dropped so the reader sees the end of the stream.
    pipe: Option<PortFile>,
    /// No more items are taken.
    closed: bool,
    /// A write failed, the reader having gone.
    failed: bool,
}

impl<B: Backlog> Sender<B> {
    /// A sender writing `pipe`, an overlapped end, holding frames in
    /// `backlog` while a write is in flight, whose writes that pend complete
    /// on `port` under `key`; the thread serving `port` hands each such
    /// completion to [`complete`](Self::complete).
    ///
    /// # Errors
    ///
    /// Returns the error of associating the pipe with the port.
    pub fn on_port(pipe: OwnedHandle, backlog: B, port: Arc<Port>, key: usize) -> io::Result<Self> {
        Self::make(pipe, backlog, port, key, false)
    }

    /// A sender as [`on_port`](Self::on_port) makes one, on a port of its
    /// own served by a finishing thread named `name`, which first calls
    /// `started` (to set its priority) and ends once the sender has
    /// finished, so joining it waits for every write to complete.
    ///
    /// # Errors
    ///
    /// Returns the error of making the completion port, associating the
    /// pipe with it, or spawning the thread.
    pub fn start(
        pipe: OwnedHandle,
        backlog: B,
        name: &str,
        started: impl FnOnce() + Send + 'static,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let port = Arc::new(Port::new()?);
        let sender = Self::make(pipe, backlog, Arc::clone(&port), OWN_KEY, true)?;
        let finishing = sender.clone();
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || {
                started();
                let mut completions = Completions::with_capacity(4);
                while !finishing.is_finished() {
                    if port.wait(&mut completions, None).is_err() {
                        return;
                    }
                    for completion in completions.iter() {
                        finishing.complete(&completion);
                    }
                }
            })?;
        Ok((sender, thread))
    }

    fn make(
        pipe: OwnedHandle,
        backlog: B,
        port: Arc<Port>,
        key: usize,
        own_thread: bool,
    ) -> io::Result<Self> {
        let pipe = port.associate(pipe, key)?;
        Ok(Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    backlog,
                    slot: PipeIo::new(0),
                    pipe: Some(pipe),
                    closed: false,
                    failed: false,
                }),
                port,
                key,
                own_thread,
            }),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State<B>> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The key this sender's completions carry on its port.
    #[must_use]
    pub fn key(&self) -> usize {
        self.shared.key
    }

    /// Sends `item`: it joins the backlog, and is written at once when no
    /// write is in flight, on this thread, so in the common case no other
    /// thread wakes. Never waits for the pipe.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Closed`] once the sender is closed, or when the
    /// write fails, the reader having gone; and the backlog's refusal when
    /// it refuses the item.
    pub fn send(&self, item: B::Item) -> Result<(), SendError<B::Refused>> {
        let mut state = self.lock();
        if state.closed {
            return Err(SendError::Closed);
        }
        state.backlog.push(item).map_err(SendError::Refused)?;
        state.drain();
        self.settled(&mut state)
    }

    /// Sends `items` together. When no write is in flight, their frames are
    /// put in one buffer and written with one write, each frame whole with
    /// its own header, so the reader wakes once for all of them; a batch
    /// larger than the pipe's buffer pends and is finished whole, as one
    /// large frame is. While a write is in flight they join the backlog, as
    /// [`send`](Self::send) would add them, and are written after it one by
    /// one. Never waits for the pipe.
    ///
    /// # Errors
    ///
    /// As [`send`](Self::send). When the backlog refuses an item, the items
    /// before it are still sent and the rest are not.
    ///
    /// A marker in a batch is reached before the batch is written, so
    /// markers are sent on their own.
    pub fn send_all(
        &self,
        items: impl IntoIterator<Item = B::Item>,
    ) -> Result<(), SendError<B::Refused>> {
        let mut state = self.lock();
        if state.closed {
            return Err(SendError::Closed);
        }
        let mut refused = None;
        for item in items {
            if let Err(refusal) = state.backlog.push(item) {
                refused = Some(refusal);
                break;
            }
        }
        if !state.slot.in_flight() {
            // Nothing is in flight, so nothing waited before these: what
            // waits now is this batch.
            state.write_all();
        }
        let settled = self.settled(&mut state);
        match refused {
            Some(refused) => Err(SendError::Refused(refused)),
            None => settled,
        }
    }

    /// Runs `inspect` on the backlog, under the sender's lock, so nothing is
    /// sent or written meanwhile.
    pub fn with_backlog<R>(&self, inspect: impl FnOnce(&mut B) -> R) -> R {
        inspect(&mut self.lock().backlog)
    }

    /// Closes the sender: nothing more is taken, what waits is still
    /// written, and then the pipe is closed, so the child reads the end of
    /// the stream.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        self.settle(&mut state);
    }

    /// Takes `completion`, carrying this sender's key, from the thread
    /// serving its port: finishes the write that pended, writes what waited
    /// meanwhile, and drops the pipe once the sender is closed and
    /// everything is written. A posted wake-up is only looked at.
    pub fn complete(&self, completion: &Completion) {
        let mut state = self.lock();
        let State { slot, pipe, .. } = &mut *state;
        if let Some(pipe) = pipe
            && !completion.is_posted()
            && slot.owns(completion)
            && let Err(error) = slot.complete(pipe, completion)
        {
            tracing::debug!(%error, "a write failed; the pipe is closed");
            state.fail();
        }
        state.drain();
        self.settle(&mut state);
    }

    /// Whether the sender has closed and written everything, its pipe
    /// dropped: no completion of its will come again.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.lock().pipe.is_none()
    }

    /// [`settle`](Self::settle)s, then says whether what was sent was
    /// taken: not when a write failed meanwhile, the sender then closed.
    fn settled(&self, state: &mut State<B>) -> Result<(), SendError<B::Refused>> {
        let failed = state.failed;
        self.settle(state);
        if failed {
            Err(SendError::Closed)
        } else {
            Ok(())
        }
    }

    /// Drops the pipe once the sender is closed and nothing is in flight or
    /// waits, and wakes a finishing thread of its own to see it.
    fn settle(&self, state: &mut State<B>) {
        if state.finished() && state.pipe.take().is_some() && self.shared.own_thread {
            let _ = self.shared.port.post(self.shared.key);
        }
    }
}

impl<B: Backlog> State<B> {
    /// Writes `item`. Returns false when the pipe has failed, the sender
    /// being closed and its backlog dropped.
    fn write(&mut self, item: B::Item) -> bool {
        let Self {
            backlog,
            slot,
            pipe,
            ..
        } = self;
        let Some(pipe) = pipe else {
            return false;
        };
        let mut length = 0;
        let started = slot.write_with(pipe, |buffer| {
            backlog.encode(item, buffer)?;
            length = buffer.len();
            Ok::<(), io::Error>(())
        });
        self.started(started, length)
    }

    /// Writes every item waiting, each frame after the one before it in one
    /// buffer, with one write. Returns false when the pipe has failed.
    fn write_all(&mut self) -> bool {
        let Self {
            backlog,
            slot,
            pipe,
            ..
        } = self;
        let Some(pipe) = pipe else {
            return false;
        };
        let mut length = 0;
        let started = slot.write_with(pipe, |buffer| {
            while let Some(item) = backlog.pop() {
                let before = buffer.len();
                if let Err(error) = backlog.encode(item, buffer) {
                    buffer.truncate(before);
                    tracing::warn!(%error, "a frame could not be encoded and is skipped");
                }
            }
            length = buffer.len();
            Ok::<(), io::Error>(())
        });
        self.started(started, length)
    }

    /// What became of a write of `length` bytes, started or not: false when
    /// the pipe has failed.
    fn started(&mut self, started: io::Result<Started>, length: usize) -> bool {
        match started {
            Ok(Started::Done(moved)) if moved == length => true,
            Ok(Started::Pending) => true,
            Ok(Started::Done(moved)) => {
                tracing::warn!(
                    moved,
                    length,
                    "a write took part of a frame; the pipe is closed"
                );
                self.fail();
                false
            }
            Ok(Started::Failed(error)) => {
                tracing::debug!(%error, "a write failed; the pipe is closed");
                self.fail();
                false
            }
            Err(error) => {
                tracing::warn!(%error, "a frame could not be encoded and is skipped");
                true
            }
        }
    }

    /// The pipe failed: nothing more is taken or written.
    fn fail(&mut self) {
        self.closed = true;
        self.failed = true;
        while self.backlog.pop().is_some() {}
    }

    /// Writes waiting items until one pends or none is left.
    fn drain(&mut self) {
        while !self.slot.in_flight() {
            let Some(item) = self.backlog.pop() else {
                return;
            };
            if !self.write(item) {
                return;
            }
        }
    }

    /// Whether everything is written and nothing more will be.
    fn finished(&self) -> bool {
        self.closed && !self.slot.in_flight() && self.backlog.is_empty()
    }
}

/// The key a child's command pipe carries on its I/O thread's port.
const COMMANDS_KEY: usize = 1;

/// The key its message pipe's writes carry there.
const MESSAGES_KEY: usize = 2;

/// What a child's I/O thread does after a command ([`ChildIo::serve`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serving {
    /// Goes on reading commands.
    Continue,
    /// Stops: the command was the last, the shutdown.
    Stop,
}

/// Why [`ChildIo::serve`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    /// The command handler said to stop.
    Stopped,
    /// verbatim.exe closed the command pipe between frames.
    CommandsEnded,
}

/// A child's side of its two pipes (`docs/design/communication.md`,
/// sections 4.6 and 6.3 to 6.4): one I/O thread, waiting only on a
/// completion port, reads verbatim.exe's commands and finishes the writes
/// to verbatim.exe that pended, while every thread that produces a message
/// writes it itself through the [`Sender`] [`new`](Self::new) returns. A
/// child calls it once at startup with its two inherited overlapped ends.
pub struct ChildIo {
    port: Arc<Port>,
    commands: PortReader,
    completions: Completions,
}

impl ChildIo {
    /// The I/O side over `commands`, the end the child reads, and the
    /// sender for `messages`, the end it writes, holding frames in
    /// `backlog` while a write is in flight. Both ends must be overlapped.
    ///
    /// # Errors
    ///
    /// Returns the error of making the port or associating either end.
    pub fn new<B: Backlog>(
        commands: OwnedHandle,
        messages: OwnedHandle,
        backlog: B,
    ) -> io::Result<(Self, Sender<B>)> {
        let port = Arc::new(Port::new()?);
        let commands = PortReader::new(port.associate(commands, COMMANDS_KEY)?);
        let sender = Sender::on_port(messages, backlog, Arc::clone(&port), MESSAGES_KEY)?;
        Ok((
            Self {
                port,
                commands,
                completions: Completions::with_capacity(8),
            },
            sender,
        ))
    }

    /// Serves the port on the calling thread, the child's I/O thread: hands
    /// `command` the parts of each command frame, in order, as it is read,
    /// and finishes `sender`'s writes that pend. Returns once `command`
    /// says to stop, when nothing after that command is handed out, or
    /// once verbatim.exe closes the command pipe.
    ///
    /// # Errors
    ///
    /// Returns the error of a wait on the port, of a command pipe that
    /// failed or ended inside a frame, or of `command`.
    pub fn serve<B: Backlog>(
        &mut self,
        sender: &Sender<B>,
        mut command: impl FnMut(Parts<'_>) -> io::Result<Serving>,
    ) -> io::Result<Served> {
        let Self {
            port,
            commands,
            completions,
        } = self;
        // The first read starts as a wake-up with the reader's key.
        port.post(COMMANDS_KEY)?;
        let mut stopped = false;
        loop {
            port.wait(completions, None)?;
            for completion in completions.iter() {
                if completion.key == sender.key() {
                    sender.complete(&completion);
                    continue;
                }
                if completion.key != COMMANDS_KEY || stopped {
                    continue;
                }
                let reading = commands.take(&completion, port, |body| {
                    if !stopped && command(body)? == Serving::Stop {
                        stopped = true;
                    }
                    Ok(())
                });
                if let Reading::Ended(ended) = reading
                    && !stopped
                {
                    ended?;
                    return Ok(Served::CommandsEnded);
                }
            }
            if stopped {
                return Ok(Served::Stopped);
            }
        }
    }

    /// Waits, serving the port, until `sender`, which the caller has
    /// closed, has written everything and closed its pipe: a child never
    /// exits with a write in flight, since its exit would cancel the write
    /// and cut the frame (`docs/design/communication.md`, section 4.6).
    /// Commands read meanwhile are not handed out. The wait is for the
    /// completions alone; verbatim.exe's time limit for the shutdown bounds
    /// it.
    pub fn finish<B: Backlog>(&mut self, sender: &Sender<B>) {
        while !sender.is_finished() {
            if let Err(error) = self.port.wait(&mut self.completions, None) {
                tracing::warn!(%error, "the I/O thread's port failed with writes in flight");
                return;
            }
            for completion in self.completions.iter() {
                if completion.key == sender.key() {
                    sender.complete(&completion);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;
    use std::io::{BufReader, Write};
    use std::time::Duration;

    use super::*;
    use crate::overlapped::{End, Flow, pipe};

    fn framed(body: &[u8]) -> Vec<u8> {
        let mut buffer = Vec::new();
        put_frame(&mut buffer, |buffer| {
            buffer.extend_from_slice(body);
            Ok(())
        })
        .expect("framed");
        buffer
    }

    fn framed_with_raw(body: &[u8], raw: &[u8]) -> Vec<u8> {
        let mut buffer = Vec::new();
        put_frame_with_raw(
            &mut buffer,
            |buffer| {
                buffer.extend_from_slice(body);
                Ok(())
            },
            |buffer| {
                buffer.extend_from_slice(raw);
                Ok(())
            },
        )
        .expect("framed");
        buffer
    }

    /// The body of the next frame `reader` holds, which has no raw section.
    fn read<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
        Ok(read_frame(reader)?.map(|frame| {
            frame
                .parts()
                .without_raw()
                .expect("no raw section")
                .to_vec()
        }))
    }

    #[test]
    fn a_frame_is_a_content_length_header_and_its_body() {
        assert_eq!(
            framed(b"{\"a\":1}"),
            b"Content-Length: 0000000007\r\n\r\n{\"a\":1}".to_vec()
        );
    }

    #[test]
    fn a_frame_with_a_raw_section_names_its_length_on_a_second_line() {
        assert_eq!(
            framed_with_raw(b"{\"a\":1}", b"xyz"),
            b"Content-Length: 0000000007\r\nRaw-Length: 0000000003\r\n\r\n{\"a\":1}xyz".to_vec()
        );
    }

    #[test]
    fn frames_with_and_without_raw_sections_read_back_as_their_parts() {
        let mut bytes = framed_with_raw(b"first", b"raw");
        bytes.extend(framed(b"second"));
        bytes.extend(framed_with_raw(b"third", b""));
        bytes.extend(b"Content-Length: 1\r\nRaw-Length: 2\r\n\r\n4ab");
        let mut reader = BufReader::with_capacity(5, bytes.as_slice());
        let mut next = || read_frame(&mut reader).expect("read").expect("a frame");
        assert_eq!(
            next().parts(),
            Parts {
                body: b"first",
                raw: Some(b"raw")
            }
        );
        assert_eq!(next().parts(), Parts::body(b"second"));
        assert_eq!(
            next().parts(),
            Parts {
                body: b"third",
                raw: Some(b"")
            }
        );
        assert_eq!(
            next().parts(),
            Parts {
                body: b"4",
                raw: Some(b"ab")
            }
        );
        assert_eq!(read_frame(&mut reader).expect("read"), None);
    }

    #[test]
    fn a_raw_section_is_refused_where_its_message_takes_none() {
        let frame = framed_with_raw(b"{}", b"x");
        let frame = read_frame(&mut frame.as_slice())
            .expect("read")
            .expect("a frame");
        assert_eq!(
            frame.parts().without_raw().expect_err("refused").kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_body_and_raw_section_over_the_limit_together_are_refused() {
        let half = MAX_FRAME_BYTES / 2 + 1;
        let header = format!("Content-Length: {half}\r\nRaw-Length: {half}\r\n\r\n");
        let mut reader = BufReader::new(header.as_bytes());
        assert_eq!(
            read_frame(&mut reader).expect_err("too large").kind(),
            io::ErrorKind::InvalidData
        );
        let mut buffer = b"kept".to_vec();
        let error = put_frame_with_raw(
            &mut buffer,
            |body| {
                body.resize(body.len() + half, b'a');
                Ok(())
            },
            |raw| {
                raw.resize(raw.len() + half, 0);
                Ok(())
            },
        )
        .expect_err("too large");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(buffer, b"kept");
    }

    #[test]
    fn frames_are_read_whole_one_after_another_and_the_end_ends_the_stream() {
        let mut bytes = framed(b"first");
        bytes.extend(framed(b""));
        bytes.extend(b"Content-Length: 6\r\n\r\nsecond");
        // A buffer smaller than a header, so each header and body arrives
        // in pieces.
        let mut reader = BufReader::with_capacity(5, bytes.as_slice());
        assert_eq!(read(&mut reader).expect("read"), Some(b"first".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), Some(Vec::new()));
        assert_eq!(read(&mut reader).expect("read"), Some(b"second".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), None);
    }

    #[test]
    fn the_end_inside_a_frame_is_an_error_not_a_message() {
        let whole = framed(b"cut short");
        for cut in [5, whole.len() - 3] {
            let mut reader = BufReader::new(&whole[..cut]);
            let error = read(&mut reader).expect_err("cut");
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[test]
    fn a_length_over_the_limit_is_refused_before_any_body_is_read() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
        let mut reader = BufReader::new(header.as_bytes());
        let error = read(&mut reader).expect_err("too large");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_header_that_is_not_one_is_refused() {
        for header in [
            &b"Content-Length: 1x\r\n\r\n"[..],
            b"Length: 1\r\n\r\nx",
            b"Content-Length: 1\r\nRaw: 1\r\n\r\nxy",
            b"Content-Length: 1\r\nRaw-Length: \r\n\r\nx",
            b"Content-Length: 1\r\nRaw-Length: 1\r\nRaw-Length: 1\r\n\r\nxy",
            b"Raw-Length: 1\r\nContent-Length: 1\r\n\r\nxy",
            &[b'9'; MAX_HEADER_BYTES + 1],
        ] {
            let mut reader = BufReader::new(header);
            let error = read(&mut reader).expect_err("refused");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn a_blocking_reader_reads_frames_from_an_overlapped_end_until_the_writer_closes() {
        let (ours, theirs) = pipe(Flow::Inbound, 0, End::Synchronous).expect("a pipe");
        let writer = thread::spawn(move || {
            let mut theirs = File::from(theirs);
            theirs.write_all(&framed(b"one")).expect("written");
            theirs.write_all(&framed(&[7; 10_000])).expect("written");
        });
        let mut reader = BufReader::new(BlockingReader::new(ours).expect("a reader"));
        assert_eq!(read(&mut reader).expect("read"), Some(b"one".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), Some(vec![7; 10_000]));
        writer.join().expect("the writer");
        assert_eq!(read(&mut reader).expect("read"), None);
    }

    /// A frame's body as a reader handed it out, and whether it gave its
    /// buffer up for it.
    type Body = (Vec<u8>, bool);

    /// A frame's body and raw section as a reader handed them out, and
    /// whether it gave its buffer up for them.
    type OwnedParts = (Vec<u8>, Option<Vec<u8>>, bool);

    /// A port reader on a pipe whose other end the test writes itself.
    struct Fixture {
        port: Port,
        reader: PortReader,
        writer: Option<File>,
        completions: Completions,
    }

    impl Fixture {
        /// A reader on a new pipe with a buffer of `buffer` bytes, its
        /// first read started.
        fn new(buffer: u32) -> Self {
            let port = Port::new().expect("a port");
            let (ours, theirs) = pipe(Flow::Inbound, buffer, End::Synchronous).expect("a pipe");
            let reader = PortReader::new(port.associate(ours, 3).expect("associated"));
            port.post(3).expect("posted");
            let mut reading = Self {
                port,
                reader,
                writer: Some(File::from(theirs)),
                completions: Completions::with_capacity(4),
            };
            assert_eq!(reading.turn().0, Vec::<Vec<u8>>::new());
            reading
        }

        /// One completion's turn: the frames handed out, and whether the
        /// reading ended, and how.
        fn turn(&mut self) -> (Vec<Vec<u8>>, Option<io::Result<()>>) {
            assert_eq!(
                self.port
                    .wait(&mut self.completions, Some(Duration::from_secs(10)))
                    .expect("waited"),
                1,
                "one completion within the bound"
            );
            let completion = self.completions.iter().next().expect("one");
            let mut frames = Vec::new();
            let reading = self.reader.take(&completion, &self.port, |body| {
                frames.push(body.without_raw().expect("no raw section").to_vec());
                Ok(())
            });
            match reading {
                Reading::Ended(result) => (frames, Some(result)),
                Reading::Waiting | Reading::Yielded => (frames, None),
            }
        }

        /// One completion's turn, as [`turn`](Self::turn), with each
        /// frame's body and whether the reader gave its buffer up for it.
        fn turn_bodies(&mut self) -> (Vec<Body>, Option<io::Result<()>>) {
            let (frames, ended) = self.turn_parts();
            let bodies = frames
                .into_iter()
                .map(|(body, raw, owned)| {
                    assert_eq!(raw, None, "no raw section");
                    (body, owned)
                })
                .collect();
            (bodies, ended)
        }

        /// One completion's turn, as [`turn_bodies`](Self::turn_bodies),
        /// with each frame's raw section too.
        fn turn_parts(&mut self) -> (Vec<OwnedParts>, Option<io::Result<()>>) {
            assert_eq!(
                self.port
                    .wait(&mut self.completions, Some(Duration::from_secs(10)))
                    .expect("waited"),
                1,
                "one completion within the bound"
            );
            let completion = self.completions.iter().next().expect("one");
            let mut frames = Vec::new();
            let reading = self.reader.take_bodies(&completion, &self.port, |body| {
                let owned = body.is_owned();
                let frame = body.into_owned();
                let Parts { body, raw } = frame.parts();
                frames.push((body.to_vec(), raw.map(<[u8]>::to_vec), owned));
                Ok(())
            });
            match reading {
                Reading::Ended(result) => (frames, Some(result)),
                Reading::Waiting | Reading::Yielded => (frames, None),
            }
        }

        /// Writes `bytes`, then takes the turn the read waiting for them
        /// completes.
        fn write(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
            self.writer
                .as_mut()
                .expect("open")
                .write_all(bytes)
                .expect("written");
            let (frames, ended) = self.turn();
            assert!(ended.is_none(), "still reading: {ended:?}");
            frames
        }

        /// Closes the writing end, and says how the reading ended.
        fn close(&mut self) -> io::Result<()> {
            self.writer = None;
            let (frames, ended) = self.turn();
            assert_eq!(frames, Vec::<Vec<u8>>::new());
            ended.expect("the reading ended")
        }
    }

    #[test]
    fn a_port_reader_hands_out_frames_whole_however_reads_split_them() {
        let mut stream = framed(b"first");
        stream.extend(framed(b""));
        stream.extend(framed(b"second"));
        stream.extend(framed(b"third"));
        let mut reading = Fixture::new(4096);
        // Inside the first header; then on into the second frame's body;
        // then the rest, the end of one frame and another whole.
        assert_eq!(reading.write(&stream[..5]), Vec::<Vec<u8>>::new());
        assert_eq!(
            reading.write(&stream[5..70]),
            [b"first".to_vec(), Vec::new()]
        );
        assert_eq!(
            reading.write(&stream[70..]),
            [b"second".to_vec(), b"third".to_vec()]
        );
        assert!(reading.close().is_ok(), "a clean end between frames");
    }

    #[test]
    fn a_port_reader_reads_a_frame_larger_than_the_pipe_whole() {
        let large = vec![9u8; 300_000];
        let mut stream = framed(&large);
        stream.extend(framed(b"after"));
        let mut reading = Fixture::new(4096);
        let mut writer = reading.writer.take().expect("open");
        let writing = thread::spawn(move || writer.write_all(&stream).expect("written"));
        // The writer closes its end once it has written, so the reading
        // ends after the last frame.
        let mut frames = Vec::new();
        let ended = loop {
            let (more, ended) = reading.turn();
            frames.extend(more);
            if let Some(ended) = ended {
                break ended;
            }
        };
        writing.join().expect("the writer");
        assert_eq!(frames, [large, b"after".to_vec()]);
        assert!(ended.is_ok(), "{ended:?}");
    }

    /// A frame larger than one read, gathered in the reader's buffer with
    /// nothing after it, is handed out as that buffer, given up, whole and
    /// as written.
    #[test]
    fn a_port_reader_gives_up_its_buffer_for_a_large_frame_read_whole() {
        let large: Vec<u8> = (0..300_000u32).map(|at| (at % 251) as u8).collect();
        let stream = framed(&large);
        let mut reading = Fixture::new(4096);
        let mut writer = reading.writer.take().expect("open");
        let writing = thread::spawn(move || writer.write_all(&stream).expect("written"));
        let mut frames = Vec::new();
        let ended = loop {
            let (more, ended) = reading.turn_bodies();
            frames.extend(more);
            if let Some(ended) = ended {
                break ended;
            }
        };
        writing.join().expect("the writer");
        assert_eq!(frames.len(), 1);
        assert!(frames[0].1, "the reader's buffer, given up");
        assert!(frames[0].0 == large, "the frame whole and as written");
        assert!(ended.is_ok(), "{ended:?}");
    }

    /// A frame gathered in the reader's buffer with the start of the next
    /// after it is lent, the buffer kept for the next; that next frame,
    /// once it fills the buffer alone, is handed out as the buffer.
    #[test]
    fn a_port_reader_lends_a_frame_followed_by_the_start_of_another() {
        let mut stream = framed(b"first");
        stream.extend(framed(b"second"));
        let first = framed(b"first").len();
        let mut reading = Fixture::new(4096);
        let mut write = |bytes: &[u8]| {
            reading
                .writer
                .as_mut()
                .expect("open")
                .write_all(bytes)
                .expect("written");
            let (frames, ended) = reading.turn_bodies();
            assert!(ended.is_none(), "still reading: {ended:?}");
            frames
        };
        assert_eq!(write(&stream[..5]), []);
        assert_eq!(write(&stream[5..first + 3]), [(b"first".to_vec(), false)]);
        assert_eq!(write(&stream[first + 3..]), [(b"second".to_vec(), true)]);
    }

    /// A frame with a raw section is handed out with its body and raw
    /// section parted, lent or given up: a small one lent, then a large
    /// one gathered over several reads as the reader's buffer.
    #[test]
    fn a_port_reader_hands_out_a_frames_raw_section_apart_from_its_body() {
        let large: Vec<u8> = (0..300_000u32).map(|at| (at % 251) as u8).collect();
        let mut stream = framed_with_raw(b"small", b"raw");
        let small = stream.len();
        stream.extend(framed_with_raw(b"large", &large));
        let mut reading = Fixture::new(4096);
        let mut writer = reading.writer.take().expect("open");
        writer.write_all(&stream[..small]).expect("written");
        let (frames, ended) = reading.turn_parts();
        assert!(ended.is_none(), "still reading: {ended:?}");
        assert_eq!(frames, [(b"small".to_vec(), Some(b"raw".to_vec()), false)]);
        let rest = stream[small..].to_vec();
        let writing = thread::spawn(move || writer.write_all(&rest).expect("written"));
        let mut frames = Vec::new();
        let ended = loop {
            let (more, ended) = reading.turn_parts();
            frames.extend(more);
            if let Some(ended) = ended {
                break ended;
            }
        };
        writing.join().expect("the writer");
        assert_eq!(frames.len(), 1);
        let (body, raw, owned) = &frames[0];
        assert_eq!(body, b"large");
        assert!(
            raw.as_deref() == Some(large.as_slice()),
            "the raw section whole"
        );
        assert!(*owned, "the reader's buffer, given up");
        assert!(ended.is_ok(), "{ended:?}");
    }

    #[test]
    fn a_port_reader_reports_the_end_inside_a_frame_as_an_error() {
        let whole = framed(b"cut short");
        let mut reading = Fixture::new(4096);
        assert_eq!(
            reading.write(&whole[..whole.len() - 3]),
            Vec::<Vec<u8>>::new()
        );
        let error = reading.close().expect_err("cut");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    /// A backlog of byte strings, at most `capacity` of them.
    struct Strings {
        items: VecDeque<Vec<u8>>,
        capacity: usize,
    }

    impl Backlog for Strings {
        type Item = Vec<u8>;
        type Refused = ();

        fn push(&mut self, item: Vec<u8>) -> Result<(), ()> {
            if self.items.len() >= self.capacity {
                return Err(());
            }
            self.items.push_back(item);
            Ok(())
        }

        fn pop(&mut self) -> Option<Vec<u8>> {
            self.items.pop_front()
        }

        fn is_empty(&self) -> bool {
            self.items.is_empty()
        }

        fn encode(&mut self, item: Vec<u8>, buffer: &mut Vec<u8>) -> io::Result<()> {
            put_frame(buffer, |buffer| {
                buffer.extend_from_slice(&item);
                Ok(())
            })
        }
    }

    fn sender(pipe: OwnedHandle, capacity: usize) -> Sender<Strings> {
        Sender::start(
            pipe,
            Strings {
                items: VecDeque::new(),
                capacity,
            },
            "transport-test-finisher",
            || {},
        )
        .expect("a sender")
        .0
    }

    #[test]
    fn frames_sent_while_a_write_pends_are_written_after_it_in_order_and_close_ends_the_stream() {
        // A small buffer and a frame larger than it, so the first write
        // pends until the reader takes it, and later sends wait behind it.
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let sender = sender(ours, 8);
        let large = vec![1u8; 100_000];
        sender.send(large.clone()).expect("sent");
        sender.send(b"second".to_vec()).expect("queued");
        sender.send(b"third".to_vec()).expect("queued");
        sender.close();
        assert_eq!(sender.send(b"late".to_vec()), Err(SendError::Closed));
        let mut reader = BufReader::new(File::from(theirs));
        assert_eq!(read(&mut reader).expect("read"), Some(large));
        assert_eq!(read(&mut reader).expect("read"), Some(b"second".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), Some(b"third".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), None);
    }

    #[test]
    fn the_backlog_policy_refuses_while_a_write_pends() {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let sender = sender(ours, 1);
        sender.send(vec![1u8; 100_000]).expect("sent, and pending");
        sender.send(b"waits".to_vec()).expect("queued");
        assert_eq!(
            sender.send(b"refused".to_vec()),
            Err(SendError::Refused(()))
        );
        let mut reader = BufReader::new(File::from(theirs));
        assert_eq!(
            read(&mut reader).expect("read").map(|body| body.len()),
            Some(100_000)
        );
        assert_eq!(read(&mut reader).expect("read"), Some(b"waits".to_vec()));
    }

    #[test]
    fn a_sender_whose_reader_stops_delays_no_other_pipe() {
        // Two senders sharing a port nobody serves: the first's reader
        // never reads, so a frame larger than its buffer pends and the next
        // waits behind it; the second's frames are still written at once.
        let port = Arc::new(Port::new().expect("a port"));
        let strings = || Strings {
            items: VecDeque::new(),
            capacity: 8,
        };
        let (stuck, _never_read) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let (other, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let stuck = Sender::on_port(stuck, strings(), Arc::clone(&port), 1).expect("a sender");
        let other = Sender::on_port(other, strings(), port, 2).expect("a sender");
        stuck.send(vec![1u8; 100_000]).expect("sent, and pending");
        stuck.send(b"behind".to_vec()).expect("queued");
        other.send(b"first".to_vec()).expect("written");
        other.send(b"second".to_vec()).expect("written");
        let mut reader = BufReader::new(File::from(theirs));
        assert_eq!(read(&mut reader).expect("read"), Some(b"first".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), Some(b"second".to_vec()));
    }

    #[test]
    fn a_send_after_the_reader_has_gone_closes_the_sender() {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let sender = sender(ours, 8);
        drop(theirs);
        assert_eq!(sender.send(b"nobody".to_vec()), Err(SendError::Closed));
        assert_eq!(sender.send(b"again".to_vec()), Err(SendError::Closed));
    }

    #[test]
    fn frames_sent_together_are_written_with_one_write_and_taken_by_one_read() {
        let port = Port::new().expect("a port");
        let (ours, theirs) = pipe(Flow::Inbound, 4096, End::Overlapped).expect("a pipe");
        let ours = port.associate(ours, 5).expect("associated");
        // The read waits before anything is written, so it completes with
        // what the first write brings, and only that.
        let mut reading = PipeIo::new(4096);
        assert!(matches!(reading.read(&ours), Started::Pending));
        let sender = sender(theirs, 8);
        sender
            .send_all([b"one".to_vec(), b"two".to_vec()])
            .expect("sent");
        let mut completions = Completions::with_capacity(4);
        assert_eq!(port.wait(&mut completions, None).expect("waited"), 1);
        let completion = completions.iter().next().expect("one completion");
        reading.complete(&ours, &completion).expect("read");
        let mut both = framed(b"one");
        both.extend(framed(b"two"));
        assert_eq!(reading.data(), both.as_slice(), "two frames, from one read");
    }

    #[test]
    fn a_batch_larger_than_the_pipe_pends_and_is_finished_whole_before_what_follows() {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let sender = sender(ours, 8);
        let large = vec![3u8; 100_000];
        sender
            .send_all([large.clone(), b"second".to_vec()])
            .expect("sent, and pending");
        sender.send(b"after".to_vec()).expect("queued");
        sender.close();
        let mut reader = BufReader::new(File::from(theirs));
        assert_eq!(read(&mut reader).expect("read"), Some(large));
        assert_eq!(read(&mut reader).expect("read"), Some(b"second".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), Some(b"after".to_vec()));
        assert_eq!(read(&mut reader).expect("read"), None);
    }

    #[test]
    fn a_childs_io_thread_finishes_the_write_in_flight_before_its_shutdown_returns() {
        // The child's last frame is larger than the pipe's buffer and
        // nobody reads it yet, so its write pends; the I/O thread closes
        // the sender and finishes. Dropping everything after `finish` stands
        // in for the child's exit, which would cancel a write still in
        // flight and cut the frame.
        let (commands, _core_commands) =
            pipe(Flow::Inbound, 4096, End::Overlapped).expect("a pipe");
        let (core, messages) = pipe(Flow::Inbound, 4096, End::Overlapped).expect("a pipe");
        let last = vec![5u8; 200_000];
        let frame = last.clone();
        let io_thread = thread::spawn(move || {
            let (mut io, sender) = ChildIo::new(
                commands,
                messages,
                Strings {
                    items: VecDeque::new(),
                    capacity: 8,
                },
            )
            .expect("the child's side");
            sender.send(frame).expect("sent, and pending");
            sender.close();
            io.finish(&sender);
            sender.is_finished()
        });
        let mut reader = BufReader::new(BlockingReader::new(core).expect("a reader"));
        assert_eq!(read(&mut reader).expect("read whole"), Some(last));
        assert_eq!(read(&mut reader).expect("read"), None);
        assert!(
            io_thread.join().expect("the I/O thread"),
            "the shutdown returned with everything written"
        );
    }

    /// Takes from `reader` until it hands out frames or ends, waiting on
    /// its event, at most ten seconds, while a read is in flight.
    fn take_frames(reader: &mut EventReader) -> (Vec<Vec<u8>>, Option<io::Result<()>>) {
        let mut frames = Vec::new();
        loop {
            match reader.take(|body| {
                frames.push(body.without_raw().expect("no raw section").to_vec());
                Ok(())
            }) {
                Taken::Frames => return (frames, None),
                Taken::Ended(ended) => return (frames, Some(ended)),
                Taken::Waiting => assert_eq!(
                    crate::event::wait_any(&[reader.event()], Some(Duration::from_secs(10)))
                        .expect("waited"),
                    Some(0),
                    "a read finishes"
                ),
            }
        }
    }

    #[test]
    fn an_event_reader_hands_out_frames_whole_and_reports_the_end_inside_one() {
        let (ours, theirs) = pipe(Flow::Inbound, 4096, End::Synchronous).expect("a pipe");
        let mut reader = EventReader::new(ours, 4096).expect("a reader");
        let mut writer = File::from(theirs);
        let mut stream = framed(b"first");
        stream.extend(framed(b"second"));
        let third = framed(b"third");
        // The first frame and part of the second; then the rest of the
        // second and part of the third, which the end cuts short.
        writer.write_all(&stream[..40]).expect("written");
        assert_eq!(take_frames(&mut reader).0, [b"first".to_vec()]);
        writer.write_all(&stream[40..]).expect("written");
        writer.write_all(&third[..10]).expect("written");
        assert_eq!(take_frames(&mut reader).0, [b"second".to_vec()]);
        drop(writer);
        let (frames, ended) = take_frames(&mut reader);
        assert_eq!(frames, Vec::<Vec<u8>>::new());
        let error = ended.expect("ended").expect_err("cut short");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn an_event_writer_finishes_a_write_larger_than_the_pipe_whole_while_another_event_is_set() {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Overlapped).expect("a pipe");
        let mut writer = EventWriter::new(ours).expect("a writer");
        let mut reader = EventReader::new(theirs, 4096).expect("a reader");
        let large = vec![7u8; 300_000];
        let written = writer
            .start(|buffer| {
                put_frame(buffer, |body| {
                    body.extend_from_slice(&large);
                    Ok(())
                })?;
                put_frame(buffer, |body| {
                    body.extend_from_slice(b"after");
                    Ok(())
                })
            })
            .expect("started");
        assert!(!written, "a write larger than the pipe pends");
        // Another event, as a host's command event, set while the write
        // pends: the wait sees it, and the write goes on.
        let command = Event::new().expect("an event");
        command.set();
        assert_eq!(
            crate::event::wait_any(&[writer.event(), &command], None).expect("waited"),
            Some(1)
        );
        assert!(writer.poll().is_none(), "the write is still in flight");
        assert!(
            take_frames(&mut reader).0 == [large],
            "the large frame is read whole"
        );
        assert_eq!(take_frames(&mut reader).0, [b"after".to_vec()]);
        writer.finish().expect("the write finished whole");
        drop(writer);
        let (frames, ended) = take_frames(&mut reader);
        assert_eq!(frames, Vec::<Vec<u8>>::new());
        assert!(ended.expect("ended").is_ok(), "a clean end between frames");
    }
}
