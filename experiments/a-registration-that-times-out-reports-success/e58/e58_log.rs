//! e58's log of what reaches the window thread: each message posted to it
//! as the thread takes it, and each message another thread sends it, from
//! `release` on (`crate::hits::log`).

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CWPSTRUCT, CallNextHookEx, GetClassNameW, HC_ACTION, MSG, PM_REMOVE, SetWindowsHookExW,
    WH_CALLWNDPROC, WH_GETMESSAGE,
};

/// Hooks the window thread, which calls this.
pub(crate) fn install() {
    let thread = unsafe { GetCurrentThreadId() };
    unsafe { SetWindowsHookExW(WH_GETMESSAGE, Some(get_message), None, thread) }
        .expect("e58's message hook");
    unsafe { SetWindowsHookExW(WH_CALLWNDPROC, Some(call_wnd_proc), None, thread) }
        .expect("e58's sent message hook");
}

fn class(hwnd: HWND) -> String {
    let mut class = [0u16; 256];
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    String::from_utf16_lossy(&class[..usize::try_from(length).unwrap_or(0)])
}

unsafe extern "system" fn get_message(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if u32::try_from(code) == Ok(HC_ACTION) && wparam.0 == PM_REMOVE.0 as usize {
        let message = unsafe { &*(lparam.0 as *const MSG) };
        crate::hits::log(&format!(
            "posted {:#06x} to {}",
            message.message,
            class(message.hwnd)
        ));
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn call_wnd_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A nonzero wParam: sent by another thread.
    if u32::try_from(code) == Ok(HC_ACTION) && wparam.0 != 0 {
        let sent = unsafe { &*(lparam.0 as *const CWPSTRUCT) };
        crate::hits::log(&format!("sent {:#06x} to {}", sent.message, class(sent.hwnd)));
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
