//! The window a class path names, the request list's `Window` subject: its
//! first class a top-level window's, each later one a window inside the
//! one before at any depth, as the taskbar's notification area and the
//! tray overflow flyout are named for the notification area list.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, FindWindowW};
use windows::core::{HSTRING, PCWSTR};

/// The window a class path names (the request list's `Window` subject): its
/// first class a top-level window's (`FindWindow`), each later one a
/// window inside the one before at any depth, searched breadth first.
pub(super) fn class_path_window(classes: &[String]) -> Option<isize> {
    let (first, rest) = classes.split_first()?;
    let mut found = find_top_level(first)?;
    for class in rest {
        let wanted = HSTRING::from(class.as_str());
        let mut level = vec![found];
        found = 'search: loop {
            let mut below = Vec::new();
            for parent in level {
                if let Some(hwnd) = child_window(parent, None, &wanted) {
                    break 'search hwnd;
                }
                let mut child = None;
                while let Some(next) = child_window(parent, child, &HSTRING::new()) {
                    below.push(next);
                    child = Some(next);
                }
            }
            if below.is_empty() {
                return None;
            }
            level = below;
        };
    }
    Some(found.0 as isize)
}

/// The child window of `parent` after `after` (the first with `None`) of
/// class `class`, any class when it is empty.
fn child_window(parent: HWND, after: Option<HWND>, class: &HSTRING) -> Option<HWND> {
    let class = if class.is_empty() {
        PCWSTR::null()
    } else {
        PCWSTR(class.as_ptr())
    };
    // SAFETY: FindWindowExW reads window state only; a stale handle yields
    // an error, not a fault, and `class` outlives the call.
    unsafe { FindWindowExW(Some(parent), after, class, PCWSTR::null()) }
        .ok()
        .filter(|hwnd| !hwnd.is_invalid())
}

/// Finds a top-level window by class name.
fn find_top_level(class: &str) -> Option<HWND> {
    // SAFETY: FindWindowW reads window state only.
    unsafe { FindWindowW(&HSTRING::from(class), PCWSTR::null()) }
        .ok()
        .filter(|hwnd| !hwnd.is_invalid())
}
