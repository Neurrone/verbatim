//! The calls a UI Automation event's check makes at delivery, as one pure
//! step function (design `docs/design/readers-tell-core-asks-build1.md`,
//! Table 11 and "Focus and foreground"): the listener's check thread asks
//! [`Check::next`] for the one call to make, makes it, records its answer,
//! and asks again until there is none, then sends the tell with what
//! [`Check::finish`] gathered. The check judges nothing: Core judges every
//! answer it carries.
//!
//! The calls are those NVDA's handler for the event's kind makes at
//! delivery, in its order:
//!
//! - a focus or a menu opening, NVDA's focus handler (`UIAHandler/__init__.py`
//!   874 to 960): the walk to the nearest window (`getNearestWindowHandle`,
//!   1383 onward, for every sender, as NVDA has no cached-window shortcut);
//!   the probe of that window (`isUIAWindow`, 1368 to 1381 and 1531 to
//!   1540) when its class is on neither of NVDA's lists and this thread's
//!   record holds no answer begun within the last half second; for each
//!   kept element with the event's runtime id, its keyboard focus (902 to
//!   923); for a `NetUITWMenuItem` menu item with no name, its previous
//!   sibling, which NVDA's object reads as it chooses its class
//!   (`NVDAObjects/UIA/__init__.py` 1227 to 1231); and the element's own
//!   keyboard focus (`shouldAllowUIAFocusEvent`, 1632 to 1636), which NVDA
//!   does not read for the placeholder menu item its class makes of one
//!   with no previous sibling (2905 to 2908); and for the task switcher's
//!   item, a `ListViewItem` list item, its raw-view parent's automation
//!   id, which NVDA's File Explorer module reads as it chooses the item's
//!   class (`appModules/explorer.py` 303 to 311), one call made for that
//!   class alone. A walk that finds no window,
//!   or a window the class lists, the probe or the probe record find read
//!   through MSAA, ends the check, as NVDA's native-element test then
//!   returns false and its handler returns (887 to 899);
//! - an element selected, a window opened and a property change, NVDA's
//!   automation-event and property-change handlers: the walk and the probe
//!   of its native-element test (819 to 824);
//! - a notification, NVDA's notification handler: the walk alone, a walk
//!   that fails telling no window, as it finds none (1063 onward,
//!   1114 to 1119).
//!
//! The probe record keeps each probe's answer with when it began, so a
//! window the record finds read through MSAA ends the check as the probe
//! would.

use std::collections::BTreeMap;

use verbatim_model::WindowHandle;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::Probe;
use verbatim_model::tell::{Checked, ElementToken, Read};
use verbatim_reader_rules::class::ClassVerdict;

/// How long an answer in a check thread's probe record stands, in
/// microseconds: NVDA keeps its `isUIAWindow` answer for half a second
/// (`UIAHandler/__init__.py` 1373).
pub const PROBE_RECORD_US: u64 = 500_000;

/// The handler whose calls an event kind's check makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    /// NVDA's focus handler: a focus, or a menu opening, which NVDA maps
    /// to a focus on the menu.
    Focus,
    /// The native-element test of NVDA's automation-event and
    /// property-change handlers: an element selected, a window opened, a
    /// property change.
    Native,
    /// NVDA's notification handler.
    Notification,
    /// A handler that makes no call: the event passes through in its place.
    None,
}

impl Handler {
    /// The handler of `kind`.
    #[must_use]
    pub fn of(kind: EventKind) -> Self {
        match kind {
            EventKind::UiaFocus | EventKind::UiaMenuOpened => Self::Focus,
            EventKind::UiaElementSelected
            | EventKind::UiaWindowOpened
            | EventKind::UiaPropertyChanged => Self::Native,
            EventKind::UiaNotification => Self::Notification,
            _ => Self::None,
        }
    }
}

/// The one call [`Check::next`] asks for, whose answer the thread records
/// in the [`Check`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// The walk to the element's nearest window: a call into the
    /// application ([`Check::walked`]).
    NearestWindow,
    /// The class verdict of the nearest window by NVDA's lists: local
    /// calls ([`Check::classed`]).
    Classes(isize),
    /// The probe of the nearest window: a window message to the
    /// application ([`Check::probed`]).
    Probe(isize),
    /// Whether a kept element with the event's runtime id still has the
    /// keyboard focus: a call into the application
    /// ([`Check::kept_focus`]).
    KeptFocus(ElementToken),
    /// Whether the element has a previous sibling: a call into the
    /// application ([`Check::previous_sibling`]).
    PreviousSibling,
    /// The automation id of the element's raw-view parent: a call into
    /// the application ([`Check::parent_automation_id`]).
    ParentAutomationId,
    /// Whether the element has the keyboard focus: a call into the
    /// application ([`Check::keyboard_focus`]).
    KeyboardFocus,
}

/// The UI Automation class of the task switcher's item, whose parent's
/// automation id the check reads (`docs/parity/windows-and-processes.md`,
/// "The Alt+Tab switcher's pane").
pub const SWITCHER_ITEM_CLASS: &str = "ListViewItem";

/// What the walk to the nearest window found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Walked {
    /// The nearest window.
    Found(isize),
    /// The walk found no window.
    NoWindow,
    /// The walk failed.
    Failed,
    /// The event's handler makes no walk.
    NotWalked,
}

/// A check thread's record of the probes it made: when each window's last
/// probe began and whether a provider answered, as NVDA's
/// `UIAWindowHandleCache` keeps its answers (`UIAHandler/__init__.py` 1368
/// to 1381). A probe begun within [`PROBE_RECORD_US`] stands, and the
/// window is not probed again.
#[derive(Debug, Default)]
pub struct ProbeRecord {
    probes: BTreeMap<isize, (u64, bool)>,
}

impl ProbeRecord {
    /// Whether a provider answered the probe of `window` the record holds
    /// begun within the last [`PROBE_RECORD_US`] at `now_us`; `None` when
    /// no such probe stands.
    #[must_use]
    pub fn standing(&self, window: isize, now_us: u64) -> Option<bool> {
        self.probes
            .get(&window)
            .filter(|&&(began, _)| now_us.saturating_sub(began) <= PROBE_RECORD_US)
            .map(|&(_, provider)| provider)
    }

    /// Records a probe of `window` begun at `began_us`, answered
    /// `provider`, letting go of every probe that no longer stands.
    fn record(&mut self, window: isize, began_us: u64, provider: bool) {
        self.probes
            .retain(|_, &mut (began, _)| began_us.saturating_sub(began) <= PROBE_RECORD_US);
        self.probes.insert(window, (began_us, provider));
    }
}

/// What the check knows of the nearest window's backend, decided once its
/// class verdict is known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probing {
    /// Read through UI Automation, by NVDA's lists or the record, with no
    /// probe made.
    Uia,
    /// Read through MSAA, by NVDA's lists or the record, with no probe
    /// made: the check ends.
    Msaa,
    /// The probe is to be made.
    Due,
    /// Made, with its answer: a provider reads the window through UI
    /// Automation, none through MSAA, which ends the check.
    Made(Probe),
}

/// One event's check: the calls its handler makes, and their answers as
/// they come.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    handler: Handler,
    /// Whether the element is a `NetUITWMenuItem` menu item with no name by
    /// its event's cache, whose previous sibling NVDA's object reads.
    placeholder_candidate: bool,
    /// Whether the element is a list item of the task switcher's class,
    /// whose parent's automation id is read.
    switcher_item: bool,
    /// The kept elements with the event's runtime id, whose keyboard
    /// focus is read.
    kept: Vec<ElementToken>,
    walked: Option<Walked>,
    probing: Option<Probing>,
    kept_answers: Vec<(ElementToken, Read<bool>)>,
    previous_sibling: Option<Read<bool>>,
    parent_automation_id: Option<Read<Option<String>>>,
    keyboard_focus: Option<Read<bool>>,
}

impl Check {
    /// The check of an event of `kind`. `placeholder_candidate` is whether
    /// its element's cache names a `NetUITWMenuItem` menu item with no
    /// name, and `kept` the elements the listener keeps with its runtime id.
    #[must_use]
    pub fn new(kind: EventKind, placeholder_candidate: bool, kept: Vec<ElementToken>) -> Self {
        let handler = Handler::of(kind);
        Self {
            handler,
            placeholder_candidate: placeholder_candidate && handler == Handler::Focus,
            kept: if handler == Handler::Focus {
                kept
            } else {
                Vec::new()
            },
            walked: (handler == Handler::None).then_some(Walked::NotWalked),
            probing: None,
            kept_answers: Vec::new(),
            previous_sibling: None,
            switcher_item: false,
            parent_automation_id: None,
            keyboard_focus: None,
        }
    }

    /// The check reads the parent's automation id when `switcher_item`:
    /// its element's cache names a list item of [`SWITCHER_ITEM_CLASS`].
    #[must_use]
    pub fn reading_parent(mut self, switcher_item: bool) -> Self {
        self.switcher_item = switcher_item && self.handler == Handler::Focus;
        self
    }

    /// The next call to make, `None` when the check is done. `record` is
    /// the thread's probe record, read at `now_us` when the probe is
    /// decided.
    pub fn next(&mut self, record: &ProbeRecord, now_us: u64) -> Option<Call> {
        let window = match self.walked {
            None => return Some(Call::NearestWindow),
            Some(Walked::Found(window)) => window,
            Some(Walked::NoWindow | Walked::Failed | Walked::NotWalked) => return None,
        };
        if self.handler == Handler::Notification {
            return None;
        }
        if self.probing == Some(Probing::Due) {
            match record.standing(window, now_us) {
                Some(true) => self.probing = Some(Probing::Uia),
                Some(false) => self.probing = Some(Probing::Msaa),
                None => return Some(Call::Probe(window)),
            }
        }
        let native = match self.probing {
            None => return Some(Call::Classes(window)),
            Some(Probing::Uia) => true,
            Some(Probing::Made(probe)) => probe.provider,
            Some(Probing::Msaa | Probing::Due) => false,
        };
        // A window read through MSAA fails NVDA's native-element test, and
        // its handler returns before any other call.
        if self.handler != Handler::Focus || !native {
            return None;
        }
        if let Some(&token) = self.kept.get(self.kept_answers.len()) {
            return Some(Call::KeptFocus(token));
        }
        if self.placeholder_candidate {
            match self.previous_sibling {
                None => return Some(Call::PreviousSibling),
                // No previous sibling, or none could be read: NVDA's
                // placeholder, which allows its focus with no read.
                Some(Read::Answered(false) | Read::Failed) => return None,
                Some(Read::Answered(true)) => {}
            }
        }
        if self.switcher_item && self.parent_automation_id.is_none() {
            return Some(Call::ParentAutomationId);
        }
        self.keyboard_focus.is_none().then_some(Call::KeyboardFocus)
    }

    /// Records the walk's answer: the nearest window, none, or a failure.
    pub fn walked(&mut self, nearest: Read<Option<isize>>) {
        self.walked = Some(match nearest {
            Read::Answered(Some(window)) if window != 0 => Walked::Found(window),
            Read::Answered(_) => Walked::NoWindow,
            Read::Failed => Walked::Failed,
        });
    }

    /// Records the nearest window's class verdict: only a window on
    /// neither list is probed.
    pub fn classed(&mut self, verdict: ClassVerdict) {
        self.probing = Some(match verdict {
            ClassVerdict::Unknown => Probing::Due,
            ClassVerdict::Uia => Probing::Uia,
            ClassVerdict::NonUia => Probing::Msaa,
        });
    }

    /// Records the probe's answer, in the thread's `record` too.
    pub fn probed(&mut self, probe: Probe, record: &mut ProbeRecord) {
        if let Ok(window) = isize::try_from(probe.window.0.cast_signed()) {
            record.record(window, probe.began_us, probe.provider);
        }
        self.probing = Some(Probing::Made(probe));
    }

    /// Records whether the kept element `token` still has the keyboard
    /// focus.
    pub fn kept_focus(&mut self, token: ElementToken, focused: Read<bool>) {
        self.kept_answers.push((token, focused));
    }

    /// Records whether the element has a previous sibling.
    pub fn previous_sibling(&mut self, has: Read<bool>) {
        self.previous_sibling = Some(has);
    }

    /// Records the automation id of the element's parent, `None` for a
    /// parent with none.
    pub fn parent_automation_id(&mut self, id: Read<Option<String>>) {
        self.parent_automation_id = Some(id);
    }

    /// Records whether the element has the keyboard focus.
    pub fn keyboard_focus(&mut self, focused: Read<bool>) {
        self.keyboard_focus = Some(focused);
    }

    /// What the walk found, what the tell carries of the check, and the
    /// parent's automation id it read, which the tell carries among its
    /// cached properties: `None` when it read none, an absent value when
    /// the parent has none or the read failed.
    #[must_use]
    pub fn finish(self) -> (Walked, Checked, Option<PropertyAnswer>) {
        let walked = match (self.handler, self.walked) {
            // A notification whose walk failed tells no window, as NVDA's
            // handler finds none.
            (Handler::Notification, Some(Walked::Failed)) => Walked::NoWindow,
            (_, walked) => walked.unwrap_or(Walked::NotWalked),
        };
        let probe = match self.probing {
            Some(Probing::Made(probe)) => Some(probe),
            Some(Probing::Uia | Probing::Msaa | Probing::Due) | None => None,
        };
        let parent = self.parent_automation_id.map(|read| PropertyAnswer {
            id: PropertyId::ParentAutomationId,
            value: match read {
                Read::Answered(Some(id)) if !id.is_empty() => Some(PropertyValue::Text(id)),
                Read::Answered(_) | Read::Failed => None,
            },
        });
        (
            walked,
            Checked {
                probe,
                keyboard_focus: self.keyboard_focus,
                kept: self.kept_answers,
                previous_sibling: self.previous_sibling,
            },
            parent,
        )
    }
}

/// `window` as the model names a window.
#[must_use]
pub fn window_handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `check` to its end, answering each call from `answer`, and
    /// returns the calls made in order with what it finished with.
    fn run(
        mut check: Check,
        record: &mut ProbeRecord,
        now_us: u64,
        mut answer: impl FnMut(Call) -> Answer,
    ) -> (Vec<Call>, Walked, Checked, Option<PropertyAnswer>) {
        let mut calls = Vec::new();
        while let Some(call) = check.next(record, now_us) {
            calls.push(call);
            match (call, answer(call)) {
                (Call::NearestWindow, Answer::Nearest(nearest)) => check.walked(nearest),
                (Call::Classes(_), Answer::Class(verdict)) => check.classed(verdict),
                (Call::Probe(window), Answer::Provider(provider)) => check.probed(
                    Probe {
                        window: window_handle(window),
                        thread: 7,
                        provider,
                        began_us: now_us,
                    },
                    record,
                ),
                (Call::KeptFocus(token), Answer::Focus(focused)) => {
                    check.kept_focus(token, focused);
                }
                (Call::PreviousSibling, Answer::Focus(has)) => check.previous_sibling(has),
                (Call::KeyboardFocus, Answer::Focus(focused)) => check.keyboard_focus(focused),
                (Call::ParentAutomationId, Answer::Parent(id)) => check.parent_automation_id(id),
                (call, answer) => panic!("{call:?} answered {answer:?}"),
            }
        }
        let (walked, checked, parent) = check.finish();
        (calls, walked, checked, parent)
    }

    #[derive(Debug)]
    enum Answer {
        Nearest(Read<Option<isize>>),
        Class(ClassVerdict),
        Provider(bool),
        Focus(Read<bool>),
        Parent(Read<Option<String>>),
    }

    /// A focus whose nearest window's class is on neither list: the walk,
    /// the class, the probe, each kept element's keyboard focus, then the
    /// element's own, in NVDA's order.
    #[test]
    fn a_focus_walks_probes_and_reads_the_kept_elements_then_its_own_focus() {
        let check = Check::new(
            EventKind::UiaFocus,
            false,
            vec![ElementToken(3), ElementToken(4)],
        );
        let (calls, walked, checked, _) =
            run(
                check,
                &mut ProbeRecord::default(),
                1_000,
                |call| match call {
                    Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                    Call::Classes(_) => Answer::Class(ClassVerdict::Unknown),
                    Call::Probe(_) => Answer::Provider(true),
                    Call::KeptFocus(ElementToken(3)) => Answer::Focus(Read::Answered(true)),
                    Call::KeptFocus(_) => Answer::Focus(Read::Failed),
                    Call::KeyboardFocus | Call::PreviousSibling => {
                        Answer::Focus(Read::Answered(false))
                    }
                    Call::ParentAutomationId => Answer::Parent(Read::Failed),
                },
            );
        assert_eq!(
            calls,
            [
                Call::NearestWindow,
                Call::Classes(5),
                Call::Probe(5),
                Call::KeptFocus(ElementToken(3)),
                Call::KeptFocus(ElementToken(4)),
                Call::KeyboardFocus,
            ]
        );
        assert_eq!(walked, Walked::Found(5));
        assert_eq!(
            checked,
            Checked {
                probe: Some(Probe {
                    window: WindowHandle(5),
                    thread: 7,
                    provider: true,
                    began_us: 1_000,
                }),
                keyboard_focus: Some(Read::Answered(false)),
                kept: vec![
                    (ElementToken(3), Read::Answered(true)),
                    (ElementToken(4), Read::Failed),
                ],
                previous_sibling: None,
            }
        );
    }

    /// The task switcher's item reads its parent's automation id after its
    /// kept elements and before its own keyboard focus, and its tell
    /// carries it; any other focus makes no such call, and an element
    /// selected never does.
    #[test]
    fn the_switchers_item_reads_its_parents_automation_id_and_only_it() {
        let answer = |call| match call {
            Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
            Call::Classes(_) => Answer::Class(ClassVerdict::Uia),
            Call::KeptFocus(_) | Call::KeyboardFocus | Call::PreviousSibling => {
                Answer::Focus(Read::Answered(true))
            }
            Call::ParentAutomationId => {
                Answer::Parent(Read::Answered(Some("SwitchItemListControl".to_owned())))
            }
            Call::Probe(_) => Answer::Provider(true),
        };
        let check =
            Check::new(EventKind::UiaFocus, false, vec![ElementToken(3)]).reading_parent(true);
        let (calls, _, _, parent) = run(check, &mut ProbeRecord::default(), 0, answer);
        assert_eq!(
            (calls, parent),
            (
                vec![
                    Call::NearestWindow,
                    Call::Classes(5),
                    Call::KeptFocus(ElementToken(3)),
                    Call::ParentAutomationId,
                    Call::KeyboardFocus,
                ],
                Some(PropertyAnswer {
                    id: PropertyId::ParentAutomationId,
                    value: Some(PropertyValue::Text("SwitchItemListControl".to_owned())),
                })
            )
        );
        let check = Check::new(EventKind::UiaFocus, false, Vec::new()).reading_parent(false);
        let (calls, _, _, parent) = run(check, &mut ProbeRecord::default(), 0, answer);
        assert_eq!(
            (calls, parent),
            (
                vec![Call::NearestWindow, Call::Classes(5), Call::KeyboardFocus],
                None
            )
        );
        let check =
            Check::new(EventKind::UiaElementSelected, false, Vec::new()).reading_parent(true);
        let (calls, _, _, parent) = run(check, &mut ProbeRecord::default(), 0, answer);
        assert_eq!(
            (calls, parent),
            (vec![Call::NearestWindow, Call::Classes(5)], None)
        );
    }

    /// A walk that finds no window, or fails, ends a focus's check: NVDA's
    /// native-element test returns false and its handler makes no more
    /// calls.
    #[test]
    fn a_focus_whose_walk_finds_no_window_makes_no_other_call() {
        for (nearest, told) in [
            (Read::Answered(None), Walked::NoWindow),
            (Read::Answered(Some(0)), Walked::NoWindow),
            (Read::Failed, Walked::Failed),
        ] {
            let check = Check::new(EventKind::UiaMenuOpened, true, vec![ElementToken(1)]);
            let (calls, walked, checked, _) = run(check, &mut ProbeRecord::default(), 0, |_| {
                Answer::Nearest(nearest)
            });
            assert_eq!(calls, [Call::NearestWindow]);
            assert_eq!(walked, told);
            assert_eq!(
                checked,
                Checked {
                    probe: None,
                    keyboard_focus: None,
                    kept: Vec::new(),
                    previous_sibling: None,
                }
            );
        }
    }

    /// A window on one of NVDA's lists is not probed: on the UI Automation
    /// list the focus's checks go on, and on the MSAA list the check ends
    /// there, as NVDA's handler returns.
    #[test]
    fn a_window_on_a_list_is_not_probed_and_the_msaa_list_ends_the_check() {
        for (verdict, goes_on) in [(ClassVerdict::Uia, true), (ClassVerdict::NonUia, false)] {
            let check = Check::new(EventKind::UiaFocus, true, vec![ElementToken(2)]);
            let (calls, _, checked, _) =
                run(check, &mut ProbeRecord::default(), 0, |call| match call {
                    Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                    Call::Classes(_) => Answer::Class(verdict),
                    _ => Answer::Focus(Read::Answered(true)),
                });
            let mut expected = vec![Call::NearestWindow, Call::Classes(5)];
            if goes_on {
                expected.extend([
                    Call::KeptFocus(ElementToken(2)),
                    Call::PreviousSibling,
                    Call::KeyboardFocus,
                ]);
            }
            assert_eq!(calls, expected, "{verdict:?}");
            assert_eq!(checked.probe, None);
        }
    }

    /// A probe that finds no provider ends a focus's check before the kept
    /// elements' and its own keyboard focus, and so does the record of such
    /// a probe within half a second.
    #[test]
    fn a_window_the_probe_finds_read_through_msaa_ends_the_check() {
        let mut record = ProbeRecord::default();
        let checked = |record: &mut ProbeRecord, now_us| {
            let check = Check::new(EventKind::UiaFocus, false, vec![ElementToken(4)]);
            run(check, record, now_us, |call| match call {
                Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                Call::Classes(_) => Answer::Class(ClassVerdict::Unknown),
                Call::Probe(_) => Answer::Provider(false),
                _ => Answer::Focus(Read::Answered(true)),
            })
        };
        let (calls, _, told, _) = checked(&mut record, 1_000);
        assert_eq!(
            calls,
            [Call::NearestWindow, Call::Classes(5), Call::Probe(5)]
        );
        assert_eq!(
            told,
            Checked {
                probe: Some(Probe {
                    window: WindowHandle(5),
                    thread: 7,
                    provider: false,
                    began_us: 1_000,
                }),
                keyboard_focus: None,
                kept: Vec::new(),
                previous_sibling: None,
            }
        );
        let (calls, _, told, _) = checked(&mut record, 2_000);
        assert_eq!(calls, [Call::NearestWindow, Call::Classes(5)]);
        assert_eq!(
            told,
            Checked {
                probe: None,
                keyboard_focus: None,
                kept: Vec::new(),
                previous_sibling: None,
            }
        );
    }

    /// The record of a probe that found a provider has the focus's checks
    /// go on with no probe made.
    #[test]
    fn a_recorded_provider_goes_on_with_no_probe() {
        let mut record = ProbeRecord::default();
        record.record(5, 1_000, true);
        let check = Check::new(EventKind::UiaFocus, false, Vec::new());
        let (calls, _, told, _) = run(check, &mut record, 1_200, |call| match call {
            Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
            Call::Classes(_) => Answer::Class(ClassVerdict::Unknown),
            _ => Answer::Focus(Read::Answered(true)),
        });
        assert_eq!(
            calls,
            [Call::NearestWindow, Call::Classes(5), Call::KeyboardFocus]
        );
        assert_eq!(told.probe, None);
        assert_eq!(told.keyboard_focus, Some(Read::Answered(true)));
    }

    /// A probe begun within the last half second stands: the window is
    /// not probed again until it has passed, as NVDA's record keeps it.
    #[test]
    fn a_probe_within_half_a_second_stands_for_the_next_check() {
        let mut record = ProbeRecord::default();
        let probes = |record: &mut ProbeRecord, now_us| {
            let check = Check::new(EventKind::UiaElementSelected, false, Vec::new());
            let (calls, _, checked, _) = run(check, record, now_us, |call| match call {
                Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                Call::Classes(_) => Answer::Class(ClassVerdict::Unknown),
                _ => Answer::Provider(false),
            });
            (
                calls.contains(&Call::Probe(5)),
                checked.probe.map(|probe| probe.began_us),
            )
        };
        assert_eq!(probes(&mut record, 1_000), (true, Some(1_000)));
        assert_eq!(probes(&mut record, 501_000), (false, None));
        assert_eq!(probes(&mut record, 501_001), (true, Some(501_001)));
    }

    /// An element selected, a window opened and a property change make
    /// the native-element test's walk and probe, and no focus read.
    #[test]
    fn a_native_test_walks_and_probes_alone() {
        for kind in [
            EventKind::UiaElementSelected,
            EventKind::UiaWindowOpened,
            EventKind::UiaPropertyChanged,
        ] {
            let check = Check::new(kind, true, vec![ElementToken(2)]);
            let (calls, walked, checked, _) =
                run(check, &mut ProbeRecord::default(), 0, |call| match call {
                    Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                    Call::Classes(_) => Answer::Class(ClassVerdict::Unknown),
                    _ => Answer::Provider(true),
                });
            assert_eq!(
                calls,
                [Call::NearestWindow, Call::Classes(5), Call::Probe(5)],
                "{kind:?}"
            );
            assert_eq!(walked, Walked::Found(5));
            assert_eq!(checked.kept, Vec::new());
            assert_eq!(checked.keyboard_focus, None);
        }
    }

    /// A notification walks alone, and a failed walk tells no window.
    #[test]
    fn a_notification_walks_alone_and_a_failed_walk_finds_no_window() {
        for (nearest, told) in [
            (Read::Answered(Some(5)), Walked::Found(5)),
            (Read::Answered(None), Walked::NoWindow),
            (Read::Failed, Walked::NoWindow),
        ] {
            let check = Check::new(EventKind::UiaNotification, false, Vec::new());
            let (calls, walked, _, _) = run(check, &mut ProbeRecord::default(), 0, |_| {
                Answer::Nearest(nearest)
            });
            assert_eq!(calls, [Call::NearestWindow]);
            assert_eq!(walked, told);
        }
    }

    /// A `NetUITWMenuItem` menu item with no name has its previous sibling
    /// read after the kept elements: with none, or none read, it is NVDA's
    /// placeholder and its keyboard focus is not read; with one, it is.
    #[test]
    fn a_nameless_net_ui_menu_item_reads_its_previous_sibling_first() {
        for (sibling, then_focus) in [
            (Read::Answered(false), false),
            (Read::Failed, false),
            (Read::Answered(true), true),
        ] {
            let check = Check::new(EventKind::UiaFocus, true, vec![ElementToken(8)]);
            let (calls, _, checked, _) =
                run(check, &mut ProbeRecord::default(), 0, |call| match call {
                    Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                    Call::Classes(_) => Answer::Class(ClassVerdict::Uia),
                    Call::PreviousSibling => Answer::Focus(sibling),
                    _ => Answer::Focus(Read::Answered(true)),
                });
            let mut expected = vec![
                Call::NearestWindow,
                Call::Classes(5),
                Call::KeptFocus(ElementToken(8)),
                Call::PreviousSibling,
            ];
            if then_focus {
                expected.push(Call::KeyboardFocus);
            }
            assert_eq!(calls, expected, "{sibling:?}");
            assert_eq!(checked.previous_sibling, Some(sibling));
            assert_eq!(
                checked.keyboard_focus,
                then_focus.then_some(Read::Answered(true))
            );
        }
    }

    /// A kind no UI Automation handler checks passes through with no call.
    #[test]
    fn a_kind_with_no_handler_makes_no_call() {
        let check = Check::new(EventKind::UiaTextChanged, true, vec![ElementToken(1)]);
        let (calls, walked, _, _) = run(check, &mut ProbeRecord::default(), 0, |call| {
            panic!("no call: {call:?}")
        });
        assert_eq!(calls, Vec::new());
        assert_eq!(walked, Walked::NotWalked);
    }
}
