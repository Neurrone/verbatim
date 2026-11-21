//! The calls a UI Automation event's check makes at delivery, as one
//! straight-line function (design
//! `docs/design/readers-tell-core-asks-build1.md`, Table 11 and "Focus and
//! foreground"): [`check`] makes each call through [`CheckCalls`], which
//! the listener's check thread answers on the event's element, and returns
//! what the tell carries. The check judges nothing: Core judges every
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
//! - an element selected, a window opened, a property change and a
//!   progress bar's range value change, NVDA's automation-event and
//!   property-change handlers: the walk and the probe
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
    /// property change, a progress bar's range value change.
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
            | EventKind::UiaPropertyChanged
            | EventKind::UiaProgress => Self::Native,
            EventKind::UiaNotification => Self::Notification,
            _ => Self::None,
        }
    }
}

/// One call a check makes, named for a test's observer, which sees each
/// before it is made ([`CheckCalls::calling`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    /// The walk to the element's nearest window: a call into the
    /// application ([`CheckCalls::nearest_window`]).
    NearestWindow,
    /// The classes of the nearest window, which NVDA's lists and the
    /// checks after its probe judge: local calls ([`CheckCalls::classes`]).
    Classes(isize),
    /// The probe of the nearest window: a window message to the
    /// application ([`CheckCalls::probe`]).
    Probe(isize),
    /// Whether a kept element with the event's runtime id still has the
    /// keyboard focus: a call into the application
    /// ([`CheckCalls::kept_focus`]).
    KeptFocus(ElementToken),
    /// The acceptance facts of the nearest window, the foreground's part
    /// included: calls to Windows, made where NVDA judges acceptance
    /// (`shouldAcceptEvent`, `UIAHandler/__init__.py` 925), before the
    /// object's own calls ([`CheckCalls::acceptance`]).
    Acceptance(isize),
    /// Whether the element has a previous sibling: a call into the
    /// application ([`CheckCalls::previous_sibling`]).
    PreviousSibling,
    /// The automation id of the element's raw-view parent: a call into
    /// the application ([`CheckCalls::parent_automation_id`]).
    ParentAutomationId,
    /// Whether the element has the keyboard focus: a call into the
    /// application ([`CheckCalls::keyboard_focus`]).
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

/// What an event's check is made of, from its kind and its element's
/// cache, with no call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckFacts {
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
}

impl CheckFacts {
    /// The check of an event of `kind`. `placeholder_candidate` is whether
    /// its element's cache names a `NetUITWMenuItem` menu item with no
    /// name, and `kept` the elements the listener keeps with its runtime id.
    #[must_use]
    pub fn new(kind: EventKind, placeholder_candidate: bool, kept: Vec<ElementToken>) -> Self {
        let handler = Handler::of(kind);
        Self {
            handler,
            placeholder_candidate: placeholder_candidate && handler == Handler::Focus,
            switcher_item: false,
            kept: if handler == Handler::Focus {
                kept
            } else {
                Vec::new()
            },
        }
    }

    /// The check reads the parent's automation id when `switcher_item`:
    /// its element's cache names a list item of [`SWITCHER_ITEM_CLASS`].
    #[must_use]
    pub fn reading_parent(mut self, switcher_item: bool) -> Self {
        self.switcher_item = switcher_item && self.handler == Handler::Focus;
        self
    }
}

/// The calls a check makes, each answered by the listener's check thread
/// on the event's element (or a test's recorded answers). A read that
/// fails answers [`Read::Failed`].
pub trait CheckCalls {
    /// The check is about to make `call`, for a test's observer.
    fn calling(&mut self, _call: Call) {}
    /// The walk to the element's nearest window.
    fn nearest_window(&mut self) -> Read<Option<isize>>;
    /// The classes of `window`.
    fn classes(&mut self, window: isize) -> WindowClasses;
    /// The raw probe of `window`, made now: whether a provider answered,
    /// with the window's thread and when the probe began.
    fn probe(&mut self, window: isize) -> Probe;
    /// The acceptance facts of `window`, read now and kept by the caller
    /// for the tell.
    fn acceptance(&mut self, window: isize);
    /// Whether the kept element `token` still has the keyboard focus.
    fn kept_focus(&mut self, token: ElementToken) -> Read<bool>;
    /// Whether the element has a previous sibling.
    fn previous_sibling(&mut self) -> Read<bool>;
    /// The automation id of the element's raw-view parent, `None` for a
    /// parent with none.
    fn parent_automation_id(&mut self) -> Read<Option<String>>;
    /// Whether the element has the keyboard focus.
    fn keyboard_focus(&mut self) -> Read<bool>;
}

/// What a check found: the walk's end, what the tell carries of the
/// check, and the parent's automation id it read, which the tell carries
/// among its cached properties (`None` when it read none, an absent value
/// when the parent has none or the read failed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// What the walk to the nearest window found.
    pub walked: Walked,
    /// What the tell carries of the check.
    pub checked: Checked,
    /// The parent's automation id, when read.
    pub parent: Option<PropertyAnswer>,
}

/// The check of an event made of `facts`: every call NVDA's handler for
/// its kind makes at delivery, in its order, through `calls`. `record` is
/// the thread's probe record, read at `now_us`, which a probe made
/// updates with its checked answer ([`listener_provider`]).
pub fn check(
    facts: &CheckFacts,
    (record, now_us): (&mut ProbeRecord, u64),
    calls: &mut impl CheckCalls,
) -> Found {
    let mut found = Found {
        walked: Walked::NotWalked,
        checked: Checked {
            probe: None,
            keyboard_focus: None,
            kept: Vec::new(),
            previous_sibling: None,
        },
        parent: None,
    };
    if facts.handler == Handler::None {
        return found;
    }
    calls.calling(Call::NearestWindow);
    let window = match calls.nearest_window() {
        Read::Answered(Some(window)) if window != 0 => window,
        Read::Answered(_) => {
            found.walked = Walked::NoWindow;
            return found;
        }
        // A notification whose walk failed tells no window, as NVDA's
        // handler finds none.
        Read::Failed => {
            found.walked = if facts.handler == Handler::Notification {
                Walked::NoWindow
            } else {
                Walked::Failed
            };
            return found;
        }
    };
    found.walked = Walked::Found(window);
    if facts.handler == Handler::Notification {
        return found;
    }
    calls.calling(Call::Classes(window));
    let classes = calls.classes(window);
    if class_verdict(&classes) == ClassVerdict::Unknown && record.standing(window, now_us).is_none()
    {
        calls.calling(Call::Probe(window));
        let raw = calls.probe(window);
        let probe = Probe {
            provider: listener_provider(&classes, raw.provider),
            ..raw
        };
        record.record(window, probe.began_us, probe.provider);
        found.checked.probe = Some(probe);
    }
    // Whatever the verdict found, the check goes on: Core judges the
    // native-element test. The automation-event handler judges acceptance
    // last; the focus handler after its duplicate test and before its
    // object's own reads (902 to 953).
    if facts.handler == Handler::Focus {
        for &token in &facts.kept {
            calls.calling(Call::KeptFocus(token));
            let focused = calls.kept_focus(token);
            found.checked.kept.push((token, focused));
        }
    }
    calls.calling(Call::Acceptance(window));
    calls.acceptance(window);
    if facts.handler != Handler::Focus {
        return found;
    }
    if facts.placeholder_candidate {
        calls.calling(Call::PreviousSibling);
        let has = calls.previous_sibling();
        found.checked.previous_sibling = Some(has);
        // No previous sibling, or none could be read: NVDA's placeholder,
        // which allows its focus with no read.
        if has != Read::Answered(true) {
            return found;
        }
    }
    if facts.switcher_item {
        calls.calling(Call::ParentAutomationId);
        found.parent = Some(PropertyAnswer {
            id: PropertyId::ParentAutomationId,
            value: match calls.parent_automation_id() {
                Read::Answered(Some(id)) if !id.is_empty() => Some(PropertyValue::Text(id)),
                Read::Answered(_) | Read::Failed => None,
            },
        });
    }
    calls.calling(Call::KeyboardFocus);
    found.checked.keyboard_focus = Some(calls.keyboard_focus());
    found
}

/// `window` as the model names a window.
#[must_use]
pub fn window_handle(window: isize) -> WindowHandle {
    WindowHandle(i64::try_from(window).unwrap_or(0).cast_unsigned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Answers each call from a closure, recording the calls in order.
    struct Answering<F> {
        answer: F,
        calls: Vec<Call>,
        now_us: u64,
    }

    impl<F: FnMut(Call) -> Answer> Answering<F> {
        fn answer(&mut self, call: Call) -> Answer {
            (self.answer)(call)
        }
    }

    impl<F: FnMut(Call) -> Answer> CheckCalls for Answering<F> {
        fn calling(&mut self, call: Call) {
            self.calls.push(call);
        }
        fn nearest_window(&mut self) -> Read<Option<isize>> {
            match self.answer(Call::NearestWindow) {
                Answer::Nearest(nearest) => nearest,
                answer => panic!("the walk answered {answer:?}"),
            }
        }
        fn classes(&mut self, window: isize) -> WindowClasses {
            match self.answer(Call::Classes(window)) {
                Answer::Class(classes) => classes,
                answer => panic!("the classes answered {answer:?}"),
            }
        }
        fn probe(&mut self, window: isize) -> Probe {
            match self.answer(Call::Probe(window)) {
                Answer::Provider(provider) => Probe {
                    window: window_handle(window),
                    thread: 7,
                    provider,
                    began_us: self.now_us,
                },
                answer => panic!("the probe answered {answer:?}"),
            }
        }
        fn acceptance(&mut self, _window: isize) {}
        fn kept_focus(&mut self, token: ElementToken) -> Read<bool> {
            self.focus(Call::KeptFocus(token))
        }
        fn previous_sibling(&mut self) -> Read<bool> {
            self.focus(Call::PreviousSibling)
        }
        fn parent_automation_id(&mut self) -> Read<Option<String>> {
            match self.answer(Call::ParentAutomationId) {
                Answer::Parent(id) => id,
                answer => panic!("the parent answered {answer:?}"),
            }
        }
        fn keyboard_focus(&mut self) -> Read<bool> {
            self.focus(Call::KeyboardFocus)
        }
    }

    impl<F: FnMut(Call) -> Answer> Answering<F> {
        fn focus(&mut self, call: Call) -> Read<bool> {
            match self.answer(call) {
                Answer::Focus(focused) => focused,
                answer => panic!("{call:?} answered {answer:?}"),
            }
        }
    }

    /// Runs the check of `facts` to its end, answering each call from
    /// `answer`, and returns the calls made in order with what it found.
    fn run(
        facts: &CheckFacts,
        record: &mut ProbeRecord,
        now_us: u64,
        answer: impl FnMut(Call) -> Answer,
    ) -> (Vec<Call>, Walked, Checked, Option<PropertyAnswer>) {
        let mut calls = Answering {
            answer,
            calls: Vec::new(),
            now_us,
        };
        let found = check(facts, (record, now_us), &mut calls);
        (calls.calls, found.walked, found.checked, found.parent)
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
        let check = CheckFacts::new(
            EventKind::UiaFocus,
            false,
            vec![ElementToken(3), ElementToken(4)],
        );
        let (calls, walked, checked, _) =
            run(
                &check,
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
            CheckFacts::new(EventKind::UiaFocus, false, vec![ElementToken(3)]).reading_parent(true);
        let (calls, _, _, parent) = run(&check, &mut ProbeRecord::default(), 0, answer);
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
        let check = CheckFacts::new(EventKind::UiaFocus, false, Vec::new()).reading_parent(false);
        let (calls, _, _, parent) = run(&check, &mut ProbeRecord::default(), 0, answer);
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
            CheckFacts::new(EventKind::UiaElementSelected, false, Vec::new()).reading_parent(true);
        let (calls, _, _, parent) = run(&check, &mut ProbeRecord::default(), 0, answer);
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
            let check = CheckFacts::new(EventKind::UiaMenuOpened, true, vec![ElementToken(1)]);
            let (calls, walked, checked, _) = run(&check, &mut ProbeRecord::default(), 0, |_| {
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
            let check = CheckFacts::new(EventKind::UiaFocus, true, vec![ElementToken(2)]);
            let (calls, _, checked, _) =
                run(&check, &mut ProbeRecord::default(), 0, |call| match call {
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
            let check = CheckFacts::new(EventKind::UiaFocus, false, vec![ElementToken(4)]);
            run(&check, record, now_us, |call| match call {
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
        let check = CheckFacts::new(EventKind::UiaFocus, false, Vec::new());
        let (calls, _, told, _) = run(&check, &mut record, 1_200, |call| match call {
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
            let check = CheckFacts::new(EventKind::UiaFocus, false, Vec::new());
            let (calls, _, checked, _) = run(&check, &mut record, 1_000, |call| match call {
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
            let check = CheckFacts::new(EventKind::UiaElementSelected, false, Vec::new());
            let (calls, _, checked, _) = run(&check, record, now_us, |call| match call {
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
            let check = CheckFacts::new(kind, true, vec![ElementToken(2)]);
            let (calls, walked, checked, _) =
                run(&check, &mut ProbeRecord::default(), 0, |call| match call {
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
            let check = CheckFacts::new(EventKind::UiaNotification, false, Vec::new());
            let (calls, walked, _, _) = run(&check, &mut ProbeRecord::default(), 0, |_| {
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
            let check = CheckFacts::new(EventKind::UiaFocus, true, vec![ElementToken(8)]);
            let (calls, _, checked, _) =
                run(&check, &mut ProbeRecord::default(), 0, |call| match call {
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
        let check = CheckFacts::new(EventKind::UiaTextChanged, true, vec![ElementToken(1)]);
        let (calls, walked, _, _) = run(&check, &mut ProbeRecord::default(), 0, |call| {
            panic!("no call: {call:?}")
        });
        assert_eq!(calls, Vec::new());
        assert_eq!(walked, Walked::NotWalked);
    }
}
