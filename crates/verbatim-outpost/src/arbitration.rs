//! Per-window backend arbitration (architecture section 4).
//!
//! Mirrors NVDA's ladder, cheapest rung first, on the window's class name
//! normalized as NVDA normalizes it (`verbatim_model::class::normalize`): a
//! config-overridable "good" class list forces UIA, and so does a Windows 11
//! shell window, recognized by its root ancestor's class; a "bad" class list
//! (seeded from NVDA's, where UIA implementations are known to interfere with
//! MSAA) forces MSAA; otherwise the window is probed with the raw probe, one
//! `WM_GETOBJECT` and no UIA connection ([`verbatim_uia::raw_probe`];
//! `books/dev/src/explanation/design/listener-without-connections.md`,
//! decision 2). A window with a provider is still set aside for MSAA when
//! its provider is one NVDA does not use: an old console's, or a list
//! view's outside Windows Forms
//! ([`verbatim_reader_rules::class::post_probe_check`]), which both read
//! from the window's class name, a local call. Class-list checks are fast
//! local window calls; the probe blocks on the target's message pump and
//! runs only on the worker.
//!
//! Core's backend cache is the one lasting keeper of a window's verdict
//! (`books/dev/src/reference/adr/0004-focus-validity.md`): a request
//! carries Core's verdict for its window, or "probe first". The outpost
//! keeps what it probes only for the request it probes in
//! ([`RequestVerdicts`]), as one program's memo, so a window a walk crosses
//! is probed at most once per request, and a "probe first" request reports
//! only a probe it made. A probe that did not answer in time
//! ([`verbatim_uia::UIA_RAW_PROBE_TIMEOUT_MS`]) has an unknown verdict
//! ([`Verdict::Unknown`]), which the outpost reads through MSAA, as NVDA
//! takes a cancelled probe as no provider.
//!
//! These verdicts decide only how the outpost reads a window; which
//! backend's events speak for a window is Core's judgement, by its own
//! verdicts (`books/dev/src/reference/adr/0001-focus.md`, "One judge per
//! fact").

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use crate::outpost::lock::{Lock, LockGuard};

use verbatim_model::class::{BAD, GOOD};
use verbatim_reader_rules::class::{
    ClassVerdict, WindowClasses, classify_with, post_probe_check, provider_usable,
};

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

/// A probe one request made: what it found, and when it began on the
/// outpost's observation clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Probed {
    /// What it found.
    pub(crate) verdict: Verdict,
    /// When it began.
    pub(crate) began_us: u64,
}

/// What one request's probes found, by window: kept for that request
/// alone, and dropped with it.
#[derive(Debug, Default)]
pub(crate) struct RequestVerdicts {
    probed: RefCell<BTreeMap<isize, Probed>>,
}

impl RequestVerdicts {
    /// The probe this request made of `hwnd`, if it made one.
    pub(crate) fn probed(&self, hwnd: isize) -> Option<Probed> {
        self.probed.borrow().get(&hwnd).copied()
    }

    fn record(&self, hwnd: isize, probed: Probed) {
        self.probed.borrow_mut().insert(hwnd, probed);
    }
}

/// The raw probe an arbitrator asks a window with:
/// [`verbatim_uia::raw_probe`], unless a unit test injects its outcome.
type Probe = Arc<dyn Fn(isize) -> verbatim_uia::RawProbe + Send + Sync>;

/// Decides the backend for each window of one application, from its
/// configuration and the class lists, probing a window they do not decide.
pub struct Arbitrator {
    good_classes: HashSet<String>,
    bad_classes: HashSet<String>,
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
        let bad_classes = BAD.iter().map(|s| (*s).to_owned()).collect();
        let good_classes = GOOD
            .iter()
            .chain(extra_good.iter())
            .map(|s| (*s).to_owned())
            .collect();
        Self {
            good_classes,
            bad_classes,
            probe: Arc::new(verbatim_uia::raw_probe),
        }
    }

    /// Answers every probe with `outcome`, counting each in `count`, for a
    /// unit test that decides what the window answered rather than timing
    /// it.
    #[cfg(test)]
    fn set_probe(&mut self, outcome: verbatim_uia::RawProbe, count: Arc<Lock<u32>>) {
        self.probe = Arc::new(move |_| {
            *count.lock() += 1;
            outcome
        });
    }

    /// The verdict the class lists give a window of `classes`:
    /// `Some(true)` for UIA, `Some(false)` for MSAA, and `None` when only
    /// the probe can decide.
    #[must_use]
    pub fn class_verdict(&self, classes: &WindowClasses) -> Option<bool> {
        match classify_with(
            |class| self.good_classes.contains(class),
            |class| self.bad_classes.contains(class),
            classes,
        ) {
            ClassVerdict::Uia => Some(true),
            ClassVerdict::NonUia => Some(false),
            ClassVerdict::Unknown => None,
        }
    }
}

/// [`decide`] for an outpost's worker, `true` for UIA: a window whose probe
/// timed out is read through MSAA.
pub(crate) fn uses_uia(
    arbitrator: &Lock<Arbitrator>,
    (verdicts, now_us): (&RequestVerdicts, u64),
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> bool {
    decide(arbitrator, (verdicts, now_us), hwnd, console, Lists::First) == Verdict::Uia
}

/// [`uses_uia`] for a window Core asks to be probed first: the class lists
/// do not decide it, and it is probed unless this request already probed
/// it, as design section 7 says Core's "probe first" is answered by a probe
/// of the request's window, always, which `verdicts` then holds.
pub(crate) fn probed_uses_uia(
    arbitrator: &Lock<Arbitrator>,
    (verdicts, now_us): (&RequestVerdicts, u64),
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
) -> bool {
    decide(
        arbitrator,
        (verdicts, now_us),
        hwnd,
        console,
        Lists::Skipped,
    ) == Verdict::Uia
}

/// Whether [`decide`] consults the class lists before the probe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Lists {
    /// The lists decide a window they name.
    First,
    /// The window is probed whatever the lists say.
    Skipped,
}

/// Decides window `hwnd`'s backend with `arbitrator`, as NVDA's
/// `isUIAWindow` decides it: the class lists, then what this request
/// probed of the window, then the provider probe, begun at `now_us`, which
/// `verdicts` keeps for the rest of the request. The probe is the raw
/// probe ([`verbatim_uia::raw_probe`]), and no connection follows it. A
/// console window with a provider is used only when `console` answers that
/// its text reports formatting
/// ([`verbatim_reader_rules::class::PostProbeCheck::Console`]), which an
/// outpost reads by the window's handle
/// ([`verbatim_uia::console_reports_formatting`]).
fn decide(
    arbitrator: &Lock<Arbitrator>,
    (verdicts, now_us): (&RequestVerdicts, u64),
    hwnd: isize,
    console: impl FnOnce() -> Option<bool>,
    lists: Lists,
) -> Verdict {
    // A window this request probed keeps that verdict for the rest of
    // it, the lists included, so that its reads agree with its probe.
    if let Some(probed) = verdicts.probed(hwnd) {
        return probed.verdict;
    }
    let classes = verbatim_win32::window::window_classes(hwnd);
    let probe = {
        let arbitrator = lock(arbitrator);
        match (lists, arbitrator.class_verdict(&classes)) {
            (Lists::First, Some(true)) => return Verdict::Uia,
            (Lists::First, Some(false)) => return Verdict::NotUia,
            (Lists::First, None) | (Lists::Skipped, _) => Arc::clone(&arbitrator.probe),
        }
    };
    let started = Instant::now();
    let probed = probe(hwnd);
    tracing::debug!(
        hwnd,
        class = classes.raw,
        ?probed,
        elapsed_ms = started.elapsed().as_millis(),
        "arbitration probed"
    );
    let verdict = match probed {
        // The worker was abandoned: nothing was sent, and nothing is
        // learned of the window.
        verbatim_uia::RawProbe::Abandoned => return Verdict::Unknown,
        verbatim_uia::RawProbe::TimedOut => Verdict::Unknown,
        verbatim_uia::RawProbe::Provider => match post_probe_check(&classes) {
            // NVDA uses some providers only after checking them; one whose
            // application did not answer the check is read through MSAA.
            Some(check) => {
                let usable = provider_usable(check, &classes, console);
                tracing::debug!(hwnd, ?check, ?usable, "arbitration checked the provider");
                if usable == Some(true) {
                    Verdict::Uia
                } else {
                    Verdict::NotUia
                }
            }
            None => Verdict::Uia,
        },
        _ => Verdict::NotUia,
    };
    verdicts.record(
        hwnd,
        Probed {
            verdict,
            began_us: now_us,
        },
    );
    verdict
}

/// `arbitrator`, locked: a poisoned lock is taken as it is, as its state
/// stays consistent.
#[track_caller]
fn lock(arbitrator: &Lock<Arbitrator>) -> LockGuard<'_, Arbitrator> {
    arbitrator.lock()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The classes of a top-level window of class `class`.
    fn top(class: &str) -> WindowClasses {
        WindowClasses::new(class, class)
    }

    #[test]
    fn extra_good_class_overrides_to_uia() {
        let arb = Arbitrator::new(&["MyAppCanvas"]);
        assert_eq!(arb.class_verdict(&top("MyAppCanvas")), Some(true));
        assert_eq!(
            Arbitrator::new(&[]).class_verdict(&top("MyAppCanvas")),
            None
        );
    }

    /// An arbitrator whose probe answers `outcome`, with its count of
    /// probes.
    fn probing(outcome: verbatim_uia::RawProbe) -> (Lock<Arbitrator>, Arc<Lock<u32>>) {
        let count = Arc::new(Lock::new(0));
        let mut arb = Arbitrator::new(&[]);
        arb.set_probe(outcome, Arc::clone(&count));
        (Lock::new(arb), count)
    }

    /// The window of these tests: no window, whose class no list names.
    const WINDOW: isize = 0x7777_0001;

    /// A window a request judges twice is probed once, and a "probe first"
    /// request reports that probe, begun when it began; the next request
    /// keeps nothing of it and probes again, so it reports only a probe it
    /// made. Source-derived, approved by Dickson on 2026-11-21: NVDA
    /// probes again once its half-second cache lapses
    /// (`nvda/source/UIAHandler/__init__.py` 1371 to 1378), and Core's
    /// cache is the one that keeps the verdict that long.
    #[test]
    fn a_probe_first_request_whose_window_has_a_kept_verdict_reports_only_a_probe_it_made() {
        let (arb, count) = probing(verbatim_uia::RawProbe::Provider);
        let first = RequestVerdicts::default();
        assert!(uses_uia(&arb, (&first, 1_000), WINDOW, || None));
        assert!(uses_uia(&arb, (&first, 2_000), WINDOW, || None));
        assert_eq!(*count.lock(), 1, "one probe within a request");
        assert_eq!(
            first.probed(WINDOW),
            Some(Probed {
                verdict: Verdict::Uia,
                began_us: 1_000
            })
        );
        let next = RequestVerdicts::default();
        assert_eq!(next.probed(WINDOW), None, "the next request keeps nothing");
        assert!(uses_uia(&arb, (&next, 600_000), WINDOW, || None));
        assert_eq!(*count.lock(), 2);
        assert_eq!(
            next.probed(WINDOW).map(|probed| probed.began_us),
            Some(600_000)
        );
    }

    /// A window Core asks to be probed first is probed though the class
    /// lists name it, and the rest of the request reads it by that probe:
    /// the desktop window, its class made a good one, answers the probe
    /// with no provider.
    #[test]
    fn a_probe_first_window_is_probed_whatever_the_class_lists_say() {
        let desktop = verbatim_win32::window::desktop_window();
        let class = verbatim_win32::window::window_classes(desktop).raw;
        let mut arb = Arbitrator::new(&[class.as_str()]);
        let count = Arc::new(Lock::new(0));
        arb.set_probe(verbatim_uia::RawProbe::Answered(0), Arc::clone(&count));
        let arb = Lock::new(arb);
        let lists = RequestVerdicts::default();
        assert!(uses_uia(&arb, (&lists, 1_000), desktop, || None));
        assert_eq!(*count.lock(), 0, "the lists decide a walk's window");
        let request = RequestVerdicts::default();
        assert!(!probed_uses_uia(&arb, (&request, 1_000), desktop, || None));
        assert_eq!(*count.lock(), 1);
        assert_eq!(
            request.probed(desktop),
            Some(Probed {
                verdict: Verdict::NotUia,
                began_us: 1_000
            })
        );
        assert!(
            !uses_uia(&arb, (&request, 2_000), desktop, || None),
            "the request's later reads agree with its probe"
        );
        assert_eq!(*count.lock(), 1);
    }

    #[test]
    fn a_probe_that_timed_out_is_read_through_msaa_for_the_rest_of_its_request() {
        let (arb, count) = probing(verbatim_uia::RawProbe::TimedOut);
        let request = RequestVerdicts::default();
        assert!(!uses_uia(&arb, (&request, 1_000), WINDOW, || None));
        assert!(!uses_uia(&arb, (&request, 2_000), WINDOW, || None));
        assert_eq!(*count.lock(), 1);
        assert_eq!(
            request.probed(WINDOW).map(|probed| probed.verdict),
            Some(Verdict::Unknown)
        );
    }

    #[test]
    fn an_abandoned_workers_probe_keeps_nothing() {
        let (arb, _) = probing(verbatim_uia::RawProbe::Abandoned);
        let request = RequestVerdicts::default();
        assert!(!uses_uia(&arb, (&request, 1_000), WINDOW, || None));
        assert_eq!(request.probed(WINDOW), None);
    }
}
