//! Window classes as NVDA judges them before reading a window: the verdict
//! NVDA's class lists (`verbatim_model::class`) give a window
//! ([`class_verdict`]), which every outpost's arbitrator and the focus
//! listener judge by, so the two always agree, and the checks NVDA makes
//! after a probe finds a provider. A function of class names alone: the
//! names are read by the caller (`verbatim_win32::window::window_classes`).

use verbatim_model::class::{BAD, GOOD, SHELL_ROOTS, normalize};

/// Whether `normalized`, a class name `verbatim_model::class::normalize`
/// gave, is one
/// NVDA recognises as a progress bar by its class: the common control, and
/// Delphi's `TProgressBar` (`NVDAObjects/IAccessible/__init__.py` 2887,
/// `UIAHandler/__init__.py` 115).
#[must_use]
pub fn is_progress_bar_class(normalized: &str) -> bool {
    matches!(normalized, "msctls_progress32" | "TProgressBar")
}

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
/// (`books/dev/src/reference/adr/0001-focus.md`, "One judge per fact"): a function
/// of the class names alone, with no
/// call and nothing kept, so the listener and the outposts always agree
/// on it.
#[must_use]
pub fn class_verdict(classes: &WindowClasses) -> ClassVerdict {
    classify_with(
        |class| GOOD.contains(&class),
        |class| BAD.contains(&class),
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

/// A check NVDA makes on a window that has a UIA provider before using it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostProbeCheck {
    /// A console: its provider is used only when its text reports
    /// formatting, which the consoles of current Windows do and older
    /// consoles, with incomplete providers, do not.
    Console,
    /// A list view: its provider is used only when it comes from Windows
    /// Forms, whose list views have no MSAA implementation; elsewhere the
    /// MSAA implementation is the more complete one.
    WindowsFormsListView,
}

/// The check NVDA makes on a window of these classes after its probe finds a
/// provider, if it makes one. NVDA's Word, Excel, and Chromium exceptions
/// apply only when it has injected its in-process helper, so they wait for
/// Verbatim's (decision D2, M7).
#[must_use]
pub fn post_probe_check(classes: &WindowClasses) -> Option<PostProbeCheck> {
    match classes.normalized.as_str() {
        "ConsoleWindowClass" => Some(PostProbeCheck::Console),
        "SysListView32" => Some(PostProbeCheck::WindowsFormsListView),
        _ => None,
    }
}

/// Whether a window of raw class `raw` comes from Windows Forms, whose
/// window classes all begin `WindowsForms10.`. It agreed with the UIA
/// framework id `WinForm` on every one of over 170 windows of Windows Forms,
/// WPF, Qt, Firefox and Win32 applications
/// (`books/dev/src/explanation/design/listener-without-connections.md`, section 3, step 5).
#[must_use]
pub fn has_windows_forms_class(raw: &str) -> bool {
    raw.starts_with("WindowsForms10.")
}

/// The answer to `check` for a window of classes `classes`: `Some(true)`
/// when its provider is used, `Some(false)` when it is excluded, `None` when
/// the application did not answer. A console asks `console`; a list view is
/// answered from its class name alone, with no call into the application,
/// by an outpost and the listener alike
/// (`books/dev/src/explanation/design/listener-without-connections.md`, section 3, step 5).
#[must_use]
pub fn provider_usable(
    check: PostProbeCheck,
    classes: &WindowClasses,
    console: impl FnOnce() -> Option<bool>,
) -> Option<bool> {
    match check {
        PostProbeCheck::Console => console(),
        PostProbeCheck::WindowsFormsListView => Some(has_windows_forms_class(&classes.raw)),
    }
}

/// Whether the focus listener counts a window of classes `classes`, whose
/// raw probe answered `raw_provider`, as read through UI Automation: the
/// probe's answer with NVDA's checks after it
/// (`nvda/source/UIAHandler/__init__.py` 1344 to 1361), the one function
/// its check threads and its reading threads judge by, so the probe a
/// tell carries and the answer a value change is judged by agree. A list
/// view counts only when its raw class is a Windows Forms class
/// ([`provider_usable`]); a console counts with no check of its provider,
/// the listener's recorded limit, since the check needs a UI Automation
/// connection the listener does not make (the `value_change` module of
/// `verbatim-listener` records it). No call.
#[must_use]
pub fn listener_provider(classes: &WindowClasses, raw_provider: bool) -> bool {
    if !raw_provider {
        return false;
    }
    match post_probe_check(classes) {
        None | Some(PostProbeCheck::Console) => true,
        Some(check @ PostProbeCheck::WindowsFormsListView) => {
            provider_usable(check, classes, || None) == Some(true)
        }
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
    /// (`verbatim_model::class::normalize`).
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
            normalized: normalize(raw).to_owned(),
            root: root.to_owned(),
        }
    }

    /// Whether this is a Windows 11 shell window NVDA reads through UIA: its
    /// root ancestor is one of the shell's top-level windows, and it is not
    /// the Start button, which reports itself through MSAA on some systems.
    fn is_shell(&self) -> bool {
        SHELL_ROOTS.contains(&self.root.as_str()) && self.raw != "Start"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The normalised name decides, so a Windows Forms bar's long class name
    /// and the common control's are the same; an edit control's is not.
    #[test]
    fn a_bar_is_recognised_by_its_normalised_class() {
        use super::is_progress_bar_class;
        for class in [
            "msctls_progress32",
            "WindowsForms10.msctls_progress32.app.0.141b42a_r9_ad1",
            "TProgressBar",
        ] {
            assert!(is_progress_bar_class(normalize(class)), "{class}");
        }
        assert!(!is_progress_bar_class(normalize("Edit")));
    }

    #[test]
    fn consoles_and_list_views_are_checked_after_the_probe() {
        assert_eq!(
            post_probe_check(&WindowClasses::new(
                "ConsoleWindowClass",
                "ConsoleWindowClass"
            )),
            Some(PostProbeCheck::Console)
        );
        assert_eq!(
            post_probe_check(&WindowClasses::new(
                "WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1",
                "x"
            )),
            Some(PostProbeCheck::WindowsFormsListView)
        );
        assert_eq!(
            post_probe_check(&WindowClasses::new("Notepad", "Notepad")),
            None
        );
    }

    #[test]
    fn a_list_view_is_windows_forms_by_its_class_name() {
        assert!(has_windows_forms_class(
            "WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1"
        ));
        assert!(!has_windows_forms_class("SysListView32"));
    }

    #[test]
    fn a_list_views_provider_is_judged_by_its_class_alone() {
        let console_never_asked = || -> Option<bool> { panic!("a list view asked the console") };
        let forms = WindowClasses::new("WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1", "x");
        let comctl = WindowClasses::new("SysListView32", "x");
        assert_eq!(
            provider_usable(
                PostProbeCheck::WindowsFormsListView,
                &forms,
                console_never_asked
            ),
            Some(true)
        );
        assert_eq!(
            provider_usable(
                PostProbeCheck::WindowsFormsListView,
                &comctl,
                console_never_asked
            ),
            Some(false)
        );
    }

    /// The listener's answer: no provider is never UI Automation; a list
    /// view's provider counts only from Windows Forms; a console's counts
    /// with no check; any other window's counts as the probe found it.
    #[test]
    fn the_listener_counts_a_provider_after_nvdas_checks() {
        let forms = WindowClasses::new("WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1", "x");
        let comctl = WindowClasses::new("SysListView32", "x");
        let console = WindowClasses::new("ConsoleWindowClass", "ConsoleWindowClass");
        let other = WindowClasses::new("Notepad", "Notepad");
        let answers: Vec<(bool, bool)> = [&forms, &comctl, &console, &other]
            .iter()
            .map(|classes| {
                (
                    listener_provider(classes, true),
                    listener_provider(classes, false),
                )
            })
            .collect();
        assert_eq!(
            answers,
            [(true, false), (false, false), (true, false), (true, false)]
        );
    }

    #[test]
    fn a_consoles_provider_is_judged_by_its_console_check() {
        let classes = WindowClasses::new("ConsoleWindowClass", "ConsoleWindowClass");
        assert_eq!(
            provider_usable(PostProbeCheck::Console, &classes, || None),
            None
        );
        assert_eq!(
            provider_usable(PostProbeCheck::Console, &classes, || Some(false)),
            Some(false)
        );
    }
}
