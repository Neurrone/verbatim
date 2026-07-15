//! Per-window backend arbitration (architecture section 4).
//!
//! Mirrors NVDA's ladder, cheapest rung first: a config-overridable "good"
//! class list forces UIA; a "bad" class list (seeded from NVDA's, where UIA
//! implementations are known to interfere with MSAA) forces MSAA; otherwise the
//! window is probed with `UiaHasServerSideProvider`. Class-list checks are fast
//! local window calls and run inline on any thread; the probe blocks on the
//! target's message pump and must run only on a deadline-guarded query-pool
//! thread ([`verbatim_uia::has_server_side_provider`]). Verdicts are cached per
//! window handle for 500 ms.
//!
//! At event delivery the cross-filter (see [`Arbitrator::verdict`]) drops MSAA
//! events whose window arbitrates to UIA and UIA focus events whose nearest
//! window does not, so the two backends never both announce the same change.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

/// How long a probe verdict stays valid, matching NVDA's window.
const CACHE_TTL: Duration = Duration::from_millis(500);

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
    cache: HashMap<isize, (bool, Instant)>,
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

    /// Records a probe result for `hwnd` without holding a lock across the
    /// (blocking) probe. Used by the runtime's non-blocking probe path.
    pub fn record_probe(&mut self, hwnd: isize, is_uia: bool) {
        self.cache.insert(hwnd, (is_uia, Instant::now()));
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

    /// Non-blocking verdict for event delivery. Returns `Some(true)` for a UIA
    /// window, `Some(false)` for a non-UIA window, and `None` when only the
    /// (blocking) probe could decide and no fresh cached verdict exists. On
    /// `None` the caller treats the window as non-UIA provisionally and should
    /// schedule a probe with [`Arbitrator::resolve_with`].
    #[must_use]
    pub fn verdict(&self, hwnd: isize, class_name: &str) -> Option<bool> {
        if self.forced.is_some() {
            return self.forced;
        }
        match self.classify(class_name) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown => self.cached_probe(hwnd),
        }
    }

    fn cached_probe(&self, hwnd: isize) -> Option<bool> {
        self.cache
            .get(&hwnd)
            .and_then(|(verdict, at)| (at.elapsed() < CACHE_TTL).then_some(*verdict))
    }

    /// Resolves a window's backend, running `probe` only if the class lists do
    /// not decide it and no fresh verdict is cached. `probe` returns `None` when
    /// it could not complete within its deadline; that is cached as non-UIA and
    /// surfaced via the returned `probe_timed_out` flag so the caller can emit a
    /// fault. Runs on a query-pool thread because `probe` may block.
    pub fn resolve_with<P>(&mut self, hwnd: isize, class_name: &str, probe: P) -> Resolution
    where
        P: FnOnce(isize) -> Option<bool>,
    {
        if let Some(forced) = self.forced {
            return Resolution {
                is_uia: forced,
                probe_timed_out: false,
            };
        }
        match self.classify(class_name) {
            ClassVerdict::Uia => Resolution {
                is_uia: true,
                probe_timed_out: false,
            },
            ClassVerdict::NonUia => Resolution {
                is_uia: false,
                probe_timed_out: false,
            },
            ClassVerdict::Unknown => {
                if let Some(cached) = self.cached_probe(hwnd) {
                    return Resolution {
                        is_uia: cached,
                        probe_timed_out: false,
                    };
                }
                let (is_uia, timed_out) = match probe(hwnd) {
                    Some(result) => (result, false),
                    None => (false, true),
                };
                self.cache.insert(hwnd, (is_uia, Instant::now()));
                Resolution {
                    is_uia,
                    probe_timed_out: timed_out,
                }
            }
        }
    }
}

/// The outcome of [`Arbitrator::resolve_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolution {
    /// Whether the window should use UIA.
    pub is_uia: bool,
    /// Whether the probe timed out (verdict defaulted to non-UIA; emit a fault).
    pub probe_timed_out: bool,
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

/// Classes that are always treated as UIA (NVDA's `goodUIAWindowClassNames`).
const GOOD_UIA_CLASSES: &[&str] = &[
    // Windows Defender Application Guard windows are always native UIA.
    "RAIL_WINDOW",
    // WinUI 3 top-level pane.
    "Microsoft.UI.Content.DesktopChildSiteBridge",
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
        let mut arb = Arbitrator::new(&[]);
        let mut probed = false;
        let resolution = arb.resolve_with(1, "RAIL_WINDOW", |_| {
            probed = true;
            Some(false)
        });
        assert!(resolution.is_uia);
        assert!(!probed, "good class must not invoke the probe");
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
    fn unknown_class_needs_probe_then_caches() {
        let mut arb = Arbitrator::new(&[]);
        assert_eq!(arb.verdict(42, "SomeUnknownClass"), None);
        let mut calls = 0;
        let first = arb.resolve_with(42, "SomeUnknownClass", |_| {
            calls += 1;
            Some(true)
        });
        assert!(first.is_uia);
        assert_eq!(calls, 1);
        // Second resolve within the TTL uses the cache, not the probe.
        let second = arb.resolve_with(42, "SomeUnknownClass", |_| {
            calls += 1;
            Some(false)
        });
        assert!(second.is_uia);
        assert_eq!(calls, 1, "cached verdict must not re-probe");
        assert_eq!(arb.verdict(42, "SomeUnknownClass"), Some(true));
    }

    #[test]
    fn probe_timeout_is_non_uia_and_flagged() {
        let mut arb = Arbitrator::new(&[]);
        let resolution = arb.resolve_with(7, "AnotherClass", |_| None);
        assert!(!resolution.is_uia);
        assert!(resolution.probe_timed_out);
    }
}
