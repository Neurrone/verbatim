//! What crosses the benchmark's pipes, and the code on either side of
//! them: the clock, the gaps between messages, frames, and the overlapped
//! reader and sender, written as `docs/design/communication.md` sections
//! 4.1 to 4.6 describe the transport.

use std::collections::VecDeque;
use std::io::{self, BufRead};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use verbatim_process::overlapped::{Completion, Completions, PipeIo, Port, PortFile, Started};
use verbatim_process::scheduling::ThreadPriority;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// The sequence number of the message that ends a run.
pub const END: u64 = u64::MAX;

/// How many bytes of padding a message carries, so it is the size of a
/// UIA focus fact with a name and a value.
pub const PADDING: usize = 300;

/// The largest frame a reader accepts, as Verbatim's protocol bounds a
/// message (32 MB), with room for the header.
const MAX_FRAME: usize = 32 * 1024 * 1024 + 1024;

/// How many bytes a reader asks for when it does not yet know a frame's
/// length: the event pipes' buffer.
pub const READ_SIZE: usize = 64 * 1024;

/// Now, in units of 100 ns, on the clock every process shares
/// (`GetSystemTimePreciseAsFileTime`, derived from the performance
/// counter).
pub fn now() -> u64 {
    // SAFETY: a plain read of the system time.
    let time = unsafe { GetSystemTimePreciseAsFileTime() };
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Runs the calling thread at `priority`, out of power throttling, or says
/// why it could not.
pub fn prioritize(priority: ThreadPriority) {
    if let Err(error) = verbatim_process::scheduling::prioritize_current_thread(priority) {
        eprintln!("relay-floor: a thread keeps its priority: {error}");
    }
}

/// One message, as a sender writes it.
#[derive(Serialize)]
pub struct Outgoing<'a> {
    pub seq: u64,
    /// When the sender began to send it.
    pub sent: u64,
    /// How long the same producer's previous send call took, in units of
    /// 100 ns; `None` for its first.
    pub send: Option<u64>,
    pub pad: &'a str,
    /// On the end message only: the sender's allocations, frames, writes
    /// and writes that pended.
    pub totals: Option<Totals>,
}

/// One message, as a relay or a receiver reads it, allocating its strings
/// as verbatim.exe's parse of a fact does.
#[derive(Deserialize)]
pub struct Incoming {
    pub seq: u64,
    pub sent: u64,
    pub send: Option<u64>,
    #[serde(rename = "pad")]
    pub _pad: String,
    pub totals: Option<Totals>,
}

/// Counts a process reports about its side of a run.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Totals {
    /// Heap allocations made while the frames were handled.
    pub allocations: u64,
    /// Frames handled.
    pub frames: u64,
    /// Overlapped writes issued; 0 for a synchronous side.
    pub writes: u64,
    /// How many of those pended rather than completing at once.
    pub pended: u64,
}

/// A small generator of the gaps between messages: exponential, with the
/// given mean, clipped to 1 to 100 ms, from a xorshift sequence.
pub struct Gaps {
    state: u64,
    mean_us: f64,
}

impl Gaps {
    pub fn new(seed: u64, mean_us: u64) -> Self {
        #[expect(clippy::cast_precision_loss, reason = "a gap of microseconds")]
        let mean_us = mean_us as f64;
        Self {
            state: seed | 1,
            mean_us,
        }
    }

    pub fn next(&mut self) -> Duration {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        #[expect(clippy::cast_precision_loss, reason = "53 bits of a uniform draw")]
        let uniform = ((self.state >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
        let gap_us = (-uniform.ln() * self.mean_us).clamp(1_000.0, 100_000.0);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to 1 to 100 ms above"
        )]
        let gap_us = gap_us as u64;
        Duration::from_micros(gap_us)
    }
}

/// A high-resolution waitable timer, which waits out a gap without raising
/// the system's timer resolution. The gaps are the workload: every thread
/// is idle when a message arrives, as with events.
pub struct Timer(HANDLE);

impl Timer {
    pub fn new() -> io::Result<Self> {
        // SAFETY: an unnamed timer with no security attributes; the handle
        // is closed on drop.
        let handle = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        .map_err(io::Error::other)?;
        Ok(Self(handle))
    }

    pub fn wait(&self, gap: Duration) -> io::Result<()> {
        // Relative due times are negative, in units of 100 ns.
        let due = -i64::try_from(gap.as_nanos() / 100).unwrap_or(i64::MAX);
        // SAFETY: the timer is open; `due` is alive across the call; no
        // completion routine.
        unsafe { SetWaitableTimer(self.0, &raw const due, 0, None, None, false) }
            .map_err(io::Error::other)?;
        // SAFETY: an open timer handle.
        if unsafe { WaitForSingleObject(self.0, INFINITE) } == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: the handle this value owns, closed once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// Encodes `body` as one frame into `frame`: a readable length header, as
/// the Language Server Protocol's (`Content-Length: N`, a blank line), then
/// the body (`docs/design/communication.md`, section 4.2).
pub fn encode(body: &[u8], frame: &mut Vec<u8>) {
    use std::io::Write as _;
    frame.clear();
    // Writing to a Vec cannot fail.
    let _ = write!(frame, "Content-Length: {}\r\n\r\n", body.len());
    frame.extend_from_slice(body);
}

/// Serializes `message` as JSON into `body` and frames it into `frame`;
/// both buffers are reused, so a steady sender allocates nothing here.
pub fn encode_json(
    message: &impl Serialize,
    body: &mut Vec<u8>,
    frame: &mut Vec<u8>,
) -> io::Result<()> {
    body.clear();
    serde_json::to_writer(&mut *body, message).map_err(io::Error::other)?;
    encode(body, frame);
    Ok(())
}

/// The header at the start of `bytes`: its length and the body's, once it
/// is all there.
fn header(bytes: &[u8]) -> io::Result<Option<(usize, usize)>> {
    const PREFIX: &[u8] = b"Content-Length: ";
    let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
        if bytes.len() > 64 {
            return Err(io::Error::other("a frame without a header"));
        }
        return Ok(None);
    };
    let line = &bytes[..end];
    let digits = line
        .strip_prefix(PREFIX)
        .ok_or_else(|| io::Error::other("a frame header without its length"))?;
    let length: usize = std::str::from_utf8(digits)
        .ok()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| io::Error::other("a frame length that is not a number"))?;
    if length > MAX_FRAME {
        return Err(io::Error::other(format!("a frame of {length} bytes")));
    }
    Ok(Some((end + 4, length)))
}

/// The body of a whole frame.
pub fn body(frame: &[u8]) -> io::Result<&[u8]> {
    let (header_length, _) = header(frame)?.ok_or_else(|| io::Error::other("a frame cut short"))?;
    Ok(&frame[header_length..])
}

/// Reads one frame from a synchronous pipe into `frame`, header and body;
/// `false` at the end of the stream before a frame begins.
pub fn read_frame(reader: &mut impl BufRead, frame: &mut Vec<u8>) -> io::Result<bool> {
    frame.clear();
    // The length line, then the blank line.
    if reader.read_until(b'\n', frame)? == 0 {
        return Ok(false);
    }
    if reader.read_until(b'\n', frame)? == 0 {
        return Err(io::Error::other("the stream ended inside a frame header"));
    }
    let (header_length, length) =
        header(frame)?.ok_or_else(|| io::Error::other("a malformed frame header"))?;
    frame.resize(header_length + length, 0);
    reader.read_exact(&mut frame[header_length..])?;
    Ok(true)
}

/// Bytes read so far, split into whole frames as they complete.
#[derive(Default)]
struct Frames {
    buffer: Vec<u8>,
}

impl Frames {
    fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole frame, header and body, taken out.
    fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some((header_length, length)) = header(&self.buffer)? else {
            return Ok(None);
        };
        let whole = header_length + length;
        if self.buffer.len() < whole {
            return Ok(None);
        }
        let rest = self.buffer.split_off(whole);
        Ok(Some(std::mem::replace(&mut self.buffer, rest)))
    }

    /// How many bytes the frame under way still needs, once its header has
    /// arrived.
    fn missing(&self) -> io::Result<Option<usize>> {
        Ok(header(&self.buffer)?.map(|(header_length, length)| {
            (header_length + length).saturating_sub(self.buffer.len())
        }))
    }

    fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// A frame read whole by a [`FrameReader`].
pub struct Arrived {
    /// Header and body.
    pub frame: Vec<u8>,
    /// When the read that completed it returned.
    pub at: u64,
    /// How many reads brought its bytes.
    pub reads: u64,
}

/// Reads frames from an overlapped pipe end on a port, one read in flight:
/// until it knows a frame's length it asks for [`READ_SIZE`] bytes; once
/// it does, it asks for the whole rest of the frame in one read into one
/// buffer of that size (section 4.1), and again for whatever that read
/// left.
pub struct FrameReader {
    file: PortFile,
    small: PipeIo,
    /// The read for the rest of a frame larger than [`READ_SIZE`].
    large: Option<PipeIo>,
    frames: Frames,
    /// Reads that brought bytes of the frame under way.
    reads: u64,
    ended: bool,
}

impl FrameReader {
    pub fn new(file: PortFile) -> Self {
        Self {
            file,
            small: PipeIo::new(READ_SIZE),
            large: None,
            frames: Frames::default(),
            reads: 0,
            ended: false,
        }
    }

    /// Whether the other end has closed the pipe.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Whether `completion` is this reader's read.
    pub fn owns(&self, completion: &Completion) -> bool {
        self.small.owns(completion) || self.large.as_ref().is_some_and(|io| io.owns(completion))
    }

    fn in_flight(&self) -> bool {
        self.small.in_flight() || self.large.as_ref().is_some_and(PipeIo::in_flight)
    }

    /// Starts reads until one waits or the pipe ends, giving `arrived`
    /// every frame they complete.
    pub fn pump(&mut self, arrived: &mut impl FnMut(Arrived) -> io::Result<()>) -> io::Result<()> {
        while !self.in_flight() && !self.ended {
            let missing = self.frames.missing()?.unwrap_or(0);
            let started = if missing > READ_SIZE {
                let large = self.large.insert(PipeIo::new(missing));
                large.read(&self.file)
            } else {
                self.large = None;
                self.small.read(&self.file)
            };
            match started {
                Started::Done(_) => self.take(now(), arrived)?,
                Started::Pending => {}
                Started::Failed(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                    self.ended = true;
                }
                Started::Failed(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// Takes this reader's `completion`, stamped `at`, then starts reads
    /// again as [`pump`](Self::pump) does.
    pub fn complete(
        &mut self,
        completion: &Completion,
        at: u64,
        arrived: &mut impl FnMut(Arrived) -> io::Result<()>,
    ) -> io::Result<()> {
        let io = match self.large.as_mut() {
            Some(large) if large.owns(completion) => large,
            _ => &mut self.small,
        };
        match io.complete(&self.file, completion) {
            Ok(_) => self.take(at, arrived)?,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => self.ended = true,
            Err(error) => return Err(error),
        }
        self.pump(arrived)
    }

    /// Takes the bytes the finished read brought.
    fn take(
        &mut self,
        at: u64,
        arrived: &mut impl FnMut(Arrived) -> io::Result<()>,
    ) -> io::Result<()> {
        let data = match self.large.as_ref() {
            Some(large) => large.data(),
            None => self.small.data(),
        };
        if data.is_empty() {
            return Ok(());
        }
        self.frames.push(data);
        self.reads += 1;
        while let Some(frame) = self.frames.next()? {
            let reads = self.reads;
            // A frame that began in this read counts it.
            self.reads = u64::from(!self.frames.is_empty());
            arrived(Arrived { frame, at, reads })?;
        }
        Ok(())
    }
}

/// Who issues a sender's writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Writes {
    /// The producing thread writes its own frame when no write is in
    /// flight, and the I/O thread only finishes writes that pended
    /// (section 4.3).
    Inline,
    /// Every frame is handed to the I/O thread, which writes it: the
    /// simpler form the design compares with.
    IoThread,
}

/// A posted packet asking the I/O thread to write the backlog.
const WAKE: usize = 1;
/// A posted packet asking the I/O thread to stop.
const STOP: usize = 2;
/// The key of the pipe's own completions.
const PIPE: usize = 3;

struct SendState {
    file: PortFile,
    slot: PipeIo,
    backlog: VecDeque<Vec<u8>>,
    writes: u64,
    pended: u64,
    failed: Option<String>,
}

impl SendState {
    fn idle(&self) -> bool {
        !self.slot.in_flight() && self.backlog.is_empty()
    }

    /// Writes from the backlog until a write pends or nothing is left.
    fn pump(&mut self) {
        while !self.slot.in_flight() && self.failed.is_none() {
            let Some(frame) = self.backlog.pop_front() else {
                return;
            };
            self.write(&frame);
        }
    }

    fn write(&mut self, frame: &[u8]) {
        self.writes += 1;
        match self.slot.write(&self.file, frame) {
            Started::Done(_) => {}
            Started::Pending => self.pended += 1,
            Started::Failed(error) => self.fail(&error),
        }
    }

    fn fail(&mut self, error: &io::Error) {
        self.failed = Some(error.to_string());
        self.backlog.clear();
    }
}

struct SendShared {
    port: Port,
    writes: Writes,
    state: Mutex<SendState>,
    /// Signalled when the sender has nothing in flight or waiting.
    drained: Condvar,
}

impl SendShared {
    fn lock(&self) -> MutexGuard<'_, SendState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One overlapped pipe end's sender, with its I/O thread: one write per
/// frame, at most one in flight, later frames waiting in order in a
/// backlog (section 4.3).
pub struct Sender {
    shared: Arc<SendShared>,
    io_thread: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Sender {
    /// A sender writing to `end`, an overlapped pipe end, whose I/O thread
    /// runs at `priority`.
    pub fn new(
        end: std::os::windows::io::OwnedHandle,
        writes: Writes,
        priority: ThreadPriority,
    ) -> io::Result<Self> {
        let port = Port::new()?;
        let file = port.associate(end, PIPE)?;
        let shared = Arc::new(SendShared {
            port,
            writes,
            state: Mutex::new(SendState {
                file,
                slot: PipeIo::new(0),
                backlog: VecDeque::new(),
                writes: 0,
                pended: 0,
                failed: None,
            }),
            drained: Condvar::new(),
        });
        let io_shared = Arc::clone(&shared);
        let io_thread = thread::Builder::new()
            .name("relay-floor-io".to_owned())
            .spawn(move || io_thread(&io_shared, priority))?;
        Ok(Self {
            shared,
            io_thread: Some(io_thread),
        })
    }

    /// Sends one frame. Inline, the calling thread writes it when nothing
    /// is in flight or waiting, and never waits for a write; otherwise it
    /// joins the backlog.
    pub fn send(&self, frame: &[u8]) -> io::Result<()> {
        let mut state = self.shared.lock();
        if let Some(error) = &state.failed {
            return Err(io::Error::other(error.clone()));
        }
        match self.shared.writes {
            Writes::Inline if state.idle() => state.write(frame),
            Writes::Inline => state.backlog.push_back(frame.to_vec()),
            Writes::IoThread => {
                state.backlog.push_back(frame.to_vec());
                drop(state);
                self.shared.port.post(WAKE)?;
            }
        }
        Ok(())
    }

    /// How many writes were issued, and how many pended.
    pub fn counts(&self) -> (u64, u64) {
        let state = self.shared.lock();
        (state.writes, state.pended)
    }

    /// Waits until every frame sent has been written whole, so the
    /// process may exit without cutting one (section 4.6), then stops the
    /// I/O thread. The wait is for the write's completion, nothing else.
    pub fn finish(mut self) -> io::Result<(u64, u64)> {
        {
            let mut state = self.shared.lock();
            while !state.idle() && state.failed.is_none() {
                state = self
                    .shared
                    .drained
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if let Some(error) = &state.failed {
                return Err(io::Error::other(error.clone()));
            }
        }
        self.shared.port.post(STOP)?;
        if let Some(io_thread) = self.io_thread.take() {
            io_thread
                .join()
                .map_err(|_| io::Error::other("the I/O thread panicked"))??;
        }
        Ok(self.counts())
    }
}

fn io_thread(shared: &SendShared, priority: ThreadPriority) -> io::Result<()> {
    prioritize(priority);
    let mut completions = Completions::with_capacity(16);
    loop {
        shared.port.wait(&mut completions, None)?;
        let mut stop = false;
        let mut state = shared.lock();
        for completion in completions.iter() {
            if completion.is_posted() {
                stop |= completion.key == STOP;
            } else if state.slot.owns(&completion) {
                let SendState { file, slot, .. } = &mut *state;
                if let Err(error) = slot.complete(file, &completion) {
                    state.fail(&error);
                }
            }
        }
        state.pump();
        if state.idle() || state.failed.is_some() {
            shared.drained.notify_all();
        }
        drop(state);
        if stop {
            return Ok(());
        }
    }
}
