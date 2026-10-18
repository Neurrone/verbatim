//! Scratch: experiments for option (c), outposts registering for focus.
#![allow(clippy::all, clippy::pedantic, unsafe_code, missing_docs)]

use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BSTR, Interface, PCWSTR, Ref, Result, implement};

fn now_us() -> u64 {
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    let v = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
    (v - 116_444_736_000_000_000) / 10
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

struct Held {
    at: Instant,
    label: String,
    element: IUIAutomationElement,
}
unsafe impl Send for Held {}

static TAG: OnceLock<String> = OnceLock::new();
static QUIET: AtomicBool = AtomicBool::new(false);
static HOLD: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU64 = AtomicU64::new(0);
static HELD: Mutex<Vec<Held>> = Mutex::new(Vec::new());
static TIMES: Mutex<Vec<(u64, i32, String)>> = Mutex::new(Vec::new());

fn describe(e: &IUIAutomationElement) -> (i32, String) {
    let pid = unsafe { e.CachedProcessId() }.unwrap_or(-1);
    let name = unsafe { e.CachedName() }
        .map(|b| b.to_string())
        .unwrap_or_else(|_| "?".into());
    let hwnd = unsafe { e.CachedNativeWindowHandle() }
        .map(|h| h.0 as isize)
        .unwrap_or(-1);
    (pid, format!("name={name:?} hwnd={hwnd}"))
}

fn arrived(reg: &str, kind: &str, sender: Option<&IUIAutomationElement>) {
    let t = now_us();
    COUNT.fetch_add(1, Ordering::SeqCst);
    let Some(e) = sender else {
        say(&format!("ev {} {reg} {kind} t={t} nosender", TAG.get().unwrap()));
        return;
    };
    let (pid, rest) = describe(e);
    if HOLD.load(Ordering::SeqCst) {
        HELD.lock().unwrap().push(Held {
            at: Instant::now(),
            label: format!("{reg} pid={pid} {rest}"),
            element: e.clone(),
        });
    }
    if QUIET.load(Ordering::SeqCst) {
        TIMES.lock().unwrap().push((t, pid, rest));
    } else {
        say(&format!(
            "ev {} {reg} {kind} t={t} pid={pid} {rest}",
            TAG.get().unwrap()
        ));
    }
}

#[implement(IUIAutomationFocusChangedEventHandler)]
struct Focus(String);
impl IUIAutomationFocusChangedEventHandler_Impl for Focus_Impl {
    fn HandleFocusChangedEvent(&self, sender: Ref<'_, IUIAutomationElement>) -> Result<()> {
        arrived(&self.0, "focus", sender.as_ref());
        Ok(())
    }
}

#[implement(IUIAutomationEventHandler)]
struct Event(String);
impl IUIAutomationEventHandler_Impl for Event_Impl {
    fn HandleAutomationEvent(
        &self,
        sender: Ref<'_, IUIAutomationElement>,
        id: UIA_EVENT_ID,
    ) -> Result<()> {
        let kind = if id == UIA_AutomationFocusChangedEventId {
            "focus".to_string()
        } else {
            format!("event{}", id.0)
        };
        arrived(&self.0, &kind, sender.as_ref());
        Ok(())
    }
}

#[implement(IUIAutomationPropertyChangedEventHandler)]
struct Prop(String);
impl IUIAutomationPropertyChangedEventHandler_Impl for Prop_Impl {
    fn HandlePropertyChangedEvent(
        &self,
        sender: Ref<'_, IUIAutomationElement>,
        id: UIA_PROPERTY_ID,
        _v: &VARIANT,
    ) -> Result<()> {
        arrived(&self.0, &format!("prop{}", id.0), sender.as_ref());
        Ok(())
    }
}

fn cache(uia: &IUIAutomation, kind: &str) -> Result<IUIAutomationCacheRequest> {
    let req = unsafe { uia.CreateCacheRequest() }?;
    let props: &[UIA_PROPERTY_ID] = match kind {
        "none" => &[],
        "pid" => &[UIA_ProcessIdPropertyId],
        "min" => &[
            UIA_ProcessIdPropertyId,
            UIA_NativeWindowHandlePropertyId,
            UIA_NamePropertyId,
        ],
        // NVDA's base cache request (UIAHandler/__init__.py 61 to 74).
        "base" => &[
            UIA_NativeWindowHandlePropertyId,
            UIA_FrameworkIdPropertyId,
            UIA_AutomationIdPropertyId,
            UIA_ClassNamePropertyId,
            UIA_ControlTypePropertyId,
            UIA_ProviderDescriptionPropertyId,
            UIA_ProcessIdPropertyId,
            UIA_IsTextPatternAvailablePropertyId,
            UIA_IsContentElementPropertyId,
            UIA_IsControlElementPropertyId,
            UIA_NamePropertyId,
            UIA_LocalizedControlTypePropertyId,
            UIA_HasKeyboardFocusPropertyId,
        ],
        other => panic!("unknown cache {other}"),
    };
    for p in props {
        unsafe { req.AddProperty(*p) }?;
    }
    if kind == "base" {
        unsafe { req.AddPattern(UIA_TextPatternId) }?;
    }
    Ok(req)
}

fn hwnd_of(s: &str) -> HWND {
    HWND(s.parse::<isize>().unwrap() as *mut c_void)
}

fn register(uia: &IUIAutomation, spec: &str, keep: &mut Vec<windows::core::IUnknown>) {
    let parts: Vec<&str> = spec.split(':').collect();
    let t0 = Instant::now();
    let result: Result<()> = (|| {
        match parts[0] {
            "desk" => {
                let req = cache(uia, parts[1])?;
                let h: IUIAutomationFocusChangedEventHandler = Focus(spec.to_string()).into();
                unsafe { uia.AddFocusChangedEventHandler(&req, &h) }?;
                keep.push(h.cast()?);
            }
            "group" | "groupcv" => {
                let req = cache(uia, parts[2])?;
                let el = unsafe { uia.ElementFromHandle(hwnd_of(parts[1])) }?;
                let u6: IUIAutomation6 = uia.cast()?;
                let g = unsafe { u6.CreateEventHandlerGroup() }?;
                let h: IUIAutomationEventHandler = Event(spec.to_string()).into();
                unsafe {
                    g.AddAutomationEventHandler(
                        UIA_AutomationFocusChangedEventId,
                        TreeScope_Subtree,
                        &req,
                        &h,
                    )
                }?;
                unsafe { u6.AddEventHandlerGroup(&el, &g) }?;
                keep.push(h.cast()?);
                keep.push(g.cast()?);
            }
            "rootgroup" => {
                // A group holding the focus event, on the desktop root.
                let req = cache(uia, parts[1])?;
                let el = unsafe { uia.GetRootElement() }?;
                let u6: IUIAutomation6 = uia.cast()?;
                let g = unsafe { u6.CreateEventHandlerGroup() }?;
                let h: IUIAutomationEventHandler = Event(spec.to_string()).into();
                unsafe {
                    g.AddAutomationEventHandler(
                        UIA_AutomationFocusChangedEventId,
                        TreeScope_Subtree,
                        &req,
                        &h,
                    )
                }?;
                unsafe { u6.AddEventHandlerGroup(&el, &g) }?;
                keep.push(h.cast()?);
                keep.push(g.cast()?);
            }
            "direct" => {
                let req = cache(uia, parts[2])?;
                let el = unsafe { uia.ElementFromHandle(hwnd_of(parts[1])) }?;
                let h: IUIAutomationEventHandler = Event(spec.to_string()).into();
                unsafe {
                    uia.AddAutomationEventHandler(
                        UIA_AutomationFocusChangedEventId,
                        &el,
                        TreeScope_Subtree,
                        &req,
                        &h,
                    )
                }?;
                keep.push(h.cast()?);
            }
            "name" => {
                let req = cache(uia, "min")?;
                let el = unsafe { uia.ElementFromHandle(hwnd_of(parts[1])) }?;
                let h: IUIAutomationPropertyChangedEventHandler = Prop(spec.to_string()).into();
                unsafe {
                    uia.AddPropertyChangedEventHandlerNativeArray(
                        &el,
                        TreeScope_Subtree,
                        &req,
                        &h,
                        &[UIA_NamePropertyId],
                    )
                }?;
                keep.push(h.cast()?);
            }
            other => panic!("unknown spec {other}"),
        }
        Ok(())
    })();
    match result {
        Ok(()) => say(&format!(
            "registered {} {spec} in {:.1}ms",
            TAG.get().unwrap(),
            t0.elapsed().as_secs_f64() * 1000.0
        )),
        Err(e) => say(&format!(
            "refused {} {spec} hr=0x{:08x} msg={:?}",
            TAG.get().unwrap(),
            e.code().0 as u32,
            e.message()
        )),
    }
}

fn process_ms(handle: windows::Win32::Foundation::HANDLE) -> (f64, f64) {
    let (mut c, mut x, mut k, mut u) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetProcessTimes(handle, &mut c, &mut x, &mut k, &mut u) }.unwrap();
    let f = |t: FILETIME| ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 / 10_000.0;
    (f(u), f(k))
}

fn client(args: &[String]) {
    TAG.set(args[0].clone()).unwrap();
    for flag in args[1].split(',') {
        match flag {
            "quiet" => QUIET.store(true, Ordering::SeqCst),
            "hold" => HOLD.store(true, Ordering::SeqCst),
            _ => {}
        }
    }
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
    let uia: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.unwrap();
    let mut keep = Vec::new();
    for spec in &args[2..] {
        register(&uia, spec, &mut keep);
    }
    say(&format!("client-ready {}", args[0]));
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.unwrap();
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.first().copied() {
            Some("reg") => register(&uia, words[1], &mut keep),
            Some("cpu") => {
                let (u, k) = process_ms(unsafe { GetCurrentProcess() });
                say(&format!(
                    "cpu {} user={u:.1} kernel={k:.1} events={}",
                    args[0],
                    COUNT.load(Ordering::SeqCst)
                ));
            }
            Some("times") => {
                let times = std::mem::take(&mut *TIMES.lock().unwrap());
                let list: Vec<String> = times.iter().map(|(t, p, _)| format!("{t}:{p}")).collect();
                say(&format!("times {} {}", args[0], list.join(" ")));
            }
            Some("held") => {
                let held = HELD.lock().unwrap();
                say(&format!("held {} count={}", args[0], held.len()));
                for (i, h) in held.iter().enumerate() {
                    let t0 = Instant::now();
                    let name: Result<BSTR> = unsafe { h.element.CurrentName() };
                    let focus = unsafe { h.element.CurrentHasKeyboardFocus() };
                    let pid = unsafe { h.element.CurrentProcessId() };
                    say(&format!(
                        "heldread {} #{i} age={:.1}s took={:.2}ms name={} focus={} pid={} [{}]",
                        args[0],
                        h.at.elapsed().as_secs_f64(),
                        t0.elapsed().as_secs_f64() * 1000.0,
                        match &name {
                            Ok(b) => format!("{:?}", b.to_string()),
                            Err(e) => format!("ERR0x{:08x}", e.code().0 as u32),
                        },
                        match &focus {
                            Ok(b) => format!("{}", b.as_bool()),
                            Err(e) => format!("ERR0x{:08x}", e.code().0 as u32),
                        },
                        match &pid {
                            Ok(p) => format!("{p}"),
                            Err(e) => format!("ERR0x{:08x}", e.code().0 as u32),
                        },
                        h.label
                    ));
                }
            }
            Some("clear") => {
                HELD.lock().unwrap().clear();
                TIMES.lock().unwrap().clear();
                COUNT.store(0, Ordering::SeqCst);
                say(&format!("cleared {}", args[0]));
            }
            Some("quit") => break,
            _ => say(&format!("unknown {line}")),
        }
    }
    unsafe {
        let _ = uia.RemoveAllEventHandlers();
    }
    drop(keep);
    std::process::exit(0);
}

/// Runs `argv` on desktop `name` (made if missing), with this process's
/// standard handles, and exits with its exit code once it ends.
fn ondesk(name: &str, argv: &[String]) {
    use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
    use windows::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows::Win32::System::StationsAndDesktops::{CreateDesktopW, DESKTOP_CONTROL_FLAGS};
    use windows::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateProcessW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, STARTUPINFOW, WaitForSingleObject,
    };
    use windows::core::PWSTR;
    let wname = wide(name);
    let _desk = unsafe {
        CreateDesktopW(
            PCWSTR(wname.as_ptr()),
            PCWSTR::null(),
            None,
            DESKTOP_CONTROL_FLAGS(0),
            GENERIC_ALL.0,
            None,
        )
    }
    .unwrap();
    let mut path = wide(&format!("WinSta0\\{name}"));
    let line: Vec<String> = argv.iter().map(|a| format!("\"{a}\"")).collect();
    let mut cmd = wide(&line.join(" "));
    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    si.lpDesktop = PWSTR(path.as_mut_ptr());
    si.dwFlags = STARTF_USESTDHANDLES;
    unsafe {
        si.hStdInput = GetStdHandle(STD_INPUT_HANDLE).unwrap_or(HANDLE::default());
        si.hStdOutput = GetStdHandle(STD_OUTPUT_HANDLE).unwrap_or(HANDLE::default());
        si.hStdError = GetStdHandle(STD_ERROR_HANDLE).unwrap_or(HANDLE::default());
    }
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            true,
            CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    }
    .unwrap();
    // The child's own pid, for experiments that read its processor time.
    eprintln!("ondesk-child-pid {}", pi.dwProcessId);
    unsafe { WaitForSingleObject(pi.hProcess, INFINITE) };
    let mut code = 0u32;
    unsafe {
        let _ = GetExitCodeProcess(pi.hProcess, &mut code);
    }
    std::process::exit(code as i32);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args[0].as_str() {
        "client" => client(&args[1..]),
        "find" => {
            let w = wide(&args[1]);
            let h = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(w.as_ptr())) }.unwrap_or_default();
            println!("{}", h.0 as isize);
        }
        "findclass" => {
            let w = wide(&args[1]);
            let h = unsafe { FindWindowW(PCWSTR(w.as_ptr()), PCWSTR::null()) }.unwrap_or_default();
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            println!("{} {pid}", h.0 as isize);
        }
        "fg" => {
            let h = hwnd_of(&args[1]);
            let mv = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: 0,
                        dwFlags: MOUSEEVENTF_MOVE,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            unsafe { SendInput(&[mv], std::mem::size_of::<INPUT>() as i32) };
            unsafe {
                if IsIconic(h).as_bool() {
                    let _ = ShowWindow(h, SW_RESTORE);
                }
            }
            let ok = unsafe { SetForegroundWindow(h) }.as_bool();
            std::thread::sleep(std::time::Duration::from_millis(100));
            let fgw = unsafe { GetForegroundWindow() };
            println!("fg ok={ok} foreground={} wanted={}", fgw.0 as isize, h.0 as isize);
        }
        "coreevent" => {
            // coreevent <tag> <hwnd|root>: UiaAddEvent for focus at the core API level.
            unsafe extern "system" fn cb(
                args: *mut UiaEventArgs,
                _data: *mut windows::Win32::System::Com::SAFEARRAY,
                _tree: std::mem::ManuallyDrop<BSTR>,
            ) {
                let id = unsafe { (*args).EventId };
                say(&format!("ev {} core event{} t={}", TAG.get().unwrap(), id, now_us()));
            }
            TAG.set(args[1].clone()).unwrap();
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
            let mut node = HUIANODE::default();
            let r = if args[2] == "root" {
                unsafe { UiaGetRootNode(&mut node) }
            } else {
                unsafe { UiaNodeFromHandle(hwnd_of(&args[2]), &mut node) }
            };
            say(&format!("node {:?}", r));
            let mut cond = UiaCondition { ConditionType: ConditionType_True };
            let mut req = UiaCacheRequest {
                pViewCondition: &mut cond,
                Scope: TreeScope_Element,
                pProperties: std::ptr::null_mut(),
                cProperties: 0,
                pPatterns: std::ptr::null_mut(),
                cPatterns: 0,
                automationElementMode: AutomationElementMode_Full,
            };
            let mut ev = HUIAEVENT::default();
            let r = unsafe {
                UiaAddEvent(
                    node,
                    UIA_AutomationFocusChangedEventId.0,
                    // The C API takes the function pointer itself; the binding's type is a pointer to it.
                    cb as usize as *mut _,
                    TreeScope_Subtree,
                    std::ptr::null_mut(),
                    0,
                    &mut req,
                    &mut ev,
                )
            };
            match r {
                Ok(()) => say(&format!("registered {} core", args[1])),
                Err(e) => say(&format!(
                    "refused {} core hr=0x{:08x} msg={:?}",
                    args[1],
                    e.code().0 as u32,
                    e.message()
                )),
            }
            say(&format!("client-ready {}", args[1]));
            for line in std::io::stdin().lock().lines() {
                if line.map(|l| l.trim() == "quit").unwrap_or(true) {
                    break;
                }
            }
            std::process::exit(0);
        }
        "findchild" => {
            let w = wide(&args[2]);
            let h = unsafe {
                FindWindowExW(Some(hwnd_of(&args[1])), None, PCWSTR(w.as_ptr()), PCWSTR::null())
            }
            .unwrap_or_default();
            println!("{}", h.0 as isize);
        }
        "minimize" => {
            let h = hwnd_of(&args[1]);
            unsafe {
                let _ = ShowWindow(h, SW_MINIMIZE);
            }
        }
        "winevent" => {
            // winevent <hwnd> <event> <idObject> <idChild>
            let h = hwnd_of(&args[1]);
            let ev = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap();
            unsafe {
                NotifyWinEvent(ev, h, args[3].parse().unwrap(), args[4].parse().unwrap())
            };
            println!("winevent sent t={}", now_us());
        }
        "cpu" => {
            let pid: u32 = args[1].parse().unwrap();
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.unwrap();
            let (u, k) = process_ms(h);
            println!("cpu pid={pid} user={u:.1} kernel={k:.1}");
        }
        "keys" => {
            // keys <tab|shifttab> <count> <gap_ms>
            let count: u32 = args[2].parse().unwrap();
            let gap: u64 = args[3].parse().unwrap();
            for _ in 0..count {
                let seq: Vec<INPUT> = match args[1].as_str() {
                    "tab" => vec![key(VK_TAB, false), key(VK_TAB, true)],
                    "shifttab" => vec![
                        key(VK_SHIFT, false),
                        key(VK_TAB, false),
                        key(VK_TAB, true),
                        key(VK_SHIFT, true),
                    ],
                    "down" => vec![key(VK_DOWN, false), key(VK_DOWN, true)],
                    "up" => vec![key(VK_UP, false), key(VK_UP, true)],
                    other => panic!("key {other}"),
                };
                unsafe { SendInput(&seq, std::mem::size_of::<INPUT>() as i32) };
                println!("key t={}", now_us());
                std::thread::sleep(std::time::Duration::from_millis(gap));
            }
        }
        "now" => println!("{}", now_us()),
        "ondesk" => ondesk(&args[1], &args[2..]),
        "hits" => {
            // hits <hwnd> <reset 0|1>: mockapp's nonzero counters, then optionally reset.
            let h = hwnd_of(&args[1]);
            let mut out = Vec::new();
            for i in 0..74usize {
                let v = unsafe { SendMessageW(h, WM_APP + 2, Some(WPARAM(i)), Some(LPARAM(0))) }.0;
                if v != 0 {
                    out.push(format!("{i}:{v}"));
                }
            }
            if args[2] == "1" {
                unsafe { SendMessageW(h, WM_APP + 3, Some(WPARAM(0)), Some(LPARAM(0))) };
            }
            println!("{}", out.join(" "));
        }
        "send" => {
            // send <hwnd> <msg> <wparam>: SendMessage, prints the result.
            let r = unsafe {
                SendMessageW(
                    hwnd_of(&args[1]),
                    args[2].parse().unwrap(),
                    Some(WPARAM(args[3].parse().unwrap())),
                    Some(LPARAM(0)),
                )
            };
            println!("{}", r.0);
        }
        other => panic!("unknown command {other}"),
    }
}
