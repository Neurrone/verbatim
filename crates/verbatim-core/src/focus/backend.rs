//! The backend cache: whether a window is read through UI Automation or
//! MSAA (`docs/design/readers-tell-core-asks-build1.md`, Table 4;
//! `docs/parity/backends.md`, "Dual-stack MSAA+UIA with per-window
//! arbitration", "The class lists the per-window verdict reads").
//!
//! A window's verdict comes from its class, normalized, when the class is
//! on the good or the bad list or the Explorer shell rule names it; such a
//! verdict is judged from each tell's classes and never kept. Otherwise it
//! comes from the newest probe of the window whose start is no earlier
//! than half a second before the moment judged, kept per window and the
//! thread that owns it. With neither, no verdict is in force, and a
//! request naming the window carries "probe first".

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use verbatim_model::WindowHandle;
use verbatim_model::request::{Probe, WindowVerdict};

/// How long a probe's verdict is in force from the probe's start, NVDA's
/// cache period.
pub(crate) const PROBE_LIFE_US: u64 = 500_000;

/// The backend a window is read through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Backend {
    /// UI Automation.
    Uia,
    /// MSAA.
    Msaa,
}

/// NVDA's class map, each name with the name it is normalized to.
const CLASS_MAP: &[(&str, &str)] = &[
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
const GOOD: &[&str] = &[
    "RAIL_WINDOW",
    "Microsoft.UI.Content.DesktopChildSiteBridge",
    "CASCADIA_HOSTING_WINDOW_CLASS",
];

/// The classes read through MSAA.
const BAD: &[&str] = &[
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
const SHELL_ROOTS: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_InputSwitchTopLevelWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
];

/// `class`'s inner name when it is a Windows Forms name (`WindowsForms`,
/// digits, a dot, the inner name, `.app.` and anything after) or an `ATL:`
/// name.
fn inner_name(class: &str) -> Option<&str> {
    if let Some(inner) = class.strip_prefix("ATL:") {
        return Some(inner);
    }
    let rest = class.strip_prefix("WindowsForms")?;
    let digits = rest.find(|character: char| !character.is_ascii_digit())?;
    if digits == 0 {
        return None;
    }
    let rest = rest.get(digits..)?.strip_prefix('.')?;
    let end = rest.find(".app.")?;
    rest.get(..end).filter(|inner| !inner.is_empty())
}

/// `class` normalized as NVDA normalizes a window's class name: replaced by
/// its target in the class map; otherwise a Windows Forms or `ATL:` name
/// reduced to its inner name, then looked up in the map; otherwise kept.
pub(crate) fn normalize(class: &str) -> &str {
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
pub(crate) fn by_class(class: &str, root_class: &str) -> Option<Backend> {
    let class = normalize(class);
    if GOOD.contains(&class) {
        return Some(Backend::Uia);
    }
    if SHELL_ROOTS.contains(&root_class) && class != "Start" {
        return Some(Backend::Uia);
    }
    BAD.contains(&class).then_some(Backend::Msaa)
}

/// A window as the cache keys it: the window and the thread that owns it,
/// so that a reused handle is a new window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct WindowKey {
    /// The window's handle.
    pub(crate) handle: u64,
    /// The thread that owns it.
    pub(crate) thread: u32,
}

/// What a window's verdict is judged from: its key, and the verdict its
/// classes give, judged once from the tell that named it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct JudgedWindow {
    /// The window.
    pub(crate) key: WindowKey,
    /// The verdict its classes give, `None` when they give none.
    pub(crate) by_class: Option<Backend>,
}

/// A probe's verdict, kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Probed {
    backend: Backend,
    began_us: u64,
}

/// The kept probe verdicts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BackendCache {
    #[serde(with = "crate::event_queue::pairs")]
    probed: BTreeMap<WindowKey, Probed>,
}

/// What a tell's verdict makes of it at its turn (Table 4, columns A to
/// C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Judgement {
    /// Read: its request carries the verdict.
    Read(WindowVerdict),
    /// Dropped with no read: its window is read through the other backend.
    Drop,
}

/// The kind of tell the cache judges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Judged {
    /// An MSAA tell other than a caret one.
    Msaa,
    /// An MSAA caret tell, which the cache does not judge (NVDA's caret
    /// bypass).
    MsaaCaret,
    /// A UI Automation tell other than a notification or a toast.
    Uia,
    /// A UI Automation notification or toast, which the cache does not
    /// judge.
    UiaUnjudged,
}

impl BackendCache {
    /// Records a probe's verdict: the newest probe's start wins.
    pub(crate) fn record(&mut self, probe: &Probe) {
        let key = WindowKey {
            handle: probe.window.0,
            thread: probe.thread,
        };
        let probed = Probed {
            backend: if probe.provider {
                Backend::Uia
            } else {
                Backend::Msaa
            },
            began_us: probe.began_us,
        };
        let newer = self
            .probed
            .get(&key)
            .is_none_or(|kept| kept.began_us <= probed.began_us);
        if newer {
            self.probed.insert(key, probed);
        }
    }

    /// A request that carried "probe first" ended with no answer for its
    /// probe (no longer awaited, withdrawn, abandoned or ended with its
    /// reader): counted as a probe that timed out, begun when the request
    /// was sent.
    pub(crate) fn unanswered(&mut self, key: WindowKey, sent_us: u64) {
        self.record(&Probe {
            window: WindowHandle(key.handle),
            thread: key.thread,
            provider: false,
            began_us: sent_us,
        });
    }

    /// Forgets every verdict of a destroyed window.
    pub(crate) fn destroyed(&mut self, window: WindowHandle) {
        self.probed.retain(|key, _| key.handle != window.0);
    }

    /// The verdict for `window` at `at_us`: its class verdict, else the
    /// newest probe begun no earlier than half a second before, else none.
    pub(crate) fn verdict(&self, window: &JudgedWindow, at_us: u64) -> Option<Backend> {
        window.by_class.or_else(|| {
            self.probed
                .get(&window.key)
                .filter(|probed| probed.began_us.saturating_add(PROBE_LIFE_US) >= at_us)
                .map(|probed| probed.backend)
        })
    }

    /// What a request naming `window`, sent at `at_us`, carries (column D).
    pub(crate) fn for_request(&self, window: &JudgedWindow, at_us: u64) -> WindowVerdict {
        match self.verdict(window, at_us) {
            Some(Backend::Uia) => WindowVerdict::Uia,
            Some(Backend::Msaa) => WindowVerdict::Msaa,
            None => WindowVerdict::ProbeFirst,
        }
    }

    /// What a tell of kind `judged`, whose callback received it at
    /// `received_us`, is at its turn at `now_us` (columns A to C): a tell
    /// is judged by the verdict in force when it was received, and its
    /// request carries the verdict in force as it is sent.
    pub(crate) fn judge(
        &self,
        judged: Judged,
        window: &JudgedWindow,
        received_us: u64,
        now_us: u64,
    ) -> Judgement {
        let request = self.for_request(window, now_us);
        let verdict = self.verdict(window, received_us);
        match (judged, verdict) {
            (Judged::Msaa, Some(Backend::Uia)) | (Judged::Uia, Some(Backend::Msaa)) => {
                Judgement::Drop
            }
            (Judged::Msaa | Judged::Uia, Some(_) | None)
            | (Judged::MsaaCaret | Judged::UiaUnjudged, _) => Judgement::Read(request),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: WindowKey = WindowKey {
        handle: 5,
        thread: 7,
    };

    fn probed() -> JudgedWindow {
        JudgedWindow {
            key: KEY,
            by_class: None,
        }
    }

    fn probe(provider: bool, began_us: u64) -> Probe {
        Probe {
            window: WindowHandle(KEY.handle),
            thread: KEY.thread,
            provider,
            began_us,
        }
    }

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
        assert_eq!(normalize("ATL:EDIT"), "Edit");
        assert_eq!(normalize("ATL:SysListView32"), "SysListView32");
    }

    #[test]
    fn any_other_class_is_kept() {
        assert_eq!(
            normalize("Windows.UI.Core.CoreWindow"),
            "Windows.UI.Core.CoreWindow"
        );
        assert_eq!(
            normalize("WindowsForms.EDIT.app.0"),
            "WindowsForms.EDIT.app.0"
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

    // Table 4, row "none in force".

    #[test]
    fn with_no_verdict_an_msaa_tell_is_read_with_probe_first() {
        let cache = BackendCache::default();
        assert_eq!(
            cache.judge(Judged::Msaa, &probed(), 1_000, 2_000),
            Judgement::Read(WindowVerdict::ProbeFirst)
        );
    }

    #[test]
    fn with_no_verdict_a_uia_tell_is_read_with_probe_first() {
        let cache = BackendCache::default();
        assert_eq!(
            cache.judge(Judged::Uia, &probed(), 1_000, 2_000),
            Judgement::Read(WindowVerdict::ProbeFirst)
        );
    }

    #[test]
    fn a_caret_tell_and_a_notification_are_not_judged_by_the_cache() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        assert_eq!(
            cache.judge(Judged::MsaaCaret, &probed(), 1_100, 1_200),
            Judgement::Read(WindowVerdict::Uia)
        );
        cache.record(&probe(false, 2_000));
        assert_eq!(
            cache.judge(Judged::UiaUnjudged, &probed(), 2_100, 2_200),
            Judgement::Read(WindowVerdict::Msaa)
        );
    }

    #[test]
    fn with_no_verdict_a_request_carries_probe_first_and_nothing_is_cleared_at_destruction() {
        let mut cache = BackendCache::default();
        assert_eq!(cache.for_request(&probed(), 0), WindowVerdict::ProbeFirst);
        cache.destroyed(WindowHandle(KEY.handle));
        assert_eq!(cache, BackendCache::default());
    }

    // Rows "UI Automation in force" and "MSAA in force".

    #[test]
    fn with_ui_automation_in_force_an_msaa_tell_is_dropped_and_a_uia_tell_read() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        assert_eq!(
            cache.judge(Judged::Msaa, &probed(), 1_200, 1_300),
            Judgement::Drop
        );
        assert_eq!(
            cache.judge(Judged::Uia, &probed(), 1_200, 1_300),
            Judgement::Read(WindowVerdict::Uia)
        );
        assert_eq!(cache.for_request(&probed(), 1_300), WindowVerdict::Uia);
    }

    #[test]
    fn with_msaa_in_force_an_msaa_tell_is_read_and_a_uia_tell_dropped() {
        let mut cache = BackendCache::default();
        cache.record(&probe(false, 1_000));
        assert_eq!(
            cache.judge(Judged::Msaa, &probed(), 1_200, 1_300),
            Judgement::Read(WindowVerdict::Msaa)
        );
        assert_eq!(
            cache.judge(Judged::Uia, &probed(), 1_200, 1_300),
            Judgement::Drop
        );
        assert_eq!(cache.for_request(&probed(), 1_300), WindowVerdict::Msaa);
    }

    #[test]
    fn a_newer_probes_answer_replaces_the_verdict_and_an_older_one_does_not() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        cache.record(&probe(false, 1_100));
        assert_eq!(cache.verdict(&probed(), 1_200), Some(Backend::Msaa));
        cache.record(&probe(true, 1_050));
        assert_eq!(cache.verdict(&probed(), 1_200), Some(Backend::Msaa));
    }

    #[test]
    fn a_verdict_lapses_half_a_second_after_its_probe_began() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        assert_eq!(
            cache.verdict(&probed(), 1_000 + PROBE_LIFE_US),
            Some(Backend::Uia)
        );
        assert_eq!(cache.verdict(&probed(), 1_001 + PROBE_LIFE_US), None);
        assert_eq!(
            cache.for_request(&probed(), 1_001 + PROBE_LIFE_US),
            WindowVerdict::ProbeFirst
        );
    }

    #[test]
    fn a_tell_is_judged_by_the_verdict_in_force_when_it_was_received() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        // Received within the probe's half second, judged long after.
        assert_eq!(
            cache.judge(Judged::Msaa, &probed(), 1_400, 900_000),
            Judgement::Drop
        );
        // Received after it lapsed.
        assert_eq!(
            cache.judge(Judged::Msaa, &probed(), 600_000, 900_000),
            Judgement::Read(WindowVerdict::ProbeFirst)
        );
    }

    #[test]
    fn a_verdict_is_cleared_when_its_window_is_destroyed() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        cache.destroyed(WindowHandle(KEY.handle));
        assert_eq!(cache.verdict(&probed(), 1_100), None);
    }

    #[test]
    fn a_reused_handle_on_another_thread_is_another_window() {
        let mut cache = BackendCache::default();
        cache.record(&probe(true, 1_000));
        let other = JudgedWindow {
            key: WindowKey {
                handle: KEY.handle,
                thread: 8,
            },
            by_class: None,
        };
        assert_eq!(cache.verdict(&other, 1_100), None);
    }

    #[test]
    fn an_unanswered_probe_counts_as_one_that_timed_out() {
        let mut cache = BackendCache::default();
        cache.unanswered(KEY, 1_000);
        assert_eq!(cache.verdict(&probed(), 1_100), Some(Backend::Msaa));
    }

    // Row "by class".

    #[test]
    fn a_class_verdict_judges_each_tell_and_needs_no_probe() {
        let mut cache = BackendCache::default();
        let good = JudgedWindow {
            key: KEY,
            by_class: Some(Backend::Uia),
        };
        let bad = JudgedWindow {
            key: KEY,
            by_class: Some(Backend::Msaa),
        };
        cache.record(&probe(false, 1_000));
        assert_eq!(
            cache.judge(Judged::Msaa, &good, 1_100, 1_200),
            Judgement::Drop
        );
        assert_eq!(
            cache.judge(Judged::Uia, &good, 1_100, 1_200),
            Judgement::Read(WindowVerdict::Uia)
        );
        assert_eq!(
            cache.judge(Judged::Msaa, &bad, 1_100, 1_200),
            Judgement::Read(WindowVerdict::Msaa)
        );
        assert_eq!(
            cache.judge(Judged::Uia, &bad, 1_100, 1_200),
            Judgement::Drop
        );
        assert_eq!(cache.for_request(&good, 9_000_000), WindowVerdict::Uia);
    }
}
