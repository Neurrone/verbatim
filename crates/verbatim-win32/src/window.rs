//! Window queries made with local calls only: none of them sends the
//! window a message or calls into another process, so they are safe on any
//! thread and cannot block on a hung application, and they are not counted
//! (`docs/performance.md`, "What counts as a call"). Window handles are
//! `isize`, zero for none, and every function tolerates any handle value,
//! failing on one that names no window. The outposts, the focus listener,
//! the MSAA client and Core's supervisor all read windows through them.

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
use windows::Win32::UI::WindowsAndMessaging::{
    ES_MULTILINE, EnumWindows, GA_PARENT, GA_ROOT, GA_ROOTOWNER, GET_ANCESTOR_FLAGS,
    GET_WINDOW_CMD, GUITHREADINFO, GWL_EXSTYLE, GWL_STYLE, GetAncestor, GetClassNameW,
    GetDesktopWindow, GetForegroundWindow, GetGUIThreadInfo, GetPropW, GetTopWindow, GetWindow,
    GetWindowLongPtrW, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsChild,
    IsHungAppWindow, IsWindow, IsWindowVisible, WS_EX_TOPMOST,
};
use windows::core::{BOOL, HSTRING, s, w};

use verbatim_model::HIDDEN_FRAME_WINDOW_PROP;
use verbatim_reader_rules::acceptance::WindowQueries;
use verbatim_reader_rules::class::WindowClasses;

use crate::provider::provider_process;

/// The `HWND` for a handle value.
fn hwnd(handle: isize) -> HWND {
    HWND(handle as *mut c_void)
}

/// Whether `handle` is a window that exists.
#[must_use]
pub fn window_exists(handle: isize) -> bool {
    // SAFETY: IsWindow tolerates any handle.
    unsafe { IsWindow(Some(hwnd(handle))) }.as_bool()
}

/// Whether `handle` is a visible window; false for one that names no
/// window.
#[must_use]
pub fn window_is_visible(handle: isize) -> bool {
    // SAFETY: IsWindowVisible tolerates any handle.
    unsafe { IsWindowVisible(hwnd(handle)) }.as_bool()
}

/// Whether `handle` is an enabled window; false for one that names no
/// window.
#[must_use]
pub fn is_enabled(handle: isize) -> bool {
    // SAFETY: IsWindowEnabled tolerates any handle.
    unsafe { IsWindowEnabled(hwnd(handle)) }.as_bool()
}

/// Whether `child` is a window inside `parent` (`IsChild`).
#[must_use]
pub fn is_child(parent: isize, child: isize) -> bool {
    // SAFETY: IsChild tolerates any pair of handles.
    unsafe { IsChild(hwnd(parent), hwnd(child)) }.as_bool()
}

/// The desktop window's handle.
#[must_use]
pub fn desktop_window() -> isize {
    // SAFETY: GetDesktopWindow has no preconditions.
    unsafe { GetDesktopWindow() }.0 as isize
}

/// The window's class name, empty when it has none or names no window.
#[must_use]
pub fn window_class_name(handle: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: GetClassNameW writes at most the local buffer's length and
    // returns the count written, zero for an invalid handle.
    let len = unsafe { GetClassNameW(hwnd(handle), &mut buffer) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    String::from_utf16_lossy(&buffer[..len.min(buffer.len())])
}

/// `handle`'s classes as the class verdict judges them: its own, normalized
/// as NVDA normalizes it, and its root ancestor's. An invalid handle's root
/// is 0, whose class reads as empty.
#[must_use]
pub fn window_classes(handle: isize) -> WindowClasses {
    WindowClasses::new(
        &window_class_name(handle),
        &window_class_name(top_level_of(handle)),
    )
}

/// The id of the thread that owns `handle`, or 0 for none. Batch limits are
/// counted per application UI thread, as NVDA counts them.
#[must_use]
pub fn window_thread(handle: isize) -> u32 {
    if handle == 0 {
        return 0;
    }
    // SAFETY: GetWindowThreadProcessId tolerates any handle, returning 0 for
    // an invalid one.
    unsafe { GetWindowThreadProcessId(hwnd(handle), None) }
}

/// Whether the window is a standard or rich edit control made multi-line
/// (`ES_MULTILINE`), which NVDA's edit control class reports as the
/// multi-line state. Any class with "edit" in its name counts, so the
/// Windows Forms and rich edit classes do too.
#[must_use]
pub fn is_multiline_edit(handle: isize) -> bool {
    if !window_class_name(handle)
        .to_ascii_lowercase()
        .contains("edit")
    {
        return false;
    }
    style(handle) & ES_MULTILINE as isize != 0
}

/// The window's style (`GWL_STYLE`), zero for one that names no window.
#[must_use]
pub fn style(handle: isize) -> isize {
    // SAFETY: a local read of the window's style; any handle is tolerated.
    unsafe { GetWindowLongPtrW(hwnd(handle), GWL_STYLE) }
}

/// The window's rectangle in screen coordinates (`GetWindowRect`), `None`
/// for one that names no window.
#[must_use]
pub fn rect(handle: isize) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: a local out-parameter; any handle is tolerated, failing.
    unsafe { GetWindowRect(hwnd(handle), &raw mut rect) }.ok()?;
    Some(rect)
}

/// The window's parent (`GetAncestor` with `GA_PARENT`), zero for none.
#[must_use]
pub fn parent(handle: isize) -> isize {
    ancestor(handle, GA_PARENT)
}

/// The window's first child window in z-order (`GetTopWindow`), zero for
/// none.
#[must_use]
pub fn top_child(handle: isize) -> isize {
    // SAFETY: GetTopWindow tolerates any handle.
    unsafe { GetTopWindow(Some(hwnd(handle))) }
        .unwrap_or_default()
        .0 as isize
}

/// The window related to `handle` by `command` (`GetWindow`), zero for
/// none.
#[must_use]
pub fn related(handle: isize, command: GET_WINDOW_CMD) -> isize {
    // SAFETY: GetWindow tolerates any handle, failing at an edge.
    unsafe { GetWindow(hwnd(handle), command) }
        .unwrap_or_default()
        .0 as isize
}

/// `handle`'s ancestor by `flags` (`GetAncestor`), 0 for none.
fn ancestor(handle: isize, flags: GET_ANCESTOR_FLAGS) -> isize {
    // SAFETY: GetAncestor tolerates any handle, returning null for an
    // invalid one.
    unsafe { GetAncestor(hwnd(handle), flags) }.0 as isize
}

/// Whether `handle` is Core's hidden main frame: it carries the marker
/// property (decision D9; see [`verbatim_model::HIDDEN_FRAME_WINDOW_PROP`])
/// and belongs to Core's process. Any process can set the property on its
/// own windows, so the owner is checked too; Core is this outpost's parent,
/// which launched it.
#[must_use]
pub fn window_is_hidden_frame(handle: isize) -> bool {
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
#[must_use]
pub fn window_belongs_to_hidden_frame(handle: isize) -> bool {
    if window_is_hidden_frame(handle) {
        return true;
    }
    let root = top_level_of(handle);
    root != 0 && root != handle && window_is_hidden_frame(root)
}

/// The system's foreground window, or 0 when no window has the foreground.
#[must_use]
pub fn foreground_window_handle() -> isize {
    // SAFETY: GetForegroundWindow has no preconditions.
    unsafe { GetForegroundWindow() }.0 as isize
}

/// `handle`'s top-level window (`GetAncestor` with `GA_ROOT`), or 0 for an
/// invalid handle.
#[must_use]
pub fn top_level_of(handle: isize) -> isize {
    // SAFETY: GetAncestor tolerates any handle, returning null for an invalid
    // one.
    unsafe { GetAncestor(hwnd(handle), GA_ROOT) }.0 as isize
}

/// Whether window `handle` itself is reported hung by the system
/// (`IsHungAppWindow`): the window an event names, not its top-level
/// window, as NVDA tests it, for a UIA event its sender's cached window
/// (`_shouldSkipEventForHungWindow`, `UIAHandler/utils.py` 438) and for an
/// MSAA event its window (`winEventToNVDAEvent`,
/// `IAccessibleHandler/__init__.py` 570). A child window whose own thread
/// answers is not hung though its top-level window's thread is not.
#[must_use]
pub fn window_is_hung(handle: isize) -> bool {
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
#[must_use]
pub fn event_window_unreadable(handle: isize) -> Option<&'static str> {
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

/// Whether window `handle` is `target_pid`'s, the application an outpost
/// serves: whether that process serves the window's UIA
/// ([`crate::provider::served_by`]), so a console window is its console host's
/// and never its client's, whom `GetWindowThreadProcessId` names, even when
/// the host cannot be read from the client
/// (`docs/design/listener-without-connections.md`, section 4.3). Every
/// filter below decides by it. Local calls only.
#[must_use]
pub fn served_by(handle: isize, target_pid: u32) -> bool {
    crate::provider::served_by(handle, target_pid)
}

/// The window with the keyboard focus right now (falling back to the active
/// window), whatever process it belongs to.
#[must_use]
pub fn focus_window() -> Option<isize> {
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
#[must_use]
pub fn front_is_another_thread_of_its_application(handle: isize) -> bool {
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
#[must_use]
pub fn window_owner(handle: isize) -> (u32, u32) {
    let mut pid = 0u32;
    // SAFETY: GetWindowThreadProcessId tolerates any handle, returning 0 for
    // an invalid one.
    let thread = unsafe { GetWindowThreadProcessId(hwnd(handle), Some(&raw mut pid)) };
    (thread, pid)
}

/// The top-level windows of `target_pid` ([`served_by`]).
#[must_use]
pub fn top_level_windows(target_pid: u32) -> Vec<isize> {
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

/// The system's windows, read with local calls, with the foreground window
/// given: the outpost's foreground reader's.
pub struct SystemWindows {
    /// The foreground window, as the caller's foreground reader read it.
    pub foreground: isize,
}

impl WindowQueries for SystemWindows {
    fn class_name(&self, window: isize) -> String {
        window_class_name(window)
    }

    fn process(&self, window: isize) -> u32 {
        window_owner(window).1
    }

    fn parent(&self, window: isize) -> isize {
        ancestor(window, GA_PARENT)
    }

    fn root(&self, window: isize) -> isize {
        ancestor(window, GA_ROOT)
    }

    fn root_owner(&self, window: isize) -> isize {
        ancestor(window, GA_ROOTOWNER)
    }

    fn is_child(&self, parent: isize, child: isize) -> bool {
        is_child(parent, child)
    }

    fn is_topmost(&self, window: isize) -> bool {
        // SAFETY: GetWindowLongW reads a window's style word; an invalid
        // handle yields 0.
        let style = unsafe { GetWindowLongW(hwnd(window), GWL_EXSTYLE) };
        style.cast_unsigned() & WS_EX_TOPMOST.0 != 0
    }

    fn desktop(&self) -> isize {
        desktop_window()
    }

    fn active_window(&self) -> isize {
        let mut info = GUITHREADINFO {
            cbSize: u32::try_from(size_of::<GUITHREADINFO>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `info` has cbSize set before the call, which fails safely.
        if unsafe { GetGUIThreadInfo(0, &raw mut info) }.is_err() {
            return 0;
        }
        info.hwndActive.0 as isize
    }

    fn foreground(&self) -> isize {
        self.foreground
    }
}

/// Reads the foreground window's handle. An outpost reads the system's
/// (`GetForegroundWindow`); a test whose application cannot take the
/// foreground on the desktop it runs on supplies its own
/// (the outpost's and the focus reader's `set_foreground_reader`).
pub type ForegroundReader = Arc<dyn Fn() -> isize + Send + Sync>;

/// Why a window cannot be read, `None` when it can: it is no window, a
/// ghost window stands in for it, or its application is hung. An outpost
/// asks the system ([`event_window_unreadable`]); a test that cannot
/// wait for Windows to judge an application hung supplies its own
/// (the outpost's and the focus reader's `set_window_readability_reader`).
pub type WindowReadabilityReader = Arc<dyn Fn(isize) -> Option<&'static str> + Send + Sync>;

/// Why window `hwnd` cannot be read, by the system's judgement, `None` when
/// it can: what an outpost's [`WindowReadabilityReader`] answers unless a
/// test supplies its own. Local calls.
#[must_use]
pub fn window_unreadable(hwnd: isize) -> Option<&'static str> {
    event_window_unreadable(hwnd)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    use verbatim_reader_rules::CONSOLE_WINDOW_CLASS;

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
                w!("verbatim-win32 test window"),
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
    fn a_console_window_is_its_console_hosts_not_its_clients() {
        verbatim_test_console::Console::with(|console| {
            let (_, client) = window_owner(console.window);
            assert_ne!(
                client, console.host,
                "Windows names the client as its owner"
            );
            assert!(served_by(console.window, console.host));
            assert!(!served_by(console.window, client));
            // Only their console windows: other top-level windows, such as
            // a thread's IME window, are not at issue.
            let consoles = |pid| {
                top_level_windows(pid)
                    .into_iter()
                    .filter(|&window| window_class_name(window) == CONSOLE_WINDOW_CLASS)
                    .collect::<Vec<_>>()
            };
            assert_eq!(consoles(console.host), [console.window]);
            assert_eq!(consoles(client), Vec::<isize>::new());
        });
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
        verbatim_test_console::Console::with_client_window(|console| {
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
        });
    }
}
