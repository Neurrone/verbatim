//! `verbatim-trace-waits`: how long threads waited to run, from a WPR trace
//! of the CPU's scheduling (`cargo xtask trace start|stop`), run by `cargo
//! xtask trace waits <trace.etl> [process.exe ...]`.
//!
//! `docs/design/communication.md` (sections 6.1 and 11) asks, for each
//! slow hand-off, whether the thread that should have taken it was ready
//! and not run, and checks that raised thread priorities never keep
//! Verbatim's own GUI thread, or the foreground application's, waiting.
//! This reads the kernel's thread events from the trace with Windows' event
//! consumer, which needs no elevation for a trace file:
//!
//! - a thread becomes ready when a `ReadyThread` event names it, or when a
//!   context switch takes the processor from it while it is still ready
//!   (pre-empted);
//! - it runs when a context switch gives it the processor; the time between
//!   is one ready wait.
//!
//! It prints, for every thread of the processes named (by default
//! `verbatim.exe`, `verbatim-outpost.exe` and `verbatim-synth-host.exe`),
//! its process, id and name, how many ready waits it had, their median,
//! 99.9th percentile and maximum in milliseconds, and how many were over 1,
//! 10 and 100 ms. With `--wakers <thread name>` it reports instead, for
//! the threads of that name, which threads woke them after they had waited
//! blocked, not ready, for half a millisecond or more, and how long they
//! had waited: the waking thread is the one running on the processor that
//! logged the ready event, known from the context switches.
//! Names come from the thread events (`SetThreadDescription`,
//! which Rust's named threads set); a process is named by the `.exe` image
//! the trace saw loaded in it.

use std::collections::HashMap;
use std::process::ExitCode;

use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, EVENT_RECORD, EVENT_TRACE_LOGFILEW, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESSTRACE_HANDLE, ProcessTrace,
};
use windows::core::{GUID, PWSTR};

/// The kernel's thread events (`Thread` in the kernel logger's MOF).
const THREAD: GUID = GUID::from_u128(0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c);

/// The kernel's image load events (`Image`).
const IMAGE: GUID = GUID::from_u128(0x2cb15d1d_5fc1_11d2_abe1_00a0c911f518);

/// Opcodes of the thread events used.
const THREAD_START: u8 = 1;
const THREAD_DC_START: u8 = 3;
const CSWITCH: u8 = 36;
const READY_THREAD: u8 = 50;
const THREAD_SET_NAME: u8 = 72;

/// Opcodes of the image events used: a load, and one of the images
/// already loaded when the trace began.
const IMAGE_LOAD: u8 = 10;
const IMAGE_DC_START: u8 = 3;

/// A context switch's `OldThreadState` for a thread still ready.
const STATE_READY: i8 = 1;

/// Everything read from the trace.
#[derive(Default)]
struct Waits {
    /// When each ready thread became ready, in 100-nanosecond units.
    ready_since: HashMap<u32, i64>,
    /// Each thread's ready waits, in 100-nanosecond units.
    by_thread: HashMap<u32, Vec<i64>>,
    /// Each thread's process.
    process_of: HashMap<u32, u32>,
    /// Each thread's name, when it has one.
    names: HashMap<u32, String>,
    /// Each process's executable's file name, lowercased.
    images: HashMap<u32, String>,
    /// When each blocked thread was switched out, in 100-nanosecond units.
    blocked_since: HashMap<u32, i64>,
    /// Each blocked wait of half a millisecond or more: the thread, the
    /// thread that readied it, and how long it was blocked.
    wakes: Vec<(u32, u32, i64)>,
    /// The thread running on each processor, by its index, from the
    /// context switches: the thread that readies another, since a ready
    /// event does not name it.
    running: HashMap<u16, u32>,
}

/// The shortest blocked wait recorded, in 100-nanosecond units.
const SHORTEST_BLOCKED: i64 = 5_000;

impl Waits {
    fn event(&mut self, record: &EVENT_RECORD) {
        let header = &record.EventHeader;
        let opcode = header.EventDescriptor.Opcode;
        let time = header.TimeStamp;
        let data = user_data(record);
        // SAFETY: both fields of the union are plain integers; the index is
        // the whole of it.
        let processor = unsafe { record.BufferContext.Anonymous.ProcessorIndex };
        if header.ProviderId == THREAD {
            match opcode {
                CSWITCH => {
                    if let Some(new) = u32_at(data, 0) {
                        self.running.insert(processor, new);
                    }
                    self.cswitch(data, time);
                }
                READY_THREAD => {
                    if let Some(thread) = u32_at(data, 0) {
                        self.ready_since.entry(thread).or_insert(time);
                        if let Some(since) = self.blocked_since.remove(&thread)
                            && time - since >= SHORTEST_BLOCKED
                        {
                            let waker = self.running.get(&processor).copied().unwrap_or(0);
                            self.wakes.push((thread, waker, time - since));
                        }
                    }
                }
                THREAD_START | THREAD_DC_START => self.thread(data),
                THREAD_SET_NAME => {
                    if let (Some(process), Some(thread)) = (u32_at(data, 0), u32_at(data, 4)) {
                        self.process_of.insert(thread, process);
                        if let Some(name) = wide_at(data, 8) {
                            self.names.insert(thread, name);
                        }
                    }
                }
                _ => {}
            }
        } else if header.ProviderId == IMAGE && matches!(opcode, IMAGE_LOAD | IMAGE_DC_START) {
            self.image(data);
        }
    }

    fn cswitch(&mut self, data: &[u8], time: i64) {
        let (Some(new), Some(old)) = (u32_at(data, 0), u32_at(data, 4)) else {
            return;
        };
        if let Some(since) = self.ready_since.remove(&new) {
            self.by_thread.entry(new).or_default().push(time - since);
        }
        // Taken off the processor while still ready: pre-empted, so it
        // waits from now; otherwise it blocks until something readies it.
        if old != 0 {
            if data
                .get(14)
                .is_some_and(|&state| state.cast_signed() == STATE_READY)
            {
                self.ready_since.entry(old).or_insert(time);
            } else {
                self.blocked_since.insert(old, time);
            }
        }
    }

    /// A thread's start, or a thread running when the trace began: its
    /// process, and its name at offset 72 (after the process and thread
    /// ids, seven pointers, the sub-process tag and four priority bytes).
    fn thread(&mut self, data: &[u8]) {
        let (Some(process), Some(thread)) = (u32_at(data, 0), u32_at(data, 4)) else {
            return;
        };
        self.process_of.insert(thread, process);
        if let Some(name) = wide_at(data, 72).filter(|name| !name.is_empty()) {
            self.names.insert(thread, name);
        }
    }

    /// An image loaded in a process: the process is named by its `.exe`
    /// (the process id at offset 16, the file name at 56).
    fn image(&mut self, data: &[u8]) {
        let (Some(process), Some(path)) = (u32_at(data, 16), wide_at(data, 56)) else {
            return;
        };
        let file = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
        if std::path::Path::new(&file)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            self.images.entry(process).or_insert(file);
        }
    }
}

fn user_data(record: &EVENT_RECORD) -> &[u8] {
    if record.UserData.is_null() {
        return &[];
    }
    // SAFETY: the consumer gives the event's payload, UserDataLength bytes
    // at UserData, valid for the callback's duration, which this borrow of
    // the record does not outlive.
    unsafe {
        std::slice::from_raw_parts(
            record.UserData.cast::<u8>(),
            usize::from(record.UserDataLength),
        )
    }
}

fn u32_at(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

/// The NUL-terminated UTF-16 string at `offset`, or to the end of the data.
fn wide_at(data: &[u8], offset: usize) -> Option<String> {
    let units: Vec<u16> = data
        .get(offset..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: the consumer passes a valid record for the callback's
    // duration, whose UserContext is the `Waits` given to OpenTraceW, which
    // lives until ProcessTrace returns and is used on this thread only.
    let record = unsafe { &*record };
    // SAFETY: as above.
    let waits = unsafe { &mut *record.UserContext.cast::<Waits>() };
    waits.event(record);
}

fn read(path: &str) -> Result<Waits, String> {
    let mut waits = Waits::default();
    let mut name: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut logfile = EVENT_TRACE_LOGFILEW {
        LogFileName: PWSTR(name.as_mut_ptr()),
        Context: (&raw mut waits).cast(),
        ..EVENT_TRACE_LOGFILEW::default()
    };
    logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_EVENT_RECORD;
    logfile.Anonymous2.EventRecordCallback = Some(on_event);
    // SAFETY: `logfile` and the name it points to outlive the trace's
    // processing below; the handle is closed once.
    let handle: PROCESSTRACE_HANDLE = unsafe { OpenTraceW(&raw mut logfile) };
    if handle.Value == u64::MAX {
        return Err(format!(
            "could not open {path}: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: a handle OpenTraceW just returned; the callback runs on this
    // thread, before ProcessTrace returns.
    let processed = unsafe { ProcessTrace(&[handle], None, None) };
    // SAFETY: as above, closed once.
    let _ = unsafe { CloseTrace(handle) };
    if processed.is_err() {
        return Err(format!("could not read {path}: error {}", processed.0));
    }
    Ok(waits)
}

fn report(waits: &Waits, processes: &[String]) {
    let mut rows: Vec<(String, u32, String, Vec<i64>)> = waits
        .by_thread
        .iter()
        .filter_map(|(&thread, list)| {
            let process = *waits.process_of.get(&thread)?;
            let image = waits.images.get(&process)?;
            processes.contains(image).then(|| {
                let mut sorted = list.clone();
                sorted.sort_unstable();
                let name = waits.names.get(&thread).cloned().unwrap_or_default();
                (format!("{image} {process}"), thread, name, sorted)
            })
        })
        .collect();
    rows.sort_by(|a, b| (&a.0, &a.2, a.1).cmp(&(&b.0, &b.2, b.1)));
    // Milliseconds to the microsecond, from 100-nanosecond units.
    let ms = |units: i64| format!("{}.{:03}", units / 10_000, units % 10_000 / 10);
    for (process, thread, name, sorted) in rows {
        let at = |permille: usize| sorted[(sorted.len() - 1) * permille / 1_000];
        let over = |limit_ms: i64| {
            sorted
                .iter()
                .filter(|&&wait| wait > limit_ms * 10_000)
                .count()
        };
        println!(
            "{process} thread {thread} {name:?}: {} waits; median {} ms, 99.9th {} ms, maximum {} ms; over 1 ms {}, over 10 ms {}, over 100 ms {}",
            sorted.len(),
            ms(at(500)),
            ms(at(999)),
            ms(*sorted.last().unwrap_or(&0)),
            over(1),
            over(10),
            over(100),
        );
    }
}

/// Which threads woke the threads named `target` after a blocked wait,
/// grouped by the waking thread, with how many waits fell in each range.
fn report_wakers(waits: &Waits, target: &str) {
    let label = |thread: u32| {
        let process = waits.process_of.get(&thread).copied().unwrap_or(0);
        let image = waits.images.get(&process).map_or("?", String::as_str);
        let name = waits.names.get(&thread).map_or("", String::as_str);
        format!("{image} {process} thread {thread} {name:?}")
    };
    let mut by_waker: HashMap<String, Vec<i64>> = HashMap::new();
    for &(thread, waker, blocked) in &waits.wakes {
        if waits.names.get(&thread).is_some_and(|name| name == target) {
            by_waker.entry(label(waker)).or_default().push(blocked);
        }
    }
    let mut rows: Vec<_> = by_waker.into_iter().collect();
    rows.sort_by_key(|(_, list)| std::cmp::Reverse(list.len()));
    for (waker, list) in rows {
        let within = |from_ms: i64, to_ms: i64| {
            list.iter()
                .filter(|&&wait| wait >= from_ms * 10_000 && wait < to_ms * 10_000)
                .count()
        };
        println!(
            "{target:?} woken by {waker}: {} waits; under 1 ms {}, 1 to 2 ms {}, 2 to 5 ms {}, 5 to 20 ms {}, 20 ms or more {}",
            list.len(),
            list.iter().filter(|&&wait| wait < 10_000).count(),
            within(1, 2),
            within(2, 5),
            within(5, 20),
            list.iter().filter(|&&wait| wait >= 200_000).count(),
        );
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!(
            "usage: verbatim-trace-waits <trace.etl> [--wakers <thread name>] [process.exe ...]"
        );
        return ExitCode::from(2);
    };
    let mut rest = &args[1..];
    let mut wakers = None;
    if rest.first().is_some_and(|arg| arg == "--wakers") {
        wakers = rest.get(1).cloned();
        rest = rest.get(2..).unwrap_or(&[]);
    }
    let mut processes: Vec<String> = rest.iter().map(|name| name.to_lowercase()).collect();
    if processes.is_empty() {
        processes = [
            "verbatim.exe",
            "verbatim-outpost.exe",
            "verbatim-synth-host.exe",
        ]
        .map(str::to_owned)
        .to_vec();
    }
    match read(path) {
        Ok(waits) => {
            match &wakers {
                Some(target) => report_wakers(&waits, target),
                None => report(&waits, &processes),
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("verbatim-trace-waits: {message}");
            ExitCode::FAILURE
        }
    }
}
