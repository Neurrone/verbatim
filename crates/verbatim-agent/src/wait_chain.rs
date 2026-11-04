//! [`Request::WaitChains`](crate::protocol::Request::WaitChains): what a
//! thread that stopped answering waits on, read with Windows' Wait Chain
//! Traversal, so a scenario whose window did not open in time names the
//! object each not-responding window's thread is blocked on, and the thread
//! that holds it, following the chain across processes.

use std::collections::HashSet;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Diagnostics::Debug::{
    CloseThreadWaitChainSession, GetThreadWaitChain, OPEN_THREAD_WAIT_CHAIN_SESSION_FLAGS,
    OpenThreadWaitChainSession, PCOGETACTIVATIONSTATE, PCOGETCALLSTATE,
    RegisterWaitChainCOMCallback, WAIT_CHAIN_THREAD_OPTIONS, WAITCHAIN_NODE_INFO,
    WCT_MAX_NODE_COUNT, WCT_OBJECT_STATUS, WCT_OBJECT_TYPE, WCT_OUT_OF_PROC_COM_FLAG,
    WCT_OUT_OF_PROC_CS_FLAG, WCT_OUT_OF_PROC_FLAG, WctAlpcType, WctComActivationType, WctComType,
    WctCriticalSectionType, WctMutexType, WctProcessWaitType, WctSendMessageType, WctSmbIoType,
    WctSocketIoType, WctStatusAbandoned, WctStatusBlocked, WctStatusError, WctStatusNoAccess,
    WctStatusNotOwned, WctStatusOwned, WctStatusPidOnly, WctStatusPidOnlyRpcss, WctStatusRunning,
    WctStatusUnknown, WctThreadType, WctThreadWaitType, WctUnknownType,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use windows::core::{BOOL, s, w};

use crate::protocol::{
    ProcessState, WaitChain, WaitNode, WaitNodeKind, WaitNodeStatus, WaitThread,
};
use crate::{desktop, process};

/// How long [`wait_chains_within`] waits for the chains: under the
/// harness's 30-second bound on a reply, so it gets the reason rather than
/// a timed-out connection.
pub const WAIT_CHAINS_TIMEOUT: Duration = Duration::from_secs(10);

/// [`wait_chains`], read on a thread of its own and answered within
/// `timeout`, so the connection's thread is never held longer.
/// Microsoft's documentation of `GetThreadWaitChain` gives no bound on how
/// long a synchronous session's call takes, and Wait Chain Traversal
/// follows a chain into other processes.
///
/// # Errors
///
/// Returns the reason when the thread cannot be started, or the chains are
/// not read within `timeout`; the reading thread is then left to finish
/// alone, its answer dropped.
pub fn wait_chains_within(
    windows: Vec<u64>,
    pids: Vec<u32>,
    timeout: Duration,
) -> Result<Vec<WaitChain>, String> {
    answer_within("the wait chains", timeout, move || {
        wait_chains(&windows, &pids)
    })
}

/// `read`'s answer, run on a thread of its own, or why there is none
/// within `timeout`; `what` names the answer in that reason.
pub(crate) fn answer_within<T: Send + 'static>(
    what: &'static str,
    timeout: Duration,
    read: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("diagnostics".to_owned())
        .spawn(move || {
            // The receiver is gone when the answer came too late.
            let _ = sender.send(read());
        })
        .map_err(|error| format!("the thread reading {what} could not be started: {error}"))?;
    receiver.recv_timeout(timeout).map_err(|error| match error {
        mpsc::RecvTimeoutError::Timeout => {
            format!("{what} were not read within {timeout:?}")
        }
        mpsc::RecvTimeoutError::Disconnected => {
            format!("the thread reading {what} ended without an answer")
        }
    })
}

/// The wait chain of the thread owning each of `windows`, then of each
/// thread of each process in `pids` that owns a top-level window, or of
/// every thread of one that owns none, each thread once. A process in
/// `pids` that has exited gets one chain saying so, read through the
/// handle the agent holds for a child it launched, which keeps the pid from
/// naming another process's threads.
#[must_use]
pub fn wait_chains(windows: &[u64], pids: &[u32]) -> Vec<WaitChain> {
    // Each subject's window, process, and thread, or why it has no thread.
    let mut subjects: Vec<(Option<u64>, u32, Result<u32, String>)> = Vec::new();
    for &window in windows {
        let mut pid = 0u32;
        // SAFETY: tolerates any handle, writing 0 for an invalid one.
        let thread =
            unsafe { GetWindowThreadProcessId(HWND(window as *mut _), Some(&raw mut pid)) };
        let thread = match thread {
            0 => Err("the window no longer exists".to_owned()),
            thread => Ok(thread),
        };
        subjects.push((Some(window), pid, thread));
    }
    for &pid in pids {
        if let Ok(ProcessState::Exited { exit_code }) = process::status(pid) {
            let exited = match exit_code {
                Some(code) => format!("the process has exited, with exit code {code}"),
                None => "the process has exited".to_owned(),
            };
            subjects.push((None, pid, Err(exited)));
            continue;
        }
        let threads = window_threads(pid);
        let threads = if threads.is_empty() {
            process_threads(pid)
        } else {
            threads
        };
        if threads.is_empty() {
            subjects.push((
                None,
                pid,
                Err("the process has no thread; it has exited".to_owned()),
            ));
        }
        subjects.extend(threads.into_iter().map(|thread| (None, pid, Ok(thread))));
    }
    let mut seen = HashSet::new();
    subjects.retain(|(_, _, thread)| match thread {
        Ok(thread) => seen.insert(*thread),
        Err(_) => true,
    });

    with_session(|opened| {
        subjects
            .into_iter()
            .map(|(window, pid, thread)| {
                let mut chain = WaitChain {
                    window,
                    pid,
                    thread: *thread.as_ref().unwrap_or(&0),
                    nodes: Vec::new(),
                    cycle: false,
                    error: None,
                };
                let read = match (opened, thread) {
                    (_, Err(reason)) => Err(reason),
                    (Err(error), Ok(_)) => Err(error.clone()),
                    (Ok(session), Ok(thread)) => read_chain(*session, thread),
                };
                match read {
                    Ok((nodes, cycle)) => {
                        chain.nodes = nodes;
                        chain.cycle = cycle;
                    }
                    Err(error) => chain.error = Some(error),
                }
                chain
            })
            .collect()
    })
}

/// `use_session`'s answer given a synchronous Wait Chain Traversal session,
/// or why one could not be opened, with COM's state functions registered
/// first ([`register_com_callback`]), and no other session of this process
/// open meanwhile: a session opened and closed while another thread's
/// session read a chain crashed this process in about one run of five.
fn with_session<T>(use_session: impl FnOnce(&Result<*mut core::ffi::c_void, String>) -> T) -> T {
    static ONE_SESSION: Mutex<()> = Mutex::new(());
    let _only = ONE_SESSION.lock().unwrap_or_else(PoisonError::into_inner);
    register_com_callback();
    // SAFETY: a synchronous session (no flags, no callback), closed below.
    let session =
        unsafe { OpenThreadWaitChainSession(OPEN_THREAD_WAIT_CHAIN_SESSION_FLAGS(0), None) };
    let opened = if session.is_null() {
        Err(format!(
            "the wait chain session could not be opened: {}",
            windows::core::Error::from_thread()
        ))
    } else {
        Ok(session)
    };
    let answer = use_session(&opened);
    if let Ok(session) = opened {
        // SAFETY: the session opened above, closed once.
        unsafe { CloseThreadWaitChainSession(session) };
    }
    answer
}

/// The nodes of `thread`'s wait chain, and whether it is a cycle.
fn read_chain(
    session: *mut core::ffi::c_void,
    thread: u32,
) -> Result<(Vec<WaitNode>, bool), String> {
    let mut nodes = [WAITCHAIN_NODE_INFO::default(); WCT_MAX_NODE_COUNT as usize];
    let mut count = WCT_MAX_NODE_COUNT;
    let mut cycle = BOOL(0);
    // SAFETY: `session` is open and synchronous; `nodes` holds `count`
    // entries, which the call writes at most.
    unsafe {
        GetThreadWaitChain(
            session,
            None,
            WAIT_CHAIN_THREAD_OPTIONS(
                WCT_OUT_OF_PROC_FLAG.0 | WCT_OUT_OF_PROC_COM_FLAG.0 | WCT_OUT_OF_PROC_CS_FLAG.0,
            ),
            thread,
            &raw mut count,
            nodes.as_mut_ptr(),
            &raw mut cycle,
        )
    }
    .map_err(|error| error.to_string())?;
    let count = usize::try_from(count).unwrap_or(0).min(nodes.len());
    Ok((nodes[..count].iter().map(node).collect(), cycle.as_bool()))
}

/// What a test is told of one node.
fn node(info: &WAITCHAIN_NODE_INFO) -> WaitNode {
    let kind = kind(info.ObjectType);
    let thread = (kind == WaitNodeKind::Thread).then(|| {
        // SAFETY: a thread node's union holds its thread object.
        let object = unsafe { info.Anonymous.ThreadObject };
        WaitThread {
            pid: object.ProcessId,
            thread: object.ThreadId,
            image: desktop::image_name(object.ProcessId).unwrap_or_default(),
        }
    });
    WaitNode {
        kind,
        status: status(info.ObjectStatus),
        thread,
    }
}

/// The vocabulary's name for a node's kind.
#[expect(
    non_upper_case_globals,
    reason = "the patterns are the windows crate's own constants"
)]
fn kind(kind: WCT_OBJECT_TYPE) -> WaitNodeKind {
    match kind {
        WctThreadType => WaitNodeKind::Thread,
        WctCriticalSectionType => WaitNodeKind::CriticalSection,
        WctSendMessageType => WaitNodeKind::SendMessage,
        WctMutexType => WaitNodeKind::Mutex,
        WctAlpcType => WaitNodeKind::Alpc,
        WctComType => WaitNodeKind::Com,
        WctComActivationType => WaitNodeKind::ComActivation,
        WctThreadWaitType => WaitNodeKind::ThreadWait,
        WctProcessWaitType => WaitNodeKind::ProcessWait,
        WctSocketIoType => WaitNodeKind::SocketIo,
        WctSmbIoType => WaitNodeKind::SmbIo,
        WctUnknownType => WaitNodeKind::Unknown,
        WCT_OBJECT_TYPE(other) => WaitNodeKind::Other(other),
    }
}

/// The vocabulary's name for a node's state.
#[expect(
    non_upper_case_globals,
    reason = "the patterns are the windows crate's own constants"
)]
fn status(status: WCT_OBJECT_STATUS) -> WaitNodeStatus {
    match status {
        WctStatusNoAccess => WaitNodeStatus::NoAccess,
        WctStatusRunning => WaitNodeStatus::Running,
        WctStatusBlocked => WaitNodeStatus::Blocked,
        WctStatusPidOnly => WaitNodeStatus::PidOnly,
        WctStatusPidOnlyRpcss => WaitNodeStatus::PidOnlyRpcss,
        WctStatusOwned => WaitNodeStatus::Owned,
        WctStatusNotOwned => WaitNodeStatus::NotOwned,
        WctStatusAbandoned => WaitNodeStatus::Abandoned,
        WctStatusUnknown => WaitNodeStatus::Unknown,
        WctStatusError => WaitNodeStatus::Error,
        WCT_OBJECT_STATUS(other) => WaitNodeStatus::Other(other),
    }
}

/// The threads of process `pid` that own a top-level window, shown or
/// not, in the windows' Z order.
fn window_threads(pid: u32) -> Vec<u32> {
    let mut threads = Vec::new();
    for window in desktop::all_top_level_windows() {
        let mut owner = 0u32;
        // SAFETY: tolerates any handle, writing 0 for an invalid one.
        let thread = unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
        if owner == pid && thread != 0 && !threads.contains(&thread) {
            threads.push(thread);
        }
    }
    threads
}

/// Every thread of process `pid`, in the order Windows lists them; empty
/// when it has none or they cannot be listed.
fn process_threads(pid: u32) -> Vec<u32> {
    // SAFETY: snapshots every thread; the handle is closed below.
    let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else {
        return Vec::new();
    };
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>()).unwrap_or(u32::MAX),
        ..Default::default()
    };
    let mut threads = Vec::new();
    // SAFETY: `snapshot` is open and `entry` has its size set.
    let mut has_entry = unsafe { Thread32First(snapshot, &raw mut entry) }.is_ok();
    while has_entry {
        if entry.th32OwnerProcessID == pid {
            threads.push(entry.th32ThreadID);
        }
        // SAFETY: as above.
        has_entry = unsafe { Thread32Next(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the snapshot opened above, closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    threads
}

/// Gives Wait Chain Traversal COM's own state functions, once, so it can
/// follow a COM call to the thread serving it (`WCT_OUT_OF_PROC_COM_FLAG`
/// needs them). When `ole32.dll` cannot be loaded, chains stop at a COM
/// node instead.
fn register_com_callback() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        // SAFETY: loads a system library, kept loaded for the process's
        // life, as the registration needs.
        let Ok(ole32) = (unsafe { LoadLibraryW(w!("ole32.dll")) }) else {
            return;
        };
        // SAFETY: looks up an export of the module just loaded.
        let call_state = unsafe { GetProcAddress(ole32, s!("CoGetCallState")) };
        // SAFETY: as above.
        let activation_state = unsafe { GetProcAddress(ole32, s!("CoGetActivationState")) };
        let (Some(call_state), Some(activation_state)) = (call_state, activation_state) else {
            return;
        };
        // SAFETY: the export has exactly the signature the callback type
        // names, as `RegisterWaitChainCOMCallback` documents.
        let call_state: PCOGETCALLSTATE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(i32, *mut u32) -> windows::core::HRESULT,
            >(call_state)
        });
        // SAFETY: as above.
        let activation_state: PCOGETACTIVATIONSTATE = Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(
                    windows::core::GUID,
                    u32,
                    *mut u32,
                ) -> windows::core::HRESULT,
            >(activation_state)
        });
        // SAFETY: both callbacks are ole32's own, which stays loaded.
        unsafe { RegisterWaitChainCOMCallback(call_state, activation_state) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::GetCurrentThreadId;

    /// This test's own thread, running, has a one-node chain naming it and
    /// this process: the session opens and the node is read.
    #[test]
    fn a_running_thread_s_chain_is_itself() {
        // SAFETY: no preconditions.
        let thread = unsafe { GetCurrentThreadId() };
        let pid = std::process::id();
        let read = with_session(|opened| {
            let session = opened.as_ref().expect("the session opens");
            read_chain(*session, thread)
        });
        let image = desktop::image_name(pid).expect("this process's image");
        assert_eq!(
            read,
            Ok((
                vec![WaitNode {
                    kind: WaitNodeKind::Thread,
                    status: WaitNodeStatus::Running,
                    thread: Some(WaitThread { pid, thread, image }),
                }],
                false
            ))
        );
    }

    /// A reading that does not finish within the bound is answered with
    /// the reason, and the reading is left to finish alone.
    #[test]
    fn a_reading_past_its_bound_is_answered_with_why() {
        let (release, released) = mpsc::channel::<()>();
        let answer = answer_within("the wait chains", Duration::from_millis(50), move || {
            released.recv()
        });
        assert_eq!(
            answer.map(|_| ()),
            Err("the wait chains were not read within 50ms".to_owned())
        );
        release.send(()).expect("the reading thread still waits");
    }

    /// A launched process that has exited, its pid kept by the handle the
    /// agent holds, has one chain saying so and naming its exit code, not
    /// any thread.
    #[test]
    fn an_exited_process_s_chain_says_it_exited() {
        let _launching = process::LAUNCHING
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        // Launched as the agent launches, inheriting none of another
        // test's handles, its handle held while its entry is kept.
        let (pid, _) = process::launch(
            "cmd",
            &["/C".to_owned(), "exit 3".to_owned()],
            None,
            &[],
            None,
            None,
            (false, false, false),
        )
        .expect("spawns cmd");
        assert_eq!(
            process::wait_for_exit(pid, Duration::from_secs(10)).expect("waits"),
            ProcessState::Exited { exit_code: Some(3) }
        );
        assert_eq!(
            wait_chains(&[], &[pid]),
            vec![WaitChain {
                window: None,
                pid,
                thread: 0,
                nodes: Vec::new(),
                cycle: false,
                error: Some("the process has exited, with exit code 3".to_owned()),
            }]
        );
    }
}
