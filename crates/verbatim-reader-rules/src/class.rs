//! Window classes as NVDA judges them before reading a window: class
//! names normalized as NVDA normalizes them ([`normalize_class_name`]),
//! so a control wrapped by Windows Forms or ATL, or a third-party class
//! compatible with a standard one, is handled as the control it is; and
//! the verdict NVDA's class lists give a window ([`class_verdict`]), which
//! every outpost's arbitrator and the focus listener judge by, so the two
//! always agree. A function of class names alone: the names are read by
//! the caller (`verbatim_win32::window::window_classes`).

/// A window class name with the parts NVDA disregards removed, mapped to the
/// well-known class it is compatible with: a Windows Forms class name is cut
/// down to the control class it wraps, an `ATL:` prefix is dropped, and the
/// result, or failing that the name itself, is looked up in NVDA's class
/// map, so a Delphi `TEdit` or a Windows Forms edit is arbitrated as an
/// `Edit`.
#[must_use]
pub fn normalize_class_name(raw: &str) -> String {
    if let Some(mapped) = mapped_class(raw) {
        return mapped.to_owned();
    }
    let unwrapped = windows_forms_class(raw).or_else(|| raw.strip_prefix("ATL:"));
    match unwrapped {
        Some(inner) => mapped_class(inner).unwrap_or(inner).to_owned(),
        None => raw.to_owned(),
    }
}

/// The control class inside a Windows Forms class name: `EDIT` in
/// `WindowsForms10.EDIT.app.0.141b42a_r9_ad1`. NVDA's pattern is
/// `WindowsForms`, digits, a dot, then the class up to the last `.app.`.
fn windows_forms_class(raw: &str) -> Option<&str> {
    let rest = raw.strip_prefix("WindowsForms")?;
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = rest.strip_prefix('.')?;
    let end = rest.rfind(".app.")?;
    Some(&rest[..end])
}

fn mapped_class(class: &str) -> Option<&'static str> {
    CLASS_MAP
        .iter()
        .find(|(from, _)| *from == class)
        .map(|(_, to)| *to)
}

/// NVDA's `windowClassMap` (`nvda/source/NVDAObjects/window/__init__.py`):
/// class names mapped to the well-known class they are compatible with.
const CLASS_MAP: &[(&str, &str)] = &[
    ("EDIT", "Edit"),
    ("TTntEdit.UnicodeClass", "Edit"),
    ("TMaskEdit", "Edit"),
    ("TTntMemo.UnicodeClass", "Edit"),
    ("TRichEdit", "RichEdit20"),
    ("TRichViewEdit", "Edit"),
    ("TInEdit.UnicodeClass", "Edit"),
    ("TInEdit", "Edit"),
    ("TEdit", "Edit"),
    ("TFilenameEdit", "Edit"),
    ("TSpinEdit", "Edit"),
    ("ThunderRT6TextBox", "Edit"),
    ("TMemo", "Edit"),
    ("RICHEDIT", "RichEdit"),
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
    ("TRichEditViewer", "RichEdit"),
    ("WFMAINRE", "RichEdit20"),
    ("RichEdit20A", "RichEdit20"),
    ("RichEdit20W", "RichEdit20"),
    ("TChatRichEdit", "RichEdit20"),
    ("TAccessibleEdit", "Edit"),
    ("TskRichEdit.UnicodeClass", "RichEdit20"),
    ("RichEdit20WPT", "RichEdit20"),
    ("RICHEDIT60W", "RICHEDIT50W"),
    ("TChatRichEdit.UnicodeClass", "RichEdit20"),
    ("TMyRichEdit", "RichEdit20"),
    ("TExRichEdit", "RichEdit20"),
    ("RichTextWndClass", "RichEdit20"),
    ("TSRichEdit", "RichEdit20"),
    ("TRxRichEdit", "RichEdit20"),
    ("ScintillaWindowImpl", "Scintilla"),
    ("RICHEDIT60W_WLXPRIVATE", "RICHEDIT50W"),
    ("TNumEdit", "Edit"),
    ("TAccessibleRichEdit", "RichEdit20"),
];

/// The classification a window's class name yields before any probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassVerdict {
    /// A "good" class: always UIA.
    Uia,
    /// A "bad" class: always MSAA.
    NonUia,
    /// Not decided by class; needs the probe.
    Unknown,
}

/// The verdict NVDA's class lists give a window of classes `classes`, by
/// the tables every outpost's arbitrator is built from and with the same
/// code, which the focus listener skips a read by
/// (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
/// fact", the verdicts): a function of the class names alone, with no
/// call and nothing kept, so the listener and the outposts always agree
/// on it.
#[must_use]
pub fn class_verdict(classes: &WindowClasses) -> ClassVerdict {
    classify_with(
        |class| GOOD_UIA_CLASSES.contains(&class),
        |class| BAD_UIA_CLASSES.contains(&class),
        classes,
    )
}

/// The class verdict, with `good` and `bad` the class lists: the good list
/// and the shell first, as in NVDA.
#[must_use]
pub fn classify_with(
    good: impl Fn(&str) -> bool,
    bad: impl Fn(&str) -> bool,
    classes: &WindowClasses,
) -> ClassVerdict {
    if good(&classes.normalized) || classes.is_shell() {
        ClassVerdict::Uia
    } else if bad(&classes.normalized) {
        ClassVerdict::NonUia
    } else {
        ClassVerdict::Unknown
    }
}

/// The class names arbitration decides on, which the caller reads with
/// inexpensive local calls safe on any thread
/// (`verbatim_win32::window::window_classes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowClasses {
    /// The window's own class, as Windows reports it.
    pub raw: String,
    /// The window's class normalized as NVDA normalizes it
    /// ([`normalize_class_name`]).
    pub normalized: String,
    /// The class of the window's root ancestor (itself, for a top-level
    /// window).
    pub root: String,
}

impl WindowClasses {
    /// The classes of a window of class `raw` whose root ancestor is of
    /// class `root`.
    #[must_use]
    pub fn new(raw: &str, root: &str) -> Self {
        Self {
            raw: raw.to_owned(),
            normalized: normalize_class_name(raw),
            root: root.to_owned(),
        }
    }

    /// Whether this is a Windows 11 shell window NVDA reads through UIA: its
    /// root ancestor is one of the shell's top-level windows, and it is not
    /// the Start button, which reports itself through MSAA on some systems.
    fn is_shell(&self) -> bool {
        SHELL_ROOT_CLASSES.contains(&self.root.as_str()) && self.raw != "Start"
    }
}

/// Classes that are always treated as UIA, before any probe runs: NVDA's
/// `goodUIAWindowClassNames` tuple (`nvda/source/UIAHandler/__init__.py`),
/// classes whose windows are always native UIA even when the probe would
/// miss them.
pub const GOOD_UIA_CLASSES: &[&str] = &[
    // Windows Defender Application Guard windows are always native UIA.
    "RAIL_WINDOW",
    // WinUI 3 top-level pane.
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    // Windows Terminal: its top-level window reports no server-side
    // provider; the XAML island child that hosts its content does.
    "CASCADIA_HOSTING_WINDOW_CLASS",
];

/// The Windows 11 shell's top-level windows, from NVDA's Explorer app
/// module's `isGoodUIAWindow` (`nvda/source/appModules/explorer.py`): a
/// window under one of these (its root ancestor's class), other than the
/// Start button, is UIA, as NVDA reclassifies it on Windows 11 (roadmap M3's
/// shell-support bullet: window-classification rules as generic core
/// policy, not per-app patches). NVDA's `ApplicationFrameWindow` entry (the
/// emoji-panel workaround) is deliberately not carried: it predates the
/// probe handling those windows correctly.
pub const SHELL_ROOT_CLASSES: &[&str] = &[
    // The shell UI root: Start, Search, Widgets, and the taskbar's own
    // elements.
    "Shell_TrayWnd",
    // NVDA explorer.py isGoodUIAWindow: the language/input switcher.
    "Shell_InputSwitchTopLevelWindow",
    // NVDA explorer.py isGoodUIAWindow: Task View and snap layouts.
    "XamlExplorerHostIslandWindow",
    // NVDA explorer.py isGoodUIAWindow: the redesigned systray overflow
    // (Windows 11 22H2 and later).
    "TopLevelWindowForOverflowXamlIsland",
];

/// Classes whose UIA implementations interfere with MSAA and are forced to
/// MSAA (seeded from NVDA's `badUIAWindowClassNames`).
pub const BAD_UIA_CLASSES: &[&str] = &[
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_names_are_normalized_as_nvda_normalizes_them() {
        assert_eq!(normalize_class_name("TEdit"), "Edit");
        assert_eq!(normalize_class_name("RichEdit20W"), "RichEdit20");
        assert_eq!(
            normalize_class_name("WindowsForms10.EDIT.app.0.141b42a_r9_ad1"),
            "Edit"
        );
        assert_eq!(
            normalize_class_name("WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1"),
            "SysListView32"
        );
        assert_eq!(
            normalize_class_name("WindowsForms10.SysTreeView32.app.0.141b42a_r9_ad1"),
            "SysTreeView32"
        );
        assert_eq!(normalize_class_name("ATL:SysListView32"), "SysListView32");
        assert_eq!(normalize_class_name("ATL:RichEdit20W"), "RichEdit20");
        assert_eq!(normalize_class_name("Notepad"), "Notepad");
        assert_eq!(
            normalize_class_name("WindowsForms10.Window"),
            "WindowsForms10.Window"
        );
    }

    /// Pins the good-class list to its two NVDA sources, so a future NVDA
    /// sync is a diff of two lists: `goodUIAWindowClassNames` in
    /// `nvda/source/UIAHandler/__init__.py`, and the Windows 11 shell class
    /// tuple inside `isGoodUIAWindow` in
    /// `nvda/source/appModules/explorer.py`.
    #[test]
    fn good_class_list_matches_its_nvda_sources() {
        // nvda/source/UIAHandler/__init__.py, goodUIAWindowClassNames.
        assert_eq!(
            GOOD_UIA_CLASSES,
            [
                "RAIL_WINDOW",
                "Microsoft.UI.Content.DesktopChildSiteBridge",
                "CASCADIA_HOSTING_WINDOW_CLASS",
            ]
        );
        // nvda/source/appModules/explorer.py, isGoodUIAWindow's Windows 11
        // shell tuple.
        assert_eq!(
            SHELL_ROOT_CLASSES,
            [
                "Shell_TrayWnd",
                "Shell_InputSwitchTopLevelWindow",
                "XamlExplorerHostIslandWindow",
                "TopLevelWindowForOverflowXamlIsland",
            ]
        );
    }

    /// Pins the bad-class list to NVDA's `badUIAWindowClassNames` in
    /// `nvda/source/UIAHandler/__init__.py`, order and all, so a future
    /// NVDA sync is a straight diff.
    #[test]
    fn bad_class_list_matches_nvda_bad_uia_window_class_names() {
        let expected = [
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
        assert_eq!(BAD_UIA_CLASSES, expected.as_slice());
    }
}
