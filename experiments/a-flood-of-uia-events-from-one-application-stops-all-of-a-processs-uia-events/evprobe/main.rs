//! Scratch: UIA event-thread probe for the M4.7 study. Handlers copy and queue only.
#![allow(clippy::all, clippy::pedantic, unsafe_code, missing_docs)]

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{FILETIME, HWND};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, GetProcessTimes, GetThreadTimes, OpenThread,
    THREAD_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::WindowsProgramming::{
    QueryThreadCycleTime,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, Ref, Result, implement};

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

struct Ev {
    t: u64,
    label: &'static str,
    pid: i32,
    name: String,
}

static TAG: OnceLock<String> = OnceLock::new();
static TX: OnceLock<Sender<Ev>> = OnceLock::new();
static WORKERS: Mutex<Vec<u32>> = Mutex::new(Vec::new());
#[derive(Default, Clone)]
struct Stats {
    n: u64,
    first: u64,
    last: u64,
}
static STATS: Mutex<Option<HashMap<&'static str, Stats>>> = Mutex::new(None);
thread_local! { static SEEN: Cell<bool> = const { Cell::new(false) }; }

/// The handler's whole work: copy the cached facts and queue them.
fn arrived(label: &'static str, sender: Option<&IUIAutomationElement>) {
    let t = now_us();
    if !SEEN.with(|s| s.replace(true)) {
        WORKERS.lock().unwrap().push(unsafe { GetCurrentThreadId() });
    }
    let (pid, name) = match sender {
        Some(e) => (
            unsafe { e.CachedProcessId() }.unwrap_or(-1),
            unsafe { e.CachedName() }.map(|b| b.to_string()).unwrap_or_default(),
        ),
        None => (-1, String::new()),
    };
    let _ = TX.get().unwrap().send(Ev { t, label, pid, name });
}

#[implement(IUIAutomationFocusChangedEventHandler)]
struct Focus(&'static str);
impl IUIAutomationFocusChangedEventHandler_Impl for Focus_Impl {
    fn HandleFocusChangedEvent(&self, sender: Ref<'_, IUIAutomationElement>) -> Result<()> {
        arrived(self.0, sender.as_ref());
        Ok(())
    }
}

#[implement(IUIAutomationEventHandler)]
struct Event(&'static str);
impl IUIAutomationEventHandler_Impl for Event_Impl {
    fn HandleAutomationEvent(&self, sender: Ref<'_, IUIAutomationElement>, _id: UIA_EVENT_ID) -> Result<()> {
        arrived(self.0, sender.as_ref());
        Ok(())
    }
}

#[implement(IUIAutomationPropertyChangedEventHandler)]
struct Prop(&'static str);
impl IUIAutomationPropertyChangedEventHandler_Impl for Prop_Impl {
    fn HandlePropertyChangedEvent(&self, sender: Ref<'_, IUIAutomationElement>, _id: UIA_PROPERTY_ID, _v: &VARIANT) -> Result<()> {
        arrived(self.0, sender.as_ref());
        Ok(())
    }
}

fn cache(uia: &IUIAutomation, kind: &str) -> Result<IUIAutomationCacheRequest> {
    let req = unsafe { uia.CreateCacheRequest() }?;
    let props: &[UIA_PROPERTY_ID] = match kind {
        "none" => &[],
        "min" => &[UIA_ProcessIdPropertyId, UIA_NativeWindowHandlePropertyId, UIA_NamePropertyId],
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
    Ok(req)
}

fn flag(flags: &[String], name: &str) -> Option<String> {
    flags.iter().find_map(|f| f.strip_prefix(&format!("{name}=")).map(str::to_string))
}

fn make_uia(flags: &[String]) -> IUIAutomation {
    let uia: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }.unwrap();
    let u2: IUIAutomation2 = uia.cast().unwrap();
    if let Some(ms) = flag(flags, "cto") {
        unsafe { u2.SetConnectionTimeout(ms.parse().unwrap()) }.unwrap();
    }
    if let Some(ms) = flag(flags, "tto") {
        unsafe { u2.SetTransactionTimeout(ms.parse().unwrap()) }.unwrap();
    }
    let u6: IUIAutomation6 = uia.cast().unwrap();
    if let Some(v) = flag(flags, "coalesce") {
        let c = if v == "1" { CoalesceEventsOptions_Enabled } else { CoalesceEventsOptions_Disabled };
        unsafe { u6.SetCoalesceEvents(c) }.unwrap();
    }
    if let Some(v) = flag(flags, "recovery") {
        let c = if v == "1" { ConnectionRecoveryBehaviorOptions_Enabled } else { ConnectionRecoveryBehaviorOptions_Disabled };
        unsafe { u6.SetConnectionRecoveryBehavior(c) }.unwrap();
    }
    uia
}

fn hwnd_of(s: &str) -> HWND {
    HWND(s.parse::<isize>().unwrap() as *mut c_void)
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn register(uia: &IUIAutomation, spec: &str, keep: &mut Vec<windows::core::IUnknown>) {
    let parts: Vec<&str> = spec.split(':').collect();
    let label = leak(spec);
    let t0 = std::time::Instant::now();
    let result: Result<()> = (|| {
        match parts[0] {
            "desk" => {
                let req = cache(uia, parts[1])?;
                let h: IUIAutomationFocusChangedEventHandler = Focus(label).into();
                unsafe { uia.AddFocusChangedEventHandler(&req, &h) }?;
                keep.push(h.cast()?);
            }
            "deskprop" => {
                // A desktop-wide (root subtree) name-change registration with a cache request.
                let req = cache(uia, parts[1])?;
                let el = unsafe { uia.GetRootElement() }?;
                let h: IUIAutomationPropertyChangedEventHandler = Prop(label).into();
                unsafe { uia.AddPropertyChangedEventHandlerNativeArray(&el, TreeScope_Subtree, &req, &h, &[UIA_NamePropertyId]) }?;
                keep.push(h.cast()?);
            }
            "name" | "qname" => {
                let req = cache(uia, "min")?;
                let el = unsafe { uia.ElementFromHandle(hwnd_of(parts[1])) }?;
                let h: IUIAutomationPropertyChangedEventHandler = Prop(label).into();
                unsafe { uia.AddPropertyChangedEventHandlerNativeArray(&el, TreeScope_Subtree, &req, &h, &[UIA_NamePropertyId]) }?;
                keep.push(h.cast()?);
            }
            "text" | "qtext" => {
                let req = cache(uia, "min")?;
                let el = unsafe { uia.ElementFromHandle(hwnd_of(parts[1])) }?;
                let h: IUIAutomationEventHandler = Event(label).into();
                unsafe { uia.AddAutomationEventHandler(UIA_Text_TextChangedEventId, &el, TreeScope_Subtree, &req, &h) }?;
                keep.push(h.cast()?);
            }
            other => panic!("unknown spec {other}"),
        }
        Ok(())
    })();
    match result {
        Ok(()) => say(&format!("registered {} {spec} in {:.1}ms", TAG.get().unwrap(), t0.elapsed().as_secs_f64() * 1000.0)),
        Err(e) => say(&format!("refused {} {spec} hr=0x{:08x}", TAG.get().unwrap(), e.code().0 as u32)),
    }
}

fn ft_ms(t: FILETIME) -> f64 {
    ((u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)) as f64 / 10_000.0
}

fn cpu_line(tag: &str) -> String {
    let (mut c, mut x, mut k, mut u) = Default::default();
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut x, &mut k, &mut u) }.unwrap();
    let mut s = format!("cpu {tag} proc_user={:.1} proc_kernel={:.1}", ft_ms(u), ft_ms(k));
    for tid in WORKERS.lock().unwrap().iter() {
        let h = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, *tid) }.unwrap();
        let (mut c, mut x, mut k, mut u) = Default::default();
        unsafe { GetThreadTimes(h, &mut c, &mut x, &mut k, &mut u) }.unwrap();
        let mut cycles = 0u64;
        unsafe { QueryThreadCycleTime(h, &mut cycles) }.unwrap();
        s += &format!(" worker{tid}_user={:.1} worker{tid}_kernel={:.1} worker{tid}_cycles={cycles}", ft_ms(u), ft_ms(k));
    }
    s
}

fn client(args: &[String]) {
    TAG.set(args[0].clone()).unwrap();
    let tag = args[0].clone();
    let flags: Vec<String> = args[1].split(',').map(str::to_string).collect();
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().unwrap();
    *STATS.lock().unwrap() = Some(HashMap::new());
    let (tx, rx) = channel::<Ev>();
    TX.set(tx).unwrap();
    // The consumer: prints the events of loud registrations, counts every one.
    let ctag = tag.clone();
    std::thread::spawn(move || {
        for ev in rx {
            {
                let mut g = STATS.lock().unwrap();
                let st = g.as_mut().unwrap().entry(ev.label).or_default();
                if st.n == 0 {
                    st.first = ev.t;
                }
                st.n += 1;
                st.last = ev.t;
            }
            if !ev.label.starts_with('q') {
                say(&format!("ev {ctag} {} t={} pid={} name={:?}", ev.label, ev.t, ev.pid, ev.name));
            }
        }
    });
    let uia = make_uia(&flags);
    let mut keep = Vec::new();
    for spec in &args[2..] {
        register(&uia, spec, &mut keep);
    }
    say(&format!("client-ready {tag}"));
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        match line.trim() {
            "stats" => {
                let g = STATS.lock().unwrap();
                let mut parts: Vec<String> = g
                    .as_ref()
                    .unwrap()
                    .iter()
                    .map(|(l, s)| format!("{l}=n{}:first{}:last{}", s.n, s.first, s.last))
                    .collect();
                parts.sort();
                say(&format!("stats {tag} {}", parts.join(" ")));
            }
            "reset" => {
                STATS.lock().unwrap().as_mut().unwrap().clear();
                say(&format!("reset {tag}"));
            }
            "cpu" => say(&cpu_line(&tag)),
            "quit" => break,
            other => say(&format!("unknown {other}")),
        }
    }
    unsafe {
        let _ = uia.RemoveAllEventHandlers();
    }
    drop(keep);
    std::process::exit(0);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
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
        "findchild" => {
            let w = wide(&args[2]);
            let h = unsafe { FindWindowExW(Some(hwnd_of(&args[1])), None, PCWSTR(w.as_ptr()), PCWSTR::null()) }.unwrap_or_default();
            println!("{}", h.0 as isize);
        }
        "winevent" => {
            let h = hwnd_of(&args[1]);
            let ev = u32::from_str_radix(args[2].trim_start_matches("0x"), 16).unwrap();
            unsafe { NotifyWinEvent(ev, h, args[3].parse().unwrap(), args[4].parse().unwrap()) };
            println!("winevent sent t={}", now_us());
        }
        other => panic!("unknown command {other}"),
    }
}

