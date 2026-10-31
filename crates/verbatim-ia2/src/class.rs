//! Window class names as NVDA compares them: normalized
//! (`verbatim_reader_rules::class::normalize_class_name`), so that a
//! control wrapped by Windows Forms or ATL, or a third-party class
//! compatible with a standard one, is handled as the control it is. Every
//! class-based rule in this crate, such as the `SysTreeView32` and
//! `SysListView32` handling in [`crate::acquire`], compares the normalized
//! name, so a Windows Forms tree view (`WindowsForms10.SysTreeView32.app.0.…`)
//! gets the tree view handling a native one does.

/// The normalized class name of the window `hwnd`; empty when it names no
/// window. A local call, safe even against a hung window.
#[must_use]
pub fn normalized_class_of(hwnd: isize) -> String {
    verbatim_reader_rules::class::normalize_class_name(&verbatim_win32::window::window_class_name(
        hwnd,
    ))
}
