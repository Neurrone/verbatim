//! Per-window backend arbitration (architecture section 4).
//!
//! Mirrors NVDA's ladder, cheapest rung first, on the window's class name
//! normalized as NVDA normalizes it ([`normalize_class_name`]): a
//! config-overridable "good" class list forces UIA, and so does a Windows 11
//! shell window, recognized by its root ancestor's class; a "bad" class list
//! (seeded from NVDA's, where UIA implementations are known to interfere with
//! MSAA) forces MSAA; otherwise the window is probed with
//! `UiaHasServerSideProvider`. A window with a provider is still set aside
//! for MSAA when its provider is one NVDA does not use: an old console's,
//! or a list view's outside Windows Forms ([`post_probe_check`]).
//! Class-list checks are fast local window calls; the probe blocks on the
//! target's message pump and runs only on the outpost's worker, under its
//! deadline ([`verbatim_uia::has_server_side_provider`]).
//!
//! A probe that finds a UIA provider is kept for the window's lifetime and
//! forgotten when the window is destroyed ([`Arbitrator::forget`]), as
//! decision D15 specifies: a server-side provider does not go away. Each
//! kept verdict also records the thread that owned the window, and a
//! window now owned by another thread is probed afresh, so a reused window
//! handle does not inherit a verdict when its destroy event was lost. A probe
//! that finds none is trusted for only [`NEGATIVE_VERDICT_LIFETIME`], NVDA's
//! cache period, and then probed again. The probe itself counts only the
//! window's own answer, so a busy window that has a provider is not
//! reported as having none ([`verbatim_uia::has_server_side_provider`]).
//!
//! The worker drops MSAA events whose window arbitrates to UIA and UIA events
//! whose window does not, so the two backends never both announce the same
//! change.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

use crate::outpost::window::{top_level_of, window_thread};
pub use verbatim_ia2::class::normalize_class_name;

/// How long a probe that found no UIA provider is trusted before the window
/// is probed again: NVDA's `isUIAWindow` cache period.
pub const NEGATIVE_VERDICT_LIFETIME: Duration = Duration::from_millis(500);

/// A kept probe result.
#[derive(Clone, Copy, Debug)]
enum Probed {
    /// The window has a UIA provider, for its whole lifetime.
    Uia,
    /// No provider was found at this time.
    NotUia(Instant),
    /// The window has a provider NVDA does not use, for its whole lifetime.
    Excluded,
}

/// The classification a window's class name yields before any probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassVerdict {
    /// A "good" class: always UIA.
    Uia,
    /// A "bad" class: always MSAA.
    NonUia,
    /// Not decided by class; needs the probe.
    Unknown,
}

/// Caches and decides the backend for each window of one application.
pub struct Arbitrator {
    good_classes: HashSet<String>,
    bad_classes: HashSet<String>,
    /// Probe results, each with the thread that owned the window when it
    /// was recorded: a UIA verdict until the window is destroyed, a non-UIA
    /// verdict for [`NEGATIVE_VERDICT_LIFETIME`].
    cache: HashMap<isize, (u32, Probed)>,
    /// A `SetBackendOverride` forcing every window: `Some(true)` for UIA,
    /// `Some(false)` for MSAA, `None` for normal arbitration.
    forced: Option<bool>,
}

impl Default for Arbitrator {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Arbitrator {
    /// Builds an arbitrator whose bad-class list is seeded from NVDA's, plus any
    /// `extra_good` classes from per-app configuration that force UIA.
    #[must_use]
    pub fn new(extra_good: &[&str]) -> Self {
        let bad_classes = BAD_UIA_CLASSES.iter().map(|s| (*s).to_owned()).collect();
        let good_classes = GOOD_UIA_CLASSES
            .iter()
            .chain(extra_good.iter())
            .map(|s| (*s).to_owned())
            .collect();
        Self {
            good_classes,
            bad_classes,
            cache: HashMap::new(),
            forced: None,
        }
    }

    /// Sets a `SetBackendOverride` command's forced backend: `Some(true)`
    /// forces UIA for every window, `Some(false)` forces MSAA, `None`
    /// restores normal arbitration.
    pub fn set_forced(&mut self, forced: Option<bool>) {
        self.forced = forced;
    }

    /// Records a probe result for `hwnd`: a UIA verdict is kept until the
    /// window is destroyed, a non-UIA verdict for
    /// [`NEGATIVE_VERDICT_LIFETIME`].
    pub fn record_probe(&mut self, hwnd: isize, is_uia: bool) {
        self.record_probe_at(hwnd, is_uia, Instant::now());
    }

    fn record_probe_at(&mut self, hwnd: isize, is_uia: bool, now: Instant) {
        let probed = if is_uia {
            Probed::Uia
        } else {
            Probed::NotUia(now)
        };
        self.cache.insert(hwnd, (window_thread(hwnd), probed));
    }

    /// Restarts, from `now`, the lifetime of every non-UIA verdict probed at
    /// or after `since`: the worker calls this when it finishes an entry
    /// that started at `since`. A slow read can outlast the lifetime of the
    /// verdict it relied on, and the MSAA and UIA facts for one focus must
    /// both see the same verdict, or a re-probe between them lets both
    /// backends announce it.
    pub fn renew_probes_since(&mut self, since: Instant, now: Instant) {
        for (_, probed) in self.cache.values_mut() {
            if let Probed::NotUia(at) = probed
                && *at >= since
            {
                *at = now;
            }
        }
    }

    /// Forgets `hwnd`'s probed verdict: the window was destroyed, and its
    /// handle may be reused by an unrelated window.
    pub fn forget(&mut self, hwnd: isize) {
        self.cache.remove(&hwnd);
    }

    /// Records that `hwnd` has a UIA provider NVDA does not use
    /// ([`post_probe_check`]): it is MSAA until the window is destroyed.
    pub fn record_excluded(&mut self, hwnd: isize) {
        self.cache
            .insert(hwnd, (window_thread(hwnd), Probed::Excluded));
    }

    fn classify(&self, classes: &WindowClasses) -> ClassVerdict {
        if self.good_classes.contains(&classes.normalized) || classes.is_shell() {
            ClassVerdict::Uia
        } else if self.bad_classes.contains(&classes.normalized) {
            ClassVerdict::NonUia
        } else {
            ClassVerdict::Unknown
        }
    }

    /// The verdict without probing: `Some(true)` for a UIA window,
    /// `Some(false)` for a non-UIA window, and `None` when only the blocking
    /// probe can decide: the window has not been probed yet, or its last
    /// probe found no provider longer ago than [`NEGATIVE_VERDICT_LIFETIME`].
    #[must_use]
    pub fn verdict(&self, hwnd: isize, classes: &WindowClasses) -> Option<bool> {
        self.verdict_at(hwnd, classes, Instant::now())
    }

    fn verdict_at(&self, hwnd: isize, classes: &WindowClasses, now: Instant) -> Option<bool> {
        if self.forced.is_some() {
            return self.forced;
        }
        match self.classify(classes) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown => match self.kept(hwnd)? {
                Probed::Uia => Some(true),
                Probed::Excluded => Some(false),
                Probed::NotUia(at) => (now.saturating_duration_since(*at)
                    < NEGATIVE_VERDICT_LIFETIME)
                    .then_some(false),
            },
        }
    }
}

impl Arbitrator {
    /// The kept verdict for `hwnd`, `None` when there is none or the window
    /// is now owned by another thread than the one it was recorded for: the
    /// handle has been reused by another window.
    fn kept(&self, hwnd: isize) -> Option<&Probed> {
        let (owner, probed) = self.cache.get(&hwnd)?;
        (*owner == window_thread(hwnd)).then_some(probed)
    }
}

/// The class names arbitration decides on, read with inexpensive local
/// calls safe on any thread.
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
    /// Reads `hwnd`'s classes.
    #[must_use]
    pub fn of(hwnd: isize) -> Self {
        // An invalid handle's root is 0, whose class reads as empty.
        Self::new(
            &window_class_name(hwnd),
            &window_class_name(top_level_of(hwnd)),
        )
    }

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
/// Verbatim's (decision D2, M6).
#[must_use]
pub fn post_probe_check(classes: &WindowClasses) -> Option<PostProbeCheck> {
    match classes.normalized.as_str() {
        "ConsoleWindowClass" => Some(PostProbeCheck::Console),
        "SysListView32" => Some(PostProbeCheck::WindowsFormsListView),
        _ => None,
    }
}

/// Reads a window's class name, an inexpensive local call safe on any thread.
#[must_use]
pub fn window_class_name(hwnd: isize) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: GetClassNameW writes at most buffer.len()-1 code units plus a NUL
    // and returns the count; an invalid handle yields 0.
    let len = unsafe { GetClassNameW(HWND(hwnd as *mut _), &mut buffer) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    String::from_utf16_lossy(&buffer[..len])
}

/// Classes that are always treated as UIA, before any probe runs: NVDA's
/// `goodUIAWindowClassNames` tuple (`nvda/source/UIAHandler/__init__.py`),
/// classes whose windows are always native UIA even when the probe would
/// miss them.
const GOOD_UIA_CLASSES: &[&str] = &[
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
const SHELL_ROOT_CLASSES: &[&str] = &[
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
const BAD_UIA_CLASSES: &[&str] = &[
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

    /// The classes of a top-level window of class `class`.
    fn top(class: &str) -> WindowClasses {
        WindowClasses::new(class, class)
    }

    #[test]
    fn good_class_is_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(1, &top("RAIL_WINDOW")), Some(true));
    }

    #[test]
    fn bad_class_is_non_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(1, &top("Edit")), Some(false));
        assert_eq!(arb.verdict(1, &top("RichEdit20")), Some(false));
    }

    #[test]
    fn extra_good_class_overrides_to_uia() {
        let arb = Arbitrator::new(&["MyAppCanvas"]);
        assert_eq!(arb.verdict(1, &top("MyAppCanvas")), Some(true));
    }

    #[test]
    fn a_verdict_of_no_provider_is_probed_again_after_its_lifetime() {
        let mut arb = Arbitrator::new(&[]);
        let probed = Instant::now();
        arb.record_probe_at(42, false, probed);
        assert_eq!(
            arb.verdict_at(42, &top("SomeUnknownClass"), probed),
            Some(false)
        );
        assert_eq!(
            arb.verdict_at(
                42,
                &top("SomeUnknownClass"),
                probed + NEGATIVE_VERDICT_LIFETIME
            ),
            None,
            "a probe that found no provider may have met a busy application"
        );
        arb.record_probe_at(42, true, probed + NEGATIVE_VERDICT_LIFETIME);
        assert_eq!(
            arb.verdict_at(
                42,
                &top("SomeUnknownClass"),
                probed + Duration::from_secs(3600)
            ),
            Some(true),
            "a provider, once found, is kept"
        );
    }

    #[test]
    fn a_verdict_of_no_provider_lasts_from_the_end_of_the_entry_that_probed_it() {
        let mut arb = Arbitrator::new(&[]);
        let earlier = Instant::now();
        let started = earlier + Duration::from_millis(10);
        arb.record_probe_at(7, false, started + Duration::from_millis(10));
        arb.record_probe_at(8, false, earlier);
        let finished = started + Duration::from_secs(2);
        arb.renew_probes_since(started, finished);
        assert_eq!(
            arb.verdict_at(
                7,
                &top("SomeUnknownClass"),
                finished + Duration::from_millis(100)
            ),
            Some(false),
            "the slow entry's own probe still holds just after it finished"
        );
        assert_eq!(
            arb.verdict_at(8, &top("SomeUnknownClass"), finished),
            None,
            "a probe from before the entry is not renewed"
        );
    }

    #[test]
    fn a_probed_verdict_lasts_until_the_window_is_destroyed() {
        let mut arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(42, &top("SomeUnknownClass")), None);
        arb.record_probe(42, true);
        assert_eq!(arb.verdict(42, &top("SomeUnknownClass")), Some(true));
        arb.forget(42);
        assert_eq!(
            arb.verdict(42, &top("SomeUnknownClass")),
            None,
            "a reused handle is probed afresh"
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

    #[test]
    fn shell_classes_arbitrate_to_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        for class in [
            "Shell_TrayWnd",
            "Shell_InputSwitchTopLevelWindow",
            "XamlExplorerHostIslandWindow",
            "TopLevelWindowForOverflowXamlIsland",
        ] {
            assert_eq!(
                arb.verdict(1, &top(class)),
                Some(true),
                "{class} must arbitrate to UIA"
            );
        }
    }

    #[test]
    fn a_window_under_a_shell_window_is_uia_except_the_start_button() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(
            arb.verdict(
                1,
                &WindowClasses::new("Windows.UI.Input.InputSite.WindowClass", "Shell_TrayWnd")
            ),
            Some(true)
        );
        assert_eq!(
            arb.verdict(1, &WindowClasses::new("Edit", "Shell_TrayWnd")),
            Some(true),
            "the shell rule comes before the bad list, as in NVDA"
        );
        assert_eq!(
            arb.verdict(1, &WindowClasses::new("Start", "Shell_TrayWnd")),
            None
        );
    }

    #[test]
    fn a_normalized_bad_class_is_msaa_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(
            arb.verdict(1, &top("WindowsForms10.EDIT.app.0.141b42a_r9_ad1")),
            Some(false)
        );
        assert_eq!(arb.verdict(1, &top("TRichEdit")), Some(false));
    }

    #[test]
    fn consoles_and_list_views_are_checked_after_the_probe() {
        assert_eq!(
            post_probe_check(&top("ConsoleWindowClass")),
            Some(PostProbeCheck::Console)
        );
        assert_eq!(
            post_probe_check(&top("WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1")),
            Some(PostProbeCheck::WindowsFormsListView)
        );
        assert_eq!(post_probe_check(&top("Notepad")), None);
    }

    #[test]
    fn an_excluded_provider_is_msaa_until_the_window_is_destroyed() {
        let mut arb = Arbitrator::new(&[]);
        let classes = top("ConsoleWindowClass");
        arb.record_excluded(5);
        assert_eq!(
            arb.verdict_at(5, &classes, Instant::now() + Duration::from_secs(3600)),
            Some(false)
        );
        arb.forget(5);
        assert_eq!(arb.verdict(5, &classes), None);
    }
}
