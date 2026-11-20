//! NVDA's window class lists and its normalization of a class name, which
//! decide whether a window is read through UI Automation or MSAA before any
//! probe, written once from `docs/parity/backends.md` ("The class lists
//! the per-window verdict reads") for Core and both readers.

use crate::Backend;

/// NVDA's class map, each name with the name it is normalized to.
pub const CLASS_MAP: &[(&str, &str)] = &[
    ("EDIT", "Edit"),
    ("TTntEdit.UnicodeClass", "Edit"),
    ("TMaskEdit", "Edit"),
    ("TTntMemo.UnicodeClass", "Edit"),
    ("TRichViewEdit", "Edit"),
    ("TInEdit.UnicodeClass", "Edit"),
    ("TInEdit", "Edit"),
    ("TEdit", "Edit"),
    ("TFilenameEdit", "Edit"),
    ("TSpinEdit", "Edit"),
    ("ThunderRT6TextBox", "Edit"),
    ("TMemo", "Edit"),
    ("TPasswordEdit", "Edit"),
    ("THppEdit.UnicodeClass", "Edit"),
    ("TUnicodeTextEdit.UnicodeClass", "Edit"),
    ("TTextEdit", "Edit"),
    ("TPropInspEdit", "Edit"),
    ("TFilterbarEdit.UnicodeClass", "Edit"),
    ("EditControl", "Edit"),
    ("TNavigableTntMemo.UnicodeClass", "Edit"),
    ("TNavigableTntEdit.UnicodeClass", "Edit"),
    ("TAltEdit.UnicodeClass", "Edit"),
    ("TAltEdit", "Edit"),
    ("TDefEdit", "Edit"),
    ("TAccessibleEdit", "Edit"),
    ("TNumEdit", "Edit"),
    ("TRichEdit", "RichEdit20"),
    ("WFMAINRE", "RichEdit20"),
    ("RichEdit20A", "RichEdit20"),
    ("RichEdit20W", "RichEdit20"),
    ("TChatRichEdit", "RichEdit20"),
    ("TskRichEdit.UnicodeClass", "RichEdit20"),
    ("RichEdit20WPT", "RichEdit20"),
    ("TChatRichEdit.UnicodeClass", "RichEdit20"),
    ("TMyRichEdit", "RichEdit20"),
    ("TExRichEdit", "RichEdit20"),
    ("RichTextWndClass", "RichEdit20"),
    ("TSRichEdit", "RichEdit20"),
    ("TRxRichEdit", "RichEdit20"),
    ("TAccessibleRichEdit", "RichEdit20"),
    ("RICHEDIT", "RichEdit"),
    ("TRichEditViewer", "RichEdit"),
    ("RICHEDIT60W", "RICHEDIT50W"),
    ("RICHEDIT60W_WLXPRIVATE", "RICHEDIT50W"),
    ("ScintillaWindowImpl", "Scintilla"),
];

/// The classes read through UI Automation.
pub const GOOD: &[&str] = &[
    "RAIL_WINDOW",
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    "CASCADIA_HOSTING_WINDOW_CLASS",
];

/// The classes read through MSAA.
pub const BAD: &[&str] = &[
    "Microsoft.IME.CandidateWindow.View",
    "SysTreeView32",
    "WuDuiListView",
    "ComboBox",
    "msctls_progress32",
    "msctls_trackbar32",
    "Edit",
    "CommonPlacesWrapperWndClass",
    "SysMonthCal32",
    "SUPERGRID",
    "RichEdit",
    "RichEdit20",
    "RICHEDIT50W",
    "Button",
    "FoxitDocWnd",
    "MozillaWindowClass",
    "MozillaDropShadowWindowClass",
    "MozillaDialogClass",
    "MozillaContentWindowClass",
];

/// The root classes of the Windows 11 shell's windows that the Explorer
/// rule reads through UI Automation, as core policy for every process.
pub const SHELL_ROOTS: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_InputSwitchTopLevelWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// `class`'s inner name when it is a Windows Forms name (`WindowsForms`,
/// any digits, a dot, the inner name up to the last `.app.`, and anything
/// after) or an `ATL:` name; an empty inner name is none.
fn inner_name(class: &str) -> Option<&str> {
    let inner = if let Some(inner) = class.strip_prefix("ATL:") {
        inner
    } else {
        let rest = class
            .strip_prefix("WindowsForms")?
            .trim_start_matches(|character: char| character.is_ascii_digit())
            .strip_prefix('.')?;
        rest.get(..rest.rfind(".app.")?)?
    };
    (!inner.is_empty()).then_some(inner)
}

/// `class` normalized as NVDA normalizes a window's class name: replaced by
/// its target in the class map; otherwise a Windows Forms or `ATL:` name
/// reduced to its inner name, then looked up in the map; otherwise kept.
#[must_use]
pub fn normalize(class: &str) -> &str {
    let mapped = |name: &str| {
        CLASS_MAP
            .iter()
            .find(|(from, _)| *from == name)
            .map(|(_, to)| *to)
    };
    if let Some(to) = mapped(class) {
        return to;
    }
    match inner_name(class) {
        Some(inner) => mapped(inner).unwrap_or(inner),
        None => class,
    }
}

/// The verdict a window's classes give, in NVDA's order: a normalized
/// class on the good list is UI Automation; then the Explorer shell rule
/// (a window whose root's class is a shell root, other than the Start
/// button); then a normalized class on the bad list is MSAA; otherwise
/// none, and the window is probed.
#[must_use]
pub fn by_class(class: &str, root_class: &str) -> Option<Backend> {
    let class = normalize(class);
    if GOOD.contains(&class) {
        return Some(Backend::Uia);
    }
    if SHELL_ROOTS.contains(&root_class) && class != "Start" {
        return Some(Backend::Uia);
    }
    BAD.contains(&class).then_some(Backend::Msaa)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapped_class_is_normalized_to_its_target() {
        assert_eq!(normalize("TEdit"), "Edit");
        assert_eq!(normalize("RichEdit20W"), "RichEdit20");
        assert_eq!(normalize("RICHEDIT60W"), "RICHEDIT50W");
        assert_eq!(normalize("ScintillaWindowImpl"), "Scintilla");
    }

    #[test]
    fn a_windows_forms_or_atl_class_is_reduced_to_its_inner_name_then_mapped() {
        assert_eq!(
            normalize("WindowsForms10.EDIT.app.0.141b42a_r9_ad1"),
            "Edit"
        );
        assert_eq!(
            normalize("WindowsForms10.SysTreeView32.app.0.1"),
            "SysTreeView32"
        );
        assert_eq!(normalize("WindowsForms.EDIT.app.0"), "Edit");
        assert_eq!(normalize("WindowsForms123.EDIT.app.0"), "Edit");
        assert_eq!(normalize("WindowsForms10.A.app.B.app.0"), "A.app.B");
        assert_eq!(normalize("ATL:EDIT"), "Edit");
        assert_eq!(normalize("ATL:SysListView32"), "SysListView32");
    }

    #[test]
    fn any_other_class_is_kept() {
        assert_eq!(
            normalize("Windows.UI.Core.CoreWindow"),
            "Windows.UI.Core.CoreWindow"
        );
        assert_eq!(normalize("WindowsForms10.EDIT"), "WindowsForms10.EDIT");
    }

    #[test]
    fn a_good_class_is_read_through_ui_automation() {
        assert_eq!(
            by_class(
                "CASCADIA_HOSTING_WINDOW_CLASS",
                "CASCADIA_HOSTING_WINDOW_CLASS"
            ),
            Some(Backend::Uia)
        );
        assert_eq!(by_class("RAIL_WINDOW", ""), Some(Backend::Uia));
    }

    #[test]
    fn a_bad_class_is_read_through_msaa_once_normalized() {
        assert_eq!(by_class("Button", "#32770"), Some(Backend::Msaa));
        assert_eq!(by_class("TEdit", "TForm1"), Some(Backend::Msaa));
        assert_eq!(
            by_class("MozillaWindowClass", "MozillaWindowClass"),
            Some(Backend::Msaa)
        );
    }

    #[test]
    fn a_window_under_a_shell_root_is_read_through_ui_automation_but_the_start_button() {
        assert_eq!(
            by_class("Windows.UI.Input.InputSite.WindowClass", "Shell_TrayWnd"),
            Some(Backend::Uia)
        );
        assert_eq!(by_class("Button", "Shell_TrayWnd"), Some(Backend::Uia));
        assert_eq!(by_class("Start", "Shell_TrayWnd"), None);
        assert_eq!(
            by_class("Button", "XamlExplorerHostIslandWindow"),
            Some(Backend::Uia)
        );
    }

    #[test]
    fn a_class_on_no_list_gives_no_verdict() {
        assert_eq!(
            by_class("Windows.UI.Core.CoreWindow", "ApplicationFrameWindow"),
            None
        );
        assert_eq!(by_class("DirectUIHWND", "CabinetWClass"), None);
    }
}
