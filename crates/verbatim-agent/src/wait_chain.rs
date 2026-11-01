//! [`Request::WaitChains`](crate::protocol::Request::WaitChains): what a
//! thread that stopped answering waits on, read with Windows' Wait Chain
//! Traversal, so a scenario whose window did not open in time names the
//! object each not-responding window's thread is blocked on, and the thread
//! that holds it, following the chain across processes.

use std::collections::HashSet;
use std::mem::size_of;
use std::sync::OnceLock;

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

use crate::desktop;
use crate::protocol::{WaitChain, WaitNode, WaitNodeKind, WaitNodeStatus, WaitThread};

/// The wait chain of the thread owning each of `windows`, then of each
/// thread of each process in `pids` that owns a top-level window, or of
/// every thread of one that owns none, each thread once.
#[must_use]
pub fn wait_chains(windows: &[u64], pids: &[u32]) -> Vec<WaitChain> {
    let mut subjects: Vec<(Option<u64>, u32, u32)> = Vec::new();
    for &window in windows {
        let mut pid = 0u32;
        // SAFETY: tolerates any handle, writing 0 for an invalid one.
        let thread =
            unsafe { GetWindowThreadProcessId(HWND(window as *mut _), Some(&raw mut pid)) };
        subjects.push((Some(window), pid, thread));
    }
    for &pid in pids {
        let threads = window_threads(pid);
        let threads = if threads.is_empty() {
            process_threads(pid)
        } else {
            threads
        };
        if threads.is_empty() {
            subjects.push((None, pid, 0));
        }
        subjects.extend(threads.into_iter().map(|thread| (None, pid, thread)));
    }
    let mut seen = HashSet::new();
    subjects.retain(|&(_, _, thread)| thread == 0 || seen.insert(thread));

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
    let chains = subjects
        .into_iter()
        .map(|(window, pid, thread)| {
            let mut chain = WaitChain {
                window,
                pid,
                thread,
                nodes: Vec::new(),
                cycle: false,
                error: None,
            };
            let read = match (&opened, thread) {
                (Err(error), _) => Err(error.clone()),
                (Ok(_), 0) if window.is_some() => Err("the window no longer exists".to_owned()),
                (Ok(_), 0) => Err("the process has no thread; it has exited".to_owned()),
                (Ok(session), thread) => read_chain(*session, thread),
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
        .collect();
    if let Ok(session) = opened {
        // SAFETY: the session opened above, closed once.
        unsafe { CloseThreadWaitChainSession(session) };
    }
    chains
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
        // SAFETY: a synchronous session, closed below.
        let session =
            unsafe { OpenThreadWaitChainSession(OPEN_THREAD_WAIT_CHAIN_SESSION_FLAGS(0), None) };
        assert!(!session.is_null());
        let read = read_chain(session, thread);
        // SAFETY: opened above.
        unsafe { CloseThreadWaitChainSession(session) };
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
}
