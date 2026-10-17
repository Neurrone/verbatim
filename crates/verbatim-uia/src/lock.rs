//! The lock per application around the UIA calls that collide
//! (`docs/design/focus-pipeline.md`, section 20; decision D13).
//!
//! # The defect this works around
//!
//! UI Automation has a defect in how an application serves a remote
//! operation. From the moment a remote operation from any process arrives in
//! an application until UIA begins evaluating it there, every new UIA
//! connection that another process makes to that application fails without
//! an error: `UiaHasServerSideProvider` answers that the window has no
//! provider after about three seconds, and `ElementFromHandle` and
//! `ElementFromHandleBuildCache` return UIA's client-side window proxy after
//! about two seconds, in place of the application's own element. A tree
//! walker step from an element already held that goes up into a window of
//! that application from another process fails the same way. The window is
//! short while the application's UI thread pumps, longer for the first
//! operation in the process (UIA sets itself up there), and as long as the
//! UI thread is busy, which is how an application's launch makes it long.
//! The experiments that found this, and what is not exposed, are in section
//! 20.1 and 20.8.
//!
//! In Verbatim the operation is the focus listener's read of a focus and the
//! connection an outpost's provider probe or element lookup, or the
//! reverse: about 3 in 100 Windows Terminal launches and 1 in 25 Notepad
//! openings read the first focus through MSAA, or announced it twice,
//! because of it.
//!
//! # The lock
//!
//! So the listener and the application's outpost share a lock for each
//! application process: an unnamed kernel mutex the supervisor creates and
//! hands each of them by handle duplication or inheritance (section 20.4).
//! Each call that can open the window (a remote operation into the
//! application) and each call the window breaks (a new connection to one of
//! its windows by its handle, and the listener's walk steps up into it from
//! another application) is made holding the lock, taken immediately before
//! the call and released immediately after it. No other call is guarded,
//! and nothing else is done while the lock is held.
//!
//! An [`AppLock`] names the application (its process id and creation time)
//! and holds the mutex's handle. An outpost, whose process serves one
//! application, sets its process's slot once ([`set_process_lock`]); each of
//! the listener's reading threads, which serve one application each, sets
//! its thread's slot ([`set_thread_lock`]). The guarded calls in this crate
//! and in `verbatim-uia-rops` take the thread's lock when it has one, and the
//! process's otherwise ([`guarded`], [`guarded_window`]), so no caller
//! changes, and a call added later through these functions is guarded
//! without anyone remembering to. With no lock set, as in Core's own UIA
//! clients, the end-to-end agent, and the tests that call these functions
//! directly, the calls run unguarded.
//!
//! The wait has no time limit of its own: each guarded call is bounded by
//! UIA's own timeouts ([`crate::CALL_TIMEOUT`]), and a thread waits for the
//! calls queued ahead of it, each within those limits (section 20.5). A
//! mutex whose holder ended while holding it is taken and reported
//! abandoned: it guards no data that could be left inconsistent.

use std::cell::RefCell;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::{
    CloseHandle, DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, FILETIME, HANDLE, HANDLE_FLAG_INHERIT,
    HANDLE_FLAGS, HWND, SetHandleInformation, WAIT_ABANDONED, WAIT_EVENT, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetProcessTimes, INFINITE, MUTEX_MODIFY_STATE, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ReleaseMutex,
    SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowThreadProcessId};
use windows::core::PCWSTR;

/// The class of a console window, whose provider is the console host, the
/// process of the window's thread, while `GetWindowThreadProcessId` names the
/// console's client as its owner.
const CONSOLE_WINDOW_CLASS: &str = "ConsoleWindowClass";

/// One application's lock: the application's process id and creation time,
/// which together name the process for as long as it runs, and a handle to
/// the mutex the supervisor made for it. Cloning shares the handle.
#[derive(Clone)]
pub struct AppLock {
    pid: u32,
    created: u64,
    mutex: Arc<OwnedHandle>,
}

impl std::fmt::Debug for AppLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppLock")
            .field("pid", &self.pid)
            .field("created", &self.created)
            .field("mutex", &self.mutex.as_raw_handle())
            .finish()
    }
}

impl AppLock {
    /// The lock of the application whose process is `pid`, created at
    /// `created` (`GetProcessTimes`' creation time, in 100-nanosecond
    /// intervals), whose mutex is `mutex`.
    #[must_use]
    pub fn new(pid: u32, created: u64, mutex: Arc<OwnedHandle>) -> Self {
        Self {
            pid,
            created,
            mutex,
        }
    }

    /// The application's process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The application's process creation time.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// The mutex's handle.
    #[must_use]
    pub fn mutex(&self) -> &Arc<OwnedHandle> {
        &self.mutex
    }

    /// Whether this lock names the process `pid` created at `created`.
    #[must_use]
    pub fn names(&self, pid: u32, created: u64) -> bool {
        self.pid == pid && self.created == created
    }
}

/// The process's lock, set once by an outpost.
static PROCESS: OnceLock<AppLock> = OnceLock::new();

thread_local! {
    /// The thread's lock, set by each of the listener's reading threads.
    static THREAD: RefCell<Option<AppLock>> = const { RefCell::new(None) };
}

/// How many times any thread of this process has taken a lock.
static TAKEN: AtomicU64 = AtomicU64::new(0);

/// Sets the process's lock, which every thread without a lock of its own
/// takes: an outpost's, whose process serves one application. Set once,
/// before the outpost installs its hook. Returns the lock back when one is
/// set already.
///
/// # Errors
///
/// The lock given, when the process has one already.
pub fn set_process_lock(lock: AppLock) -> Result<(), AppLock> {
    PROCESS.set(lock)
}

/// The process's lock, if one is set.
#[must_use]
pub fn process_lock() -> Option<&'static AppLock> {
    PROCESS.get()
}

/// Sets the calling thread's lock, or clears it with `None`: each of the
/// listener's reading threads sets the lock of the application it serves.
pub fn set_thread_lock(lock: Option<AppLock>) {
    THREAD.with(|slot| *slot.borrow_mut() = lock);
}

/// The calling thread's own lock, if it has one.
#[must_use]
pub fn thread_lock() -> Option<AppLock> {
    THREAD.with(|slot| slot.borrow().clone())
}

/// The lock a guarded call on this thread takes: the thread's, else the
/// process's.
fn current() -> Option<AppLock> {
    thread_lock().or_else(|| PROCESS.get().cloned())
}

/// How many times the threads of this process have taken a lock, for a
/// test that counts the acquisitions of a thread it cannot take them from
/// itself.
#[must_use]
pub fn taken_in_process() -> u64 {
    TAKEN.load(Ordering::Relaxed)
}

/// Runs `call`, a UIA call into the application of this thread's lock (a
/// remote operation), holding that lock, or unguarded when the thread has
/// none. `name` names the call in the log.
pub fn guarded<T>(name: &'static str, call: impl FnOnce() -> T) -> T {
    match current() {
        Some(lock) => guarded_by(&lock, name, call),
        None => call(),
    }
}

/// Runs `call`, a new connection to the window `hwnd` by its handle,
/// holding this thread's lock when the window's provider process
/// ([`provider_process`]) is the lock's application, and unguarded
/// otherwise: a connection to a window of another application takes no
/// lock (section 20.9). `name` names the call in the log.
pub fn guarded_window<T>(name: &'static str, hwnd: isize, call: impl FnOnce() -> T) -> T {
    match current() {
        Some(lock) if provider_process(hwnd) == Some(lock.pid) => guarded_by(&lock, name, call),
        _ => call(),
    }
}

/// Runs `call` holding `lock`: for a call into another application than the
/// thread's own, as the listener's walk step up into a frame host's window
/// is (section 20.3). `name` names the call in the log.
pub fn guarded_by<T>(lock: &AppLock, name: &'static str, call: impl FnOnce() -> T) -> T {
    let _held = Held::take(lock, name, || {});
    call()
}

/// The only rights a lock's holders are given: to wait on the mutex and to
/// release it (`SYNCHRONIZE` and `MUTEX_MODIFY_STATE`).
pub const LOCK_RIGHTS: u32 = SYNCHRONIZATION_SYNCHRONIZE.0 | MUTEX_MODIFY_STATE.0;

/// A new mutex with no name, which no other program can open, not
/// inheritable and not owned: the supervisor's for one application.
///
/// # Errors
///
/// The error of `CreateMutexW`.
pub fn new_mutex() -> std::io::Result<OwnedHandle> {
    // SAFETY: no security attributes, no initial owner, and no name.
    let handle =
        unsafe { CreateMutexW(None, false, PCWSTR::null()) }.map_err(std::io::Error::other)?;
    // SAFETY: the handle was just returned by CreateMutexW, and is owned by
    // nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// Duplicates `mutex` into the process `process`, open with the right to
/// duplicate handles into it, with [`LOCK_RIGHTS`] alone and not
/// inheritable, and returns the new handle's value in that process, which
/// that process owns from now on.
///
/// # Errors
///
/// The error of `DuplicateHandle`.
pub fn duplicate_into(mutex: &OwnedHandle, process: &impl AsRawHandle) -> std::io::Result<usize> {
    // SAFETY: the pseudo-handle of this process, which needs no closing.
    let this = unsafe { GetCurrentProcess() };
    let mut copy = HANDLE::default();
    // SAFETY: both process handles are open for the call, `mutex` is this
    // process's own handle, and `copy` is a local the call writes.
    unsafe {
        DuplicateHandle(
            this,
            HANDLE(mutex.as_raw_handle()),
            HANDLE(process.as_raw_handle()),
            &raw mut copy,
            LOCK_RIGHTS,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )
    }
    .map_err(std::io::Error::other)?;
    Ok(copy.0 as usize)
}

/// A copy of `mutex` in this process with [`LOCK_RIGHTS`] alone, for a
/// child that inherits it: an inherited handle keeps the access of the
/// handle it was inherited from, so the child is given this copy rather
/// than the supervisor's own, which has full access. It is not inheritable
/// itself; the launch makes it inheritable for its `CreateProcessW` alone,
/// as it does the child's pipes (`verbatim_process::launch_with_ends`).
///
/// # Errors
///
/// The error of `DuplicateHandle`.
pub fn restricted_copy(mutex: &OwnedHandle) -> std::io::Result<OwnedHandle> {
    // SAFETY: the pseudo-handle of this process, which needs no closing.
    let this = unsafe { GetCurrentProcess() };
    let mut copy = HANDLE::default();
    // SAFETY: as in `duplicate_into`, into this process.
    unsafe {
        DuplicateHandle(
            this,
            HANDLE(mutex.as_raw_handle()),
            this,
            &raw mut copy,
            LOCK_RIGHTS,
            false,
            DUPLICATE_HANDLE_OPTIONS(0),
        )
    }
    .map_err(std::io::Error::other)?;
    // SAFETY: the handle was just made by DuplicateHandle, and is owned by
    // nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(copy.0) })
}

/// Takes the lock's handle a process was given, by duplication or by
/// inheritance, as `value`, and makes it not inheritable, so no process
/// this one creates holds it.
///
/// # Errors
///
/// The error of `SetHandleInformation`, for a value that is no handle.
///
/// # Safety
///
/// `value` must be a handle this process was given for its own use and that
/// nothing else in it owns.
pub unsafe fn take_given(value: usize) -> std::io::Result<OwnedHandle> {
    let handle = HANDLE(value as *mut _);
    // SAFETY: the caller's contract; the call changes only the flag.
    unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)) }
        .map_err(std::io::Error::other)?;
    // SAFETY: the caller's contract: owned by nothing else.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}

/// The creation time of the process `process`, open for limited queries, in
/// 100-nanosecond intervals since 1601 (`GetProcessTimes`): with its id, it
/// names the process for as long as a handle to it is open.
///
/// # Errors
///
/// The error of `GetProcessTimes`.
pub fn process_created(process: &impl AsRawHandle) -> std::io::Result<u64> {
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the handle is open for the call; the times are locals.
    unsafe {
        GetProcessTimes(
            HANDLE(process.as_raw_handle()),
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .map_err(std::io::Error::other)?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// Opens the process `pid` for limited queries and to be waited on, or,
/// when waiting is refused, for limited queries alone, as the supervisor
/// opens an application it holds, and reads its creation time: a reading
/// thread's hold on its application, which keeps the id from being reused
/// while the thread serves it.
///
/// # Errors
///
/// The error of `OpenProcess` or `GetProcessTimes`.
pub fn open_process(pid: u32) -> std::io::Result<(OwnedHandle, u64)> {
    // SAFETY: OpenProcess with these rights fails safely on an invalid or
    // inaccessible pid; a handle it returns is owned below.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            false,
            pid,
        )
    }
    .or_else(|_| {
        // SAFETY: as above, with the one right.
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
    })
    .map_err(std::io::Error::other)?;
    // SAFETY: just returned by OpenProcess, owned by nothing else.
    let process = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let created = process_created(&process)?;
    Ok((process, created))
}

/// The process that serves UIA for the window `hwnd`: its owner by
/// `GetWindowThreadProcessId`, except for a console window, whose owner by
/// that call is the console's client while its provider is the console
/// host. Windows names the client's thread for a console window too, so the
/// host is read from the client instead ([`console_host_of`]). `None` for a
/// window that no longer exists. Local calls only; a console window costs
/// the client's lookup too.
#[must_use]
pub fn provider_process(hwnd: isize) -> Option<u32> {
    let window = HWND(hwnd as *mut _);
    let mut class = [0u16; 32];
    // SAFETY: the buffer outlives the call, which writes at most its
    // length; an invalid handle answers 0.
    let length = unsafe { GetClassNameW(window, &mut class) };
    let mut owner = 0u32;
    // SAFETY: `owner` is a local; an invalid handle answers thread 0.
    let thread = unsafe { GetWindowThreadProcessId(window, Some(&raw mut owner)) };
    if thread == 0 {
        return None;
    }
    let class = usize::try_from(length)
        .ok()
        .and_then(|length| class.get(..length))
        .map(String::from_utf16_lossy);
    if class.as_deref() == Some(CONSOLE_WINDOW_CLASS) {
        return console_host_of(owner).or(Some(owner));
    }
    Some(owner)
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `ntdll`'s `NtQueryInformationProcess`, for the one information
    /// class the `windows` crate's features here do not reach.
    fn NtQueryInformationProcess(
        process: HANDLE,
        class: u32,
        information: *mut std::ffi::c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// `ProcessConsoleHostProcess`: the console host of a console's client.
const PROCESS_CONSOLE_HOST_PROCESS: u32 = 49;

/// The console host of the console client `client`, the process that
/// serves its console window, or `None` when the client cannot be opened or
/// has no console: `NtQueryInformationProcess`'s `ProcessConsoleHostProcess`,
/// whose answer carries flags in its two low bits.
fn console_host_of(client: u32) -> Option<u32> {
    // SAFETY: OpenProcess with a query-only right fails safely; the handle
    // is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, client) }.ok()?;
    let mut host = 0usize;
    // SAFETY: `host` is a pointer-sized local, the class's answer; the
    // handle is open with query access.
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            PROCESS_CONSOLE_HOST_PROCESS,
            (&raw mut host).cast(),
            u32::try_from(size_of::<usize>()).unwrap_or(0),
            std::ptr::null_mut(),
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    let host = u32::try_from(host & !3).ok()?;
    (status >= 0 && host != 0).then_some(host)
}

/// How a guarded call took its lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Taken {
    /// At once: nobody held it.
    Free,
    /// After waiting for its holder to release it.
    AfterWaiting,
    /// From a holder that ended holding it, which gives it to this waiter.
    Abandoned,
    /// Not at all: the wait failed, and the call runs unguarded.
    Failed,
}

/// A lock held for one guarded call, released when dropped.
struct Held<'a> {
    lock: &'a AppLock,
    name: &'static str,
    taken: Taken,
}

impl<'a> Held<'a> {
    /// Takes `lock` for the call `name`, waiting for as long as another
    /// thread or process holds it, after calling `on_wait` when it must
    /// wait. A first wait that does not block finds it free, which costs one
    /// kernel call and wakes nobody; only a wait that blocks is logged, with
    /// how long it was.
    fn take(lock: &'a AppLock, name: &'static str, on_wait: impl FnOnce()) -> Self {
        let handle = HANDLE(lock.mutex.as_raw_handle());
        tracing::trace!(
            call = name,
            pid = lock.pid,
            "a guarded UIA call takes its application's lock"
        );
        // SAFETY: a zero-timeout wait on the mutex's handle, open for the
        // life of `lock`.
        let mut waited = unsafe { WaitForSingleObject(handle, 0) };
        let mut taken = Taken::Free;
        if waited == WAIT_TIMEOUT {
            tracing::debug!(
                call = name,
                pid = lock.pid,
                "a guarded UIA call waits for its application's lock"
            );
            on_wait();
            let started = Instant::now();
            // SAFETY: as above, with no time limit: the holder's call ends
            // within UIA's own timeouts (section 20.5).
            waited = unsafe { WaitForSingleObject(handle, INFINITE) };
            taken = Taken::AfterWaiting;
            tracing::debug!(
                call = name,
                pid = lock.pid,
                waited_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
                "a guarded UIA call took its application's lock after waiting"
            );
        }
        let taken = report(waited, lock, name).unwrap_or(taken);
        crate::calls::count_lock();
        TAKEN.fetch_add(1, Ordering::Relaxed);
        Self { lock, name, taken }
    }
}

/// Logs a wait that did not simply take the lock, and says how it ended:
/// one whose holder ended holding it, which gives the lock to this waiter,
/// or one that failed, after which the call runs anyway, unguarded. `None`
/// for a wait that took it.
fn report(waited: WAIT_EVENT, lock: &AppLock, name: &'static str) -> Option<Taken> {
    if waited == WAIT_OBJECT_0 {
        return None;
    }
    if waited == WAIT_ABANDONED {
        tracing::info!(
            call = name,
            pid = lock.pid,
            "an application's lock was abandoned by a holder that ended; it is taken"
        );
        Some(Taken::Abandoned)
    } else {
        tracing::warn!(
            call = name,
            pid = lock.pid,
            error = %windows::core::Error::from_thread(),
            "an application's lock could not be waited for; the call runs unguarded"
        );
        Some(Taken::Failed)
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        if self.taken == Taken::Failed {
            return;
        }
        // SAFETY: releases the mutex this thread took in `take`, its handle
        // open for the life of the lock.
        if let Err(error) = unsafe { ReleaseMutex(HANDLE(self.lock.mutex.as_raw_handle())) } {
            tracing::warn!(call = self.name, %error, "an application's lock could not be released");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::Win32::System::Threading::{
        CREATE_SUSPENDED, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, ResumeThread,
        STARTF_USESHOWWINDOW, STARTUPINFOW,
    };
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, EVENT_OBJECT_NAMECHANGE, GetWindowTextW,
        MSG, MsgWaitForMultipleObjects, OBJID_WINDOW, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
        SW_HIDE, WINDOW_EX_STYLE, WINEVENT_OUTOFCONTEXT, WS_OVERLAPPED,
    };
    use windows::core::{PWSTR, w};

    use super::*;

    const WAIT: Duration = Duration::from_secs(10);

    fn lock_of(pid: u32) -> AppLock {
        AppLock::new(pid, 0, Arc::new(new_mutex().expect("a mutex")))
    }

    /// Runs `call` as a connection to `hwnd` with `lock` as this thread's
    /// lock, and answers how many times it took a lock.
    fn locks_taken_connecting(lock: &AppLock, hwnd: isize) -> u32 {
        let before = thread_lock();
        set_thread_lock(Some(lock.clone()));
        let _ = crate::calls::take_locks();
        guarded_window("a test's connection", hwnd, || {});
        let taken = crate::calls::take_locks();
        set_thread_lock(before);
        taken
    }

    #[test]
    fn a_guarded_call_waits_while_another_thread_holds_the_lock() {
        let lock = lock_of(std::process::id());
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let holder = {
            let lock = lock.clone();
            let order = Arc::clone(&order);
            std::thread::spawn(move || {
                guarded_by(&lock, "the holder's call", || {
                    held_tx.send(()).expect("the test waits");
                    release_rx.recv().expect("the test releases");
                    order.lock().expect("the order").push("holder ended");
                });
            })
        };
        held_rx
            .recv_timeout(WAIT)
            .expect("the holder holds the lock");
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let waiter = {
            let lock = lock.clone();
            let order = Arc::clone(&order);
            std::thread::spawn(move || {
                let held = Held::take(&lock, "the waiter's call", || {
                    waiting_tx.send(()).expect("the test waits");
                });
                order.lock().expect("the order").push("waiter ran");
                held.taken
            })
        };
        waiting_rx
            .recv_timeout(WAIT)
            .expect("the waiter waits for the lock");
        release_tx.send(()).expect("the holder waits");
        holder.join().expect("the holder ended");
        assert_eq!(
            waiter.join().expect("the waiter ended"),
            Taken::AfterWaiting
        );
        assert_eq!(
            *order.lock().expect("the order"),
            ["holder ended", "waiter ran"]
        );
    }

    #[test]
    fn a_lock_whose_holder_ended_holding_it_is_taken_and_reported_abandoned() {
        let lock = lock_of(std::process::id());
        {
            let lock = lock.clone();
            std::thread::spawn(move || {
                // Taken and never released: the thread ends holding it.
                std::mem::forget(Held::take(&lock, "the holder's call", || {}));
            })
            .join()
            .expect("the holder ended");
        }
        let held = Held::take(&lock, "the next call", || {});
        assert_eq!(held.taken, Taken::Abandoned);
    }

    #[test]
    fn a_connection_to_a_window_of_the_locks_own_process_takes_the_lock() {
        // SAFETY: a predefined class, no parent, menu, or creation data.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("the lock's own window"),
                WS_OVERLAPPED,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .expect("a window of the test's own");
        let taken = locks_taken_connecting(&lock_of(std::process::id()), window.0 as isize);
        // SAFETY: the window this thread created.
        let _ = unsafe { DestroyWindow(window) };
        assert_eq!(taken, 1);
    }

    #[test]
    fn a_console_window_takes_its_console_hosts_lock_not_its_owners_nor_another_processes() {
        let console = Console::start();
        let mut owner = 0u32;
        // SAFETY: `owner` is a local; the window is the console's.
        unsafe { GetWindowThreadProcessId(HWND(console.window as *mut _), Some(&raw mut owner)) };
        assert_ne!(owner, console.host, "the console's client owns its window");
        assert_eq!(provider_process(console.window), Some(console.host));
        assert_eq!(
            locks_taken_connecting(&lock_of(console.host), console.window),
            1
        );
        assert_eq!(locks_taken_connecting(&lock_of(owner), console.window), 0);
        // A window whose provider is another process than the lock's.
        assert_eq!(
            locks_taken_connecting(&lock_of(std::process::id()), console.window),
            0
        );
    }

    thread_local! {
        /// The title a console's window is waited for with, and the window
        /// once its name changed to it.
        static TITLE: RefCell<String> = const { RefCell::new(String::new()) };
        static RENAMED: Cell<isize> = const { Cell::new(0) };
    }

    /// The `WinEvent` callback: notes the window whose name changed to the
    /// title waited for.
    unsafe extern "system" fn renamed(
        _: HWINEVENTHOOK,
        _: u32,
        hwnd: HWND,
        object: i32,
        _: i32,
        _: u32,
        _: u32,
    ) {
        if object != OBJID_WINDOW.0 {
            return;
        }
        let mut text = [0u16; 128];
        // SAFETY: the buffer outlives the call, which writes at most its
        // length.
        let length = unsafe { GetWindowTextW(hwnd, &mut text) };
        let text = String::from_utf16_lossy(&text[..usize::try_from(length).unwrap_or(0)]);
        if TITLE.with(|title| *title.borrow() == text) {
            RENAMED.with(|renamed| renamed.set(hwnd.0 as isize));
        }
    }

    /// A console host the test starts, `conhost.exe` running `cmd.exe` in
    /// a hidden console window, in a job that ends both when dropped.
    struct Console {
        host: u32,
        window: isize,
        _job: OwnedHandle,
    }

    impl Console {
        /// Starts the console and waits for the evidence that its client
        /// runs in it: the window's title changed by the client's `title`
        /// command, raised as a `WinEvent` from the console host.
        fn start() -> Self {
            let title = format!("verbatim-lock-test-{}", std::process::id());
            TITLE.with(|cell| cell.borrow_mut().clone_from(&title));
            RENAMED.with(|renamed| renamed.set(0));
            // SAFETY: an unnamed job with no security attributes.
            let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.expect("a job");
            // SAFETY: just returned, owned by nothing else.
            let job = unsafe { OwnedHandle::from_raw_handle(job.0) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // SAFETY: `limits` is the structure the class names, sized.
            unsafe {
                SetInformationJobObject(
                    HANDLE(job.as_raw_handle()),
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>()).unwrap_or(0),
                )
            }
            .expect("the job's limit");
            let mut command: Vec<u16> = format!("conhost.exe cmd.exe /k title {title}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let startup = STARTUPINFOW {
                cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
                dwFlags: STARTF_USESHOWWINDOW,
                wShowWindow: u16::try_from(SW_HIDE.0).unwrap_or(0),
                ..Default::default()
            };
            let mut info = PROCESS_INFORMATION::default();
            // SAFETY: a writable, nul-terminated command line; no handles
            // are inherited, no standard handles given, and no console of
            // this process's, so the console host opens a window of its
            // own.
            unsafe {
                CreateProcessW(
                    None,
                    Some(PWSTR(command.as_mut_ptr())),
                    None,
                    None,
                    false,
                    CREATE_SUSPENDED | DETACHED_PROCESS,
                    None,
                    None,
                    &raw const startup,
                    &raw mut info,
                )
            }
            .expect("the console host starts");
            // SAFETY: just returned, owned here.
            let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess.0) };
            // SAFETY: just returned, owned here.
            let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread.0) };
            // SAFETY: both handles are open.
            unsafe {
                AssignProcessToJobObject(
                    HANDLE(job.as_raw_handle()),
                    HANDLE(process.as_raw_handle()),
                )
            }
            .expect("the console host joins the job");
            // SAFETY: an out-of-context hook on this thread, for the
            // console host's events alone; the callback is a plain function.
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_OBJECT_NAMECHANGE,
                    EVENT_OBJECT_NAMECHANGE,
                    None,
                    Some(renamed),
                    info.dwProcessId,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            // SAFETY: the suspended thread's handle, open.
            unsafe { ResumeThread(HANDLE(thread.as_raw_handle())) };
            let deadline = Instant::now() + WAIT;
            while RENAMED.with(Cell::get) == 0 {
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(
                    !left.is_zero(),
                    "the console's window took its client's title"
                );
                // SAFETY: no handles; wakes for a message or at the deadline.
                unsafe {
                    MsgWaitForMultipleObjects(
                        None,
                        false,
                        u32::try_from(left.as_millis()).unwrap_or(u32::MAX),
                        QS_ALLINPUT,
                    )
                };
                let mut msg = MSG::default();
                // SAFETY: `msg` is a local each call writes.
                while unsafe { PeekMessageW(&raw mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                    // SAFETY: the message just retrieved.
                    unsafe { DispatchMessageW(&raw const msg) };
                }
            }
            // SAFETY: the hook set above, removed once.
            let _ = unsafe { UnhookWinEvent(hook) };
            Self {
                host: info.dwProcessId,
                window: RENAMED.with(Cell::get),
                _job: job,
            }
        }
    }
}
