//! The desktop's state, for a test to establish and check what it starts
//! from and to leave the desktop as it found it:
//! [`Request::ForegroundInfo`](crate::protocol::Request::ForegroundInfo),
//! [`Request::CloseWindows`](crate::protocol::Request::CloseWindows),
//! [`Request::MinimizeAll`](crate::protocol::Request::MinimizeAll),
//! [`Request::SetForeground`](crate::protocol::Request::SetForeground), and
//! the conditions
//! [`Request::WaitForWindow`](crate::protocol::Request::WaitForWindow)
//! waits for.
//!
//! NVDA's system tests check before every assertion that the window they
//! opened is the foreground window, failing with the foreground window's
//! title and the open windows' titles when it is not, and close the windows
//! they opened with an ordinary close request, waiting for them to go. These
//! are the same operations for Verbatim's suite.

use std::ffi::c_void;
use std::mem::size_of;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetForegroundWindow,
    GetWindowLongPtrW, GetWindowTextW, GetWindowThreadProcessId, IsHungAppWindow, IsIconic,
    IsWindowVisible, PostMessageW, SMTO_ABORTIFHUNG, SW_RESTORE, SW_SHOWMINNOACTIVE,
    SendMessageTimeoutW, SetForegroundWindow, ShowWindowAsync, WM_CLOSE, WM_COMMAND,
    WS_EX_NOACTIVATE, WS_MINIMIZEBOX,
};
use windows::core::{BOOL, PWSTR, w};

use crate::protocol::{ForegroundInfo, WindowCondition, WindowInfo};
use crate::wait;

/// The foreground window and every visible, titled top-level window,
/// owned ones such as dialogs included, each with its title, class, and
/// image name.
#[must_use]
pub fn foreground_info() -> ForegroundInfo {
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    ForegroundInfo {
        foreground: (!foreground.is_invalid()).then(|| window_info(foreground)),
        windows: top_level_windows().into_iter().map(window_info).collect(),
    }
}

/// Sends a close request to every shown top-level window whose title
/// contains `title_contains` ([`shown_top_level_windows`]), and waits up
/// to `timeout` for them to go, on window events ([`wait::until`]).
/// Returns how many were still shown when it gave up; zero means all
/// closed. A window that is closed and cloaked, as the Settings app keeps
/// its closed window, has gone from the desktop.
#[must_use]
pub fn close_windows(title_contains: &str, timeout: Duration) -> u32 {
    let matching = || -> Vec<HWND> {
        shown_top_level_windows()
            .into_iter()
            .filter(|&window| window_text(window).contains(title_contains))
            .collect()
    };
    for window in matching() {
        // Checked again just before the close: a window destroyed since the
        // enumeration could have its handle reused by an unrelated one.
        if !window_text(window).contains(title_contains) {
            continue;
        }
        // SAFETY: PostMessageW carries no pointer and tolerates a window
        // that has since gone; the title was checked just above.
        unsafe {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    wait::until(|| matching().is_empty(), timeout);
    u32::try_from(matching().len()).unwrap_or(u32::MAX)
}

/// Sends a close request to `window` while it is shown
/// ([`shown_top_level_windows`]) and still owned by process `pid` with
/// class `class`, and waits up to `timeout`, on window events
/// ([`wait::until`]), for it to go. Its title is not checked: a window can
/// change its title while it lives, as a File Explorer window does when it
/// navigates. The process and class, which never change while the window
/// lives, are checked instead, so a handle Windows has given another window
/// since is never closed. Returns whether it has gone; a window that is
/// closed and cloaked has, as for [`close_windows`].
#[must_use]
pub fn close_window(window: u64, pid: u32, class: &str, timeout: Duration) -> bool {
    let window = HWND(usize::try_from(window).unwrap_or(0) as *mut c_void);
    let is_open = || {
        let mut owner = 0u32;
        // SAFETY: tolerates any handle, writing 0 for an invalid one.
        unsafe {
            GetWindowThreadProcessId(window, Some(&raw mut owner));
        }
        owner == pid && class_name(window) == class && shown_top_level_windows().contains(&window)
    };
    if is_open() {
        // SAFETY: PostMessageW carries no pointer and tolerates a window
        // that has since gone; its owner and class were checked just above.
        unsafe {
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    wait::until(|| !is_open(), timeout);
    !is_open()
}

/// Whether `condition` holds on the desktop now.
#[must_use]
pub fn holds(condition: &WindowCondition) -> bool {
    match condition {
        WindowCondition::Foreground {
            title_contains,
            unsaved,
        } => foreground_info().foreground.is_some_and(|window| {
            !window.cloaked
                && window.title.contains(title_contains.as_str())
                && unsaved.is_none_or(|unsaved| window.title.starts_with('*') == unsaved)
        }),
        WindowCondition::Present { title_contains } => shown_top_level_windows()
            .into_iter()
            .any(|window| window_text(window).contains(title_contains.as_str())),
        WindowCondition::Absent { title_contains } => !shown_top_level_windows()
            .into_iter()
            .any(|window| window_text(window).contains(title_contains.as_str())),
        WindowCondition::AllMinimized => shown_top_level_windows()
            .into_iter()
            .all(|window| !minimizable(window) || is_minimized(window)),
    }
}

/// Waits up to `timeout` for `condition`, on window events
/// ([`wait::until`]), and returns whether it held and the desktop then.
#[must_use]
pub fn wait_for(condition: &WindowCondition, timeout: Duration) -> (bool, ForegroundInfo) {
    let met = wait::until(|| holds(condition), timeout);
    (met, foreground_info())
}

/// Minimizes every window as the taskbar's Show Desktop command does, and
/// gives the desktop the foreground: the state every scenario starts from.
/// Minimizing leaves the foreground on the window that had it, minimized,
/// so the desktop's window, Program Manager, is then brought forward with
/// `SetForegroundWindow`, injecting no input. Once (2026-10-08) a Notepad
/// document a scenario had opened just before stayed restored after the
/// taskbar's command and the run failed; the suspected cause, unconfirmed,
/// is that the command skips a window the taskbar has not taken in yet.
/// So the command is sent and waited for, and only each window still
/// restored once the taskbar has handled it is logged, at info, with its
/// class and how long its process has run, and then minimized directly
/// (`ShowWindowAsync`, which never waits on the window's thread), without
/// activating another window (`SW_SHOWMINNOACTIVE`; `SW_MINIMIZE` activates
/// the next top-level window in Z order, which can be the taskbar). Waits up to `timeout` for
/// every window that can be minimized to be ([`WindowCondition::AllMinimized`]),
/// and then for the desktop to hold the foreground. Returns whether each
/// held, the second false when the first did not, as the desktop is then
/// not given the foreground, and the desktop as it was.
#[must_use]
pub fn minimize_all(timeout: Duration) -> (bool, bool, ForegroundInfo) {
    /// The taskbar's command that minimizes every window.
    const MINIMIZE_ALL: usize = 419;
    // SAFETY: looks a window up by class; no pointer is kept.
    if let Ok(taskbar) = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) } {
        // SAFETY: a command message carrying no pointer; it returns once
        // the taskbar has handled it, or after the limit if the taskbar
        // does not answer.
        unsafe {
            let _ = SendMessageTimeoutW(
                taskbar,
                WM_COMMAND,
                WPARAM(MINIMIZE_ALL),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                TASKBAR_LIMIT_MS,
                None,
            );
        }
    }
    for window in shown_top_level_windows()
        .into_iter()
        .filter(|&window| minimizable(window) && !is_minimized(window))
    {
        let info = window_info(window);
        tracing::info!(
            class = info.class,
            image = info.image,
            title = info.title,
            process_age_ms = process_age_ms(info.pid),
            "a window was still restored once the taskbar's Minimize All was handled; minimized directly"
        );
        // SAFETY: tolerates any handle; the minimize is posted to the
        // window's thread, and its answer is not waited for.
        let _ = unsafe { ShowWindowAsync(window, SW_SHOWMINNOACTIVE) };
    }
    let (minimized, desktop) = wait_for(&WindowCondition::AllMinimized, timeout);
    if !minimized {
        return (false, false, desktop);
    }
    // SAFETY: looks a window up by class; no pointer is kept.
    let Ok(desktop_window) = (unsafe { FindWindowW(w!("Progman"), None) }) else {
        return (true, false, foreground_info());
    };
    // SAFETY: a window just found; a stale one fails.
    let _ = unsafe { SetForegroundWindow(desktop_window) };
    let (in_front, desktop) = wait_for(
        &WindowCondition::Foreground {
            title_contains: window_text(desktop_window),
            unsaved: None,
        },
        timeout,
    );
    (true, in_front, desktop)
}

/// Brings `window` to the foreground as clicking its taskbar button does,
/// injecting no input, and returns whether it is the foreground window
/// afterwards.
///
/// A minimized window is restored and then set as the foreground with
/// `SetForegroundWindow`; both the minimize and the restore are posted to
/// the window's thread (`ShowWindowAsync`), so a hung window cannot block
/// the agent, and each is waited for on its evidence, the window's
/// minimized state, up to [`MINIMIZE_LIMIT`]. A request that is not
/// posted, as for a destroyed window, fails the call at once. A request
/// whose wait fails stays posted: a window that answers later carries it
/// out then, during whatever comes next, so the failure is logged with
/// that consequence named, and the next call reads the window's state
/// afresh before doing anything. A restored window that is not already in
/// front is minimized first, without activating another window
/// (`SW_SHOWMINNOACTIVE`), and restored once it is. `SW_MINIMIZE` would
/// activate the next top-level window in Z order: in a local run on
/// 2026-10-17 that was the taskbar, whose Start button Verbatim then
/// announced. `SetForegroundWindow` alone succeeds only while the agent
/// may set the foreground, which it may only after injecting the last
/// input (`docs/tooling/troubleshooting.md`, "Windows' foreground lock
/// keeps launched applications behind"). msinfo32 is such a window: it
/// ignores the minimized show state it is launched with and opens restored
/// and inactive (found 2026-10-09). Such a window is logged, at info.
///
/// The answer is the window being the foreground. When
/// `SetForegroundWindow` refuses and the window is not in front, the call
/// fails at once; when it accepts, the window is waited for on window
/// events up to [`MINIMIZE_LIMIT`], since with the restore posted rather
/// than handled before the call, the switch completes on the window's
/// thread after the call returns (`conhost_leave_flood` on 2026-10-26:
/// accepted, and not yet in front as the call returned).
///
/// Each call logs, at info, how long each step took from the call's
/// start: the minimize requested, the minimize observed, the restore
/// observed, and `SetForegroundWindow`'s answer, and whether the window
/// was then in front, so a slow switch can be explained from the agent's
/// log.
#[must_use]
pub fn set_foreground(window: u64) -> bool {
    let started = Instant::now();
    let mut steps = SwitchSteps::default();
    let window = HWND(usize::try_from(window).unwrap_or(0) as *mut c_void);
    // SAFETY: GetForegroundWindow has no preconditions.
    let in_front = unsafe { GetForegroundWindow() } == window;
    if !in_front && !is_minimized(window) {
        let info = window_info(window);
        tracing::info!(
            class = info.class,
            image = info.image,
            title = info.title,
            "a window to bring forward was restored and not in front; minimized before it is restored"
        );
        // SAFETY: tolerates any handle; the minimize is posted to the
        // window's thread, and false means it was not posted.
        let posted = unsafe { ShowWindowAsync(window, SW_SHOWMINNOACTIVE) }.as_bool();
        steps.minimize_requested = Some(started.elapsed());
        if !posted {
            tracing::info!("the minimize was not posted to the window");
            steps.log(window, started, false, false);
            return false;
        }
        let minimized = wait::until(|| is_minimized(window), MINIMIZE_LIMIT);
        steps.minimized = minimized.then(|| started.elapsed());
        if !minimized {
            tracing::warn!(
                "the window was not minimized in time; the posted minimize stays queued and \
                 minimizes it whenever the window answers"
            );
            steps.log(window, started, false, false);
            return false;
        }
    }
    if is_minimized(window) {
        // SAFETY: tolerates any handle; the restore is posted to the
        // window's thread, and false means it was not posted.
        let posted = unsafe { ShowWindowAsync(window, SW_RESTORE) }.as_bool();
        if !posted {
            tracing::info!("the restore was not posted to the window");
            steps.log(window, started, false, false);
            return false;
        }
        let restored = wait::until(|| !is_minimized(window), MINIMIZE_LIMIT);
        steps.restored = restored.then(|| started.elapsed());
        if !restored {
            tracing::warn!(
                "the window was not restored in time; the posted restore stays queued and \
                 restores it whenever the window answers"
            );
            steps.log(window, started, false, false);
            return false;
        }
    }
    // SAFETY: tolerates any handle; a stale one fails.
    let accepted = unsafe { SetForegroundWindow(window) }.as_bool();
    steps.set_foreground = Some(started.elapsed());
    // SAFETY: GetForegroundWindow has no preconditions.
    let in_front_now = || unsafe { GetForegroundWindow() } == window;
    let in_front = if accepted {
        wait::until(in_front_now, MINIMIZE_LIMIT)
    } else {
        in_front_now()
    };
    steps.log(window, started, accepted, in_front);
    in_front
}

/// When each step of one [`set_foreground`] call finished, measured from
/// the call's start; `None` for a step the call did not take.
#[derive(Default)]
struct SwitchSteps {
    minimize_requested: Option<Duration>,
    minimized: Option<Duration>,
    restored: Option<Duration>,
    set_foreground: Option<Duration>,
}

impl SwitchSteps {
    /// Logs the steps for `window`, by its title, whether
    /// `SetForegroundWindow` accepted it (false when it was not called),
    /// and whether it was then in front.
    fn log(&self, window: HWND, started: Instant, accepted: bool, in_front: bool) {
        tracing::info!(
            title = window_text(window),
            minimize_requested = ?self.minimize_requested,
            minimized = ?self.minimized,
            restored = ?self.restored,
            set_foreground = ?self.set_foreground,
            accepted,
            in_front,
            total = ?started.elapsed(),
            "bring forward steps"
        );
    }
}

/// How long [`set_foreground`] waits for a window it minimized to be
/// minimized, for one it restored to be restored, and for the window to
/// be in front: well within the client's read timeout.
const MINIMIZE_LIMIT: Duration = Duration::from_secs(5);

/// Whether `window` has a minimize box, so Show Desktop minimizes it.
fn minimizable(window: HWND) -> bool {
    // SAFETY: reads a window's style; 0 for an invalid handle.
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) };
    u32::try_from(style).is_ok_and(|style| style & WS_MINIMIZEBOX.0 != 0)
}

/// Whether `window` has the `WS_EX_NOACTIVATE` style, so Windows never
/// brings it to the foreground.
fn no_activate(window: HWND) -> bool {
    // SAFETY: reads a window's extended style; 0 for an invalid handle.
    let style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    u32::try_from(style).is_ok_and(|style| style & WS_EX_NOACTIVATE.0 != 0)
}

/// Whether `window` is cloaked: kept by the window manager but not shown,
/// as a suspended app's window is.
fn is_cloaked(window: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: the out-parameter is a u32, the size DWMWA_CLOAKED writes.
    let read = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            u32::try_from(size_of::<u32>()).unwrap_or(4),
        )
    };
    read.is_ok() && cloaked != 0
}

/// Whether `window` is minimized.
fn is_minimized(window: HWND) -> bool {
    // SAFETY: tolerates any handle.
    unsafe { IsIconic(window) }.as_bool()
}

/// What a test is told of `window`.
pub(crate) fn window_info(window: HWND) -> WindowInfo {
    let mut pid = 0u32;
    // SAFETY: tolerates any handle, writing 0 for an invalid one.
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
    WindowInfo {
        window: window.0 as u64,
        pid,
        title: window_text(window),
        class: class_name(window),
        image: image_name(pid).unwrap_or_default(),
        cloaked: is_cloaked(window),
        minimized: is_minimized(window),
        // SAFETY: tolerates any handle.
        hung: unsafe { IsHungAppWindow(window) }.as_bool(),
        no_activate: no_activate(window),
        // SAFETY: tolerates any handle.
        hidden: !unsafe { IsWindowVisible(window) }.as_bool(),
    }
}

/// The shown top-level windows: [`top_level_windows`] without the cloaked
/// ones, which the window manager keeps but does not show, as a suspended
/// app's window or the Settings app's closed window. Every condition and
/// close request judges windows by these, so a cloaked window is absent
/// to all of them alike; the report ([`foreground_info`]) still lists it,
/// marked cloaked.
pub(crate) fn shown_top_level_windows() -> Vec<HWND> {
    top_level_windows()
        .into_iter()
        .filter(|&window| !is_cloaked(window))
        .collect()
}

/// The visible, titled top-level windows, in Z order. Owned windows are
/// included: a message box or dialog shown on its own, with its owner
/// hidden, as the "System Properties" message box GitHub's runner image
/// shows at logon is, would otherwise be on screen and in no report.
fn top_level_windows() -> Vec<HWND> {
    all_top_level_windows()
        .into_iter()
        .filter(|&window| {
            // SAFETY: tolerates any handle.
            unsafe { IsWindowVisible(window) }.as_bool() && !window_text(window).is_empty()
        })
        .collect()
}

/// Every top-level window, shown or not, titled or not, in Z order: what a
/// scenario's starting windows are recorded from
/// ([`crate::scenario_windows`]).
pub(crate) fn all_top_level_windows() -> Vec<HWND> {
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the vector passed below, alive for the call.
        let windows = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        windows.push(window);
        BOOL(1)
    }
    let mut windows: Vec<HWND> = Vec::new();
    // SAFETY: `visit` only pushes into the vector, which outlives the
    // synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(
            Some(visit),
            LPARAM((&raw mut windows).cast::<c_void>() as isize),
        );
    }
    windows
}

fn window_text(window: HWND) -> String {
    let mut buffer = [0u16; 512];
    // SAFETY: writes at most the buffer's length; 0 for a window with no
    // text or an invalid handle.
    let length = unsafe { GetWindowTextW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

pub(crate) fn class_name(window: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: writes at most the buffer's length; 0 for an invalid handle.
    let length = unsafe { GetClassNameW(window, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(length).unwrap_or(0)])
}

/// How long the taskbar is given to handle Minimize All.
const TASKBAR_LIMIT_MS: u32 = 5_000;

/// How long process `pid` has run, in milliseconds; `None` when it cannot
/// be read.
fn process_age_ms(pid: u32) -> Option<u64> {
    // SAFETY: a query-only open that fails safely; the handle is closed
    // below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `handle` is open with query access; the out-parameters are
    // locals.
    let read = unsafe {
        GetProcessTimes(
            handle,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    read.ok()?;
    // SAFETY: no preconditions.
    let now = unsafe { GetSystemTimeAsFileTime() };
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    // Filetime ticks are 100 nanoseconds.
    Some(ticks(now).saturating_sub(ticks(created)) / 10_000)
}

/// `pid`'s executable file name, such as `notepad.exe`.
fn image_name(pid: u32) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: a query-only open that fails safely; the handle is closed
    // below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    // SAFETY: `handle` is open with query access; the buffer outlives the
    // call, which writes at most `length` units.
    let read = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(handle) };
    read.ok()?;
    let path = String::from_utf16_lossy(&buffer[..usize::try_from(length).ok()?]);
    Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, DESKTOP_CREATEWINDOW,
        DESKTOP_ENUMERATE, DESKTOP_READOBJECTS, DESKTOP_WRITEOBJECTS, HDESK, SetThreadDesktop,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::core::{HSTRING, PCWSTR, w};

    #[test]
    fn closing_windows_no_window_matches_reports_none_left() {
        assert_eq!(
            close_windows(
                "verbatim-agent-test-no-window-has-this-title",
                Duration::from_millis(50)
            ),
            0
        );
    }

    /// The class of the test's windows.
    const CLASS: PCWSTR = w!("VerbatimAgentDesktopTest");

    /// The test windows' procedure: the default one.
    unsafe extern "system" fn window_procedure(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: the arguments Windows passed this window procedure.
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    /// Makes a top-level window of the test's class on this thread's
    /// desktop.
    fn make_window(title: PCWSTR, style: WINDOW_STYLE, owner: Option<HWND>) -> HWND {
        // SAFETY: the class is registered, the strings are static, and the
        // owner, when there is one, is a live window of this thread.
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS,
                title,
                style,
                0,
                0,
                100,
                100,
                owner,
                None,
                None,
                None,
            )
        }
        .expect("creates a window")
    }

    /// The report lists exactly the visible, titled top-level windows,
    /// owned ones included, in Z order, where an owned window is above its
    /// owner. The windows are made on a desktop of the test's own, which
    /// holds nothing else and is never shown, so what is listed does not
    /// depend on the machine and the user's desktop is left alone.
    #[test]
    fn the_foreground_report_lists_visible_windows() {
        let access = DESKTOP_CREATEWINDOW.0
            | DESKTOP_ENUMERATE.0
            | DESKTOP_READOBJECTS.0
            | DESKTOP_WRITEOBJECTS.0;
        let name = format!("verbatim-agent-test-{}", std::process::id());
        // SAFETY: a valid name and no device, mode, or security attributes.
        let desktop = unsafe {
            CreateDesktopW(
                &HSTRING::from(name),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                access,
                None,
            )
        }
        .expect("creates a desktop");
        let handle = desktop.0 as usize;
        let (info, (listed, owned)) = thread::spawn(move || {
            // SAFETY: this new thread has no windows or hooks yet, and the
            // desktop stays open until the thread has ended.
            unsafe { SetThreadDesktop(HDESK(handle as *mut c_void)) }
                .expect("moves to the test's desktop");
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_procedure),
                lpszClassName: CLASS,
                ..WNDCLASSW::default()
            };
            // SAFETY: a class with a valid window procedure and a static name.
            assert_ne!(unsafe { RegisterClassW(&raw const class) }, 0, "registers");
            let listed = make_window(w!("Listed"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, None);
            let owned = make_window(w!("Owned"), WS_POPUP | WS_VISIBLE, Some(listed));
            let windows = [
                listed,
                make_window(PCWSTR::null(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, None),
                make_window(w!("Hidden"), WS_OVERLAPPEDWINDOW, None),
                owned,
            ];
            let info = foreground_info();
            for window in windows.into_iter().rev() {
                // SAFETY: a window this thread made.
                unsafe { DestroyWindow(window) }.expect("destroys the window");
            }
            (info, (listed.0 as u64, owned.0 as u64))
        })
        .join()
        .expect("the window thread ends");
        // SAFETY: the desktop opened above; its only thread has ended.
        unsafe { CloseDesktop(desktop) }.expect("closes the desktop");

        let image = std::env::current_exe()
            .expect("this test's executable")
            .file_name()
            .expect("a file name")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            info,
            ForegroundInfo {
                foreground: None,
                windows: vec![
                    WindowInfo {
                        window: owned,
                        pid: std::process::id(),
                        title: "Owned".to_owned(),
                        class: "VerbatimAgentDesktopTest".to_owned(),
                        image: image.clone(),
                        cloaked: false,
                        minimized: false,
                        hung: false,
                        no_activate: false,
                        hidden: false,
                    },
                    WindowInfo {
                        window: listed,
                        pid: std::process::id(),
                        title: "Listed".to_owned(),
                        class: "VerbatimAgentDesktopTest".to_owned(),
                        image,
                        cloaked: false,
                        minimized: false,
                        hung: false,
                        no_activate: false,
                        hidden: false,
                    },
                ],
            }
        );
    }
}
