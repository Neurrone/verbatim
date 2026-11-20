//! A console the tests start: `conhost.exe` running the crate's own client,
//! `verbatim-console-client`, in a hidden console window, optionally with a
//! hidden window of the client's own on another thread, for the tests of a
//! console window's provider process in `verbatim-win32` and
//! `verbatim-outpost`. A dev-dependency only, so the
//! crates it serves are built once for their tests and their builds alike.
//!
//! The console runs on a desktop made for it, and the test on a thread of
//! that desktop, because a console host started on the interactive
//! desktop takes the foreground even with its window hidden, which breaks
//! an end-to-end run using that desktop.

use std::cell::{Cell, RefCell};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GENERIC_ALL, HANDLE, HWND, STILL_ACTIVE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, GetProcessWindowStation,
    GetUserObjectInformationW, HDESK, SetThreadDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, DETACHED_PROCESS, GetExitCodeProcess, OpenProcess,
    PROCESS_INFORMATION, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW, ResumeThread, STARTF_USESHOWWINDOW, STARTUPINFOW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_NAMECHANGE, FindWindowW, GetWindowTextW, MSG,
    MsgWaitForMultipleObjects, OBJID_WINDOW, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_HIDE,
    WINEVENT_OUTOFCONTEXT,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// How long the console's client is waited for.
const WAIT: Duration = Duration::from_secs(10);

/// The console's client, which the build script compiles
/// (`src/bin/verbatim-console-client.rs`).
const CLIENT: &str = concat!(env!("OUT_DIR"), "\\verbatim-console-client.exe");

/// Numbers this process's desktops, so each is new.
static DESKTOPS: AtomicUsize = AtomicUsize::new(0);

/// A desktop made for one console, in this process's window station,
/// closed once the test's thread on it has ended; the system destroys it
/// when nothing uses it any more.
struct Desktop {
    handle: HDESK,
    /// Its name after its window station's, as a process is started on it.
    path: Vec<u16>,
}

impl Desktop {
    fn new() -> Self {
        let name = format!(
            "verbatim-console-test-{}-{}",
            std::process::id(),
            DESKTOPS.fetch_add(1, Ordering::Relaxed)
        );
        // SAFETY: a nul-terminated name, no device or mode, and no
        // security attributes: the desktop gets this user's default
        // access, and its handle is not inheritable.
        let handle = unsafe {
            CreateDesktopW(
                &HSTRING::from(name.as_str()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                GENERIC_ALL.0,
                None,
            )
        }
        .unwrap_or_else(|error| panic!("the desktop {name} could not be made: {error}"));
        let path = format!("{}\\{name}", window_station_name())
            .encode_utf16()
            .chain(Some(0))
            .collect();
        Self { handle, path }
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        // SAFETY: the handle `new` made, closed once, after the thread
        // that used it has ended.
        let closed = unsafe { CloseDesktop(self.handle) };
        if let Err(error) = closed
            && !std::thread::panicking()
        {
            panic!("the console's desktop could not be closed: {error}");
        }
    }
}

/// The name of this process's window station.
fn window_station_name() -> String {
    // SAFETY: no preconditions; the handle is not to be closed.
    let station = unsafe { GetProcessWindowStation() }
        .unwrap_or_else(|error| panic!("this process has no window station: {error}"));
    let mut name = [0u16; 256];
    // SAFETY: `name` is writable for the size given, in bytes.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(station.0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            u32::try_from(size_of_val(&name)).unwrap_or(0),
            None,
        )
    }
    .unwrap_or_else(|error| panic!("the window station's name could not be read: {error}"));
    let length = name
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..length])
}

/// Makes a desktop, starts a console on it with `launch` from a new
/// thread of that desktop, and runs `test` with the console on that
/// thread, so the window, `WinEvent` and UIA calls it makes see the
/// console's desktop; a panic in either is raised again here.
fn on_desktop<R: Send>(
    launch: impl FnOnce(&[u16]) -> Console + Send,
    test: impl FnOnce(&Console) -> R + Send,
) -> R {
    let desktop = Desktop::new();
    // The handle as a number, since a handle is not `Send`.
    let handle = desktop.handle.0 as isize;
    let path = desktop.path.as_slice();
    let outcome = std::thread::scope(|scope| {
        scope
            .spawn(move || {
                // SAFETY: a desktop handle that stays open until this
                // thread has ended; the thread is new, so it has no
                // windows or hooks yet.
                unsafe { SetThreadDesktop(HDESK(handle as *mut _)) }
                    .expect("the test's thread moves to the console's desktop");
                let console = launch(path);
                test(&console)
            })
            .join()
    });
    drop(desktop);
    outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

thread_local! {
    /// The title a console's window is waited for with, and the window
    /// once its name changed to it.
    static TITLE: RefCell<String> = const { RefCell::new(String::new()) };
    static RENAMED: Cell<isize> = const { Cell::new(0) };
    /// Every name a window of the console host took while it was waited
    /// for, for the failure message.
    static NAMES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// The `WinEvent` callback: notes every name a window took, and the
/// window whose name changed to the title waited for.
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
    NAMES.with(|names| names.borrow_mut().push(text.clone()));
    if TITLE.with(|title| *title.borrow() == text) {
        RENAMED.with(|renamed| renamed.set(hwnd.0 as isize));
    }
}

/// Each process in `job`, the console host and its client, with its image
/// and whether it is running or the code it exited with, for a failure
/// message.
fn job_processes(job: &OwnedHandle) -> String {
    /// The list's header and room for more processes than the job holds.
    #[repr(C)]
    struct ProcessIds {
        assigned: u32,
        listed: u32,
        ids: [usize; 16],
    }
    let mut list = ProcessIds {
        assigned: 0,
        listed: 0,
        ids: [0; 16],
    };
    // SAFETY: `list` has the layout the class names, with room for 16
    // ids, and its size is given.
    let listed = unsafe {
        QueryInformationJobObject(
            Some(HANDLE(job.as_raw_handle())),
            JobObjectBasicProcessIdList,
            (&raw mut list).cast(),
            u32::try_from(size_of::<ProcessIds>()).unwrap_or(0),
            None,
        )
    };
    if let Err(error) = listed {
        return format!("not listed: {error}");
    }
    let count = usize::try_from(list.listed)
        .unwrap_or(0)
        .min(list.ids.len());
    list.ids[..count]
        .iter()
        .map(|&id| {
            let id = u32::try_from(id).unwrap_or(0);
            // SAFETY: a query of a process by id, no handle inherited.
            let process = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, id) }
            {
                // SAFETY: just returned, owned here.
                Ok(handle) => unsafe { OwnedHandle::from_raw_handle(handle.0) },
                Err(error) => return format!("{id} (not opened: {error})"),
            };
            let mut image = [0u16; 260];
            let mut length = u32::try_from(image.len()).unwrap_or(0);
            // SAFETY: the buffer and its length outlive the call, which
            // writes at most that length.
            let image = match unsafe {
                QueryFullProcessImageNameW(
                    HANDLE(process.as_raw_handle()),
                    PROCESS_NAME_WIN32,
                    PWSTR(image.as_mut_ptr()),
                    &raw mut length,
                )
            } {
                Ok(()) => String::from_utf16_lossy(&image[..usize::try_from(length).unwrap_or(0)]),
                Err(error) => format!("image not read: {error}"),
            };
            let mut code = 0u32;
            // SAFETY: an open process handle and a local the call writes.
            let state =
                match unsafe { GetExitCodeProcess(HANDLE(process.as_raw_handle()), &raw mut code) }
                {
                    Ok(()) if code == STILL_ACTIVE.0.cast_unsigned() => "running".to_owned(),
                    Ok(()) => format!("exited with {code:#x}"),
                    Err(error) => format!("state not read: {error}"),
                };
            format!("{id} {image} {state}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A console host the test starts, `conhost.exe` running
/// `verbatim-console-client` in a hidden console window on a desktop of
/// its own, in a job that ends both when dropped.
pub struct Console {
    /// The console host's process id.
    pub host: u32,
    /// The console's window.
    pub window: isize,
    /// A hidden window of the console's client, made on a thread of its
    /// own, another thread than the one Windows names as the console
    /// window's ([`Console::with_client_window`]); 0 for a
    /// console started with [`Console::with`].
    pub client_window: isize,
    _job: OwnedHandle,
}

impl Console {
    /// Starts the console on a desktop made for it, waits for the
    /// evidence that its client runs in it, and runs `test` with it on a
    /// thread of that desktop, returning what `test` returns. The evidence
    /// is the window's title changed by the client, raised as a `WinEvent`
    /// from the console host, set by the crate's own client
    /// (`books/dev/src/reference/crates/verbatim-test-console.md`,
    /// "verbatim-console-client").
    ///
    /// # Panics
    ///
    /// When the console cannot be started, its client has not set its
    /// title within ten seconds, or `test` panics.
    pub fn with<R: Send>(test: impl FnOnce(&Self) -> R + Send) -> R {
        on_desktop(|desktop| Self::launch(desktop, None), test)
    }

    /// Runs `test` as [`Console::with`] does, the console's client having
    /// a hidden top-level window of its own, made before the client
    /// sets the console's title, so the window exists once the title is
    /// seen and is found by its own title.
    ///
    /// # Panics
    ///
    /// As [`Console::with`], and when the client's window is not found.
    pub fn with_client_window<R: Send>(test: impl FnOnce(&Self) -> R + Send) -> R {
        on_desktop(
            |desktop| {
                let marker = format!("verbatim-console-client-window-{}", std::process::id());
                let mut console = Self::launch(desktop, Some(&marker));
                let marker = HSTRING::from(marker);
                // SAFETY: no class, a nul-terminated title; answers an
                // error when no top-level window of this thread's desktop
                // has it.
                let window = unsafe { FindWindowW(PCWSTR::null(), &marker) }
                    .expect("the client's window was made before the title was set");
                console.client_window = window.0 as isize;
                console
            },
            test,
        )
    }

    /// Starts the console on the desktop named `desktop`, nul-terminated,
    /// which this thread is on, its client making a hidden window titled
    /// `window`, when given, before it sets the console window's title.
    fn launch(desktop: &[u16], window: Option<&str>) -> Self {
        let title = format!("verbatim-console-test-{}", std::process::id());
        TITLE.with(|cell| cell.borrow_mut().clone_from(&title));
        RENAMED.with(|renamed| renamed.set(0));
        NAMES.with(|names| names.borrow_mut().clear());
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
        let mut command: Vec<u16> = format!(
            "conhost.exe \"{CLIENT}\" {title} {}",
            window.unwrap_or_default()
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let mut desktop_path = desktop.to_vec();
        let startup = STARTUPINFOW {
            cb: u32::try_from(size_of::<STARTUPINFOW>()).unwrap_or(0),
            lpDesktop: PWSTR(desktop_path.as_mut_ptr()),
            dwFlags: STARTF_USESHOWWINDOW,
            wShowWindow: u16::try_from(SW_HIDE.0).unwrap_or(0),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: a writable, nul-terminated command line and desktop
        // name, which outlive the call; no handles
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
            AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(process.as_raw_handle()))
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
                "the console's window took its client's title {title:?} within {WAIT:?}; \
                 the console host's windows took the names {:?}; the job's processes: {}",
                NAMES.with(|names| names.borrow().clone()),
                job_processes(&job)
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
            client_window: 0,
            _job: job,
        }
    }
}
