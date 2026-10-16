//! Window queries an outpost makes with local calls only: none of them sends
//! the window a message or calls into another process, so they are safe on
//! any thread and cannot block on a hung application.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GA_PARENT, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, GWL_EXSTYLE,
    GetAncestor, GetForegroundWindow, GetGUIThreadInfo, GetPropW, GetWindowLongW,
    GetWindowThreadProcessId, InternalGetWindowText, IsChild, IsHungAppWindow, IsWindow,
    IsWindowVisible, WS_EX_TOPMOST,
};
use windows::core::{BOOL, HSTRING, s, w};

use verbatim_model::{HIDDEN_FRAME_WINDOW_PROP, Pid, WindowFacts, WindowHandle};

use super::text_reads::CONSOLE_WINDOW_CLASS;
use crate::arbitration::window_class_name;

/// Milliseconds since the Unix epoch, the observation timestamp that anchors
/// the keypress-to-audio latency timeline. Shared with the listener, which
/// stamps facts at observation.
pub(crate) fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

/// Whether `handle` is Core's hidden main frame: it carries the marker
/// property (decision D9; see [`verbatim_model::HIDDEN_FRAME_WINDOW_PROP`])
/// and belongs to Core's process. Any process can set the property on its
/// own windows, so the owner is checked too; Core is this outpost's parent,
/// which launched it.
pub(super) fn window_is_hidden_frame(handle: isize) -> bool {
    let name = HSTRING::from(HIDDEN_FRAME_WINDOW_PROP);
    // SAFETY: GetPropW reads a window property by name; an invalid or
    // property-less window yields a null handle.
    let value = unsafe { GetPropW(hwnd(handle), &name) };
    !value.0.is_null() && core_pid().is_some_and(|core| window_owner(handle).1 == core)
}

/// Core's process id: this process's parent, read once. `None` when it
/// cannot be read.
fn core_pid() -> Option<u32> {
    static CORE: OnceLock<Option<u32>> = OnceLock::new();
    *CORE.get_or_init(parent_pid)
}

/// This process's parent's id, from a snapshot of the running processes.
/// Core's outposts end with Core (they run in its kill-on-close job), so
/// the id cannot have been reused while this process runs under Core.
fn parent_pid() -> Option<u32> {
    // SAFETY: a process snapshot has no preconditions; the handle is owned
    // here and closed below.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let own = std::process::id();
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(size_of::<PROCESSENTRY32W>()).unwrap_or(0),
        ..Default::default()
    };
    let mut parent = None;
    // SAFETY: `snapshot` is the valid snapshot just taken; `entry` has its
    // `dwSize` set, as the call requires.
    let mut has_entry = unsafe { Process32FirstW(snapshot, &raw mut entry) }.is_ok();
    while has_entry {
        if entry.th32ProcessID == own {
            parent = Some(entry.th32ParentProcessID);
            break;
        }
        // SAFETY: as above; the call overwrites `entry` with the next
        // process.
        has_entry = unsafe { Process32NextW(snapshot, &raw mut entry) }.is_ok();
    }
    // SAFETY: the snapshot handle is owned here and closed once.
    let _ = unsafe { CloseHandle(snapshot) };
    parent
}

/// Whether `handle` is Core's hidden main frame or a child window of it. The
/// marker is set on the frame only, so a control hosted in its own child
/// window inside the frame is caught through its top-level window. Core's
/// hidden frame must never be announced: it transits real focus while
/// Verbatim's menu and settings dialog open.
pub(super) fn window_belongs_to_hidden_frame(handle: isize) -> bool {
    if window_is_hidden_frame(handle) {
        return true;
    }
    let root = top_level_of(handle);
    root != 0 && root != handle && window_is_hidden_frame(root)
}

/// The system's foreground window, or 0 when no window has the foreground.
pub(crate) fn foreground_window_handle() -> isize {
    // SAFETY: GetForegroundWindow has no preconditions.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// `handle`'s top-level window (`GetAncestor` with `GA_ROOT`), or 0 for an
/// invalid handle.
pub(crate) fn top_level_of(handle: isize) -> isize {
    // SAFETY: GetAncestor tolerates any handle, returning null for an invalid
    // one.
    unsafe { GetAncestor(hwnd(handle), GA_ROOT) }.0 as isize
}

/// The application an event in `handle` belongs to when it is not
/// `target_pid`, the outpost's target: for a window in a console window,
/// which only the console host's outpost reports, the console's client,
/// which Windows names as the console window's owner, as NVDA gives a
/// console window and its text area the client's app module
/// (`UIA._get_processID`, `NVDAObjects/UIA/__init__.py` 2336 to 2346;
/// `appModuleHandler.getAppModuleForNVDAObject`, `appModuleHandler.py` 227
/// to 238). `None` for every other window.
pub(super) fn console_client(handle: isize, target_pid: u32) -> Option<Pid> {
    let top = top_level_of(handle);
    client_of(&window_class_name(top), window_owner(top).1, target_pid).map(Pid)
}

/// The client of a top-level window of class `class` that
/// `GetWindowThreadProcessId` says `owner` owns, reported by `target_pid`'s
/// outpost: the owner, for a console window that is not the target's own.
fn client_of(class: &str, owner: u32, target_pid: u32) -> Option<u32> {
    (class == CONSOLE_WINDOW_CLASS && owner != 0 && owner != target_pid).then_some(owner)
}

/// The class name of `handle`'s parent window, or `None` when it has none.
pub(super) fn parent_class(handle: isize) -> Option<String> {
    // SAFETY: GetAncestor tolerates any handle, returning null for an invalid
    // one.
    let parent = unsafe { GetAncestor(hwnd(handle), GA_PARENT) };
    (!parent.0.is_null()).then(|| window_class_name(parent.0 as isize))
}

/// `handle` as the model's opaque window handle.
fn window_handle(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

/// Whether `window` has the topmost extended style.
fn window_is_topmost(window: HWND) -> bool {
    // SAFETY: GetWindowLongW reads a window's style word; an invalid handle
    // yields 0.
    let style = unsafe { GetWindowLongW(window, GWL_EXSTYLE) };
    style.cast_unsigned() & WS_EX_TOPMOST.0 != 0
}

/// Facts about `handle`'s window for the reducer's attention classification
/// (`docs/parity/focus-events.md`, "Event acceptance"): its top-level window, the top of
/// its owner chain, whether it or its top-level window is topmost, and, for a
/// `Windows.UI.Core` window only, whether it is the input thread's active
/// window or inside it — NVDA's test for UWP windows — and whether it is in
/// the system's foreground window right now.
pub(super) fn window_facts(handle: isize) -> WindowFacts {
    let window = hwnd(handle);
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    let root = unsafe { GetAncestor(window, GA_ROOT) };
    // SAFETY: as above.
    let root_owner = unsafe { GetAncestor(window, GA_ROOTOWNER) };
    let root = if root.0.is_null() { window } else { root };
    let root_owner = if root_owner.0.is_null() {
        root
    } else {
        root_owner
    };
    let under_active_window = window_class_name(handle)
        .starts_with("Windows.UI.Core")
        .then(|| {
            let mut info = GUITHREADINFO {
                cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
                ..Default::default()
            };
            // SAFETY: `info` has cbSize set before the call.
            unsafe { GetGUIThreadInfo(0, &raw mut info) }.is_ok()
                && !info.hwndActive.0.is_null()
                && (info.hwndActive == window
                    // SAFETY: IsChild tolerates any pair of handles.
                    || unsafe { IsChild(info.hwndActive, window) }.as_bool())
        });
    // SAFETY: GetForegroundWindow has no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    let in_foreground = !foreground.0.is_null()
        && (root == foreground
            || root_owner == foreground
            // SAFETY: GetAncestor tolerates any handle.
            || root_owner == unsafe { GetAncestor(foreground, GA_ROOTOWNER) });
    WindowFacts {
        top_level: window_handle(root.0 as isize),
        root_owner: window_handle(root_owner.0 as isize),
        topmost: window_is_topmost(window) || window_is_topmost(root),
        under_active_window,
        in_foreground,
    }
}

/// The desktop window's handle.
pub(super) fn desktop_window() -> isize {
    // SAFETY: GetDesktopWindow has no preconditions.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize
}

/// Whether `hwnd` is one of the shell's windows that take the focus or the
/// foreground only while a switch is staged: the Alt+Tab switcher's and
/// Task View's staging windows, the Windows+X menu's host, and the desktop
/// shell's application manager window. NVDA's File Explorer app module
/// leaves the focus on them unsaid (`appModules/explorer.py` lines 443 to
/// 449, issues 5116 and 8137): NVDA still makes such a window its focus
/// and runs its foreground event, which cuts speech, and only the
/// announcement is skipped. The outposts and the focus listener drop the
/// focus on them altogether, so speech is not cut there (`docs/parity/windows-and-processes.md`,
/// "The shell's staging windows"): on Alt+Tab, Explorer's
/// `ForegroundStaging` window takes the focus with no name, and was said
/// as "pane" before the window switched to.
pub(crate) fn is_shell_transition_window(hwnd: isize) -> bool {
    matches!(
        window_class_name(hwnd).as_str(),
        "ForegroundStaging" | "LauncherTipWnd" | "ApplicationManager_DesktopShellWindow"
    )
}

/// Whether window `handle` itself is reported hung by the system
/// (`IsHungAppWindow`): the window an event names, not its top-level
/// window, as NVDA tests it, for a UIA event its sender's cached window
/// (`_shouldSkipEventForHungWindow`, `UIAHandler/utils.py` 438) and for an
/// MSAA event its window (`winEventToNVDAEvent`,
/// `IAccessibleHandler/__init__.py` 570). A child window whose own thread
/// answers is not hung though its top-level window's thread is not.
pub(crate) fn window_is_hung(handle: isize) -> bool {
    // SAFETY: IsHungAppWindow tolerates any handle.
    unsafe { IsHungAppWindow(hwnd(handle)) }.as_bool()
}

/// Why an event's own window, `handle`, cannot be read, or `None` when it
/// can: it is no window, or no longer one; a ghost window stands in for it;
/// or the system reports its application hung. NVDA drops such an event
/// after accepting it and before it creates its object, in that order
/// (`winEventToNVDAEvent`, `source/IAccessibleHandler/__init__.py`), since
/// creating it would call into an application that does not answer. Every
/// check is a local call.
pub(super) fn event_window_unreadable(handle: isize) -> Option<&'static str> {
    // SAFETY: IsWindow tolerates any handle.
    if handle == 0 || !unsafe { IsWindow(Some(hwnd(handle))) }.as_bool() {
        return Some("no window");
    }
    if ghost_window_from_hung_window().is_some_and(|ghost| {
        // SAFETY: the function takes any window handle and returns null
        // when no ghost window stands in for it.
        !unsafe { ghost(hwnd(handle)) }.0.is_null()
    }) {
        return Some("a ghost window stands in for it");
    }
    // SAFETY: IsHungAppWindow tolerates any handle.
    if unsafe { IsHungAppWindow(hwnd(handle)) }.as_bool() {
        return Some("its application is not responding");
    }
    None
}

/// `GhostWindowFromHungWindow`, an undocumented export of user32 that NVDA
/// uses where present (`source/winBindings/user32.py`): the window that
/// stands in for a hung window, or null. Looked up once.
fn ghost_window_from_hung_window() -> Option<unsafe extern "system" fn(HWND) -> HWND> {
    static FUNCTION: OnceLock<Option<unsafe extern "system" fn(HWND) -> HWND>> = OnceLock::new();
    *FUNCTION.get_or_init(|| {
        // SAFETY: user32 is loaded in every process that has windows,
        // which this one does through its hooks; the handle is not freed.
        let user32 = unsafe { GetModuleHandleW(w!("user32.dll")) }.ok()?;
        // SAFETY: a lookup by a NUL-terminated name in a loaded module.
        let function = unsafe { GetProcAddress(user32, s!("GhostWindowFromHungWindow")) }?;
        // SAFETY: the export takes a window handle and returns one
        // (`HWND GhostWindowFromHungWindow(HWND)`), the signature NVDA
        // declares for it; both are pointer-sized.
        Some(unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "system" fn(HWND) -> HWND,
            >(function)
        })
    })
}

/// The id of the thread that owns `handle`, or 0 for none. Batch limits are
/// counted per application UI thread, as NVDA counts them.
pub(crate) fn window_thread(handle: isize) -> u32 {
    if handle == 0 {
        return 0;
    }
    // SAFETY: GetWindowThreadProcessId tolerates any handle, returning 0 for
    // an invalid one.
    unsafe { GetWindowThreadProcessId(hwnd(handle), None) }
}

/// `handle`'s window text, read with `InternalGetWindowText`, which never
/// sends the window a message. `None` when the window has no text.
pub(super) fn window_text(handle: isize) -> Option<String> {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer outlives the call, which writes at most its length.
    let length = unsafe { InternalGetWindowText(hwnd(handle), &mut buffer) };
    let length = usize::try_from(length).ok()?;
    let text = String::from_utf16_lossy(&buffer[..length.min(buffer.len())]);
    (!text.trim().is_empty()).then_some(text)
}

/// `GetForegroundWindow()` if it belongs to `target_pid`, else `None`. Always
/// a genuine top-level window when it returns `Some`.
pub(super) fn foreground_window_of(target_pid: u32) -> Option<isize> {
    // SAFETY: GetForegroundWindow has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    (window_owner(window.0 as isize).1 == target_pid).then_some(window.0 as isize)
}

/// The target's top-level window for a tree dump: its foreground window,
/// else its first visible top-level window, skipping Core's hidden frame.
pub(super) fn main_window_of(target_pid: u32) -> Option<isize> {
    foreground_window_of(target_pid)
        .filter(|&window| !window_is_hidden_frame(window))
        .or_else(|| {
            top_level_windows(target_pid)
                .into_iter()
                .find(|&window| !window_is_hidden_frame(window) && window_is_visible(window))
        })
}

/// Whether `handle` is visible.
pub(crate) fn window_is_visible(handle: isize) -> bool {
    // SAFETY: IsWindowVisible tolerates any handle.
    unsafe { IsWindowVisible(hwnd(handle)) }.as_bool()
}

/// The window with the keyboard focus right now (falling back to the active
/// window), whatever process it belongs to.
pub(super) fn focus_window() -> Option<isize> {
    // SAFETY: `info` has cbSize set before the call; the call fails safely.
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        GetGUIThreadInfo(0, &raw mut info).ok()?;
        let window = if info.hwndFocus.0.is_null() {
            info.hwndActive
        } else {
            info.hwndFocus
        };
        (!window.0.is_null()).then_some(window.0 as isize)
    }
}

/// Whether the window in front belongs to the same application as `handle`
/// but another UI thread: the user has moved on from `handle` to a window
/// that could answer reads a slow `handle` is holding up in this outpost.
/// NVDA's watchdog asks whether the user has moved on from the window it is
/// waiting on (`_shouldRecoverAfterMinTimeout`), because its one core thread
/// serves every application; an outpost serves one, so only that
/// application's other threads are held up. `false` when either window's
/// owner cannot be told.
pub(super) fn front_is_another_thread_of_its_application(handle: isize) -> bool {
    focus_window().is_some_and(|front| is_another_thread_of_its_application(handle, front))
}

/// Whether `other` belongs to the same application as `handle` but another
/// UI thread. `false` when either window's owner cannot be told.
fn is_another_thread_of_its_application(handle: isize, other: isize) -> bool {
    let (thread, pid) = window_owner(handle);
    let (other_thread, other_pid) = window_owner(other);
    thread != 0 && other_thread != 0 && other_pid == pid && other_thread != thread
}

/// The thread and process that own `handle`, zeros for an invalid window.
pub(crate) fn window_owner(handle: isize) -> (u32, u32) {
    let mut pid = 0u32;
    // SAFETY: GetWindowThreadProcessId tolerates any handle, returning 0 for
    // an invalid one.
    let thread = unsafe { GetWindowThreadProcessId(hwnd(handle), Some(&raw mut pid)) };
    (thread, pid)
}

/// The focus window of `target_pid`, or `None` if the keyboard focus is not
/// in that process.
pub(crate) fn focus_window_of(target_pid: u32) -> Option<isize> {
    let window = focus_window()?;
    let mut pid = 0u32;
    // SAFETY: the call fails safely on a window that has since been destroyed.
    unsafe {
        GetWindowThreadProcessId(hwnd(window), Some(&raw mut pid));
    }
    (pid == target_pid).then_some(window)
}

/// The top-level windows belonging to `target_pid`.
pub(crate) fn top_level_windows(target_pid: u32) -> Vec<isize> {
    struct Search {
        target_pid: u32,
        windows: Vec<isize>,
    }
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the search passed below, alive for the call.
        let search = unsafe { &mut *(lparam.0 as *mut Search) };
        // A panic here would abort the process at the callback's boundary;
        // caught, it ends the enumeration instead.
        let visited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_, pid) = window_owner(window.0 as isize);
            if pid == search.target_pid {
                search.windows.push(window.0 as isize);
            }
        }));
        BOOL::from(visited.is_ok())
    }
    let mut search = Search {
        target_pid,
        windows: Vec::new(),
    };
    // SAFETY: `visit` reads only the search state, which outlives the
    // synchronous EnumWindows call.
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM((&raw mut search) as isize));
    }
    search.windows
}

/// Every descendant window of `parent`, in the order `EnumChildWindows`
/// gives them: each child, then its own descendants. A local enumeration.
pub(crate) fn descendant_windows(parent: isize) -> Vec<isize> {
    unsafe extern "system" fn visit(window: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the list passed below, alive for the call.
        let windows = unsafe { &mut *(lparam.0 as *mut Vec<isize>) };
        windows.push(window.0 as isize);
        BOOL::from(true)
    }
    let mut windows: Vec<isize> = Vec::new();
    // SAFETY: `visit` only pushes onto the list, which outlives the
    // synchronous EnumChildWindows call; an invalid parent enumerates
    // nothing.
    unsafe {
        let _ = EnumChildWindows(
            Some(hwnd(parent)),
            Some(visit),
            LPARAM((&raw mut windows) as isize),
        );
    }
    windows
}

/// Waits, for as long as it takes, for process `pid` to exit, and says
/// whether it did: true once it has exited, or when no process has that
/// id (it exited before the wait began); false when the process cannot be
/// waited on, such as one running at a higher integrity level.
pub(crate) fn wait_for_process_exit(pid: u32) -> bool {
    use windows::Win32::Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: a wait-only open of a plain process id; the handle is closed
    // below.
    let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
        Ok(process) => process,
        Err(error) => return error.code() == ERROR_INVALID_PARAMETER.to_hresult(),
    };
    // SAFETY: the handle opened above, with SYNCHRONIZE access.
    let waited = unsafe { WaitForSingleObject(process, INFINITE) };
    // SAFETY: the handle opened above, closed once.
    let _ = unsafe { CloseHandle(process) };
    waited == WAIT_OBJECT_0
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    use super::*;

    /// A message-only window owned by the calling thread: never on any
    /// desktop, so a test run puts no window on the one it runs on.
    fn create_window() -> isize {
        // SAFETY: a predefined class, the message-only parent, no menu or
        // creation data; the window is destroyed by the thread that created
        // it.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("verbatim-outpost test window"),
                WS_OVERLAPPED,
                0,
                0,
                10,
                10,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .expect("create a window");
        window.0 as isize
    }

    fn destroy_window(handle: isize) {
        // SAFETY: called on the thread that created `handle`.
        unsafe { DestroyWindow(hwnd(handle)) }.expect("destroy a window");
    }

    #[test]
    fn only_a_live_window_can_be_read_for_an_event() {
        let window = create_window();
        assert_eq!(event_window_unreadable(window), None);
        destroy_window(window);
        assert_eq!(event_window_unreadable(window), Some("no window"));
        assert_eq!(event_window_unreadable(0), Some("no window"));
    }

    #[test]
    fn an_event_in_a_console_window_belongs_to_the_consoles_client() {
        // The console host's outpost, 30, reports the window Windows says
        // its client, 20, owns.
        assert_eq!(client_of(CONSOLE_WINDOW_CLASS, 20, 30), Some(20));
    }

    #[test]
    fn an_event_in_any_other_window_belongs_to_the_target() {
        assert_eq!(client_of("Notepad", 20, 30), None);
        assert_eq!(
            client_of(CONSOLE_WINDOW_CLASS, 30, 30),
            None,
            "a console window that is the target's own"
        );
        assert_eq!(
            client_of(CONSOLE_WINDOW_CLASS, 0, 30),
            None,
            "a window whose owner cannot be told"
        );
    }

    #[test]
    fn only_a_window_on_another_thread_of_the_application_counts_as_moved_on() {
        let slow = create_window();
        let same_thread = create_window();
        let (created, receive) = mpsc::channel();
        let (finish, finished) = mpsc::channel::<()>();
        let other_thread = thread::spawn(move || {
            let window = create_window();
            created.send(window).expect("hand over the window");
            finished.recv().expect("wait for the checks");
            destroy_window(window);
        });
        let other = receive.recv().expect("the window of the other thread");

        assert!(is_another_thread_of_its_application(slow, other));
        assert!(!is_another_thread_of_its_application(slow, same_thread));
        assert!(!is_another_thread_of_its_application(slow, slow));
        assert!(
            !is_another_thread_of_its_application(slow, 0),
            "a window whose owner cannot be told never counts"
        );

        finish.send(()).expect("release the other thread");
        other_thread.join().expect("the other thread ends");
        destroy_window(same_thread);
        destroy_window(slow);
    }
}
