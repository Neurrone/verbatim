//! The transport for the pipes between verbatim.exe and its outposts and
//! listener (`docs/design/communication.md`, section 4): how a message is
//! framed, how a thread that blocks reads an overlapped end, and the
//! [`Sender`] through which verbatim.exe writes a child's commands.
//!
//! A frame is a text header giving the body's length, as the Language
//! Server Protocol's, then the body: `Content-Length: ` and the length in
//! ten decimal digits, a blank line (`\r\n\r\n`), then that many bytes of
//! JSON. The length has a fixed width so a frame can be encoded in place,
//! its body after a header whose digits are filled in once the body's
//! length is known; a reader takes any number of digits. A frame is written
//! with one write and read whole: the reader takes the header, then asks
//! for the whole body at once. The end of a pipe in the middle of a frame
//! is an error, never a message.
//!
//! A [`Sender`] writes a frame inline, on the thread that sends it, when no
//! write is in flight and nothing waits ahead of it, so in the common case
//! no other thread wakes. A write the pipe cannot take at once pends; the
//! frames sent meanwhile wait in the sender's [`Backlog`], under the
//! caller's overflow policy, and the sender's finishing thread, woken only
//! by that write's completion, writes them in order. A frame once started
//! is always finished: nothing cancels a write.

use std::io::{self, BufRead, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use windows::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_IO_PENDING, HANDLE};
use windows::Win32::Storage::FileSystem::ReadFile;
use windows::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::CreateEventW;

use crate::overlapped::{Completion, Completions, PipeIo, Port, PortFile, Started, win32_error};

/// The largest frame body: a terminal's output can be megabytes; anything
/// larger is a fault.
pub const MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// The buffer of an event pipe in the direction its data flows, so a focus
/// read with its ancestors, or a reply with a line of text, completes its
/// write at once (`docs/design/communication.md`, section 4.1).
pub const EVENT_PIPE_BUFFER: u32 = 64 * 1024;

/// What a header starts with, and what ends it.
const HEADER_NAME: &[u8] = b"Content-Length: ";
const HEADER_END: &[u8] = b"\r\n\r\n";

/// The width of the length a writer puts in the header.
const LENGTH_DIGITS: usize = 10;

/// The longest header a reader accepts.
const MAX_HEADER_BYTES: usize = 64;

/// Puts a frame in `buffer`, which is cleared first: the header, then the
/// body `put_body` appends.
///
/// # Errors
///
/// Returns `put_body`'s error, or [`io::ErrorKind::InvalidInput`] for a
/// body larger than [`MAX_FRAME_BYTES`].
pub fn put_frame(
    buffer: &mut Vec<u8>,
    put_body: impl FnOnce(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<()> {
    buffer.clear();
    buffer.extend_from_slice(HEADER_NAME);
    let digits = buffer.len();
    buffer.extend_from_slice(&[b'0'; LENGTH_DIGITS]);
    buffer.extend_from_slice(HEADER_END);
    let body = buffer.len();
    put_body(buffer)?;
    let length = buffer.len() - body;
    if length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a frame of {length} bytes is larger than the {MAX_FRAME_BYTES} a frame may take"
            ),
        ));
    }
    let length = format!("{length:0LENGTH_DIGITS$}");
    buffer[digits..digits + LENGTH_DIGITS].copy_from_slice(length.as_bytes());
    Ok(())
}

/// Reads one frame's body. Returns `Ok(None)` when the stream ends before
/// a frame begins: the writer closed its end.
///
/// # Errors
///
/// Returns an error for I/O failures; [`io::ErrorKind::UnexpectedEof`] when
/// the stream ends inside a frame; and [`io::ErrorKind::InvalidData`] for a
/// header that is not one, or a length over [`MAX_FRAME_BYTES`], read no
/// further than the header.
pub fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let Some(length) = read_header(reader)? else {
        return Ok(None);
    };
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

/// Reads a header and returns the length it gives, or `None` at the end of
/// the stream before any of it.
fn read_header<R: BufRead>(reader: &mut R) -> io::Result<Option<usize>> {
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

/// The length a complete header gives.
fn parse_header(header: &[u8]) -> io::Result<usize> {
    let digits = header
        .strip_prefix(HEADER_NAME)
        .and_then(|rest| rest.strip_suffix(HEADER_END))
        .filter(|digits| !digits.is_empty() && digits.iter().all(u8::is_ascii_digit))
        .ok_or_else(|| invalid("a frame's header is not a Content-Length header"))?;
    let length = std::str::from_utf8(digits)
        .ok()
        .and_then(|digits| digits.parse::<usize>().ok())
        .filter(|&length| length <= MAX_FRAME_BYTES)
        .ok_or_else(|| {
            invalid(&format!(
                "a frame is larger than the {MAX_FRAME_BYTES} bytes a frame may take"
            ))
        })?;
    Ok(length)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

/// An overlapped pipe end read by a thread that blocks until each read
/// completes, as a [`Read`]: what verbatim.exe's reader threads read a
/// child's messages through while they exist. The end of the pipe, its
/// writer having closed it, reads as the end of the stream.
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
        // SAFETY: a manual-reset event, unnamed, with default security; the
        // handle returned is owned below.
        let event = unsafe { CreateEventW(None, true, false, None) }.map_err(win32_error)?;
        Ok(Self {
            pipe,
            // SAFETY: a valid handle just returned, owned by nothing else.
            event: unsafe { OwnedHandle::from_raw_handle(event.0 as RawHandle) },
        })
    }
}

impl Read for BlockingReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let pipe = HANDLE(self.pipe.as_raw_handle());
        let mut overlapped = OVERLAPPED {
            hEvent: HANDLE(self.event.as_raw_handle()),
            ..OVERLAPPED::default()
        };
        // SAFETY: the buffer and the OVERLAPPED outlive the read: whether it
        // completes at once or pends, this function returns only once
        // GetOverlappedResult, told to wait, has seen it finish.
        let started = unsafe { ReadFile(pipe, Some(buffer), None, Some(&raw mut overlapped)) };
        match started {
            Ok(()) => {}
            Err(error) if error.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(error) if error.code() == ERROR_BROKEN_PIPE.to_hresult() => return Ok(0),
            Err(error) => return Err(win32_error(error)),
        }
        let mut moved = 0u32;
        // SAFETY: the read started above on this handle with this
        // OVERLAPPED; the call waits for it to finish.
        match unsafe { GetOverlappedResult(pipe, &raw const overlapped, &raw mut moved, true) } {
            Ok(()) => Ok(moved as usize),
            Err(error) if error.code() == ERROR_BROKEN_PIPE.to_hresult() => Ok(0),
            Err(error) => Err(win32_error(error)),
        }
    }
}

/// The frames a [`Sender`] holds while a write is in flight, under the
/// caller's overflow policy, and how each is encoded.
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

    /// Encodes `item` as a frame into `buffer` ([`put_frame`]), just before
    /// it is written, so a time stamped then is the write's.
    ///
    /// # Errors
    ///
    /// Returns the encoding's error; the item is then skipped.
    fn encode(item: Self::Item, buffer: &mut Vec<u8>) -> io::Result<()>;
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
    /// `started` (to set its priority) and exits once the sender has
    /// finished.
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
    ) -> io::Result<Self> {
        let port = Arc::new(Port::new()?);
        let sender = Self::make(pipe, backlog, Arc::clone(&port), OWN_KEY, true)?;
        let finishing = sender.clone();
        thread::Builder::new()
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
        Ok(sender)
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

    /// Sends `item`: writes it at once when no write is in flight and
    /// nothing waits, and otherwise adds it to the backlog. Never waits for
    /// the pipe.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Closed`] once the sender is closed, and the
    /// backlog's refusal when it refuses the item.
    pub fn send(&self, item: B::Item) -> Result<(), SendError<B::Refused>> {
        let mut state = self.lock();
        if state.closed {
            return Err(SendError::Closed);
        }
        if state.slot.in_flight() || !state.backlog.is_empty() {
            return state.backlog.push(item).map_err(SendError::Refused);
        }
        if state.write(item) {
            Ok(())
        } else {
            self.settle(&mut state);
            Err(SendError::Closed)
        }
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
        let Some(pipe) = &self.pipe else {
            return false;
        };
        let mut length = 0;
        let started = self.slot.write_with(pipe, |buffer| {
            B::encode(item, buffer)?;
            length = buffer.len();
            Ok::<(), io::Error>(())
        });
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs::File;
    use std::io::{BufReader, Write};

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

    #[test]
    fn a_frame_is_a_content_length_header_and_its_body() {
        assert_eq!(
            framed(b"{\"a\":1}"),
            b"Content-Length: 0000000007\r\n\r\n{\"a\":1}".to_vec()
        );
    }

    #[test]
    fn frames_are_read_whole_one_after_another_and_the_end_ends_the_stream() {
        let mut bytes = framed(b"first");
        bytes.extend(framed(b""));
        bytes.extend(b"Content-Length: 6\r\n\r\nsecond");
        // A buffer smaller than a header, so each header and body arrives
        // in pieces.
        let mut reader = BufReader::with_capacity(5, bytes.as_slice());
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"first".to_vec())
        );
        assert_eq!(read_frame(&mut reader).expect("read"), Some(Vec::new()));
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"second".to_vec())
        );
        assert_eq!(read_frame(&mut reader).expect("read"), None);
    }

    #[test]
    fn the_end_inside_a_frame_is_an_error_not_a_message() {
        let whole = framed(b"cut short");
        for cut in [5, whole.len() - 3] {
            let mut reader = BufReader::new(&whole[..cut]);
            let error = read_frame(&mut reader).expect_err("cut");
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
    }

    #[test]
    fn a_length_over_the_limit_is_refused_before_any_body_is_read() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_BYTES + 1);
        let mut reader = BufReader::new(header.as_bytes());
        let error = read_frame(&mut reader).expect_err("too large");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_header_that_is_not_one_is_refused() {
        for header in [
            &b"Content-Length: 1x\r\n\r\n"[..],
            b"Length: 1\r\n\r\nx",
            &[b'9'; MAX_HEADER_BYTES + 1],
        ] {
            let mut reader = BufReader::new(header);
            let error = read_frame(&mut reader).expect_err("refused");
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
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"one".to_vec())
        );
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(vec![7; 10_000])
        );
        writer.join().expect("the writer");
        assert_eq!(read_frame(&mut reader).expect("read"), None);
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

        fn encode(item: Vec<u8>, buffer: &mut Vec<u8>) -> io::Result<()> {
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
        assert_eq!(read_frame(&mut reader).expect("read"), Some(large));
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"second".to_vec())
        );
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"third".to_vec())
        );
        assert_eq!(read_frame(&mut reader).expect("read"), None);
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
            read_frame(&mut reader)
                .expect("read")
                .map(|body| body.len()),
            Some(100_000)
        );
        assert_eq!(
            read_frame(&mut reader).expect("read"),
            Some(b"waits".to_vec())
        );
    }

    #[test]
    fn a_send_after_the_reader_has_gone_closes_the_sender() {
        let (ours, theirs) = pipe(Flow::Outbound, 4096, End::Synchronous).expect("a pipe");
        let sender = sender(ours, 8);
        drop(theirs);
        assert_eq!(sender.send(b"nobody".to_vec()), Err(SendError::Closed));
        assert_eq!(sender.send(b"again".to_vec()), Err(SendError::Closed));
    }
}
