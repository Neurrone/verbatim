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
//!   kept element with the event's runtime id that can be the readers'
//!   last focus, its keyboard focus (902 to 923,
//!   `kept::KeptElements::with_runtime_id`); the window's acceptance
//!   facts, where NVDA judges acceptance (`shouldAcceptEvent`, 925),
//!   before its object is made (932); for a `NetUITWMenuItem` menu item
//!   with no name, its previous
//!   sibling, which NVDA's object reads as it chooses its class
//!   (`NVDAObjects/UIA/__init__.py` 1227 to 1231); and the element's own
//!   keyboard focus (`shouldAllowUIAFocusEvent`, 1632 to 1636), which NVDA
//!   does not read for the placeholder menu item its class makes of one
//!   with no previous sibling (2905 to 2908); and for the task switcher's
//!   item, a `ListViewItem` list item, its raw-view parent's automation
//!   id, which NVDA's File Explorer module reads as it chooses the item's
//!   class (`appModules/explorer.py` 303 to 311), one call made for that
//!   class alone. A walk that finds no window ends the check. A window the
//!   class lists, the probe or the probe record find read through MSAA
//!   does not: Core's backend cache is the one owner of a window's
//!   verdict and judges NVDA's native-element test (887 to 899), so the
//!   check makes the keyboard-focus reads whatever it found and carries
//!   any probe it made;
//! - an element selected, a window opened and a property change, NVDA's
//!   automation-event and property-change handlers: the walk and the probe
//!   of its native-element test (819 to 824), then the window's acceptance
//!   facts, as the handler judges acceptance next (832);
//! - a notification, NVDA's notification handler: the walk alone, a walk
//!   that fails telling no window, as it finds none (1063 onward,
//!   1114 to 1119). NVDA judges no acceptance there; the facts serve
//!   Core's own rules and are read as the tell is placed.
//!
//! A probe's answer is the raw probe's with NVDA's checks after it
//! (`UIAHandler/__init__.py` 1344 to 1361), judged by
//! [`listener_provider`]: a list view's provider counts only from Windows
//! Forms, and a console's with no check, the listener's recorded limit.
//!
//! The probe record keeps each probe's answer with when it began, only to
//! skip repeating a probe call within half a second: a guess that can only
//! cost calls, since Core recorded the same probe from the earlier tell at
//! its arrival, so Core never lacks a verdict the record had.

use std::collections::BTreeMap;

use verbatim_model::WindowHandle;
use verbatim_model::event_kind::EventKind;
use verbatim_model::property::{PropertyAnswer, PropertyId, PropertyValue};
use verbatim_model::request::Probe;
use verbatim_model::tell::{Checked, ElementToken, Read};
use verbatim_reader_rules::class::{ClassVerdict, WindowClasses, class_verdict, listener_provider};

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
    /// The classes of the nearest window, which NVDA's lists and the
    /// checks after its probe judge: local calls ([`Check::classed`]).
    Classes(isize),
    /// The probe of the nearest window: a window message to the
    /// application ([`Check::probed`]).
    Probe(isize),
    /// Whether a kept element with the event's runtime id still has the
    /// keyboard focus: a call into the application
    /// ([`Check::kept_focus`]).
    KeptFocus(ElementToken),
    /// The acceptance facts of the nearest window, the foreground's part
    /// included: calls to Windows, made where NVDA judges acceptance
    /// (`shouldAcceptEvent`, `UIAHandler/__init__.py` 925), before the
    /// object's own calls ([`Check::accepted`]).
    Acceptance(isize),
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
    /// made.
    Msaa,
    /// The probe is to be made.
    Due,
    /// Made, with its answer: a provider reads the window through UI
    /// Automation, none through MSAA.
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
    /// The nearest window's classes, once read: the probe's answer is
    /// judged with them.
    classes: Option<WindowClasses>,
    probing: Option<Probing>,
    kept_answers: Vec<(ElementToken, Read<bool>)>,
    /// Whether the acceptance facts were read.
    accepted: bool,
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
            classes: None,
            probing: None,
            kept_answers: Vec::new(),
            accepted: false,
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
        if self.probing.is_none() {
            return Some(Call::Classes(window));
        }
        // Only the focus handler reads more. Whatever the verdict found, it
        // goes on: Core judges the native-element test. The automation-event
        // handler judges acceptance last.
        if self.handler != Handler::Focus {
            return (!self.accepted).then_some(Call::Acceptance(window));
        }
        if let Some(&token) = self.kept.get(self.kept_answers.len()) {
            return Some(Call::KeptFocus(token));
        }
        // NVDA judges acceptance after its duplicate test and before its
        // object reads the previous sibling and the parent's automation id
        // and the keyboard focus (902 to 953).
        if !self.accepted {
            return Some(Call::Acceptance(window));
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

    /// Records the nearest window's classes: by NVDA's lists' verdict,
    /// only a window on neither list is probed.
    pub fn classed(&mut self, classes: WindowClasses) {
        self.probing = Some(match class_verdict(&classes) {
            ClassVerdict::Unknown => Probing::Due,
            ClassVerdict::Uia => Probing::Uia,
            ClassVerdict::NonUia => Probing::Msaa,
        });
        self.classes = Some(classes);
    }

    /// Records the raw probe, `raw`, whose `provider` is whether the window
    /// answered with a provider: the probe the tell carries, and the
    /// thread's `record` keeps, is the checked answer
    /// ([`listener_provider`]).
    pub fn probed(&mut self, raw: Probe, record: &mut ProbeRecord) {
        let provider = self
            .classes
            .as_ref()
            .is_some_and(|classes| listener_provider(classes, raw.provider));
        let probe = Probe { provider, ..raw };
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

    /// Records that the acceptance facts were read: the thread keeps them
    /// for the tell.
    pub fn accepted(&mut self) {
        self.accepted = true;
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
            if let Call::Acceptance(_) = call {
                check.accepted();
                continue;
            }
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

    /// A window on neither of NVDA's lists, which is probed.
    fn unlisted() -> WindowClasses {
        WindowClasses::new("Notepad", "Notepad")
    }

    /// A window on NVDA's good list.
    fn listed_uia() -> WindowClasses {
        WindowClasses::new(
            "CASCADIA_HOSTING_WINDOW_CLASS",
            "CASCADIA_HOSTING_WINDOW_CLASS",
        )
    }

    /// A window on NVDA's bad list.
    fn listed_msaa() -> WindowClasses {
        WindowClasses::new("Edit", "#32770")
    }

    #[derive(Debug)]
    enum Answer {
        Nearest(Read<Option<isize>>),
        Class(WindowClasses),
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
                    Call::Classes(_) => Answer::Class(unlisted()),
                    Call::Probe(_) => Answer::Provider(true),
                    Call::KeptFocus(ElementToken(3)) => Answer::Focus(Read::Answered(true)),
                    Call::KeptFocus(_) => Answer::Focus(Read::Failed),
                    Call::KeyboardFocus | Call::PreviousSibling => {
                        Answer::Focus(Read::Answered(false))
                    }
                    Call::ParentAutomationId => Answer::Parent(Read::Failed),
                    Call::Acceptance(_) => panic!("the run reads the facts"),
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
                Call::Acceptance(5),
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
            Call::Classes(_) => Answer::Class(listed_uia()),
            Call::KeptFocus(_) | Call::KeyboardFocus | Call::PreviousSibling => {
                Answer::Focus(Read::Answered(true))
            }
            Call::ParentAutomationId => {
                Answer::Parent(Read::Answered(Some("SwitchItemListControl".to_owned())))
            }
            Call::Probe(_) => Answer::Provider(true),
            Call::Acceptance(_) => panic!("the run reads the facts"),
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
                    Call::Acceptance(5),
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
                vec![
                    Call::NearestWindow,
                    Call::Classes(5),
                    Call::Acceptance(5),
                    Call::KeyboardFocus
                ],
                None
            )
        );
        let check =
            Check::new(EventKind::UiaElementSelected, false, Vec::new()).reading_parent(true);
        let (calls, _, _, parent) = run(check, &mut ProbeRecord::default(), 0, answer);
        assert_eq!(
            (calls, parent),
            (
                vec![Call::NearestWindow, Call::Classes(5), Call::Acceptance(5)],
                None
            )
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

    /// A window on one of NVDA's lists is not probed, and the focus's
    /// checks go on whichever list it is on: Core's backend cache judges
    /// the native-element test, so the check makes the keyboard-focus
    /// reads in a window read through MSAA too.
    #[test]
    fn a_window_on_a_list_is_not_probed_and_the_checks_go_on() {
        for classes in [listed_uia(), listed_msaa()] {
            let check = Check::new(EventKind::UiaFocus, true, vec![ElementToken(2)]);
            let (calls, _, checked, _) =
                run(check, &mut ProbeRecord::default(), 0, |call| match call {
                    Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                    Call::Classes(_) => Answer::Class(classes.clone()),
                    _ => Answer::Focus(Read::Answered(true)),
                });
            assert_eq!(
                calls,
                [
                    Call::NearestWindow,
                    Call::Classes(5),
                    Call::KeptFocus(ElementToken(2)),
                    Call::Acceptance(5),
                    Call::PreviousSibling,
                    Call::KeyboardFocus,
                ],
                "{classes:?}"
            );
            assert_eq!(checked.probe, None);
        }
    }

    /// A probe that finds no provider is carried, and the focus's checks go
    /// on to the kept elements' and its own keyboard focus; the record of
    /// that probe within half a second only spares the next check the
    /// probe call, as Core recorded the probe from the earlier tell.
    #[test]
    fn a_window_the_probe_finds_read_through_msaa_still_has_its_keyboard_focus_read() {
        let mut record = ProbeRecord::default();
        let checked = |record: &mut ProbeRecord, now_us| {
            let check = Check::new(EventKind::UiaFocus, false, vec![ElementToken(4)]);
            run(check, record, now_us, |call| match call {
                Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                Call::Classes(_) => Answer::Class(unlisted()),
                Call::Probe(_) => Answer::Provider(false),
                _ => Answer::Focus(Read::Answered(false)),
            })
        };
        let (calls, _, told, _) = checked(&mut record, 1_000);
        assert_eq!(
            calls,
            [
                Call::NearestWindow,
                Call::Classes(5),
                Call::Probe(5),
                Call::KeptFocus(ElementToken(4)),
                Call::Acceptance(5),
                Call::KeyboardFocus,
            ]
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
                keyboard_focus: Some(Read::Answered(false)),
                kept: vec![(ElementToken(4), Read::Answered(false))],
                previous_sibling: None,
            }
        );
        let (calls, _, told, _) = checked(&mut record, 2_000);
        assert_eq!(
            calls,
            [
                Call::NearestWindow,
                Call::Classes(5),
                Call::KeptFocus(ElementToken(4)),
                Call::Acceptance(5),
                Call::KeyboardFocus,
            ]
        );
        assert_eq!(
            told,
            Checked {
                probe: None,
                keyboard_focus: Some(Read::Answered(false)),
                kept: vec![(ElementToken(4), Read::Answered(false))],
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
            Call::Classes(_) => Answer::Class(unlisted()),
            _ => Answer::Focus(Read::Answered(true)),
        });
        assert_eq!(
            calls,
            [
                Call::NearestWindow,
                Call::Classes(5),
                Call::Acceptance(5),
                Call::KeyboardFocus
            ]
        );
        assert_eq!(told.probe, None);
        assert_eq!(told.keyboard_focus, Some(Read::Answered(true)));
    }

    /// The probe a tell carries is the raw probe's answer with NVDA's
    /// checks after it: a list view's provider counts only from Windows
    /// Forms, a console's with no further call, and any other window's as
    /// the probe found it; the record keeps the checked answer.
    #[test]
    fn the_probe_carries_the_checked_answer() {
        for (raw, counted) in [
            ("SysListView32", false),
            ("WindowsForms10.SysListView32.app.0.2bf8098_r6_ad1", true),
            ("ConsoleWindowClass", true),
        ] {
            let mut record = ProbeRecord::default();
            let check = Check::new(EventKind::UiaFocus, false, Vec::new());
            let (calls, _, checked, _) = run(check, &mut record, 1_000, |call| match call {
                Call::NearestWindow => Answer::Nearest(Read::Answered(Some(5))),
                Call::Classes(_) => Answer::Class(WindowClasses::new(raw, "#32770")),
                Call::Probe(_) => Answer::Provider(true),
                _ => Answer::Focus(Read::Answered(true)),
            });
            assert_eq!(
                calls,
                [
                    Call::NearestWindow,
                    Call::Classes(5),
                    Call::Probe(5),
                    Call::Acceptance(5),
                    Call::KeyboardFocus
                ],
                "{raw}"
            );
            assert_eq!(
                checked.probe,
                Some(Probe {
                    window: WindowHandle(5),
                    thread: 7,
                    provider: counted,
                    began_us: 1_000,
                }),
                "{raw}"
            );
            assert_eq!(record.standing(5, 1_000), Some(counted), "{raw}");
        }
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
                Call::Classes(_) => Answer::Class(unlisted()),
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
                    Call::Classes(_) => Answer::Class(unlisted()),
                    _ => Answer::Provider(true),
                });
            assert_eq!(
                calls,
                [
                    Call::NearestWindow,
                    Call::Classes(5),
                    Call::Probe(5),
                    Call::Acceptance(5)
                ],
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
                    Call::Classes(_) => Answer::Class(listed_uia()),
                    Call::PreviousSibling => Answer::Focus(sibling),
                    _ => Answer::Focus(Read::Answered(true)),
                });
            let mut expected = vec![
                Call::NearestWindow,
                Call::Classes(5),
                Call::KeptFocus(ElementToken(8)),
                Call::Acceptance(5),
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
