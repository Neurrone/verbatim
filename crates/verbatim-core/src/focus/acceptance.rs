//! The acceptance test: whether an event's window counts, judged in Core
//! as a pure rule over the facts its tell carries, in the order of
//! `docs/parity/focus-events.md`, "Event acceptance" (ADR 0003, "Acceptance
//! judged in Core"). The first rule that decides ends the test.
//!
//! Not applied: the table application support fills, empty until
//! extensions (M5); and the windowless Chromium rule, which needs a read
//! and has no application Verbatim supports. UI Automation notifications
//! are not tested by window, so their caller does not ask.

use serde::{Deserialize, Serialize};
use verbatim_model::event_kind::EventKind;
use verbatim_model::tell::{AcceptanceFacts, Event, Relation};

/// The classes whose shows are accepted.
const SHOW_CLASSES: [&str; 6] = [
    "Frame Notification Bar",
    "tooltips_class32",
    "mscandui21.candidate",
    "mscandui40.candidate",
    "MSCandUIWindow_Candidate",
    "TTrayAlert",
];

/// Whether `class` is one of the classes whose shows are accepted.
pub(crate) fn show_class(class: &str) -> bool {
    SHOW_CLASSES.contains(&class)
}

/// The parent class of a toast's window.
const TOAST_PARENT: &str = "ToastChildWindowClass";

/// The class prefix of a Windows Runtime core window.
const CORE_WINDOW_PREFIX: &str = "Windows.UI.Core";

/// Why an event was accepted, each one of the test's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Accepted {
    /// It has no window, so nothing can be tested.
    NoWindow,
    /// A value change, with background progress bars reported.
    BackgroundProgress,
    /// A show from one of the six classes.
    ShowClass,
    /// An alert whose window's parent is a toast's.
    Toast,
    /// A menu end, popup end, switch end or desktop switch, from any
    /// window.
    AnyWindow,
    /// An event of the desktop window.
    DesktopWindow,
    /// A core window equal to or under the foreground thread's active
    /// window.
    CoreWindowActive,
    /// It is in the foreground ([`AcceptanceFacts::in_foreground`]).
    InForeground,
    /// It, or its top-level window, is topmost.
    Topmost,
}

impl Accepted {
    /// Whether it was accepted by something other than the foreground
    /// relation, so that the foreground stop does not reach its entry: a
    /// value change accepted only because background progress bars are
    /// reported among them, as NVDA stops nothing at a foreground change.
    pub(crate) fn exempt_from_foreground(self) -> bool {
        match self {
            Self::NoWindow
            | Self::BackgroundProgress
            | Self::ShowClass
            | Self::Toast
            | Self::AnyWindow
            | Self::DesktopWindow
            | Self::Topmost => true,
            Self::CoreWindowActive | Self::InForeground => false,
        }
    }
}

/// Why an event was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Refused {
    /// A hide.
    Hide,
    /// A show from a class not among the six.
    ShowClass,
    /// No rule accepted it.
    Unrelated,
}

/// The test's answer.
pub(crate) type Acceptance = Result<Accepted, Refused>;

/// Whether `tell` is accepted on its own facts, with no setting: the
/// verdict of every tell but an MSAA change, a UI Automation value change
/// and a UI Automation progress bar's change, which their entry stores as
/// it is made.
pub(crate) fn accepted(tell: &Event) -> Acceptance {
    accept(tell.kind, crate::entries::window_facts(tell), false)
}

/// Whether an event of `kind`, whose tell window has `facts` (`None` when
/// it has no window), is accepted, with background progress bars reported
/// or not: a value change, or a UI Automation progress bar's change, which
/// NVDA maps to its value change event, is accepted from any window with
/// them reported.
pub(crate) fn accept(
    kind: EventKind,
    facts: Option<&AcceptanceFacts>,
    background_progress: bool,
) -> Acceptance {
    let Some(facts) = facts else {
        return Ok(Accepted::NoWindow);
    };
    match kind {
        EventKind::ValueChange | EventKind::UiaProgress if background_progress => {
            return Ok(Accepted::BackgroundProgress);
        }
        EventKind::Hide => return Err(Refused::Hide),
        EventKind::Show => {
            return if show_class(&facts.window.class) {
                Ok(Accepted::ShowClass)
            } else {
                Err(Refused::ShowClass)
            };
        }
        EventKind::Alert if facts.parent_class == TOAST_PARENT => return Ok(Accepted::Toast),
        EventKind::MenuEnd
        | EventKind::PopupEnd
        | EventKind::SwitchEnd
        | EventKind::DesktopSwitch => return Ok(Accepted::AnyWindow),
        _ => {}
    }
    if facts.desktop {
        return Ok(Accepted::DesktopWindow);
    }
    if facts.window.class.starts_with(CORE_WINDOW_PREFIX)
        && facts.window_to_active != Relation::Outside
    {
        return Ok(Accepted::CoreWindowActive);
    }
    if facts.in_foreground() {
        return Ok(Accepted::InForeground);
    }
    if facts.window_topmost || facts.root_topmost {
        return Ok(Accepted::Topmost);
    }
    Err(Refused::Unrelated)
}

#[cfg(test)]
mod tests {
    use verbatim_model::tell::WindowFacet;
    use verbatim_model::{Pid, WindowHandle};

    use super::*;

    fn facet(handle: u64, class: &str) -> WindowFacet {
        WindowFacet {
            handle: WindowHandle(handle),
            class: class.to_owned(),
            process: Some(Pid(4)),
            thread: 5,
        }
    }

    /// A background window: unrelated to the foreground, not topmost.
    fn background(class: &str) -> AcceptanceFacts {
        AcceptanceFacts {
            window: facet(20, class),
            parent_class: String::new(),
            root: facet(20, class),
            root_owner: facet(20, class),
            window_topmost: false,
            root_topmost: false,
            foreground: WindowHandle(10),
            foreground_root_owner: WindowHandle(10),
            window_to_foreground: Relation::Outside,
            root_owner_to_foreground: Relation::Outside,
            root_owner_is_foregrounds: false,
            active_window: WindowHandle(10),
            window_to_active: Relation::Outside,
            desktop: false,
            hidden_frame: false,
            hidden_frame_owned: false,
        }
    }

    #[test]
    fn an_event_with_no_window_is_accepted() {
        assert_eq!(
            accept(EventKind::MsaaFocus, None, false),
            Ok(Accepted::NoWindow)
        );
    }

    #[test]
    fn a_background_value_change_is_accepted_only_with_background_progress_bars_reported() {
        let facts = background("msctls_progress32");
        assert_eq!(
            accept(EventKind::ValueChange, Some(&facts), true),
            Ok(Accepted::BackgroundProgress)
        );
        assert_eq!(
            accept(EventKind::ValueChange, Some(&facts), false),
            Err(Refused::Unrelated)
        );
    }

    #[test]
    fn a_hide_is_refused_even_in_the_foreground() {
        let mut facts = background("Button");
        facts.window_to_foreground = Relation::Under;
        assert_eq!(
            accept(EventKind::Hide, Some(&facts), false),
            Err(Refused::Hide)
        );
    }

    #[test]
    fn a_show_is_accepted_by_its_class_alone() {
        for class in SHOW_CLASSES {
            assert_eq!(
                accept(EventKind::Show, Some(&background(class)), false),
                Ok(Accepted::ShowClass)
            );
        }
        let mut facts = background("Button");
        facts.window_to_foreground = Relation::Equal;
        assert_eq!(
            accept(EventKind::Show, Some(&facts), false),
            Err(Refused::ShowClass)
        );
    }

    #[test]
    fn an_alert_under_a_toast_window_is_accepted_and_another_background_alert_refused() {
        let mut facts = background("Windows.UI.Core.CoreWindow");
        facts.parent_class = TOAST_PARENT.to_owned();
        assert_eq!(
            accept(EventKind::Alert, Some(&facts), false),
            Ok(Accepted::Toast)
        );
        assert_eq!(
            accept(EventKind::Alert, Some(&background("#32770")), false),
            Err(Refused::Unrelated)
        );
    }

    #[test]
    fn a_menu_end_popup_end_switch_end_and_desktop_switch_are_accepted_from_any_window() {
        for kind in [
            EventKind::MenuEnd,
            EventKind::PopupEnd,
            EventKind::SwitchEnd,
            EventKind::DesktopSwitch,
        ] {
            assert_eq!(
                accept(kind, Some(&background("#32768")), false),
                Ok(Accepted::AnyWindow)
            );
        }
    }

    #[test]
    fn a_menu_start_is_tested_like_any_other_event() {
        assert_eq!(
            accept(EventKind::MenuStart, Some(&background("#32768")), false),
            Err(Refused::Unrelated)
        );
        assert_eq!(
            accept(EventKind::PopupStart, Some(&background("#32768")), false),
            Err(Refused::Unrelated)
        );
    }

    #[test]
    fn an_event_of_the_desktop_window_is_accepted() {
        let mut facts = background("#32769");
        facts.desktop = true;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Ok(Accepted::DesktopWindow)
        );
    }

    #[test]
    fn a_core_window_equal_to_or_under_the_active_window_is_accepted() {
        for relation in [Relation::Equal, Relation::Under] {
            let mut facts = background("Windows.UI.Core.CoreWindow");
            facts.window_to_active = relation;
            assert_eq!(
                accept(EventKind::MsaaFocus, Some(&facts), false),
                Ok(Accepted::CoreWindowActive)
            );
        }
    }

    #[test]
    fn a_core_window_outside_the_active_window_goes_on_to_the_later_rules() {
        let mut facts = background("Windows.UI.Core.CoreWindow");
        assert_eq!(
            accept(EventKind::UiaFocus, Some(&facts), false),
            Err(Refused::Unrelated)
        );
        facts.root_topmost = true;
        assert_eq!(
            accept(EventKind::UiaFocus, Some(&facts), false),
            Ok(Accepted::Topmost)
        );
    }

    #[test]
    fn the_active_window_rule_applies_only_to_core_windows() {
        let mut facts = background("Button");
        facts.window_to_active = Relation::Under;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Err(Refused::Unrelated)
        );
    }

    #[test]
    fn a_window_sharing_the_foregrounds_root_owner_is_accepted() {
        let mut facts = background("#32770");
        facts.root_owner_is_foregrounds = true;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Ok(Accepted::InForeground)
        );
    }

    #[test]
    fn a_window_or_its_root_owner_in_the_foreground_is_accepted() {
        for (window, root_owner) in [
            (Relation::Equal, Relation::Outside),
            (Relation::Under, Relation::Outside),
            (Relation::Outside, Relation::Equal),
            (Relation::Outside, Relation::Under),
        ] {
            let mut facts = background("Button");
            facts.window_to_foreground = window;
            facts.root_owner_to_foreground = root_owner;
            assert_eq!(
                accept(EventKind::MsaaFocus, Some(&facts), false),
                Ok(Accepted::InForeground)
            );
        }
    }

    #[test]
    fn a_topmost_window_or_root_is_accepted() {
        let mut facts = background("#32768");
        facts.window_topmost = true;
        assert_eq!(
            accept(EventKind::PopupStart, Some(&facts), false),
            Ok(Accepted::Topmost)
        );
        let mut facts = background("ComboLBox");
        facts.root_topmost = true;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Ok(Accepted::Topmost)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&background("Button")), false),
            Err(Refused::Unrelated)
        );
    }

    #[test]
    fn the_rules_decide_in_order() {
        // The desktop window comes before the foreground relation, and the
        // relation before a topmost window.
        let mut facts = background("#32769");
        facts.desktop = true;
        facts.window_to_foreground = Relation::Equal;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Ok(Accepted::DesktopWindow)
        );
        let mut facts = background("Button");
        facts.root_owner_is_foregrounds = true;
        facts.window_to_foreground = Relation::Under;
        facts.window_topmost = true;
        assert_eq!(
            accept(EventKind::MsaaFocus, Some(&facts), false),
            Ok(Accepted::InForeground)
        );
    }
}
