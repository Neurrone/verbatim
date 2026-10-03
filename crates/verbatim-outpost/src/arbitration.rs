//! Per-window backend arbitration (architecture section 4).
//!
//! Mirrors NVDA's ladder, cheapest rung first: a config-overridable "good"
//! class list forces UIA; a "bad" class list (seeded from NVDA's, where UIA
//! implementations are known to interfere with MSAA) forces MSAA; otherwise the
//! window is probed with `UiaHasServerSideProvider`. Class-list checks are fast
//! local window calls; the probe blocks on the target's message pump and runs
//! only on the outpost's worker, under its deadline
//! ([`verbatim_uia::has_server_side_provider`]).
//!
//! A probe that finds a UIA provider is kept for the window's lifetime and
//! forgotten when the window is destroyed ([`Arbitrator::forget`]), as
//! decision D15 specifies: a server-side provider does not go away. A probe
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
    /// Probe results: a UIA verdict until the window is destroyed, a non-UIA
    /// verdict for [`NEGATIVE_VERDICT_LIFETIME`].
    cache: HashMap<isize, Probed>,
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
        self.cache.insert(hwnd, probed);
    }

    /// Restarts, from `now`, the lifetime of every non-UIA verdict probed at
    /// or after `since`: the worker calls this when it finishes an entry
    /// that started at `since`. A slow read can outlast the lifetime of the
    /// verdict it relied on, and the MSAA and UIA facts for one focus must
    /// both see the same verdict, or a re-probe between them lets both
    /// backends announce it.
    pub fn renew_probes_since(&mut self, since: Instant, now: Instant) {
        for probed in self.cache.values_mut() {
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

    fn classify(&self, class_name: &str) -> ClassVerdict {
        if self.good_classes.contains(class_name) {
            ClassVerdict::Uia
        } else if self.bad_classes.contains(class_name) {
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
    pub fn verdict(&self, hwnd: isize, class_name: &str) -> Option<bool> {
        self.verdict_at(hwnd, class_name, Instant::now())
    }

    fn verdict_at(&self, hwnd: isize, class_name: &str, now: Instant) -> Option<bool> {
        if self.forced.is_some() {
            return self.forced;
        }
        match self.classify(class_name) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown => match self.cache.get(&hwnd)? {
                Probed::Uia => Some(true),
                Probed::NotUia(at) => (now.saturating_duration_since(*at)
                    < NEGATIVE_VERDICT_LIFETIME)
                    .then_some(false),
            },
        }
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

/// Classes that are always treated as UIA, before any probe runs. Two
/// NVDA-lifted lists concatenated, each pinned by its own unit test so a
/// future NVDA sync is a diff of two lists:
///
/// - NVDA's `goodUIAWindowClassNames` tuple
///   (`nvda/source/UIAHandler/__init__.py`): classes whose windows are
///   always native UIA even when the probe would miss them.
/// - The Windows 11 shell set from NVDA's Explorer app module's
///   `isGoodUIAWindow` (`nvda/source/appModules/explorer.py`): the shell
///   root and top-level shell feature windows — taskbar, systray overflow,
///   Task View and snap layouts, and the input switcher — that NVDA
///   reclassifies as UIA on Windows 11 (roadmap M3's shell-support bullet:
///   window-classification rules as generic core policy, not per-app
///   patches). NVDA checks these against the event window's *root ancestor*
///   class; Verbatim's per-window arbitration checks the window's own class,
///   which covers the same windows because each named class is itself the
///   top-level window of its shell surface. NVDA's `ApplicationFrameWindow`
///   entry (the emoji-panel workaround) and its `Start`-class exclusion are
///   deliberately not carried: the former predates the probe handling those
///   windows correctly, and the latter only matters under NVDA's
///   IAccessible-first event handling.
const GOOD_UIA_CLASSES: &[&str] = &[
    // NVDA goodUIAWindowClassNames: Windows Defender Application Guard
    // windows are always native UIA.
    "RAIL_WINDOW",
    // NVDA goodUIAWindowClassNames: WinUI 3 top-level pane.
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    // NVDA explorer.py isGoodUIAWindow: Windows 11 shell UI root — Start,
    // Search, Widgets, and the taskbar's own elements.
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

    #[test]
    fn good_class_is_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(1, "RAIL_WINDOW"), Some(true));
    }

    #[test]
    fn bad_class_is_non_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(1, "Edit"), Some(false));
        assert_eq!(arb.verdict(1, "RichEdit20"), Some(false));
    }

    #[test]
    fn extra_good_class_overrides_to_uia() {
        let arb = Arbitrator::new(&["MyAppCanvas"]);
        assert_eq!(arb.verdict(1, "MyAppCanvas"), Some(true));
    }

    #[test]
    fn a_verdict_of_no_provider_is_probed_again_after_its_lifetime() {
        let mut arb = Arbitrator::new(&[]);
        let probed = Instant::now();
        arb.record_probe_at(42, false, probed);
        assert_eq!(arb.verdict_at(42, "SomeUnknownClass", probed), Some(false));
        assert_eq!(
            arb.verdict_at(42, "SomeUnknownClass", probed + NEGATIVE_VERDICT_LIFETIME),
            None,
            "a probe that found no provider may have met a busy application"
        );
        arb.record_probe_at(42, true, probed + NEGATIVE_VERDICT_LIFETIME);
        assert_eq!(
            arb.verdict_at(42, "SomeUnknownClass", probed + Duration::from_secs(3600)),
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
            arb.verdict_at(7, "SomeUnknownClass", finished + Duration::from_millis(100)),
            Some(false),
            "the slow entry's own probe still holds just after it finished"
        );
        assert_eq!(
            arb.verdict_at(8, "SomeUnknownClass", finished),
            None,
            "a probe from before the entry is not renewed"
        );
    }

    #[test]
    fn a_probed_verdict_lasts_until_the_window_is_destroyed() {
        let mut arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(42, "SomeUnknownClass"), None);
        arb.record_probe(42, true);
        assert_eq!(arb.verdict(42, "SomeUnknownClass"), Some(true));
        arb.forget(42);
        assert_eq!(
            arb.verdict(42, "SomeUnknownClass"),
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
        let uia_handler_good = ["RAIL_WINDOW", "Microsoft.UI.Content.DesktopChildSiteBridge"];
        // nvda/source/appModules/explorer.py, isGoodUIAWindow's Windows 11
        // shell tuple (checked there against the root ancestor's class).
        let explorer_shell = [
            "Shell_TrayWnd",
            "Shell_InputSwitchTopLevelWindow",
            "XamlExplorerHostIslandWindow",
            "TopLevelWindowForOverflowXamlIsland",
        ];
        let expected: Vec<&str> = uia_handler_good.into_iter().chain(explorer_shell).collect();
        assert_eq!(GOOD_UIA_CLASSES, expected.as_slice());
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
                arb.verdict(1, class),
                Some(true),
                "{class} must arbitrate to UIA"
            );
        }
    }
}
