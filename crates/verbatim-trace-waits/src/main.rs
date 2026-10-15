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
//! its process, id and name, how many times it was woken (readied from a
//! wait), its time on a processor (from a context switch giving it one to
//! the next taking it away, so a thread already running when the trace
//! began is counted from its first switch), how many ready waits it had, their median,
//! 99.9th percentile and maximum in milliseconds, and how many were over 1,
//! 10 and 100 ms. With `--wakers <thread name>` (or
//! `<process.exe>:<thread name>`, as for `--stacks` below) it reports
//! instead, for the threads it names, which threads woke them after they had waited
//! blocked, not ready, for half a millisecond or more, and how long they
//! had waited: the waking thread is the one running on the processor that
//! logged the ready event, known from the context switches.
//! With `--stacks <thread name>` (or `<process.exe>:<thread name>` for
//! that process's threads of that name, an empty name for those without one,
//! or `<process.exe>:#<thread id>` for one thread by its id)
//! it reports the stacks the trace recorded
//! where those threads were readied, each frame named as its function (as
//! `symbols.rs` reads it) or else as a module and offset, the most
//! frequent first: the code that woke them,
//! which names the cause even when the readying thread is an interrupt's.
//! `--symbol-server` fetches system images' PDBs from Microsoft's public
//! symbol server for both reports (`server.rs`).
//! With `--samples <thread name>` (named the same way) it reports the CPU
//! samples the trace took of those threads (the CPU profile samples every
//! processor each millisecond): how many, and for each module how many
//! were running in it and how many had it anywhere on their stack, which
//! says where their processor time went.
//! Names come from the thread events (`SetThreadDescription`,
//! which Rust's named threads set); a process is named by the `.exe` image
//! the trace saw loaded in it.

mod server;
mod symbols;

use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, EVENT_RECORD, EVENT_TRACE_LOGFILEW, OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD,
    PROCESSTRACE_HANDLE, ProcessTrace,
};
use windows::core::{GUID, PWSTR};

/// The kernel's thread events (`Thread` in the kernel logger's MOF).
const THREAD: GUID = GUID::from_u128(0x3d6fa8d1_fe05_11d0_9dda_00c04fd7ba7c);

/// The kernel's stack walk events, each the stack of the event logged just
/// before it on the same processor.
const STACK_WALK: GUID = GUID::from_u128(0xdef2fe46_7bd6_4b80_bd94_f57fe20d0ce3);

/// The stack walk event's opcode.
const STACK: u8 = 32;

/// How many frames of a stack are reported.
const FRAMES: usize = 32;

/// The kernel's profiling events (`PerfInfo`), and the opcode of a CPU
/// sample: the instruction pointer at offset 0 and the thread at 8.
const PERF_INFO: GUID = GUID::from_u128(0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc);
const SAMPLED_PROFILE: u8 = 46;

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
    /// How many times each thread was readied from a wait: its wake-ups.
    woken: HashMap<u32, usize>,
    /// When each running thread was given its processor, and each thread's
    /// time on a processor, in 100-nanosecond units.
    on_cpu_since: HashMap<u32, i64>,
    cpu: HashMap<u32, i64>,
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
    /// The thread name whose readying stacks are collected, if any.
    stack_target: Option<String>,
    /// Processors whose next stack walk is a collected readying's.
    stack_pending: HashSet<u16>,
    /// The readying stacks collected, each as its process and return
    /// addresses, innermost first, and how many times each.
    stacks: HashMap<(u32, Vec<u64>), usize>,
    /// Each process's loaded images: base, size and file name; the
    /// kernel's under process 0.
    modules: HashMap<u32, Vec<(u64, u64, String)>>,
    /// The thread name whose CPU samples are collected, if any.
    sample_target: Option<String>,
    /// Sampled threads whose next stack walk is their sample's.
    sample_pending: HashSet<u32>,
    /// How many samples were taken, and in which module each was running.
    samples: usize,
    sampled_in: HashMap<String, usize>,
    /// How many samples had each module anywhere on their stack.
    sampled_under: HashMap<String, usize>,
    /// Each collected sample's stack: its process and return addresses,
    /// innermost first.
    sample_stacks: Vec<(u32, Vec<u64>)>,
    /// Each image's path on its device, by its process (0 for the
    /// kernel's) and base.
    paths: HashMap<(u32, u64), String>,
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
                        if self
                            .stack_target
                            .as_ref()
                            .is_some_and(|target| self.targets(thread, target))
                        {
                            self.stack_pending.insert(processor);
                        }
                        self.ready_since.entry(thread).or_insert(time);
                        *self.woken.entry(thread).or_default() += 1;
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
        } else if header.ProviderId == PERF_INFO && opcode == SAMPLED_PROFILE {
            self.sample(data);
        } else if header.ProviderId == STACK_WALK && opcode == STACK {
            if u32_at(data, 12).is_some_and(|thread| self.sample_pending.remove(&thread)) {
                self.sample_stack(data);
            } else if self.stack_pending.remove(&processor) {
                self.stack(data);
            }
        }
    }

    fn cswitch(&mut self, data: &[u8], time: i64) {
        let (Some(new), Some(old)) = (u32_at(data, 0), u32_at(data, 4)) else {
            return;
        };
        if let Some(since) = self.ready_since.remove(&new) {
            self.by_thread.entry(new).or_default().push(time - since);
        }
        if new != 0 {
            self.on_cpu_since.insert(new, time);
        }
        if let Some(since) = self.on_cpu_since.remove(&old) {
            *self.cpu.entry(old).or_default() += time - since;
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
        if let (Some(base), Some(size)) = (u64_at(data, 0), u64_at(data, 8)) {
            self.paths.insert((process, base), path.clone());
            self.modules
                .entry(process)
                .or_default()
                .push((base, size, file.clone()));
        }
        if std::path::Path::new(&file)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            self.images.entry(process).or_insert(file);
        }
    }
}

impl Waits {
    /// Whether `thread` is one `target` names: a thread of that name in
    /// any process, or, for `<process.exe>:<name>`, one of that name in a
    /// process of that image, an empty name meaning a thread with none,
    /// and `<process.exe>:#<thread id>` the thread of that id.
    fn targets(&self, thread: u32, target: &str) -> bool {
        let name = self.names.get(&thread);
        match target.split_once(':') {
            Some((image, wanted)) => {
                let in_image = self
                    .process_of
                    .get(&thread)
                    .and_then(|process| self.images.get(process))
                    .is_some_and(|known| known.eq_ignore_ascii_case(image));
                in_image
                    && match (wanted.strip_prefix('#'), name) {
                        (Some(id), _) => id.parse() == Ok(thread),
                        (None, Some(name)) => name == wanted,
                        (None, None) => wanted.is_empty(),
                    }
            }
            None => name.is_some_and(|name| name == target),
        }
    }

    /// A readying's stack: the process and thread it was taken in at
    /// offsets 8 and 12, then the return addresses from 16, named when
    /// the stacks are reported.
    fn stack(&mut self, data: &[u8]) {
        let process = u32_at(data, 8).unwrap_or(0);
        let addresses: Vec<u64> = (16..data.len())
            .step_by(8)
            .filter_map(|offset| u64_at(data, offset))
            .take(FRAMES)
            .collect();
        *self.stacks.entry((process, addresses)).or_default() += 1;
    }

    /// A CPU sample: counted, with the module it was running in, when its
    /// thread is the one asked for; its stack follows.
    fn sample(&mut self, data: &[u8]) {
        let (Some(address), Some(thread)) = (u64_at(data, 0), u32_at(data, 8)) else {
            return;
        };
        if !self
            .sample_target
            .as_ref()
            .is_some_and(|target| self.targets(thread, target))
        {
            return;
        }
        let process = self.process_of.get(&thread).copied().unwrap_or(0);
        let module = self.module(process, address);
        self.samples += 1;
        *self.sampled_in.entry(module).or_default() += 1;
        self.sample_pending.insert(thread);
    }

    /// A collected sample's stack: each module on it counted once.
    fn sample_stack(&mut self, data: &[u8]) {
        let process = u32_at(data, 8).unwrap_or(0);
        let addresses: Vec<u64> = (16..data.len())
            .step_by(8)
            .filter_map(|offset| u64_at(data, offset))
            .collect();
        let modules: HashSet<String> = addresses
            .iter()
            .map(|&address| self.module(process, address))
            .collect();
        self.sample_stacks.push((process, addresses));
        for module in modules {
            *self.sampled_under.entry(module).or_default() += 1;
        }
    }

    /// The image holding `address`, in `process` or the kernel.
    fn image_at(&self, process: u32, address: u64) -> Option<&(u64, u64, String)> {
        self.owned_image_at(process, address)
            .map(|(_, image)| image)
    }

    /// The image holding `address`, with the process it was loaded in:
    /// `process`, or 0 for the kernel's.
    fn owned_image_at(&self, process: u32, address: u64) -> Option<(u32, &(u64, u64, String))> {
        [process, 0].into_iter().find_map(|owner| {
            self.modules.get(&owner)?.iter().find_map(|image| {
                let (base, size, _) = image;
                (address >= *base && address < base + size).then_some((owner, image))
            })
        })
    }

    /// `address` named as the function holding it, `module!function`,
    /// when its image has symbols or exports; otherwise module and offset.
    fn function(&self, symbols: &mut symbols::Symbols, process: u32, address: u64) -> String {
        let Some((owner, (base, size, name))) = self.owned_image_at(process, address) else {
            return format!("{address:#x}");
        };
        let named = self.paths.get(&(owner, *base)).and_then(|path| {
            let image = symbols::Image {
                base: *base,
                size: *size,
                path,
            };
            symbols.name(owner, address, &image)
        });
        named.map_or_else(
            || format!("{name}+{:#x}", address - base),
            |function| format!("{name}!{function}"),
        )
    }

    /// The name of the module holding `address`, or "?" when none does.
    fn module(&self, process: u32, address: u64) -> String {
        self.image_at(process, address)
            .map_or_else(|| "?".to_owned(), |(_, _, name)| name.clone())
    }
}

fn u64_at(data: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        data.get(offset..offset + 8)?.try_into().ok()?,
    ))
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

fn read(
    path: &str,
    stack_target: Option<String>,
    sample_target: Option<String>,
) -> Result<Waits, String> {
    let mut waits = Waits {
        stack_target,
        sample_target,
        ..Waits::default()
    };
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
            "{process} thread {thread} {name:?}: woken {}, on a processor {} ms; {} waits; median {} ms, 99.9th {} ms, maximum {} ms; over 1 ms {}, over 10 ms {}, over 100 ms {}",
            waits.woken.get(&thread).copied().unwrap_or(0),
            ms(waits.cpu.get(&thread).copied().unwrap_or(0)),
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

/// The blocked waits of the threads `target` names (as [`Waits::targets`]
/// reads it), grouped by the thread that woke them, each labelled by its
/// process image, process, thread id and name.
fn woken_by(waits: &Waits, target: &str) -> HashMap<String, Vec<i64>> {
    let label = |thread: u32| {
        let process = waits.process_of.get(&thread).copied().unwrap_or(0);
        let image = waits.images.get(&process).map_or("?", String::as_str);
        let name = waits.names.get(&thread).map_or("", String::as_str);
        format!("{image} {process} thread {thread} {name:?}")
    };
    let mut by_waker: HashMap<String, Vec<i64>> = HashMap::new();
    for &(thread, waker, blocked) in &waits.wakes {
        if waits.targets(thread, target) {
            by_waker.entry(label(waker)).or_default().push(blocked);
        }
    }
    by_waker
}

/// Which threads woke the threads `target` names after a blocked wait,
/// grouped by the waking thread, with how many waits fell in each range.
fn report_wakers(waits: &Waits, target: &str) {
    let mut rows: Vec<_> = woken_by(waits, target).into_iter().collect();
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

/// The CPU samples collected: how many, then each module by how many were
/// running in it, and by how many had it anywhere on their stack.
fn report_samples(waits: &Waits, target: &str, mut symbols: symbols::Symbols) {
    println!("{target:?}: {} samples", waits.samples);
    for (title, counts) in [
        ("running in", &waits.sampled_in),
        ("on the stack", &waits.sampled_under),
    ] {
        let mut rows: Vec<_> = counts.iter().collect();
        rows.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (module, count) in rows.into_iter().take(15) {
            println!("{title} {module}: {count}");
        }
    }
    // Where the time went by function: the function each sample was in,
    // then each function anywhere on its stack, counted once a sample,
    // Verbatim's own first.
    let mut running_in: HashMap<String, usize> = HashMap::new();
    let mut under: HashMap<String, usize> = HashMap::new();
    let mut calling_out: HashMap<String, usize> = HashMap::new();
    for (process, addresses) in &waits.sample_stacks {
        let functions: Vec<String> = addresses
            .iter()
            .map(|&address| waits.function(&mut symbols, *process, address))
            .collect();
        if let Some(innermost) = functions.first() {
            *running_in.entry(innermost.clone()).or_default() += 1;
        }
        if let Some(ours) = functions.iter().find(|name| name.starts_with("verbatim")) {
            *calling_out.entry(ours.clone()).or_default() += 1;
        }
        let distinct: HashSet<&String> = functions.iter().collect();
        for function in distinct {
            *under.entry(function.clone()).or_default() += 1;
        }
    }
    let ranked = |counts: &HashMap<String, usize>, keep: &dyn Fn(&str) -> bool, title: &str| {
        let mut rows: Vec<_> = counts.iter().filter(|(name, _)| keep(name)).collect();
        rows.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (function, count) in rows.into_iter().take(30) {
            println!("{title} {function}: {count}");
        }
    };
    ranked(&running_in, &|_| true, "running in function");
    ranked(&calling_out, &|_| true, "Verbatim's innermost");
    ranked(
        &under,
        &|name| name.starts_with("verbatim"),
        "Verbatim's on the stack",
    );
    ranked(
        &under,
        &|name| {
            !name.starts_with("verbatim") && !name.contains(".sys") && !name.starts_with("ntoskrnl")
        },
        "others' on the stack",
    );
}

/// The threads of the processes named, totalled by process image and
/// thread name (threads of one name, such as each outpost's worker, summed
/// across processes): how many times they were woken and their time on a
/// processor, the most woken first, after the totals for each image and
/// for all of them.
fn report_by_name(waits: &Waits, processes: &[String]) {
    let mut by_name: HashMap<(String, String), (usize, i64)> = HashMap::new();
    let threads: HashSet<u32> = waits
        .woken
        .keys()
        .chain(waits.cpu.keys())
        .copied()
        .collect();
    for thread in threads {
        let Some(image) = waits
            .process_of
            .get(&thread)
            .and_then(|process| waits.images.get(process))
        else {
            continue;
        };
        if !processes.contains(image) {
            continue;
        }
        let name = waits
            .names
            .get(&thread)
            .cloned()
            .unwrap_or_else(|| "(unnamed)".to_owned());
        let entry = by_name.entry((image.clone(), name)).or_default();
        entry.0 += waits.woken.get(&thread).copied().unwrap_or(0);
        entry.1 += waits.cpu.get(&thread).copied().unwrap_or(0);
    }
    let ms = |units: i64| format!("{}.{}", units / 10_000, units % 10_000 / 1_000);
    let mut by_image: HashMap<&str, (usize, i64)> = HashMap::new();
    for ((image, _), (woken, cpu)) in &by_name {
        let entry = by_image.entry(image).or_default();
        entry.0 += woken;
        entry.1 += cpu;
    }
    let all = by_image.values().fold((0, 0), |(woken, cpu), entry| {
        (woken + entry.0, cpu + entry.1)
    });
    println!("all: woken {}, on a processor {} ms", all.0, ms(all.1));
    let mut images: Vec<_> = by_image.into_iter().collect();
    images.sort_unstable();
    for (image, (woken, cpu)) in images {
        println!("{image}: woken {woken}, on a processor {} ms", ms(cpu));
    }
    let mut rows: Vec<_> = by_name.into_iter().collect();
    rows.sort_by_key(|(_, (woken, _))| std::cmp::Reverse(*woken));
    for ((image, name), (woken, cpu)) in rows {
        println!(
            "{image} {name:?}: woken {woken}, on a processor {} ms",
            ms(cpu)
        );
    }
}

/// The readying stacks collected, each frame named as the function holding
/// it, the most frequent first, the top ten.
fn report_stacks(waits: &Waits, mut symbols: symbols::Symbols) {
    let mut named: HashMap<Vec<String>, usize> = HashMap::new();
    for ((process, addresses), count) in &waits.stacks {
        let image = waits.images.get(process).map_or("?", String::as_str);
        let frames: Vec<String> = std::iter::once(format!("in {image} {process}"))
            .chain(
                addresses
                    .iter()
                    .map(|&address| waits.function(&mut symbols, *process, address)),
            )
            .collect();
        *named.entry(frames).or_default() += count;
    }
    let mut rows: Vec<_> = named.iter().collect();
    rows.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (frames, count) in rows.into_iter().take(10) {
        println!("{count} times:");
        for frame in frames {
            println!("    {frame}");
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--symbols <folder>`, anywhere and any number of times: folders
    // searched for every image's PDB.
    let mut symbol_folders = Vec::new();
    while let Some(at) = args.iter().position(|arg| arg == "--symbols") {
        args.remove(at);
        if at < args.len() {
            symbol_folders.push(args.remove(at));
        }
    }
    // `--symbol-server`, and `--symbol-cache <folder>` for where its PDBs
    // are kept, by default `symbols` beside the build's own folder
    // (`target\symbols`).
    let symbol_server = args.iter().position(|arg| arg == "--symbol-server");
    if let Some(at) = symbol_server {
        args.remove(at);
    }
    let mut symbol_cache = None;
    if let Some(at) = args.iter().position(|arg| arg == "--symbol-cache") {
        args.remove(at);
        if at < args.len() {
            symbol_cache = Some(std::path::PathBuf::from(args.remove(at)));
        }
    }
    let Some(path) = args.first() else {
        eprintln!(
            "usage: verbatim-trace-waits <trace.etl> [--by-name | --wakers <thread> | --stacks <thread> | --samples <thread>] [--symbols <folder>] [--symbol-server [--symbol-cache <folder>]] [process.exe ...], a <thread> being a thread name, <process.exe>:<thread name> or <process.exe>:#<thread id>"
        );
        return ExitCode::from(2);
    };
    let mut rest = &args[1..];
    let mut wakers = None;
    let mut stacks = None;
    let mut samples = None;
    let mut by_name = false;
    match rest.first().map(String::as_str) {
        Some("--wakers") => {
            wakers = rest.get(1).cloned();
            rest = rest.get(2..).unwrap_or(&[]);
        }
        Some("--stacks") => {
            stacks = rest.get(1).cloned();
            rest = rest.get(2..).unwrap_or(&[]);
        }
        Some("--samples") => {
            samples = rest.get(1).cloned();
            rest = rest.get(2..).unwrap_or(&[]);
        }
        Some("--by-name") => {
            by_name = true;
            rest = rest.get(1..).unwrap_or(&[]);
        }
        _ => {}
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
    let server = symbol_server.map(|_| {
        server::Server::new(symbol_cache.unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|exe| Some(exe.parent()?.parent()?.join("symbols")))
                .unwrap_or_else(|| "symbols".into())
        }))
    });
    let symbols = || symbols::Symbols::new(symbol_folders.clone(), server);
    match read(path, stacks.clone(), samples.clone()) {
        Ok(waits) => {
            match (&wakers, &stacks, &samples) {
                (Some(target), _, _) => report_wakers(&waits, target),
                (None, Some(_), _) => report_stacks(&waits, symbols()),
                (None, None, Some(target)) => report_samples(&waits, target, symbols()),
                (None, None, None) if by_name => report_by_name(&waits, &processes),
                (None, None, None) => report(&waits, &processes),
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("verbatim-trace-waits: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--wakers verbatim-outpost.exe:<name>` takes that process's threads
    /// of that name, and an empty name its unnamed threads, as
    /// `docs/tooling/measuring.md` ("Measuring performance") documents.
    #[test]
    fn a_process_waker_selector_takes_that_process_named_and_unnamed_threads() {
        let mut waits = Waits::default();
        waits.images.insert(10, "verbatim-outpost.exe".to_owned());
        waits.images.insert(20, "verbatim.exe".to_owned());
        // Thread 1 is the outpost's worker and thread 2 an unnamed outpost
        // thread; thread 3 is a worker of verbatim.exe, and thread 4 the
        // waker of them all.
        for (thread, process) in [(1, 10), (2, 10), (3, 20), (4, 20)] {
            waits.process_of.insert(thread, process);
        }
        waits.names.insert(1, "verbatim-worker".to_owned());
        waits.names.insert(3, "verbatim-worker".to_owned());
        waits.names.insert(4, "main".to_owned());
        waits.wakes = vec![(1, 4, 10_000), (2, 4, 20_000), (3, 4, 30_000)];
        let waker = r#"verbatim.exe 20 thread 4 "main""#.to_owned();

        assert_eq!(
            woken_by(&waits, "verbatim-outpost.exe:verbatim-worker"),
            HashMap::from([(waker.clone(), vec![10_000])])
        );
        assert_eq!(
            woken_by(&waits, "verbatim-outpost.exe:"),
            HashMap::from([(waker, vec![20_000])])
        );
    }

    /// `<process.exe>:#<thread id>` takes the one thread of that id in a
    /// process of that image, unnamed threads included, as
    /// `docs/tooling/measuring.md` ("Measuring performance") documents.
    #[test]
    fn a_thread_id_selector_takes_that_thread_alone() {
        let mut waits = Waits::default();
        waits.images.insert(10, "verbatim-outpost.exe".to_owned());
        waits.images.insert(20, "verbatim.exe".to_owned());
        // Threads 1 and 2 are unnamed outpost threads, 3 a named one of
        // verbatim.exe.
        for (thread, process) in [(1, 10), (2, 10), (3, 20)] {
            waits.process_of.insert(thread, process);
        }
        waits.names.insert(3, "main".to_owned());
        let taken = |target: &str| -> Vec<u32> {
            [1, 2, 3]
                .into_iter()
                .filter(|&thread| waits.targets(thread, target))
                .collect()
        };

        assert_eq!(taken("verbatim-outpost.exe:#2"), vec![2]);
        assert_eq!(taken("verbatim.exe:#3"), vec![3]);
        assert_eq!(taken("verbatim.exe:#2"), Vec::<u32>::new());
    }
}
