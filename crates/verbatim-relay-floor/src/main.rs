//! `verbatim-relay-floor`: the platform floor's benchmark for Verbatim's
//! pipes and verbatim.exe's relay (`docs/archive/relay.md`, section 6;
//! `docs/design/communication.md`, section 11, step 2), run by `cargo
//! xtask relay-floor`.
//!
//! It measures how long a message takes to cross processes on this
//! machine under its own power plan, with the pipes Verbatim uses and the
//! gaps between messages that leave every thread idle when a message
//! arrives, as events do. Roles stand in for Verbatim's processes: a
//! sender for the focus listener, a relay for verbatim.exe, a receiver for
//! an outpost, and a host for a synthesizer host. The relay is this
//! process, which launches the others as Verbatim launches its children,
//! contained in a job (`verbatim_process::launch_with_ends`, over
//! anonymous pipes it makes itself or over `overlapped::pipe`s). The
//! variants:
//!
//! - F0: two threads in one process and a condition variable, the floor
//!   for one wake-up.
//! - F1: a sender thread here writes straight to the receiver over one
//!   pipe, the floor for one hop.
//! - F2: sender to receiver through a relay that is one thread reading
//!   synchronously and writing at once, the floor for any third process.
//! - F3: the same through a relay built as `docs/archive/relay.md` section 4.1 designs
//!   verbatim.exe's: one thread on a completion port, its own ends
//!   overlapped, the children's synchronous.
//! - F4: the same through a relay built as verbatim.exe's was before stage
//!   A step 9: a reader thread, a channel, an owner thread, a condition
//!   variable, and a writer thread. The baseline.
//! - F5: F3's relay with the children's ends overlapped too, the child
//!   side of the communication design: the sender's producing threads
//!   write their own frames and an I/O thread only finishes writes that
//!   pended (or, with `--writes io-thread`, the I/O thread writes every
//!   frame), and the receiver reads on its I/O thread's port.
//! - F6: a synthesizer host's round trip: a command, a fixed burst of CPU
//!   standing in for synthesizing a first chunk, and a reply larger than
//!   the 4 KiB pipe that paces it.
//! - `frames`: how many reads a 1, 2 and 32 MB frame takes, with the read
//!   waiting first and with the data arriving first, and a child that
//!   drains a 2 MB write before it exits.
//!
//! Every process opts out of power throttling, and every thread on the
//! measured path runs at the priority asked for. Each message carries the
//! time its sender began to send it; the relay and the receiver keep their
//! stamps in memory and report them when the run ends, so nothing is
//! written to a file or a console on the measured path.

mod stats;
mod wire;

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::os::windows::io::{FromRawHandle, OwnedHandle, RawHandle};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use verbatim_process::overlapped::{self, Completions, End, Flow, PipeIo, Port, Started};
use verbatim_process::scheduling::{ThreadPriority, opt_out_of_power_throttling};
use verbatim_process::{ChildEnds, ChildSpec, Contained};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::CreatePipe;

use stats::{Samples, allocations};
use wire::{
    Arrived, END, FrameReader, Gaps, Incoming, Outgoing, PADDING, Sender, Timer, Totals, Writes,
    body, encode, encode_json, now, prioritize, read_frame,
};

#[global_allocator]
static ALLOCATOR: stats::Counting = stats::Counting;

/// The event pipes' buffer (`docs/design/communication.md`, section 4.1).
const EVENT_BUFFER: u32 = 64 * 1024;

/// The synthesizer host's audio pipe's buffer, whose smallness paces the
/// host.
const PCM_BUFFER: u32 = 4096;

/// How long the relay waits for a child to exit once the run has ended.
const EXIT_BOUND: Duration = Duration::from_secs(60);

/// What one run sends.
#[derive(Clone, Copy, Debug)]
struct Plan {
    /// Frames per variant.
    messages: u64,
    /// The mean gap between bursts, in microseconds.
    mean_gap_us: u64,
    seed: u64,
    priority: ThreadPriority,
    /// Frames sent back to back after each gap.
    burst: u64,
    /// The sender's producing threads, all writing to one pipe.
    producers: u64,
    /// Who writes an overlapped sender's frames.
    writes: Writes,
    /// F6: the host's burst of CPU per command, in microseconds.
    burst_us: u64,
    /// F6: the size of the host's reply, in bytes.
    reply_bytes: u64,
}

impl Plan {
    /// The plan as child arguments.
    fn arguments(&self) -> String {
        format!(
            "--messages {} --mean-gap-us {} --seed {} --priority {} --burst {} --producers {} \
             --writes {} --burst-us {} --reply-bytes {}",
            self.messages,
            self.mean_gap_us,
            self.seed,
            priority_name(self.priority),
            self.burst,
            self.producers,
            writes_name(self.writes),
            self.burst_us,
            self.reply_bytes,
        )
    }
}

/// What a variant found: its hops, and lines of counts.
#[derive(Default)]
struct Outcome {
    hops: Vec<Samples>,
    notes: Vec<String>,
}

/// The variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Variant {
    F0,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    Frames,
}

impl Variant {
    const ALL: [Variant; 8] = [
        Self::F0,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::Frames,
    ];

    fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|variant| format!("{variant:?}").eq_ignore_ascii_case(text))
    }

    fn describe(self) -> &'static str {
        match self {
            Self::F0 => "one wake-up in one process",
            Self::F1 => "one hop, sender straight to receiver",
            Self::F2 => "through a minimal relay process",
            Self::F3 => "through a relay on a completion port, the children synchronous",
            Self::F4 => "through a relay built as verbatim.exe's was before stage A step 9",
            Self::F5 => "through a relay on a completion port, the children overlapped",
            Self::F6 => "a synthesizer host's round trip",
            Self::Frames => "large frames, and a child draining a write before it exits",
        }
    }
}

/// The load the run is measured under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Load {
    Idle,
    /// A normal-priority spinning thread on every logical processor.
    All,
    /// One fewer, leaving one logical processor free.
    AllButOne,
}

/// Spinning threads at normal priority, stopped when dropped.
struct Spinners {
    stop: Arc<AtomicBool>,
    threads: Vec<thread::JoinHandle<()>>,
}

impl Spinners {
    fn start(load: Load) -> Self {
        let processors = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let count = match load {
            Load::Idle => 0,
            Load::All => processors,
            Load::AllButOne => processors.saturating_sub(1),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..count)
            .map(|_| {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self { stop, threads }
    }
}

impl Drop for Spinners {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Where a sender's frames go: a synchronous pipe end, or an overlapped
/// one with its [`Sender`].
enum Out {
    Sync(Mutex<File>),
    Overlapped(Sender),
}

impl Out {
    fn new(end: OwnedHandle, overlapped: bool, plan: &Plan) -> io::Result<Self> {
        Ok(if overlapped {
            Self::Overlapped(Sender::new(end, plan.writes, plan.priority)?)
        } else {
            Self::Sync(Mutex::new(File::from(end)))
        })
    }

    /// Sends one frame with one write.
    fn send(&self, frame: &[u8]) -> io::Result<()> {
        match self {
            Self::Sync(file) => file
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .write_all(frame),
            Self::Overlapped(sender) => sender.send(frame),
        }
    }

    /// Writes issued and writes that pended, for an overlapped end.
    fn counts(&self) -> (u64, u64) {
        match self {
            Self::Sync(_) => (0, 0),
            Self::Overlapped(sender) => sender.counts(),
        }
    }

    /// Waits until every frame is written whole.
    fn finish(self) -> io::Result<()> {
        match self {
            Self::Sync(_) => Ok(()),
            Self::Overlapped(sender) => sender.finish().map(drop),
        }
    }
}

/// Sends the plan's frames to `out` from its producing threads, each
/// burst after a gap, then the end, carrying the sender's totals since
/// `allocations_at`.
fn produce(out: &Out, plan: Plan, allocations_at: u64) -> io::Result<()> {
    let producers = plan.producers.max(1);
    thread::scope(|scope| {
        let threads: Vec<_> = (0..producers)
            .map(|first| scope.spawn(move || produce_from(out, plan, first, producers)))
            .collect();
        threads.into_iter().try_for_each(|thread| {
            thread
                .join()
                .map_err(|_| io::Error::other("a producer panicked"))?
        })
    })?;
    let (writes, pended) = out.counts();
    let totals = Totals {
        allocations: allocations() - allocations_at,
        frames: plan.messages,
        writes,
        pended,
    };
    let mut frame = Vec::new();
    encode_json(
        &Outgoing {
            seq: END,
            sent: 0,
            send: None,
            pad: "",
            totals: Some(totals),
        },
        &mut Vec::new(),
        &mut frame,
    )?;
    out.send(&frame)
}

/// One producing thread: sends the frames numbered `first`, `first +
/// producers`, and so on.
fn produce_from(out: &Out, plan: Plan, first: u64, producers: u64) -> io::Result<()> {
    prioritize(plan.priority);
    let timer = Timer::new()?;
    let mut gaps = Gaps::new(plan.seed.wrapping_add(first), plan.mean_gap_us);
    let pad = "x".repeat(PADDING);
    let mut body = Vec::with_capacity(PADDING + 256);
    let mut frame = Vec::with_capacity(PADDING + 256);
    let mut previous = None;
    let mut seq = first;
    while seq < plan.messages {
        timer.wait(gaps.next())?;
        for _ in 0..plan.burst.max(1) {
            if seq >= plan.messages {
                break;
            }
            let sent = now();
            encode_json(
                &Outgoing {
                    seq,
                    sent,
                    send: previous,
                    pad: &pad,
                    totals: None,
                },
                &mut body,
                &mut frame,
            )?;
            let before = now();
            out.send(&frame)?;
            previous = Some(now().saturating_sub(before));
            seq += producers;
        }
    }
    Ok(())
}

/// What a receiver reports when the run ends: the sender's totals, its
/// own, and `[seq, sent, read, send]` per frame, `send` being `u64::MAX`
/// where the frame carried none.
#[derive(Serialize, Deserialize)]
struct Report {
    sender: Totals,
    receiver: Totals,
    stamps: Vec<[u64; 4]>,
}

/// A receiver's frames so far.
struct Received {
    stamps: Vec<[u64; 4]>,
    sender: Option<Totals>,
}

impl Received {
    fn take(&mut self, frame: &[u8], read: u64) -> io::Result<()> {
        let message: Incoming = serde_json::from_slice(body(frame)?).map_err(io::Error::other)?;
        if message.seq == END {
            self.sender = Some(message.totals.unwrap_or_default());
        } else {
            self.stamps.push([
                message.seq,
                message.sent,
                read,
                message.send.unwrap_or(u64::MAX),
            ]);
        }
        Ok(())
    }
}

/// The receiver: stamps each frame as its read returns and, when the end
/// comes, reports every stamp as one frame.
fn receive_all(
    commands: OwnedHandle,
    out: OwnedHandle,
    overlapped: bool,
    plan: Plan,
) -> io::Result<()> {
    prioritize(plan.priority);
    let capacity = usize::try_from(plan.messages).unwrap_or(0);
    let mut received = Received {
        stamps: Vec::with_capacity(capacity),
        sender: None,
    };
    let start = allocations();
    if overlapped {
        let port = Port::new()?;
        let mut reader = FrameReader::new(port.associate(commands, 1)?);
        let mut completions = Completions::with_capacity(16);
        reader.pump(&mut |arrived: Arrived| received.take(&arrived.frame, arrived.at))?;
        while received.sender.is_none() {
            if reader.ended() {
                return Err(io::Error::other("the relay ended early"));
            }
            port.wait(&mut completions, None)?;
            let woke = now();
            for completion in completions.iter() {
                if reader.owns(&completion) {
                    reader.complete(&completion, woke, &mut |arrived: Arrived| {
                        received.take(&arrived.frame, arrived.at)
                    })?;
                }
            }
        }
        // The end was the last frame: the relay closes the pipe next, which
        // ends the read still waiting.
        while !reader.ended() {
            port.wait(&mut completions, None)?;
            for completion in completions.iter() {
                if reader.owns(&completion) {
                    reader.complete(&completion, now(), &mut |_| Ok(()))?;
                }
            }
        }
    } else {
        let mut reader = BufReader::new(File::from(commands));
        let mut frame = Vec::new();
        while received.sender.is_none() {
            if !read_frame(&mut reader, &mut frame)? {
                return Err(io::Error::other("the relay ended early"));
            }
            received.take(&frame, now())?;
        }
    }
    let own = Totals {
        allocations: allocations() - start,
        frames: received.stamps.len() as u64,
        writes: 0,
        pended: 0,
    };
    let report = Report {
        sender: received.sender.unwrap_or_default(),
        receiver: own,
        stamps: received.stamps,
    };
    let mut frame = Vec::new();
    encode_json(&report, &mut Vec::new(), &mut frame)?;
    let out = Out::new(out, overlapped, &plan)?;
    out.send(&frame)?;
    out.finish()
}

/// A child launched over anonymous pipes, with Core's synchronous ends.
struct SyncChild {
    process: Contained,
    to_child: File,
    from_child: File,
}

/// A child launched over `overlapped::pipe`s, with Core's overlapped ends.
struct OverlappedChild {
    process: Contained,
    to_child: OwnedHandle,
    from_child: OwnedHandle,
}

/// The child's command line: its role, its pipes, whether they are
/// overlapped, and the plan.
fn child_arguments(role: &str, plan: Plan, overlapped: bool) -> impl Fn(usize, usize) -> String {
    let role = role.to_owned();
    move |pipe_in, pipe_out| {
        format!(
            "{role} --pipe-in {pipe_in} --pipe-out {pipe_out}{} {}",
            if overlapped { " --overlapped" } else { "" },
            plan.arguments()
        )
    }
}

/// Launches this executable in `role` as Verbatim launched its children
/// before the communication design: two anonymous pipes, synchronous at
/// both ends, with the system's default buffers. The variants that measure
/// those pipes (F1, F2 and F4) launch their children this way, so the
/// pipes are made here, `verbatim_process` making only overlapped ones.
fn launch_sync(role: &str, plan: Plan) -> io::Result<SyncChild> {
    let exe = std::env::current_exe()?;
    let arguments = child_arguments(role, plan, false);
    let (commands, to_child) = anonymous_pipe()?;
    let (from_child, messages) = anonymous_pipe()?;
    let process = verbatim_process::launch_with_ends(
        &ChildSpec {
            exe: &exe,
            arguments: &arguments,
            log_stem: "relay-floor",
            memory_cap: None,
        },
        ChildEnds { commands, messages },
    )?;
    Ok(SyncChild {
        process,
        to_child: File::from(to_child),
        from_child: File::from(from_child),
    })
}

/// Makes one anonymous pipe with the system's default buffer, both ends
/// synchronous and neither inheritable (`launch_with_ends` lets the child
/// inherit its end only while it is created): the read end, then the
/// write end.
fn anonymous_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    // SAFETY: CreatePipe writes both out-handles before they are used; with
    // no security attributes, neither end is inheritable.
    unsafe { CreatePipe(&raw mut read, &raw mut write, None, 0) }.map_err(io::Error::from)?;
    // SAFETY: CreatePipe succeeded, so each is a valid pipe handle this
    // process now owns and nothing else holds.
    let read = unsafe { OwnedHandle::from_raw_handle(read.0 as RawHandle) };
    // SAFETY: as for the read end.
    let write = unsafe { OwnedHandle::from_raw_handle(write.0 as RawHandle) };
    Ok((read, write))
}

/// Launches this executable in `role` over two `overlapped::pipe`s: Core's
/// ends overlapped, the child's as `child` says, the pipe from the child
/// with `from_child_buffer`.
fn launch_overlapped(
    role: &str,
    plan: Plan,
    child: End,
    from_child_buffer: u32,
) -> io::Result<OverlappedChild> {
    let exe = std::env::current_exe()?;
    let arguments = child_arguments(role, plan, child == End::Overlapped);
    let (to_child, commands) = overlapped::pipe(Flow::Outbound, EVENT_BUFFER, child)?;
    let (from_child, messages) = overlapped::pipe(Flow::Inbound, from_child_buffer, child)?;
    let process = verbatim_process::launch_with_ends(
        &ChildSpec {
            exe: &exe,
            arguments: &arguments,
            log_stem: "relay-floor",
            memory_cap: None,
        },
        ChildEnds { commands, messages },
    )?;
    Ok(OverlappedChild {
        process,
        to_child,
        from_child,
    })
}

/// Reads one frame, whole, from Core's overlapped end `from`, on a port of
/// its own, then waits for the other end to close; returns the frame and
/// how many reads it took.
fn read_one_overlapped(from: OwnedHandle) -> io::Result<(Vec<u8>, u64)> {
    let port = Port::new()?;
    let mut reader = FrameReader::new(port.associate(from, 1)?);
    let mut completions = Completions::with_capacity(4);
    let mut got: Option<(Vec<u8>, u64)> = None;
    let mut keep = |arrived: Arrived| {
        if got.is_none() {
            got = Some((arrived.frame, arrived.reads));
        }
        Ok(())
    };
    reader.pump(&mut keep)?;
    while !reader.ended() {
        port.wait(&mut completions, None)?;
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, now(), &mut keep)?;
            }
        }
    }
    got.ok_or_else(|| io::Error::other("the child closed its pipe before its frame"))
}

fn parse_report(frame: &[u8]) -> io::Result<Report> {
    serde_json::from_slice(body(frame)?).map_err(io::Error::other)
}

fn wait_for_exit(child: &Contained, role: &str) -> io::Result<()> {
    if child.wait_for_exit(EXIT_BOUND) && child.exit_code() == Some(0) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "the {role} did not exit cleanly within {EXIT_BOUND:?} ({:?})",
            child.exit_code()
        )))
    }
}

/// The send-call samples a report carries.
fn send_calls(report: &Report) -> Samples {
    let mut calls = Samples::new("send call");
    for &[_, _, _, send] in &report.stamps {
        if send != u64::MAX {
            calls.add(0, send);
        }
    }
    calls
}

/// F0: one wake-up between two threads of this process.
fn run_f0(plan: Plan) -> io::Result<Outcome> {
    let slot = Arc::new((Mutex::new(VecDeque::<(u64, u64)>::new()), Condvar::new()));
    let receiver_slot = Arc::clone(&slot);
    let receiver = thread::spawn(move || {
        prioritize(plan.priority);
        let (queue, ready) = &*receiver_slot;
        let mut stamps = Vec::new();
        loop {
            let mut waiting = queue.lock().unwrap_or_else(PoisonError::into_inner);
            let (seq, sent) = loop {
                if let Some(item) = waiting.pop_front() {
                    break item;
                }
                waiting = ready.wait(waiting).unwrap_or_else(PoisonError::into_inner);
            };
            let read = now();
            drop(waiting);
            if seq == END {
                return stamps;
            }
            stamps.push((sent, read));
        }
    });
    let sender_slot = Arc::clone(&slot);
    let sender = thread::spawn(move || -> io::Result<()> {
        prioritize(plan.priority);
        let timer = Timer::new()?;
        let mut gaps = Gaps::new(plan.seed, plan.mean_gap_us);
        let (queue, ready) = &*sender_slot;
        for seq in 0..plan.messages {
            timer.wait(gaps.next())?;
            queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back((seq, now()));
            ready.notify_one();
        }
        queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back((END, 0));
        ready.notify_one();
        Ok(())
    });
    sender
        .join()
        .map_err(|_| io::Error::other("the sender panicked"))??;
    let stamps = receiver
        .join()
        .map_err(|_| io::Error::other("the receiver panicked"))?;
    let mut hop = Samples::new("wake-up");
    for (sent, read) in stamps {
        hop.add(sent, read);
    }
    Ok(Outcome {
        hops: vec![hop],
        notes: Vec::new(),
    })
}

/// F1: sending threads here write straight to the receiver.
fn run_f1(plan: Plan) -> io::Result<Outcome> {
    let receiver = launch_sync("--receiver", plan)?;
    let out = Out::Sync(Mutex::new(receiver.to_child));
    produce(&out, plan, allocations())?;
    drop(out);
    let mut reader = BufReader::new(receiver.from_child);
    let mut frame = Vec::new();
    if !read_frame(&mut reader, &mut frame)? {
        return Err(io::Error::other("the receiver sent no report"));
    }
    let report = parse_report(&frame)?;
    wait_for_exit(&receiver.process, "receiver")?;
    let mut hop = Samples::new("sender to receiver");
    for &[_, sent, read, _] in &report.stamps {
        hop.add(sent, read);
    }
    let calls = send_calls(&report);
    Ok(Outcome {
        hops: vec![hop, calls],
        notes: vec![format!(
            "allocations: sender {}, receiver {}",
            stats::per_frame(report.sender.allocations, report.sender.frames),
            stats::per_frame(report.receiver.allocations, report.receiver.frames),
        )],
    })
}

/// What the relay stamped for one message: when its read returned and when
/// it began the write.
#[derive(Clone, Copy)]
struct RelayStamp {
    seq: u64,
    read: u64,
    write: u64,
}

/// Joins the relay's stamps with the receiver's report into the hops.
fn relayed_outcome(stamps: &[RelayStamp], report: &Report, relay: Totals) -> io::Result<Outcome> {
    if report.stamps.len() != stamps.len() {
        return Err(io::Error::other(format!(
            "the relay passed {} messages and the receiver reported {}",
            stamps.len(),
            report.stamps.len()
        )));
    }
    let mut in_hop = Samples::new("sender to relay");
    let mut relay_hop = Samples::new("relay");
    let mut out_hop = Samples::new("relay to receiver");
    let mut whole = Samples::new("end to end");
    let by_seq: HashMap<u64, RelayStamp> = stamps.iter().map(|stamp| (stamp.seq, *stamp)).collect();
    for &[seq, sent, read, _] in &report.stamps {
        let stamp = by_seq
            .get(&seq)
            .ok_or_else(|| io::Error::other(format!("the receiver had {seq}, the relay not")))?;
        in_hop.add(sent, stamp.read);
        relay_hop.add(stamp.read, stamp.write);
        out_hop.add(stamp.write, read);
        whole.add(sent, read);
    }
    Ok(Outcome {
        hops: vec![in_hop, relay_hop, out_hop, whole, send_calls(report)],
        notes: vec![
            format!(
                "allocations: sender {}; relay {}; receiver {}",
                stats::per_frame(report.sender.allocations, report.sender.frames),
                stats::per_frame(relay.allocations, relay.frames),
                stats::per_frame(report.receiver.allocations, report.receiver.frames),
            ),
            format!(
                "writes: sender {}; relay {}",
                stats::pended(report.sender.writes, report.sender.pended),
                stats::pended(relay.writes, relay.pended),
            ),
        ],
    })
}

/// The relay's totals: its allocations since `start` and its frames.
fn relay_totals(start: u64, frames: usize, writes: u64, pended: u64) -> Totals {
    Totals {
        allocations: allocations() - start,
        frames: frames as u64,
        writes,
        pended,
    }
}

fn parse_incoming(frame: &[u8]) -> io::Result<Incoming> {
    serde_json::from_slice(body(frame)?).map_err(io::Error::other)
}

/// F2: one thread reads synchronously and writes at once.
fn run_f2(plan: Plan) -> io::Result<Outcome> {
    let sender = launch_sync("--sender", plan)?;
    let receiver = launch_sync("--receiver", plan)?;
    let mut to_receiver = receiver.to_child;
    prioritize(plan.priority);
    let mut reader = BufReader::new(sender.from_child);
    let mut stamps = Vec::with_capacity(usize::try_from(plan.messages).unwrap_or(0));
    let mut frame = Vec::new();
    let start = allocations();
    loop {
        if !read_frame(&mut reader, &mut frame)? {
            return Err(io::Error::other("the sender ended early"));
        }
        let read = now();
        let message = parse_incoming(&frame)?;
        let write = now();
        to_receiver.write_all(&frame)?;
        if message.seq == END {
            break;
        }
        stamps.push(RelayStamp {
            seq: message.seq,
            read,
            write,
        });
    }
    let relay = relay_totals(start, stamps.len(), 0, 0);
    drop(to_receiver);
    let mut reader = BufReader::new(receiver.from_child);
    if !read_frame(&mut reader, &mut frame)? {
        return Err(io::Error::other("the receiver sent no report"));
    }
    let report = parse_report(&frame)?;
    wait_for_exit(&sender.process, "sender")?;
    wait_for_exit(&receiver.process, "receiver")?;
    relayed_outcome(&stamps, &report, relay)
}

/// The frames a port relay has read and not yet written.
#[derive(Default)]
struct RelayQueue {
    queue: VecDeque<(u64, u64, Vec<u8>)>,
}

impl RelayQueue {
    /// Parses the frame, as verbatim.exe parses what it routes, and queues
    /// it with the time its read returned.
    fn take(&mut self, arrived: Arrived) -> io::Result<()> {
        let message = parse_incoming(&arrived.frame)?;
        self.queue
            .push_back((message.seq, arrived.at, arrived.frame));
        Ok(())
    }
}

/// F3 and F5: one thread on a completion port, as verbatim.exe's relay: it
/// reads until a read waits, writes with one write in flight, and waits
/// only on the port. The children's ends are as `children` says.
fn run_port_relay(plan: Plan, children: End) -> io::Result<Outcome> {
    const FROM_SENDER: usize = 1;
    const TO_RECEIVER: usize = 2;
    let sender = launch_overlapped("--sender", plan, children, EVENT_BUFFER)?;
    let receiver = launch_overlapped("--receiver", plan, children, EVENT_BUFFER)?;
    prioritize(plan.priority);
    let port = Port::new()?;
    let mut reader = FrameReader::new(port.associate(sender.from_child, FROM_SENDER)?);
    let to_receiver = port.associate(receiver.to_child, TO_RECEIVER)?;
    let mut write = PipeIo::new(0);
    let mut queue = RelayQueue::default();
    let mut stamps = Vec::with_capacity(usize::try_from(plan.messages).unwrap_or(0));
    let mut completions = Completions::with_capacity(16);
    let (mut writes, mut pended) = (0, 0);
    let mut forwarded_end = false;
    let start = allocations();
    reader.pump(&mut |arrived| queue.take(arrived))?;
    loop {
        // Write what is queued, one write in flight.
        while !write.in_flight() {
            let Some((seq, read_at, frame)) = queue.queue.pop_front() else {
                break;
            };
            let write_at = now();
            if seq == END {
                forwarded_end = true;
            } else {
                stamps.push(RelayStamp {
                    seq,
                    read: read_at,
                    write: write_at,
                });
            }
            writes += 1;
            match write.write(&to_receiver, &frame) {
                Started::Done(_) => {}
                Started::Pending => pended += 1,
                Started::Failed(error) => return Err(error),
            }
        }
        if forwarded_end && !write.in_flight() {
            break;
        }
        if reader.ended() && !forwarded_end && queue.queue.is_empty() {
            return Err(io::Error::other("the sender ended early"));
        }
        port.wait(&mut completions, None)?;
        // The first instruction after the wake, as verbatim.exe's relay
        // stamps a read's completion.
        let woke = now();
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, woke, &mut |arrived| queue.take(arrived))?;
            } else if write.owns(&completion) {
                write.complete(&to_receiver, &completion)?;
            }
        }
    }
    let relay = relay_totals(start, stamps.len(), writes, pended);
    // Nothing is in flight on the receiver's pipe: the end was the last
    // thing written. The sender closes its pipe once its last write has
    // completed, which ends the read still waiting.
    drop(to_receiver);
    while !reader.ended() {
        port.wait(&mut completions, None)?;
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, now(), &mut |_| Ok(()))?;
            }
        }
    }
    let (frame, _) = read_one_overlapped(receiver.from_child)?;
    let report = parse_report(&frame)?;
    wait_for_exit(&sender.process, "sender")?;
    wait_for_exit(&receiver.process, "receiver")?;
    relayed_outcome(&stamps, &report, relay)
}

/// F4: a reader thread, a channel, an owner thread, a condition variable,
/// and a writer thread, as verbatim.exe was before stage A step 9.
fn run_f4(plan: Plan) -> io::Result<Outcome> {
    let sender = launch_sync("--sender", plan)?;
    let receiver = launch_sync("--receiver", plan)?;
    let from_sender = sender.from_child;
    let mut to_receiver = receiver.to_child;
    let priority = plan.priority;
    let start = allocations();
    let (to_owner, owner_events) = crossbeam_channel::unbounded::<(u64, u64, Vec<u8>)>();
    let reader = thread::spawn(move || -> io::Result<()> {
        prioritize(priority);
        let mut reader = BufReader::new(from_sender);
        loop {
            let mut frame = Vec::new();
            if !read_frame(&mut reader, &mut frame)? {
                return Ok(());
            }
            let read = now();
            let message = parse_incoming(&frame)?;
            let end = message.seq == END;
            if to_owner.send((message.seq, read, frame)).is_err() || end {
                return Ok(());
            }
        }
    });
    let shared = Arc::new((
        Mutex::new(VecDeque::<(u64, u64, Vec<u8>)>::new()),
        Condvar::new(),
    ));
    let owner_shared = Arc::clone(&shared);
    let owner = thread::spawn(move || {
        prioritize(priority);
        for item in owner_events {
            let end = item.0 == END;
            let (queue, ready) = &*owner_shared;
            queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_back(item);
            ready.notify_one();
            if end {
                return;
            }
        }
    });
    let capacity = usize::try_from(plan.messages).unwrap_or(0);
    let writer = thread::spawn(move || -> io::Result<Vec<RelayStamp>> {
        prioritize(priority);
        let mut stamps = Vec::with_capacity(capacity);
        loop {
            let (seq, read, frame) = {
                let (queue, ready) = &*shared;
                let mut waiting = queue.lock().unwrap_or_else(PoisonError::into_inner);
                loop {
                    if let Some(item) = waiting.pop_front() {
                        break item;
                    }
                    waiting = ready.wait(waiting).unwrap_or_else(PoisonError::into_inner);
                }
            };
            let write = now();
            to_receiver.write_all(&frame)?;
            if seq == END {
                return Ok(stamps);
            }
            stamps.push(RelayStamp { seq, read, write });
        }
    });
    reader
        .join()
        .map_err(|_| io::Error::other("the reader panicked"))??;
    owner
        .join()
        .map_err(|_| io::Error::other("the owner panicked"))?;
    let stamps = writer
        .join()
        .map_err(|_| io::Error::other("the writer panicked"))??;
    let relay = relay_totals(start, stamps.len(), 0, 0);
    let mut reader = BufReader::new(receiver.from_child);
    let mut frame = Vec::new();
    if !read_frame(&mut reader, &mut frame)? {
        return Err(io::Error::other("the receiver sent no report"));
    }
    let report = parse_report(&frame)?;
    wait_for_exit(&sender.process, "sender")?;
    wait_for_exit(&receiver.process, "receiver")?;
    relayed_outcome(&stamps, &report, relay)
}

/// A synthesizer host's reply, as the host writes it.
#[derive(Serialize)]
struct ReplyOut<'a> {
    seq: u64,
    sent: u64,
    /// When the host's read of the command returned.
    got: u64,
    /// When the host began its reply, after the burst.
    replied: u64,
    pad: &'a str,
}

/// A synthesizer host's reply, as Core reads it.
#[derive(Deserialize)]
struct ReplyIn {
    seq: u64,
    sent: u64,
    got: u64,
    replied: u64,
}

/// The host: reads each command, burns `burst_us` of CPU as synthesis of a
/// first chunk would, and replies with `reply_bytes` in one write on its
/// 4 KiB pipe, which paces it.
fn host(commands: OwnedHandle, out: OwnedHandle, plan: Plan) -> io::Result<()> {
    prioritize(plan.priority);
    let mut reader = BufReader::new(File::from(commands));
    let mut out = File::from(out);
    let pad = "x".repeat(usize::try_from(plan.reply_bytes.saturating_sub(128)).unwrap_or(0));
    let (mut command, mut body, mut frame) = (Vec::new(), Vec::new(), Vec::new());
    let burst = plan.burst_us * 10;
    while read_frame(&mut reader, &mut command)? {
        let got = now();
        let message = parse_incoming(&command)?;
        if message.seq == END {
            break;
        }
        while now().saturating_sub(got) < burst {
            std::hint::spin_loop();
        }
        let replied = now();
        encode_json(
            &ReplyOut {
                seq: message.seq,
                sent: message.sent,
                got,
                replied,
                pad: &pad,
            },
            &mut body,
            &mut frame,
        )?;
        out.write_all(&frame)?;
    }
    Ok(())
}

/// F6: Core's synthesizer thread sends a command after each gap and waits
/// on its port for the whole reply, its read already waiting.
fn run_f6(plan: Plan) -> io::Result<Outcome> {
    const COMMANDS: usize = 1;
    const REPLIES: usize = 2;
    let host = launch_overlapped("--host", plan, End::Synchronous, PCM_BUFFER)?;
    prioritize(plan.priority);
    let port = Port::new()?;
    let commands = port.associate(host.to_child, COMMANDS)?;
    let mut reader = FrameReader::new(port.associate(host.from_child, REPLIES)?);
    let mut write = PipeIo::new(0);
    let mut completions = Completions::with_capacity(16);
    let timer = Timer::new()?;
    let mut gaps = Gaps::new(plan.seed, plan.mean_gap_us);
    let (mut json, mut frame) = (Vec::new(), Vec::new());
    let (mut writes, mut pended) = (0u64, 0u64);
    let mut reply: Option<Arrived> = None;
    let mut hops = HostHops::default();
    reader.pump(&mut |arrived| {
        reply = Some(arrived);
        Ok(())
    })?;
    for seq in 0..=plan.messages {
        let last = seq == plan.messages;
        if !last {
            timer.wait(gaps.next())?;
        }
        // A command write that pended has completed by now in practice;
        // the wait is for that evidence all the same.
        while write.in_flight() {
            port.wait(&mut completions, None)?;
            for completion in completions.iter() {
                if write.owns(&completion) {
                    write.complete(&commands, &completion)?;
                }
            }
        }
        let sent = now();
        encode_json(
            &Outgoing {
                seq: if last { END } else { seq },
                sent,
                send: None,
                pad: "",
                totals: None,
            },
            &mut json,
            &mut frame,
        )?;
        writes += 1;
        match write.write(&commands, &frame) {
            Started::Done(_) => {}
            Started::Pending => pended += 1,
            Started::Failed(error) => return Err(error),
        }
        if last {
            break;
        }
        while reply.is_none() {
            if reader.ended() {
                return Err(io::Error::other("the host ended early"));
            }
            port.wait(&mut completions, None)?;
            let woke = now();
            for completion in completions.iter() {
                if reader.owns(&completion) {
                    reader.complete(&completion, woke, &mut |arrived| {
                        reply = Some(arrived);
                        Ok(())
                    })?;
                } else if write.owns(&completion) {
                    write.complete(&commands, &completion)?;
                }
            }
        }
        let arrived = reply.take().ok_or_else(|| io::Error::other("no reply"))?;
        hops.record(&arrived, seq)?;
    }
    // The host exits on the end, which ends the read still waiting.
    while !reader.ended() || write.in_flight() {
        port.wait(&mut completions, None)?;
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, now(), &mut |_| Ok(()))?;
            } else if write.owns(&completion) {
                write.complete(&commands, &completion)?;
            }
        }
    }
    wait_for_exit(&host.process, "host")?;
    Ok(hops.outcome(plan, writes, pended))
}

/// F6's hops, and how many reads each reply took.
struct HostHops {
    command: Samples,
    burst: Samples,
    reply: Samples,
    round_trip: Samples,
    reads_per_reply: HashMap<u64, u64>,
}

impl Default for HostHops {
    fn default() -> Self {
        Self {
            command: Samples::new("command to host"),
            burst: Samples::new("host's burst"),
            reply: Samples::new("reply to Core, whole"),
            round_trip: Samples::new("round trip"),
            reads_per_reply: HashMap::new(),
        }
    }
}

impl HostHops {
    /// Records the reply to command `seq`.
    fn record(&mut self, arrived: &Arrived, seq: u64) -> io::Result<()> {
        let message: ReplyIn =
            serde_json::from_slice(body(&arrived.frame)?).map_err(io::Error::other)?;
        if message.seq != seq {
            return Err(io::Error::other(format!("reply {} to {seq}", message.seq)));
        }
        self.command.add(message.sent, message.got);
        self.burst.add(message.got, message.replied);
        self.reply.add(message.replied, arrived.at);
        self.round_trip.add(message.sent, arrived.at);
        *self.reads_per_reply.entry(arrived.reads).or_default() += 1;
        Ok(())
    }

    fn outcome(self, plan: Plan, writes: u64, pended: u64) -> Outcome {
        let mut reads: Vec<_> = self.reads_per_reply.into_iter().collect();
        reads.sort_unstable();
        let reads = reads
            .iter()
            .map(|(reads, count)| format!("{count} in {reads}"))
            .collect::<Vec<_>>()
            .join(", ");
        Outcome {
            hops: vec![self.command, self.burst, self.reply, self.round_trip],
            notes: vec![
                format!(
                    "replies of {} bytes on a {PCM_BUFFER}-byte pipe, by reads taken: {reads}",
                    plan.reply_bytes
                ),
                format!("command writes: {}", stats::pended(writes, pended)),
            ],
        }
    }
}

/// One large frame over an event pipe in this process, both ends
/// overlapped on one port: how many reads it took, whether its write
/// pended, and how long from the write's start to the frame being whole.
fn one_large_frame(size: usize, read_first: bool) -> io::Result<(u64, bool, f64)> {
    let port = Port::new()?;
    let (ours, theirs) = overlapped::pipe(Flow::Outbound, EVENT_BUFFER, End::Overlapped)?;
    let writer = port.associate(ours, 1)?;
    let mut reader = FrameReader::new(port.associate(theirs, 2)?);
    let mut frame = Vec::new();
    encode(&vec![b'x'; size], &mut frame);
    let mut write = PipeIo::new(0);
    let mut completions = Completions::with_capacity(16);
    let mut got: Option<Arrived> = None;
    let start_write = |write: &mut PipeIo| match write.write(&writer, &frame) {
        Started::Done(_) => Ok(false),
        Started::Pending => Ok(true),
        Started::Failed(error) => Err(error),
    };
    let start;
    let pended;
    if read_first {
        reader.pump(&mut |arrived| {
            got = Some(arrived);
            Ok(())
        })?;
        start = now();
        pended = start_write(&mut write)?;
    } else {
        start = now();
        pended = start_write(&mut write)?;
        reader.pump(&mut |arrived| {
            got = Some(arrived);
            Ok(())
        })?;
    }
    while got.is_none() || write.in_flight() {
        port.wait(&mut completions, None)?;
        let woke = now();
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, woke, &mut |arrived| {
                    got = Some(arrived);
                    Ok(())
                })?;
            } else if write.owns(&completion) {
                write.complete(&writer, &completion)?;
            }
        }
    }
    let arrived = got.ok_or_else(|| io::Error::other("no frame"))?;
    if body(&arrived.frame)?.len() != size {
        return Err(io::Error::other("the frame arrived cut"));
    }
    // Close the writing end, which ends the read the reader re-armed.
    drop(writer);
    while !reader.ended() {
        port.wait(&mut completions, None)?;
        for completion in completions.iter() {
            if reader.owns(&completion) {
                reader.complete(&completion, now(), &mut |_| Ok(()))?;
            }
        }
    }
    #[expect(clippy::cast_precision_loss, reason = "a span far below 2^52")]
    let elapsed_ms = arrived.at.saturating_sub(start) as f64 / 10_000.0;
    Ok((arrived.reads, pended, elapsed_ms))
}

/// The size of the frame the drain check's child writes.
const DRAIN_BYTES: usize = 2 * 1024 * 1024;

/// The drain check's child: writes one 2 MB frame on its overlapped pipe
/// and exits only once that write has completed (section 4.6).
fn drainer(out: OwnedHandle, plan: Plan) -> io::Result<()> {
    let sender = Sender::new(out, Writes::Inline, plan.priority)?;
    let mut frame = Vec::new();
    encode(&vec![b'x'; DRAIN_BYTES], &mut frame);
    sender.send(&frame)?;
    sender.finish().map(drop)
}

/// Large frames: reads per frame in each order, and the drain check.
fn run_frames(plan: Plan) -> io::Result<Outcome> {
    let mut notes = Vec::new();
    for size in [1024 * 1024, 2 * 1024 * 1024, 32 * 1024 * 1024] {
        for read_first in [true, false] {
            let (reads, pended, elapsed_ms) = one_large_frame(size, read_first)?;
            notes.push(format!(
                "{} MB frame, {}: {reads} reads; the write {}; whole after {elapsed_ms:.3} ms",
                size / (1024 * 1024),
                if read_first {
                    "the read waiting first"
                } else {
                    "the data arriving first"
                },
                if pended {
                    "pended"
                } else {
                    "completed at once"
                },
            ));
        }
    }
    let child = launch_overlapped("--drainer", plan, End::Overlapped, EVENT_BUFFER)?;
    let (frame, reads) = read_one_overlapped(child.from_child)?;
    drop(child.to_child);
    wait_for_exit(&child.process, "drainer")?;
    let whole = body(&frame)?.len();
    if whole != DRAIN_BYTES {
        return Err(io::Error::other(format!(
            "the drainer's frame arrived cut: {whole} of {DRAIN_BYTES} bytes"
        )));
    }
    notes.push(format!(
        "a child that wrote 2 MB and exited once the write completed: all {whole} bytes, \
         in {reads} reads, exit code 0"
    ));
    Ok(Outcome {
        hops: Vec::new(),
        notes,
    })
}

fn priority_name(priority: ThreadPriority) -> &'static str {
    match priority {
        ThreadPriority::Normal => "normal",
        ThreadPriority::AboveNormal => "above-normal",
        ThreadPriority::Highest => "highest",
        ThreadPriority::TimeCritical => "time-critical",
    }
}

fn parse_priority(text: &str) -> Option<ThreadPriority> {
    [
        ThreadPriority::Normal,
        ThreadPriority::AboveNormal,
        ThreadPriority::Highest,
        ThreadPriority::TimeCritical,
    ]
    .into_iter()
    .find(|priority| priority_name(*priority) == text)
}

fn writes_name(writes: Writes) -> &'static str {
    match writes {
        Writes::Inline => "inline",
        Writes::IoThread => "io-thread",
    }
}

/// The value after `flag` in `args`.
fn value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn number(args: &[String], flag: &str, default: u64) -> Result<u64, String> {
    value(args, flag).map_or(Ok(default), |text| {
        text.parse()
            .map_err(|_| format!("{flag} takes a whole number, not {text}"))
    })
}

fn plan_from(args: &[String]) -> Result<Plan, String> {
    let priority = value(args, "--priority").unwrap_or("highest");
    let writes = match value(args, "--writes").unwrap_or("inline") {
        "inline" => Writes::Inline,
        "io-thread" => Writes::IoThread,
        text => return Err(format!("no way of writing is called {text}")),
    };
    Ok(Plan {
        messages: number(args, "--messages", 20_000)?,
        mean_gap_us: number(args, "--mean-gap-us", 20_000)?,
        seed: number(args, "--seed", now())?,
        priority: parse_priority(priority)
            .ok_or_else(|| format!("no priority is called {priority}"))?,
        burst: number(args, "--burst", 1)?.max(1),
        producers: number(args, "--producers", 1)?.max(1),
        writes,
        burst_us: number(args, "--burst-us", 1_000)?,
        reply_bytes: number(args, "--reply-bytes", 8_192)?,
    })
}

/// A child role's two inherited pipe ends.
fn child_ends(args: &[String]) -> Result<(OwnedHandle, OwnedHandle), String> {
    let pipe_in = number(args, "--pipe-in", 0)?;
    let pipe_out = number(args, "--pipe-out", 0)?;
    // SAFETY: the relay passes the values of the pipe ends it made for this
    // process, which inherited them and uses them nowhere else;
    // `inherited_pipe_handles` checks they are distinct open pipes.
    unsafe {
        verbatim_process::inherited_pipe_handles(
            usize::try_from(pipe_in).map_err(|error| error.to_string())?,
            usize::try_from(pipe_out).map_err(|error| error.to_string())?,
        )
    }
    .map_err(|error| error.to_string())
}

/// Runs a child role.
fn child(role: &str, args: &[String]) -> Result<(), String> {
    let plan = plan_from(args)?;
    let overlapped = args.iter().any(|arg| arg == "--overlapped");
    let (commands, out) = child_ends(args)?;
    let result = match role {
        "--sender" => {
            // The sender reads nothing; its command pipe stays open until it
            // exits.
            let _commands = commands;
            let start = allocations();
            Out::new(out, overlapped, &plan)
                .and_then(|out| produce(&out, plan, start).and_then(|()| out.finish()))
        }
        "--receiver" => receive_all(commands, out, overlapped, plan),
        "--host" => host(commands, out, plan),
        "--drainer" => {
            let _commands = commands;
            drainer(out, plan)
        }
        _ => return Err(format!("no role is called {role}")),
    };
    result.map_err(|error| error.to_string())
}

fn run(args: &[String]) -> Result<(), String> {
    let plan = plan_from(args)?;
    let variants = match value(args, "--variant").unwrap_or("all") {
        "all" => Variant::ALL.to_vec(),
        text => vec![Variant::parse(text).ok_or_else(|| format!("no variant is called {text}"))?],
    };
    let load = match value(args, "--load").unwrap_or("idle") {
        "idle" => Load::Idle,
        "all" => Load::All,
        "all-but-one" => Load::AllButOne,
        text => return Err(format!("no load is called {text}")),
    };
    println!(
        "relay floor: {} frames per variant, mean gap {} us, bursts of {}, {} producers \
         writing {}, seed {}, priority {}, load {load:?}, {} logical processors",
        plan.messages,
        plan.mean_gap_us,
        plan.burst,
        plan.producers,
        writes_name(plan.writes),
        plan.seed,
        priority_name(plan.priority),
        thread::available_parallelism().map_or(1, std::num::NonZero::get),
    );
    for variant in variants {
        let spinners = Spinners::start(load);
        let result = match variant {
            Variant::F0 => run_f0(plan),
            Variant::F1 => run_f1(plan),
            Variant::F2 => run_f2(plan),
            Variant::F3 => run_port_relay(plan, End::Synchronous),
            Variant::F4 => run_f4(plan),
            Variant::F5 => run_port_relay(plan, End::Overlapped),
            Variant::F6 => run_f6(plan),
            Variant::Frames => run_frames(plan),
        };
        drop(spinners);
        let mut outcome = result.map_err(|error| format!("{variant:?}: {error}"))?;
        println!("{variant:?}, {}:", variant.describe());
        for hop in &mut outcome.hops {
            println!("  {}", hop.report());
        }
        for note in &outcome.notes {
            println!("  {note}");
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    if let Err(error) = opt_out_of_power_throttling() {
        eprintln!("relay-floor: power throttling stays as Windows decides: {error}");
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some(role @ ("--sender" | "--receiver" | "--host" | "--drainer")) => child(role, &args),
        _ => Err(
            "usage: verbatim-relay-floor run [--variant f0|f1|f2|f3|f4|f5|f6|frames|all] \
                  [--messages N] [--mean-gap-us N] [--seed N] \
                  [--priority normal|above-normal|highest|time-critical] \
                  [--load idle|all|all-but-one] [--burst N] [--producers N] \
                  [--writes inline|io-thread] [--burst-us N] [--reply-bytes N]"
                .to_owned(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("relay-floor: {error}");
            ExitCode::FAILURE
        }
    }
}
