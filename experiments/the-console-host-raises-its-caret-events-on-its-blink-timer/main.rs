//! Which signals does the console host raise after a caret key, and when?
//!
//! Usage: conhost-caret-probe <cmd|powershell> <out.tsv> <cycles> <steps> <interval_ms>
//!
//! Opens a classic console window (`conhost.exe <shell>`), types a line,
//! then presses Left `steps` times and Right `steps` times, `cycles` times,
//! `interval_ms` apart, timestamping every WinEvent on the console window
//! (and every console WinEvent and caret object event anywhere) and every
//! UIA event and property change in the window's subtree, with the console
//! cursor read through the console API before and after each press as the
//! ground truth for whether the caret moved.
#![allow(clippy::too_many_lines)]

use std::fmt::Write as _;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE, HWND, LPARAM};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Console::{
    AttachConsole, CONSOLE_SCREEN_BUFFER_INFO, FreeConsole, GetConsoleScreenBufferInfo,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateProcessW, GetCurrentThreadId, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, HWINEVENTHOOK, IUIAutomation, SetWinEventHook, TreeScope_Subtree,
    UIA_EVENT_ID, UIA_PROPERTY_ID,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
    VIRTUAL_KEY, VK_LEFT, VK_RIGHT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, MSG, PostThreadMessageW, SetForegroundWindow, TranslateMessage,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_QUIT,
};
use windows::core::{BOOL, PCWSTR, PWSTR};

static CONSOLE: AtomicIsize = AtomicIsize::new(0);
static RECORDS: Mutex<Vec<(f64, String)>> = Mutex::new(Vec::new());
static CLOCK: OnceLock<(i64, f64)> = OnceLock::new();

fn now_us() -> f64 {
    let (start, per_us) = *CLOCK.get().expect("clock");
    let mut now = 0;
    unsafe { QueryPerformanceCounter(&raw mut now) }.expect("qpc");
    (now - start) as f64 / per_us
}

fn record(what: String) {
    let at = now_us();
    RECORDS.lock().unwrap().push((at, what));
}

unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    thread: u32,
    _time: u32,
) {
    let console = CONSOLE.load(Ordering::Relaxed);
    let on_console = console != 0 && hwnd.0 as isize == console;
    let console_kind = (0x4001..=0x40FF).contains(&event);
    let caret_object = id_object == -8;
    if on_console || console_kind || caret_object {
        record(format!(
            "win\t{event:#06x}\thwnd={:#x}\tobj={id_object}\tchild={id_child}\tthread={thread}\tconsole={on_console}",
            hwnd.0 as isize
        ));
    }
}

mod hearing {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationEventHandler, IUIAutomationEventHandler_Impl,
        IUIAutomationFocusChangedEventHandler, IUIAutomationFocusChangedEventHandler_Impl,
        IUIAutomationPropertyChangedEventHandler, IUIAutomationPropertyChangedEventHandler_Impl,
        UIA_EVENT_ID, UIA_PROPERTY_ID,
    };

    #[windows_core::implement(
        IUIAutomationEventHandler,
        IUIAutomationFocusChangedEventHandler,
        IUIAutomationPropertyChangedEventHandler
    )]
    pub struct Hearing;

    pub fn new() -> IUIAutomationEventHandler {
        Hearing.into()
    }

    fn who(sender: &windows_core::Ref<IUIAutomationElement>) -> String {
        sender.as_ref().map_or_else(String::new, |element| unsafe {
            let name = element.CurrentClassName().map(|s| s.to_string()).unwrap_or_default();
            let ty = element.CurrentControlType().map(|t| t.0).unwrap_or_default();
            format!("class={name}\ttype={ty}")
        })
    }

    impl IUIAutomationEventHandler_Impl for Hearing_Impl {
        fn HandleAutomationEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            eventid: UIA_EVENT_ID,
        ) -> windows_core::Result<()> {
            let at = super::now_us();
            let who = who(&sender);
            super::RECORDS
                .lock()
                .unwrap()
                .push((at, format!("uia\t{}\t{who}", eventid.0)));
            Ok(())
        }
    }

    impl IUIAutomationFocusChangedEventHandler_Impl for Hearing_Impl {
        fn HandleFocusChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
        ) -> windows_core::Result<()> {
            let at = super::now_us();
            let who = who(&sender);
            super::RECORDS
                .lock()
                .unwrap()
                .push((at, format!("uiafocus\t20005\t{who}")));
            Ok(())
        }
    }

    impl IUIAutomationPropertyChangedEventHandler_Impl for Hearing_Impl {
        fn HandlePropertyChangedEvent(
            &self,
            sender: windows_core::Ref<IUIAutomationElement>,
            propertyid: UIA_PROPERTY_ID,
            _newvalue: &VARIANT,
        ) -> windows_core::Result<()> {
            let at = super::now_us();
            let who = who(&sender);
            super::RECORDS
                .lock()
                .unwrap()
                .push((at, format!("uiaprop\t{}\t{who}", propertyid.0)));
            Ok(())
        }
    }
}

fn console_windows() -> Vec<isize> {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        if String::from_utf16_lossy(&class[..n]) == "ConsoleWindowClass" {
            let found = unsafe { &mut *(found.0 as *mut Vec<isize>) };
            found.push(hwnd.0 as isize);
        }
        true.into()
    }
    let mut found: Vec<isize> = Vec::new();
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut found as isize)) };
    found
}

fn child_of(parent: u32, name: &str) -> Option<u32> {
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut ok = unsafe { Process32FirstW(snap, &raw mut entry) }.is_ok();
    while ok {
        let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
        let exe = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
        if entry.th32ParentProcessID == parent && exe == name {
            return Some(entry.th32ProcessID);
        }
        ok = unsafe { Process32NextW(snap, &raw mut entry) }.is_ok();
    }
    None
}

fn key(vk: VIRTUAL_KEY, ch: u16, up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    let mut scan = 0;
    if ch != 0 {
        flags |= KEYEVENTF_UNICODE;
        scan = ch;
    } else {
        scan = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC) } as u16;
        if vk == VK_LEFT || vk == VK_RIGHT {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: if ch != 0 { VIRTUAL_KEY(0) } else { vk },
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    assert_eq!(sent as usize, inputs.len(), "SendInput");
}

fn cursor(out: HANDLE) -> (i16, i16) {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    unsafe { GetConsoleScreenBufferInfo(out, &raw mut info) }.expect("screen buffer info");
    (info.dwCursorPosition.X, info.dwCursorPosition.Y)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let shell = args[1].clone();
    let out_path = args[2].clone();
    let cycles: usize = args[3].parse().unwrap();
    let steps: usize = args[4].parse().unwrap();
    let interval: u64 = args[5].parse().unwrap();

    let mut freq = 0;
    let mut start = 0;
    unsafe {
        QueryPerformanceFrequency(&raw mut freq).unwrap();
        QueryPerformanceCounter(&raw mut start).unwrap();
    }
    CLOCK.set((start, freq as f64 / 1e6)).unwrap();

    // The WinEvent thread.
    let (tx, rx) = std::sync::mpsc::channel();
    let win_thread = std::thread::spawn(move || unsafe {
        let hook = SetWinEventHook(
            1,
            0x7FFF_FFFF,
            None,
            Some(on_win_event),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        assert!(!hook.is_invalid(), "hook");
        tx.send(GetCurrentThreadId()).unwrap();
        let mut msg = MSG::default();
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    });
    let win_thread_id = rx.recv().unwrap();

    // Launch the console.
    let before = console_windows();
    let exe = if shell == "cmd" { "cmd.exe" } else { "powershell.exe" };
    let mut command: Vec<u16> = format!("conhost.exe {exe} -NoLogo")
        .replace("cmd.exe -NoLogo", "cmd.exe")
        .encode_utf16()
        .chain([0])
        .collect();
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        dwFlags: windows::Win32::System::Threading::STARTF_USESHOWWINDOW,
        wShowWindow: 1,
        ..Default::default()
    };
    let mut info = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            windows::Win32::System::Threading::CREATE_NEW_CONSOLE,
            None,
            PCWSTR::null(),
            &raw const startup,
            &raw mut info,
        )
    }
    .expect("launch");
    let conhost_pid = info.dwProcessId;
    let mut window = 0;
    for _ in 0..100 {
        if let Some(&w) = console_windows().iter().find(|w| !before.contains(w)) {
            window = w;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(window != 0, "no console window");
    CONSOLE.store(window, Ordering::Relaxed);
    let mut shell_pid = None;
    for _ in 0..100 {
        shell_pid = child_of(conhost_pid, exe);
        if shell_pid.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let shell_pid = shell_pid.expect("shell process");
    let hwnd = HWND(window as *mut _);
    let fg_at_launch = unsafe { GetForegroundWindow() } == hwnd;
    std::thread::sleep(Duration::from_millis(if shell == "cmd" { 3000 } else { 10000 }));
    let fg_after_wait = unsafe { GetForegroundWindow() } == hwnd;
    let mut fg_text = String::new();
    let _ = write!(fg_text, "fg_at_launch={fg_at_launch} fg_after_wait={fg_after_wait}");
    unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
            MOUSEEVENTF_MOVE, MOUSEINPUT,
        };
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, GetWindowRect, SM_CXSCREEN, SM_CYSCREEN};
        let mut rect = windows::Win32::Foundation::RECT::default();
        GetWindowRect(hwnd, &raw mut rect).unwrap();
        let x = (rect.left + rect.right) / 2;
        let y = (rect.top + rect.bottom) / 2;
        let ax = x * 65535 / GetSystemMetrics(SM_CXSCREEN);
        let ay = y * 65535 / GetSystemMetrics(SM_CYSCREEN);
        let mouse = |flags| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: ax, dy: ay, mouseData: 0, dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 },
            },
        };
        send(&[mouse(windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS(0)), mouse(MOUSEEVENTF_LEFTDOWN), mouse(MOUSEEVENTF_LEFTUP)]);
    }
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(unsafe { GetForegroundWindow() }, hwnd, "console not in front");
    let mut window_pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut window_pid)) };

    // The UIA thread.
    let (utx, urx) = std::sync::mpsc::channel::<()>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let uia_thread = std::thread::spawn(move || unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap();
        let uia: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).unwrap();
        let root = uia.ElementFromHandle(HWND(window as *mut _)).unwrap();
        let handler = hearing::new();
        for id in [20014, 20015, 20024, 20026, 20032, 20033, 20034, 20035, 20036] {
            uia.AddAutomationEventHandler(UIA_EVENT_ID(id), &root, TreeScope_Subtree, None, &handler)
                .unwrap();
        }
        let props: Vec<UIA_PROPERTY_ID> = [
            30001, 30005, 30008, 30045, 30092, 30093, 30096, 30100, 30003, 30009, 30010,
        ]
        .into_iter()
        .map(UIA_PROPERTY_ID)
        .collect();
        uia.AddPropertyChangedEventHandlerNativeArray(
            &root,
            TreeScope_Subtree,
            None,
            &windows_core::Interface::cast::<
                windows::Win32::UI::Accessibility::IUIAutomationPropertyChangedEventHandler,
            >(&handler)
            .unwrap(),
            &props,
        )
        .unwrap();
        uia.AddFocusChangedEventHandler(
            None,
            &windows_core::Interface::cast::<
                windows::Win32::UI::Accessibility::IUIAutomationFocusChangedEventHandler,
            >(&handler)
            .unwrap(),
        )
        .unwrap();
        ready_tx.send(()).unwrap();
        let _ = urx.recv();
        let _ = uia.RemoveAllEventHandlers();
    });
    ready_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(1000));

    // Ground truth through the console API.
    unsafe {
        let _ = FreeConsole();
        AttachConsole(shell_pid).expect("attach");
    }
    let out = unsafe {
        CreateFileW(
            windows::core::w!("CONOUT$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .expect("CONOUT$");

    // Type the line.
    let typed_from = cursor(out);
    let line = "echo the quick brown fox jumps over the lazy dog 0123456789 abcdefghij";
    for ch in line.encode_utf16() {
        let scan = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW(ch) };
        assert!(scan & 0x0100 == 0 && scan != -1, "plain key for {ch}");
        let vk = VIRTUAL_KEY((scan & 0xFF) as u16);
        send(&[key(vk, 0, false), key(vk, 0, true)]);
        std::thread::sleep(Duration::from_millis(8));
    }
    std::thread::sleep(Duration::from_millis(1500));
    let typed_to = cursor(out);
    if !(typed_to.0 - typed_from.0 >= 60 || typed_to.1 > typed_from.1) {
        std::fs::write(&out_path, unsafe { format!("not typed {typed_from:?} {typed_to:?} {fg_text} window={window:#x} visible={} fg={:?} shell_pid={shell_pid} conhost={conhost_pid}", windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd).as_bool(), GetForegroundWindow()) }).unwrap();
        unsafe {
            use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
            for pid in [shell_pid, conhost_pid] {
                if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                    let _ = TerminateProcess(process, 0);
                }
            }
        }
        std::process::exit(2);
    }
    record("mark\tidle-start".into());
    std::thread::sleep(Duration::from_millis(3000));
    record("mark\tidle-end".into());

    let mut presses = String::new();
    let mut index = 0;
    for _ in 0..cycles {
        for vk in [VK_LEFT, VK_RIGHT] {
            for _ in 0..steps {
                let before = cursor(out);
                let at = now_us();
                send(&[key(vk, 0, false), key(vk, 0, true)]);
                record(format!("key\t{index}"));
                let mut moved_ms = -1.0;
                while now_us() - at < 100_000.0 {
                    if cursor(out) != before {
                        moved_ms = (now_us() - at) / 1000.0;
                        break;
                    }
                }
                let left = interval as f64 - (now_us() - at) / 1000.0;
                std::thread::sleep(Duration::from_micros((left.max(0.0) * 1000.0) as u64));
                let after = cursor(out);
                let _ = writeln!(
                    presses,
                    "{index}\t{at:.1}\t{}\t{},{}\t{},{}\t{moved_ms:.2}",
                    if vk == VK_LEFT { "left" } else { "right" },
                    before.0,
                    before.1,
                    after.0,
                    after.1
                );
                index += 1;
            }
        }
    }
    std::thread::sleep(Duration::from_millis(1000));
    record("mark\tend".into());

    utx.send(()).unwrap();
    uia_thread.join().unwrap();
    unsafe { PostThreadMessageW(win_thread_id, WM_QUIT, Default::default(), LPARAM(0)) }.unwrap();
    win_thread.join().unwrap();

    let mut text = format!(
        "# shell={shell} conhost_pid={conhost_pid} shell_pid={shell_pid} window={window:#x} window_pid={window_pid} interval={interval}\n"
    );
    text.push_str("## presses: index\tat_us\tkey\tbefore\tafter\n");
    text.push_str(&presses);
    text.push_str("## records: at_us\tsource\t...\n");
    for (at, what) in RECORDS.lock().unwrap().iter() {
        let _ = writeln!(text, "{at:.1}\t{what}");
    }
    std::fs::write(&out_path, text).unwrap();

    // Close the console and its shell.
    unsafe {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
        for pid in [shell_pid, conhost_pid] {
            if let Ok(process) = OpenProcess(PROCESS_TERMINATE, false, pid) {
                let _ = TerminateProcess(process, 0);
            }
        }
    }
}
