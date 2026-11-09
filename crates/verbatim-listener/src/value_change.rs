//! What the listener's event thread knows of each application's windows
//! and objects, for its value changes: the one record of them, which
//! nothing else reads or writes, so a value change dropped here takes no
//! shared lock and makes no call. The reading threads send what they find
//! over a channel, drained at the start of each value change's callback.
//!
//! NVDA's whole test of a window with a provider is judged here, with one
//! limit: a console window with a provider is answered as a window NVDA
//! reads through UIA with no check, as the check needs a UIA connection or a
//! held element and the listener makes no connection and holds none on this
//! path, so an old console with an incomplete provider differs from NVDA's
//! test (`nvda/source/UIAHandler/__init__.py` 1344 to 1345); its value
//! changes reach no bar.
//!
//! An application's knowledge is kept to a least-recently-used cap of
//! applications, as no exit is reported for a process with no outpost, and
//! each table is capped. A reused window's handle has no answer, by the
//! owning thread each answer is kept with.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};

use verbatim_listener_logic::focus::{
    BAR_VERDICTS_KEPT, BarObject, BarVerdict, ClassicBarRead, WindowAnswer, classic_value_change,
};
use verbatim_model::Pid;
use verbatim_reader_rules::verdicts::{
    NEGATIVE_VERDICT_LIFETIME_US, Probe, Probed, VerdictChange, Verdicts,
};

/// The most applications the record keeps, the least recently used
/// forgotten first.
const MAX_APPS: usize = 64;

/// The most windows an application's record keeps an answer for.
const WINDOWS_KEPT: usize = 256;

/// What a reading thread found, for the event thread's record.
#[derive(Debug)]
pub(crate) enum Found {
    /// A window was asked and answered.
    Window {
        /// The application.
        pid: Pid,
        /// The job that asked.
        job: u64,
        /// The answer.
        probe: Probe,
    },
    /// A job that asked a window ended with no answer: the window is no
    /// longer being asked, and nothing was learned.
    NotAsked {
        /// The application.
        pid: Pid,
        /// The window.
        hwnd: isize,
        /// The job that asked.
        job: u64,
    },
    /// What an object was found to be.
    Object {
        /// The application.
        pid: Pid,
        /// The object.
        object: BarObject,
        /// The thread that owned its window when the event came.
        owner: u32,
        /// What it is.
        verdict: BarVerdict,
    },
}

/// One value change, as the callback judges it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ValueChange {
    /// The application it goes to.
    pub(crate) pid: Pid,
    /// Its object.
    pub(crate) object: BarObject,
    /// The thread that owns its window now.
    pub(crate) owner: u32,
    /// When the callback observed it.
    pub(crate) at_us: u64,
}

/// What the callback does with a value change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Judgement {
    /// Nothing: dropped with no lock and no read.
    Dropped,
    /// Queues this read.
    Read(ClassicBarRead),
    /// Queues a job that asks the window, numbered this, and carries the
    /// event.
    Ask(u64),
}

/// What an application's objects were found to be: at most
/// [`BAR_VERDICTS_KEPT`], the oldest forgotten first.
#[derive(Default)]
struct BarMemo {
    answers: HashMap<BarObject, (u32, BarVerdict)>,
    order: VecDeque<BarObject>,
}

impl BarMemo {
    /// What `object`, whose window is owned by `owner`, was found to be.
    fn get(&self, object: BarObject, owner: u32) -> Option<BarVerdict> {
        self.answers
            .get(&object)
            .filter(|(kept, _)| *kept == owner)
            .map(|&(_, verdict)| verdict)
    }

    /// Remembers what `object` was found to be.
    fn remember(&mut self, object: BarObject, owner: u32, verdict: BarVerdict) {
        if self.answers.insert(object, (owner, verdict)).is_none() {
            self.order.push_back(object);
        }
        if self.order.len() > BAR_VERDICTS_KEPT
            && let Some(oldest) = self.order.pop_front()
        {
            self.answers.remove(&oldest);
        }
    }
}

/// What the event thread knows of one application.
#[derive(Default)]
struct AppKnowledge {
    /// The windows' answers, at most [`WINDOWS_KEPT`].
    windows: Verdicts,
    /// The windows being asked, each by the job that asks.
    asking: HashMap<isize, u64>,
    /// The objects' answers.
    objects: BarMemo,
    /// When it was last used, by the record's own count.
    used: u64,
}

/// The event thread's record ([module docs](self)).
pub(crate) struct ValueChangeKnowledge {
    apps: HashMap<Pid, AppKnowledge>,
    found: Receiver<Found>,
    sender: Sender<Found>,
    /// The object of the last MSAA focus, whose value changes the limit
    /// always keeps, as NVDA keeps its focus's
    /// (`nvda/source/IAccessibleHandler/__init__.py` 1027 to 1035).
    focused: Option<BarObject>,
    /// The last job number given.
    jobs: u64,
    /// The record's count of uses, for the least recently used.
    uses: u64,
}

impl ValueChangeKnowledge {
    /// An empty record.
    pub(crate) fn new() -> Self {
        let (sender, found) = channel();
        Self {
            apps: HashMap::new(),
            found,
            sender,
            focused: None,
            jobs: 0,
            uses: 0,
        }
    }

    /// What a job's answers are sent through.
    pub(crate) fn sender(&self) -> Sender<Found> {
        self.sender.clone()
    }

    /// The object with the MSAA focus now.
    pub(crate) fn focus_moved(&mut self, object: BarObject) {
        self.focused = Some(object);
    }

    /// Whether `object` has the MSAA focus.
    pub(crate) fn is_focused(&self, object: BarObject) -> bool {
        self.focused == Some(object)
    }

    /// Takes in what the reading threads have found.
    pub(crate) fn drain(&mut self) {
        while let Ok(found) = self.found.try_recv() {
            self.take_in(&found);
        }
    }

    fn take_in(&mut self, found: &Found) {
        match *found {
            Found::Window { pid, job, probe } => {
                let Some(app) = self.apps.get_mut(&pid) else {
                    return;
                };
                app.clear_asking(probe.hwnd, job);
                app.windows.apply(&VerdictChange::Probed(probe));
                if app.windows.len() > WINDOWS_KEPT {
                    app.windows.evict_oldest();
                }
            }
            Found::NotAsked { pid, hwnd, job } => {
                if let Some(app) = self.apps.get_mut(&pid) {
                    app.clear_asking(hwnd, job);
                }
            }
            Found::Object {
                pid,
                object,
                owner,
                verdict,
            } => {
                if let Some(app) = self.apps.get_mut(&pid) {
                    app.objects.remember(object, owner, verdict);
                }
            }
        }
    }

    /// Judges one value change, of a class NVDA does not list as a bar, by
    /// its window's answer and then its object's.
    pub(crate) fn judge(&mut self, event: &ValueChange) -> Judgement {
        let hwnd = event.object.window;
        self.uses = self.uses.saturating_add(1);
        let used = self.uses;
        if !self.apps.contains_key(&event.pid) && self.apps.len() >= MAX_APPS {
            let oldest = self
                .apps
                .iter()
                .min_by_key(|(_, app)| app.used)
                .map(|(pid, _)| *pid);
            if let Some(oldest) = oldest {
                self.apps.remove(&oldest);
            }
        }
        let app = self.apps.entry(event.pid).or_default();
        app.used = used;
        let answer = app
            .windows
            .kept_at(hwnd, event.owner, event.at_us)
            .or_else(|| {
                app.asking
                    .contains_key(&hwnd)
                    .then(|| app.windows.newest(hwnd, event.owner))
                    .flatten()
            });
        match answer {
            Some(Probed::Uia) => Judgement::Dropped,
            Some(Probed::Excluded | Probed::NotUia(_) | Probed::TimedOut(_)) => {
                match classic_value_change(false, true, app.objects.get(event.object, event.owner))
                {
                    Some(read) => Judgement::Read(read),
                    None => Judgement::Dropped,
                }
            }
            None if app.asking.contains_key(&hwnd) => Judgement::Dropped,
            None => {
                self.jobs = self.jobs.saturating_add(1);
                app.asking.insert(hwnd, self.jobs);
                Judgement::Ask(self.jobs)
            }
        }
    }
}

impl AppKnowledge {
    /// `hwnd` is asked no more by job `job`: a stale job's end clears no
    /// newer job's asking.
    fn clear_asking(&mut self, hwnd: isize, job: u64) {
        if self.asking.get(&hwnd) == Some(&job) {
            self.asking.remove(&hwnd);
        }
    }
}

/// The kept form of `answer`, given at the probe's end `now_us`.
pub(crate) fn probed_of(answer: WindowAnswer, now_us: u64) -> Probed {
    let until = now_us.saturating_add(NEGATIVE_VERDICT_LIFETIME_US);
    match answer {
        WindowAnswer::Uia => Probed::Uia,
        WindowAnswer::Excluded => Probed::Excluded,
        WindowAnswer::NotUia => Probed::NotUia(until),
        WindowAnswer::TimedOut => Probed::TimedOut(until),
    }
}

#[cfg(test)]
mod tests {
    use verbatim_listener_logic::focus::BarVerdict::{Bar, NotABar};

    use super::*;

    const WINDOW: isize = 0x10;
    const OWNER: u32 = 7;
    const APP: Pid = Pid(1);

    fn object(id: i32) -> BarObject {
        BarObject {
            window: WINDOW,
            object: id,
            child: 0,
        }
    }

    fn change(id: i32, at_us: u64) -> ValueChange {
        ValueChange {
            pid: APP,
            object: object(id),
            owner: OWNER,
            at_us,
        }
    }

    /// Takes in `answer` for job 1, probed from `from_us`, its probe ending
    /// at `from_us`.
    fn answered(knowledge: &mut ValueChangeKnowledge, job: u64, from_us: u64, probed: Probed) {
        knowledge.take_in(&Found::Window {
            pid: APP,
            job,
            probe: Probe {
                hwnd: WINDOW,
                owner: OWNER,
                from_us,
                probed,
            },
        });
    }

    /// A record whose window has `probed` at time 0, from a first ask.
    fn knowing(probed: Probed) -> ValueChangeKnowledge {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
        answered(&mut knowledge, 1, 0, probed);
        knowledge
    }

    /// The cell "`Uia`, lasting": dropped with no read.
    #[test]
    fn a_window_with_a_provider_drops_its_value_changes() {
        let mut knowledge = knowing(Probed::Uia);
        assert_eq!(knowledge.judge(&change(1, 10_000_000)), Judgement::Dropped);
    }

    /// The cell "`Excluded`": lasting, and goes on to the object's memo.
    #[test]
    fn an_excluded_window_goes_on_to_the_objects_memo() {
        let mut knowledge = knowing(Probed::Excluded);
        assert_eq!(
            knowledge.judge(&change(1, 10_000_000)),
            Judgement::Read(ClassicBarRead::Unknown)
        );
    }

    /// The cell "`NotUia` within its 500 ms": goes on to the object's memo.
    #[test]
    fn a_kept_no_goes_on_to_the_objects_memo() {
        let mut knowledge = knowing(Probed::NotUia(500));
        assert_eq!(
            knowledge.judge(&change(1, 499)),
            Judgement::Read(ClassicBarRead::Unknown)
        );
    }

    /// The cell "`TimedOut` within its 500 ms": as a "no".
    #[test]
    fn a_kept_timeout_goes_on_to_the_objects_memo() {
        let mut knowledge = knowing(Probed::TimedOut(500));
        assert_eq!(
            knowledge.judge(&change(1, 499)),
            Judgement::Read(ClassicBarRead::Unknown)
        );
    }

    /// The cell "none, not in `asking`": one job asks and carries it.
    #[test]
    fn a_window_with_no_answer_is_asked_once() {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
    }

    /// The cell "none, in `asking`": dropped, Dickson's decision of
    /// 2026-11-09.
    #[test]
    fn a_change_while_a_windows_first_check_runs_is_dropped() {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
        assert_eq!(knowledge.judge(&change(2, 1)), Judgement::Dropped);
    }

    /// The cell "a kept no past its 500 ms, not in `asking`": asked again.
    #[test]
    fn a_kept_no_past_its_keep_is_asked_again() {
        let mut knowledge = knowing(Probed::NotUia(500));
        assert_eq!(knowledge.judge(&change(1, 500)), Judgement::Ask(2));
    }

    /// The cell "past its 500 ms, in `asking`": its old answer is used.
    #[test]
    fn a_windows_old_answer_is_used_while_it_is_asked_again() {
        let mut knowledge = knowing(Probed::NotUia(500));
        assert_eq!(knowledge.judge(&change(1, 500)), Judgement::Ask(2));
        assert_eq!(
            knowledge.judge(&change(2, 501)),
            Judgement::Read(ClassicBarRead::Unknown)
        );
    }

    /// The memo's cell "Not a bar": dropped.
    #[test]
    fn a_known_non_bar_is_dropped() {
        let mut knowledge = knowing(Probed::Excluded);
        knowledge.take_in(&Found::Object {
            pid: APP,
            object: object(1),
            owner: OWNER,
            verdict: NotABar,
        });
        assert_eq!(knowledge.judge(&change(1, 1)), Judgement::Dropped);
    }

    /// The memo's cell "A bar": a read of it.
    #[test]
    fn a_known_bar_is_read() {
        let mut knowledge = knowing(Probed::Excluded);
        knowledge.take_in(&Found::Object {
            pid: APP,
            object: object(1),
            owner: OWNER,
            verdict: Bar,
        });
        assert_eq!(
            knowledge.judge(&change(1, 1)),
            Judgement::Read(ClassicBarRead::KnownBar)
        );
    }

    /// The memo's cell "Unknown": its role, then the bar.
    #[test]
    fn an_unknown_object_is_read_by_its_role() {
        let mut knowledge = knowing(Probed::Excluded);
        assert_eq!(
            knowledge.judge(&change(1, 1)),
            Judgement::Read(ClassicBarRead::Unknown)
        );
    }

    /// A reused window's handle has no answer: the window is asked again,
    /// and the object's memo is not used.
    #[test]
    fn a_reused_handle_has_no_answer() {
        let mut knowledge = knowing(Probed::Excluded);
        knowledge.take_in(&Found::Object {
            pid: APP,
            object: object(1),
            owner: OWNER,
            verdict: NotABar,
        });
        let reused = ValueChange {
            owner: 8,
            ..change(1, 1)
        };
        assert_eq!(knowledge.judge(&reused), Judgement::Ask(2));
    }

    /// A job's end with no answer, on any path that drops it, clears
    /// `asking` and records nothing.
    #[test]
    fn a_job_ending_unanswered_clears_asking_and_records_nothing() {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
        let sender = knowledge.sender();
        sender
            .send(Found::NotAsked {
                pid: APP,
                hwnd: WINDOW,
                job: 1,
            })
            .expect("the record holds the receiver");
        knowledge.drain();
        assert_eq!(knowledge.judge(&change(1, 1)), Judgement::Ask(2));
    }

    /// A stale job's end cannot clear a newer job's asking.
    #[test]
    fn a_stale_job_does_not_clear_a_newer_ones_asking() {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
        knowledge.take_in(&Found::NotAsked {
            pid: APP,
            hwnd: WINDOW,
            job: 1,
        });
        assert_eq!(knowledge.judge(&change(1, 1)), Judgement::Ask(2));
        knowledge.take_in(&Found::NotAsked {
            pid: APP,
            hwnd: WINDOW,
            job: 1,
        });
        assert_eq!(knowledge.judge(&change(2, 2)), Judgement::Dropped);
    }

    /// The least recently used application is forgotten past the cap, and
    /// a used one stays.
    #[test]
    fn the_applications_are_kept_to_the_cap_least_recently_used_first() {
        let mut knowledge = ValueChangeKnowledge::new();
        let of = |pid: u32, at_us: u64| ValueChange {
            pid: Pid(pid),
            ..change(1, at_us)
        };
        for pid in 0..u32::try_from(MAX_APPS).expect("small") {
            knowledge.judge(&of(pid, 0));
        }
        assert_eq!(knowledge.apps.len(), MAX_APPS);
        knowledge.judge(&of(0, 1));
        knowledge.judge(&of(1000, 2));
        assert_eq!(knowledge.apps.len(), MAX_APPS);
        assert!(knowledge.apps.contains_key(&Pid(0)), "the one just used");
        assert!(!knowledge.apps.contains_key(&Pid(1)), "the oldest");
    }

    /// A window's answers are kept to the cap, the oldest forgotten.
    #[test]
    fn the_windows_are_kept_to_the_cap() {
        let mut knowledge = ValueChangeKnowledge::new();
        assert_eq!(knowledge.judge(&change(1, 0)), Judgement::Ask(1));
        let hwnds = isize::try_from(WINDOWS_KEPT).expect("small") + 1;
        for (index, hwnd) in (0..=hwnds).enumerate() {
            let from_us = u64::try_from(index).expect("small");
            knowledge.take_in(&Found::Window {
                pid: APP,
                job: 0,
                probe: Probe {
                    hwnd: 0x1000 + hwnd,
                    owner: OWNER,
                    from_us,
                    probed: Probed::Uia,
                },
            });
        }
        let app = knowledge.apps.get(&APP).expect("an application");
        assert_eq!(app.windows.len(), WINDOWS_KEPT);
        assert_eq!(app.windows.newest(0x1000, OWNER), None, "the oldest");
        assert_eq!(app.windows.newest(0x1000 + hwnds, OWNER), Some(Probed::Uia));
    }

    /// An object's answers are kept to the cap, the oldest forgotten.
    #[test]
    fn the_objects_are_kept_to_the_cap() {
        let mut memo = BarMemo::default();
        for id in 0..=i32::try_from(BAR_VERDICTS_KEPT).expect("small") {
            memo.remember(object(id), OWNER, NotABar);
        }
        assert_eq!(memo.get(object(0), OWNER), None, "the oldest went");
        assert_eq!(memo.get(object(1), OWNER), Some(NotABar));
        assert_eq!(memo.answers.len(), BAR_VERDICTS_KEPT);
    }

    /// A window's answer learned for an application forgotten meanwhile is
    /// ignored, and no record is made of it.
    #[test]
    fn an_answer_for_a_forgotten_application_is_ignored() {
        let mut knowledge = ValueChangeKnowledge::new();
        knowledge.take_in(&Found::Object {
            pid: Pid(9),
            object: object(1),
            owner: OWNER,
            verdict: Bar,
        });
        assert!(knowledge.apps.is_empty());
    }

    /// A probe's keep ends 500 ms after its end.
    #[test]
    fn a_probes_keep_ends_half_a_second_after_it() {
        assert_eq!(probed_of(WindowAnswer::Uia, 10), Probed::Uia);
        assert_eq!(probed_of(WindowAnswer::Excluded, 10), Probed::Excluded);
        assert_eq!(probed_of(WindowAnswer::NotUia, 10), Probed::NotUia(500_010));
        assert_eq!(
            probed_of(WindowAnswer::TimedOut, 10),
            Probed::TimedOut(500_010)
        );
    }

    /// The object with the MSAA focus is the last one told.
    #[test]
    fn the_last_msaa_focus_is_the_focused_object() {
        let mut knowledge = ValueChangeKnowledge::new();
        knowledge.focus_moved(object(1));
        knowledge.focus_moved(object(2));
        assert!(knowledge.is_focused(object(2)));
        assert!(!knowledge.is_focused(object(1)));
    }
}
