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
//! and nothing else is done while the lock is held. The listener's
//! arbitration probe is not guarded: it is the raw probe
//! ([`crate::raw_probe`]), a window message rather than a connection, which
//! answered correctly beside a queued remote operation.
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
    DUPLICATE_HANDLE_OPTIONS, DuplicateHandle, FILETIME, HANDLE, HANDLE_FLAG_INHERIT, HANDLE_FLAGS,
    SetHandleInformation, WAIT_ABANDONED, WAIT_EVENT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetProcessTimes, INFINITE, MUTEX_MODIFY_STATE, OpenProcess,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ReleaseMutex,
    SYNCHRONIZATION_SYNCHRONIZE, WaitForSingleObject,
};
use windows::core::PCWSTR;

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
/// ([`crate::provider_process`]) is the lock's application, and unguarded
/// otherwise: a connection to a window of another application takes no
/// lock (section 20.9). `name` names the call in the log.
pub fn guarded_window<T>(name: &'static str, hwnd: isize, call: impl FnOnce() -> T) -> T {
    match current() {
        Some(lock) if crate::provider::provider_process(hwnd) == Some(lock.pid) => {
            guarded_by(&lock, name, call)
        }
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
    use std::sync::mpsc;
    use std::time::Duration;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowThreadProcessId, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    use super::*;
    use crate::provider_process;
    use verbatim_test_console::Console;

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
}
