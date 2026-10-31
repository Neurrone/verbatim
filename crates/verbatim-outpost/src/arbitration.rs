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
//! The verdicts probes found are the outpost reducer's state ([`Verdicts`]):
//! a program reads its own copy and commits each probe as it returns, and
//! the event thread reads the push-time copy
//! (`docs/design/ownership.md`). The arbitrator
//! holds the configuration they are read with: the class lists, the
//! override and the clocks.
//!
//! A probe that finds a UIA provider is kept for the window's lifetime and
//! forgotten when the window is destroyed ([`VerdictChange::Forget`]), as
//! decision D15 specifies: a server-side provider does not go away. Each
//! kept verdict also records the thread that owned the window, and a
//! window now owned by another thread is probed afresh, so a reused window
//! handle does not inherit a verdict when its destroy event was lost. A probe
//! that finds none is kept for [`NEGATIVE_VERDICT_LIFETIME`], NVDA's cache
//! period, from the probe's end, and then the window is probed again. The
//! probe asks once, as NVDA does, and a window too busy to answer it in time
//! ([`verbatim_uia::RAW_PROBE_TIMEOUT_MS`]) has an unknown verdict
//! ([`Verdict::Unknown`]), which the outpost reads through MSAA, as NVDA
//! takes a cancelled probe as no provider, and keeps as "no" for the same
//! half second from the probe's end, a deliberate difference from NVDA,
//! which times its cache from before the probe, so a busy window is not
//! probed again within half a second of a probe that waited three seconds
//! (`docs/design/focus-pipeline-rework.md`, "Who owns each duplicated
//! fact", the verdicts).
//!
//! A kept verdict is compared with the observation of the event being
//! judged, never with when the worker handles it: each worker entry is
//! judged at its event's observation, or at its start for an entry with no
//! event ([`Judgement::judged_at`]), and a verdict judges the events
//! observed from its probe's beginning, stamped with the observation of the
//! judgement that probed, until its keep ends. So a focus's `WinEvent` copy
//! and its UIA focus, observed a few milliseconds apart, meet one verdict
//! whatever order and delay they reach the worker in, as NVDA's one cache
//! meets both as they arrive. A verdict judges no event observed before its
//! probe began: each window keeps its previous verdict for those, one being
//! enough behind the intake's marker barrier, since the intake orders by
//! arrival and the outpost's own event can be handled before a listener's
//! read observed earlier. The reducer records a verdict only from the worker
//! in charge: a probe returning on an abandoned worker records nothing but
//! in its own copy, so no verdict lands between a focus's two judgements
//! after the first was made; a probe that timed out is kept as unknown
//! whichever worker made it, so a window is not probed again more often
//! than NVDA probes it.
//!
//! The keep runs out by time because nothing else can end it: an
//! application that starts answering the probe raises no event, as a
//! provider is created on demand when the window is asked for one. Its ends
//! and the stamps of entries with no event are read from the arbitrator's
//! observation clock ([`ObservationClock`], the run's shared clock by
//! default), so a test that counts an operation's calls moves that time
//! itself ([`Arbitrator::set_observation_clock`]) and the probe is made
//! again exactly when the test says.
//!
//! The worker drops MSAA events whose window arbitrates to UIA and UIA events
//! whose window does not, so the two backends never both announce the same
//! change.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

use verbatim_ia2::WindowCache;

pub use crate::outpost::Verdicts;
use crate::outpost::window::{top_level_of, window_thread};
use crate::outpost::{Probe as KeptProbe, Probed, VerdictChange};
pub use verbatim_ia2::class::normalize_class_name;

/// How long a probe that found no UIA provider is trusted before the window
/// is probed again: NVDA's `isUIAWindow` cache period.
pub const NEGATIVE_VERDICT_LIFETIME: Duration = Duration::from_millis(500);

/// What time it is, for the lifetime of a kept verdict and the time budget
/// of a classic walk: [`Instant::now`] unless a test sets its own
/// ([`Arbitrator::set_clock`]).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// What time it is on the run's shared clock, in microseconds
/// ([`crate::protocol::now_us`]), for a kept verdict's keep and the stamp
/// an entry with no event is judged at: the shared clock unless a test sets
/// its own ([`Arbitrator::set_observation_clock`]).
pub type ObservationClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// [`NEGATIVE_VERDICT_LIFETIME`] in microseconds, as a keep's end is
/// stamped.
const NEGATIVE_VERDICT_LIFETIME_US: u64 = 500_000;

/// Whether `budget` has passed by `clock` since this call: the out-of-time
/// check a classic walk makes between its calls.
pub(crate) fn time_limit(clock: &Clock, budget: Duration) -> impl Fn() -> bool + '_ {
    let deadline = clock() + budget;
    move || clock() >= deadline
}

/// A window's backend as [`decide`] judges it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// The window uses UIA.
    Uia,
    /// It does not.
    NotUia,
    /// Its probe did not answer in time, so its backend is not known.
    Unknown,
}

/// The raw probe an arbitrator asks a window with:
/// [`verbatim_uia::raw_probe`], unless a unit test injects its outcome.
type Probe = Arc<dyn Fn(isize) -> verbatim_uia::RawProbe + Send + Sync>;

/// The classification a window's class name yields before any probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassVerdict {
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
pub(crate) fn class_verdict(classes: &WindowClasses) -> ClassVerdict {
    classify_with(
        |class| GOOD_UIA_CLASSES.contains(&class),
        |class| BAD_UIA_CLASSES.contains(&class),
        classes,
    )
}

/// The class verdict, with `good` and `bad` the class lists: the good list
/// and the shell first, as in NVDA.
fn classify_with(
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

/// What a judgement of window backends reads and records verdicts through:
/// a program's copy of the reducer's verdicts, at its entry's observation
/// (`docs/design/ownership.md`).
pub(crate) trait Judgement {
    /// The observation the judgement is made at: its entry's event's, or
    /// now on the arbitrator's observation clock for a judgement with no
    /// event.
    fn judged_at(&self, arbitrator: &Arbitrator) -> u64;
    /// The verdicts it reads: its own copy, with what it recorded.
    fn verdicts(&self) -> Arc<Verdicts>;
    /// Keeps `change` in its own copy and commits it to the reducer, which
    /// records it only from the worker generation in charge, but a probe
    /// that timed out and a window's forgetting from any.
    fn record(&self, change: VerdictChange);
}

/// Decides the backend for each window of one application, from its
/// configuration, the class lists, the override and the clocks, and the
/// verdicts the reducer keeps ([`Verdicts`]), which each caller gives.
pub struct Arbitrator {
    good_classes: HashSet<String>,
    bad_classes: HashSet<String>,
    /// A `SetBackendOverride` forcing every window: `Some(true)` for UIA,
    /// `Some(false)` for MSAA, `None` for normal arbitration.
    forced: Option<bool>,
    /// The time a classic walk's budget is measured by.
    clock: Clock,
    /// The time a keep's end and an entry with no event are stamped by.
    observed: ObservationClock,
    /// The probe [`decide`] asks a window with.
    probe: Probe,
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
            forced: None,
            clock: Arc::new(Instant::now),
            observed: Arc::new(crate::protocol::now_us),
            probe: Arc::new(verbatim_uia::raw_probe),
        }
    }

    /// Answers every probe with `outcome`, for a unit test that decides
    /// what the window answered rather than timing it.
    #[cfg(test)]
    pub(crate) fn set_probe(&mut self, outcome: verbatim_uia::RawProbe) {
        self.probe = Arc::new(move |_| outcome);
    }

    /// Reads the time from `clock` from now on, in place of the system's:
    /// for a test that decides when a classic walk's time budget is spent.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// Reads the observation time from `clock` from now on, in place of the
    /// run's shared clock: for a test that decides when a verdict's keep
    /// ends, by stamping the events it judges on the same clock.
    pub fn set_observation_clock(&mut self, clock: ObservationClock) {
        self.observed = clock;
    }

    /// The time by the arbitrator's clock, which a classic walk's budget is
    /// measured in.
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

    /// Now on the arbitrator's observation clock.
    #[must_use]
    pub fn observed_now(&self) -> u64 {
        (self.observed)()
    }

    /// Sets a `SetBackendOverride` command's forced backend: `Some(true)`
    /// forces UIA for every window, `Some(false)` forces MSAA, `None`
    /// restores normal arbitration.
    pub fn set_forced(&mut self, forced: Option<bool>) {
        self.forced = forced;
    }

    /// Records in `verdicts` a probe of `hwnd` that found a provider when
    /// `is_uia`, judging from now on the observation clock, the probe's
    /// end: a UIA verdict is kept until the window is destroyed, a non-UIA
    /// verdict for [`NEGATIVE_VERDICT_LIFETIME`] from now. What an
    /// outpost's program commits for its probe, for a test that probes by
    /// hand.
    pub fn record_probe(&self, verdicts: &mut Verdicts, hwnd: isize, is_uia: bool) {
        let now = self.observed_now();
        verdicts.apply(&self.probed(hwnd, now, is_uia));
    }

    /// The change recording a probe of `hwnd`, judging from `from_us`,
    /// that found a provider when `is_uia`, ending now on the observation
    /// clock.
    fn probed(&self, hwnd: isize, from_us: u64, is_uia: bool) -> VerdictChange {
        let probed = if is_uia {
            Probed::Uia
        } else {
            Probed::NotUia(
                self.observed_now()
                    .saturating_add(NEGATIVE_VERDICT_LIFETIME_US),
            )
        };
        kept(hwnd, from_us, probed)
    }

    /// The change recording that `hwnd` did not answer its probe in time,
    /// judging from `from_us`, the probe having returned now: no provider
    /// for [`NEGATIVE_VERDICT_LIFETIME`] from now, recorded whichever
    /// worker probed, so that a window is not probed again more often than
    /// NVDA probes it (`docs/design/focus-pipeline-rework.md`, the
    /// verdicts). A verdict kept for the window's lifetime stays, since a
    /// provider does not go away ([`Verdicts`]).
    fn timed_out(&self, hwnd: isize, from_us: u64) -> VerdictChange {
        let until = self
            .observed_now()
            .saturating_add(NEGATIVE_VERDICT_LIFETIME_US);
        kept(hwnd, from_us, Probed::TimedOut(until))
    }

    fn classify(&self, classes: &WindowClasses) -> ClassVerdict {
        classify_with(
            |class| self.good_classes.contains(class),
            |class| self.bad_classes.contains(class),
            classes,
        )
    }

    /// The verdict without probing, given the kept `verdicts`, for an event
    /// observed now on the observation clock: `Some(true)` for a UIA
    /// window, `Some(false)` for a non-UIA window, and `None` when only the
    /// blocking probe can decide, as no kept verdict judges that
    /// observation.
    #[must_use]
    pub fn verdict(
        &self,
        verdicts: &Verdicts,
        hwnd: isize,
        classes: &WindowClasses,
    ) -> Option<bool> {
        self.verdict_at(verdicts, hwnd, classes, self.observed_now())
    }

    /// The verdict without probing for an event observed at `at_us`.
    #[must_use]
    pub fn verdict_at(
        &self,
        verdicts: &Verdicts,
        hwnd: isize,
        classes: &WindowClasses,
        at_us: u64,
    ) -> Option<bool> {
        if self.forced.is_some() {
            return self.forced;
        }
        match self.classify(classes) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown => match verdicts.kept_at(hwnd, window_thread(hwnd), at_us)? {
                Probed::Uia => Some(true),
                Probed::Excluded | Probed::NotUia(_) | Probed::TimedOut(_) => Some(false),
            },
        }
    }

    /// The verdict [`verdict`](Self::verdict) gives, and whether it holds
    /// for the window's lifetime until the override changes: a forced one,
    /// one from the class lists, or a kept probe that found a provider, or
    /// one NVDA does not use. A probe that found none runs out, and a
    /// window not yet probed has no verdict, so neither lasts.
    #[must_use]
    pub fn lasting_verdict(
        &self,
        verdicts: &Verdicts,
        hwnd: isize,
        classes: &WindowClasses,
    ) -> (Option<bool>, bool) {
        let lasting = self.forced.is_some()
            || self.classify(classes) != ClassVerdict::Unknown
            || verdicts.lasts(hwnd, window_thread(hwnd));
        (self.verdict(verdicts, hwnd, classes), lasting)
    }
}

/// The change keeping `probed` for `hwnd`, judging from `from_us`, with the
/// thread that owns the window now.
fn kept(hwnd: isize, from_us: u64, probed: Probed) -> VerdictChange {
    VerdictChange::Probed(KeptProbe {
        hwnd,
        owner: window_thread(hwnd),
        from_us,
        probed,
    })
}

// The class names arbitration decides on, read with inexpensive local
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

/// Whether window `hwnd` is judged UIA without a probe, from the class
/// lists or a kept verdict of `judgement`'s, at its observation: local
/// calls only.
pub(crate) fn known_to_use_uia(
    arbitrator: &Mutex<Arbitrator>,
    judgement: &dyn Judgement,
    hwnd: isize,
) -> bool {
    let classes = WindowClasses::of(hwnd);
    let verdicts = judgement.verdicts();
    let arbitrator = lock(arbitrator);
    let at_us = judgement.judged_at(&arbitrator);
    arbitrator.verdict_at(&verdicts, hwnd, &classes, at_us) == Some(true)
}

/// [`decide`] for an outpost's worker, `true` for UIA: a window whose probe
/// timed out is read through MSAA, and kept as a "no" for
/// [`NEGATIVE_VERDICT_LIFETIME`] from the probe's end.
pub(crate) fn uses_uia(
    arbitrator: &Mutex<Arbitrator>,
    judgement: &dyn Judgement,
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> bool {
    decide(arbitrator, judgement, hwnd, console) == Verdict::Uia
}

/// Decides window `hwnd`'s backend with `arbitrator`, as NVDA's
/// `isUIAWindow` decides it, judged at `judgement`'s observation: the class
/// lists and its kept verdicts first, then the provider probe, whose answer
/// `judgement` records (a provider for the window's lifetime, none for a
/// short while), and the reducer keeps only from the worker in charge. A
/// probe that timed out is [`Verdict::Unknown`], kept whichever worker made
/// it. The probe is the raw probe ([`verbatim_uia::raw_probe`]), and no
/// connection follows it. A console window with a provider is used only
/// when `console` answers that its text reports formatting
/// ([`PostProbeCheck::Console`]), which an outpost reads by the window's
/// handle ([`verbatim_uia::console_reports_formatting`]).
fn decide(
    arbitrator: &Mutex<Arbitrator>,
    judgement: &dyn Judgement,
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> Verdict {
    let classes = WindowClasses::of(hwnd);
    let verdicts = judgement.verdicts();
    let (probe, at_us) = {
        let arbitrator = lock(arbitrator);
        let at_us = judgement.judged_at(&arbitrator);
        match arbitrator.verdict_at(&verdicts, hwnd, &classes, at_us) {
            Some(true) => return Verdict::Uia,
            Some(false) => return Verdict::NotUia,
            None => (Arc::clone(&arbitrator.probe), at_us),
        }
    };
    drop(verdicts);
    let started = std::time::Instant::now();
    let probed = probe(hwnd);
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
                let change = lock(arbitrator).probed(hwnd, at_us, true);
                judgement.record(change);
                Verdict::Uia
            }
            Some(false) => {
                judgement.record(kept(hwnd, at_us, Probed::Excluded));
                Verdict::NotUia
            }
            // The application did not answer: asked again next time.
            None => Verdict::NotUia,
        };
    }
    if probed == verbatim_uia::RawProbe::TimedOut {
        // Kept from the probe's end whichever worker probed, so that a busy
        // window is not probed again within half a second of a probe that
        // waited its whole bound.
        let change = lock(arbitrator).timed_out(hwnd, at_us);
        judgement.record(change);
        return Verdict::Unknown;
    }
    let change = lock(arbitrator).probed(hwnd, at_us, is_uia);
    judgement.record(change);
    if is_uia {
        Verdict::Uia
    } else {
        Verdict::NotUia
    }
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
    /// `overrides` the count of override changes so far and `verdicts` the
    /// push-time copy of the reducer's verdicts, taken only while the
    /// window's backend is not decided for its lifetime. Its classes are
    /// read with `classes_of` the first time. Local calls only, so safe on
    /// the thread that takes in an outpost's `WinEvents`.
    pub(crate) fn known_to_use_uia(
        &mut self,
        (arbitrator, verdicts, overrides): (
            &Mutex<Arbitrator>,
            impl FnOnce() -> Arc<Verdicts>,
            u64,
        ),
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
        let verdicts = verdicts();
        let (verdict, lasting) = lock(arbitrator).lasting_verdict(&verdicts, hwnd, &known.classes);
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
    use std::cell::{Cell, RefCell};

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
        let verdicts = || Arc::new(Verdicts::default());
        windows.known_to_use_uia((arbitrator, verdicts, 0), (42, 7), |_| {
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
    fn good_class_is_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(
            arb.verdict(&Verdicts::default(), 1, &top("RAIL_WINDOW")),
            Some(true)
        );
    }

    #[test]
    fn bad_class_is_non_uia_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(
            arb.verdict(&Verdicts::default(), 1, &top("Edit")),
            Some(false)
        );
        assert_eq!(
            arb.verdict(&Verdicts::default(), 1, &top("RichEdit20")),
            Some(false)
        );
    }

    #[test]
    fn extra_good_class_overrides_to_uia() {
        let arb = Arbitrator::new(&["MyAppCanvas"]);
        assert_eq!(
            arb.verdict(&Verdicts::default(), 1, &top("MyAppCanvas")),
            Some(true)
        );
    }

    /// An arbitrator with the reducer's record of verdicts, as the tests
    /// judge `WINDOW` with them.
    struct Judged {
        arb: Mutex<Arbitrator>,
        /// The reducer's record.
        record: RefCell<Verdicts>,
        /// The worker generation in charge.
        in_charge: Cell<u64>,
    }

    impl Judged {
        fn set_probe(&self, probe: verbatim_uia::RawProbe) {
            lock(&self.arb).set_probe(probe);
        }

        fn set_in_charge(&self, generation: u64) {
            self.in_charge.set(generation);
        }

        /// `WINDOW`'s verdict by the reducer's record, for an event of
        /// classes `classes` observed at `at_us`.
        fn verdict_at(&self, classes: &WindowClasses, at_us: u64) -> Option<bool> {
            lock(&self.arb).verdict_at(&self.record.borrow(), WINDOW, classes, at_us)
        }
    }

    /// One entry's judgement, by worker `generation`, at `at_us`: its own
    /// copy of the record, and each change committed to the record as the
    /// reducer records it, only from the generation in charge but a probe
    /// that timed out and a window's forgetting.
    struct Entry<'a> {
        judged: &'a Judged,
        at_us: u64,
        generation: u64,
        copy: RefCell<Arc<Verdicts>>,
    }

    impl Judgement for Entry<'_> {
        fn judged_at(&self, _: &Arbitrator) -> u64 {
            self.at_us
        }

        fn verdicts(&self) -> Arc<Verdicts> {
            Arc::clone(&self.copy.borrow())
        }

        fn record(&self, change: VerdictChange) {
            Arc::make_mut(&mut self.copy.borrow_mut()).apply(&change);
            if self.generation == self.judged.in_charge.get() || change.kept_from_any_generation() {
                self.judged.record.borrow_mut().apply(&change);
            }
        }
    }

    /// An arbitrator whose observation clock reads the time the test sets
    /// in the returned cell, in microseconds, which starts at 0, and whose
    /// probe answers `probe`, with an empty record.
    fn arbitrator_on_test_clock(probe: verbatim_uia::RawProbe) -> (Judged, Arc<Mutex<u64>>) {
        let time = Arc::new(Mutex::new(0));
        let mut arb = Arbitrator::new(&[]);
        let read = Arc::clone(&time);
        arb.set_observation_clock(Arc::new(move || {
            *read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }));
        arb.set_probe(probe);
        let judged = Judged {
            arb: Mutex::new(arb),
            record: RefCell::new(Verdicts::default()),
            in_charge: Cell::new(0),
        };
        (judged, time)
    }

    /// The test clock's cell set to `us`.
    fn set(time: &Mutex<u64>, us: u64) {
        *time.lock().unwrap() = us;
    }

    /// The window the tests judge: not a real window, so its classes are
    /// on no list and its probe is the one the test injects.
    const WINDOW: isize = 42;

    /// Whether worker generation `generation`'s entry judged at `at_us`
    /// finds `WINDOW` UIA, probing it if no kept verdict judges that
    /// observation.
    fn judged_at(judged: &Judged, at_us: u64, generation: u64) -> bool {
        let entry = Entry {
            judged,
            at_us,
            generation,
            copy: RefCell::new(Arc::new(judged.record.borrow().clone())),
        };
        uses_uia(&judged.arb, &entry, WINDOW, || None)
    }

    #[test]
    fn a_verdict_of_no_provider_is_probed_again_after_its_lifetime() {
        let (arb, time) = arbitrator_on_test_clock(verbatim_uia::RawProbe::Answered(0));
        let classes = top("SomeUnknownClass");
        assert!(!judged_at(&arb, 0, 0));
        assert_eq!(
            arb.verdict_at(&classes, NEGATIVE_VERDICT_LIFETIME_US - 1),
            Some(false)
        );
        assert_eq!(
            arb.verdict_at(&classes, NEGATIVE_VERDICT_LIFETIME_US),
            None,
            "a probe that found no provider may have met a busy application"
        );
        arb.set_probe(verbatim_uia::RawProbe::Provider);
        set(&time, NEGATIVE_VERDICT_LIFETIME_US);
        assert!(judged_at(&arb, NEGATIVE_VERDICT_LIFETIME_US, 0));
        assert_eq!(
            arb.verdict_at(&classes, 3_600_000_000),
            Some(true),
            "a provider, once found, is kept"
        );
    }

    #[test]
    fn a_verdict_of_no_provider_runs_out_by_the_events_observation() {
        let (arb, time) = arbitrator_on_test_clock(verbatim_uia::RawProbe::Answered(0));
        let classes = top("SomeUnknownClass");
        // Probed at 1 ms, the probe answering at 2 ms.
        set(&time, 2_000);
        assert!(!judged_at(&arb, 1_000, 0));
        // Handled long after, an event is judged by when it was observed.
        set(&time, 10_000_000);
        assert_eq!(
            arb.verdict_at(&classes, 2_000 + NEGATIVE_VERDICT_LIFETIME_US - 1),
            Some(false)
        );
        assert_eq!(
            arb.verdict_at(&classes, 2_000 + NEGATIVE_VERDICT_LIFETIME_US),
            None
        );
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
    fn a_probe_that_timed_out_is_kept_for_half_a_second_from_its_end() {
        let (arb, time) = arbitrator_on_test_clock(verbatim_uia::RawProbe::TimedOut);
        let classes = top("SomeUnknownClass");
        // Judged at its event's observation, 0, the probe returning at 3 s.
        let probe_ended = 3_000_000;
        set(&time, probe_ended);
        assert!(!judged_at(&arb, 0, 0));
        assert_eq!(
            arb.verdict_at(&classes, probe_ended + NEGATIVE_VERDICT_LIFETIME_US - 1),
            Some(false),
            "the slow no holds for events observed until half a second after the probe's end"
        );
        assert_eq!(
            arb.verdict_at(&classes, probe_ended + NEGATIVE_VERDICT_LIFETIME_US),
            None,
            "half a second after the probe's end the window is probed again"
        );
    }

    #[test]
    fn a_kept_verdict_judges_an_event_observed_before_its_keep_ended() {
        let (arb, time) = arbitrator_on_test_clock(verbatim_uia::RawProbe::TimedOut);
        // The copy, observed at 0 and handled at once, waits 3 seconds for
        // its probe, which times out.
        set(&time, 3_000_000);
        assert!(!judged_at(&arb, 0, 0));
        // Its UIA focus, observed at 10 ms, is handled after it, at 3 s, and
        // a provider has appeared meanwhile: judged at its observation, it
        // meets the unknown, with no probe.
        arb.set_probe(verbatim_uia::RawProbe::Provider);
        assert!(!judged_at(&arb, 10_000, 0));
        // An event observed after the keep ended is probed again.
        assert!(judged_at(&arb, 3_000_000 + NEGATIVE_VERDICT_LIFETIME_US, 0));
    }

    #[test]
    fn a_fresh_probe_does_not_judge_an_event_observed_before_it() {
        let (arb, time) = arbitrator_on_test_clock(verbatim_uia::RawProbe::Answered(0));
        let classes = top("SomeUnknownClass");
        // A "no" judged at 0, kept for events observed until 500 ms.
        assert!(!judged_at(&arb, 0, 0));
        // The outpost's own event observed at 600 ms is handled first and
        // probes afresh: the window now has a provider.
        arb.set_probe(verbatim_uia::RawProbe::Provider);
        set(&time, 600_000);
        assert!(judged_at(&arb, 600_000, 0));
        // A listener's read observed at 450 ms, handled after it, is judged
        // by the verdict that stood at its observation.
        assert_eq!(arb.verdict_at(&classes, 450_000), Some(false));
        assert_eq!(arb.verdict_at(&classes, 600_000), Some(true));
    }

    #[test]
    fn an_abandoned_workers_probe_records_no_verdict() {
        let (arb, _) = arbitrator_on_test_clock(verbatim_uia::RawProbe::Provider);
        let classes = top("SomeUnknownClass");
        arb.set_in_charge(1);
        assert!(
            judged_at(&arb, 0, 0),
            "the abandoned worker's own entry still reads its probe's answer"
        );
        assert_eq!(
            arb.verdict_at(&classes, 0),
            None,
            "nothing kept for the worker in charge"
        );
    }

    #[test]
    fn an_abandoned_workers_timed_out_probe_is_kept_as_unknown() {
        let (arb, _) = arbitrator_on_test_clock(verbatim_uia::RawProbe::TimedOut);
        let classes = top("SomeUnknownClass");
        arb.set_in_charge(1);
        assert!(!judged_at(&arb, 0, 0));
        assert_eq!(arb.verdict_at(&classes, 0), Some(false));
    }

    #[test]
    fn a_probed_verdict_lasts_until_the_window_is_destroyed() {
        let arb = Arbitrator::new(&[]);
        let mut verdicts = Verdicts::default();
        assert_eq!(arb.verdict(&verdicts, 42, &top("SomeUnknownClass")), None);
        arb.record_probe(&mut verdicts, 42, true);
        assert_eq!(
            arb.verdict(&verdicts, 42, &top("SomeUnknownClass")),
            Some(true)
        );
        verdicts.apply(&VerdictChange::Forget(42));
        assert_eq!(
            arb.verdict(&verdicts, 42, &top("SomeUnknownClass")),
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
                arb.verdict(&Verdicts::default(), 1, &top(class)),
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
                &Verdicts::default(),
                1,
                &WindowClasses::new("Windows.UI.Input.InputSite.WindowClass", "Shell_TrayWnd")
            ),
            Some(true)
        );
        assert_eq!(
            arb.verdict(
                &Verdicts::default(),
                1,
                &WindowClasses::new("Edit", "Shell_TrayWnd")
            ),
            Some(true),
            "the shell rule comes before the bad list, as in NVDA"
        );
        assert_eq!(
            arb.verdict(
                &Verdicts::default(),
                1,
                &WindowClasses::new("Start", "Shell_TrayWnd")
            ),
            None
        );
    }

    #[test]
    fn a_normalized_bad_class_is_msaa_without_probing() {
        let arb = Arbitrator::new(&[]);
        assert_eq!(
            arb.verdict(
                &Verdicts::default(),
                1,
                &top("WindowsForms10.EDIT.app.0.141b42a_r9_ad1")
            ),
            Some(false)
        );
        assert_eq!(
            arb.verdict(&Verdicts::default(), 1, &top("TRichEdit")),
            Some(false)
        );
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
        let arb = Arbitrator::new(&[]);
        let mut verdicts = Verdicts::default();
        let classes = top("ConsoleWindowClass");
        verdicts.apply(&kept(5, arb.observed_now(), Probed::Excluded));
        assert_eq!(
            arb.verdict_at(&verdicts, 5, &classes, arb.observed_now() + 3_600_000_000),
            Some(false)
        );
        verdicts.apply(&VerdictChange::Forget(5));
        assert_eq!(arb.verdict(&verdicts, 5, &classes), None);
    }
}
