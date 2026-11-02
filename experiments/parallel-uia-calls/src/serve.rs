//! The synthetic provider process, modelled on the collide tool's server
//! (`experiments/a-new-connection-fails-while-another-processs-remote-operation-is-queued/`):
//! top-level windows, each answering UIA's root object id with a
//! server-side provider made on its window's thread, which is in a
//! single-threaded apartment. What it adds: windows on chosen UI threads,
//! three provider threading modes, a per-window delay inside the name read
//! (sleeping, or waiting while COM dispatches incoming calls), and counters
//! of concurrent provider entries and of entries off the window's thread.
//!
//! serve layout=0,0,1 mode=sta|agile|nocom [msaa=bridge]
//! - layout: window i is made on UI thread layout[i]; titled `puia-serve-<i>`.
//! - sta: ServerSideProvider | UseComThreading, a non-agile object, so COM
//!   brings every call to the window's thread (as mockapp and the collide
//!   server do). agile: the same flags, an agile object, so COM calls it on
//!   whichever thread the call arrives. nocom: ServerSideProvider alone,
//!   leaving threading to UIA.
//!
//! Window messages: WM_APP+1 (posted, wParam ms) busies the thread without
//! pumping; WM_APP+2 returns and resets the counters; WM_APP+4 (wParam ms,
//! lParam 0 sleep, 1 pump, or 2 sleep once) sets this window's name-read
//! delay; WM_APP+5 (wParam 0) clears the provider-call log, and (wParam 1)
//! writes it to the file named by `log=` and clears it. The log holds each
//! name read's QueryPerformanceCounter values at entry and exit and its
//! thread, so a client can split a call into its parts on one clock.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::SeqCst};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COWAIT_DISPATCH_CALLS, COWAIT_DISPATCH_WINDOW_MESSAGES,
    CoInitializeEx, CoWaitForMultipleHandles,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThreadId};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BSTR, IUnknown, PCWSTR, Result, w};
use windows_core::implement;

use crate::{Args, get};

const MAX: usize = 8;
static UI_TID: [AtomicU32; MAX] = [const { AtomicU32::new(0) }; MAX];
static DELAY: [AtomicU64; MAX] = [const { AtomicU64::new(0) }; MAX];
static CUR: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static OFF: AtomicU64 = AtomicU64::new(0);
static mut MODE: u8 = 0; // 0 sta, 1 agile, 2 nocom
// msaa=bridge: OBJID_CLIENT is answered with UiaReturnRawElementProvider
// too, so an MSAA client gets UIA's MSAA bridge over COM instead of
// oleacc's own window proxy.
static mut BRIDGE: bool = false;
static LOG: std::sync::Mutex<Vec<(i64, i64, u32)>> = std::sync::Mutex::new(Vec::new());
static mut LOG_PATH: String = String::new();

fn qpc() -> i64 {
    let mut v = 0i64;
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut v) };
    v
}

#[inline(never)]
fn name_read(index: usize) -> Result<VARIANT> {
    let enter = qpc();
    let now = CUR.fetch_add(1, SeqCst) + 1;
    PEAK.fetch_max(now, SeqCst);
    CALLS.fetch_add(1, SeqCst);
    if unsafe { GetCurrentThreadId() } != UI_TID[index].load(SeqCst) {
        OFF.fetch_add(1, SeqCst);
    }
    let mut d = DELAY[index].load(SeqCst);
    if d >> 32 == 2 {
        // Sleep once: only the first name read after the setting sleeps.
        d = DELAY[index].swap(0, SeqCst);
    }
    let (ms, pump) = (d & 0xffff_ffff, d >> 32 == 1);
    if ms > 0 {
        if pump {
            // Waits while COM dispatches incoming calls and sent messages,
            // as a provider waiting on its own outgoing call does.
            let ev = unsafe { CreateEventW(None, true, false, None) }.unwrap();
            let _ = unsafe {
                CoWaitForMultipleHandles(
                    (COWAIT_DISPATCH_CALLS.0 | COWAIT_DISPATCH_WINDOW_MESSAGES.0) as u32,
                    ms as u32,
                    &[ev],
                )
            };
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(ev) };
        } else {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
    CUR.fetch_sub(1, SeqCst);
    let exit = qpc();
    LOG.lock().unwrap().push((enter, exit, unsafe { GetCurrentThreadId() }));
    Ok(VARIANT::from(BSTR::from(format!("serve window {index}"))))
}

/// The invoke pattern's Invoke: raises the invoked event, as a button does.
#[inline(never)]
fn invoked(p: IRawElementProviderSimple) -> Result<()> {
    unsafe { UiaRaiseAutomationEvent(&p, UIA_Invoke_InvokedEventId) }
}

fn options() -> ProviderOptions {
    if unsafe { MODE } == 2 {
        ProviderOptions_ServerSideProvider
    } else {
        ProviderOptions(ProviderOptions_ServerSideProvider.0 | ProviderOptions_UseComThreading.0)
    }
}

#[implement(IRawElementProviderSimple, IInvokeProvider, Agile = false)]
struct StaProvider {
    hwnd: isize,
    index: usize,
}
#[implement(IRawElementProviderSimple, IInvokeProvider)]
struct AgileProvider {
    hwnd: isize,
    index: usize,
}

fn property(index: usize, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
    if id == UIA_NamePropertyId { name_read(index) } else { Ok(VARIANT::default()) }
}

impl IInvokeProvider_Impl for StaProvider_Impl {
    fn Invoke(&self) -> Result<()> { invoked(windows_core::ComObjectInterface::<IRawElementProviderSimple>::as_interface_ref(self).to_owned()) }
}
impl IRawElementProviderSimple_Impl for StaProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> { Ok(options()) }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_InvokePatternId { Ok(windows_core::ComObjectInterface::<IInvokeProvider>::as_interface_ref(self).to_owned().into()) } else { Err(windows_core::Error::empty()) }
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> { property(self.index, id) }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as *mut _)) }
    }
}
impl IInvokeProvider_Impl for AgileProvider_Impl {
    fn Invoke(&self) -> Result<()> { invoked(windows_core::ComObjectInterface::<IRawElementProviderSimple>::as_interface_ref(self).to_owned()) }
}
impl IRawElementProviderSimple_Impl for AgileProvider_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> { Ok(options()) }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        if id == UIA_InvokePatternId { Ok(windows_core::ComObjectInterface::<IInvokeProvider>::as_interface_ref(self).to_owned().into()) } else { Err(windows_core::Error::empty()) }
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> { property(self.index, id) }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        unsafe { UiaHostProviderFromHwnd(HWND(self.hwnd as *mut _)) }
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let index = unsafe { GetWindowLongPtrW(h, GWLP_USERDATA) } as usize;
    match msg {
        WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId || (unsafe { BRIDGE } && lp.0 as i32 == OBJID_CLIENT.0) => {
            let p: IRawElementProviderSimple = if unsafe { MODE } == 1 {
                AgileProvider { hwnd: h.0 as isize, index }.into()
            } else {
                StaProvider { hwnd: h.0 as isize, index }.into()
            };
            unsafe { UiaReturnRawElementProvider(h, wp, lp, &p) }
        }
        m if m == WM_APP + 1 => {
            std::thread::sleep(std::time::Duration::from_millis(wp.0 as u64));
            LRESULT(0)
        }
        m if m == WM_APP + 2 => {
            let r = PEAK.swap(0, SeqCst) | (OFF.swap(0, SeqCst) << 16) | (CALLS.swap(0, SeqCst) << 40);
            LRESULT(r as isize)
        }
        m if m == WM_APP + 5 => {
            let mut l = LOG.lock().unwrap();
            let n = l.len();
            if wp.0 == 1 {
                let text: String = l.iter().map(|(a, b, t)| format!("{a} {b} {t}
")).collect();
                std::fs::write(unsafe { &*std::ptr::addr_of!(LOG_PATH) }, text).unwrap();
            }
            l.clear();
            LRESULT(n as isize)
        }
        m if m == WM_APP + 4 => {
            DELAY[index].store(wp.0 as u64 | ((lp.0 as u64) << 32), SeqCst);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
}

pub fn run(a: &Args) {
    unsafe {
        MODE = match get(a, "mode", "sta") {
            "agile" => 1,
            "nocom" => 2,
            _ => 0,
        };
        BRIDGE = get(a, "msaa", "window") == "bridge";
        LOG_PATH = get(a, "log", "puia-serve-log.txt").to_string();
    };
    LOG.lock().unwrap().reserve(1 << 16);
    let layout: Vec<usize> = get(a, "layout", "0,0,1").split(',').map(|s| s.parse().unwrap()).collect();
    let nthreads = layout.iter().max().unwrap() + 1;
    let mut joins = Vec::new();
    for t in 0..nthreads {
        let mine: Vec<usize> = (0..layout.len()).filter(|i| layout[*i] == t).collect();
        joins.push(std::thread::spawn(move || unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap();
            let inst = GetModuleHandleW(None).unwrap();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: inst.into(),
                lpszClassName: w!("PuiaServe"),
                ..Default::default()
            };
            RegisterClassW(&wc);
            for i in &mine {
                UI_TID[*i].store(GetCurrentThreadId(), SeqCst);
                let title: Vec<u16> = format!("puia-serve-{i}").encode_utf16().chain([0]).collect();
                let h = CreateWindowExW(
                    WINDOW_EX_STYLE(0), w!("PuiaServe"), PCWSTR(title.as_ptr()),
                    WS_OVERLAPPEDWINDOW, 0, 0, 300, 200, None, None, Some(inst.into()), None,
                ).unwrap();
                SetWindowLongPtrW(h, GWLP_USERDATA, *i as isize);
                println!("window {i} hwnd={} tid={}", h.0 as isize, GetCurrentThreadId());
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                DispatchMessageW(&msg);
            }
        }));
    }
    println!("ready");
    let _ = HANDLE::default();
    // Runs until killed.
    for j in joins {
        let _ = j.join();
    }
}
