//! A console the tests start: `conhost.exe` running PowerShell in a hidden
//! console window, for the tests of a console window's provider process in
//! this crate and, through the `test-console` feature, in others.

use std::cell::{Cell, RefCell};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
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
    DispatchMessageW, EVENT_OBJECT_NAMECHANGE, GetWindowTextW, MSG, MsgWaitForMultipleObjects,
    OBJID_WINDOW, PM_REMOVE, PeekMessageW, QS_ALLINPUT, SW_HIDE, WINEVENT_OUTOFCONTEXT,
};
use windows::core::{PCWSTR, PWSTR};

/// How long the console's client is waited for.
const WAIT: Duration = Duration::from_secs(10);

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

/// A console host the test starts, `conhost.exe` running PowerShell in
/// a hidden console window, in a job that ends both when dropped.
pub struct Console {
    /// The console host's process id.
    pub host: u32,
    /// The console's window.
    pub window: isize,
    _job: OwnedHandle,
}

impl Console {
    /// Starts the console and waits for the evidence that its client
    /// runs in it: the window's title changed by the client, raised as a
    /// `WinEvent` from the console host. The client is PowerShell, whose
    /// `WindowTitle` sets the title exactly as given; `cmd.exe`'s `title`
    /// command prefixes "Administrator: " when elevated, as GitHub's
    /// runner is, so the title waited for never came there.
    ///
    /// # Panics
    ///
    /// When the console cannot be started, or its client has not set its
    /// title within ten seconds.
    #[must_use]
    pub fn start() -> Self {
        let title = format!("verbatim-console-test-{}", std::process::id());
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
        let mut command: Vec<u16> = format!(
            "conhost.exe powershell.exe -NoLogo -NoProfile -NoExit -Command \
             $Host.UI.RawUI.WindowTitle='{title}'"
        )
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
