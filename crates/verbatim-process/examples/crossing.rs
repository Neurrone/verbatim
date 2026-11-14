//! The latency of one crossing between two processes over Verbatim's pipes,
//! with nothing else of Verbatim's on the way.
//!
//! The parent plays Core and launches a copy of itself as a contained child
//! that plays a reader (the listener or an outpost), over the pipes the
//! supervisor makes: unnamed, overlapped at both ends, 64 KiB each way. One
//! round is what ADR 0003 makes of an event: the child's producer thread
//! tells Core, Core asks the child at once, and the child's I/O thread
//! answers. The child's producer waits for the round to end, then sleeps a
//! jittered interval, so every round starts from idle threads.
//!
//! Shapes, chosen with `--shape`:
//!
//! - `floor`: the platform alone. Overlapped pipes and completion ports
//!   read and written with `PipeIo`, fixed-size messages, no framing, no
//!   sender and no relay.
//! - `direct`: the transport as the children use it today (`ChildIo` and a
//!   `Sender` in the child, the producer writing inline), and Core's thread
//!   reading the child's pipe itself with a `PortReader` and writing its
//!   requests inline.
//! - `relay`: today's path into Core: a relay thread at the highest
//!   priority reads the child's pipe and posts each frame to Core's
//!   `port_queue`, as verbatim.exe's relay posts outpost frames to the
//!   reducer; Core writes its requests inline on a sender whose pended
//!   writes the relay finishes.
//!
//! Thread priorities are the ones the product uses: the child's I/O and
//! producer threads and the relay at `Highest`, Core at normal unless
//! `--core-highest`.
//!
//! Every stamp is the performance counter, which is the same in every
//! process, so each leg is timed one way. A write's stamp is taken just
//! before the send; a read's when the frame is in the reading thread's
//! hands.
//!
//! Run: `cargo run -p verbatim-process --example crossing [--release] --
//! --shape relay`. Options: `--rounds N` (2000), `--warmup N` (100),
//! `--interval-us N` (5000; each gap is that plus up to half of it again),
//! `--bytes N` (512, each message's body), `--core-highest`, `--csv PATH`
//! (every round's stamps, in microseconds from the round's start),
//! `--slow-us N` (1000: every crossing slower than this is listed with its
//! start and end as UTC times of day, as `verbatim-trace-waits` takes them).
//! The threads are named for a trace: `crossing-core`, `crossing-relay`,
//! `crossing-producer` and `crossing-io`.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::io;
use std::os::windows::io::OwnedHandle;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use verbatim_process::overlapped::{
    Completion, Completions, End, Flow, PipeIo, Port, PortFile, Started, pipe,
};
use verbatim_process::port_queue::port_queue;
use verbatim_process::scheduling::{ThreadPriority, prioritize_current_thread};
use verbatim_process::transport::{
    Backlog, ChildIo, EVENT_PIPE_BUFFER, PortReader, Reading, Sender, Served, Serving, put_frame,
};
use verbatim_process::{ChildEnds, ChildSpec, launch_with_ends};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// The performance counter's ticks: the same clock in every process.
fn ticks() -> i64 {
    let mut count = 0;
    // SAFETY: a valid out-pointer; the call does not fail on Windows 10.
    unsafe {
        let _ = QueryPerformanceCounter(&raw mut count);
    }
    count
}

/// Ticks per second.
fn frequency() -> i64 {
    let mut frequency = 0;
    // SAFETY: a valid out-pointer; the call does not fail on Windows 10.
    unsafe {
        let _ = QueryPerformanceFrequency(&raw mut frequency);
    }
    frequency
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Floor,
    Direct,
    Relay,
}

impl Shape {
    fn parse(text: &str) -> Self {
        match text {
            "floor" => Self::Floor,
            "direct" => Self::Direct,
            "relay" => Self::Relay,
            other => fail(&format!("unknown shape {other}: floor, direct or relay")),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Direct => "direct",
            Self::Relay => "relay",
        }
    }
}

/// The run's settings, given to the parent and handed to the child.
#[derive(Clone, Debug)]
struct Settings {
    shape: Shape,
    /// Rounds measured, after the warm-up.
    rounds: usize,
    warmup: usize,
    interval_us: u64,
    /// Each message's body, in bytes.
    bytes: usize,
    core_highest: bool,
    csv: Option<String>,
    /// Crossings slower than this, in microseconds, are listed.
    slow_us: u64,
    /// The child's two inherited pipe handles: commands, then messages.
    child_pipes: Option<(usize, usize)>,
}

/// Room for a message's fields: its kind, its round and two stamps.
const MESSAGE_FIELDS: usize = 1 + 8 + 8 + 8;

fn fail(message: &str) -> ! {
    eprintln!("crossing: {message}");
    std::process::exit(2);
}

fn parse_settings(arguments: &[String]) -> Settings {
    let mut settings = Settings {
        shape: Shape::Relay,
        rounds: 2000,
        warmup: 100,
        interval_us: 5000,
        bytes: 512,
        core_highest: false,
        csv: None,
        slow_us: 1000,
        child_pipes: None,
    };
    let mut pipes = (None, None);
    let mut at = 0;
    let value = |at: usize| -> &str {
        arguments.get(at + 1).map_or_else(
            || fail(&format!("{} needs a value", arguments[at])),
            String::as_str,
        )
    };
    let number = |at: usize| -> u64 {
        value(at)
            .parse()
            .unwrap_or_else(|_| fail(&format!("{} needs a number", arguments[at])))
    };
    while at < arguments.len() {
        match arguments[at].as_str() {
            "--shape" => settings.shape = Shape::parse(value(at)),
            "--rounds" => settings.rounds = to_usize(number(at)),
            "--warmup" => settings.warmup = to_usize(number(at)),
            "--interval-us" => settings.interval_us = number(at),
            "--bytes" => settings.bytes = to_usize(number(at)),
            "--csv" => settings.csv = Some(value(at).to_owned()),
            "--slow-us" => settings.slow_us = number(at),
            "--pipe-in" => pipes.0 = Some(to_usize(number(at))),
            "--pipe-out" => pipes.1 = Some(to_usize(number(at))),
            "--core-highest" => {
                settings.core_highest = true;
                at += 1;
                continue;
            }
            other => fail(&format!("unknown option {other}")),
        }
        at += 2;
    }
    if settings.bytes < MESSAGE_FIELDS {
        fail(&format!("--bytes must be at least {MESSAGE_FIELDS}"));
    }
    if settings.rounds == 0 {
        fail("--rounds must be at least 1");
    }
    settings.child_pipes = match pipes {
        (Some(pipe_in), Some(pipe_out)) => Some((pipe_in, pipe_out)),
        (None, None) => None,
        _ => fail("--pipe-in and --pipe-out go together"),
    };
    settings
}

fn to_usize(value: u64) -> usize {
    usize::try_from(value).unwrap_or_else(|_| fail("a number too large"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// The child tells Core of an event: its stamp is the write's.
    Tell,
    /// Core asks the child.
    Request,
    /// The child answers: its stamps are the request's read and the
    /// answer's write.
    Answer,
}

#[derive(Clone, Copy, Debug)]
struct Message {
    kind: Kind,
    round: u64,
    stamps: [i64; 2],
}

impl Message {
    /// Appends the message's body, `bytes` long, padded with spaces.
    fn put(&self, buffer: &mut Vec<u8>, bytes: usize) {
        let start = buffer.len();
        buffer.push(match self.kind {
            Kind::Tell => 0,
            Kind::Request => 1,
            Kind::Answer => 2,
        });
        buffer.extend_from_slice(&self.round.to_le_bytes());
        buffer.extend_from_slice(&self.stamps[0].to_le_bytes());
        buffer.extend_from_slice(&self.stamps[1].to_le_bytes());
        buffer.resize(start + bytes, b' ');
    }

    fn parse(body: &[u8], bytes: usize) -> Self {
        assert_eq!(body.len(), bytes, "a message of the run's size");
        let kind = match body[0] {
            0 => Kind::Tell,
            1 => Kind::Request,
            2 => Kind::Answer,
            other => panic!("a message of unknown kind {other}"),
        };
        let field = |at: usize| -> [u8; 8] { body[at..at + 8].try_into().expect("eight bytes") };
        Self {
            kind,
            round: u64::from_le_bytes(field(1)),
            stamps: [i64::from_le_bytes(field(9)), i64::from_le_bytes(field(17))],
        }
    }
}

/// A sender's backlog: every message in order, framed as the transport
/// frames every message.
struct Queue {
    waiting: VecDeque<Message>,
    bytes: usize,
}

impl Queue {
    fn new(bytes: usize) -> Self {
        Self {
            waiting: VecDeque::new(),
            bytes,
        }
    }
}

impl Backlog for Queue {
    type Item = Message;
    type Refused = Infallible;

    fn push(&mut self, item: Message) -> Result<(), Infallible> {
        self.waiting.push_back(item);
        Ok(())
    }

    fn pop(&mut self) -> Option<Message> {
        self.waiting.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.waiting.is_empty()
    }

    fn encode(&mut self, item: Message, buffer: &mut Vec<u8>) -> io::Result<()> {
        let bytes = self.bytes;
        put_frame(buffer, |body| {
            item.put(body, bytes);
            Ok(())
        })
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let is_child = arguments.first().map(String::as_str) == Some("--child");
    let (name, run): (&str, fn(&Settings)) = if is_child {
        ("crossing-io", child)
    } else {
        ("crossing-core", parent)
    };
    let settings = parse_settings(&arguments[usize::from(is_child)..]);
    // Core's work and the child's I/O run on a named thread, so a trace
    // names them.
    let worker = thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || run(&settings))
        .unwrap_or_else(|error| fail(&format!("a thread: {error}")));
    if worker.join().is_err() {
        fail("the main work panicked");
    }
}

// The child: a reader.

fn child(settings: &Settings) {
    let Some((pipe_in, pipe_out)) = settings.child_pipes else {
        fail("a child needs its pipes");
    };
    // SAFETY: the two values are the handles the parent's launch made this
    // process inherit, and nothing else in it uses them.
    let (commands, messages) =
        unsafe { verbatim_process::inherited_pipe_handles(pipe_in, pipe_out) }
            .unwrap_or_else(|error| fail(&format!("the inherited pipes: {error}")));
    set_priority(ThreadPriority::Highest);
    let total = settings.warmup + settings.rounds;
    match settings.shape {
        Shape::Floor => floor_child(settings, commands, messages, total),
        Shape::Direct | Shape::Relay => transport_child(settings, commands, messages, total),
    }
}

/// Starts `work` on a thread named `name`, so a trace names it.
fn named(name: &str, work: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(work)
        .unwrap_or_else(|error| fail(&format!("starting {name}: {error}")))
}

fn set_priority(priority: ThreadPriority) {
    prioritize_current_thread(priority)
        .unwrap_or_else(|error| fail(&format!("setting a thread's priority: {error}")));
}

/// The gaps between rounds: `interval_us` plus up to half of it again, so
/// rounds fall at no fixed phase of the system's timers.
struct Gaps {
    state: u64,
    interval_us: u64,
}

impl Gaps {
    fn new(interval_us: u64) -> Self {
        Self {
            state: ticks().cast_unsigned() | 1,
            interval_us,
        }
    }

    fn sleep(&mut self) {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let jitter = self.state % (self.interval_us / 2).max(1);
        thread::sleep(Duration::from_micros(self.interval_us + jitter));
    }
}

/// The producer's rounds: a tell after each gap, then a wait for the
/// round's answer, so each round starts with every thread idle.
fn produce(
    interval_us: u64,
    total: usize,
    answered: &mpsc::Receiver<u64>,
    mut tell: impl FnMut(Message),
) {
    set_priority(ThreadPriority::Highest);
    let mut gaps = Gaps::new(interval_us);
    for round in 0..total as u64 {
        gaps.sleep();
        tell(Message {
            kind: Kind::Tell,
            round,
            stamps: [ticks(), 0],
        });
        let done = answered
            .recv()
            .unwrap_or_else(|_| fail("the I/O thread ended inside a round"));
        assert_eq!(done, round, "the answer of the round told");
    }
}

fn transport_child(
    settings: &Settings,
    commands: OwnedHandle,
    messages: OwnedHandle,
    total: usize,
) {
    let bytes = settings.bytes;
    let (mut io, sender) = ChildIo::new(commands, messages, Queue::new(bytes))
        .unwrap_or_else(|error| fail(&format!("the child's I/O: {error}")));
    let (answered, rounds) = mpsc::channel();
    let producer = {
        let sender = sender.clone();
        let interval_us = settings.interval_us;
        named("crossing-producer", move || {
            produce(interval_us, total, &rounds, |tell| {
                sender
                    .send(tell)
                    .unwrap_or_else(|error| fail(&format!("a tell: {error:?}")));
            });
        })
    };
    let served = io
        .serve(&sender, |parts| {
            let read = ticks();
            let request = Message::parse(parts.without_raw()?, bytes);
            assert_eq!(request.kind, Kind::Request, "Core sends only requests");
            sender
                .send(Message {
                    kind: Kind::Answer,
                    round: request.round,
                    stamps: [read, ticks()],
                })
                .map_err(|error| io::Error::other(format!("an answer: {error:?}")))?;
            answered
                .send(request.round)
                .map_err(|_| io::Error::other("the producer ended"))?;
            Ok(Serving::Continue)
        })
        .unwrap_or_else(|error| fail(&format!("serving: {error}")));
    assert_eq!(
        served,
        Served::CommandsEnded,
        "the child serves until Core ends"
    );
    producer
        .join()
        .unwrap_or_else(|_| fail("the producer panicked"));
    sender.close();
    io.finish(&sender);
}

/// One end's writes in the floor shape: a slot whose writes are expected
/// to complete at once, a round never filling the pipe; one that pends is
/// finished by the thread serving the port.
struct FloorWriter {
    file: PortFile,
    slot: Mutex<PipeIo>,
}

impl FloorWriter {
    fn write(&self, message: &Message, bytes: usize) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(!slot.in_flight(), "a floor write while one pends");
        let started = slot
            .write_with(&self.file, |buffer| {
                message.put(buffer, bytes);
                Ok::<(), Infallible>(())
            })
            .unwrap_or_else(|never| match never {});
        match started {
            Started::Done(moved) => assert_eq!(moved, bytes, "a whole message written"),
            Started::Pending => {}
            Started::Failed(error) => fail(&format!("a floor write: {error}")),
        }
    }

    fn complete(&self, completion: &Completion) {
        let mut slot = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        let moved = slot
            .complete(&self.file, completion)
            .unwrap_or_else(|error| fail(&format!("a pended floor write: {error}")));
        assert!(moved > 0, "a pended write moved its bytes");
    }
}

/// One end's reads in the floor shape, gathering fixed-size messages.
struct FloorReader {
    file: PortFile,
    slot: PipeIo,
    gathered: Vec<u8>,
}

impl FloorReader {
    fn new(file: PortFile, bytes: usize) -> Self {
        Self {
            file,
            slot: PipeIo::new(bytes),
            gathered: Vec::new(),
        }
    }

    /// Reads until a read pends, handing out each whole message as it is
    /// read; returns false when the writer has closed its end. `completion`
    /// is the read's completion, or `None` to start reading.
    fn take(
        &mut self,
        completion: Option<&Completion>,
        bytes: usize,
        mut message: impl FnMut(Message),
    ) -> bool {
        let mut done = completion.map(|completion| self.slot.complete(&self.file, completion));
        loop {
            if let Some(done) = done.take() {
                match done {
                    Ok(0) => return false,
                    Ok(_) => {
                        self.gathered.extend_from_slice(self.slot.data());
                        while self.gathered.len() >= bytes {
                            let whole: Vec<u8> = self.gathered.drain(..bytes).collect();
                            message(Message::parse(&whole, bytes));
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return false,
                    Err(error) => fail(&format!("a floor read: {error}")),
                }
            }
            match self.slot.read(&self.file) {
                Started::Pending => return true,
                Started::Done(moved) => done = Some(Ok(moved)),
                Started::Failed(error) => done = Some(Err(error)),
            }
        }
    }
}

const READ_KEY: usize = 1;
const WRITE_KEY: usize = 2;

fn floor_child(settings: &Settings, commands: OwnedHandle, messages: OwnedHandle, total: usize) {
    let bytes = settings.bytes;
    let port = Port::new().unwrap_or_else(|error| fail(&format!("a port: {error}")));
    let mut reader = FloorReader::new(
        port.associate(commands, READ_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
        bytes,
    );
    let writer = Arc::new(FloorWriter {
        file: port
            .associate(messages, WRITE_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
        slot: Mutex::new(PipeIo::new(0)),
    });
    let (answered, rounds) = mpsc::channel();
    let producer = {
        let writer = Arc::clone(&writer);
        let interval_us = settings.interval_us;
        named("crossing-producer", move || {
            produce(interval_us, total, &rounds, |tell| {
                writer.write(&tell, bytes);
            });
        })
    };
    let mut answer = |request: Message| {
        let read = ticks();
        assert_eq!(request.kind, Kind::Request, "Core sends only requests");
        writer.write(
            &Message {
                kind: Kind::Answer,
                round: request.round,
                stamps: [read, ticks()],
            },
            bytes,
        );
        answered
            .send(request.round)
            .unwrap_or_else(|_| fail("the producer ended"));
    };
    let mut completions = Completions::with_capacity(8);
    let mut open = reader.take(None, bytes, &mut answer);
    while open {
        port.wait(&mut completions, None)
            .unwrap_or_else(|error| fail(&format!("waiting: {error}")));
        for completion in completions.iter() {
            match completion.key {
                READ_KEY => open = reader.take(Some(&completion), bytes, &mut answer),
                WRITE_KEY => writer.complete(&completion),
                other => fail(&format!("a completion with key {other}")),
            }
        }
    }
    producer
        .join()
        .unwrap_or_else(|_| fail("the producer panicked"));
}

// The parent: Core.

/// One round's stamps, in ticks.
#[derive(Clone, Copy, Debug, Default)]
struct Round {
    tell_write: i64,
    /// When the relay had the tell; 0 in the shapes with no relay.
    tell_relay: i64,
    tell_take: i64,
    request_write: i64,
    request_read: i64,
    answer_write: i64,
    answer_relay: i64,
    answer_take: i64,
}

/// What Core does with each message: a tell is asked about at once, an
/// answer ends the round. Returns whether every round has ended.
struct Core {
    rounds: Vec<Round>,
    total: usize,
    answered: usize,
}

impl Core {
    fn new(total: usize) -> Self {
        Self {
            rounds: vec![Round::default(); total],
            total,
            answered: 0,
        }
    }

    /// Takes `message`, which the relay had at `relayed` (0 without one),
    /// and returns the request to send for a tell.
    fn take(&mut self, message: Message, relayed: i64) -> Option<Message> {
        let taken = ticks();
        let round = usize::try_from(message.round).expect("a round number");
        assert!(round < self.total, "a round within the run");
        let record = &mut self.rounds[round];
        match message.kind {
            Kind::Tell => {
                assert_eq!(round, self.answered, "tells come one round at a time");
                record.tell_write = message.stamps[0];
                record.tell_relay = relayed;
                record.tell_take = taken;
                record.request_write = ticks();
                Some(Message {
                    kind: Kind::Request,
                    round: message.round,
                    stamps: [0, 0],
                })
            }
            Kind::Answer => {
                assert_eq!(round, self.answered, "the answer of the round asked");
                record.request_read = message.stamps[0];
                record.answer_write = message.stamps[1];
                record.answer_relay = relayed;
                record.answer_take = taken;
                self.answered += 1;
                None
            }
            Kind::Request => panic!("a child sends no request"),
        }
    }

    fn done(&self) -> bool {
        self.answered == self.total
    }
}

fn parent(settings: &Settings) {
    let total = settings.warmup + settings.rounds;
    let (to_child, commands) = pipe(Flow::Outbound, EVENT_PIPE_BUFFER, End::Overlapped)
        .unwrap_or_else(|error| fail(&format!("the command pipe: {error}")));
    let (from_child, messages) = pipe(Flow::Inbound, EVENT_PIPE_BUFFER, End::Overlapped)
        .unwrap_or_else(|error| fail(&format!("the message pipe: {error}")));
    let exe = std::env::current_exe().unwrap_or_else(|error| fail(&format!("this exe: {error}")));
    let handed = settings.clone();
    let arguments = move |pipe_in: usize, pipe_out: usize| {
        format!(
            "--child --shape {} --rounds {} --warmup {} --interval-us {} --bytes {} --pipe-in {pipe_in} --pipe-out {pipe_out}",
            handed.shape.name(),
            handed.rounds,
            handed.warmup,
            handed.interval_us,
            handed.bytes,
        )
    };
    let child = launch_with_ends(
        &ChildSpec {
            exe: &exe,
            arguments: &arguments,
            log_stem: "crossing-child",
            memory_cap: None,
        },
        ChildEnds { commands, messages },
    )
    .unwrap_or_else(|error| fail(&format!("launching the child: {error}")));
    if settings.core_highest {
        set_priority(ThreadPriority::Highest);
    }
    let mut core = Core::new(total);
    match settings.shape {
        Shape::Floor => floor_core(settings, &mut core, to_child, from_child),
        Shape::Direct => direct_core(settings, &mut core, to_child, from_child),
        Shape::Relay => relay_core(settings, &mut core, to_child, from_child),
    }
    if !child.wait_for_exit(Duration::from_secs(10)) {
        fail("the child did not exit");
    }
    let exited = child.exit_code();
    if exited != Some(0) {
        fail(&format!(
            "the child exited with {exited:?}; its log is {}",
            verbatim_process::launch_log_dir(exe.parent().unwrap_or(Path::new(".")))
                .join("crossing-child.log")
                .display()
        ));
    }
    report(settings, &core.rounds[settings.warmup..]);
}

fn floor_core(
    settings: &Settings,
    core: &mut Core,
    to_child: OwnedHandle,
    from_child: OwnedHandle,
) {
    let bytes = settings.bytes;
    let port = Port::new().unwrap_or_else(|error| fail(&format!("a port: {error}")));
    let mut reader = FloorReader::new(
        port.associate(from_child, READ_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
        bytes,
    );
    let writer = FloorWriter {
        file: port
            .associate(to_child, WRITE_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
        slot: Mutex::new(PipeIo::new(0)),
    };
    let finished = std::cell::Cell::new(false);
    let mut handle = |message: Message| {
        if let Some(request) = core.take(message, 0) {
            writer.write(&request, bytes);
        }
        finished.set(core.done());
    };
    let mut completions = Completions::with_capacity(8);
    let mut open = reader.take(None, bytes, &mut handle);
    while open && !finished.get() {
        port.wait(&mut completions, None)
            .unwrap_or_else(|error| fail(&format!("waiting: {error}")));
        for completion in completions.iter() {
            match completion.key {
                READ_KEY => open = reader.take(Some(&completion), bytes, &mut handle),
                WRITE_KEY => writer.complete(&completion),
                other => fail(&format!("a completion with key {other}")),
            }
        }
    }
    assert!(core.done(), "every round ended before the child's pipe");
    // Dropping the command pipe's end ends the child's reading, and so the
    // child. The read in flight on the child's pipe ends as the child exits.
    let FloorWriter { file, slot } = writer;
    drop(file);
    drop(slot);
    while reader.take_end(&port, &mut completions, bytes) {}
}

impl FloorReader {
    /// Waits for the read in flight to end once the child has gone, so its
    /// memory is not freed under it.
    fn take_end(&mut self, port: &Port, completions: &mut Completions, bytes: usize) -> bool {
        port.wait(completions, None)
            .unwrap_or_else(|error| fail(&format!("waiting: {error}")));
        let mut open = true;
        for completion in completions.iter() {
            if completion.key == READ_KEY {
                open = self.take(Some(&completion), bytes, |_| {
                    panic!("a message after the last round")
                });
            }
        }
        open
    }
}

fn direct_core(
    settings: &Settings,
    core: &mut Core,
    to_child: OwnedHandle,
    from_child: OwnedHandle,
) {
    let bytes = settings.bytes;
    let port = Arc::new(Port::new().unwrap_or_else(|error| fail(&format!("a port: {error}"))));
    let mut reader = PortReader::new(
        port.associate(from_child, READ_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
    );
    let sender = Sender::on_port(to_child, Queue::new(bytes), Arc::clone(&port), WRITE_KEY)
        .unwrap_or_else(|error| fail(&format!("the sender: {error}")));
    port.post(READ_KEY)
        .unwrap_or_else(|error| fail(&format!("starting the read: {error}")));
    let mut completions = Completions::with_capacity(8);
    loop {
        port.wait(&mut completions, None)
            .unwrap_or_else(|error| fail(&format!("waiting: {error}")));
        for completion in completions.iter() {
            if completion.key == WRITE_KEY {
                sender.complete(&completion);
                continue;
            }
            let reading = reader.take(&completion, &port, |parts| {
                let message = Message::parse(parts.without_raw()?, bytes);
                if let Some(request) = core.take(message, 0) {
                    sender
                        .send(request)
                        .map_err(|error| io::Error::other(format!("a request: {error:?}")))?;
                }
                if core.done() {
                    // The child's command pipe ends once nothing is in
                    // flight, which ends the child.
                    sender.close();
                }
                Ok(())
            });
            if let Reading::Ended(ended) = reading {
                ended.unwrap_or_else(|error| fail(&format!("reading the child: {error}")));
                assert!(core.done(), "the child ended before the last round");
                return;
            }
        }
    }
}

/// What the relay posts to Core: a frame's body and when the relay had it.
struct Relayed {
    body: Vec<u8>,
    at: i64,
}

fn relay_core(
    settings: &Settings,
    core: &mut Core,
    to_child: OwnedHandle,
    from_child: OwnedHandle,
) {
    let bytes = settings.bytes;
    let port = Arc::new(Port::new().unwrap_or_else(|error| fail(&format!("a port: {error}"))));
    let mut reader = PortReader::new(
        port.associate(from_child, READ_KEY)
            .unwrap_or_else(|error| fail(&format!("associating: {error}"))),
    );
    let sender = Sender::on_port(to_child, Queue::new(bytes), Arc::clone(&port), WRITE_KEY)
        .unwrap_or_else(|error| fail(&format!("the sender: {error}")));
    let (poster, mut inbox) = port_queue::<Relayed>()
        .unwrap_or_else(|error| fail(&format!("Core's port queue: {error}")));
    let relay = {
        let sender = sender.clone();
        let port = Arc::clone(&port);
        named("crossing-relay", move || {
            set_priority(ThreadPriority::Highest);
            port.post(READ_KEY)
                .unwrap_or_else(|error| fail(&format!("starting the read: {error}")));
            let mut completions = Completions::with_capacity(8);
            let mut reading = true;
            while reading || !sender.is_finished() {
                port.wait(&mut completions, None)
                    .unwrap_or_else(|error| fail(&format!("waiting: {error}")));
                for completion in completions.iter() {
                    if completion.key == WRITE_KEY {
                        sender.complete(&completion);
                        continue;
                    }
                    if !reading {
                        continue;
                    }
                    let taken = reader.take(&completion, &port, |parts| {
                        let at = ticks();
                        let body = parts.without_raw()?.to_vec();
                        poster
                            .send(Relayed { body, at })
                            .map_err(|_| io::Error::other("Core has gone"))
                    });
                    if let Reading::Ended(ended) = taken {
                        ended.unwrap_or_else(|error| fail(&format!("reading the child: {error}")));
                        reading = false;
                    }
                }
            }
        })
    };
    while !core.done() {
        let relayed = inbox
            .recv()
            .unwrap_or_else(|| fail("the relay ended before the last round"));
        let message = Message::parse(&relayed.body, bytes);
        if let Some(request) = core.take(message, relayed.at) {
            sender
                .send(request)
                .unwrap_or_else(|error| fail(&format!("a request: {error:?}")));
        }
    }
    sender.close();
    relay.join().unwrap_or_else(|_| fail("the relay panicked"));
}

// The report.

/// A leg of a round: its name, and its span in ticks.
type Leg = (&'static str, fn(&Round) -> i64);

/// Ticks as microseconds.
struct Scale(f64);

impl Scale {
    fn us(&self, ticks: i64) -> f64 {
        #[allow(clippy::cast_precision_loss, reason = "spans of at most seconds")]
        let ticks = ticks as f64;
        ticks / self.0
    }
}

/// Lists every crossing slower than `--slow-us`, with its start and end as
/// UTC times of day to the microsecond, as `verbatim-trace-waits` takes a
/// window. A tick is put on the system's clock through one pair of readings
/// taken together now, which places it within a few microseconds.
fn list_slow(settings: &Settings, rounds: &[Round], scale: &Scale) {
    let now_ticks = ticks();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| fail("the system's clock is before 1970"));
    #[expect(
        clippy::cast_possible_truncation,
        reason = "microseconds since 1970 fit in an i64"
    )]
    let now_us = now.as_micros() as i64;
    let clock = |at: i64| -> String {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a span of seconds in microseconds"
        )]
        let before = scale.us(now_ticks - at).round() as i64;
        let of_day = (now_us - before).rem_euclid(86_400_000_000);
        format!(
            "{:02}:{:02}:{:02}.{:06}",
            of_day / 3_600_000_000,
            of_day / 60_000_000 % 60,
            of_day / 1_000_000 % 60,
            of_day % 1_000_000
        )
    };
    #[expect(clippy::cast_precision_loss, reason = "a threshold of microseconds")]
    let slow = settings.slow_us as f64;
    let mut listed = 0;
    for (index, round) in rounds.iter().enumerate() {
        let legs = [
            ("tell", round.tell_write, round.tell_relay, round.tell_take),
            ("request", round.request_write, 0, round.request_read),
            (
                "answer",
                round.answer_write,
                round.answer_relay,
                round.answer_take,
            ),
        ];
        for (name, from, relayed, to) in legs {
            let took = scale.us(to - from);
            if took <= slow {
                continue;
            }
            if listed == 0 {
                println!("crossings slower than {} us:", settings.slow_us);
            }
            listed += 1;
            let relay = if relayed == 0 {
                String::new()
            } else {
                format!(", at the relay {}", clock(relayed))
            };
            println!(
                "round {} {name}: {took:.1} us, from {} to {}{relay}",
                settings.warmup + index,
                clock(from),
                clock(to),
            );
        }
    }
    if listed == 0 {
        println!("no crossing slower than {} us", settings.slow_us);
    }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "an index within a sample of thousands"
    )]
    let at = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[at]
}

fn report(settings: &Settings, rounds: &[Round]) {
    #[allow(clippy::cast_precision_loss, reason = "the counter's frequency")]
    let scale = Scale(frequency() as f64 / 1_000_000.0);
    let mut legs: Vec<Leg> = vec![
        ("tell: child write to Core", |r| r.tell_take - r.tell_write),
        ("request: Core write to child", |r| {
            r.request_read - r.request_write
        }),
        ("answer: child write to Core", |r| {
            r.answer_take - r.answer_write
        }),
        ("Core's ask: request out to answer in", |r| {
            r.answer_take - r.request_write
        }),
        ("whole round: tell to answer in", |r| {
            r.answer_take - r.tell_write
        }),
    ];
    if settings.shape == Shape::Relay {
        legs.extend::<[Leg; 4]>([
            ("tell, pipe: child write to relay", |r| {
                r.tell_relay - r.tell_write
            }),
            ("tell, post: relay to Core", |r| r.tell_take - r.tell_relay),
            ("answer, pipe: child write to relay", |r| {
                r.answer_relay - r.answer_write
            }),
            ("answer, post: relay to Core", |r| {
                r.answer_take - r.answer_relay
            }),
        ]);
    }
    println!(
        "shape {}, {} rounds after {} warm-up, {}-byte bodies, gaps {} to {} us, Core at {} priority, {}",
        settings.shape.name(),
        rounds.len(),
        settings.warmup,
        settings.bytes,
        settings.interval_us,
        settings.interval_us + settings.interval_us / 2,
        if settings.core_highest {
            "highest"
        } else {
            "normal"
        },
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
    );
    println!(
        "{:<40} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
        "leg, microseconds", "min", "median", "p90", "p99", "p99.9", "max"
    );
    for (name, leg) in &legs {
        let mut sample: Vec<f64> = rounds.iter().map(|round| scale.us(leg(round))).collect();
        sample.sort_by(f64::total_cmp);
        println!(
            "{name:<40} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
            sample[0],
            percentile(&sample, 0.5),
            percentile(&sample, 0.9),
            percentile(&sample, 0.99),
            percentile(&sample, 0.999),
            sample[sample.len() - 1],
        );
    }
    list_slow(settings, rounds, &scale);
    if let Some(path) = &settings.csv {
        let mut text = String::from(
            "tell_write,tell_relay,tell_take,request_write,request_read,answer_write,answer_relay,answer_take\n",
        );
        for round in rounds {
            let since = |at: i64| {
                if at == 0 {
                    String::new()
                } else {
                    format!("{:.2}", scale.us(at - round.tell_write))
                }
            };
            let _ = writeln!(
                text,
                "0,{},{},{},{},{},{},{}",
                since(round.tell_relay),
                since(round.tell_take),
                since(round.request_write),
                since(round.request_read),
                since(round.answer_write),
                since(round.answer_relay),
                since(round.answer_take),
            );
        }
        std::fs::write(path, text)
            .unwrap_or_else(|error| fail(&format!("writing {path}: {error}")));
    }
}
