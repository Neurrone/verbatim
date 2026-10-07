//! Window class names as NVDA compares them: normalized, so that a control
//! wrapped by Windows Forms or ATL, or a third-party class compatible with a
//! standard one, is handled as the control it is (NVDA's
//! `normalizeWindowClassName`). Every class-based rule in this crate, such as
//! the `SysTreeView32` and `SysListView32` handling in [`crate::acquire`],
//! and the outpost's backend arbitration compare the normalized name, so a
//! Windows Forms tree view (`WindowsForms10.SysTreeView32.app.0.…`) gets the
//! tree view handling a native one does.

/// The normalized class name of the window `hwnd`; empty when it names no
/// window. A local call, safe even against a hung window.
#[must_use]
pub fn normalized_class_of(hwnd: isize) -> String {
    normalize_class_name(&crate::window::class_name(hwnd))
}

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
}
