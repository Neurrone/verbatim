//! Window queries an outpost makes with local calls only: none of them sends
//! the window a message or calls into another process, so they are safe on
//! any thread and cannot block on a hung application.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_PARENT, GA_ROOT, GA_ROOTOWNER, GUITHREADINFO, GWL_STYLE, GetAncestor,
    GetForegroundWindow, GetGUIThreadInfo, GetPropW, GetWindowLongPtrW, GetWindowThreadProcessId,
    InternalGetWindowText, IsChild, IsHungAppWindow, IsWindow, IsWindowVisible, WS_DISABLED,
};
use windows::core::{BOOL, HSTRING, s, w};

use verbatim_model::{HIDDEN_FRAME_WINDOW_PROP, Pid, WindowFacts, WindowHandle};
use verbatim_uia::provider_process;

use super::text_reads::CONSOLE_WINDOW_CLASS;
use crate::arbitration::window_class_name;

/// Milliseconds since the Unix epoch on the run's shared clock
/// ([`crate::protocol::now_us`]), the observation timestamp that orders
/// events across processes and anchors the keypress-to-audio latency
/// timeline. Shared with the listener, which stamps facts at observation.
pub(crate) fn now_ms() -> u64 {
    crate::protocol::now_us() / 1_000
}

/// The observation `timing` holds, in microseconds on the run's shared
/// clock; now, when `timing` holds none.
pub(crate) fn observed_us(timing: &crate::protocol::EventTiming) -> u64 {
    match timing.observed_at_us {
        0 => crate::protocol::now_us(),
        observed_at_us => observed_at_us,
    }
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
pub(crate) fn window_belongs_to_hidden_frame(handle: isize) -> bool {
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

/// Whether `child` is `parent` or one of its child windows, by the parent
/// chain alone (`IsChild`, which follows no owner), as NVDA's
/// `winUser.isDescendantWindow` decides it. A local call.
pub(crate) fn is_descendant_window(parent: isize, child: isize) -> bool {
    // SAFETY: IsChild tolerates any pair of handles.
    parent == child || unsafe { IsChild(hwnd(parent), hwnd(child)) }.as_bool()
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
pub(super) fn window_handle(handle: isize) -> WindowHandle {
    WindowHandle(u64::from_ne_bytes(handle.to_ne_bytes()))
}

/// Facts about `handle`'s window for the reducer's attention classification
/// (`docs/parity/focus-events.md`, "Event acceptance"): its top-level window, the top of
/// its owner chain, whether it or its top-level window is topmost, and, for a
/// `Windows.UI.Core` window only, whether it is the input thread's active
/// window or inside it — NVDA's test for UWP windows — whether it is in
/// the system's foreground window right now, and its class, which Core's
/// rules for an application's windows match (decision D6).
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
    let class = window_class_name(handle);
    // How the window stands to the foreground window, by the acceptance
    // test's own rule (`acceptance::foreground_relation`), against the system's
    // foreground window now.
    let relation = super::acceptance::foreground_relation(
        handle,
        &class,
        &super::acceptance::SystemWindows {
            foreground: foreground_window_handle(),
        },
    );
    WindowFacts {
        top_level: window_handle(root.0 as isize),
        root_owner: window_handle(root_owner.0 as isize),
        topmost: relation.topmost,
        under_active_window: relation.under_active_window,
        in_foreground: relation.in_foreground,
        class,
        application: String::new(),
        alt_held: false,
    }
}

/// Whether Alt is held now, by the system's asynchronous key state, as
/// NVDA's File Explorer module reads it when it judges a task switching
/// list item's container (`appModules/explorer.py`,
/// `MultitaskingViewFrameListItem._get_container`): a local call.
pub(super) fn alt_is_held() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};
    // SAFETY: GetAsyncKeyState takes any virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_MENU.0)) };
    // The high bit, the sign, says the key is down.
    state < 0
}

/// The desktop window's handle.
pub(super) fn desktop_window() -> isize {
    // SAFETY: GetDesktopWindow has no preconditions.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() }.0 as isize
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

/// The parent window of `handle` (`GetAncestor` with `GA_PARENT`), 0 for
/// none. A local call.
pub(super) fn parent_window(handle: isize) -> isize {
    // SAFETY: GetAncestor tolerates any handle, answering null for an
    // invalid one.
    unsafe { GetAncestor(hwnd(handle), GA_PARENT) }.0 as isize
}

/// Whether window `handle`'s style disables it (`WS_DISABLED`), the one
/// state Windows' own state gives it as a plain window that the model
/// carries (`NVDAObjects/window/__init__.py`, `Window._get_states`). A
/// local call.
pub(super) fn window_disabled(handle: isize) -> bool {
    // SAFETY: GetWindowLongPtrW tolerates any handle, answering 0 for an
    // invalid one.
    let style = unsafe { GetWindowLongPtrW(hwnd(handle), GWL_STYLE) };
    style.cast_unsigned() & usize::try_from(WS_DISABLED.0).unwrap_or(0) != 0
}

/// Whether `handle` is a window that exists. A local call.
pub(crate) fn window_exists(handle: isize) -> bool {
    // SAFETY: IsWindow tolerates any handle.
    unsafe { IsWindow(Some(hwnd(handle))) }.as_bool()
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

/// Whether window `handle` is `target_pid`'s, the application an outpost
/// serves: whether that process serves the window's UIA
/// ([`verbatim_uia::served_by`]), so a console window is its console host's
/// and never its client's, whom `GetWindowThreadProcessId` names, even when
/// the host cannot be read from the client
/// (`docs/design/listener-without-connections.md`, section 4.3). Every
/// filter below decides by it. Local calls only.
pub(crate) fn served_by(handle: isize, target_pid: u32) -> bool {
    verbatim_uia::served_by(handle, target_pid)
}

/// `GetForegroundWindow()` if it is `target_pid`'s ([`served_by`]), else
/// `None`. Always a genuine top-level window when it returns `Some`.
pub(super) fn foreground_window_of(target_pid: u32) -> Option<isize> {
    // SAFETY: GetForegroundWindow has no preconditions.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    served_by(window.0 as isize, target_pid).then_some(window.0 as isize)
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
pub(crate) fn focus_window() -> Option<isize> {
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

/// How long a slow call into an application is waited for before it is
/// abandoned because the user has moved on to a window of the same
/// application on another UI thread: NVDA's `MIN_CORE_ALIVE_TIMEOUT`,
/// after which NVDA cancels a slow call once the foreground has changed.
/// One application can own windows on several threads (File Explorer's
/// folder windows, the taskbar, and the Alt+Tab switcher), and those must
/// not wait behind a slow window the user has left. Both an outpost's
/// worker and the focus listener's reading threads apply it, each judged
/// by [`front_is_another_thread_of_its_application`].
pub(crate) const MOVED_ON_GRACE: Duration = Duration::from_millis(500);

/// Whether the window in front belongs to the same application as `handle`
/// but another UI thread: the user has moved on from `handle` to a window
/// that could answer reads a slow `handle` is holding up, in its outpost's
/// worker or in its application's reading thread in the focus listener.
/// NVDA's watchdog asks whether the user has moved on from the window it is
/// waiting on (`_shouldRecoverAfterMinTimeout`), because its one core thread
/// serves every application; an outpost, or a reading thread, serves one,
/// so only that application's other threads are held up. The application
/// is the window's provider process ([`provider_process`]), so a console
/// window is its console host's, never its client's. `false` when either
/// window's owner or provider cannot be told, so a console window whose
/// host cannot be read (`provider_process` answers `None`, as for an
/// elevated client) is never moved on from, nor moved on to. Console
/// host's Find, Properties and Defaults windows are on the console
/// window's real thread, which `GetWindowThreadProcessId` reports as the
/// client's for the console window, so with one of them in front a slow
/// read of the console window is abandoned, as NVDA's watchdog abandons
/// it. Local calls only.
pub(crate) fn front_is_another_thread_of_its_application(handle: isize) -> bool {
    focus_window().is_some_and(|front| is_another_thread_of_its_application(handle, front))
}

/// Whether `other` is served by the same process as `handle`, by
/// [`provider_process`], on another UI thread than `handle`'s. `false` when
/// either window's owner or provider cannot be told. The providers are read
/// only for windows on different threads: `GetClassNameW` and
/// `GetWindowThreadProcessId` for each, and for a console window its
/// client's `OpenProcess`, `NtQueryInformationProcess` and `CloseHandle`.
fn is_another_thread_of_its_application(handle: isize, other: isize) -> bool {
    let (thread, _) = window_owner(handle);
    let (other_thread, _) = window_owner(other);
    thread != 0
        && other_thread != 0
        && other_thread != thread
        && provider_process(handle)
            .is_some_and(|provider| provider_process(other) == Some(provider))
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
/// in a window of that application ([`served_by`]).
pub(crate) fn focus_window_of(target_pid: u32) -> Option<isize> {
    let window = focus_window()?;
    served_by(window, target_pid).then_some(window)
}

/// The top-level windows of `target_pid` ([`served_by`]).
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
            if served_by(window.0 as isize, search.target_pid) {
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

    /// An event the outpost's own handlers took in is stamped with the
    /// microseconds of its observation, its own clock read.
    #[test]
    fn an_observation_is_stamped_from_its_own_clock_read() {
        let timing = crate::protocol::EventTiming {
            observed_at_us: 6_543_210,
            ..crate::protocol::EventTiming::default()
        };
        assert_eq!(observed_us(&timing), 6_543_210);
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
    fn a_console_window_is_its_console_hosts_not_its_clients() {
        let console = verbatim_test_console::Console::start();
        let (_, client) = window_owner(console.window);
        assert_ne!(
            client, console.host,
            "Windows names the client as its owner"
        );
        assert!(served_by(console.window, console.host));
        assert!(!served_by(console.window, client));
        // Only their console windows: other top-level windows, such as a
        // thread's IME window, are not at issue.
        let consoles = |pid| {
            top_level_windows(pid)
                .into_iter()
                .filter(|&window| window_class_name(window) == CONSOLE_WINDOW_CLASS)
                .collect::<Vec<_>>()
        };
        assert_eq!(consoles(console.host), [console.window]);
        assert_eq!(consoles(client), Vec::<isize>::new());
    }

    #[test]
    fn any_other_window_is_its_owners() {
        let window = create_window();
        let served = served_by(window, std::process::id());
        destroy_window(window);
        assert!(served);
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

    #[test]
    fn a_console_window_and_its_clients_own_window_are_not_one_application() {
        let console = verbatim_test_console::Console::start_with_client_window();
        let (console_thread, client) = window_owner(console.window);
        let (window_thread, window_owner_pid) = window_owner(console.client_window);
        assert_eq!(
            window_owner_pid, client,
            "Windows names the client as both windows' owner"
        );
        assert_ne!(
            window_thread, console_thread,
            "the client's window is on another thread than the console window's"
        );

        assert!(
            !is_another_thread_of_its_application(console.client_window, console.window),
            "a console window in front is its console host's"
        );
        assert!(
            !is_another_thread_of_its_application(console.window, console.client_window),
            "a console window being read is its console host's"
        );
    }
}
