//! [`Request::RunnerLoad`](crate::protocol::Request::RunnerLoad): the
//! machine's load when a scenario fails, so a window that stopped
//! answering on a stalled runner is reported with what else the machine
//! was doing: processor and memory use, the disks' queue, the processes
//! that spent the most processor time over a short interval, the
//! Desktop Window Manager, which draws every window, and the running
//! services with the process each runs in, which name a busy `svchost.exe`.
//!
//! Every part is read without a window or UI call, and without opening any
//! process: the processes' times, working sets and thread counts come from
//! one system-wide query (`NtQuerySystemInformation`), the system's from
//! `GetSystemTimes` and `GlobalMemoryStatusEx`, the disks' queue from
//! the performance data helper, and the services from the service control
//! manager's list. So a frozen desktop, or a process that
//! cannot be read, holds none of it up. The parts are read on threads of
//! their own and the answer is given at its time limit with what has been
//! read, each part not read saying so.
//!
//! The interval is the span the processor and disk figures are measured
//! over, not a wait for evidence: nothing here waits for an application.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows::Win32::Foundation::ERROR_MORE_DATA;
use windows::Win32::Foundation::{FILETIME, STATUS_INFO_LENGTH_MISMATCH};
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW,
    PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::Services::{
    CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW, OpenSCManagerW,
    SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE, SERVICE_WIN32,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;
use windows::core::{PCWSTR, w};

use crate::pace::Pacer;
use crate::protocol::{
    DiskQueue, MemoryLoad, ProcessLoad, ProcessorLoad, RunnerLoad, RunningService,
};

/// How many of the processes that spent the most processor time are
/// listed.
pub const TOP_PROCESSES: usize = 12;

/// The largest buffer the process list is read into, in bytes: far above
/// any machine's list, so a size that keeps growing ends as an error.
const MAX_PROCESS_LIST_BYTES: usize = 64 << 20;

/// One part of the load, sent by the thread that read it.
enum Part {
    Memory(Result<MemoryLoad, String>),
    Processor(Result<ProcessorLoad, String>),
    Disk(Result<DiskQueue, String>),
    Services(Result<Vec<RunningService>, String>),
}

/// The machine's load: memory and the running services at once, and the
/// processor and disk figures over `interval`, the services and the disk
/// each read on a thread of their own. Answers
/// within `timeout` with what has been read; a part not read by then says
/// so, and its thread is left to finish alone, its answer dropped.
#[must_use]
pub fn runner_load(interval: Duration, timeout: Duration) -> RunnerLoad {
    let started = Instant::now();
    let deadline = started + timeout;
    let not_read = |what: &str| format!("{what} was not read within {timeout:?}");
    let mut load = RunnerLoad {
        memory: Err(not_read("the memory")),
        processor: Err(not_read("the processor use")),
        disk: Err(not_read("the disks' queue")),
        services: Err(not_read("the services' list")),
        elapsed_ms: 0,
    };
    let (sender, receiver) = mpsc::channel();
    let processor_sender = sender.clone();
    let services_sender = sender.clone();
    // On a thread of its own, since the service control manager answers
    // through its own process, which a stalled machine can hold up.
    if let Err(error) = thread::Builder::new()
        .name("runner-services".to_owned())
        .spawn(move || {
            let _ = services_sender.send(Part::Services(services()));
        })
    {
        load.services = Err(format!(
            "the thread reading it could not be started: {error}"
        ));
    }
    if let Err(error) = thread::Builder::new()
        .name("runner-load".to_owned())
        .spawn(move || {
            // The receiver is gone when an answer came too late.
            let _ = processor_sender.send(Part::Memory(memory()));
            let _ = processor_sender.send(Part::Processor(processor(interval)));
        })
    {
        let why = format!("the thread reading it could not be started: {error}");
        load.memory = Err(why.clone());
        load.processor = Err(why);
    }
    if let Err(error) = thread::Builder::new()
        .name("runner-disk".to_owned())
        .spawn(move || {
            let _ = sender.send(Part::Disk(disk_queue(interval)));
        })
    {
        load.disk = Err(format!(
            "the thread reading it could not be started: {error}"
        ));
    }
    // Ends when every thread has sent all it read and gone, or at the
    // deadline.
    while let Ok(part) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match part {
            Part::Memory(memory) => load.memory = memory,
            Part::Processor(processor) => load.processor = processor,
            Part::Disk(disk) => load.disk = disk,
            Part::Services(services) => load.services = services,
        }
    }
    load.elapsed_ms = milliseconds(started.elapsed());
    load
}

/// `duration` in whole milliseconds.
fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The physical memory and commit in use.
fn memory() -> Result<MemoryLoad, String> {
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(size_of::<MEMORYSTATUSEX>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    // SAFETY: `status` is a local with its length set, as the call needs.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }
        .map_err(|error| format!("GlobalMemoryStatusEx failed: {error}"))?;
    let megabytes = |bytes: u64| bytes >> 20;
    Ok(MemoryLoad {
        load_percent: status.dwMemoryLoad,
        total_mb: megabytes(status.ullTotalPhys),
        available_mb: megabytes(status.ullAvailPhys),
        commit_limit_mb: megabytes(status.ullTotalPageFile),
        commit_available_mb: megabytes(status.ullAvailPageFile),
    })
}

/// One process as the system-wide query reports it.
struct ProcessTimes {
    pid: u32,
    /// When it was made, in 100-nanosecond units, which tells a process
    /// from a later one given the same id.
    created: i64,
    name: String,
    /// User and kernel time together, in 100-nanosecond units.
    processor: u64,
    working_set: usize,
    threads: u32,
}

/// Processor use over `interval`: the system's from `GetSystemTimes`, and
/// each process's from two readings of the process list.
fn processor(interval: Duration) -> Result<ProcessorLoad, String> {
    let milliseconds_of_interval = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
    let mut pacer =
        Pacer::new(milliseconds_of_interval).map_err(|error| format!("the timer: {error}"))?;
    let started = Instant::now();
    let (first_idle, first_total) = system_times()?;
    let first = processes()?;
    pacer
        .wait_for(1)
        .map_err(|error| format!("the timer: {error}"))?;
    let (idle, total) = system_times()?;
    let second = processes()?;
    let interval_ms = milliseconds(started.elapsed());
    let total = total.saturating_sub(first_total);
    let busy = total.saturating_sub(idle.saturating_sub(first_idle));
    let share = |spent: u64| -> u32 {
        spent
            .saturating_mul(1000)
            .checked_div(total)
            .map_or(0, |tenths| u32::try_from(tenths).unwrap_or(u32::MAX))
    };
    let mut loads: Vec<(u64, ProcessLoad)> = second
        .iter()
        .map(|process| {
            let before = first
                .iter()
                .find(|earlier| earlier.pid == process.pid && earlier.created == process.created)
                .map_or(0, |earlier| earlier.processor);
            let spent = process.processor.saturating_sub(before);
            (
                spent,
                ProcessLoad {
                    name: process.name.clone(),
                    pid: process.pid,
                    processor_tenths: share(spent),
                    processor_ms: process.processor / 10_000,
                    working_set_kb: u64::try_from(process.working_set >> 10).unwrap_or(u64::MAX),
                    threads: process.threads,
                },
            )
        })
        .collect();
    let dwm = loads
        .iter()
        .filter(|(_, load)| load.name.eq_ignore_ascii_case("dwm.exe"))
        .map(|(_, load)| load.clone())
        .collect();
    // The idle process, id 0, is the time no process spent.
    loads.retain(|(_, load)| load.pid != 0);
    loads.sort_by(|(left, _), (right, _)| right.cmp(left));
    Ok(ProcessorLoad {
        interval_ms,
        busy_tenths: share(busy),
        processes: u32::try_from(second.len()).unwrap_or(u32::MAX),
        top: loads
            .into_iter()
            .take(TOP_PROCESSES)
            .map(|(_, load)| load)
            .collect(),
        dwm,
    })
}

/// The time every processor has spent idle, and in all, since the system
/// started, in 100-nanosecond units.
fn system_times() -> Result<(u64, u64), String> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the out-parameters are locals.
    unsafe {
        GetSystemTimes(
            Some(&raw mut idle),
            Some(&raw mut kernel),
            Some(&raw mut user),
        )
    }
    .map_err(|error| format!("GetSystemTimes failed: {error}"))?;
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    // Kernel time includes the idle time.
    Ok((ticks(idle), ticks(kernel) + ticks(user)))
}

/// Every process, read with one system-wide query, which opens none of
/// them.
fn processes() -> Result<Vec<ProcessTimes>, String> {
    // `u64`s, so the entries the query writes are aligned.
    let mut buffer: Vec<u64> = vec![0; (512 << 10) / 8];
    loop {
        let bytes = buffer.len() * 8;
        let mut needed = 0u32;
        // SAFETY: the buffer is writable for `bytes` bytes, and `needed` is
        // a local.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                buffer.as_mut_ptr().cast(),
                u32::try_from(bytes).unwrap_or(u32::MAX),
                &raw mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // The list grew past the buffer: read it again into one with
            // room for the size the query asked for and more.
            let grown = (needed as usize).max(bytes) + (256 << 10);
            if grown > MAX_PROCESS_LIST_BYTES {
                return Err(format!(
                    "the process list asked for {needed} bytes, more than the {MAX_PROCESS_LIST_BYTES} allowed"
                ));
            }
            buffer = vec![0; grown.div_ceil(8)];
            continue;
        }
        if status.is_err() {
            return Err(format!(
                "NtQuerySystemInformation failed: 0x{:08X}",
                status.0
            ));
        }
        return Ok(parse_processes(&buffer));
    }
}

/// The processes the query wrote into `buffer`.
fn parse_processes(buffer: &[u64]) -> Vec<ProcessTimes> {
    let base = buffer.as_ptr().cast::<u8>();
    let mut offset = 0usize;
    let mut processes = Vec::new();
    loop {
        if offset + size_of::<SYSTEM_PROCESS_INFORMATION>() > size_of_val(buffer) {
            return processes;
        }
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "read with read_unaligned, which needs no alignment"
        )]
        let at = base
            .wrapping_add(offset)
            .cast::<SYSTEM_PROCESS_INFORMATION>();
        // SAFETY: the query wrote a chain of entries into `buffer`, each at
        // the offset the one before gives, and the entry at `offset` lies
        // within it, as checked above; reading one copies it out.
        let entry = unsafe { at.read_unaligned() };
        // The documented structure leaves the times among its reserved
        // bytes, laid out since Windows 7 as the private working set (8
        // bytes), the hard fault count and the threads' high-water mark (4
        // each), the cycle time, then the creation, user and kernel times
        // (8 each).
        let field = |at: usize| {
            i64::from_le_bytes(
                entry.Reserved1[at..at + 8]
                    .try_into()
                    .expect("eight bytes of a 48-byte array"),
            )
        };
        let (created, user, kernel) = (field(24), field(32), field(40));
        let name = if entry.ImageName.Buffer.is_null() || entry.ImageName.Length == 0 {
            "(idle)".to_owned()
        } else {
            // SAFETY: the name points into `buffer`, `Length` bytes long.
            String::from_utf16_lossy(unsafe {
                std::slice::from_raw_parts(
                    entry.ImageName.Buffer.0,
                    usize::from(entry.ImageName.Length) / 2,
                )
            })
        };
        processes.push(ProcessTimes {
            pid: u32::try_from(entry.UniqueProcessId.0 as usize).unwrap_or(u32::MAX),
            created,
            name,
            processor: u64::try_from(user).unwrap_or(0) + u64::try_from(kernel).unwrap_or(0),
            working_set: entry.WorkingSetSize,
            threads: entry.NumberOfThreads,
        });
        if entry.NextEntryOffset == 0 {
            return processes;
        }
        offset += entry.NextEntryOffset as usize;
    }
}

/// The largest buffer the services' list is read into, in bytes: far
/// above any machine's list, so a size that keeps growing ends as an error.
const MAX_SERVICE_LIST_BYTES: usize = 16 << 20;

/// Every running Win32 service and the process it runs in, from the
/// service control manager's list, which opens no process.
fn services() -> Result<Vec<RunningService>, String> {
    // SAFETY: the local machine's active database; the handle is closed
    // below, once.
    let manager =
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) }
            .map_err(|error| format!("OpenSCManagerW failed: {error}"))?;
    let read = list_services(manager);
    // SAFETY: the handle opened above.
    // A failed close leaves only a handle until the agent ends, and the
    // list is read already, so its error is not reported.
    let _ = unsafe { CloseServiceHandle(manager) };
    read
}

/// [`services`]' list, from the open `manager`.
fn list_services(manager: SC_HANDLE) -> Result<Vec<RunningService>, String> {
    // `u64`s, so the entries the call writes are aligned.
    let mut buffer: Vec<u64> = vec![0; (64 << 10) / 8];
    let mut services = Vec::new();
    let mut resume = 0u32;
    loop {
        let mut needed = 0u32;
        let mut returned = 0u32;
        // One pointer for the call's writes and the reads after it.
        let start = buffer.as_mut_ptr().cast::<u8>();
        // SAFETY: a byte view of all of `buffer`, used only by the call
        // below.
        let bytes = unsafe { std::slice::from_raw_parts_mut(start, buffer.len() * 8) };
        // SAFETY: `manager` is open, `bytes` is writable, and the counts and
        // the resume handle are locals.
        let listed = unsafe {
            EnumServicesStatusExW(
                manager,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_ACTIVE,
                Some(bytes),
                &raw mut needed,
                &raw mut returned,
                Some(&raw mut resume),
                PCWSTR::null(),
            )
        };
        let more = match listed {
            Ok(()) => false,
            Err(error) if error.code() == ERROR_MORE_DATA.to_hresult() => true,
            Err(error) => return Err(format!("EnumServicesStatusExW failed: {error}")),
        };
        #[expect(
            clippy::cast_ptr_alignment,
            reason = "a `u64` buffer is aligned for the entries' pointers"
        )]
        let entries = start.cast::<ENUM_SERVICE_STATUS_PROCESSW>();
        for index in 0..returned as usize {
            // SAFETY: the call wrote `returned` entries at the buffer's
            // start, each name pointing into the buffer, which is not
            // written again until they are read.
            let entry = unsafe { &*entries.wrapping_add(index) };
            // SAFETY: a terminated string in `buffer`, as above.
            let name = unsafe { entry.lpServiceName.to_string() }
                .map_err(|error| format!("a service's name is not UTF-16: {error}"))?;
            services.push(RunningService {
                name,
                pid: entry.ServiceStatusProcess.dwProcessId,
            });
        }
        if !more {
            return Ok(services);
        }
        // With nothing returned the buffer is too small for the next
        // entry: grow it to the size the call asked for.
        if returned == 0 {
            let grown = (needed as usize).max(buffer.len() * 8) + (16 << 10);
            if grown > MAX_SERVICE_LIST_BYTES {
                return Err(format!(
                    "the services' list asked for {needed} bytes, more than the {MAX_SERVICE_LIST_BYTES} allowed"
                ));
            }
            buffer = vec![0; grown.div_ceil(8)];
        }
    }
}

/// The disks' queue, all physical disks together, over `interval`, from
/// the performance data helper's counters, named in English so a
/// machine's language does not change them.
fn disk_queue(interval: Duration) -> Result<DiskQueue, String> {
    let mut pacer = Pacer::new(u32::try_from(interval.as_millis()).unwrap_or(u32::MAX))
        .map_err(|error| format!("the timer: {error}"))?;
    let mut query = PDH_HQUERY::default();
    // SAFETY: no data source, so the live counters; `query` is a local.
    let opened = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut query) };
    if opened != 0 {
        return Err(format!("PdhOpenQueryW failed: 0x{opened:08X}"));
    }
    let read = read_disk_queue(query, &mut pacer);
    // SAFETY: closes the query opened above, once, with its counters.
    let _ = unsafe { PdhCloseQuery(query) };
    read
}

/// [`disk_queue`]'s reading, from the open `query`.
fn read_disk_queue(query: PDH_HQUERY, pacer: &mut Pacer) -> Result<DiskQueue, String> {
    let current = add_counter(
        query,
        w!(r"\PhysicalDisk(_Total)\Current Disk Queue Length"),
    )?;
    let average = add_counter(query, w!(r"\PhysicalDisk(_Total)\Avg. Disk Queue Length"))?;
    collect(query)?;
    pacer
        .wait_for(1)
        .map_err(|error| format!("the timer: {error}"))?;
    collect(query)?;
    Ok(DiskQueue {
        current_hundredths: hundredths(current)?,
        average_hundredths: hundredths(average)?,
    })
}

/// Adds the counter `path` to `query`.
fn add_counter(query: PDH_HQUERY, path: PCWSTR) -> Result<PDH_HCOUNTER, String> {
    let mut counter = PDH_HCOUNTER::default();
    // SAFETY: `query` is open, `path` a static string, `counter` a local.
    let added = unsafe { PdhAddEnglishCounterW(query, path, 0, &raw mut counter) };
    if added == 0 {
        Ok(counter)
    } else {
        // SAFETY: a static, terminated string.
        let name = unsafe { path.to_string() }.unwrap_or_default();
        Err(format!(
            "PdhAddEnglishCounterW({name}) failed: 0x{added:08X}"
        ))
    }
}

/// Collects `query`'s counters.
fn collect(query: PDH_HQUERY) -> Result<(), String> {
    // SAFETY: `query` is open.
    let collected = unsafe { PdhCollectQueryData(query) };
    if collected == 0 {
        Ok(())
    } else {
        Err(format!("PdhCollectQueryData failed: 0x{collected:08X}"))
    }
}

/// `counter`'s value, in hundredths.
fn hundredths(counter: PDH_HCOUNTER) -> Result<u32, String> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: `counter` belongs to an open query collected twice; `value` is
    // a local.
    let formatted =
        unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &raw mut value) };
    if formatted != 0 {
        return Err(format!(
            "PdhGetFormattedCounterValue failed: 0x{formatted:08X}"
        ));
    }
    // SAFETY: asked for as a double, so the union holds one.
    let double = unsafe { value.Anonymous.doubleValue };
    // A queue length is never negative, and its hundredths fit a `u32`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a non-negative queue length rounded to hundredths"
    )]
    Ok((double * 100.0).round().max(0.0) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::mpsc::TryRecvError;

    /// While two threads of this process spin, the sample reads the
    /// memory, the processor use with this process among the top
    /// processes, the most first, with at least one processor's share, the Desktop Window Manager, and the
    /// disks' queue.
    #[test]
    fn a_process_spinning_is_among_the_top_processes() {
        // Each spins until its sender is dropped.
        let (stops, spinners): (Vec<_>, Vec<_>) = (0..2)
            .map(|_| {
                let (stop, stopped) = mpsc::channel::<()>();
                let spinner = thread::spawn(move || {
                    while matches!(stopped.try_recv(), Err(TryRecvError::Empty)) {
                        std::hint::spin_loop();
                    }
                });
                (stop, spinner)
            })
            .unzip();
        let load = runner_load(Duration::from_secs(1), Duration::from_secs(20));
        drop(stops);
        for spinner in spinners {
            spinner.join().expect("the spinner ends");
        }

        let memory = load.memory.expect("the memory is read");
        assert!(memory.total_mb > 0, "{memory:?}");
        assert!(memory.available_mb <= memory.total_mb, "{memory:?}");
        let processor = load.processor.expect("the processor use is read");
        assert!(processor.interval_ms >= 1_000, "{processor:?}");
        assert!(processor.busy_tenths > 0, "{processor:?}");
        assert_eq!(processor.top.len(), TOP_PROCESSES, "{processor:?}");
        assert!(
            processor
                .top
                .windows(2)
                .all(|pair| pair[0].processor_tenths >= pair[1].processor_tenths),
            "the most first: {processor:?}"
        );
        let this = processor
            .top
            .iter()
            .find(|process| process.pid == std::process::id())
            .unwrap_or_else(|| panic!("this process is among the top: {processor:?}"));
        // Two threads spinning for the whole interval spend two
        // processors' time; at least one processor's share is asked for,
        // since the spinners share the machine with everything else.
        let processors = thread::available_parallelism()
            .expect("the processor count")
            .get();
        let one_processor = u32::try_from(1000 / processors).expect("a share in tenths");
        assert!(
            this.processor_tenths >= one_processor,
            "at least one of {processors} processors' share, {one_processor} tenths: {this:?}"
        );
        assert!(this.working_set_kb > 0, "{this:?}");
        assert!(this.threads >= 3, "the spinners and the sampler: {this:?}");
        assert!(!processor.dwm.is_empty(), "the Desktop Window Manager runs");
        assert!(
            processor
                .dwm
                .iter()
                .all(|dwm| dwm.name.eq_ignore_ascii_case("dwm.exe") && dwm.threads > 0),
            "{:?}",
            processor.dwm
        );
        load.disk.expect("the disks' queue is read");
        let services = load.services.expect("the services are read");
        // The event log service runs on every Windows, in a process of its
        // own kind, `svchost.exe`, which is among the processes listed.
        let event_log = services
            .iter()
            .find(|service| service.name.eq_ignore_ascii_case("EventLog"))
            .unwrap_or_else(|| panic!("the event log service runs: {services:?}"));
        assert_ne!(event_log.pid, 0, "{event_log:?}");
    }

    /// A sample whose parts cannot be read within its time limit, since
    /// its interval is longer, answers at the limit with the memory, read
    /// at once, and says the processor use and the disks' queue were not
    /// read. The services, read at once too, are not asserted: how fast
    /// the service control manager answers is not this test's to fix.
    #[test]
    fn a_sample_answers_at_its_time_limit_with_what_it_has() {
        let load = runner_load(Duration::from_secs(10), Duration::from_millis(500));
        assert!(load.memory.is_ok(), "{:?}", load.memory);
        assert_eq!(
            load.processor,
            Err("the processor use was not read within 500ms".to_owned())
        );
        assert_eq!(
            load.disk,
            Err("the disks' queue was not read within 500ms".to_owned())
        );
        assert!(
            (500..10_000).contains(&load.elapsed_ms),
            "answered at its limit, not the interval's end: {} ms",
            load.elapsed_ms
        );
    }
}
