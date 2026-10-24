//! Per-window backend arbitration (architecture section 4).
//!
//! Mirrors NVDA's ladder, cheapest rung first, on the window's class name
//! normalized as NVDA normalizes it ([`normalize_class_name`]): a
//! config-overridable "good" class list forces UIA, and so does a Windows 11
//! shell window, recognized by its root ancestor's class; a "bad" class list
//! (seeded from NVDA's, where UIA implementations are known to interfere with
//! MSAA) forces MSAA; otherwise the window is probed, by an outpost on its
//! own application's windows and by the focus listener alike, with the raw
//! probe, one `WM_GETOBJECT` and no UIA connection
//! ([`verbatim_uia::raw_probe`];
//! `docs/design/listener-without-connections.md`, decision 2). A window
//! with a provider is still set aside
//! for MSAA when its provider is one NVDA does not use: an old console's,
//! or a list view's outside Windows Forms ([`post_probe_check`]), which
//! both read from the window's class name, a local call.
//! Class-list checks are fast local window calls; the probe blocks on the
//! target's message pump and runs only on a deadline-guarded worker.
//!
//! A probe that finds a UIA provider is kept for the window's lifetime and
//! forgotten when the window is destroyed ([`Arbitrator::forget`]), as
//! decision D15 specifies: a server-side provider does not go away. Each
//! kept verdict also records the thread that owned the window, and a
//! window now owned by another thread is probed afresh, so a reused window
//! handle does not inherit a verdict when its destroy event was lost. A probe
//! that finds none is trusted for only [`NEGATIVE_VERDICT_LIFETIME`], NVDA's
//! cache period, and then probed again. The probe asks once, as NVDA does,
//! so a window too busy to answer the probe in time
//! ([`verbatim_uia::RAW_PROBE_TIMEOUT_MS`]) is read through MSAA for the
//! rest of the outpost's entry that probed it, and by the focus listener
//! for the read that probed it, but that "no" is kept no longer, and the
//! next entry or read probes again: NVDA times its cached verdict from
//! before the probe, so a "no" that took the probe's whole wait is already
//! out of date when it is stored.
//!
//! The renewal is by time because nothing else can drive it: an
//! application that starts answering the probe raises no event, as a
//! provider is created on demand when the window is asked for one. The time
//! is read from the arbitrator's [`Clock`], the system's by default, so a
//! test that counts an operation's calls sets the time itself
//! ([`Arbitrator::set_clock`]) and the probe is renewed exactly when the test
//! says, not when the test happens to have taken half a second.
//!
//! The worker drops MSAA events whose window arbitrates to UIA and UIA events
//! whose window does not, so the two backends never both announce the same
//! change.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

use verbatim_ia2::WindowCache;

use crate::outpost::window::{top_level_of, window_thread};
pub use verbatim_ia2::class::normalize_class_name;

/// How long a probe that found no UIA provider is trusted before the window
/// is probed again: NVDA's `isUIAWindow` cache period.
pub const NEGATIVE_VERDICT_LIFETIME: Duration = Duration::from_millis(500);

/// What time it is, for the lifetime of a kept verdict and the time budget
/// of a classic walk: [`Instant::now`] unless a test sets its own
/// ([`Arbitrator::set_clock`]).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Whether `budget` has passed by `clock` since this call: the out-of-time
/// check a classic walk makes between its calls.
pub(crate) fn time_limit(clock: &Clock, budget: Duration) -> impl Fn() -> bool + '_ {
    let deadline = clock() + budget;
    move || clock() >= deadline
}

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
    /// The windows whose probe timed out in the worker's entry under way,
    /// each a "no" until that entry ends
    /// ([`record_timed_out`](Self::record_timed_out)).
    timed_out: Vec<isize>,
    /// A `SetBackendOverride` forcing every window: `Some(true)` for UIA,
    /// `Some(false)` for MSAA, `None` for normal arbitration.
    forced: Option<bool>,
    /// The time a verdict is recorded and checked at.
    clock: Clock,
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
            timed_out: Vec::new(),
            forced: None,
            clock: Arc::new(Instant::now),
        }
    }

    /// Reads the time from `clock` from now on, in place of the system's:
    /// for a test that decides when a verdict of no provider runs out, and
    /// when a classic walk's time budget is spent.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// The time by the arbitrator's clock, which a verdict's lifetime is
    /// measured in, and [`renew_probes_since`](Self::renew_probes_since)
    /// takes.
    #[must_use]
    pub fn now(&self) -> Instant {
        (self.clock)()
    }

    /// The arbitrator's clock, which the outpost's classic walks measure
    /// their time budgets by too.
    #[must_use]
    pub fn clock(&self) -> Clock {
        Arc::clone(&self.clock)
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
        self.record_probe_at(hwnd, is_uia, self.now());
    }

    fn record_probe_at(&mut self, hwnd: isize, is_uia: bool, now: Instant) {
        let probed = if is_uia {
            Probed::Uia
        } else {
            Probed::NotUia(now)
        };
        self.cache.insert(hwnd, (window_thread(hwnd), probed));
    }

    /// Records that `hwnd` did not answer its probe in time: no provider,
    /// for the rest of the worker's entry that probed it, so that the entry
    /// probes the window once, and from its next entry on the window is
    /// asked again ([`begin_entry`](Self::begin_entry),
    /// [`renew_probes_since`](Self::renew_probes_since)).
    pub fn record_timed_out(&mut self, hwnd: isize) {
        if !self.timed_out.contains(&hwnd) {
            self.timed_out.push(hwnd);
        }
    }

    /// The time, by [`now`](Self::now), that a worker's entry begins at,
    /// which the worker hands [`renew_probes_since`](Self::renew_probes_since)
    /// as the entry ends. Forgets every probe that timed out before it,
    /// including one from the entry of a worker abandoned meanwhile.
    pub fn begin_entry(&mut self) -> Instant {
        self.timed_out.clear();
        self.now()
    }

    /// Restarts, from `now`, the lifetime of every non-UIA verdict probed at
    /// or after `since`, both read with [`now`](Self::now), and forgets
    /// every probe that timed out, which is never renewed: the worker calls
    /// this when it finishes an entry that started at `since`. A slow read
    /// can outlast the lifetime of the verdict it relied on, and the MSAA
    /// and UIA facts for one focus must both see the same verdict, or a
    /// re-probe between them lets both backends announce it.
    pub fn renew_probes_since(&mut self, since: Instant, now: Instant) {
        self.timed_out.clear();
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
        self.timed_out.retain(|&window| window != hwnd);
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
    /// probe can decide: the window has not been probed yet, its last probe
    /// found no provider longer ago than [`NEGATIVE_VERDICT_LIFETIME`], or
    /// its probe timed out in an entry that has ended
    /// ([`record_timed_out`](Self::record_timed_out)).
    #[must_use]
    pub fn verdict(&self, hwnd: isize, classes: &WindowClasses) -> Option<bool> {
        self.verdict_at(hwnd, classes, self.now())
    }

    fn verdict_at(&self, hwnd: isize, classes: &WindowClasses, now: Instant) -> Option<bool> {
        if self.forced.is_some() {
            return self.forced;
        }
        match self.classify(classes) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown if self.timed_out.contains(&hwnd) => Some(false),
            ClassVerdict::Unknown => match self.kept(hwnd)? {
                Probed::Uia => Some(true),
                Probed::Excluded => Some(false),
                Probed::NotUia(at) => (now.saturating_duration_since(*at)
                    < NEGATIVE_VERDICT_LIFETIME)
                    .then_some(false),
            },
        }
    }

    /// The verdict [`verdict`](Self::verdict) gives, and whether it holds
    /// for the window's lifetime until the override changes: a forced one,
    /// one from the class lists, or a kept probe that found a provider, or
    /// one NVDA does not use. A probe that found none is renewed by time,
    /// and a window not yet probed has no verdict, so neither lasts.
    #[must_use]
    pub fn lasting_verdict(&self, hwnd: isize, classes: &WindowClasses) -> (Option<bool>, bool) {
        let lasting = self.forced.is_some()
            || self.classify(classes) != ClassVerdict::Unknown
            || matches!(self.kept(hwnd), Some(Probed::Uia | Probed::Excluded));
        (self.verdict(hwnd, classes), lasting)
    }

    /// Whether a kept probe found that `hwnd`, of `classes`, has a UIA
    /// provider of its own. A verdict from the class lists or a forced
    /// override does not say this: a window UIA by class may be served only
    /// by UIA's proxies, as the taskbar's `Shell_TrayWnd` is. A probe kept
    /// for a window its classes decide does not say it either, because the
    /// focus listener records one for every UIA focus it reports, including
    /// those of windows UIA only by class.
    #[must_use]
    pub fn probe_found_provider(&self, hwnd: isize, classes: &WindowClasses) -> bool {
        self.classify(classes) == ClassVerdict::Unknown
            && matches!(self.kept(hwnd), Some(Probed::Uia))
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
/// (`docs/design/listener-without-connections.md`, section 3, step 5).
fn has_windows_forms_class(raw: &str) -> bool {
    raw.starts_with("WindowsForms10.")
}

/// The answer to `check` for a window of classes `classes`: `Some(true)`
/// when its provider is used, `Some(false)` when it is excluded, `None` when
/// the application did not answer. A console asks `console`; a list view is
/// answered from its class name alone, with no call into the application,
/// by an outpost and the listener alike
/// (`docs/design/listener-without-connections.md`, section 3, step 5).
fn provider_usable(
    check: PostProbeCheck,
    classes: &WindowClasses,
    console: impl FnOnce() -> Option<bool>,
) -> Option<bool> {
    match check {
        PostProbeCheck::Console => console(),
        PostProbeCheck::WindowsFormsListView => Some(has_windows_forms_class(&classes.raw)),
    }
}

/// Decides window `hwnd`'s backend with `arbitrator`, as NVDA's
/// `isUIAWindow` decides it, for an outpost's events and for the focus
/// listener's UIA focus judgement alike: the class lists and kept verdicts first, then
/// the provider probe, whose answer the arbitrator keeps (a provider for the
/// window's lifetime, none for a short while, and nothing for a probe that
/// timed out). Returns `true` for UIA. The
/// probe is the raw probe ([`verbatim_uia::raw_probe`]), for the listener
/// and the outposts alike, and no connection follows it. A console window
/// with a provider is used only when `console` answers that its text
/// reports formatting
/// ([`PostProbeCheck::Console`]): an outpost reads that by the window's
/// handle ([`verbatim_uia::console_reports_formatting`]), and the listener,
/// which connects to no window, from the elements it holds
/// ([`verbatim_uia::held_console_reports_formatting`]).
pub(crate) fn uses_uia(
    arbitrator: &Mutex<Arbitrator>,
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> bool {
    decide(arbitrator, hwnd, console, false).0
}

/// Window `hwnd`'s backend as [`uses_uia`] would decide it without a call
/// into the application, from the class lists or a kept verdict: `None`
/// when only the probe can decide.
pub(crate) fn known_verdict(arbitrator: &Mutex<Arbitrator>, hwnd: isize) -> Option<bool> {
    let classes = WindowClasses::of(hwnd);
    lock(arbitrator).verdict(hwnd, &classes)
}

/// [`uses_uia`] for an outpost's worker, which keeps a probe that timed out
/// as a "no" until its entry ends ([`Arbitrator::record_timed_out`]), so
/// that one entry probes a window once and decides its backend once; and
/// whether this call probed the window and kept a verdict of UIA for
/// its lifetime: an outpost's event thread may then narrow its hook
/// (`outpost::narrowing`).
pub(crate) fn uses_uia_keeping(
    arbitrator: &Mutex<Arbitrator>,
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> (bool, bool) {
    decide(arbitrator, hwnd, console, true)
}

/// [`uses_uia_keeping`], keeping a probe that timed out for the entry only
/// when `keep_timed_out`: the focus listener's reader, which has no entries
/// and probes once for each read, keeps nothing of it.
fn decide(
    arbitrator: &Mutex<Arbitrator>,
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
    keep_timed_out: bool,
) -> (bool, bool) {
    let classes = WindowClasses::of(hwnd);
    if let Some(verdict) = lock(arbitrator).verdict(hwnd, &classes) {
        return (verdict, false);
    }
    let started = std::time::Instant::now();
    let probed = verbatim_uia::raw_probe(hwnd);
    let is_uia = probed.has_provider();
    tracing::debug!(
        hwnd,
        class = classes.raw,
        ?probed,
        elapsed_ms = started.elapsed().as_millis(),
        "arbitration probed"
    );
    if is_uia && let Some(check) = post_probe_check(&classes) {
        // NVDA uses some providers only after checking them.
        let usable = provider_usable(check, &classes, console);
        tracing::debug!(hwnd, ?check, ?usable, "arbitration checked the provider");
        return match usable {
            Some(true) => {
                lock(arbitrator).record_probe(hwnd, true);
                (true, true)
            }
            Some(false) => {
                lock(arbitrator).record_excluded(hwnd);
                (false, false)
            }
            // The application did not answer: asked again next time.
            None => (false, false),
        };
    }
    if probed == verbatim_uia::RawProbe::TimedOut {
        // Kept for the entry only: NVDA's verdict, timed from before its
        // probe, is out of date by the time a probe that took the whole
        // wait returns, and a starting console whose slow "no" was kept
        // past its entry was read through MSAA at the next foreground
        // change.
        if keep_timed_out {
            lock(arbitrator).record_timed_out(hwnd);
        }
    } else {
        lock(arbitrator).record_probe(hwnd, is_uia);
    }
    (is_uia, is_uia)
}

/// What an outpost's event thread knows of each window it takes in
/// `WinEvents` from, so a flood of events from one window reads its
/// classes once and asks the arbitrator only until its backend is decided
/// for the window's lifetime ([`Arbitrator::lasting_verdict`]). A window's
/// entry goes when the window is destroyed ([`forget`](Self::forget)), or
/// when its handle is found owned by another thread ([`WindowCache`]), and
/// every entry's backend when the override changes.
#[derive(Default)]
pub(crate) struct EventWindows {
    windows: WindowCache<KnownWindow>,
    /// The count of override changes the entries were decided under.
    overrides: u64,
}

/// What [`EventWindows`] keeps of one window.
struct KnownWindow {
    classes: WindowClasses,
    /// Its backend, once decided for its lifetime: `Some(true)` for UIA.
    lasting: Option<bool>,
}

impl EventWindows {
    /// Whether window `hwnd`, owned by thread `owner`, is already known to
    /// be read through UIA, from the class lists or a kept verdict, without
    /// the probe ([`uses_uia`] probes when neither decides), with
    /// `overrides` the count of override changes so far. Its classes are
    /// read with `classes_of` the first time. Local calls only, so safe on
    /// the thread that takes in an outpost's `WinEvents`.
    pub(crate) fn known_to_use_uia(
        &mut self,
        (arbitrator, overrides): (&Mutex<Arbitrator>, u64),
        (hwnd, owner): (isize, u32),
        classes_of: impl FnOnce(isize) -> WindowClasses,
    ) -> bool {
        if overrides != self.overrides {
            self.windows.clear();
            self.overrides = overrides;
        }
        let known = self.windows.get_or_learn(hwnd, owner, || KnownWindow {
            classes: classes_of(hwnd),
            lasting: None,
        });
        if let Some(uia) = known.lasting {
            return uia;
        }
        let (verdict, lasting) = lock(arbitrator).lasting_verdict(hwnd, &known.classes);
        if lasting {
            known.lasting = verdict;
        }
        verdict == Some(true)
    }

    /// Forgets window `hwnd`: it was destroyed, and its handle may be
    /// reused.
    pub(crate) fn forget(&mut self, hwnd: isize) {
        self.windows.forget(hwnd);
    }
}

/// `arbitrator`, locked: a poisoned lock is taken as it is, as its state
/// stays consistent.
fn lock(arbitrator: &Mutex<Arbitrator>) -> std::sync::MutexGuard<'_, Arbitrator> {
    arbitrator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
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

    /// Whether the event thread knows window 42, owned by thread 7 and of a
    /// class NVDA always reads through UIA, to be read through UIA, each
    /// read of its classes counted in `reads`.
    fn uia_window_known(
        windows: &mut EventWindows,
        arbitrator: &Mutex<Arbitrator>,
        reads: &std::cell::Cell<u32>,
    ) -> bool {
        windows.known_to_use_uia((arbitrator, 0), (42, 7), |_| {
            reads.set(reads.get() + 1);
            top("RAIL_WINDOW")
        })
    }

    #[test]
    fn the_event_thread_reads_a_windows_classes_once_for_its_events() {
        let arbitrator = Mutex::new(Arbitrator::default());
        let mut windows = EventWindows::default();
        let reads = std::cell::Cell::new(0);
        for _ in 0..3 {
            assert!(uia_window_known(&mut windows, &arbitrator, &reads));
        }
        assert_eq!(reads.get(), 1);
    }

    #[test]
    fn the_event_thread_forgets_a_destroyed_window() {
        let arbitrator = Mutex::new(Arbitrator::default());
        let mut windows = EventWindows::default();
        let reads = std::cell::Cell::new(0);
        assert!(uia_window_known(&mut windows, &arbitrator, &reads));
        windows.forget(42);
        assert!(uia_window_known(&mut windows, &arbitrator, &reads));
        assert_eq!(reads.get(), 2, "read again once the window was destroyed");
    }

    #[test]
    fn only_a_kept_probe_says_a_window_has_a_provider_of_its_own() {
        let mut arb = Arbitrator::new(&[]);
        let unknown = top("SomeAppWindow");
        assert_eq!(arb.verdict(1, &top("Shell_TrayWnd")), Some(true));
        assert!(
            !arb.probe_found_provider(1, &top("Shell_TrayWnd")),
            "a verdict from the class"
        );
        arb.set_forced(Some(true));
        assert!(!arb.probe_found_provider(1, &unknown), "a forced verdict");
        arb.set_forced(None);
        arb.record_probe(1, true);
        assert!(arb.probe_found_provider(1, &unknown));
        arb.record_probe(2, false);
        assert!(
            !arb.probe_found_provider(2, &unknown),
            "a probe that found none"
        );
    }

    #[test]
    fn a_probe_kept_for_a_window_its_class_decides_says_no_provider() {
        let mut arb = Arbitrator::new(&[]);
        arb.record_probe(1, true);
        assert!(!arb.probe_found_provider(1, &top("Shell_TrayWnd")));
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
    fn a_verdict_of_no_provider_runs_out_by_the_arbitrators_clock() {
        let start = Instant::now();
        let time = Arc::new(std::sync::Mutex::new(start));
        let mut arb = Arbitrator::new(&[]);
        let read = Arc::clone(&time);
        arb.set_clock(Arc::new(move || {
            *read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }));
        let classes = top("SomeUnknownClass");
        arb.record_probe(42, false);
        *time.lock().unwrap() =
            start + NEGATIVE_VERDICT_LIFETIME.saturating_sub(Duration::from_millis(1));
        assert_eq!(arb.verdict(42, &classes), Some(false));
        *time.lock().unwrap() = start + NEGATIVE_VERDICT_LIFETIME;
        assert_eq!(arb.verdict(42, &classes), None);
    }

    #[test]
    fn a_walks_time_limit_runs_out_by_the_arbitrators_clock() {
        let start = Instant::now();
        let time = Arc::new(std::sync::Mutex::new(start));
        let mut arb = Arbitrator::new(&[]);
        let read = Arc::clone(&time);
        arb.set_clock(Arc::new(move || {
            *read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }));
        let budget = Duration::from_secs(1);
        let clock = arb.clock();
        let out_of_time = time_limit(&clock, budget);
        *time.lock().unwrap() = start + budget.saturating_sub(Duration::from_millis(1));
        assert!(!out_of_time());
        *time.lock().unwrap() = start + budget;
        assert!(out_of_time());
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
    fn a_probe_that_timed_out_leaves_no_verdict_past_its_entry() {
        let mut arb = Arbitrator::new(&[]);
        let classes = top("SomeUnknownClass");
        let started = Instant::now();
        arb.record_timed_out(7);
        let finished = started + NEGATIVE_VERDICT_LIFETIME * 4;
        assert_eq!(
            arb.verdict_at(7, &classes, finished),
            Some(false),
            "the slow no holds for the rest of the entry that probed"
        );
        arb.renew_probes_since(started, finished);
        assert_eq!(
            arb.verdict_at(7, &classes, finished),
            None,
            "the next entry probes the window again"
        );
    }

    #[test]
    fn an_entry_begins_with_no_probe_that_timed_out() {
        let mut arb = Arbitrator::new(&[]);
        arb.record_timed_out(7);
        let started = arb.begin_entry();
        assert_eq!(arb.verdict_at(7, &top("SomeUnknownClass"), started), None);
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
